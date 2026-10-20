//! A phone's player in this computer's own media controls (K7): while a
//! phone reports what it plays (T8d), Windows' media controls (the volume
//! flyout, the keyboard's media keys) show its song with Previous,
//! Play/Pause and Next, and a press goes to the phone as the Sessions
//! page's does. The tray program holds it, with Settings'
//! `phone_media_controls` on.

use std::collections::BTreeMap;

use pocketdesk_core::local::phone_media::PhoneMediaState;
use pocketdesk_core::protocol::phone_media::PhoneMedia;

/// The phone to show: a playing one before a paused one, then the newest
/// report; none when no phone reports.
pub fn pick(phones: &BTreeMap<String, PhoneMediaState>) -> Option<(&str, &PhoneMedia)> {
    phones
        .iter()
        .max_by_key(|(_, state)| (state.media.is_playing, state.updated_unix))
        .map(|(id, state)| (id.as_str(), &state.media))
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
pub use windows_player::{Player, Watch};

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

    use super::{Button, command, pick};

    /// The name Windows shows for it: the one the server registers for its
    /// toasts, so the flyout says Pocketdesk, not the program's file.
    const AUMID: windows::core::PCWSTR = w!("Pocketdesk.Server");

    /// The phone shown, and whether it plays, for a press.
    type Target = Arc<Mutex<Option<(String, bool)>>>;

    /// A phone's player in Windows' media controls, on a hidden window of
    /// its own. Dropped, it leaves them.
    pub struct Player {
        hwnd: HWND,
        smtc: SystemMediaTransportControls,
        token: i64,
        target: Target,
        shown: Option<(String, PhoneMedia)>,
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
            // The thread may never have touched WinRT; one already set up
            // makes this a harmless error.
            let _ = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
            let _ = unsafe {
                windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(AUMID)
            };
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
                let token = smtc.ButtonPressed(&TypedEventHandler::<
                    SystemMediaTransportControls,
                    SystemMediaTransportControlsButtonPressedEventArgs,
                >::new(move |_, args| {
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
                    press(&state_dir, &pressed, button);
                    Ok(())
                }))?;
                smtc.SetIsEnabled(false)?;
                Ok::<_, windows::core::Error>((smtc, token, target))
            })();
            match made {
                Ok((smtc, token, target)) => Ok(Self {
                    hwnd,
                    smtc,
                    token,
                    target,
                    shown: None,
                }),
                Err(e) => {
                    let _ = unsafe { DestroyWindow(hwnd) };
                    Err(e)
                }
            }
        }

        /// Show `now`'s phone and what it plays, or nothing.
        pub fn show(&mut self, now: Option<(&str, &PhoneMedia)>) -> windows::core::Result<()> {
            let now = now.map(|(id, media)| (id.to_string(), media.clone()));
            if now == self.shown {
                return Ok(());
            }
            *self.target.lock().unwrap_or_else(|e| e.into_inner()) = now
                .as_ref()
                .map(|(id, media)| (id.clone(), media.is_playing));
            let updater = self.smtc.DisplayUpdater()?;
            match &now {
                None => {
                    updater.ClearAll()?;
                    updater.Update()?;
                    self.smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed)?;
                    self.smtc.SetIsEnabled(false)?;
                }
                Some((_, media)) => {
                    updater.SetType(MediaPlaybackType::Music)?;
                    let music = updater.MusicProperties()?;
                    music.SetTitle(&HSTRING::from(media.title.as_str()))?;
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

    impl Drop for Player {
        fn drop(&mut self) {
            let _ = self.show(None);
            let _ = self.smtc.RemoveButtonPressed(self.token);
            let _ = unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Send `button`'s command to the phone `target` names, as the Sessions
    /// page's buttons do (held to its Media permission there).
    fn press(state_dir: &Path, target: &Target, button: Button) {
        let Some((phone, playing)) = target.lock().unwrap_or_else(|e| e.into_inner()).clone()
        else {
            return;
        };
        let Some(command) = command(button, playing) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if let Err(e) = crate::model::press_phone_media(state_dir, &phone, command, now) {
            eprintln!("Pocketdesk tray: a media press was not sent: {e}");
        }
    }

    /// The player as Settings has it (K7): read again when the config
    /// changes, made while on, dropped when off.
    #[derive(Default)]
    pub struct Watch {
        stamp: Option<SystemTime>,
        read: bool,
        on: bool,
        player: Option<Player>,
        /// A player Windows would not make is not tried again until the
        /// setting changes.
        failed: bool,
    }

    impl Watch {
        /// Follow the setting, and show what `phones` play.
        pub fn follow(
            &mut self,
            state_dir: &Path,
            phones: &std::collections::BTreeMap<String, PhoneMediaState>,
        ) {
            let stamp = pocketdesk_core::local::config::Config::path()
                .ok()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());
            if !self.read || stamp != self.stamp {
                // A config that does not read keeps the player as it is.
                if let Ok(config) = pocketdesk_core::local::config::Config::load() {
                    if config.phone_media_controls != self.on {
                        self.failed = false;
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
            if self.player.is_none() && !self.failed {
                match Player::new(state_dir.to_path_buf()) {
                    Ok(player) => self.player = Some(player),
                    Err(e) => {
                        eprintln!("Pocketdesk tray: no media controls ({e})");
                        self.failed = true;
                    }
                }
            }
            if let Some(player) = &mut self.player
                && let Err(e) = player.show(pick(phones))
            {
                eprintln!("Pocketdesk tray: the media controls did not update ({e})");
                self.player = None;
                self.failed = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            updated_unix: at,
        }
    }

    /// K7: a playing phone comes before a paused one, then the newest.
    #[test]
    fn the_playing_phone_is_shown_first() {
        let mut phones = BTreeMap::new();
        assert_eq!(pick(&phones), None);
        phones.insert("a".to_string(), state("paused, newest", false, 30));
        phones.insert("b".to_string(), state("playing, older", true, 10));
        phones.insert("c".to_string(), state("playing, newer", true, 20));
        assert_eq!(pick(&phones).map(|(id, _)| id), Some("c"));
        phones.remove("c");
        phones.remove("b");
        assert_eq!(pick(&phones).map(|(id, _)| id), Some("a"));
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
        let mut player = Player::new(dir.clone()).expect("a player");
        let mut phones = BTreeMap::new();
        phones.insert("probe".to_string(), state("K7 probe song", true, 1));
        player.show(pick(&phones)).expect("shown");
        let find = || -> Option<(String, Status, windows::Media::Control::GlobalSystemMediaTransportControlsSession)> {
            let manager = done(Manager::RequestAsync().ok()?)?;
            let sessions = manager.GetSessions().ok()?;
            for session in sessions {
                let props = done(session.TryGetMediaPropertiesAsync().ok()?)?;
                if props.Title().ok()? == "K7 probe song" {
                    let status = session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()?;
                    let source = session.SourceAppUserModelId().ok()?.to_string();
                    return Some((source, status, session));
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
        let (source, status, session) = seen.expect("the song in the media controls");
        eprintln!("shown by {source}");
        assert_eq!(status, Status::Playing);
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
