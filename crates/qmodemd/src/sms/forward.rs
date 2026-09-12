//! Persistent SMS forwarding jobs. No credentials or message bodies in logs.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sink {
    pub id: String,
    #[serde(default)]
    pub enabled: bool,
    pub target: Target,
    #[serde(default = "attempts")]
    pub max_attempts: u32,
}
fn attempts() -> u32 {
    5
}
impl std::fmt::Debug for Sink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sink")
            .field("id", &self.id)
            .field("enabled", &self.enabled)
            .finish()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Telegram {
        bot_token: String,
        chat_id: String,
    },
    Webhook {
        url: String,
        #[serde(default = "post")]
        method: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        format: String,
    },
    Serverchan {
        token: String,
        #[serde(default)]
        channel: String,
        #[serde(default)]
        noip: String,
        #[serde(default)]
        openid: String,
    },
    Pushdeer {
        pushkey: String,
        #[serde(default = "pushdeer")]
        endpoint: String,
    },
    Feishu {
        webhook_key: String,
    },
    Custom {
        path: String,
        #[serde(default)]
        args: Vec<String>,
    },
}
fn post() -> String {
    "POST".into()
}
fn pushdeer() -> String {
    "https://api2.pushdeer.com".into()
}
fn url_valid(url: &str) -> Result<()> {
    let u = reqwest::Url::parse(url)?;
    ensure!(
        ["http", "https"].contains(&u.scheme())
            && u.username().is_empty()
            && u.password().is_none(),
        "forwarding URL must be HTTP(S) without embedded credentials"
    );
    Ok(())
}
impl Sink {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 48
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "invalid forwarding sink id"
        );
        ensure!(
            (1..=20).contains(&self.max_attempts),
            "forwarding attempts must be 1 to 20"
        );
        match &self.target {
            Target::Webhook {
                url,
                method,
                headers,
                format,
            } => {
                url_valid(url)?;
                ensure!(
                    ["GET", "POST", "PUT"].contains(&method.as_str()),
                    "unsupported webhook method"
                );
                ensure!(
                    format.len() <= 8192 && headers.len() <= 16,
                    "webhook configuration too large"
                );
                for (k, v) in headers {
                    reqwest::header::HeaderName::from_bytes(k.as_bytes())?;
                    reqwest::header::HeaderValue::from_str(v)?;
                }
            }
            Target::Telegram { bot_token, chat_id } => {
                ensure!(
                    !bot_token.is_empty()
                        && bot_token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b":_-".contains(&b))
                        && !chat_id.is_empty(),
                    "invalid Telegram configuration"
                );
            }
            Target::Serverchan { token, .. } | Target::Feishu { webhook_key: token } => {
                ensure!(
                    !token.is_empty()
                        && token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                    "invalid forwarding key"
                );
            }
            Target::Pushdeer { pushkey, endpoint } => {
                ensure!(!pushkey.is_empty(), "PushDeer key required");
                url_valid(endpoint)?;
            }
            Target::Custom { path, args } => {
                ensure!(
                    std::path::Path::new(path).is_absolute() && args.len() <= 32,
                    "invalid forwarding executable"
                );
            }
        }
        Ok(())
    }
}
fn render(template: &str, sender: &str, time: &str, content: &str) -> String {
    // Single pass: message content containing placeholder text is never substituted again.
    let mut output = String::new();
    let mut remaining = template;
    while !remaining.is_empty() {
        if let Some((token, value)) = [
            ("{SENDER}", sender),
            ("{TIME}", time),
            ("{CONTENT}", content),
        ]
        .into_iter()
        .find(|(t, _)| remaining.starts_with(t))
        {
            output.push_str(value);
            remaining = &remaining[token.len()..];
        } else {
            let c = remaining.chars().next().unwrap();
            output.push(c);
            remaining = &remaining[c.len_utf8()..];
        }
    }
    output
}
pub async fn deliver(target: &Target, message: &Value) -> Result<()> {
    let sender = message["peer"].as_str().unwrap_or("");
    let content = message["content"].as_str().unwrap_or("");
    let time = chrono::DateTime::from_timestamp(message["timestamp"].as_i64().unwrap_or(0), 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    if let Target::Custom { path, args } = target {
        let mut cmd = tokio::process::Command::new(path);
        cmd.args(args)
            .env("SMS_SENDER", sender)
            .env("SMS_TIME", &time)
            .env("SMS_CONTENT", content)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let result = tokio::time::timeout(Duration::from_secs(30), cmd.status())
            .await
            .context("custom forwarding timed out")??;
        ensure!(result.success(), "custom forwarding failed");
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let request=match target{
  Target::Telegram{bot_token,chat_id}=>client.post(format!("https://api.telegram.org/bot{bot_token}/sendMessage")).json(&json!({"chat_id":chat_id,"text":format!("QModem SMS: ({sender})\n\n🕒 Time: {time}\n💬 Content: {content}")})),
  Target::Serverchan{token,channel,noip,openid}=>client.post(format!("https://sctapi.ftqq.com/{token}.send")).json(&json!({"title":format!("QModem SMS: ({sender})"),"desp":format!("**Time:** {time}\n\n**Sender:** {sender}\n\n**Content:**\n{content}"),"channel":channel,"noip":noip,"openid":openid})),
  Target::Pushdeer{pushkey,endpoint}=>client.post(format!("{}/message/push",endpoint.trim_end_matches('/'))).form(&[("pushkey",pushkey.as_str()),("text",&format!("QModem SMS: ({sender})\n\nTime: {time}\nContent: {content}")),("type","text")]),
  Target::Feishu{webhook_key}=>client.post(format!("https://open.feishu.cn/open-apis/bot/v2/hook/{webhook_key}")).json(&json!({"msg_type":"interactive","card":{"header":{"template":"blue","title":{"content":"💬 短信通知","tag":"plain_text"}},"elements":[{"tag":"div","text":{"tag":"plain_text","content":format!("来源号码：{sender}\n接收时间：{time}\n\n{content}")}}]}})),
  Target::Webhook{url,method,headers,format}=>{
   let payload=if format.is_empty(){format!("{sender}/{content}({time})")}else{render(format,sender,&time,content)};
   let mut parsed=reqwest::Url::parse(url)?;
   let mut request=if method=="GET"{parsed.path_segments_mut().map_err(|_|anyhow::anyhow!("invalid webhook base URL"))?.push(&payload);client.get(parsed)}else{client.request(reqwest::Method::from_bytes(method.as_bytes())?,parsed).header(reqwest::header::CONTENT_TYPE,"application/json").body(payload)};
   for(k,v)in headers{request=request.header(k,v);}request
  },Target::Custom{..}=>unreachable!(),
 };
    let key = crate::auth::digest(&format!("{}:{}", message["modem_id"], message["id"]));
    let mut response = request
        .header("Idempotency-Key", key)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("forwarding transport failed"))?;
    ensure!(
        response.status().is_success(),
        "forwarding endpoint returned HTTP {}",
        response.status().as_u16()
    );
    if matches!(target, Target::Webhook { .. }) {
        return Ok(());
    }
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("forwarding response failed"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= 65536,
            "forwarding response too large"
        );
        bytes.extend(chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).context("forwarding response was not JSON")?;
    let success = match target {
        Target::Telegram { .. } => body["ok"] == true,
        Target::Feishu { .. } => body["code"] == 0 || body["StatusCode"] == 0,
        Target::Serverchan { .. } | Target::Pushdeer { .. } => body["code"] == 0,
        _ => true,
    };
    ensure!(success, "forwarding provider rejected the request");
    Ok(())
}
pub fn enqueue(db: &mut Connection, modem: &str, sinks: &[Sink]) -> Result<()> {
    let tx = db.transaction()?;
    let known = {
        let mut stmt = tx.prepare("SELECT sink_id FROM sms_forward_start WHERE modem_id=?")?;
        stmt.query_map([modem], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in known
        .iter()
        .filter(|id| !sinks.iter().any(|s| &s.id == *id && s.enabled))
    {
        tx.execute("UPDATE sms_deliveries SET state='cancelled',claim_token=NULL,last_error='forwarding destination disabled' WHERE modem_id=? AND sink_id=? AND state IN ('pending','inflight','failed')",params![modem,id])?;
        tx.execute(
            "DELETE FROM sms_forward_start WHERE modem_id=? AND sink_id=?",
            params![modem, id],
        )?;
    }
    for sink in sinks.iter().filter(|s| s.enabled) {
        let fingerprint = crate::auth::digest(&serde_json::to_string(&sink.target)?);
        let old = tx
            .query_row(
                "SELECT fingerprint FROM sms_forward_start WHERE modem_id=? AND sink_id=?",
                params![modem, sink.id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if old.as_deref() != Some(&fingerprint) {
            tx.execute("UPDATE sms_deliveries SET state='cancelled',claim_token=NULL,last_error='forwarding destination changed' WHERE modem_id=? AND sink_id=? AND state IN ('pending','inflight','failed')",params![modem,sink.id])?;
            tx.execute("INSERT INTO sms_forward_start(modem_id,sink_id,minimum_id,fingerprint) VALUES (?,?,(SELECT COALESCE(MAX(id),0) FROM messages WHERE modem_id=?),?) ON CONFLICT(modem_id,sink_id) DO UPDATE SET minimum_id=excluded.minimum_id,fingerprint=excluded.fingerprint",params![modem,sink.id,modem,fingerprint])?;
        }
        tx.execute("INSERT OR IGNORE INTO sms_deliveries(message_id,modem_id,sink_id,state,available_at) SELECT m.id,m.modem_id,?1,'pending',?2 FROM messages m JOIN sms_forward_start s ON s.modem_id=m.modem_id AND s.sink_id=?1 WHERE m.modem_id=?3 AND m.id>s.minimum_id AND m.direction='received' AND m.delivery_status='received' AND m.content!='' AND COALESCE(json_extract(m.metadata,'$.legacy'),0)=0",params![sink.id,super::database::now(),modem])?;
    }
    tx.commit()?;
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct Claim {
    pub id: i64,
    pub token: String,
    pub sink_id: String,
    pub attempt: u32,
    pub message: Value,
}
pub fn claim(db: &mut Connection, modem: &str, sinks: &[Sink]) -> Result<Option<Claim>> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = super::database::now();
    tx.execute("UPDATE sms_deliveries SET state='pending',claim_token=NULL WHERE state='inflight' AND available_at<=?",[now])?;
    for sink in sinks.iter().filter(|s| s.enabled) {
        tx.execute("UPDATE sms_deliveries SET state='failed',last_error='attempt limit reached after interrupted delivery' WHERE modem_id=? AND sink_id=? AND state='pending' AND attempts>=?",params![modem,sink.id,sink.max_attempts])?;
        let found=tx.query_row("SELECT id,message_id,attempts FROM sms_deliveries WHERE modem_id=? AND sink_id=? AND state='pending' AND available_at<=? AND attempts<? ORDER BY id LIMIT 1",params![modem,sink.id,now,sink.max_attempts],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,u32>(2)?))).optional()?;
        if let Some((id, message_id, attempt)) = found {
            let mut bytes = [0; 16];
            getrandom::fill(&mut bytes)
                .map_err(|_| anyhow::anyhow!("claim token generation failed"))?;
            let token = super::pdu::hex(&bytes);
            tx.execute("UPDATE sms_deliveries SET state='inflight',claim_token=?,available_at=?,attempts=attempts+1 WHERE id=?",params![token,now+120,id])?;
            let message = super::database::get(&tx, modem, message_id)?
                .context("delivery message disappeared")?;
            tx.commit()?;
            return Ok(Some(Claim {
                id,
                token,
                sink_id: sink.id.clone(),
                attempt: attempt + 1,
                message,
            }));
        }
    }
    tx.commit()?;
    Ok(None)
}
pub fn complete(db: &Connection, claim: &Claim, success: bool, max_attempts: u32) -> Result<()> {
    let state = if success {
        "delivered"
    } else if claim.attempt >= max_attempts {
        "failed"
    } else {
        "pending"
    };
    let changed=db.execute("UPDATE sms_deliveries SET state=?,claim_token=NULL,available_at=?,last_error=? WHERE id=? AND state='inflight' AND claim_token=?",params![state,super::database::now()+(5i64*2i64.pow(claim.attempt.min(10))).min(3600),if success{None}else{Some("forwarding failed")},claim.id,claim.token])?;
    ensure!(
        changed == 1,
        "delivery claim expired or was already completed"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_never_reinterpret_message_text() {
        assert_eq!(
            render("{SENDER}: {CONTENT}", "10086", "now", "literal {SENDER}"),
            "10086: literal {SENDER}"
        );
    }
    #[test]
    fn claim_completion_is_exclusive_and_token_bound() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::storage::initialize(&dir.path().join("db")).unwrap();
        let sink = Sink {
            id: "hook".into(),
            enabled: true,
            target: Target::Webhook {
                url: "http://localhost/hook".into(),
                method: "POST".into(),
                headers: Default::default(),
                format: String::new(),
            },
            max_attempts: 2,
        };
        enqueue(&mut db, "m", std::slice::from_ref(&sink)).unwrap();
        db.execute("INSERT INTO messages(modem_id,direction,peer,content,timestamp,delivery_status) VALUES ('m','received','10086','hello',1,'received')",[]).unwrap();
        enqueue(&mut db, "m", std::slice::from_ref(&sink)).unwrap();
        let claim1 = claim(&mut db, "m", std::slice::from_ref(&sink))
            .unwrap()
            .unwrap();
        assert!(claim(&mut db, "m", &[sink]).unwrap().is_none());
        complete(&db, &claim1, true, 2).unwrap();
        assert!(complete(&db, &claim1, true, 2).is_err());
    }
    #[test]
    fn expired_last_attempt_fails_and_changed_destinations_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = crate::storage::initialize(&dir.path().join("db")).unwrap();
        let mut sink = Sink {
            id: "hook".into(),
            enabled: true,
            max_attempts: 1,
            target: Target::Webhook {
                url: "http://localhost/a".into(),
                method: "POST".into(),
                headers: Default::default(),
                format: String::new(),
            },
        };
        enqueue(&mut db, "m", std::slice::from_ref(&sink)).unwrap();
        db.execute("INSERT INTO messages(modem_id,direction,peer,content,timestamp,delivery_status) VALUES ('m','received','10086','hello',1,'received')",[]).unwrap();
        enqueue(&mut db, "m", std::slice::from_ref(&sink)).unwrap();
        let first = claim(&mut db, "m", std::slice::from_ref(&sink))
            .unwrap()
            .unwrap();
        db.execute(
            "UPDATE sms_deliveries SET available_at=0 WHERE id=?",
            [first.id],
        )
        .unwrap();
        assert!(
            claim(&mut db, "m", std::slice::from_ref(&sink))
                .unwrap()
                .is_none()
        );
        let state: String = db
            .query_row(
                "SELECT state FROM sms_deliveries WHERE id=?",
                [first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
        if let Target::Webhook { url, .. } = &mut sink.target {
            *url = "http://localhost/b".into()
        }
        enqueue(&mut db, "m", std::slice::from_ref(&sink)).unwrap();
        let state: String = db
            .query_row(
                "SELECT state FROM sms_deliveries WHERE id=?",
                [first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "cancelled");
        assert!(complete(&db, &first, true, 1).is_err());
        assert!(claim(&mut db, "m", &[sink]).unwrap().is_none());
    }
    #[tokio::test]
    async fn webhook_preserves_message_text_and_checks_http_status() {
        use axum::{
            Router,
            extract::State,
            http::{HeaderMap, StatusCode},
            routing::post,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, String)>(2);
        let app = Router::new()
            .route(
                "/hook",
                post(
                    |State(tx): State<tokio::sync::mpsc::Sender<(String, String)>>,
                     headers: HeaderMap,
                     body: String| async move {
                        tx.send((headers["idempotency-key"].to_str().unwrap().into(), body))
                            .await
                            .unwrap();
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route("/reject", post(|| async { StatusCode::BAD_GATEWAY }))
            .with_state(tx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut target = Target::Webhook {
            url: format!("http://{address}/hook"),
            method: "POST".into(),
            headers: Default::default(),
            format: "{SENDER}:{CONTENT}".into(),
        };
        let message = json!({"id":1,"modem_id":"m","peer":"10086","content":"原样 {TIME} & \"test\"","timestamp":1});
        deliver(&target, &message).await.unwrap();
        let (key, body) = rx.recv().await.unwrap();
        assert_eq!(key.len(), 64);
        assert_eq!(body, "10086:原样 {TIME} & \"test\"");
        if let Target::Webhook { url, .. } = &mut target {
            *url = format!("http://{address}/reject")
        }
        assert!(deliver(&target, &message).await.is_err());
        server.abort();
    }
}
