//! What the shared keyboard and mouse knows of the screens of the owner's
//! computers.
//!
//! The server keeps `<state>/shared_screens.json` (owner-only): this
//! computer's own screens first, then each paired computer's as it last
//! said them on the link (its `Hello` or `Screens`). Only the server writes
//! it; the layout is built from it. What another computer says is its to
//! claim, so it is clipped here before it is kept ([`clean_name`],
//! [`clean_screens`]).

use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::paths::SHARED_SCREENS_FILE;

/// The longest name kept or sent for a computer, in bytes; a longer one is
/// cut short at a character.
pub const MAX_NAME_BYTES: usize = 255;

/// The most screens kept or sent for one computer; the rest are left off.
pub const MAX_SCREENS: usize = 64;

/// A monitor, in its computer's own desktop space and logical pixels (what a
/// 125% display shows, not what it has).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Screen {
    /// Whether it has an area and its far edges are still in `i32` (so
    /// `x + width` and `y + height` never wrap).
    pub fn is_sound(&self) -> bool {
        let fits = |start: i32, len: u32| {
            len > 0
                && i32::try_from(len)
                    .ok()
                    .and_then(|len| start.checked_add(len))
                    .is_some()
        };
        fits(self.x, self.width) && fits(self.y, self.height)
    }
}

/// `name` cut to at most [`MAX_NAME_BYTES`] at a character, without its
/// control characters.
pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let mut end = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// `screens` without the unsound ones ([`Screen::is_sound`]), at most
/// [`MAX_SCREENS`] of them.
pub fn clean_screens(screens: &[Screen]) -> Vec<Screen> {
    screens
        .iter()
        .filter(|s| s.is_sound())
        .take(MAX_SCREENS)
        .copied()
        .collect()
}

/// One computer's screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerScreens {
    pub device_id: String,
    pub name: String,
    pub screens: Vec<Screen>,
    /// When its name or screens last changed, in Unix seconds.
    pub changed_unix: u64,
    /// Whether this is this computer's own entry (the first).
    #[serde(default)]
    pub this_computer: bool,
}

/// This computer's screens first, then the other computers' in the order
/// they were first heard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreensFile {
    #[serde(default)]
    pub computers: Vec<ComputerScreens>,
}

impl ScreensFile {
    /// This computer's entry, if it has one.
    pub fn this_computer(&self) -> Option<&ComputerScreens> {
        self.computers.first().filter(|c| c.this_computer)
    }

    /// The computer `device_id`'s entry.
    pub fn get(&self, device_id: &str) -> Option<&ComputerScreens> {
        self.computers.iter().find(|c| c.device_id == device_id)
    }

    /// Set this computer's own screens, as the first entry.
    pub fn set_own(&mut self, device_id: &str, name: &str, screens: &[Screen], now_unix: u64) {
        let unchanged = self.this_computer().is_some_and(|own| {
            own.device_id == device_id && own.name == name && own.screens == screens
        });
        if unchanged {
            return;
        }
        self.computers
            .retain(|c| !c.this_computer && c.device_id != device_id);
        self.computers.insert(
            0,
            ComputerScreens {
                device_id: device_id.to_string(),
                name: clean_name(name),
                screens: clean_screens(screens),
                changed_unix: now_unix,
                this_computer: true,
            },
        );
    }

    /// Keep what the computer `device_id` said of itself: its screens, and
    /// its name when it said one. Both are clipped first. A computer that
    /// names this one's own id is ignored.
    pub fn heard(
        &mut self,
        device_id: &str,
        name: Option<&str>,
        screens: &[Screen],
        now_unix: u64,
    ) {
        let screens = clean_screens(screens);
        let name = name.map(clean_name);
        match self.computers.iter_mut().find(|c| c.device_id == device_id) {
            Some(own) if own.this_computer => {}
            Some(known) => {
                let name = name.unwrap_or_else(|| known.name.clone());
                if known.name != name || known.screens != screens {
                    known.name = name;
                    known.screens = screens;
                    known.changed_unix = now_unix;
                }
            }
            None => self.computers.push(ComputerScreens {
                device_id: device_id.to_string(),
                name: name.unwrap_or_default(),
                screens,
                changed_unix: now_unix,
                this_computer: false,
            }),
        }
    }
}

