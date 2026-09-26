//! A Rust implementation of the hook-relay client.
//!
//! The command-line interface follows `src/client/index.ts` in
//! <https://github.com/itkq/hook-relay>.

mod protocol;
mod relay;

use std::io::IsTerminal;
use std::net::{Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use axum::{Json, Router, routing::get};
use clap::Parser;
use serde_json::json;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;
use tracing::{error, info, warn};
use url::Url;

use crate::relay::{Outcome, Relay, stopped};

/// Written by the Release PR workflow; see build.rs.
const VERSION: &str = env!("HOOK_RELAY_CLIENT_VERSION");

/// Receives webhooks relayed by a hook-relay server over WebSocket and
/// forwards them to another endpoint.
#[derive(Debug, Parser)]
#[command(name = "hook-relay-client", version = VERSION)]
struct Cli {
    /// Server endpoint URL, such as wss://relay.example.com
    #[arg(long)]
    server_endpoint: Url,

    /// Forward endpoint URL, such as http://localhost:9000
    #[arg(long)]
    forward_endpoint: Url,

    /// Receive only webhooks for this path (default: all paths)
    #[arg(long)]
    path: Option<String>,

    /// Log level: error, warn, info, debug, or trace
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: tracing::Level,

    /// Receive only webhooks whose body matches this JavaScript regular
    /// expression; the server evaluates it and rejects unsafe patterns
    #[arg(long)]
    filter_body_regex: Option<String>,

    /// Reconnect interval in milliseconds
    #[arg(long, default_value_t = 1000)]
    reconnect_interval_ms: u64,

    /// Port of the local HTTP server that reports the client ID
    #[arg(long, env = "PORT", default_value_t = 3001)]
    port: u16,

    /// Passphrase for the challenge response
    #[arg(long, env = "CHALLENGE_PASSPHRASE", hide_env_values = true)]
    challenge_passphrase: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.log_level);
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging(level: tracing::Level) {
    let logs = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false);
    if std::io::stdout().is_terminal() {
        logs.init();
    } else {
        logs.json().flatten_event(true).init();
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let client_id = uuid::Uuid::new_v4().to_string();
    let (stop_tx, mut stop) = watch::channel(false);
    tokio::spawn(handle_signals(stop_tx));

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, cli.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("could not listen on port {}", cli.port))?;
    info!("Server started on port {}", cli.port);
    let id = client_id.clone();
    let app = Router::new().route(
        "/",
        get(move || async move { Json(json!({ "clientId": id })) }),
    );
    let mut server_stop = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { stopped(&mut server_stop).await })
            .await
    });

    let relay = Relay {
        client_id,
        server_endpoint: cli.server_endpoint,
        forward_endpoint: cli.forward_endpoint,
        challenge_passphrase: cli.challenge_passphrase,
        path: cli.path,
        filter_body_regex: cli.filter_body_regex,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("could not build the HTTP client")?,
    };
    let interval = Duration::from_millis(cli.reconnect_interval_ms);
    loop {
        if relay.connect(&mut stop).await? == Outcome::Shutdown {
            break;
        }
        tokio::select! {
            () = stopped(&mut stop) => break,
            _ = tokio::time::sleep(interval) => info!("Attempting to reconnect..."),
        }
    }

    server.await??;
    info!("Server closed");
    Ok(())
}

/// Turns `stop` true on the first SIGINT or SIGTERM and exits on the second.
async fn handle_signals(stop: watch::Sender<bool>) {
    let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
    for count in 0.. {
        let name = tokio::select! {
            _ = sigint.recv() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        };
        info!("{name} received");
        if count > 0 {
            warn!("Shutdown already in progress, forcing exit");
            std::process::exit(1);
        }
        info!("Shutdown initiated");
        stop.send_replace(true);
    }
}
