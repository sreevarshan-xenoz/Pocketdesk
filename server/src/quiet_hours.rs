//! Quiet hours, and quiet while presenting (P3): phones' notifications and
//! texts held as a pause holds them (K2a), logged and kept to answer, no
//! toast. Read at each one, so a change in Settings applies to the next.

use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::quiet_hours::{LocalTime, Schedule};

/// Why this computer is quiet now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quiet {
    /// Within the schedule.
    Hours,
    /// Presenting, or something in full screen (Windows); the desktop's
    /// own Do Not Disturb (GNOME, KDE; Q4).
    Presenting,
}

impl Quiet {
    /// The event's field that says why a notification was not shown.
    pub fn field(self) -> &'static str {
        match self {
            Quiet::Hours => "quiet_hours",
            Quiet::Presenting => "presenting",
        }
    }
}

/// Why `config` makes this computer quiet at `at`, with `presenting`
/// asked only when its switch is on.
pub fn decide(
    config: &Config,
    at: Option<LocalTime>,
    presenting: impl FnOnce() -> bool,
) -> Option<Quiet> {
    if let (Some(schedule), Some(at)) = (Schedule::from_config(config), at)
        && schedule.holds(at)
    {
        return Some(Quiet::Hours);
    }
    (config.quiet_presenting && presenting()).then_some(Quiet::Presenting)
}

/// Why this computer is quiet now, if it is.
#[cfg(not(test))]
pub fn now() -> Option<Quiet> {
    decide(
        &crate::events::current_config(),
        pocketdesk_core::local::quiet_hours::local_now(),
        presenting,
    )
}

/// In tests, only the thread's own answer (P3 review), never the owner's
/// config or the time the tests run at.
#[cfg(test)]
pub fn now() -> Option<Quiet> {
    QUIET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(id, _)| *id == std::thread::current().id())
        .map(|(_, quiet)| *quiet)
}

/// The test threads quiet now, and why.
#[cfg(test)]
static QUIET: std::sync::Mutex<Vec<(std::thread::ThreadId, Quiet)>> =
    std::sync::Mutex::new(Vec::new());

/// Quiet on this thread until the guard drops (tests).
#[cfg(test)]
pub fn quiet_for_test(quiet: Quiet) -> impl Drop {
    struct Loud(std::thread::ThreadId);
    impl Drop for Loud {
        fn drop(&mut self) {
            QUIET
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|(id, _)| *id != self.0);
        }
    }
    let id = std::thread::current().id();
    QUIET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, quiet));
    Loud(id)
}

/// Whether Windows says this computer is presenting, or runs a full-screen
/// app or game: its own "do not disturb" for notifications. A test seam,
/// `POCKETDESK_PRESENTING_FILE`, answers by whether that file exists.
#[cfg_attr(test, allow(dead_code))]
fn presenting() -> bool {
    presenting_with(std::env::var_os("POCKETDESK_PRESENTING_FILE"))
}

/// [`presenting`], with the seam's file (P3 review: tested without
/// changing the process's environment).
fn presenting_with(seam: Option<std::ffi::OsString>) -> bool {
    if let Some(path) = seam {
        return std::path::Path::new(&path).exists();
    }
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{
            QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
            SHQueryUserNotificationState,
        };
        // SAFETY: a query with no arguments but its answer.
        unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| {
            [
                QUNS_BUSY,
                QUNS_RUNNING_D3D_FULL_SCREEN,
                QUNS_PRESENTATION_MODE,
            ]
            .contains(&state)
        })
    }
    #[cfg(target_os = "linux")]
    {
        linux::do_not_disturb()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

/// Q4: the desktop's own Do Not Disturb on Linux, as GNOME and KDE say it.
/// A thread of its own asks every 5 s while "While presenting" is on, each
/// with a second's deadline, and the handlers only read its last answer
/// (Q4 review: asked on the runtime's threads under a lock, a hung
/// `gsettings` held every notification behind it). A desktop that says
/// nothing is not quiet; the first answer comes within a second of the
/// first ask.
#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    static QUIET: AtomicBool = AtomicBool::new(false);
    static ASKING: AtomicBool = AtomicBool::new(false);

    pub fn do_not_disturb() -> bool {
        if !ASKING.swap(true, Ordering::AcqRel) {
            let _ = std::thread::Builder::new()
                .name("do-not-disturb".into())
                .spawn(|| {
                    loop {
                        let quiet = gnome() == Some(true) || kde() == Some(true);
                        QUIET.store(quiet, Ordering::Release);
                        std::thread::sleep(Duration::from_secs(5));
                        // Ends with the switch; the next ask starts it again.
                        if !crate::events::current_config().quiet_presenting {
                            QUIET.store(false, Ordering::Release);
                            ASKING.store(false, Ordering::Release);
                            break;
                        }
                    }
                })
                .inspect_err(|_| ASKING.store(false, Ordering::Release));
        }
        QUIET.load(Ordering::Acquire)
    }

    /// GNOME: banners off is its Do Not Disturb.
    fn gnome() -> Option<bool> {
        let out = output_within(
            "gsettings",
            &["get", "org.gnome.desktop.notifications", "show-banners"],
        )?;
        super::gnome_quiet(&out)
    }

    /// KDE: its notification server says it is inhibited.
    fn kde() -> Option<bool> {
        let out = output_within(
            "gdbus",
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.Notifications",
                "--object-path",
                "/org/freedesktop/Notifications",
                "--method",
                "org.freedesktop.DBus.Properties.Get",
                "org.freedesktop.Notifications",
                "Inhibited",
            ],
        )?;
        super::inhibited(&out)
    }

    /// A command's output, or none when it fails or takes over a second.
    fn output_within(program: &str, args: &[&str]) -> Option<String> {
        use std::io::Read;
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = String::new();
            let _ = stdout.read_to_string(&mut out);
            out
        });
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return reader.join().ok(),
                Ok(Some(_)) | Err(_) => return None,
                Ok(None) if Instant::now() >= end => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

