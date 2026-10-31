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
    /// Wheel notches: `axis` 0 vertical, 1 horizontal; positive is down,
    /// or right. How the portal is told them is [`wheel_call`]'s choice.
    Scroll { axis: u32, steps: i32 },
    /// A fine wheel turn, in 120ths of a notch (`axis` and signs as
    /// [`PortalEvent::Scroll`]): [`fine_wheel_call`] says how it goes.
    FineScroll { axis: u32, v120: i32 },
    /// A key by evdev code.
    Key { code: i32, pressed: bool },
    /// A key by X keysym, for text.
    Keysym { sym: i32, pressed: bool },
}

/// The X keysym that types `c`: Latin-1 as itself, the controls a
/// keyboard has by name, others none (N8 review: a control character as a
/// Unicode keysym is not one), anything else as a Unicode keysym.
pub fn keysym_for(c: char) -> Option<i32> {
    Some(match c {
        '\n' | '\r' => 0xff0d, // Return
        '\t' => 0xff09,        // Tab
        '\u{8}' => 0xff08,     // BackSpace
        '\u{1b}' => 0xff1b,    // Escape
        '\u{7f}' => 0xffff,    // Delete
        c if c.is_control() => return None,
        ' '..='~' | '\u{a0}'..='\u{ff}' => c as i32,
        _ => 0x0100_0000 | c as i32,
    })
}

