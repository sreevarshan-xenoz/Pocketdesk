//! The key that sends this computer's clipboard to a phone (H3), and the
//! one that opens the window typing on a phone (N9): parsed from desktop
//! Settings ("ctrl+alt+p"), and which phone each goes to. Kept apart from
//! Windows so it is tested; `tray_main` registers them with
//! `RegisterHotKey` and acts on `WM_HOTKEY`, or on Linux binds them through
//! the GlobalShortcuts portal (R4, `hotkey_portal`).

use std::path::Path;

use crate::model::PhoneRow;

/// What the tray program says of the key (H3 review): the tray has no
/// console, so Settings shows this instead.
pub const STATUS_FILE: &str = "clipboard_hotkey.json";

/// The typing key's (N9).
pub const TYPING_STATUS_FILE: &str = "typing_hotkey.json";

/// The key as the tray program last saw it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The key's text in Settings when this was written.
    pub key: String,
    /// It is registered and pressed sends.
    pub registered: bool,
    /// Another app holds the combination.
    pub taken: bool,
    /// The last press found no phone the clipboard reaches.
    pub no_phone: bool,
    /// The desktop has no way to take a key (Linux without the
    /// GlobalShortcuts portal; R4).
    pub unsupported: bool,
}

pub fn write_status(state_dir: &Path, status: &Status) {
    write_status_to(state_dir, STATUS_FILE, status);
}

/// The status of the key whose file is `file`.
pub fn write_status_to(state_dir: &Path, file: &str, status: &Status) {
    let body = serde_json::json!({
        "key": status.key,
        "registered": status.registered,
        "taken": status.taken,
        "no_phone": status.no_phone,
        "unsupported": status.unsupported,
    });
    let _ = std::fs::write(state_dir.join(file), body.to_string());
}

pub fn read_status(state_dir: &Path) -> Option<Status> {
    read_status_from(state_dir, STATUS_FILE)
}

/// The status of the key whose file is `file`.
pub fn read_status_from(state_dir: &Path, file: &str) -> Option<Status> {
    let raw = std::fs::read_to_string(state_dir.join(file)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let flag = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    Some(Status {
        key: v.get("key")?.as_str()?.to_string(),
        registered: flag("registered"),
        taken: flag("taken"),
        no_phone: flag("no_phone"),
        unsupported: flag("unsupported"),
    })
}

/// What Settings says of the key `text` from the tray's [Status]; None when
/// there is nothing to say, or the status is of another key.
pub fn status_words(text: &str, status: Option<&Status>) -> Option<&'static str> {
    status_words_saying(
        text,
        status,
        "Registered. The last press found no phone the clipboard reaches.",
    )
}

/// The typing key's words (N9).
pub fn typing_status_words(text: &str, status: Option<&Status>) -> Option<&'static str> {
    status_words_saying(
        text,
        status,
        "Registered. The last press found no linked phone this computer may type on.",
    )
}

/// [status_words], saying `no_phone` after a press that found none.
fn status_words_saying(
    text: &str,
    status: Option<&Status>,
    no_phone: &'static str,
) -> Option<&'static str> {
    let text = text.trim();
    if text.is_empty() || parse(text).is_none() {
        return None;
    }
    match status.filter(|s| s.key == text) {
        Some(s) if s.unsupported => Some(
            "This desktop has no global shortcuts portal (KDE Plasma 6, GNOME 48 and later have one), so no key is taken.",
        ),
        None => {
            Some("Not taken up yet: the tray program (pocketdesk-tray) registers it while it runs.")
        }
        Some(s) if s.taken => {
            Some("Another app holds this combination; the tray tries again every 30 s.")
        }
        Some(s) if s.no_phone => Some(no_phone),
        Some(s) if s.registered => Some("Registered."),
        Some(_) => None,
    }
}

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

/// The key as the GlobalShortcuts portal's preferred trigger reads it (R4):
/// "win+shift+c" is "LOGO+SHIFT+c". None where [`parse`] takes none.
pub fn portal_trigger(text: &str) -> Option<String> {
    parse(text)?;
    let parts: Vec<String> = text
        .split('+')
        .map(|p| p.trim().to_ascii_lowercase())
        .collect();
    let (last, mods) = parts.split_last()?;
    let mut out: Vec<String> = mods
        .iter()
        .map(|m| {
            match m.as_str() {
                "ctrl" | "control" => "CTRL",
                "alt" => "ALT",
                "shift" => "SHIFT",
                _ => "LOGO",
            }
            .to_string()
        })
        .collect();
    out.push(if last.len() > 1 {
        last.to_ascii_uppercase()
    } else {
        last.clone()
    });
    Some(out.join("+"))
}

