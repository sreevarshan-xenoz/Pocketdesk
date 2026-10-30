//! The owner's other computers this one paired with (2f), and the desktop
//! app's requests about them.
//!
//! The server keeps `<state>/computers.json` (owner-only): each computer it
//! paired with, what it pinned for it, its iroh endpoint and how its link
//! is doing, plus the last pairing's outcome. Only the server writes it.
//! The app asks through `<state>/computers_out/<id>.json`, one request per
//! file, taken (read and deleted) by the server: pair with a computer's
//! pairing link, or forget one. A pairing link holds a PIN, so requests are
//! owner-only too.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{COMPUTERS_DIR, COMPUTERS_FILE};

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 120;

/// One paired computer, as this one sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerComputer {
    pub device_id: String,
    pub name: String,
    /// Its addresses from the pairing link, best first.
    pub addresses: Vec<String>,
    pub control_port: u16,
    /// Its streaming port, from the pairing link (BK7: where its screen is
    /// shown from on the home network). Files written before read the default.
    #[serde(default = "default_stream_port")]
    pub stream_port: u16,
    /// Hex SHA-256 of its certificate, pinned for each address.
    pub fingerprint: String,
    /// Its iroh endpoint (`endpointId`, `relayUrls`, `directAddrs`, `dht`),
    /// from the link and then from its endpoint packets; `None` when it has
    /// no internet road.
    #[serde(default)]
    pub endpoint: Option<serde_json::Value>,
    pub paired_unix: u64,
    /// How its link is doing (`connecting`, `linked`, `refused`, or
    /// `away: <why>`).
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub link_unix: u64,
    /// Whether this computer lets it drive this one's keyboard and mouse
    /// (shared input). A computer that paired with this one has the
    /// Keyboard and mouse permission in the trust store instead; one this
    /// computer paired with has no entry there (an entry would let it
    /// connect as a paired device), so the grant is kept here. Off when
    /// paired, again too.
    #[serde(default)]
    pub lets_drive: bool,
    /// Paired both ways from a link made for another of the owner's
    /// computers (PC1, "This is another of my computers"): each computer
    /// lists the other here and trusts it, seeing and controlling it
    /// without asking. Only one of the two keeps the link between them
    /// ([`keeps_link`]). Off in files written before.
    #[serde(default)]
    pub mine: bool,
    /// How to wake it (PC6), as it said on its link: its adapter's MAC and
    /// broadcast address. `None` until it says, or once it says to forget.
    /// Owner-only, as the whole file is: a MAC identifies the computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<ComputerWake>,
    /// When its screen was last asked for here (PC6: unix seconds; 0
    /// never), so the Overview and the tray put the computers viewed most
    /// recently first.
    #[serde(default)]
    pub viewed_unix: u64,
    /// Whether the window that shows its screen here sends keys by their
    /// position (scancode mode, for games) rather than typing text: the
    /// window's "Keys by position", as last chosen for this computer.
    /// `None` until chosen, when Settings' `computer_view_scancodes` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys_by_position: Option<bool>,
}

/// `computers` with the one viewed most recently first (PC6), those never
/// viewed after, each group in the order they paired.
pub fn by_recent(computers: &[PeerComputer]) -> Vec<&PeerComputer> {
    let mut sorted: Vec<&PeerComputer> = computers.iter().collect();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.viewed_unix));
    sorted
}

/// How to wake a paired computer (PC6): what it told this one about the
/// adapter its link leaves by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerWake {
    /// `AA:BB:CC:DD:EE:FF`.
    pub mac: String,
    /// Its subnet's directed broadcast address (IPv4).
    pub broadcast: std::net::Ipv4Addr,
    /// Ethernet: Wi-Fi rarely wakes a computer.
    #[serde(default)]
    pub wired: bool,
    /// When it last said (unix seconds).
    #[serde(default)]
    pub told_unix: u64,
}

fn default_stream_port() -> u16 {
    crate::streaming::client::DEFAULT_STREAMING_PORT
}

/// Whether this computer (`own_id`) dials and keeps the link to `computer`
/// (PC1). It always does to one it paired with one way: it pasted that
/// one's link, and that one does not list it. Of two paired both ways, each
/// listing the other, only the one whose id sorts first dials; the other
/// takes that link as the computer it pasted would, and keeps none of its
/// own. Both work it out alike, from their two ids alone, so they agree
/// whichever pasted the other's link and in whatever order they start: one
/// link between them, never two.
pub fn keeps_link(own_id: &str, computer: &PeerComputer) -> bool {
    !computer.mine || own_id < computer.device_id.as_str()
}

/// What a computer pasting a link made for one of the owner's computers
/// tells the computer it pairs with about itself, inside its pairing ask
/// (PC1): what that computer needs to list it in turn and reach it. Its
/// certificate is not among them: the computer it pairs with takes that
/// from the connection itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerFacts {
    pub name: String,
    /// Its addresses, best first.
    pub addresses: Vec<String>,
    pub control_port: u16,
    pub stream_port: u16,
    /// Its iroh endpoint (`endpointId`, `relayUrls`, `directAddrs`), when
    /// it has the internet road.
    #[serde(default)]
    pub endpoint: Option<serde_json::Value>,
}

/// The most addresses a computer's facts are taken with.
const MOST_ADDRESSES: usize = 16;

