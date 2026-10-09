//! Linux Link Server
//!
//! Background daemon for remote desktop access over Tailscale.

use anyhow::Result;
use clap::Parser;
use linux_link_server::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    platform::init_process();
    let cli = cli::Cli::parse();

    // Load configuration first
    let config = config::Config::load()?;

    // Initialize logging with config log_level
    tracing_subscriber::registry()
        // Logs go to stderr so a command's stdout is only ever its data —
        // `capabilities --json` and `--markdown` are consumed by scripts and by
        // the docs generator, which cannot filter an ANSI log line out of them.
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::try_new(&config.log_level)
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
            }),
        )
        .init();

    tracing::info!("Linux Link Server starting");

    match cli.command.unwrap_or(cli::Commands::Start) {
        cli::Commands::Start => {
            // R4 A2: register the session-outcome telemetry sink before any
            // streaming pipeline can run.
            session_telemetry::init();
            service::run(config).await
        }
        cli::Commands::Stop => service::stop().await,
        cli::Commands::Status => service::print_status().await,
        cli::Commands::Sessions { count, json } => session_telemetry::print_sessions(count, json),
        cli::Commands::Bench {
            target,
            frames,
            repeat,
            json,
            record,
            baseline,
            tolerance_pct,
        } => {
            // The exit code is the contract a CI job reads, so it is the only
            // place in this binary that ends the process directly.
            std::process::exit(bench::run(
                &target,
                frames,
                repeat,
                json,
                record,
                baseline,
                tolerance_pct,
            ))
        }
        cli::Commands::List => service::list_peers().await,
        cli::Commands::Watch { interval } => service::watch_peers(interval).await,
        cli::Commands::Capabilities { json, markdown } => {
            service::print_capabilities(json, markdown).await
        }
        cli::Commands::Connect { peer, port } => {
            let identity = kde::host_identity();
            service::connect_peer(peer, port, &identity).await
        }
        cli::Commands::Pair { pin, grant } => service::pair(pin, grant).await,
        cli::Commands::Unpair { device_id } => service::unpair(device_id).await,
        cli::Commands::Kick { device } => service::kick(device).await,
    }
}
