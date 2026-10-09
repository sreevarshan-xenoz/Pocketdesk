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

    let background = matches!(cli.command, Some(cli::Commands::Start { background: true }));
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::try_new(&config.log_level)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    });
    // Held for the process lifetime: dropping it stops the log writer.
    let _log_guard = if background {
        platform::detach_console();
        let dir = state::state_dir()?.join("logs");
        std::fs::create_dir_all(&dir)?;
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("linux-link")
            .filename_suffix("log")
            .max_log_files(7)
            .build(&dir)?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer),
            )
            .with(filter)
            .init();
        Some(guard)
    } else {
        tracing_subscriber::registry()
            // Logs go to stderr so a command's stdout is only ever its data —
            // `capabilities --json` and `--markdown` are consumed by scripts and
            // by the docs generator, which cannot filter an ANSI log line out.
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .with(filter)
            .init();
        None
    };

    tracing::info!("Linux Link Server starting");

    match cli
        .command
        .unwrap_or(cli::Commands::Start { background: false })
    {
        cli::Commands::Start { .. } => {
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
        #[cfg(windows)]
        cli::Commands::Install { elevated } => install_windows::install(elevated),
        #[cfg(windows)]
        cli::Commands::Uninstall => install_windows::uninstall(),
        #[cfg(windows)]
        cli::Commands::FirewallAdd => install_windows::apply_firewall(true),
        #[cfg(windows)]
        cli::Commands::FirewallRemove => install_windows::apply_firewall(false),
    }
}
