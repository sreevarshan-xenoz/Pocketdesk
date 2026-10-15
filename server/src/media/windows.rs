//! The current media session on Windows: the Global System Media Transport
//! Controls, which browsers, Spotify and the Media Player report to.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

use super::{Command, NowPlaying};

/// The session the system names as current, if any.
fn current() -> Result<Option<Session>, String> {
    let manager = Manager::RequestAsync()
        .and_then(|request| request.join())
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
        let properties = session.TryGetMediaPropertiesAsync()?.join()?;
        let info = session.GetPlaybackInfo()?;
        let controls = info.Controls()?;
        Ok(NowPlaying {
            title: properties.Title()?.to_string(),
            artist: properties.Artist()?.to_string(),
            app: session.SourceAppUserModelId()?.to_string(),
            is_playing: info.PlaybackStatus()? == Status::Playing,
            can_play_pause: controls.IsPlayPauseToggleEnabled()?,
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
    .and_then(|request| request.join())
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
