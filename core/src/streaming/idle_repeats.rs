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
//!   the crop moved, the capture was pointed at another window): repeats keep
//!   passing, at the capture's one a second, until the encoder has actually
//!   put a keyframe out ([`RepeatGate::keyframe_sent`]). An encoder that takes
//!   the request at once (in-process) answers on the next repeat; the FFmpeg
//!   sidecar cannot be told to emit one and answers at its next scheduled
//!   keyframe, so for it the repeats go on as they did before this gate;
//! - the first repeats after a change, [`SETTLE_REPEATS`] plus however many
//!   frames the encoder still holds, so the picture comes out of an encoder
//!   that keeps frames until more input arrives (AMF holds one, Media
//!   Foundation more), and its rate control gets the same still input a few
//!   times and sharpens the picture it first sent coarse.
//!
//! A repeat that replaced a waiting frame in the capture's one-slot channel
//! counts as a change ([`RepeatGate::admit`]), and a screencopy heartbeat is
//! marked a repeat only when its pixels are the last frame's ([`StillPixels`]).

/// How many repeats after a changed frame are still sent, on top of the
/// frames the encoder holds, for it to refine a picture it sent coarse. A
/// repeat is a few hundred bytes of skipped blocks on a still screen, so these
/// cost close to nothing.
pub const SETTLE_REPEATS: u32 = 2;

/// How many repeats after a changed frame still go through when the encoder
/// holds `held` frames: those come out only as more input arrives.
pub fn settle_repeats(held: usize) -> u32 {
    SETTLE_REPEATS.saturating_add(u32::try_from(held).unwrap_or(u32::MAX))
}

