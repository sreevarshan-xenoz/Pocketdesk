//! The name each computer gives itself in its identity (U3), by the
//! address this phone dialed it at, for the app to keep and show.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn names() -> &'static Mutex<HashMap<String, String>> {
    static NAMES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

/// Keep `name` (already cleaned by the core's handshake) for `address`; no
/// name keeps the last one heard.
pub(crate) fn note(address: &str, name: Option<String>) {
    if let Some(name) = name {
        names()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(address.to_string(), name);
    }
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

    #[test]
    fn a_name_is_kept_per_address_and_none_keeps_the_last() {
        note("192.0.2.40", Some("Study PC".to_string()));
        note("192.0.2.40", None);
        note("192.0.2.41", Some("Laptop".to_string()));
        assert_eq!(name("192.0.2.40").as_deref(), Some("Study PC"));
        assert_eq!(name("192.0.2.41").as_deref(), Some("Laptop"));
        assert_eq!(name("192.0.2.42"), None);
    }
}
