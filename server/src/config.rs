use anyhow::{Context, Result};
use linux_link_core::DEFAULT_CONTROL_PORT;
use linux_link_core::streaming::CaptureBackend;
use linux_link_core::streaming::VideoQualityPreset;
use linux_link_core::streaming::client::DEFAULT_STREAMING_PORT;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    #[serde(default = "default_control_port")]
    pub control_port: u16,
    #[serde(default = "default_streaming_port")]
    pub streaming_port: u16,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub video_quality: VideoQualityPreset,
    /// Require PIN pairing on the control channel before plugin packets are
    /// dispatched (R3 Tier-2 #11b). Set false in config.toml for open-LAN use.
    #[serde(default = "default_pairing_required")]
    pub pairing_required: bool,
    /// Allow H.265/HEVC when a connecting client declares it can decode it
    /// (R4 C1). Off by default: sessions stay H.264. Turning this on assumes
    /// the box has a working HEVC encoder (libx265 or a HW hevc path) — if
    /// opening it fails at runtime the session still falls back (C2).
    #[serde(default)]
    pub allow_hevc: bool,
    /// Capture backend selection (R4 B3). `"auto"` (default) detects the
    /// display server and falls back (screencopy → portal on Wayland, X11 on
    /// bare X11). `"screencopy"`, `"portal"` or `"x11"` pins the pipeline —
    /// useful to force the portal when screencopy misbehaves, or to stay on
    /// XWayland capture.
    #[serde(default)]
    pub capture_backend: CaptureBackend,
    /// Windows only: the playback device the phone's mic is played into, by
    /// (part of) its name. Default "CABLE Input" — a stock VB-CABLE install.
    /// Apps then record from the cable's other end ("CABLE Output").
    #[serde(default)]
    pub mic_relay_device: Option<String>,
}

fn default_pairing_required() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            control_port: DEFAULT_CONTROL_PORT,
            streaming_port: DEFAULT_STREAMING_PORT,
            log_level: "info".to_string(),
            video_quality: VideoQualityPreset::Balanced,
            pairing_required: true,
            allow_hevc: false,
            capture_backend: CaptureBackend::Auto,
            mic_relay_device: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let config_path = dirs::config_dir()
            .context("unable to determine config directory")?
            .join("linux-link")
            .join("config.toml");

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let raw = std::fs::read_to_string(&config_path)
            .with_context(|| format!("failed reading {}", config_path.display()))?;
        let parsed: Self = toml::from_str(&raw)
            .with_context(|| format!("invalid TOML in {}", config_path.display()))?;
        Ok(parsed)
    }
}

const fn default_control_port() -> u16 {
    DEFAULT_CONTROL_PORT
}

const fn default_streaming_port() -> u16 {
    DEFAULT_STREAMING_PORT
}

fn default_log_level() -> String {
    "info".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_backend_defaults_to_auto_and_parses_override() {
        let default: Config = toml::from_str("").unwrap();
        assert!(matches!(default.capture_backend, CaptureBackend::Auto));

        let pinned: Config = toml::from_str("capture_backend = \"portal\"").unwrap();
        assert!(matches!(pinned.capture_backend, CaptureBackend::Portal));

        let bad = toml::from_str::<Config>("capture_backend = \"v4l2\"");
        assert!(bad.is_err(), "unknown backend must fail to parse");
    }
}
