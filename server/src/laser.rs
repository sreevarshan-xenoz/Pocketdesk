//! The phone's laser dot (G4): a red dot the air mouse moves on the
//! computer's screen without moving its pointer, for a talk. Where it is
//! and when it hides is decided here, kept apart from the screen so it is
//! tested; the dot is drawn by `laser_windows`, or on Linux by
//! `laser_linux` on a compositor with the layer shell (N10); elsewhere it
//! is not drawn, and the phone is not offered it: [`available`].

use std::time::{Duration, Instant};

/// The dot's size in pixels on a 1080-line screen; larger screens get a
/// larger dot (G4 review: 24 physical pixels were a speck on a 4K one).
pub const DOT_PX: i32 = 24;

/// The dot's size on the screen holding `at`: [`DOT_PX`] per 1080 lines,
/// never smaller.
pub fn dot_px(screens: &[Area], at: (i32, i32)) -> i32 {
    screens
        .iter()
        .find(|s| s.contains(at.0, at.1))
        .map_or(DOT_PX, |s| (s.height * DOT_PX / 1080).max(DOT_PX))
}

/// A dot still this long hides by itself: the phone's release may be lost.
pub const IDLE: Duration = Duration::from_secs(2);

/// A rectangle of the desktop, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.left + self.width && y >= self.top && y < self.top + self.height
    }

    fn middle(&self) -> (i32, i32) {
        (self.left + self.width / 2, self.top + self.height / 2)
    }
}

/// Where the dot is, if it is shown.
#[derive(Debug, Default)]
pub struct Dot {
    at: Option<(i32, i32)>,
    moved: Option<Instant>,
}

impl Dot {
    /// Move the dot by `dx`/`dy` over `screens` at `now`, showing it in the
    /// middle of the screen holding `pointer` when it was hidden; the dot is
    /// held to the screens. Its new place, or `None` with no screen known.
    pub fn moved(
        &mut self,
        dx: i16,
        dy: i16,
        screens: &[Area],
        pointer: (i32, i32),
        now: Instant,
    ) -> Option<(i32, i32)> {
        let start = match self.at {
            Some(at) => at,
            None => screens
                .iter()
                .find(|s| s.contains(pointer.0, pointer.1))
                .or_else(|| screens.first())?
                .middle(),
        };
        let wanted = (start.0 + i32::from(dx), start.1 + i32::from(dy));
        let at = held_to(wanted, start, screens)?;
        self.at = Some(at);
        self.moved = Some(now);
        Some(at)
    }

    /// The dot was still too long, its release likely lost (G4 review): it
    /// is hidden, but keeps its place, so the next move goes on from there.
    pub fn rest(&mut self) {
        self.moved = None;
    }

    /// Hide the dot; the next move starts it again from the pointer's screen.
    pub fn hide(&mut self) {
        self.at = None;
        self.moved = None;
    }

    /// Whether the dot is shown and may soon be idle: its thread then
    /// checks often, and rarely while it is hidden.
    pub fn idle_soon(&self) -> bool {
        self.moved.is_some()
    }

    /// Whether the dot has been still long enough to hide by itself.
    pub fn idle(&self, now: Instant) -> bool {
        self.moved
            .is_some_and(|moved| now.saturating_duration_since(moved) >= IDLE)
    }

    pub fn shown(&self) -> bool {
        self.at.is_some()
    }
}

/// `wanted`, kept where it is on a screen; else held to the edge of the
/// screen the dot was on at `from`, as a pointer is, so it never stops in
/// the gap beside a smaller screen or jumps to another one's corner.
fn held_to(wanted: (i32, i32), from: (i32, i32), screens: &[Area]) -> Option<(i32, i32)> {
    if screens.iter().any(|s| s.contains(wanted.0, wanted.1)) {
        return Some(wanted);
    }
    let on = screens
        .iter()
        .find(|s| s.contains(from.0, from.1))
        .or_else(|| screens.first())?;
    Some((
        wanted.0.clamp(on.left, on.left + on.width.max(1) - 1),
        wanted.1.clamp(on.top, on.top + on.height.max(1) - 1),
    ))
}

/// Whether this computer can draw the dot: Windows, or a Wayland
/// compositor with the layer shell (N10; GNOME, KDE and X11 have none).
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    return compositor_draws();
    #[cfg(not(target_os = "linux"))]
    cfg!(windows)
}

/// How often the compositor is asked again whether it draws the dot.
#[cfg(target_os = "linux")]
const ASK_EVERY: Duration = Duration::from_secs(60);

/// The longest the first answer is waited for.
#[cfg(target_os = "linux")]
const FIRST_WAIT: Duration = Duration::from_millis(300);

