//! What input a session of a window lent to another computer may send, and
//! where (window handoff, phase 5: Windows as the source).
//!
//! The rule everything here serves: **a borrower of one window may affect only
//! that window, only while it is lent to that device, and when in doubt the
//! packet is dropped.** Input is injected into the desktop, where a click goes
//! to whatever is under the pointer and a key to whatever is in front, so the
//! window is not a boundary the injector knows. This guard is: it sits between
//! the session's packets and the injector and lets through only what lands in
//! the lent window.
//!
//! - **The lease** ([`super::window_target::Lease`]) is checked before every
//!   packet. A session whose registration changed in any way (the window was
//!   given back, the index went to another lend, the handle names another
//!   window now) sends nothing more and is told to end ([`Out::end`]).
//! - **A pointer move** is placed inside the window's visible frame (and on the
//!   desktop) from where the client's picture point falls in it, asked of the
//!   window at each move; a move with nowhere to land is dropped, as is one
//!   whose spot is covered by another window (the pointer never enters one).
//! - **A click, a scroll** (which have no place of their own and would land at
//!   the owner's pointer) first put the pointer at the last place the client
//!   pointed, in the window, and are sent only when the window is in front and
//!   that spot belongs to the window.
//! - **A key or text** is sent only when the window is in front. Keys go
//!   through an allow-list (letters, digits, punctuation, editing, arrows,
//!   function and keypad keys, modifiers), never the Windows key, media or
//!   power keys, nor Alt+Tab, Alt+Esc, Ctrl+Esc or Ctrl+Shift+Esc, which the
//!   desktop handles whatever is in front.
//! - **In front** ([`belongs_to`]) is the lent window itself or a window that
//!   belongs to it (owned, through its owners, by it: its dialogs and menus)
//!   of the same program. When it is not, the window is raised once
//!   synchronously, at most every [`RAISE_EVERY`], and looked at again; if it
//!   is still not in front the packet is dropped and what this session holds
//!   down (keys, buttons, the pen) is let go, so nothing sticks.
//! - **Gamepad, the laser dot and relative moves** have no place in a window
//!   and are dropped.
//!
//! What remains: the checks run before the injector acts, not at the same
//! instant (another window may come in front, or the owner's own hand may move
//! the pointer, in that gap), and the injector rounds a place to a pixel of its
//! own (a point on the very edge of the frame may land one pixel off).
//!
//! The decisions are pure and tested everywhere over a fake [`Os`]; the real
//! one is `window_target::Machine`.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use super::input_packet::{InputPacket, PEN_BARREL, PEN_CONTACT, PenSample};
use super::window_target::{Lease, Rect};

/// The least time between two attempts to bring the window to the front.
pub const RAISE_EVERY: Duration = Duration::from_millis(300);

/// How many owners are followed up from a window to find the lent one.
const MOST_OWNERS: usize = 8;

/// The buttons a packet may name (left, middle, right, back, forward).
const MOST_BUTTON: u8 = 4;

/// What the guard asks of the computer's windows. Each answer is "no" when
/// anything is unknown.
pub trait Os {
    /// Whether the lease still stands: the registration under its index is
    /// exactly the lease's and the handle still names the window that was
    /// lent (`window_target::lease_alive`).
    fn lease_holds(&self, lease: &Lease) -> bool;
    /// The visible frame of the lent window on the desktop, in pixels.
    fn frame(&self, lease: &Lease) -> Option<Rect>;
    /// Whether the window in front is the lent one or belongs to it.
    fn front_is_lent(&self, lease: &Lease) -> bool;
    /// Whether the window that takes a click at this desktop pixel is the lent
    /// one or belongs to it.
    fn hit_is_lent(&self, lease: &Lease, at: (i32, i32)) -> bool;
    /// Bring the window to the front, now, as far as Windows allows.
    fn raise(&self, lease: &Lease);
}

/// Whether `candidate` (a window of the program `candidate_pid`) is the lent
/// window `lent` (of the program `lent_pid`) or belongs to it: a window of the
/// same program whose owner, or owner's owner, is the lent window. Its dialogs
/// and popups are the lent window's own; any other window is not, however it
/// is stacked. `owner_of` gives a window's owner (`0` for none).
pub fn belongs_to(
    candidate: isize,
    candidate_pid: u32,
    lent: isize,
    lent_pid: u32,
    owner_of: impl Fn(isize) -> isize,
) -> bool {
    if candidate == 0 || lent == 0 || lent_pid == 0 || candidate_pid != lent_pid {
        return false;
    }
    let mut at = candidate;
    for _ in 0..MOST_OWNERS {
        if at == lent {
            return true;
        }
        let owner = owner_of(at);
        if owner == 0 || owner == at {
            return false;
        }
        at = owner;
    }
    false
}

/// Whether the desktop pixel `at` is inside `frame`.
pub fn frame_contains(frame: Rect, at: (i32, i32)) -> bool {
    let inside = |p: i32, start: i32, len: u32| {
        i64::from(p) >= i64::from(start) && i64::from(p) < i64::from(start) + i64::from(len)
    };
    inside(at.0, frame.0, frame.2) && inside(at.1, frame.1, frame.3)
}

