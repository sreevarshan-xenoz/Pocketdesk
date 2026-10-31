//! Files the desktop app asks the server to send to a phone (roadmap F7b).
//!
//! The app writes one request per file as `<state>/outbox/<id>.json`; the
//! server takes (reads and deletes) every request it finds and offers the
//! file to the phone. One file per request, so two sends queued at once
//! cannot overwrite each other. A request the server has not taken within
//! [`REQUEST_TTL_SECS`] is dropped: the owner who asked has moved on.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::OUTBOX_DIR;

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// One file to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest {
    /// The request's name; also its file name, and the sent list's id.
    pub id: String,
    /// The phone to send to (its paired device id).
    pub device_id: String,
    /// The file on this computer.
    pub path: String,
    pub created_unix: u64,
}

/// Ask the server to send `path` to `device_id` (the app's side).
pub fn request(
    state_dir: &Path,
    device_id: &str,
    path: &str,
    now_unix: u64,
) -> io::Result<SendRequest> {
    let request = SendRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        path: path.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(OUTBOX_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side). Each file is removed
/// once read; one that is stale, stamped in the future, unreadable, or
/// names another request than its file is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<SendRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(OUTBOX_DIR)) else {
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
            continue; // a temp file mid-write, or something else entirely
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<SendRequest>(&raw).ok())
        else {
            continue;
        };
        // Same machine, same clock: a request from the future is not one.
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
    Ok(state_dir.join(OUTBOX_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-outbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_request_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_all(&dir, 100).is_empty(), "no outbox yet");
        let sent = request(&dir, "phone-1", "C:\\Users\\me\\a.pdf", 100).unwrap();
        assert!(is_valid_id(&sent.id));
        assert_eq!(take_all(&dir, 101), [sent]);
        assert!(take_all(&dir, 101).is_empty(), "taken");
        assert_eq!(std::fs::read_dir(dir.join(OUTBOX_DIR)).unwrap().count(), 0);
    }

    #[test]
    fn stale_future_and_mislabelled_requests_are_dropped_and_deleted() {
        let dir = temp_dir("stale");
        let old = request(&dir, "p", "/a", 100).unwrap();
        let future = request(&dir, "p", "/b", 500).unwrap();
        let moved = request(&dir, "p", "/c", 160).unwrap();
        let outbox = dir.join(OUTBOX_DIR);
        std::fs::rename(
            outbox.join(format!("{}.json", moved.id)),
            outbox.join("someone-else.json"),
        )
        .unwrap();
        std::fs::write(outbox.join("junk.json"), "{not json").unwrap();
        assert!(take_all(&dir, 161).is_empty(), "{old:?} {future:?}");
        assert_eq!(std::fs::read_dir(&outbox).unwrap().count(), 0);
    }

    #[test]
    fn requests_come_out_oldest_first() {
        let dir = temp_dir("order");
        let b = request(&dir, "p", "/b", 110).unwrap();
        let a = request(&dir, "p", "/a", 105).unwrap();
        assert_eq!(take_all(&dir, 111), [a, b]);
    }

    #[test]
    fn a_device_id_never_names_the_file() {
        let dir = temp_dir("names");
        let sent = request(&dir, "../../trusted_devices", "/a", 100).unwrap();
        assert!(
            dir.join(OUTBOX_DIR)
                .join(format!("{}.json", sent.id))
                .exists()
        );
        assert!(!dir.join("trusted_devices.json").exists());
    }
}
