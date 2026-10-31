//! Fine scrolling: a wheel turn in 120ths of a notch, as the kernel's
//! `REL_WHEEL_HI_RES`, libinput's `value120` and Windows' `WHEEL_DELTA`
//! count it. A mouse wheel turns 120 a notch; a touchpad or a high-resolution
//! wheel turns less, many times.
//!
//! A sender that has finer turns than notches sends them beside the whole
//! notches they add up to ([`InputPacket::MouseScroll`]'s `fine`), so an older
//! receiver, which reads only the notches, still scrolls by the same total.
//! [`Notches`] is that adding up, kept between turns, for senders and for the
//! backends that can only inject whole notches.

use std::sync::atomic::{AtomicBool, Ordering};

use super::input_packet::InputPacket;

/// One wheel notch, in the fine units.
pub const NOTCH: i32 = 120;

/// One notch as smooth scroll, in the axis units a compositor gets from a
/// mouse (libinput's 15 degrees a notch).
pub const NOTCH_PIXELS: f64 = 15.0;

/// A fine turn (`v120`) as smooth scroll in [`NOTCH_PIXELS`] units.
pub fn pixels(v120: i32) -> f64 {
    f64::from(v120) * NOTCH_PIXELS / f64::from(NOTCH)
}

/// Fine turns not yet whole notches, each way: what a sender or a
/// notch-only backend keeps between turns so slow scrolling adds up instead
/// of being lost. A turn the other way starts its axis over, so a remainder
/// left from scrolling down never eats the first part of scrolling up.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Notches {
    rest: (i32, i32),
}

impl Notches {
    /// Add a turn of `x` and `y` 120ths; the whole notches it completes on
    /// each axis, in the same signs. The rest is kept.
    pub fn add(&mut self, x: i32, y: i32) -> (i32, i32) {
        (add_axis(&mut self.rest.0, x), add_axis(&mut self.rest.1, y))
    }

    /// Forget what was not yet a notch (a new gesture, another computer).
    pub fn clear(&mut self) {
        self.rest = (0, 0);
    }

    /// What is kept, each way.
    pub fn rest(&self) -> (i32, i32) {
        self.rest
    }
}

fn add_axis(rest: &mut i32, v: i32) -> i32 {
    if v == 0 {
        return 0;
    }
    if (*rest > 0 && v < 0) || (*rest < 0 && v > 0) {
        *rest = 0;
    }
    *rest = rest.saturating_add(v);
    let whole = *rest / NOTCH;
    *rest -= whole * NOTCH;
    whole
}

/// `v` as an `i16`, saturating.
pub fn clamp16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// What a sender of [`InputPacket::MouseScroll`] keeps: whether it sends
/// the fine turn (the "Smooth scrolling" setting) and the notches that turn
/// adds up to so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelSender {
    pub smooth: bool,
    notches: Notches,
}

impl Default for WheelSender {
    fn default() -> Self {
        WheelSender::new(true)
    }
}

impl WheelSender {
    pub fn new(smooth: bool) -> Self {
        WheelSender {
            smooth,
            notches: Notches::default(),
        }
    }

    /// A turn of `x` and `y` 120ths in the wire's signs (positive up, and
    /// right) as the packet to send, if any: with smooth scrolling, every
    /// turn, carrying the fine values and the whole notches completed by it;
    /// without, only a turn that completes a notch, as notches alone (what
    /// every build before fine scrolling sent).
    pub fn turn(&mut self, x: i32, y: i32) -> Option<InputPacket> {
        // What the wire can carry, so old and new receivers scroll alike.
        let (x, y) = (i32::from(clamp16(x)), i32::from(clamp16(y)));
        if x == 0 && y == 0 {
            return None;
        }
        let (nx, ny) = self.notches.add(x, y);
        if self.smooth {
            Some(InputPacket::MouseScroll {
                dx: clamp16(nx),
                dy: clamp16(ny),
                fine: Some((x as i16, y as i16)),
            })
        } else if nx != 0 || ny != 0 {
            Some(InputPacket::MouseScroll {
                dx: clamp16(nx),
                dy: clamp16(ny),
                fine: None,
            })
        } else {
            None
        }
    }

