//! The transfers under way (C1): the server's mirror of them,
//! `<state>/transfers.json`, which the desktop app's Files page shows, and
//! the app's requests to cancel one, `<state>/transfer_cancels/<id>.json`,
//! one file each, taken (read and deleted) by the server.
//!
//! The server rewrites the mirror at least every [`HEARTBEAT_SECS`] while
//! anything moves, so a reader can tell a live list from one a stopped
//! server left behind ([`Mirror::is_fresh`]).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{TRANSFER_CANCELS_DIR, TRANSFERS_FILE};

/// How often the server rewrites a list with transfers in it.
pub const HEARTBEAT_SECS: u64 = 2;
/// How old a list may be and still describe a running server.
const FRESH_SECS: u64 = 5;
/// How long a cancel request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// Which way the bytes go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// From this computer to the phone (an offer, or a file it browsed).
    ToPhone,
    /// From the phone to this computer (a share).
    FromPhone,
}

/// One transfer under way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// The id a cancel names; new for each attempt.
    pub id: String,
    pub direction: Direction,
    /// The phone (or computer) at the other end.
    pub device_id: String,
    pub name: String,
    /// The folder a file of a folder send belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The file's size in bytes; 0 while not yet known.
    pub size: u64,
    /// How much the receiving end has, counting what it had before.
    pub done: u64,
    /// Bytes a second over the last few seconds.
    pub rate: u64,
    pub started_unix: u64,
}

/// The list as the server last wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mirror {
    pub updated_unix: u64,
    pub transfers: Vec<Transfer>,
}

impl Mirror {
    /// Whether the list is recent enough to describe a running server. A
    /// stamp from the future counts as fresh (a clock set back).
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.updated_unix) <= FRESH_SECS
    }
}

/// The list, if there is a readable one.
pub fn read(state_dir: &Path) -> Option<Mirror> {
    let raw = std::fs::read_to_string(state_dir.join(TRANSFERS_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// The transfers under way now: none when the list is missing, unreadable
/// or stale.
pub fn under_way(state_dir: &Path, now_unix: u64) -> Vec<Transfer> {
    read(state_dir)
        .filter(|m| m.is_fresh(now_unix))
        .map(|m| m.transfers)
        .unwrap_or_default()
}

/// Replace the list (the server's side).
pub fn write(state_dir: &Path, mirror: &Mirror) -> io::Result<()> {
    let json = serde_json::to_string(mirror).map_err(io::Error::other)?;
    let path = state_dir.join(TRANSFERS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// A request to stop the transfer `transfer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRequest {
    /// The request's own id (its file's name).
    pub id: String,
    /// The [`Transfer::id`] to stop.
    pub transfer: String,
    pub created_unix: u64,
}

/// Ask the server to stop `transfer` (the app's side).
pub fn request_cancel(
    state_dir: &Path,
    transfer: &str,
    now_unix: u64,
) -> io::Result<CancelRequest> {
    if !is_valid_id(transfer) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a transfer id",
        ));
    }
    let request = CancelRequest {
        id: uuid::Uuid::new_v4().to_string(),
        transfer: transfer.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(TRANSFER_CANCELS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting cancel request (the server's side). Each file is
/// removed once read; one that is stale, from the future, unreadable or
/// named for another request is dropped.
pub fn take_cancels(state_dir: &Path, now_unix: u64) -> Vec<CancelRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(TRANSFER_CANCELS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<CancelRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) && is_valid_id(&request.transfer)
        {
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
    Ok(state_dir
        .join(TRANSFER_CANCELS_DIR)
        .join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-transfers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn transfer(id: &str) -> Transfer {
        Transfer {
            id: id.to_string(),
            direction: Direction::ToPhone,
            device_id: "phone-1".to_string(),
            name: "film.mkv".to_string(),
            folder: None,
            size: 2_000,
            done: 500,
            rate: 100,
            started_unix: 90,
        }
    }

    #[test]
    fn a_fresh_list_is_under_way_and_a_stale_one_is_not() {
        let dir = temp_dir("fresh");
        assert!(under_way(&dir, 100).is_empty(), "no list yet");
        let mirror = Mirror {
            updated_unix: 100,
            transfers: vec![transfer("a1b2")],
        };
        write(&dir, &mirror).unwrap();
        assert_eq!(read(&dir), Some(mirror.clone()));
        assert_eq!(under_way(&dir, 100 + FRESH_SECS), mirror.transfers);
        assert!(
            under_way(&dir, 101 + FRESH_SECS).is_empty(),
            "a stopped server's list"
        );
    }

    #[test]
    fn the_list_reads_as_the_app_expects() {
        let raw = r#"{"updated_unix":5,"transfers":[{"id":"a1","direction":"from_phone","device_id":"p","name":"n","folder":"Photos","size":9,"done":3,"rate":1,"started_unix":4}]}"#;
        let mirror: Mirror = serde_json::from_str(raw).unwrap();
        assert_eq!(mirror.transfers[0].direction, Direction::FromPhone);
        assert_eq!(mirror.transfers[0].folder.as_deref(), Some("Photos"));
        // No folder: the field is left out.
        let json = serde_json::to_string(&transfer("a1")).unwrap();
        assert!(!json.contains("folder"), "{json}");
    }

    #[test]
    fn a_cancel_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_cancels(&dir, 100).is_empty());
        let asked = request_cancel(&dir, "a1b2", 100).unwrap();
        assert_eq!(take_cancels(&dir, 101), [asked]);
        assert!(take_cancels(&dir, 101).is_empty(), "taken");
    }

    #[test]
    fn a_stale_future_or_badly_named_cancel_is_dropped() {
        let dir = temp_dir("stale");
        request_cancel(&dir, "a1b2", 100).unwrap();
        assert!(take_cancels(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty());
        request_cancel(&dir, "a1b2", 200).unwrap();
        assert!(take_cancels(&dir, 150).is_empty(), "from the future");
        assert!(
            request_cancel(&dir, "../x", 100).is_err(),
            "not a transfer id"
        );
        assert!(
            std::fs::read_dir(dir.join(TRANSFER_CANCELS_DIR))
                .unwrap()
                .next()
                .is_none(),
            "dropped, not left behind"
        );
    }
}
