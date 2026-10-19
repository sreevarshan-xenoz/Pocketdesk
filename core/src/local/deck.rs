//! The deck (F1): buttons the owner sets up on the computer (`[[deck]]` in
//! config.toml), which a phone shows as a grid and presses; the computer
//! does each button's action under that phone's permissions. This module
//! holds the buttons and checks them; the server does the actions.

use serde::{Deserialize, Serialize};

use crate::input::evdev_codes::{ALL_NAMED, KeyCode};

/// The most buttons a deck holds.
pub const MAX_BUTTONS: usize = 48;
/// The most keys in one button's combination.
pub const MAX_KEYS: usize = 5;
/// The longest text a button types.
pub const MAX_TEXT: usize = 1000;
/// The longest link a button opens.
pub const MAX_LINK: usize = 2048;

/// A button as config.toml names it: a label and one action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckButton {
    /// What the phone shows on it.
    pub label: String,
    /// A key combination, `ctrl+shift+m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<String>,
    /// A named task (`[[tasks]]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// `playPause`, `next` or `previous`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    /// An http(s) link to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Text to type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// What a button does, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeckAction {
    /// Press these in order, then let go of them in reverse.
    Keys(Vec<KeyCode>),
    Task(String),
    Media(MediaKey),
    Link(String),
    Text(String),
}

/// The computer's media buttons a deck can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
}

impl MediaKey {
    /// The name the media controls take (`crate`'s `media::Command::parse`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlayPause => "playPause",
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}

impl DeckAction {
    /// The kind the phone is told: never the keys, link or text themselves.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Keys(_) => "keys",
            Self::Task(_) => "task",
            Self::Media(_) => "media",
            Self::Link(_) => "link",
            Self::Text(_) => "text",
        }
    }
}

impl DeckButton {
    /// The button's action, or why it cannot be one.
    pub fn action(&self) -> Result<DeckAction, String> {
        label_problem(&self.label).map_or(Ok(()), |p| Err(p.to_string()))?;
        let set = [
            self.keys.is_some(),
            self.task.is_some(),
            self.media.is_some(),
            self.link.is_some(),
            self.text.is_some(),
        ]
        .iter()
        .filter(|s| **s)
        .count();
        if set != 1 {
            return Err(
                "a button needs exactly one action: keys, a task, media, a link or text".into(),
            );
        }
        if let Some(keys) = &self.keys {
            return parse_keys(keys).map(DeckAction::Keys);
        }
        if let Some(task) = &self.task {
            return match super::tasks::name_problem(task) {
                None => Ok(DeckAction::Task(task.clone())),
                Some(p) => Err(p.to_string()),
            };
        }
        if let Some(media) = &self.media {
            return match media.trim() {
                "playPause" => Ok(DeckAction::Media(MediaKey::PlayPause)),
                "next" => Ok(DeckAction::Media(MediaKey::Next)),
                "previous" => Ok(DeckAction::Media(MediaKey::Previous)),
                _ => Err("media is playPause, next or previous".into()),
            };
        }
        if let Some(link) = &self.link {
            let link = link.trim();
            let lower = link.to_ascii_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://")) {
                return Err("a link starts with http:// or https://".into());
            }
            if link.len() > MAX_LINK {
                return Err(format!("a link is at most {MAX_LINK} bytes"));
            }
            return Ok(DeckAction::Link(link.to_string()));
        }
        let text = self.text.clone().unwrap_or_default();
        if text.is_empty() {
            return Err("the text is empty".into());
        }
        if text.chars().count() > MAX_TEXT {
            return Err(format!("the text is at most {MAX_TEXT} characters"));
        }
        Ok(DeckAction::Text(text))
    }
}

/// Why a label cannot be one: 1 to 40 characters, no control characters.
pub fn label_problem(label: &str) -> Option<&'static str> {
    if label.trim().is_empty() {
        Some("a button needs a label")
    } else if label.chars().count() > 40 {
        Some("a label is at most 40 characters")
    } else if label.chars().any(char::is_control) {
        Some("a label cannot hold control characters")
    } else {
        None
    }
}

/// The keys of a combination such as `ctrl+shift+m`: names joined by `+`,
/// without case, each an evdev key name (`f5`, `pageup`) or an alias.
pub fn parse_keys(combo: &str) -> Result<Vec<KeyCode>, String> {
    let names: Vec<&str> = combo.split('+').map(str::trim).collect();
    if names.iter().any(|n| n.is_empty()) {
        return Err("keys are names joined by +, such as ctrl+shift+m".into());
    }
    if names.len() > MAX_KEYS {
        return Err(format!("at most {MAX_KEYS} keys at once"));
    }
    let mut codes = Vec::with_capacity(names.len());
    for name in names {
        let code = key_code(name).ok_or_else(|| format!("\"{name}\" is not a key's name"))?;
        if codes.contains(&code) {
            return Err(format!("\"{name}\" is there twice"));
        }
        codes.push(code);
    }
    Ok(codes)
}

/// The code for one key name.
fn key_code(name: &str) -> Option<KeyCode> {
    let lower = name.to_ascii_lowercase();
    let alias = match lower.as_str() {
        "ctrl" | "control" => "leftctrl",
        "shift" => "leftshift",
        "alt" | "option" => "leftalt",
        "win" | "super" | "meta" | "cmd" | "windows" => "leftmeta",
        "altgr" => "rightalt",
        "esc" => "esc",
        "escape" => "esc",
        "del" => "delete",
        "ins" => "insert",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        "return" => "enter",
        "prtsc" | "printscreen" => "sysrq",
        "playpause" => "playpause",
        other => other,
    };
    let wanted = format!("KEY_{}", alias.to_ascii_uppercase());
    ALL_NAMED
        .iter()
        .find(|(n, _)| *n == wanted)
        .map(|(_, code)| *code)
}

