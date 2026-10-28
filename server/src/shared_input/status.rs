//! How the shared keyboard and mouse stand now, for the desktop app's
//! Keyboard and mouse page (`shared_input_status.json`): where this
//! computer's keyboard and mouse are, and each paired computer's link,
//! including whether it refused to be driven. Its pure part
//! ([`link_word`], [`status_from`], [`RefusalLog`]) has no I/O;
//! [`write_if_changed`] keeps the file, and the computers paired now
//! ([`paired_computers`]) keep `shared_screens.json` to themselves.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use pocketdesk_core::local::shared_input::{
    self, LINK_AWAY, LINK_DIRECT, LINK_RELAYED, REFUSED_NOT_ALLOWED, REFUSED_SHARING_OFF,
    StatusFile,
};
use pocketdesk_core::protocol::kdeconnect::TrustStore;

use super::link;
use super::wire::REFUSE_SHARING_OFF;

/// The computer this one's keyboard and mouse are on now (the driver's
/// word); empty while they are here.
static KEYBOARD_ON: Mutex<String> = Mutex::new(String::new());

/// Note where this computer's keyboard and mouse are: on `device_id`, or
/// here with `None`.
pub fn set_keyboard_on(device_id: Option<&str>) {
    let mut on = KEYBOARD_ON.lock().unwrap_or_else(|e| e.into_inner());
    if on.as_str() != device_id.unwrap_or_default() {
        *on = device_id.unwrap_or_default().to_string();
    }
}

fn keyboard_on() -> String {
    KEYBOARD_ON
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// What one computer's link is, in the file's words: none is away; a
/// computer refusing to be driven says why (`refused`, as
/// [`Msg::Refuse`](super::wire::Msg::Refuse) gave it); else direct or
/// relayed (`direct`, whether it is direct now).
pub fn link_word(direct: Option<bool>, refused: Option<u8>) -> &'static str {
    match (direct, refused) {
        (None, _) => LINK_AWAY,
        (Some(_), Some(REFUSE_SHARING_OFF)) => REFUSED_SHARING_OFF,
        (Some(_), Some(_)) => REFUSED_NOT_ALLOWED,
        (Some(true), None) => LINK_DIRECT,
        (Some(false), None) => LINK_RELAYED,
    }
}

/// The status of the `paired` computers: `links`, whether each linked one
/// is direct; `refused`, the reason each refusing one gave; `answered`, the
/// ones that sent their screens (they let this computer drive them).
pub fn status_from(
    paired: &BTreeSet<String>,
    keyboard_on: String,
    links: &HashMap<String, bool>,
    refused: &HashMap<String, u8>,
    answered: &BTreeSet<String>,
) -> StatusFile {
    let link: BTreeMap<String, String> = paired
        .iter()
        .map(|id| {
            let word = link_word(links.get(id).copied(), refused.get(id).copied());
            (id.clone(), word.to_string())
        })
        .collect();
    let answered = paired
        .iter()
        .filter(|id| {
            answered.contains(*id) && links.contains_key(*id) && !refused.contains_key(*id)
        })
        .cloned()
        .collect();
    StatusFile {
        keyboard_on,
        link,
        answered,
    }
}

/// The status now, of the `paired` computers, from what the links know.
pub fn now(paired: &BTreeSet<String>) -> StatusFile {
    let links: HashMap<String, bool> = link::linked()
        .into_iter()
        .filter_map(|id| {
            let direct = link::is_direct(&link::connection(&id)?);
            Some((id, direct))
        })
        .collect();
    let refused = link::refusals()
        .into_iter()
        .map(|(id, (reason, _))| (id, reason))
        .collect();
    let answered = link::screens_heard().into_keys().collect();
    status_from(paired, keyboard_on(), &links, &refused, &answered)
}

/// The owner's other computers paired now, either way: those that paired
/// with this one (the trust store's computers) and those this one paired
/// with (`computers.json`). `None` when either could not be read (a file
/// being replaced, or one that does not read): no reading, never "none
/// paired". Blocking file work.
pub fn paired_computers(state_dir: &Path) -> Option<BTreeSet<String>> {
    let trust = crate::state::trust_store_path().ok();
    paired_at(state_dir, trust.as_deref())
}

/// [`paired_computers`] with the trust store at `trust` (`None`: it could
/// not even be found).
pub fn paired_at(state_dir: &Path, trust: Option<&Path>) -> Option<BTreeSet<String>> {
    let computers = pocketdesk_core::local::computers::try_read(state_dir)
        .inspect_err(|e| tracing::debug!("Shared input: the computers did not read: {e}"))
        .ok()?;
    let store = TrustStore::read(trust?)
        .inspect_err(|e| tracing::debug!("Shared input: the trust store did not read: {e:#}"))
        .ok()?;
    let mut paired: BTreeSet<String> = computers
        .computers
        .into_iter()
        .map(|c| c.device_id)
        .collect();
    paired.extend(
        store
            .paired_devices()
            .into_iter()
            .filter(|id| store.is_computer(id)),
    );
    Some(paired)
}

