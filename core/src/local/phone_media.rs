//! The phones' media on the computer (roadmap T8d): each phone's last
//! report, kept by the server in `<state>/phone_media.json`, and the
//! presses the desktop app asks the server to send a phone, as
//! `<state>/phone_media_out/<id>.json`, one file per press, taken (read and
//! deleted) by the server. Only the server writes the reports. A report's
//! cover (N2) is kept in `<state>/phone_media_art/`, checked by its header.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{PHONE_MEDIA_ART_DIR, PHONE_MEDIA_DIR, PHONE_MEDIA_FILE};
use crate::protocol::phone_media::{PhoneMedia, is_command};

/// The app id Pocketdesk's own entry in Windows' media controls goes by
/// (K7: the tray shows a phone's music there), which the server's reading
/// of the computer's music must skip (N5): one name for both.
pub const MEDIA_CONTROLS_APP_ID: &str = "Pocketdesk.Server";

/// How long a press waits for the server before it is dropped: a press is
/// for now, not for whenever the phone comes back.
pub const PRESS_TTL_SECS: u64 = 10;

/// A phone's last report and when it came.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneMediaState {
    pub media: PhoneMedia,
    pub updated_unix: u64,
    /// The cover's file in `phone_media_art/` (N2), when one shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

/// The largest cover kept, in bytes: the phone sends at most 256 px a side.
pub const MAX_ART_BYTES: usize = 256 << 10;
/// The longest side a cover may declare.
pub const MAX_ART_SIDE: u32 = 512;
/// The most covers kept, for all phones.
pub const MAX_ARTS: usize = 64;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The start of `phone`'s covers' names.
fn art_prefix(phone: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{}-", hex(&Sha256::digest(phone.as_bytes())[..6]))
}

/// The name `phone`'s cover `art_id` is kept under.
pub fn art_name(phone: &str, art_id: &str) -> String {
    format!("{}{art_id}.jpg", art_prefix(phone))
}

/// Whether `phone`'s cover `art_id` is kept.
pub fn has_art(state_dir: &Path, phone: &str, art_id: &str) -> bool {
    state_dir
        .join(PHONE_MEDIA_ART_DIR)
        .join(art_name(phone, art_id))
        .is_file()
}

/// Keep `bytes` as `phone`'s cover `art_id`, in place of its others, if it
/// is a JPEG of at most [`MAX_ART_BYTES`] declaring at most [`MAX_ART_SIDE`]
/// a side, whose SHA-256 starts with `art_id`; at most [`MAX_ARTS`] are
/// kept. The name kept, or why not.
pub fn keep_art(state_dir: &Path, phone: &str, art_id: &str, bytes: &[u8]) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let refused = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_string());
    if crate::protocol::phone_media::art_id(art_id).is_none() {
        return Err(refused("not a cover's id"));
    }
    let fits = super::notification_icons::jpeg_size(bytes)
        .is_some_and(|(w, h)| w > 0 && h > 0 && w <= MAX_ART_SIDE && h <= MAX_ART_SIDE);
    if bytes.len() > MAX_ART_BYTES || !fits {
        return Err(refused("not a cover"));
    }
    if !hex(&Sha256::digest(bytes)).starts_with(art_id) {
        return Err(refused("the cover is not the one named"));
    }
    let dir = state_dir.join(PHONE_MEDIA_ART_DIR);
    let name = art_name(phone, art_id);
    let path = dir.join(&name);
    if !path.is_file() {
        super::notification_icons::write(&path, bytes)?;
    }
    remove_arts(&dir, phone, Some(&name));
    super::notification_icons::prune(&dir, "", "jpg", MAX_ARTS);
    Ok(name)
}

/// Forget `phone`'s covers.
pub fn forget_art(state_dir: &Path, phone: &str) {
    remove_arts(&state_dir.join(PHONE_MEDIA_ART_DIR), phone, None);
}

fn remove_arts(dir: &Path, phone: &str, keep: Option<&str>) {
    let prefix = art_prefix(phone);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix) && name.ends_with(".jpg") && Some(name) != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Where the cover `name` (a [`PhoneMediaState::art`]) is, if it is one