/// `text` as key presses and releases, one keysym a character (a CRLF is
/// one Return; a control with no key is skipped).
pub fn text_events(text: &str) -> Vec<PortalEvent> {
    let normalized = text.replace("\r\n", "\n");
    normalized
        .chars()
        .filter_map(keysym_for)
        .flat_map(|sym| {
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

/// One wheel notch as smooth scroll, in the axis units a compositor gets
/// from a mouse: libinput's 15 degrees a notch, which KWin hands on as 15.0,
/// and what the wlroots path (`wlr_input::NOTCH`) and a
/// uinput `REL_WHEEL` notch scroll by.
pub const SMOOTH_NOTCH: f64 = 15.0;

/// How the portal's backend has to be told a wheel notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelStyle {
    /// `NotifyPointerAxisDiscrete` steps: GNOME (mutter makes each step a
    /// real wheel click) and the portal's own meaning of the call.
    Discrete,
    /// `NotifyPointerAxis` in pixels: KDE, whose backend hands
    /// `NotifyPointerAxisDiscrete`'s steps to KWin's fake input as pixels,
    /// so a notch scrolled by one (checked headless on Plasma 6.8.2).
    Smooth,
}

/// The style for the desktop named by `XDG_CURRENT_DESKTOP` (colon
/// separated, as `KDE` or `ubuntu:GNOME`) or, without one, by
/// `KDE_FULL_SESSION`.
pub fn wheel_style(current_desktop: Option<&str>, kde_full_session: bool) -> WheelStyle {
    let kde = match current_desktop {
        Some(names) if !names.trim().is_empty() => names
            .split(':')
            .any(|name| name.trim().eq_ignore_ascii_case("KDE")),
        _ => kde_full_session,
    };
    if kde {
        WheelStyle::Smooth
    } else {
        WheelStyle::Discrete
    }
}

/// [`wheel_style`] for this process's session.
pub fn session_wheel_style() -> WheelStyle {
    wheel_style(
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        std::env::var_os("KDE_FULL_SESSION").is_some(),
    )
}

/// A portal call that turns the wheel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelCall {
    /// `NotifyPointerAxisDiscrete(axis, steps)`.
    Discrete { axis: u32, steps: i32 },
    /// `NotifyPointerAxis(dx, dy)` with `finish` set: one notch is a whole
    /// scroll, so nothing kinetic carries on after it.
    Smooth { dx: f64, dy: f64 },
}

/// `steps` notches on `axis` (0 vertical, 1 horizontal; positive is down,
/// or right) as the call `style` takes, none for no notches.
///
/// KDE's backend turns `NotifyPointerAxis`'s `dy` over before KWin sees it
/// (it sends `-dy` as the vertical axis value, and `dx` as it is), so a
/// notch down is a negative `dy` there and its axis value comes out as
/// [`SMOOTH_NOTCH`] down, as a mouse's would.
pub fn wheel_call(style: WheelStyle, axis: u32, steps: i32) -> Option<WheelCall> {
    if steps == 0 {
        return None;
    }
    Some(match style {
        WheelStyle::Discrete => WheelCall::Discrete { axis, steps },
        WheelStyle::Smooth => smooth_call(axis, f64::from(steps) * SMOOTH_NOTCH),
    })
}

/// `pixels` down (or right) on `axis` as `NotifyPointerAxis` takes them on
/// KDE, whose backend turns `dy` over.
fn smooth_call(axis: u32, pixels: f64) -> WheelCall {
    if axis == 0 {
        WheelCall::Smooth {
            dx: 0.0,
            dy: -pixels,
        }
    } else {
        WheelCall::Smooth {
            dx: pixels,
            dy: 0.0,
        }
    }
}

/// A fine turn of `v120` 120ths of a notch on `axis` (signs as
/// [`wheel_call`]) as the call `style` takes, if any.
///
/// KDE takes it as it comes, in pixels ([`SMOOTH_NOTCH`] a notch), so a
/// touchpad scrolls as finely there as it does here. GNOME is told whole
/// steps: what is not yet a notch waits in `rest` until it is, so slow
/// scrolling adds up instead of being lost (mutter makes each step a real
/// wheel click; its smooth call's scale was not checked headless).
pub fn fine_wheel_call(
    style: WheelStyle,
    axis: u32,
    v120: i32,
    rest: &mut crate::streaming::wheel::Notches,
) -> Option<WheelCall> {
    if v120 == 0 {
        return None;
    }
    match style {
        WheelStyle::Smooth => Some(smooth_call(axis, crate::streaming::wheel::pixels(v120))),
        WheelStyle::Discrete => {
            let steps = if axis == 0 {
                rest.add(0, v120).1
            } else {
                rest.add(v120, 0).0
            };
            wheel_call(style, axis, steps)
        }
    }
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

/// The sessions' workers while their captures hold them, oldest first.
/// Input goes to the newest; an older one is kept until its capture ends,
/// since dropping its sender would end its worker and close the portal
/// session another phone still watches (N8 review).
static SESSIONS: Mutex<Vec<(u64, Sender)>> = Mutex::new(Vec::new());

/// Tell the newest session `event`; false when there is none.
pub fn send(event: PortalEvent) -> bool {
    match SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).last() {
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
    SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, tx));
    Registered(id)
}

