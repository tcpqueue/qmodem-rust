mod at;
mod auth;
mod config;
mod http;
mod listener;
mod logging;
mod storage;
mod vendor;

use anyhow::{Result, ensure};
use clap::{Parser, Subcommand};
use config::{Config, LogFormat, LogLevel, ServicePatch};
use serde_json::json;
use std::{net::IpAddr, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "QModem Rust service")]
struct Cli {
    #[arg(long, global = true, default_value = "/etc/qmodem-rust.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Serve,
    /// Send one command through the native serial transport (configured modems only).
    At {
        #[arg(long)]
        modem: String,
        #[arg(long)]
        command: String,
        #[arg(long, default_value_t = 10000)]
        timeout_ms: u64,
    },
    /// Update only specified settings. Restart the service to apply changes.
    SetService {
        #[arg(long)]
        listen: Option<IpAddr>,
        #[arg(long)]
        port: Option<u16>,
        /// Linux device name. Use 'any' to remove the device restriction.
        #[arg(long)]
        interface: Option<String>,
        #[arg(long, value_enum)]
        log_level: Option<LogLevel>,
        #[arg(long, value_enum)]
        log_format: Option<LogFormat>,
    },
    Check,
    ServiceInfo,
    Interfaces,
    /// Create a first access token, print it once, and store only its hash.
    InitAuth,
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // Interface discovery remains usable even when the configuration is broken.
    if matches!(cli.command, Command::Interfaces) {
        println!("{}", json!({"interfaces":listener::interfaces()?}));
        return Ok(());
    }
    let cfg = Config::load(&cli.config)?;
    match cli.command {
        Command::SetService {
            listen,
            port,
            interface,
            log_level,
            log_format,
        } => {
            config::set_service(
                &cli.config,
                ServicePatch {
                    listen,
                    port,
                    interface: interface.map(|s| if s == "any" { String::new() } else { s }),
                    log_level,
                    log_format,
                },
            )?;
            println!("{}", json!({"saved":true,"restart_required":true}));
        }
        Command::At {
            modem,
            command,
            timeout_ms,
        } => {
            logging::init(&cfg.logging)?;
            let device = cfg
                .modems
                .iter()
                .find(|m| m.id == modem && m.enabled)
                .ok_or_else(|| anyhow::anyhow!("enabled modem not found"))?;
            let step = at::Step::command(&command, std::time::Duration::from_millis(timeout_ms))?;
            let pool = at::PortPool::default();
            let replies = pool.get(&device.at_port).await?.execute(vec![step]).await?;
            println!("{}", serde_json::to_string(&replies)?);
        }
        Command::Check => println!("configuration valid"),
        Command::ServiceInfo => println!("{}", http::service_info(&cfg)),
        Command::Interfaces => unreachable!(),
        Command::InitAuth => println!("{}", json!({"token":auth::initialize(&cli.config)?})),
        Command::Serve => {
            logging::init(&cfg.logging)?;
            if let Err(error) = serve(cfg).await {
                tracing::error!(error=%error,"service stopped with an error");
                return Err(error);
            }
        }
    }
    Ok(())
}
async fn serve(cfg: Config) -> Result<()> {
    ensure!(
        cfg.server.listen.is_loopback() || !cfg.auth.token_hash.is_empty(),
        "non-loopback listening requires an access token; run init-auth first"
    );
    let listener = listener::bind(&cfg.server)?;
    let _db = storage::initialize(std::path::Path::new(&cfg.storage.sqlite))?;
    tracing::info!(listen=%listener.local_addr()?,interface=%cfg.server.interface,level=cfg.logging.level.as_str(),"service started");
    tracing::debug!("SQLite schema ready");
    axum::serve(listener, http::router(cfg))
        .with_graceful_shutdown(shutdown())
        .await?;
    tracing::info!("service stopped");
    Ok(())
}
async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = terminate.recv()=>{},}
    tracing::info!("shutdown requested");
}