    /// Forget what was not yet a notch.
    pub fn clear(&mut self) {
        self.notches.clear();
    }
}

/// "Smooth scrolling" as this process last read it, for paths that must
/// not read a file (the server's shared-input channels). On until told.
static SMOOTH: AtomicBool = AtomicBool::new(true);

/// Note the setting as read.
pub fn set_smooth(on: bool) {
    SMOOTH.store(on, Ordering::Relaxed);
}

/// The setting as last noted.
pub fn smooth() -> bool {
    SMOOTH.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_turns_add_up_to_notches_and_keep_the_rest() {
        let mut n = Notches::default();
        // A touchpad's slow scroll: 30 at a time, a notch every fourth.
        let got: Vec<_> = (0..8).map(|_| n.add(0, 30).1).collect();
        assert_eq!(got, [0, 0, 0, 1, 0, 0, 0, 1]);
        assert_eq!(n.rest(), (0, 0));
        assert_eq!(n.add(0, 50), (0, 0));
        assert_eq!(n.add(0, 200), (0, 2));
        assert_eq!(n.rest(), (0, 10));
        // Down as well as up, truncated toward zero.
        assert_eq!(n.add(-250, 0), (-2, 0));
        assert_eq!(n.rest(), (-10, 10));
    }

    #[test]
    fn a_turn_the_other_way_starts_over() {
        let mut n = Notches::default();
        assert_eq!(n.add(0, 110), (0, 0));
        // Without the start over, 20 back would make a notch up here.
        assert_eq!(n.add(0, -20), (0, 0));
        assert_eq!(n.rest(), (0, -20));
        assert_eq!(n.add(0, -100), (0, -1));
        n.clear();
        assert_eq!(n.rest(), (0, 0));
    }

    #[test]
    fn whole_notches_go_as_they_are() {
        let mut n = Notches::default();
        assert_eq!(n.add(120, -360), (1, -3));
        assert_eq!(n.rest(), (0, 0));
        assert_eq!(n.add(i32::MAX, 0).0, i32::MAX / NOTCH);
    }

    #[test]
    fn a_smooth_sender_sends_every_turn_with_its_notches() {
        let mut s = WheelSender::new(true);
        let fine = |p: Option<InputPacket>| match p {
            Some(InputPacket::MouseScroll { dx, dy, fine }) => (dx, dy, fine),
            other => panic!("{other:?}"),
        };
        assert_eq!(fine(s.turn(0, 40)), (0, 0, Some((0, 40))));
        assert_eq!(fine(s.turn(0, 40)), (0, 0, Some((0, 40))));
        // The third completes a notch: an older receiver scrolls once.
        assert_eq!(fine(s.turn(0, 40)), (0, 1, Some((0, 40))));
        assert_eq!(fine(s.turn(-130, 0)), (-1, 0, Some((-130, 0))));
        assert!(s.turn(0, 0).is_none());
        // Too far for the wire saturates, the notches with it.
        assert_eq!(fine(s.turn(0, 100_000)), (0, 273, Some((0, i16::MAX))));
    }

    #[test]
    fn without_smooth_only_whole_notches_go() {
        let mut s = WheelSender::new(false);
        assert!(s.turn(0, 60).is_none());
        match s.turn(0, 60) {
            Some(InputPacket::MouseScroll { dx, dy, fine }) => {
                assert_eq!((dx, dy, fine), (0, 1, None));
            }
            other => panic!("{other:?}"),
        }
        assert!(s.turn(0, 60).is_none());
        s.clear();
        assert!(s.turn(0, 60).is_none());
    }

    #[test]
    fn a_notch_is_fifteen_pixels() {
        assert_eq!(pixels(120), 15.0);
        assert_eq!(pixels(-30), -3.75);
        assert_eq!(pixels(8), 1.0);
    }

    #[test]
    fn values_beyond_an_i16_saturate() {
        assert_eq!(clamp16(40_000), i16::MAX);
        assert_eq!(clamp16(-40_000), i16::MIN);
        assert_eq!(clamp16(-5), -5);
    }
}
