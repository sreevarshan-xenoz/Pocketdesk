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

/// Keep what the computer at `address` said (its name already cleaned by
/// the core's handshake); no name keeps the last one heard, and no identity
/// keeps the last word on answering.
pub(crate) fn note(address: &str, claims: PeerClaims) {
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
}
