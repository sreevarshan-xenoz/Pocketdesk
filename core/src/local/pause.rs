//! A pause on what the phones bring to this computer (K2a): for a while,
//! or until resumed, phones' notifications and texts are not shown, links
//! not opened, and a phone ringing this computer
//! silent. Kept in `<state>/pause.json`; the
//! desktop app, its tray and the CLI write it, and the server reads it at
//! each thing it holds.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::PAUSE_FILE;

/// A pause in force from `since_unix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub since_unix: u64,
    /// When it ends by itself; `None` until resumed.
    pub until_unix: Option<u64>,
}

impl Pause {
    /// Whether it holds at `now_unix`.
    pub fn active(&self, now_unix: u64) -> bool {
        self.until_unix.is_none_or(|until| now_unix < until)
    }

    /// Minutes left at `now_unix`, rounded up; `None` until resumed.
    pub fn minutes_left(&self, now_unix: u64) -> Option<u64> {
        self.until_unix
            .map(|until| until.saturating_sub(now_unix).div_ceil(60))
    }
}

/// The pause as written, a missing file none; one that does not parse is
/// an error, so a change never writes over it.
pub fn load(state_dir: &Path) -> io::Result<Option<Pause>> {
    match std::fs::read_to_string(state_dir.join(PAUSE_FILE)) {
        Ok(raw) => serde_json::from_str::<Option<Pause>>(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The pause in force at `now_unix`, if any; none when it cannot be read.
pub fn current(state_dir: &Path, now_unix: u64) -> Option<Pause> {
    load(state_dir)
        .ok()
        .flatten()
        .filter(|pause| pause.active(now_unix))
}

fn write(state_dir: &Path, pause: Option<Pause>) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let json = serde_json::to_string(&pause).map_err(io::Error::other)?;
    let path = state_dir.join(PAUSE_FILE);
    let tmp = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// Change the pause under its lock: a file that does not parse is never
/// written over.
fn change(state_dir: &Path, pause: Option<Pause>) -> io::Result<()> {
    let _lock = crate::protocol::kdeconnect::StoreLock::acquire(&state_dir.join(PAUSE_FILE))
        .map_err(|e| io::Error::other(e.to_string()))?;
    load(state_dir)?;
    write(state_dir, pause)
}

/// Pause from `now_unix` for `minutes`, or until resumed with `None`.
pub fn pause(state_dir: &Path, minutes: Option<u32>, now_unix: u64) -> io::Result<Pause> {
    let pause = Pause {
        since_unix: now_unix,
        until_unix: minutes.map(|m| now_unix + u64::from(m.max(1)) * 60),
    };
    change(state_dir, Some(pause))?;
    Ok(pause)
}

/// End the pause, if any.
pub fn resume(state_dir: &Path) -> io::Result<()> {
    change(state_dir, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-pause-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_pause_for_minutes_ends_at_its_time() {
        let dir = dir("minutes");
        assert_eq!(current(&dir, 1_000), None, "none at first");
        let pause = pause(&dir, Some(60), 1_000).unwrap();
        assert_eq!(pause.until_unix, Some(1_000 + 3_600));
        assert_eq!(current(&dir, 1_000), Some(pause));
        assert_eq!(pause.minutes_left(1_000), Some(60));
        assert_eq!(pause.minutes_left(1_000 + 3_599), Some(1), "rounded up");
        assert_eq!(current(&dir, 1_000 + 3_600), None, "over at its time");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn until_resumed_holds_until_resumed() {
        let dir = dir("resume");
        let pause = pause(&dir, None, 1_000).unwrap();
        assert_eq!(pause.minutes_left(9_999_999), None);
        assert_eq!(
            current(&dir, 9_999_999),
            Some(pause),
            "never ends by itself"
        );
        resume(&dir).unwrap();
        assert_eq!(current(&dir, 1_001), None);
        resume(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_no_pause_and_never_written_over() {
        let dir = dir("broken");
        std::fs::write(dir.join(PAUSE_FILE), "not json").unwrap();
        assert_eq!(current(&dir, 1), None);
        assert!(pause(&dir, Some(60), 1).is_err());
        assert!(resume(&dir).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join(PAUSE_FILE)).unwrap(),
            "not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
