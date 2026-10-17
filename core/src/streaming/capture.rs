//! Screen capture backend selection.
//!
//! [`start_capture_auto`] walks the pure [`capture_attempts`] decision table
//! for the detected display server and starts the first backend that comes up.
//! Every backend pushes BGRA [`VideoFrame`]s into the same channel. The Linux
//! backends live in `capture_linux.rs` (portal + X11) and
//! `capture_screencopy.rs` (wlroots).

use anyhow::{Result, bail};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(target_os = "linux")]
use super::capture_linux::{
    check_x11_available, start_capture, start_x11_capture, try_portal_available,
};
use super::{StreamingConfig, VideoFrame};

/// PipeWire frame capture session.
///
/// Manages the lifetime of the background PipeWire thread.
/// When dropped, the capture task is cancelled and cleaned up.
pub struct CaptureSession {
    config: StreamingConfig,
    cancel: CancellationToken,
}

impl CaptureSession {
    /// Create a new capture session.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn new(config: StreamingConfig, cancel: CancellationToken) -> Self {
        Self { config, cancel }
    }

    /// Get the capture configuration.
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        info!("CaptureSession dropped, cancelling capture");
        self.cancel.cancel();
    }
}

/// Capture rect for `index` in the xcap enumeration — the *same* ordering the
/// server's monitors plugin reports to clients, so a phone-side picker index
/// and the server capture target agree. `None` when enumeration fails or the
/// index is out of range; callers then keep the legacy whole-screen behaviour.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn monitor_rect(index: u32) -> Option<(i32, i32, u32, u32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors.get(index as usize)?;
    Some((
        monitor.x().ok()?,
        monitor.y().ok()?,
        monitor.width().ok()?,
        monitor.height().ok()?,
    ))
}

/// The bounding box of every monitor: the whole desktop layout, which is
/// the space Windows' absolute `SendInput` (`MOUSEEVENTF_VIRTUALDESK`)
/// normalizes over.
pub(super) fn layout_rect() -> Option<(i32, i32, u32, u32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let rects: Vec<(i32, i32, i32, i32)> = monitors
        .iter()
        .filter_map(|m| {
            let (x, y) = (m.x().ok()?, m.y().ok()?);
            Some((
                x,
                y,
                x + m.width().ok()? as i32,
                y + m.height().ok()? as i32,
            ))
        })
        .collect();
    let left = rects.iter().map(|r| r.0).min()?;
    let top = rects.iter().map(|r| r.1).min()?;
    let right = rects.iter().map(|r| r.2).max()?;
    let bottom = rects.iter().map(|r| r.3).max()?;
    Some((left, top, (right - left) as u32, (bottom - top) as u32))
}

/// Which display server is currently running
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    /// Wayland compositor (via XDG Desktop Portal / PipeWire)
    Wayland,
    /// X11 server (via XDG Desktop Portal or direct X11)
    X11,
    /// Windows desktop (DWM). Decided at compile time, not from the
    /// environment: every interactive Windows session has one.
    Windows,
    /// No display server detected
    None,
}

/// Detect the active display server from environment variables.
pub fn detect_display_server() -> DisplayServer {
    if cfg!(windows) {
        return DisplayServer::Windows;
    }

    // Wayland is indicated by WAYLAND_DISPLAY being set
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        debug!("Detected Wayland display server");
        return DisplayServer::Wayland;
    }

    // X11 is indicated by DISPLAY being set
    if std::env::var("DISPLAY").is_ok() {
        debug!("Detected X11 display server");
        return DisplayServer::X11;
    }

    // XDG_CURRENT_DESKTOP may indicate a desktop environment
    // Check if we're on a headless system or SSH without X forwarding
    if std::env::var("XDG_CURRENT_DESKTOP").is_ok() {
        // This could be a pure Wayland session without WAYLAND_DISPLAY set explicitly
        debug!("Desktop environment detected via XDG_CURRENT_DESKTOP, assuming Wayland");
        return DisplayServer::Wayland;
    }

    warn!("No display server detected (neither WAYLAND_DISPLAY nor DISPLAY is set)");
    DisplayServer::None
}

