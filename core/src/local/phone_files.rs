//! The phone's storage on the computer (T10g): each phone's last listing,
//! kept by the server in `<state>/phone_files.json` (owner-only), and the
//! desktop app's requests for the server to send, one file each in
//! `<state>/phone_files_out/`, taken within [`REQUEST_TTL_SECS`].

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{PHONE_FILES_DIR, PHONE_FILES_FILE};
use crate::protocol::phone_files::{Listing, valid_path};

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// A listing, and when it came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub listing: Listing,
    pub at_unix: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FilesFile {
    phones: BTreeMap<String, Kept>,
}

/// Each phone's last listing; none when there is no file or it cannot be
/// read.
pub fn read(state_dir: &Path) -> BTreeMap<String, Kept> {
    std::fs::read_to_string(state_dir.join(PHONE_FILES_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<FilesFile>(&raw).ok())
        .map(|f| f.phones)
        .unwrap_or_default()
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Keep `listing` as `device_id`'s last.
pub fn keep(state_dir: &Path, device_id: &str, listing: Listing, now_unix: u64) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut phones = read(state_dir);
    phones.insert(
        device_id.to_string(),
        Kept {
            listing,
            at_unix: now_unix,
        },
    );
    let json = serde_json::to_string(&FilesFile { phones }).map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_FILES_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    // What is on a phone: owner-only.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Drop them all (at start: they describe a run that ended).
pub fn clear(state_dir: &Path) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::remove_file(state_dir.join(PHONE_FILES_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the app asks of the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    List,
    Fetch,
}

/// One request for the server to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub device_id: String,
    pub kind: Kind,
    pub root: u32,
    pub path: Vec<String>,
    pub created_unix: u64,
}

/// Ask the server to send a request (the app's side); a path out of
/// bounds is refused here.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    kind: Kind,
    root: u32,
    path: &[String],
    now_unix: u64,
) -> io::Result<Request> {
    if !valid_path(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a path the phone takes",
        ));
    }
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        kind,
        root,
        path: path.to_vec(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(PHONE_FILES_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first; a stale,
/// misnamed or out-of-bounds one is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PHONE_FILES_DIR)) else {
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
            continue;
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Request>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) && valid_path(&request.path) {
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
    Ok(state_dir.join(PHONE_FILES_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-phone-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_listing_is_kept_per_phone_and_cleared_at_start() {
        let dir = temp_dir("keep");
        let listing = Listing {
            root: Some(0),
            ..Default::default()
        };
        keep(&dir, "phone", listing.clone(), 5).unwrap();
        keep(&dir, "other", Listing::default(), 6).unwrap();
        let kept = read(&dir);
        assert_eq!(kept["phone"].listing, listing);
        assert_eq!(kept.len(), 2);
        clear(&dir).unwrap();
        assert!(read(&dir).is_empty());
    }

    #[test]
    fn a_request_is_taken_once_while_fresh_and_never_one_that_climbs() {
        let dir = temp_dir("requests");
        let asked = request(
            &dir,
            "phone",
            Kind::Fetch,
            0,
            &["Docs".into(), "a.pdf".into()],
            1_000,
        )
        .unwrap();
        request(
            &dir,
            "phone",
            Kind::List,
            0,
            &[],
            1_000 - REQUEST_TTL_SECS - 1,
        )
        .unwrap();
        assert!(request(&dir, "phone", Kind::List, 0, &["..".into()], 1_000).is_err());
        let taken = take_all(&dir, 1_001);
        assert_eq!(taken, vec![asked]);
        assert!(take_all(&dir, 1_001).is_empty());
    }
}
