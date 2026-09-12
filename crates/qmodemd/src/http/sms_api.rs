use super::*;
use crate::sms::{self, database};
use axum::extract::Query;
use std::path::PathBuf;
fn db_path(state: &AppState) -> PathBuf {
    state.config().storage.sqlite.into()
}
fn failure(error: anyhow::Error) -> ApiError {
    ApiError::new(StatusCode::BAD_GATEWAY, "sms_failed", error.to_string())
}
fn modem_id(state: &AppState, id: &str) -> Result<(), ApiError> {
    if state.config().modems.iter().any(|m| m.id == id) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "modem_not_found",
            "Configured modem not found",
        ))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    peer: Option<String>,
    before: Option<i64>,
    #[serde(default = "page_limit")]
    limit: u32,
}
fn page_limit() -> u32 {
    50
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    memory: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    is_read: bool,
}
async fn list(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    if !(1..=200).contains(&page.limit) {
        return Err(ApiError::invalid("limit must be 1 through 200"));
    }
    database::run(db_path(&state), move |db| {
        database::list(db, &id, page.peer.as_deref(), page.before, page.limit)
    })
    .await
    .map(success)
    .map_err(failure)
}
async fn conversations(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    database::run(db_path(&state), move |db| database::conversations(db, &id))
        .await
        .map(success)
        .map_err(failure)
}
async fn get_message(
    State(state): State<Shared>,
    Path((id, message)): Path<(String, i64)>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    database::run(db_path(&state), move |db| database::get(db, &id, message))
        .await
        .map_err(failure)?
        .map(success)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "message_not_found",
                "Message not found",
            )
        })
}
async fn mark(
    State(state): State<Shared>,
    Path((id, message)): Path<(String, i64)>,
    input: Result<Json<Mark>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    let read = body(input)?.is_read;
    let updated = database::run(db_path(&state), move |db| {
        Ok(db.execute(
            "UPDATE messages SET is_read=? WHERE modem_id=? AND id=?",
            rusqlite::params![read, id, message],
        )?)
    })
    .await
    .map_err(failure)?;
    if updated == 0 {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "message_not_found",
            "Message not found",
        ));
    }
    Ok(success(json!({"updated":true})))
}
async fn delete(
    State(state): State<Shared>,
    Path((id, message)): Path<(String, i64)>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    let deleted=database::run(db_path(&state),move|db|{
  let sending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE modem_id=? AND id=? AND delivery_status='sending')",rusqlite::params![id,message],|r|r.get(0))?;
  anyhow::ensure!(!sending,"cannot delete an in-flight SMS");
  Ok(db.execute("DELETE FROM messages WHERE modem_id=? AND id=?",rusqlite::params![id,message])?)
 }).await.map_err(failure)?;
    Ok(success(json!({"deleted":deleted,"scope":"history"})))
}
async fn sync(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<Memory>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let memory = body(input)?.memory;
    sms::validate_memory(&memory).map_err(|e| ApiError::invalid(e.to_string()))?;
    sms::sync(modem, state.ports.clone(), db_path(&state), memory)
        .await
        .map(success)
        .map_err(failure)
}
async fn send(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<sms::Send>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let request = body(input)?;
    sms::pdu::encode(&request.peer, &request.content, 0)
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    sms::send(modem, state.ports.clone(), db_path(&state), request)
        .await
        .map(success)
        .map_err(failure)
}
async fn sim_list(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Query(memory): Query<Memory>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let (messages, errors) = sms::read(&modem, &state.ports, &memory.memory)
        .await
        .map_err(failure)?;
    Ok(success(
        json!({"items":messages.iter().map(|m|json!({"index":m.index,"pdu":m.pdu,"decoded":m.decoded})).collect::<Vec<_>>(),"errors":errors}),
    ))
}
async fn storage(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let port = state
        .ports
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await
        .map_err(failure)?;
    let replies = port
        .run_named(
            Box::new(crate::at::Sequence::new(
                vec![Step::command("AT+CPMS?", Duration::from_secs(10)).unwrap()],
                false,
            )),
            Some(id),
            "sms_storage",
        )
        .await?;
    let memories = replies[0]
        .response
        .lines()
        .find_map(|l| l.trim().strip_prefix("+CPMS:"))
        .map(vendor::cells::fields);
    Ok(success(
        json!({"success":replies[0].modem_success,"memories":memories,"replies":replies}),
    ))
}
async fn set_storage(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<Memory>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let memory = body(input)?.memory;
    sms::validate_memory(&memory).map_err(|e| ApiError::invalid(e.to_string()))?;
    let port = state
        .ports
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await
        .map_err(failure)?;
    let step = Step::command(
        &format!("AT+CPMS=\"{memory}\",\"{memory}\",\"{memory}\""),
        Duration::from_secs(10),
    )
    .unwrap();
    let replies = port
        .run_named(
            Box::new(crate::at::Sequence::new(vec![step], false)),
            Some(id),
            "sms_storage_set",
        )
        .await?;
    if !replies[0].modem_success {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "modem_rejected",
            "SMS storage change rejected",
        ));
    }
    Ok(success(json!({"saved":true,"replies":replies})))
}
pub(super) fn routes() -> Router<Shared> {
    Router::new()
        .route(
            "/api/v1/modems/{id}/sms/import",
            post(import_legacy).layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/api/v1/modems/{id}/sms/deliveries", get(deliveries))
        .route(
            "/api/v1/modems/{id}/sms/deliveries/{delivery}/retry",
            post(retry_delivery),
        )
        .route(
            "/api/v1/modems/{id}/sms/config",
            get(settings).put(configure),
        )
        .route("/api/v1/modems/{id}/sms", get(list))
        .route("/api/v1/modems/{id}/sms/conversations", get(conversations))
        .route(
            "/api/v1/modems/{id}/sms/{message}",
            get(get_message).patch(mark).delete(delete),
        )
        .route("/api/v1/modems/{id}/sms/sync", post(sync))
        .route("/api/v1/modems/{id}/sms/send", post(send))
        .route("/api/v1/modems/{id}/sms/send-pdu", post(send_raw))
        .route(
            "/api/v1/modems/{id}/sms/sim",
            get(sim_list).delete(delete_sim),
        )
        .route(
            "/api/v1/modems/{id}/sms/storage",
            get(storage).put(set_storage),
        )
}

async fn settings(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let runtime = state.sms_status.lock().await.get(&id).cloned();
    Ok(success(json!({"config":modem.sms,"runtime":runtime})))
}
async fn configure(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<sms::Settings>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut modem = configured_modem(&state, &id)?;
    modem.sms = body(input)?;
    modem
        .sms
        .validate()
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let sinks = modem.sms.forwarding.clone();
    let modem_id = id.clone();
    database::run(db_path(&state), move |db| {
        sms::forward::enqueue(db, &modem_id, &sinks)
    })
    .await
    .map_err(failure)?;
    persist_modem(&state, id, Some(modem)).await?;
    Ok(success(json!({"saved":true})))
}

async fn delete_sim(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<sms::DeleteSim>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    sms::delete_sim(&modem, &state.ports, body(input)?)
        .await
        .map(success)
        .map_err(failure)
}

async fn deliveries(
    State(state): State<Shared>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    let items=database::run(db_path(&state),move|db|{let mut stmt=db.prepare("SELECT id,message_id,sink_id,state,attempts,available_at,last_error FROM sms_deliveries WHERE modem_id=? ORDER BY id DESC LIMIT 200")?;
 Ok(stmt.query_map([id],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"message_id":r.get::<_,i64>(1)?,"sink_id":r.get::<_,String>(2)?,"state":r.get::<_,String>(3)?,"attempts":r.get::<_,u32>(4)?,"available_at":r.get::<_,i64>(5)?,"last_error":r.get::<_,Option<String>>(6)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)}).await.map_err(failure)?;
    Ok(success(json!({"items":items})))
}
async fn retry_delivery(
    State(state): State<Shared>,
    Path((id, delivery)): Path<(String, i64)>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    let changed=database::run(db_path(&state),move|db|Ok(db.execute("UPDATE sms_deliveries SET state='pending',attempts=0,available_at=?,last_error=NULL WHERE id=? AND modem_id=? AND state='failed'",rusqlite::params![database::now(),delivery,id])?)).await.map_err(failure)?;
    Ok(success(json!({"retried":changed==1})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyImport {
    source: String,
    document: Value,
}
async fn import_legacy(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<LegacyImport>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    modem_id(&state, &id)?;
    let request = body(input)?;
    database::run(db_path(&state), move |db| {
        sms::legacy::import(db, &id, &request.source, &request.document)
    })
    .await
    .map(success)
    .map_err(failure)
}

async fn send_raw(
    State(state): State<Shared>,
    Path(id): Path<String>,
    input: Result<Json<sms::RawSend>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let modem = configured_modem(&state, &id)?;
    let request = body(input)?;
    sms::pdu::decode(&request.pdu).map_err(|e| ApiError::invalid(e.to_string()))?;
    sms::send_raw(modem, state.ports.clone(), db_path(&state), request)
        .await
        .map(success)
        .map_err(failure)
}
