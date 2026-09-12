use crate::{
    at::{AtError, ErrorKind, PortPool, Step},
    auth,
    config::Config,
    listener, vendor,
};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State, rejection::JsonRejection},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct AppState {
    config: Config,
    ports: PortPool,
    runtime: vendor::Runtime,
}
type Shared = Arc<AppState>;
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn service_info(cfg: &Config) -> Value {
    json!({"listen":cfg.server.listen.to_string(),"port":cfg.server.port,"interface":cfg.server.interface,
        "log_level":cfg.logging.level,"log_format":cfg.logging.format,
        "auth_configured":!cfg.auth.token_hash.is_empty(),"stage":"development"})
}
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<Value>,
}
impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", message)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut error = json!({"code":self.code,"message":self.message});
        if let Some(details) = self.details {
            error["details"] = details;
        }
        (self.status, Json(json!({"error":error}))).into_response()
    }
}
impl From<AtError> for ApiError {
    fn from(e: AtError) -> Self {
        let (status, code) = match e.kind {
            ErrorKind::State => (StatusCode::INTERNAL_SERVER_ERROR, "runtime_state_failed"),
            ErrorKind::Timeout => (StatusCode::GATEWAY_TIMEOUT, "at_timeout"),
            ErrorKind::QueueFull => (StatusCode::TOO_MANY_REQUESTS, "at_queue_full"),
            ErrorKind::Unsynchronized => (StatusCode::CONFLICT, "at_unsynchronized"),
            ErrorKind::Io | ErrorKind::Closed => {
                (StatusCode::SERVICE_UNAVAILABLE, "serial_unavailable")
            }
            ErrorKind::Overflow => (StatusCode::BAD_GATEWAY, "at_response_too_large"),
        };
        Self::new(status, code, e.message)
    }
}
fn success(value: Value) -> Json<Value> {
    Json(json!({"data":value}))
}
fn body<T>(input: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    input
        .map(|Json(v)| v)
        .map_err(|e| ApiError::new(e.status(), "invalid_request", e.body_text()))
}
async fn health() -> Json<Value> {
    Json(
        json!({"service":"qmodemd","version":env!("CARGO_PKG_VERSION"),"stage":"development","modem_api_ready":false,"native_at_ready":true}),
    )
}
async fn service(State(state): State<Shared>) -> Json<Value> {
    success(service_info(&state.config))
}
async fn devices() -> Result<Json<Value>, ApiError> {
    listener::interfaces()
        .map(|v| success(json!({"interfaces":v})))
        .map_err(|e| {
            tracing::error!(error=%e,"could not list network interfaces");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "interface_discovery_failed",
                "Could not read network interfaces",
            )
        })
}
fn configured_modem<'a>(
    state: &'a AppState,
    id: &str,
) -> Result<&'a crate::config::Modem, ApiError> {
    let modem = state
        .config
        .modems
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "modem_not_found",
                "Configured modem not found",
            )
        })?;
    if !modem.enabled {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "modem_disabled",
            "Modem is disabled",
        ));
    }
    Ok(modem)
}
async fn modems(State(state): State<Shared>) -> Json<Value> {
    success(
        json!({"items":state.config.modems.iter().map(|m|json!({"id":m.id,"name":m.name,"manufacturer":m.manufacturer,"model":m.model,"platform":m.platform,"bus":m.bus,"enabled":m.enabled})).collect::<Vec<_>>() }),
    )
}
async fn capabilities(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let mut operations = vec![
        "get_imei",
        "set_imei",
        "get_sim_slot",
        "get_sim_capabilities",
        "set_sim_slot",
        "get_mode",
        "set_mode",
        "get_network_prefer",
        "set_network_prefer",
        "soft_reboot",
    ];
    if modem.manufacturer.eq_ignore_ascii_case("quectel") {
        operations.extend(["get_5g_lan", "set_5g_lan", "get_band_lock", "set_band_lock"]);
    }
    Ok(success(
        json!({"operations":operations,"coverage":"partial","hardware_verified":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtRequest {
    command: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
fn default_timeout() -> u64 {
    10000
}
async fn send_at(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<AtRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let request = body(input)?;
    let modem = configured_modem(&state, &id)?;
    let step = Step::command(&request.command, Duration::from_millis(request.timeout_ms))
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let port = state.ports.get(&modem.at_port).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "serial_unavailable",
            "Could not open the configured AT port",
        )
    })?;
    let replies = port
        .run_named(
            Box::new(crate::at::Sequence::new(vec![step], false)),
            Some(modem.id.clone()),
            "raw_at",
        )
        .await?;
    Ok(success(json!({"replies":replies})))
}
async fn action(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<vendor::Operation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let operation = body(input)?;
    let modem = configured_modem(&state, &id)?;
    if let Some(data) = vendor::local(modem, &operation, &state.runtime).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_state_failed",
            "Could not read software SIM state",
        )
    })? {
        return Ok(success(data));
    }
    let program = vendor::plan(modem, &operation, &state.runtime)
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let port = state.ports.get(&modem.at_port).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "serial_unavailable",
            "Could not open the configured AT port",
        )
    })?;
    let replies = port
        .run_named(program, Some(modem.id.clone()), operation.name())
        .await?;
    let mut data = vendor::finish(modem, &operation, &replies).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "invalid_modem_response",
            e.to_string(),
        )
    })?;
    if data["success"] != true {
        let (code, message) = match data["error_code"].as_str() {
            Some("sim_switch_unconfirmed") => (
                "sim_switch_unconfirmed",
                "SIM slot did not reach the requested value after five reads",
            ),
            Some("imei_unconfirmed") => (
                "imei_unconfirmed",
                "IMEI readback did not match the requested value",
            ),
            Some("invalid_modem_response") => {
                ("invalid_modem_response", "No usable query response")
            }
            _ => ("modem_rejected", "The modem rejected the operation"),
        };
        let mut error = ApiError::new(StatusCode::BAD_GATEWAY, code, message);
        error.details = Some(data);
        return Err(error);
    }
    if let Some(object) = data.as_object_mut() {
        object.remove("error_code");
    }
    Ok(success(data))
}

