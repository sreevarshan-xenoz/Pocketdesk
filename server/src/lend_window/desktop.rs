//! Lending a window of a Windows computer (phase 5 of the window handoff:
//! Windows as the source). The same `list`, `lend` and `return` as on Sway and
//! Hyprland, the same gate, and a viewer that cannot tell the difference; what
//! differs is what a lend is.
//!
//! **No second screen.** Windows has no virtual display without a driver
//! (IddCx is out of scope), so a lent window stays on the desktop where it is
//! and its owner sees it too. A lend is then: (1) the window is registered
//! ([`window_target`]) under an index past every monitor's, which is the
//! "monitor index" the answer carries and the borrower's session dials; (2)
//! that session captures the window alone with Windows.Graphics.Capture, and
//! lands the borrower's pointer in the window's frame (the streamer's
//! `PointerSource::Window`), bringing the window to the front before a click,
//! scroll or key; (3) nothing else moves. A minimised window is restored (a
//! minimised window sends no frames), a window the borrower's window is a
//! different size from is sized to the borrower's picture (the "resize follows
//! the viewer" of the other systems), and `return` puts the window back as it
//! was: its show state (minimised, maximised, normal) and its place.
//!
//! **What a lend grants, and to whom.** A lend is a grant of one window to one
//! device, and nothing a borrower does reaches past it. (1) Only a window the
//! list would show (the picker's windows, not this program's own, not the
//! shell's own windows such as the desktop and the taskbar, not a program
//! this server could not drive or look into) may be lent: a handle named any
//! other way is refused as a window that is not there. (2) The registration
//! names the device it was lent to, the window's handle, the program and the
//! thread that own the window, and a generation no other registration has;
//! a session is accepted only from that device and holds a lease on exactly
//! that registration ([`window_target`]), so a window given back, closed,
//! lent again or whose handle Windows gave to another window ends the
//! sessions of the old grant at once and none of them can drive the new one.
//! (3) A handle is never followed alone: every later look at the window (its
//! facts, its place to put back, its size) must find the same program and
//! thread, or the window is taken for gone and left alone, so a recycled
//! handle is never moved, sized or restored. (4) Giving a window back ends
//! the grant first, before the window is put back and whether or not it goes;
//! a lend asked for again is a new grant. What the session may send, and
//! where, is in core's `window_guard`.
//!
//! **What the picture is.** The window's frame as the desktop draws it (title
//! bar and borders, without the invisible shadow around): DWM's extended frame
//! bounds, which is what Windows.Graphics.Capture produces for a window, and
//! the rectangle the pointer is mapped into. A request to size the window is
//! for that picture, so the outer size asked of Windows is the picture's plus
//! the shadow ([`outer_for_picture`]).
//!
//! **What is kept, and for how long.** Where each window was is kept in memory
//! only: a window handle does not outlive the program that owns it, and one
//! that came back after a restart of this server could belong to another
//! window, so nothing is written to `lent_windows.json` and nothing is put
//! back at the next start. A server that crashes while a window is lent leaves
//! it as the borrower last sized it; a clean stop puts every window back.
//!
//! All of this is written against [`Desktop`], the few things asked of
//! Windows, so the rules and the lend's whole flow are tested on any system
//! with a fake desktop; [`super::win32`] is the real one and is tested only on
//! Windows, by the owner's checks.

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use pocketdesk_core::streaming::window_target::{self, Identity, Lease, RegisterError};
use serde_json::{Value, json};

use super::{Env, Lent, Seen, error};
use crate::hyprland::HyprWindow;
use crate::virtual_screen::{Output, Rect};

/// How a window is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    Normal,
    Maximized,
    Minimized,
}

impl Show {
    /// From a `WINDOWPLACEMENT`'s `showCmd`: the minimising commands (2
    /// `SW_SHOWMINIMIZED`, 6 `SW_MINIMIZE`, 7 `SW_SHOWMINNOACTIVE`, 11
    /// `SW_FORCEMINIMIZE`) are minimised, 3 (`SW_SHOWMAXIMIZED`) is maximised,
    /// and anything else is shown as it is.
    pub fn from_cmd(cmd: u32) -> Self {
        match cmd {
            2 | 6 | 7 | 11 => Self::Minimized,
            3 => Self::Maximized,
            _ => Self::Normal,
        }
    }

    /// The `showCmd` that puts a window back so, without taking the focus
    /// where Windows allows that (`SW_SHOWNOACTIVATE`, `SW_SHOWMINNOACTIVE`;
    /// a maximised window has no such form).
    pub fn to_cmd(self) -> u32 {
        match self {
            Self::Normal => 4,
            Self::Maximized => 3,
            Self::Minimized => 7,
        }
    }
}

/// Where a window is, as `GetWindowPlacement` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub show: Show,
    /// The place and size it has when shown normally: left, top, right,
    /// bottom.
    pub normal: [i32; 4],
    /// The rest of the placement, kept so it goes back whole.
    pub min_at: [i32; 2],
    pub max_at: [i32; 2],
    pub flags: u32,
}

/// What the desktop says of a window now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// The frame the capture shows: left, top, width, height.
    pub frame: [i32; 4],
    pub minimized: bool,
    pub maximized: bool,
    /// The program and thread that own the window now: a handle that names
    /// another window later has another.
    pub identity: Identity,
}

/// What kind of window a handle is, for whether it may be lent at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kind {
    /// The window's class (Windows' own name for what kind of window it is:
    /// `Progman` is the desktop).
    pub class: String,
    /// Its program runs with more rights than this server (elevated while the
    /// server is not), or could not be looked into: input cannot be sent to
    /// it, and what it shows is not the owner's to lend by a borrower's pick.
    pub beyond_us: bool,
}

/// What lending a window asks of Windows. [`super::win32::Win32`] is the real
/// one; the tests have a fake.
pub trait Desktop: Sync {
    /// Whether Windows.Graphics.Capture can capture a window here.
    fn can_capture(&self) -> bool;
    /// The windows a user would pick (what Alt-Tab shows), as the window
    /// picker describes them.
    fn windows(&self) -> io::Result<Vec<HyprWindow>>;
    /// The window now; `None` when it is gone (closed, or not a window).
    fn facts(&self, hwnd: isize) -> Option<Facts>;
    /// What kind of window it is; `None` when it is gone.
    fn kind(&self, hwnd: isize) -> Option<Kind>;
    fn placement(&self, hwnd: isize) -> io::Result<Placement>;
    fn set_placement(&self, hwnd: isize, placement: &Placement) -> io::Result<()>;
    /// Out of the taskbar or maximised, to the place it has when shown
    /// normally.
    fn restore(&self, hwnd: isize) -> io::Result<()>;
    /// Size the window so that its frame (what [`Facts::frame`] says) is
    /// `width`×`height`, and keep it on its screen (the work area: a window
    /// that grew is moved back by what it overhangs, see [`keep_on_screen`]).
    /// A window may not take every size, and one that is minimised or
    /// maximised is not asked ([`can_size`]).
    fn size_picture(&self, hwnd: isize, width: u32, height: u32) -> io::Result<()>;
}

// ---- The rules ----

/// The window `id` names: `0x` and the handle in hex (what the window picker
/// calls its address); no other id is a handle, and zero is none.
pub fn parse_hwnd(id: &str) -> Option<isize> {
    let hex = id.strip_prefix("0x")?;
    if !(1..=16).contains(&hex.len())
        || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let value = usize::from_str_radix(hex, 16).ok()?;
    (value != 0).then_some(value as isize)
}

/// The window id the borrower is given for a handle.
pub fn window_id(hwnd: isize) -> String {
    format!("0x{:x}", hwnd as usize)
}

/// The device and the window of the key a lend is recorded under
/// (`<device>#lend-<window>`, [`super::lend_key`]).
fn split_key(key: &str) -> Option<(&str, isize)> {
    let (device, window) = key.rsplit_once("#lend-")?;
    Some((device, parse_hwnd(window)?))
}

/// What the picture's own name says: a screen's, so the lend's answer and
/// record read as they do anywhere.
pub fn output_name(index: u32) -> String {
    format!("WIN-{index}")
}

/// Whether a window of the program `class` (its file's name, lower case, as
/// the window picker gives it) is this one's own: the app and the server never
/// lend their own windows (the viewing windows would show themselves).
pub fn is_own_program(class: &str) -> bool {
    class.to_ascii_lowercase().starts_with("pocketdesk")
}

