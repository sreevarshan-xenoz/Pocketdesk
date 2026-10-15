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
    /// Your own relays for phones away from home (`pocketdesk relay`), as
    /// URLs (`https://relay.example.org`, or `http://<address>:3340`). Empty
    /// uses n0's public relays. Read at server start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_urls: Vec<String>,
    /// What this computer publishes on the Mainline DHT under its WAN id, so
    /// a phone away from home finds it after its relay or address changed
    /// (roadmap N3). Read at server start.
    #[serde(default)]
    pub dht_publish: DhtPublish,
    /// Show the phones' own notifications here, when a phone shares them
    /// (roadmap N5a). Each phone's Notifications permission still applies.
    #[serde(default = "default_true")]
    pub phone_notifications: bool,
    /// The most notifications shown from one phone in a minute.
    #[serde(default = "default_notifications_per_minute")]
    pub notifications_per_minute: u32,
    /// A notification for each text a phone forwards (roadmap N5b).
    #[serde(default = "default_true")]
    pub sms_notify: bool,
    /// How many texts the Messages history keeps.
    #[serde(default = "default_sms_keep")]
    pub sms_keep: u32,
    /// Open a link a phone hands over in the default browser (roadmap T8a);
    /// off, it is shown as a notification instead.
    #[serde(default = "default_true")]
    pub open_links: bool,
    /// A notification when a phone in a session reports a low battery
    /// (roadmap T8b).
    #[serde(default = "default_true")]
    pub phone_battery_alerts: bool,
    /// Tell paired phones how to wake this computer: its adapter's MAC and
    /// broadcast address (roadmap N6).
    #[serde(default = "default_true")]
    pub wake_announce: bool,
    /// The UDP port the home router forwards to this computer for a wake
    /// from outside, told to phones; 0 when none is set up.
    #[serde(default)]
    pub wake_port: u16,
    /// With several phones connected, who drives (roadmap N7).
    #[serde(default)]
    pub input_control: InputControlMode,
    /// Seconds without input after which the driving phone lets go.
    #[serde(default = "default_input_idle_secs")]
    pub input_idle_secs: u32,
    /// Hours an unfinished file from a phone is kept so a retry continues
    /// it (roadmap T6); 0 keeps none, and a retry starts over.
    #[serde(default = "default_transfer_keep_hours")]
    pub transfer_keep_hours: u32,
}

fn default_transfer_keep_hours() -> u32 {
    24
}

fn default_input_idle_secs() -> u32 {
    10
}

/// Who drives when several phones are connected (roadmap N7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputControlMode {
    /// One phone at a time: the first to send input, until it goes quiet.
    #[default]
    One,
    /// Every phone with the Input permission.
    Shared,
}

fn default_true() -> bool {
    true
}

fn default_notifications_per_minute() -> u32 {
    20
}

fn default_sms_keep() -> u32 {
    1000
}

/// What the desktop's signed DHT record holds (roadmap N3). Private, LAN and
/// tailnet addresses are never in it, whatever the value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DhtPublish {
    /// The relay URLs only.
    #[default]
    Relay,
    /// The relay URLs and this computer's public IP addresses.
    Addresses,
    /// Nothing is published.
    Off,
}

impl DhtPublish {
    /// The config value, as written in config.toml and `wan_status.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::Addresses => "addresses",
            Self::Off => "off",
        }
    }
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
            relay_urls: Vec::new(),
            dht_publish: DhtPublish::Relay,
            phone_notifications: true,
            notifications_per_minute: default_notifications_per_minute(),
            sms_notify: true,
            sms_keep: default_sms_keep(),
            open_links: true,
            phone_battery_alerts: true,
            wake_announce: true,
            wake_port: 0,
            input_control: InputControlMode::One,
            input_idle_secs: default_input_idle_secs(),
            transfer_keep_hours: default_transfer_keep_hours(),
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
        "relay_urls",
        "dht_publish",
        "phone_notifications",
        "notifications_per_minute",
        "sms_notify",
        "open_links",
        "phone_battery_alerts",
        "sms_keep",
        "wake_announce",
        "wake_port",
        "input_control",
        "input_idle_secs",
        "transfer_keep_hours",
    ];
}

