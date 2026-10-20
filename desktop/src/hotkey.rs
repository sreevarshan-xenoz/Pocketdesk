//! The key that sends this computer's clipboard to a phone (H3): parsed
//! from desktop Settings ("ctrl+alt+p"), and which phone it goes to. Kept
//! apart from Windows so it is tested; `tray_main` registers it with
//! `RegisterHotKey` and acts on `WM_HOTKEY`.

use crate::model::PhoneRow;

/// `RegisterHotKey`'s modifier bits (MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN).
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;
/// Held down, the key repeats nothing (MOD_NOREPEAT).
pub const MOD_NOREPEAT: u32 = 0x4000;

/// A key combination as `RegisterHotKey` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

/// Parse "ctrl+alt+p": one or more of ctrl, alt, shift and win, then one
/// letter, digit or F1 to F12. None for an empty text or anything else, and
/// for a key without a modifier (it would take that key from every app).
pub fn parse(text: &str) -> Option<Hotkey> {
    let parts: Vec<String> = text
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    let (last, mods) = parts.split_last()?;
    let mut modifiers = 0;
    for m in mods {
        modifiers |= match m.as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" | "super" | "meta" => MOD_WIN,
            _ => return None,
        };
    }
    // Shift alone with a letter is typing a capital.
    if modifiers & (MOD_CONTROL | MOD_ALT | MOD_WIN) == 0 {
        return None;
    }
    let key = match last.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => u32::from(c.to_ascii_uppercase()),
        [b'f', rest @ ..] => {
            let n: u32 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            if (1..=12).contains(&n) {
                0x70 + n - 1 // VK_F1..VK_F12
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(Hotkey {
        modifiers: modifiers | MOD_NOREPEAT,
        key,
    })
}

/// The phone the key sends to: one the clipboard reaches now, the one in
/// the newest session when several do; else none.
pub fn phone<'a>(rows: &'a [PhoneRow<'a>]) -> Option<&'a str> {
    rows.iter()
        .filter(|r| r.clipboard.is_ok())
        .max_by_key(|r| r.last_session.unwrap_or(0))
        .map(|r| r.device.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Device;

    #[test]
    fn a_combination_parses_and_a_bare_key_does_not() {
        assert_eq!(
            parse("ctrl+alt+p"),
            Some(Hotkey {
                modifiers: MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                key: u32::from(b'P')
            })
        );
        assert_eq!(
            parse(" Win + Shift + F5 "),
            Some(Hotkey {
                modifiers: MOD_WIN | MOD_SHIFT | MOD_NOREPEAT,
                key: 0x74
            })
        );
        assert_eq!(parse("ctrl+9").map(|h| h.key), Some(u32::from(b'9')));
        assert_eq!(parse(""), None);
        assert_eq!(parse("p"), None, "no modifier");
        assert_eq!(parse("shift+p"), None, "a capital");
        assert_eq!(parse("ctrl+f13"), None);
        assert_eq!(parse("ctrl+hyper+p"), None);
        assert_eq!(parse("ctrl+pp"), None);
    }

    #[test]
    fn the_key_goes_to_the_phone_in_the_newest_session_the_clipboard_reaches() {
        let devices = [
            Device {
                id: "a".to_string(),
                ..Default::default()
            },
            Device {
                id: "b".to_string(),
                ..Default::default()
            },
        ];
        let row = |device: &'static Device, last: Option<u64>, reach: bool| PhoneRow {
            device,
            place: crate::model::Place::Away,
            last_session: last,
            ring: Err("x"),
            mute: Err("x"),
            file: Err("x"),
            link: Err("x"),
            clipboard: if reach { Ok(()) } else { Err("x") },
        };
        let devices: &'static [Device; 2] = Box::leak(Box::new(devices));
        assert_eq!(
            phone(&[
                row(&devices[0], Some(10), true),
                row(&devices[1], Some(20), true)
            ]),
            Some("b")
        );
        assert_eq!(
            phone(&[
                row(&devices[0], Some(10), true),
                row(&devices[1], Some(20), false)
            ]),
            Some("a")
        );
        assert_eq!(phone(&[row(&devices[0], None, false)]), None);
    }
}
