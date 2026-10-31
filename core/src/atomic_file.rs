//! Putting a freshly written temp file in place of a state file.
//!
//! Every state file (the trust store, `computers.json`, config, device keys,
//! …) is saved the same way: written whole to a temp file beside it, then
//! renamed over it, so a reader never sees half a file. [`replace`] is that
//! last step, and every such save goes through it. [`temp_beside`] names a
//! temp file of the save's own.
//!
//! On Unix it is a plain `rename(2)`. On Windows a rename over a file can fail
//! for a moment while another process holds the target or the temp file
//! open without `FILE_SHARE_DELETE` (an antivirus scan, the Search indexer,
//! a backup or sync client, an editor). The error is then
//! `ERROR_SHARING_VIOLATION` (32), `ERROR_LOCK_VIOLATION` (33) or
//! `ERROR_ACCESS_DENIED` (5), and it clears once that handle closes, so
//! [`replace`] retries those with a short, bounded backoff (cargo, rustup and
//! Git for Windows do the same). Error 5 also means a target that can never
//! be replaced (a folder, a read-only file); that fails at once. Our own
//! readers use `std::fs`, whose Windows opens already share read, write and
//! delete, so they never block a save.
//!
//! The rename itself stays `std::fs::rename`. On Windows (std as of Rust
//! 1.99) that is `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` and, when that
//! fails with `ERROR_ACCESS_DENIED`, a second try through
//! `SetFileInformationByHandle(FileRenameInfoEx)` with
//! `FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_POSIX_SEMANTICS`,
//! which replaces a target held open with `FILE_SHARE_DELETE`. Calling
//! `MoveFileExW` or `ReplaceFileW` directly would lose that second try and
//! fail on the same sharing errors. If the last try fails the temp file is
//! removed, so a failed save never leaves a copy of the data behind.
//!
//! **It can block.** On Windows a contended save sleeps up to about half a
//! second before it gives up. Call it from blocking code: async code goes
//! through `tokio::task::spawn_blocking` (or `block_in_place` on a
//! multi-thread runtime). A caller that saves under a mutex holds it that
//! long too, which is right where the mutex exists to keep its file's
//! writes in turn.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The waits between tries on Windows: about half a second in all, so a
/// save held up by a scan still lands, and a file that stays locked fails
/// promptly instead of hanging the caller.
const BACKOFF_MS: [u64; 6] = [10, 20, 40, 80, 160, 200];

/// Rename `tmp` over `dest`, replacing it whole. On Windows a transient
/// sharing error is retried for about half a second (blocking the calling
/// thread; see the module docs); on failure `tmp` is removed and the last
/// error returned, so `tmp` must be the save's own file.
pub fn replace(tmp: impl AsRef<Path>, dest: impl AsRef<Path>) -> io::Result<()> {
    replace_with(
        tmp.as_ref(),
        dest.as_ref(),
        |from, to| std::fs::rename(from, to),
        classify,
        std::thread::sleep,
    )
}

