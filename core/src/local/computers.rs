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
}

impl ComputersFile {
    /// Take the computer `device_id` out, and its pairing's line with it
    /// (BD3: the line, and any address a failure named, outlived Unpair).
    pub fn remove(&mut self, device_id: &str) -> Option<PeerComputer> {
        let at = self
            .computers
            .iter()
            .position(|c| c.device_id == device_id)?;
        let computer = self.computers.remove(at);
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
    /// this is.
    Pair { link: String },
    /// Forget a paired computer.
    Forget { device_id: String },
}

/// One request, as a file in `computers_out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerRequest {
    pub id: String,
    #[serde(flatten)]
    pub ask: ComputerAsk,
    pub created_unix: u64,
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
    let request = ComputerRequest {
        id: uuid::Uuid::new_v4().to_string(),
        ask,
        created_unix: now_unix,
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
    taken.sort_by_key(|r| r.created_unix);
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
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 100,
            link: String::new(),
            link_unix: 0,
        }
    }

    #[test]
    fn a_request_is_taken_once_and_a_stale_one_never() {
        let dir = temp_dir("requests");
        assert!(take_all(&dir, 100).is_empty(), "nothing asked yet");
        let pair = request(
            &dir,
            ComputerAsk::Pair {
                link: "pocketdesk://pair?v=1".to_string(),
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

    #[test]
    fn a_request_names_what_it_asks_on_disk() {
        let wire = serde_json::to_value(ComputerRequest {
            id: "a".to_string(),
            ask: ComputerAsk::Forget {
                device_id: "desk".to_string(),
            },
            created_unix: 1,
        })
        .unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"id": "a", "ask": "forget", "device_id": "desk", "created_unix": 1})
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
