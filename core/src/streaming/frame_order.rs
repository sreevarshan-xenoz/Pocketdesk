//! The phone's video frames in order (BG3). Each frame rides a QUIC stream
//! of its own and the client reads them side by side, so a frame stuck on a
//! lost packet holds up no other stream; this puts the frames that arrive
//! back in sequence for the decoder. A keyframe restarts the picture: every
//! frame before it is moot, so it goes at once and those still coming are
//! skipped (the reader stops their streams, the computer resets them). A
//! hole in the sequence holds the frames after it until it fills, a keyframe
//! comes, or [`GIVE_UP`] passes; past that the frames after it go anyway,
//! as they did before BG3. A frame whose header came is on its way (QUIC
//! resends what is lost), so a hole it fills asks for no keyframe, and a
//! keyframe on its way is never given up: a big one can take longer than
//! the small frames after it.
//!
//! A computer from before BG3 numbers each encoder's frames from 0, so a
//! rebuilt encoder (a new bitrate, size or crop) starts again at a keyframe
//! 0: that starts a new run here, and frames of the old run still arriving
//! (far ahead of the new count) are moot. A computer since BG3 numbers the
//! wire's frames once per session.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::transport::PacketHeader;

/// The longest the frames after a hole wait for it.
pub const GIVE_UP: Duration = Duration::from_millis(500);

/// The most frames held behind a hole: past this the hole is given up.
pub const MAX_HELD: usize = 64;

/// When a hole should ask the computer for a keyframe: a retransmission
/// takes about a round trip and a half, so a frame missing for two round
/// trips (and at least 60 ms) is not coming soon.
pub fn ask_after(rtt: Duration) -> Duration {
    (rtt * 2 + Duration::from_millis(30)).max(Duration::from_millis(60))
}

/// A frame for the decoder, and how long it waited here.
#[derive(Debug)]
pub struct Ready {
    pub header: PacketHeader,
    pub data: Vec<u8>,
    pub held: Duration,
}

#[derive(Debug)]
enum Slot {
    Frame {
        header: PacketHeader,
        data: Vec<u8>,
        at: Instant,
    },
    /// A frame whose stream ended without it (reset or broken).
    Lost,
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct FrameOrder {
    /// The sequence the decoder takes next; `None` until the first
    /// keyframe (or a given-up wait) starts the picture.
    next: Option<u64>,
    held: BTreeMap<u64, Slot>,
    /// Since when a hole has held frames.
    hole_since: Option<Instant>,
    /// Frames whose header came and whose rest is on its way, and whether
    /// each is a keyframe.
    coming: BTreeMap<u64, bool>,
    /// Where the old run had got to when a keyframe 0 started a new one,
    /// while the new run is still far below it.
    restarted_from: Option<u64>,
    /// Frames skipped as moot or lost, for the log.
    pub skipped: u64,
}

impl FrameOrder {
    /// Frames below this are moot: their streams may be stopped.
    pub fn floor(&self) -> u64 {
        self.next.unwrap_or(0)
    }

    /// Since when frames wait behind a hole, if they do.
    pub fn hole_since(&self) -> Option<Instant> {
        self.hole_since
    }

    /// Whether frame `seq` is moot: before the frame the decoder takes
    /// next, or a straggler of a run a keyframe 0 ended. Its stream may be
    /// stopped.
    pub fn is_moot(&self, seq: u64) -> bool {
        self.next.is_some_and(|next| seq < next)
            || self.restarted_from.is_some_and(|old| {
                seq + MAX_HELD as u64 >= old && self.next.unwrap_or(0) + (MAX_HELD as u64) < old
            })
    }

    /// Frame `header`'s header came: the rest is on its way.
    pub fn coming(&mut self, header: &PacketHeader) {
        self.new_run(header);
        if !self.is_moot(header.sequence) {
            self.coming.insert(header.sequence, header.is_keyframe);
        }
    }

