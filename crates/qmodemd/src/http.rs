use anyhow::Context;
mod sms_api;
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
    config: std::sync::RwLock<Config>,
    config_path: Option<std::path::PathBuf>,
    config_writer: tokio::sync::Mutex<()>,
    discovery_writer: tokio::sync::Mutex<()>,
    ports: PortPool,
    status: crate::status::Cache,
    network: Arc<crate::network::Manager>,
    startup_status: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Value>>>,
    maintenance_status: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Value>>>,
    sms_status: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Value>>>,
    runtime: vendor::Runtime,
}
impl AppState {
    fn config(&self) -> Config {
        self.config
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
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
    success(service_info(&state.config()))
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
fn configured_modem(state: &AppState, id: &str) -> Result<crate::config::Modem, ApiError> {
    let modem = state
        .config()
        .modems
        .iter()
        .find(|m| m.id == id)
        .cloned()
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
        json!({"items":state.config().modems.iter().map(|m|json!({"id":m.id,"name":m.name,"manufacturer":m.manufacturer,"model":m.model,"platform":m.platform,"bus":m.bus,"enabled":m.enabled})).collect::<Vec<_>>() }),
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
        "get_usage_stats",
    ];
    if modem.manufacturer.eq_ignore_ascii_case("quectel") {
        operations.extend([
            "get_5g_lan",
            "set_5g_lan",
            "get_band_lock",
            "set_band_lock",
            "write_usage_stats",
            "clear_usage_stats",
        ]);
        if ["qualcomm", "lte12", "lte", "unisoc"].contains(&modem.platform.as_str()) {
            operations.extend(["get_neighborcell", "set_cell_lock", "unlock_cell"]);
        }
    }
    Ok(success(
        json!({"operations":operations,"coverage":"partial","hardware_verified":false}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AtRequest {
    command: String,
    #[serde(default)]
    port: AtPort,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum AtPort {
    #[default]
    Primary,
    Sms,
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
    let port = state
        .ports
        .get(match request.port {
            AtPort::Primary => &modem.at_port,
            AtPort::Sms => modem.sms_at_port.as_deref().unwrap_or(&modem.at_port),
        })
        .await
        .map_err(|_| {
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
    if matches!(operation, vendor::Operation::SetSimSlot { .. }) {
        return switch_sim_workflow(state, id, operation).await;
    }
    let modem = configured_modem(&state, &id)?;
    if let Some(data) = vendor::local(&modem, &operation, &state.runtime).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_state_failed",
            "Could not read software SIM state",
        )
    })? {
        return Ok(success(data));
    }
    let program = vendor::plan(&modem, &operation, &state.runtime)
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let port = state.ports.get(&modem.at_port).await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "serial_unavailable",
            "Could not open the configured AT port",
        )
    })?;
    state.status.invalidate().await;
    let replies = port
        .run_named(program, Some(modem.id.clone()), operation.name())
        .await?;
    state.status.invalidate().await;
    let mut data = vendor::finish(&modem, &operation, &replies).map_err(|e| {
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
            Ok(event) if event.correlation == "closed" => return None,
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
    for modem in &state.config().modems {
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
    if !token.is_some_and(|s| auth::authorized(&state.config().auth.token_hash, s)) {
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
#[cfg(test)]
pub fn router(cfg: Config) -> Router {
    router_with_path(cfg, None)
}
pub fn router_with_path(cfg: Config, path: Option<std::path::PathBuf>) -> Router {
    let state = Arc::new(AppState {
        runtime: vendor::Runtime::new(&cfg.storage.runtime_dir),
        config: std::sync::RwLock::new(cfg),
        config_path: path,
        config_writer: tokio::sync::Mutex::new(()),
        discovery_writer: tokio::sync::Mutex::new(()),
        ports: PortPool::default(),
        status: crate::status::Cache::default(),
        network: Default::default(),
        startup_status: Default::default(),
        maintenance_status: Default::default(),
        sms_status: Default::default(),
    });
    if state.config_path.is_some() && state.config().discovery.enabled {
        start_discovery(Arc::downgrade(&state));
    }
    if state.config_path.is_some() {
        start_initialization(Arc::downgrade(&state));
        start_leds(Arc::downgrade(&state));
        start_forwarding(Arc::downgrade(&state));
        start_maintenance_workers(Arc::downgrade(&state));
        start_network_workers(Arc::downgrade(&state));
        start_sms_workers(Arc::downgrade(&state));
    }
    let api = Router::new()
        .merge(sms_api::routes())
        .route("/api/v1/system/service", get(service))
        .route("/api/v1/system/interfaces", get(devices))
        .route("/api/v1/modems", get(modems))
        .route(
            "/api/v1/modems/{id}/config",
            get(modem_settings).put(save_modem).delete(delete_modem),
        )
        .route("/api/v1/modems/{id}/ports/close", post(close_port))
        .route("/api/v1/discovery", get(discover))
        .route("/api/v1/discovery/{id}/probe", post(probe_device))
        .route("/api/v1/discovery/{id}/bind", post(bind_device))
        .route("/api/v1/modems/{id}/capabilities", get(capabilities))
        .route("/api/v1/modems/{id}/status", get(modem_status))
        .route(
            "/api/v1/modems/{id}/startup",
            get(startup_state).put(save_startup),
        )
        .route(
            "/api/v1/modems/{id}/cell-lock",
            axum::routing::put(save_cell_lock),
        )
        .route("/api/v1/modems/{id}/reboot", post(hard_reboot))
        .route(
            "/api/v1/modems/{id}/maintenance",
            get(maintenance).put(configure_maintenance),
        )
        .route("/api/v1/modems/{id}/debug/config", get(debug_config))
        .route(
            "/api/v1/modems/{id}/logs",
            get(modem_logs).delete(clear_modem_logs),
        )
        .route("/api/v1/modems/{id}/hardware", get(hardware))
        .route("/api/v1/modems/{id}/traffic/history", get(traffic_history))
        .route("/api/v1/modems/{id}/network", post(network_operation))
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

async fn modem_settings(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = state
        .config()
        .modems
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "modem_not_found",
                "Configured modem not found",
            )
        })?;
    Ok(success(json!(modem)))
}
async fn persist_modem(
    state: &Shared,
    id: String,
    modem: Option<crate::config::Modem>,
) -> Result<(), ApiError> {
    let _writer = state.config_writer.lock().await;
    let mut config = state.config();
    config.modems.retain(|m| m.id != id);
    if let Some(modem) = &modem {
        config.modems.push(modem.clone());
    }
    config
        .validate()
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    if let Some(path) = state.config_path.clone() {
        let modems =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<crate::config::Modem>> {
                crate::config::update(&path, |document| {
                    let mut disk = Config::parse(&document.to_string())?;
                    disk.modems.retain(|m| m.id != id);
                    if let Some(modem) = modem {
                        disk.modems.push(modem);
                    }
                    disk.validate()?;
                    let encoded = toml::to_string(&disk)?.parse::<toml_edit::DocumentMut>()?;
                    document["modems"] = encoded["modems"].clone();
                    Ok(())
                })?;
                Ok(Config::load(&path)?.modems)
            })
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_write_failed",
                    "Configuration worker stopped",
                )
            })?
            .map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_write_failed",
                    e.to_string(),
                )
            })?;
        config.modems = modems;
    }
    if config.modems.iter().any(|m| !m.sms.forwarding.is_empty())
        || state
            .config()
            .modems
            .iter()
            .any(|m| !m.sms.forwarding.is_empty())
    {
        let forwarding_modems = config.modems.clone();
        crate::sms::database::run(config.storage.sqlite.clone().into(), move |db| {
            for m in forwarding_modems {
                let mut sinks = m.sms.forwarding;
                if !m.enabled {
                    for sink in &mut sinks {
                        sink.enabled = false;
                    }
                }
                crate::sms::forward::enqueue(db, &m.id, &sinks)?;
            }
            Ok(())
        })
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "forwarding_config_failed",
                "Could not update forwarding configuration",
            )
        })?;
    }
    state
        .config
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .modems = config.modems;
    Ok(())
}
async fn save_modem(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<crate::config::Modem>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut modem = body(input)?;
    if modem.sms_at_port.as_deref() == Some("") {
        modem.sms_at_port = None;
    }
    if modem.interface.as_deref() == Some("") {
        modem.interface = None;
    }
    if modem.id != id {
        return Err(ApiError::invalid("URL id must match modem id"));
    }
    persist_modem(&state, id, Some(modem)).await?;
    Ok(success(json!({"saved":true,"restart_required":false})))
}
async fn delete_modem(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    persist_modem(&state, id, None).await?;
    Ok(success(json!({"deleted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosePort {
    role: String,
}
async fn close_port(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<ClosePort>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let role = body(input)?.role;
    let modem = configured_modem(&state, &id)?;
    let path = match role.as_str() {
        "at" => &modem.at_port,
        "sms" => modem.sms_at_port.as_ref().unwrap_or(&modem.at_port),
        _ => return Err(ApiError::invalid("role must be at or sms")),
    };
    state
        .ports
        .close(path)
        .await
        .map_err(|e| ApiError::new(StatusCode::CONFLICT, "port_busy", e.to_string()))?;
    Ok(success(
        json!({"closed":true,"reopens_on_next_request":true}),
    ))
}
async fn inventory() -> Result<Vec<crate::discovery::Device>, ApiError> {
    tokio::task::spawn_blocking(|| crate::discovery::scan(std::path::Path::new("/sys")))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "discovery_failed",
                "Scanner stopped",
            )
        })?
        .map_err(|e| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "discovery_failed",
                e.to_string(),
            )
        })
}
async fn discover() -> Result<Json<Value>, ApiError> {
    Ok(success(json!({"items":inventory().await?,"probed":false})))
}
async fn probe_device(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = state.discovery_writer.try_lock().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "discovery_busy",
            "Device discovery is already running",
        )
    })?;
    let device = inventory()
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "device_not_found",
                "Device disappeared or is outside the supported scope",
            )
        })?;
    Ok(success(json!(
        crate::discovery::probe(device, &state.ports).await
    )))
}
async fn bind_device(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _guard = state.discovery_writer.try_lock().map_err(|_| {
        ApiError::new(
            StatusCode::CONFLICT,
            "discovery_busy",
            "Device discovery is already running",
        )
    })?;
    let device = inventory()
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "device_not_found",
                "Device disappeared",
            )
        })?;
    tokio::task::spawn_blocking(move || {
        crate::discovery::bind_option(&device, std::path::Path::new("/sys"))
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "driver_bind_failed",
            "Driver worker stopped",
        )
    })?
    .map_err(|e| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "driver_bind_failed",
            e.to_string(),
        )
    })?;
    Ok(success(json!({"bound":true,"rescan_required":true})))
}

