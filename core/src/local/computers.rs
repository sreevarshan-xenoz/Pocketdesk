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
}

impl ComputersFile {
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
    /// Let a computer this one paired with drive this one's keyboard and
    /// mouse, or not ([`PeerComputer::lets_drive`]).
    LetDrive { device_id: String, allow: bool },
    /// Show a paired computer's screen in a window here (BK7): the server
    /// dials it and relays its picture to the app ([`super::view`]).
    /// `monitor` is which of its screens, from 0. A newer view replaces an
    /// older one.
    View {
        device_id: String,
        #[serde(default)]
        monitor: u32,
    },
    /// Stop showing the computer's screen (its window closed): the view
    /// `session_id` (`view.json`'s) when the window knew it, else the one
    /// shown now. A Stop for a view since replaced stops nothing (BK7
    /// review M1).
    StopView {
        #[serde(default)]
        session_id: Option<u64>,
    },
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
            if stale && path.extension().is_some_and(|e| e == "tmp") {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<ComputerRequest>(&raw).ok())
        else {
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
            },
            100,
        )
        .unwrap();
        let stop = request(
            &dir,
            ComputerAsk::StopView {
                session_id: Some(9),
            },
            101,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 102), [view, stop]);
        let wire = serde_json::to_value(ComputerRequest {
            id: "a".to_string(),
            ask: ComputerAsk::StopView {
                session_id: Some(9),
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
                monitor: 0
            },
            "the first screen unless named"
        );
        let raw = serde_json::to_string(&computer("desk")).unwrap();
        let old: PeerComputer =
            serde_json::from_str(&raw.replace(",\"stream_port\":4716", "")).unwrap();
        assert_eq!(old.stream_port, 4716);
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
                    ComputerAsk::StopView { session_id: None }
                } else {
                    ComputerAsk::View {
                        device_id: format!("desk-{n}"),
                        monitor: 0,
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
        assert_eq!(old.ask, ComputerAsk::StopView { session_id: None });
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
