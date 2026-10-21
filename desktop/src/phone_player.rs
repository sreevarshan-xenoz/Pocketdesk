//! A phone's player in this computer's own media controls (K7): while a
//! phone reports what it plays (T8d), Windows' media controls (the volume
//! flyout, the keyboard's media keys) show its song with Previous,
//! Play/Pause and Next, and a press goes to the phone as the Sessions
//! page's does. The tray program holds it, with Settings'
//! `phone_media_controls` on.

use std::collections::BTreeMap;

use pocketdesk_core::local::phone_media::PhoneMediaState;

/// How long a report counts with no word since: the phone says it again
/// each minute while its app is in front, and is silent when it is not
/// (K7 review: a frozen "Playing" whose presses went nowhere).
pub const FRESH_SECS: u64 = 150;

/// How long a paused phone keeps the media controls (K7 review: the
/// owner's own media keys went to a phone paused for good).
pub const PAUSED_SECS: u64 = 300;

/// Whether `state` may show for a phone `in_session` or `linked` at
/// `now_unix` (N2): fresh, and in a session (K7), or linked with a report
/// from the phone's background reporter, which says it again while it
/// plays. The app's own reporter stops with the app, so its last report
/// from a phone no longer in a session is not shown (K7 review's frozen
/// song).
pub fn may_show(state: &PhoneMediaState, in_session: bool, linked: bool, now_unix: u64) -> bool {
    now_unix.saturating_sub(state.updated_unix) <= FRESH_SECS
        && (in_session || (linked && state.media.background))
}

/// Which phone the media controls show, kept from one look to the next.
#[derive(Debug, Default)]
pub struct Choice {
    shown: Option<String>,
    /// Since when each phone has been paused.
    paused_since: std::collections::HashMap<String, u64>,
}

impl Choice {
    /// The phone to show at `now_unix`: one [`may_show`] allows (in a
    /// session, or `linked` with a background report), playing or paused
    /// for at most [`PAUSED_SECS`]. The one shown stays while it qualifies
    /// and nothing plays in its stead (K7 review: two playing phones
    /// swapped each minute); else a playing one first, then the newest.
    pub fn pick<'a>(
        &mut self,
        phones: &'a BTreeMap<String, PhoneMediaState>,
        in_session: &[&str],
        linked: &[&str],
        now_unix: u64,
    ) -> Option<(&'a str, &'a PhoneMediaState)> {
        self.paused_since
            .retain(|id, _| phones.get(id).is_some_and(|s| !s.media.is_playing));
        for (id, state) in phones {
            if !state.media.is_playing {
                self.paused_since.entry(id.clone()).or_insert(now_unix);
            }
        }
        let candidates: Vec<(&str, &PhoneMediaState)> = phones
            .iter()
            .filter(|(id, state)| {
                may_show(
                    state,
                    in_session.contains(&id.as_str()),
                    linked.contains(&id.as_str()),
                    now_unix,
                ) && (state.media.is_playing
                    || self
                        .paused_since
                        .get(*id)
                        .is_some_and(|since| now_unix.saturating_sub(*since) < PAUSED_SECS))
            })
            .map(|(id, state)| (id.as_str(), state))
            .collect();
        let any_playing = candidates.iter().any(|(_, s)| s.media.is_playing);
        let kept = self.shown.as_deref().and_then(|shown| {
            candidates
                .iter()
                .find(|(id, s)| *id == shown && (s.media.is_playing || !any_playing))
        });
        let chosen = kept.or_else(|| {
            candidates
                .iter()
                .max_by_key(|(_, s)| (s.media.is_playing, s.updated_unix))
        });
        self.shown = chosen.map(|(id, _)| id.to_string());
        chosen.map(|(id, state)| (*id, *state))
    }
}

/// A button the media controls pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Play,
    Pause,
    Next,
    Previous,
}

/// The phone's command for `button`, with the phone `playing` or not: its
/// player only toggles, so Play while it plays (or Pause while paused) is
/// nothing, never the opposite.
pub fn command(button: Button, playing: bool) -> Option<&'static str> {
    match button {
        Button::Play if playing => None,
        Button::Pause if !playing => None,
        Button::Play | Button::Pause => Some("playPause"),
        Button::Next => Some("next"),
        Button::Previous => Some("previous"),
    }
}

#[cfg(windows)]
pub use windows_player::{Player, Watch, prepare};

