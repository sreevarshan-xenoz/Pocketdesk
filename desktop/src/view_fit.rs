//! "Fit the screen to this window" for the window that shows another
//! computer's screen (item 8 of the plan after 0.8.3).
//!
//! With it on, the window asks its server (`ComputerAsk::View::fit`) for a
//! screen of the size of its picture area, in pixels; the server asks the
//! other computer, when it runs Hyprland or Sway, to make a headless screen
//! of that size (the server's `fit_screen`) and shows that screen
//! in place of a real one. As the window is resized, a new ask follows once
//! its size has held for [`DEBOUNCE`], and the other computer resizes the
//! same screen. It is an empty extra desktop there: nothing is moved onto
//! it, and it goes when the window closes, turns the switch off or shows
//! another screen. A Windows computer cannot (its screens come from a
//! graphics driver), so the switch is greyed there, and one that fails (no
//! Hyprland or Sway, its owner's switch, no answer, its four screens taken)
//! says why and shows the screen the window asked for.
//!
//! The decisions here are pure and tested; the asks are in
//! [`crate::computer_view`].

use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::local::computers::FitSize;
use pocketdesk_core::local::view::{ViewFit, ViewStatus};

/// How long the picture area must keep a size before the other computer's
/// screen follows it: a drag of the window's edge makes many sizes.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// The switch's label, on the bar and (said in full) in Settings.
pub const LABEL: &str = "Fit the screen to this window";

/// One window's fit: its switch and what it last asked.
#[derive(Debug, Default)]
pub struct FitWindow {
    /// The window's switch: on, it asks for a screen of its size.
    pub on: bool,
    /// Settings' choice, which a window starts from as it begins showing a
    /// computer.
    pub default: bool,
    /// What the picture area measured the last pass it was drawn.
    pub area: Option<FitSize>,
    /// The size the view now held (or being asked for) was asked at; none
    /// when no fit was asked.
    pub asked: Option<FitSize>,
    /// A size the area has held since a time, while it differs from
    /// `asked`.
    pub seen: Option<(FitSize, Instant)>,
    /// A switch from the bar that Settings has not heard yet.
    pub saved: Option<bool>,
}

/// The size of a picture area `points` big at `pixels_per_point`, in
/// pixels with even sides; none for an area too small to be a screen.
pub fn area_size(points: egui::Vec2, pixels_per_point: f32) -> Option<FitSize> {
    let side = |v: f32| {
        let px = (v * pixels_per_point).round();
        (px.is_finite() && px >= 2.0).then(|| (px as u32 / 2) * 2)
    };
    let (width, height) = (side(points.x)?, side(points.y)?);
    // The other computer raises a small side to its own least; an area
    // smaller than a sliver is a window being made, not one to fit.
    (width >= 64 && height >= 64).then_some(FitSize { width, height })
}

/// What a pass does about the area's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing: no fit asked, or the area is the size asked.
    Idle,
    /// The area changed and has not held yet: look again in this long.
    Wait(Duration),
    /// The area held its new size: ask again at it.
    Refit,
}

/// Judge the area `area` at `now` against the size `asked`, debouncing its
/// changes: `seen` is the size the area has held and since when.
pub fn step(
    asked: Option<FitSize>,
    seen: &mut Option<(FitSize, Instant)>,
    area: FitSize,
    now: Instant,
) -> Step {
    let Some(asked) = asked else {
        *seen = None;
        return Step::Idle;
    };
    if area == asked {
        *seen = None;
        return Step::Idle;
    }
    match *seen {
        Some((size, since)) if size == area => {
            let held = now.saturating_duration_since(since);
            if held >= DEBOUNCE {
                *seen = None;
                Step::Refit
            } else {
                Step::Wait(DEBOUNCE - held)
            }
        }
        _ => {
            *seen = Some((area, now));
            Step::Wait(DEBOUNCE)
        }
    }
}

