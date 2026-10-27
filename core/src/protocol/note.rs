//! A note from the phone to the computer (BF4): text or a link shared to
//! "Note to computer" is appended, with the time, to a Markdown file the
//! owner picks on the computer. Under the Files permission.

/// Phone → computer: `{text}`.
pub const NOTE: &str = "kdeconnect.pocketdesk.note";
/// Computer → phone: `{saved: true}`, or `{saved: false, reason}` with
/// `no_file` (none picked in Settings), `empty` or `not_written`.
pub const NOTE_SAVED: &str = "kdeconnect.pocketdesk.note_saved";

/// The most characters a note keeps.
pub const MAX_NOTE: usize = 4000;

/// `text` as it is kept: line ends made `\n`, control characters other than
/// line ends and tabs dropped, blank lines at either end trimmed, at most
/// [`MAX_NOTE`] characters (cut with `…`). `None` when nothing is left.
pub fn clean_note(text: &str) -> Option<String> {
    let kept: String = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let trimmed = kept.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_NOTE {
        return Some(trimmed.to_string());
    }
    let cut: String = trimmed.chars().take(MAX_NOTE - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// The answer's body: saved, or why not.
pub fn saved_body(refused: Option<&str>) -> serde_json::Value {
    match refused {
        None => serde_json::json!({ "saved": true }),
        Some(reason) => serde_json::json!({ "saved": false, "reason": reason }),
    }
}

/// What an answer says: `Ok(())` saved, else the reason.
pub fn saved_of(body: &serde_json::Value) -> Result<(), String> {
    if body.get("saved").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    Err(body
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("not_written")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_keeps_its_lines_and_drops_control_characters() {
        assert_eq!(
            clean_note("\r\n  Buy milk\r\n\tand bread\u{7}  \n\n").as_deref(),
            Some("Buy milk\n\tand bread")
        );
        assert_eq!(clean_note(" \n\t "), None);
        let long = "a".repeat(MAX_NOTE + 10);
        let cut = clean_note(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_NOTE);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_answer_says_saved_or_why_not() {
        assert_eq!(saved_of(&saved_body(None)), Ok(()));
        assert_eq!(
            saved_of(&saved_body(Some("no_file"))),
            Err("no_file".to_string())
        );
        assert_eq!(
            saved_of(&serde_json::json!({})),
            Err("not_written".to_string())
        );
    }
}
