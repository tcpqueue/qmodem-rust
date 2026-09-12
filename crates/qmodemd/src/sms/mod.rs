pub mod database;
pub mod pdu;
use crate::{
    at::{AtError, Next, PortPool, Program, Reply, Sequence, Step},
    config::Modem,
};
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
fn command(c: &str) -> Step {
    Step::command(c, Duration::from_secs(30)).expect("validated SMS command")
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub request_id: String,
    pub peer: String,
    pub content: String,
}
#[derive(Debug, Clone)]
pub struct Message {
    pub index: i64,
    pub pdu: String,
    pub decoded: pdu::Decoded,
}
fn parse_list(replies: &[Reply]) -> Result<(Vec<Message>, Vec<String>)> {
    ensure!(
        replies.iter().all(|r| r.modem_success),
        "modem rejected SMS listing"
    );
    let mut messages = vec![];
    let mut errors = vec![];
    let mut index = None;
    for line in replies
        .last()
        .map(|r| r.response.as_str())
        .unwrap_or("")
        .lines()
        .map(str::trim)
    {
        if let Some(header) = line.strip_prefix("+CMGL:") {
            index = header
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .filter(|n| *n >= 0);
            continue;
        }
        if let Some(i) = index.take() {
            if line.is_empty() {
                index = Some(i);
                continue;
            }
            match pdu::decode(line) {
                Ok(decoded) => messages.push(Message {
                    index: i,
                    pdu: line.to_ascii_uppercase(),
                    decoded,
                }),
                Err(_) => errors.push(format!("SMS index {i}: invalid or unsupported PDU")),
            }
        }
    }
    if let Some(i) = index {
        errors.push(format!("SMS index {i}: missing PDU"));
    }
    Ok((messages, errors))
}
pub fn validate_memory(memory: &str) -> Result<()> {
    ensure!(
        ["SM", "ME", "MT"].contains(&memory),
        "SMS memory must be SM, ME or MT"
    );
    Ok(())
}
pub async fn read(
    modem: &Modem,
    pool: &PortPool,
    memory: &str,
) -> Result<(Vec<Message>, Vec<String>)> {
    validate_memory(memory)?;
    let port = pool
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await?;
    let steps = vec![
        command(&format!(
            "AT+CPMS=\"{memory}\",\"{}\",\"{}\"",
            modem.sms.memories[1], modem.sms.memories[2]
        )),
        command("AT+CMGF=0"),
        command("AT+CMGL=4"),
    ];
    let replies = port
        .run_named(
            Box::new(Sequence::new(steps, false)),
            Some(modem.id.clone()),
            "sms_sync",
        )
        .await?;
    parse_list(&replies)
}
pub async fn sync(modem: Modem, pool: PortPool, path: PathBuf, memory: String) -> Result<Value> {
    let (messages, errors) = read(&modem, &pool, &memory).await?;
    let total = messages.len();
    let imported = database::run(path, move |db| {
        let mut imported = 0;
        for message in messages {
            if database::import(db, &modem.id, message.index, &message.pdu, &message.decoded)? {
                imported += 1;
            }
        }
        Ok(imported)
    })
    .await?;
    Ok(json!({"listed":total,"imported":imported,"errors":errors,"partial":!errors.is_empty()}))
}
pub async fn send(modem: Modem, pool: PortPool, path: PathBuf, request: Send) -> Result<Value> {
    ensure!(
        !request.request_id.is_empty()
            && request.request_id.len() <= 64
            && request
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "request_id must be 1 to 64 letters, digits, hyphens or underscores"
    );
    let mut seed = [0];
    getrandom::fill(&mut seed)
        .map_err(|e| anyhow::anyhow!("SMS reference generation failed: {e}"))?;
    let parts = pdu::encode(&request.peer, &request.content, seed[0])?;
    let part_count = parts.len();
    // The owned task persists its outcome even when an HTTP client leaves.
    tokio::spawn(async move {
  let cfg=modem.clone();let data=request.clone();let (id,fresh)=database::run(path.clone(),move|db|database::begin_send(db,&cfg.id,&data.request_id,&data.peer,&data.content)).await?;
  if fresh {
   let attempt=async {
    let port=pool.get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port)).await?;
    let replies=port.run_named(Box::new(Submit::new(parts)),Some(modem.id.clone()),"sms_send").await?;
    let submitted=replies.iter().filter_map(|r|r.response.lines().find_map(|l|l.trim().strip_prefix("+CMGS:")).map(str::trim)).collect::<Vec<_>>();
    Ok::<_,anyhow::Error>((replies.iter().all(|r|r.modem_success) && submitted.len()==part_count,json!({"submitted_parts":submitted.len(),"references":submitted})))
   }.await;
   let (outcome,details)=match attempt {Ok((true,details))=>("submitted",details),Ok((false,details))=>("failed",details),Err(_)=>("unknown",json!({"reason":"Transport failed; submission may have reached the modem. Do not automatically resend."}))};
   database::run(path.clone(),move|db|database::finish_send(db,id,outcome,&details)).await?;
  }
  database::run(path,move|db|Ok(database::get(db,&modem.id,id)?.expect("recorded send"))).await
 }).await?
}
struct Submit {
    steps: std::collections::VecDeque<Step>,
}
impl Submit {
    fn new(parts: Vec<pdu::Encoded>) -> Self {
        let mut steps = std::collections::VecDeque::new();
        for part in parts {
            steps.push_back(command("AT+CMGF=0"));
            let mut prompt = command(&format!("AT+CMGS={}", part.tpdu_length));
            prompt.flags = vec![
                ">".into(),
                "ERROR".into(),
                "+CMS ERROR:".into(),
                "+CME ERROR:".into(),
            ];
            steps.push_back(prompt);
            let mut payload = part.pdu.into_bytes();
            payload.push(0x1a);
            let mut step = command("AT");
            step.bytes = payload;
            step.timeout = Duration::from_secs(120);
            steps.push_back(step);
        }
        Self { steps }
    }
}
impl Program for Submit {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        if replies.last().is_some_and(|r| !r.modem_success) {
            return Ok(Next::Finish);
        }
        Ok(self.steps.pop_front().map_or(Next::Finish, Next::Command))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cmgl_decode_errors_are_reported_without_losing_valid_messages() {
        let pdu = pdu::encode("10086", "hello", 0).unwrap().remove(0).pdu;
        let replies = vec![Reply {
            status: 0,
            terminal: "OK".into(),
            modem_success: true,
            response: format!("+CMGL: 1,0,,20\r\n{pdu}\r\n+CMGL: 2,0,,20\r\nBAD\r\nOK\r\n"),
        }];
        let (messages, errors) = parse_list(&replies).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].decoded.content, "hello");
        assert_eq!(errors.len(), 1);
    }
    #[test]
    fn submit_stops_before_payload_when_prompt_is_rejected() {
        let mut program = Submit::new(pdu::encode("10086", "hello", 0).unwrap());
        let mut replies = vec![];
        assert!(matches!(program.next(&replies).unwrap(), Next::Command(_)));
        replies.push(Reply {
            status: 0,
            terminal: "OK".into(),
            modem_success: true,
            response: "OK\r\n".into(),
        });
        let Next::Command(step) = program.next(&replies).unwrap() else {
            panic!()
        };
        assert!(step.bytes.starts_with(b"AT+CMGS="));
        replies.push(Reply {
            status: 0,
            terminal: "+CMS ERROR:".into(),
            modem_success: false,
            response: "+CMS ERROR: 500\r\n".into(),
        });
        assert!(matches!(program.next(&replies).unwrap(), Next::Finish));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sms_sends_through_native_pty_and_records_idempotent_result() {
        let pty = nix::pty::openpty(None, None).unwrap();
        let port = nix::unistd::ttyname(&pty.slave).unwrap();
        drop(pty.slave);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let emulator = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut master = std::fs::File::from(pty.master);
            let mut read_until = |terminal: u8| {
                let mut line = Vec::new();
                let mut one = [0];
                for _ in 0..10000 {
                    match master.read_exact(&mut one) {
                        Ok(()) => {
                            line.push(one[0]);
                            if one[0] == terminal {
                                return line;
                            }
                        }
                        Err(e) if e.raw_os_error() == Some(5) => {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
                panic!("serial read limit")
            };
            assert_eq!(read_until(b'\n'), b"AT+CMGF=0\r\n");
            master.write_all(b"OK\r\n").unwrap();
            let mut command = Vec::new();
            loop {
                let mut one = [0];
                master.read_exact(&mut one).unwrap();
                command.push(one[0]);
                if one[0] == b'\n' {
                    break;
                }
            }
            assert!(command.starts_with(b"AT+CMGS="));
            master.write_all(b"> ").unwrap();
            let mut payload = Vec::new();
            loop {
                let mut one = [0];
                master.read_exact(&mut one).unwrap();
                if one[0] == 0x1a {
                    break;
                }
                payload.push(one[0]);
            }
            let decoded = pdu::decode(std::str::from_utf8(&payload).unwrap()).unwrap();
            assert_eq!(decoded.content, "原生短信验证");
            master.write_all(b"+CMGS: 7\r\nOK\r\n").unwrap();
            done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sms.sqlite3");
        let pool = PortPool::default();
        let modem:Modem=serde_json::from_value(json!({"id":"m1","name":"test","manufacturer":"quectel","platform":"qualcomm","bus":"usb","at_port":port})).unwrap();
        let request = Send {
            request_id: "native-1".into(),
            peer: "10086".into(),
            content: "原生短信验证".into(),
        };
        let first = send(modem.clone(), pool.clone(), path.clone(), request.clone())
            .await
            .unwrap();
        assert_eq!(first["delivery_status"], "submitted");
        let second = send(modem.clone(), pool.clone(), path, request)
            .await
            .unwrap();
        assert_eq!(second["id"], first["id"]);
        assert_eq!(pool.inspect(&modem.at_port).await.unwrap().1.completed, 1);
        done_tx.send(()).unwrap();
        emulator.join().unwrap();
    }
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub mode: Mode,
    pub poll_interval_seconds: u64,
    pub memories: [String; 3],
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Manual,
    Poll,
    Urc,
    SimOnly,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Manual,
            poll_interval_seconds: 30,
            memories: ["SM".into(), "SM".into(), "SM".into()],
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (5..=86400).contains(&self.poll_interval_seconds),
            "SMS poll interval must be 5 to 86400 seconds"
        );
        for memory in &self.memories {
            validate_memory(memory)?;
        }
        Ok(())
    }
}
/// URC setup is allowed only when the model and firmware match the upstream rule.
pub async fn setup_urc(
    modem: &Modem,
    pool: &PortPool,
) -> Result<(
    tokio::sync::broadcast::Receiver<crate::at::SerialEvent>,
    String,
)> {
    let catalog: Value =
        serde_json::from_str(include_str!("../../../../data/supported-models.json"))?;
    let rule = &catalog["modem_support"]["usb"][modem.model.to_ascii_lowercase()]["settings"]["urc"]
        ["sms"];
    let required = rule["firmware"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("SMS URC unsupported for this model"))?
        .to_owned();
    let setup = rule["setup"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing URC setup"))?
        .to_owned();
    let prefix = rule["prefix"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing URC prefix"))?
        .to_owned();
    let port = pool
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await?;
    let receiver = port.subscribe();
    let replies = port
        .run_named(
            Box::new(UrcSetup {
                required,
                setup,
                sent: false,
            }),
            Some(modem.id.clone()),
            "sms_urc_setup",
        )
        .await?;
    ensure!(
        replies.len() == 2 && replies.iter().all(|r| r.modem_success),
        "SMS URC setup rejected or firmware did not match"
    );
    Ok((receiver, prefix))
}
struct UrcSetup {
    required: String,
    setup: String,
    sent: bool,
}
impl Program for UrcSetup {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        if !self.sent {
            self.sent = true;
            return Ok(Next::Command(command("AT+CGMR")));
        }
        if replies.len() == 1
            && replies[0].modem_success
            && replies[0].response.contains(&self.required)
        {
            return Ok(Next::Command(command(&self.setup)));
        }
        Ok(Next::Finish)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSim {
    pub index: u16,
    pub expected_pdu: String,
    pub memory: String,
}
pub async fn delete_sim(modem: &Modem, pool: &PortPool, request: DeleteSim) -> Result<Value> {
    validate_memory(&request.memory)?;
    pdu::decode(&request.expected_pdu)?;
    let port = pool
        .get(modem.sms_at_port.as_deref().unwrap_or(&modem.at_port))
        .await?;
    let replies = port
        .run_named(
            Box::new(DeleteProgram { request, stage: 0 }),
            Some(modem.id.clone()),
            "sms_delete_sim",
        )
        .await?;
    ensure!(
        replies.len() == 4 && replies.iter().all(|r| r.modem_success),
        "SIM message changed or deletion was rejected; no unverified index was deleted"
    );
    Ok(json!({"deleted":true,"scope":"sim"}))
}
struct DeleteProgram {
    request: DeleteSim,
    stage: u8,
}
impl Program for DeleteProgram {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        if replies.last().is_some_and(|r| !r.modem_success) {
            return Ok(Next::Finish);
        }
        let step = match self.stage {
            0 => command(&format!("AT+CPMS=\"{}\"", self.request.memory)),
            1 => command("AT+CMGF=0"),
            2 => command(&format!("AT+CMGR={}", self.request.index)),
            3 => {
                if !replies.last().is_some_and(|r| {
                    r.response
                        .lines()
                        .any(|l| l.trim().eq_ignore_ascii_case(&self.request.expected_pdu))
                }) {
                    return Ok(Next::Finish);
                }
                command(&format!("AT+CMGD={}", self.request.index))
            }
            _ => return Ok(Next::Finish),
        };
        self.stage += 1;
        Ok(Next::Command(step))
    }
}
