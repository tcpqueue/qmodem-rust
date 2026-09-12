//! One owned worker per canonical serial device. Jobs survive caller cancellation
//! once transmission starts, so the next caller never consumes an unfinished reply.
mod protocol;
mod queue;
pub use queue::QueueView;
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

// Expose only known verbs, never arguments or arbitrary command payloads.
fn command_label(bytes: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(bytes).to_ascii_uppercase();
    let verb = text.trim().split(['=', '?', ';']).next().unwrap_or("");
    match verb {
        "AT" => "AT",
        "AT+CPIN" => "AT+CPIN",
        "AT+COPS" => "AT+COPS",
        "AT+CGDCONT" => "AT+CGDCONT",
        "AT^AUTHDATA" => "AT^AUTHDATA",
        "AT^NDISDUP" => "AT^NDISDUP",
        "AT^NDISSTATQRY" => "AT^NDISSTATQRY",
        "AT^SETAUTODIAL" => "AT^SETAUTODIAL",
        "AT+QNETDEVCTL" => "AT+QNETDEVCTL",
        "AT+QCFG" => "AT+QCFG",
        "AT+CMGS" => "AT+CMGS",
        _ => "AT / payload",
    }
}

#[derive(Clone)]
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
    State,
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
/// Programs are constructed by trusted vendor code, never deserialized from HTTP.
/// The worker owns the complete program, including retries and delays.
pub enum Next {
    Command(Step),
    Wait(Duration),
    Finish,
}
pub trait Program: Send {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError>;
}
pub struct Sequence {
    steps: std::collections::VecDeque<Step>,
    continue_on_error: bool,
}
impl Sequence {
    pub fn new(steps: Vec<Step>, continue_on_error: bool) -> Self {
        Self {
            steps: steps.into(),
            continue_on_error,
        }
    }
}
impl Program for Sequence {
    fn next(&mut self, replies: &[Reply]) -> std::result::Result<Next, AtError> {
        if !self.continue_on_error && replies.last().is_some_and(|r| !r.modem_success) {
            return Ok(Next::Finish);
        }
        Ok(self.steps.pop_front().map_or(Next::Finish, Next::Command))
    }
}
struct Job {
    id: u64,
    program: Box<dyn Program>,
    reply: oneshot::Sender<std::result::Result<Vec<Reply>, AtError>>,
}
#[derive(Clone)]
pub struct Port {
    monitor: queue::Monitor,
    sender: mpsc::Sender<Job>,
    events: broadcast::Sender<SerialEvent>,
    worker: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}