/// The buttons a phone may be shown: those that check, with their index in
/// the config (a press names it), at most [`MAX_BUTTONS`].
pub fn usable(buttons: &[DeckButton]) -> Vec<(usize, &DeckButton, DeckAction)> {
    buttons
        .iter()
        .enumerate()
        .take(MAX_BUTTONS)
        .filter_map(|(i, b)| b.action().ok().map(|a| (i, b, a)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(label: &str) -> DeckButton {
        DeckButton {
            label: label.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_combination_of_names_and_aliases_parses_in_order() {
        assert_eq!(
            parse_keys("ctrl+shift+m"),
            Ok(vec![
                KeyCode::KEY_LEFTCTRL,
                KeyCode::KEY_LEFTSHIFT,
                KeyCode::KEY_M
            ])
        );
        assert_eq!(
            parse_keys(" Win + PgUp "),
            Ok(vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_PAGEUP])
        );
        assert_eq!(parse_keys("f5"), Ok(vec![KeyCode::KEY_F5]));
        assert_eq!(
            parse_keys("alt+tab"),
            Ok(vec![KeyCode::KEY_LEFTALT, KeyCode::KEY_TAB])
        );
        assert_eq!(parse_keys("esc"), Ok(vec![KeyCode::KEY_ESC]));
        assert_eq!(
            parse_keys("ctrl+del"),
            Ok(vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_DELETE])
        );
    }

    #[test]
    fn a_bad_combination_says_why() {
        assert!(parse_keys("ctrl+").unwrap_err().contains("joined by +"));
        assert!(parse_keys("ctrl+banana").unwrap_err().contains("banana"));
        assert!(parse_keys("ctrl+ctrl").unwrap_err().contains("twice"));
        assert!(parse_keys("a+b+c+d+e+f").unwrap_err().contains("at most 5"));
    }

    #[test]
    fn each_kind_of_button_checks() {
        let keys = DeckButton {
            keys: Some("ctrl+c".into()),
            ..button("Copy")
        };
        assert_eq!(keys.action().unwrap().kind(), "keys");
        let task = DeckButton {
            task: Some("build".into()),
            ..button("Build")
        };
        assert_eq!(task.action(), Ok(DeckAction::Task("build".into())));
        let media = DeckButton {
            media: Some("next".into()),
            ..button("Next")
        };
        assert_eq!(media.action(), Ok(DeckAction::Media(MediaKey::Next)));
        let link = DeckButton {
            link: Some("https://example.org/".into()),
            ..button("Site")
        };
        assert_eq!(link.action().unwrap().kind(), "link");
        let text = DeckButton {
            text: Some("Thanks!".into()),
            ..button("Thanks")
        };
        assert_eq!(text.action(), Ok(DeckAction::Text("Thanks!".into())));
    }

    #[test]
    fn a_button_needs_a_label_and_exactly_one_action() {
        assert!(
            button("Nothing")
                .action()
                .unwrap_err()
                .contains("exactly one")
        );
        let two = DeckButton {
            keys: Some("a".into()),
            text: Some("b".into()),
            ..button("Two")
        };
        assert!(two.action().unwrap_err().contains("exactly one"));
        let unnamed = DeckButton {
            keys: Some("a".into()),
            ..button("  ")
        };
        assert!(unnamed.action().unwrap_err().contains("label"));
        let long = DeckButton {
            keys: Some("a".into()),
            ..button(&"x".repeat(41))
        };
        assert!(long.action().unwrap_err().contains("40"));
    }

    #[test]
    fn links_media_and_text_are_held_to_their_limits() {
        let js = DeckButton {
            link: Some("javascript:alert(1)".into()),
            ..button("Bad")
        };
        assert!(js.action().unwrap_err().contains("http"));
        let long = DeckButton {
            link: Some(format!("https://example.org/{}", "a".repeat(MAX_LINK))),
            ..button("Long")
        };
        assert!(long.action().is_err());
        let media = DeckButton {
            media: Some("stop".into()),
            ..button("Stop")
        };
        assert!(media.action().is_err());
        let empty = DeckButton {
            text: Some(String::new()),
            ..button("Empty")
        };
        assert!(empty.action().is_err());
        let novel = DeckButton {
            text: Some("w".repeat(MAX_TEXT + 1)),
            ..button("Novel")
        };
        assert!(novel.action().is_err());
    }

    #[test]
    fn the_phone_is_shown_the_usable_buttons_by_their_config_index() {
        let buttons = vec![
            DeckButton {
                keys: Some("ctrl+c".into()),
                ..button("Copy")
            },
            button("Broken"),
            DeckButton {
                media: Some("playPause".into()),
                ..button("Play")
            },
        ];
        let shown: Vec<(usize, &str)> = usable(&buttons)
            .iter()
            .map(|(i, b, _)| (*i, b.label.as_str()))
            .collect();
        assert_eq!(shown, vec![(0, "Copy"), (2, "Play")]);
    }

    #[test]
    fn buttons_read_from_toml() {
        let toml = r#"
            [[deck]]
            label = "Mute mic"
            keys = "ctrl+shift+m"

            [[deck]]
            label = "Build"
            task = "build"
        "#;
        #[derive(Deserialize)]
        struct Holder {
            deck: Vec<DeckButton>,
        }
        let held: Holder = toml::from_str(toml).unwrap();
        assert_eq!(held.deck.len(), 2);
        assert_eq!(held.deck[0].keys.as_deref(), Some("ctrl+shift+m"));
        assert_eq!(held.deck[1].action(), Ok(DeckAction::Task("build".into())));
    }
}