async fn modem_status(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    state
        .status
        .get(&modem, &state.ports)
        .await
        .map(success)
        .map_err(|e| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "status_unavailable",
                e.to_string(),
            )
        })
}

#[cfg(test)]
mod modem_config_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn config_crud_updates_live_state_preserves_auth_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = Config::parse(include_str!("../../../config/qmodem.example.toml")).unwrap();
        cfg.auth.token_hash = auth::digest("test-token");
        std::fs::write(&path, toml::to_string(&cfg).unwrap()).unwrap();
        let app = router_with_path(cfg, Some(path.clone()));
        let modem = json!({"id":"m1","name":"test","manufacturer":"quectel","platform":"qualcomm","bus":"usb","at_port":"/dev/no-modem"});
        let request = |method: &str, uri: &str, data: Value| {
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, "Bearer test-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(data.to_string()))
                .unwrap()
        };
        let saved = app
            .clone()
            .oneshot(request("PUT", "/api/v1/modems/m1/config", modem.clone()))
            .await
            .unwrap();
        assert_eq!(saved.status(), 200);
        assert_eq!(Config::load(&path).unwrap().modems.len(), 1);
        let read = app
            .clone()
            .oneshot(request("GET", "/api/v1/modems/m1/config", json!(null)))
            .await
            .unwrap();
        assert_eq!(read.status(), 200);
        let mut invalid = modem;
        invalid["manufacturer"] = json!("fibocom");
        assert_eq!(
            app.clone()
                .oneshot(request("PUT", "/api/v1/modems/m1/config", invalid))
                .await
                .unwrap()
                .status(),
            422
        );
        assert_eq!(
            Config::load(&path).unwrap().modems[0].manufacturer,
            "quectel"
        );
        assert_eq!(
            app.oneshot(request("DELETE", "/api/v1/modems/m1/config", json!(null)))
                .await
                .unwrap()
                .status(),
            200
        );
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.modems.is_empty());
        assert_eq!(cfg.auth.token_hash, auth::digest("test-token"));
    }
}