/// and kept.
pub fn art_path(state_dir: &Path, name: &str) -> Option<PathBuf> {
    let (prefix, rest) = name.split_once('-')?;
    let id = rest.strip_suffix(".jpg")?;
    let is_hex = |s: &str| s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (prefix.len() == 12 && is_hex(prefix) && crate::protocol::phone_media::art_id(id).is_some())
        .then(|| state_dir.join(PHONE_MEDIA_ART_DIR).join(name))
        .filter(|p| p.is_file())
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
    // What a phone plays is listening history: owner-only, as texts are.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
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
            ..Default::default()
        }
    }

    /// A JPEG's start declaring `w` x `h` (what `jpeg_size` reads), then `tail`.
    fn jpeg(w: u16, h: u16, tail: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        bytes.extend_from_slice(&h.to_be_bytes());
        bytes.extend_from_slice(&w.to_be_bytes());
        bytes.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1, 0xFF, 0xD9]);
        bytes.extend_from_slice(tail);
        bytes
    }

    fn id_of(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex(&Sha256::digest(bytes)[..8])
    }

    #[test]
    fn a_cover_is_kept_by_its_id_and_replaces_the_phone_s_older_one() {
        let dir = temp_dir("art");
        let first = jpeg(256, 256, b"one");
        let name = keep_art(&dir, "phone-a", &id_of(&first), &first).unwrap();
        assert!(has_art(&dir, "phone-a", &id_of(&first)));
        assert_eq!(
            art_path(&dir, &name),
            Some(dir.join(PHONE_MEDIA_ART_DIR).join(&name))
        );
        let other = jpeg(100, 100, b"other phone");
        keep_art(&dir, "phone-b", &id_of(&other), &other).unwrap();
        let second = jpeg(256, 256, b"two");
        keep_art(&dir, "phone-a", &id_of(&second), &second).unwrap();
        assert!(
            !has_art(&dir, "phone-a", &id_of(&first)),
            "the older cover went"
        );
        assert!(has_art(&dir, "phone-a", &id_of(&second)));
        assert!(
            has_art(&dir, "phone-b", &id_of(&other)),
            "another phone's stays"
        );
        forget_art(&dir, "phone-a");
        assert!(!has_art(&dir, "phone-a", &id_of(&second)));
        assert!(has_art(&dir, "phone-b", &id_of(&other)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cover_that_is_not_one_is_refused() {
        let dir = temp_dir("art-refused");
        let png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let huge = jpeg(4000, 4000, b"");
        let long = jpeg(10, 10, &vec![0; MAX_ART_BYTES]);
        let good = jpeg(10, 10, b"good");
        for (id, bytes) in [
            (id_of(&png), png.as_slice()),
            (id_of(&huge), huge.as_slice()),
            (id_of(&long), long.as_slice()),
            ("0000000000000000".to_string(), good.as_slice()),
            ("../../x".to_string(), good.as_slice()),
        ] {
            assert!(keep_art(&dir, "phone", &id, bytes).is_err(), "{id}");
        }
        let kept = std::fs::read_dir(dir.join(PHONE_MEDIA_ART_DIR))
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(kept, 0);
        assert_eq!(art_path(&dir, "../trusted_devices.json"), None);
        assert_eq!(
            art_path(&dir, "0123456789ab-0123456789abcdef.jpg"),
            None,
            "not kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_most_so_many_covers_are_kept() {
        let dir = temp_dir("art-many");
        for n in 0..MAX_ARTS + 5 {
            let bytes = jpeg(10, 10, format!("{n}").as_bytes());
            keep_art(&dir, &format!("phone-{n}"), &id_of(&bytes), &bytes).unwrap();
        }
        let count = std::fs::read_dir(dir.join(PHONE_MEDIA_ART_DIR))
            .unwrap()
            .count();
        assert_eq!(count, MAX_ARTS);
        let _ = std::fs::remove_dir_all(&dir);
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
                art: Some("0123456789ab-0123456789abcdef.jpg".to_string()),
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
