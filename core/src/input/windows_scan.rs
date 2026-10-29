//! A key as Windows' low-level keyboard hook names it (a set-1 scancode and
//! whether it carries the E0 prefix, `LLKHF_EXTENDED`) as the evdev code the
//! wire carries: the inverse of the server's `input_sendinput::stroke_for`,
//! which presses an evdev code on Windows. A plain table, tested on every
//! system, shared by the server's shared-input hook and the desktop app's
//! keyboard grab for another computer's window (PC4).

/// The evdev code for the key with set-1 `scan` (and the E0 prefix when
/// `extended`), or `None` for one this does not know (it is not sent).
pub fn evdev_from_scan(scan: u16, extended: bool) -> Option<u16> {
    if extended {
        return match scan {
            0x1C => Some(96),  // KEY_KPENTER
            0x1D => Some(97),  // KEY_RIGHTCTRL
            0x35 => Some(98),  // KEY_KPSLASH
            0x37 => Some(99),  // KEY_SYSRQ (Print Screen)
            0x38 => Some(100), // KEY_RIGHTALT (AltGr)
            0x45 => Some(69),  // KEY_NUMLOCK: the hook marks it extended
            0x46 => Some(119), // Ctrl+Break: KEY_PAUSE
            0x47 => Some(102), // KEY_HOME
            0x48 => Some(103), // KEY_UP
            0x49 => Some(104), // KEY_PAGEUP
            0x4B => Some(105), // KEY_LEFT
            0x4D => Some(106), // KEY_RIGHT
            0x4F => Some(107), // KEY_END
            0x50 => Some(108), // KEY_DOWN
            0x51 => Some(109), // KEY_PAGEDOWN
            0x52 => Some(110), // KEY_INSERT
            0x53 => Some(111), // KEY_DELETE
            0x5B => Some(125), // KEY_LEFTMETA
            0x5C => Some(126), // KEY_RIGHTMETA
            0x5D => Some(139), // KEY_MENU (the application key)
            0x20 => Some(113), // KEY_MUTE
            0x2E => Some(114), // KEY_VOLUMEDOWN
            0x30 => Some(115), // KEY_VOLUMEUP
            0x19 => Some(163), // KEY_NEXTSONG
            0x22 => Some(164), // KEY_PLAYPAUSE
            0x10 => Some(165), // KEY_PREVIOUSSONG
            // E0 2A and E0 36 are the "fake shifts" Windows wraps around
            // the navigation keys; they are not keys of their own.
            _ => None,
        };
    }
    match scan {
        // Pause comes as 0x45 without the prefix (NumLock has it).
        0x45 => Some(119), // KEY_PAUSE
        // Alt+Print Screen.
        0x54 => Some(99), // KEY_SYSRQ
        // Evdev numbered the main block after set-1 scancodes.
        c @ 1..=88 => Some(c),
        0x64..=0x6E => Some(183 + (scan - 0x64)), // KEY_F13..KEY_F23
        0x76 => Some(194),                        // KEY_F24
        0x70 => Some(93),                         // KEY_KATAKANAHIRAGANA
        0x73 => Some(89),                         // KEY_RO
        0x79 => Some(92),                         // KEY_HENKAN
        0x7B => Some(94),                         // KEY_MUHENKAN
        0x7D => Some(124),                        // KEY_YEN
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_block_is_its_own_number_and_the_rest_are_named() {
        assert_eq!(evdev_from_scan(0x1E, false), Some(30)); // A
        assert_eq!(evdev_from_scan(0x1D, false), Some(29)); // left Ctrl
        assert_eq!(evdev_from_scan(0x1D, true), Some(97)); // right Ctrl
        assert_eq!(evdev_from_scan(0x4B, false), Some(75)); // keypad 4
        assert_eq!(evdev_from_scan(0x4B, true), Some(105)); // Left
        assert_eq!(evdev_from_scan(0x45, true), Some(69)); // NumLock
        assert_eq!(evdev_from_scan(0x45, false), Some(119)); // Pause
        assert_eq!(evdev_from_scan(0x64, false), Some(183)); // F13
        assert_eq!(evdev_from_scan(0x76, false), Some(194)); // F24
        // The fake shift around the navigation keys, and no key at all.
        assert_eq!(evdev_from_scan(0x2A, true), None);
        assert_eq!(evdev_from_scan(0, false), None);
        assert_eq!(evdev_from_scan(0x7F, false), None);
    }
}