fn start_discovery(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut seen = std::collections::HashMap::<String, (String, std::time::Instant)>::new();
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let settings = state.config().discovery;
            if !settings.enabled {
                break;
            }
            if let Ok(_guard) = state.discovery_writer.try_lock() {
                match inventory().await {
                    Ok(devices) => {
                        seen.retain(|id, _| devices.iter().any(|d| &d.id == id));
                        for device in devices {
                            let signature = format!(
                                "{}:{}:{:?}:{:?}",
                                device.vendor_id,
                                device.product_id,
                                device.serial,
                                device.at_candidates
                            );
                            if seen.get(&device.id).is_some_and(|(old, at)| {
                                old == &signature && at.elapsed() < Duration::from_secs(60)
                            }) {
                                continue;
                            }
                            seen.insert(device.id.clone(), (signature, std::time::Instant::now()));
                            let existing = state.config().modems.into_iter().find(|m| {
                                m.id == device.id || device.at_candidates.contains(&m.at_port)
                            });
                            if existing.as_ref().is_some_and(|m| !m.enabled) {
                                continue;
                            }
                            if settings.bind_option_driver
                                && device.needs_option_binding
                                && device.at_candidates.is_empty()
                            {
                                let device = device.clone();
                                let result = tokio::task::spawn_blocking(move || {
                                    crate::discovery::bind_option(
                                        &device,
                                        std::path::Path::new("/sys"),
                                    )
                                })
                                .await;
                                if !matches!(result, Ok(Ok(()))) {
                                    tracing::warn!("automatic option driver binding failed");
                                }
                                continue;
                            }
                            if !settings.auto_register || device.at_candidates.is_empty() {
                                continue;
                            }
                            if existing.as_ref().is_some_and(|m| {
                                device.at_candidates.contains(&m.at_port)
                                    && std::path::Path::new(&m.at_port).exists()
                            }) {
                                continue;
                            }
                            let identified = crate::discovery::probe(device, &state.ports).await;
                            if let Some(mut modem) = identified.modem {
                                if let Some(old) = existing {
                                    if old.manufacturer != modem.manufacturer
                                        || old.model != modem.model
                                    {
                                        tracing::warn!(modem_id=%old.id,"replacement modem has different identity; keeping existing configuration");
                                        continue;
                                    }
                                    modem.id = old.id;
                                    modem.name = old.name;
                                    modem.apn = old.apn;
                                    modem.pdp_index = old.pdp_index;
                                    modem.bands = old.bands;
                                    modem.sms = old.sms;
                                    modem.network = old.network;
                                    modem.monitor = old.monitor;
                                    modem.traffic = old.traffic;
                                    modem.startup = old.startup;
                                }
                                let id = modem.id.clone();
                                match persist_modem(&state, id.clone(), Some(modem)).await {
                                    Ok(()) => {
                                        tracing::info!(modem_id=%id,"discovered modem configuration saved")
                                    }
                                    Err(_) => {
                                        seen.remove(&id);
                                        tracing::error!(modem_id=%id,"could not save discovered modem");
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => tracing::warn!("device inventory unavailable"),
                }
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(settings.interval_seconds)).await;
        }
    });
}

fn start_sms_workers(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut workers =
            std::collections::HashMap::<String, (String, tokio::task::JoinHandle<()>)>::new();
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let cfg = state.config();
            let active = cfg
                .modems
                .iter()
                .filter(|m| {
                    m.enabled
                        && matches!(m.sms.mode, crate::sms::Mode::Poll | crate::sms::Mode::Urc)
                })
                .collect::<Vec<_>>();
            workers.retain(|id, (_, handle)| {
                let keep = active.iter().any(|m| &m.id == id);
                if !keep {
                    handle.abort();
                }
                keep
            });
            for modem in active {
                let signature = serde_json::to_string(modem).expect("serializable modem");
                if workers
                    .get(&modem.id)
                    .is_some_and(|(old, handle)| old == &signature && !handle.is_finished())
                {
                    continue;
                }
                if let Some((_, handle)) = workers.remove(&modem.id) {
                    handle.abort();
                }
                let modem = modem.clone();
                let id = modem.id.clone();
                let pool = state.ports.clone();
                let db = std::path::PathBuf::from(&cfg.storage.sqlite);
                let statuses = state.sms_status.clone();
                let handle = tokio::spawn(async move {
                    let mut receiver = None;
                    let mut prefix = String::new();
                    loop {
                        let attempt = async {
                            if modem.sms.mode == crate::sms::Mode::Urc && receiver.is_none() {
                                let (rx, rule) = crate::sms::setup_urc(&modem, &pool).await?;
                                receiver = Some(rx);
                                prefix = rule;
                            }
                            crate::sms::sync(
                                modem.clone(),
                                pool.clone(),
                                db.clone(),
                                modem.sms.memories[0].clone(),
                            )
                            .await
                        }
                        .await;
                        let mut snapshot = match attempt {
                            Ok(data) => {
                                json!({"state":"ready","last_sync_at":crate::sms::database::now(),"result":data})
                            }
                            Err(error) => {
                                receiver = None;
                                json!({"state":"degraded","last_attempt_at":crate::sms::database::now(),"error":error.to_string()})
                            }
                        };
                        snapshot["mode"] = json!(modem.sms.mode);
                        statuses.lock().await.insert(modem.id.clone(), snapshot);
                        if let Some(rx) = receiver.as_mut() {
                            loop {
                                match rx.recv().await {
                                    Ok(event) if event.correlation == "closed" => {
                                        receiver = None;
                                        break;
                                    }
                                    Ok(event)
                                        if event.correlation == "unsolicited"
                                            && event.line.starts_with(&prefix) =>
                                    {
                                        break;
                                    }
                                    Ok(_) => {}
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                        break;
                                    }
                                    Err(_) => {
                                        receiver = None;
                                        break;
                                    }
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(
                                modem.sms.poll_interval_seconds,
                            ))
                            .await;
                        }
                    }
                });
                workers.insert(id, (signature, handle));
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        for (_, (_, handle)) in workers {
            handle.abort();
        }
    });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkOperation {
    operation: String,
}
async fn network_operation(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<NetworkOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let operation = body(input)?.operation;
    let ports = state.ports.clone();
    let manager = state.network.clone();
    let runtime = state.runtime.clone();
    let value =
        tokio::spawn(async move { manager.operate(modem, ports, runtime, &operation).await })
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "network_worker_failed",
                    "Network worker stopped",
                )
            })?
            .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "network_failed", e.to_string()))?;
    Ok(success(value))
}

