//! The few OS services the server needs that differ between Linux and Windows,
//! behind one function each so shared code never names an OS API.
//!
//! Plugins with a whole OS-specific backend (privacy, audio control, …) keep
//! their own split; this module is for the small, cross-cutting helpers.

use std::time::Duration;

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