/// Q4: GNOME's `show-banners` answer read: `false` is Do Not Disturb.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gnome_quiet(out: &str) -> Option<bool> {
    match out.trim() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// Q4: `gdbus`'s answer for KDE's `Inhibited`, such as `(<true>,)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn inhibited(out: &str) -> Option<bool> {
    let out = out.trim();
    if out.contains("<true>") {
        Some(true)
    } else if out.contains("<false>") {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q4: GNOME's and KDE's answers read; anything else says nothing.
    #[test]
    fn the_desktops_do_not_disturb_is_read() {
        assert_eq!(gnome_quiet("false\n"), Some(true));
        assert_eq!(gnome_quiet("true\n"), Some(false));
        assert_eq!(gnome_quiet("No such schema"), None);
        assert_eq!(inhibited("(<true>,)\n"), Some(true));
        assert_eq!(inhibited("(<false>,)"), Some(false));
        assert_eq!(inhibited("Error: GDBus.Error"), None);
    }

    fn at(weekday: u8, minute: u16) -> Option<LocalTime> {
        Some(LocalTime { weekday, minute })
    }

    #[test]
    fn the_schedule_holds_only_within_its_hours() {
        let mut config = Config {
            quiet_hours: true,
            ..Config::default()
        };
        assert_eq!(
            decide(&config, at(0, 23 * 60), || false),
            Some(Quiet::Hours)
        );
        assert_eq!(decide(&config, at(0, 12 * 60), || false), None);
        assert_eq!(
            decide(&config, None, || false),
            None,
            "no local time: not quiet"
        );
        config.quiet_hours = false;
        assert_eq!(decide(&config, at(0, 23 * 60), || false), None);
    }

    #[test]
    fn presenting_holds_only_with_its_switch_on_and_is_asked_only_then() {
        let mut config = Config::default();
        let asked = std::cell::Cell::new(false);
        assert_eq!(
            decide(&config, at(0, 12 * 60), || {
                asked.set(true);
                true
            }),
            None
        );
        assert!(!asked.get(), "not asked with the switch off");
        config.quiet_presenting = true;
        assert_eq!(
            decide(&config, at(0, 12 * 60), || true),
            Some(Quiet::Presenting)
        );
        assert_eq!(decide(&config, at(0, 12 * 60), || false), None);
    }

    #[test]
    fn the_seam_answers_by_its_file() {
        let path = std::env::temp_dir().join(format!("pd-presenting-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(!presenting_with(Some(path.clone().into())));
        std::fs::write(&path, b"").unwrap();
        assert!(presenting_with(Some(path.clone().into())));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_test_is_quiet_only_by_its_guard() {
        assert_eq!(now(), None);
        let guard = quiet_for_test(Quiet::Presenting);
        assert_eq!(now(), Some(Quiet::Presenting));
        drop(guard);
        assert_eq!(now(), None);
    }

    #[test]
    fn each_reason_has_its_field() {
        assert_eq!(Quiet::Hours.field(), "quiet_hours");
        assert_eq!(Quiet::Presenting.field(), "presenting");
    }
}
