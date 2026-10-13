//! The server's `config.toml`: load, and save without losing what is there.
//!
//! The server reads it once at startup; the desktop app edits it. A save
//! touches only the keys whose values changed, through `toml_edit`, so a
//! person's comments and keys this version does not know survive, and saving
//! an unchanged config rewrites nothing.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths;
use crate::DEFAULT_CONTROL_PORT;
use crate::streaming::client::DEFAULT_STREAMING_PORT;
use crate::streaming::{CaptureBackend, VideoQualityPreset};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_relay_device: Option<String>,
    /// Announce this computer on the local network over mDNS, so phones on
    /// the same network list it (roadmap F6). Read at server start.
    #[serde(default = "default_lan_discovery")]
    pub lan_discovery: bool,
}

fn default_lan_discovery() -> bool {
    true
}

fn default_pairing_required() -> bool {
    true
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

impl Default for Config {
    fn default() -> Self {
        Self {
            control_port: DEFAULT_CONTROL_PORT,
            streaming_port: DEFAULT_STREAMING_PORT,
            log_level: default_log_level(),
            video_quality: VideoQualityPreset::Balanced,
            pairing_required: true,
            allow_hevc: false,
            capture_backend: CaptureBackend::Auto,
            mic_relay_device: None,
            lan_discovery: true,
        }
    }
}

/// Why a config could not be read or written.
#[derive(Debug)]
pub enum ConfigError {
    Io { path: PathBuf, source: io::Error },
    Parse { path: PathBuf, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse { path, message } => {
                write!(f, "invalid TOML in {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { .. } => None,
        }
    }
}

impl Config {
    /// `<config dir>/pocketdesk/config.toml`.
    pub fn path() -> io::Result<PathBuf> {
        Ok(paths::config_dir()?.join(paths::CONFIG_FILE))
    }

    /// The config at its usual place; defaults when there is no file.
    pub fn load() -> Result<Self, ConfigError> {
        let path = Self::path().map_err(|source| ConfigError::Io {
            path: PathBuf::from(paths::CONFIG_FILE),
            source,
        })?;
        Self::load_from(&path)
    }

    /// Save to the usual place (see [`Config::save_to`]).
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::path().map_err(|source| ConfigError::Io {
            path: PathBuf::from(paths::CONFIG_FILE),
            source,
        })?;
        self.save_to(&path)
    }

    /// The config at `path`; defaults when the file does not exist. A file
    /// that exists but does not parse is an error, never silently defaults.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&raw).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Write this config to `path`, changing only keys whose value differs
    /// from what the file already says. Writes nothing when nothing changed.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let parse_error = |message: String| ConfigError::Parse {
            path: path.to_path_buf(),
            message,
        };
        let io_error = |source: io::Error| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        };
        let original = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => return Err(io_error(source)),
        };
        let mut doc: toml_edit::DocumentMut = original
            .parse()
            .map_err(|e: toml_edit::TomlError| parse_error(e.to_string()))?;
        let wanted = toml::Table::try_from(self).map_err(|e| parse_error(e.to_string()))?;
        let current: toml::Table =
            toml::from_str(&original).map_err(|e| parse_error(e.to_string()))?;
        // What the file means today, defaults filled in: a key the file leaves
        // out but whose default is the wanted value is not a change.
        let effective: Config =
            toml::from_str(&original).map_err(|e| parse_error(e.to_string()))?;
        let effective =
            toml::Table::try_from(&effective).map_err(|e| parse_error(e.to_string()))?;
        for (key, value) in &wanted {
            if effective.get(key) == Some(value) {
                continue;
            }
            // Render the one value through toml, then hand it to toml_edit,
            // so the rest of the document keeps its exact formatting.
            let rendered = toml::Value::to_string(value);
            let item: toml_edit::Value = rendered
                .parse()
                .map_err(|e: toml_edit::TomlError| parse_error(e.to_string()))?;
            match doc.get_mut(key).and_then(|i| i.as_value_mut()) {
                Some(existing) => {
                    let decor = existing.decor().clone();
                    *existing = item;
                    *existing.decor_mut() = decor;
                }
                None => doc[key.as_str()] = toml_edit::Item::Value(item),
            }
        }
        // A key this config no longer sets (an option cleared to `None`) goes.
        for key in current.keys() {
            if Self::KNOWN_KEYS.contains(&key.as_str()) && !wanted.contains_key(key) {
                doc.remove(key);
            }
        }
        let updated = doc.to_string();
        if updated == original {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        crate::secure_fs::write_private(path, updated.as_bytes()).map_err(io_error)
    }

    /// Every key this version reads; others in the file are left alone.
    const KNOWN_KEYS: &'static [&'static str] = &[
        "control_port",
        "streaming_port",
        "log_level",
        "video_quality",
        "pairing_required",
        "allow_hevc",
        "capture_backend",
        "mic_relay_device",
        "lan_discovery",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::CaptureBackend;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let path = temp("missing");
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.control_port, 1716);
        assert!(config.pairing_required);
        assert!(config.lan_discovery, "announced on the network by default");
    }

    #[test]
    fn saving_changes_only_the_edited_keys_and_keeps_comments() {
        let path = temp("edit");
        let original = "# my notes
control_port = 1800 # keep
future_key = \"x\"
";
        std::fs::write(&path, original).unwrap();
        let mut config = Config::load_from(&path).unwrap();
        config.save_to(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "an unchanged save is byte-identical"
        );
        config.allow_hevc = true;
        config.save_to(&path).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# my notes") && saved.contains("future_key = \"x\""));
        assert!(saved.contains("control_port = 1800 # keep"));
        assert!(saved.contains("allow_hevc = true"));
        assert_eq!(Config::load_from(&path).unwrap().control_port, 1800);
    }

    #[test]
    fn turning_lan_discovery_off_is_saved_and_read_back() {
        let path = temp("lan");
        let mut config = Config::load_from(&path).unwrap();
        config.lan_discovery = false;
        config.save_to(&path).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("lan_discovery = false")
        );
        assert!(!Config::load_from(&path).unwrap().lan_discovery);
    }

    #[test]
    fn invalid_toml_is_an_error_not_defaults() {
        let path = temp("bad");
        std::fs::write(&path, "control_port = [").unwrap();
        assert!(matches!(
            Config::load_from(&path),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn capture_backend_defaults_to_auto_and_parses_override() {
        let default: Config = toml::from_str("").unwrap();
        assert!(matches!(default.capture_backend, CaptureBackend::Auto));
        let pinned: Config = toml::from_str("capture_backend = \"portal\"").unwrap();
        assert!(matches!(pinned.capture_backend, CaptureBackend::Portal));
        assert!(toml::from_str::<Config>("capture_backend = \"v4l2\"").is_err());
    }
}
