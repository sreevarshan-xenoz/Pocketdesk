// Pocketdesk Server
//
// Background daemon for remote desktop access over Tailscale.

use anyhow::Result;
use clap::Parser;
use pocketdesk_server::*;
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
            .filename_prefix("pocketdesk")
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

    tracing::info!("Pocketdesk server starting");

    let command = cli
        .command
        .unwrap_or(cli::Commands::Start { background: false });
    let wants_json = command.wants_json();
    let result = run(command, config).await;
    // A script reading `--json` reads its failures there too.
    if wants_json && let Err(e) = &result {
        local_api::print(&local_api::error(format!("{e:#}")));
        std::process::exit(local_api::exit::FAILED);
    }
    result
}

async fn run(command: cli::Commands, config: config::Config) -> Result<()> {
    match command {
        cli::Commands::Start { .. } => {
            // R4 A2: register the session-outcome telemetry sink before any
            // streaming pipeline can run.
            session_telemetry::init();
            service::run(config).await
        }
        cli::Commands::Stop => service::stop().await,
        cli::Commands::Status { json: false } => service::print_status().await,
        cli::Commands::Status { json: true } => local_api::print_status(),
        cli::Commands::Devices { json } => local_api::print_devices(json),
        cli::Commands::Send {
            phone,
            path,
            wait,
            json,
        } => {
            // Like `bench`, the exit code is what a script reads.
            std::process::exit(local_api::run_send(&phone, &path, wait, json))
        }
        cli::Commands::Sent { count, json } => local_api::print_files(true, count, json),
        cli::Commands::Received { count, json } => local_api::print_files(false, count, json),
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
        cli::Commands::List { json } => service::list_peers(json).await,
        cli::Commands::Watch { interval } => service::watch_peers(interval).await,
        cli::Commands::Capabilities { json, markdown } => {
            service::print_capabilities(json, markdown).await
        }
        cli::Commands::Connect { peer, port } => {
            let identity = kde::host_identity();
            service::connect_peer(peer, port, &identity).await
        }
        cli::Commands::Pair { pin, grant, json } => service::pair(pin, grant, json).await,
        cli::Commands::Unpair { device_id, json } => service::unpair(device_id, json).await,
        cli::Commands::Kick { device, json } => {
            std::process::exit(local_api::run_kick(&device, json))
        }
        cli::Commands::Relay {
            port,
            hostname,
            email,
            allow,
            allow_paired,
            open,
        } => {
            relay::run(relay::RelayOptions {
                port,
                hostname,
                email,
                allow,
                allow_paired,
                open,
            })
            .await
        }
        cli::Commands::RelayIds { json } => {
            let ids = relay::relay_ids()?;
            if json {
                let ids: Vec<_> = ids
                    .iter()
                    .map(|(who, id)| serde_json::json!({ "who": who, "id": id }))
                    .collect();
                local_api::print(&local_api::versioned(serde_json::json!({ "ids": ids })));
            } else {
                for (who, id) in ids {
                    println!("{who}: {id}");
                }
            }
            Ok(())
        }
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