/// The screens known. A missing or unreadable file is empty.
pub fn read(state_dir: &Path) -> ScreensFile {
    try_read(state_dir).unwrap_or_default()
}

/// [`read`], but a file that is there and does not read is an error.
pub fn try_read(state_dir: &Path) -> io::Result<ScreensFile> {
    match std::fs::read_to_string(state_dir.join(SHARED_SCREENS_FILE)) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ScreensFile::default()),
        Err(e) => Err(e),
    }
}

/// The server's writes, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

/// Change the file under `change` (the server's side), written whole and
/// owner-only, and only when it changed. A file that does not read is
/// started again: it holds nothing that is not heard again.
pub fn update(state_dir: &Path, change: impl FnOnce(&mut ScreensFile)) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = state_dir.join(SHARED_SCREENS_FILE);
    let mut file = try_read(state_dir).unwrap_or_default();
    let before = file.clone();
    change(&mut file);
    if file == before && path.exists() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn what_a_peer_says_is_clipped() {
        let many = vec![screen(0, 0, 10, 10); 500];
        assert_eq!(clean_screens(&many).len(), MAX_SCREENS);
        // No area, or a far edge past i32: dropped.
        let odd = [
            screen(0, 0, 0, 10),
            screen(0, 0, 10, 0),
            screen(i32::MAX, 0, 1, 1),
            screen(0, i32::MAX - 5, 1, 10),
            screen(0, 0, u32::MAX, 1),
            screen(-10, -10, 10, 10),
        ];
        assert_eq!(clean_screens(&odd), vec![screen(-10, -10, 10, 10)]);

        let long = "\u{4e2d}".repeat(200);
        let name = clean_name(&long);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.chars().all(|c| c == '\u{4e2d}'));
        assert_eq!(clean_name("Desk\n\u{7}top"), "Desktop");
    }

    #[test]
    fn this_computer_stays_first() {
        let mut file = ScreensFile::default();
        file.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 5);
        file.set_own("me", "Desk", &[screen(0, 0, 1920, 1080)], 6);
        assert_eq!(
            file.this_computer().map(|c| c.device_id.as_str()),
            Some("me")
        );
        assert_eq!(file.computers.len(), 2);
        file.heard("other", None, &[], 7);
        file.set_own("me", "Desk", &[screen(0, 0, 2560, 1440)], 8);
        let ids: Vec<&str> = file
            .computers
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(ids, ["me", "peer", "other"]);
        assert_eq!(file.computers[0].changed_unix, 8);
        // A peer claiming this computer's id changes nothing.
        file.heard("me", Some("Impostor"), &[], 9);
        assert_eq!(file.computers[0].name, "Desk");
    }

    #[test]
    fn a_peer_is_changed_in_place_and_keeps_its_name() {
        let mut file = ScreensFile::default();
        file.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 5);
        // The same again is not a change.
        file.heard("peer", None, &[screen(0, 0, 1600, 1000)], 6);
        assert_eq!(file.get("peer").unwrap().changed_unix, 5);
        // New screens without a name keep the name it had.
        file.heard("peer", None, &[screen(0, 0, 1280, 800)], 7);
        let peer = file.get("peer").unwrap();
        assert_eq!(peer.name, "Laptop");
        assert_eq!(peer.screens, vec![screen(0, 0, 1280, 800)]);
        assert_eq!(peer.changed_unix, 7);
    }

    #[test]
    fn written_whole_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pd-shared-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read(&dir), ScreensFile::default());
        update(&dir, |f| {
            f.set_own("me", "Desk", &[screen(0, 0, 1920, 1080)], 1)
        })
        .unwrap();
        update(&dir, |f| {
            f.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 2)
        })
        .unwrap();
        let back = read(&dir);
        assert_eq!(back.computers.len(), 2);
        assert!(back.computers[0].this_computer);
        // A file that does not read is started again.
        std::fs::write(dir.join(SHARED_SCREENS_FILE), "{not json").unwrap();
        update(&dir, |f| f.set_own("me", "Desk", &[], 3)).unwrap();
        assert_eq!(read(&dir).computers.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
