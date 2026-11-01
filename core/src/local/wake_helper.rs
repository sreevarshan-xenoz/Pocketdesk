//! The phone this computer holds as its wake helper: kept in
//! `<state>/wake_helper.json` for the server, which tells the helper what
//! to do and the owner phones where it is, and for the desktop app, which
//! shows it and lets the owner remove it. Nothing here is a secret (a key
//! the phone itself announced, and the relays it is reached through); the
//! file is owner-only all the same, and no log or event names a key.
//!
//! One helper at most. When the owner removes it here, its device id is
//! remembered as refused: the phone keeps offering itself every few
//! minutes, and only its owner turning "Wake helper" off and on again on
//! the phone (an offer marked `fresh`) undoes the removal.

use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::WAKE_HELPER_FILE;
use crate::protocol::kdeconnect::StoreLock;
use crate::protocol::wake_helper::Offer;

/// The most removed phones remembered.
const MAX_REFUSED: usize = 16;

/// The phone held as helper, as it offered itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Helper {
    /// The phone's device id here.
    pub device_id: String,
    /// The helper endpoint's key (not the phone's own device key).
    pub iroh: String,
    #[serde(default)]
    pub relays: Vec<String>,
    /// When it last offered itself (unix seconds).
    #[serde(default)]
    pub seen: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeHelperFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<Helper>,
    /// Phones the owner removed as helper, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<String>,
}

/// What came of an offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// The phone is the helper now (or stays it).
    Helper,
    /// The phone stopped being the helper (it offered `on: false`).
    Stopped,
    /// The owner removed this phone: not taken.
    Refused,
}

