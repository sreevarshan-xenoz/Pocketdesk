//! Phone apps whose notifications this computer does not show (K2b).
//!
//! "Not from this app", on a phone notification's toast or its row on the
//! Activity page, adds the app here; Settings lists them with Unmute. Kept
//! in `<state>/muted_apps.json`, matched by the app's name as the phone
//! gives it, case and spaces at either end ignored. The phone's own list of
//! apps it sends is left alone. The desktop app and the server both write
//! it, each a whole file at once.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::MUTED_APPS_FILE;

/// The most apps kept: past it, a mute is refused with a reason.
pub const MAX_APPS: usize = 200;

/// The longest app name kept, in characters.
const MAX_NAME_CHARS: usize = 120;

/// One muted app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutedApp {
    /// Its name as the phone gave it when it was muted.
    pub app: String,
    pub since_unix: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct MutedFile {
    apps: Vec<MutedApp>,
}

/// An app's name as it is matched: trimmed and lowercased.
fn key(app: &str) -> String {
    app.trim().to_lowercase()
}

/// The muted apps, oldest first. A missing file is none; one that does not
/// parse is an error, so a mute never writes over it.
pub fn load(state_dir: &Path) -> io::Result<Vec<MutedApp>> {
    match std::fs::read_to_string(state_dir.join(MUTED_APPS_FILE)) {
        Ok(raw) => serde_json::from_str::<MutedFile>(&raw)
            .map(|file| file.apps)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The muted apps to show or match, none when the file cannot be read.
pub fn read(state_dir: &Path) -> Vec<MutedApp> {
    load(state_dir).unwrap_or_default()
}

/// Whether `app` is in `muted`.
pub fn is_muted(muted: &[MutedApp], app: &str) -> bool {
    let app = key(app);
    !app.is_empty() && muted.iter().any(|m| key(&m.app) == app)
}

/// Write `apps` whole, owner-only, through a temp file of this writer's
/// own, so a reader never sees it half written.
fn write(state_dir: &Path, apps: Vec<MutedApp>) -> io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let json = serde_json::to_string(&MutedFile { apps }).map_err(io::Error::other)?;
    let path = state_dir.join(MUTED_APPS_FILE);
    let tmp = path.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Change the list under its lock (K2b review: the desktop app and the
/// server's toast handlers both change it, and each change is read, change,
/// write).
fn change(
    state_dir: &Path,
    edit: impl FnOnce(&mut Vec<MutedApp>) -> io::Result<bool>,
) -> io::Result<()> {
    let _lock = crate::protocol::kdeconnect::StoreLock::acquire(&state_dir.join(MUTED_APPS_FILE))
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut apps = load(state_dir)?;
    if edit(&mut apps)? {
        write(state_dir, apps)?;
    }
    Ok(())
}

/// Mute `app` from `now_unix`; muting one already muted changes nothing.
pub fn mute(state_dir: &Path, app: &str, now_unix: u64) -> io::Result<()> {
    let name: String = app.trim().chars().take(MAX_NAME_CHARS).collect();
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an app with no name",
        ));
    }
    change(state_dir, |apps| {
        if is_muted(apps, &name) {
            return Ok(false);
        }
        if apps.len() >= MAX_APPS {
            return Err(io::Error::other(format!(
                "{MAX_APPS} apps are muted already; unmute one first"
            )));
        }
        apps.push(MutedApp {
            app: name,
            since_unix: now_unix,
        });
        Ok(true)
    })
}

/// Unmute `app`; one not muted changes nothing.
pub fn unmute(state_dir: &Path, app: &str) -> io::Result<()> {
    let gone = key(app);
    change(state_dir, |apps| {
        let before = apps.len();
        apps.retain(|m| key(&m.app) != gone);
        Ok(apps.len() != before)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-muted-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_app_is_muted_and_unmuted_whatever_its_case() {
        let dir = dir("mute");
        assert!(read(&dir).is_empty(), "none at first");
        mute(&dir, "Gmail", 100).unwrap();
        mute(&dir, "  gmail ", 200).unwrap();
        let apps = read(&dir);
        assert_eq!(apps.len(), 1, "once");
        assert_eq!(
            apps[0],
            MutedApp {
                app: "Gmail".to_string(),
                since_unix: 100
            }
        );
        assert!(is_muted(&apps, "GMAIL"));
        assert!(!is_muted(&apps, "Gmail Go"));
        assert!(!is_muted(&apps, ""), "a nameless app is never muted");
        unmute(&dir, "gMail").unwrap();
        assert!(read(&dir).is_empty());
        unmute(&dir, "never muted").unwrap();
        assert!(mute(&dir, "   ", 1).is_err(), "no name");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// K2b review: two writers at once each keep their change.
    #[test]
    fn writers_at_once_lose_nothing() {
        let dir = dir("race");
        let threads: Vec<_> = (0..8)
            .map(|n| {
                let dir = dir.clone();
                std::thread::spawn(move || mute(&dir, &format!("app {n}"), 1).unwrap())
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(read(&dir).len(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn at_most_two_hundred_apps_are_kept() {
        let dir = dir("cap");
        for n in 0..MAX_APPS {
            mute(&dir, &format!("app {n}"), 1).unwrap();
        }
        assert!(mute(&dir, "one more", 1).is_err());
        assert_eq!(read(&dir).len(), MAX_APPS);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_reads_as_none_and_is_never_written_over() {
        let dir = dir("broken");
        std::fs::write(dir.join(MUTED_APPS_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty());
        assert!(mute(&dir, "Gmail", 1).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join(MUTED_APPS_FILE)).unwrap(),
            "not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
