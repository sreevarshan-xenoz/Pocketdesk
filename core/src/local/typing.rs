//! What the computer types on a phone (M4):
//! `<state>/typing_out/<ms>-<seq>.json`, one file a flush of the desktop
//! app's typing window (about every 100 ms while it is typed in), holding
//! that flush's batches in order. The server takes (reads and deletes) the
//! files in name order, the time typed then the window's own count, and
//! sends each batch to its phone. A flush older than [`TYPING_TTL_MS`] is
//! dropped: late keys would land in another field.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::paths::TYPING_DIR;
use crate::protocol::typing::{TypeEvent, fits};

/// How long a flush waits for its phone: 10 s.
pub const TYPING_TTL_MS: u64 = 10_000;

/// One flush, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    device_id: String,
    seq: u64,
    created_ms: u64,
    batches: Vec<Vec<Value>>,
}

/// One batch, taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeBatch {
    pub device_id: String,
    pub seq: u64,
    pub created_ms: u64,
    pub events: Vec<TypeEvent>,
}

/// A flush's file name: its time then its count, both zero-padded, so the
/// names sort in the order typed.
fn name(created_ms: u64, seq: u64) -> String {
    format!("{created_ms:013}-{seq:010}.json")
}

/// The time and count a file's name gives, if it is a flush's.
fn parse_name(file: &str) -> Option<(u64, u64)> {
    let stem = file.strip_suffix(".json")?;
    let (ms, seq) = stem.split_once('-')?;
    if ms.len() != 13 || seq.len() != 10 {
        return None;
    }
    Some((ms.parse().ok()?, seq.parse().ok()?))
}

/// Ask the server to type `batches` on `device_id`, in order, as one flush.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    seq: u64,
    batches: &[Vec<TypeEvent>],
    now_ms: u64,
) -> io::Result<()> {
    if device_id.is_empty() || batches.is_empty() || !batches.iter().all(|b| fits(b)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a batch"));
    }
    let dir = state_dir.join(TYPING_DIR);
    // Once (M4 review): making it private again each flush rewrote its
    // permissions under the server's feet every 100 ms.
    if !dir.is_dir() {
        crate::secure_fs::create_private_dir(&dir)?;
    }
    let stored = Stored {
        device_id: device_id.to_string(),
        seq,
        created_ms: now_ms,
        batches: batches
            .iter()
            .map(|b| b.iter().map(TypeEvent::to_json).collect())
            .collect(),
    };
    let file = dir.join(name(now_ms, seq));
    let json = serde_json::to_string(&stored).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

/// Take every waiting batch (the server's side), in the order typed. Each
/// file is removed once read; one stale, from the future, unreadable or
/// holding a batch that does not fit is dropped whole, and a temp file a
/// writer left is swept once stale.
pub fn take_all(state_dir: &Path, now_ms: u64) -> Vec<TypeBatch> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(TYPING_DIR)) else {
        return Vec::new();
    };
    let fresh = |created_ms: u64| created_ms <= now_ms && now_ms - created_ms <= TYPING_TTL_MS;
    let mut files: Vec<(u64, u64, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(tmp) = file.strip_suffix(".tmp") {
            if parse_name(tmp).is_none_or(|(ms, _)| !fresh(ms)) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        match parse_name(file) {
            Some((ms, seq)) => files.push((ms, seq, path)),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    files.sort();
    let mut taken = Vec::new();
    for (ms, seq, path) in files {
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(stored) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Stored>(&raw).ok())
        else {
            continue;
        };
        if stored.created_ms != ms || stored.seq != seq || !fresh(ms) {
            continue;
        }
        let batches = stored
            .batches
            .iter()
            .map(|b| {
                b.iter()
                    .map(TypeEvent::from_json)
                    .collect::<Option<Vec<_>>>()
                    .filter(|e| fits(e))
            })
            .collect::<Option<Vec<_>>>();
        let Some(batches) = batches else {
            continue;
        };
        taken.extend(batches.into_iter().map(|events| TypeBatch {
            device_id: stored.device_id.clone(),
            seq,
            created_ms: ms,
            events,
        }));
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-typing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn flushes_are_taken_once_in_the_order_typed_batches_and_all() {
        let dir = dir("order");
        let a = vec![TypeEvent::Text("a".into())];
        let b = vec![TypeEvent::Text("b".into())];
        // Two flushes in one millisecond keep their order by count; a flush
        // keeps its batches' order.
        request(&dir, "phone", 2, &[vec![TypeEvent::Enter]], 1_000).unwrap();
        request(&dir, "phone", 1, &[a, b], 1_000).unwrap();
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
    fn a_stale_or_bad_flush_is_dropped_and_a_stale_temp_file_swept() {
        let dir = dir("stale");
        request(&dir, "phone", 1, &[vec![TypeEvent::Enter]], 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + TYPING_TTL_MS + 1).is_empty(),
            "stale"
        );
        assert!(request(&dir, "phone", 1, &[], 1_000).is_err(), "empty");
        assert!(
            request(&dir, "phone", 1, &[vec![]], 1_000).is_err(),
            "an empty batch"
        );
        assert!(
            request(&dir, "", 1, &[vec![TypeEvent::Enter]], 1_000).is_err(),
            "no phone"
        );
        let tmp = dir.join(TYPING_DIR).join(format!("{}.tmp", name(1_000, 9)));
        std::fs::write(&tmp, b"half").unwrap();
        take_all(&dir, 1_000 + TYPING_TTL_MS + 1);
        assert!(!tmp.exists(), "a stale temp file is swept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_sort_in_the_order_typed() {
        assert!(name(999, 2) < name(1_000, 1));
        assert!(name(1_000, 1) < name(1_000, 2));
        assert_eq!(parse_name(&name(1_000, 7)), Some((1_000, 7)));
        assert_eq!(parse_name("1000-7.json"), None);
    }
}
