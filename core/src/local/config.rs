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
    /// Let a slow link lower a session's frame rate (60, 30, 15) so each
    /// frame keeps enough bits to stay readable (R6f). On by default; a
    /// phone's "Full quality" turns it off for its session.
    #[serde(default = "default_true")]
    pub slow_link_frame_rate: bool,
    /// Draw a phone's laser dot (G4): the air mouse in Remote moves a red
    /// dot on the screen instead of the pointer. On by default; drawn on
    /// Windows and on wlroots compositors (N10).
    #[serde(default = "default_true")]
    pub laser_pointer: bool,
    /// Move files with a phone linked by the internet road (2c), on a
    /// stream of its own control connection. On by default: it shows relays
    /// nothing a session there does not (sizes and timing, never content).
    #[serde(default = "default_true")]
    pub internet_files: bool,
    /// Received files keep the date they were last changed where they came
    /// from, and sent files carry theirs (K1). On by default.
    #[serde(default = "default_true")]
    pub keep_file_dates: bool,
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
    /// GNOME and KDE (N8): keep the screen-sharing portal's grant until the
    /// owner revokes it, so later sessions start with no one at the desk.
    /// Off, the portal asks at every session.
    #[serde(default)]
    pub remember_screen_grant: bool,
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
    /// A phone notification that can be answered shows its buttons and a
    /// reply box here (N3); off, it is plain and the Notifications page
    /// still answers.
    #[serde(default = "default_true")]
    pub toast_answers: bool,
    /// A phone notification closed here (its toast's ✕, the Notifications
    /// page's Clear, a Linux notification closed) is cleared on the phone
    /// too (L1). On by default.
    #[serde(default = "default_true")]
    pub clear_on_phone: bool,
    /// A phone notification shows its app's icon and, when the phone sends
    /// one, its picture (K3); off, none is kept or shown. On by default.
    #[serde(default = "default_true")]
    pub notification_pictures: bool,
    /// A phone's ongoing updates (progress, timers) show as one card that
    /// updates in place (N4). On by default; the phone sends them only
    /// with its own switch on.
    #[serde(default = "default_true")]
    pub phone_live_updates: bool,
    /// Where received files land (C2): an absolute folder, or empty for
    /// Downloads. A folder that is gone falls back to Downloads.
    #[serde(default)]
    pub downloads_folder: String,
    /// The window showing a phone's screen controls the phone, when the
    /// phone allows it (C8); off, the window only shows.
    #[serde(default = "default_true")]
    pub cast_control: bool,
    /// A notification for each text a phone forwards (roadmap N5b).
    #[serde(default = "default_true")]
    pub sms_notify: bool,
    /// How many texts the Messages history keeps.
    #[serde(default = "default_sms_keep")]
    pub sms_keep: u32,
    /// Keep the contact names phones send with their texts (T8e).
    #[serde(default = "default_true")]
    pub sms_names: bool,
    /// Open a link a phone hands over in the default browser (roadmap T8a);
    /// off, it is shown as a notification instead.
    #[serde(default = "default_true")]
    pub open_links: bool,
    /// Answer a paired phone's ask for this computer's state: CPU, memory,
    /// disks, battery, uptime (F2).
    #[serde(default = "default_true")]
    pub share_state: bool,
    /// Let a phone with Input add a screen of its own shape (T10j, Sway).
    #[serde(default = "default_true")]
    pub virtual_screens: bool,
    /// Send what is copied on this computer to connected phones with the
    /// Clipboard permission, as it changes (T10b).
    #[serde(default = "default_true")]
    pub clipboard_push: bool,
    /// With `clipboard_push`, tell those phones of an image copied here too,
    /// for them to fetch (M3). Off by default: a screenshot copied all day
    /// would go to every phone.
    #[serde(default)]
    pub clipboard_images: bool,
    /// Share what is copied with the owner's other paired computers, both
    /// ways (2g). Off by default: the content crosses machines.
    #[serde(default)]
    pub computer_clipboard: bool,
    /// Keep the last this many texts that crossed between this computer and
    /// its phones, either way (D3); 0 keeps none. Off by default: the texts
    /// stay on disk, owner-only.
    #[serde(default)]
    pub clipboard_history: u32,
    /// A key that sends this computer's clipboard to a phone from anywhere
    /// (H3): "win+shift+c" and the like, through the tray program on
    /// Windows. Empty (the default) is off.
    #[serde(default)]
    pub clipboard_hotkey: String,
    /// A key that opens the window typing on a phone from anywhere (N9,
    /// M4 part 2), through the tray program on Windows. Empty is off.
    #[serde(default)]
    pub typing_hotkey: String,
    /// Show a phone's player in this computer's own media controls (K7):
    /// Windows' volume flyout and media keys, or an MPRIS player on Linux
    /// (N10), through the tray program.
    /// Off by default.
    #[serde(default)]
    pub phone_media_controls: bool,
    /// Serve the phones' shared folders (T10g) read-only as WebDAV on
    /// loopback, behind a token (D8). Off by default.
    #[serde(default)]
    pub phone_drive: bool,
    /// Whether the drive takes files and folders into a phone's folders its
    /// owner marked (N6). On: nothing is written without the phone's mark.
    #[serde(default = "default_true")]
    pub phone_drive_write: bool,
    /// How long a phone with "Lock this computer when it leaves" on must be
    /// gone before the computer locks (D9), in minutes, 2 to 60.
    #[serde(default = "default_away_lock_minutes")]
    pub away_lock_minutes: u32,
    /// Whether phones are told when this computer is in use (N7), so a
    /// phone with "Toast only on the computer I'm using" on keeps its
    /// notifications off the others' screens. On.
    #[serde(default = "default_true")]
    pub tell_in_use: bool,
    /// Work with KDE Connect devices (3a–3c): on by default (the owner's
    /// decision); off, a connection opening with `{` is refused as before.
    #[serde(default = "default_true")]
    pub kdeconnect_compat: bool,
    /// A notification when a phone in a session reports a low battery
    /// (roadmap T8b).
    #[serde(default = "default_true")]
    pub phone_battery_alerts: bool,
    /// Tell me when a phone has charged to this many percent (L3): 0 is
    /// off (the default), else 80, 90 or 100.
    #[serde(default)]
    pub charged_alert: u8,
    /// Whether a phone running out of room is told of once (N9). On.
    #[serde(default = "default_true")]
    pub phone_storage_alerts: bool,
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
    /// Keep the event log other programs read (`pocketdesk events`,
    /// ecosystem phase 1a).
    #[serde(default = "default_true")]
    pub events: bool,
    /// How much an event says: metadata, or the content too.
    #[serde(default)]
    pub events_detail: super::events::EventsDetail,
    /// A command line run for each event of a kind, with the event's JSON on
    /// stdin and in `POCKETDESK_EVENT` (`[hooks]` in config.toml).
    #[serde(default)]
    pub hooks: std::collections::BTreeMap<String, String>,
    /// Commands a phone with the Tasks permission may start by name
    /// (`[[tasks]]`, ecosystem 1c).
    #[serde(default)]
    pub tasks: Vec<super::tasks::TaskSpec>,
    /// Buttons a phone shows as its deck and presses (`[[deck]]`, F1).
    #[serde(default)]
    pub deck: Vec<super::deck::DeckButton>,
    /// Phones may list and press the deck's buttons.
    #[serde(default = "default_true")]
    pub deck_enabled: bool,
    /// Commands for phones' automation apps that Home Assistant may press
    /// (`[[phone_command]]`, M9).
    #[serde(default)]
    pub phone_command: Vec<super::commands::PhoneCommandSpec>,
    /// Publish this computer and its phones to Home Assistant through an
    /// MQTT broker (ecosystem 1e).
    #[serde(default)]
    pub home_assistant: bool,
    /// The broker: `mqtt://host:port`, or `host[:port]`.
    #[serde(default)]
    pub mqtt_broker: String,
    /// The broker's user name and password, when it asks for them.
    #[serde(default)]
    pub mqtt_user: String,
    #[serde(default)]
    pub mqtt_password: String,
}