/// Whether a window the picker described is one to list: not this server's or
/// the app's, shown, and with a size.
pub fn keep_listed(window: &HyprWindow, own_pid: u32) -> bool {
    window.mapped
        && !window.hidden
        && window.pid != own_pid
        && !is_own_program(&window.class)
        && window.size[0] > 0
        && window.size[1] > 0
}

/// The outer size to ask Windows for so a window's frame is `want` wide and
/// tall: the frame is inside the outer rectangle (the invisible shadow around
/// it is the difference), so what the window has outside its frame is added.
/// Never less than one pixel a side.
pub fn outer_for_picture(want: (u32, u32), outer: (i32, i32), frame: (i32, i32)) -> (i32, i32) {
    let side = |want: u32, outer: i32, frame: i32| {
        let want = i64::from(want.min(i32::MAX as u32));
        let shadow = (i64::from(outer) - i64::from(frame)).max(0);
        (want + shadow).clamp(1, i64::from(i32::MAX)) as i32
    };
    (
        side(want.0, outer.0, frame.0),
        side(want.1, outer.1, frame.1),
    )
}

/// Whether a window with a frame of `have` needs sizing to `want`: a pixel
/// either way is no change (an odd side is made even by the encoder, and a
/// window left alone is a window the owner did not notice).
pub fn needs_resize(have: (i32, i32), want: (u32, u32)) -> bool {
    let off = |have: i32, want: u32| (i64::from(have) - i64::from(want)).abs() > 1;
    off(have.0, want.0) || off(have.1, want.1)
}

/// Whether a window is not where it was and must be put back: how it is shown
/// or where it sits when shown normally differs.
pub fn needs_restore(before: &Placement, now: &Placement) -> bool {
    before.show != now.show || before.normal != now.normal
}

/// The classes of windows that are the shell's own: the desktop, the taskbar
/// and the surfaces it draws on (Start, search, the notification area).
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "NotifyIconOverflowWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "ForegroundStaging",
];

/// Whether `class` is one of the shell's own windows.
pub fn is_shell_class(class: &str) -> bool {
    SHELL_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(class))
}

/// Whether a window of this kind may be lent: not the shell's own, and not a
/// program with more rights than this server.
pub fn lendable_kind(kind: &Kind) -> bool {
    !is_shell_class(&kind.class) && !kind.beyond_us
}

/// Whether a window in this state can be asked to take a size: a minimised
/// window has no frame to size, and a maximised one stays as large as its
/// screen lets it. Such a window is lent as it is and the answer is the size
/// it has (the viewer scales the picture), never an error.
pub fn can_size(facts: &Facts) -> bool {
    !facts.minimized && !facts.maximized
}

/// Where a window that was just sized goes so that it is on its screen: its
/// left and top (the outer rectangle's) given the outer size and the work
/// area of its screen (left, top, right, bottom). A window larger than the
/// work area on a side is put at the work area's edge on that side (the
/// start of it shows); one that overhangs is moved back by what it overhangs;
/// one inside is left where it is.
pub fn keep_on_screen(left: i32, top: i32, width: i32, height: i32, work: [i32; 4]) -> (i32, i32) {
    let axis = |start: i32, len: i32, low: i32, high: i32| {
        let (start, len, low, high) = (
            i64::from(start),
            i64::from(len),
            i64::from(low),
            i64::from(high),
        );
        let placed = if len >= high - low {
            low
        } else {
            start.clamp(low, high - len)
        };
        placed as i32
    };
    (
        axis(left, width, work[0], work[2]),
        axis(top, height, work[1], work[3]),
    )
}

// ---- The environment ----

/// A window lent: the lease on its registration (its handle, the index a
/// session captures it by, the program and thread that own it, the device it
/// is lent to).
#[derive(Debug, Clone)]
struct Screen {
    lease: Lease,
}

impl Screen {
    fn hwnd(&self) -> isize {
        self.lease.hwnd
    }

    fn index(&self) -> u32 {
        self.lease.index
    }

    fn identity(&self) -> Identity {
        self.lease.identity
    }
}

/// Where a lent window was, and which window that was.
#[derive(Debug, Clone, Copy)]
struct Held {
    placement: Placement,
    identity: Identity,
}

#[derive(Default)]
struct State {
    /// The lends, by the key the lend path uses.
    screens: BTreeMap<String, Screen>,
    /// Where each lent window was, until it is put back.
    placed: BTreeMap<isize, Held>,
}

/// Lending as Windows does it: [`Env`] over a [`Desktop`].
pub struct WinEnv<D> {
    desk: D,
    state: Mutex<State>,
}

/// What a lend of a window that cannot be lent says, whatever the reason (it
/// is not there, it is not one that is listed): a borrower learns nothing
/// about windows it was not offered.
const NOT_THERE: &str = "No such window on this computer (it may have closed)";

