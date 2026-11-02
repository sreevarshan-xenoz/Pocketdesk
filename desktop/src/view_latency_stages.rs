//! The latency probe's stage table (phase 0b): where the time between a
//! pointer move and its picture goes. [`crate::view_latency_live`] times a
//! move from its send to its picture; the host's trace
//! ([`pocketdesk_core::streaming::latency_trace`]) and the stand-in
//! viewer's own record of the access units it read and the pictures it was
//! given tie that time to the steps in between. Everything is on the wall
//! clock of one machine.
//!
//! For each move whose picture was seen, the frame that showed it is
//! named by the frame number in the picture's counting strip, and followed
//! back: the pattern's frame, its encoder in and out, the packet's write,
//! the access unit the viewer read, the picture, the marker found. A step
//! with no record at either end is left out of that stage's samples.

use std::collections::HashMap;

use pocketdesk_core::streaming::latency_trace::{Event, Step};

/// A move the stand-in sent, and the picture that showed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub sent: u64,
    pub x_norm: u16,
    pub seen: Option<Seen>,
}

/// The picture that showed a move: when the decoder gave it (the callback's
/// entry), the frame number its strip said, and when the marker was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub picture_at: u64,
    pub n: Option<u16>,
    pub found_at: u64,
}

/// An access unit the stand-in read from the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Au {
    pub at: u64,
    pub seq: u64,
}

/// The stages, in the order a move takes them.
pub const STAGES: [&str; 17] = [
    "send > host input",
    "host input > frame",
    "frame > encoder in",
    "encoder in > out",
    "encoder out > written",
    "written > viewer AU",
    "AU > picture",
    "picture > found",
    "total (send > found)",
    "frame > encoder out",
    "AUs fed after its AU",
    "frames in before its out",
    "enc: copy frame",
    "enc: BGRA > NV12",
    "enc: upload to card",
    "enc: send_frame",
    "enc: sent > packet out",
];

/// The rows that count (units, frames) rather than time.
const COUNTS: [usize; 2] = [10, 11];

/// What each stage took over the moves, in microseconds (negative where
/// the two clocks disagree, which one machine's do not), by [`STAGES`].
#[derive(Debug, Default)]
pub struct Table {
    pub samples: [Vec<i64>; STAGES.len()],
    /// Moves whose picture was seen.
    pub moves: usize,
}

/// The `p`th (0 to 1) of `v` by nearest rank; sorts it.
fn percentile(v: &mut [i64], p: f64) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let rank = (p.clamp(0.0, 1.0) * v.len() as f64).ceil() as usize;
    Some(v[rank.clamp(1, v.len()) - 1])
}

fn ms(us: Option<i64>) -> String {
    us.map_or_else(
        || "-".to_string(),
        |us| format!("{:.1}", us as f64 / 1000.0),
    )
}

impl Table {
    /// The stage `i`'s median and 95th percentile, in microseconds.
    pub fn of(&mut self, i: usize) -> (Option<i64>, Option<i64>) {
        (
            percentile(&mut self.samples[i], 0.5),
            percentile(&mut self.samples[i], 0.95),
        )
    }

    /// The table as lines of text: a stage a line, its median and 95th
    /// percentile in milliseconds, and how many moves it has a sample from.
    /// The rows in [`COUNTS`] are counts, not times.
    pub fn lines(&mut self) -> Vec<String> {
        let mut out = vec![format!(
            "{:<24} {:>8} {:>8} {:>6}   ({} moves seen)",
            "stage", "p50 ms", "p95 ms", "n", self.moves
        )];
        for (i, name) in STAGES.iter().enumerate() {
            let (p50, p95) = self.of(i);
            let n = self.samples[i].len();
            let (p50, p95) = if COUNTS.contains(&i) {
                (
                    p50.map_or_else(|| "-".into(), |v| v.to_string()),
                    p95.map_or_else(|| "-".into(), |v| v.to_string()),
                )
            } else {
                (ms(p50), ms(p95))
            };
            out.push(format!("{name:<24} {p50:>8} {p95:>8} {n:>6}"));
        }
        out
    }
}

fn diff(from: u64, to: u64) -> i64 {
    to as i64 - from as i64
}

