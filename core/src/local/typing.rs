//! What the computer types on a phone (M4): `<state>/typing_out/<id>.json`,
//! one batch a file, written by the desktop app's typing window about every
//! 100 ms while it is typed in, and taken (read and deleted) by the server,
//! which sends each to its phone in order. A batch older than
//! [`TYPING_TTL_MS`] is dropped: late keys would land in another field.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::approval::is_valid_id;
use super::paths::TYPING_DIR;
use crate::protocol::typing::{TypeEvent, fits};

/// How long a batch waits for its phone: 10 s.
pub const TYPING_TTL_MS: u64 = 10_000;

/// One batch for a phone, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    id: String,
    device_id: String,
    /// The window's own count: batches of one millisecond keep their order.
    seq: u64,
    created_ms: u64,
    events: Vec<Value>,
}

/// One batch, taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeBatch {
    pub device_id: String,
    pub seq: u64,
    pub created_ms: u64,
    pub events: Vec<TypeEvent>,
}

/// Ask the server to type `events` on `device_id`.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    seq: u64,
    events: &[TypeEvent],
    now_ms: u64,
) -> io::Result<()> {
    if device_id.is_empty() || !fits(events) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a batch"));
    }
    let stored = Stored {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        seq,
        created_ms: now_ms,
        events: events.iter().map(TypeEvent::to_json).collect(),
    };
    let file = path(state_dir, &stored.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(TYPING_DIR))?;
    let json = serde_json::to_string(&stored).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)
}

/// Take every waiting batch (the server's side), in the order typed. Each
/// file is removed once read; one stale, from the future, unreadable or
/// not a batch that fits is dropped.
pub fn take_all(state_dir: &Path, now_ms: u64) -> Vec<TypeBatch> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(TYPING_DIR)) else {
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
        let Some(stored) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Stored>(&raw).ok())
        else {
            continue;
        };
        let fresh = stored.created_ms <= now_ms && now_ms - stored.created_ms <= TYPING_TTL_MS;
        let events = stored
            .events
            .iter()
            .map(TypeEvent::from_json)
            .collect::<Option<Vec<_>>>();
        if let Some(events) = events.filter(|e| fits(e))
            && fresh
            && stored.id == stem
            && is_valid_id(&stored.id)
        {
            taken.push(TypeBatch {
                device_id: stored.device_id,
                seq: stored.seq,
                created_ms: stored.created_ms,
                events,
            });
        }
    }
    taken.sort_by_key(|b| (b.created_ms, b.seq));
    taken
}

fn path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a batch id",
        ));
    }
    Ok(state_dir.join(TYPING_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-typing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn batches_are_taken_once_in_the_order_typed() {
        let dir = dir("order");
        let a = [TypeEvent::Text("a".into())];
        let b = [TypeEvent::Text("b".into())];
        let c = [TypeEvent::Enter];
        // Two in one millisecond keep their order by seq.
        request(&dir, "phone", 2, &b, 1_000).unwrap();
        request(&dir, "phone", 1, &a, 1_000).unwrap();
        request(&dir, "phone", 3, &c, 1_050).unwrap();
        let taken = take_all(&dir, 1_100);
        let events: Vec<_> = taken.iter().flat_map(|b| b.events.clone()).collect();
        assert_eq!(
            events,
            [
                TypeEvent::Text("a".into()),
                TypeEvent::Text("b".into()),
                TypeEvent::Enter
            ]
        );
        assert!(take_all(&dir, 1_100).is_empty(), "taken once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_or_bad_batch_is_dropped() {
        let dir = dir("stale");
        request(&dir, "phone", 1, &[TypeEvent::Enter], 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + TYPING_TTL_MS + 1).is_empty(),
            "stale"
        );
        assert!(request(&dir, "phone", 1, &[], 1_000).is_err(), "empty");
        assert!(
            request(&dir, "", 1, &[TypeEvent::Enter], 1_000).is_err(),
            "no phone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
