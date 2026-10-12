//! The key codes the Android client is able to put on the wire.
//!
//! The wire carries Linux evdev `KEY_*` codes (`InputPacket::KeyEvent`), and two
//! tables on either end have to agree about which ones exist: the client's
//! Android→evdev mapping, and the server's evdev→backend mapping. Neither end
//! can import the other's table — the client's is Android-only and lives in the
//! bridge, the server's is compositor-only and lives in `server` — so an
//! unannounced key added on one side degrades on the other in silence, which is
//! exactly how roadmap 2057-2060 stayed alive.
//!
//! This set is the contract between them. The client asserts it never emits a
//! code outside the set, and the server asserts every code inside the set has a
//! mapping. A key that needs a new entry therefore fails a test on both ends
//! rather than reaching a user as a dead key.

/// Inclusive ranges of evdev `KEY_*` codes the Android client can emit, in
/// ascending order and non-overlapping.
///
/// `KEY_RESERVED` (0) is deliberately absent: the client emits it for an Android
/// keycode it does not know, and both ends treat 0 as "ignore".
///
/// Derived from `android_to_evdev_keycode` in `android/bridge/src/api.rs`; the
/// bridge's own test walks every Android keycode and fails if this list stops
/// covering what it produces.
pub const PHONE_EVDEV_KEY_RANGES: &[(u16, u16)] = &[
    // Esc, 1-0, Minus, Equal, BackSpace, Tab, Q..P, [ ], Enter, Ctrl, A..L,
    // ; ' `, Shift, Backslash, Z..M through Comma/Dot/Slash, Shift, KP
    // Multiply, Alt, Space, Caps Lock, F1-F10, Num Lock
    (1, 69),
    (78, 78),   // KP Plus
    (87, 88),   // F11, F12
    (97, 97),   // Right Ctrl
    (99, 100),  // SysRq (the Print key), Right Alt
    (102, 111), // Home, Up, Page Up, Left, Right, End, Down, Page Down, Insert, Delete
    (114, 115), // Volume Down, Volume Up
    (125, 126), // Left Meta (Super), Right Meta (Super)
    (139, 139), // Menu (the application-keys key)
    (163, 165), // Media Next, Play/Pause, Previous
];

/// Whether the Android client can put this evdev code on the wire.
pub fn phone_can_emit(code: u16) -> bool {
    PHONE_EVDEV_KEY_RANGES
        .iter()
        .any(|&(start, end)| (start..=end).contains(&code))
}

/// Every code the client can emit, flattened — what a server-side table is
/// required to cover.
pub fn phone_emittable_codes() -> Vec<u16> {
    PHONE_EVDEV_KEY_RANGES
        .iter()
        .flat_map(|&(start, end)| start..=end)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_sorted_and_do_not_overlap() {
        // A list that overlaps or wanders backwards makes `contains`-style
        // reasoning about coverage impossible to read.
        let mut previous_end = None;
        for &(start, end) in PHONE_EVDEV_KEY_RANGES {
            assert!(start <= end, "empty range {start}..={end}");
            if let Some(previous_end) = previous_end {
                assert!(
                    start > previous_end + 1,
                    "range {start}..={end} touches or adjoins the one before it"
                );
            }
            previous_end = Some(end);
        }
    }

    #[test]
    fn reserved_and_out_of_range_codes_are_not_emittable() {
        assert!(!phone_can_emit(0));
        assert!(!phone_can_emit(255));
        assert!(!phone_can_emit(u16::MAX));
    }

    #[test]
    fn the_flattened_set_matches_the_ranges() {
        let codes = phone_emittable_codes();
        assert_eq!(codes.len(), 93);
        assert!(codes.iter().all(|&c| phone_can_emit(c)));
        assert!(
            codes.contains(&43) && codes.contains(&110),
            "Backslash and Insert"
        );
    }
}
