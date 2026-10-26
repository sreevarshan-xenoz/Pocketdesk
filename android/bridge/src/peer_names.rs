//! What each computer says of itself in its identity, by the address this
//! phone dialed it at: the name it gives itself (U3), for the app to keep
//! and show, and whether it answers notifications that ask (C3). Heard
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

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: Option<&str>) -> PeerClaims {
        PeerClaims {
            name: name.map(str::to_string),
            sends: None,
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
            },
        );
        assert!(acks_notifications(address) && takes_live(address));
        forget(address);
        assert_eq!(name(address), None);
        assert!(!acks_notifications(address));
        assert!(!takes_live(address));
        note(address, named(None));
        assert_eq!(name(address), None, "another computer there gives none");
    }

    /// N4: a computer that sends N3's open takes live cards.
    #[test]
    fn a_computer_of_this_cycle_takes_live_cards() {
        let says = |sends: Vec<&str>| PeerClaims {
            name: None,
            sends: Some(sends.into_iter().map(str::to_string).collect()),
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
}