/// Why the switch is not available for the computer in `status`, if it
/// is not: a Windows computer cannot make a screen for a window.
pub fn unavailable(name: &str, status: &ViewStatus) -> Option<String> {
    (status.windows == Some(true)).then(|| {
        format!(
            "Not available: {name} runs Windows, which cannot make a screen for a window here yet."
        )
    })
}

/// The switch's hover words.
pub fn hover(name: &str, status: &ViewStatus) -> String {
    unavailable(name, status).unwrap_or_else(|| {
        format!(
            "Ask {name} to make a screen as big as this window and show that, resized as the window is: an empty extra desktop there, nothing is moved onto it, and it goes when the window closes. Only a Linux computer on Hyprland or Sway can, and its Devices page needs Input control on for this computer."
        )
    })
}

/// The screen the bar marks as shown: the one the other computer made to
/// fit the window when it made one, else the one asked for.
pub fn shown_monitor(asked: u32, status: &ViewStatus) -> u32 {
    match *status.fit {
        ViewFit::Fitted { .. } | ViewFit::Borrowed { .. } => status.monitor,
        _ => asked,
    }
}

/// What the window says of the fit under its bar, if anything.
pub fn words(status: &ViewStatus) -> Option<String> {
    match &*status.fit {
        ViewFit::Off => None,
        ViewFit::Asking => Some("Making a screen to fit this window\u{2026}".to_string()),
        ViewFit::Fitted { width, height } => Some(format!(
            "Showing a {width}\u{d7}{height} screen made to fit this window."
        )),
        ViewFit::Borrowed { .. } => None,
        ViewFit::Failed(why) => Some(format!(
            "Could not fit the screen to this window ({why}); showing the screen asked for."
        )),
    }
}

/// [`words`], or, for a borrowed window ("Borrow a window…"), what the
/// window says while the other computer is asked to lend it.
pub fn words_for(status: &ViewStatus, name: &str, borrowed: bool) -> Option<String> {
    if !borrowed {
        return words(status);
    }
    matches!(*status.fit, ViewFit::Asking)
        .then(|| format!("Asking {name} to lend the window\u{2026}"))
}