/// The key as people read it: "win+shift+c" is "Win+Shift+C" (H4).
pub fn display(text: &str) -> String {
    text.split('+')
        .map(|part| {
            let part = part.trim();
            let mut chars = part.chars();
            match chars.next() {
                Some(first) if part.len() == 1 => first.to_ascii_uppercase().to_string(),
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// The phone the typing key opens the window for (N9): a linked one this
/// computer may control, the one in the newest session first.
pub fn typing_phone<'a>(rows: &'a [PhoneRow<'a>]) -> Option<&'a str> {
    rows.iter()
        .filter(|r| {
            r.device.linked
                && r.device
                    .permissions
                    .allows(pocketdesk_core::protocol::permissions::Capability::Cast)
        })
        .max_by_key(|r| r.last_session.unwrap_or(0))
        .map(|r| r.device.id.as_str())
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

    /// R4: the portal's preferred trigger, from Settings' text.
    #[test]
    fn a_key_reads_as_the_portal_s_trigger() {
        assert_eq!(
            super::portal_trigger("win+shift+c").as_deref(),
            Some("LOGO+SHIFT+c")
        );
        assert_eq!(
            super::portal_trigger("Ctrl+Alt+f5").as_deref(),
            Some("CTRL+ALT+F5")
        );
        assert_eq!(super::portal_trigger("super+7").as_deref(), Some("LOGO+7"));
        assert_eq!(super::portal_trigger("shift+a"), None);
        assert_eq!(super::portal_trigger(""), None);
    }

    /// R4: a desktop with no portal says so, and the file keeps it.
    #[test]
    fn a_desktop_without_the_portal_says_so() {
        let status = super::Status {
            key: "win+shift+c".to_string(),
            unsupported: true,
            ..Default::default()
        };
        assert!(
            super::status_words("win+shift+c", Some(&status))
                .is_some_and(|w| w.contains("no global shortcuts portal"))
        );
        let dir = std::env::temp_dir().join(format!("pd-hotkey-portal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        super::write_status(&dir, &status);
        assert_eq!(super::read_status(&dir), Some(status));
        let _ = std::fs::remove_dir_all(&dir);
    }
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
        assert_eq!(parse("ctrl+"), None);
        assert_eq!(parse("ctrl++p"), None);
        assert_eq!(parse("+"), None);
    }

    #[test]
    fn a_key_reads_as_people_write_it() {
        assert_eq!(display("win+shift+c"), "Win+Shift+C");
        assert_eq!(display(" CTRL + alt + f5 "), "Ctrl+Alt+F5");
    }

    /// H3 review: Settings says what the tray made of the key.
    #[test]
    fn settings_says_what_the_tray_made_of_the_key() {
        let taken = Status {
            key: "ctrl+alt+p".to_string(),
            taken: true,
            ..Default::default()
        };
        assert!(
            status_words("ctrl+alt+p", Some(&taken))
                .unwrap()
                .contains("Another app")
        );
        assert!(
            status_words("ctrl+alt+p", None)
                .unwrap()
                .contains("tray program")
        );
        let other = Status {
            key: "ctrl+alt+o".to_string(),
            registered: true,
            ..Default::default()
        };
        assert!(
            status_words("ctrl+alt+p", Some(&other))
                .unwrap()
                .contains("tray program")
        );
        assert_eq!(status_words("", Some(&taken)), None);
        assert_eq!(status_words("p", None), None, "a bad key says so elsewhere");
        let pressed = Status {
            key: "ctrl+alt+p".to_string(),
            registered: true,
            no_phone: true,
            ..Default::default()
        };
        assert!(
            status_words("ctrl+alt+p", Some(&pressed))
                .unwrap()
                .contains("no phone")
        );
        // N9: the typing key's own words.
        assert!(
            typing_status_words("ctrl+alt+p", Some(&pressed))
                .unwrap()
                .contains("type on")
        );
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
            playing: None,
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

    /// N9: the typing key opens the window for a linked phone this computer
    /// may control, the newest session's first.
    #[test]
    fn the_typing_key_goes_to_a_linked_phone_it_may_control() {
        use pocketdesk_core::protocol::permissions::Capability;
        let device = |id: &str, linked: bool, cast: bool| {
            let mut d = Device {
                id: id.to_string(),
                linked,
                ..Default::default()
            };
            d.permissions.set(Capability::Cast, cast);
            d
        };
        let devices: &'static [Device; 3] = Box::leak(Box::new([
            device("a", true, true),
            device("b", true, false),
            device("c", false, true),
        ]));
        let row = |device: &'static Device, last: Option<u64>| PhoneRow {
            device,
            place: crate::model::Place::Away,
            last_session: last,
            ring: Err("x"),
            mute: Err("x"),
            file: Err("x"),
            link: Err("x"),
            clipboard: Err("x"),
            playing: None,
        };
        let rows = [
            row(&devices[0], Some(1)),
            row(&devices[1], Some(9)),
            row(&devices[2], Some(9)),
        ];
        assert_eq!(
            typing_phone(&rows),
            Some("a"),
            "linked and controllable only"
        );
        assert_eq!(typing_phone(&rows[1..]), None);
    }
}
