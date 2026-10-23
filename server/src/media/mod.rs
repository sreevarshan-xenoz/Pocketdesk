//! The computer's media for the phone (roadmap T8c): what is playing, and
//! Play/Pause, Next and Previous, answered on the `kdeconnect.pocketdesk.audio`
//! request (`{action: "media"}`, `{action: "mediaControl", command}`), so the
//! Audio permission covers it.
//!
//! Every function blocks; callers use `spawn_blocking`. Windows reads the
//! Global System Media Transport Controls (`windows.rs`); elsewhere MPRIS
//! through `playerctl` (`playerctl.rs`).
//!
//! O6: the `media` answer lists the computer's players, and any request may
//! name one (`player`, an id from that list); one not named, or gone, is
//! the system's own choice, as before.

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
/// line each, followed by the player when one was named), so a device
/// check never touches this computer's players. O6: `players`, a list of
/// the same objects, stands for other players, each known by its `app`.
fn seam() -> Option<std::path::PathBuf> {
    // Absolute only, as the other seams (N5 review).
    std::env::var_os("POCKETDESK_MEDIA_FILE")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn seam_value(file: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// The seam's players: its own (its top level), then its `players`.
fn seam_players(v: &Value) -> Vec<Player> {
    let mut list: Vec<Value> = v
        .get("players")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    if v.get("app").is_some() {
        list.insert(0, v.clone());
    }
    list.iter()
        .map(seam_playing)
        .map(|p| Player {
            id: p.app.clone(),
            name: p.app,
            playing: p.is_playing,
        })
        .collect()
}

/// The seam's player named `player`, else its own (its top level).
fn seam_player(v: &Value, player: Option<&str>) -> Value {
    player
        .and_then(|p| {
            v.get("players")?
                .as_array()?
                .iter()
                .find(|o| o.get("app").and_then(|a| a.as_str()) == Some(p))
                .cloned()
        })
        .unwrap_or_else(|| v.clone())
}

fn seam_playing(v: &Value) -> NowPlaying {
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
        player: text("app"),
        is_playing: v
            .get("isPlaying")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        can_play_pause: true,
        can_next: true,
        can_previous: true,
    }
}

/// One of the computer's players (O6): its id, which a request names it
/// by; the name it goes by, as the system gives it; whether it plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub id: String,
    pub name: String,
    pub playing: bool,
}

/// The most players listed: a phone picks from a short list.
pub const MAX_PLAYERS: usize = 16;

/// Whether `id` may name a player: what the lists hold, short, with no
/// control character and no comma (`playerctl -p` takes a list).
pub fn valid_player(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(|c| c.is_control() || c == ',')
}

/// The player a request names, if it may name one.
pub fn requested_player(body: &Value) -> Option<String> {
    body.get("player")
        .and_then(|p| p.as_str())
        .filter(|p| valid_player(p))
        .map(str::to_string)
}

/// The computer's players, at most [`MAX_PLAYERS`].
pub fn players() -> Result<Vec<Player>, String> {
    let mut all = match seam() {
        None => platform::players()?,
        Some(file) => seam_players(&seam_value(&file).unwrap_or(Value::Null)),
    };
    // Each once (O6 review: the seam's own player is also in its list).
    let mut seen = std::collections::HashSet::new();
    all.retain(|p| valid_player(&p.id) && seen.insert(p.id.clone()));
    all.truncate(MAX_PLAYERS);
    Ok(all)
}

/// What is playing now on `player`, else the system's own; `None` when
/// nothing is.
pub fn now_playing(player: Option<&str>) -> Result<Option<NowPlaying>, String> {
    let Some(file) = seam() else {
        return platform::now_playing(player);
    };
    Ok(seam_value(&file).map(|v| seam_playing(&seam_player(&v, player))))
}

/// Ask `player` (else the system's own) to play or pause, or skip.
pub fn command(command: Command, player: Option<&str>) -> Result<(), String> {
    let Some(file) = seam() else {
        return platform::command(command, player);
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
        .and_then(|mut f| match player {
            Some(p) => writeln!(f, "{name} {p}"),
            None => writeln!(f, "{name}"),
        })
        .map_err(|e| e.to_string())
}

/// The largest cover sent, in bytes.
pub const MAX_COVER: usize = 256 << 10;

/// The playing track's cover, if it has one: a JPEG or a PNG by its header,
/// at most [`MAX_COVER`] (N5); `player`'s, else the system's own player's.
pub fn cover(player: Option<&str>) -> Option<Vec<u8>> {
    let bytes = match seam() {
        Some(file) => {
            let v = seam_player(&seam_value(&file)?, player);
            let path = v.get("art")?.as_str()?.to_string();
            std::fs::read(path).ok()?
        }
        None => platform::cover(player)?,
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
    /// The player's id, as [`players`] lists it (O6).
    pub player: String,
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
            "player": p.player,
            "isPlaying": p.is_playing,
            "canPlayPause": p.can_play_pause,
            "canNext": p.can_next,
            "canPrevious": p.can_previous,
        })
    });
    json!({ "ok": true, "playing": playing })
}