/// Where a client's picture point lands on the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aim {
    /// The desktop pixel, inside the frame and on the desktop.
    pub pixel: (i32, i32),
    /// The same as the 0..=65535 point of the desktop the injector takes.
    pub norm: (u16, u16),
}

/// Place a picture point (0..=65535 per axis of the window's frame) on the
/// desktop: inside the part of `frame` that is on `layout` (the box round every
/// monitor), the frame's own pixels at both ends. `None` when the frame or the
/// layout is not known or has no size, or nothing of the frame is on the
/// layout (nothing of the window is there to click).
pub fn aim(frame: Option<Rect>, layout: Option<Rect>, norm: (u16, u16)) -> Option<Aim> {
    let (frame, layout) = (frame?, layout?);
    if frame.2 == 0 || frame.3 == 0 || layout.2 == 0 || layout.3 == 0 {
        return None;
    }
    let max = f64::from(u16::MAX);
    let axis = |norm: u16, start: i32, len: u32, space: i32, space_len: u32| {
        let low = i64::from(start).max(i64::from(space));
        let high = (i64::from(start) + i64::from(len)).min(i64::from(space) + i64::from(space_len));
        if high <= low {
            return None;
        }
        let want = f64::from(start) + f64::from(norm) / max * f64::from(len - 1);
        let pixel = (want.round() as i64).clamp(low, high - 1);
        let scaled = (pixel - i64::from(space)) as f64 / f64::from(space_len) * max;
        Some((pixel as i32, scaled.round().clamp(0.0, max) as u16))
    };
    let (px, nx) = axis(norm.0, frame.0, frame.2, layout.0, layout.2)?;
    let (py, ny) = axis(norm.1, frame.1, frame.3, layout.1, layout.3)?;
    Some(Aim {
        pixel: (px, py),
        norm: (nx, ny),
    })
}

/// The keys a borrower of a window may press, as evdev keycodes (what the
/// packets carry): Esc, the digit and letter rows with their punctuation,
/// Backspace, Tab, Enter, Space, the modifiers (Ctrl, Shift, Alt), Caps Lock,
/// F1 to F12, the keypad, and the editing and arrow keys. Not the Windows key,
/// print screen, the media, volume and power keys, nor any key not named.
fn key_listed(key: u16) -> bool {
    matches!(key, 1..=83 | 86..=88 | 96..=98 | 100 | 102..=111)
}

const KEY_ESC: u16 = 1;
const KEY_TAB: u16 = 15;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTALT: u16 = 56;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_RIGHTALT: u16 = 100;

/// Whether `key` may be pressed while `held` are down (the keys this session
/// pressed): on the list, and not a chord the desktop handles whatever is in
/// front (Alt+Tab, Alt+Esc, Ctrl+Esc, which with Shift is the task manager).
pub fn key_allowed(key: u16, held: &BTreeSet<u16>) -> bool {
    if !key_listed(key) {
        return false;
    }
    let alt = held.contains(&KEY_LEFTALT) || held.contains(&KEY_RIGHTALT);
    let ctrl = held.contains(&KEY_LEFTCTRL) || held.contains(&KEY_RIGHTCTRL);
    !((key == KEY_TAB && alt) || (key == KEY_ESC && (alt || ctrl)))
}

/// What to do with a packet: the packets to inject (in order), and whether the
/// session must end.
#[derive(Debug, Default)]
pub struct Out {
    pub send: Vec<InputPacket>,
    pub end: bool,
}

impl Out {
    fn dropped() -> Self {
        Self::default()
    }

    fn send(packets: Vec<InputPacket>) -> Self {
        Self {
            send: packets,
            end: false,
        }
    }
}

/// The guard of one window session.
pub struct WindowGuard<O> {
    lease: Lease,
    os: O,
    /// The box round every monitor, read at the start.
    layout: Option<Rect>,
    /// The last place the client pointed, as it said it (picture point).
    last: Option<(u16, u16)>,
    /// What this session holds down at the injector.
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u8>,
    pen: Option<PenSample>,
    /// When the window was last brought to the front.
    raised: Option<Instant>,
    ended: bool,
}

impl<O: Os> WindowGuard<O> {
    pub fn new(lease: Lease, os: O, layout: Option<Rect>) -> Self {
        Self {
            lease,
            os,
            layout,
            last: None,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            pen: None,
            raised: None,
            ended: false,
        }
    }

    pub fn lease(&self) -> &Lease {
        &self.lease
    }

    /// Whether the lease still stands; once it does not, it never does again
    /// for this guard (a new lend is a new session).
    pub fn still_lent(&mut self) -> bool {
        if !self.ended && !self.os.lease_holds(&self.lease) {
            self.ended = true;
        }
        !self.ended
    }

    /// The session is over: let go of everything this session holds down.
    pub fn finish(&mut self) -> Vec<InputPacket> {
        self.ended = true;
        self.release_all()
    }

