//! Files the desktop has received from phones (roadmap F7).
//!
//! The server appends each completed transfer to `<state>/received.json`,
//! the newest last, keeping [`KEEP`] entries; the desktop app lists them.
//! Only the server writes the file, so a reader never has to merge.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::RECEIVED_FILE;

/// How many received files the list remembers.
pub const KEEP: usize = 50;

/// One file received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceivedFile {
    /// The name it was saved under.
    pub name: String,
    /// Where it was saved.
    pub path: String,
    pub size: u64,
    pub received_unix: u64,
    /// The device it came from (its id, or its address on the TCP channel).
    pub from: String,
}

#[derive(Default, Serialize, Deserialize)]
struct ReceivedList {
    files: Vec<ReceivedFile>,
}

/// The received files, oldest first. A missing or unreadable list is empty.
pub fn read(state_dir: &Path) -> Vec<ReceivedFile> {
    std::fs::read_to_string(state_dir.join(RECEIVED_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<ReceivedList>(&raw).ok())
        .map(|list| list.files)
        .unwrap_or_default()
}

/// Add `file` to the list, dropping the oldest beyond [`KEEP`]. The caller
/// serializes calls; the file is replaced whole, so a reader never sees it
/// half-written.
pub fn record(state_dir: &Path, file: ReceivedFile) -> io::Result<()> {
    let mut files = read(state_dir);
    files.push(file);
    let excess = files.len().saturating_sub(KEEP);
    files.drain(..excess);
    let json = serde_json::to_string(&ReceivedList { files }).map_err(io::Error::other)?;
    let path = state_dir.join(RECEIVED_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-received-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(n: u64) -> ReceivedFile {
        ReceivedFile {
            name: format!("f{n}.jpg"),
            path: format!("/downloads/f{n}.jpg"),
            size: n,
            received_unix: n,
            from: "phone".to_string(),
        }
    }

    #[test]
    fn received_files_are_kept_newest_last_up_to_the_limit() {
        let dir = temp_dir("keep");
        assert!(read(&dir).is_empty(), "no list yet");
        for n in 0..(KEEP as u64 + 5) {
            record(&dir, file(n)).unwrap();
        }
        let files = read(&dir);
        assert_eq!(files.len(), KEEP);
        assert_eq!(files.first(), Some(&file(5)));
        assert_eq!(files.last(), Some(&file(KEEP as u64 + 4)));
    }

    #[test]
    fn a_corrupt_list_reads_as_empty_and_is_replaced() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(RECEIVED_FILE), "{not json").unwrap();
        assert!(read(&dir).is_empty());
        record(&dir, file(1)).unwrap();
        assert_eq!(read(&dir), [file(1)]);
    }
}
