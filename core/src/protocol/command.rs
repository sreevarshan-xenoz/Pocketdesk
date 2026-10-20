//! A command the computer sends the phone's automation apps (L6): the
//! packet, and what its name and text may be before either end takes it.

use super::kdeconnect::is_invisible_format;

/// Computer → phone: `{name, text}`. Under the Command permission.
pub const COMMAND: &str = "kdeconnect.pocketdesk.command";

/// The most characters a command's name has.
pub const MAX_NAME: usize = 64;

/// The most bytes a command's text has.
pub const MAX_TEXT: usize = 4096;

/// Whether `name` may name a command: 1 to [`MAX_NAME`] characters, none a
/// control or an invisible format character, not all spaces.
pub fn valid_name(name: &str) -> bool {
    let count = name.chars().count();
    (1..=MAX_NAME).contains(&count)
        && !name.trim().is_empty()
        && !name
            .chars()
            .any(|c| c.is_control() || is_invisible_format(c))
}

/// Whether `text` may go with a command: at most [`MAX_TEXT`] bytes, no
/// control character but line breaks and tabs.
pub fn valid_text(text: &str) -> bool {
    text.len() <= MAX_TEXT
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

/// A command's body.
pub fn command_body(name: &str, text: &str) -> serde_json::Value {
    serde_json::json!({ "name": name, "text": text })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L6: a name is short and printable; text is bounded.
    #[test]
    fn a_command_is_held_to_its_bounds() {
        assert!(valid_name("lights off"));
        assert!(valid_name(&"n".repeat(MAX_NAME)));
        assert!(!valid_name(&"n".repeat(MAX_NAME + 1)));
        assert!(!valid_name(""));
        assert!(!valid_name("   "));
        assert!(!valid_name("a\nb"));
        assert!(!valid_name("a\u{202E}b"), "an override");
        assert!(valid_text(""));
        assert!(valid_text("line one\nline two\ttab"));
        assert!(!valid_text("bell\u{7}"));
        assert!(!valid_text(&"x".repeat(MAX_TEXT + 1)));
        assert_eq!(
            command_body("lights", "off"),
            serde_json::json!({ "name": "lights", "text": "off" })
        );
    }
}
