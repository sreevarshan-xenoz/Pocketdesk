//! The computer's media for the phone (roadmap T8c): what is playing, and
//! Play/Pause, Next and Previous, answered on the `kdeconnect.pocketdesk.audio`
//! request (`{action: "media"}`, `{action: "mediaControl", command}`), so the
//! Audio permission covers it.
//!
//! Every function blocks; callers use `spawn_blocking`. Windows reads the
//! Global System Media Transport Controls (`windows.rs`); elsewhere MPRIS
//! through `playerctl` (`playerctl.rs`).

use serde_json::{Value, json};

#[cfg(not(windows))]
mod playerctl;
#[cfg(not(windows))]
pub use playerctl::{command, now_playing};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{command, now_playing};

/// The longest title, artist or app name sent, in characters.
const MAX_TEXT: usize = 200;

/// The media session the system names as current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    /// The app playing it, as the system names it.
    pub app: String,
    pub is_playing: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
}

/// What the phone may ask the player to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
}

impl Command {
    /// The command a request names, or `None` for any other.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "playPause" => Some(Self::PlayPause),
            "next" => Some(Self::Next),
            "previous" => Some(Self::Previous),
            _ => None,
        }
    }
}

/// `text` without control characters, trimmed, at most [`MAX_TEXT`]
/// characters.
pub fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_TEXT)
        .collect()
}

/// The answer to `{action: "media"}`: `{ok, playing}`, `playing` null when
/// nothing is, its text cleaned.
pub fn answer(playing: Option<NowPlaying>) -> Value {
    let playing = playing.map(|p| {
        json!({
            "title": clean(&p.title),
            "artist": clean(&p.artist),
            "app": clean(&p.app),
            "isPlaying": p.is_playing,
            "canPlayPause": p.can_play_pause,
            "canNext": p.can_next,
            "canPrevious": p.can_previous,
        })
    });
    json!({ "ok": true, "playing": playing })
}

/// One line of `playerctl metadata --format` with [`PLAYERCTL_FORMAT`]:
/// status, title, artist and player, split by U+001F. `playerctl` cannot
/// say which buttons a player takes, so all are offered.
pub fn parse_playerctl(output: &str) -> Option<NowPlaying> {
    let line = output.lines().next()?;
    let mut fields = line.split(char::from(0x1f));
    let status = fields.next()?;
    let (title, artist, app) = (fields.next()?, fields.next()?, fields.next()?);
    Some(NowPlaying {
        title: title.to_string(),
        artist: artist.to_string(),
        app: app.to_string(),
        is_playing: status == "Playing",
        can_play_pause: true,
        can_next: true,
        can_previous: true,
    })
}

/// The format `playerctl metadata` is asked for (U+001F between fields).
pub const PLAYERCTL_FORMAT: &str = "{{status}}\u{1f}{{title}}\u{1f}{{artist}}\u{1f}{{playerName}}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_three_commands_are_taken() {
        assert_eq!(Command::parse("playPause"), Some(Command::PlayPause));
        assert_eq!(Command::parse("next"), Some(Command::Next));
        assert_eq!(Command::parse("previous"), Some(Command::Previous));
        for other in ["stop", "PlayPause", "", "seek", "open"] {
            assert_eq!(Command::parse(other), None, "{other}");
        }
    }

    #[test]
    fn what_is_playing_is_sent_cleaned_and_nothing_is_null() {
        assert_eq!(answer(None), json!({"ok": true, "playing": null}));
        let playing = NowPlaying {
            title: " Song\u{7}\nTitle ".to_string(),
            artist: "A".repeat(300),
            app: "Spotify.exe".to_string(),
            is_playing: true,
            can_play_pause: true,
            can_next: false,
            can_previous: true,
        };
        let sent = answer(Some(playing));
        assert_eq!(sent["ok"], true);
        assert_eq!(sent["playing"]["title"], "SongTitle");
        assert_eq!(
            sent["playing"]["artist"].as_str().unwrap().chars().count(),
            MAX_TEXT
        );
        assert_eq!(sent["playing"]["app"], "Spotify.exe");
        assert_eq!(sent["playing"]["isPlaying"], true);
        assert_eq!(sent["playing"]["canPlayPause"], true);
        assert_eq!(sent["playing"]["canNext"], false);
        assert_eq!(sent["playing"]["canPrevious"], true);
    }

    #[test]
    fn playerctls_answer_is_read_and_no_player_is_nothing() {
        let line = "Playing\u{1f}Song\u{1f}Artist\u{1f}firefox\n";
        assert_eq!(
            parse_playerctl(line),
            Some(NowPlaying {
                title: "Song".to_string(),
                artist: "Artist".to_string(),
                app: "firefox".to_string(),
                is_playing: true,
                can_play_pause: true,
                can_next: true,
                can_previous: true,
            })
        );
        assert_eq!(
            parse_playerctl("Paused\u{1f}S\u{1f}\u{1f}vlc").map(|p| p.is_playing),
            Some(false)
        );
        assert_eq!(parse_playerctl(""), None);
        assert_eq!(parse_playerctl("No players found"), None);
    }
}
