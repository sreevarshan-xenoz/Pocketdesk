//! The phone notifications the computer can answer (T10c), and the answers
//! the desktop app asks the server to send.
//!
//! The server keeps each phone notification that offers buttons or a reply
//! in `<state>/phone_notifications.json` (owner-only, the newest
//! [`MAX_KEPT`]), drops it when the phone closes it, and empties the file
//! at start. The app shows them; a press or a reply it makes is
//! `<state>/notification_out/<id>.json`, taken by the server within
//! [`ANSWER_TTL_SECS`].

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{NOTIFICATION_OUT_DIR, PHONE_NOTES_FILE};
use crate::protocol::notification_action::{valid_action, valid_key, valid_reply};

/// Notifications kept, at most, over all phones.
pub const MAX_KEPT: usize = 30;

/// How long an answer waits for the server before it is dropped.
pub const ANSWER_TTL_SECS: u64 = 60;

/// One phone notification that can be answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneNote {
    pub device_id: String,
    /// The phone's id for it.
    pub key: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub at_unix: u64,
    /// Its buttons, by title.
    #[serde(default)]
    pub actions: Vec<String>,
    /// Whether it takes a typed reply.
    #[serde(default)]
    pub reply: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct NotesFile {
    notes: Vec<PhoneNote>,
}

/// The kept notifications, oldest first; none when there is no file or it
/// cannot be read.
pub fn read(state_dir: &Path) -> Vec<PhoneNote> {
    std::fs::read_to_string(state_dir.join(PHONE_NOTES_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<NotesFile>(&raw).ok())
        .map(|file| file.notes)
        .unwrap_or_default()
}

fn write(state_dir: &Path, notes: Vec<PhoneNote>) -> io::Result<()> {
    let json = serde_json::to_string(&NotesFile { notes }).map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_NOTES_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    // What a phone's notifications say: owner-only, as texts are.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Keep `note`, replacing the phone's earlier one of the same key; the
/// oldest go past [`MAX_KEPT`].
pub fn keep(state_dir: &Path, note: PhoneNote) -> io::Result<()> {
    let mut notes = read(state_dir);
    notes.retain(|n| !(n.device_id == note.device_id && n.key == note.key));
    notes.push(note);
    let excess = notes.len().saturating_sub(MAX_KEPT);
    notes.drain(..excess);
    write(state_dir, notes)
}

/// Drop the phone's notification `key` (the phone closed it).
pub fn forget(state_dir: &Path, device_id: &str, key: &str) -> io::Result<()> {
    let mut notes = read(state_dir);
    let before = notes.len();
    notes.retain(|n| !(n.device_id == device_id && n.key == key));
    if notes.len() == before {
        return Ok(());
    }
    write(state_dir, notes)
}

/// Drop them all (at start: they belong to a run that ended).
pub fn clear(state_dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(state_dir.join(PHONE_NOTES_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// What the computer answers: a button, or a typed reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Action(String),
    Reply(String),
}

impl Answer {
    fn valid(&self) -> bool {
        match self {
            Answer::Action(title) => valid_action(title),
            Answer::Reply(message) => valid_reply(message),
        }
    }
}

/// One answer for the server to send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub id: String,
    pub device_id: String,
    pub key: String,
    pub answer: Answer,
    pub created_unix: u64,
}

/// Ask the server to send `answer` to the phone's notification `key` (the
/// app's side). An answer out of bounds is refused here.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    key: &str,
    answer: Answer,
    now_unix: u64,
) -> io::Result<AnswerRequest> {
    if !valid_key(key) || !answer.valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an answer the phone takes",
        ));
    }
    let request = AnswerRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        key: key.to_string(),
        answer,
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(NOTIFICATION_OUT_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    // A reply is what the person typed: owner-only.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting answer (the server's side), oldest first. Each file
/// is removed once read; one that is stale, from the future, unreadable,
/// named for another request, or out of bounds is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<AnswerRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(NOTIFICATION_OUT_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<AnswerRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= ANSWER_TTL_SECS;
        if fresh
            && request.id == stem
            && is_valid_id(&request.id)
            && valid_key(&request.key)
            && request.answer.valid()
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
    Ok(state_dir
        .join(NOTIFICATION_OUT_DIR)
        .join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-notes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn note(device: &str, key: &str, at: u64) -> PhoneNote {
        PhoneNote {
            device_id: device.into(),
            key: key.into(),
            app: "Chat".into(),
            title: "Ann".into(),
            text: "Lunch?".into(),
            at_unix: at,
            actions: vec!["Mark as read".into()],
            reply: true,
        }
    }

    #[test]
    fn the_newest_are_kept_and_a_closed_one_goes() {
        let dir = temp_dir("keep");
        for i in 0..(MAX_KEPT as u64 + 5) {
            keep(&dir, note("phone", &format!("k{i}"), i)).unwrap();
        }
        let notes = read(&dir);
        assert_eq!(notes.len(), MAX_KEPT);
        assert_eq!(notes[0].key, "k5", "the oldest went");
        // The same key again replaces, not adds.
        keep(&dir, note("phone", "k10", 99)).unwrap();
        assert_eq!(read(&dir).len(), MAX_KEPT);
        assert_eq!(read(&dir).last().unwrap().at_unix, 99);
        forget(&dir, "phone", "k10").unwrap();
        assert!(read(&dir).iter().all(|n| n.key != "k10"));
        forget(&dir, "other-phone", "k11").unwrap();
        assert!(
            read(&dir).iter().any(|n| n.key == "k11"),
            "only that phone's"
        );
        clear(&dir).unwrap();
        assert!(read(&dir).is_empty());
        clear(&dir).unwrap();
    }

    #[test]
    fn an_answer_is_taken_once_and_only_while_fresh() {
        let dir = temp_dir("answers");
        let reply = request(
            &dir,
            "phone",
            "k1",
            Answer::Reply("On my way".into()),
            1_000,
        )
        .unwrap();
        request(
            &dir,
            "phone",
            "k1",
            Answer::Action("Mark as read".into()),
            1_001,
        )
        .unwrap();
        request(
            &dir,
            "phone",
            "k2",
            Answer::Reply("late".into()),
            1_000 - ANSWER_TTL_SECS - 1,
        )
        .unwrap();
        let taken = take_all(&dir, 1_005);
        assert_eq!(taken.len(), 2, "the stale one dropped");
        assert_eq!(taken[0], reply);
        assert!(take_all(&dir, 1_005).is_empty(), "taken once");
        assert!(request(&dir, "phone", "k1", Answer::Reply("  ".into()), 1_000).is_err());
        assert!(request(&dir, "phone", "", Answer::Action("Ok".into()), 1_000).is_err());
    }
}