/// The stage table for `moves`, from the host's trace `host` (the events of
/// this run only) and the viewer's access units `aus`.
pub fn table(host: &[Event], aus: &[Au], moves: &[Move]) -> Table {
    // By what each event is joined on.
    let mut encoder_in: HashMap<u64, u64> = HashMap::new();
    let mut encoder_out: HashMap<u64, (u64, u64)> = HashMap::new();
    let mut written: HashMap<u64, u64> = HashMap::new();
    let mut encoder_in_times: Vec<u64> = Vec::new();
    let mut steps: HashMap<(u64, Step), u64> = HashMap::new();
    let mut patterns: Vec<(u64, u64, u64)> = Vec::new();
    let mut inputs: Vec<(u64, u16)> = Vec::new();
    for event in host {
        match *event {
            Event::Input { at, x_norm } => inputs.push((at, x_norm)),
            Event::Pattern { at, n, ts, .. } => patterns.push((at, n, ts)),
            Event::EncoderIn { at, ts } => {
                encoder_in.entry(ts).or_insert(at);
                encoder_in_times.push(at);
            }
            Event::EncoderStep { at, ts, step } => {
                steps.entry((ts, step)).or_insert(at);
            }
            Event::EncoderOut { at, ts, seq, .. } => {
                encoder_out.entry(ts).or_insert((at, seq));
            }
            Event::Written { at, seq } => {
                written.entry(seq).or_insert(at);
            }
        }
    }
    let mut au_at: HashMap<u64, u64> = HashMap::new();
    for au in aus {
        au_at.entry(au.seq).or_insert(au.at);
    }
    let mut t = Table::default();
    for m in moves {
        let Some(seen) = m.seen else { continue };
        t.moves += 1;
        t.samples[8].push(diff(m.sent, seen.found_at));
        t.samples[7].push(diff(seen.picture_at, seen.found_at));
        let input = inputs
            .iter()
            .find(|(at, x)| *at >= m.sent && *x == m.x_norm)
            .map(|(at, _)| *at);
        if let Some(input) = input {
            t.samples[0].push(diff(m.sent, input));
        }
        let Some(n) = seen.n else { continue };
        // The frame that showed it: the newest frame of that number made
        // before its picture came (the number repeats every 65536 frames,
        // which is minutes), and after the move arrived.
        let frame = patterns
            .iter()
            .filter(|(at, fn_, _)| {
                *fn_ as u16 == n && *at <= seen.picture_at && input.is_none_or(|i| *at >= i)
            })
            .max_by_key(|(at, _, _)| *at)
            .copied();
        let Some((frame_at, _, ts)) = frame else {
            continue;
        };
        if let Some(input) = input {
            t.samples[1].push(diff(input, frame_at));
        }
        let enc_in = encoder_in.get(&ts).copied();
        if let Some(enc_in) = enc_in {
            t.samples[2].push(diff(frame_at, enc_in));
        }
        let Some(&(out_at, seq)) = encoder_out.get(&ts) else {
            continue;
        };
        if let Some(enc_in) = enc_in {
            t.samples[3].push(diff(enc_in, out_at));
        }
        t.samples[9].push(diff(frame_at, out_at));
        if let Some(enc_in) = enc_in {
            // Frames the encoder was given after this one and before it gave
            // this one's packet: an encoder that holds a frame back has one
            // or more.
            let later = encoder_in_times
                .iter()
                .filter(|a| **a > enc_in && **a < out_at)
                .count();
            t.samples[11].push(later as i64);
            // The steps inside it, where it says them (VA-API): each from
            // the one before.
            let mut from = enc_in;
            for (row, step) in [
                (12, Step::Copied),
                (13, Step::Scaled),
                (14, Step::Uploaded),
                (15, Step::Sent),
            ] {
                let Some(&at) = steps.get(&(ts, step)) else {
                    break;
                };
                t.samples[row].push(diff(from, at));
                from = at;
            }
            if steps.contains_key(&(ts, Step::Sent)) {
                t.samples[16].push(diff(from, out_at));
            }
        }
        let wrote = written.get(&seq).copied();
        if let Some(wrote) = wrote {
            t.samples[4].push(diff(out_at, wrote));
        }
        let Some(&au) = au_at.get(&seq) else { continue };
        if let Some(wrote) = wrote {
            t.samples[5].push(diff(wrote, au));
        }
        t.samples[6].push(diff(au, seen.picture_at));
        // Access units read after this one and before its picture came:
        // a decoder that gives a picture only when the next unit arrives
        // has one or more.
        let later = aus
            .iter()
            .filter(|a| a.at > au && a.at < seen.picture_at)
            .count();
        t.samples[10].push(later as i64);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One move's whole chain, 1 ms a stage: sent at 1000, the picture at
    /// `1000 + 8000`, found 50 us later.
    fn chain() -> (Vec<Event>, Vec<Au>, Vec<Move>) {
        let host = vec![
            Event::Input {
                at: 2000,
                x_norm: 7,
            },
            // An earlier frame of the same number (65536 frames before) and
            // one with another number are not the frame.
            Event::Pattern {
                at: 100,
                n: 65_536 + 3,
                ts: 1,
                x_norm: None,
            },
            Event::Pattern {
                at: 3000,
                n: 4,
                ts: 55,
                x_norm: Some(7),
            },
            Event::Pattern {
                at: 3500,
                n: 3,
                ts: 77,
                x_norm: Some(7),
            },
            Event::EncoderIn { at: 3600, ts: 77 },
            // The encoder's steps for it, and the next frame going in
            // before its packet comes out.
            Event::EncoderStep {
                at: 3700,
                ts: 77,
                step: Step::Copied,
            },
            Event::EncoderStep {
                at: 3900,
                ts: 77,
                step: Step::Scaled,
            },
            Event::EncoderStep {
                at: 4000,
                ts: 77,
                step: Step::Uploaded,
            },
            Event::EncoderStep {
                at: 4050,
                ts: 77,
                step: Step::Sent,
            },
            Event::EncoderIn { at: 4300, ts: 88 },
            Event::EncoderOut {
                at: 4600,
                ts: 77,
                seq: 12,
                bytes: 900,
                key: false,
            },
            Event::Written { at: 5600, seq: 12 },
        ];
        let aus = vec![
            Au { at: 6600, seq: 12 },
            Au { at: 7000, seq: 13 },
            Au { at: 9500, seq: 14 },
        ];
        let moves = vec![
            Move {
                sent: 1000,
                x_norm: 7,
                seen: Some(Seen {
                    picture_at: 9000,
                    n: Some(3),
                    found_at: 9050,
                }),
            },
            // Not seen: counted nowhere.
            Move {
                sent: 20_000,
                x_norm: 8,
                seen: None,
            },
        ];
        (host, aus, moves)
    }

    #[test]
    fn a_moves_chain_gives_each_stage_its_time() {
        let (host, aus, moves) = chain();
        let mut t = table(&host, &aus, &moves);
        assert_eq!(t.moves, 1);
        let at = |t: &mut Table, i: usize| t.of(i).0;
        assert_eq!(at(&mut t, 0), Some(1000), "send > host input");
        assert_eq!(at(&mut t, 1), Some(1500), "input > frame 3 (not 4)");
        assert_eq!(at(&mut t, 2), Some(100), "frame > encoder in");
        assert_eq!(at(&mut t, 3), Some(1000), "encoder in > out");
        assert_eq!(at(&mut t, 4), Some(1000), "out > written");
        assert_eq!(at(&mut t, 5), Some(1000), "written > viewer AU");
        assert_eq!(at(&mut t, 6), Some(2400), "AU > picture");
        assert_eq!(at(&mut t, 7), Some(50), "picture > found");
        assert_eq!(at(&mut t, 8), Some(8050), "total");
        assert_eq!(at(&mut t, 9), Some(1100), "frame > encoder out");
        assert_eq!(
            at(&mut t, 10),
            Some(1),
            "one unit read after its own and before the picture (seq 13)"
        );
        assert_eq!(at(&mut t, 11), Some(1), "one frame in before its out");
        let steps: Vec<_> = (12..=16).map(|i| at(&mut t, i)).collect();
        assert_eq!(
            steps,
            [Some(100), Some(200), Some(100), Some(50), Some(550)],
            "copy, scale, upload, send, and the wait for the packet"
        );
    }

    #[test]
    fn a_missing_step_leaves_that_stage_and_the_ones_after_it_out() {
        let (mut host, aus, moves) = chain();
        // No encoder out: the later stages have no samples, the earlier do.
        host.retain(|e| !matches!(e, Event::EncoderOut { .. }));
        let mut t = table(&host, &aus, &moves);
        assert_eq!(t.of(2).0, Some(100));
        for i in [3, 4, 5, 6, 9, 10, 11, 12, 16] {
            assert!(t.samples[i].is_empty(), "{}", STAGES[i]);
        }
        assert_eq!(t.of(8).0, Some(8050), "the total never needs the host");
        // A picture whose strip could not be read: only the ends.
        let (host, aus, mut moves) = chain();
        moves[0].seen.as_mut().unwrap().n = None;
        let mut t = table(&host, &aus, &moves);
        assert_eq!(t.of(0).0, Some(1000));
        assert!(t.samples[1].is_empty() && t.samples[2].is_empty());
        assert_eq!(t.of(7).0, Some(50));
    }

    #[test]
    fn the_median_and_the_95th_over_many_moves() {
        let mut t = Table::default();
        t.samples[0] = (1..=100).map(|i| i * 1000).collect();
        assert_eq!(t.of(0), (Some(50_000), Some(95_000)));
        assert_eq!(t.of(1), (None, None), "no samples");
        let lines = t.lines();
        assert!(lines[0].contains("p50 ms"), "{lines:?}");
        assert!(lines[1].starts_with("send > host input"), "{lines:?}");
        assert!(lines[1].contains("50.0") && lines[1].contains("95.0"));
        assert!(lines[2].contains(" -"), "a stage with no samples says so");
        assert_eq!(lines.len(), STAGES.len() + 1);
    }
}
