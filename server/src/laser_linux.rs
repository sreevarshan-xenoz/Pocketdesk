//! The laser dot on Linux (G4 on wlroots compositors, N10): drawn on the
//! layer shell's overlay by `core::streaming::laser_dot`, moved where
//! [`crate::laser::Dot`] says. One thread owns the compositor connection,
//! opened for each showing (so the screens are read as they are then) and
//! closed when the dot hides; the input task sends it moves.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::laser_dot::{Overlay, Screen};

use crate::laser::{Area, Dot, dot_px};

enum Move {
    By(i16, i16),
    Hide,
}

static SENDER: OnceLock<Mutex<Option<Sender<Move>>>> = OnceLock::new();

/// The dot's thread, started at the first move; `None` when it could not start.
fn sender() -> Option<Sender<Move>> {
    let slot = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("laser-dot".into())
            .spawn(move || run(rx))
            .is_ok();
        Mutex::new(started.then_some(tx))
    });
    slot.lock().ok()?.clone()
}

/// The phone's laser packet: queued for the dot's thread.
pub fn handle(dx: i16, dy: i16, show: bool) {
    // A hide with no dot yet starts no thread (as on Windows).
    if !show && SENDER.get().is_none() {
        return;
    }
    if let Some(tx) = sender() {
        let _ = tx.send(if show { Move::By(dx, dy) } else { Move::Hide });
    }
}

fn area(screen: Screen) -> Area {
    Area {
        left: screen.left,
        top: screen.top,
        width: screen.width,
        height: screen.height,
    }
}

/// The dot's thread: moves as they come; the overlay goes when the dot is
/// told to hide or is still for [`crate::laser::IDLE`].
fn run(moves: Receiver<Move>) {
    let mut dot = Dot::default();
    let mut overlay: Option<Overlay> = None;
    let mut known: Vec<Area> = Vec::new();
    let mut warned = false;
    loop {
        let wait = if dot.idle_soon() { 50 } else { 1000 };
        let next = moves.recv_timeout(Duration::from_millis(wait));
        let now = Instant::now();
        match next {
            Ok(Move::By(dx, dy)) => {
                if overlay.is_none() {
                    overlay = Overlay::open();
                    match &overlay {
                        Some(open) => {
                            known = open.screens().into_iter().map(area).collect();
                            warned = false;
                        }
                        None => {
                            if !warned {
                                tracing::warn!("The laser dot could not be drawn here");
                                warned = true;
                            }
                            continue;
                        }
                    }
                }
                // A Wayland client cannot read the pointer: the dot starts
                // on the first screen's middle (Dot holds the choice).
                let pointer = known
                    .first()
                    .map_or((0, 0), |s| (s.left + s.width / 2, s.top + s.height / 2));
                if let Some((x, y)) = dot.moved(dx, dy, &known, pointer, now) {
                    let size = dot_px(&known, (x, y)).max(1) as u32;
                    let shown = overlay.as_mut().is_some_and(|o| o.show_at(x, y, size));
                    if !shown {
                        // The compositor took it away: opened again next move.
                        overlay = None;
                    }
                }
            }
            Ok(Move::Hide) => {
                dot.hide();
                overlay = None;
            }
            Err(RecvTimeoutError::Timeout) => {
                // Still too long: the phone's release was likely lost. The
                // dot goes, its place is kept (G4 review).
                if dot.idle(now) {
                    dot.rest();
                    overlay = None;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
