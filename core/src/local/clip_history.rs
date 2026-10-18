//! The clipboard's history across devices (D3): the texts that crossed
//! between this computer and its phones, either way, newest first, kept
//! owner-only in `<state>/clip_history.json` while `clipboard_history` (a
//! count, 0 for off) allows. The server adds; the desktop app reads, and
//! empties it with Clear. A copy marked private never crosses, so it is
//! never here; nor is a pairing link.
//!
//! The desktop app or the local API asks for an entry to go to a phone as
//! `<state>/clipboard_out/<id>.json`, taken by the server.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{CLIP_HISTORY_FILE, CLIPBOARD_OUT_DIR};

/// The most entries `clipboard_history` may ask for.
pub const MAX_ENTRIES: u32 = 100;

/// The longest text kept, in bytes; a longer one is kept cut, and says so.
pub const MAX_TEXT: usize = 64 * 1024;

/// How long a send request waits for the server.
pub const REQUEST_TTL_SECS: u64 = 60;

/// One text that crossed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipEntry {
    pub text: String,
    /// The phone (or computer) it came from; `None` for this computer.
    pub from: Option<String>,
    pub at_unix: u64,
    /// The text was longer than [`MAX_TEXT`] and is kept cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

impl ClipEntry {
    /// An entry for `text` from `from` at `at_unix`, cut to [`MAX_TEXT`] on
    /// a character boundary.
    pub fn new(text: &str, from: Option<&str>, at_unix: u64) -> Self {
        let cut = text.len() > MAX_TEXT;
        let mut end = text.len().min(MAX_TEXT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].to_string(),
            from: from.map(str::to_string),
            at_unix,
            cut,
        }
    }
}

/// Put `entry` first in `entries`: the same text again moves to the top
/// (with its new origin and time); at most `limit` are kept.
pub fn add(entries: &mut Vec<ClipEntry>, entry: ClipEntry, limit: u32) {
    if limit == 0 || entry.text.trim().is_empty() {
        return;
    }
    entries.retain(|e| e.text != entry.text);
    entries.insert(0, entry);
    entries.truncate(limit.min(MAX_ENTRIES) as usize);
}

#[derive(Default, Serialize, Deserialize)]
struct HistoryFile {
    entries: Vec<ClipEntry>,
}

/// The history, newest first. A missing or unreadable file is empty.
pub fn read(state_dir: &Path) -> Vec<ClipEntry> {
    std::fs::read_to_string(state_dir.join(CLIP_HISTORY_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<HistoryFile>(&raw).ok())
        .map(|file| file.entries)
        .unwrap_or_default()
}

/// Replace the history with `entries`, whole and owner-only.
pub fn write(state_dir: &Path, entries: &[ClipEntry]) -> io::Result<()> {
    let json = serde_json::to_string(&HistoryFile {
        entries: entries.to_vec(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(CLIP_HISTORY_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// Add `entry` to the history on disk, at most `limit` kept.
pub fn record(state_dir: &Path, entry: ClipEntry, limit: u32) -> io::Result<()> {
    let mut entries = read(state_dir);
    add(&mut entries, entry, limit);
    write(state_dir, &entries)
}

/// Empty the history: Clear, or the setting turned off.
pub fn clear(state_dir: &Path) -> io::Result<()> {
    match std::fs::remove_file(state_dir.join(CLIP_HISTORY_FILE)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// One text to put on a phone's clipboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest {
    pub id: String,
    pub device_id: String,
    pub text: String,
    pub created_unix: u64,
}

/// Ask the server to put `text` on `device_id`'s clipboard.
pub fn request_send(
    state_dir: &Path,
    device_id: &str,
    text: &str,
    now_unix: u64,
) -> io::Result<SendRequest> {
    if text.trim().is_empty() || text.len() > MAX_TEXT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty, or longer than a clipboard entry is kept",
        ));
    }
    let request = SendRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        text: text.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(CLIPBOARD_OUT_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting send (the server's side): read and removed; one that
/// is stale, from the future, unreadable or misnamed is dropped.
pub fn take_sends(state_dir: &Path, now_unix: u64) -> Vec<SendRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(CLIPBOARD_OUT_DIR)) else {
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
            .and_then(|raw| serde_json::from_str::<SendRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) && request.text.len() <= MAX_TEXT
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
    Ok(state_dir.join(CLIPBOARD_OUT_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-clip-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn newest_first_the_same_text_moves_to_the_top_and_the_limit_holds() {
        let mut entries = Vec::new();
        add(&mut entries, ClipEntry::new("one", Some("p"), 1), 3);
        add(&mut entries, ClipEntry::new("two", None, 2), 3);
        add(&mut entries, ClipEntry::new("one", None, 3), 3);
        assert_eq!(
            entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(entries[0].from, None, "its new origin");
        add(&mut entries, ClipEntry::new("three", None, 4), 3);
        add(&mut entries, ClipEntry::new("four", None, 5), 3);
        assert_eq!(
            entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["four", "three", "one"]
        );
        // Off, or blank: nothing.
        add(&mut entries, ClipEntry::new("five", None, 6), 0);
        add(&mut entries, ClipEntry::new("  \n", None, 6), 3);
        assert_eq!(entries[0].text, "four");
    }

    #[test]
    fn a_long_text_is_kept_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_TEXT); // two bytes each
        let entry = ClipEntry::new(&long, None, 1);
        assert!(entry.cut);
        assert!(entry.text.len() <= MAX_TEXT);
        assert!(entry.text.chars().all(|c| c == 'é'));
        assert!(!ClipEntry::new("short", None, 1).cut);
    }

    #[test]
    fn recorded_read_and_cleared() {
        let dir = temp_dir("rw");
        assert!(read(&dir).is_empty());
        record(&dir, ClipEntry::new("a", Some("p"), 1), 20).unwrap();
        record(&dir, ClipEntry::new("b", None, 2), 20).unwrap();
        assert_eq!(read(&dir).len(), 2);
        assert_eq!(read(&dir)[0].text, "b");
        clear(&dir).unwrap();
        assert!(read(&dir).is_empty());
        clear(&dir).unwrap();
        std::fs::write(dir.join(CLIP_HISTORY_FILE), "nonsense").unwrap();
        assert!(read(&dir).is_empty(), "unreadable is empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_send_is_taken_once_fresh_and_never_empty() {
        let dir = temp_dir("send");
        assert!(request_send(&dir, "p", " ", 100).is_err());
        let asked = request_send(&dir, "p", "hello", 100).unwrap();
        assert_eq!(take_sends(&dir, 101), [asked]);
        assert!(take_sends(&dir, 101).is_empty(), "taken");
        request_send(&dir, "p", "late", 100).unwrap();
        assert!(
            take_sends(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "stale"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