async fn events(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let port = state.ports.get(&modem.at_port).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "serial_unavailable",
            "Could not open the configured AT port",
        )
    })?;
    let stream = futures_util::stream::unfold(port.subscribe(), |mut receiver| async move {
        let event = match receiver.recv().await {
            Ok(event) => Event::default()
                .event("serial")
                .json_data(event)
                .expect("serialize serial event"),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => Event::default()
                .event("gap")
                .json_data(json!({"dropped":dropped}))
                .expect("serialize gap"),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        };
        Some((Ok(event), receiver))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
async fn queues(State(state): State<Shared>) -> Json<Value> {
    let mut modems = Vec::new();
    for modem in &state.config.modems {
        let mut paths = vec![(&modem.at_port, vec!["at"])];
        if let Some(sms) = &modem.sms_at_port {
            if sms == &modem.at_port {
                paths[0].1.push("sms");
            } else {
                paths.push((sms, vec!["sms"]));
            }
        }
        let mut ports = Vec::new();
        for (path, roles) in paths {
            let snapshot = state.ports.inspect(path).await;
            ports.push(match snapshot {
                Some((canonical,queue))=>json!({"path":path,"roles":roles,"canonical_path":canonical,"opened":true,"queue":queue}),
                None=>json!({"path":path,"roles":roles,"canonical_path":null,"opened":false,"queue":null}),
            });
        }
        modems.push(json!({"id":modem.id,"name":modem.name,"enabled":modem.enabled,"manufacturer":modem.manufacturer,"model":modem.model,"bus":modem.bus,"ports":ports}));
    }
    success(json!({"modems":modems,"history_limit_per_port":32,"payloads_included":false}))
}
async fn catalog() -> Json<Value> {
    success(
        serde_json::from_str(include_str!("../../../data/supported-models.json"))
            .expect("validated bundled modem catalog"),
    )
}
async fn authenticate(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    if !token.is_some_and(|s| auth::authorized(&state.config.auth.token_hash, s)) {
        let mut response = ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A valid access token is required",
        )
        .into_response();
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Bearer"),
        );
        return response;
    }
    next.run(req).await
}
async fn observe(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let request_id = REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let start = Instant::now();
    let mut response = next.run(req).await;
    let status = response.status().as_u16();
    let elapsed_ms = start.elapsed().as_millis() as u64;
    if status >= 500 {
        tracing::error!(%method,status,elapsed_ms,request_id,"HTTP request failed");
    } else if status >= 400 {
        tracing::warn!(%method,status,elapsed_ms,request_id,"HTTP request rejected");
    } else {
        tracing::debug!(%method,status,elapsed_ms,request_id,"HTTP request completed");
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "x-request-id",
        header::HeaderValue::from_str(&request_id.to_string()).unwrap(),
    );
    response
}
async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "API route not found")
}
async fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "HTTP method not allowed",
    )
}
pub fn router(cfg: Config) -> Router {
    let state = Arc::new(AppState {
        runtime: vendor::Runtime::new(&cfg.storage.runtime_dir),
        config: cfg,
        ports: PortPool::default(),
    });
    let api = Router::new()
        .route("/api/v1/system/service", get(service))
        .route("/api/v1/system/interfaces", get(devices))
        .route("/api/v1/modems", get(modems))
        .route("/api/v1/modems/{id}/capabilities", get(capabilities))
        .route("/api/v1/modems/{id}/at", post(send_at))
        .route("/api/v1/modems/{id}/actions", post(action))
        .route("/api/v1/modems/{id}/events", get(events))
        .route("/api/v1/catalog", get(catalog))
        .route("/api/v1/queues", get(queues))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/", get(crate::web::index))
        .route("/queues", get(crate::web::index))
        .route("/licenses", get(crate::web::licenses))
        .route("/api/health", get(health))
        .merge(api)
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn(observe))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    fn app() -> Router {
        let mut cfg = Config::parse(include_str!("../../../config/qmodem.example.toml")).unwrap();
        cfg.auth.token_hash = auth::digest("test-token");
        router(cfg)
    }
    #[tokio::test]
    async fn auth_and_envelopes_are_consistent() {
        let app = app();
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/service")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
        let r = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/service")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert!(r.headers().contains_key("x-request-id"));
        let bytes = axum::body::to_bytes(r.into_body(), 65536).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("\"data\""));
        assert!(!text.contains("token_hash"));
        assert!(!text.contains("test-token"));
        let r = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
        let body: Value =
            serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 65536).await.unwrap())
                .unwrap();
        assert_eq!(body["error"]["code"], "not_found");
    }
    #[tokio::test]
    async fn malformed_action_cannot_open_a_device() {
        let r = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/modems/m1/actions")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{\"operation\":\"invented\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 422);
        let body: Value =
            serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 65536).await.unwrap())
                .unwrap();
        assert_eq!(body["error"]["code"], "invalid_request");
    }
}

