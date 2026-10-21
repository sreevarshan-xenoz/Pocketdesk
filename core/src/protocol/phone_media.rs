//! The phone's media on the computer (roadmap T8d): what the phone plays,
//! and the computer's Play/Pause, Next and Previous for it. Text is held to
//! one clean line here, for both directions (T8c's media too).

use serde::{Deserialize, Serialize};

use super::kdeconnect::is_invisible_format;

/// Phone → computer: what the phone plays, or `{}` when nothing does.
pub const PHONE_MEDIA: &str = "kdeconnect.pocketdesk.phone_media";

/// Computer → phone: `{command}`, one of [`COMMANDS`].
pub const PHONE_MEDIA_CONTROL: &str = "kdeconnect.pocketdesk.phone_media_control";

/// What a press may ask a player to do.
pub const COMMANDS: [&str; 3] = ["playPause", "next", "previous"];

/// The longest title, artist or app name kept, in characters.
pub const MAX_TEXT: usize = 200;

/// `text` on one line: control characters (line breaks among them) become
/// spaces, runs of spaces one, invisible format characters (a right-to-left
/// override, zero-width ones) go; at most [`MAX_TEXT`] characters.
pub fn clean_line(text: &str) -> String {
    let spaced: String = text
        .chars()
        .filter(|c| !is_invisible_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEXT)
        .collect()
}

/// The length of a cover's id: hex digits of its JPEG's SHA-256 (N2).
pub const ART_ID_LEN: usize = 16;

/// What a phone plays, its text cleaned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneMedia {
    pub title: String,
    pub artist: String,
    /// The playing app's name.
    pub app: String,
    pub is_playing: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
    /// Sent by the phone's background reporter (N2), which says it again
    /// while it plays whether or not a session runs.
    #[serde(default)]
    pub background: bool,
    /// The cover shown with it (N2): [`ART_ID_LEN`] hex digits naming the
    /// JPEG, whose bytes come once (`art`) and are kept by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_id: Option<String>,
}

/// `id` if it is a cover's id, [`ART_ID_LEN`] lowercase hex digits.
pub fn art_id(id: &str) -> Option<String> {
    (id.len() == ART_ID_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| id.to_string())
}

/// A phone's media report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaReport {
    Playing(PhoneMedia),
    /// Nothing plays any more (`{}`).
    Nothing,
}

impl MediaReport {
    /// The report in a [`PHONE_MEDIA`] body, or `None` for one that is
    /// neither `{}` nor has a title and `isPlaying`.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let fields = body.as_object()?;
        if fields.is_empty() {
            return Some(Self::Nothing);
        }
        let text = |key: &str| clean_line(fields.get(key).and_then(|v| v.as_str()).unwrap_or(""));
        let flag = |key: &str| fields.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        fields.get("title")?.as_str()?;
        let is_playing = fields.get("isPlaying")?.as_bool()?;
        Some(Self::Playing(PhoneMedia {
            title: text("title"),
            artist: text("artist"),
            app: text("app"),
            is_playing,
            can_play_pause: flag("canPlayPause"),
            can_next: flag("canNext"),
            can_previous: flag("canPrevious"),
            background: flag("background"),
            art_id: fields
                .get("artId")
                .and_then(|v| v.as_str())
                .and_then(art_id),
        }))
    }
}

/// Whether `command` is one a press may send.
pub fn is_command(command: &str) -> bool {
    COMMANDS.contains(&command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_is_kept_to_one_clean_line() {
        assert_eq!(clean_line(" Song\u{7}\nTitle\u{202e} x "), "Song Title x");
        assert_eq!(clean_line(&"A".repeat(300)).chars().count(), MAX_TEXT);
        assert_eq!(clean_line(""), "");
    }

    #[test]
    fn a_report_is_read_cleaned_and_nothing_is_nothing() {
        assert_eq!(
            MediaReport::from_body(&json!({})),
            Some(MediaReport::Nothing)
        );
        let report = MediaReport::from_body(&json!({
            "title": "Song\nOne", "artist": "Artist", "app": "Spotify",
            "isPlaying": true, "canPlayPause": true, "canNext": true,
        }));
        assert_eq!(
            report,
            Some(MediaReport::Playing(PhoneMedia {
                title: "Song One".to_string(),
                artist: "Artist".to_string(),
                app: "Spotify".to_string(),
                is_playing: true,
                can_play_pause: true,
                can_next: true,
                can_previous: false,
                background: false,
                art_id: None,
            }))
        );
        for bad in [
            json!({"isPlaying": true}),
            json!({"title": "x"}),
            json!({"title": 5, "isPlaying": true}),
            json!([]),
            json!("x"),
        ] {
            assert_eq!(MediaReport::from_body(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_background_report_and_its_cover_s_id_are_read() {
        let Some(MediaReport::Playing(media)) = MediaReport::from_body(&json!({
            "title": "Song", "isPlaying": false, "background": true,
            "artId": "0123456789abcdef",
        })) else {
            panic!("a report");
        };
        assert!(media.background);
        assert_eq!(media.art_id.as_deref(), Some("0123456789abcdef"));
        for bad in [
            "0123456789ABCDEF",
            "0123",
            "../../etc/passwdx",
            "0123456789abcdeg",
            "",
        ] {
            let Some(MediaReport::Playing(media)) = MediaReport::from_body(&json!({
                "title": "Song", "isPlaying": true, "artId": bad,
            })) else {
                panic!("a report");
            };
            assert_eq!(media.art_id, None, "{bad}");
        }
    }

    #[test]
    fn only_the_three_commands_are_presses() {
        for good in COMMANDS {
            assert!(is_command(good));
        }
        for bad in ["stop", "PlayPause", "", "seek"] {
            assert!(!is_command(bad), "{bad}");
        }
    }
}
