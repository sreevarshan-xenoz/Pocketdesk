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
    /// Presenting, or something in full screen (Windows).
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

/// In tests, never: a hold must not depend on the time the tests run at,
/// nor on the owner's config.
#[cfg(test)]
pub fn now() -> Option<Quiet> {
    None
}

/// Whether Windows says this computer is presenting, or runs a full-screen
/// app or game: its own "do not disturb" for notifications. A test seam,
/// `POCKETDESK_PRESENTING_FILE`, answers by whether that file exists.
#[cfg_attr(test, allow(dead_code))]
fn presenting() -> bool {
    if let Some(path) = std::env::var_os("POCKETDESK_PRESENTING_FILE") {
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
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("POCKETDESK_PRESENTING_FILE", &path) };
        assert!(!presenting());
        std::fs::write(&path, b"").unwrap();
        assert!(presenting());
        let _ = std::fs::remove_file(&path);
        unsafe { std::env::remove_var("POCKETDESK_PRESENTING_FILE") };
    }

    #[test]
    fn each_reason_has_its_field() {
        assert_eq!(Quiet::Hours.field(), "quiet_hours");
        assert_eq!(Quiet::Presenting.field(), "presenting");
    }
}