    /// A keyframe 0 after the picture started: a rebuilt encoder of a
    /// computer from before BG3 started a new run, and all before it is
    /// moot.
    fn new_run(&mut self, header: &PacketHeader) {
        let Some(old) = self.next else {
            return;
        };
        if header.is_keyframe && header.sequence == 0 && old > 0 {
            self.skipped += self
                .held
                .values()
                .filter(|slot| matches!(slot, Slot::Frame { .. }))
                .count() as u64;
            self.held.clear();
            self.coming.clear();
            self.hole_since = None;
            self.next = None;
            self.restarted_from = Some(old);
        }
    }

    /// Whether a hole should ask the computer for a keyframe at `now`, over
    /// a path whose round trip is `rtt`: frames wait behind it, nothing on
    /// its way fills it (nor, before the first picture, starts it), and it
    /// has waited [`ask_after`].
    pub fn should_ask(&self, now: Instant, rtt: Duration) -> bool {
        let Some(since) = self.hole_since else {
            return false;
        };
        // The frame itself, or any keyframe after it (which makes the hole
        // moot), on its way: asking now would only reset that keyframe for
        // another, and on a slow link the next one too.
        let next = self.next.unwrap_or(0);
        let filling = (self.next.is_some() && self.coming.contains_key(&next))
            || self.coming.range(next..).any(|(_, key)| *key);
        !filling && now.saturating_duration_since(since) >= ask_after(rtt)
    }

    /// Frame `header` arrived whole at `now`: the frames to decode now, in
    /// order.
    pub fn arrived(&mut self, header: PacketHeader, data: Vec<u8>, now: Instant) -> Vec<Ready> {
        let seq = header.sequence;
        self.new_run(&header);
        self.coming.remove(&seq);
        if self.is_moot(seq) {
            self.skipped += 1;
            return Vec::new();
        }
        if header.is_keyframe {
            self.skip_below(seq);
            self.next = Some(seq);
        }
        self.held.insert(
            seq,
            Slot::Frame {
                header,
                data,
                at: now,
            },
        );
        self.drain(now)
    }

    /// Frame `seq` will not arrive (its stream was reset or broke): a hole
    /// that only a keyframe or [`GIVE_UP`] passes.
    pub fn lost(&mut self, seq: u64, now: Instant) -> Vec<Ready> {
        self.coming.remove(&seq);
        if self.is_moot(seq) {
            return Vec::new();
        }
        self.held.entry(seq).or_insert(Slot::Lost);
        self.drain(now)
    }

    /// The frames after a hole that waited past [`GIVE_UP`] (unless a
    /// keyframe on its way would fill it), or past [`MAX_HELD`] of them:
    /// they go anyway, from the first held. The picture is then broken
    /// until a keyframe; the caller asks for one.
    pub fn give_up(&mut self, now: Instant) -> Vec<Ready> {
        // From the first frame held: frames lost before it are passed too.
        let Some(first) = self
            .held
            .iter()
            .find_map(|(seq, slot)| matches!(slot, Slot::Frame { .. }).then_some(*seq))
        else {
            return Vec::new();
        };
        let waited = self
            .hole_since
            .is_some_and(|since| now.saturating_duration_since(since) >= GIVE_UP);
        let key_coming = self.coming.range(..first).any(|(_, key)| *key);
        if self.held.len() <= MAX_HELD && (!waited || key_coming) {
            return Vec::new();
        }
        if let Some(next) = self.next {
            self.skipped += first.saturating_sub(next);
        }
        self.next = Some(first);
        self.coming = self.coming.split_off(&first);
        self.held = self.held.split_off(&first);
        self.drain(now)
    }

    fn skip_below(&mut self, seq: u64) {
        self.coming = self.coming.split_off(&seq);
        let kept = self.held.split_off(&seq);
        self.skipped += self
            .held
            .values()
            .filter(|slot| matches!(slot, Slot::Frame { .. }))
            .count() as u64;
        self.held = kept;
    }

