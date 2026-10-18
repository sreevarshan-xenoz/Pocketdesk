//! What each of the server's background loops costs at idle (reliability
//! pass, R6b). R6 measured the idle server at 1.35 % of one core on Windows,
//! double 2026-10-14's 0.68 %, and 0.25 % on Linux, with a dozen pollers
//! added in between. This runs each family of them alone, with no phone and
//! a throwaway state folder, and reads the process's CPU time over a fixed
//! window, so a change to them has a before and after.
//!
//! Ignored (it takes minutes); run it with
//! `cargo test -p pocketdesk-server --release --lib idle_cost -- --ignored --nocapture`.

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::time::{Duration, Instant};

    /// This process's CPU time so far (user and kernel).
    fn cpu_time() -> Duration {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::FILETIME;
            use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
            let (mut created, mut exited, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            // SAFETY: the current process's pseudo-handle, and four FILETIMEs.
            unsafe {
                GetProcessTimes(
                    GetCurrentProcess(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                )
            }
            .expect("GetProcessTimes");
            let ticks =
                |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            // FILETIME counts 100 ns.
            Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
        }
        #[cfg(not(windows))]
        {
            let stat = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
            let fields: Vec<&str> = stat
                .rsplit(')')
                .next()
                .unwrap()
                .split_whitespace()
                .collect();
            // utime and stime, fields 14 and 15 (12 and 13 after the name).
            let ticks: u64 =
                fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap();
            Duration::from_millis(ticks * 10)
        }
    }

    type Loop = fn() -> Pin<Box<dyn Future<Output = ()> + Send>>;

    /// The loops `service::run` starts that wake on a timer, by name.
    fn families() -> Vec<(&'static str, Loop)> {
        vec![
            ("live_sessions (mirror, kicks, control, pairings)", || {
                Box::pin(crate::live_sessions::run_watcher())
            }),
            ("offers (outbox)", || Box::pin(crate::offers::run_watcher())),
            ("links (links_out)", || {
                Box::pin(crate::plugins::links::run_watcher())
            }),
            ("ring_phone (rings_out)", || {
                Box::pin(crate::ring_phone::run_watcher())
            }),
            ("phone_battery (writer)", || {
                Box::pin(crate::plugins::phone_battery::run_writer())
            }),
            ("home_assistant (off: config check)", || {
                Box::pin(crate::home_assistant::run())
            }),
            ("phone_media (presses)", || {
                Box::pin(crate::plugins::phone_media::run_watcher())
            }),
            ("phone_files (requests)", || {
                Box::pin(crate::plugins::phone_files::run_watcher())
            }),
            ("phone_photos (requests)", || {
                Box::pin(crate::plugins::phone_photos::run_watcher())
            }),
            ("virtual_screen (sweeper)", || {
                Box::pin(crate::virtual_screen::run_sweeper())
            }),
            ("sms (outbox)", || {
                Box::pin(crate::plugins::sms::run_watcher())
            }),
            ("notification (answers)", || {
                Box::pin(crate::plugins::notification::run_answers())
            }),
            ("clipboard_push (watcher)", || {
                Box::pin(crate::clipboard_push::run())
            }),
            (
                "kdeconnect discovery (every 2 s, the network every 10 s)",
                || Box::pin(crate::kdeconnect_compat::discovery::run(1716, true)),
            ),
            ("peer_computers (requests)", || {
                Box::pin(async {
                    let certs = pocketdesk_core::streaming::transport::CertManager::new().unwrap();
                    crate::peer_computers::run(std::sync::Arc::new(certs), None).await
                })
            }),
            ("tailscale discovery (every 10 s)", || {
                Box::pin(async {
                    if let Ok(client) = pocketdesk_core::tailscale::TailscaleClient::new() {
                        pocketdesk_core::tailscale::discovery::DiscoveryService::new(client)
                            .run(Duration::from_secs(10))
                            .await;
                    }
                })
            }),
        ]
    }

    /// CPU time over `window` with `run` alone on a fresh runtime, as a share
    /// of one core.
    fn measure(run: Option<Loop>, window: Duration) -> f64 {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        if let Some(run) = run {
            runtime.spawn(run());
        }
        // Let it start (folders made, first reads done) before the window.
        std::thread::sleep(Duration::from_secs(3));
        let (cpu, wall) = (cpu_time(), Instant::now());
        std::thread::sleep(window);
        let used = cpu_time().saturating_sub(cpu).as_secs_f64();
        let share = used / wall.elapsed().as_secs_f64() * 100.0;
        runtime.shutdown_timeout(Duration::from_secs(2));
        share
    }

    #[test]
    #[ignore = "measures for minutes; run by hand, in release"]
    fn what_each_background_loop_costs_at_idle() {
        // Throwaway state and config: the Home Assistant and clipboard loops
        // read the config, and must not act on the owner's.
        let base = std::env::temp_dir().join(format!("pd-idle-cost-{}", std::process::id()));
        let dir = pocketdesk_core::local::paths::use_state_dir(base.join("state"));
        pocketdesk_core::local::paths::use_config_dir(base.join("config"));
        // As the server does, unless asked to measure the pollers alone.
        if std::env::var_os("POCKETDESK_IDLE_NO_WATCHER").is_none() {
            crate::inbox::start(dir);
        }
        let window = Duration::from_secs(
            std::env::var("POCKETDESK_IDLE_WINDOW_S")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30),
        );
        let baseline = measure(None, window);
        println!("{:<52} {:>8}", "loop", "% core");
        println!("{:<52} {baseline:>8.3}", "(an empty runtime)");
        let mut total = 0.0;
        for (name, run) in families() {
            let share = (measure(Some(run), window) - baseline).max(0.0);
            total += share;
            println!("{name:<52} {share:>8.3}");
        }
        println!("{:<52} {total:>8.3}", "sum of the loops");
    }
}
