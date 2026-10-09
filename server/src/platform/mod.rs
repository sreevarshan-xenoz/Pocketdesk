//! The few OS services the server needs that differ between Linux and Windows,
//! behind one function each so shared code never names an OS API.
//!
//! Plugins with a whole OS-specific backend (privacy, audio control, …) keep
//! their own split; this module is for the small, cross-cutting helpers.

use std::time::Duration;

#[cfg(windows)]
pub(crate) mod audio_windows;
#[cfg(windows)]
pub(crate) mod clipboard_windows;
#[cfg(windows)]
mod windows;

/// One-time process setup, before anything enumerates monitors, windows or
/// injects input.
///
/// On Windows this declares per-monitor-v2 DPI awareness: without it the
/// process is handed DPI-virtualized coordinates, so monitor rects, window
/// bounds and absolute input all drift on any display not at 100% scale.
pub fn init_process() {
    #[cfg(windows)]
    windows::init_process();
}

/// Leave the console this process was started with, so a background server
/// launched at logon shows no window. No-op off Windows.
pub fn detach_console() {
    #[cfg(windows)]
    windows::detach_console();
}

/// Show a desktop notification. Blocking; returns whether one was shown, so
/// callers can fall back to the log (the PIN, for one, must reach the user
/// somehow).
///
/// Linux: `notify-send` as "Linux Link", expiring after `expire`. Windows: a
/// toast under the server's own AppUserModelID (registered per user on first
/// use; Windows keeps toasts in the Action Center rather than expiring them).
pub fn notify(summary: &str, body: &str, expire: Duration) -> bool {
    #[cfg(windows)]
    {
        let _ = expire;
        windows::notify(summary, body)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("notify-send")
            .args([
                "--app-name",
                "Linux Link",
                "--expire",
                &expire.as_millis().to_string(),
                summary,
                body,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Why the server is shutting down.
pub type ShutdownReason = &'static str;

/// Every way this OS can ask a running server to stop, as one channel.
///
/// Call once, at startup, and keep the receiver: it is fed by Ctrl-C on every
/// OS, by SIGTERM on Unix, and on Windows by the named event [`request_stop`]
/// signals (Windows has no SIGTERM for `linux-link stop` to send).
pub fn shutdown_requests() -> tokio::sync::mpsc::Receiver<ShutdownReason> {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let ctrl_c = tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = ctrl_c.send("Ctrl-C").await;
        }
    });
    #[cfg(windows)]
    windows::listen_for_stop_event(tx);
    // `linux-link stop` sends SIGTERM; without a handler the default action
    // killed the server before its PID-file guard could run.
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                if term.recv().await.is_some() {
                    let _ = tx.send("SIGTERM").await;
                }
            }
            Err(e) => tracing::warn!("cannot listen for SIGTERM: {e}"),
        }
    });
    rx
}

/// Terminate the server running as `pid` when it cannot be asked: Windows
/// only, and only if `pid` really is this program. `Ok(false)` means refused.
#[cfg(windows)]
pub fn force_stop(pid: u32) -> std::io::Result<bool> {
    windows::force_stop(pid)
}

/// Ask the server running as `pid` to shut down gracefully. Returns `false`
/// when no graceful path exists, so the caller can fall back to force.
pub async fn request_stop(pid: u32) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        Ok(windows::signal_stop_event(pid))
    }
    #[cfg(not(windows))]
    {
        let status = tokio::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .await?;
        Ok(status.success())
    }
}