/// Check if screen capture is available on this system
pub async fn check_availability() -> Result<bool> {
    match detect_display_server() {
        #[cfg(target_os = "linux")]
        DisplayServer::Wayland => {
            // Check PipeWire portal availability by trying to create a session
            let available = try_portal_available().await;
            if available {
                info!("Screen capture available via PipeWire/XDP");
            } else {
                warn!("Wayland detected but XDG Desktop Portal not available");
            }
            Ok(available)
        }
        #[cfg(target_os = "linux")]
        DisplayServer::X11 => {
            // Check X11 availability — try connecting to the display
            let x11_available = check_x11_available();
            if x11_available {
                info!("Screen capture available via X11");
            } else {
                warn!("X11 detected but display connection failed");
            }
            Ok(x11_available)
        }
        #[cfg(not(target_os = "linux"))]
        DisplayServer::Wayland | DisplayServer::X11 => Ok(false),
        DisplayServer::Windows => {
            debug!("Screen capture not available — this build has no Windows capture backend");
            Ok(false)
        }
        DisplayServer::None => {
            debug!("Screen capture not available — no display server");
            Ok(false)
        }
    }
}

pub use super::capture_backend::CaptureBackend;

/// Auto-detect display server and start the appropriate capture method.
///
/// With `CaptureBackend::Auto` this tries screencopy first on wlroots
/// compositors, then the PipeWire portal, then X11 — falling back on any setup
/// failure reported before the first frame. An explicit backend pins the
/// pipeline (single attempt); a runtime failure then surfaces rather than
/// silently switching. Split from [`capture_attempts`] so the ordering is
/// unit-testable without touching real capture.
#[allow(clippy::too_many_arguments)]
// Off Linux every parameter feeds a backend that this build does not have yet.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub async fn start_capture_auto(
    config: StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
    window_rx: tokio::sync::watch::Receiver<u64>,
    window_mode: std::sync::Arc<std::sync::atomic::AtomicBool>,
    backend: CaptureBackend,
) -> Result<CaptureSession> {
    let attempts = capture_attempts(backend, detect_display_server())?;
    let mut last_err: Option<anyhow::Error> = None;
    for backend_tried in attempts {
        // Each attempt has a token of its own under the session's (R6g
        // review): a backend that fails cancels its token, which had been
        // the session's, so the next backend started already cancelled and
        // the fallback never worked.
        let attempt = cancel.child_token();
        let result = match backend_tried {
            CaptureBackend::Auto => continue,
            // window_rx/window_mode are cheap to clone (watch Receiver + Arc)
            // and only the screencopy backend uses them.
            #[cfg(target_os = "linux")]
            CaptureBackend::Screencopy => super::capture_screencopy::start_screencopy_capture(
                config.clone(),
                frame_tx.clone(),
                attempt.clone(),
                window_rx.clone(),
                window_mode.clone(),
            ),
            #[cfg(target_os = "linux")]
            CaptureBackend::Portal => {
                start_capture(config.clone(), frame_tx.clone(), attempt.clone()).await
            }
            #[cfg(target_os = "linux")]
            CaptureBackend::X11 => {
                start_x11_capture(config.clone(), frame_tx.clone(), attempt.clone()).await
            }
            // `capture_attempts` never lists a Linux backend off Linux; the arm
            // only keeps the match exhaustive.
            #[cfg(not(target_os = "linux"))]
            CaptureBackend::Screencopy | CaptureBackend::Portal | CaptureBackend::X11 => {
                Err(anyhow::anyhow!("{backend_tried:?} capture is Linux-only"))
            }
            #[cfg(windows)]
            CaptureBackend::Wgc => super::capture_wgc::start_wgc_capture(
                config.clone(),
                frame_tx.clone(),
                attempt.clone(),
                window_rx.clone(),
                window_mode.clone(),
            ),
            #[cfg(windows)]
            CaptureBackend::Dxgi => super::capture_dxgi::start_dxgi_capture(
                config.clone(),
                frame_tx.clone(),
                attempt.clone(),
                window_rx.clone(),
                window_mode.clone(),
            ),
            #[cfg(not(windows))]
            CaptureBackend::Wgc | CaptureBackend::Dxgi => {
                Err(anyhow::anyhow!("{backend_tried:?} capture is Windows-only"))
            }
        };
        match result {
            Ok(session) => {
                info!("Capture started via backend {backend_tried:?}");
                // A capture that dies once started ends the session, as it
                // did when it held the session's token.
                tokio::spawn(end_session_with_capture(attempt, cancel.clone()));
                return Ok(session);
            }
            Err(e) => {
                info!("Capture backend {backend_tried:?} unavailable ({e:#}); trying next");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::Error::msg("no capture backend available")))
}

/// When a started capture's token is cancelled (it died, or the session is
/// over), the session's is too.
async fn end_session_with_capture(capture: CancellationToken, session: CancellationToken) {
    capture.cancelled().await;
    session.cancel();
}

/// Pure decision table: which backends to try, in order, for a configured
/// `backend` given the `detected` display server. `Err` means the combination
/// cannot work. Keeping this separate from [`start_capture_auto`] makes the
/// ordering testable without spawning real capture.
pub fn capture_attempts(
    backend: CaptureBackend,
    detected: DisplayServer,
) -> Result<Vec<CaptureBackend>> {
    if detected == DisplayServer::Windows {
        // A config.toml written on Linux must still load here; it fails at
        // capture with a reason instead of at parse.
        return match backend {
            CaptureBackend::Auto => Ok(vec![CaptureBackend::Wgc, CaptureBackend::Dxgi]),
            CaptureBackend::Wgc => Ok(vec![CaptureBackend::Wgc]),
            CaptureBackend::Dxgi => Ok(vec![CaptureBackend::Dxgi]),
            other => bail!(
                "capture_backend = \"{}\" is a Linux backend and is not available on Windows",
                other.config_key()
            ),
        };
    }
    let wayland = detected == DisplayServer::Wayland;
    let x11 = detected == DisplayServer::X11;
    let list = match backend {
        CaptureBackend::Auto => {
            if wayland {
                vec![CaptureBackend::Screencopy, CaptureBackend::Portal]
            } else if x11 {
                vec![CaptureBackend::X11]
            } else {
                bail!("No display server detected — cannot start capture");
            }
        }
        CaptureBackend::Screencopy => {
            if wayland {
                vec![CaptureBackend::Screencopy]
            } else {
                bail!("capture_backend = \"screencopy\" requires a Wayland compositor");
            }
        }
        CaptureBackend::Portal => {
            if wayland || x11 {
                // The portal also fronts X11 sessions; an explicit request is
                // honoured and a real portal failure surfaces at runtime.
                vec![CaptureBackend::Portal]
            } else {
                bail!("capture_backend = \"portal\" requires a desktop session");
            }
        }
        CaptureBackend::X11 => {
            if x11 || wayland {
                // On Wayland this captures only XWayland's root window.
                vec![CaptureBackend::X11]
            } else {
                bail!("capture_backend = \"x11\" requires an X11 (or XWayland) display");
            }
        }
        CaptureBackend::Wgc | CaptureBackend::Dxgi => bail!(
            "capture_backend = \"{}\" is a Windows backend and is not available here",
            backend.config_key()
        ),
    };
    Ok(list)
}

/// Create a test frame for development/testing
#[cfg(test)]
pub fn create_test_frame(width: u32, height: u32) -> VideoFrame {
    let stride = width * 4; // BGRA = 4 bytes per pixel
    let data = vec![0u8; (stride * height) as usize];

    VideoFrame {
        data,
        width,
        height,
        stride,
        timestamp: std::time::Instant::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_failed_attempt_leaves_the_session_and_a_started_capture_ends_it() {
        let session = CancellationToken::new();
        // A backend that fails cancels its own token only.
        let failed = session.child_token();
        failed.cancel();
        assert!(!session.is_cancelled(), "the next backend starts live");
        // One that started and then died ends the session.
        let started = session.child_token();
        let watching = tokio::spawn(end_session_with_capture(started.clone(), session.clone()));
        started.cancel();
        watching.await.unwrap();
        assert!(session.is_cancelled());
    }

    #[test]
    fn test_create_test_frame() {
        let frame = create_test_frame(1920, 1080);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.stride, 1920 * 4);
        assert_eq!(frame.data.len(), (1920 * 4 * 1080) as usize);
    }

    #[test]
    fn test_streaming_config_default() {
        let config = StreamingConfig::default();
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.fps, 60);
    }

    #[test]
    fn auto_ordering_by_display_server() {
        use CaptureBackend::{Portal, Screencopy, X11};
        // Wayland: screencopy first, portal fallback.
        assert_eq!(
            capture_attempts(CaptureBackend::Auto, DisplayServer::Wayland).unwrap(),
            vec![Screencopy, Portal]
        );
        // Bare X11: GetImage only.
        assert_eq!(
            capture_attempts(CaptureBackend::Auto, DisplayServer::X11).unwrap(),
            vec![X11]
        );
        // Headless: nothing to try.
        assert!(capture_attempts(CaptureBackend::Auto, DisplayServer::None).is_err());
    }

    #[test]
    fn explicit_backend_pins_single_attempt() {
        use CaptureBackend::{Portal, Screencopy, X11};
        assert_eq!(
            capture_attempts(Screencopy, DisplayServer::Wayland).unwrap(),
            vec![Screencopy]
        );
        assert_eq!(
            capture_attempts(Portal, DisplayServer::X11).unwrap(),
            vec![Portal]
        );
        // X11 is allowed on Wayland (XWayland root), documented caveat.
        assert_eq!(
            capture_attempts(X11, DisplayServer::Wayland).unwrap(),
            vec![X11]
        );
    }

    #[test]
    fn linux_backends_are_refused_on_windows_with_their_config_name() {
        for backend in CaptureBackend::LINUX {
            let err = capture_attempts(backend, DisplayServer::Windows).unwrap_err();
            assert!(
                err.to_string().contains(backend.config_key()),
                "{backend:?}: {err}"
            );
        }
    }

    #[test]
    fn windows_auto_tries_wgc_then_dxgi_and_a_pin_holds() {
        use CaptureBackend::{Auto, Dxgi, Wgc};
        assert_eq!(
            capture_attempts(Auto, DisplayServer::Windows).unwrap(),
            vec![Wgc, Dxgi]
        );
        assert_eq!(
            capture_attempts(Wgc, DisplayServer::Windows).unwrap(),
            vec![Wgc]
        );
        assert_eq!(
            capture_attempts(Dxgi, DisplayServer::Windows).unwrap(),
            vec![Dxgi]
        );
    }

    #[test]
    fn windows_backends_are_refused_off_windows_with_their_config_name() {
        for backend in [CaptureBackend::Wgc, CaptureBackend::Dxgi] {
            for server in [
                DisplayServer::Wayland,
                DisplayServer::X11,
                DisplayServer::None,
            ] {
                let err = capture_attempts(backend, server).unwrap_err();
                assert!(
                    err.to_string().contains(backend.config_key()),
                    "{backend:?} on {server:?}: {err}"
                );
            }
        }
    }

    #[test]
    fn impossible_backend_combinations_error() {
        use CaptureBackend::{Portal, Screencopy, X11};
        assert!(capture_attempts(Screencopy, DisplayServer::X11).is_err());
        assert!(capture_attempts(Screencopy, DisplayServer::None).is_err());
        assert!(capture_attempts(Portal, DisplayServer::None).is_err());
        assert!(capture_attempts(X11, DisplayServer::None).is_err());
    }
}