/// Write `status` to `shared_input_status.json` unless it is what was last
/// written (`last`), and, with sharing on and the file there, keep
/// `shared_screens.json` to the `paired` computers. With no reading of
/// which are paired (`None`) nothing is pruned and each computer's link is
/// left as last written. Blocking file work.
pub fn write_if_changed(
    state_dir: &Path,
    mut status: StatusFile,
    paired: Option<&BTreeSet<String>>,
    sharing_on: bool,
    last: &mut Option<StatusFile>,
) {
    match paired {
        Some(paired) => {
            let there = state_dir
                .join(pocketdesk_core::local::paths::SHARED_SCREENS_FILE)
                .exists();
            if sharing_on
                && there
                && let Err(e) = shared_input::update(state_dir, |file| file.keep_only(paired))
            {
                tracing::debug!("Shared input: the screens were not pruned: {e}");
            }
        }
        None => {
            if let Some(last) = last.as_ref() {
                status.link = last.link.clone();
                status.answered = last.answered.clone();
            }
        }
    }
    if last.as_ref() == Some(&status) {
        return;
    }
    match shared_input::write_status(state_dir, &status) {
        Ok(()) => *last = Some(status),
        Err(e) => tracing::debug!("Shared input: the status was not written: {e}"),
    }
}

/// The first wait before a computer that refused is greeted again.
const REGREET_FIRST: Duration = Duration::from_secs(5);

/// The longest wait before it is.
const REGREET_MOST: Duration = Duration::from_secs(60);

/// The refusals said, from or to each computer: the last reason and how
/// many times in a row, so the log says each once (until the reason
/// changes, the computer takes input or its link goes) and a computer that
/// keeps refusing is greeted ever less often.
#[derive(Debug, Default)]
pub struct RefusalLog {
    last: HashMap<String, (u8, u32)>,
}

impl RefusalLog {
    /// Note a refusal from or to `id` for `reason`: whether it is news,
    /// to be said at info (the first, or another reason).
    pub fn note(&mut self, id: &str, reason: u8) -> bool {
        match self.last.get_mut(id) {
            Some((last, times)) if *last == reason => {
                *times = times.saturating_add(1);
                false
            }
            _ => {
                self.last.insert(id.to_string(), (reason, 1));
                true
            }
        }
    }

    /// How long before `id` is greeted again: 5 s, doubled for each
    /// refusal in a row for the same reason, at most a minute.
    pub fn backoff(&self, id: &str) -> Duration {
        let times = self.last.get(id).map_or(1, |(_, times)| *times);
        let doublings = times.saturating_sub(1).min(8);
        (REGREET_FIRST * 2u32.pow(doublings)).min(REGREET_MOST)
    }

    /// `id` took input (it answered with its screens, or it may drive this
    /// one now): its next refusal is news again.
    pub fn clear(&mut self, id: &str) {
        self.last.remove(id);
    }

