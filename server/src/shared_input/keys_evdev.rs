//! Keys as Linux's libei gives them to the computer driving (BK8): evdev
//! codes, with which keys are down tracked from the key events themselves.
//!
//! The portal's keyboard comes only while the keyboard and mouse are away,
//! and some compositors never say the modifiers' state on their own
//! (xdg-desktop-portal-hyprland #436, GNOME's portal): [`Keys`] counts
//! Ctrl, Alt, Shift and Super from their key events, spots the switch key
//! by them, and forgets what was down at each switch, so a modifier held
//! across one never fires the switch key later. The keys sent down to the
//! computer driven are let go of there by the driver at each switch
//! ([`HeldInput`](super::held::HeldInput)).
//!
//! Pure: tested on every system.

use std::collections::BTreeSet;

use pocketdesk_core::local::hotkey::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

use super::drive::Captured;

/// The evdev codes of the modifiers, with their `RegisterHotKey` bit.
const MODIFIERS: [(u16, u32); 8] = [
    (29, MOD_CONTROL), // KEY_LEFTCTRL
    (97, MOD_CONTROL), // KEY_RIGHTCTRL
    (56, MOD_ALT),     // KEY_LEFTALT
    (100, MOD_ALT),    // KEY_RIGHTALT
    (42, MOD_SHIFT),   // KEY_LEFTSHIFT
    (54, MOD_SHIFT),   // KEY_RIGHTSHIFT
    (125, MOD_WIN),    // KEY_LEFTMETA
    (126, MOD_WIN),    // KEY_RIGHTMETA
];

/// The evdev code of the key with Windows' virtual-key code `vk`, for the
/// keys a switch key may name ([`hotkey::parse`]: a letter or digit, by
/// its place on a US keyboard, F1 to F12, or an arrow).
///
/// [`hotkey::parse`]: pocketdesk_core::local::hotkey::parse
pub fn evdev_of_vk(vk: u32) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, // A to M
        49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, // N to Z
    ];
    let vk16 = u16::try_from(vk).ok()?;
    match vk {
        0x41..=0x5A => LETTERS.get(usize::from(vk16 - 0x41)).copied(),
        0x30 => Some(11),                      // 0
        0x31..=0x39 => Some(vk16 - 0x31 + 2),  // 1 to 9
        0x70..=0x79 => Some(vk16 - 0x70 + 59), // F1 to F10
        0x7A => Some(87),                      // F11
        0x7B => Some(88),                      // F12
        0x25 => Some(105),                     // left
        0x26 => Some(103),                     // up
        0x27 => Some(106),                     // right
        0x28 => Some(108),                     // down
        _ => None,
    }
}

/// The keys down as libei said them, and the switch key.
#[derive(Debug, Default)]
pub struct Keys {
    down: BTreeSet<u16>,
    /// The switch key: its key's evdev code and its modifier bits.
    hotkey: Option<(u16, u32)>,
    /// The switch key went down and its key-up is still to be swallowed.
    hotkey_down: bool,
}