impl Drop for Registered {
    fn drop(&mut self) {
        SESSIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != self.0);
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
            Some(0x0100_20ac),
            "outside Latin-1: Unicode keysym"
        );
        assert_eq!(keysym_for('\n'), Some(0xff0d));
        assert_eq!(keysym_for('\u{1b}'), Some(0xff1b), "Escape");
        assert_eq!(keysym_for('\u{7f}'), Some(0xffff), "Delete");
        assert_eq!(keysym_for('\u{7}'), None, "a bell types nothing");
        assert_eq!(text_events("a\u{7}").len(), 2, "skipped");
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
        assert_eq!(
            old_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty),
            "the newest wins, and the older one's worker is not ended by it (N8 review)"
        );
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

    #[test]
    fn kde_scrolls_smooth_and_the_rest_by_steps() {
        assert_eq!(wheel_style(Some("KDE"), false), WheelStyle::Smooth);
        assert_eq!(wheel_style(Some("kde"), false), WheelStyle::Smooth);
        assert_eq!(
            wheel_style(Some("X-Generic:KDE"), false),
            WheelStyle::Smooth
        );
        assert_eq!(wheel_style(Some("GNOME"), false), WheelStyle::Discrete);
        assert_eq!(
            wheel_style(Some("ubuntu:GNOME"), false),
            WheelStyle::Discrete
        );
        assert_eq!(
            wheel_style(Some("KDEish"), false),
            WheelStyle::Discrete,
            "a whole name only"
        );
        assert_eq!(
            wheel_style(Some("GNOME"), true),
            WheelStyle::Discrete,
            "the named desktop wins"
        );
        assert_eq!(
            wheel_style(None, true),
            WheelStyle::Smooth,
            "no name: KDE's own"
        );
        assert_eq!(wheel_style(Some(""), true), WheelStyle::Smooth);
        assert_eq!(
            wheel_style(None, false),
            WheelStyle::Discrete,
            "the spec's call"
        );
    }

    #[test]
    fn a_fine_turn_is_pixels_on_kde_and_added_up_steps_elsewhere() {
        use crate::streaming::wheel::Notches;
        let mut rest = Notches::default();
        // KDE: every turn, in its signs; a quarter notch down is 3.75 px.
        assert_eq!(
            fine_wheel_call(WheelStyle::Smooth, 0, 30, &mut rest),
            Some(WheelCall::Smooth { dx: 0.0, dy: -3.75 })
        );
        assert_eq!(
            fine_wheel_call(WheelStyle::Smooth, 1, -60, &mut rest),
            Some(WheelCall::Smooth { dx: -7.5, dy: 0.0 })
        );
        assert_eq!(rest, Notches::default(), "KDE keeps nothing back");
        assert_eq!(fine_wheel_call(WheelStyle::Smooth, 0, 0, &mut rest), None);
        // GNOME: a step once the turns make a notch, on each axis apart.
        let calls: Vec<_> = (0..4)
            .map(|_| fine_wheel_call(WheelStyle::Discrete, 0, 40, &mut rest))
            .collect();
        assert_eq!(
            calls,
            [
                None,
                None,
                Some(WheelCall::Discrete { axis: 0, steps: 1 }),
                None
            ]
        );
        assert_eq!(
            fine_wheel_call(WheelStyle::Discrete, 1, -250, &mut rest),
            Some(WheelCall::Discrete { axis: 1, steps: -2 })
        );
        assert_eq!(rest.rest(), (-10, 40));
    }

    #[test]
    fn a_notch_is_one_step_or_fifteen_pixels_in_kdes_signs() {
        assert_eq!(wheel_call(WheelStyle::Discrete, 0, 0), None);
        assert_eq!(wheel_call(WheelStyle::Smooth, 1, 0), None);
        assert_eq!(
            wheel_call(WheelStyle::Discrete, 0, -2),
            Some(WheelCall::Discrete { axis: 0, steps: -2 }),
            "steps pass as they are"
        );
        assert_eq!(
            wheel_call(WheelStyle::Discrete, 1, 3),
            Some(WheelCall::Discrete { axis: 1, steps: 3 })
        );
        // KDE sends -dy to KWin: a notch down is dy -15, an axis value of 15.
        assert_eq!(
            wheel_call(WheelStyle::Smooth, 0, 1),
            Some(WheelCall::Smooth { dx: 0.0, dy: -15.0 }),
            "down"
        );
        assert_eq!(
            wheel_call(WheelStyle::Smooth, 0, -2),
            Some(WheelCall::Smooth { dx: 0.0, dy: 30.0 }),
            "two up"
        );
        assert_eq!(
            wheel_call(WheelStyle::Smooth, 1, 1),
            Some(WheelCall::Smooth { dx: 15.0, dy: 0.0 }),
            "right, as it is"
        );
        assert_eq!(
            wheel_call(WheelStyle::Smooth, 1, -1),
            Some(WheelCall::Smooth { dx: -15.0, dy: 0.0 }),
            "left"
        );
    }
}