    /// What to inject, if anything, for a packet the client sent.
    pub fn admit(&mut self, packet: InputPacket, now: Instant) -> Out {
        if self.ended {
            return Out::dropped();
        }
        if !self.still_lent() {
            return Out {
                send: self.release_all(),
                end: true,
            };
        }
        match packet {
            InputPacket::MouseMoveAbs { x_norm, y_norm } => {
                self.last = Some((x_norm, y_norm));
                match self.aim_at((x_norm, y_norm)) {
                    Some(at) if self.os.hit_is_lent(&self.lease, at.pixel) => {
                        Out::send(vec![move_to(at)])
                    }
                    _ => Out::dropped(),
                }
            }
            InputPacket::Pen(pen) => self.pen_sample(pen, now),
            InputPacket::MouseClick { button, pressed } => self.click(button, pressed, now),
            InputPacket::MouseScroll { .. } => {
                let Some(at) = self.pointer_for_action(now) else {
                    return self.refused();
                };
                Out::send(vec![move_to(at), packet])
            }
            InputPacket::KeyEvent { key, pressed } => self.key(key, pressed, now),
            InputPacket::Text(_) => {
                if !self.in_front(now) {
                    return self.refused();
                }
                Out::send(vec![packet])
            }
            // A move by a distance (it could leave the window), a gamepad and
            // the laser dot have no place in a window; nothing else is input.
            _ => Out::dropped(),
        }
    }

    /// The place on the desktop for a picture point, from the window's frame
    /// now.
    fn aim_at(&self, norm: (u16, u16)) -> Option<Aim> {
        aim(self.os.frame(&self.lease), self.layout, norm)
    }

    /// Whether the lent window is in front, raising it once if it is not (not
    /// more often than [`RAISE_EVERY`]).
    fn in_front(&mut self, now: Instant) -> bool {
        if self.os.front_is_lent(&self.lease) {
            return true;
        }
        let due = self
            .raised
            .is_none_or(|at| now.saturating_duration_since(at) >= RAISE_EVERY);
        if !due {
            return false;
        }
        self.raised = Some(now);
        self.os.raise(&self.lease);
        self.os.front_is_lent(&self.lease)
    }

    /// The place a click or a scroll lands, once everything agrees it may: the
    /// window in front, the last place the client pointed in the window, and
    /// that place the window's. `None` otherwise.
    fn pointer_for_action(&mut self, now: Instant) -> Option<Aim> {
        let last = self.last?;
        if !self.in_front(now) {
            return None;
        }
        let at = self.aim_at(last)?;
        self.os.hit_is_lent(&self.lease, at.pixel).then_some(at)
    }

    /// A packet turned away because the window is not in front or not under
    /// the pointer: nothing is sent, and what is held down is let go.
    fn refused(&mut self) -> Out {
        Out::send(self.release_all())
    }

    fn click(&mut self, button: u8, pressed: bool, now: Instant) -> Out {
        if button > MOST_BUTTON {
            return Out::dropped();
        }
        if !pressed {
            // The end of a press this session made, and nothing else.
            return if self.buttons.remove(&button) {
                Out::send(vec![InputPacket::MouseClick {
                    button,
                    pressed: false,
                }])
            } else {
                Out::dropped()
            };
        }
        let Some(at) = self.pointer_for_action(now) else {
            return self.refused();
        };
        self.buttons.insert(button);
        Out::send(vec![
            move_to(at),
            InputPacket::MouseClick {
                button,
                pressed: true,
            },
        ])
    }

    fn key(&mut self, key: u16, pressed: bool, now: Instant) -> Out {
        if !pressed {
            // The end of a press this session made, and nothing else.
            return if self.keys.remove(&key) {
                Out::send(vec![InputPacket::KeyEvent { key, pressed }])
            } else {
                Out::dropped()
            };
        }
        if !key_allowed(key, &self.keys) {
            return Out::dropped();
        }
        if !self.in_front(now) {
            return self.refused();
        }
        self.keys.insert(key);
        Out::send(vec![InputPacket::KeyEvent { key, pressed }])
    }

    fn pen_sample(&mut self, pen: PenSample, now: Instant) -> Out {
        self.last = Some((pen.x_norm, pen.y_norm));
        let acts = pen.flags & (PEN_CONTACT | PEN_BARREL) != 0;
        // A pen that presses acts as a click does: the window in front first.
        if acts && !self.in_front(now) {
            return self.refused();
        }
        let placed = self
            .aim_at((pen.x_norm, pen.y_norm))
            .filter(|at| self.os.hit_is_lent(&self.lease, at.pixel));
        let Some(at) = placed else {
            // Nowhere in the window to put it: a pen that was down is lifted,
            // anything else is dropped.
            return Out::send(self.release_pen().into_iter().collect());
        };
        let mut sample = pen;
        (sample.x_norm, sample.y_norm) = at.norm;
        self.pen = acts.then_some(sample);
        Out::send(vec![InputPacket::Pen(sample)])
    }

