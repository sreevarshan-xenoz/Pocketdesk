//! Phone notifications cleared on this computer (L1), to be cleared on the
//! phone: `<state>/notification_clears/<id>.json`, one file a clear, written
//! by whoever saw it cleared (the desktop app's Notifications page, the
//! server's toast or D-Bus handler) and taken (read and deleted) by the
//! server, which asks the phone.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::NOTIFICATION_CLEARS_DIR;
use crate::protocol::notification_action::valid_key;

/// How long a clear waits for its phone: a minute, as an answer does.
pub const CLEAR_TTL_SECS: u64 = 60;

/// One notification to clear on its phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// The phone's id for the notification.
    pub key: String,
    pub created_unix: u64,
}

/// Ask the server to clear `device_id`'s notification `key` on the phone.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    key: &str,
    now_unix: u64,
) -> io::Result<ClearRequest> {
    if !valid_key(key) || device_id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a notification the phone knows",
        ));
    }
    let request = ClearRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        key: key.to_string(),
        created_unix: now_unix,
    };
    let file = path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(NOTIFICATION_CLEARS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting clear (the server's side), oldest first. Each file is
/// removed once read; one stale, from the future, unreadable, named for
/// another or out of bounds is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<ClearRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(NOTIFICATION_CLEARS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<ClearRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= CLEAR_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) && valid_key(&request.key) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a clear id",
        ));
    }
    Ok(state_dir
        .join(NOTIFICATION_CLEARS_DIR)
        .join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-clears-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// L1: a clear is kept, taken once, and a stale one dropped.
    #[test]
    fn a_clear_is_taken_once_within_its_time() {
        let dir = dir("take");
        let kept = request(&dir, "phone-a", "0|com.chat|1|null|10", 1_000).unwrap();
        assert_eq!(take_all(&dir, 1_010), vec![kept]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "phone-a", "k", 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + CLEAR_TTL_SECS + 1).is_empty(),
            "stale"
        );
        request(&dir, "phone-a", "k", 2_000).unwrap();
        assert!(take_all(&dir, 1_999).is_empty(), "from the future");
        assert!(request(&dir, "phone-a", "", 1).is_err());
        assert!(request(&dir, "", "k", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
