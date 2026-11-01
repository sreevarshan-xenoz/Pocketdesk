//! A direct link (no shared network): this computer's own Wi-Fi, made for
//! one phone for a while, so Pocketdesk works on a train, in a café with
//! client isolation, or on a guest network that lets devices see only the
//! internet.
//!
//! This module is the part both programs share, and none of it touches the
//! network:
//!
//! * the **Wi-Fi details** a link carries (a made-up name and a random
//!   WPA2 passphrase, [`WifiJoin`]) and the standard `WIFI:` text for them
//!   ([`wifi_uri`]);
//! * the **plan**: whether this card can host a hotspot beside its own
//!   connection, read from `iw` ([`plan_from`], [`probe`]), so the desktop
//!   can say before it starts that this computer's Wi-Fi will go down;
//! * the **requests** the desktop leaves for the server
//!   (`<state>/direct_link_out/<id>.json`) and the **status** the server
//!   writes back (`<state>/direct_link.json`, owner-only: it holds the
//!   passphrase while the link is on);
//! * the **idle timer** that turns the link off when no phone is using it.
//!
//! The server makes and removes the access point (`server/src/direct_link.rs`):
//! NetworkManager over D-Bus on Linux, and on Windows the Mobile Hotspot
//! (`direct_link_windows`), whose settings are the whole system's, so
//! what was set there is recorded first and put back after
//! ([`HotspotRestore`]). The name and passphrase are never logged.

use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{
    DIRECT_LINK_DIR, DIRECT_LINK_FILE, DIRECT_LINK_OWNED_FILE, DIRECT_LINK_PLAN_FILE,
};

/// Every network name this makes starts so: a phone's list shows what made
/// it.
pub const SSID_PREFIX: &str = "pocketdesk-";
/// The most characters in a passphrase, and the fewest, by WPA2.
pub const PASSPHRASE_MIN: usize = 8;
pub const PASSPHRASE_MAX: usize = 63;
/// A network name is at most 32 bytes.
pub const SSID_MAX: usize = 32;
/// The minutes with no phone using the link before it turns itself off.
pub const DEFAULT_IDLE_MINUTES: u32 = 10;
pub const MIN_IDLE_MINUTES: u32 = 1;
pub const MAX_IDLE_MINUTES: u32 = 120;
/// The most a link lives, whatever is using it: a connection from the
/// network held the idle timer off for good, and a hotspot nobody asked for
/// again should not outlast a day's use.
pub const MAX_LIFETIME_SECS: u64 = 8 * 60 * 60;
/// How long a request waits for the server before it is dropped: a link
/// started a minute later than asked would surprise.
pub const REQUEST_TTL_SECS: u64 = 60;
/// A status the server has not rewritten for this long was left by a
/// server that is no longer running (it rewrites an "on" status every few
/// seconds).
pub const STATUS_FRESH_SECS: u64 = 30;

/// The name and passphrase the phone joins with.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiJoin {
    pub ssid: String,
    pub passphrase: String,
}

// Never printed: a debug line of a link must not leak the key.
impl std::fmt::Debug for WifiJoin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WifiJoin(..)")
    }
}

impl WifiJoin {
    /// Whether both parts are ones an access point can use: a name of 1 to
    /// 32 bytes with no control characters, and a passphrase of 8 to 63
    /// printable ASCII characters.
    pub fn is_valid(&self) -> bool {
        !self.ssid.is_empty()
            && self.ssid.len() <= SSID_MAX
            && !self.ssid.chars().any(char::is_control)
            && (PASSPHRASE_MIN..=PASSPHRASE_MAX).contains(&self.passphrase.len())
            && self.passphrase.bytes().all(|b| (0x20..=0x7e).contains(&b))
    }
}

/// A fresh network name: the prefix and four random letters or digits.
pub fn new_ssid() -> String {
    format!("{SSID_PREFIX}{}", random_chars(4, LOWER_DIGITS))
}

/// A fresh passphrase: 20 characters of 55 (about 115 bits), from the OS CSPRNG.
pub fn new_passphrase() -> String {
    random_chars(20, ALPHANUMERIC)
}

const LOWER_DIGITS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
const ALPHANUMERIC: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789";

fn random_chars(len: usize, alphabet: &[u8]) -> String {
    use rand::Rng;
    (0..len)
        .map(|_| alphabet[rand::rngs::OsRng.gen_range(0..alphabet.len())] as char)
        .collect()
}

/// The standard Wi-Fi QR text (`WIFI:T:WPA;S:name;P:key;;`) a phone's
/// camera reads. `\`, `;`, `,`, `:` and `"` are escaped with a backslash.
pub fn wifi_uri(join: &WifiJoin) -> String {
    fn escape(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
    format!(
        "WIFI:T:WPA;S:{};P:{};;",
        escape(&join.ssid),
        escape(&join.passphrase)
    )
}

/// The band the owner prefers (`direct_link_band`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Band {
    /// The card's choice: 2.4 GHz, which every phone can join.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "2.4")]
    Ghz24,
    #[serde(rename = "5")]
    Ghz5,
}

impl Band {
    /// The setting's words (`"auto"`, `"2.4"`, `"5"`); anything else is
    /// auto.
    pub fn from_setting(text: &str) -> Self {
        match text.trim() {
            "2.4" => Self::Ghz24,
            "5" => Self::Ghz5,
            _ => Self::Auto,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ghz24 => "2.4",
            Self::Ghz5 => "5",
        }
    }

    /// NetworkManager's name for it (`bg` or `a`), none for auto.
    pub fn nm_band(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Ghz24 => Some("bg"),
            Self::Ghz5 => Some("a"),
        }
    }

    /// The Mobile Hotspot's name for the band
    /// (`TetheringWiFiBand`: 0 auto, 1 2.4 GHz, 2 5 GHz), the number the
    /// owner's own setting is also kept as.
    pub fn tethering_code(self) -> i32 {
        match self {
            Self::Auto => TETHERING_AUTO,
            Self::Ghz24 => TETHERING_24,
            Self::Ghz5 => TETHERING_5,
        }
    }
}

/// `TetheringWiFiBand` as the Mobile Hotspot numbers it.
pub const TETHERING_AUTO: i32 = 0;
pub const TETHERING_24: i32 = 1;
pub const TETHERING_5: i32 = 2;

/// The Mobile Hotspot band to ask for, given the owner's choice and which
/// bands this computer's card can host on (`supported(code)`). Automatic
/// means 2.4 GHz where the card has it, as on Linux (every phone joins it);
/// a band the card cannot host is left to the card (auto) rather than
/// failing the link.
pub fn pick_tethering_band(want: Band, supported: impl Fn(i32) -> bool) -> i32 {
    let code = match want {
        Band::Auto | Band::Ghz24 => TETHERING_24,
        Band::Ghz5 => TETHERING_5,
    };
    if supported(code) {
        code
    } else {
        TETHERING_AUTO
    }
}

