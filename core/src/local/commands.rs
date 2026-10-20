//! Commands the computer sends a phone's automation apps (L6):
//! `<state>/commands_out/<id>.json`, one file a command, written by
//! `pocketdesk phone-command` and taken (read and deleted) by the server,
//! which sends it to the phone if it is connected within a minute.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::COMMANDS_DIR;
use crate::protocol::command::{valid_name, valid_text};

/// How long a command waits for its phone: a minute.
pub const COMMAND_TTL_SECS: u64 = 60;

/// One command for a phone's automation apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    pub name: String,
    #[serde(default)]
    pub text: String,
    pub created_unix: u64,
}

/// Ask the server to send `device_id` the command `name` with `text`.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    name: &str,
    text: &str,
    now_unix: u64,
) -> io::Result<CommandRequest> {
    if !valid_name(name) || !valid_text(text) || device_id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a command the phone takes",
        ));
    }
    let request = CommandRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        name: name.to_string(),
        text: text.to_string(),
        created_unix: now_unix,
    };
    let file = path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(COMMANDS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting command (the server's side), oldest first. Each file
/// is removed once read; one stale, from the future, unreadable, named for
/// another or out of bounds is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<CommandRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(COMMANDS_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<CommandRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= COMMAND_TTL_SECS;
        if fresh
            && request.id == stem
            && is_valid_id(&request.id)
            && valid_name(&request.name)
            && valid_text(&request.text)
        {
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
            "not a command id",
        ));
    }
    Ok(state_dir.join(COMMANDS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-commands-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// L6: a command is taken once within its time; one out of bounds is not kept.
    #[test]
    fn a_command_is_taken_once_within_its_time() {
        let dir = dir("take");
        let kept = request(&dir, "phone-a", "lights", "off", 1_000).unwrap();
        assert_eq!(take_all(&dir, 1_010), vec![kept]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "phone-a", "lights", "", 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + COMMAND_TTL_SECS + 1).is_empty(),
            "stale"
        );
        assert!(request(&dir, "phone-a", "", "x", 1).is_err());
        assert!(request(&dir, "", "lights", "x", 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