/// What a dimmed picture says while the screen is being fitted again.
pub const FITTING: &str = "Fitting the screen to the window\u{2026}";

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> FitSize {
        FitSize { width, height }
    }

    #[test]
    fn the_area_is_measured_in_even_pixels() {
        assert_eq!(
            area_size(egui::vec2(1280.0, 720.0), 1.0),
            Some(size(1280, 720))
        );
        assert_eq!(
            area_size(egui::vec2(1279.6, 719.4), 1.5),
            Some(size(1918, 1078)),
            "scaled, rounded, then even"
        );
        assert_eq!(
            area_size(egui::vec2(1000.5, 600.0), 1.0),
            Some(size(1000, 600))
        );
        for odd in [
            egui::vec2(0.0, 600.0),
            egui::vec2(30.0, 600.0),
            egui::vec2(600.0, 40.0),
            egui::vec2(f32::NAN, 600.0),
            egui::vec2(-5.0, 600.0),
        ] {
            assert_eq!(area_size(odd, 1.0), None, "{odd:?}");
        }
    }

    #[test]
    fn a_change_holds_before_it_is_asked() {
        let t0 = Instant::now();
        let mut seen = None;
        let asked = Some(size(1280, 720));
        // The size asked: nothing to do.
        assert_eq!(step(asked, &mut seen, size(1280, 720), t0), Step::Idle);
        // A new size: look again in the debounce.
        assert_eq!(
            step(asked, &mut seen, size(1000, 600), t0),
            Step::Wait(DEBOUNCE)
        );
        // Not yet held long enough.
        assert_eq!(
            step(
                asked,
                &mut seen,
                size(1000, 600),
                t0 + Duration::from_millis(200)
            ),
            Step::Wait(Duration::from_millis(300))
        );
        // A drag changes it again: the clock starts over.
        assert_eq!(
            step(
                asked,
                &mut seen,
                size(900, 600),
                t0 + Duration::from_millis(300)
            ),
            Step::Wait(DEBOUNCE)
        );
        assert_eq!(
            step(
                asked,
                &mut seen,
                size(900, 600),
                t0 + Duration::from_millis(799)
            ),
            Step::Wait(Duration::from_millis(1))
        );
        // Held: ask again at it.
        assert_eq!(
            step(
                asked,
                &mut seen,
                size(900, 600),
                t0 + Duration::from_millis(800)
            ),
            Step::Refit
        );
        // Asked at that size, nothing more.
        assert_eq!(
            step(
                Some(size(900, 600)),
                &mut seen,
                size(900, 600),
                t0 + Duration::from_secs(2)
            ),
            Step::Idle
        );
    }

    #[test]
    fn a_size_that_changes_and_comes_back_asks_nothing() {
        let t0 = Instant::now();
        let mut seen = None;
        let asked = Some(size(1280, 720));
        step(asked, &mut seen, size(1000, 600), t0);
        assert!(seen.is_some());
        assert_eq!(
            step(
                asked,
                &mut seen,
                size(1280, 720),
                t0 + Duration::from_millis(100)
            ),
            Step::Idle
        );
        assert!(seen.is_none(), "forgotten");
    }

    #[test]
    fn no_fit_asked_asks_nothing_whatever_the_size() {
        let mut seen = Some((size(1, 1), Instant::now()));
        assert_eq!(
            step(None, &mut seen, size(1000, 600), Instant::now()),
            Step::Idle
        );
        assert!(seen.is_none());
    }

    #[test]
    fn a_windows_computer_cannot_and_says_why() {
        let windows = ViewStatus {
            windows: Some(true),
            ..ViewStatus::default()
        };
        let linux = ViewStatus {
            windows: Some(false),
            ..ViewStatus::default()
        };
        let unknown = ViewStatus::default();
        assert!(
            unavailable("Desk", &windows)
                .unwrap()
                .contains("runs Windows")
        );
        assert!(hover("Desk", &windows).starts_with("Not available"));
        assert!(unavailable("Lap", &linux).is_none());
        assert!(unavailable("Lap", &unknown).is_none(), "asked, then told");
        assert!(hover("Lap", &linux).contains("Hyprland or Sway"));
    }

    #[test]
    fn the_bar_marks_the_screen_that_was_made() {
        let fitted = ViewStatus {
            monitor: 2,
            fit: Box::new(ViewFit::Fitted {
                width: 1280,
                height: 720,
            }),
            ..ViewStatus::default()
        };
        assert_eq!(shown_monitor(0, &fitted), 2);
        let failed = ViewStatus {
            monitor: 0,
            fit: Box::new(ViewFit::Failed("no".into())),
            ..ViewStatus::default()
        };
        assert_eq!(shown_monitor(1, &failed), 1, "the one asked for");
        assert_eq!(shown_monitor(1, &ViewStatus::default()), 1);
    }

    #[test]
    fn the_window_says_how_the_fit_stands() {
        let said = |fit| {
            words(&ViewStatus {
                fit: Box::new(fit),
                ..ViewStatus::default()
            })
        };
        assert_eq!(said(ViewFit::Off), None);
        assert!(
            said(ViewFit::Asking)
                .unwrap()
                .starts_with("Making a screen")
        );
        let fitted = said(ViewFit::Fitted {
            width: 1280,
            height: 720,
        })
        .unwrap();
        assert!(fitted.contains("1280\u{d7}720"), "{fitted}");
        let failed = said(ViewFit::Failed("Lap: no link".into())).unwrap();
        assert!(
            failed.contains("Lap: no link") && failed.contains("asked for"),
            "{failed}"
        );
    }
}