/// Why the Mobile Hotspot cannot be used, from `TetheringCapability`'s
/// number (0 enabled, 1 group policy, 2 hardware, 3 operator, 4 edition,
/// 5 a required app, 6 unknown, 7 system capability); none when it can.
pub fn unavailable_for_tethering_capability(code: i32) -> Option<Unavailable> {
    match code {
        0 => None,
        2 | 7 => Some(Unavailable::NoAccessPoint),
        _ => Some(Unavailable::Blocked),
    }
}

/// What a Mobile Hotspot that would not start says, from
/// `TetheringOperationStatus`'s number. A few words with nothing from the
/// operator in them.
pub fn tethering_failure_words(code: i32) -> &'static str {
    match code {
        2 | 4 | 5 => {
            "Windows' Mobile Hotspot could not start its check with the connection's provider."
        }
        3 => "Wi-Fi is switched off on this computer. Turn it on, then try again.",
        6 => "Windows is still changing the hotspot. Try again in a moment.",
        7 => "Bluetooth is switched off on this computer.",
        8 => {
            "This computer's internet connection has limited connectivity, which Mobile Hotspot will not share."
        }
        10 => "A radio restriction (airplane mode, for one) stops the hotspot.",
        11 => {
            "The hotspot could not use the band chosen without interfering with this computer's own Wi-Fi. Try Automatic."
        }
        _ => "Windows' Mobile Hotspot did not start.",
    }
}

/// This computer's address on the network a Mobile Hotspot just made: an
/// address that was not there before the start, in Windows' hotspot range
/// (192.168.137.0/24) first, else any private one; failing that the range's
/// own gateway address, if it is there now. `before` and `after` are this
/// computer's IPv4 addresses.
pub fn pick_link_address(before: &[Ipv4Addr], after: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    let in_range = |a: &Ipv4Addr| a.octets()[..3] == [192, 168, 137];
    let mut fresh: Vec<Ipv4Addr> = after
        .iter()
        .filter(|a| !before.contains(a) && a.is_private())
        .copied()
        .collect();
    fresh.sort();
    fresh
        .iter()
        .find(|a| in_range(a))
        .or_else(|| fresh.first())
        .copied()
        .or_else(|| {
            after
                .iter()
                .find(|a| **a == WINDOWS_HOTSPOT_GATEWAY)
                .copied()
        })
}

/// The address Windows gives itself on a hotspot's network.
pub const WINDOWS_HOTSPOT_GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 137, 1);

/// The band a Wi-Fi channel number is in, for a card that must keep one
/// channel (2.4 GHz channels are 1 to 14, 5 GHz from 32).
pub fn band_of_channel(channel: u32) -> Band {
    if (1..=14).contains(&channel) {
        Band::Ghz24
    } else {
        Band::Ghz5
    }
}

// ---- What this computer's card can do ----

/// How the link is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// On a second virtual interface, beside the card's own connection.
    SecondInterface,
    /// The hotspot takes over the card: its own connection ends while the
    /// link is on.
    TakesOverCard,
}

/// Why no link can be made here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    /// Not a platform this makes links on yet.
    Unsupported,
    /// `iw` is missing, or says this computer has no Wi-Fi card.
    NoWifi,
    /// A Wi-Fi card that cannot host an access point.
    NoAccessPoint,
    /// NetworkManager is not running: it makes the hotspot.
    NoNetworkManager,
    /// Windows: no internet connection for the Mobile Hotspot to share, and
    /// it will not start without one.
    NoConnectionProfile,
    /// Windows: Mobile Hotspot is turned off here by policy, the hardware,
    /// the operator or the system.
    Blocked,
    /// Windows: the owner's own Mobile Hotspot is on, and a link would
    /// change it under the devices using it.
    HotspotInUse,
}

impl Unavailable {
    /// What the desktop says.
    pub fn words(self) -> &'static str {
        match self {
            Self::Unsupported => "A direct link is not available on this system yet.",
            Self::NoWifi => "This computer has no Wi-Fi card a direct link can use.",
            Self::NoAccessPoint => "This computer's Wi-Fi card cannot make its own network.",
            Self::NoNetworkManager => {
                "A direct link needs NetworkManager, which is not running here."
            }
            Self::NoConnectionProfile => {
                "Windows' Mobile Hotspot shares an internet connection, and this computer has none right now. Connect it to a network (Ethernet or Wi-Fi), then try again."
            }
            Self::Blocked => {
                "Mobile Hotspot is turned off on this computer (by policy, by its hardware or by Windows), so it cannot make a direct link."
            }
            Self::HotspotInUse => {
                "Windows' Mobile Hotspot is on already. Turn it off in Settings, Network & internet, Mobile hotspot, so a direct link does not change a hotspot you are using."
            }
        }
    }
}

/// What starting a link would do on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// The card's interface (`wlan0`): the hotspot goes on it, or beside it.
    pub device: String,
    pub mode: Mode,
    /// A channel the hotspot must use (the card keeps one channel for both
    /// connections), so its band follows the card's connection.
    pub channel: Option<u32>,
    /// This computer's own Wi-Fi goes down while the link is on.
    pub disconnects: bool,
}

/// One group of an interface combination: `#{ managed } <= 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub modes: Vec<String>,
    pub max: u32,
}

/// One `valid interface combinations` entry of `iw list`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Combination {
    pub groups: Vec<Group>,
    pub total: u32,
    pub channels: u32,
}

impl Combination {
    /// Whether one managed interface and one access point can both be up.
    pub fn allows_ap_and_managed(&self) -> bool {
        let find = |mode: &str| {
            self.groups
                .iter()
                .position(|g| g.max >= 1 && g.modes.iter().any(|m| m == mode))
        };
        let (Some(ap), Some(managed)) = (find("AP"), find("managed")) else {
            return false;
        };
        let room = if ap == managed {
            self.groups[ap].max >= 2
        } else {
            true
        };
        room && self.total >= 2
    }
}

/// One wireless device (`Wiphy phy0`) of `iw list`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wiphy {
    pub index: u32,
    pub supports_ap: bool,
    pub combinations: Vec<Combination>,
}

impl Wiphy {
    /// The most channels any combination with an access point beside a
    /// managed interface allows (1: both on one channel), or `None` when
    /// none allows the two at once.
    pub fn concurrent_channels(&self) -> Option<u32> {
        self.combinations
            .iter()
            .filter(|c| c.allows_ap_and_managed())
            .map(|c| c.channels)
            .max()
    }
}