/// Whether a frame goes on to the encoder.
///
/// `no_repeats`: the session needs no idle repeats. `repeat`: the capture
/// marked the frame as the idle heartbeat. `refresh_due`: a fresh picture is
/// owed (a keyframe asked for, or the crop or window changed).
/// `repeats_passed`: repeats sent since the last changed frame. `held`:
/// frames the encoder holds until more input arrives.
pub fn feed(
    no_repeats: bool,
    repeat: bool,
    refresh_due: bool,
    repeats_passed: u32,
    held: usize,
) -> bool {
    !(no_repeats && repeat && !refresh_due && repeats_passed >= settle_repeats(held))
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

    /// A fresh picture is owed: every frame, repeat or not, goes through
    /// until [`keyframe_sent`](Self::keyframe_sent).
    pub fn refresh(&mut self) {
        self.refresh_due = true;
    }

    /// The encoder put a keyframe out: the fresh picture owed is sent. Not
    /// cleared when a frame passes, because an encoder that cannot be told to
    /// emit a keyframe (the sidecar) answers only at its next scheduled one,
    /// and a frame that passed would not have been it.
    pub fn keyframe_sent(&mut self) {
        self.refresh_due = false;
    }

    /// Whether this frame goes on to the encoder; remembers it.
    ///
    /// `repeat`: the capture marked it as the idle heartbeat. `replaced`: the
    /// capture's one-slot channel replaced a frame still waiting before this
    /// one was taken (`FrameReceiver::last_replaced`): the heartbeat can have
    /// overwritten a changed frame with the same pixels, so this one counts
    /// as the change. `held`: frames the encoder holds until more input
    /// arrives (`VideoEncoder::held_frames`).
    pub fn admit(&mut self, repeat: bool, replaced: bool, held: usize) -> bool {
        let repeat = repeat && !replaced;
        let go = feed(self.no_repeats, repeat, self.refresh_due, self.passed, held);
        if !go {
            self.skipped += 1;
        } else if !repeat || self.refresh_due {
            self.passed = 0;
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

/// Whether a screencopy heartbeat shows the pixels last sent.
///
/// Screencopy's undamaged heartbeat carries a fresh copy of the screen, and a
/// compositor that reports no damage at once (Hyprland) can under-report a
/// change: the heartbeat then is the only way the change gets out. It is a
/// repeat only when its pixels are the last frame's, byte for byte, so one
/// that differs is a real change and goes through the gate. Only the last
/// heartbeat's pixels are kept (one copy a second at most); frames sent for
/// damage are not copied, so the first heartbeat after one is sent as a change.
#[derive(Debug, Default)]
pub struct StillPixels {
    last: Vec<u8>,
    /// `last` holds the pixels of the frame sent last.
    known: bool,
    /// The capture buffer holds pixels no frame has carried yet.
    unsent: bool,
}

impl StillPixels {
    /// A frame was sent for reported damage (or the source changed, or the
    /// buffer was reallocated): its pixels are not kept.
    pub fn changed(&mut self) {
        self.known = false;
        self.unsent = false;
    }

    /// A copy finished, undamaged, and was not sent: the buffer now holds
    /// pixels no frame has carried.
    pub fn unsent(&mut self) {
        self.unsent = true;
    }

    /// A heartbeat with these pixels is sent: whether they are the last
    /// frame's again.
    pub fn heartbeat(&mut self, pixels: &[u8]) -> bool {
        let same = self.known && self.last.as_slice() == pixels;
        if !same {
            self.last.clear();
            self.last.extend_from_slice(pixels);
        }
        self.known = true;
        self.unsent = false;
        same
    }

    /// The buffer's own pixels are sent after a copy that never finished
    /// (wlroots, no damage): the compositor wrote nothing, so they are the
    /// last frame's again unless an undamaged copy filled the buffer since.
    pub fn buffer_repeat(&mut self) -> bool {
        let same = !self.unsent;
        if !same {
            // The buffer's pixels went out, and were not kept.
            self.known = false;
            self.unsent = false;
        }
        same
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
                    assert!(feed(no_repeats, false, refresh_due, passed, 0));
                }
            }
        }
        // A client that did not say it needs none (a phone, an older
        // computer's window) gets every repeat, as before.
        assert!(feed(false, true, false, 99, 0));
        assert!(feed(false, true, true, 99, 0));
        // One that did gets the settling ones, then none.
        assert!(feed(true, true, false, 0, 0));
        assert!(feed(true, true, false, SETTLE_REPEATS - 1, 0));
        assert!(!feed(true, true, false, SETTLE_REPEATS, 0));
        assert!(!feed(true, true, false, 99, 0));
        // Unless a fresh picture is owed: a keyframe asked for.
        assert!(feed(true, true, true, 99, 0));
    }

    #[test]
    fn a_repeat_after_a_change_settles_and_then_stops() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false, false, 0), "the change");
        assert!(gate.admit(true, false, 0), "first settling repeat");
        assert!(gate.admit(true, false, 0), "second settling repeat");
        assert!(!gate.admit(true, false, 0), "then nothing");
        assert!(!gate.admit(true, false, 0));
        assert_eq!(gate.skipped(), 2);
        // A change starts the count again.
        assert!(gate.admit(false, false, 0));
        assert!(gate.admit(true, false, 0));
        assert!(gate.admit(true, false, 0));
        assert!(!gate.admit(true, false, 0));
        assert_eq!(gate.skipped(), 3);
    }

    /// An encoder that holds frames until more input arrives (AMF one, Media
    /// Foundation more) has not put the changed frame out yet: repeats go on
    /// until it has, then the settling ones.
    #[test]
    fn repeats_pass_while_the_encoder_still_holds_the_changed_frame() {
        assert_eq!(settle_repeats(0), SETTLE_REPEATS);
        assert_eq!(settle_repeats(3), SETTLE_REPEATS + 3);
        assert_eq!(settle_repeats(usize::MAX), u32::MAX);
        for held in [1usize, 3] {
            let mut gate = RepeatGate::new(true);
            assert!(gate.admit(false, false, held), "the change");
            for n in 0..settle_repeats(held) {
                assert!(gate.admit(true, false, held), "repeat {n}, {held} held");
            }
            assert!(!gate.admit(true, false, held), "then nothing");
        }
        // Session start: the first picture is held by the encoder (its lag is
        // learned as frames go in), and a still screen sends only repeats
        // after it; they pass until it comes out.
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false, false, 1), "the first picture");
        assert!(gate.admit(true, false, 1), "pushes it out");
    }

    /// A keyframe asked for on a still screen is owed until the encoder puts
    /// one out: a sidecar encoder cannot be told to emit one, and answers
    /// only at its next scheduled keyframe, so passing one frame is not it.
    #[test]
    fn a_keyframe_asked_for_on_a_still_screen_is_owed_until_one_is_sent() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false, false, 0));
        for _ in 0..SETTLE_REPEATS {
            assert!(gate.admit(true, false, 0));
        }
        assert!(!gate.admit(true, false, 0), "still");
        gate.refresh();
        // The sidecar's request only logs: repeats go on, a second apart.
        for n in 0..20 {
            assert!(gate.admit(true, false, 0), "repeat {n} while it is owed");
        }
        // A changed frame in between does not settle the debt either.
        assert!(gate.admit(false, false, 0));
        assert!(gate.admit(true, false, 0));
        // The keyframe leaves the encoder; the repeats after it settle and
        // stop again.
        gate.keyframe_sent();
        assert!(gate.admit(true, false, 0));
        assert!(gate.admit(true, false, 0));
        assert!(!gate.admit(true, false, 0));
        assert_eq!(gate.skipped(), 2);
    }

    /// An encoder that takes the request at once answers on the next repeat.
    #[test]
    fn a_keyframe_taken_at_once_costs_one_repeat() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false, false, 0));
        for _ in 0..SETTLE_REPEATS {
            assert!(gate.admit(true, false, 0));
        }
        assert!(!gate.admit(true, false, 0));
        gate.refresh();
        assert!(gate.admit(true, false, 0), "the keyframe's picture");
        gate.keyframe_sent();
        assert!(gate.admit(true, false, 0));
        assert!(gate.admit(true, false, 0));
        assert!(!gate.admit(true, false, 0));
    }

    /// The capture's one-slot channel can replace a changed frame with a
    /// heartbeat that has its pixels; the heartbeat then stands for the
    /// change and is not dropped.
    #[test]
    fn a_repeat_that_replaced_a_waiting_frame_counts_as_the_change() {
        let mut gate = RepeatGate::new(true);
        assert!(gate.admit(false, false, 0));
        for _ in 0..SETTLE_REPEATS {
            assert!(gate.admit(true, false, 0));
        }
        assert!(!gate.admit(true, false, 0), "a plain repeat is dropped");
        assert!(gate.admit(true, true, 0), "one that replaced a frame goes");
        // And starts the settling repeats after it.
        assert!(gate.admit(true, false, 0));
        assert!(gate.admit(true, false, 0));
        assert!(!gate.admit(true, false, 0));
    }

    #[test]
    fn a_session_that_needs_the_repeats_drops_none() {
        let mut gate = RepeatGate::new(false);
        for _ in 0..50 {
            assert!(gate.admit(true, false, 0));
        }
        assert_eq!(gate.skipped(), 0);
        assert_eq!(RepeatGate::default().skipped(), 0);
        assert!(RepeatGate::default().admit(true, false, 0), "drops none");
    }

    /// Hyprland reports no damage and the heartbeat carries a fresh copy: it
    /// is a repeat only when the pixels are the last frame's.
    #[test]
    fn a_heartbeat_that_differs_from_the_last_frame_is_not_a_repeat() {
        let mut still = StillPixels::default();
        let a = vec![1u8; 64];
        let mut b = a.clone();
        b[63] = 2;
        // After a frame sent for damage, nothing is kept: a change.
        still.changed();
        assert!(!still.heartbeat(&a), "the first one is sent as a change");
        assert!(still.heartbeat(&a), "same pixels again: a repeat");
        assert!(still.heartbeat(&a));
        assert!(!still.heartbeat(&b), "pixels that differ: a real change");
        assert!(still.heartbeat(&b), "and then the new picture is still");
        assert!(!still.heartbeat(&b[..32]), "a different size differs");
        still.changed();
        assert!(!still.heartbeat(&b[..32]), "a frame for damage went between");
    }

    /// A copy that filled the buffer with pixels no frame carried: the buffer
    /// sent after a silent copy is not the last frame again.
    #[test]
    fn the_buffer_sent_after_a_silent_copy_is_a_repeat_only_if_it_is_unchanged() {
        let mut still = StillPixels::default();
        still.changed();
        assert!(still.buffer_repeat(), "the compositor wrote nothing since");
        still.unsent();
        assert!(!still.buffer_repeat(), "an undamaged copy filled it since");
        assert!(still.buffer_repeat(), "sent now, so it is the last again");
        // A heartbeat compares against what was sent last: the buffer sent
        // after unsent pixels was not kept, so the next one is a change.
        let a = vec![5u8; 16];
        assert!(!still.heartbeat(&a));
        still.unsent();
        assert!(still.heartbeat(&a), "compared by pixels, not by the flag");
    }
}