impl<D: Desktop> WinEnv<D> {
    pub fn new(desk: D) -> Self {
        Self {
            desk,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The window `hwnd` now, if it is still the window `identity` owned
    /// when it was lent; a handle that names another window is gone.
    fn facts_of(&self, hwnd: isize, identity: Identity) -> Option<Facts> {
        self.desk.facts(hwnd).filter(|f| f.identity == identity)
    }

    /// Whether the window `hwnd` is one the list would show a borrower: the
    /// only windows that may be lent.
    fn lendable(&self, hwnd: isize) -> bool {
        let id = window_id(hwnd);
        Env::windows(self).is_ok_and(|all| all.iter().any(|w| w.address == id))
    }

    /// The answer for a window lent: its screen's name and index and the size
    /// its picture has now.
    fn reply(&self, screen: &Screen) -> Value {
        match self.facts_of(screen.hwnd(), screen.identity()) {
            Some(facts) => json!({
                "name": output_name(screen.index()),
                "index": screen.index(),
                "width": facts.frame[2].max(0),
                "height": facts.frame[3].max(0),
            }),
            None => error("The window closed"),
        }
    }

    /// A minimised window sends no frames: restore one that is, as often as
    /// it is asked (a view that dials again finds it as it was left).
    fn unminimise(&self, screen: &Screen) {
        if self
            .facts_of(screen.hwnd(), screen.identity())
            .is_some_and(|f| f.minimized)
            && let Err(e) = self.desk.restore(screen.hwnd())
        {
            tracing::warn!("A lent window could not be brought back from the taskbar: {e}");
        }
    }

    /// Size the window's picture to `width`×`height` if it is not already. A
    /// window that cannot be asked (minimised, maximised) is left as it is and
    /// the answer says the size it has: that is no error.
    fn size_to(&self, screen: &Screen, width: u32, height: u32) -> io::Result<()> {
        let Some(facts) = self.facts_of(screen.hwnd(), screen.identity()) else {
            return Err(io::Error::other("the window closed"));
        };
        if can_size(&facts) && needs_resize((facts.frame[2], facts.frame[3]), (width, height)) {
            self.desk.size_picture(screen.hwnd(), width, height)?;
        }
        Ok(())
    }

    /// Put the window `hwnd` back as `before` says, if it is not as it was and
    /// it is still the window that was lent: a window that is gone, or a
    /// handle that names another window now, is left alone.
    fn put_back_as(&self, hwnd: isize, held: &Held) -> io::Result<()> {
        if self.facts_of(hwnd, held.identity).is_none() {
            return Ok(());
        }
        let now = self.desk.placement(hwnd)?;
        if needs_restore(&held.placement, &now) {
            self.desk.set_placement(hwnd, &held.placement)?;
        }
        Ok(())
    }

    /// The screen of a lend asked for again, with a grant that stands: the one
    /// it has, or, when that was ended (the window was being given back and
    /// would not go), a new one for the same device, window and screen name.
    fn regrant(&self, state: &mut State, key: &str, screen: Screen) -> Result<Screen, Value> {
        if self.facts_of(screen.hwnd(), screen.identity()).is_none() {
            return Err(error("The window closed"));
        }
        if window_target::lease_holds(&screen.lease) {
            return Ok(screen);
        }
        let again = window_target::register(
            screen.hwnd(),
            &screen.lease.device,
            screen.identity(),
            Some(screen.index()),
        );
        match again {
            Ok((lease, _)) => {
                let screen = Screen { lease };
                state.screens.insert(key.to_string(), screen.clone());
                Ok(screen)
            }
            Err(RegisterError::Full) => Err(error("Too many windows are lent at once")),
            Err(_) => Err(error("Another computer has borrowed that window")),
        }
    }
}

impl<D: Desktop> Env for WinEnv<D> {
    fn cannot(&self) -> Option<&'static str> {
        (!self.desk.can_capture()).then_some(
            "This computer cannot lend a window: Windows.Graphics.Capture is not available (Windows 10 version 1903 or later)",
        )
    }

    fn seen(&self, window: &str) -> io::Result<Option<Seen>> {
        let hwnd = parse_hwnd(window).ok_or_else(|| super::other("not a Windows window id"))?;
        // A window that is lent is the window that was lent: a handle that
        // names another one now is gone (the sweep then ends the lend).
        let lent = self
            .state()
            .screens
            .values()
            .find(|s| s.hwnd() == hwnd)
            .map(Screen::identity);
        let facts = match lent {
            Some(identity) => self.facts_of(hwnd, identity),
            None => self.desk.facts(hwnd),
        };
        Ok(facts.map(|facts| Seen {
            id: window.to_string(),
            // Nothing tiles on this desktop and a window is never "put" on a
            // screen: these say only that there is nothing to keep.
            floating: true,
            at: [facts.frame[0], facts.frame[1]],
            size: [facts.frame[2], facts.frame[3]],
            origin: [facts.frame[0], facts.frame[1]],
            frame: [facts.frame[2], facts.frame[3]],
            output: String::new(),
            workspace: "desktop".to_string(),
            workspace_id: 1,
            border: String::new(),
            border_width: 0,
            fullscreen: 0,
            fullscreen_client: 0,
        }))
    }

    fn windows(&self) -> io::Result<Vec<HyprWindow>> {
        let own = std::process::id();
        Ok(self
            .desk
            .windows()?
            .into_iter()
            .filter(|w| keep_listed(w, own))
            .filter(|w| {
                parse_hwnd(&w.address)
                    .and_then(|hwnd| self.desk.kind(hwnd))
                    .is_some_and(|kind| lendable_kind(&kind))
            })
            .collect())
    }

    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
        let Some((device, hwnd)) = split_key(key) else {
            return error("not a window");
        };
        let mut state = self.state();
        if let Some(screen) = state.screens.get(key).cloned() {
            // Asked again: the same window, shown if it was minimised since,
            // under a grant that stands.
            let screen = match self.regrant(&mut state, key, screen) {
                Ok(screen) => screen,
                Err(refused) => return refused,
            };
            self.unminimise(&screen);
            return self.reply(&screen);
        }
        // Only a window the list shows may be lent, whatever handle is named.
        let Some(facts) = self.desk.facts(hwnd).filter(|_| self.lendable(hwnd)) else {
            return error(NOT_THERE);
        };
        let (lease, created) = match window_target::register(hwnd, device, facts.identity, None) {
            Ok(made) => made,
            Err(RegisterError::Full) => return error("Too many windows are lent at once"),
            Err(RegisterError::Held) => return error("Another computer has borrowed that window"),
            Err(RegisterError::Nothing) => return error("not a window"),
        };
        // Only what this call made is undone by it: a registration that was
        // there already (the same device's, the same window) is not ours to
        // remove.
        let undo = |lease: &Lease| {
            if created {
                window_target::unregister(lease);
            }
        };
        let before = match self.desk.placement(hwnd) {
            Ok(placement) => placement,
            Err(e) => {
                undo(&lease);
                return error(format!("Could not read where the window is: {e}"));
            }
        };
        if facts.minimized
            && let Err(e) = self.desk.restore(hwnd)
        {
            undo(&lease);
            return error(format!(
                "Could not bring the window back from the taskbar: {e}"
            ));
        }
        let screen = Screen { lease };
        // The first place of this window is the one to put back: one taken
        // for the same window already is kept.
        if state
            .placed
            .get(&hwnd)
            .is_none_or(|held| held.identity != facts.identity)
        {
            state.placed.insert(
                hwnd,
                Held {
                    placement: before,
                    identity: facts.identity,
                },
            );
        }
        state.screens.insert(key.to_string(), screen.clone());
        // A window that will not take the size is lent at the size it has: the
        // answer says which.
        if let Err(e) = self.size_to(&screen, width, height) {
            tracing::warn!("A lent window was not sized to the borrower's window: {e}");
        }
        self.reply(&screen)
    }

    fn drop_screen(&self, key: &str) {
        let mut state = self.state();
        let Some(screen) = state.screens.remove(key) else {
            return;
        };
        window_target::unregister(&screen.lease);
        // A lend that failed after the window was touched: back as it was.
        if let Some(held) = state.placed.remove(&screen.hwnd())
            && let Err(e) = self.put_back_as(screen.hwnd(), &held)
        {
            tracing::warn!("A window that was not lent after all could not be put back: {e}");
        }
    }

    fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
        let Some(screen) = self.state().screens.get(key).cloned() else {
            return error("That window is not lent");
        };
        // A resize is no new grant: a lend that was ended stays ended until
        // it is asked for again.
        if !window_target::lease_holds(&screen.lease) {
            return error("That window is not lent");
        }
        self.unminimise(&screen);
        if let Err(e) = self.size_to(&screen, width, height) {
            return error(format!("The window was not sized: {e}"));
        }
        self.reply(&screen)
    }

    fn output(&self, name: &str) -> Option<Output> {
        let screen = self
            .state()
            .screens
            .values()
            .find(|s| output_name(s.index()) == name)
            .cloned()?;
        let facts = self.facts_of(screen.hwnd(), screen.identity())?;
        Some(Output {
            name: name.to_string(),
            rect: Rect {
                x: facts.frame[0],
                y: facts.frame[1],
                width: facts.frame[2],
                height: facts.frame[3],
            },
        })
    }

    fn send(&self, _lent: &Lent, _to: &Output, _width: u32, _height: u32) -> io::Result<()> {
        // Nothing moves: the window is lent where it stands, and was sized
        // when its screen was made.
        Ok(())
    }

    fn follow(&self, _lent: &Lent, _to: &Output, _width: u32, _height: u32) -> io::Result<()> {
        // Sized by `resize_screen`, which says the size it came to.
        Ok(())
    }

    fn back(&self, lent: &Lent) -> io::Result<()> {
        let hwnd = parse_hwnd(&lent.window).ok_or_else(super::no_place)?;
        let mut state = self.state();
        // The grant ends first, before the window is moved back and whether
        // or not it goes: the sessions bound to it stop at once. A window
        // that will not go back stays lent for another try, and a lend asked
        // for again is a new grant.
        for screen in state.screens.values().filter(|s| s.hwnd() == hwnd) {
            window_target::unregister(&screen.lease);
        }
        let Some(held) = state.placed.get(&hwnd).copied() else {
            return Ok(());
        };
        // A window that will not go back keeps its place in the table, so a
        // later try can.
        self.put_back_as(hwnd, &held)?;
        state.placed.remove(&hwnd);
        Ok(())
    }

    fn save(&self, _lent: &[Lent]) -> io::Result<()> {
        // Handles are not kept across a restart (see the module's notes).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lend_window::{Gone, Lends, Looks, Present, lend_with, list_with, return_with};
    use std::time::Instant;

    const DEVICE: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    /// A window of the fake desktop.
    #[derive(Clone)]
    struct Win {
        frame: [i32; 4],
        /// Outside the frame: the shadow Windows adds to each side.
        shadow: i32,
        placement: Placement,
        /// The smallest picture it will take.
        least: (i32, i32),
        title: &'static str,
        class: &'static str,
        pid: u32,
        /// Who owns it: a handle recycled for another window has another.
        identity: Identity,
        /// Windows' own class name of it.
        win_class: &'static str,
        /// Its program has more rights than this server.
        beyond_us: bool,
    }

    struct Fake {
        wins: Mutex<BTreeMap<isize, Win>>,
        capture: bool,
        /// Every call that changed a window, in order.
        log: Mutex<Vec<String>>,
        /// A window that refuses to go back.
        stuck: Mutex<Vec<isize>>,
        /// A window whose place cannot be read.
        unreadable: Mutex<Vec<isize>>,
        /// Whether the list shows a minimised window (the real picker does
        /// not; this models one minimised between the list and the lend).
        lists_minimised: bool,
    }

    fn placement(show: Show, normal: [i32; 4]) -> Placement {
        Placement {
            show,
            normal,
            min_at: [-1, -1],
            max_at: [-1, -1],
            flags: 0,
        }
    }

    fn win(x: i32, y: i32, w: i32, h: i32) -> Win {
        Win {
            frame: [x, y, w, h],
            shadow: 7,
            placement: placement(Show::Normal, [x, y, x + w, y + h]),
            least: (200, 100),
            title: "Notes",
            class: "notepad",
            pid: 4242,
            identity: Identity { pid: 4242, tid: 7 },
            win_class: "Notepad",
            beyond_us: false,
        }
    }

    impl Fake {
        fn new(wins: &[(isize, Win)]) -> Self {
            Self {
                wins: Mutex::new(wins.iter().cloned().collect()),
                capture: true,
                log: Mutex::new(Vec::new()),
                stuck: Mutex::new(Vec::new()),
                unreadable: Mutex::new(Vec::new()),
                lists_minimised: false,
            }
        }

        fn win(&self, hwnd: isize) -> Win {
            self.wins.lock().unwrap().get(&hwnd).cloned().unwrap()
        }

        fn with(&self, hwnd: isize, change: impl FnOnce(&mut Win)) {
            change(self.wins.lock().unwrap().get_mut(&hwnd).unwrap());
        }

        fn close(&self, hwnd: isize) {
            self.wins.lock().unwrap().remove(&hwnd);
        }

        /// The handle now names another program's window.
        fn recycle(&self, hwnd: isize, pid: u32) {
            self.with(hwnd, |w| {
                w.identity = Identity { pid, tid: 99 };
                w.pid = pid;
            });
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Desktop for Fake {
        fn can_capture(&self) -> bool {
            self.capture
        }

        fn windows(&self) -> io::Result<Vec<HyprWindow>> {
            Ok(self
                .wins
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, w)| self.lists_minimised || w.placement.show != Show::Minimized)
                .map(|(hwnd, w)| HyprWindow {
                    address: window_id(*hwnd),
                    title: w.title.to_string(),
                    class: w.class.to_string(),
                    pid: w.pid,
                    mapped: true,
                    hidden: false,
                    at: [w.frame[0], w.frame[1]],
                    size: [w.frame[2], w.frame[3]],
                    monitor: 0,
                    fullscreen: 0,
                    workspace: Default::default(),
                })
                .collect())
        }

        fn facts(&self, hwnd: isize) -> Option<Facts> {
            let w = self.wins.lock().unwrap().get(&hwnd)?.clone();
            Some(Facts {
                frame: w.frame,
                minimized: w.placement.show == Show::Minimized,
                maximized: w.placement.show == Show::Maximized,
                identity: w.identity,
            })
        }

        fn kind(&self, hwnd: isize) -> Option<Kind> {
            let w = self.wins.lock().unwrap().get(&hwnd)?.clone();
            Some(Kind {
                class: w.win_class.to_string(),
                beyond_us: w.beyond_us,
            })
        }

        fn placement(&self, hwnd: isize) -> io::Result<Placement> {
            if self.unreadable.lock().unwrap().contains(&hwnd) {
                return Err(io::Error::other("the place cannot be read"));
            }
            self.wins
                .lock()
                .unwrap()
                .get(&hwnd)
                .map(|w| w.placement)
                .ok_or_else(|| io::Error::other("gone"))
        }

        fn set_placement(&self, hwnd: isize, p: &Placement) -> io::Result<()> {
            if self.stuck.lock().unwrap().contains(&hwnd) {
                return Err(io::Error::other("the window will not move"));
            }
            self.log.lock().unwrap().push(format!("set {hwnd:#x}"));
            let mut wins = self.wins.lock().unwrap();
            let w = wins
                .get_mut(&hwnd)
                .ok_or_else(|| io::Error::other("gone"))?;
            w.placement = *p;
            if p.show == Show::Normal {
                w.frame = [
                    p.normal[0],
                    p.normal[1],
                    p.normal[2] - p.normal[0],
                    p.normal[3] - p.normal[1],
                ];
            }
            Ok(())
        }

        fn restore(&self, hwnd: isize) -> io::Result<()> {
            self.log.lock().unwrap().push(format!("restore {hwnd:#x}"));
            let mut wins = self.wins.lock().unwrap();
            let w = wins
                .get_mut(&hwnd)
                .ok_or_else(|| io::Error::other("gone"))?;
            w.placement.show = Show::Normal;
            let n = w.placement.normal;
            w.frame = [n[0], n[1], n[2] - n[0], n[3] - n[1]];
            Ok(())
        }

        fn size_picture(&self, hwnd: isize, width: u32, height: u32) -> io::Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(format!("size {hwnd:#x} {width}x{height}"));
            let mut wins = self.wins.lock().unwrap();
            let w = wins
                .get_mut(&hwnd)
                .ok_or_else(|| io::Error::other("gone"))?;
            if w.placement.show != Show::Normal {
                return Err(io::Error::other("sized while not shown normally"));
            }
            w.frame[2] = (width as i32).max(w.least.0);
            w.frame[3] = (height as i32).max(w.least.1);
            let n = &mut w.placement.normal;
            n[2] = n[0] + w.frame[2];
            n[3] = n[1] + w.frame[3];
            Ok(())
        }
    }

    /// The tests share the process-wide table of window indexes: the ones that
    /// lend take turns, so a test that fills the table does not starve another.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::lend_window::table_turn()
    }

    /// The window the registration under `index` holds for DEVICE, if any.
    fn lent_hwnd(index: u32) -> Option<isize> {
        window_target::lease_for(index, Some(DEVICE))
            .ok()
            .map(|l| l.hwnd)
    }

    fn first(v: &Value) -> &Value {
        &v["windows"][0]
    }

    // ---- The rules ----

    #[test]
    fn only_a_hex_handle_names_a_window() {
        assert_eq!(parse_hwnd("0x1a2b"), Some(0x1a2b));
        assert_eq!(parse_hwnd("0xffffffffffffff"), Some(0xff_ffff_ffff_ffff));
        for no in [
            "",
            "0x",
            "1a2b",
            "5",
            "0X1A2B",
            "0x1A2B",
            "0x0",
            "0x00",
            " 0x1",
            "0x1 ",
            "0x1g",
            "0x-1",
            "0x11111111111111111",
            "-0x1",
            "0x1;2",
            "0x1,2",
        ] {
            assert_eq!(parse_hwnd(no), None, "{no:?}");
        }
        // Whatever it is, an id that is a handle is also a window id the
        // protocol takes.
        assert!(crate::lend_window::is_window_id(&window_id(0x1a2b)));
        assert_eq!(parse_hwnd(&window_id(0x7ffe_1234)), Some(0x7ffe_1234));
    }

    #[test]
    fn the_key_of_a_lend_names_its_window() {
        let key = crate::lend_window::lend_key(DEVICE, "0xabc");
        assert_eq!(split_key(&key), Some((DEVICE, 0xabc)));
        assert_eq!(split_key("no key"), None);
        assert_eq!(split_key(&format!("{DEVICE}#lend-nothex")), None);
        // A device id that has the separator in it keeps all of itself.
        assert_eq!(
            split_key("odd#lend-id#lend-0x5"),
            Some(("odd#lend-id", 0x5))
        );
    }

    #[test]
    fn the_screen_of_a_window_is_a_plain_name() {
        let name = output_name(1003);
        assert_eq!(name, "WIN-1003");
        assert!(crate::lend_window::is_plain(&name));
    }

    #[test]
    fn this_programs_own_windows_are_never_listed() {
        for own in [
            "pocketdesk",
            "pocketdesk-desktop",
            "PocketDesk",
            "pocketdesk-server",
        ] {
            assert!(is_own_program(own), "{own}");
        }
        for other in ["notepad", "chrome", "", "desk", "mypocketdesk"] {
            assert!(!is_own_program(other), "{other}");
        }
        let mut w = HyprWindow {
            address: "0x1".into(),
            title: "t".into(),
            class: "notepad".into(),
            pid: 10,
            mapped: true,
            hidden: false,
            at: [0, 0],
            size: [800, 600],
            monitor: 0,
            fullscreen: 0,
            workspace: Default::default(),
        };
        assert!(keep_listed(&w, 99));
        assert!(!keep_listed(&w, 10), "this process's own window");
        w.class = "pocketdesk-desktop".into();
        assert!(!keep_listed(&w, 99), "the app's window");
        w.class = "notepad".into();
        w.size = [0, 600];
        assert!(!keep_listed(&w, 99), "no width");
        w.size = [800, 0];
        assert!(!keep_listed(&w, 99), "no height");
        w.size = [800, 600];
        w.hidden = true;
        assert!(!keep_listed(&w, 99), "hidden");
        w.hidden = false;
        w.mapped = false;
        assert!(!keep_listed(&w, 99), "unmapped");
    }

    #[test]
    fn the_outer_size_adds_the_shadow_to_the_picture() {
        // A window whose outer rectangle is 14 px wider and 7 taller than its
        // frame (a shadow at the sides and the bottom).
        assert_eq!(
            outer_for_picture((800, 600), (1214, 807), (1200, 800)),
            (814, 607)
        );
        // A window with no shadow, or one reported with a frame past its
        // outer rectangle: nothing is added, never taken away.
        assert_eq!(
            outer_for_picture((800, 600), (1200, 800), (1200, 800)),
            (800, 600)
        );
        assert_eq!(
            outer_for_picture((800, 600), (1190, 790), (1200, 800)),
            (800, 600)
        );
        // A side is never less than a pixel, and a huge one does not wrap.
        assert_eq!(outer_for_picture((0, 0), (10, 10), (10, 10)), (1, 1));
        assert_eq!(
            outer_for_picture((u32::MAX, 5), (10, 10), (10, 10)),
            (i32::MAX, 5)
        );
    }

    #[test]
    fn a_pixel_off_is_no_resize() {
        assert!(!needs_resize((800, 600), (800, 600)));
        assert!(!needs_resize((801, 599), (800, 600)));
        assert!(needs_resize((802, 600), (800, 600)));
        assert!(needs_resize((800, 598), (800, 600)));
        assert!(needs_resize((0, 0), (800, 600)));
    }

    #[test]
    fn a_window_is_put_back_when_its_show_or_its_place_changed() {
        let before = placement(Show::Normal, [10, 10, 810, 610]);
        assert!(!needs_restore(&before, &before));
        // The rest of the placement (where it would go minimised, flags) is
        // not what the owner sees.
        let mut same = before;
        same.min_at = [5, 5];
        same.flags = 1;
        assert!(!needs_restore(&before, &same));
        assert!(needs_restore(
            &before,
            &placement(Show::Maximized, before.normal)
        ));
        assert!(needs_restore(
            &before,
            &placement(Show::Minimized, before.normal)
        ));
        assert!(needs_restore(
            &before,
            &placement(Show::Normal, [10, 10, 900, 610])
        ));
        assert!(needs_restore(
            &before,
            &placement(Show::Normal, [20, 10, 820, 610])
        ));
    }

    #[test]
    fn show_commands_read_as_windows_means_them() {
        for (cmd, show) in [
            (1, Show::Normal),
            (4, Show::Normal),
            (5, Show::Normal),
            (9, Show::Normal),
            (0, Show::Normal),
            (3, Show::Maximized),
            (2, Show::Minimized),
            (6, Show::Minimized),
            (7, Show::Minimized),
            (11, Show::Minimized),
        ] {
            assert_eq!(Show::from_cmd(cmd), show, "{cmd}");
        }
        for show in [Show::Normal, Show::Maximized, Show::Minimized] {
            assert_eq!(Show::from_cmd(show.to_cmd()), show);
        }
    }

    // ---- The flow, over the fake desktop ----

    fn setup(hwnd: isize) -> (Lends, WinEnv<Fake>) {
        let fake = Fake::new(&[(hwnd, win(100, 80, 800, 600))]);
        (Lends::new(), WinEnv::new(fake))
    }

    fn index_of(reply: &Value) -> u32 {
        let index = reply["index"].as_u64().expect("an index") as u32;
        assert!(window_target::is_window_index(index), "{reply}");
        index
    }

    #[test]
    fn a_window_is_lent_where_it_stands_and_the_answer_names_its_capture_index() {
        let _turn = serial();
        let hwnd = 0x8101;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let reply = lend_with(&lends, &env, DEVICE, &id);
        assert!(reply.get("error").is_none(), "{reply}");
        let index = index_of(&reply);
        assert_eq!(reply["window"], id.as_str());
        assert_eq!(reply["width"], 800);
        assert_eq!(reply["height"], 600);
        assert_eq!(reply["name"], output_name(index).as_str());
        // The session that dials that index finds the window by it.
        assert_eq!(lent_hwnd(index), Some(hwnd));
        // Nothing moved or was sized: the window is lent as it stands.
        assert!(env.desk.log().is_empty(), "{:?}", env.desk.log());
        assert_eq!(env.desk.win(hwnd).frame, [100, 80, 800, 600]);
        // Back: the index is free again, and still nothing was touched.
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1, "{back}");
        assert_eq!(lent_hwnd(index), None);
        assert!(env.desk.log().is_empty());
    }

    #[test]
    fn asked_again_the_same_screen_comes_back() {
        let _turn = serial();
        let hwnd = 0x8102;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let a = lend_with(&lends, &env, DEVICE, &id);
        let b = lend_with(&lends, &env, DEVICE, &id);
        assert_eq!(index_of(&a), index_of(&b));
        assert_eq!(b["width"], 800);
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn another_computer_cannot_take_a_lent_window() {
        let _turn = serial();
        let hwnd = 0x8103;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        assert!(lend_with(&lends, &env, DEVICE, &id).get("error").is_none());
        let refused = lend_with(&lends, &env, OTHER, &id);
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("Another computer")
        );
        // And it cannot return what it did not borrow.
        let none = return_with(&lends, &env, OTHER, Some(&id));
        assert_eq!(none["returned"], 0);
        assert_eq!(
            lent_hwnd(index_of(&lend_with(&lends, &env, DEVICE, &id))),
            Some(hwnd)
        );
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_window_that_is_not_there_is_not_lent_and_leaves_nothing() {
        let _turn = serial();
        let (lends, env) = setup(0x8104);
        let reply = lend_with(&lends, &env, DEVICE, &window_id(0x8199));
        assert!(
            reply["error"].as_str().unwrap().contains("No such window"),
            "{reply}"
        );
        assert!(env.state().screens.is_empty());
        // An id that is no handle never reaches the desktop.
        let bad = lend_with(&lends, &env, DEVICE, "5");
        assert!(bad.get("error").is_some());
    }

    #[test]
    fn a_lend_with_no_capture_says_why() {
        let _turn = serial();
        let mut fake = Fake::new(&[(0x8105, win(0, 0, 800, 600))]);
        fake.capture = false;
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let reply = lend_with(&lends, &env, DEVICE, &window_id(0x8105));
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("Windows.Graphics.Capture")
        );
        let listed = list_with(&lends, &env, DEVICE);
        assert!(listed.get("error").is_some());
    }

    #[test]
    fn a_minimised_window_is_restored_and_goes_back_minimised() {
        let _turn = serial();
        let hwnd = 0x8106;
        let mut fake = Fake::new(&[(hwnd, win(100, 80, 800, 600))]);
        // Minimised after the list was made and before the lend.
        fake.lists_minimised = true;
        let (lends, env) = (Lends::new(), WinEnv::new(fake));
        env.desk.with(hwnd, |w| {
            w.placement = placement(Show::Minimized, [100, 80, 900, 680]);
        });
        let id = window_id(hwnd);
        let reply = lend_with(&lends, &env, DEVICE, &id);
        assert!(reply.get("error").is_none(), "{reply}");
        // Restored so it sends frames, at the size it has when shown.
        assert_eq!(env.desk.win(hwnd).placement.show, Show::Normal);
        assert_eq!(reply["width"], 800);
        // The owner minimises it again meanwhile: the borrower's next ask
        // brings it back.
        env.desk.with(hwnd, |w| w.placement.show = Show::Minimized);
        let again = lend_with(&lends, &env, DEVICE, &id);
        assert!(again.get("error").is_none());
        assert_eq!(env.desk.win(hwnd).placement.show, Show::Normal);
        // Returned: as it was, minimised.
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1);
        assert_eq!(env.desk.win(hwnd).placement.show, Show::Minimized);
    }

    #[test]
    fn a_window_sized_to_the_borrower_is_put_back_at_its_own_size_and_place() {
        let _turn = serial();
        let hwnd = 0x8107;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let first = lend_with(&lends, &env, DEVICE, &id);
        let index = index_of(&first);
        // The borrower's window is another size: the window follows, and the
        // answer is the size it came to.
        let size = Some((1000u32, 700u32));
        let r = crate::lend_window::lend_sized(&lends, &env, DEVICE, &id, size);
        assert!(r.get("error").is_none(), "{r}");
        assert_eq!(index_of(&r), index, "the same screen, resized in place");
        assert_eq!(
            (r["width"].as_u64(), r["height"].as_u64()),
            (Some(1000), Some(700))
        );
        assert_eq!(env.desk.win(hwnd).frame[2], 1000);
        assert_eq!(env.desk.win(hwnd).frame[3], 700);
        // A window with a least size answers the size it really is.
        let tiny = crate::lend_window::lend_sized(&lends, &env, DEVICE, &id, Some((320, 320)));
        assert_eq!(
            (tiny["width"].as_u64(), tiny["height"].as_u64()),
            (Some(320), Some(320))
        );
        env.desk.with(hwnd, |w| w.least = (500, 400));
        let held = crate::lend_window::lend_sized(&lends, &env, DEVICE, &id, Some((400, 400)));
        assert!(held.get("error").is_none(), "{held}");
        assert_eq!(
            (held["width"].as_u64(), held["height"].as_u64()),
            (Some(500), Some(400)),
            "the real size, not the asked one"
        );
        // The same size again changes nothing.
        let before = env.desk.log().len();
        crate::lend_window::lend_sized(&lends, &env, DEVICE, &id, Some((500, 400)));
        assert_eq!(env.desk.log().len(), before);
        // Return: its own size and place again.
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1, "{back}");
        assert_eq!(env.desk.win(hwnd).frame, [100, 80, 800, 600]);
        assert_eq!(env.desk.win(hwnd).placement.normal, [100, 80, 900, 680]);
    }

    #[test]
    fn a_maximised_window_is_lent_as_it_is_and_goes_back_maximised() {
        let _turn = serial();
        let hwnd = 0x8108;
        let (lends, env) = setup(hwnd);
        env.desk.with(hwnd, |w| {
            w.placement = placement(Show::Maximized, [100, 80, 900, 680]);
        });
        let id = window_id(hwnd);
        // Lent at the maximised size: nothing is changed.
        let r = lend_with(&lends, &env, DEVICE, &id);
        assert!(r.get("error").is_none(), "{r}");
        assert_eq!(env.desk.win(hwnd).placement.show, Show::Maximized);
        // Sizing it needs a window shown normally; Windows will not size a
        // maximised one, so the fake refuses, and the lend carries on at the
        // size it has (a resize is cosmetic).
        let sized = crate::lend_window::lend_sized(&lends, &env, DEVICE, &id, Some((640, 480)));
        assert!(sized.get("error").is_none(), "{sized}");
        assert_eq!(env.desk.win(hwnd).frame[2], 800);
        assert_eq!(sized["width"], 800, "the answer is the size it has");
        assert!(
            env.desk.log().iter().all(|l| !l.starts_with("size ")),
            "a maximised window is not asked to take a size: {:?}",
            env.desk.log()
        );
        return_with(&lends, &env, DEVICE, None);
        assert_eq!(env.desk.win(hwnd).placement.show, Show::Maximized);
    }

    #[test]
    fn a_window_the_owner_moved_goes_back_to_where_it_was() {
        let _turn = serial();
        let hwnd = 0x8109;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        assert!(lend_with(&lends, &env, DEVICE, &id).get("error").is_none());
        env.desk.with(hwnd, |w| {
            w.placement.normal = [400, 300, 1200, 900];
            w.frame = [400, 300, 800, 600];
        });
        return_with(&lends, &env, DEVICE, None);
        assert_eq!(env.desk.win(hwnd).placement.normal, [100, 80, 900, 680]);
        assert!(env.desk.log().iter().any(|l| l.starts_with("set ")));
    }

    #[test]
    fn a_window_left_as_it_was_is_not_touched_on_return() {
        let _turn = serial();
        let hwnd = 0x810a;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        lend_with(&lends, &env, DEVICE, &id);
        return_with(&lends, &env, DEVICE, Some(&id));
        assert!(env.desk.log().is_empty(), "{:?}", env.desk.log());
    }

    #[test]
    fn a_window_that_will_not_go_back_stays_lent_for_another_try() {
        let _turn = serial();
        let hwnd = 0x810b;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        env.desk
            .with(hwnd, |w| w.placement.normal = [0, 0, 500, 500]);
        env.desk.stuck.lock().unwrap().push(hwnd);
        let failed = return_with(&lends, &env, DEVICE, Some(&id));
        assert!(failed.get("error").is_some(), "{failed}");
        assert_eq!(failed["returned"], 0);
        assert_eq!(
            lent_hwnd(index),
            None,
            "the grant ended with the return, whether or not the window went back"
        );
        assert!(
            env.state()
                .screens
                .contains_key(&crate::lend_window::lend_key(DEVICE, &id)),
            "it stays lent, for the next try"
        );
        // Free again: the next try puts it back.
        env.desk.stuck.lock().unwrap().clear();
        let ok = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(ok["returned"], 1, "{ok}");
        assert_eq!(env.desk.win(hwnd).placement.normal, [100, 80, 900, 680]);
        assert_eq!(lent_hwnd(index), None);
    }

    #[test]
    fn a_window_closed_while_lent_is_found_closed_and_its_index_freed() {
        let _turn = serial();
        let hwnd = 0x810c;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        env.desk.close(hwnd);
        // The window's capture sees it gone; the output is gone too.
        assert!(env.output(&output_name(index)).is_none());
        // A return of a window that is not there is not an error.
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1, "{back}");
        assert_eq!(lent_hwnd(index), None);
    }

    #[test]
    fn the_sweep_ends_a_closed_window_after_two_looks_and_keeps_a_shown_one() {
        let _turn = serial();
        let (shown, closing) = (0x810d, 0x810e);
        let fake = Fake::new(&[
            (shown, win(0, 0, 800, 600)),
            (closing, win(50, 50, 640, 480)),
        ]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let (a, b) = (window_id(shown), window_id(closing));
        let ia = index_of(&lend_with(&lends, &env, DEVICE, &a));
        let ib = index_of(&lend_with(&lends, &env, DEVICE, &b));
        let live = vec![DEVICE.to_string()];
        let captures = vec![(DEVICE.to_string(), ia), (DEVICE.to_string(), ib)];
        let present = Present {
            live: &live,
            captures: &captures,
            linked: &|_| true,
        };
        let mut looks = Looks::default();
        let now = Instant::now();
        env.desk.close(closing);
        let first = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now);
        assert!(first.is_empty(), "one look is not enough");
        let second = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].window, b);
        assert_eq!(second[0].why, Gone::Closed);
        assert_eq!(lent_hwnd(ib), None);
        assert_eq!(lent_hwnd(ia), Some(shown), "the shown window stays lent");
        // The switch turned off gives the rest back.
        let off = crate::lend_window::sweep_with(&lends, &env, false, &present, &mut looks, now);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].why, Gone::SwitchOff);
        assert_eq!(lent_hwnd(ia), None);
    }

    #[test]
    fn a_borrower_that_went_away_gets_its_window_back_at_the_next_look() {
        let _turn = serial();
        let hwnd = 0x810f;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        env.desk
            .with(hwnd, |w| w.placement.normal = [0, 0, 300, 300]);
        let present = Present {
            live: &[],
            captures: &[],
            linked: &|_| false,
        };
        let mut looks = Looks::default();
        // A window lent a moment ago is left alone for a few seconds; later,
        // with nobody there, it goes back.
        let later = Instant::now() + std::time::Duration::from_secs(10);
        let ended = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, later);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].why, Gone::BorrowerAway);
        assert_eq!(env.desk.win(hwnd).placement.normal, [100, 80, 900, 680]);
        assert_eq!(lent_hwnd(index), None);
    }

    #[test]
    fn the_list_names_other_programs_windows_only() {
        let _turn = serial();
        let (mine, theirs) = (0x8110, 0x8111);
        let mut own = win(0, 0, 800, 600);
        own.class = "pocketdesk-desktop";
        own.title = "Pocketdesk";
        let fake = Fake::new(&[(mine, own), (theirs, win(10, 10, 640, 480))]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let listed = list_with(&lends, &env, DEVICE);
        let windows = listed["windows"].as_array().expect("a list");
        assert_eq!(windows.len(), 1, "{listed}");
        assert_eq!(first(&listed)["address"], window_id(theirs).as_str());
        assert_eq!(first(&listed)["title"], "Notes");
        assert_eq!(first(&listed)["width"], 640);
        assert_eq!(first(&listed)["lent"], false);
        // Once lent, it says so to its borrower, and is gone from the list of
        // another computer.
        lend_with(&lends, &env, DEVICE, &window_id(theirs));
        assert_eq!(first(&list_with(&lends, &env, DEVICE))["lent"], true);
        let other = list_with(&lends, &env, OTHER);
        assert!(other["windows"].as_array().unwrap().is_empty(), "{other}");
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_window_smaller_than_the_smallest_screen_is_grown_to_it_and_goes_back() {
        let _turn = serial();
        let hwnd = 0x8112;
        let fake = Fake::new(&[(hwnd, win(30, 40, 10, 10))]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let id = window_id(hwnd);
        // The same rule as for a screen anywhere (320 at least a side): it is
        // listed, and lent at that size.
        assert_eq!(
            list_with(&lends, &env, DEVICE)["windows"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let reply = lend_with(&lends, &env, DEVICE, &id);
        assert!(reply.get("error").is_none(), "{reply}");
        assert_eq!(
            (reply["width"].as_u64(), reply["height"].as_u64()),
            (Some(320), Some(320))
        );
        return_with(&lends, &env, DEVICE, None);
        assert_eq!(env.desk.win(hwnd).frame, [30, 40, 10, 10]);
    }

    #[test]
    fn nothing_is_written_for_the_next_start() {
        let (_, env) = setup(0x8114);
        assert!(env.save(&[]).is_ok());
    }

    #[test]
    fn more_windows_than_there_are_indexes_are_refused_and_leave_the_rest_alone() {
        let _turn = serial();
        let n = window_target::MOST as isize + 3;
        let wins: Vec<(isize, Win)> = (0..n).map(|i| (0x8200 + i, win(0, 0, 640, 480))).collect();
        let env = WinEnv::new(Fake::new(&wins));
        let lends = Lends::new();
        let mut refused = 0;
        let mut taken = Vec::new();
        for (hwnd, _) in &wins {
            let r = lend_with(&lends, &env, DEVICE, &window_id(*hwnd));
            match r.get("error") {
                Some(_) => refused += 1,
                None => taken.push(*hwnd),
            }
        }
        assert_eq!(refused, 3, "all but the table's eight are refused");
        assert_eq!(taken.len(), window_target::MOST as usize);
        // A refused window was not touched, and is not recorded as lent.
        for (hwnd, _) in &wins {
            if !taken.contains(hwnd) {
                assert!(!env.state().placed.contains_key(hwnd));
            }
        }
        return_with(&lends, &env, DEVICE, None);
    }

    // ---- What a lend grants, and to whom ----

    #[test]
    fn the_shells_own_windows_and_programs_beyond_us_are_not_lendable() {
        for class in [
            "Progman",
            "progman",
            "WorkerW",
            "Shell_TrayWnd",
            "Shell_SecondaryTrayWnd",
            "Windows.UI.Core.CoreWindow",
        ] {
            assert!(is_shell_class(class), "{class}");
            let kind = Kind {
                class: class.into(),
                beyond_us: false,
            };
            assert!(!lendable_kind(&kind), "{class}");
        }
        for class in ["Notepad", "CabinetWClass", "Chrome_WidgetWin_1", "", "Prog"] {
            assert!(!is_shell_class(class), "{class}");
            let kind = Kind {
                class: class.into(),
                beyond_us: false,
            };
            assert!(lendable_kind(&kind), "{class}");
        }
        let elevated = Kind {
            class: "Notepad".into(),
            beyond_us: true,
        };
        assert!(!lendable_kind(&elevated));
    }

    #[test]
    fn only_a_window_that_is_shown_normally_is_asked_to_take_a_size() {
        let facts = |minimized, maximized| Facts {
            frame: [0, 0, 800, 600],
            minimized,
            maximized,
            identity: Identity { pid: 1, tid: 2 },
        };
        assert!(can_size(&facts(false, false)));
        assert!(!can_size(&facts(true, false)));
        assert!(!can_size(&facts(false, true)));
    }

    #[test]
    fn a_window_that_grew_is_moved_back_onto_its_screen() {
        let work = [0, 0, 1920, 1040];
        // Inside: left where it is.
        assert_eq!(keep_on_screen(100, 80, 800, 600, work), (100, 80));
        // Overhanging right and bottom: back by what it overhangs.
        assert_eq!(keep_on_screen(1500, 700, 800, 600, work), (1120, 440));
        // Overhanging left and top (a screen left of or above this one).
        assert_eq!(keep_on_screen(-50, -30, 800, 600, work), (0, 0));
        // Larger than the work area: its start shows.
        assert_eq!(keep_on_screen(300, 200, 2400, 1300, work), (0, 0));
        // Exactly the work area.
        assert_eq!(keep_on_screen(0, 0, 1920, 1040, work), (0, 0));
        // A work area that does not start at the origin (a second screen).
        let second = [1920, 0, 3840, 1040];
        assert_eq!(keep_on_screen(3500, 900, 800, 600, second), (3040, 440));
        assert_eq!(keep_on_screen(1000, 10, 800, 600, second), (1920, 10));
        // Sizes that would overflow a plain sum are held.
        assert_eq!(
            keep_on_screen(i32::MAX - 5, i32::MIN + 5, i32::MAX, 10, work),
            (0, 0)
        );
    }

    #[test]
    fn only_a_listed_window_is_lent_whatever_handle_is_named() {
        let _turn = serial();
        let (fine, desktop, elevated, own, minimised, taskbar) =
            (0x8301, 0x8302, 0x8303, 0x8304, 0x8305, 0x8306);
        let mut shell = win(0, 0, 1920, 1080);
        shell.win_class = "Progman";
        shell.title = "Program Manager";
        shell.class = "explorer";
        let mut bar = win(0, 1040, 1920, 40);
        bar.win_class = "Shell_TrayWnd";
        bar.class = "explorer";
        let mut high = win(10, 10, 640, 480);
        high.beyond_us = true;
        high.identity = Identity { pid: 4, tid: 5 };
        let mut ours = win(10, 10, 640, 480);
        ours.pid = std::process::id();
        let mut away = win(10, 10, 640, 480);
        away.placement = placement(Show::Minimized, [10, 10, 650, 490]);
        let fake = Fake::new(&[
            (fine, win(10, 10, 640, 480)),
            (desktop, shell),
            (elevated, high),
            (own, ours),
            (minimised, away),
            (taskbar, bar),
        ]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        // The list shows the one window.
        let listed = list_with(&lends, &env, DEVICE);
        assert_eq!(listed["windows"].as_array().unwrap().len(), 1, "{listed}");
        assert_eq!(first(&listed)["address"], window_id(fine).as_str());
        // Any other handle is refused in the same words as a window that is
        // not there, and nothing is touched or recorded.
        for hwnd in [desktop, elevated, own, minimised, taskbar, 0x8399] {
            let reply = lend_with(&lends, &env, DEVICE, &window_id(hwnd));
            assert!(
                reply["error"].as_str().unwrap().contains("No such window"),
                "{hwnd:#x}: {reply}"
            );
        }
        assert!(env.state().screens.is_empty());
        assert!(env.state().placed.is_empty());
        assert!(env.desk.log().is_empty());
        let ok = lend_with(&lends, &env, DEVICE, &window_id(fine));
        assert!(ok.get("error").is_none(), "{ok}");
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_window_is_lent_to_the_device_that_asked_and_to_no_other() {
        let _turn = serial();
        let hwnd = 0x8310;
        let (lends, env) = setup(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &window_id(hwnd)));
        let lease = window_target::lease_for(index, Some(DEVICE)).expect("its borrower holds it");
        assert_eq!(lease.hwnd, hwnd);
        assert_eq!(lease.device, DEVICE);
        assert_eq!(
            window_target::lease_for(index, Some(OTHER)).unwrap_err(),
            window_target::Refusal::OtherDevice,
            "another paired device cannot dial the index"
        );
        assert_eq!(
            window_target::lease_for(index, None).unwrap_err(),
            window_target::Refusal::OtherDevice
        );
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn giving_a_window_back_ends_the_grant_and_the_index_is_not_given_out_again_at_once() {
        let _turn = serial();
        let hwnd = 0x8320;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        let lease = window_target::lease_for(index, Some(DEVICE)).unwrap();
        assert!(window_target::lease_holds(&lease));
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1);
        assert!(
            !window_target::lease_holds(&lease),
            "a session bound to the lend is over"
        );
        // Another window lent next is not under the index just freed, and the
        // old lease holds nothing of it either.
        let other = 0x8321;
        let fake = Fake::new(&[(hwnd, win(100, 80, 800, 600)), (other, win(0, 0, 640, 480))]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let again = index_of(&lend_with(&lends, &env, DEVICE, &window_id(other)));
        assert_ne!(again, index, "not the index freed a moment ago");
        assert!(!window_target::lease_holds(&lease));
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_swept_lend_and_a_dropped_screen_end_their_grants_too() {
        let _turn = serial();
        let (a, b) = (0x8330, 0x8331);
        let fake = Fake::new(&[(a, win(0, 0, 800, 600)), (b, win(50, 50, 640, 480))]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let ia = index_of(&lend_with(&lends, &env, DEVICE, &window_id(a)));
        let ib = index_of(&lend_with(&lends, &env, DEVICE, &window_id(b)));
        let lease_a = window_target::lease_for(ia, Some(DEVICE)).unwrap();
        let lease_b = window_target::lease_for(ib, Some(DEVICE)).unwrap();
        // A borrower that went away: swept back.
        let present = Present {
            live: &[],
            captures: &[],
            linked: &|_| false,
        };
        let mut looks = Looks::default();
        let later = Instant::now() + std::time::Duration::from_secs(10);
        let ended = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, later);
        assert_eq!(ended.len(), 2);
        assert!(!window_target::lease_holds(&lease_a));
        assert!(!window_target::lease_holds(&lease_b));
        // A screen dropped on its own (a lend that failed after the window
        // was registered).
        let c = 0x8332;
        let fake = Fake::new(&[(c, win(0, 0, 800, 600))]);
        let env = WinEnv::new(fake);
        let lends = Lends::new();
        let ic = index_of(&lend_with(&lends, &env, DEVICE, &window_id(c)));
        let lease_c = window_target::lease_for(ic, Some(DEVICE)).unwrap();
        env.drop_screen(&crate::lend_window::lend_key(DEVICE, &window_id(c)));
        assert!(!window_target::lease_holds(&lease_c));
    }

    #[test]
    fn a_lend_asked_for_again_after_a_return_that_failed_is_a_new_grant_for_the_same_screen() {
        let _turn = serial();
        let hwnd = 0x8340;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let first = lend_with(&lends, &env, DEVICE, &id);
        let index = index_of(&first);
        let old = window_target::lease_for(index, Some(DEVICE)).unwrap();
        env.desk
            .with(hwnd, |w| w.placement.normal = [0, 0, 500, 500]);
        env.desk.stuck.lock().unwrap().push(hwnd);
        assert!(
            return_with(&lends, &env, DEVICE, Some(&id))
                .get("error")
                .is_some()
        );
        assert!(!window_target::lease_holds(&old), "ended by the return");
        // The borrower dials again and asks again: the window is still lent
        // to it, under a grant of its own and the same name.
        let again = lend_with(&lends, &env, DEVICE, &id);
        assert!(again.get("error").is_none(), "{again}");
        assert_eq!(index_of(&again), index, "the screen keeps its name");
        let new = window_target::lease_for(index, Some(DEVICE)).unwrap();
        assert_ne!(new.generation, old.generation);
        assert!(
            !window_target::lease_holds(&old),
            "the old session stays over"
        );
        env.desk.stuck.lock().unwrap().clear();
        assert_eq!(return_with(&lends, &env, DEVICE, Some(&id))["returned"], 1);
        assert!(!window_target::lease_holds(&new));
    }

    #[test]
    fn a_handle_that_names_another_window_is_never_touched() {
        let _turn = serial();
        let hwnd = 0x8350;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        let lease = window_target::lease_for(index, Some(DEVICE)).unwrap();
        // The window is moved by its owner (so putting it back would write),
        // closed, and Windows gives the handle to another program's window.
        env.desk
            .with(hwnd, |w| w.placement.normal = [400, 300, 1200, 900]);
        env.desk.recycle(hwnd, 77);
        // It is not the lent window: gone to every look.
        assert!(env.seen(&id).unwrap().is_none());
        assert!(env.output(&output_name(index)).is_none());
        let sized = env.resize_screen(&crate::lend_window::lend_key(DEVICE, &id), 640, 480);
        assert!(sized.get("error").is_some(), "{sized}");
        // Returned: counted as put back, and the new window is left exactly as
        // it is (nothing set, nothing restored, nothing sized).
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1, "{back}");
        assert!(env.desk.log().is_empty(), "{:?}", env.desk.log());
        assert_eq!(env.desk.win(hwnd).placement.normal, [400, 300, 1200, 900]);
        assert!(!window_target::lease_holds(&lease));
        // And nothing of the lend is left to be put back on the next return.
        assert!(env.state().placed.is_empty());
    }

    #[test]
    fn the_sweep_ends_a_lend_whose_handle_was_given_to_another_window() {
        let _turn = serial();
        let hwnd = 0x8351;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        let index = index_of(&lend_with(&lends, &env, DEVICE, &id));
        let live = vec![DEVICE.to_string()];
        let captures = vec![(DEVICE.to_string(), index)];
        let present = Present {
            live: &live,
            captures: &captures,
            linked: &|_| true,
        };
        let mut looks = Looks::default();
        let now = Instant::now();
        env.desk.recycle(hwnd, 88);
        assert!(
            crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now)
                .is_empty()
        );
        let ended = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].why, Gone::Closed);
        assert_eq!(lent_hwnd(index), None);
        assert!(env.desk.log().is_empty(), "{:?}", env.desk.log());
    }

    #[test]
    fn a_lend_that_fails_undoes_only_what_it_made() {
        let _turn = serial();
        // Nothing was registered before: a window whose place cannot be read
        // is refused and leaves no registration.
        let hwnd = 0x8360;
        let (lends, env) = setup(hwnd);
        env.desk.unreadable.lock().unwrap().push(hwnd);
        let id = window_id(hwnd);
        let refused = lend_with(&lends, &env, DEVICE, &id);
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("where the window is")
        );
        assert!(env.state().screens.is_empty());
        assert!(env.state().placed.is_empty());
        let probe = window_target::register(hwnd, DEVICE, Identity { pid: 4242, tid: 7 }, None);
        let (probe_lease, made) = probe.expect("nothing was left registered");
        assert!(made, "so a registration made now is new");
        window_target::unregister(&probe_lease);

        // The same window was registered before this call (a grant to this
        // device that this environment does not know): the failed lend does
        // not remove it.
        let before = window_target::register(hwnd, DEVICE, Identity { pid: 4242, tid: 7 }, None)
            .expect("registered")
            .0;
        let refused = lend_with(&lends, &env, DEVICE, &id);
        assert!(refused.get("error").is_some());
        assert!(
            window_target::lease_holds(&before),
            "a registration this call did not make is left alone"
        );
        window_target::unregister(&before);
    }

    #[test]
    fn a_window_lent_to_one_device_cannot_be_registered_for_another_by_a_side_door() {
        let _turn = serial();
        let hwnd = 0x8370;
        let (lends, env) = setup(hwnd);
        let id = window_id(hwnd);
        lend_with(&lends, &env, DEVICE, &id);
        // The environment asked directly for the same window under another
        // device's key (the lend path refuses first; this is the second wall).
        let reply = env.add_screen(&crate::lend_window::lend_key(OTHER, &id), 800, 600);
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("Another computer"),
            "{reply}"
        );
        assert!(
            !env.state()
                .screens
                .contains_key(&crate::lend_window::lend_key(OTHER, &id))
        );
        return_with(&lends, &env, DEVICE, None);
    }
}
