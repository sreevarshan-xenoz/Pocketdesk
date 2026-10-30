//! What each computer says of itself in its identity, by the address this
//! phone dialed it at: the name it gives itself (U3) and the Pocketdesk
//! release it runs (item 4), for the app to keep and show, and whether it
//! answers notifications that ask (C3). Heard
//! again at every handshake, so a computer replaced or changed at an address
//! is seen at its next connection.

use pocketdesk_core::protocol::connection::PeerClaims;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn names() -> &'static Mutex<HashMap<String, String>> {
    static NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

fn acks() -> &'static Mutex<HashMap<String, bool>> {
    static ACKS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    ACKS.get_or_init(Default::default)
}

fn versions() -> &'static Mutex<HashMap<String, String>> {
    static VERSIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    VERSIONS.get_or_init(Default::default)
}

fn lives() -> &'static Mutex<HashMap<String, bool>> {
    static LIVES: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    LIVES.get_or_init(Default::default)
}

/// Keep what the computer at `address` said (its name already cleaned by
/// the core's handshake); no name keeps the last one heard, and no identity
/// keeps the last word on answering.
pub(crate) fn note(address: &str, claims: PeerClaims) {
    if let Some(takes) = claims.takes_live() {
        lives()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(address.to_string(), takes);
    }
    if let Some(answers) = claims.acks_notifications() {
        acks()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(address.to_string(), answers);
    }
    if let Some(name) = claims.name {
        names()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(address.to_string(), name);
    }
    // An identity that says no release (an older computer, replaced at the
    // address) forgets the last one: an old release is not shown for it.
    let mut versions = versions().lock().unwrap_or_else(|e| e.into_inner());
    match claims.version {
        Some(version) => {
            versions.insert(address.to_string(), version);
        }
        None if claims.sends.is_some() => {
            versions.remove(address);
        }
        None => {}
    }
}

/// Forget what the computer at `address` said of itself, as the phone lets
/// it go (AZ3: removed or moved, its name stayed, and a computer saved
/// there later that gives none was saved under it for good, with its
/// answering flags).
pub(crate) fn forget(address: &str) {
    names()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(address);
    acks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(address);
    lives()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(address);
    versions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(address);
}

/// Whether the computer at `address` said, when last heard, that it
/// answers a phone notification that asks (C3).
pub(crate) fn acks_notifications(address: &str) -> bool {
    acks()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(address)
        .copied()
        .unwrap_or(false)
}

/// Whether the computer at `address` said, when last heard, that it shows
/// a phone's ongoing updates as one card (N4).
pub fn takes_live(address: &str) -> bool {
    lives()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(address)
        .copied()
        .unwrap_or(false)
}

/// The name the computer at `address` last gave itself, if any.
pub fn name(address: &str) -> Option<String> {
    names()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(address)
        .cloned()
}

/// The Pocketdesk release the computer at `address` said it runs when
/// last heard (item 4), if it said one.
pub fn version(address: &str) -> Option<String> {
    versions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(address)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: Option<&str>) -> PeerClaims {
        PeerClaims {
            name: name.map(str::to_string),
            sends: None,
            version: None,
        }
    }

    #[test]
    fn a_name_is_kept_per_address_and_none_keeps_the_last() {
        note("192.0.2.40", named(Some("Study PC")));
        note("192.0.2.40", named(None));
        note("192.0.2.41", named(Some("Laptop")));
        assert_eq!(name("192.0.2.40").as_deref(), Some("Study PC"));
        assert_eq!(name("192.0.2.41").as_deref(), Some("Laptop"));
        assert_eq!(name("192.0.2.42"), None);
    }

    /// C3: whether a computer answers follows what it said at its latest
    /// handshake; a handshake without an identity changes nothing.
    #[test]
    fn whether_a_computer_answers_follows_its_latest_identity() {
        let says = |answers: bool| PeerClaims {
            name: None,
            sends: Some(if answers {
                vec!["kdeconnect.notification.ack".to_string()]
            } else {
                vec![]
            }),
            version: None,
        };
        assert!(!acks_notifications("192.0.2.50"), "never heard");
        note("192.0.2.50", says(true));
        assert!(acks_notifications("192.0.2.50"));
        note("192.0.2.50", named(None));
        assert!(acks_notifications("192.0.2.50"), "no identity, no change");
        note("192.0.2.50", says(false));
        assert!(!acks_notifications("192.0.2.50"), "an older computer now");
    }

    /// AZ3: a computer let go takes what it said along, so another saved
    /// at its address later that gives no name is never called by its.
    #[test]
    fn a_computer_let_go_takes_what_it_said_along() {
        let address = "192.0.2.70";
        note(
            address,
            PeerClaims {
                name: Some("Study PC".to_string()),
                sends: Some(vec![
                    "kdeconnect.notification.ack".to_string(),
                    "kdeconnect.pocketdesk.notification_open".to_string(),
                ]),
                version: Some("0.8.1".to_string()),
            },
        );
        assert!(acks_notifications(address) && takes_live(address));
        forget(address);
        assert_eq!(name(address), None);
        assert!(!acks_notifications(address));
        assert!(!takes_live(address));
        assert_eq!(version(address), None, "its release goes too");
        note(address, named(None));
        assert_eq!(name(address), None, "another computer there gives none");
    }

    /// N4: a computer that sends N3's open takes live cards.
    #[test]
    fn a_computer_of_this_cycle_takes_live_cards() {
        let says = |sends: Vec<&str>| PeerClaims {
            name: None,
            sends: Some(sends.into_iter().map(str::to_string).collect()),
            version: None,
        };
        assert!(!takes_live("192.0.2.60"));
        note(
            "192.0.2.60",
            says(vec!["kdeconnect.pocketdesk.notification_open"]),
        );
        assert!(takes_live("192.0.2.60"));
        note("192.0.2.60", says(vec!["kdeconnect.notification.ack"]));
        assert!(!takes_live("192.0.2.60"), "an older computer");
    }

    /// Item 4: the release a computer says is kept per address; a handshake
    /// with no identity changes nothing, and an identity that says none (an
    /// older computer now at the address) forgets it.
    #[test]
    fn a_computers_release_follows_its_latest_identity() {
        let address = "192.0.2.90";
        let says = |release: Option<&str>| PeerClaims {
            name: None,
            sends: Some(Vec::new()),
            version: release.map(str::to_string),
        };
        assert_eq!(version(address), None, "never heard");
        note(address, says(Some("0.8.1")));
        assert_eq!(version(address).as_deref(), Some("0.8.1"));
        note(address, named(None));
        assert_eq!(version(address).as_deref(), Some("0.8.1"), "no identity");
        note(address, says(None));
        assert_eq!(version(address), None, "an older computer now");
    }
}