/// Read `iw list`.
pub fn parse_iw_list(text: &str) -> Vec<Wiphy> {
    let mut wiphys: Vec<Wiphy> = Vec::new();
    #[derive(PartialEq)]
    enum In {
        Nothing,
        Modes,
        Combos,
    }
    let mut section = In::Nothing;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Wiphy ") {
            let index = rest
                .trim()
                .strip_prefix("phy")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            wiphys.push(Wiphy {
                index,
                ..Wiphy::default()
            });
            section = In::Nothing;
            continue;
        }
        let Some(wiphy) = wiphys.last_mut() else {
            continue;
        };
        let trimmed = line.trim();
        // A line at one tab starts a section; deeper ones belong to it.
        let indent = line.chars().take_while(|c| *c == '\t').count();
        if indent <= 1 {
            section = if trimmed.starts_with("Supported interface modes:") {
                In::Modes
            } else if trimmed.starts_with("valid interface combinations:") {
                In::Combos
            } else {
                In::Nothing
            };
            continue;
        }
        match section {
            In::Modes => {
                if trimmed.trim_start_matches('*').trim() == "AP" {
                    wiphy.supports_ap = true;
                }
            }
            In::Combos => {
                if let Some(entry) = trimmed.strip_prefix('*') {
                    wiphy.combinations.push(parse_combination(entry));
                } else if let Some(last) = wiphy.combinations.last_mut() {
                    // A wrapped line continues the entry above it.
                    let more = parse_combination(trimmed);
                    last.groups.extend(more.groups);
                    last.total = last.total.max(more.total);
                    last.channels = last.channels.max(more.channels);
                }
            }
            In::Nothing => {}
        }
    }
    wiphys
}

fn parse_combination(text: &str) -> Combination {
    let mut combination = Combination::default();
    let mut rest = text;
    while let Some(start) = rest.find("#{") {
        let after = &rest[start + 2..];
        let Some(close) = after.find('}') else { break };
        let modes: Vec<String> = after[..close]
            .split(',')
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        let tail = after[close + 1..].trim_start();
        let max = tail
            .strip_prefix("<=")
            .and_then(|t| leading_number(t.trim_start()))
            .unwrap_or(0);
        combination.groups.push(Group { modes, max });
        rest = &after[close + 1..];
    }
    if let Some(n) = number_after(text, "total <=") {
        combination.total = n;
    }
    if let Some(n) = number_after(text, "#channels <=") {
        combination.channels = n;
    }
    combination
}

fn number_after(text: &str, label: &str) -> Option<u32> {
    let at = text.find(label)?;
    leading_number(text[at + label.len()..].trim_start())
}

fn leading_number(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// One interface of `iw dev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub phy: u32,
    pub kind: String,
    /// The channel it is on, when it is connected.
    pub channel: Option<u32>,
}

/// Read `iw dev`.
pub fn parse_iw_dev(text: &str) -> Vec<Interface> {
    let mut found: Vec<Interface> = Vec::new();
    let mut phy = 0u32;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(n) = trimmed.strip_prefix("phy#") {
            phy = n.trim().parse().unwrap_or(0);
        } else if let Some(name) = trimmed.strip_prefix("Interface ") {
            found.push(Interface {
                name: name.trim().to_string(),
                phy,
                kind: String::new(),
                channel: None,
            });
        } else if let Some(current) = found.last_mut() {
            if let Some(kind) = trimmed.strip_prefix("type ") {
                current.kind = kind.trim().to_string();
            } else if let Some(rest) = trimmed.strip_prefix("channel ") {
                current.channel = leading_number(rest);
            }
        }
    }
    found
}

/// Whether a `/proc/self/status` says this process may add a network
/// interface: it holds `CAP_NET_ADMIN` (bit 12) in its effective set.
pub fn can_add_interface_from_status(status: &str) -> bool {
    status
        .lines()
        .find_map(|l| l.strip_prefix("CapEff:"))
        .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        .is_some_and(|caps| caps & (1 << 12) != 0)
}

/// What starting a link would do, from `iw list` and `iw dev`.
/// `can_add_interface` says whether this program may make a second
/// interface (it needs `CAP_NET_ADMIN`; a desktop user's does not have it,
/// and then the hotspot takes over the card).
pub fn plan_from(
    iw_list: &str,
    iw_dev: &str,
    can_add_interface: bool,
) -> Result<Plan, Unavailable> {
    let wiphys = parse_iw_list(iw_list);
    let stations: Vec<Interface> = parse_iw_dev(iw_dev)
        .into_iter()
        .filter(|i| i.kind == "managed")
        .collect();
    if stations.is_empty() {
        return Err(Unavailable::NoWifi);
    }
    // The first card that can host: one that is connected is preferred
    // (the one the owner is using is the one to say something about).
    let hosts = |i: &&Interface| wiphys.iter().any(|w| w.index == i.phy && w.supports_ap);
    let station = stations
        .iter()
        .filter(hosts)
        .find(|i| i.channel.is_some())
        .or_else(|| stations.iter().find(hosts))
        .ok_or(Unavailable::NoAccessPoint)?;
    let wiphy = wiphys
        .iter()
        .find(|w| w.index == station.phy)
        .ok_or(Unavailable::NoAccessPoint)?;
    let connected = station.channel.is_some();
    // Beside the card's own connection: the card allows both, and this
    // program may make the interface. One channel for both means the
    // hotspot follows the card's connection.
    if can_add_interface && let Some(channels) = wiphy.concurrent_channels() {
        return Ok(Plan {
            device: station.name.clone(),
            mode: Mode::SecondInterface,
            channel: if channels <= 1 { station.channel } else { None },
            disconnects: false,
        });
    }
    Ok(Plan {
        device: station.name.clone(),
        mode: Mode::TakesOverCard,
        channel: None,
        disconnects: connected,
    })
}

/// [`plan_from`] for this computer: runs `iw` and reads this process's
/// capabilities (Linux), or asks the Mobile Hotspot (Windows). Read-only (it
/// changes nothing).
pub fn probe() -> Result<Plan, Unavailable> {
    #[cfg(target_os = "linux")]
    {
        let iw = |args: &[&str]| -> Option<String> {
            let out = std::process::Command::new(iw_path()?)
                .args(args)
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let list = iw(&["list"]).ok_or(Unavailable::NoWifi)?;
        let dev = iw(&["dev"]).ok_or(Unavailable::NoWifi)?;
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        plan_from(&list, &dev, can_add_interface_from_status(&status))
    }
    #[cfg(windows)]
    {
        super::direct_link_windows::probe()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(Unavailable::Unsupported)
    }
}

/// Where `iw` is, by its usual places (the desktop's `PATH` may lack
/// `/usr/sbin`).
#[cfg(target_os = "linux")]
pub fn iw_path() -> Option<PathBuf> {
    ["/usr/bin/iw", "/usr/sbin/iw", "/sbin/iw", "/bin/iw"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

// ---- The desktop's asks and the server's word ----

/// What the desktop asks the server to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Start,
    Stop,
}

/// One ask, as `<state>/direct_link_out/<id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub action: Action,
    pub band: Band,
    pub idle_minutes: u32,
    /// The owner saw that this computer's Wi-Fi goes down and said yes. The
    /// server starts a link that takes over the card only with it.
    #[serde(default)]
    pub accept_takeover: bool,
    pub created_unix: u64,
}

impl Request {
    pub fn new(
        action: Action,
        band: Band,
        idle_minutes: u32,
        accept_takeover: bool,
        now_unix: u64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            action,
            band,
            idle_minutes: idle_minutes.clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES),
            accept_takeover,
            created_unix: now_unix,
        }
    }
}