    /// Hand over the frames that follow `next` without a gap. A frame lost
    /// at `next` stays a hole: the frames after it need it.
    fn drain(&mut self, now: Instant) -> Vec<Ready> {
        let mut ready = Vec::new();
        if let Some(mut next) = self.next {
            while matches!(self.held.get(&next), Some(Slot::Frame { .. })) {
                if let Some(Slot::Frame { header, data, at }) = self.held.remove(&next) {
                    ready.push(Ready {
                        header,
                        data,
                        held: now.saturating_duration_since(at),
                    });
                }
                next += 1;
            }
            self.next = Some(next);
            // The new run has caught up with where the old one was: its
            // stragglers are long gone.
            if self
                .restarted_from
                .is_some_and(|old| next + MAX_HELD as u64 >= old)
            {
                self.restarted_from = None;
            }
        }
        // A hole that moved on (frames went) is a new one, timed from now.
        self.hole_since = if self.held.is_empty() {
            None
        } else if ready.is_empty() {
            Some(self.hole_since.unwrap_or(now))
        } else {
            Some(now)
        };
        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::transport::STREAM_KIND_VIDEO;

    fn frame(sequence: u64, is_keyframe: bool) -> (PacketHeader, Vec<u8>) {
        (
            PacketHeader {
                sequence,
                stream_kind: STREAM_KIND_VIDEO,
                is_keyframe,
                timestamp_us: 0,
            },
            vec![sequence as u8],
        )
    }

    fn seqs(ready: &[Ready]) -> Vec<u64> {
        ready.iter().map(|r| r.header.sequence).collect()
    }

    fn arrive(order: &mut FrameOrder, seq: u64, key: bool, now: Instant) -> Vec<u64> {
        let (header, data) = frame(seq, key);
        seqs(&order.arrived(header, data, now))
    }

    #[test]
    fn frames_go_in_order_and_a_late_one_releases_those_behind_it() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert_eq!(arrive(&mut order, 1, false, t), [1]);
        // 2 is stuck on a lost packet; 3 and 4 wait for it.
        assert!(arrive(&mut order, 3, false, t).is_empty());
        assert!(arrive(&mut order, 4, false, t).is_empty());
        assert_eq!(order.hole_since(), Some(t));
        let later = t + Duration::from_millis(30);
        let ready = {
            let (header, data) = frame(2, false);
            order.arrived(header, data, later)
        };
        assert_eq!(seqs(&ready), [2, 3, 4]);
        assert_eq!(
            ready[1].held,
            Duration::from_millis(30),
            "3 says how long it waited"
        );
        assert_eq!(order.hole_since(), None);
        assert_eq!(order.floor(), 5);
    }

    #[test]
    fn a_keyframe_goes_at_once_and_makes_the_frames_before_it_moot() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert_eq!(arrive(&mut order, 5, true, t), [5]);
        assert_eq!(order.floor(), 6, "1 to 4 are moot: their streams may stop");
        assert_eq!(order.skipped, 1, "the held 2");
        assert!(
            arrive(&mut order, 1, false, t).is_empty(),
            "a moot frame arriving late is dropped"
        );
        assert_eq!(order.skipped, 2);
        assert_eq!(arrive(&mut order, 6, false, t), [6]);
        assert_eq!(order.hole_since(), None);
    }