impl ComputerFacts {
    /// The computer these facts describe, as the computer it paired with
    /// lists it: `device_id` from the connection, `fingerprint` (hex) the
    /// certificate it proved there. Its addresses are put in order (no
    /// loopback, link-local or IPv6), at most 16; a port of 0 is the
    /// default one. Its name and endpoint are taken as they are: the
    /// server cleans the one and checks the other.
    pub fn into_peer(self, device_id: &str, fingerprint: &str, now_unix: u64) -> PeerComputer {
        let mut addresses = super::pair_link::order_addresses(self.addresses);
        addresses.truncate(MOST_ADDRESSES);
        let or = |port: u16, default: u16| if port == 0 { default } else { port };
        PeerComputer {
            device_id: device_id.to_string(),
            name: self.name,
            addresses,
            control_port: or(self.control_port, crate::DEFAULT_CONTROL_PORT),
            stream_port: or(self.stream_port, default_stream_port()),
            fingerprint: fingerprint.to_ascii_lowercase(),
            endpoint: self.endpoint,
            paired_unix: now_unix,
            link: "connecting".to_string(),
            link_unix: now_unix,
            lets_drive: false,
            mine: true,
            wake: None,
            viewed_unix: 0,
            keys_by_position: None,
        }
    }
}

/// How the last pairing went, for the app to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingOutcome {
    /// The name the link gave.
    pub name: String,
    /// `paired`, or `failed: <why>`.
    pub outcome: String,
    pub unix: u64,
    /// The request it answers ([`ComputerRequest::id`]).
    #[serde(default)]
    pub request: String,
    /// The computer its link named (BD3). Absent in files written before.
    #[serde(default)]
    pub device_id: String,
}

