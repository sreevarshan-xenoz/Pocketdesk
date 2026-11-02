//! A still screen sends nothing to a client that needs no idle repeats.
//!
//! Three capture backends (wlroots screencopy, Windows Graphics Capture and
//! DXGI) repeat the last frame once a second while the screen is still,
//! because the phone's decoder reads ten silent seconds as a dead link. Another
//! computer's window does not: it holds the link by the connection's own
//! keepalive, and shows the last picture for as long as it likes. Such a client
//! says so with [`super::client::CODEC_CAP_NO_IDLE_REPEATS`], and the encode
//! task then drops each frame the capture marks as a repeat before it reaches
//! the encoder. Dropped there, not after encoding, so the picture chain the
//! client decodes stays whole and the sequence numbers stay without a gap.
//!
//! Three things still go through on a still screen:
//!
//! - a changed frame, always;
//! - a repeat while a fresh picture is owed (the client asked for a keyframe,
//!   the crop moved, the capture was pointed at another window): the next
//!   repeat is the picture that answers it, so a keyframe asked for on a still
//!   screen waits for the capture's next heartbeat, a second at most, as before;
//! - the first [`SETTLE_REPEATS`] repeats after a change, so the encoder's rate
//!   control gets the same still input a few times and sharpens the picture it
//!   first sent coarse, as it did when every repeat was sent.

/// How many repeats after a changed frame are still sent, for the encoder to
/// refine a picture it sent coarse. A repeat is a few hundred bytes of
/// skipped blocks on a still screen, so these cost close to nothing.
pub const SETTLE_REPEATS: u32 = 2;

/// Whether a frame goes on to the encoder.
///
/// `no_repeats`: the session needs no idle repeats. `repeat`: the capture
/// marked the frame as the idle heartbeat. `refresh_due`: a fresh picture is
/// owed (a keyframe asked for, or the crop or window changed).
/// `repeats_passed`: repeats sent since the last changed frame.
pub fn feed(no_repeats: bool, repeat: bool, refresh_due: bool, repeats_passed: u32) -> bool {
    !(no_repeats && repeat && !refresh_due && repeats_passed >= SETTLE_REPEATS)
}

/// The decision for one session, with what it has to remember.
#[derive(Debug, Clone, Default)]
pub struct RepeatGate {
    no_repeats: bool,
    refresh_due: bool,
    passed: u32,
    skipped: u64,
}

impl RepeatGate {
    /// A gate for a session that needs no idle repeats, or not.
    pub fn new(no_repeats: bool) -> Self {
        Self {
            no_repeats,
            ..Self::default()
        }
    }

    /// A fresh picture is owed: the next frame, repeat or not, goes through.
    pub fn refresh(&mut self) {
        self.refresh_due = true;
    }

    /// Whether this frame goes on to the encoder; remembers it.
    pub fn admit(&mut self, repeat: bool) -> bool {
        let go = feed(self.no_repeats, repeat, self.refresh_due, self.passed);
        if !go {
            self.skipped += 1;
        } else if !repeat || self.refresh_due {
            self.passed = 0;
            self.refresh_due = false;
        } else {
            self.passed = self.passed.saturating_add(1);
        }
        go
    }

    /// How many repeats were dropped.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision, for every way a frame can come.
    #[test]
    fn a_still_screen_is_dropped_only_for_a_client_that_needs_no_repeats() {
        // A changed frame always goes.
        for no_repeats in [false, true] {
            for refresh_due in [false, true] {
                for passed in [0, SETTLE_REPEATS, 99] {
                    assert!(feed(no_repeats, false, refresh_due, passed));
                }
            }
        }
        // A client that did not say it needs none (a phone, an older
        // computer's window) gets every repeat, as before.
        assert!(feed(false, true, false, 99));
        assert!(feed(false, true, true, 99));
        // One that did gets the settling ones, then none.
        assert!(feed(true, true, false, 0));
        assert!(feed(true, true, false, SETTLE_REPEATS - 1));
        assert!(!feed(true, true, false, SETTLE_REPEATS));
        assert!(!feed(true, true, false, 99));
        // Unless a fresh picture is owed: a keyframe asked for.
        assert!(feed(true, true, true, 99));
    }

    #[test]
    fn a_repeat_after_a_change_settles_and_then_stops() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false), "the change");
        assert!(gate.admit(true), "first settling repeat");
        assert!(gate.admit(true), "second settling repeat");
        assert!(!gate.admit(true), "then nothing");
        assert!(!gate.admit(true));
        assert_eq!(gate.skipped(), 2);
        // A change starts the count again.
        assert!(gate.admit(false));
        assert!(gate.admit(true));
        assert!(gate.admit(true));
        assert!(!gate.admit(true));
        assert_eq!(gate.skipped(), 3);
    }

    #[test]
    fn a_keyframe_asked_for_on_a_still_screen_is_answered_by_the_next_repeat() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false));
        for _ in 0..SETTLE_REPEATS {
            assert!(gate.admit(true));
        }
        assert!(!gate.admit(true), "still");
        gate.refresh();
        assert!(gate.admit(true), "the keyframe's picture");
        // It is answered once; the repeats after it settle and stop again.
        assert!(gate.admit(true));
        assert!(gate.admit(true));
        assert!(!gate.admit(true));
        assert_eq!(gate.skipped(), 2);
    }

    #[test]
    fn a_session_that_needs_the_repeats_drops_none() {
        let mut gate = RepeatGate::new(false);
        for _ in 0..50 {
            assert!(gate.admit(true));
        }
        assert_eq!(gate.skipped(), 0);
        assert_eq!(RepeatGate::default().skipped(), 0);
        assert!(RepeatGate::default().admit(true), "drops none");
    }
}
