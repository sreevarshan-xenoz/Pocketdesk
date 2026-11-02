//! The latency probe's stage trace (phase 0b of the window handoff work).
//! The probe ([`super::latency_probe`]) says how long a pointer move takes
//! to its picture; this says where that time goes. With
//! `POCKETDESK_LATENCY_TRACE=<file>` and the probe mode on, the host
//! appends one line to the file for each step a frame and a move take, with
//! the wall clock in microseconds (one machine, so the host's and the
//! stand-in viewer's clocks are the same one):
//!
//! ```text
//! in      <at> <x_norm>                          a move arrived from a session
//! pat     <at> <n> <ts> <x_norm | ->             the test pattern made frame n
//! encin   <at> <ts>                              the frame went into the encoder
//! encout  <at> <ts> <seq> <bytes> <key>          a packet came out of it
//! wrote   <at> <seq>                             the packet was written to its stream
//! encstep <at> <ts> <step>                       a step inside an encoder (see [`Step`])
//! ```
//!
//! `ts` names a frame: microseconds from the trace's start to the frame's
//! own timestamp, which the encoder carries to its packet, so a packet is
//! tied back to the frame it came from; `seq` is the packet's sequence, the
//! one the relay forwards to the viewer.
//!
//! Off by default and cheap: with no variable (or outside the probe mode)
//! every call below is one atomic load, and nothing is opened or written.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::latency_probe;

/// The variable: the file the host's trace is appended to.
pub const VAR: &str = "POCKETDESK_LATENCY_TRACE";

struct Trace {
    file: File,
    /// Frame timestamps are told in microseconds from here.
    epoch: Instant,
}

/// The steps inside an in-process encoder that say their time, in the order
/// they come (the VA-API encoder; the others say none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// The frame is copied into the encoder's own buffer.
    Copied = 1,
    /// BGRA converted to NV12 on the processor.
    Scaled = 2,
    /// The frame uploaded to a surface on the card: NV12 when the processor
    /// converted it, BGRA when the card does.
    Uploaded = 3,
    /// The frame handed to the codec (`avcodec_send_frame` returned).
    Sent = 4,
    /// BGRA converted to NV12 by the card's video processor, after the
    /// upload (the card path has no `Copied` or `Scaled`).
    Converted = 5,
}

impl Step {
    fn from_number(n: u64) -> Option<Self> {
        Some(match n {
            1 => Self::Copied,
            2 => Self::Scaled,
            3 => Self::Uploaded,
            4 => Self::Sent,
            5 => Self::Converted,
            _ => return None,
        })
    }
}

static TRACE: OnceLock<Option<Trace>> = OnceLock::new();

/// The trace file for `path`, only in the probe mode.
fn open(probe: bool, path: Option<OsString>) -> Option<Trace> {
    let path = path.filter(|p| !p.is_empty())?;
    if !probe {
        tracing::warn!(
            "{VAR} is ignored: it needs {}=probe",
            latency_probe::PATTERN_VAR
        );
        return None;
    }
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            tracing::warn!("{VAR}: the latency stages are traced to {path:?}");
            Some(Trace {
                file,
                epoch: Instant::now(),
            })
        }
        Err(e) => {
            tracing::warn!("{VAR}: {path:?} cannot be opened ({e}); no trace");
            None
        }
    }
}

/// The trace, opened (or refused) on the first call: the test pattern asks
/// as it starts, before its first frame, so the trace's start is earlier
/// than every frame's timestamp.
fn trace() -> Option<&'static Trace> {
    TRACE
        .get_or_init(|| open(latency_probe::mode(), std::env::var_os(VAR)))
        .as_ref()
}

/// Whether this process traces.
pub fn on() -> bool {
    trace().is_some()
}

/// The wall clock now, in microseconds since the Unix epoch.
pub fn wall_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// A frame's timestamp as the trace names it.
fn frame_us(epoch: Instant, ts: Instant) -> u64 {
    ts.saturating_duration_since(epoch).as_micros() as u64
}

