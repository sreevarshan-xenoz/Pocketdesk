//! The window's own stage trace (phase 1d of the window handoff work): where
//! the time goes from an access unit being read off the relay to its picture
//! being painted. With `POCKETDESK_VIEWER_TRACE=<file>` the window appends
//! one line per step with the wall clock in microseconds (one machine, so
//! the host's trace of [`pocketdesk_core::streaming::latency_trace`] shares
//! the clock):
//!
//! ```text
//! vau     <at> <seq>   an access unit was read off the relay, about to be fed
//! vpic    <at> <n>     the decoder's picture reached the window (the callback)
//! vpass   <at> <n>     the window's pass took the picture, to give it to the GPU
//! vpaint  <at> <n>     the GPU finished drawing it (after `glFinish`)
//! ```
//!
//! `n` is the frame number the test pattern's counting strip says (`-` when
//! there is none), which ties a picture back to the host's frame. `glFinish`
//! is called only while tracing, so the paint step includes the GPU's time.
//! Off by default: with no variable every call is one atomic load.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};

use pocketdesk_core::streaming::latency_probe;
use pocketdesk_core::streaming::latency_trace::wall_us;

use crate::nv12::Layout;
use crate::video::{Picture, Planes};

/// The variable: the file the window's trace is appended to.
pub const VAR: &str = "POCKETDESK_VIEWER_TRACE";

static TRACE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
/// The frame number of the picture the last pass took (-1: none).
static LAST: AtomicI32 = AtomicI32::new(-1);

fn open(path: &Path) -> Option<Mutex<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(Mutex::new)
}

/// Trace to `path` whatever the variable says; a test's way in. Only the
/// first call or use of the trace counts.
#[cfg(test)]
pub fn start(path: &Path) {
    let _ = TRACE.set(open(path));
}

fn trace() -> Option<&'static Mutex<File>> {
    TRACE
        .get_or_init(|| {
            std::env::var_os(VAR)
                .filter(|p| !p.is_empty())
                .and_then(|p| open(Path::new(&p)))
        })
        .as_ref()
}

/// Whether the window traces.
pub fn on() -> bool {
    trace().is_some()
}

fn line(kind: &str, at: u64, what: &str) {
    if let Some(file) = trace() {
        let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(file, "{kind} {at} {what}");
    }
}

/// The frame number `picture`'s counting strip says.
pub fn frame_number(picture: &Picture) -> Option<u16> {
    if picture.planes != Planes::Nv12 {
        return None;
    }
    let (luma, _) = Layout::new(picture.width, picture.height).split(&picture.data)?;
    latency_probe::read_frame_number(luma, picture.width, picture.height)
}

fn number(n: Option<u16>) -> String {
    n.map_or_else(|| "-".to_string(), |n| n.to_string())
}

/// An access unit was read, `seq` its sequence.
pub fn au(seq: u64) {
    if on() {
        line("vau", wall_us(), &seq.to_string());
    }
}

/// The decoder gave `picture` to the window.
pub fn picture(picture: &Picture) {
    if on() {
        let at = wall_us();
        line("vpic", at, &number(frame_number(picture)));
    }
}

/// The window's pass took `picture`.
pub fn pass(picture: &Picture) {
    if on() {
        let at = wall_us();
        let n = frame_number(picture);
        LAST.store(n.map_or(-1, i32::from), Ordering::Relaxed);
        line("vpass", at, &number(n));
    }
}

/// A step of the pass that took the last picture: `kind` (`vui` the window's
/// own widgets built, `vcb` the GPU's paint callback began, `vupl` the
/// planes uploaded and the GPU idle, `vpaint` the picture drawn and the GPU
/// idle again).
pub fn mark(kind: &str) {
    if on() {
        let at = wall_us();
        let n = LAST.load(Ordering::Relaxed);
        line(kind, at, &number(u16::try_from(n).ok()));
    }
}

/// Calls [`mark`] with `kind` when dropped.
pub struct Ends(pub &'static str);

impl Drop for Ends {
    fn drop(&mut self) {
        mark(self.0);
    }
}
