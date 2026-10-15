//! Files the desktop has sent, or tried to send, to phones (roadmap F7b).
//!
//! The server records each request it takes from the outbox in
//! `<state>/sent.json` and updates it as the transfer goes, keeping
//! [`KEEP`] entries; the desktop app lists them. Only the server writes the
//! file, so a reader never has to merge.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::SENT_FILE;

/// How many sent files the list remembers.
pub const KEEP: usize = 50;

/// Where a send stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum SentState {
    /// The phone was told about it and has not fetched it yet.
    Offered,
    /// The phone saved it.
    Sent,
    /// It did not reach the phone, and why.
    Failed(String),
}

/// One file sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentFile {
    /// The outbox request's id.
    pub id: String,
    pub name: String,
    /// The file on this computer.
    pub path: String,
    pub size: u64,
    pub device_id: String,
    pub sent_unix: u64,
    pub state: SentState,
    /// What the person should know beside the state: what a folder send
    /// (T6b) left out. Absent in lists written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct SentList {
    files: Vec<SentFile>,
}

/// The sent files, oldest first. A missing or unreadable list is empty.
pub fn read(state_dir: &Path) -> Vec<SentFile> {
    std::fs::read_to_string(state_dir.join(SENT_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<SentList>(&raw).ok())
        .map(|list| list.files)
        .unwrap_or_default()
}

/// Add `file`, dropping the oldest beyond [`KEEP`]. The caller serializes
/// calls; the file is replaced whole.
pub fn record(state_dir: &Path, file: SentFile) -> io::Result<()> {
    let mut files = read(state_dir);
    files.push(file);
    let excess = files.len().saturating_sub(KEEP);
    files.drain(..excess);
    write(state_dir, files)
}

/// Set the state of the send `id`; nothing happens if it is no longer
/// listed.
pub fn update(state_dir: &Path, id: &str, state: SentState) -> io::Result<()> {
    let mut files = read(state_dir);
    let Some(file) = files.iter_mut().find(|f| f.id == id) else {
        return Ok(());
    };
    file.state = state;
    write(state_dir, files)
}

fn write(state_dir: &Path, files: Vec<SentFile>) -> io::Result<()> {
    let json = serde_json::to_string(&SentList { files }).map_err(io::Error::other)?;
    let path = state_dir.join(SENT_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-sent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(n: u64) -> SentFile {
        SentFile {
            id: format!("id-{n}"),
            name: format!("f{n}.pdf"),
            path: format!("/home/me/f{n}.pdf"),
            size: n,
            device_id: "phone".to_string(),
            sent_unix: n,
            state: SentState::Offered,
            note: None,
        }
    }

    #[test]
    fn sent_files_are_kept_newest_last_up_to_the_limit() {
        let dir = temp_dir("keep");
        assert!(read(&dir).is_empty());
        for n in 0..(KEEP as u64 + 3) {
            record(&dir, file(n)).unwrap();
        }
        let files = read(&dir);
        assert_eq!(files.len(), KEEP);
        assert_eq!(files.first(), Some(&file(3)));
    }

    #[test]
    fn a_note_is_kept_and_a_row_without_one_still_reads() {
        let dir = temp_dir("note");
        let mut noted = file(1);
        noted.note = Some("2 files left out".to_string());
        record(&dir, noted.clone()).unwrap();
        assert_eq!(read(&dir), [noted]);
        // A row without one is written as before notes existed, and such
        // a row still reads.
        let plain = serde_json::to_string(&file(2)).unwrap();
        assert!(!plain.contains("note"), "{plain}");
        std::fs::write(dir.join(SENT_FILE), format!(r#"{{"files":[{plain}]}}"#)).unwrap();
        assert_eq!(read(&dir), [file(2)]);
    }

    #[test]
    fn a_send_is_updated_by_its_id() {
        let dir = temp_dir("update");
        record(&dir, file(1)).unwrap();
        record(&dir, file(2)).unwrap();
        update(
            &dir,
            "id-1",
            SentState::Failed("phone not connected".into()),
        )
        .unwrap();
        update(&dir, "id-2", SentState::Sent).unwrap();
        update(&dir, "gone", SentState::Sent).unwrap();
        let files = read(&dir);
        assert_eq!(
            files[0].state,
            SentState::Failed("phone not connected".into())
        );
        assert_eq!(files[1].state, SentState::Sent);
    }

    #[test]
    fn a_corrupt_list_reads_as_empty_and_is_replaced() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(SENT_FILE), "{not json").unwrap();
        assert!(read(&dir).is_empty());
        record(&dir, file(1)).unwrap();
        assert_eq!(read(&dir), [file(1)]);
    }

    #[test]
    fn the_state_reads_naturally_in_json() {
        let json = serde_json::to_string(&SentState::Failed("x".into())).unwrap();
        assert_eq!(json, r#"{"state":"failed","reason":"x"}"#);
        let json = serde_json::to_string(&SentState::Sent).unwrap();
        assert_eq!(json, r#"{"state":"sent"}"#);
    }
}
