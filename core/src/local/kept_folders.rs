//! Phone folders kept on this computer, by themselves (the phone's
//! "Folders kept on my computer").
//!
//! The server records each folder a phone keeps here in
//! `<state>/kept_folders.json`: where it lands, how much it holds, and how
//! its last run went. The desktop app's Files page lists them. Only the
//! server writes the file, so a reader never has to merge.
//!
//! The registry is the server's own record, never the phone's word: the
//! folder's place on disk is chosen here, once, from the phone's name and
//! the folder's label, and stays when either is renamed.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::KEPT_FOLDERS_FILE;

/// How many phone folders the registry holds, so a phone cannot grow it
/// without end.
pub const MAX_FOLDERS: usize = 64;

/// How many recent problems one folder remembers.
pub const KEEP_ERRORS: usize = 5;

/// The most of a problem's words kept.
pub const ERROR_CHARS: usize = 200;

/// One problem, with when it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptError {
    pub at_unix: u64,
    pub message: String,
}

/// One phone folder kept here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptFolder {
    /// The phone's id.
    pub device_id: String,
    /// The folder's id on that phone (32 hex digits, the phone's own).
    pub folder_id: String,
    /// The phone's name when the folder was first kept.
    pub phone: String,
    /// The folder's name on the phone.
    pub label: String,
    /// The folder's place here, as an absolute path.
    pub path: String,
    /// The phone's folder below the kept-folders root, and the kept
    /// folder below that: the names that were made for them.
    pub phone_dir: String,
    pub folder_dir: String,
    /// Files and bytes held now, counted at the end of each run.
    pub files: u64,
    pub bytes: u64,
    /// When a file last arrived or a run last ended; 0 for never.
    pub last_unix: u64,
    /// How the last run went: files received, and files that failed.
    #[serde(default)]
    pub last_sent: u64,
    #[serde(default)]
    pub last_failed: u64,
    /// Files this phone deleted here as it mirrored its deletions, in all.
    #[serde(default)]
    pub deleted: u64,
    /// Deletions the phone asked for and this computer did not make (its
    /// setting is off), in all.
    #[serde(default)]
    pub deletes_skipped: u64,
    /// Whether the phone's last run asked to mirror deletions.
    #[serde(default)]
    pub mirror: bool,
    /// The newest problems, oldest first.
    #[serde(default)]
    pub errors: Vec<KeptError>,
}

#[derive(Default, Serialize, Deserialize)]
struct KeptList {
    folders: Vec<KeptFolder>,
}

