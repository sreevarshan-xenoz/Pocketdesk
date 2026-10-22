//! K7 on Linux (N10): a phone's player as an MPRIS player on the session
//! bus, `org.mpris.MediaPlayer2.pocketdesk_phone`, so the desktop's media
//! controls (GNOME's and KDE's, `playerctl`, the keyboard's media keys
//! where the desktop routes them to MPRIS) show the phone's song with its
//! cover, and a press goes to the phone as the Sessions page's does. The
//! tray program holds it, with Settings' `phone_media_controls` on. It
//! serves on a thread of its own, with a small runtime for zbus; the tray
//! tells it what to show.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use pocketdesk_core::local::phone_media::PhoneMediaState;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::{Button, Choice, command};

/// The player's name on the session bus.
const BUS_NAME: &str = "org.mpris.MediaPlayer2.pocketdesk_phone";

/// Where MPRIS players are served.
const PATH: &str = "/org/mpris/MediaPlayer2";

/// What the player shows: the phone, its song and cover, or nothing.
#[derive(Debug, Clone, Default, PartialEq)]
struct Shown {
    phone: String,
    title: String,
    artist: String,
    app: String,
    playing: bool,
    can_play_pause: bool,
    can_next: bool,
    can_previous: bool,
    /// The cover as a `file://` URL.
    art: Option<String>,
}

type Current = Arc<Mutex<Option<Shown>>>;

/// `org.mpris.MediaPlayer2`: who the player is. It cannot be raised or quit.
struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Pocketdesk (the phone's music)".to_string()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// `org.mpris.MediaPlayer2.Player`: the song, and the presses.
struct Player {
    current: Current,
    state_dir: PathBuf,
    /// The player's own thread, told of a press's change so it is said at
    /// once (N10 review); weak, so the tray letting go still ends it.
    said: tokio::sync::mpsc::WeakUnboundedSender<()>,
}

impl Player {
    fn shown(&self) -> Option<Shown> {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Send `button`'s command to the phone shown, as the Sessions page's
    /// buttons do; a play or pause it sent is shown at once.
    fn press(&self, button: Button) {
        let Some(shown) = self.shown() else {
            return;
        };
        let Some(command) = command(button, shown.playing) else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Err(e) = crate::model::press_phone_media(&self.state_dir, &shown.phone, command, now)
        {
            eprintln!("Pocketdesk tray: a media press was not sent: {e}");
            return;
        }
        if command == "playPause" {
            let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(shown) = current.as_mut() {
                shown.playing = !shown.playing;
            }
            drop(current);
            if let Some(said) = self.said.upgrade() {
                let _ = said.send(());
            }
        }
    }
}

/// A string as an MPRIS value.
fn value(text: &str) -> OwnedValue {
    OwnedValue::try_from(Value::from(text)).unwrap_or_else(|_| OwnedValue::from(0u8))
}

/// The metadata MPRIS has for `shown`: a track id, its title, artist, album
/// (the phone's app) and cover.
fn metadata(shown: Option<&Shown>) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let Some(shown) = shown else {
        return map;
    };
    if let Ok(path) = ObjectPath::try_from("/dev/pocketdesk/phone/track")
        && let Ok(id) = OwnedValue::try_from(Value::from(path))
    {
        map.insert("mpris:trackid".to_string(), id);
    }
    // A player that names no song is named by its app.
    let title = if shown.title.is_empty() {
        &shown.app
    } else {
        &shown.title
    };
    map.insert("xesam:title".to_string(), value(title));
    if !shown.artist.is_empty()
        && let Ok(artists) = OwnedValue::try_from(Value::from(vec![shown.artist.clone()]))
    {
        map.insert("xesam:artist".to_string(), artists);
    }
    map.insert("xesam:album".to_string(), value(&shown.app));
    if let Some(art) = &shown.art {
        map.insert("mpris:artUrl".to_string(), value(art));
    }
    map
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play_pause(&self) {
        let playing = self.shown().is_some_and(|s| s.playing);
        self.press(if playing { Button::Pause } else { Button::Play });
    }

    fn play(&self) {
        self.press(Button::Play);
    }

    fn pause(&self) {
        self.press(Button::Pause);
    }

    fn stop(&self) {
        self.press(Button::Pause);
    }

    fn next(&self) {
        self.press(Button::Next);
    }

    fn previous(&self) {
        self.press(Button::Previous);
    }

    fn seek(&self, _offset: i64) {}

    fn set_position(&self, _track: ObjectPath<'_>, _position: i64) {}

    fn open_uri(&self, _uri: String) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match self.shown() {
            Some(s) if s.playing => "Playing",
            Some(_) => "Paused",
            None => "Stopped",
        }
        .to_string()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(self.shown().as_ref())
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.shown().is_some_and(|s| s.can_play_pause)
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.shown().is_some_and(|s| s.can_play_pause)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.shown().is_some_and(|s| s.can_next)
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.shown().is_some_and(|s| s.can_previous)
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        0
    }
}

/// The player's thread: it serves until the tray lets go of the sender.
fn serve(
    state_dir: PathBuf,
    current: Current,
) -> Result<tokio::sync::mpsc::UnboundedSender<()>, String> {
    let (tell, mut told) = tokio::sync::mpsc::unbounded_channel::<()>();
    let said = tell.clone();
    let (ready, started) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("mpris-player".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let player = Player {
                    current: current.clone(),
                    state_dir,
                    said: said.downgrade(),
                };
                drop(said);
                let connection = async {
                    zbus::connection::Builder::session()?
                        .serve_at(PATH, Root)?
                        .serve_at(PATH, player)?
                        .build()
                        .await
                }
                .await;
                let connection = match connection {
                    Ok(connection) => connection,
                    Err(e) => {
                        let _ = ready.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = ready.send(Ok(()));
                let Ok(player) = connection
                    .object_server()
                    .interface::<_, Player>(PATH)
                    .await
                else {
                    return;
                };
                // The name is held only while a phone's song shows (N10
                // review: a stopped player listed first took playerctl's
                // presses, and crowded KDE's list).
                let mut named = false;
                // Each change said, so the desktop's controls follow it.
                while told.recv().await.is_some() {
                    let showing = current.lock().unwrap_or_else(|e| e.into_inner()).is_some();
                    if showing && !named {
                        named = connection.request_name(BUS_NAME).await.is_ok();
                    } else if !showing && named {
                        let _ = connection.release_name(BUS_NAME).await;
                        named = false;
                    }
                    let emitter = player.signal_emitter();
                    let player = player.get().await;
                    let _ = player.playback_status_changed(emitter).await;
                    let _ = player.metadata_changed(emitter).await;
                    let _ = player.can_play_changed(emitter).await;
                    let _ = player.can_pause_changed(emitter).await;
                    let _ = player.can_go_next_changed(emitter).await;
                    let _ = player.can_go_previous_changed(emitter).await;
                }
            });
        })
        .map_err(|e| e.to_string())?;
    match started.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(tell),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the session bus did not answer in time".to_string()),
    }
}

