//! The current media session on Windows: the Global System Media Transport
//! Controls, which browsers, Spotify and the Media Player report to.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

use super::{Command, DEADLINE, NowPlaying, Player};

/// The result of `operation`, waited for at most [`DEADLINE`]: a player
/// that never answers must not hold the phone's control connection, which
/// waits on this call. One still running then is cancelled.
fn within<T: windows::core::RuntimeType + 'static>(
    operation: windows_future::IAsyncOperation<T>,
) -> windows::core::Result<T> {
    use windows_future::AsyncStatus;
    let deadline = std::time::Instant::now() + DEADLINE;
    while operation.Status()? == AsyncStatus::Started {
        if std::time::Instant::now() >= deadline {
            let _ = operation.Cancel();
            return Err(windows::core::Error::new(
                windows::Win32::Foundation::E_ABORT,
                "the player did not answer in time",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    operation.GetResults()
}

/// The name Pocketdesk's own entry in the media controls goes by: the
/// tray shows a phone's music there (K7), never the computer's own.
const OWN: &str = pocketdesk_core::local::phone_media::MEDIA_CONTROLS_APP_ID;

/// Which of `sessions` (each app's name and whether it plays) is the
/// computer's music: the `wanted` one while it is there (O6), else the
/// system's `current` unless it is Pocketdesk's own, else the first other
/// one playing, else the first other one (N5: the phone's music shown here
/// must not go back to it).
fn choose(
    wanted: Option<&str>,
    current: Option<&str>,
    sessions: &[(String, bool)],
) -> Option<usize> {
    let other = |app: &str| app != OWN;
    let named = |c: &str| sessions.iter().position(|(app, _)| app == c);
    wanted
        .filter(|w| other(w))
        .and_then(named)
        .or_else(|| current.filter(|c| other(c)).and_then(named))
        .or_else(|| {
            sessions
                .iter()
                .position(|(app, playing)| other(app) && *playing)
        })
        .or_else(|| sessions.iter().position(|(app, _)| other(app)))
}

/// Each session, with its app's name and whether it plays.
fn sessions(manager: &Manager) -> (Vec<Session>, Vec<(String, bool)>) {
    let sessions: Vec<Session> = manager
        .GetSessions()
        .map(|list| list.into_iter().collect())
        .unwrap_or_default();
    let named = sessions
        .iter()
        .map(|s| {
            let app = s
                .SourceAppUserModelId()
                .map(|a| a.to_string())
                .unwrap_or_default();
            let playing = s
                .GetPlaybackInfo()
                .and_then(|i| i.PlaybackStatus())
                .is_ok_and(|p| p == Status::Playing);
            (app, playing)
        })
        .collect();
    (sessions, named)
}

fn manager() -> Result<Manager, String> {
    Manager::RequestAsync()
        .and_then(within)
        .map_err(|e| format!("the media sessions could not be read: {e}"))
}

/// The computer's players (O6): every session but Pocketdesk's own.
pub fn players() -> Result<Vec<Player>, String> {
    let (_, named) = sessions(&manager()?);
    let mut players: Vec<Player> = Vec::new();
    for (app, playing) in named {
        if app.is_empty() || app == OWN {
            continue;
        }
        // An app with two sessions is one player: the first is the one named.
        if let Some(seen) = players.iter_mut().find(|p| p.id == app) {
            seen.playing |= playing;
            continue;
        }
        players.push(Player {
            id: app.clone(),
            name: app,
            playing,
        });
    }
    Ok(players)
}

/// The computer's media session, `wanted`'s while it is there, if any.
fn current(wanted: Option<&str>) -> Result<Option<Session>, String> {
    let manager = manager()?;
    // No current session comes back as an error for a null result.
    let current = manager
        .GetCurrentSession()
        .ok()
        .and_then(|s| s.SourceAppUserModelId().ok())
        .map(|a| a.to_string());
    let (sessions, named) = sessions(&manager);
    Ok(choose(wanted, current.as_deref(), &named).map(|i| sessions[i].clone()))
}

/// The session's cover (its thumbnail), at most a little over the largest
/// sent, read whole.
pub fn cover(player: Option<&str>) -> Option<Vec<u8>> {
    use windows::Storage::Streams::DataReader;
    let session = current(player).ok()??;
    let properties = within(session.TryGetMediaPropertiesAsync().ok()?).ok()?;
    let stream = within(properties.Thumbnail().ok()?.OpenReadAsync().ok()?).ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > super::MAX_COVER as u64 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let loaded = within(reader.LoadAsync(size as u32).ok()?).ok()?;
    let mut bytes = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

/// What is playing now, or `None` when nothing is.
pub fn now_playing(player: Option<&str>) -> Result<Option<NowPlaying>, String> {
    let Some(session) = current(player)? else {
        return Ok(None);
    };
    let read = || -> windows::core::Result<NowPlaying> {
        let properties = within(session.TryGetMediaPropertiesAsync()?)?;
        let info = session.GetPlaybackInfo()?;
        let controls = info.Controls()?;
        let app = session.SourceAppUserModelId()?.to_string();
        Ok(NowPlaying {
            title: properties.Title()?.to_string(),
            artist: properties.Artist()?.to_string(),
            player: app.clone(),
            app,
            is_playing: info.PlaybackStatus()? == Status::Playing,
            // Some players enable only Play, or only Pause, not the toggle;
            // the toggle request plays or pauses them all the same.
            can_play_pause: controls.IsPlayPauseToggleEnabled()?
                || controls.IsPlayEnabled()?
                || controls.IsPauseEnabled()?,
            can_next: controls.IsNextEnabled()?,
            can_previous: controls.IsPreviousEnabled()?,
        })
    };
    read()
        .map(Some)
        .map_err(|e| format!("the media session could not be read: {e}"))
}

/// Ask `player`'s session (else the current one) to play or pause, or skip.
pub fn command(command: Command, player: Option<&str>) -> Result<(), String> {
    let Some(session) = current(player)? else {
        return Err("nothing is playing".to_string());
    };
    let done = match command {
        Command::PlayPause => session.TryTogglePlayPauseAsync(),
        Command::Next => session.TrySkipNextAsync(),
        Command::Previous => session.TrySkipPreviousAsync(),
    }
    .and_then(within)
    .map_err(|e| format!("the player could not be asked: {e}"))?;
    if done {
        Ok(())
    } else {
        Err("the player did not take it".to_string())
    }
}

#[cfg(test)]
mod tests {
    /// N5: Pocketdesk's own entry is never the computer's music.
    #[test]
    fn pocketdesk_s_own_entry_is_never_the_computer_s() {
        let s = |app: &str, playing: bool| (app.to_string(), playing);
        let sessions = [
            s(super::OWN, true),
            s("Spotify.exe", false),
            s("Chrome", true),
        ];
        assert_eq!(
            super::choose(None, Some("Spotify.exe"), &sessions),
            Some(1),
            "the system's own"
        );
        assert_eq!(
            super::choose(None, Some(super::OWN), &sessions),
            Some(2),
            "another playing"
        );
        assert_eq!(
            super::choose(None, None, &[s(super::OWN, true), s("Spotify.exe", false)]),
            Some(1)
        );
        assert_eq!(
            super::choose(None, Some(super::OWN), &[s(super::OWN, true)]),
            None
        );
    }

    /// O6: the player the phone chose, while it is there; never Pocketdesk's own.
    #[test]
    fn the_chosen_player_is_used_while_it_is_there() {
        let s = |app: &str, playing: bool| (app.to_string(), playing);
        let sessions = [
            s("Spotify.exe", true),
            s("Chrome", false),
            s(super::OWN, true),
        ];
        assert_eq!(
            super::choose(Some("Chrome"), Some("Spotify.exe"), &sessions),
            Some(1)
        );
        assert_eq!(
            super::choose(Some("Gone.exe"), Some("Spotify.exe"), &sessions),
            Some(0),
            "gone: the system's own"
        );
        assert_eq!(super::choose(Some(super::OWN), None, &sessions), Some(0));
    }

    /// Reads this computer's media session. Run by hand with something
    /// playing (`cargo test -p pocketdesk-server media -- --ignored`).
    #[test]
    #[ignore]
    fn the_current_media_session_is_read() {
        let playing = super::now_playing(None).unwrap();
        println!("{:?}", super::players());
        println!("{playing:?}");
    }
}
