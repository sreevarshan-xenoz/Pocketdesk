//! Linux evdev key and button codes as plain numbers, for every OS.
//!
//! The wire carries evdev `KEY_*` codes ([`super::keys`]), so a server on an OS
//! without the `evdev` crate still has to *name* them. [`KeyCode`] mirrors the
//! shape of `evdev::KeyCode` (a newtype over `u16` with `KEY_*` associated
//! constants), which lets a table written as `KeyCode::KEY_ESC.0` compile
//! against either one. Values are `linux/input-event-codes.h`; the server's
//! Linux tests hold every constant here to the real `evdev` crate.

/// An evdev key or button code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCode(pub u16);

macro_rules! key_codes {
    ($($name:ident = $value:expr),* $(,)?) => {
        #[allow(missing_docs)]
        impl KeyCode {
            $(pub const $name: KeyCode = KeyCode($value);)*
        }

        /// Every named code, for tests that hold this table to another one.
        pub const ALL_NAMED: &[(&str, KeyCode)] = &[$((stringify!($name), KeyCode::$name)),*];
    };
}

key_codes! {
    KEY_ESC = 1,
    KEY_1 = 2,
    KEY_2 = 3,
    KEY_3 = 4,
    KEY_4 = 5,
    KEY_5 = 6,
    KEY_6 = 7,
    KEY_7 = 8,
    KEY_8 = 9,
    KEY_9 = 10,
    KEY_0 = 11,
    KEY_MINUS = 12,
    KEY_EQUAL = 13,
    KEY_BACKSPACE = 14,
    KEY_TAB = 15,
    KEY_Q = 16,
    KEY_W = 17,
    KEY_E = 18,
    KEY_R = 19,
    KEY_T = 20,
    KEY_Y = 21,
    KEY_U = 22,
    KEY_I = 23,
    KEY_O = 24,
    KEY_P = 25,
    KEY_LEFTBRACE = 26,
    KEY_RIGHTBRACE = 27,
    KEY_ENTER = 28,
    KEY_LEFTCTRL = 29,
    KEY_A = 30,
    KEY_S = 31,
    KEY_D = 32,
    KEY_F = 33,
    KEY_G = 34,
    KEY_H = 35,
    KEY_J = 36,
    KEY_K = 37,
    KEY_L = 38,
    KEY_SEMICOLON = 39,
    KEY_APOSTROPHE = 40,
    KEY_GRAVE = 41,
    KEY_LEFTSHIFT = 42,
    KEY_BACKSLASH = 43,
    KEY_Z = 44,
    KEY_X = 45,
    KEY_C = 46,
    KEY_V = 47,
    KEY_B = 48,
    KEY_N = 49,
    KEY_M = 50,
    KEY_COMMA = 51,
    KEY_DOT = 52,
    KEY_SLASH = 53,
    KEY_RIGHTSHIFT = 54,
    KEY_KPASTERISK = 55,
    KEY_LEFTALT = 56,
    KEY_SPACE = 57,
    KEY_CAPSLOCK = 58,
    KEY_F1 = 59,
    KEY_F2 = 60,
    KEY_F3 = 61,
    KEY_F4 = 62,
    KEY_F5 = 63,
    KEY_F6 = 64,
    KEY_F7 = 65,
    KEY_F8 = 66,
    KEY_F9 = 67,
    KEY_F10 = 68,
    KEY_NUMLOCK = 69,
    KEY_SCROLLLOCK = 70,
    KEY_KPPLUS = 78,
    KEY_F11 = 87,
    KEY_F12 = 88,
    KEY_RIGHTCTRL = 97,
    KEY_SYSRQ = 99,
    KEY_RIGHTALT = 100,
    KEY_HOME = 102,
    KEY_UP = 103,
    KEY_PAGEUP = 104,
    KEY_LEFT = 105,
    KEY_RIGHT = 106,
    KEY_END = 107,
    KEY_DOWN = 108,
    KEY_PAGEDOWN = 109,
    KEY_INSERT = 110,
    KEY_DELETE = 111,
    KEY_MUTE = 113,
    KEY_VOLUMEDOWN = 114,
    KEY_VOLUMEUP = 115,
    KEY_PAUSE = 119,
    KEY_LEFTMETA = 125,
    KEY_RIGHTMETA = 126,
    KEY_MENU = 139,
    KEY_NEXTSONG = 163,
    KEY_PLAYPAUSE = 164,
    KEY_PREVIOUSSONG = 165,
    KEY_F13 = 183,
    KEY_F14 = 184,
    KEY_F15 = 185,
    KEY_F16 = 186,
    KEY_F17 = 187,
    KEY_F18 = 188,
    KEY_F19 = 189,
    KEY_F20 = 190,
    KEY_F21 = 191,
    KEY_F22 = 192,
    KEY_F23 = 193,
    KEY_F24 = 194,
    KEY_UNKNOWN = 240,
    BTN_LEFT = 0x110,
    BTN_RIGHT = 0x111,
    BTN_MIDDLE = 0x112,
    BTN_SIDE = 0x113,
    BTN_EXTRA = 0x114,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_codes_do_not_collide() {
        let mut codes: Vec<u16> = ALL_NAMED.iter().map(|(_, c)| c.0).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(before, codes.len(), "two names share one evdev code");
    }

    #[test]
    fn every_phone_emittable_code_has_a_name() {
        for code in super::super::keys::phone_emittable_codes() {
            assert!(
                ALL_NAMED.iter().any(|(_, c)| c.0 == code),
                "evdev code {code} can arrive from the phone but has no name here"
            );
        }
    }
}
