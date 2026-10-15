//! The current media player on Linux: MPRIS through `playerctl`, as the
//! Sound sheet drives `wpctl`/`pactl`.

use std::process::{Command as Process, Stdio};

use super::{Command, NowPlaying, PLAYERCTL_FORMAT, parse_playerctl};

fn playerctl(args: &[&str]) -> Result<std::process::Output, String> {
    Process::new("playerctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "playerctl is not installed".to_string(),
            _ => format!("playerctl: {e}"),
        })
}

/// What is playing now, or `None` when no player is running.
pub fn now_playing() -> Result<Option<NowPlaying>, String> {
    let output = playerctl(&["metadata", "--format", PLAYERCTL_FORMAT])?;
    // With no player it fails, saying "No players found".
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_playerctl(&String::from_utf8_lossy(&output.stdout)))
}

/// Ask the current player to play or pause, or skip.
pub fn command(command: Command) -> Result<(), String> {
    let verb = match command {
        Command::PlayPause => "play-pause",
        Command::Next => "next",
        Command::Previous => "previous",
    };
    let output = playerctl(&[verb])?;
    if output.status.success() {
        Ok(())
    } else {
        Err("no player took it".to_string())
    }
}