/// `answer` with the computer's players beside it (O6): `players`, each
/// `{id, name, isPlaying}`, the name cleaned.
pub fn with_players(mut answer: Value, players: &[Player]) -> Value {
    answer["players"] = players
        .iter()
        .map(|p| json!({ "id": p.id, "name": clean(&p.name), "isPlaying": p.playing }))
        .collect();
    answer
}

/// One line of `playerctl metadata --format` with [`PLAYERCTL_FORMAT`]:
/// status, title, artist, player and its instance, split by U+001F.
/// `playerctl` cannot say which buttons a player takes, so all are offered.
pub fn parse_playerctl(output: &str) -> Option<NowPlaying> {
    let line = output.lines().next()?;
    let mut fields = line.split(char::from(0x1f));
    let status = fields.next()?;
    let (title, artist, app) = (fields.next()?, fields.next()?, fields.next()?);
    let instance = fields.next().filter(|i| !i.is_empty()).unwrap_or(app);
    Some(NowPlaying {
        title: title.to_string(),
        artist: artist.to_string(),
        app: app.to_string(),
        player: instance.to_string(),
        is_playing: status == "Playing",
        can_play_pause: true,
        can_next: true,
        can_previous: true,
    })
}

/// The format `playerctl metadata` is asked for (U+001F between fields).
pub const PLAYERCTL_FORMAT: &str =
    "{{status}}\u{1f}{{title}}\u{1f}{{artist}}\u{1f}{{playerName}}\u{1f}{{playerInstance}}";

/// The format `playerctl -a metadata` is asked for to list the players:
/// each one's instance, name and status (O6).
pub const PLAYERCTL_LIST_FORMAT: &str = "{{playerInstance}}\u{1f}{{playerName}}\u{1f}{{status}}";

/// The players in `playerctl -a metadata --format` with
/// [`PLAYERCTL_LIST_FORMAT`], one a line, each once.
pub fn parse_playerctl_list(output: &str) -> Vec<Player> {
    let mut players: Vec<Player> = Vec::new();
    for line in output.lines() {
        let mut fields = line.split(char::from(0x1f));
        let (Some(id), Some(name), Some(status)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if id.is_empty() || players.iter().any(|p| p.id == id) {
            continue;
        }
        players.push(Player {
            id: id.to_string(),
            name: name.to_string(),
            playing: status == "Playing",
        });
    }
    players
}

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
            player: "Spotify.exe".to_string(),
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
        assert_eq!(sent["playing"]["player"], "Spotify.exe");
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
                player: "firefox".to_string(),
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
        // O6: the instance, when the player has one, is its id.
        assert_eq!(
            parse_playerctl("Playing\u{1f}S\u{1f}A\u{1f}firefox\u{1f}firefox.instance_1_23")
                .map(|p| p.player),
            Some("firefox.instance_1_23".to_string())
        );
    }

    #[test]
    fn the_players_are_listed_once_each() {
        let out = "spotify\u{1f}spotify\u{1f}Playing\n\
                   firefox.instance_1_2\u{1f}firefox\u{1f}Paused\n\
                   spotify\u{1f}spotify\u{1f}Playing\n\
                   broken line\n";
        assert_eq!(
            parse_playerctl_list(out),
            vec![
                Player {
                    id: "spotify".to_string(),
                    name: "spotify".to_string(),
                    playing: true
                },
                Player {
                    id: "firefox.instance_1_2".to_string(),
                    name: "firefox".to_string(),
                    playing: false
                },
            ]
        );
    }

    #[test]
    fn a_player_is_named_by_a_short_plain_id() {
        assert!(valid_player("Spotify.exe"));
        assert!(valid_player(
            "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"
        ));
        assert!(!valid_player(""));
        assert!(!valid_player("spotify,%any"));
        assert!(!valid_player("a\nb"));
        assert!(!valid_player(&"x".repeat(257)));
        assert_eq!(
            requested_player(&json!({"player": "vlc"})),
            Some("vlc".to_string())
        );
        assert_eq!(requested_player(&json!({"player": 3})), None);
        assert_eq!(requested_player(&json!({})), None);
    }

    #[test]
    fn the_seam_stands_for_several_players() {
        let v = json!({
            "title": "Tab", "app": "Chrome", "isPlaying": false,
            "players": [{"title": "Song", "app": "Spotify.exe", "isPlaying": true}]
        });
        let ids: Vec<String> = seam_players(&v).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["Chrome", "Spotify.exe"]);
        assert_eq!(
            seam_playing(&seam_player(&v, Some("Spotify.exe"))).title,
            "Song"
        );
        assert_eq!(
            seam_playing(&seam_player(&v, Some("Gone"))).title,
            "Tab",
            "gone: its own"
        );
        assert_eq!(seam_playing(&seam_player(&v, None)).title, "Tab");
    }

    #[test]
    fn the_answer_lists_the_players() {
        let sent = with_players(
            answer(None),
            &[Player {
                id: "vlc".to_string(),
                name: "VLC\u{202e}".to_string(),
                playing: false,
            }],
        );
        assert_eq!(
            sent["players"],
            json!([{"id": "vlc", "name": "VLC", "isPlaying": false}])
        );
    }
}
