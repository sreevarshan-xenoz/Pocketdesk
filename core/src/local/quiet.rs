//! Mutes the desktop app or the local API asks the server to send to a phone
//! (D2), as `<state>/quiet_out/<id>.json`, one file per request, taken (read
//! and deleted) by the server and sent to the phone over its live link as
//! `kdeconnect.pocketdesk.quiet {minutes}`. Zero minutes ends a mute.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::QUIET_DIR;

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// The longest mute: a day, as the phone allows.
pub use crate::protocol::quiet::MAX_MINUTES;

/// One mute (or its end) for a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// For how long, 0 to end a mute.
    pub minutes: u32,
    pub created_unix: u64,
}

/// Ask the server to mute `device_id` for `minutes` (0 ends a mute).
pub fn request(
    state_dir: &Path,
    device_id: &str,
    minutes: u32,
    now_unix: u64,
) -> io::Result<QuietRequest> {
    if minutes > MAX_MINUTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("at most {MAX_MINUTES} minutes"),
        ));
    }
    let request = QuietRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        minutes,
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(QUIET_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side). Each file is removed once
/// read; one that is stale, from the future, unreadable, out of range or
/// named for another request is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<QuietRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(QUIET_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<QuietRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) && request.minutes <= MAX_MINUTES
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
    Ok(state_dir.join(QUIET_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-quiet-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_request_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_all(&dir, 100).is_empty(), "nothing queued yet");
        let asked = request(&dir, "phone-1", 60, 100).unwrap();
        let ended = request(&dir, "phone-1", 0, 101).unwrap();
        assert_eq!(take_all(&dir, 102), [asked, ended], "oldest first");
        assert!(take_all(&dir, 102).is_empty(), "taken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_day_at_most() {
        let dir = temp_dir("day");
        assert!(request(&dir, "phone-1", MAX_MINUTES, 100).is_ok());
        assert!(request(&dir, "phone-1", MAX_MINUTES + 1, 100).is_err());
        // One written by hand past the limit is dropped.
        let file = dir
            .join(QUIET_DIR)
            .join("00000000-0000-0000-0000-000000000001.json");
        std::fs::write(
            &file,
            r#"{"id":"00000000-0000-0000-0000-000000000001","device_id":"p","minutes":5000,"created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 100).len(), 1, "only the day");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_or_future_request_is_dropped() {
        let dir = temp_dir("stale");
        request(&dir, "phone-1", 60, 100).unwrap();
        assert!(take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty());
        request(&dir, "phone-1", 60, 200).unwrap();
        assert!(take_all(&dir, 150).is_empty(), "from the future");
        assert!(
            std::fs::read_dir(dir.join(QUIET_DIR))
                .unwrap()
                .next()
                .is_none(),
            "dropped, not left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
