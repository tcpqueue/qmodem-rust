mod config;
mod storage;

use anyhow::{Result, ensure};
use axum::{Json, Router, routing::get};
use clap::{Parser, Subcommand};
use config::Config;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};

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
    /// Start the HTTP service. Modem APIs are not implemented yet.
    Serve,
    /// Atomically update basic service settings; takes effect after restart.
    SetService {
        #[arg(long)]
        listen: std::net::IpAddr,
        #[arg(long)]
        port: u16,
    },
    /// Validate TOML without creating a database or changing the system.
    Check,
    /// Return only non-secret service settings for the LuCI control panel.
    ServiceInfo,
}

async fn health() -> Json<Value> {
    Json(
        json!({"service":"qmodemd", "version":env!("CARGO_PKG_VERSION"), "stage":"bootstrap", "modem_api_ready":false}),
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    match cli.command {
        Command::SetService { listen, port } => {
            ensure!(
                listen.is_loopback(),
                "this bootstrap build only supports loopback listening"
            );
            config::set_service(&cli.config, listen, port)?;
            println!("{}", json!({"saved":true,"restart_required":true}));
        }
        Command::Check => println!("configuration valid"),
        Command::ServiceInfo => println!(
            "{}",
            json!({"listen":cfg.server.listen.to_string(), "port":cfg.server.port, "stage":"bootstrap"})
        ),
        Command::Serve => {
            // Until authentication is implemented, only the loopback health endpoint is exposed.
            ensure!(
                cfg.server.listen.is_loopback(),
                "this bootstrap build only supports loopback listening; authenticated administration is not implemented yet"
            );
            let _db = storage::initialize(std::path::Path::new(&cfg.storage.sqlite))?;
            let address = SocketAddr::new(cfg.server.listen, cfg.server.port);
            let listener = tokio::net::TcpListener::bind(address).await?;
            eprintln!("qmodemd listening on {}", listener.local_addr()?);
            let app = Router::new().route("/api/health", get(health));
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown())
                .await?;
        }
    }
    Ok(())
}

async fn shutdown() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}