/// Leave `request` for the server.
pub fn request(state_dir: &Path, request: &Request) -> io::Result<()> {
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(DIRECT_LINK_DIR))?;
    let json = serde_json::to_string(request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)
}

/// Take every waiting request, oldest first (the server's side). Each file
/// is removed once read; a stale or unreadable one is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(DIRECT_LINK_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue; // a temp file mid-write
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Request>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn request_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a valid request id",
        ));
    }
    Ok(state_dir.join(DIRECT_LINK_DIR).join(format!("{id}.json")))
}

/// Where the link is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    On,
    /// It could not be made; [`Status::error`] says why.
    Failed,
}

/// `<state>/direct_link.json`: the server's word on the link. It exists
/// only while there is something to say, and holds the passphrase while the
/// link is on (owner-only, like the PIN file).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub mode: Option<Mode>,
    /// The phone's way in, once the link is on.
    pub wifi: Option<WifiJoin>,
    /// The computer's address on its own network, once the link is on.
    pub address: Option<String>,
    pub started_unix: u64,
    /// When it was last rewritten: how the desktop knows the server still
    /// runs.
    pub updated_unix: u64,
    pub idle_minutes: u32,
    /// Seconds until it turns off for want of a phone (while on).
    pub idle_left_secs: Option<u64>,
    /// Whether a phone is using it now.
    pub in_use: bool,
    pub error: Option<String>,
}

impl std::fmt::Debug for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Status({:?})", self.state)
    }
}

impl Status {
    /// Written by a server that still runs.
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.updated_unix) <= STATUS_FRESH_SECS
    }
}

/// Write the status (the server's side), owner-only.
pub fn write_status(state_dir: &Path, status: &Status) -> io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(status).map_err(io::Error::other)?;
    let file = state_dir.join(DIRECT_LINK_FILE);
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

/// The status on file, if any and readable.
pub fn read_status(state_dir: &Path) -> Option<Status> {
    let raw = std::fs::read_to_string(state_dir.join(DIRECT_LINK_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Take the status off (the link is off).
pub fn clear_status(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(DIRECT_LINK_FILE));
}

/// How long a plan on file is believed: the server refreshes it every half
/// minute while no link is on.
pub const PLAN_FRESH_SECS: u64 = 90;

/// The server's word on what a link could do here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReport {
    pub updated_unix: u64,
    pub plan: Result<Plan, Unavailable>,
}

/// Leave the plan for the desktop to read. Nothing secret in it.
pub fn write_plan(state_dir: &Path, report: &PlanReport) -> io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(report).map_err(io::Error::other)?;
    let file = state_dir.join(DIRECT_LINK_PLAN_FILE);
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)
}

/// The plan on file, if there is one and it is recent: a plan a stopped
/// server left says nothing about now.
pub fn read_plan(state_dir: &Path, now_unix: u64) -> Option<Result<Plan, Unavailable>> {
    let raw = std::fs::read_to_string(state_dir.join(DIRECT_LINK_PLAN_FILE)).ok()?;
    let report: PlanReport = serde_json::from_str(&raw).ok()?;
    (now_unix.saturating_sub(report.updated_unix) <= PLAN_FRESH_SECS).then_some(report.plan)
}

/// The plan is taken off file as the server starts and stops.
pub fn clear_plan(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(DIRECT_LINK_PLAN_FILE));
}

// ---- Idle ----

/// Turns the link off after `limit` with no phone using it. Counted in
/// seconds the caller reads from any clock.
#[derive(Debug, Clone)]
pub struct IdleTimer {
    limit: Duration,
    idle_since: u64,
}

impl IdleTimer {
    /// Starts idle at `now_secs`: a phone has the limit to scan and join.
    pub fn new(limit_minutes: u32, now_secs: u64) -> Self {
        Self {
            limit: Duration::from_secs(
                u64::from(limit_minutes.clamp(MIN_IDLE_MINUTES, MAX_IDLE_MINUTES)) * 60,
            ),
            idle_since: now_secs,
        }
    }

    /// Note whether a phone is using the link at `now_secs`.
    pub fn tick(&mut self, now_secs: u64, busy: bool) {
        if busy {
            self.idle_since = now_secs;
        }
    }

    /// Seconds left before the link turns off, as of `now_secs`.
    pub fn left(&self, now_secs: u64) -> u64 {
        self.limit
            .as_secs()
            .saturating_sub(now_secs.saturating_sub(self.idle_since))
    }

    pub fn expired(&self, now_secs: u64) -> bool {
        self.left(now_secs) == 0
    }
}

/// Whether a link started at `started_unix` has lived its longest.
pub fn lifetime_over(started_unix: u64, now_unix: u64) -> bool {
    now_unix.saturating_sub(started_unix) >= MAX_LIFETIME_SECS
}

// ---- What this state folder's server made ----

/// The connection a takeover took the card from, to bring back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Restore {
    /// The card's interface (`wlan0`).
    pub device: String,
    /// NetworkManager's path of the connection that was active on it.
    pub connection: String,
}

/// What a server made, written before it makes it and removed once it is
/// gone, so a start after a crash removes exactly this and no other
/// server's (a throwaway server's start must not take down the main one's
/// link). Nothing secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owned {
    /// The NetworkManager connection's uuid.
    pub uuid: String,
    /// The virtual interface added, when there is one.
    pub interface: Option<String>,
    /// The connection to bring back after a takeover.
    pub restore: Option<Restore>,
    /// Windows: the owner's own Mobile Hotspot settings, to put back. The
    /// one place a passphrase other than the link's is kept: the owner's
    /// hotspot passphrase, in this owner-only file, until the link ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotspot: Option<HotspotRestore>,
}

/// The Mobile Hotspot settings on Windows (`ssid`, `passphrase`, and the
/// band as `TetheringWiFiBand`'s number, none where this Windows has no
/// band setting).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotspotSettings {
    pub ssid: String,
    pub passphrase: String,
    pub band: Option<i32>,
}

impl std::fmt::Debug for HotspotSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HotspotSettings(..)")
    }
}

/// What the Mobile Hotspot was before a link: Windows keeps one setting
/// for the whole system, so a link changes the owner's, and ends by putting
/// it back (and leaving the hotspot off if it was off).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotspotRestore {
    /// It was on, so it goes back on.
    pub was_on: bool,
    pub settings: HotspotSettings,
}

impl std::fmt::Debug for HotspotRestore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HotspotRestore(was_on: {})", self.was_on)
    }
}