impl Keys {
    pub fn new(hotkey: Option<Hotkey>) -> Keys {
        Keys {
            hotkey: hotkey.and_then(|h| {
                let wanted = h.modifiers & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN);
                Some((evdev_of_vk(h.key)?, wanted))
            }),
            ..Keys::default()
        }
    }

    /// The modifier bits held now.
    pub fn modifiers(&self) -> u32 {
        MODIFIERS
            .iter()
            .filter(|(code, _)| self.down.contains(code))
            .fold(0, |mods, (_, bit)| mods | bit)
    }

    /// The key `code` went down or up: what the driver is told, if
    /// anything. The switch key with exactly its modifiers is the switch
    /// (its key-up swallowed too), never sent.
    pub fn key(&mut self, code: u16, down: bool) -> Option<Captured> {
        if let Some((key, mods)) = self.hotkey
            && code == key
        {
            if down && self.modifiers() == mods {
                return (!std::mem::replace(&mut self.hotkey_down, true))
                    .then_some(Captured::Hotkey);
            }
            if !down && std::mem::take(&mut self.hotkey_down) {
                return None;
            }
        }
        if down {
            self.down.insert(code);
        } else {
            self.down.remove(&code);
        }
        Some(Captured::EvKey { code, down })
    }

    /// The capture began or ended: what was down is forgotten (its key-ups
    /// may never come here), and returned, in order.
    pub fn switched(&mut self) -> Vec<u16> {
        self.hotkey_down = false;
        std::mem::take(&mut self.down).into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::hotkey::parse;

    fn ev(code: u16, down: bool) -> Option<Captured> {
        Some(Captured::EvKey { code, down })
    }

    #[test]
    fn a_switch_keys_key_is_its_evdev_code() {
        let code = |text: &str| parse(text).and_then(|h| evdev_of_vk(h.key));
        assert_eq!(code("ctrl+alt+a"), Some(30));
        assert_eq!(code("ctrl+alt+q"), Some(16));
        assert_eq!(code("ctrl+alt+m"), Some(50));
        assert_eq!(code("ctrl+alt+z"), Some(44));
        assert_eq!(code("ctrl+alt+1"), Some(2));
        assert_eq!(code("ctrl+alt+9"), Some(10));
        assert_eq!(code("ctrl+alt+0"), Some(11));
        assert_eq!(code("ctrl+alt+f1"), Some(59));
        assert_eq!(code("ctrl+alt+f10"), Some(68));
        assert_eq!(code("ctrl+alt+f11"), Some(87));
        assert_eq!(code("ctrl+alt+f12"), Some(88));
        assert_eq!(code("ctrl+alt+left"), Some(105));
        assert_eq!(code("ctrl+alt+up"), Some(103));
        assert_eq!(code("ctrl+alt+right"), Some(106));
        assert_eq!(code("ctrl+alt+down"), Some(108));
        assert_eq!(evdev_of_vk(0x0D), None);
        assert_eq!(evdev_of_vk(u32::MAX), None);
    }

    #[test]
    fn modifiers_are_counted_from_their_keys() {
        let mut keys = Keys::new(None);
        assert_eq!(keys.modifiers(), 0);
        for (code, bit) in MODIFIERS {
            keys.key(code, true);
            assert_eq!(keys.modifiers(), bit, "{code}");
            keys.key(code, false);
        }
        keys.key(97, true);
        keys.key(54, true);
        keys.key(30, true);
        assert_eq!(keys.modifiers(), MOD_CONTROL | MOD_SHIFT);
        keys.key(97, false);
        assert_eq!(keys.modifiers(), MOD_SHIFT);
    }

    #[test]
    fn the_switch_key_is_spotted_by_its_modifiers_and_never_sent() {
        let mut keys = Keys::new(parse("ctrl+alt+right"));
        assert_eq!(keys.key(29, true), ev(29, true));
        assert_eq!(keys.key(100, true), ev(100, true), "AltGr is Alt");
        assert_eq!(keys.key(106, true), Some(Captured::Hotkey));
        // Held: no second switch; its key-up is swallowed too.
        assert_eq!(keys.key(106, true), None);
        assert_eq!(keys.key(106, false), None);
        assert_eq!(keys.key(100, false), ev(100, false));
        // The key alone, or with one modifier more, is a key.
        assert_eq!(keys.key(106, true), ev(106, true));
        assert_eq!(keys.key(106, false), ev(106, false));
        keys.key(56, true);
        keys.key(42, true);
        assert_eq!(keys.key(106, true), ev(106, true));
        assert_eq!(keys.key(106, false), ev(106, false));
        // No switch key: every key is sent.
        let mut keys = Keys::new(None);
        keys.key(29, true);
        keys.key(56, true);
        assert_eq!(keys.key(106, true), ev(106, true));
    }

    /// xdph #436: the modifiers' state is never said, so one held across a
    /// switch would stick: what was down is forgotten at each switch.
    #[test]
    fn what_was_down_is_forgotten_at_each_switch() {
        let mut keys = Keys::new(parse("ctrl+alt+right"));
        keys.key(29, true);
        keys.key(56, true);
        keys.key(30, true);
        assert_eq!(keys.switched(), vec![29, 30, 56]);
        assert_eq!(keys.modifiers(), 0);
        // Ctrl and Alt still held across it are not known down: the arrow
        // alone is a key, not the switch.
        assert_eq!(keys.key(106, true), ev(106, true));
        assert_eq!(keys.switched(), vec![106]);
        // A switch key pressed as the capture ended: its key-up is a key-up
        // again after it.
        keys.key(29, true);
        keys.key(56, true);
        assert_eq!(keys.key(106, true), Some(Captured::Hotkey));
        keys.switched();
        assert_eq!(keys.key(106, false), ev(106, false));
    }
}
