//! A crash leaves a report (reliability pass, 2026-10-16): the server has
//! died with exit 139 (an access violation) right after a session's codec
//! negotiation three times, never under a debugger, so nothing said where.
//! Now a panic, and on Windows an access violation or any other unhandled
//! exception, appends what happened and a backtrace to `<state>/crash.log`
//! (owner's folder), and the process ends as it would have. On Windows the
//! first few faults of a run are also reported as they are raised, with the
//! module they are in, since one in a process already exiting never reaches
//! the unhandled-exception filter.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file a report goes to, set once at start: the handlers must not look
/// anything up.
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// The report starts over past this size.
const MAX_LOG: u64 = 256 * 1024;

/// The file's name in the state folder.
pub const CRASH_FILE: &str = "crash.log";

/// One report: when, which build, what happened, and the backtrace.
pub fn render(unix: u64, what: &str, backtrace: &str) -> String {
    format!(
        "=== pocketdesk {} crashed at unix {unix}: {what}\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
    )
}

/// Append `report` to the log at `path`, starting over when it has grown
/// past [`MAX_LOG`].
pub fn append(path: &Path, report: &str) {
    let large = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!large)
        .truncate(large)
        .open(path);
    if let Ok(mut file) = file {
        let _ = file.write_all(report.as_bytes());
        let _ = file.flush();
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write a report for `what` now, with this thread's backtrace.
pub fn report(what: &str) {
    if let Some(path) = LOG.get() {
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        append(path, &render(now_unix(), what, &backtrace));
    }
}

/// Report crashes to `<state_dir>/crash.log` from now on: panics on every
/// OS (the default hook still prints them), and unhandled exceptions on
/// Windows.
pub fn install(state_dir: &Path) {
    if LOG.set(state_dir.join(CRASH_FILE)).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        report(&format!(
            "panic on thread {}: {info}",
            thread.name().unwrap_or("unnamed")
        ));
        previous(info);
    }));
    #[cfg(windows)]
    crate::platform::install_exception_report();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_names_the_build_and_what_happened() {
        let report = render(7, "access violation at 0x1234", "  0: frame");
        assert!(report.starts_with(&format!(
            "=== pocketdesk {} crashed at unix 7: access violation at 0x1234",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(report.contains("0: frame"));
    }

    #[test]
    fn the_log_starts_over_once_it_is_large() {
        let dir = std::env::temp_dir().join(format!("pd-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CRASH_FILE);
        append(&path, "one\n");
        append(&path, "two\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        std::fs::write(&path, vec![b'x'; (MAX_LOG + 1) as usize]).unwrap();
        append(&path, "three\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "three\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