    /// The sample that lifts a pen this session put down, if one is down.
    fn release_pen(&mut self) -> Option<InputPacket> {
        let mut pen = self.pen.take()?;
        pen.flags &= !(PEN_CONTACT | PEN_BARREL);
        pen.pressure = 0;
        Some(InputPacket::Pen(pen))
    }

    /// Everything this session holds down, let go, and forgotten.
    fn release_all(&mut self) -> Vec<InputPacket> {
        let mut out: Vec<InputPacket> = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|key| InputPacket::KeyEvent {
                key,
                pressed: false,
            })
            .collect();
        out.extend(std::mem::take(&mut self.buttons).into_iter().map(|button| {
            InputPacket::MouseClick {
                button,
                pressed: false,
            }
        }));
        out.extend(self.release_pen());
        out
    }
}

fn move_to(at: Aim) -> InputPacket {
    InputPacket::MouseMoveAbs {
        x_norm: at.norm.0,
        y_norm: at.norm.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::window_target::Identity;
    use std::cell::RefCell;
    use std::rc::Rc;

    const FRAME: Rect = (300, 100, 800, 600);
    /// Two screens side by side: 1920 + 2688 wide.
    const LAYOUT: Rect = (0, 0, 4608, 1080);
    const LENT: isize = 0x100;
    const PID: u32 = 7;

    /// A desktop that is told what is in front, what is under the pointer, and
    /// whether a raise works; and that keeps what was asked of it.
    #[derive(Default)]
    struct World {
        holds: bool,
        frame: Option<Rect>,
        front: isize,
        front_pid: u32,
        /// Windows that take a click at a place: `(frame, window, pid)`,
        /// topmost first; anything else on the desktop is `desktop`.
        stack: Vec<(Rect, isize, u32)>,
        /// What a raise does: the window it brings to the front, if it works.
        raise_works: bool,
        raises: u32,
        owners: Vec<(isize, isize)>,
    }

    #[derive(Clone)]
    struct Fake(Rc<RefCell<World>>);

    impl Fake {
        fn new() -> Self {
            Self(Rc::new(RefCell::new(World {
                holds: true,
                frame: Some(FRAME),
                front: LENT,
                front_pid: PID,
                stack: vec![(FRAME, LENT, PID)],
                raise_works: true,
                ..World::default()
            })))
        }
        fn world(&self) -> std::cell::RefMut<'_, World> {
            self.0.borrow_mut()
        }
    }

    impl Os for Fake {
        fn lease_holds(&self, _lease: &Lease) -> bool {
            self.0.borrow().holds
        }
        fn frame(&self, _lease: &Lease) -> Option<Rect> {
            self.0.borrow().frame
        }
        fn front_is_lent(&self, lease: &Lease) -> bool {
            let w = self.0.borrow();
            belongs_to(w.front, w.front_pid, lease.hwnd, lease.identity.pid, |h| {
                w.owners
                    .iter()
                    .find(|(win, _)| *win == h)
                    .map_or(0, |(_, o)| *o)
            })
        }
        fn hit_is_lent(&self, lease: &Lease, at: (i32, i32)) -> bool {
            let w = self.0.borrow();
            let hit = w
                .stack
                .iter()
                .find(|(frame, _, _)| frame_contains(*frame, at));
            hit.is_some_and(|(_, win, pid)| {
                belongs_to(*win, *pid, lease.hwnd, lease.identity.pid, |h| {
                    w.owners
                        .iter()
                        .find(|(x, _)| *x == h)
                        .map_or(0, |(_, o)| *o)
                })
            })
        }
        fn raise(&self, lease: &Lease) {
            let mut w = self.0.borrow_mut();
            w.raises += 1;
            if w.raise_works {
                w.front = lease.hwnd;
                w.front_pid = lease.identity.pid;
                // The lent window is now on top of everything.
                let at = w.frame.unwrap_or(FRAME);
                w.stack.insert(0, (at, lease.hwnd, lease.identity.pid));
            }
        }
    }

    fn lease() -> Lease {
        Lease {
            index: 1000,
            hwnd: LENT,
            generation: 1,
            identity: Identity { pid: PID, tid: 8 },
            device: "0123456789abcdef".into(),
        }
    }

    fn guard(world: &Fake) -> WindowGuard<Fake> {
        WindowGuard::new(lease(), world.clone(), Some(LAYOUT))
    }

    fn packets(out: &Out) -> Vec<String> {
        out.send.iter().map(|p| format!("{p:?}")).collect()
    }

    fn at(guard: &mut WindowGuard<Fake>, x: u16, y: u16) {
        let out = guard.admit(
            InputPacket::MouseMoveAbs {
                x_norm: x,
                y_norm: y,
            },
            Instant::now(),
        );
        assert_eq!(out.send.len(), 1, "the move was placed");
    }

    fn click(button: u8, pressed: bool) -> InputPacket {
        InputPacket::MouseClick { button, pressed }
    }

    fn key(key: u16, pressed: bool) -> InputPacket {
        InputPacket::KeyEvent { key, pressed }
    }

    fn to_pixel(norm: (u16, u16)) -> (i32, i32) {
        let px = |n: u16, start: i32, len: u32| {
            start + (f64::from(n) / 65535.0 * f64::from(len)).round() as i32
        };
        (
            px(norm.0, LAYOUT.0, LAYOUT.2),
            px(norm.1, LAYOUT.1, LAYOUT.3),
        )
    }

    // ---- The pure rules ----

    #[test]
    fn a_pictures_points_land_inside_the_windows_frame() {
        for (norm, want) in [
            ((0, 0), (300, 100)),
            ((65535, 65535), (1099, 699)),
            ((32768, 32768), (700, 400)),
        ] {
            let aimed = aim(Some(FRAME), Some(LAYOUT), norm).expect("placed");
            assert!(frame_contains(FRAME, aimed.pixel), "{norm:?}");
            assert_eq!(aimed.pixel, want, "{norm:?}");
            let (x, y) = to_pixel(aimed.norm);
            assert!(
                (x - want.0).abs() <= 1 && (y - want.1).abs() <= 1,
                "{norm:?} came to ({x}, {y}), wanted about {want:?}"
            );
        }
    }

    #[test]
    fn a_point_with_nowhere_to_land_is_not_placed() {
        assert_eq!(aim(None, Some(LAYOUT), (1, 1)), None);
        assert_eq!(aim(Some(FRAME), None, (1, 1)), None);
        assert_eq!(aim(Some((0, 0, 0, 600)), Some(LAYOUT), (1, 1)), None);
        assert_eq!(aim(Some((0, 0, 800, 0)), Some(LAYOUT), (1, 1)), None);
        assert_eq!(aim(Some(FRAME), Some((0, 0, 0, 1080)), (1, 1)), None);
        // A window wholly off every screen: nothing of it is there to click.
        for gone in [
            (9000, 100, 800, 600),
            (100, -2000, 800, 600),
            (-900, 100, 800, 600),
            (4608, 100, 800, 600),
        ] {
            assert_eq!(aim(Some(gone), Some(LAYOUT), (100, 100)), None, "{gone:?}");
        }
    }

    #[test]
    fn a_window_half_off_the_layout_is_aimed_at_what_is_on_it() {
        let frame = Some((4400, 100, 800, 600));
        let aimed = aim(frame, Some(LAYOUT), (65535, 0)).expect("placed");
        assert_eq!(aimed.pixel.0, 4607, "the layout's last pixel, not past it");
        assert!(aimed.pixel.0 < 4608 && aimed.pixel.1 >= 100);
        let aimed = aim(Some((-300, 100, 800, 600)), Some(LAYOUT), (0, 0)).expect("placed");
        assert_eq!(aimed.pixel.0, 0, "the layout's first pixel");
    }

    #[test]
    fn only_the_lent_window_and_its_own_dialogs_are_in_front() {
        // The lent window itself.
        assert!(belongs_to(0x100, 7, 0x100, 7, |_| 0));
        // Its dialog, owned by it, and a dialog's dialog.
        let owners = |h| match h {
            0x200 => 0x100,
            0x300 => 0x200,
            _ => 0,
        };
        assert!(belongs_to(0x200, 7, 0x100, 7, owners));
        assert!(belongs_to(0x300, 7, 0x100, 7, owners));
        // Another window of the same program that the lent one does not own.
        assert!(!belongs_to(0x400, 7, 0x100, 7, owners));
        // The same handle number in another program (a recycled one).
        assert!(!belongs_to(0x100, 8, 0x100, 7, |_| 0));
        // A dialog of another program owning the lent window's number.
        assert!(!belongs_to(0x200, 8, 0x100, 7, owners));
        // The lent window's own owner is not it: lending a dialog does not
        // lend the window that owns it.
        assert!(!belongs_to(0x100, 7, 0x200, 7, owners));
        // No window, no program, or a loop of owners.
        assert!(!belongs_to(0, 7, 0x100, 7, |_| 0));
        assert!(!belongs_to(0x100, 0, 0x100, 0, |_| 0));
        assert!(!belongs_to(0x500, 7, 0x100, 7, |h| if h == 0x500 {
            0x600
        } else {
            0x500
        }));
        // A deep chain is not followed forever.
        assert!(!belongs_to(0x1000, 7, 0x100, 7, |h| h + 1));
    }

    #[test]
    fn keys_are_an_allow_list_and_the_desktops_own_chords_are_out() {
        let none = BTreeSet::new();
        for ok in [
            1u16, 14, 15, 28, 30, 57, 59, 68, 87, 88, 29, 42, 56, 54, 58, 71, 83, 96, 102, 108, 111,
        ] {
            assert!(key_allowed(ok, &none), "{ok}");
        }
        // The Windows keys, print screen, media, volume, power, sleep, wake,
        // the menu key and anything unnamed.
        for no in [
            0u16,
            84,
            85,
            99,
            101,
            112,
            113,
            114,
            115,
            116,
            125,
            126,
            127,
            139,
            142,
            143,
            164,
            200,
            255,
            1000,
            u16::MAX,
        ] {
            assert!(!key_allowed(no, &none), "{no}");
        }
        let alt: BTreeSet<u16> = [KEY_LEFTALT].into();
        let right_alt: BTreeSet<u16> = [KEY_RIGHTALT].into();
        let ctrl: BTreeSet<u16> = [KEY_LEFTCTRL, 42].into();
        assert!(!key_allowed(KEY_TAB, &alt));
        assert!(!key_allowed(KEY_TAB, &right_alt));
        assert!(!key_allowed(KEY_ESC, &alt));
        assert!(!key_allowed(KEY_ESC, &ctrl), "Ctrl+Esc and Ctrl+Shift+Esc");
        assert!(
            key_allowed(KEY_TAB, &ctrl),
            "Ctrl+Tab belongs to the window"
        );
        assert!(
            key_allowed(62, &alt),
            "Alt+F4 closes the lent window, which is its own"
        );
    }

    // ---- The guard ----

    #[test]
    fn a_pointer_move_lands_in_the_frame_and_is_sent() {
        let world = Fake::new();
        let mut g = guard(&world);
        let out = g.admit(
            InputPacket::MouseMoveAbs {
                x_norm: 0,
                y_norm: 0,
            },
            Instant::now(),
        );
        assert!(!out.end);
        let [InputPacket::MouseMoveAbs { x_norm, y_norm }] = out.send[..] else {
            panic!("{:?}", packets(&out));
        };
        let (x, y) = to_pixel((x_norm, y_norm));
        assert!((x - 300).abs() <= 1 && (y - 100).abs() <= 1);
    }

    #[test]
    fn a_move_is_dropped_when_another_window_covers_the_spot() {
        let world = Fake::new();
        world
            .world()
            .stack
            .insert(0, ((300, 100, 800, 300), 0x999, 99));
        let mut g = guard(&world);
        // The top half is covered, the bottom is the lent window's.
        let out = g.admit(
            InputPacket::MouseMoveAbs {
                x_norm: 100,
                y_norm: 100,
            },
            Instant::now(),
        );
        assert!(
            out.send.is_empty(),
            "the pointer never enters another window"
        );
        let out = g.admit(
            InputPacket::MouseMoveAbs {
                x_norm: 100,
                y_norm: 60000,
            },
            Instant::now(),
        );
        assert_eq!(out.send.len(), 1);
    }

    #[test]
    fn moves_by_a_distance_a_gamepad_and_the_laser_are_dropped() {
        let world = Fake::new();
        let mut g = guard(&world);
        for p in [
            InputPacket::MouseMove { dx: 5, dy: 5 },
            InputPacket::Gamepad {
                axes: [0; 6],
                buttons: 1,
            },
            InputPacket::Laser {
                dx: 1,
                dy: 1,
                show: true,
            },
            InputPacket::RequestKeyframe,
            InputPacket::ViewOnly { enabled: false },
        ] {
            let out = g.admit(p.clone(), Instant::now());
            assert!(out.send.is_empty() && !out.end, "{p:?}");
        }
    }

    #[test]
    fn a_click_with_no_place_pointed_at_yet_is_dropped() {
        let world = Fake::new();
        let mut g = guard(&world);
        let out = g.admit(click(0, true), Instant::now());
        assert!(out.send.is_empty(), "it would land at the owner's pointer");
        let out = g.admit(
            InputPacket::MouseScroll {
                dx: 0,
                dy: 1,
                fine: None,
            },
            Instant::now(),
        );
        assert!(out.send.is_empty());
    }

    #[test]
    fn a_click_puts_the_pointer_in_the_window_first_and_its_release_follows() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 32768, 32768);
        let out = g.admit(click(0, true), Instant::now());
        assert_eq!(out.send.len(), 2);
        assert!(matches!(out.send[0], InputPacket::MouseMoveAbs { .. }));
        assert!(matches!(
            out.send[1],
            InputPacket::MouseClick {
                button: 0,
                pressed: true
            }
        ));
        let out = g.admit(click(0, false), Instant::now());
        assert_eq!(out.send.len(), 1);
        // A release of a button this session never pressed is nothing.
        let out = g.admit(click(2, false), Instant::now());
        assert!(out.send.is_empty());
        // A button that does not exist.
        let out = g.admit(click(9, true), Instant::now());
        assert!(out.send.is_empty());
    }

    #[test]
    fn a_click_follows_the_window_that_moved() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 0, 0);
        // The window moved right by 500 pixels since the last move.
        world.world().frame = Some((800, 100, 800, 600));
        world.world().stack = vec![((800, 100, 800, 600), LENT, PID)];
        let out = g.admit(click(0, true), Instant::now());
        let InputPacket::MouseMoveAbs { x_norm, .. } = &out.send[0] else {
            panic!("{:?}", packets(&out));
        };
        assert!(
            (to_pixel((*x_norm, 0)).0 - 800).abs() <= 1,
            "in the new frame"
        );
    }

    #[test]
    fn a_click_is_dropped_when_the_window_is_gone_or_off_the_screens() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 100, 100);
        world.world().frame = None;
        assert!(g.admit(click(0, true), Instant::now()).send.is_empty());
        world.world().frame = Some((9000, 0, 800, 600));
        assert!(g.admit(click(0, true), Instant::now()).send.is_empty());
    }

    #[test]
    fn a_click_under_a_window_that_cannot_be_raised_is_dropped_and_nothing_sticks() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 32768, 32768);
        // A key goes down while the window is in front.
        assert_eq!(g.admit(key(30, true), Instant::now()).send.len(), 1);
        // Another program takes the front and covers the window, and Windows
        // refuses to give it back.
        {
            let mut w = world.world();
            w.front = 0x999;
            w.front_pid = 99;
            w.stack.insert(0, (FRAME, 0x999, 99));
            w.raise_works = false;
        }
        let out = g.admit(click(0, true), Instant::now());
        assert_eq!(world.world().raises, 1, "it tried once");
        assert!(!out.end);
        assert_eq!(
            packets(&out),
            vec![format!("{:?}", key(30, false))],
            "no click, and the key that was down is let go"
        );
        // Nothing is held now: another try is just dropped.
        let out = g.admit(click(0, true), Instant::now());
        assert!(out.send.is_empty());
    }

    #[test]
    fn a_window_that_can_be_raised_is_raised_once_and_the_click_goes_through() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 32768, 32768);
        {
            let mut w = world.world();
            w.front = 0x999;
            w.front_pid = 99;
            w.stack.insert(0, (FRAME, 0x999, 99));
        }
        // The move above found it covered only if it had been; this one is
        // the click, which raises first and then sees the window under it.
        let out = g.admit(click(0, true), Instant::now());
        assert_eq!(world.world().raises, 1);
        assert_eq!(out.send.len(), 2, "{:?}", packets(&out));
    }

    #[test]
    fn a_raise_is_not_tried_again_within_the_pause() {
        let world = Fake::new();
        world.world().front = 0x999;
        world.world().front_pid = 99;
        world.world().raise_works = false;
        let mut g = guard(&world);
        let t = Instant::now();
        assert!(g.admit(key(30, true), t).send.is_empty());
        assert!(
            g.admit(key(31, true), t + Duration::from_millis(100))
                .send
                .is_empty()
        );
        assert_eq!(world.world().raises, 1, "once in the pause");
        assert!(g.admit(key(32, true), t + RAISE_EVERY).send.is_empty());
        assert_eq!(world.world().raises, 2);
    }

    #[test]
    fn a_dialog_of_the_window_in_front_takes_the_keys() {
        let world = Fake::new();
        {
            let mut w = world.world();
            w.front = 0x200;
            w.owners.push((0x200, LENT));
        }
        let mut g = guard(&world);
        assert_eq!(g.admit(key(30, true), Instant::now()).send.len(), 1);
        assert_eq!(world.world().raises, 0, "nothing to raise");
        // A dialog of another program is not its own.
        world.world().front_pid = 99;
        world.world().raise_works = false;
        let out = g.admit(key(31, true), Instant::now());
        assert_eq!(world.world().raises, 1);
        assert!(
            out.send
                .iter()
                .all(|p| !matches!(p, InputPacket::KeyEvent { pressed: true, .. }))
        );
    }

    #[test]
    fn keys_and_text_go_only_while_the_window_is_in_front() {
        let world = Fake::new();
        let mut g = guard(&world);
        assert_eq!(g.admit(key(30, true), Instant::now()).send.len(), 1);
        assert_eq!(g.admit(key(30, false), Instant::now()).send.len(), 1);
        assert_eq!(
            g.admit(InputPacket::Text("hi".into()), Instant::now())
                .send
                .len(),
            1
        );
        // The window is not in front and cannot be put there.
        {
            let mut w = world.world();
            w.front = 0x999;
            w.front_pid = 99;
            w.raise_works = false;
        }
        let out = g.admit(
            InputPacket::Text("hi".into()),
            Instant::now() + Duration::from_secs(5),
        );
        assert!(out.send.is_empty());
    }

    #[test]
    fn the_windows_key_and_alt_tab_never_get_through() {
        let world = Fake::new();
        let mut g = guard(&world);
        assert!(g.admit(key(125, true), Instant::now()).send.is_empty());
        assert!(g.admit(key(126, true), Instant::now()).send.is_empty());
        assert!(
            g.admit(key(116, true), Instant::now()).send.is_empty(),
            "power"
        );
        assert_eq!(
            g.admit(key(56, true), Instant::now()).send.len(),
            1,
            "Alt alone"
        );
        assert!(
            g.admit(key(15, true), Instant::now()).send.is_empty(),
            "Alt+Tab"
        );
        assert_eq!(g.admit(key(56, false), Instant::now()).send.len(), 1);
        assert_eq!(
            g.admit(key(15, true), Instant::now()).send.len(),
            1,
            "Tab alone"
        );
    }

    #[test]
    fn a_release_of_a_key_never_pressed_here_is_dropped() {
        let world = Fake::new();
        let mut g = guard(&world);
        assert!(g.admit(key(30, false), Instant::now()).send.is_empty());
    }

    #[test]
    fn a_scroll_goes_to_the_pointer_placed_in_the_window() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 1000, 1000);
        let scroll = InputPacket::MouseScroll {
            dx: 0,
            dy: -1,
            fine: Some((0, -120)),
        };
        let out = g.admit(scroll, Instant::now());
        assert_eq!(out.send.len(), 2);
        assert!(matches!(out.send[0], InputPacket::MouseMoveAbs { .. }));
        assert!(matches!(
            out.send[1],
            InputPacket::MouseScroll { dy: -1, .. }
        ));
    }

    #[test]
    fn a_lease_that_is_lost_ends_the_session_and_lets_go_of_what_was_held() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 100, 100);
        assert_eq!(g.admit(click(0, true), Instant::now()).send.len(), 2);
        assert_eq!(g.admit(key(30, true), Instant::now()).send.len(), 1);
        // The window was given back.
        world.world().holds = false;
        let out = g.admit(key(31, true), Instant::now());
        assert!(out.end);
        assert_eq!(
            packets(&out),
            vec![
                format!("{:?}", key(30, false)),
                format!("{:?}", click(0, false))
            ],
            "the key and the button, let go, and not the new key"
        );
        assert!(!g.still_lent());
        // Even if the registration comes back (a new lend), this session is
        // over: nothing more is sent, ever.
        world.world().holds = true;
        for p in [
            click(0, true),
            key(30, true),
            InputPacket::MouseMoveAbs {
                x_norm: 1,
                y_norm: 1,
            },
        ] {
            let out = g.admit(p, Instant::now());
            assert!(out.send.is_empty() && !out.end);
        }
        assert!(!g.still_lent());
    }

    #[test]
    fn still_lent_notices_a_lost_lease_between_packets() {
        let world = Fake::new();
        let mut g = guard(&world);
        assert!(g.still_lent());
        world.world().holds = false;
        assert!(!g.still_lent());
        assert!(!g.still_lent());
    }

    #[test]
    fn finishing_lets_go_of_what_is_held_once() {
        let world = Fake::new();
        let mut g = guard(&world);
        at(&mut g, 100, 100);
        g.admit(click(2, true), Instant::now());
        g.admit(key(29, true), Instant::now());
        let released = g.finish();
        assert_eq!(released.len(), 2);
        assert!(g.finish().is_empty());
        assert!(g.admit(key(30, true), Instant::now()).send.is_empty());
    }

    #[test]
    fn a_pen_is_placed_in_the_window_and_let_go_when_it_cannot_be() {
        let world = Fake::new();
        let mut g = guard(&world);
        let down = PenSample {
            x_norm: 32768,
            y_norm: 32768,
            pressure: 30000,
            flags: PEN_CONTACT,
            ..PenSample::default()
        };
        let out = g.admit(InputPacket::Pen(down), Instant::now());
        let [InputPacket::Pen(placed)] = out.send[..] else {
            panic!("{:?}", packets(&out));
        };
        let (x, y) = to_pixel((placed.x_norm, placed.y_norm));
        assert!((x - 700).abs() <= 1 && (y - 400).abs() <= 1, "in the frame");
        // The window is gone while the pen is down: it is lifted.
        world.world().frame = None;
        let out = g.admit(InputPacket::Pen(down), Instant::now());
        let [InputPacket::Pen(lift)] = out.send[..] else {
            panic!("{:?}", packets(&out));
        };
        assert_eq!(lift.flags & PEN_CONTACT, 0);
        assert_eq!(lift.pressure, 0);
        // And nothing more, with no pen down.
        assert!(
            g.admit(InputPacket::Pen(down), Instant::now())
                .send
                .is_empty()
        );
    }

    #[test]
    fn a_pen_pressing_where_another_window_covers_is_dropped() {
        let world = Fake::new();
        world.world().stack.insert(0, (FRAME, 0x999, 99));
        world.world().raise_works = false;
        let mut g = guard(&world);
        let down = PenSample {
            x_norm: 100,
            y_norm: 100,
            pressure: 30000,
            flags: PEN_CONTACT,
            ..PenSample::default()
        };
        assert!(
            g.admit(InputPacket::Pen(down), Instant::now())
                .send
                .is_empty()
        );
    }

    #[test]
    fn the_guard_of_the_real_machine_off_windows_sends_nothing() {
        use crate::streaming::window_target::Machine;
        if cfg!(not(windows)) {
            let mut g = WindowGuard::new(lease(), Machine, Some(LAYOUT));
            let out = g.admit(
                InputPacket::MouseMoveAbs {
                    x_norm: 1,
                    y_norm: 1,
                },
                Instant::now(),
            );
            assert!(out.end, "no lease stands where there are no windows");
            assert!(out.send.is_empty());
        }
    }
}
