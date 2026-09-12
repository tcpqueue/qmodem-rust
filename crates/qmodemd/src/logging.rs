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
        .with_writer(std::io::stderr);
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