    /// Forget the computers no longer `linked`.
    pub fn keep_linked(&mut self, linked: &[String]) {
        self.last.retain(|id, _| linked.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_input::wire::REFUSE_NOT_ALLOWED;

    #[test]
    fn each_link_says_how_it_stands() {
        assert_eq!(link_word(None, None), LINK_AWAY);
        // A link gone is away, whatever was refused on it.
        assert_eq!(link_word(None, Some(REFUSE_SHARING_OFF)), LINK_AWAY);
        assert_eq!(link_word(Some(true), None), LINK_DIRECT);
        assert_eq!(link_word(Some(false), None), LINK_RELAYED);
        assert_eq!(
            link_word(Some(true), Some(REFUSE_SHARING_OFF)),
            REFUSED_SHARING_OFF
        );
        assert_eq!(
            link_word(Some(true), Some(REFUSE_NOT_ALLOWED)),
            REFUSED_NOT_ALLOWED
        );
        // A reason this version does not know is still a refusal.
        assert_eq!(link_word(Some(true), Some(200)), REFUSED_NOT_ALLOWED);
    }

    #[test]
    fn the_status_lists_paired_computers_only() {
        let paired = BTreeSet::from(["A".to_string(), "B".to_string(), "C".to_string()]);
        let links = HashMap::from([
            ("A".to_string(), true),
            ("B".to_string(), true),
            ("gone".to_string(), true),
        ]);
        let refused = HashMap::from([("B".to_string(), REFUSE_NOT_ALLOWED)]);
        let answered = BTreeSet::from(["A".to_string(), "C".to_string(), "gone".to_string()]);
        let status = status_from(&paired, "A".to_string(), &links, &refused, &answered);
        assert_eq!(status.keyboard_on, "A");
        assert_eq!(
            status.link,
            BTreeMap::from([
                ("A".to_string(), LINK_DIRECT.to_string()),
                ("B".to_string(), REFUSED_NOT_ALLOWED.to_string()),
                ("C".to_string(), LINK_AWAY.to_string()),
            ])
        );
        // C answered on a link that has gone since: not now.
        assert_eq!(status.answered, BTreeSet::from(["A".to_string()]));
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-si-status-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_failed_read_prunes_nothing() {
        use pocketdesk_core::local::paths::{COMPUTERS_FILE, SHARED_SCREENS_FILE};
        use pocketdesk_core::local::shared_input::Screen;
        let dir = temp("read");
        let trust = dir.join("trusted_devices.json");
        let screen = Screen {
            x: 0,
            y: 0,
            width: 1600,
            height: 1000,
        };
        shared_input::update(&dir, |f| {
            f.set_own("me", "Desk", &[screen], 1);
            f.heard("peer", Some("Laptop"), &[screen], 1);
        })
        .unwrap();
        // No files at all: nothing is paired, and that is a reading.
        assert_eq!(paired_at(&dir, Some(&trust)), Some(BTreeSet::new()));
        // A computers file that does not read, or no trust store to read:
        // no reading, so nothing is pruned and the links stay as they were.
        std::fs::write(dir.join(COMPUTERS_FILE), "{not json").unwrap();
        let paired = paired_at(&dir, Some(&trust));
        assert_eq!(paired, None);
        assert_eq!(paired_at(&dir, None), None);
        let before = StatusFile {
            keyboard_on: String::new(),
            link: BTreeMap::from([("peer".to_string(), LINK_DIRECT.to_string())]),
            answered: BTreeSet::new(),
        };
        let mut last = Some(before.clone());
        let now = StatusFile {
            keyboard_on: "peer".to_string(),
            ..StatusFile::default()
        };
        write_if_changed(&dir, now, paired.as_ref(), true, &mut last);
        let screens = shared_input::read(&dir);
        assert!(screens.get("peer").is_some(), "kept through a bad read");
        let written = shared_input::read_status(&dir);
        assert_eq!(written.link, before.link, "the links as they were");
        assert_eq!(written.keyboard_on, "peer");
        // A good reading prunes; with sharing off the screens are left be.
        std::fs::remove_file(dir.join(COMPUTERS_FILE)).unwrap();
        let none = paired_at(&dir, Some(&trust));
        write_if_changed(&dir, StatusFile::default(), none.as_ref(), false, &mut last);
        assert!(
            shared_input::read(&dir).get("peer").is_some(),
            "sharing off"
        );
        write_if_changed(&dir, StatusFile::default(), none.as_ref(), true, &mut last);
        assert!(shared_input::read(&dir).get("peer").is_none(), "unpaired");
        // With sharing off and no screens file, none is made.
        std::fs::remove_file(dir.join(SHARED_SCREENS_FILE)).unwrap();
        write_if_changed(&dir, StatusFile::default(), none.as_ref(), false, &mut last);
        assert!(!dir.join(SHARED_SCREENS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refusal_is_said_once_and_tried_ever_less_often() {
        let mut log = RefusalLog::default();
        assert!(log.note("L", REFUSE_NOT_ALLOWED), "the first is news");
        assert_eq!(log.backoff("L"), Duration::from_secs(5));
        assert!(!log.note("L", REFUSE_NOT_ALLOWED), "the same again is not");
        assert_eq!(log.backoff("L"), Duration::from_secs(10));
        log.note("L", REFUSE_NOT_ALLOWED);
        assert_eq!(log.backoff("L"), Duration::from_secs(20));
        log.note("L", REFUSE_NOT_ALLOWED);
        log.note("L", REFUSE_NOT_ALLOWED);
        assert_eq!(
            log.backoff("L"),
            Duration::from_secs(60),
            "at most a minute"
        );
        for _ in 0..100 {
            log.note("L", REFUSE_NOT_ALLOWED);
        }
        assert_eq!(log.backoff("L"), Duration::from_secs(60));
        // Another reason is news, and starts again.
        assert!(log.note("L", REFUSE_SHARING_OFF));
        assert_eq!(log.backoff("L"), Duration::from_secs(5));
        // It took input: the next is news again.
        log.clear("L");
        assert!(log.note("L", REFUSE_SHARING_OFF));
        // Its link went: forgotten.
        log.note("M", REFUSE_SHARING_OFF);
        log.keep_linked(&["M".to_string()]);
        assert!(log.note("L", REFUSE_SHARING_OFF));
        assert!(!log.note("M", REFUSE_SHARING_OFF));
        // One never refused waits the first time only.
        assert_eq!(log.backoff("N"), Duration::from_secs(5));
    }

    #[test]
    fn the_keyboard_is_here_until_the_driver_says_otherwise() {
        set_keyboard_on(Some("L"));
        assert_eq!(keyboard_on(), "L");
        set_keyboard_on(None);
        assert_eq!(keyboard_on(), "");
    }
}
