//! Screenshots and locks the computer asks a phone for (O2):
//! `<state>/phone_actions_out/<id>.json`, one file an ask, written by the
//! desktop app and taken (read and deleted) by the server, which sends it
//! to the phone if it is connected within a minute.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::PHONE_ACTIONS_DIR;
use crate::protocol::phone_action::PhoneAction;

/// How long an ask waits for its phone: a minute, as a call does.
pub const ACTION_TTL_SECS: u64 = 60;

/// One ask of a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneActionRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// The action's word ([`PhoneAction::as_str`]).
    pub action: String,
    /// The app an `open` opens (P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub created_unix: u64,
}

impl PhoneActionRequest {
    /// The action asked for, when it is one.
    pub fn action(&self) -> Option<PhoneAction> {
        PhoneAction::parse(&self.action)
    }
}

/// Ask the server to have `device_id` do `action`.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    action: PhoneAction,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    write_request(state_dir, device_id, action, None, now_unix)
}

/// P2: ask the server to have `device_id` open the app `app`.
pub fn request_open(
    state_dir: &Path,
    device_id: &str,
    app: &str,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    if !crate::protocol::phone_action::valid_app_id(app) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    write_request(state_dir, device_id, PhoneAction::Open, Some(app), now_unix)
}

fn write_request(
    state_dir: &Path,
    device_id: &str,
    action: PhoneAction,
    app: Option<&str>,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    if device_id.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no phone"));
    }
    let request = PhoneActionRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        action: action.as_str().to_string(),
        app: app.map(str::to_string),
        created_unix: now_unix,
    };
    let file = path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(PHONE_ACTIONS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting ask (the server's side), oldest first. Each file is
/// removed once read; one stale, from the future, unreadable, named for
/// another or not an action is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<PhoneActionRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PHONE_ACTIONS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<PhoneActionRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= ACTION_TTL_SECS;
        // An open names a valid app, and nothing else names one (P2).
        let app_right = match request.action() {
            Some(PhoneAction::Open) => request
                .app
                .as_deref()
                .is_some_and(crate::protocol::phone_action::valid_app_id),
            Some(_) => request.app.is_none(),
            None => false,
        };
        if fresh && request.id == stem && is_valid_id(&request.id) && app_right {
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
            "not an ask's id",
        ));
    }
    Ok(state_dir.join(PHONE_ACTIONS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-phone-actions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// O2: an ask is taken once within its minute, and a stale or unknown one dropped.
    #[test]
    fn an_ask_is_taken_once_within_its_time() {
        let dir = dir("take");
        let kept = request(&dir, "phone-a", PhoneAction::Screenshot, 1_000).unwrap();
        assert_eq!(kept.action(), Some(PhoneAction::Screenshot));
        assert_eq!(take_all(&dir, 1_010), vec![kept]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "phone-a", PhoneAction::Lock, 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + ACTION_TTL_SECS + 1).is_empty(),
            "stale"
        );
        // A file naming no action, written by hand, is dropped.
        let mut odd = request(&dir, "phone-a", PhoneAction::Lock, 1_000).unwrap();
        odd.action = "unlock".to_string();
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "not an action");
        assert!(request(&dir, "", PhoneAction::Lock, 1).is_err());
        // P2: an open carries its app; a bad one is refused or dropped.
        let open = request_open(&dir, "phone-a", "org.example.app", 1_000).unwrap();
        assert_eq!(take_all(&dir, 1_000), vec![open]);
        assert!(request_open(&dir, "phone-a", "bad id", 1_000).is_err());
        let mut odd = request(&dir, "phone-a", PhoneAction::Open, 1_000).unwrap();
        odd.app = None;
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "an open without its app");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
