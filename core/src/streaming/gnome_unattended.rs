//! "Unattended on GNOME": the switch and the choice it makes.
//!
//! GNOME's screen-sharing portal asks someone at the desk, once, in a
//! dialog. With this switch on, and only on a GNOME session, the capture
//! tries mutter's own D-Bus API first (`org.gnome.Mutter.ScreenCast` and
//! `org.gnome.Mutter.RemoteDesktop`, as gnome-remote-desktop's headless
//! mode does), which asks no one. Off, or anywhere else, nothing changes:
//! the portal is used as before. The D-Bus work itself is
//! `capture_mutter` (Linux); this part is plain data, tested anywhere.

use std::sync::atomic::{AtomicBool, Ordering};

/// The owner's switch (`gnome_unattended` in the config, off by default),
/// set by the server before each session starts.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn the switch on or off.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

/// Whether the owner switched it on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Which way a capture goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Mutter's own D-Bus API, with no dialog (and, if it refuses, the
    /// portal after it).
    Mutter,
    /// The screen-sharing portal, which asks at the desk the first time.
    Portal,
}

/// Whether `current_desktop` (`XDG_CURRENT_DESKTOP`: colon separated, as
/// `GNOME` or `ubuntu:GNOME`) names GNOME, by a whole name.
pub fn is_gnome(current_desktop: Option<&str>) -> bool {
    current_desktop.is_some_and(|names| {
        names
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("GNOME"))
    })
}

/// The route for a capture: mutter's only with the switch on and a GNOME
/// session.
pub fn route(switch: bool, current_desktop: Option<&str>) -> Route {
    if switch && is_gnome(current_desktop) {
        Route::Mutter
    } else {
        Route::Portal
    }
}

/// [`route`] for this process: the switch as set, this session's desktop.
pub fn session_route() -> Route {
    route(
        enabled(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    )
}

/// Say, once a process, that mutter's D-Bus path did not work and the
/// portal is used instead (false after the first time). The caller logs
/// it; a host that tries it every session does not repeat itself.
pub fn first_fallback() -> bool {
    static SAID: AtomicBool = AtomicBool::new(false);
    !SAID.swap(true, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_is_named_by_a_whole_name() {
        assert!(is_gnome(Some("GNOME")));
        assert!(is_gnome(Some("gnome")));
        assert!(is_gnome(Some("ubuntu:GNOME")));
        assert!(is_gnome(Some("GNOME-Classic:GNOME")));
        assert!(!is_gnome(Some("GNOME-Flashback")), "a whole name only");
        assert!(!is_gnome(Some("KDE")));
        assert!(!is_gnome(Some("")));
        assert!(!is_gnome(None));
    }

    #[test]
    fn mutter_only_with_the_switch_on_and_a_gnome_session() {
        assert_eq!(route(true, Some("GNOME")), Route::Mutter);
        assert_eq!(route(true, Some("ubuntu:GNOME")), Route::Mutter);
        assert_eq!(route(false, Some("GNOME")), Route::Portal, "off by default");
        assert_eq!(route(true, Some("KDE")), Route::Portal);
        assert_eq!(route(true, Some("sway")), Route::Portal);
        assert_eq!(route(true, None), Route::Portal, "no desktop named");
    }

    #[test]
    fn the_switch_is_off_until_set() {
        // The only test that sets it, so no other one sees it on.
        assert!(!enabled());
        set_enabled(true);
        assert!(enabled());
        assert_eq!(
            route(enabled(), Some("GNOME")),
            Route::Mutter,
            "the switch feeds the route"
        );
        set_enabled(false);
        assert!(!enabled());
    }

    #[test]
    fn a_fallback_is_said_once() {
        assert!(first_fallback());
        assert!(!first_fallback());
    }
}
