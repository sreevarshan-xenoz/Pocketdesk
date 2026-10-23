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
        pocketdesk_core::local::quiet_hours::presenting,
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
        assert!(!pocketdesk_core::local::quiet_hours::presenting_with(Some(
            path.clone().into()
        )));
        std::fs::write(&path, b"").unwrap();
        assert!(pocketdesk_core::local::quiet_hours::presenting_with(Some(
            path.clone().into()
        )));
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