/// Why `url` cannot be a relay in `relay_urls`, or `None` when it can.
pub fn relay_url_problem(url: &str) -> Option<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let Some(rest) = ["http://", "https://"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &url[scheme.len()..]))
    else {
        return Some(format!(
            "{url:?} is not an http:// or https:// address of a relay"
        ));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.starts_with(':') {
        return Some(format!("{url:?} names no host"));
    }
    None
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
    fn a_relay_url_needs_http_or_https_and_a_host() {
        for good in [
            "https://relay.example.org",
            "http://192.0.2.10:3340",
            "http://[2001:db8::1]:3340/",
        ] {
            assert_eq!(relay_url_problem(good), None, "{good}");
        }
        for bad in [
            "relay.example.org:3340",
            "localhost:3340",
            "ftp://relay.example.org",
            "https://",
            "http://:3340",
            "",
        ] {
            assert!(relay_url_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn your_own_relays_are_read_from_the_config() {
        let path = temp("relays");
        std::fs::write(&path, "relay_urls = [\"https://relay.example.org\"]\n").unwrap();
        assert_eq!(
            Config::load_from(&path).unwrap().relay_urls,
            vec!["https://relay.example.org".to_string()]
        );
        assert!(
            Config::default().relay_urls.is_empty(),
            "n0's relays unless set"
        );
    }

    #[test]
    fn what_the_dht_record_holds_is_read_from_the_config() {
        assert_eq!(
            Config::default().dht_publish,
            DhtPublish::Relay,
            "relay addresses only, unless set"
        );
        for (text, want) in [
            ("relay", DhtPublish::Relay),
            ("addresses", DhtPublish::Addresses),
            ("off", DhtPublish::Off),
        ] {
            let path = temp(&format!("dht-{text}"));
            std::fs::write(&path, format!("dht_publish = \"{text}\"\n")).unwrap();
            assert_eq!(Config::load_from(&path).unwrap().dht_publish, want);
        }
        let path = temp("dht-bad");
        std::fs::write(&path, "dht_publish = \"everything\"\n").unwrap();
        assert!(
            Config::load_from(&path).is_err(),
            "an unknown value is refused"
        );
    }

    #[test]
    fn several_phones_default_to_one_driver_and_save() {
        let config = Config::default();
        assert_eq!(config.input_control, InputControlMode::One);
        assert_eq!(config.input_idle_secs, 10);
        let path = temp("input-control");
        let mut config = Config::load_from(&path).unwrap();
        config.input_control = InputControlMode::Shared;
        config.input_idle_secs = 30;
        config.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.input_control, InputControlMode::Shared);
        assert_eq!(back.input_idle_secs, 30);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("input_control = \"shared\"")
        );
    }

    #[test]
    fn unfinished_transfers_are_kept_a_day_by_default_and_the_choice_saves() {
        assert_eq!(Config::default().transfer_keep_hours, 24);
        let path = temp("transfer-keep");
        let mut config = Config::load_from(&path).unwrap();
        config.transfer_keep_hours = 0;
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap().transfer_keep_hours, 0);
    }

    #[test]
    fn wake_settings_default_to_announcing_with_no_outside_port_and_save() {
        let config = Config::default();
        assert!(config.wake_announce);
        assert_eq!(config.wake_port, 0);
        let path = temp("wake-settings");
        let mut config = Config::load_from(&path).unwrap();
        config.wake_announce = false;
        config.wake_port = 40009;
        config.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert!(!back.wake_announce);
        assert_eq!(back.wake_port, 40009);
    }

    #[test]
    fn low_battery_warnings_are_on_by_default_and_the_choice_saves() {
        assert!(Config::default().phone_battery_alerts);
        let path = temp("battery-alerts");
        let mut config = Config::load_from(&path).unwrap();
        config.phone_battery_alerts = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().phone_battery_alerts);
    }

    #[test]
    fn links_from_phones_open_by_default_and_the_choice_saves() {
        assert!(Config::default().open_links);
        let path = temp("open-links");
        let mut config = Config::load_from(&path).unwrap();
        config.open_links = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().open_links);
    }

    #[test]
    fn notification_and_text_settings_have_safe_defaults_and_save() {
        let config = Config::default();
        assert!(config.phone_notifications && config.sms_notify);
        assert_eq!(
            (config.notifications_per_minute, config.sms_keep),
            (20, 1000)
        );
        let path = temp("notify-settings");
        let mut config = Config::load_from(&path).unwrap();
        config.notifications_per_minute = 5;
        config.sms_notify = false;
        config.dht_publish = DhtPublish::Off;
        config.relay_urls = vec!["https://relay.example.org".into()];
        config.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.notifications_per_minute, 5);
        assert!(!back.sms_notify);
        assert_eq!(back.dht_publish, DhtPublish::Off);
        assert_eq!(
            back.relay_urls,
            vec!["https://relay.example.org".to_string()]
        );
        let mut cleared = back;
        cleared.relay_urls.clear();
        cleared.save_to(&path).unwrap();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("relay_urls"),
            "an emptied list leaves the file"
        );
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
