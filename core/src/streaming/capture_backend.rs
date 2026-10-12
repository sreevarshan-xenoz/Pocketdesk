//! The capture backend a config file can pin. Plain data, kept outside the
//! `capture`-gated module so code that only reads or writes config (the
//! desktop app, through `core::local`) can name it without the capture stack.

/// Which capture backend the server should use. `Auto` (the default) keeps the
/// historical detect-then-fallback behaviour; the explicit variants are a
/// config override (`capture_backend` in config.toml) to pin or reorder the
/// pipeline — e.g. force the portal when screencopy misbehaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureBackend {
    /// Detect the display server and try the best backend, falling back.
    #[default]
    Auto,
    /// wlroots `zwlr_screencopy` (Hyprland): native, no portal grant dialog.
    Screencopy,
    /// XDG Desktop Portal + PipeWire.
    Portal,
    /// Direct X11 `GetImage` (works on Wayland only through XWayland's root).
    X11,
    /// Windows.Graphics.Capture (Windows 10 1903+): monitor or single window.
    Wgc,
    /// DXGI Desktop Duplication: Windows fallback, monitors only, no cursor.
    Dxgi,
}

impl CaptureBackend {
    /// Every backend this build can drive, in `Auto`'s preference order.
    /// `linux-link capabilities` enumerates from here (roadmap item 4).
    #[cfg(not(windows))]
    pub const ALL: &'static [CaptureBackend] = &[
        CaptureBackend::Auto,
        CaptureBackend::Screencopy,
        CaptureBackend::Portal,
        CaptureBackend::X11,
    ];
    /// Every backend this build can drive, in `Auto`'s preference order.
    /// `linux-link capabilities` enumerates from here (roadmap item 4).
    #[cfg(windows)]
    pub const ALL: &'static [CaptureBackend] = &[
        CaptureBackend::Auto,
        CaptureBackend::Wgc,
        CaptureBackend::Dxgi,
    ];

    /// The Linux backends, which a config file may name on any OS.
    pub const LINUX: [CaptureBackend; 3] = [
        CaptureBackend::Screencopy,
        CaptureBackend::Portal,
        CaptureBackend::X11,
    ];

    /// The `capture_backend` value that pins this backend in config.toml — the
    /// serde spelling, so the reported name is always the accepted one.
    pub fn config_key(&self) -> &'static str {
        match self {
            CaptureBackend::Auto => "auto",
            CaptureBackend::Screencopy => "screencopy",
            CaptureBackend::Portal => "portal",
            CaptureBackend::X11 => "x11",
            CaptureBackend::Wgc => "wgc",
            CaptureBackend::Dxgi => "dxgi",
        }
    }
}