impl PairingOutcome {
    /// Whether this is the pairing of `computer` (BD3: its line, and any
    /// address a failure named, outlived Unpair). One written before the
    /// id was kept is matched by name.
    pub fn is_of(&self, computer: &PeerComputer) -> bool {
        if self.device_id.is_empty() {
            self.name == computer.name
        } else {
            self.device_id == computer.device_id
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputersFile {
    #[serde(default)]
    pub computers: Vec<PeerComputer>,
    #[serde(default)]
    pub last_pairing: Option<PairingOutcome>,
    /// The computers that paired here both ways (PC1) whose grant waits for
    /// the link between the two: until it comes up, which shows the other
    /// computer recorded the pairing too, each is trusted as a computer
    /// paired one way (PC1 review M1). Absent in files written before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    /// The computers whose grant both ways waited for the link longer than
    /// [`PENDING_FOR`] and lapsed (PC1 re-review): each stays paired one
    /// way here until it pairs again. Absent in files written before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lapsed: Vec<String>,
    /// This computer's ask of a nearby one, while it goes and as it ended
    /// ([`super::nearby`]). Absent in files written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_out: Option<super::nearby::NearbyAsk>,
    /// A nearby computer's ask on this one's screen, until answered or run
    /// out ([`super::nearby`]). Absent in files written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_in: Option<super::nearby::NearbyRequest>,
    /// When a nearby pairing ask last failed here (a wrong code or a
    /// dropped connection, [`super::nearby::HostDesk::failed`]), for the
    /// owner's note.
    /// Absent in files written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_tried: Option<u64>,
    /// Until when (unix seconds) nearby asks are paused here, after too
    /// many failed or shown asks ([`super::nearby::HostDesk::paused_until`]).
    /// Absent in files written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nearby_paused_until: Option<u64>,
    /// The Pocketdesk release each listed computer said it runs (item 4),
    /// by device id: from the identity that admits this one on the link it
    /// dials. A computer that dials this one says it in the trust store
    /// instead. Absent in files written before.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub versions: BTreeMap<String, String>,
    /// Remote revoke: this computer's last ask of another computer's
    /// devices ("Devices there…"), and what it answered. Absent in files
    /// written before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub there: Option<DevicesThere>,
}

/// This computer's ask of the devices paired with another of the owner's
/// computers (remote revoke), as the server keeps it for the app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesThere {
    /// The computer asked.
    pub device_id: String,
    /// The ask on the wire waiting for its answer
    /// ([`crate::protocol::remote_devices::Request::request`]); empty once
    /// answered.
    #[serde(default)]
    pub request: String,
    /// When it was asked (unix seconds).
    pub asked_unix: u64,
    /// The device a removal asked for, while it waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removing: Option<String>,
    /// The devices there, as last listed.
    #[serde(default)]
    pub devices: Vec<crate::protocol::remote_devices::ThereDevice>,
    /// When the list was last answered (unix seconds), 0 never.
    #[serde(default)]
    pub listed_unix: u64,
    /// What the last removal came to: `removed <name>`, or `refused:
    /// <code>` ([`crate::protocol::remote_devices::Refusal::code`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The computer refused to answer this one at all (`not_owner`), or
    /// it could not be asked (`not_linked`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// How long a grant both ways waits for the link before it lapses (PC1).
pub const PENDING_FOR: u64 = 10 * 60;

impl ComputersFile {
    /// The release the listed computer `device_id` last said it runs.
    pub fn version(&self, device_id: &str) -> Option<&str> {
        self.computers
            .iter()
            .any(|c| c.device_id == device_id)
            .then(|| self.versions.get(device_id).map(String::as_str))
            .flatten()
    }

    /// Keep the release the listed computer `device_id` says it runs,
    /// cleaned; a computer no longer listed is dropped. Whether it changed.
    pub fn note_version(&mut self, device_id: &str, raw: &str) -> bool {
        let listed = |id: &str| self.computers.iter().any(|c| c.device_id == id);
        let before = self.versions.len();
        let kept: BTreeMap<String, String> = std::mem::take(&mut self.versions)
            .into_iter()
            .filter(|(id, _)| listed(id))
            .collect();
        self.versions = kept;
        let pruned = self.versions.len() != before;
        let Some(version) = crate::version::clean_version(raw).filter(|_| listed(device_id)) else {
            return pruned;
        };
        let changed = self.versions.get(device_id) != Some(&version);
        self.versions.insert(device_id.to_string(), version);
        changed || pruned
    }

    /// Let each grant both ways that has waited for the link longer than
    /// [`PENDING_FOR`] since its computer paired (`paired_unix`) lapse at
    /// `now_unix`, or one whose computer is gone; the lapsed ones. A lapsed
    /// computer keeps `mine` (its listing, Forget and the link's direction
    /// stay as they were), but is no longer [`Self::both_ways`] (range
    /// review M2, PR #572 review I3).
    pub fn lapse_pending(&mut self, now_unix: u64) -> Vec<String> {
        let paired = |id: &str| {
            self.computers
                .iter()
                .find(|c| c.device_id == id)
                .map(|c| c.paired_unix)
        };
        let lapsed: Vec<String> = self
            .pending
            .iter()
            .filter(|id| paired(id).is_none_or(|at| now_unix.saturating_sub(at) >= PENDING_FOR))
            .cloned()
            .collect();
        self.pending.retain(|id| !lapsed.contains(id));
        for id in &lapsed {
            if paired(id).is_some() && !self.lapsed.contains(id) {
                self.lapsed.push(id.clone());
            }
        }
        lapsed
    }

    /// Whether `device_id` is paired both ways now: listed as `mine`, and
    /// its grant not lapsed (PR #572 review I3). Only such a computer is
    /// told this one's MAC and the link's both-ways facts.
    pub fn both_ways(&self, device_id: &str) -> bool {
        self.computers
            .iter()
            .any(|c| c.device_id == device_id && c.mine)
            && !self.lapsed.iter().any(|id| id == device_id)
    }

    /// Whether `device_id` is listed here with a certificate other than
    /// `fingerprint` (hex, either case): a link or a pairing that reuses a
    /// listed computer's id with another certificate is someone else's
    /// (PC1 review C1, M2), and is refused until that computer is unpaired.
    pub fn listed_with_other_certificate(&self, device_id: &str, fingerprint: &str) -> bool {
        self.computers
            .iter()
            .any(|c| c.device_id == device_id && !c.fingerprint.eq_ignore_ascii_case(fingerprint))
    }

    /// Take the computer `device_id` out, and its pairing's line with it
    /// (BD3: the line, and any address a failure named, outlived Unpair).
    pub fn remove(&mut self, device_id: &str) -> Option<PeerComputer> {
        let at = self
            .computers
            .iter()
            .position(|c| c.device_id == device_id)?;
        let computer = self.computers.remove(at);
        self.pending.retain(|id| id != device_id);
        self.lapsed.retain(|id| id != device_id);
        // Its devices' list goes with it (remote revoke).
        if self
            .there
            .as_ref()
            .is_some_and(|t| t.device_id == device_id)
        {
            self.there = None;
        }
        if self
            .last_pairing
            .as_ref()
            .is_some_and(|l| l.is_of(&computer))
        {
            self.last_pairing = None;
        }
        Some(computer)
    }
}

/// What the app asks of the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum ComputerAsk {
    /// Pair with the computer whose pairing link (`pocketdesk://pair?...`)
    /// this is. `mine`: the owner confirmed here, for a link that asks to
    /// pair both ways (PC1), that the computer it names may see and control
    /// this one at any time without asking. Without it such a link pairs one
    /// way, and this computer trusts nothing of the link's maker.
    Pair {
        link: String,
        #[serde(default)]
        mine: bool,
    },
    /// Forget a paired computer.
    Forget { device_id: String },
    /// Forget the computer `device_id` only as it was listed when this was
    /// asked (its `paired_unix`): the server's own ask when that computer
    /// paired here the other way (BD1), which a pairing since must outlive
    /// (PC1 review M5).
    ForgetPaired { device_id: String, paired_unix: u64 },
    /// Let a computer this one paired with drive this one's keyboard and
    /// mouse, or not ([`PeerComputer::lets_drive`]).
    LetDrive { device_id: String, allow: bool },
    /// Show a paired computer's screen in a window here (BK7): the server
    /// dials it and relays its picture to the app ([`super::view`]).
    /// `monitor` is which of its screens, from 0. A newer view replaces an
    /// older one. `hevc`: this app's ffmpeg decodes HEVC and its
    /// settings let a view ask for it (PC5); an app before PC5 says not, so
    /// its views stay H.264.
    View {
        device_id: String,
        #[serde(default)]
        monitor: u32,
        #[serde(default)]
        hevc: bool,
        /// Which of the app's screen windows asks (PC11): 0 the first, the
        /// one every app before PC11 has; up to [`super::view::MAX_VIEWS`]
        /// views are held at once, one a window, and a window's new ask
        /// replaces only its own view.
        #[serde(default, skip_serializing_if = "is_zero")]
        window: u8,
    },
    /// Ask the nearby computer `device_id` (found by its announcement, at
    /// `address`, control port `port`) to pair ([`super::nearby`]). `mine`:
    /// the owner here said both are theirs (PC1), which pairs both ways
    /// only if its owner says so too.
    PairNearby {
        device_id: String,
        name: String,
        address: String,
        port: u16,
        #[serde(default)]
        mine: bool,
    },
    /// Send the magic packet that wakes the computer `device_id` (PC6),
    /// as it told this one ([`PeerComputer::wake`]).
    Wake { device_id: String },
    /// Remember for the computer `device_id` whether its screen window
    /// sends keys by position ([`PeerComputer::keys_by_position`]), as the
    /// window's bar was switched.
    KeysByPosition { device_id: String, on: bool },
    /// Stop this computer's ask of a nearby one.
    CancelNearby,
    /// The owner here confirmed that both screens show the code of this
    /// computer's ask `nearby` (its [`super::nearby::NearbyAsk::request`]):
    /// nothing pairs before, whatever the other computer answers.
    ConfirmNearby { nearby: String },
    /// The owner's answer to a nearby computer's ask `nearby` (its
    /// [`super::nearby::NearbyRequest::id`]; not `id`, the request's own)
    /// on this one's screen; `mine`, with Accept: both are theirs (PC1).
    AnswerNearby {
        nearby: String,
        accept: bool,
        #[serde(default)]
        mine: bool,
    },
    /// List the devices paired with the computer `device_id`, one of the
    /// owner's paired both ways (remote revoke): the answer goes in
    /// [`ComputersFile::there`].
    DevicesThere { device_id: String },
    /// Remove the device `remove` from the computer `device_id` (remote
    /// revoke), as its own Devices page's Unpair would.
    RemoveThere { device_id: String, remove: String },
    /// Forget what the computer `device_id` listed (remote revoke): its
    /// "Devices there…" was closed, and the names of its devices are not
    /// kept here past that.
    CloseThere { device_id: String },
    /// Stop showing the computer's screen (its window closed): the view
    /// `session_id` (`view.json`'s) when the window knew it, else the one
    /// shown now. A Stop for a view since replaced stops nothing (BK7
    /// review M1). `lock`: the window's lock on close (PC8) did not reach
    /// the view relay (no connection as it closed, between joins, or its
    /// write failed), so the server asks that computer to lock as it stops
    /// the view, and says here when it could not (PR #568 review I2).
    StopView {
        #[serde(default)]
        session_id: Option<u64>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        lock: bool,
        /// The window that closed (PC11): with no `session_id`, its view
        /// is the one stopped.
        #[serde(default, skip_serializing_if = "is_zero")]
        window: u8,
    },
}

/// Whether a window number is the first window's, left out of a request.
fn is_zero(n: &u8) -> bool {
    *n == 0
}

/// One request, as a file in `computers_out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerRequest {
    pub id: String,
    #[serde(flatten)]
    pub ask: ComputerAsk,
    pub created_unix: u64,
    /// The milliseconds past `created_unix`, so requests left in one
    /// second are taken in order (BK7 review M1). 0 in files from before.
    #[serde(default)]
    pub created_ms: u16,
}

/// The computers and the last pairing. A missing or unreadable file is
/// empty.
pub fn read(state_dir: &Path) -> ComputersFile {
    try_read(state_dir).unwrap_or_default()
}

/// [`read`], but a file that is there and does not read is an error.
pub fn try_read(state_dir: &Path) -> io::Result<ComputersFile> {
    match std::fs::read_to_string(state_dir.join(COMPUTERS_FILE)) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ComputersFile::default()),
        Err(e) => Err(e),
    }
}

