//! The current media player on Linux: MPRIS through `playerctl`, as the
//! Sound sheet drives `wpctl`/`pactl`.

use std::process::{Command as Process, Stdio};
use std::time::{Duration, Instant};

use super::{
    Command, DEADLINE, NowPlaying, PLAYERCTL_FORMAT, PLAYERCTL_LIST_FORMAT, Player,
    parse_playerctl, parse_playerctl_list,
};

/// `args` for `player` first, else whichever `playerctl` picks (O6).
fn for_player(player: Option<&str>, args: &[&str]) -> Vec<String> {
    let mut all = Vec::new();
    if let Some(p) = player {
        all.push("-p".to_string());
        all.push(format!("{p},%any"));
    }
    all.extend(args.iter().map(|a| a.to_string()));
    all
}

/// Run `playerctl` with `args`, waiting at most [`DEADLINE`]: a player that
/// hangs on D-Bus must not hold the phone's control connection. One still
/// running then is killed.
fn playerctl<S: AsRef<std::ffi::OsStr>>(args: &[S]) -> Result<std::process::Output, String> {
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

/// The running players (O6).
pub fn players() -> Result<Vec<Player>, String> {
    let output = playerctl(&["-a", "metadata", "--format", PLAYERCTL_LIST_FORMAT])?;
    // With no player it fails, saying "No players found".
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_playerctl_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// What is playing now, or `None` when no player is running.
pub fn now_playing(player: Option<&str>) -> Result<Option<NowPlaying>, String> {
    let output = playerctl(&for_player(
        player,
        &["metadata", "--format", PLAYERCTL_FORMAT],
    ))?;
    // With no player it fails, saying "No players found".
    if !output.status.success() {
        return Ok(None);
    }
    Ok(parse_playerctl(&String::from_utf8_lossy(&output.stdout)))
}

/// The track's cover, where the player names a local file (`mpris:artUrl`).
pub fn cover(player: Option<&str>) -> Option<Vec<u8>> {
    let output = playerctl(&for_player(player, &["metadata", "mpris:artUrl"])).ok()?;
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
    // A player names the file: only a plain file is read, and never past
    // the limit (N5 review: a FIFO or a device would hang the watcher).
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > super::MAX_COVER as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    std::fs::File::open(path)
        .ok()?
        .take(super::MAX_COVER as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= super::MAX_COVER).then_some(bytes)
}

/// Ask `player` (else the current one) to play or pause, or skip.
pub fn command(command: Command, player: Option<&str>) -> Result<(), String> {
    let verb = match command {
        Command::PlayPause => "play-pause",
        Command::Next => "next",
        Command::Previous => "previous",
    };
    let output = playerctl(&for_player(player, &[verb]))?;
    if output.status.success() {
        Ok(())
    } else {
        Err("no player took it".to_string())
    }
}