/// The file; a missing one is none, one that does not read is none too (it
/// holds nothing that must not be forgotten: the helper offers again).
pub fn read(state_dir: &Path) -> WakeHelperFile {
    std::fs::read_to_string(state_dir.join(WAKE_HELPER_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write(state_dir: &Path, file: &WakeHelperFile) -> io::Result<()> {
    let json = serde_json::to_string_pretty(file).map_err(io::Error::other)?;
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(WAKE_HELPER_FILE);
    let tmp = state_dir.join(format!(
        "{WAKE_HELPER_FILE}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut out = std::fs::File::create(&tmp)?;
    out.write_all(json.as_bytes())?;
    out.sync_all()?;
    drop(out);
    crate::atomic_file::replace(&tmp, &path)
}

/// Change the file under its lock: `change` gets what it holds and says
/// whether to write it back.
fn update<R>(
    state_dir: &Path,
    change: impl FnOnce(&mut WakeHelperFile) -> (R, bool),
) -> io::Result<R> {
    let _lock = StoreLock::acquire(&state_dir.join(WAKE_HELPER_FILE)).map_err(io::Error::other)?;
    let mut file = read(state_dir);
    let (result, write_back) = change(&mut file);
    if write_back {
        write(state_dir, &file)?;
    }
    Ok(result)
}

/// Take `device_id`'s offer at `now`. The caller has already checked it is
/// an owner phone; the key is the one the offer named.
pub fn take_offer(state_dir: &Path, device_id: &str, offer: &Offer, now: u64) -> io::Result<Taken> {
    update(state_dir, |file| {
        let is_helper = file
            .helper
            .as_ref()
            .is_some_and(|h| h.device_id == device_id);
        if !offer.on {
            if is_helper {
                file.helper = None;
                return (Taken::Stopped, true);
            }
            return (Taken::Stopped, false);
        }
        let refused = file.refused.iter().any(|d| d == device_id);
        if refused && !offer.fresh {
            return (Taken::Refused, false);
        }
        file.refused.retain(|d| d != device_id);
        let held = Helper {
            device_id: device_id.to_string(),
            iroh: offer.iroh.clone(),
            relays: offer.relays.clone(),
            seen: now,
        };
        // A newer phone replaces the helper: one at most.
        let unchanged = file.helper.as_ref().is_some_and(|h| {
            h.device_id == held.device_id && h.iroh == held.iroh && h.relays == held.relays
        });
        // A repeat of the same offer only moves `seen` forward every so often,
        // so the file is not rewritten each time.
        let fresh_enough = file
            .helper
            .as_ref()
            .is_some_and(|h| now.saturating_sub(h.seen) < 3600);
        if unchanged && fresh_enough && !refused {
            return (Taken::Helper, false);
        }
        file.helper = Some(held);
        (Taken::Helper, true)
    })
}

/// The owner removed the helper here: it is dropped and remembered as
/// refused. Returns the phone that was removed, if any.
pub fn remove(state_dir: &Path) -> io::Result<Option<String>> {
    update(state_dir, |file| {
        let Some(helper) = file.helper.take() else {
            return (None, false);
        };
        file.refused.retain(|d| *d != helper.device_id);
        file.refused.push(helper.device_id.clone());
        while file.refused.len() > MAX_REFUSED {
            file.refused.remove(0);
        }
        (Some(helper.device_id), true)
    })
}

/// `device_id` is no longer paired here (or no longer an owner phone): it is
/// no helper, and no removal of it is remembered.
pub fn forget_device(state_dir: &Path, device_id: &str) -> io::Result<bool> {
    update(state_dir, |file| {
        let was_helper = file
            .helper
            .as_ref()
            .is_some_and(|h| h.device_id == device_id);
        if was_helper {
            file.helper = None;
        }
        let had = file.refused.len();
        file.refused.retain(|d| d != device_id);
        let changed = was_helper || file.refused.len() != had;
        (was_helper, changed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-wakehelper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn offer(key: &str, fresh: bool) -> Offer {
        Offer {
            on: true,
            fresh,
            iroh: key.to_string(),
            relays: vec!["https://relay.example.com/".to_string()],
        }
    }

    #[test]
    fn an_offer_makes_the_phone_the_helper_and_stopping_ends_it() {
        let dir = dir("offer");
        assert_eq!(read(&dir), WakeHelperFile::default());
        assert_eq!(
            take_offer(&dir, "phone-1", &offer(KEY, true), 100).unwrap(),
            Taken::Helper
        );
        let held = read(&dir).helper.unwrap();
        assert_eq!(held.device_id, "phone-1");
        assert_eq!(held.iroh, KEY);
        // Another phone's off offer leaves it alone.
        let off = Offer {
            on: false,
            fresh: false,
            iroh: String::new(),
            relays: vec![],
        };
        assert_eq!(
            take_offer(&dir, "phone-2", &off, 101).unwrap(),
            Taken::Stopped
        );
        assert!(read(&dir).helper.is_some());
        take_offer(&dir, "phone-1", &off, 102).unwrap();
        assert!(read(&dir).helper.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_phone_replaces_the_helper_and_there_is_one() {
        let dir = dir("one");
        take_offer(&dir, "phone-1", &offer(KEY, true), 100).unwrap();
        take_offer(&dir, "phone-2", &offer(OTHER, true), 101).unwrap();
        assert_eq!(read(&dir).helper.unwrap().device_id, "phone-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_removed_phone_stays_removed_until_its_owner_turns_it_on_again() {
        let dir = dir("refused");
        take_offer(&dir, "phone-1", &offer(KEY, true), 100).unwrap();
        assert_eq!(remove(&dir).unwrap().as_deref(), Some("phone-1"));
        assert!(read(&dir).helper.is_none());
        // Its next periodic offer is refused...
        assert_eq!(
            take_offer(&dir, "phone-1", &offer(KEY, false), 200).unwrap(),
            Taken::Refused
        );
        assert!(read(&dir).helper.is_none());
        // ...and a fresh one, from the owner's switch, is taken.
        assert_eq!(
            take_offer(&dir, "phone-1", &offer(KEY, true), 300).unwrap(),
            Taken::Helper
        );
        assert!(read(&dir).refused.is_empty());
        assert_eq!(remove(&dir).unwrap().as_deref(), Some("phone-1"));
        assert_eq!(remove(&dir).unwrap(), None, "nothing left to remove");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unpaired_phone_is_no_helper_and_leaves_no_removal_behind() {
        let dir = dir("forget");
        take_offer(&dir, "phone-1", &offer(KEY, true), 100).unwrap();
        assert!(forget_device(&dir, "phone-1").unwrap());
        assert!(read(&dir).helper.is_none());
        take_offer(&dir, "phone-3", &offer(KEY, true), 100).unwrap();
        remove(&dir).unwrap();
        assert!(!forget_device(&dir, "phone-3").unwrap(), "no longer helper");
        assert!(read(&dir).refused.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removals_remembered_are_few() {
        let dir = dir("few");
        for i in 0..(MAX_REFUSED + 4) {
            let phone = format!("phone-{i}");
            take_offer(&dir, &phone, &offer(KEY, true), 100).unwrap();
            remove(&dir).unwrap();
        }
        let refused = read(&dir).refused;
        assert_eq!(refused.len(), MAX_REFUSED);
        assert_eq!(
            refused.last().unwrap(),
            &format!("phone-{}", MAX_REFUSED + 3)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_does_not_read_holds_nothing() {
        let dir = dir("garbage");
        std::fs::write(dir.join(WAKE_HELPER_FILE), "not json").unwrap();
        assert_eq!(read(&dir), WakeHelperFile::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
