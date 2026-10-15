//! The current media session on Windows: the Global System Media Transport
//! Controls, which browsers, Spotify and the Media Player report to.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

use super::{Command, DEADLINE, NowPlaying};

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

/// The session the system names as current, if any.
fn current() -> Result<Option<Session>, String> {
    let manager = Manager::RequestAsync()
        .and_then(within)
        .map_err(|e| format!("the media sessions could not be read: {e}"))?;
    // No current session comes back as an error for a null result.
    Ok(manager.GetCurrentSession().ok())
}

/// What is playing now, or `None` when nothing is.
pub fn now_playing() -> Result<Option<NowPlaying>, String> {
    let Some(session) = current()? else {
        return Ok(None);
    };
    let read = || -> windows::core::Result<NowPlaying> {
        let properties = within(session.TryGetMediaPropertiesAsync()?)?;
        let info = session.GetPlaybackInfo()?;
        let controls = info.Controls()?;
        Ok(NowPlaying {
            title: properties.Title()?.to_string(),
            artist: properties.Artist()?.to_string(),
            app: session.SourceAppUserModelId()?.to_string(),
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

/// Ask the current session to play or pause, or skip.
pub fn command(command: Command) -> Result<(), String> {
    let Some(session) = current()? else {
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
    /// Reads this computer's media session. Run by hand with something
    /// playing (`cargo test -p pocketdesk-server media -- --ignored`).
    #[test]
    #[ignore]
    fn the_current_media_session_is_read() {
        let playing = super::now_playing().unwrap();
        println!("{playing:?}");
    }
}
