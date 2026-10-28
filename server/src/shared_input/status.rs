//! How the shared keyboard and mouse stand now, for the desktop app's
//! Keyboard and mouse page (`shared_input_status.json`): where this
//! computer's keyboard and mouse are, and each paired computer's link,
//! including whether it refused to be driven. Its pure part
//! ([`link_word`], [`status_from`]) has no I/O; [`write_if_changed`] keeps
//! the file, and the computers paired now ([`paired_computers`]) keep
//! `shared_screens.json` to themselves.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;

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
/// with (`computers.json`). Blocking file work.
pub fn paired_computers(state_dir: &Path) -> BTreeSet<String> {
    let mut paired: BTreeSet<String> = pocketdesk_core::local::computers::read(state_dir)
        .computers
        .into_iter()
        .map(|c| c.device_id)
        .collect();
    if let Ok(path) = crate::state::trust_store_path()
        && let Ok(store) = TrustStore::read(path)
    {
        paired.extend(
            store
                .paired_devices()
                .into_iter()
                .filter(|id| store.is_computer(id)),
        );
    }
    paired
}

/// Write `status` to `shared_input_status.json` unless it is what was last
/// written (`last`), and keep `shared_screens.json` to the `paired`
/// computers. Blocking file work.
pub fn write_if_changed(
    state_dir: &Path,
    status: StatusFile,
    paired: &BTreeSet<String>,
    last: &mut Option<StatusFile>,
) {
    if let Err(e) = shared_input::update(state_dir, |file| file.keep_only(paired)) {
        tracing::debug!("Shared input: the screens were not pruned: {e}");
    }
    if last.as_ref() == Some(&status) {
        return;
    }
    match shared_input::write_status(state_dir, &status) {
        Ok(()) => *last = Some(status),
        Err(e) => tracing::debug!("Shared input: the status was not written: {e}"),
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

    #[test]
    fn the_keyboard_is_here_until_the_driver_says_otherwise() {
        set_keyboard_on(Some("L"));
        assert_eq!(keyboard_on(), "L");
        set_keyboard_on(None);
        assert_eq!(keyboard_on(), "");
    }
}
