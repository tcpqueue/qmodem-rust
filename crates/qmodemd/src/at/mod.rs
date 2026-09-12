//! One owned worker per canonical serial device. Jobs survive caller cancellation
//! once transmission starts, so the next caller never consumes an unfinished reply.
mod protocol;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, ensure};
use protocol::{Decoder, default_flags, end_match};
use serde::Serialize;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, mpsc, oneshot},
    time::{Instant, timeout_at},
};
use tokio_serial::SerialPortBuilderExt;

const RESPONSE_LIMIT: usize = 256 * 1024;
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(100);

pub struct Step {
    pub bytes: Vec<u8>,
    pub flags: Vec<String>,
    pub timeout: Duration,
}
impl Step {
    pub fn command(command: &str, timeout: Duration) -> Result<Self> {
        ensure!(
            !command.is_empty()
                && command.len() <= 4096
                && command
                    .get(..2)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("AT")),
            "command must start with AT and be at most 4096 bytes"
        );
        ensure!(
            !command.chars().any(char::is_control),
            "AT command must not contain control characters"
        );
        ensure!(
            !timeout.is_zero() && timeout <= Duration::from_secs(120),
            "AT timeout must be between 1 ms and 120 seconds"
        );
        Ok(Self {
            bytes: format!("{command}\r\n").into_bytes(),
            flags: default_flags(),
            timeout,
        })
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    /// Like the upstream transport, 0 means a terminal was received; ERROR is
    /// distinguished by terminal and modem_success, not by a fabricated timeout.
    pub status: i32,
    pub terminal: String,
    pub modem_success: bool,
    pub response: String,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Timeout,
    Io,
    Overflow,
    Unsynchronized,
    QueueFull,
    Closed,
}
#[derive(Debug, Clone, Serialize)]
pub struct AtError {
    pub kind: ErrorKind,
    pub message: String,
}
impl std::fmt::Display for AtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for AtError {}
fn err(kind: ErrorKind, message: impl Into<String>) -> AtError {
    AtError {
        kind,
        message: message.into(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SerialEvent {
    pub correlation: &'static str,
    pub line: String,
}
fn emit(events: &broadcast::Sender<SerialEvent>, correlation: &'static str, line: String) {
    let _ = events.send(SerialEvent { correlation, line });
}
struct Job {
    steps: Vec<Step>,
    reply: oneshot::Sender<std::result::Result<Vec<Reply>, AtError>>,
}
#[derive(Clone)]
pub struct Port {
    sender: mpsc::Sender<Job>,
    events: broadcast::Sender<SerialEvent>,
}
impl Port {
    fn start<T>(stream: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(32);
        let (events, _) = broadcast::channel(64);
        tokio::spawn(worker(stream, receiver, events.clone()));
        Self { sender, events }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<SerialEvent> {
        self.events.subscribe()
    }
    pub async fn execute(&self, steps: Vec<Step>) -> std::result::Result<Vec<Reply>, AtError> {
        if steps.is_empty() || steps.len() > 16 {
            return Err(err(
                ErrorKind::Overflow,
                "a transaction requires 1 to 16 steps",
            ));
        }
        let (reply, result) = oneshot::channel();
        self.sender
            .try_send(Job { steps, reply })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => err(ErrorKind::QueueFull, "AT queue is full"),
                mpsc::error::TrySendError::Closed(_) => err(ErrorKind::Closed, "AT port is closed"),
            })?;
        result
            .await
            .map_err(|_| err(ErrorKind::Closed, "AT port worker stopped"))?
    }
}
#[derive(Clone, Default)]
pub struct PortPool {
    ports: Arc<Mutex<HashMap<PathBuf, Port>>>,
}
impl PortPool {
    pub async fn get(&self, path: &str) -> Result<Port> {
        let canonical =
            std::fs::canonicalize(path).with_context(|| format!("open serial device {path}"))?;
        ensure!(
            canonical.starts_with("/dev"),
            "serial device must resolve under /dev"
        );
        let mut ports = self.ports.lock().await;
        if let Some(port) = ports.get(&canonical) {
            return Ok(port.clone());
        }
        let path_text = canonical.to_str().context("serial path is not UTF-8")?;
        let mut stream = tokio_serial::new(path_text, 115200)
            .open_native_async()
            .context("open serial transport")?;
        stream
            .set_exclusive(true)
            .context("claim serial transport")?;
        let port = Port::start(stream);
        ports.insert(canonical, port.clone());
        tracing::info!(target:"qmodemd::at","serial transport opened");
        Ok(port)
    }
}

async fn worker<T: AsyncRead + AsyncWrite + Unpin>(
    mut stream: T,
    mut jobs: mpsc::Receiver<Job>,
    events: broadcast::Sender<SerialEvent>,
) {
    let mut decoder = Decoder::default();
    let mut synchronized = true;
    let mut idle = [0; 4096];
    loop {
        tokio::select! {
            biased;
            job=jobs.recv()=>{
                let Some(job)=job else{break;};
                if job.reply.is_closed(){continue;}
                if !synchronized {
                    let _=job.reply.send(Err(err(ErrorKind::Unsynchronized,"previous transaction did not finish; restart the service after recovering the modem")));
                    continue;
                }
                let started=Instant::now();
                let mut replies=Vec::new();
                let mut failed=None;
                for step in job.steps {
                    match exchange(&mut stream,&mut decoder,&step,&events).await {
                        Ok(reply)=>{
                            let rejected=!reply.modem_success;
                            replies.push(reply);
                            if rejected{break;}
                        },
                        Err(error)=>{
                            tracing::warn!(target:"qmodemd::at",kind=?error.kind,"AT transaction failed");
                            let recoverable=error.kind==ErrorKind::Timeout || error.kind==ErrorKind::Overflow;
                            // The caller gets its error promptly; queued transactions remain held.
                            failed=Some((error,if recoverable{Some(step.flags)}else{None}));break;
                        }
                    }
                }
                if let Some((error,flags))=failed {
                    let _=job.reply.send(Err(error));
                    synchronized=if let Some(flags)=flags{recover(&mut stream,&mut decoder,&flags,&events).await}else{false};
                    if !synchronized {tracing::error!(target:"qmodemd::at","serial transport quarantined after unfinished transaction");}
                }else{
                    while let Ok(Some(line))=decoder.next(&[]) {emit(&events,"unsolicited",line);}
                    tracing::debug!(target:"qmodemd::at",elapsed_ms=started.elapsed().as_millis() as u64,steps=replies.len(),"AT transaction completed");
                    let _=job.reply.send(Ok(replies));
                }
            },
            read=stream.read(&mut idle)=>{
                match read {
                    Ok(0)|Err(_)=>{tracing::warn!(target:"qmodemd::at","serial device disconnected");break;},
                    Ok(n)=>{
                        tracing::trace!(target:"qmodemd::at",bytes=n,"unsolicited serial data");
                        if decoder.push(&idle[..n]).is_err() {
                            decoder.clear();emit(&events,"overflow",String::new());
                        }
                        while let Ok(Some(line))=decoder.next(&[]) {emit(&events,"unsolicited",line);}
                    }
                }
            }
        }
    }
}

async fn exchange<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
    decoder: &mut Decoder,
    step: &Step,
    events: &broadcast::Sender<SerialEvent>,
) -> std::result::Result<Reply, AtError> {
    decoder.clear();
    let deadline = Instant::now() + step.timeout;
    match timeout_at(deadline, stream.write_all(&step.bytes)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => return Err(err(ErrorKind::Io, "write serial device failed")),
        Err(_) => return Err(err(ErrorKind::Timeout, "serial write timed out")),
    }
    let mut response = String::new();
    let mut read_buffer = [0; 4096];
    loop {
        while let Some(line) = decoder.next(&step.flags)? {
            if response.len() + line.len() + 2 > RESPONSE_LIMIT {
                return Err(err(ErrorKind::Overflow, "AT response exceeded limit"));
            }
            emit(
                events,
                if end_match(&line, &step.flags).is_some() {
                    "terminal"
                } else {
                    "response"
                },
                line.clone(),
            );
            response.push_str(&line);
            response.push_str("\r\n");
            if let Some(flag) = end_match(&line, &step.flags) {
                let success = !(flag == "ERROR"
                    || flag == "NO CARRIER"
                    || flag.starts_with("+CME ERROR")
                    || flag.starts_with("+CMS ERROR"));
                return Ok(Reply {
                    status: 0,
                    terminal: flag.to_owned(),
                    modem_success: success,
                    response,
                });
            }
        }
        match timeout_at(deadline, stream.read(&mut read_buffer)).await {
            Err(_) => return Err(err(ErrorKind::Timeout, "AT response timed out")),
            Ok(Err(_)) | Ok(Ok(0)) => return Err(err(ErrorKind::Io, "serial device disconnected")),
            Ok(Ok(n)) => decoder.push(&read_buffer[..n])?,
        }
    }
}

async fn recover<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
    decoder: &mut Decoder,
    flags: &[String],
    events: &broadcast::Sender<SerialEvent>,
) -> bool {
    let deadline = Instant::now() + RECOVERY_TIMEOUT;
    let mut terminal = false;
    let mut buf = [0; 4096];
    loop {
        loop {
            match decoder.next(flags) {
                Ok(Some(line)) => {
                    emit(events, "recovery", line.clone());
                    if end_match(&line, flags).is_some() {
                        terminal = true;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    decoder.clear();
                    break;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        let until = if terminal {
            std::cmp::min(deadline, Instant::now() + QUIET)
        } else {
            deadline
        };
        match timeout_at(until, stream.read(&mut buf)).await {
            Err(_) => {
                decoder.clear();
                if terminal {
                    tracing::debug!(target:"qmodemd::at","late response drained; serial transport synchronized");
                }
                return terminal;
            }
            Ok(Ok(n)) if n > 0 => {
                if decoder.push(&buf[..n]).is_err() {
                    decoder.clear();
                }
            }
            _ => return false,
        }
    }
}