#[cfg(windows)]
mod windows_player {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use pocketdesk_core::local::phone_media::PhoneMediaState;
    use pocketdesk_core::protocol::phone_media::PhoneMedia;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    };
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::{
        ISystemMediaTransportControlsInterop, RO_INIT_SINGLETHREADED, RoInitialize,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
        WS_EX_TOOLWINDOW, WS_OVERLAPPED,
    };
    use windows::core::{HSTRING, w};

    use super::{Button, Choice, command};

    /// The name Windows shows for it: the one the server registers for its
    /// toasts, so the flyout says Pocketdesk, not the program's file; and
    /// the one the server skips reading the computer's music (N5).
    use pocketdesk_core::local::phone_media::MEDIA_CONTROLS_APP_ID as AUMID;

    /// Ready this thread and process for the media controls, before any
    /// window (K7 review): WinRT on the tray's thread, and the name the
    /// flyout shows. Call once, first.
    pub fn prepare() {
        // One already set up makes this a harmless error.
        let _ = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
        let _ = unsafe {
            windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(
                AUMID,
            ))
        };
    }

    /// The phone shown, and whether it plays, for a press.
    type Target = Arc<Mutex<Option<(String, bool)>>>;

    /// A phone's player in Windows' media controls, on a hidden window of
    /// its own. Dropped, it leaves them.
    pub struct Player {
        hwnd: HWND,
        smtc: SystemMediaTransportControls,
        token: i64,
        target: Target,
        /// A press changed what shows: the next report is applied anew.
        stale: Arc<std::sync::atomic::AtomicBool>,
        shown: Option<(String, PhoneMedia, Option<String>)>,
        /// Where the covers are (N2).
        state_dir: PathBuf,
    }

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    impl Player {
        /// Register with the media controls; a press goes to the shown
        /// phone through `state_dir`'s `phone_media_out/`.
        pub fn new(state_dir: PathBuf) -> windows::core::Result<Self> {
            let covers = state_dir.clone();
            let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: w!("PocketdeskPhonePlayer"),
                ..Default::default()
            };
            // A second player registers it again, which fails harmlessly.
            unsafe { RegisterClassW(&class) };
            // Never shown: the media controls need a window to belong to.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    w!("PocketdeskPhonePlayer"),
                    w!("Pocketdesk"),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance),
                    None,
                )
            }?;
            let made = (|| {
                let interop = windows::core::factory::<
                    SystemMediaTransportControls,
                    ISystemMediaTransportControlsInterop,
                >()?;
                let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd) }?;
                let target: Target = Arc::default();
                let pressed = Arc::clone(&target);
                let stale = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let restale = Arc::clone(&stale);
                let token = smtc.ButtonPressed(&TypedEventHandler::<
                    SystemMediaTransportControls,
                    SystemMediaTransportControlsButtonPressedEventArgs,
                >::new(move |smtc, args| {
                    let Some(args) = args.as_ref() else {
                        return Ok(());
                    };
                    let button = match args.Button()? {
                        SystemMediaTransportControlsButton::Play => Button::Play,
                        SystemMediaTransportControlsButton::Pause => Button::Pause,
                        SystemMediaTransportControlsButton::Next => Button::Next,
                        SystemMediaTransportControlsButton::Previous => Button::Previous,
                        _ => return Ok(()),
                    };
                    // Shown at once as the press leaves it (K7 review: the
                    // phone's word takes seconds, and a second press meanwhile
                    // undid the first); the phone's next report is applied
                    // even if it says the same.
                    if let Some(playing) = press(&state_dir, &pressed, button)
                        && let Some(smtc) = smtc.as_ref()
                    {
                        let _ = smtc.SetPlaybackStatus(if playing {
                            MediaPlaybackStatus::Playing
                        } else {
                            MediaPlaybackStatus::Paused
                        });
                    }
                    restale.store(true, std::sync::atomic::Ordering::Relaxed);
                    Ok(())
                }))?;
                smtc.SetIsEnabled(false)?;
                Ok::<_, windows::core::Error>((smtc, token, target, stale))
            })();
            match made {
                Ok((smtc, token, target, stale)) => Ok(Self {
                    hwnd,
                    smtc,
                    token,
                    target,
                    stale,
                    shown: None,
                    state_dir: covers,
                }),
                Err(e) => {
                    let _ = unsafe { DestroyWindow(hwnd) };
                    Err(e)
                }
            }
        }

        /// Show `now`'s phone and what it plays, with its cover (N2), or
        /// nothing.
        pub fn show(&mut self, now: Option<(&str, &PhoneMediaState)>) -> windows::core::Result<()> {
            let now =
                now.map(|(id, state)| (id.to_string(), state.media.clone(), state.art.clone()));
            let stale = self.stale.swap(false, std::sync::atomic::Ordering::Relaxed);
            if now == self.shown && !stale {
                return Ok(());
            }
            *self.target.lock().unwrap_or_else(|e| e.into_inner()) = now
                .as_ref()
                .map(|(id, media, _)| (id.clone(), media.is_playing));
            let updater = self.smtc.DisplayUpdater()?;
            match &now {
                None => {
                    updater.ClearAll()?;
                    updater.Update()?;
                    self.smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
                    self.smtc.SetIsEnabled(false)?;
                }
                Some((_, media, art)) => {
                    // From nothing, so a cover gone does not stay.
                    updater.ClearAll()?;
                    updater.SetType(MediaPlaybackType::Music)?;
                    let cover = art.as_deref().and_then(|name| {
                        pocketdesk_core::local::phone_media::art_path(&self.state_dir, name)
                    });
                    if let Some(path) = cover {
                        // A cover that will not load leaves the song shown.
                        match thumbnail(&path) {
                            Ok(cover) => updater.SetThumbnail(&cover)?,
                            Err(e) => eprintln!("Pocketdesk tray: a cover was not shown ({e})"),
                        }
                    }
                    let music = updater.MusicProperties()?;
                    // A player that names no song is named by its app.
                    let title = if media.title.is_empty() {
                        &media.app
                    } else {
                        &media.title
                    };
                    music.SetTitle(&HSTRING::from(title.as_str()))?;
                    music.SetArtist(&HSTRING::from(media.artist.as_str()))?;
                    updater.Update()?;
                    self.smtc.SetIsPlayEnabled(media.can_play_pause)?;
                    self.smtc.SetIsPauseEnabled(media.can_play_pause)?;
                    self.smtc.SetIsNextEnabled(media.can_next)?;
                    self.smtc.SetIsPreviousEnabled(media.can_previous)?;
                    self.smtc.SetPlaybackStatus(if media.is_playing {
                        MediaPlaybackStatus::Playing
                    } else {
                        MediaPlaybackStatus::Paused
                    })?;
                    self.smtc.SetIsEnabled(true)?;
                }
            }
            self.shown = now;
            Ok(())
        }
    }

    /// The cover at `path` as the media controls take one.
    fn thumbnail(path: &Path) -> windows::core::Result<RandomAccessStreamReference> {
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
        RandomAccessStreamReference::CreateFromFile(&file)
    }

    impl Drop for Player {
        fn drop(&mut self) {
            let _ = self.show(None);
            let _ = self.smtc.RemoveButtonPressed(self.token);
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Send `button`'s command to the phone `target` names, as the Sessions
    /// page's buttons do (held to its Media permission there); whether it
    /// plays after a play or pause sent, which `target` then says.
    fn press(state_dir: &Path, target: &Target, button: Button) -> Option<bool> {
        let mut target = target.lock().unwrap_or_else(|e| e.into_inner());
        let (phone, playing) = target.clone()?;
        let command = command(button, playing)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Err(e) = crate::model::press_phone_media(state_dir, &phone, command, now) {
            eprintln!("Pocketdesk tray: a media press was not sent: {e}");
            return None;
        }
        (command == "playPause").then(|| {
            *target = Some((phone, !playing));
            !playing
        })
    }

    /// The player as Settings has it (K7): read again when the config
    /// changes, made while on, dropped when off.
    #[derive(Default)]
    pub struct Watch {
        stamp: Option<SystemTime>,
        read: bool,
        on: bool,
        player: Option<Player>,
        /// When a player Windows would not make was last tried: tried
        /// again a minute later (K7 review: one failure at sign-in, before
        /// the shell was up, had ended it until the setting changed).
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
                self.player = None;
                return;
            }
            let shown = self.choice.pick(phones, in_session, linked, now_unix);
            if self.player.is_none() && self.failed.is_none_or(|at| at.elapsed() >= RETRY) {
                match Player::new(state_dir.to_path_buf()) {
                    Ok(player) => {
                        self.player = Some(player);
                        self.failed = None;
                    }
                    Err(e) => {
                        eprintln!("Pocketdesk tray: no media controls ({e})");
                        self.failed = Some(std::time::Instant::now());
                    }
                }
            }
            if let Some(player) = &mut self.player
                && let Err(e) = player.show(shown)
            {
                eprintln!("Pocketdesk tray: the media controls did not update ({e})");
                self.player = None;
                self.failed = Some(std::time::Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_media::PhoneMedia;

    fn state(title: &str, playing: bool, at: u64) -> PhoneMediaState {
        PhoneMediaState {
            media: PhoneMedia {
                title: title.to_string(),
                artist: "Artist".to_string(),
                app: "Music".to_string(),
                is_playing: playing,
                can_play_pause: true,
                can_next: true,
                can_previous: false,
                ..Default::default()
            },
            updated_unix: at,
            art: None,
        }
    }

    /// K7: a playing phone comes before a paused one, then the newest, of
    /// the phones in a session.
    #[test]
    fn the_playing_phone_is_shown_first() {
        let mut phones = BTreeMap::new();
        let all = ["a", "b", "c"];
        assert_eq!(Choice::default().pick(&phones, &all, &[], 30), None);
        phones.insert("a".to_string(), state("paused, newest", false, 30));
        phones.insert("b".to_string(), state("playing, older", true, 10));
        phones.insert("c".to_string(), state("playing, newer", true, 20));
        assert_eq!(
            Choice::default()
                .pick(&phones, &all, &[], 30)
                .map(|(id, _)| id),
            Some("c")
        );
        assert_eq!(
            Choice::default()
                .pick(&phones, &["a", "b"], &[], 30)
                .map(|(id, _)| id),
            Some("b"),
            "only phones in a session (K7 review)"
        );
        phones.remove("c");
        phones.remove("b");
        assert_eq!(
            Choice::default()
                .pick(&phones, &all, &[], 30)
                .map(|(id, _)| id),
            Some("a")
        );
    }

    /// K7 review: a report not said again is gone, and a paused phone
    /// keeps its place five minutes.
    #[test]
    fn stale_and_long_paused_phones_go() {
        let mut phones = BTreeMap::new();
        phones.insert("a".to_string(), state("song", true, 1_000));
        let mut choice = Choice::default();
        assert!(
            choice
                .pick(&phones, &["a"], &[], 1_000 + FRESH_SECS)
                .is_some()
        );
        assert_eq!(
            choice.pick(&phones, &["a"], &[], 1_001 + FRESH_SECS),
            None,
            "stale"
        );
        phones.insert("a".to_string(), state("song", false, 2_000));
        assert!(choice.pick(&phones, &["a"], &[], 2_000).is_some());
        phones.insert("a".to_string(), state("song", false, 2_000 + 120));
        assert!(choice.pick(&phones, &["a"], &[], 2_000 + 120).is_some());
        phones.insert("a".to_string(), state("song", false, 2_000 + PAUSED_SECS));
        assert_eq!(
            choice.pick(&phones, &["a"], &[], 2_000 + PAUSED_SECS),
            None,
            "paused too long"
        );
        phones.insert(
            "a".to_string(),
            state("song", true, 2_000 + PAUSED_SECS + 10),
        );
        assert!(
            choice
                .pick(&phones, &["a"], &[], 2_000 + PAUSED_SECS + 10)
                .is_some(),
            "playing again"
        );
    }

    /// K7 review: the phone shown stays while it plays, whichever said so
    /// last.
    #[test]
    fn the_phone_shown_stays_while_it_plays() {
        let mut phones = BTreeMap::new();
        phones.insert("a".to_string(), state("one", true, 10));
        phones.insert("b".to_string(), state("two", true, 20));
        let mut choice = Choice::default();
        assert_eq!(
            choice.pick(&phones, &["a", "b"], &[], 20).map(|(id, _)| id),
            Some("b")
        );
        phones.insert("a".to_string(), state("one", true, 70));
        assert_eq!(
            choice.pick(&phones, &["a", "b"], &[], 70).map(|(id, _)| id),
            Some("b")
        );
        phones.insert("b".to_string(), state("two", false, 75));
        assert_eq!(
            choice.pick(&phones, &["a", "b"], &[], 75).map(|(id, _)| id),
            Some("a"),
            "a playing one before the shown one paused"
        );
    }

    /// N2: a linked phone shows with a background report only, and a
    /// stale one never.
    #[test]
    fn a_linked_phone_shows_with_a_background_report() {
        let mut background = state("song", true, 100);
        background.media.background = true;
        let in_app = state("song", true, 100);
        assert!(may_show(&in_app, true, false, 100), "in a session, as K7");
        assert!(may_show(&background, false, true, 100));
        assert!(
            !may_show(&in_app, false, true, 100),
            "the app's own report outlives its session"
        );
        assert!(!may_show(&background, false, false, 100), "away");
        assert!(
            !may_show(&background, false, true, 101 + FRESH_SECS),
            "stale"
        );
        let mut phones = BTreeMap::new();
        phones.insert("a".to_string(), background);
        assert_eq!(
            Choice::default()
                .pick(&phones, &[], &["a"], 100)
                .map(|(id, _)| id),
            Some("a")
        );
    }

    /// K7: a press toggles only when it would change something.
    #[test]
    fn a_button_is_the_phone_s_command() {
        assert_eq!(command(Button::Pause, true), Some("playPause"));
        assert_eq!(command(Button::Play, false), Some("playPause"));
        assert_eq!(command(Button::Play, true), None);
        assert_eq!(command(Button::Pause, false), None);
        assert_eq!(command(Button::Next, false), Some("next"));
        assert_eq!(command(Button::Previous, true), Some("previous"));
    }

    /// Live (K7): a probe phone's song shows in Windows' media controls, as
    /// another program reads them, and goes. Shows a session in the volume
    /// flyout for a few seconds, with no sound.
    /// `cargo test -p pocketdesk-desktop --lib phone_player -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "registers with this computer's media controls"]
    fn a_phone_s_song_shows_in_the_media_controls() {
        use windows::Media::Control::{
            GlobalSystemMediaTransportControlsSessionManager as Manager,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
        };
        // A WinRT call's result, waited for at most a second.
        fn done<T: windows::core::RuntimeType + 'static>(
            operation: windows_future::IAsyncOperation<T>,
        ) -> Option<T> {
            for _ in 0..100 {
                if operation.Status().ok()? != windows_future::AsyncStatus::Started {
                    return operation.GetResults().ok();
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = operation.Cancel();
            None
        }
        let dir = std::env::temp_dir().join(format!("pd-k7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        prepare();
        let mut player = Player::new(dir.clone()).expect("a player");
        // N2: with a cover, kept as the server keeps one.
        let mut cover = Vec::new();
        image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]))
            .write_to(
                &mut std::io::Cursor::new(&mut cover),
                image::ImageFormat::Jpeg,
            )
            .expect("a JPEG");
        let id: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(&cover)[..8]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let name = pocketdesk_core::local::phone_media::keep_art(&dir, "probe", &id, &cover)
            .expect("the cover kept");
        let mut probe = state("K7 probe song", true, 1);
        probe.art = Some(name);
        let mut phones = BTreeMap::new();
        phones.insert("probe".to_string(), probe);
        player
            .show(Choice::default().pick(&phones, &["probe"], &[], 1))
            .expect("shown");
        let find = || -> Option<(String, Status, bool, windows::Media::Control::GlobalSystemMediaTransportControlsSession)> {
            let manager = done(Manager::RequestAsync().ok()?)?;
            let sessions = manager.GetSessions().ok()?;
            for session in sessions {
                let props = done(session.TryGetMediaPropertiesAsync().ok()?)?;
                if props.Title().ok()? == "K7 probe song" {
                    let status = session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()?;
                    let source = session.SourceAppUserModelId().ok()?.to_string();
                    let thumbnail = props.Thumbnail().is_ok();
                    return Some((source, status, thumbnail, session));
                }
            }
            None
        };
        let mut seen = None;
        for _ in 0..30 {
            seen = find();
            if seen.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let (source, status, thumbnail, session) = seen.expect("the song in the media controls");
        eprintln!("shown by {source}");
        assert_eq!(status, Status::Playing);
        assert!(thumbnail, "the cover in the media controls (N2)");
        // A press from the media controls reaches the phone's outbox.
        assert_eq!(
            done(session.TryTogglePlayPauseAsync().expect("a press")),
            Some(true)
        );
        let outbox = dir.join(pocketdesk_core::local::paths::PHONE_MEDIA_DIR);
        let mut pressed = String::new();
        for _ in 0..30 {
            if let Some(file) = std::fs::read_dir(&outbox)
                .ok()
                .and_then(|mut entries| entries.next())
                .and_then(|entry| entry.ok())
            {
                pressed = std::fs::read_to_string(file.path()).unwrap_or_default();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            pressed.contains("\"probe\"") && pressed.contains("playPause"),
            "{pressed}"
        );
        // Shown paused at once, before the phone's word (K7 review).
        let mut paused = false;
        for _ in 0..30 {
            if find().is_some_and(|(_, status, _, _)| status == Status::Paused) {
                paused = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(paused, "paused at once");
        player.show(None).expect("gone");
        let mut gone = false;
        for _ in 0..30 {
            if find().is_none() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(gone, "no song left behind");
        drop(player);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
