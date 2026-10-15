//! The phones' media on the computer (roadmap T8d): each phone's last
//! report, kept by the server in `<state>/phone_media.json`, and the
//! presses the desktop app asks the server to send a phone, as
//! `<state>/phone_media_out/<id>.json`, one file per press, taken (read and
//! deleted) by the server. Only the server writes the reports.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{PHONE_MEDIA_DIR, PHONE_MEDIA_FILE};
use crate::protocol::phone_media::{PhoneMedia, is_command};

/// How long a press waits for the server before it is dropped: a press is
/// for now, not for whenever the phone comes back.
pub const PRESS_TTL_SECS: u64 = 10;

/// A phone's last report and when it came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneMediaState {
    pub media: PhoneMedia,
    pub updated_unix: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct MediaFile {
    phones: BTreeMap<String, PhoneMediaState>,
}

/// Each phone's last report of something playing, by device id. A missing
/// or unreadable file is empty.
pub fn read(state_dir: &Path) -> BTreeMap<String, PhoneMediaState> {
    std::fs::read_to_string(state_dir.join(PHONE_MEDIA_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<MediaFile>(&raw).ok())
        .map(|file| file.phones)
        .unwrap_or_default()
}

/// Replace the file with `phones`, whole, so a reader never sees it
/// half-written.
pub fn write(state_dir: &Path, phones: &BTreeMap<String, PhoneMediaState>) -> io::Result<()> {
    let json = serde_json::to_string(&MediaFile {
        phones: phones.clone(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_MEDIA_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// One press for a phone's player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPress {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// One of `phone_media::COMMANDS`.
    pub command: String,
    pub created_unix: u64,
}

/// Ask the server to send `command` to `device_id`'s player (the app's
/// side). A command that is not a press is refused here.
pub fn press(
    state_dir: &Path,
    device_id: &str,
    command: &str,
    now_unix: u64,
) -> io::Result<MediaPress> {
    if !is_command(command) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a media command",
        ));
    }
    let press = MediaPress {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        command: command.to_string(),
        created_unix: now_unix,
    };
    let file = press_path(state_dir, &press.id)?;
    std::fs::create_dir_all(state_dir.join(PHONE_MEDIA_DIR))?;
    let json = serde_json::to_string(&press).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &file)?;
    Ok(press)
}

/// Take every waiting press (the server's side). Each file is removed once
/// read; one that is stale, from the future, unreadable, named for another
/// press, or not a command is dropped.
pub fn take_presses(state_dir: &Path, now_unix: u64) -> Vec<MediaPress> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PHONE_MEDIA_DIR)) else {
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
        let Some(press) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<MediaPress>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            press.created_unix <= now_unix && now_unix - press.created_unix <= PRESS_TTL_SECS;
        if fresh && press.id == stem && is_valid_id(&press.id) && is_command(&press.command) {
            taken.push(press);
        }
    }
    taken.sort_by_key(|p| p.created_unix);
    taken
}

fn press_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a press id",
        ));
    }
    Ok(state_dir.join(PHONE_MEDIA_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-phone-media-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn song() -> PhoneMedia {
        PhoneMedia {
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            app: "Spotify".to_string(),
            is_playing: true,
            can_play_pause: true,
            can_next: true,
            can_previous: true,
        }
    }

    #[test]
    fn the_reports_are_written_and_read_back_and_a_bad_file_is_empty() {
        let dir = temp_dir("reports");
        assert!(read(&dir).is_empty());
        let mut phones = BTreeMap::new();
        phones.insert(
            "phone-a".to_string(),
            PhoneMediaState {
                media: song(),
                updated_unix: 100,
            },
        );
        write(&dir, &phones).unwrap();
        assert_eq!(read(&dir), phones);
        std::fs::write(dir.join(PHONE_MEDIA_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty());
    }

    #[test]
    fn a_press_is_taken_once_and_a_stale_or_bad_one_never() {
        let dir = temp_dir("presses");
        assert!(take_presses(&dir, 100).is_empty());
        let pressed = press(&dir, "phone-a", "next", 100).unwrap();
        assert_eq!(take_presses(&dir, 101), [pressed]);
        assert!(take_presses(&dir, 101).is_empty(), "taken");

        assert_eq!(
            press(&dir, "phone-a", "stop", 100).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        press(&dir, "phone-a", "playPause", 100).unwrap();
        assert!(
            take_presses(&dir, 100 + PRESS_TTL_SECS + 1).is_empty(),
            "stale"
        );
        press(&dir, "phone-a", "playPause", 200).unwrap();
        assert!(take_presses(&dir, 100).is_empty(), "from the future");

        // Written by hand, past the check: the server's side checks again.
        let id = uuid::Uuid::new_v4().to_string();
        let forged = MediaPress {
            id: id.clone(),
            device_id: "phone-a".to_string(),
            command: "seek".to_string(),
            created_unix: 100,
        };
        std::fs::write(
            dir.join(PHONE_MEDIA_DIR).join(format!("{id}.json")),
            serde_json::to_string(&forged).unwrap(),
        )
        .unwrap();
        let mislabelled = MediaPress {
            id: uuid::Uuid::new_v4().to_string(),
            ..press(&dir, "phone-a", "next", 100).unwrap()
        };
        std::fs::write(
            dir.join(PHONE_MEDIA_DIR)
                .join(format!("{}.json", uuid::Uuid::new_v4())),
            serde_json::to_string(&mislabelled).unwrap(),
        )
        .unwrap();
        let taken = take_presses(&dir, 101);
        assert_eq!(taken.len(), 1, "only the honest press: {taken:?}");
        assert_eq!(taken[0].command, "next");
    }
}