async fn switch_sim_workflow(
    state: Shared,
    id: String,
    operation: vendor::Operation,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let program = vendor::plan(&modem, &operation, &state.runtime)
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let task = tokio::spawn(async move {
        let lock = state.network.lock(&id).await;
        let _guard = lock.lock().await;
        if let vendor::Operation::SetSimSlot { slot } = &operation
            && vendor::family(&modem).map_err(|e| ApiError::invalid(e.to_string()))?
                == vendor::Family::TdtechMt5700
        {
            // Upstream persists this software state even when opening the AT port fails.
            state.runtime.set(&id, *slot).map_err(|e| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "runtime_state_failed",
                    e.to_string(),
                )
            })?;
        }
        let port = state.ports.get(&modem.at_port).await.map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "serial_unavailable",
                "Could not open the configured AT port",
            )
        })?;
        let replies = port.run_named(program, Some(id), "set_sim_slot").await?;
        state.status.invalidate().await;
        let mut result = vendor::finish(&modem, &operation, &replies).map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "invalid_modem_response",
                e.to_string(),
            )
        })?;
        if result["success"] != true {
            let mut error = ApiError::new(
                StatusCode::BAD_GATEWAY,
                "sim_switch_unconfirmed",
                "SIM switch did not complete",
            );
            error.details = Some(result);
            return Err(error);
        }
        match state
            .network
            .operate_locked(modem, state.ports.clone(), state.runtime.clone(), "redial")
            .await
        {
            Ok(network) => {
                result["network"] = network;
                Ok(success(result))
            }
            Err(error) => {
                result["success"] = json!(false);
                result["sim_switched"] = json!(true);
                result["redial_error"] = json!(error.to_string());
                let mut failure = ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "sim_redial_failed",
                    "SIM switched, but network redial failed",
                );
                failure.details = Some(result);
                Err(failure)
            }
        }
    });
    task.await.map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "sim_worker_failed",
            "SIM worker stopped",
        )
    })?
}
fn start_network_workers(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut attempted = std::collections::HashMap::<String, String>::new();
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let cfg = state.config();
            for modem in cfg
                .modems
                .iter()
                .filter(|m| m.enabled && m.network.auto_connect)
            {
                if !crate::lifecycle::is_ready(
                    modem,
                    state.startup_status.lock().await.get(&modem.id),
                ) {
                    continue;
                }
                let key = serde_json::to_string(modem).expect("serializable modem");
                if attempted.get(&modem.id) == Some(&key) {
                    continue;
                }
                if !std::path::Path::new(&modem.at_port).exists() {
                    continue;
                }
                attempted.insert(modem.id.clone(), key);
                let manager = state.network.clone();
                let ports = state.ports.clone();
                let runtime = state.runtime.clone();
                let modem = modem.clone();
                tokio::spawn(async move {
                    if let Err(error) = manager
                        .operate(modem.clone(), ports, runtime, "connect")
                        .await
                    {
                        tracing::warn!(modem_id=%modem.id,error=%error,"automatic connection failed");
                    }
                });
            }
            attempted.retain(|id, _| {
                cfg.modems.iter().any(|m| {
                    &m.id == id
                        && m.enabled
                        && m.network.auto_connect
                        && std::path::Path::new(&m.at_port).exists()
                })
            });
            drop(state);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn monitor_switch(state: Shared, modem: &crate::config::Modem) -> anyhow::Result<()> {
    let runtime = &state.runtime;
    let local = vendor::local(modem, &vendor::Operation::GetSimSlot, runtime)?;
    let (current, slots) = if let Some(local) = local {
        (local["data"]["sim_slot"].as_u64(), vec![0, 1])
    } else {
        let port = state.ports.get(&modem.at_port).await?;
        let mut values = Vec::new();
        for op in [
            vendor::Operation::GetSimCapabilities,
            vendor::Operation::GetSimSlot,
        ] {
            let replies = port
                .run_named(
                    vendor::plan(modem, &op, runtime)?,
                    Some(modem.id.clone()),
                    "watchdog_sim_query",
                )
                .await?;
            values.push(vendor::finish(modem, &op, &replies)?);
        }
        (
            values[1]["data"]["sim_slot"].as_u64(),
            values[0]["data"]["slots"]
                .as_array()
                .context("SIM slot capabilities unavailable")?
                .iter()
                .filter_map(Value::as_u64)
                .collect(),
        )
    };
    let current = current.context("SIM slot is unknown")?;
    let next = slots
        .into_iter()
        .find(|s| *s != current)
        .context("no alternative SIM slot")?;
    let _ = switch_sim_workflow(
        state,
        modem.id.clone(),
        vendor::Operation::SetSimSlot { slot: next as u8 },
    )
    .await
    .map_err(|e| anyhow::anyhow!(e.message))?;
    Ok(())
}
fn start_maintenance_workers(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut workers =
            std::collections::HashMap::<String, (String, tokio::task::JoinHandle<()>)>::new();
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let cfg = state.config();
            let active = cfg
                .modems
                .iter()
                .filter(|m| {
                    m.enabled && (m.monitor.enabled || m.traffic.enabled || m.traffic.reset.enabled)
                })
                .collect::<Vec<_>>();
            workers.retain(|id, (_, handle)| {
                let keep = active.iter().any(|m| &m.id == id);
                if !keep {
                    handle.abort();
                }
                keep
            });
            for modem in active {
                let signature = serde_json::to_string(modem).expect("serializable modem");
                if workers
                    .get(&modem.id)
                    .is_some_and(|(old, handle)| old == &signature && !handle.is_finished())
                {
                    continue;
                }
                if let Some((_, handle)) = workers.remove(&modem.id) {
                    handle.abort();
                }
                let modem = modem.clone();
                let id = modem.id.clone();
                let pool = state.ports.clone();
                let manager = state.network.clone();
                let runtime = state.runtime.clone();
                let statuses = state.maintenance_status.clone();
                let path = std::path::PathBuf::from(&cfg.storage.sqlite);
                let weak = weak.clone();
                let handle = tokio::spawn(async move {
                    let mut count = crate::monitor::Counter::new(&modem.monitor);
                    let mut last_traffic = None;
                    let mut last_monitor = None;
                    let mut snapshot = json!({});
                    loop {
                        let now = Instant::now();
                        match crate::schedule::tick(
                            modem.clone(),
                            pool.clone(),
                            path.clone(),
                            runtime.clone(),
                        )
                        .await
                        {
                            Ok(Some(result)) => snapshot["traffic_reset"] = result,
                            Ok(None) => {}
                            Err(_) => {
                                snapshot["traffic_reset"] =
                                    json!({"error":"scheduled reset failed"})
                            }
                        }
                        if modem.traffic.enabled
                            && last_traffic.is_none_or(|t: Instant| {
                                t.elapsed() >= Duration::from_secs(modem.traffic.interval_seconds)
                            })
                        {
                            last_traffic = Some(now);
                            snapshot["traffic"] = match crate::monitor::traffic(
                                modem.clone(),
                                pool.clone(),
                                path.clone(),
                                runtime.clone(),
                            )
                            .await
                            {
                                Ok(data) => {
                                    json!({"sampled_at":crate::sms::database::now(),"data":data})
                                }
                                Err(_) => json!({"error":"traffic sample failed"}),
                            };
                        }
                        if modem.monitor.enabled
                            && last_monitor.is_none_or(|t: Instant| {
                                t.elapsed() >= Duration::from_secs(modem.monitor.interval_seconds)
                            })
                        {
                            last_monitor = Some(now);
                            let (ready, healthy) = crate::monitor::check(
                                &modem,
                                &manager,
                                pool.clone(),
                                runtime.clone(),
                            )
                            .await
                            .unwrap_or((false, false));
                            let (trigger, data) =
                                count.observe(ready, healthy, &modem.monitor, now);
                            snapshot["watchdog"] = data;
                            if trigger {
                                let mut results = vec![];
                                for action in &modem.monitor.actions {
                                    let result = match action {
                                        crate::monitor::Action::Redial => manager
                                            .operate(
                                                modem.clone(),
                                                pool.clone(),
                                                runtime.clone(),
                                                "redial",
                                            )
                                            .await
                                            .map(|_| ()),
                                        crate::monitor::Action::SwitchSim => {
                                            if let Some(state) = weak.upgrade() {
                                                monitor_switch(state, &modem).await
                                            } else {
                                                break;
                                            }
                                        }
                                        crate::monitor::Action::At { commands } => {
                                            crate::monitor::send_at(&modem, &pool, commands).await
                                        }
                                        crate::monitor::Action::Exec { path, args } => {
                                            crate::monitor::execute(path, args).await
                                        }
                                    };
                                    results.push(json!({"success":result.is_ok(),"error":result.err().map(|e|e.to_string())}));
                                }
                                snapshot["watchdog"]["actions"] = json!(results);
                                tracing::warn!(modem_id=%modem.id,"watchdog threshold reached");
                            }
                        }
                        statuses
                            .lock()
                            .await
                            .insert(modem.id.clone(), snapshot.clone());
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                });
                workers.insert(id, (signature, handle));
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        for (_, (_, handle)) in workers {
            handle.abort();
        }
    });
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceConfig {
    monitor: crate::monitor::Settings,
    traffic: crate::monitor::TrafficSettings,
}
async fn maintenance(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    Ok(success(
        json!({"monitor":modem.monitor,"traffic":modem.traffic,"runtime":state.maintenance_status.lock().await.get(&id)}),
    ))
}
async fn configure_maintenance(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<MaintenanceConfig>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut modem = configured_modem(&state, &id)?;
    let settings = body(input)?;
    modem.monitor = settings.monitor;
    modem.traffic = settings.traffic;
    persist_modem(&state, id, Some(modem)).await?;
    Ok(success(json!({"saved":true})))
}
async fn traffic_history(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    configured_modem(&state, &id)?;
    let path = state.config().storage.sqlite.into();
    let items=crate::sms::database::run(path,move|db|{let mut stmt=db.prepare("SELECT timestamp,rx_bytes,tx_bytes,source FROM traffic WHERE modem_id=? ORDER BY timestamp DESC LIMIT 1000")?;Ok(stmt.query_map([id],|r|Ok(json!({"timestamp":r.get::<_,i64>(0)?,"rx_bytes":r.get::<_,i64>(1)?,"tx_bytes":r.get::<_,i64>(2)?,"source":r.get::<_,String>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)}).await.map_err(|e|ApiError::new(StatusCode::INTERNAL_SERVER_ERROR,"history_failed",e.to_string()))?;
    Ok(success(json!({"items":items})))
}

fn start_forwarding(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let cfg = state.config();
            for modem in cfg
                .modems
                .iter()
                .filter(|m| m.enabled && m.sms.forwarding.iter().any(|s| s.enabled))
            {
                let id = modem.id.clone();
                let sinks = modem.sms.forwarding.clone();
                let path = std::path::PathBuf::from(&cfg.storage.sqlite);
                let next = crate::sms::database::run(path.clone(), move |db| {
                    crate::sms::forward::enqueue(db, &id, &sinks)?;
                    crate::sms::forward::claim(db, &id, &sinks)
                })
                .await;
                if let Ok(Some(claim)) = next
                    && let Some(sink) = modem
                        .sms
                        .forwarding
                        .iter()
                        .find(|s| s.id == claim.sink_id && s.enabled)
                {
                    let success = crate::sms::forward::deliver(&sink.target, &claim.message)
                        .await
                        .is_ok();
                    let max = sink.max_attempts;
                    let job = claim.id;
                    if crate::sms::database::run(path, move |db| {
                        crate::sms::forward::complete(db, &claim, success, max)
                    })
                    .await
                    .is_err()
                    {
                        tracing::error!(job_id = job, "could not record SMS forwarding outcome");
                    } else if !success {
                        tracing::warn!(job_id = job, "SMS forwarding failed");
                    }
                }
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

fn start_initialization(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        let mut known = std::collections::HashMap::<String, String>::new();
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let cfg = state.config();
            known.retain(|id, _| {
                cfg.modems
                    .iter()
                    .any(|m| &m.id == id && m.enabled && std::path::Path::new(&m.at_port).exists())
            });
            for modem in cfg.modems.iter().filter(|m| m.enabled) {
                if !std::path::Path::new(&modem.at_port).exists() {
                    continue;
                }
                let key = crate::lifecycle::signature(modem);
                if known.get(&modem.id) == Some(&key) {
                    continue;
                }
                known.insert(modem.id.clone(), key.clone());
                state
                    .startup_status
                    .lock()
                    .await
                    .insert(modem.id.clone(), json!({"state":"pending"}));
                let result = crate::lifecycle::initialize(modem, &state.ports).await;
                let mut status = match result {
                    Ok(v) => v,
                    Err(_) => {
                        json!({"state":"failed","error":"modem initialization failed"})
                    }
                };
                status["signature"] = json!(key);
                if status["state"] == "failed" {
                    known.remove(&modem.id);
                }
                state
                    .startup_status
                    .lock()
                    .await
                    .insert(modem.id.clone(), status);
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}
async fn startup_state(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    Ok(success(
        json!({"config":modem.startup,"runtime":state.startup_status.lock().await.get(&id)}),
    ))
}
async fn hard_reboot(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    crate::lifecycle::hard_reboot(&modem, &state.ports, &state.runtime)
        .await
        .map(success)
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, "reboot_failed", e.to_string()))
}

fn start_leds(weak: std::sync::Weak<AppState>) {
    tokio::spawn(async move {
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            for modem in state.config().modems.iter().filter(|m| {
                m.enabled && (m.startup.sim_led.is_some() || m.startup.network_led.is_some())
            }) {
                if modem.startup.sim_led.is_some() {
                    let ready = async {
                        let replies = state
                            .ports
                            .get(&modem.at_port)
                            .await?
                            .run_named(
                                Box::new(crate::at::Sequence::new(
                                    vec![Step::command("AT+CPIN?", Duration::from_secs(3))?],
                                    false,
                                )),
                                Some(modem.id.clone()),
                                "sim_led",
                            )
                            .await?;
                        Ok::<_, anyhow::Error>(replies.last().is_some_and(|r| {
                            r.modem_success
                                && r.response.lines().any(|l| l.trim() == "+CPIN: READY")
                        }))
                    }
                    .await
                    .unwrap_or(false);
                    if crate::lifecycle::led(modem.startup.sim_led.as_deref(), ready).is_err() {
                        tracing::warn!(modem_id=%modem.id,"could not update SIM LED");
                    }
                }
                if modem.startup.network_led.is_some() {
                    let ready = state
                        .network
                        .operate(
                            modem.clone(),
                            state.ports.clone(),
                            state.runtime.clone(),
                            "status",
                        )
                        .await
                        .is_ok_and(|s| s["up"] == true);
                    if crate::lifecycle::led(modem.startup.network_led.as_deref(), ready).is_err() {
                        tracing::warn!(modem_id=%modem.id,"could not update network LED");
                    }
                }
            }
            drop(state);
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

async fn save_startup(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<crate::lifecycle::Settings>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut modem = configured_modem(&state, &id)?;
    modem.startup = body(input)?;
    persist_modem(&state, id, Some(modem)).await?;
    Ok(success(json!({"saved":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CellLockSettings {
    lock: Option<vendor::cells::Lock>,
    persist: bool,
}
async fn save_cell_lock(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<CellLockSettings>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let input = body(input)?;
    tokio::spawn(async move {
        let operation = match &input.lock {
            Some(lock) => vendor::Operation::SetCellLock { lock: lock.clone() },
            None => vendor::Operation::UnlockCell,
        };
        let result = action(State(state.clone()), Path(id.clone()), Ok(Json(operation))).await?;
        if input.persist {
            let mut modem = configured_modem(&state, &id)?;
            modem.startup.cell_lock = input.lock;
            if let Err(mut error) = persist_modem(&state, id, Some(modem)).await {
                error.details = Some(json!({"applied":true,"persisted":false}));
                return Err(error);
            }
        }
        Ok(result)
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cell_lock_worker_failed",
            "Cell lock worker stopped",
        )
    })?
}

#[derive(Deserialize)]
struct DebugLanguage {
    lang: Option<String>,
}
async fn debug_config(
    State(state): State<Shared>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<DebugLanguage>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let data: Value = serde_json::from_str(if q.lang.as_deref() == Some("en") {
        include_str!("../../../data/at_commands_en.json")
    } else {
        include_str!("../../../data/at_commands_zh.json")
    })
    .expect("bundled AT catalogue");
    let vendor = if modem.manufacturer.eq_ignore_ascii_case("quectel") {
        "quectel"
    } else {
        "huawei"
    };
    Ok(success(
        json!({"general":data["general"],"vendor":data[vendor][&modem.platform],"ports":{"primary":modem.at_port,"sms":modem.sms_at_port}}),
    ))
}
async fn modem_logs(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    configured_modem(&state, &id)?;
    Ok(success(crate::logging::read(Some(&id))))
}
async fn clear_modem_logs(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    configured_modem(&state, &id)?;
    crate::logging::clear(Some(&id));
    Ok(success(json!({"cleared":true,"scope":"service_memory"})))
}
async fn hardware(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let mut leds=std::fs::read_dir("/sys/class/leds").into_iter().flatten().filter_map(|e|e.ok()).map(|e|json!({"name":e.file_name().to_string_lossy(),"path":e.path().join("brightness").to_string_lossy()})).collect::<Vec<_>>();
    leds.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(success(
        json!({"leds":leds,"reboot":{"soft":true,"gpio_configured":modem.startup.gpio_value_path.is_some(),"gpio_available":modem.startup.gpio_value_path.as_ref().is_some_and(|p|std::path::Path::new(p).exists()),"fallback":"soft_reboot"}}),
    ))
}
