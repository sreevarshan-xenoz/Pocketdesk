//! Numbers the computer asks a phone to dial (L4):
//! `<state>/dials_out/<id>.json`, one file a call, written by the desktop
//! app or `pocketdesk call` and taken (read and deleted) by the server,
//! which sends it to the phone if it is connected within a minute.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::DIALS_DIR;
use crate::protocol::dial::clean_number;

/// How long a call waits for its phone: a minute.
pub const DIAL_TTL_SECS: u64 = 60;

/// One number for a phone to dial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// The number, cleaned ([`clean_number`]).
    pub number: String,
    pub created_unix: u64,
}

/// Ask the server to have `device_id` dial `number`.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    number: &str,
    now_unix: u64,
) -> io::Result<DialRequest> {
    let Some(number) = clean_number(number) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a phone number",
        ));
    };
    if device_id.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no phone"));
    }
    let request = DialRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        number,
        created_unix: now_unix,
    };
    let file = path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(DIALS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting call (the server's side), oldest first. Each file is
/// removed once read; one stale, from the future, unreadable, named for
/// another or not a number is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<DialRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(DIALS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<DialRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= DIAL_TTL_SECS;
        if fresh
            && request.id == stem
            && is_valid_id(&request.id)
            && clean_number(&request.number).as_deref() == Some(request.number.as_str())
        {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a call id"));
    }
    Ok(state_dir.join(DIALS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-dials-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// L4: a call is kept cleaned, taken once, and a stale one dropped.
    #[test]
    fn a_call_is_taken_once_within_its_time() {
        let dir = dir("take");
        let kept = request(&dir, "phone-a", "+1 555 010 0100", 1_000).unwrap();
        assert_eq!(kept.number, "+15550100100");
        assert_eq!(take_all(&dir, 1_010), vec![kept]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "phone-a", "112", 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + DIAL_TTL_SECS + 1).is_empty(),
            "stale"
        );
        assert!(request(&dir, "phone-a", "call me", 1).is_err());
        assert!(request(&dir, "", "112", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
