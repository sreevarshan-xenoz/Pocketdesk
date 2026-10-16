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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputersFile {
    #[serde(default)]
    pub computers: Vec<PeerComputer>,
    #[serde(default)]
    pub last_pairing: Option<PairingOutcome>,
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
fn try_read(state_dir: &Path) -> io::Result<ComputersFile> {
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
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first. Each file
/// is removed once read; one that is stale, from the future, unreadable or
/// named for another request is dropped.
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
            continue; // a temp file mid-write
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
