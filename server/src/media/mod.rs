//! The computer's media for the phone (roadmap T8c): what is playing, and
//! Play/Pause, Next and Previous, answered on the `kdeconnect.pocketdesk.audio`
//! request (`{action: "media"}`, `{action: "mediaControl", command}`), so the
//! Audio permission covers it.
//!
//! Every function blocks; callers use `spawn_blocking`. Windows reads the
//! Global System Media Transport Controls (`windows.rs`); elsewhere MPRIS
//! through `playerctl` (`playerctl.rs`).

use pocketdesk_core::protocol::phone_media::clean_line as clean;
use serde_json::{Value, json};

#[cfg(not(windows))]
mod playerctl;
#[cfg(not(windows))]
use playerctl as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// A test seam (N5): with `POCKETDESK_MEDIA_FILE` set, what plays is read
/// from that JSON file (`{title, artist, app, isPlaying, art}`, `art` a
/// picture's path) and presses are written beside it (`<file>.presses`, a
/// line each), so a device check never touches this computer's players.
fn seam() -> Option<std::path::PathBuf> {
    std::env::var_os("POCKETDESK_MEDIA_FILE").map(std::path::PathBuf::from)
}

fn seam_value(file: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// What is playing now, or `None` when nothing is.
pub fn now_playing() -> Result<Option<NowPlaying>, String> {
    let Some(file) = seam() else {
        return platform::now_playing();
    };
    Ok(seam_value(&file).map(|v| {
        let text = |key: &str| {
            v.get(key)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string()
        };
        NowPlaying {
            title: text("title"),
            artist: text("artist"),
            app: text("app"),
            is_playing: v
                .get("isPlaying")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            can_play_pause: true,
            can_next: true,
            can_previous: true,
        }
    }))
}

/// Ask the player to play or pause, or skip.
pub fn command(command: Command) -> Result<(), String> {
    let Some(file) = seam() else {
        return platform::command(command);
    };
    use std::io::Write;
    let name = match command {
        Command::PlayPause => "playPause",
        Command::Next => "next",
        Command::Previous => "previous",
    };
    let mut presses = file.as_os_str().to_owned();
    presses.push(".presses");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::PathBuf::from(presses))
        .and_then(|mut f| writeln!(f, "{name}"))
        .map_err(|e| e.to_string())
}

/// The largest cover sent, in bytes.
pub const MAX_COVER: usize = 256 << 10;

/// The playing track's cover, if it has one: a JPEG or a PNG by its header,
/// at most [`MAX_COVER`] (N5).
pub fn cover() -> Option<Vec<u8>> {
    let bytes = match seam() {
        Some(file) => {
            let path = seam_value(&file)?.get("art")?.as_str()?.to_string();
            std::fs::read(path).ok()?
        }
        None => platform::cover()?,
    };
    let picture =
        bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    (picture && bytes.len() <= MAX_COVER).then_some(bytes)
}

/// The longest a player is waited for: the phone's control connection waits
/// on the answer, so a player that hangs must not hold it.
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(3);

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
    use pocketdesk_core::protocol::phone_media::MAX_TEXT;

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
            title: " Song\u{7}\nTitle\u{202e} x ".to_string(),
            artist: "A".repeat(300),
            app: "Spotify.exe".to_string(),
            is_playing: true,
            can_play_pause: true,
            can_next: false,
            can_previous: true,
        };
        let sent = answer(Some(playing));
        assert_eq!(sent["ok"], true);
        assert_eq!(
            sent["playing"]["title"], "Song Title x",
            "lines become spaces"
        );
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
