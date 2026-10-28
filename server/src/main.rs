// Pocketdesk Server
//
// Background daemon for remote desktop access over Tailscale.

use anyhow::Result;
use clap::Parser;
use pocketdesk_server::*;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// How long the process waits, once a command is done, for work that cannot
/// be cancelled (a call blocking a runtime thread) before it ends anyway.
/// Without a bound a stopped server lingered on such a call, its executable
/// still in use for the uninstaller that stopped it.
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// What `#[tokio::main]` builds, but its shutdown bounded by [`EXIT_GRACE`].
fn main() -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(main_async());
    runtime.shutdown_timeout(EXIT_GRACE);
    result
}

async fn main_async() -> Result<()> {
    platform::init_process();
    // A crash leaves a report in the state folder (crash.log).
    if let Ok(dir) = state::state_dir() {
        crash_report::install(&dir);
    }
    // Started by a browser as the extension's host (ecosystem 1d): its own
    // arguments, which are no command of ours.
    let plain: Vec<String> = std::env::args().collect();
    if browser_host::launched_by_browser(&plain) {
        std::process::exit(browser_host::run());
    }
    // With --json, even a usage error is JSON (exit 2, like clap's own).
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cli = match cli::Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => match local_api::usage_error(&args, &e) {
            Some(out) => {
                local_api::print(&out);
                std::process::exit(local_api::exit::BAD_ARGUMENTS);
            }
            None => e.exit(),
        },
    };
    // Clap takes --json only on a command that has it, so its presence is
    // the answer for every command, new ones included.
    let wants_json = args.iter().any(|a| a == "--json");
    // The browser host needs no config, so a config.toml that does not parse
    // never stops it (nor the desktop app's switch for it).
    if let Some(cli::Commands::BrowserHost { install, remove }) = &cli.command {
        if *install || *remove {
            return browser_host::run_install(*remove);
        }
        std::process::exit(browser_host::run());
    }

    // A tel: link (M8) needs no config: a config.toml that does not parse
    // must not leave a browser's link doing nothing without a word.
    match &cli.command {
        Some(cli::Commands::Tel { uri }) => std::process::exit(tel_handler::run_tel(uri)),
        Some(cli::Commands::TelHandler { remove }) => {
            return tel_handler::run_tel_handler(*remove);
        }
        // BF1: before the log starts, so a wrapped command's output is
        // its own.
        Some(cli::Commands::Done {
            pid,
            no_output,
            title,
            command,
        }) => std::process::exit(done::run(*pid, command, !*no_output, title.as_deref())),
        // BF3: before the log starts too, so a script reads only the answer.
        Some(cli::Commands::Ask { wait, question }) => {
            std::process::exit(pocketdesk_server::asks::run_cli(&question.join(" "), *wait))
        }
        _ => {}
    }

    // Load configuration first. A config.toml that does not parse fails a
    // `--json` command as JSON too.
    let config = match config::Config::load() {
        Ok(config) => config,
        Err(e) if wants_json => {
            local_api::print(&local_api::error(format!("{e:#}")));
            std::process::exit(local_api::exit::FAILED);
        }
        Err(e) => return Err(e.into()),
    };

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
        cli::Commands::PhoneCommand {
            phone,
            name,
            text,
            json,
        } => std::process::exit(local_api::run_command(&phone, &name, &text, json)),
        cli::Commands::Call {
            phone,
            number,
            json,
        } => std::process::exit(local_api::run_call(&phone, &number, json)),
        cli::Commands::Link { phone, url, json } => {
            std::process::exit(local_api::run_link(&phone, &url, json))
        }
        cli::Commands::Clipboard { send, to, json } => {
            std::process::exit(local_api::run_clipboard(send.zip(to.as_deref()), json))
        }
        cli::Commands::Done { .. } | cli::Commands::Ask { .. } => {
            unreachable!("handled before the config is read")
        }
        cli::Commands::Ring { phone, json } => {
            std::process::exit(local_api::run_ring(&phone, json))
        }
        cli::Commands::Quiet {
            phone,
            minutes,
            off,
            json,
        } => {
            // 0 is the wire's "end", asked for with --off; --minutes takes
            // 1 to 1440, so --minutes 0 is a usage error (D2 review).
            let minutes = if off { 0 } else { minutes };
            std::process::exit(local_api::run_quiet(&phone, minutes, json))
        }
        cli::Commands::Pause {
            minutes,
            until_resumed,
            resume,
            json,
        } => {
            let ask = if resume {
                local_api::PauseAsk::Resume
            } else if until_resumed {
                local_api::PauseAsk::Pause(None)
            } else if let Some(minutes) = minutes {
                local_api::PauseAsk::Pause(Some(minutes))
            } else {
                local_api::PauseAsk::Show
            };
            std::process::exit(local_api::run_pause(ask, json))
        }
        cli::Commands::Send {
            phone,
            path,
            wait,
            json,
        } => {
            // Like `bench`, the exit code is what a script reads.
            std::process::exit(local_api::run_send(&phone, &path, wait, json))
        }
        cli::Commands::SendToPhone { paths } => {
            std::process::exit(local_api::run_send_to_phone(&paths))
        }
        cli::Commands::SendMenu { remove } => local_api::run_send_menu(remove),
        cli::Commands::Tel { .. } | cli::Commands::TelHandler { .. } => {
            unreachable!("handled before the config is read")
        }
        cli::Commands::Sent { count, json } => local_api::print_files(true, count, json),
        cli::Commands::Received { count, json } => local_api::print_files(false, count, json),
        cli::Commands::Events { since_start, json } => local_api::watch(since_start, json),
        cli::Commands::BrowserHost { .. } => unreachable!("handled before the config is read"),
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
        cli::Commands::ShareTest {
            computer,
            keys,
            moves,
        } => std::process::exit(shared_input::share_test::run_cli(&computer, keys, moves)),
        cli::Commands::PhoneScreenTest { phone, script } => {
            std::process::exit(shared_input::phone_test::run_cli(&phone, &script))
        }
        cli::Commands::List { json } => service::list_peers(json).await,
        cli::Commands::Watch { interval } => service::watch_peers(interval).await,
        cli::Commands::Capabilities { packets: true, .. } => {
            print!("{}", packets::render_markdown());
            Ok(())
        }
        cli::Commands::Capabilities { json, markdown, .. } => {
            service::print_capabilities(json, markdown).await
        }
        cli::Commands::Connect { peer, port } => {
            let identity = kde::host_identity();
            service::connect_peer(peer, port, &identity).await
        }
        cli::Commands::Pair {
            pin,
            grant,
            someone_else,
            json,
        } => service::pair(pin, grant, someone_else, json).await,
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
        cli::Commands::Install {
            elevated,
            no_firewall,
        } => install_windows::install(elevated, !no_firewall),
        #[cfg(windows)]
        cli::Commands::Uninstall => install_windows::uninstall(),
        #[cfg(windows)]
        cli::Commands::FirewallAdd => install_windows::apply_firewall(true),
        #[cfg(windows)]
        cli::Commands::FirewallRemove => install_windows::apply_firewall(false),
    }
}
