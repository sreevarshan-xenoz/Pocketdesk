//! KDE Connect devices this computer has seen (3a), for the desktop app, and
//! the app's request to look for them.
//!
//! The server keeps `<state>/kdeconnect_seen.json`, the newest first, at
//! most [`KEEP`]: each device that completed the handshake, by its KDE id
//! and name. Only the server writes it. The app asks the server to look
//! (to broadcast this computer's identity on the network) by writing, into
//! `<state>/kdeconnect_find`, until when.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::{KDECONNECT_FIND_FILE, KDECONNECT_SEEN_FILE};

/// How many devices the list remembers.
pub const KEEP: usize = 32;

/// The longest the app may ask the server to look for, in seconds.
pub const MAX_FIND_SECS: u64 = 120;

/// One KDE Connect device seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenDevice {
    pub device_id: String,
    pub name: String,
    pub seen_unix: u64,
    #[serde(default)]
    pub paired: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct SeenFile {
    #[serde(default)]
    devices: Vec<SeenDevice>,
}

/// The devices seen, newest first. A missing or unreadable file is empty.
pub fn read(state_dir: &Path) -> Vec<SeenDevice> {
    std::fs::read_to_string(state_dir.join(KDECONNECT_SEEN_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<SeenFile>(&raw).ok())
        .map(|file| file.devices)
        .unwrap_or_default()
}

/// Note `device` (the server's side): it goes first, in place of an older
/// note of the same id, and the list keeps [`KEEP`].
pub fn record(state_dir: &Path, device: SeenDevice) -> io::Result<()> {
    // One writer at a time: links come and go side by side.
    // Keeps this file's writes in turn. On Windows a contended save holds it up to
    // about half a second (`atomic_file::replace` waits out another handle).
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = read(state_dir);
    devices.retain(|d| d.device_id != device.device_id);
    devices.insert(0, device);
    devices.truncate(KEEP);
    let json = serde_json::to_string_pretty(&SeenFile { devices }).map_err(io::Error::other)?;
    let path = state_dir.join(KDECONNECT_SEEN_FILE);
    let tmp = crate::atomic_file::temp_beside(&path);
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// Ask the server to look for devices for `secs` from `now_unix` (the
/// app's side), at most [`MAX_FIND_SECS`].
pub fn ask_to_find(state_dir: &Path, now_unix: u64, secs: u64) -> io::Result<()> {
    let until = now_unix + secs.min(MAX_FIND_SECS);
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(state_dir.join(KDECONNECT_FIND_FILE), until.to_string())
}

/// Whether the app asks the server to look now: until a time not past and
/// not further off than [`MAX_FIND_SECS`].
pub fn finding(state_dir: &Path, now_unix: u64) -> bool {
    find_until(state_dir, now_unix).is_some()
}

/// Until when the app asks the server to look, while it does: each press
/// of "Find" names a new time, which is how the server tells one request
/// from the next.
pub fn find_until(state_dir: &Path, now_unix: u64) -> Option<u64> {
    std::fs::read_to_string(state_dir.join(KDECONNECT_FIND_FILE))
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|until| *until >= now_unix && until - now_unix <= MAX_FIND_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-kdeconnect-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seen(id: &str, at: u64) -> SeenDevice {
        SeenDevice {
            device_id: id.to_string(),
            name: "Phone".to_string(),
            seen_unix: at,
            paired: false,
        }
    }

    #[test]
    fn a_device_seen_again_moves_first_and_the_list_keeps_its_size() {
        let dir = temp_dir("seen");
        assert!(read(&dir).is_empty());
        record(&dir, seen("a", 1)).unwrap();
        record(&dir, seen("b", 2)).unwrap();
        record(&dir, seen("a", 3)).unwrap();
        let ids: Vec<_> = read(&dir).into_iter().map(|d| d.device_id).collect();
        assert_eq!(ids, ["a", "b"]);
        for n in 0..40 {
            record(&dir, seen(&format!("d{n}"), 10 + n)).unwrap();
        }
        assert_eq!(read(&dir).len(), KEEP);
    }

    #[test]
    fn looking_lasts_as_long_as_asked_and_no_longer_than_two_minutes() {
        let dir = temp_dir("find");
        assert!(!finding(&dir, 100));
        ask_to_find(&dir, 100, 60).unwrap();
        assert!(finding(&dir, 100) && finding(&dir, 160));
        assert!(!finding(&dir, 161));
        ask_to_find(&dir, 100, 10_000).unwrap();
        assert!(!finding(&dir, 100 + MAX_FIND_SECS + 1));
        std::fs::write(dir.join(KDECONNECT_FIND_FILE), "99999999999").unwrap();
        assert!(!finding(&dir, 100), "a time too far off is not taken");
    }
}