/// The compositor's answer, kept by a thread of its own that asks it each
/// [`ASK_EVERY`] (N10 review: asked on the accept loop for every
/// connection, a stalled compositor stopped the server taking any).
#[cfg(target_os = "linux")]
fn compositor_draws() -> bool {
    use std::sync::Once;
    use std::sync::atomic::{AtomicU8, Ordering};
    const UNKNOWN: u8 = 0;
    const YES: u8 = 1;
    const NO: u8 = 2;
    static ANSWER: AtomicU8 = AtomicU8::new(UNKNOWN);
    static ASKER: Once = Once::new();
    ASKER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("laser-ask".into())
            .spawn(|| {
                loop {
                    let draws = pocketdesk_core::streaming::laser_dot::available();
                    ANSWER.store(if draws { YES } else { NO }, Ordering::Relaxed);
                    std::thread::sleep(ASK_EVERY);
                }
            });
    });
    let until = Instant::now() + FIRST_WAIT;
    while ANSWER.load(Ordering::Relaxed) == UNKNOWN && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    ANSWER.load(Ordering::Relaxed) == YES
}

/// The phone's laser packet: move the dot, or hide it with `show: false`.
pub fn handle(dx: i16, dy: i16, show: bool) {
    #[cfg(windows)]
    crate::laser_windows::handle(dx, dy, show);
    #[cfg(target_os = "linux")]
    crate::laser_linux::handle(dx, dy, show);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = (dx, dy, show);
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Area = Area {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    };
    const RIGHT: Area = Area {
        left: 1920,
        top: 0,
        width: 1280,
        height: 720,
    };

    #[test]
    fn a_dot_starts_in_the_middle_of_the_pointer_s_screen_and_moves_from_there() {
        let now = Instant::now();
        let mut dot = Dot::default();
        assert!(!dot.shown());
        assert_eq!(
            dot.moved(0, 0, &[LEFT, RIGHT], (2000, 100), now),
            Some((2560, 360))
        );
        assert_eq!(
            dot.moved(-10, 20, &[LEFT, RIGHT], (0, 0), now),
            Some((2550, 380)),
            "from where it is, not the pointer"
        );
        dot.hide();
        assert!(!dot.shown());
        assert_eq!(
            dot.moved(0, 0, &[LEFT, RIGHT], (5, 5), now),
            Some((960, 540)),
            "shown again on the pointer's screen"
        );
    }

    #[test]
    fn a_dot_is_held_to_the_screens_never_in_the_gap_between_them() {
        let now = Instant::now();
        let mut dot = Dot::default();
        dot.moved(0, 0, &[LEFT, RIGHT], (2000, 100), now);
        // Straight down from the small right screen: its bottom edge.
        assert_eq!(
            dot.moved(0, 2000, &[LEFT, RIGHT], (0, 0), now),
            Some((2560, 719))
        );
        // Into the left screen, then far left: that screen's edge.
        assert_eq!(
            dot.moved(-1000, 0, &[LEFT, RIGHT], (0, 0), now),
            Some((1560, 719))
        );
        assert_eq!(
            dot.moved(-32_000, 0, &[LEFT, RIGHT], (0, 0), now),
            Some((0, 719))
        );
        // Right again past the small screen's bottom row: held on the left one.
        dot.moved(0, 900, &[LEFT, RIGHT], (0, 0), now);
        assert_eq!(
            dot.moved(32_000, 0, &[LEFT, RIGHT], (0, 0), now),
            Some((1919, 1079))
        );
        assert_eq!(
            Dot::default().moved(1, 1, &[], (0, 0), now),
            None,
            "no screen"
        );
    }

    #[test]
    fn a_rested_dot_goes_on_from_its_place_and_a_hidden_one_starts_again() {
        let now = Instant::now();
        let mut dot = Dot::default();
        dot.moved(0, 0, &[LEFT], (0, 0), now);
        dot.moved(100, 0, &[LEFT], (0, 0), now);
        dot.rest();
        assert!(!dot.idle(now + IDLE), "rested: not idle again");
        assert_eq!(dot.moved(10, 0, &[LEFT], (0, 0), now), Some((1070, 540)));
        dot.hide();
        assert_eq!(dot.moved(10, 0, &[LEFT], (0, 0), now), Some((970, 540)));
    }

    #[test]
    fn a_dot_grows_with_its_screen() {
        let tall = Area {
            left: 1920,
            top: 0,
            width: 3840,
            height: 2160,
        };
        assert_eq!(dot_px(&[LEFT, tall], (10, 10)), DOT_PX);
        assert_eq!(dot_px(&[LEFT, tall], (2000, 10)), 48);
        assert_eq!(dot_px(&[RIGHT], (2000, 10)), DOT_PX, "never smaller");
        assert_eq!(dot_px(&[], (0, 0)), DOT_PX);
    }

    #[test]
    fn a_still_dot_hides_after_two_seconds() {
        let now = Instant::now();
        let mut dot = Dot::default();
        assert!(!dot.idle(now), "a hidden dot is not idle");
        dot.moved(1, 1, &[LEFT], (0, 0), now);
        assert!(!dot.idle(now + Duration::from_millis(1_900)));
        assert!(dot.idle(now + IDLE));
    }
}