fn default_transfer_keep_hours() -> u32 {
    24
}

fn default_away_lock_minutes() -> u32 {
    5
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
            slow_link_frame_rate: true,
            laser_pointer: true,
            internet_files: true,
            keep_file_dates: true,
            capture_backend: CaptureBackend::Auto,
            mic_relay_device: None,
            lan_discovery: true,
            remember_screen_grant: false,
            relay_urls: Vec::new(),
            dht_publish: DhtPublish::Relay,
            phone_notifications: true,
            toast_answers: true,
            clear_on_phone: true,
            phone_live_updates: true,
            notification_pictures: true,
            downloads_folder: String::new(),
            cast_control: true,
            notifications_per_minute: default_notifications_per_minute(),
            sms_notify: true,
            sms_keep: default_sms_keep(),
            sms_names: true,
            open_links: true,
            share_state: true,
            virtual_screens: true,
            clipboard_push: true,
            clipboard_images: false,
            computer_clipboard: false,
            clipboard_history: 0,
            clipboard_hotkey: String::new(),
            typing_hotkey: String::new(),
            phone_media_controls: false,
            phone_drive: false,
            phone_drive_write: true,
            away_lock_minutes: default_away_lock_minutes(),
            tell_in_use: true,
            kdeconnect_compat: true,
            phone_battery_alerts: true,
            charged_alert: 0,
            phone_storage_alerts: true,
            wake_announce: true,
            wake_port: 0,
            input_control: InputControlMode::One,
            input_idle_secs: default_input_idle_secs(),
            transfer_keep_hours: default_transfer_keep_hours(),
            events: true,
            events_detail: super::events::EventsDetail::Metadata,
            hooks: std::collections::BTreeMap::new(),
            tasks: Vec::new(),
            deck: Vec::new(),
            deck_enabled: true,
            phone_command: Vec::new(),
            home_assistant: false,
            mqtt_broker: String::new(),
            mqtt_user: String::new(),
            mqtt_password: String::new(),
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
        self.save_filtered(path, None)
    }

    /// Write only `keys` of this config to `path`, as [`Config::save_to`]
    /// does, leaving every other key as the file has it now (I3: the desktop
    /// app writes what its form changed, so a hand edit to the file made
    /// while it ran is kept).
    pub fn save_keys_to(&self, path: &Path, keys: &[&str]) -> Result<(), ConfigError> {
        self.save_filtered(path, Some(keys))
    }

    fn save_filtered(&self, path: &Path, only: Option<&[&str]>) -> Result<(), ConfigError> {
        let wanted_key = |key: &str| only.is_none_or(|keys| keys.contains(&key));
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
            if !wanted_key(key) || effective.get(key) == Some(value) {
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
            if Self::KNOWN_KEYS.contains(&key.as_str())
                && wanted_key(key)
                && !wanted.contains_key(key)
            {
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

    /// The keys whose values differ between `self` and `other`, in
    /// [`Self::KNOWN_KEYS`] order: what a save would change (U5: the
    /// desktop app says which of them apply at once).
    pub fn changed_keys(&self, other: &Config) -> Vec<&'static str> {
        let a = toml::Table::try_from(self).unwrap_or_default();
        let b = toml::Table::try_from(other).unwrap_or_default();
        Self::KNOWN_KEYS
            .iter()
            .copied()
            .filter(|key| a.get(*key) != b.get(*key))
            .collect()
    }

    /// Every key this version reads; others in the file are left alone.
    pub const KNOWN_KEYS: &'static [&'static str] = &[
        "control_port",
        "streaming_port",
        "log_level",
        "video_quality",
        "pairing_required",
        "allow_hevc",
        "slow_link_frame_rate",
        "laser_pointer",
        "internet_files",
        "keep_file_dates",
        "capture_backend",
        "mic_relay_device",
        "lan_discovery",
        "remember_screen_grant",
        "relay_urls",
        "dht_publish",
        "phone_notifications",
        "toast_answers",
        "clear_on_phone",
        "phone_live_updates",
        "notification_pictures",
        "downloads_folder",
        "cast_control",
        "notifications_per_minute",
        "sms_notify",
        "open_links",
        "share_state",
        "virtual_screens",
        "clipboard_push",
        "clipboard_images",
        "clipboard_hotkey",
        "typing_hotkey",
        "phone_media_controls",
        "computer_clipboard",
        "clipboard_history",
        "phone_drive",
        "phone_drive_write",
        "away_lock_minutes",
        "tell_in_use",
        "kdeconnect_compat",
        "phone_battery_alerts",
        "charged_alert",
        "phone_storage_alerts",
        "sms_keep",
        "sms_names",
        "wake_announce",
        "wake_port",
        "input_control",
        "input_idle_secs",
        "transfer_keep_hours",
        "events",
        "events_detail",
        "hooks",
        "tasks",
        "deck",
        "deck_enabled",
        "phone_command",
        "home_assistant",
        "mqtt_broker",
        "mqtt_user",
        "mqtt_password",
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

    #[test]
    fn changed_keys_name_what_differs_and_nothing_else() {
        let a = Config::default();
        assert!(a.changed_keys(&a).is_empty());
        let mut b = a.clone();
        b.control_port = 1800;
        b.internet_files = !a.internet_files;
        b.mqtt_password = "x".to_string();
        b.tasks.push(crate::local::tasks::TaskSpec {
            name: "n".to_string(),
            command: "c".to_string(),
            ask: true,
            deadline_secs: 60,
        });
        assert_eq!(
            a.changed_keys(&b),
            ["control_port", "internet_files", "tasks", "mqtt_password"]
        );
        // An option cleared to none is a change too.
        let mut c = a.clone();
        c.mic_relay_device = Some("CABLE".to_string());
        assert_eq!(c.changed_keys(&a), ["mic_relay_device"]);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.toml")
    }

    /// I3: a form saves what it changed, never what it read before a hand
    /// edit to the file.
    #[test]
    fn saving_some_keys_leaves_the_others_as_the_file_has_them() {
        let path = temp("save-keys");
        std::fs::write(&path, "control_port = 1716\nstreaming_port = 4716\n").unwrap();
        // The app read the file, then the owner set the stream port by hand.
        let read = Config::load_from(&path).unwrap();
        std::fs::write(&path, "control_port = 1716\nstreaming_port = 5000\n").unwrap();
        let mut edited = read.clone();
        edited.control_port = 1800;
        edited
            .save_keys_to(&path, &read.changed_keys(&edited))
            .unwrap();
        let now = Config::load_from(&path).unwrap();
        assert_eq!(now.control_port, 1800, "the form's change is written");
        assert_eq!(now.streaming_port, 5000, "the hand edit stays");
        // Nothing changed in the form: nothing written.
        let before = std::fs::read_to_string(&path).unwrap();
        now.save_keys_to(&path, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn events_and_hooks_are_saved_and_read_back() {
        let path = temp("events");
        let mut config = Config::default();
        assert!(config.events, "the log is on by default");
        assert_eq!(
            config.events_detail,
            crate::local::events::EventsDetail::Metadata
        );
        config.events_detail = crate::local::events::EventsDetail::Full;
        config
            .hooks
            .insert("file_received".into(), "echo got it".into());
        config.save_to(&path).unwrap();
        let back = Config::load_from(&path).unwrap();
        assert_eq!(back.events_detail, crate::local::events::EventsDetail::Full);
        assert_eq!(
            back.hooks.get("file_received").map(String::as_str),
            Some("echo got it")
        );
        // A [hooks] table written by hand reads as well.
        std::fs::write(&path, "[hooks]\nsession_started = \"echo hi\"\n").unwrap();
        let hand = Config::load_from(&path).unwrap();
        assert_eq!(
            hand.hooks.get("session_started").map(String::as_str),
            Some("echo hi")
        );
        // Saving it again keeps the hook and adds another setting.
        let mut changed = hand.clone();
        changed.open_links = false;
        changed.save_to(&path).unwrap();
        let again = Config::load_from(&path).unwrap();
        assert!(!again.open_links);
        assert_eq!(again.hooks, hand.hooks);
    }

    #[test]
    fn phone_commands_are_saved_and_read_back() {
        let path = temp("phone-command");
        let mut config = Config::default();
        config
            .phone_command
            .push(crate::local::commands::PhoneCommandSpec {
                label: "Lights off".into(),
                phone: "00000000-0000-4000-8000-0000000000c4".into(),
                name: "lights".into(),
                text: "off".into(),
            });
        config.save_to(&path).unwrap();
        let again = Config::load_from(&path).unwrap();
        assert_eq!(again.phone_command, config.phone_command);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn deck_buttons_are_saved_and_read_back() {
        let path = temp("deck");
        let mut config = Config::default();
        assert!(config.deck_enabled, "the deck is on unless turned off");
        config.deck.push(crate::local::deck::DeckButton {
            label: "Mute mic".into(),
            keys: Some("ctrl+shift+m".into()),
            ..Default::default()
        });
        config.deck.push(crate::local::deck::DeckButton {
            label: "Next".into(),
            media: Some("next".into()),
            ..Default::default()
        });
        config.deck_enabled = false;
        config.save_to(&path).unwrap();
        let again = Config::load_from(&path).unwrap();
        assert_eq!(again.deck, config.deck);
        assert!(!again.deck_enabled);
    }

    #[test]
    fn tasks_are_saved_and_read_back_and_a_hand_written_one_reads() {
        let path = temp("tasks");
        let mut config = Config::default();
        config.tasks.push(crate::local::tasks::TaskSpec {
            name: "Back up".into(),
            command: "backup.sh --all".into(),
            ask: false,
            deadline_secs: 600,
        });
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap().tasks, config.tasks);
        std::fs::write(&path, "[[tasks]]\nname = \"Build\"\ncommand = \"make\"\n").unwrap();
        let hand = Config::load_from(&path).unwrap();
        assert_eq!(hand.tasks.len(), 1);
        assert!(hand.tasks[0].ask, "asks first unless it says not to");
    }

    #[test]
    fn the_example_config_reads_as_it_ships() {
        // README says to copy it; a value it spells that the server refuses
        // stops the server at start.
        let example = concat!(env!("CARGO_MANIFEST_DIR"), "/../config.toml.example");
        let config = Config::load_from(std::path::Path::new(example))
            .unwrap_or_else(|e| panic!("config.toml.example does not read: {e}"));
        assert_eq!(config.video_quality, VideoQualityPreset::Balanced);
    }

    #[test]
    fn video_quality_reads_in_either_case_and_saves_in_lowercase() {
        let path = temp("video-quality");
        for (text, want) in [
            ("low", VideoQualityPreset::Low),
            ("Low", VideoQualityPreset::Low),
            ("balanced", VideoQualityPreset::Balanced),
            ("High", VideoQualityPreset::High),
        ] {
            std::fs::write(
                &path,
                format!(
                    "video_quality = \"{text}\"
"
                ),
            )
            .unwrap();
            assert_eq!(
                Config::load_from(&path).unwrap().video_quality,
                want,
                "{text}"
            );
        }
        let config = Config {
            video_quality: VideoQualityPreset::High,
            ..Config::default()
        };
        std::fs::write(&path, "").unwrap();
        config.save_to(&path).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("video_quality = \"high\"")
        );
    }

    #[test]
    fn home_assistant_is_off_by_default_and_its_keys_read_back() {
        let path = temp("home-assistant");
        assert!(!Config::default().home_assistant);
        std::fs::write(
            &path,
            "[hooks]
phone_battery = \"x\"
",
        )
        .unwrap();
        let mut config = Config::load_from(&path).unwrap();
        config.home_assistant = true;
        config.mqtt_broker = "mqtt://192.0.2.5:1883".into();
        config.mqtt_user = "pocketdesk".into();
        // Made at run time: a literal here reads as a committed password to
        // secret scanners, and it is only a fixture.
        let fixture = format!("fixture-{}", 7);
        config.mqtt_password = fixture.clone();
        config.save_to(&path).unwrap();
        let again = Config::load_from(&path).unwrap();
        assert!(again.home_assistant);
        assert_eq!(again.mqtt_broker, "mqtt://192.0.2.5:1883");
        assert_eq!(again.mqtt_user, "pocketdesk");
        assert_eq!(again.mqtt_password, fixture);
        assert_eq!(again.hooks.len(), 1, "the table after them still reads");
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
    fn files_cross_the_internet_road_by_default_and_the_choice_saves() {
        assert!(Config::default().internet_files);
        assert!(Config::default().keep_file_dates, "K1: on by default");
        let path = temp("internet-files");
        let mut config = Config::load_from(&path).unwrap();
        assert!(config.internet_files, "a file without the key");
        config.internet_files = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().internet_files);
    }

    #[test]
    fn a_slow_link_may_lower_the_frame_rate_by_default_and_the_choice_saves() {
        assert!(Config::default().slow_link_frame_rate);
        let path = temp("slow-link-fps");
        let mut config = Config::load_from(&path).unwrap();
        assert!(config.slow_link_frame_rate, "a file without the key");
        config.slow_link_frame_rate = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().slow_link_frame_rate);
        assert!(Config::default().laser_pointer, "G4: on by default");
    }

    #[test]
    fn contact_names_are_kept_by_default_and_the_choice_saves() {
        assert!(Config::default().sms_names);
        let path = temp("sms-names");
        let mut config = Config::load_from(&path).unwrap();
        config.sms_names = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().sms_names);
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
    fn the_clipboard_goes_to_phones_by_default_and_the_choice_saves() {
        assert!(Config::default().clipboard_push);
        let path = temp("clipboard-push");
        let mut config = Config::load_from(&path).unwrap();
        config.clipboard_push = false;
        config.save_to(&path).unwrap();
        assert!(!Config::load_from(&path).unwrap().clipboard_push);
        assert!(
            !Config::default().computer_clipboard,
            "the clipboard crosses to other computers only when asked (2g)"
        );
        assert!(Config::default().kdeconnect_compat, "on by default (3a)");
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