/// One line of the trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Input {
        at: u64,
        x_norm: u16,
    },
    Pattern {
        at: u64,
        n: u64,
        ts: u64,
        x_norm: Option<u16>,
    },
    EncoderIn {
        at: u64,
        ts: u64,
    },
    EncoderOut {
        at: u64,
        ts: u64,
        seq: u64,
        bytes: usize,
        key: bool,
    },
    Written {
        at: u64,
        seq: u64,
    },
    EncoderStep {
        at: u64,
        ts: u64,
        step: Step,
    },
}

impl Event {
    /// The line that says it, with no newline.
    pub fn line(&self) -> String {
        let mut s = String::with_capacity(64);
        let _ = match *self {
            Event::Input { at, x_norm } => write!(s, "in {at} {x_norm}"),
            Event::Pattern { at, n, ts, x_norm } => match x_norm {
                Some(x) => write!(s, "pat {at} {n} {ts} {x}"),
                None => write!(s, "pat {at} {n} {ts} -"),
            },
            Event::EncoderIn { at, ts } => write!(s, "encin {at} {ts}"),
            Event::EncoderOut {
                at,
                ts,
                seq,
                bytes,
                key,
            } => write!(s, "encout {at} {ts} {seq} {bytes} {}", u8::from(key)),
            Event::Written { at, seq } => write!(s, "wrote {at} {seq}"),
            Event::EncoderStep { at, ts, step } => write!(s, "encstep {at} {ts} {}", step as u8),
        };
        s
    }

    /// The event a line says; `None` for a line that is not one (a torn or
    /// foreign line is skipped, not fatal).
    pub fn parse(line: &str) -> Option<Event> {
        fn num(words: &mut std::str::SplitWhitespace) -> Option<u64> {
            words.next()?.parse().ok()
        }
        let mut words = line.split_whitespace();
        let kind = words.next()?;
        let at = num(&mut words)?;
        let event = match kind {
            "in" => Event::Input {
                at,
                x_norm: u16::try_from(num(&mut words)?).ok()?,
            },
            "pat" => {
                let (n, ts) = (num(&mut words)?, num(&mut words)?);
                let x = words.next()?;
                Event::Pattern {
                    at,
                    n,
                    ts,
                    x_norm: if x == "-" {
                        None
                    } else {
                        Some(x.parse().ok()?)
                    },
                }
            }
            "encin" => Event::EncoderIn {
                at,
                ts: num(&mut words)?,
            },
            "encout" => Event::EncoderOut {
                at,
                ts: num(&mut words)?,
                seq: num(&mut words)?,
                bytes: usize::try_from(num(&mut words)?).ok()?,
                key: num(&mut words)? != 0,
            },
            "encstep" => Event::EncoderStep {
                at,
                ts: num(&mut words)?,
                step: Step::from_number(num(&mut words)?)?,
            },
            "wrote" => Event::Written {
                at,
                seq: num(&mut words)?,
            },
            _ => return None,
        };
        Some(event)
    }

    /// When it happened (wall clock, microseconds).
    pub fn at(&self) -> u64 {
        match *self {
            Event::Input { at, .. }
            | Event::Pattern { at, .. }
            | Event::EncoderIn { at, .. }
            | Event::EncoderOut { at, .. }
            | Event::Written { at, .. }
            | Event::EncoderStep { at, .. } => at,
        }
    }
}

/// The events of a trace file, in the order written; none for a file that
/// is not there.
pub fn read(path: &Path) -> Vec<Event> {
    std::fs::read_to_string(path)
        .map(|text| text.lines().filter_map(Event::parse).collect())
        .unwrap_or_default()
}

fn put(t: &Trace, event: Event) {
    let mut line = event.line();
    line.push('\n');
    // One write a line: appended whole, and a failed write costs only that
    // line.
    let _ = (&t.file).write_all(line.as_bytes());
}

/// A move arrived from a session.
pub fn input(x_norm: u16) {
    if let Some(t) = trace() {
        put(
            t,
            Event::Input {
                at: wall_us(),
                x_norm,
            },
        );
    }
}

/// The pattern made frame `n`, stamped `ts`, with the marker at `x_norm`
/// (the pointer it read).
pub fn pattern(n: u64, ts: Instant, x_norm: Option<u16>) {
    if let Some(t) = trace() {
        put(
            t,
            Event::Pattern {
                at: wall_us(),
                n,
                ts: frame_us(t.epoch, ts),
                x_norm,
            },
        );
    }
}