/// A temp file beside `dest` that no other save shares, in this process or
/// another (`<dest>.<pid>.<n>.tmp`): the server and the desktop app both
/// save some files, and a failed save removes only its own temp file.
pub fn temp_beside(dest: &Path) -> PathBuf {
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = dest.as_os_str().to_owned();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// A finished or crashed save's temp file is never reused (each save names
/// its own), so one a crash left behind sits there holding a copy of the
/// file's data. Only ones this old are swept: a save in flight in another
/// process (the desktop app beside the server) is young.
pub const LEFTOVER_AGE: Duration = Duration::from_secs(10 * 60);

/// Whether `name` is `<known file>.<pid>.<n>.tmp`, a [`temp_beside`] name;
/// returns the pid. `known` says which file names count.
fn leftover_pid(name: &str, known: &impl Fn(&str) -> bool) -> Option<u32> {
    let rest = name.strip_suffix(".tmp")?;
    let (rest, n) = rest.rsplit_once('.')?;
    let (stem, pid) = rest.rsplit_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(n) || !digits(pid) || !known(stem) {
        return None;
    }
    pid.parse().ok()
}

/// Delete the temp files a crash left in `dir`: regular files named
/// `<known file>.<digits>.<digits>.tmp`, last written at least `min_age`
/// before `now`, whose pid is not this process's. Nothing else is touched
/// (no folders, no other names). Returns how many went.
pub fn sweep_leftovers(
    dir: &Path,
    known: impl Fn(&str) -> bool,
    now: std::time::SystemTime,
    min_age: Duration,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(pid) = leftover_pid(&name, &known) else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        // `symlink_metadata`: a link is not a file of ours to follow.
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= min_age);
        if meta.is_file() && old && std::fs::remove_file(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// What a failed rename calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    /// A lasting error: fail now.
    No,
    /// Another handle on the file: wait it out.
    Yes,
    /// `ERROR_ACCESS_DENIED`: another handle, unless the target is one
    /// that can never be replaced.
    UnlessDestRefuses,
}

/// The retry policy, with the rename, the error test and the sleep passed
/// in so tests can drive it.
fn replace_with(
    tmp: &Path,
    dest: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    classify: impl Fn(&io::Error) -> Retry,
    mut sleep: impl FnMut(Duration),
) -> io::Result<()> {
    let mut waits = BACKOFF_MS.iter();
    // Looked at once, on the first `ERROR_ACCESS_DENIED`.
    let mut refuses = None;
    loop {
        let e = match rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        let retry = match classify(&e) {
            Retry::No => false,
            Retry::Yes => true,
            Retry::UnlessDestRefuses => !*refuses.get_or_insert_with(|| dest_refuses(dest)),
        };
        match waits.next() {
            Some(&ms) if retry => sleep(Duration::from_millis(ms)),
            _ => {
                let _ = std::fs::remove_file(tmp);
                return Err(e);
            }
        }
    }
}

/// A target no wait will let a file replace: a folder, or a read-only file
/// (std's rename does not override the read-only attribute).
fn dest_refuses(dest: &Path) -> bool {
    std::fs::metadata(dest).is_ok_and(|m| m.is_dir() || m.permissions().readonly())
}

/// What a rename error calls for: on Windows, codes 32 and 33 (and 5,
/// mostly) are another handle on the target or the temp file. Never on
/// other systems, where a rename over a file does not fail for that reason
/// (and 5, 32, 33 are other errnos).
fn classify(e: &io::Error) -> Retry {
    match e.raw_os_error() {
        Some(code) if cfg!(windows) => classify_windows_code(code),
        _ => Retry::No,
    }
}

/// `ERROR_SHARING_VIOLATION` and `ERROR_LOCK_VIOLATION` always clear;
/// `ERROR_ACCESS_DENIED` may.
fn classify_windows_code(code: i32) -> Retry {
    match code {
        32 | 33 => Retry::Yes,
        5 => Retry::UnlessDestRefuses,
        _ => Retry::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pd-atomic-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The Windows classification, on any system.
    fn windows_code(e: &io::Error) -> Retry {
        e.raw_os_error().map_or(Retry::No, classify_windows_code)
    }

    #[test]
    fn a_transient_error_is_retried_until_the_rename_lands() {
        let dir = temp_dir("retry");
        let (tmp, dest) = (dir.join("a.tmp"), dir.join("a"));
        std::fs::write(&tmp, b"new").unwrap();
        let mut calls = 0;
        let mut slept = Vec::new();
        replace_with(
            &tmp,
            &dest,
            |from, to| {
                calls += 1;
                if calls < 3 {
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    std::fs::rename(from, to)
                }
            },
            windows_code,
            |d| slept.push(d.as_millis()),
        )
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(slept, [10, 20]);
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_retries_are_bounded_and_the_temp_file_goes_on_failure() {
        let dir = temp_dir("bounded");
        let (tmp, dest) = (dir.join("b.tmp"), dir.join("b"));
        std::fs::write(&tmp, b"secret").unwrap();
        let mut calls = 0;
        let mut slept = Vec::new();
        let err = replace_with(
            &tmp,
            &dest,
            |_, _| {
                calls += 1;
                Err(io::Error::from_raw_os_error(5))
            },
            windows_code,
            |d| slept.push(d.as_millis() as u64),
        )
        .unwrap_err();
        assert_eq!(err.raw_os_error(), Some(5));
        assert_eq!(calls, BACKOFF_MS.len() + 1);
        assert_eq!(slept, BACKOFF_MS);
        assert!(
            slept.iter().sum::<u64>() < 1000,
            "bounded to well under a second"
        );
        assert!(!tmp.exists(), "a failed save leaves no copy behind");
        assert!(!dest.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn any_other_error_fails_at_once_and_removes_the_temp_file() {
        let dir = temp_dir("other");
        let (tmp, dest) = (dir.join("c.tmp"), dir.join("c"));
        std::fs::write(&tmp, b"x").unwrap();
        let mut calls = 0;
        let err = replace_with(
            &tmp,
            &dest,
            |_, _| {
                calls += 1;
                Err(io::Error::from(io::ErrorKind::NotFound))
            },
            windows_code,
            |_| panic!("no wait for a lasting error"),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(calls, 1);
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_three_sharing_codes_count_and_only_on_windows() {
        assert_eq!(classify_windows_code(32), Retry::Yes);
        assert_eq!(classify_windows_code(33), Retry::Yes);
        assert_eq!(classify_windows_code(5), Retry::UnlessDestRefuses);
        for code in [2, 3, 17, 80, 183] {
            assert_eq!(classify_windows_code(code), Retry::No);
        }
        for code in [5, 32, 33] {
            let here = classify(&io::Error::from_raw_os_error(code));
            if cfg!(windows) {
                assert_ne!(here, Retry::No);
            } else {
                assert_eq!(here, Retry::No, "errno {code} is not a sharing error");
            }
        }
        assert_eq!(classify(&io::Error::from(io::ErrorKind::Other)), Retry::No);
    }

    /// Access denied by a target that can never be replaced fails at once;
    /// a sharing violation there still waits (the check is only for 5).
    #[test]
    fn access_denied_by_a_folder_or_a_read_only_file_fails_at_once() {
        let dir = temp_dir("refuses");
        let folder = dir.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let read_only = dir.join("read-only");
        std::fs::write(&read_only, b"old").unwrap();
        let mut perms = std::fs::metadata(&read_only).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&read_only, perms).unwrap();
        for dest in [&folder, &read_only] {
            let tmp = dir.join("h.tmp");
            std::fs::write(&tmp, b"new").unwrap();
            let mut calls = 0;
            let err = replace_with(
                &tmp,
                dest,
                |_, _| {
                    calls += 1;
                    Err(io::Error::from_raw_os_error(5))
                },
                windows_code,
                |_| panic!("no wait for {}", dest.display()),
            )
            .unwrap_err();
            assert_eq!(err.raw_os_error(), Some(5));
            assert_eq!(calls, 1, "{}", dest.display());
            assert!(!tmp.exists());
        }
        // A plain file that exists: error 5 is waited out.
        let plain = dir.join("plain");
        std::fs::write(&plain, b"old").unwrap();
        let tmp = dir.join("i.tmp");
        std::fs::write(&tmp, b"new").unwrap();
        let mut calls = 0;
        replace_with(
            &tmp,
            &plain,
            |from, to| {
                calls += 1;
                if calls == 1 {
                    Err(io::Error::from_raw_os_error(5))
                } else {
                    std::fs::rename(from, to)
                }
            },
            windows_code,
            |_| {},
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(std::fs::read(&plain).unwrap(), b"new");
        // A sharing violation on a folder is still retried.
        let tmp = dir.join("j.tmp");
        std::fs::write(&tmp, b"new").unwrap();
        let mut calls = 0;
        let _ = replace_with(
            &tmp,
            &folder,
            |_, _| {
                calls += 1;
                Err(io::Error::from_raw_os_error(32))
            },
            windows_code,
            |_| {},
        );
        assert_eq!(calls, BACKOFF_MS.len() + 1);
        let mut perms = std::fs::metadata(&read_only).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&read_only, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real rename over a folder (error 5 on Windows) fails without
    /// waiting out the backoff.
    #[test]
    fn a_real_replace_over_a_folder_fails_promptly() {
        let dir = temp_dir("folder");
        let (tmp, dest) = (dir.join("k.tmp"), dir.join("k"));
        std::fs::create_dir_all(dest.join("inside")).unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        let started = std::time::Instant::now();
        assert!(replace(&tmp, &dest).is_err());
        assert!(
            started.elapsed() < Duration::from_millis(300),
            "{:?}",
            started.elapsed()
        );
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn known(name: &str) -> bool {
        matches!(name, "computers.json" | "trusted_devices.json")
    }

    fn old_file(dir: &Path, name: &str) {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
    }

    #[test]
    fn the_sweep_takes_old_leftovers_of_known_files_only() {
        let dir = temp_dir("sweep");
        let other = std::process::id().wrapping_add(1);
        let gone = [
            format!("computers.json.{other}.0.tmp"),
            format!("trusted_devices.json.{other}.17.tmp"),
        ];
        let kept = [
            // not a known file
            format!("notes.json.{other}.0.tmp"),
            // near misses of the shape
            format!("computers.json.{other}.tmp"),
            format!("computers.json.{other}.x.tmp"),
            "computers.json.x.1.tmp".to_string(),
            "computers.json..1.tmp".to_string(),
            format!("computers.json.{other}.1.tmp.bak"),
            format!("computers.json.{other}.1"),
            format!("xcomputers.json.{other}.1.tmp"),
            format!("computers.json.{other}.1.2.tmp"),
            "computers.json".to_string(),
            "computers.json.tmp".to_string(),
            // this process's own, even old
            format!("computers.json.{}.3.tmp", std::process::id()),
        ];
        for n in gone.iter().chain(&kept) {
            old_file(&dir, n);
        }
        // a young one, as a save in flight is
        let young = format!("computers.json.{other}.9.tmp");
        std::fs::write(dir.join(&young), b"x").unwrap();
        // a folder with a leftover's name
        let folder = format!("computers.json.{other}.5.tmp");
        std::fs::create_dir(dir.join(&folder)).unwrap();

        let n = sweep_leftovers(&dir, known, std::time::SystemTime::now(), LEFTOVER_AGE);
        assert_eq!(n, gone.len());
        for n in &gone {
            assert!(!dir.join(n).exists(), "{n} should go");
        }
        for n in kept.iter().chain([&young, &folder]) {
            assert!(dir.join(n).exists(), "{n} should stay");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sweep_of_a_missing_folder_is_nothing() {
        let dir = temp_dir("sweep-missing").join("absent");
        let n = sweep_leftovers(&dir, known, std::time::SystemTime::now(), LEFTOVER_AGE);
        assert_eq!(n, 0);
    }

    #[test]
    fn each_temp_name_is_the_saves_own_and_sits_beside_its_file() {
        let dest = Path::new("state").join("computers.json");
        let (a, b) = (temp_beside(&dest), temp_beside(&dest));
        assert_ne!(a, b);
        for t in [&a, &b] {
            assert_eq!(t.parent(), dest.parent());
            let name = t.file_name().unwrap().to_str().unwrap();
            let ours = format!("computers.json.{}.", std::process::id());
            assert!(name.starts_with(&ours) && name.ends_with(".tmp"), "{name}");
        }
    }

    #[test]
    fn a_real_replace_overwrites_and_a_failed_one_removes_the_temp_file() {
        let dir = temp_dir("real");
        let (tmp, dest) = (dir.join("d.tmp"), dir.join("d"));
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        replace(&tmp, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!tmp.exists());
        // Into a folder that does not exist: a lasting error.
        std::fs::write(&tmp, b"newer").unwrap();
        assert!(replace(&tmp, dir.join("missing").join("d")).is_err());
        assert!(!tmp.exists(), "the temp file is cleaned up");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reader holding the target open through `std::fs` (as every reader
    /// of ours does) never blocks a save.
    #[test]
    fn a_reader_with_the_default_share_mode_does_not_block_a_save() {
        use std::io::Read;
        let dir = temp_dir("reader");
        let (tmp, dest) = (dir.join("e.tmp"), dir.join("e"));
        std::fs::write(&dest, b"old").unwrap();
        let mut reader = std::fs::File::open(&dest).unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        replace(&tmp, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        let mut seen = Vec::new();
        reader.read_to_end(&mut seen).unwrap();
        assert_eq!(seen, b"old", "the open reader keeps the file it opened");
        drop(reader);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Another process holding the target without `FILE_SHARE_DELETE` (an
    /// antivirus scan, an editor) for a moment: the save waits it out.
    #[cfg(windows)]
    #[test]
    fn a_save_waits_out_a_handle_that_does_not_share_delete() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let dir = temp_dir("holder");
        let (tmp, dest) = (dir.join("f.tmp"), dir.join("f"));
        std::fs::write(&dest, b"old").unwrap();
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&dest)
            .unwrap();
        // While it is held for good, the plain rename is refused.
        std::fs::write(&tmp, b"new").unwrap();
        let refused = std::fs::rename(&tmp, &dest).unwrap_err();
        assert_ne!(classify(&refused), Retry::No, "{refused:?}");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(holder);
        });
        replace(&tmp, &dest).unwrap();
        release.join().unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A handle that never lets go: the save fails within the bound and
    /// leaves no temp file.
    #[cfg(windows)]
    #[test]
    fn a_save_against_a_lasting_lock_fails_and_cleans_up() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("lasting");
        let (tmp, dest) = (dir.join("g.tmp"), dir.join("g"));
        std::fs::write(&dest, b"old").unwrap();
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&dest)
            .unwrap();
        std::fs::write(&tmp, b"new").unwrap();
        let started = std::time::Instant::now();
        let err = replace(&tmp, &dest).unwrap_err();
        assert_ne!(classify(&err), Retry::No, "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!tmp.exists());
        drop(holder);
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
