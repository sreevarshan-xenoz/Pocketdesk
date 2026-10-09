//! The few OS services the server needs that differ between Linux and Windows,
//! behind one function each so shared code never names an OS API.
//!
//! Plugins with a whole OS-specific backend (privacy, audio control, …) keep
//! their own split; this module is for the small, cross-cutting helpers.

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