/// Whether the Mobile Hotspot as it is now can be made into a link, and
/// recorded as the owner's to put back: why not, in words, or none. It must
/// be off (a link would change it under whatever uses it), have a name and
/// a passphrase that were really read (an empty one could never be put back
/// and the owner's would be lost), and not be named like a link (that is a
/// left-over link's, never the owner's, and recording it would lose the
/// owner's settings for good).
pub fn hotspot_start_refusal(before: &HotspotRestore) -> Option<&'static str> {
    if before.was_on {
        return Some(Unavailable::HotspotInUse.words());
    }
    let settings = &before.settings;
    if settings.ssid.is_empty() || settings.passphrase.is_empty() {
        return Some(
            "Windows' Mobile Hotspot has no name or passphrase Pocketdesk could read, so it could not be put back afterwards. Set the hotspot up in Windows' Settings first.",
        );
    }
    if settings.ssid.starts_with(SSID_PREFIX) {
        return Some(
            "Windows' Mobile Hotspot is still named like a direct link's (an earlier link did not end cleanly). Give it a name of your own in Windows' Settings, then try again.",
        );
    }
    None
}

/// One step of putting the Mobile Hotspot back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotspotStep {
    Stop,
    Configure,
    Start,
}

/// What to do to put the Mobile Hotspot back, given what it is now
/// (`current_ssid`, and whether it is `on`). Only a hotspot still set to a
/// name this made (`pocketdesk-...`) is touched: one the owner set up
/// since, after a crash, is theirs. Stop first, so no phone is left on the
/// link's network while its settings change; the owner's own hotspot goes
/// back on only if it was on.
pub fn hotspot_restore_steps(
    restore: &HotspotRestore,
    current_ssid: &str,
    on: bool,
) -> Vec<HotspotStep> {
    if !current_ssid.starts_with(SSID_PREFIX) {
        return Vec::new();
    }
    let mut steps = Vec::new();
    if on {
        steps.push(HotspotStep::Stop);
    }
    steps.push(HotspotStep::Configure);
    if restore.was_on {
        steps.push(HotspotStep::Start);
    }
    steps
}

/// What to bring back once a link ends: only a takeover took something,
/// and only a card that had a connection had something taken.
pub fn restore_for(mode: Mode, device: &str, previous: Option<String>) -> Option<Restore> {
    match (mode, previous) {
        (Mode::TakesOverCard, Some(connection)) if connection != "/" && !connection.is_empty() => {
            Some(Restore {
                device: device.to_string(),
                connection,
            })
        }
        _ => None,
    }
}

pub fn write_owned(state_dir: &Path, owned: &Owned) -> io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(owned).map_err(io::Error::other)?;
    let file = state_dir.join(DIRECT_LINK_OWNED_FILE);
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