    #[test]
    fn the_first_picture_waits_for_its_keyframe() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        // A delta's stream can finish before the keyframe's.
        assert!(arrive(&mut order, 1, false, t).is_empty());
        assert_eq!(arrive(&mut order, 0, true, t), [0, 1]);
    }

    #[test]
    fn a_lost_frame_is_a_hole_a_keyframe_passes() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(seqs(&order.lost(1, t)).is_empty());
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert_eq!(order.hole_since(), Some(t));
        assert_eq!(arrive(&mut order, 3, true, t), [3]);
        assert_eq!(order.hole_since(), None);
        assert!(seqs(&order.lost(2, t)).is_empty(), "a moot loss is nothing");
    }

    #[test]
    fn a_hole_is_given_up_after_a_while_or_too_many_frames() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert!(arrive(&mut order, 3, false, t).is_empty());
        assert!(
            seqs(&order.give_up(t + GIVE_UP / 2)).is_empty(),
            "still within the wait"
        );
        assert_eq!(seqs(&order.give_up(t + GIVE_UP)), [2, 3]);
        assert_eq!(order.skipped, 1, "1 never came");
        assert_eq!(order.hole_since(), None);

        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        for seq in 2..=(MAX_HELD as u64 + 2) {
            assert!(arrive(&mut order, seq, false, t).is_empty());
        }
        assert_eq!(
            order.give_up(t).len(),
            MAX_HELD + 1,
            "too many held: they go"
        );
    }

    #[test]
    fn a_hole_asks_for_a_keyframe_only_when_nothing_on_its_way_fills_it() {
        let t = Instant::now();
        let rtt = Duration::from_millis(5);
        let later = t + ask_after(rtt);
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert!(!order.should_ask(t, rtt), "not yet");
        assert!(order.should_ask(later, rtt), "1 never even started");

        // A big keyframe still coming, the small frames after it done first.
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        order.coming(&frame(1, true).0);
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert!(!order.should_ask(later, rtt), "the keyframe is on its way");
        assert!(
            order.give_up(t + GIVE_UP * 4).is_empty(),
            "and is never given up"
        );
        assert_eq!(arrive(&mut order, 1, true, later), [1, 2]);

        // The first picture: its keyframe on its way.
        let mut order = FrameOrder::default();
        order.coming(&frame(0, true).0);
        assert!(arrive(&mut order, 1, false, t).is_empty());
        assert!(!order.should_ask(later, rtt));
        assert!(order.give_up(t + GIVE_UP).is_empty());
    }

    /// A computer from before BG3 numbers each encoder from 0: a rebuilt one
    /// starts a new run at a keyframe 0, and the old run's stragglers are
    /// moot.
    #[test]
    fn a_keyframe_0_starts_a_new_run_and_the_old_one_s_stragglers_are_moot() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        for seq in 1..300 {
            assert_eq!(arrive(&mut order, seq, false, t), [seq]);
        }
        assert!(
            arrive(&mut order, 301, false, t).is_empty(),
            "300 still coming"
        );
        order.coming(&frame(0, true).0);
        assert_eq!(order.floor(), 0, "the new run's frames are not moot");
        assert!(!order.is_moot(1));
        assert!(
            order.is_moot(300) && order.is_moot(302),
            "the old run's are"
        );
        assert!(
            arrive(&mut order, 1, false, t).is_empty(),
            "waits for its keyframe"
        );
        assert_eq!(arrive(&mut order, 0, true, t), [0, 1]);
        assert!(
            arrive(&mut order, 300, false, t).is_empty(),
            "a straggler is dropped"
        );
        assert!(
            !order.should_ask(t + GIVE_UP, Duration::ZERO),
            "and leaves no hole"
        );
        assert_eq!(arrive(&mut order, 2, false, t), [2]);
    }

    /// A keyframe on its way past a hole fills it: asking for another would
    /// reset it on the computer, and on a slow link the next one too.
    #[test]
    fn a_keyframe_on_its_way_past_a_hole_asks_for_none() {
        let t = Instant::now();
        let rtt = Duration::from_millis(5);
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(seqs(&order.lost(1, t)).is_empty());
        assert!(arrive(&mut order, 2, false, t).is_empty());
        order.coming(&frame(5, true).0);
        assert!(!order.should_ask(t + GIVE_UP, rtt));
        assert_eq!(arrive(&mut order, 5, true, t), [5]);
    }

    /// A hole that partly fills is a new hole, timed from then.
    #[test]
    fn a_hole_that_moved_on_is_timed_afresh() {
        let t = Instant::now();
        let mut order = FrameOrder::default();
        assert_eq!(arrive(&mut order, 0, true, t), [0]);
        assert!(arrive(&mut order, 2, false, t).is_empty());
        assert!(arrive(&mut order, 5, false, t).is_empty());
        let later = t + Duration::from_millis(400);
        assert_eq!(arrive(&mut order, 1, false, later), [1, 2]);
        assert_eq!(order.hole_since(), Some(later), "3 is a new hole");
        assert!(
            order.give_up(t + GIVE_UP).is_empty(),
            "not given up 100 ms in"
        );
    }

    #[test]
    fn a_hole_asks_for_a_keyframe_after_two_round_trips() {
        assert_eq!(
            ask_after(Duration::from_millis(5)),
            Duration::from_millis(60)
        );
        assert_eq!(
            ask_after(Duration::from_millis(100)),
            Duration::from_millis(230)
        );
    }
}
