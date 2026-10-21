//! The current media player on Linux: MPRIS through `playerctl`, as the
//! Sound sheet drives `wpctl`/`pactl`.

use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use super::{Command, DEADLINE, NowPlaying, PLAYERCTL_FORMAT, parse_playerctl};

/// Run `playerctl` with `args`, waiting at most [`DEADLINE`]: a player that
/// hangs on D-Bus must not hold the phone's control connection. One still
/// running then is killed.
fn playerctl(args: &[&str]) -> Result<std::process::Output, String> {
    let mut child = Process::new("playerctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "playerctl is not installed".to_string(),
            _ => format!("playerctl: {e}"),
        })?;
    let deadline = Instant::now() + DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("the player did not answer in time".to_string());
            }
            Err(e) => return Err(format!("playerctl: {e}")),
        }
    }
    child
        .wait_with_output()
        .map_err(|e| format!("playerctl: {e}"))
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

/// The track's cover, where the player names a local file (`mpris:artUrl`).
pub fn cover() -> Option<Vec<u8>> {
    let output = playerctl(&["metadata", "mpris:artUrl"]).ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = url.strip_prefix("file://")?;
    // Its percent-escapes, as a file URI has them.
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%'
            && let Some(hex) = path.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            bytes.push(b);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > super::MAX_COVER as u64 {
        return None;
    }
    std::fs::read(path).ok()
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
