//! Alerts for the phone (BF1): `pocketdesk done` watches a command, or a
//! process already running, and when it ends leaves an alert as
//! `<state>/alerts_out/<id>.json`, taken (read and deleted) by the server,
//! which shows it as one of this computer's notifications. A phone that
//! shows the computer's notifications gets it, at home or by the internet
//! road: a long build, render or backup can be left.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::ALERTS_DIR;

/// How long an alert waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 120;

/// The last lines of output an alert carries, at most.
pub const TAIL_LINES: usize = 3;

/// The characters of each of those lines kept, at most.
pub const TAIL_LINE_CHARS: usize = 120;

/// One alert for the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub title: String,
    pub text: String,
    pub created_unix: u64,
}

/// How a watched command or process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It exited with this code.
    Exit(i32),
    /// It was stopped by a signal (no exit code).
    Stopped,
    /// It ended, how is not known (a process watched by its id on Linux).
    Gone,
    /// It could not be started.
    NotStarted,
}

/// The alert's title and text for `what` (the command, or the process),
/// ended so after `took`, with the last lines of its output (`tail`).
pub fn words(what: &str, ended: Ended, took: Duration, tail: &[String]) -> (String, String) {
    let what = shorten(what.trim(), 60);
    let title = match ended {
        Ended::Exit(0) | Ended::Gone => format!("Done: {what}"),
        // A Windows crash code reads as one in hex (review), not -1073741819.
        Ended::Exit(code) if code < 0 => format!("Failed ({:#010X}): {what}", code as u32),
        Ended::Exit(code) => format!("Failed ({code}): {what}"),
        Ended::Stopped => format!("Stopped: {what}"),
        Ended::NotStarted => format!("Did not start: {what}"),
    };
    let mut text = format!("took {}", took_words(took));
    let kept: Vec<String> = tail
        .iter()
        .map(|line| shorten(line.trim_end(), TAIL_LINE_CHARS))
        .filter(|line| !line.trim().is_empty())
        .collect();
    for line in kept.iter().skip(kept.len().saturating_sub(TAIL_LINES)) {
        text.push('\n');
        text.push_str(line);
    }
    (title, text)
}

/// "45 s", "3 min 12 s", "2 h 5 min".
pub fn took_words(took: Duration) -> String {
    let secs = took.as_secs();
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

/// `text` cut to `chars` characters, an ellipsis marking the cut; control
/// characters (a terminal's colours) are dropped.
fn shorten(text: &str, chars: usize) -> String {
    let clean: String = strip_escapes(text)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if clean.chars().count() <= chars {
        clean
    } else {
        let mut cut: String = clean.chars().take(chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// `text` without ANSI escape sequences (`ESC [ … letter`).
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Leave an alert for the server (the command line's side).
pub fn request(state_dir: &Path, title: &str, text: &str, now_unix: u64) -> io::Result<Alert> {
    let alert = Alert {
        id: uuid::Uuid::new_v4().to_string(),
        title: title.to_string(),
        text: text.to_string(),
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &alert.id)?;
    std::fs::create_dir_all(state_dir.join(ALERTS_DIR))?;
    let json = serde_json::to_string(&alert).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(alert)
}

/// Take every waiting alert (the server's side), oldest first. Each file is
/// removed once read; one that is stale, from the future, unreadable or
/// named for another alert is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<Alert> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(ALERTS_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue; // a temp file mid-write
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(alert) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<Alert>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            alert.created_unix <= now_unix && now_unix - alert.created_unix <= REQUEST_TTL_SECS;
        if fresh && alert.id == stem && is_valid_id(&alert.id) {
            taken.push(alert);
        }
    }
    taken.sort_by_key(|a| a.created_unix);
    taken
}

fn request_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir.join(ALERTS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-alerts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// BF1: an alert is taken once, and a stale one never.
    #[test]
    fn an_alert_is_taken_once() {
        let dir = temp_dir("once");
        assert!(take_all(&dir, 100).is_empty());
        let asked = request(&dir, "Done: make", "took 3 s", 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert!(take_all(&dir, 101).is_empty(), "taken");
        request(&dir, "Done: old", "took 1 s", 100).unwrap();
        assert!(
            take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "stale"
        );
    }

    /// BF1: the title says how it ended, the text how long it took and its
    /// last lines.
    #[test]
    fn an_alert_says_how_it_ended() {
        let tail = [
            "   Compiling a",
            "",
            "\u{1b}[32mFinished\u{1b}[0m release",
            "error: x",
            "last",
        ]
        .map(str::to_string);
        let (title, text) = words(
            "cargo build",
            Ended::Exit(0),
            Duration::from_secs(192),
            &tail,
        );
        assert_eq!(title, "Done: cargo build");
        assert_eq!(text, "took 3 min 12 s\nFinished release\nerror: x\nlast");
        assert_eq!(
            words("make", Ended::Exit(2), Duration::ZERO, &[]).0,
            "Failed (2): make"
        );
        assert_eq!(
            words("app", Ended::Exit(-1073741819), Duration::ZERO, &[]).0,
            "Failed (0xC0000005): app"
        );
        assert_eq!(
            words("make", Ended::Stopped, Duration::ZERO, &[]).0,
            "Stopped: make"
        );
        assert_eq!(
            words("nope", Ended::NotStarted, Duration::ZERO, &[]).0,
            "Did not start: nope"
        );
        assert_eq!(
            words("process 42", Ended::Gone, Duration::ZERO, &[]).0,
            "Done: process 42"
        );
        assert_eq!(took_words(Duration::from_secs(7505)), "2 h 5 min");
    }

    /// A long command and a long line are cut, not sent whole.
    #[test]
    fn long_words_are_cut() {
        let (title, text) = words(
            &"a".repeat(200),
            Ended::Exit(0),
            Duration::ZERO,
            &["b".repeat(500)],
        );
        assert_eq!(title.chars().count(), "Done: ".len() + 60);
        assert!(title.ends_with('…'));
        assert_eq!(
            text.lines().nth(1).unwrap().chars().count(),
            TAIL_LINE_CHARS
        );
    }
}
