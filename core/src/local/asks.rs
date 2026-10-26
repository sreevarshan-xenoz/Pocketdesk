//! Questions for the owner's phone (BF3): `pocketdesk ask` writes
//! `<state>/asks_out/<id>.json`, which the server takes (reads and deletes)
//! and asks the phones with the Ask permission; the first answer it hears
//! goes to `<state>/asks_in/<id>.json`, which `pocketdesk ask` takes.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths::{ASK_ANSWERS_DIR, ASKS_DIR};
use crate::protocol::ask::{Answer, MAX_WAIT_SECS, clean_question, is_ask_id};

/// One question, until it is answered or `expires_unix`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskRequest {
    pub id: String,
    /// The question, cleaned ([`clean_question`]).
    pub question: String,
    pub created_unix: u64,
    pub expires_unix: u64,
}

/// An answer, and the phone it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskAnswer {
    pub id: String,
    pub answer: Answer,
    pub device_id: String,
}

/// Ask the server to put `question` to the phones for `wait_secs` (at most
/// [`MAX_WAIT_SECS`]).
pub fn request(
    state_dir: &Path,
    question: &str,
    wait_secs: u64,
    now_unix: u64,
) -> io::Result<AskRequest> {
    let Some(question) = clean_question(question) else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no question"));
    };
    if wait_secs == 0 || wait_secs > MAX_WAIT_SECS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a question waits 1 to {MAX_WAIT_SECS} seconds"),
        ));
    }
    let request = AskRequest {
        id: uuid::Uuid::new_v4().to_string(),
        question,
        created_unix: now_unix,
        expires_unix: now_unix + wait_secs,
    };
    write(&state_dir.join(ASKS_DIR), &request.id, &request)?;
    Ok(request)
}

/// Take every waiting question (the server's side), oldest first. Each file
/// is removed once read; one expired, from the future, unreadable or named
/// for another is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<AskRequest> {
    let mut taken: Vec<AskRequest> = take_files(&state_dir.join(ASKS_DIR))
        .into_iter()
        .filter_map(|(stem, raw)| {
            let request = serde_json::from_str::<AskRequest>(&raw).ok()?;
            let fresh = request.created_unix <= now_unix
                && now_unix < request.expires_unix
                && request.expires_unix - request.created_unix <= MAX_WAIT_SECS;
            (fresh
                && request.id == stem
                && is_ask_id(&request.id)
                && clean_question(&request.question).as_deref() == Some(request.question.as_str()))
            .then_some(request)
        })
        .collect();
    taken.sort_by_key(|r| r.created_unix);
    taken
}

/// Keep the answer to question `id` for `pocketdesk ask` (the server's side).
pub fn answer(state_dir: &Path, id: &str, answer: Answer, device_id: &str) -> io::Result<()> {
    let answer = AskAnswer {
        id: id.to_string(),
        answer,
        device_id: device_id.to_string(),
    };
    write(&state_dir.join(ASK_ANSWERS_DIR), id, &answer)
}

/// The answer to question `id`, if one has come; taken.
pub fn take_answer(state_dir: &Path, id: &str) -> Option<AskAnswer> {
    let file = path(&state_dir.join(ASK_ANSWERS_DIR), id).ok()?;
    let raw = std::fs::read_to_string(&file).ok()?;
    let _ = std::fs::remove_file(&file);
    serde_json::from_str::<AskAnswer>(&raw)
        .ok()
        .filter(|a| a.id == id)
}

/// Take question `id` back while the server has not yet taken it, and any
/// answer that came too late (`pocketdesk ask` giving up).
pub fn withdraw(state_dir: &Path, id: &str) {
    for folder in [ASKS_DIR, ASK_ANSWERS_DIR] {
        if let Ok(file) = path(&state_dir.join(folder), id) {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn take_files(folder: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let file = entry.path();
        let Some(stem) = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue; // a temp file mid-write
        };
        let raw = std::fs::read_to_string(&file);
        let _ = std::fs::remove_file(&file);
        if let Ok(raw) = raw {
            taken.push((stem, raw));
        }
    }
    taken
}

fn write<T: Serialize>(folder: &Path, id: &str, value: &T) -> io::Result<()> {
    let file = path(folder, id)?;
    crate::secure_fs::create_private_dir(folder)?;
    let json = serde_json::to_string(value).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)
}

fn path(folder: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_ask_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a question id",
        ));
    }
    Ok(folder.join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-asks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_question_is_taken_once_while_it_waits() {
        let dir = dir("take");
        let asked = request(&dir, " Deploy\nnow? ", 60, 1_000).unwrap();
        assert_eq!(asked.question, "Deploy now?");
        assert_eq!(asked.expires_unix, 1_060);
        assert_eq!(take_all(&dir, 1_010), vec![asked]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "Late?", 60, 1_000).unwrap();
        assert!(take_all(&dir, 1_060).is_empty(), "expired");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_question_needs_words_and_a_wait_within_an_hour() {
        let dir = dir("refuse");
        assert!(request(&dir, "  ", 60, 1).is_err());
        assert!(request(&dir, "Go?", 0, 1).is_err());
        assert!(request(&dir, "Go?", MAX_WAIT_SECS + 1, 1).is_err());
        assert!(request(&dir, "Go?", MAX_WAIT_SECS, 1).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_answer_is_taken_once_and_withdrawing_drops_both_sides() {
        let dir = dir("answer");
        let asked = request(&dir, "Go?", 60, 1_000).unwrap();
        assert_eq!(take_answer(&dir, &asked.id), None);
        answer(&dir, &asked.id, Answer::Yes, "phone-a").unwrap();
        assert_eq!(
            take_answer(&dir, &asked.id),
            Some(AskAnswer {
                id: asked.id.clone(),
                answer: Answer::Yes,
                device_id: "phone-a".to_string(),
            })
        );
        assert_eq!(take_answer(&dir, &asked.id), None, "taken");
        answer(&dir, &asked.id, Answer::No, "phone-a").unwrap();
        withdraw(&dir, &asked.id);
        assert_eq!(take_answer(&dir, &asked.id), None);
        assert!(take_all(&dir, 1_001).is_empty(), "withdrawn");
        assert!(answer(&dir, "../x", Answer::Yes, "phone-a").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
