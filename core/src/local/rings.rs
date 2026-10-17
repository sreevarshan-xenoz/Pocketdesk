//! Rings the desktop app or the local API asks the server to send to a phone
//! (N1), as `<state>/rings_out/<id>.json`, one file per ring, taken (read
//! and deleted) by the server and sent to the phone over its live link.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::RINGS_DIR;

/// How long a ring waits for the server before it is dropped: a phone found
/// a minute later should not start ringing.
pub const REQUEST_TTL_SECS: u64 = 60;

/// One ring for a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RingRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    pub created_unix: u64,
}

/// Ask the server to ring `device_id` (the app's and the local API's side).
pub fn request(state_dir: &Path, device_id: &str, now_unix: u64) -> io::Result<RingRequest> {
    let request = RingRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(RINGS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting ring (the server's side). Each file is removed once
/// read; one that is stale, from the future, unreadable or named for another
/// request is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<RingRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(RINGS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<RingRequest>(&raw).ok())
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
    Ok(state_dir.join(RINGS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-rings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_ring_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_all(&dir, 100).is_empty(), "nothing queued yet");
        let asked = request(&dir, "phone-1", 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert!(take_all(&dir, 101).is_empty(), "taken");
    }

    #[test]
    fn a_stale_or_future_ring_is_dropped() {
        let dir = temp_dir("stale");
        request(&dir, "phone-1", 100).unwrap();
        assert!(
            take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "a minute late: the phone may be found"
        );
        request(&dir, "phone-1", 200).unwrap();
        assert!(take_all(&dir, 150).is_empty(), "from the future");
        assert!(
            std::fs::read_dir(dir.join(RINGS_DIR))
                .unwrap()
                .next()
                .is_none(),
            "dropped, not left behind"
        );
    }
}
