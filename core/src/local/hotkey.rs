//! A key combination from Settings ("ctrl+alt+p"), as Windows'
//! `RegisterHotKey` takes it: the desktop app's keys (the clipboard's and
//! the typing window's) and the server's shared keyboard and mouse
//! (`shared_input_hotkey`, "ctrl+alt+right") read it the same way.

/// `RegisterHotKey`'s modifier bits (MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN).
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;
/// Held down, the key repeats nothing (MOD_NOREPEAT).
pub const MOD_NOREPEAT: u32 = 0x4000;

/// A key combination as `RegisterHotKey` takes it: its modifier bits and
/// the key's virtual-key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

/// The arrow keys' names and virtual-key codes (VK_LEFT, VK_UP, VK_RIGHT,
/// VK_DOWN).
const ARROWS: [(&str, u32); 4] = [
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
];

/// Parse "ctrl+alt+p": one or more of ctrl, alt, shift and win, then one
/// letter, digit, F1 to F12 or arrow key (left, right, up, down). None for
/// an empty text or anything else, and for a key without a modifier (it
/// would take that key from every app).
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
        [b'f', rest @ ..] if !rest.is_empty() && rest.iter().all(u8::is_ascii_digit) => {
            let n: u32 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            if (1..=12).contains(&n) {
                0x70 + n - 1 // VK_F1..VK_F12
            } else {
                return None;
            }
        }
        _ => ARROWS
            .iter()
            .find(|(name, _)| name == last)
            .map(|&(_, vk)| vk)?,
    };
    Some(Hotkey {
        modifiers: modifiers | MOD_NOREPEAT,
        key,
    })
}

/// Whether `name` is one of the arrow keys [`parse`] takes.
pub fn is_arrow(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    ARROWS.iter().any(|(arrow, _)| *arrow == name)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(parse("ctrl+f").map(|h| h.key), Some(u32::from(b'F')));
        assert_eq!(parse("ctrl+f+1"), None);
        assert_eq!(parse("ctrl+hyper+p"), None);
        assert_eq!(parse("ctrl+pp"), None);
        assert_eq!(parse("ctrl+"), None);
        assert_eq!(parse("ctrl++p"), None);
        assert_eq!(parse("+"), None);
    }

    #[test]
    fn the_arrow_keys_parse() {
        let vk = |text: &str| parse(text).map(|h| h.key);
        assert_eq!(vk("ctrl+alt+left"), Some(0x25));
        assert_eq!(vk("ctrl+alt+up"), Some(0x26));
        assert_eq!(vk("Ctrl + Alt + RIGHT"), Some(0x27));
        assert_eq!(vk("win+down"), Some(0x28));
        // An arrow alone, or with Shift alone, is still no key to take.
        assert_eq!(parse("right"), None);
        assert_eq!(parse("shift+left"), None);
        assert_eq!(parse("ctrl+rightt"), None);
        assert!(is_arrow(" Right "));
        assert!(!is_arrow("r"));
    }

    /// Shared input's default (`shared_input_hotkey`) is a key.
    #[test]
    fn the_shared_input_default_parses() {
        let default = crate::local::config::Config::default().shared_input_hotkey;
        assert_eq!(default, "ctrl+alt+right");
        assert_eq!(
            parse(&default),
            Some(Hotkey {
                modifiers: MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                key: 0x27
            })
        );
    }
}