pub fn read_owned(state_dir: &Path) -> Option<Owned> {
    let raw = std::fs::read_to_string(state_dir.join(DIRECT_LINK_OWNED_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn clear_owned(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(DIRECT_LINK_OWNED_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(ssid: &str, passphrase: &str) -> WifiJoin {
        WifiJoin {
            ssid: ssid.into(),
            passphrase: passphrase.into(),
        }
    }

    #[test]
    fn the_wifi_text_escapes_what_the_format_reserves() {
        assert_eq!(
            wifi_uri(&join("pocketdesk-ab12", "Abcdef123456")),
            "WIFI:T:WPA;S:pocketdesk-ab12;P:Abcdef123456;;"
        );
        assert_eq!(
            wifi_uri(&join("a;b,c:d\"e\\f", "p;q:r,s\"t\\u")),
            r#"WIFI:T:WPA;S:a\;b\,c\:d\"e\\f;P:p\;q\:r\,s\"t\\u;;"#
        );
    }

    #[test]
    fn made_up_names_and_passphrases_are_valid_and_differ() {
        let ssid = new_ssid();
        assert!(ssid.starts_with(SSID_PREFIX) && ssid.len() == SSID_PREFIX.len() + 4);
        let key = new_passphrase();
        assert_eq!(key.len(), 20);
        assert!(key.bytes().all(|b| ALPHANUMERIC.contains(&b)), "{key}");
        assert!(join(&ssid, &key).is_valid(), "an access point can use them");
        // 115 bits: two in a row are never the same.
        assert_ne!(key, new_passphrase());
        // Only letters and digits: nothing for the QR text to escape.
        assert_eq!(
            wifi_uri(&join(&ssid, &key)),
            format!("WIFI:T:WPA;S:{ssid};P:{key};;")
        );
    }

    #[test]
    fn details_an_access_point_cannot_use_are_not_valid() {
        assert!(join("x", "12345678").is_valid());
        assert!(!join("", "12345678").is_valid(), "no name");
        assert!(
            !join(&"n".repeat(33), "12345678").is_valid(),
            "name too long"
        );
        assert!(!join("a\nb", "12345678").is_valid(), "control character");
        assert!(!join("x", "1234567").is_valid(), "key too short");
        assert!(!join("x", &"k".repeat(64)).is_valid(), "key too long");
        assert!(!join("x", "pässword1").is_valid(), "not ASCII");
    }

    #[test]
    fn a_debug_line_never_holds_the_name_or_the_key() {
        let shown = format!("{:?}", join("pocketdesk-ab12", "Abcdef123456"));
        assert!(
            !shown.contains("ab12") && !shown.contains("Abcdef"),
            "{shown}"
        );
        let status = Status {
            state: State::On,
            mode: Some(Mode::TakesOverCard),
            wifi: Some(join("pocketdesk-ab12", "Abcdef123456")),
            address: Some("192.0.2.1".into()),
            started_unix: 1,
            updated_unix: 1,
            idle_minutes: 10,
            idle_left_secs: None,
            in_use: false,
            error: None,
        };
        let shown = format!("{status:?}");
        assert!(
            !shown.contains("ab12") && !shown.contains("Abcdef"),
            "{shown}"
        );
    }

    #[test]
    fn bands_read_and_say_their_settings() {
        for (text, band) in [
            ("auto", Band::Auto),
            ("2.4", Band::Ghz24),
            ("5", Band::Ghz5),
            ("  5 ", Band::Ghz5),
            ("6", Band::Auto),
            ("", Band::Auto),
        ] {
            assert_eq!(Band::from_setting(text), band, "{text:?}");
        }
        assert_eq!(Band::Ghz24.setting(), "2.4");
        assert_eq!(Band::Ghz5.nm_band(), Some("a"));
        assert_eq!(Band::Auto.nm_band(), None);
        assert_eq!(band_of_channel(9), Band::Ghz24);
        assert_eq!(band_of_channel(36), Band::Ghz5);
    }

    /// An Intel card as `iw list` prints it (trimmed): one managed interface
    /// and one access point may both be up if they share a channel.
    const ONE_CHANNEL: &str = "Wiphy phy0
\twiphy index: 0
\tSupported interface modes:
\t\t * IBSS
\t\t * managed
\t\t * AP
\t\t * P2P-client
\t\t * P2P-GO
\t\t * P2P-device
\tvalid interface combinations:
\t\t * #{ managed } <= 1, #{ P2P-client, P2P-GO } <= 1, #{ P2P-device } <= 1,
\t\t   total <= 3, #channels <= 2
\t\t * #{ managed } <= 1, #{ AP, P2P-client, P2P-GO } <= 1, #{ P2P-device } <= 1,
\t\t   total <= 3, #channels <= 1
\tHT Capability overrides:
\t\t * MCS: ff ff ff ff ff ff ff ff ff ff
\tDevice supports TX status socket option.
";

    const NO_AP: &str = "Wiphy phy0
\tSupported interface modes:
\t\t * managed
\t\t * monitor
\tvalid interface combinations:
\t\t * #{ managed } <= 1, total <= 1, #channels <= 1
\tBand 1:
";

    const DEV_CONNECTED: &str = "phy#0
\tUnnamed/non-netdev interface
\t\twdev 0x6
\t\ttype P2P-device
\tInterface wlan0
\t\tifindex 3
\t\ttype managed
\t\tchannel 9 (2452 MHz), width: 40 MHz, center1: 2442 MHz
\t\ttxpower 22.00 dBm
";

    const DEV_IDLE: &str = "phy#0
\tInterface wlan0
\t\tifindex 3
\t\ttype managed
";

    #[test]
    fn the_interface_combinations_are_read() {
        let wiphys = parse_iw_list(ONE_CHANNEL);
        assert_eq!(wiphys.len(), 1);
        let phy = &wiphys[0];
        assert!(phy.supports_ap);
        assert_eq!(phy.combinations.len(), 2);
        assert_eq!(phy.combinations[0].channels, 2);
        assert!(!phy.combinations[0].allows_ap_and_managed(), "no AP in it");
        assert_eq!(phy.combinations[1].total, 3);
        assert_eq!(phy.combinations[1].groups.len(), 3);
        assert!(phy.combinations[1].allows_ap_and_managed());
        assert_eq!(phy.concurrent_channels(), Some(1));
        // A group holding both modes needs room for two.
        let one_group = parse_combination("#{ managed, AP } <= 1, total <= 2, #channels <= 1");
        assert!(!one_group.allows_ap_and_managed());
        let two = parse_combination("#{ managed, AP } <= 2, total <= 2, #channels <= 2");
        assert!(two.allows_ap_and_managed());
        assert!(!parse_iw_list(NO_AP)[0].supports_ap);
        assert_eq!(parse_iw_list(NO_AP)[0].concurrent_channels(), None);
        assert!(parse_iw_list("").is_empty());
        assert!(parse_iw_list("garbage\nnot iw").is_empty());
    }

    #[test]
    fn the_interfaces_and_their_channels_are_read() {
        let found = parse_iw_dev(DEV_CONNECTED);
        let wlan = found.iter().find(|i| i.name == "wlan0").unwrap();
        assert_eq!(
            (wlan.phy, wlan.kind.as_str(), wlan.channel),
            (0, "managed", Some(9))
        );
        assert_eq!(parse_iw_dev(DEV_IDLE)[0].channel, None);
        assert!(parse_iw_dev("").is_empty());
    }

    #[test]
    fn the_capability_to_add_an_interface_is_read_from_proc() {
        assert!(!can_add_interface_from_status(
            "Name:\tx\nCapEff:\t0000000000000000\n"
        ));
        // CAP_NET_ADMIN is bit 12.
        assert!(can_add_interface_from_status("CapEff:\t0000000000001000\n"));
        assert!(can_add_interface_from_status("CapEff:\t000001ffffffffff\n"));
        assert!(!can_add_interface_from_status(
            "CapEff:\t0000000000000fff\n"
        ));
        assert!(!can_add_interface_from_status("nothing here"));
    }

    #[test]
    fn a_second_interface_needs_the_card_and_the_privilege() {
        // The card allows it and this program may: beside, on the card's
        // own channel (the combination keeps one), nothing goes down.
        let plan = plan_from(ONE_CHANNEL, DEV_CONNECTED, true).unwrap();
        assert_eq!(plan.mode, Mode::SecondInterface);
        assert_eq!(plan.device, "wlan0");
        assert_eq!(plan.channel, Some(9));
        assert!(!plan.disconnects);
        // Without the privilege it takes over the card, and says so: the
        // card's connection goes down.
        let plan = plan_from(ONE_CHANNEL, DEV_CONNECTED, false).unwrap();
        assert_eq!(plan.mode, Mode::TakesOverCard);
        assert_eq!(plan.channel, None);
        assert!(plan.disconnects);
        // A card that is not connected loses nothing by it.
        let plan = plan_from(ONE_CHANNEL, DEV_IDLE, false).unwrap();
        assert_eq!(plan.mode, Mode::TakesOverCard);
        assert!(!plan.disconnects);
        // A card that allows two channels is not tied to the connection's.
        let two_channels = ONE_CHANNEL.replace("#channels <= 1", "#channels <= 2");
        assert_eq!(
            plan_from(&two_channels, DEV_CONNECTED, true)
                .unwrap()
                .channel,
            None
        );
        // One with no concurrent mode takes over even with the privilege.
        let plan = plan_from(NO_CONCURRENT_AP, DEV_CONNECTED, true).unwrap();
        assert_eq!(plan.mode, Mode::TakesOverCard);
        assert!(plan.disconnects);
    }

    /// An access point only on its own.
    const NO_CONCURRENT_AP: &str = "Wiphy phy0
\tSupported interface modes:
\t\t * managed
\t\t * AP
\tvalid interface combinations:
\t\t * #{ managed } <= 1, total <= 1, #channels <= 1
\t\t * #{ AP } <= 1, total <= 1, #channels <= 1
";

    #[test]
    fn no_card_or_one_that_cannot_host_makes_no_plan() {
        assert_eq!(plan_from("", "", true), Err(Unavailable::NoWifi));
        assert_eq!(
            plan_from(NO_AP, DEV_CONNECTED, true),
            Err(Unavailable::NoAccessPoint)
        );
        // A card whose phy is not the one listed is not guessed at.
        let other_phy = DEV_CONNECTED.replace("phy#0", "phy#7");
        assert_eq!(
            plan_from(ONE_CHANNEL, &other_phy, true),
            Err(Unavailable::NoAccessPoint)
        );
        assert!(!Unavailable::NoWifi.words().is_empty());
    }

    #[test]
    fn the_idle_timer_counts_from_the_last_use() {
        let mut timer = IdleTimer::new(10, 1_000);
        assert_eq!(timer.left(1_000), 600, "a phone has the limit to join");
        assert!(!timer.expired(1_599));
        assert!(timer.expired(1_600));
        // A phone using it puts the clock back.
        timer.tick(1_500, true);
        assert!(!timer.expired(1_600));
        assert_eq!(timer.left(1_600), 500);
        timer.tick(1_900, false);
        assert_eq!(timer.left(2_000), 100, "idle does not reset it");
        assert!(timer.expired(2_100));
        // A clock that went back does not turn it off early.
        assert_eq!(timer.left(0), 600);
        // The limit is held to its range.
        assert_eq!(IdleTimer::new(0, 0).left(0), 60);
        assert_eq!(IdleTimer::new(10_000, 0).left(0), 120 * 60);
    }

    #[test]
    fn only_a_takeover_of_a_connected_card_brings_something_back() {
        let was = "/org/freedesktop/NetworkManager/Settings/7";
        let restore = restore_for(Mode::TakesOverCard, "wlan0", Some(was.to_string())).unwrap();
        assert_eq!(restore.device, "wlan0");
        assert_eq!(restore.connection, was);
        // A second interface took nothing from the card.
        assert!(restore_for(Mode::SecondInterface, "wlan0", Some("/x/1".into())).is_none());
        // A card with no connection had none taken.
        assert!(restore_for(Mode::TakesOverCard, "wlan0", None).is_none());
        assert!(restore_for(Mode::TakesOverCard, "wlan0", Some("/".into())).is_none());
        assert!(restore_for(Mode::TakesOverCard, "wlan0", Some(String::new())).is_none());
    }

    #[test]
    fn what_a_server_made_is_kept_per_state_folder_and_cleared() {
        let dir = state_dir("owned");
        assert!(read_owned(&dir).is_none());
        let owned = Owned {
            uuid: "00000000-0000-4000-8000-000000000000".into(),
            interface: Some("pdlink0".into()),
            restore: Some(Restore {
                device: "wlan0".into(),
                connection: "/x/1".into(),
            }),
            hotspot: None,
        };
        write_owned(&dir, &owned).unwrap();
        assert_eq!(read_owned(&dir), Some(owned));
        // Another state folder knows nothing of it.
        assert!(read_owned(&state_dir("owned-other")).is_none());
        clear_owned(&dir);
        assert!(read_owned(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(state_dir("owned-other"));
    }

    fn hotspot(was_on: bool) -> HotspotRestore {
        HotspotRestore {
            was_on,
            settings: HotspotSettings {
                ssid: "home-hotspot".into(),
                passphrase: "k".repeat(16),
                band: Some(TETHERING_5),
            },
        }
    }

    #[test]
    fn the_owners_hotspot_settings_are_kept_with_what_the_server_made() {
        let dir = state_dir("owned-hotspot");
        let owned = Owned {
            uuid: "00000000-0000-4000-8000-000000000000".into(),
            interface: None,
            restore: None,
            hotspot: Some(hotspot(true)),
        };
        write_owned(&dir, &owned).unwrap();
        assert_eq!(read_owned(&dir), Some(owned));
        // A record an older server wrote has no hotspot and still reads.
        std::fs::write(
            dir.join(DIRECT_LINK_OWNED_FILE),
            r#"{"uuid":"u","interface":null,"restore":null}"#,
        )
        .unwrap();
        assert_eq!(read_owned(&dir).unwrap().hotspot, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_output_never_shows_the_owners_hotspot() {
        let text = format!("{:?} {:?}", hotspot(true), hotspot(false).settings);
        assert!(!text.contains("home-hotspot"), "{text}");
        assert!(!text.contains("kkkk"), "{text}");
    }

    #[test]
    fn a_hotspot_that_was_off_is_stopped_and_left_off() {
        let ours = "pocketdesk-ab12";
        assert_eq!(
            hotspot_restore_steps(&hotspot(false), ours, true),
            vec![HotspotStep::Stop, HotspotStep::Configure]
        );
        // Already stopped: only its settings go back.
        assert_eq!(
            hotspot_restore_steps(&hotspot(false), ours, false),
            vec![HotspotStep::Configure]
        );
    }

    #[test]
    fn a_hotspot_that_was_on_goes_back_on_with_its_settings() {
        assert_eq!(
            hotspot_restore_steps(&hotspot(true), "pocketdesk-ab12", true),
            vec![
                HotspotStep::Stop,
                HotspotStep::Configure,
                HotspotStep::Start
            ]
        );
    }

    #[test]
    fn a_hotspot_the_owner_has_set_up_since_is_left_alone() {
        assert!(hotspot_restore_steps(&hotspot(true), "home-hotspot", true).is_empty());
        assert!(hotspot_restore_steps(&hotspot(false), "", false).is_empty());
    }

    #[test]
    fn the_band_asked_of_the_mobile_hotspot_follows_what_the_card_can_host() {
        let all = |_: i32| true;
        let only_24 = |c: i32| c == TETHERING_24;
        let none = |_: i32| false;
        // Automatic is 2.4 GHz where there is one.
        assert_eq!(pick_tethering_band(Band::Auto, all), TETHERING_24);
        assert_eq!(pick_tethering_band(Band::Ghz24, all), TETHERING_24);
        assert_eq!(pick_tethering_band(Band::Ghz5, all), TETHERING_5);
        // A band the card cannot host is the card's to choose.
        assert_eq!(pick_tethering_band(Band::Ghz5, only_24), TETHERING_AUTO);
        assert_eq!(pick_tethering_band(Band::Auto, none), TETHERING_AUTO);
        assert_eq!(Band::Ghz24.tethering_code(), 1);
        assert_eq!(Band::Ghz5.tethering_code(), 2);
        assert_eq!(Band::Auto.tethering_code(), 0);
    }

    #[test]
    fn a_hotspot_windows_will_not_allow_says_why() {
        assert_eq!(unavailable_for_tethering_capability(0), None);
        assert_eq!(
            unavailable_for_tethering_capability(2),
            Some(Unavailable::NoAccessPoint)
        );
        assert_eq!(
            unavailable_for_tethering_capability(7),
            Some(Unavailable::NoAccessPoint)
        );
        for blocked in [1, 3, 4, 5, 6, 99] {
            assert_eq!(
                unavailable_for_tethering_capability(blocked),
                Some(Unavailable::Blocked),
                "{blocked}"
            );
        }
        // Every reason has words, and none says what Linux needs.
        for why in [
            Unavailable::NoConnectionProfile,
            Unavailable::Blocked,
            Unavailable::HotspotInUse,
        ] {
            assert!(!why.words().contains("NetworkManager"), "{why:?}");
        }
        assert!(
            Unavailable::NoConnectionProfile
                .words()
                .contains("internet connection")
        );
        for code in 1..=12 {
            assert!(!tethering_failure_words(code).is_empty());
        }
        assert!(tethering_failure_words(3).contains("Wi-Fi"));
        assert!(tethering_failure_words(8).contains("limited"));
    }

    #[test]
    fn the_computers_address_on_the_hotspot_is_the_one_that_appeared() {
        let a = |d: u8| Ipv4Addr::new(192, 0, 2, d);
        let lan = Ipv4Addr::new(192, 168, 1, 20);
        let gw = WINDOWS_HOTSPOT_GATEWAY;
        // The hotspot's range first, even beside another new private one.
        assert_eq!(
            pick_link_address(&[lan], &[lan, Ipv4Addr::new(10, 0, 0, 5), gw]),
            Some(gw)
        );
        // Else any new private address.
        assert_eq!(
            pick_link_address(&[lan], &[lan, Ipv4Addr::new(10, 0, 0, 5)]),
            Some(Ipv4Addr::new(10, 0, 0, 5))
        );
        // A public one (not an address a phone's network has) is never it.
        assert_eq!(pick_link_address(&[], &[a(1)]), None);
        // Nothing new: only the gateway, if it is there now.
        assert_eq!(pick_link_address(&[lan, gw], &[lan, gw]), Some(gw));
        assert_eq!(pick_link_address(&[lan], &[lan]), None);
        assert_eq!(pick_link_address(&[], &[]), None);
    }

    #[test]
    fn a_hotspot_that_cannot_be_put_back_is_never_recorded_as_the_owners() {
        let ok = hotspot(false);
        assert_eq!(hotspot_start_refusal(&ok), None);
        // On: a link would change it under what uses it.
        assert_eq!(
            hotspot_start_refusal(&hotspot(true)),
            Some(Unavailable::HotspotInUse.words())
        );
        // A failed read is empty, and an empty passphrase cannot be put back.
        let mut empty = hotspot(false);
        empty.settings.passphrase = String::new();
        assert!(
            hotspot_start_refusal(&empty)
                .unwrap()
                .contains("could not be put back")
        );
        let mut nameless = hotspot(false);
        nameless.settings.ssid = String::new();
        assert!(hotspot_start_refusal(&nameless).is_some());
        // A left-over link's name is not the owner's.
        let mut left = hotspot(false);
        left.settings.ssid = format!("{SSID_PREFIX}ab12");
        assert!(hotspot_start_refusal(&left).unwrap().contains("named like"));
    }

    #[test]
    fn the_plan_on_file_is_read_while_recent_and_not_after() {
        let dir = state_dir("plan");
        assert!(read_plan(&dir, 1_000).is_none());
        for plan in [
            Ok(Plan {
                device: "wlan0".into(),
                mode: Mode::TakesOverCard,
                channel: None,
                disconnects: true,
            }),
            Err(Unavailable::NoConnectionProfile),
        ] {
            let report = PlanReport {
                updated_unix: 1_000,
                plan,
            };
            write_plan(&dir, &report).unwrap();
            assert_eq!(read_plan(&dir, 1_000 + PLAN_FRESH_SECS), Some(report.plan));
            assert!(read_plan(&dir, 1_000 + PLAN_FRESH_SECS + 1).is_none());
        }
        std::fs::write(dir.join(DIRECT_LINK_PLAN_FILE), "junk").unwrap();
        assert!(read_plan(&dir, 1_000).is_none());
        clear_plan(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_windows_log_line_names_the_link_or_the_owners_hotspot() {
        // The Windows backend's source is read on every system: no logging
        // line in it carries a name, a passphrase or the settings.
        for line in include_str!("direct_link_windows.rs").lines() {
            let line = line.trim();
            if line.starts_with("tracing::") {
                for word in ["passphrase", "ssid", "join", "settings", "restore"] {
                    assert!(!line.contains(word), "{line}");
                }
            }
        }
    }

    #[test]
    fn a_link_lives_at_most_eight_hours_whatever_uses_it() {
        assert!(!lifetime_over(1_000, 1_000 + MAX_LIFETIME_SECS - 1));
        assert!(lifetime_over(1_000, 1_000 + MAX_LIFETIME_SECS));
        assert!(
            !lifetime_over(1_000, 0),
            "a clock that went back does not end it"
        );
    }

    fn state_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-dl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn requests_are_taken_once_and_only_when_fresh() {
        let dir = state_dir("requests");
        let ask = Request::new(Action::Start, Band::Ghz5, 15, true, 1_000);
        request(&dir, &ask).unwrap();
        let old = Request::new(Action::Start, Band::Auto, 10, false, 1_000 - 61);
        request(&dir, &old).unwrap();
        let future = Request::new(Action::Stop, Band::Auto, 10, false, 1_100);
        request(&dir, &future).unwrap();
        let taken = take_all(&dir, 1_000);
        assert_eq!(taken, vec![ask.clone()]);
        assert_eq!(taken[0].band, Band::Ghz5);
        assert!(taken[0].accept_takeover);
        assert!(take_all(&dir, 1_000).is_empty(), "taken once");
        // A file named for another id, or not a request, is dropped.
        let wrong = state_dir("requests-wrong").join(DIRECT_LINK_DIR);
        std::fs::create_dir_all(&wrong).unwrap();
        std::fs::write(
            wrong.join("other.json"),
            serde_json::to_string(&ask).unwrap(),
        )
        .unwrap();
        std::fs::write(wrong.join("junk.json"), "{").unwrap();
        assert!(take_all(wrong.parent().unwrap(), 1_000).is_empty());
        assert!(!wrong.join("other.json").exists(), "and removed");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(wrong.parent().unwrap());
    }

    #[test]
    fn a_request_holds_the_idle_minutes_to_their_range() {
        let ask = Request::new(Action::Start, Band::Auto, 0, false, 5);
        assert_eq!(ask.idle_minutes, MIN_IDLE_MINUTES);
        let ask = Request::new(Action::Start, Band::Auto, 5_000, false, 5);
        assert_eq!(ask.idle_minutes, MAX_IDLE_MINUTES);
        let bad = Request {
            id: "../x".into(),
            ..ask
        };
        assert!(request(&state_dir("badid"), &bad).is_err());
    }

    #[test]
    fn the_status_is_written_read_and_cleared() {
        let dir = state_dir("status");
        assert!(read_status(&dir).is_none());
        let status = Status {
            state: State::On,
            mode: Some(Mode::SecondInterface),
            wifi: Some(join("pocketdesk-ab12", "Abcdef123456")),
            address: Some("192.0.2.1".into()),
            started_unix: 100,
            updated_unix: 120,
            idle_minutes: 10,
            idle_left_secs: Some(500),
            in_use: true,
            error: None,
        };
        write_status(&dir, &status).unwrap();
        assert_eq!(read_status(&dir), Some(status.clone()));
        assert!(status.is_fresh(150));
        assert!(!status.is_fresh(120 + STATUS_FRESH_SECS + 1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(DIRECT_LINK_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "owner-only: it holds the passphrase");
        }
        clear_status(&dir);
        assert!(read_status(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Read-only: what this computer's card would do. Run by hand on a
    /// computer with Wi-Fi: `cargo test -p pocketdesk-core --features local
    /// direct_link::tests::this_computer -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads this computer's Wi-Fi card; changes nothing"]
    fn this_computer_says_what_a_link_would_do() {
        match probe() {
            Ok(plan) => println!(
                "PLAN mode={:?} disconnects={} channel={:?}",
                plan.mode, plan.disconnects, plan.channel
            ),
            Err(why) => println!("NO PLAN: {why:?}"),
        }
    }
}