#[cfg(test)]
mod native_api_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn http_to_native_pty_and_vendor_parser_work_end_to_end() {
        let pty = nix::pty::openpty(None, None).unwrap();
        let path = nix::unistd::ttyname(&pty.slave).unwrap();
        drop(pty.slave);
        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let simulator = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut master = std::fs::File::from(pty.master);
            // PTY master returns EIO until its slave is opened by the HTTP request.
            for (expected, response) in [
                (b"AT\r\n".as_slice(), b"\r\nOK\r\n".as_slice()),
                (
                    b"AT+QCFG=\"usbnet\"\r\n".as_slice(),
                    b"\r\n+QCFG: \"usbnet\",2\r\nOK\r\n".as_slice(),
                ),
            ] {
                let mut actual = vec![0; expected.len()];
                let mut attempts = 0;
                loop {
                    match master.read_exact(&mut actual) {
                        Ok(()) => break,
                        Err(e) if e.raw_os_error() == Some(5) && attempts < 100 => {
                            attempts += 1;
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("read pseudo modem: {e}"),
                    }
                }
                assert_eq!(actual, expected);
                master.write_all(response).unwrap();
            }
            close_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
        let mut cfg = Config::parse(include_str!("../../../config/qmodem.example.toml")).unwrap();
        cfg.auth.token_hash = auth::digest("test-token");
        cfg.modems.push(serde_json::from_value(json!({"id":"m1","name":"PTY modem","manufacturer":"quectel","platform":"qualcomm","bus":"usb","at_port":path})).unwrap());
        let app = router(cfg);
        for (path, input) in [
            ("at", json!({"command":"AT"})),
            ("actions", json!({"operation":"get_mode"})),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/v1/modems/m1/{path}"))
                        .header(header::AUTHORIZATION, "Bearer test-token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(input.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let value: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 65536)
                    .await
                    .unwrap(),
            )
            .unwrap();
            if path == "actions" {
                assert_eq!(value["data"]["data"]["mode"], "mbim");
            } else {
                assert_eq!(value["data"]["replies"][0]["terminal"], "OK");
            }
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/queues")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let snapshot: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        let queue = &snapshot["data"]["modems"][0]["ports"][0]["queue"];
        assert_eq!(queue["state"], "idle");
        assert_eq!(queue["completed"], 2);
        assert_eq!(queue["recent"][0]["operation"], "get_mode");
        assert_eq!(queue["recent"][0]["modem_id"], "m1");
        assert!(queue["recent"][0].get("response").is_none());
        close_tx.send(()).unwrap();
        simulator.join().unwrap();
    }
}

#[cfg(test)]
mod queue_api_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn queue_inspection_requires_auth_and_never_opens_configured_ports() {
        let mut cfg = Config::parse(include_str!("../../../config/qmodem.example.toml")).unwrap();
        cfg.auth.token_hash = auth::digest("test-token");
        cfg.modems.push(serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":"quectel","platform":"qualcomm","bus":"usb","at_port":"/dev/does-not-exist","sms_at_port":"/dev/does-not-exist"})).unwrap());
        let app = router(cfg);
        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/queues")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), 401);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/queues")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        let ports = body["data"]["modems"][0]["ports"].as_array().unwrap();
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0]["roles"], json!(["at", "sms"]));
        assert_eq!(ports[0]["opened"], false);
        assert!(ports[0]["queue"].is_null());
    }
    #[tokio::test]
    async fn mt5700_software_reads_work_without_opening_a_serial_port() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::parse(include_str!("../../../config/qmodem.example.toml")).unwrap();
        cfg.storage.runtime_dir = dir.path().join("runtime").to_str().unwrap().into();
        cfg.auth.token_hash = auth::digest("test-token");
        cfg.modems.push(serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":"tdtech","model":"mt5700m-cn","platform":"hisilicon","bus":"usb","at_port":"/dev/does-not-exist"})).unwrap());
        let app = router(cfg);
        for operation in ["get_sim_slot", "get_sim_capabilities"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/modems/m1/actions")
                        .header(header::AUTHORIZATION, "Bearer test-token")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(json!({"operation":operation}).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 65536)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body["data"]["data"]["source"], "software");
            assert_eq!(body["data"]["data"]["hardware_verified"], false);
        }
    }
}
