//! Input through the RemoteDesktop portal (N8), for GNOME and KDE when
//! uinput cannot be opened: the capture's portal session is a RemoteDesktop
//! session that also shares the screen, so one grant covers both, and its
//! worker takes these events from the input injector.
//!
//! The events and the mapping to them are plain data, tested anywhere; the
//! registry holds the live session's sender while a capture runs.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// One thing to tell the portal, in its terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PortalEvent {
    /// Relative pointer motion, in pixels.
    Motion { dx: f64, dy: f64 },
    /// Absolute pointer position, normalized 0..=65535 over the shared screen.
    Absolute { x_norm: u16, y_norm: u16 },
    /// A pointer button by evdev code (`BTN_LEFT` 0x110 ...).
    Button { code: i32, pressed: bool },
    /// Wheel notches: `axis` 0 vertical, 1 horizontal.
    Scroll { axis: u32, steps: i32 },
    /// A key by evdev code.
    Key { code: i32, pressed: bool },
    /// A key by X keysym, for text.
    Keysym { sym: i32, pressed: bool },
}

/// The X keysym that types `c`: Latin-1 as itself, a few controls by name,
/// anything else as a Unicode keysym.
pub fn keysym_for(c: char) -> i32 {
    match c {
        '\n' | '\r' => 0xff0d, // Return
        '\t' => 0xff09,        // Tab
        '\u{8}' => 0xff08,     // BackSpace
        ' '..='~' | '\u{a0}'..='\u{ff}' => c as i32,
        _ => 0x0100_0000 | c as i32,
    }
}

/// `text` as key presses and releases, one keysym a character (a CRLF is
/// one Return).
pub fn text_events(text: &str) -> Vec<PortalEvent> {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .chars()
        .flat_map(|c| {
            let sym = keysym_for(c);
            [
                PortalEvent::Keysym { sym, pressed: true },
                PortalEvent::Keysym {
                    sym,
                    pressed: false,
                },
            ]
        })
        .collect()
}

/// Where a normalized position falls on a stream of `size`, in its pixels.
pub fn absolute_position(x_norm: u16, y_norm: u16, size: (u32, u32)) -> (f64, f64) {
    let scale = |n: u16, len: u32| f64::from(n) / 65535.0 * f64::from(len.saturating_sub(1));
    (scale(x_norm, size.0), scale(y_norm, size.1))
}

/// The input injector found no other way in and asks the capture for a
/// RemoteDesktop session.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Ask for (or stop asking for) input through the portal.
pub fn want(on: bool) {
    WANTED.store(on, Ordering::Release);
}

/// Whether the capture should open a RemoteDesktop session.
pub fn wanted() -> bool {
    WANTED.load(Ordering::Acquire)
}

type Sender = tokio::sync::mpsc::UnboundedSender<PortalEvent>;

/// The live session's worker, while a capture holds one.
static SESSION: Mutex<Option<(u64, Sender)>> = Mutex::new(None);

/// Tell the live session `event`; false when there is none.
pub fn send(event: PortalEvent) -> bool {
    match SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some((_, tx)) => tx.send(event).is_ok(),
        None => false,
    }
}

/// A capture's session, registered while it lives; the newest wins, and
/// dropping it clears only its own.
pub struct Registered(u64);

/// Register `tx` as the live session's worker.
pub fn register(tx: Sender) -> Registered {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some((id, tx));
    Registered(id)
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if session.as_ref().is_some_and(|(id, _)| *id == self.0) {
            *session = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_becomes_keysym_presses() {
        assert_eq!(
            text_events("aÉ"),
            [
                PortalEvent::Keysym {
                    sym: 0x61,
                    pressed: true
                },
                PortalEvent::Keysym {
                    sym: 0x61,
                    pressed: false
                },
                PortalEvent::Keysym {
                    sym: 0xc9,
                    pressed: true
                },
                PortalEvent::Keysym {
                    sym: 0xc9,
                    pressed: false
                },
            ]
        );
        assert_eq!(
            keysym_for('€'),
            0x0100_20ac,
            "outside Latin-1: Unicode keysym"
        );
        assert_eq!(keysym_for('\n'), 0xff0d);
        assert_eq!(text_events("a\r\nb").len(), 6, "a CRLF is one Return");
    }

    #[test]
    fn a_normalized_position_lands_on_the_stream() {
        assert_eq!(absolute_position(0, 0, (1920, 1080)), (0.0, 0.0));
        assert_eq!(
            absolute_position(65535, 65535, (1920, 1080)),
            (1919.0, 1079.0)
        );
        let (x, _) = absolute_position(32768, 0, (1920, 1080));
        assert!((x - 959.5).abs() < 0.1);
    }

    #[test]
    fn events_reach_the_newest_session_and_stop_with_it() {
        let (old_tx, mut old_rx) = tokio::sync::mpsc::unbounded_channel();
        let (new_tx, mut new_rx) = tokio::sync::mpsc::unbounded_channel();
        let old = register(old_tx);
        let new = register(new_tx);
        assert!(send(PortalEvent::Key {
            code: 30,
            pressed: true
        }));
        assert!(new_rx.try_recv().is_ok());
        assert!(old_rx.try_recv().is_err(), "the newest wins");
        drop(old);
        assert!(
            send(PortalEvent::Key {
                code: 30,
                pressed: false
            }),
            "an older one's end leaves it"
        );
        drop(new);
        assert!(
            !send(PortalEvent::Key {
                code: 30,
                pressed: true
            }),
            "none left"
        );
    }
}
