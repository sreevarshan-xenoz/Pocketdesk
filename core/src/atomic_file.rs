//! Putting a freshly written temp file in place of a state file.
//!
//! Every state file (the trust store, `computers.json`, config, device keys,
//! …) is saved the same way: written whole to a temp file beside it, then
//! renamed over it, so a reader never sees half a file. [`replace`] is that
//! last step, and every such save goes through it.
//!
//! On Unix it is a plain `rename(2)`. On Windows a rename over a file can fail
//! for a moment while another process holds the target or the temp file
//! open without `FILE_SHARE_DELETE` — an antivirus scan, the Search indexer,
//! a backup or sync client, an editor. The error is then
//! `ERROR_ACCESS_DENIED` (5), `ERROR_SHARING_VIOLATION` (32) or
//! `ERROR_LOCK_VIOLATION` (33), and it clears once that handle closes, so
//! [`replace`] retries those with a short, bounded backoff (cargo, rustup and
//! Git for Windows do the same). Our own readers use `std::fs`, whose Windows
//! opens already share read, write and delete, so they never block a save.
//!
//! The rename itself stays `std::fs::rename`: on Windows it uses POSIX
//! rename semantics where the volume has them (a target held open with
//! `FILE_SHARE_DELETE` is still replaced) and falls back to
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` elsewhere. Calling `MoveFileExW`
//! or `ReplaceFileW` directly would give up the first and fail on the same
//! sharing errors. If the last try fails the temp file is removed, so a
//! failed save never leaves a copy of the data behind.

use std::io;
use std::path::Path;
use std::time::Duration;

/// The waits between tries on Windows: about half a second in all, so a
/// save held up by a scan still lands, and a file that stays locked fails
/// promptly instead of hanging the caller.
const BACKOFF_MS: [u64; 6] = [10, 20, 40, 80, 160, 200];

/// Rename `tmp` over `dest`, replacing it whole. On Windows a transient
/// sharing error is retried for about half a second; on failure `tmp` is
/// removed and the last error returned.
pub fn replace(tmp: impl AsRef<Path>, dest: impl AsRef<Path>) -> io::Result<()> {
    replace_with(
        tmp.as_ref(),
        dest.as_ref(),
        |from, to| std::fs::rename(from, to),
        is_transient,
        std::thread::sleep,
    )
}

/// The retry policy, with the rename, the error test and the sleep passed
/// in so tests can drive it.
fn replace_with(
    tmp: &Path,
    dest: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
    transient: impl Fn(&io::Error) -> bool,
    mut sleep: impl FnMut(Duration),
) -> io::Result<()> {
    let mut waits = BACKOFF_MS.iter();
    loop {
        match rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => match waits.next() {
                Some(&ms) if transient(&e) => sleep(Duration::from_millis(ms)),
                _ => {
                    let _ = std::fs::remove_file(tmp);
                    return Err(e);
                }
            },
        }
    }
}

/// A rename error worth waiting out: on Windows, another handle on the
/// target or the temp file. Never on other systems, where a rename over a
/// file does not fail for that reason.
fn is_transient(e: &io::Error) -> bool {
    cfg!(windows) && e.raw_os_error().is_some_and(is_windows_sharing_code)
}

/// `ERROR_ACCESS_DENIED`, `ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`.
fn is_windows_sharing_code(code: i32) -> bool {
    matches!(code, 5 | 32 | 33)
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

    fn windows_code(e: &io::Error) -> bool {
        e.raw_os_error().is_some_and(is_windows_sharing_code)
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
        for code in [5, 32, 33] {
            assert!(is_windows_sharing_code(code));
            assert_eq!(
                is_transient(&io::Error::from_raw_os_error(code)),
                cfg!(windows)
            );
        }
        for code in [2, 3, 17, 80, 183] {
            assert!(!is_windows_sharing_code(code));
        }
        assert!(!is_transient(&io::Error::from(io::ErrorKind::Other)));
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
        assert!(is_transient(&refused), "{refused:?}");
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
        assert!(is_transient(&err), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!tmp.exists());
        drop(holder);
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
