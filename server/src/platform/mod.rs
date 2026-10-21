//! The few OS services the server needs that differ between Linux and Windows,
//! behind one function each so shared code never names an OS API.
//!
//! Plugins with a whole OS-specific backend (privacy, audio control, …) keep
//! their own split; this module is for the small, cross-cutting helpers.

use std::time::Duration;

#[cfg(windows)]
pub(crate) mod audio_windows;
// Built everywhere so its conversions are tested on every OS; only Windows
// reads and writes DIBs.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod clip_image;
#[cfg(windows)]
pub(crate) mod clipboard_windows;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::system32;
#[cfg(all(windows, test))]
pub(crate) use windows::user_value;
#[cfg(windows)]
pub(crate) use windows::{
    delete_user_key, delete_user_value, install_exception_report, set_user_default_value,
    set_user_value,
};

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
/// Linux: `notify-send` as "Pocketdesk", expiring after `expire`. Windows: a
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
            .args(notify_send_args(summary, body, expire))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// `notify-send`'s arguments for a notification. Notification servers read
/// the body as markup, and the text may be a stranger's (a phone's
/// notification, N5a): escaped, and after `--`, so a body like `--wait` is
/// not taken for an option. The expiry is `--expire-time`: libnotify's
/// notify-send rejects `--expire` and shows nothing.
#[cfg_attr(windows, allow(dead_code))]
pub fn notify_send_args(summary: &str, body: &str, expire: Duration) -> Vec<String> {
    vec![
        "--app-name".to_string(),
        "Pocketdesk".to_string(),
        "--expire-time".to_string(),
        expire.as_millis().to_string(),
        "--".to_string(),
        markup_escaped(summary),
        markup_escaped(body),
    ]
}

/// `text` with the characters freedesktop notification markup gives meaning
/// to escaped, so it is shown as written (`<b>`, a link, `&` stay text).
#[cfg_attr(windows, allow(dead_code))]
pub fn markup_escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Show a phone's notification (N5a) as a toast tagged `tag`, so
/// [`remove_tagged`] can take it back (N5c). Windows only; Linux shows and
/// closes them over D-Bus in the notification plugin.
#[cfg(windows)]
pub fn notify_tagged(
    summary: &str,
    body: &str,
    tag: &str,
    answers: Option<&crate::toast_answers::Answerable>,
    app: Option<&str>,
    pictures: &crate::toast_answers::Pictures,
) -> bool {
    windows::notify_tagged(summary, body, tag, answers, app, pictures)
}

/// Take back the toast [`notify_tagged`] showed with `tag`.
#[cfg(windows)]
pub fn remove_tagged(tag: &str) {
    windows::remove_tagged(tag)
}

/// Why the server is shutting down.
pub type ShutdownReason = &'static str;

/// Every way this OS can ask a running server to stop, as one channel.
///
/// Call once, at startup, and keep the receiver: it is fed by Ctrl-C on every
/// OS, by SIGTERM on Unix, and on Windows by the named event [`request_stop`]
/// signals (Windows has no SIGTERM for `pocketdesk stop` to send).
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
    // `pocketdesk stop` sends SIGTERM; without a handler the default action
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

#[cfg(test)]
mod markup_tests {
    /// libnotify's notify-send (0.8 on Ubuntu 24.04) knows `--expire-time`
    /// and rejects `--expire`, which made every Linux notification fail.
    #[test]
    fn notify_send_is_given_options_it_knows() {
        let args = super::notify_send_args("Text <b>", "--wait", std::time::Duration::from_secs(8));
        assert_eq!(
            args,
            [
                "--app-name",
                "Pocketdesk",
                "--expire-time",
                "8000",
                "--",
                "Text &lt;b&gt;",
                "--wait"
            ]
        );
    }

    #[test]
    fn markup_in_a_notification_is_shown_as_text() {
        assert_eq!(
            super::markup_escaped("<a href=\"x\">pay</a> & <b>now</b>"),
            "&lt;a href=\"x\"&gt;pay&lt;/a&gt; &amp; &lt;b&gt;now&lt;/b&gt;"
        );
    }
}