/// The kept folders. A missing or unreadable list is empty.
pub fn read(state_dir: &Path) -> Vec<KeptFolder> {
    std::fs::read_to_string(state_dir.join(KEPT_FOLDERS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<KeptList>(&raw).ok())
        .map(|list| list.folders)
        .unwrap_or_default()
}

/// Replace the list (the server's side; it serializes its calls). The file
/// is replaced whole, so a reader never sees it half-written.
pub fn write(state_dir: &Path, folders: &[KeptFolder]) -> io::Result<()> {
    let json = serde_json::to_string(&KeptList {
        folders: folders.to_vec(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(KEPT_FOLDERS_FILE);
    let tmp = crate::atomic_file::temp_beside(&path);
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &path)
}

/// The bytes all the folders hold together.
pub fn used_bytes(folders: &[KeptFolder]) -> u64 {
    folders
        .iter()
        .fold(0u64, |sum, f| sum.saturating_add(f.bytes))
}

/// Why a file of `incoming` bytes may not be kept, or `None` when it may:
/// the folders hold `used` bytes, `replacing` of them this file's own
/// older copy, and the cap is `cap_gb` GiB (0 for none).
pub fn cap_refusal(used: u64, incoming: u64, replacing: u64, cap_gb: u32) -> Option<String> {
    if cap_gb == 0 {
        return None;
    }
    let cap = u64::from(cap_gb) * 1024 * 1024 * 1024;
    let after = used.saturating_sub(replacing).saturating_add(incoming);
    if after <= cap {
        return None;
    }
    Some(format!(
        "{}: {} of the {cap_gb} GB cap; raise the cap there or free space",
        crate::protocol::kept::FULL,
        gigabytes(used)
    ))
}

/// `3_221_225_472` as "3.0 GB", one decimal.
fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// Whether a phone's deletions are made here: it asked (its own switch)
/// and this computer allows it (its setting). Both, never one.
pub fn deletes_allowed(phone_mirrors: bool, computer_allows: bool) -> bool {
    phone_mirrors && computer_allows
}

/// A name for a new child of a folder, close to `want`, that none of
/// `taken` has: `want`, else `want (2)`, `want (3)`... Names are compared
/// without regard to case, as the disk may.
pub fn unique_name(taken: &[String], want: &str) -> String {
    let has = |name: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    if !has(want) {
        return want.to_string();
    }
    (2u32..)
        .map(|n| format!("{want} ({n})"))
        .find(|name| !has(name))
        .unwrap_or_else(|| want.to_string())
}

/// Add a problem to `folder`, keeping the newest [`KEEP_ERRORS`]; the same
/// words twice in a row are one.
pub fn push_error(folder: &mut KeptFolder, now_unix: u64, message: &str) {
    let message: String = message.chars().take(ERROR_CHARS).collect();
    if let Some(last) = folder.errors.last_mut()
        && last.message == message
    {
        last.at_unix = now_unix;
        return;
    }
    folder.errors.push(KeptError {
        at_unix: now_unix,
        message,
    });
    let excess = folder.errors.len().saturating_sub(KEEP_ERRORS);
    folder.errors.drain(..excess);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(device: &str, id: &str, bytes: u64) -> KeptFolder {
        KeptFolder {
            device_id: device.to_string(),
            folder_id: id.to_string(),
            phone: "Phone".to_string(),
            label: "Notes".to_string(),
            path: "/kept/Phone/Notes".to_string(),
            phone_dir: "Phone".to_string(),
            folder_dir: "Notes".to_string(),
            files: 1,
            bytes,
            last_unix: 5,
            last_sent: 1,
            last_failed: 0,
            deleted: 0,
            deletes_skipped: 0,
            mirror: false,
            errors: Vec::new(),
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-kept-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_list_round_trips_and_a_missing_one_is_empty() {
        let dir = temp_dir("round");
        assert!(read(&dir).is_empty());
        let list = vec![
            folder("a", &"1".repeat(32), 10),
            folder("b", &"2".repeat(32), 20),
        ];
        write(&dir, &list).unwrap();
        assert_eq!(read(&dir), list);
        assert_eq!(used_bytes(&list), 30);
        std::fs::write(dir.join(KEPT_FOLDERS_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty(), "unreadable is empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_refuses_what_would_pass_it_and_counts_a_replaced_copy_once() {
        const GIB: u64 = 1024 * 1024 * 1024;
        assert_eq!(cap_refusal(10 * GIB, GIB, 0, 0), None, "0 is no limit");
        assert_eq!(cap_refusal(9 * GIB, GIB, 0, 10), None, "exactly the cap");
        let why = cap_refusal(9 * GIB + 1, GIB, 0, 10).expect("over");
        assert!(why.contains("full") && why.contains("10 GB"), "{why}");
        // The file's older copy goes when the new one lands.
        assert_eq!(cap_refusal(10 * GIB, GIB, GIB, 10), None);
        assert!(cap_refusal(10 * GIB, 2 * GIB, GIB, 10).is_some());
        // Nothing overflows.
        assert!(cap_refusal(u64::MAX, u64::MAX, 0, 1).is_some());
        assert_eq!(cap_refusal(0, 0, u64::MAX, 1), None);
    }

    #[test]
    fn a_delete_needs_the_phone_and_the_computer_both() {
        assert!(deletes_allowed(true, true));
        assert!(!deletes_allowed(true, false));
        assert!(!deletes_allowed(false, true));
        assert!(!deletes_allowed(false, false));
    }

    #[test]
    fn a_new_name_never_takes_one_that_is_there() {
        let taken = vec!["Notes".to_string(), "notes (2)".to_string()];
        assert_eq!(unique_name(&taken, "Docs"), "Docs");
        assert_eq!(unique_name(&taken, "NOTES"), "NOTES (3)");
        assert_eq!(unique_name(&[], "Notes"), "Notes");
    }

    #[test]
    fn problems_are_bounded_and_a_repeat_is_one() {
        let mut f = folder("a", &"1".repeat(32), 0);
        push_error(&mut f, 1, "full");
        push_error(&mut f, 2, "full");
        assert_eq!(f.errors.len(), 1);
        assert_eq!(f.errors[0].at_unix, 2);
        for n in 0..10 {
            push_error(&mut f, 3, &format!("problem {n}"));
        }
        assert_eq!(f.errors.len(), KEEP_ERRORS);
        assert_eq!(f.errors.last().unwrap().message, "problem 9");
        push_error(&mut f, 4, &"x".repeat(1000));
        assert_eq!(
            f.errors.last().unwrap().message.chars().count(),
            ERROR_CHARS
        );
    }
}
