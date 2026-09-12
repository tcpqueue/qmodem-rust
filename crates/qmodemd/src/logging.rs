use crate::config::{LogFormat, Logging};
use anyhow::{Result, anyhow};
use tracing_subscriber::EnvFilter;

pub fn init(settings: &Logging) -> Result<()> {
    // Neither RUST_LOG nor third-party debug logs override the operator's setting.
    let filter = EnvFilter::new(format!("off,qmodemd={}", settings.level.as_str()));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .with_writer(LogWriter::default);
    match settings.format {
        LogFormat::Text => builder
            .try_init()
            .map_err(|e| anyhow!("initialize logging: {e}")),
        LogFormat::Json => builder
            .json()
            .try_init()
            .map_err(|e| anyhow!("initialize logging: {e}")),
    }
}

#[derive(Default)]
struct LogWriter {
    bytes: Vec<u8>,
}
impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::io::stderr().write_all(bytes)?;
        let keep = (16384usize.saturating_sub(self.bytes.len())).min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..keep]);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}
#[derive(Default)]
struct Ring {
    next: u64,
    bytes: usize,
    entries: std::collections::VecDeque<(u64, String)>,
}
static RING: std::sync::OnceLock<std::sync::Mutex<Ring>> = std::sync::OnceLock::new();
fn ring() -> &'static std::sync::Mutex<Ring> {
    RING.get_or_init(Default::default)
}
impl Drop for LogWriter {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(&self.bytes).trim_end().to_owned();
        let mut ring = ring()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ring.next += 1;
        let sequence = ring.next;
        ring.bytes += line.len();
        ring.entries.push_back((sequence, line));
        while ring.entries.len() > 1024 || ring.bytes > 256 * 1024 {
            if let Some((_, line)) = ring.entries.pop_front() {
                ring.bytes -= line.len();
            }
        }
    }
}
fn belongs(line: &str, modem: &str) -> bool {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
        return value["fields"]["modem_id"] == modem;
    }
    line.split_whitespace().any(|field| {
        field
            .strip_prefix("modem_id=")
            .is_some_and(|id| id.trim_matches('"') == modem)
    })
}
pub fn read(modem: Option<&str>) -> serde_json::Value {
    let ring = ring()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    serde_json::json!({"items":ring.entries.iter().filter(|(_,line)|modem.is_none_or(|id|belongs(line,id))).map(|(seq,line)|serde_json::json!({"sequence":seq,"line":line})).collect::<Vec<_>>(),"scope":"service_memory","max_bytes":256*1024})
}
pub fn clear(modem: Option<&str>) {
    let mut ring = ring()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ring.entries
        .retain(|(_, line)| modem.is_some_and(|id| !belongs(line, id)));
    ring.bytes = ring.entries.iter().map(|(_, line)| line.len()).sum();
}
