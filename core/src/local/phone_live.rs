//! The phones' ongoing updates shown here (N4), kept by the server in
//! `<state>/phone_live.json` for the desktop app's Notifications page: one
//! card per notification, replaced by each update, gone with its cancel or
//! its phone's unpairing, with pairing required (AY5).

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::PHONE_LIVE_FILE;
use crate::protocol::live::Live;

/// The most cards kept, over all phones.
pub const MAX_CARDS: usize = 20;

/// A bar not heard of this long is taken for gone (N4 review: a lost
/// cancel left it for good); a timer may run longer unchanged.
pub const STALE_SECS: u64 = 30 * 60;

impl LiveCard {
    /// Whether it is still to be shown at `now_unix`.
    pub fn current(&self, now_unix: u64) -> bool {
        self.live.chronometer.is_some() || now_unix.saturating_sub(self.updated_unix) < STALE_SECS
    }
}

/// One ongoing update as last heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveCard {
    pub device_id: String,
    /// The notification's phone-scoped tag on this computer.
    pub tag: String,
    pub app: String,
    pub title: String,
    pub text: String,
    pub live: Live,
    pub updated_unix: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct CardsFile {
    cards: Vec<LiveCard>,
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The cards kept, newest last; none when there is no file or it does not read.
pub fn read(state_dir: &Path) -> Vec<LiveCard> {
    std::fs::read_to_string(state_dir.join(PHONE_LIVE_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<CardsFile>(&raw).ok())
        .map(|f| f.cards)
        .unwrap_or_default()
}

fn write(state_dir: &Path, cards: Vec<LiveCard>) -> io::Result<()> {
    let json = serde_json::to_string(&CardsFile { cards }).map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_LIVE_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    // What a phone's apps are doing: owner-only.
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// Keep `card` in place of its earlier one; whether it is new (its first
/// update). Past [`MAX_CARDS`] the oldest go.
pub fn put(state_dir: &Path, card: LiveCard) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut cards = read(state_dir);
    let before = cards.len();
    cards.retain(|c| !(c.device_id == card.device_id && c.tag == card.tag));
    let new = cards.len() == before;
    let now = card.updated_unix;
    cards.retain(|c| c.current(now));
    cards.push(card);
    let excess = cards.len().saturating_sub(MAX_CARDS);
    cards.drain(..excess);
    write(state_dir, cards)?;
    Ok(new)
}

/// Drop `device_id`'s card `tag` (its cancel); whether there was one.
pub fn remove(state_dir: &Path, device_id: &str, tag: &str) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut cards = read(state_dir);
    let before = cards.len();
    cards.retain(|c| !(c.device_id == device_id && c.tag == tag));
    if cards.len() == before {
        return Ok(false);
    }
    write(state_dir, cards)?;
    Ok(true)
}

/// Keep only the cards of phones `connected` names; whether any went.
pub fn keep_only(state_dir: &Path, connected: &[String]) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut cards = read(state_dir);
    let before = cards.len();
    cards.retain(|c| connected.contains(&c.device_id));
    if cards.len() == before {
        return Ok(false);
    }
    write(state_dir, cards)?;
    Ok(true)
}

/// Forget every card: what played in an earlier run is not going on now.
pub fn clear(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(PHONE_LIVE_FILE));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(device: &str, tag: &str, progress: u64) -> LiveCard {
        LiveCard {
            device_id: device.into(),
            tag: tag.into(),
            app: "Files".into(),
            title: "Downloading".into(),
            text: "movie.mp4".into(),
            live: Live {
                progress,
                max: 100,
                indeterminate: false,
                chronometer: None,
                countdown: false,
                words: None,
            },
            updated_unix: 1,
        }
    }

    #[test]
    fn a_card_is_new_once_then_replaced_and_gone_with_its_cancel() {
        let dir = std::env::temp_dir().join(format!("pd-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(put(&dir, card("a", "t1", 10)).unwrap(), "new");
        assert!(!put(&dir, card("a", "t1", 50)).unwrap(), "an update");
        assert!(put(&dir, card("b", "t1", 10)).unwrap(), "another phone's");
        assert_eq!(read(&dir).len(), 2);
        assert_eq!(
            read(&dir)
                .iter()
                .find(|c| c.device_id == "a")
                .unwrap()
                .live
                .progress,
            50
        );
        assert!(remove(&dir, "a", "t1").unwrap());
        assert!(!remove(&dir, "a", "t1").unwrap());
        assert!(keep_only(&dir, &[]).unwrap());
        assert!(read(&dir).is_empty());
        for n in 0..MAX_CARDS + 3 {
            put(&dir, card("a", &format!("t{n}"), 1)).unwrap();
        }
        assert_eq!(read(&dir).len(), MAX_CARDS);
        clear(&dir);
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