/// The player as Settings has it (K7): read again when the config changes,
/// served while on, gone when off.
#[derive(Default)]
pub struct Watch {
    stamp: Option<SystemTime>,
    read: bool,
    on: bool,
    current: Current,
    /// The phone's last report as shown, kept apart from `current`, which a
    /// press changes ahead of the phone (N10 review: a press was undone by
    /// the next look, until the phone's own report came).
    report: Option<Shown>,
    tell: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    /// When a player that could not serve was last tried: a minute later
    /// again (no session bus yet at sign-in).
    failed: Option<std::time::Instant>,
    choice: Choice,
}

/// How long a player that failed waits before it is tried again.
const RETRY: std::time::Duration = std::time::Duration::from_secs(60);

impl Watch {
    /// Follow the setting, and show what the phones `in_session`, or
    /// `linked` with a background report (N2), play at `now_unix`.
    pub fn follow(
        &mut self,
        state_dir: &Path,
        phones: &std::collections::BTreeMap<String, PhoneMediaState>,
        in_session: &[&str],
        linked: &[&str],
        now_unix: u64,
    ) {
        let stamp = pocketdesk_core::local::config::Config::path()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if !self.read || stamp != self.stamp {
            // A config that does not read keeps the player as it is.
            if let Ok(config) = pocketdesk_core::local::config::Config::load() {
                if config.phone_media_controls != self.on {
                    self.failed = None;
                }
                self.on = config.phone_media_controls;
                self.read = true;
            }
            self.stamp = stamp;
        }
        if !self.on {
            // Its sender gone, its thread ends and the name is let go.
            self.tell = None;
            self.report = None;
            *self.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        }
        let shown = self
            .choice
            .pick(phones, in_session, linked, now_unix)
            .map(|(id, state)| shown(state_dir, id, state));
        let mut new = false;
        if self.tell.is_none() && self.failed.is_none_or(|at| at.elapsed() >= RETRY) {
            match serve(state_dir.to_path_buf(), self.current.clone()) {
                Ok(tell) => {
                    self.tell = Some(tell);
                    self.failed = None;
                    new = true;
                }
                Err(e) => {
                    eprintln!("Pocketdesk tray: no media controls ({e})");
                    self.failed = Some(std::time::Instant::now());
                }
            }
        }
        let changed = self.report != shown;
        if changed {
            self.report = shown.clone();
            *self.current.lock().unwrap_or_else(|e| e.into_inner()) = shown;
        }
        if (changed || new)
            && let Some(tell) = &self.tell
            && tell.send(()).is_err()
        {
            // Its thread ended (the bus went away): served again next time.
            self.tell = None;
        }
    }
}

/// What the player shows for `phone`'s `state`, its cover found in
/// `state_dir`.
fn shown(state_dir: &Path, phone: &str, state: &PhoneMediaState) -> Shown {
    let art = state
        .art
        .as_deref()
        .and_then(|name| pocketdesk_core::local::phone_media::art_path(state_dir, name))
        .map(|path| file_url(&path));
    Shown {
        phone: phone.to_string(),
        title: state.media.title.clone(),
        artist: state.media.artist.clone(),
        app: state.media.app.clone(),
        playing: state.media.is_playing,
        can_play_pause: state.media.can_play_pause,
        can_next: state.media.can_next,
        can_previous: state.media.can_previous,
        art,
    }
}

/// `path` as a `file://` URL, its bytes outside a path's plain ones
/// percent-encoded (N10 review: a space or `#` in the state folder made
/// the cover's URL one no shell read).
fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            url.push(char::from(byte));
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metadata_names_the_song_or_its_app_and_its_cover() {
        let song = Shown {
            phone: "p".to_string(),
            title: String::new(),
            artist: "Artist".to_string(),
            app: "Music".to_string(),
            art: Some("file:///tmp/cover.jpg".to_string()),
            ..Default::default()
        };
        let map = metadata(Some(&song));
        assert_eq!(map["xesam:title"], value("Music"), "no title: its app");
        assert_eq!(map["mpris:artUrl"], value("file:///tmp/cover.jpg"));
        assert!(map.contains_key("mpris:trackid") && map.contains_key("xesam:artist"));
        assert!(metadata(None).is_empty());
    }

    #[test]
    fn a_cover_url_is_percent_encoded() {
        assert_eq!(
            file_url(Path::new("/home/a b/#state/c.jpg")),
            "file:///home/a%20b/%23state/c.jpg"
        );
    }
}