/// The server's writes, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

/// Change the file under `change` (the server's side), written whole and
/// owner-only, so a reader never sees it half-written. A file that does not
/// read is left alone (an error), never replaced by an empty one.
pub fn update(state_dir: &Path, change: impl FnOnce(&mut ComputersFile)) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = try_read(state_dir)?;
    let before = file.clone();
    change(&mut file);
    if file == before && state_dir.join(COMPUTERS_FILE).exists() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let path = state_dir.join(COMPUTERS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Leave a request for the server (the app's side).
pub fn request(state_dir: &Path, ask: ComputerAsk, now_unix: u64) -> io::Result<ComputerRequest> {
    let created_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_millis() as u16);
    let request = ComputerRequest {
        id: uuid::Uuid::new_v4().to_string(),
        ask,
        created_unix: now_unix,
        created_ms,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(COMPUTERS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    if let Err(e) = std::fs::rename(&tmp, &file) {
        // Not left behind (BD3 review): a Pair request holds the PIN.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first. Each file
/// is removed once read; one that is stale, from the future, unreadable or
/// named for another request is dropped, and so is one left half-written
/// past a request's life (BD3: a crash mid-write left it for good, a Pair
/// one with the PIN).
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<ComputerRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(COMPUTERS_DIR)) else {
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
            // A temp file mid-write, or left by a crash once it is stale.
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|at| now_unix.saturating_sub(at.as_secs()) > REQUEST_TTL_SECS);
            // A claimed one left by a crash holds a Pair request's PIN too.
            if stale && path.extension().is_some_and(|e| e == "tmp" || e == "taken") {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        };
        // Claimed before it is read (PR #553 review I1): a request the
        // app withdrew meanwhile is not there to claim, and one claimed is
        // not there to withdraw.
        let Some(claimed) = claim(&path) else {
            continue;
        };
        let Some(request) = read_claimed(&claimed) else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| (r.created_unix, r.created_ms));
    taken
}

/// Claim the request at `path` for the server, atomically: renamed out of
/// the app's reach, so [`withdraw`] and the server never both win. Where
/// it went, or none when it was withdrawn (or claimed) first.
fn claim(path: &Path) -> Option<PathBuf> {
    let claimed = path.with_extension("taken");
    std::fs::rename(path, &claimed).ok().map(|()| claimed)
}

/// Read a claimed request, which goes once read.
fn read_claimed(claimed: &Path) -> Option<ComputerRequest> {
    let raw = std::fs::read_to_string(claimed);
    let _ = std::fs::remove_file(claimed);
    raw.ok()
        .and_then(|raw| serde_json::from_str::<ComputerRequest>(&raw).ok())
}

/// Take back the request `id` before the server takes it (the app's
/// Cancel): true when it was still waiting, so the server never sees it;
/// false when the server had claimed it already (or it is gone).
pub fn withdraw(state_dir: &Path, id: &str) -> io::Result<bool> {
    match std::fs::remove_file(request_path(state_dir, id)?) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn request_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir.join(COMPUTERS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UX 1: a request taken back before the server takes it never
    /// reaches it; once taken, there is nothing to take back.
    /// Pair a computer nearby: each of its asks is taken as it was left,
    /// none of its fields mistaken for the request's own id.
    #[test]
    fn nearby_asks_are_taken_as_left() {
        let dir = temp_dir("nearby-asks");
        let now = crate::local::pin::now_unix();
        let asks = [
            ComputerAsk::PairNearby {
                device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
                name: "Test Tower".to_string(),
                address: "192.0.2.40".to_string(),
                port: 1716,
                mine: true,
            },
            ComputerAsk::CancelNearby,
            ComputerAsk::ConfirmNearby {
                nearby: "r-1".to_string(),
            },
            ComputerAsk::AnswerNearby {
                nearby: "0a1b2c3d4e5f6071".to_string(),
                accept: true,
                mine: false,
            },
        ];
        for ask in asks {
            let left = request(&dir, ask, now).unwrap();
            assert_eq!(take_all(&dir, now), [left]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_waiting_request_can_be_withdrawn() {
        let dir = temp_dir("withdraw");
        let now = crate::local::pin::now_unix();
        let ask = ComputerAsk::Forget {
            device_id: "b".to_string(),
        };
        let first = request(&dir, ask.clone(), now).unwrap();
        assert!(withdraw(&dir, &first.id).unwrap());
        assert!(take_all(&dir, now).is_empty(), "withdrawn");
        let second = request(&dir, ask, now).unwrap();
        assert_eq!(take_all(&dir, now).len(), 1);
        assert!(!withdraw(&dir, &second.id).unwrap(), "taken already");
        assert!(withdraw(&dir, "../x").is_err(), "not a request id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #553 review I1: the server claims a request before it reads
    /// it, so Cancel and the server never both win. Claimed first, Cancel
    /// is too late; withdrawn first, there is nothing to claim.
    #[test]
    fn a_request_is_claimed_or_withdrawn_never_both() {
        let dir = temp_dir("claim");
        let now = crate::local::pin::now_unix();
        let ask = ComputerAsk::Forget {
            device_id: "b".to_string(),
        };
        let first = request(&dir, ask.clone(), now).unwrap();
        let path = request_path(&dir, &first.id).unwrap();
        let claimed = claim(&path).expect("claimed");
        assert!(!withdraw(&dir, &first.id).unwrap(), "too late: it is taken");
        assert_eq!(read_claimed(&claimed).map(|r| r.id), Some(first.id));
        assert!(!claimed.exists(), "read once, then gone");
        let second = request(&dir, ask, now).unwrap();
        let path = request_path(&dir, &second.id).unwrap();
        assert!(withdraw(&dir, &second.id).unwrap());
        assert!(claim(&path).is_none(), "withdrawn: nothing to claim");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-computers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn computer(id: &str) -> PeerComputer {
        PeerComputer {
            device_id: id.to_string(),
            name: "Desk".to_string(),
            addresses: vec!["192.0.2.10".to_string()],
            control_port: 1716,
            stream_port: 4716,
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 100,
            link: String::new(),
            link_unix: 0,
            lets_drive: false,
            mine: false,
            wake: None,
            viewed_unix: 0,
            keys_by_position: None,
        }
    }

    /// PC1: of two computers paired both ways, exactly one keeps the link,
    /// the same one whichever pasted the other's link and in whatever
    /// order they look; to one paired one way, the one that pasted always
    /// does.
    #[test]
    fn one_link_is_kept_between_two_computers_in_every_order() {
        let ids = ["0a-desk", "7f-laptop", "c3-tower"];
        for a in ids {
            for b in ids.iter().copied().filter(|b| *b != a) {
                let (b_on_a, a_on_b) = (
                    PeerComputer {
                        mine: true,
                        ..computer(b)
                    },
                    PeerComputer {
                        mine: true,
                        ..computer(a)
                    },
                );
                let dialers = [keeps_link(a, &b_on_a), keeps_link(b, &a_on_b)];
                assert_eq!(
                    dialers.iter().filter(|d| **d).count(),
                    1,
                    "{a} and {b}: {dialers:?}"
                );
                assert_eq!(keeps_link(a, &b_on_a), a < b, "the first id dials");
                // One way: only the computer that pasted lists the other,
                // and it dials, whichever id sorts first.
                assert!(keeps_link(a, &computer(b)));
            }
        }
        // A file from before PC1 reads as one way.
        let raw = serde_json::to_string(&computer("desk")).unwrap();
        let old: PeerComputer = serde_json::from_str(&raw.replace(",\"mine\":false", "")).unwrap();
        assert!(!old.mine);
    }

    /// PC1: what a computer says of itself as it pairs both ways becomes
    /// its entry on the other: its addresses in order (no loopback), a
    /// port of 0 the default, the certificate the connection proved, and
    /// the entry marked as paired both ways.
    #[test]
    fn a_computers_facts_become_its_entry_on_the_other() {
        let facts: ComputerFacts = serde_json::from_value(serde_json::json!({
            "name": "Laptop",
            "addresses": ["127.0.0.1", "100.80.1.2", "192.0.2.40", "fe80::1"],
            "controlPort": 1716,
            "streamPort": 0,
        }))
        .unwrap();
        assert_eq!(facts.endpoint, None, "no internet road");
        let peer = facts.into_peer("lap", &"AB".repeat(32), 50);
        assert_eq!(peer.device_id, "lap");
        assert_eq!(peer.name, "Laptop");
        assert_eq!(peer.addresses, ["192.0.2.40", "100.80.1.2"]);
        assert_eq!(peer.control_port, 1716);
        assert_eq!(
            peer.stream_port,
            crate::streaming::client::DEFAULT_STREAMING_PORT
        );
        assert_eq!(peer.fingerprint, "ab".repeat(32));
        assert!(peer.mine && !peer.lets_drive);
        assert_eq!(peer.link, "connecting");
        let many = ComputerFacts {
            name: "Many".to_string(),
            addresses: (1..=40).map(|n| format!("192.0.2.{n}")).collect(),
            control_port: 1,
            stream_port: 2,
            endpoint: None,
        };
        assert_eq!(many.into_peer("m", "", 1).addresses.len(), 16);
    }

    #[test]
    fn letting_a_computer_drive_is_asked_and_kept() {
        let dir = temp_dir("let-drive");
        let ask = ComputerAsk::LetDrive {
            device_id: "desk-b".to_string(),
            allow: true,
        };
        let asked = request(&dir, ask.clone(), 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        // A file from before the switch reads as not letting it.
        let raw = serde_json::to_string(&computer("desk-b")).unwrap();
        let raw = raw.replace(",\"lets_drive\":false", "");
        let old: PeerComputer = serde_json::from_str(&raw).unwrap();
        assert!(!old.lets_drive);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_request_is_taken_once_and_a_stale_one_never() {
        let dir = temp_dir("requests");
        assert!(take_all(&dir, 100).is_empty(), "nothing asked yet");
        let pair = request(
            &dir,
            ComputerAsk::Pair {
                link: "pocketdesk://pair?v=1".to_string(),
                mine: false,
            },
            100,
        )
        .unwrap();
        let forget = request(
            &dir,
            ComputerAsk::Forget {
                device_id: "desk".to_string(),
            },
            101,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 102), [pair, forget]);
        assert!(take_all(&dir, 102).is_empty(), "taken");

        request(
            &dir,
            ComputerAsk::Forget {
                device_id: "desk".to_string(),
            },
            100,
        )
        .unwrap();
        assert!(
            take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "stale"
        );
        request(
            &dir,
            ComputerAsk::Forget {
                device_id: "desk".to_string(),
            },
            500,
        )
        .unwrap();
        assert!(take_all(&dir, 400).is_empty(), "from the future");
    }

    /// BK7: a view is asked for a computer and one of its screens, and
    /// ended; a computer from a file written before has the default stream
    /// port.
    #[test]
    fn a_view_is_asked_and_stopped() {
        let dir = temp_dir("view");
        let view = request(
            &dir,
            ComputerAsk::View {
                device_id: "desk".to_string(),
                monitor: 1,
                hevc: true,
                window: 0,
            },
            100,
        )
        .unwrap();
        let stop = request(
            &dir,
            ComputerAsk::StopView {
                session_id: Some(9),
                lock: false,
                window: 0,
            },
            101,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 102), [view, stop]);
        let wire = serde_json::to_value(ComputerRequest {
            id: "a".to_string(),
            ask: ComputerAsk::StopView {
                session_id: Some(9),
                lock: false,
                window: 0,
            },
            created_unix: 1,
            created_ms: 2,
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"id": "a", "ask": "stop_view", "session_id": 9, "created_unix": 1, "created_ms": 2})
        );
        let asked: ComputerRequest =
            serde_json::from_str(r#"{"id":"a","ask":"view","device_id":"desk","created_unix":1}"#)
                .unwrap();
        assert_eq!(
            asked.ask,
            ComputerAsk::View {
                device_id: "desk".to_string(),
                monitor: 0,
                hevc: false,
                window: 0,
            },
            "the first screen unless named, and no HEVC from an app before PC5"
        );
        let raw = serde_json::to_string(&computer("desk")).unwrap();
        let old: PeerComputer =
            serde_json::from_str(&raw.replace(",\"stream_port\":4716", "")).unwrap();
        assert_eq!(old.stream_port, 4716);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC6: the computers viewed most recently come first, those never
    /// viewed after in the order they paired; a file from before reads as
    /// never viewed.
    #[test]
    fn the_computers_viewed_last_come_first() {
        let viewed = |id: &str, at: u64| PeerComputer {
            viewed_unix: at,
            ..computer(id)
        };
        let list = [
            viewed("a", 0),
            viewed("b", 50),
            viewed("c", 0),
            viewed("d", 90),
        ];
        let ids: Vec<&str> = by_recent(&list)
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(ids, ["d", "b", "a", "c"]);
        let raw = serde_json::to_string(&computer("a")).unwrap();
        let old: PeerComputer =
            serde_json::from_str(&raw.replace(",\"viewed_unix\":0", "")).unwrap();
        assert_eq!(old.viewed_unix, 0);
    }

    /// PC6: a wake is asked for a computer by its id; a computer from a
    /// file written before knows no way to wake it.
    #[test]
    fn a_wake_is_asked_for_a_computer() {
        let dir = temp_dir("wake");
        let ask = ComputerAsk::Wake {
            device_id: "desk".to_string(),
        };
        let asked = request(&dir, ask.clone(), 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert_eq!(
            serde_json::to_value(&ask).unwrap(),
            serde_json::json!({"ask": "wake", "device_id": "desk"})
        );
        let raw = serde_json::to_string(&computer("desk")).unwrap();
        assert!(!raw.contains("wake"), "nothing known is not written");
        let old: PeerComputer = serde_json::from_str(&raw).unwrap();
        assert_eq!(old.wake, None);
        let told = PeerComputer {
            wake: Some(ComputerWake {
                mac: "00:00:5E:00:53:01".to_string(),
                broadcast: std::net::Ipv4Addr::new(192, 0, 2, 255),
                wired: true,
                told_unix: 5,
            }),
            ..computer("desk")
        };
        let back: PeerComputer =
            serde_json::from_str(&serde_json::to_string(&told).unwrap()).unwrap();
        assert_eq!(back, told);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keys by position: the window's choice for a computer is asked by its
    /// id and kept on its entry; a file from before, or a computer never
    /// switched, has no choice of its own.
    #[test]
    fn keys_by_position_is_asked_and_kept_for_a_computer() {
        let dir = temp_dir("keys-by-position");
        let ask = ComputerAsk::KeysByPosition {
            device_id: "desk".to_string(),
            on: true,
        };
        let asked = request(&dir, ask.clone(), 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert_eq!(
            serde_json::to_value(&ask).unwrap(),
            serde_json::json!({"ask": "keys_by_position", "device_id": "desk", "on": true})
        );
        let raw = serde_json::to_string(&computer("desk")).unwrap();
        assert!(
            !raw.contains("keys_by_position"),
            "no choice is not written"
        );
        let old: PeerComputer = serde_json::from_str(&raw).unwrap();
        assert_eq!(old.keys_by_position, None);
        for chosen in [Some(true), Some(false)] {
            let kept = PeerComputer {
                keys_by_position: chosen,
                ..computer("desk")
            };
            let back: PeerComputer =
                serde_json::from_str(&serde_json::to_string(&kept).unwrap()).unwrap();
            assert_eq!(back, kept);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// BK7 review M1: requests left in one second are taken in the order
    /// they were left, by their milliseconds, so a window closed and opened
    /// again at once is not ended by its own Stop.
    #[test]
    fn requests_in_one_second_keep_their_order() {
        // The milliseconds come from the clock while the second is the
        // test's own, so a run that crosses into the clock's next second (a
        // slow machine's sleeps can take 15 ms) cannot keep its order: such
        // a run is tried again, and one that stays in its second must.
        let mut kept = false;
        for attempt in 0..10 {
            let dir = temp_dir(&format!("order-{attempt}"));
            let mut asked = Vec::new();
            for n in 0..12 {
                let ask = if n % 2 == 0 {
                    ComputerAsk::StopView {
                        session_id: None,
                        lock: false,
                        window: 0,
                    }
                } else {
                    ComputerAsk::View {
                        device_id: format!("desk-{n}"),
                        monitor: 0,
                        hevc: false,
                        window: 0,
                    }
                };
                asked.push(request(&dir, ask, 100).unwrap());
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
            let in_one_second = asked.windows(2).all(|w| w[0].created_ms < w[1].created_ms);
            if in_one_second {
                assert_eq!(take_all(&dir, 100), asked);
                kept = true;
            }
            let _ = std::fs::remove_dir_all(&dir);
            if kept {
                break;
            }
        }
        assert!(kept, "ten runs each crossed a second");
        // One from before the milliseconds were kept reads as 0 of them.
        let old: ComputerRequest =
            serde_json::from_str(r#"{"id":"a","ask":"stop_view","created_unix":1}"#).unwrap();
        assert_eq!(old.created_ms, 0);
        assert_eq!(
            old.ask,
            ComputerAsk::StopView {
                session_id: None,
                lock: false,
                window: 0
            }
        );
        // The lock rides the stop only when asked (PR #568 review I2).
        let locking = serde_json::to_value(ComputerAsk::StopView {
            session_id: None,
            lock: true,
            window: 0,
        })
        .unwrap();
        assert_eq!(
            locking,
            serde_json::json!({"ask": "stop_view", "session_id": null, "lock": true})
        );
    }

    /// PC1 review C1: a pairing ask says whether the owner here let the
    /// link's maker see and control this computer; one from before says not.
    #[test]
    fn a_pairing_ask_carries_the_owners_consent_here() {
        let old: ComputerRequest = serde_json::from_str(
            r#"{"id":"a","ask":"pair","link":"pocketdesk://pair?v=1&mine=1","created_unix":1}"#,
        )
        .unwrap();
        assert_eq!(
            old.ask,
            ComputerAsk::Pair {
                link: "pocketdesk://pair?v=1&mine=1".to_string(),
                mine: false,
            },
            "a link's own mine=1 is not this owner's consent"
        );
        let wire = serde_json::to_value(ComputerAsk::Pair {
            link: "l".to_string(),
            mine: true,
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"ask": "pair", "link": "l", "mine": true})
        );
    }

    #[test]
    fn a_request_names_what_it_asks_on_disk() {
        let wire = serde_json::to_value(ComputerRequest {
            id: "a".to_string(),
            ask: ComputerAsk::Forget {
                device_id: "desk".to_string(),
            },
            created_unix: 1,
            created_ms: 0,
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"id": "a", "ask": "forget", "device_id": "desk", "created_unix": 1, "created_ms": 0})
        );
    }

    /// BD3: the last pairing is a computer's by the id its link named, or
    /// by name in a file written before the id was kept.
    #[test]
    fn the_last_pairing_is_known_by_its_computer() {
        let outcome = |device_id: &str, name: &str| PairingOutcome {
            name: name.to_string(),
            outcome: "paired".to_string(),
            unix: 1,
            request: "a".to_string(),
            device_id: device_id.to_string(),
        };
        let desk = computer("desk");
        assert!(outcome("desk", "Other").is_of(&desk));
        assert!(
            !outcome("lap", &desk.name).is_of(&desk),
            "another computer, named alike"
        );
        assert!(outcome("", &desk.name).is_of(&desk));
        assert!(!outcome("", "Other").is_of(&desk));
        let old: PairingOutcome =
            serde_json::from_str(r#"{"name":"Desk","outcome":"paired","unix":1}"#).unwrap();
        assert_eq!(old.device_id, "");
    }

    /// PC1 review C1, M2: a listed computer's id with another certificate
    /// is someone else's; the same certificate, in either case, or an id
    /// not listed, is not.
    /// Item 4: a listed computer's release is kept cleaned, read back only
    /// while it is listed, and dropped once it is not.
    #[test]
    fn a_listed_computers_release_is_kept_while_it_is_listed() {
        let mut file = ComputersFile {
            computers: vec![computer("desk")],
            ..ComputersFile::default()
        };
        assert!(file.note_version("desk", "0.8.1"));
        assert!(!file.note_version("desk", "0.8.1"), "unchanged");
        assert!(!file.note_version("desk", "newest"), "not a version");
        assert!(!file.note_version("stranger", "0.8.1"), "not listed");
        assert_eq!(file.version("desk"), Some("0.8.1"));
        assert_eq!(file.version("stranger"), None);
        file.computers.clear();
        assert_eq!(file.version("desk"), None, "forgotten");
        file.computers.push(computer("lap"));
        assert!(file.note_version("lap", "0.8.2"));
        assert_eq!(
            file.versions.keys().collect::<Vec<_>>(),
            ["lap"],
            "a forgotten one's goes"
        );
    }

    #[test]
    fn a_listed_id_with_another_certificate_is_someone_elses() {
        let file = ComputersFile {
            computers: vec![computer("desk")],
            ..ComputersFile::default()
        };
        assert!(file.listed_with_other_certificate("desk", &"cd".repeat(32)));
        assert!(!file.listed_with_other_certificate("desk", &"AB".repeat(32)));
        assert!(!file.listed_with_other_certificate("lap", &"cd".repeat(32)));
    }

    /// PC1 re-review: a grant both ways that waited for the link longer
    /// than ten minutes lapses (its computer then stays paired one way, and
    /// is said to have lapsed); a younger one waits on, and one whose
    /// computer is gone lapses too.
    #[test]
    fn a_grant_waiting_for_the_link_lapses_after_ten_minutes() {
        let paired_at = |id: &str, at: u64| PeerComputer {
            paired_unix: at,
            mine: true,
            ..computer(id)
        };
        let mut file = ComputersFile {
            computers: vec![paired_at("old", 1_000), paired_at("young", 1_500)],
            pending: vec!["old".to_string(), "young".to_string(), "gone".to_string()],
            ..ComputersFile::default()
        };
        let lapsed = file.lapse_pending(1_000 + PENDING_FOR);
        assert_eq!(lapsed, ["old", "gone"]);
        assert_eq!(file.pending, ["young"]);
        assert_eq!(file.lapsed, ["old"], "said for a computer still listed");
        let mine = |id: &str| file.computers.iter().any(|c| c.device_id == id && c.mine);
        assert!(mine("old"), "its listing stays (PR #572 review I3)");
        assert!(!file.both_ways("old"), "but it is not both ways now");
        assert!(file.both_ways("young"), "still waiting, still both ways");
        let old = file
            .computers
            .iter()
            .find(|c| c.device_id == "old")
            .unwrap();
        assert!(
            !keeps_link("zz-own", old),
            "the link's direction does not flip"
        );
        assert!(keeps_link("a-own", old));
        assert!(file.lapse_pending(1_000 + PENDING_FOR).is_empty(), "once");
        file.remove("old");
        assert!(file.lapsed.is_empty(), "gone with its computer");
    }

    /// BD3: a computer taken out takes its pairing's line, not another's.
    #[test]
    fn a_computer_removed_takes_its_pairing_line() {
        let line = |device_id: &str| PairingOutcome {
            name: "Desk".to_string(),
            outcome: "failed: the computer at 192.0.2.10 is not the one".to_string(),
            unix: 1,
            request: "a".to_string(),
            device_id: device_id.to_string(),
        };
        let mut file = ComputersFile {
            computers: vec![computer("desk"), computer("lap")],
            last_pairing: Some(line("lap")),
            ..ComputersFile::default()
        };
        assert_eq!(
            file.remove("desk").map(|c| c.device_id),
            Some("desk".to_string())
        );
        assert_eq!(file.last_pairing, Some(line("lap")), "another computer's");
        assert_eq!(
            file.remove("lap").map(|c| c.device_id),
            Some("lap".to_string())
        );
        assert_eq!(file.last_pairing, None);
        assert_eq!(file.remove("gone"), None);
    }

    /// BD3: a request left half-written by a crash goes once it is stale;
    /// one being written now stays.
    #[test]
    fn a_half_written_request_goes_once_stale() {
        let dir = temp_dir("tmp");
        let out = dir.join(COMPUTERS_DIR);
        std::fs::create_dir_all(&out).unwrap();
        let tmp = out.join("left.json.tmp");
        std::fs::write(
            &tmp,
            r#"{"ask":"pair","link":"pocketdesk://pair?pin=123456"}"#,
        )
        .unwrap();
        let written = std::fs::metadata(&tmp)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(take_all(&dir, written + 1).is_empty());
        assert!(tmp.exists(), "being written");
        assert!(take_all(&dir, written + REQUEST_TTL_SECS + 1).is_empty());
        assert!(!tmp.exists(), "left by a crash");
    }

    #[test]
    fn the_file_is_changed_whole_and_reads_empty_when_missing_or_bad() {
        let dir = temp_dir("file");
        assert_eq!(read(&dir), ComputersFile::default());
        update(&dir, |file| file.computers.push(computer("desk"))).unwrap();
        update(&dir, |file| {
            file.last_pairing = Some(PairingOutcome {
                name: "Desk".to_string(),
                outcome: "paired".to_string(),
                unix: 100,
                request: "a".to_string(),
                device_id: "desk".to_string(),
            })
        })
        .unwrap();
        let file = read(&dir);
        assert_eq!(file.computers, [computer("desk")]);
        assert_eq!(file.last_pairing.unwrap().outcome, "paired");
        std::fs::write(dir.join(COMPUTERS_FILE), "{not json").unwrap();
        assert_eq!(read(&dir), ComputersFile::default());
        assert!(update(&dir, |file| file.computers.clear()).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join(COMPUTERS_FILE)).unwrap(),
            "{not json",
            "a bad file is never wiped"
        );
    }
}