/// The frame stamped `ts` went into the encoder.
pub fn encoder_in(ts: Instant) {
    if let Some(t) = trace() {
        put(
            t,
            Event::EncoderIn {
                at: wall_us(),
                ts: frame_us(t.epoch, ts),
            },
        );
    }
}

/// The encoder gave a packet, of the frame stamped `ts`.
pub fn encoder_out(ts: Instant, seq: u64, bytes: usize, key: bool) {
    if let Some(t) = trace() {
        put(
            t,
            Event::EncoderOut {
                at: wall_us(),
                ts: frame_us(t.epoch, ts),
                seq,
                bytes,
                key,
            },
        );
    }
}

/// A step inside the encoder, of the frame stamped `ts`.
pub fn encoder_step(ts: Instant, step: Step) {
    if let Some(t) = trace() {
        put(
            t,
            Event::EncoderStep {
                at: wall_us(),
                ts: frame_us(t.epoch, ts),
                step,
            },
        );
    }
}

/// Packet `seq` was written to its stream.
pub fn written(seq: u64) {
    if let Some(t) = trace() {
        put(t, Event::Written { at: wall_us(), seq });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_is_read_back_as_it_was_written() {
        let events = [
            Event::Input {
                at: 1_700_000_000_123_456,
                x_norm: 49152,
            },
            Event::Pattern {
                at: 5,
                n: 70_000,
                ts: 9,
                x_norm: Some(0),
            },
            Event::Pattern {
                at: 5,
                n: 0,
                ts: 0,
                x_norm: None,
            },
            Event::EncoderIn { at: 6, ts: 9 },
            Event::EncoderOut {
                at: 7,
                ts: 9,
                seq: 3,
                bytes: 4096,
                key: true,
            },
            Event::Written { at: 8, seq: 3 },
            Event::EncoderStep {
                at: 9,
                ts: 9,
                step: Step::Uploaded,
            },
        ];
        for event in events {
            assert_eq!(Event::parse(&event.line()), Some(event), "{event:?}");
        }
    }

    #[test]
    fn a_line_that_is_not_an_event_is_skipped() {
        for line in [
            "",
            "in",
            "in x 1",
            "in 5",
            "in 5 70000",
            "pat 5 1 2",
            "wrote 5",
            "encstep 5 6 0",
            "encstep 5 6 6",
            "encout 1 2 3 4",
            "later 1 2",
            "in 5 6 and a torn",
        ] {
            let parsed = Event::parse(line);
            // A line with more than it needs still reads (the extra words
            // are ignored); the others do not.
            assert_eq!(parsed.is_some(), line == "in 5 6 and a torn", "{line:?}");
        }
    }

    #[test]
    fn a_trace_file_reads_in_order_and_a_missing_one_reads_empty() {
        let dir = std::env::temp_dir().join(format!("pd-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("trace.log");
        std::fs::write(&file, "in 1 2\ngarbage\nwrote 3 4\n").unwrap();
        assert_eq!(
            read(&file),
            vec![
                Event::Input { at: 1, x_norm: 2 },
                Event::Written { at: 3, seq: 4 }
            ]
        );
        assert!(read(&dir.join("none.log")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_trace_opens_only_in_the_probe_mode_and_with_a_file() {
        let dir = std::env::temp_dir().join(format!("pd-trace-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.log");
        assert!(
            open(false, Some(file.clone().into())).is_none(),
            "not probe"
        );
        assert!(!file.exists(), "nothing created outside the probe mode");
        assert!(open(true, None).is_none(), "no variable");
        assert!(
            open(true, Some(OsString::new())).is_none(),
            "empty variable"
        );
        assert!(open(true, Some(file.clone().into())).is_some());
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_frames_name_counts_from_the_traces_start() {
        let epoch = Instant::now();
        let ts = epoch + std::time::Duration::from_micros(1500);
        assert_eq!(frame_us(epoch, ts), 1500);
        assert_eq!(frame_us(ts, epoch), 0, "never before the start");
    }
}