impl Port {
    fn start<T>(stream: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel(32);
        let (events, _) = broadcast::channel(64);
        let monitor = queue::Monitor::default();
        let task = tokio::spawn(worker(stream, receiver, events.clone(), monitor.clone()));
        Self {
            sender,
            events,
            monitor,
            worker: Arc::new(Mutex::new(Some(task))),
        }
    }
    pub async fn close(&self) -> Result<()> {
        self.monitor.freeze()?;
        if let Some(task) = self.worker.lock().await.take() {
            task.abort();
            let _ = task.await;
        }
        self.monitor.close();
        emit(&self.events, "closed", String::new());
        Ok(())
    }
    pub fn snapshot(&self) -> QueueView {
        self.monitor.snapshot()
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
        self.run(Box::new(Sequence::new(steps, false))).await
    }
    pub async fn run(&self, program: Box<dyn Program>) -> std::result::Result<Vec<Reply>, AtError> {
        self.run_named(program, None, "at").await
    }
    pub async fn run_named(
        &self,
        program: Box<dyn Program>,
        modem_id: Option<String>,
        operation: &'static str,
    ) -> std::result::Result<Vec<Reply>, AtError> {
        let (reply, result) = oneshot::channel();
        self.monitor
            .submit(&self.sender, program, reply, modem_id, operation)?;
        result
            .await
            .map_err(|_| err(ErrorKind::Closed, "AT port worker stopped"))?
    }
}
#[derive(Clone, Default)]
pub struct PortPool {
    ports: Arc<Mutex<HashMap<PathBuf, Port>>>,
    aliases: Arc<Mutex<HashMap<String, PathBuf>>>,
}
impl PortPool {
    /// Inspect without opening a device or sending an AT command.
    pub async fn inspect(&self, path: &str) -> Option<(String, QueueView)> {
        let canonical = self
            .aliases
            .lock()
            .await
            .get(path)
            .cloned()
            .or_else(|| std::fs::canonicalize(path).ok())?;
        self.ports
            .lock()
            .await
            .get(&canonical)
            .map(|port| (canonical.to_string_lossy().into_owned(), port.snapshot()))
    }
    /// Close only when no transaction or recovery is active. Old handles reject new jobs.
    pub async fn close(&self, path: &str) -> Result<()> {
        let canonical = self
            .aliases
            .lock()
            .await
            .get(path)
            .cloned()
            .or_else(|| std::fs::canonicalize(path).ok());
        let ports = self.ports.lock().await;
        if let Some(port) = canonical.and_then(|key| ports.get(&key)) {
            port.close().await?;
        }
        Ok(())
    }
    pub async fn get(&self, path: &str) -> Result<Port> {
        let canonical =
            std::fs::canonicalize(path).with_context(|| format!("open serial device {path}"))?;
        ensure!(
            canonical.starts_with("/dev"),
            "serial device must resolve under /dev"
        );
        self.aliases
            .lock()
            .await
            .insert(path.to_owned(), canonical.clone());
        let mut ports = self.ports.lock().await;
        if let Some(port) = ports.get(&canonical) {
            if port.snapshot().state != "closed" {
                return Ok(port.clone());
            }
            port.close().await?;
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
    monitor: queue::Monitor,
) {
    let mut decoder = Decoder::default();
    let mut synchronized = true;
    let mut pending_flags: Option<Vec<String>> = None;
    let mut late_quiet: Option<Instant> = None;
    let mut idle = [0; 4096];
    loop {
        tokio::select! {
            biased;
            _=tokio::time::sleep_until(late_quiet.unwrap_or_else(|| Instant::now()+Duration::from_secs(86400))), if late_quiet.is_some()=>{
                decoder.clear();
                pending_flags=None;
                late_quiet=None;
                synchronized=true;
                monitor.phase("idle");
                tracing::info!(target:"qmodemd::at","late terminal drained; serial transport synchronized");
            },
            job=jobs.recv()=>{
                let Some(mut job)=job else{break;};
                let cancelled=job.reply.is_closed();
                monitor.begin(job.id,cancelled);
                if cancelled{continue;}
                if !synchronized {
                    monitor.finish("unsynchronized",job.reply.is_closed());
                    monitor.phase("quarantined");
                    let _=job.reply.send(Err(err(ErrorKind::Unsynchronized,"previous transaction did not finish; close the idle port after recovering the modem")));
                    continue;
                }
                let started=Instant::now();
                let mut replies=Vec::new();
                let mut failed=None;
                let mut program_error=None;
                let mut actions=0;
                loop {
                    actions+=1;
                    if actions>128 {
                        program_error=Some(err(ErrorKind::State,"AT program exceeded action limit"));break;
                    }
                    let next=match job.program.next(&replies) {
                        Ok(next)=>next,
                        Err(e)=>{program_error=Some(e);break;}
                    };
                    let step=match next {
                        Next::Finish=>break,
                        Next::Wait(duration)=>{
                            monitor.phase("waiting");
                            if duration>Duration::from_secs(5) {
                                program_error=Some(err(ErrorKind::State,"AT program delay exceeded limit"));break;
                            }
                            if let Err(e)=wait_idle(&mut stream,&mut decoder,duration,&events).await {
                                failed=Some((e,None));break;
                            }
                            continue;
                        },
                        Next::Command(step)=>step,
                    };
                    let label = command_label(&step.bytes);
                    monitor.command(label);
                    match exchange(&mut stream,&mut decoder,&step,&events).await {
                        Ok(reply)=>{
                            replies.push(reply);
                        },
                        Err(mut error)=>{
                            error.message = format!("{} ({label})", error.message);
                            tracing::warn!(target:"qmodemd::at",kind=?error.kind,command=label,"AT transaction failed");
                            let recoverable=error.kind==ErrorKind::Timeout || error.kind==ErrorKind::Overflow;
                            if recoverable && step.flags.iter().any(|f|f == ">") {
                                // Cancel entry only before any SMS payload was submitted.
                                // Never resend or cancel an uncertain submitted SMS.
                                let _=tokio::time::timeout(QUIET,stream.write_all(&[0x1b])).await;
                            }
                            // The caller gets its error promptly; queued transactions remain held.
                            failed=Some((error,if recoverable{Some(default_flags())}else{None}));break;
                        }
                    }
                }
                if let Some((error,flags))=failed {
                    monitor.finish(match error.kind {ErrorKind::Timeout=>"timeout",ErrorKind::Overflow=>"overflow",_=>"transport_error"},job.reply.is_closed());
                    monitor.phase("recovering");
                    let _=job.reply.send(Err(error));
                    pending_flags=flags;
                    synchronized=if let Some(flags)=pending_flags.as_ref(){recover(&mut stream,&mut decoder,flags,&events).await}else{false};
                    if synchronized { pending_flags=None; }
                    monitor.phase(if synchronized{"idle"}else{"quarantined"});
                    if !synchronized {tracing::error!(target:"qmodemd::at","serial transport quarantined after unfinished transaction");}
                }else if let Some(error)=program_error {
                    monitor.finish("program_error",job.reply.is_closed());
                    let _=job.reply.send(Err(error));
                }else{
                    while let Ok(Some(line))=decoder.next(&[]) {emit(&events,"unsolicited",line);}
                    tracing::debug!(target:"qmodemd::at",elapsed_ms=started.elapsed().as_millis() as u64,steps=replies.len(),"AT transaction completed");
                    monitor.finish(if replies.iter().all(|r|r.modem_success){"completed"}else{"completed_with_modem_error"},job.reply.is_closed());
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
                        let mut got_terminal = false;
                        while let Ok(Some(line))=decoder.next(&[]) {
                            if pending_flags.as_ref().is_some_and(|flags| end_match(&line,flags).is_some()) {
                                got_terminal=true;
                            }
                            emit(&events,if synchronized {"unsolicited"} else {"recovery"},line);
                        }
                        if got_terminal || late_quiet.is_some() {
                            late_quiet=Some(Instant::now()+QUIET);
                        }
                    }
                }
            }
        }
    }
    monitor.close();
    emit(&events, "closed", String::new());
}

async fn exchange<T: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut T,
    decoder: &mut Decoder,
    step: &Step,
    events: &broadcast::Sender<SerialEvent>,
) -> std::result::Result<Reply, AtError> {
    // Complete trailing URCs from the preceding command before starting another.
    // Keep partial lines: a URC may be fragmented across this boundary.
    while let Some(line) = decoder.next(&[])? {
        emit(events, "unsolicited", line);
    }
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
            if end_match(&line, &step.flags).is_none()
                && protocol::is_unsolicited(&line, &step.bytes)
            {
                emit(events, "unsolicited", line);
                continue;
            }
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
                if terminal {
                    decoder.clear();
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

/// Keep draining URCs while holding the transaction's place in the port queue.
async fn wait_idle<T: AsyncRead + Unpin>(
    stream: &mut T,
    decoder: &mut Decoder,
    duration: Duration,
    events: &broadcast::Sender<SerialEvent>,
) -> std::result::Result<(), AtError> {
    let deadline = Instant::now() + duration;
    let mut buf = [0; 4096];
    loop {
        while let Some(line) = decoder.next(&[])? {
            emit(events, "unsolicited", line);
        }
        match timeout_at(deadline, stream.read(&mut buf)).await {
            Err(_) => return Ok(()),
            Ok(Ok(n)) if n > 0 => decoder.push(&buf[..n])?,
            _ => {
                return Err(err(
                    ErrorKind::Io,
                    "serial device disconnected during transaction delay",
                ));
            }
        }
    }
}
