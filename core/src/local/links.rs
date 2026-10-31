//! Links the desktop app or the local API asks the server to hand to a
//! phone (roadmap T8a), as `<state>/links_out/<id>.json`, one file per
//! link, taken (read and deleted) by the server and pushed to the phone. A
//! link is checked when it is asked for and again when it is taken.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::LINKS_DIR;
use crate::protocol::link::link_problem;

/// How long a link waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 60;

/// One link to hand to a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    pub url: String,
    pub created_unix: u64,
}

/// Ask the server to hand `url` to `device_id` (the app's and the local
/// API's side). A link that may not be opened is refused here.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    url: &str,
    now_unix: u64,
) -> io::Result<LinkRequest> {
    if let Some(problem) = link_problem(url) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, problem));
    }
    let request = LinkRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        url: url.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(LINKS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting link (the server's side). Each file is removed once
/// read; one that is stale, from the future, unreadable, named for another
/// request, or a link that may not be opened is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<LinkRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(LINKS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<LinkRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh
            && request.id == stem
            && is_valid_id(&request.id)
            && link_problem(&request.url).is_none()
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
    Ok(state_dir.join(LINKS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-links-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_link_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_all(&dir, 100).is_empty(), "nothing queued yet");
        let asked = request(&dir, "phone-1", "https://example.org/a", 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert!(take_all(&dir, 101).is_empty(), "taken");
    }

    #[test]
    fn a_link_that_may_not_be_opened_is_refused_and_never_taken() {
        let dir = temp_dir("bad");
        let refused = request(&dir, "phone-1", "file:///etc/passwd", 100).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
        // Written by hand, past the check: the server's side checks again.
        let id = uuid::Uuid::new_v4().to_string();
        let forged = LinkRequest {
            id: id.clone(),
            device_id: "phone-1".to_string(),
            url: "javascript:alert(1)".to_string(),
            created_unix: 100,
        };
        std::fs::create_dir_all(dir.join(LINKS_DIR)).unwrap();
        std::fs::write(
            dir.join(LINKS_DIR).join(format!("{id}.json")),
            serde_json::to_string(&forged).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 101).is_empty());
        let stale = request(&dir, "phone-1", "https://example.org/", 100).unwrap();
        assert!(
            take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "{stale:?}"
        );
    }
}
