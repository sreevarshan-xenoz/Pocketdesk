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

use pocketdesk_core::streaming::window_target;
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
    fn placement(&self, hwnd: isize) -> io::Result<Placement>;
    fn set_placement(&self, hwnd: isize, placement: &Placement) -> io::Result<()>;
    /// Out of the taskbar or maximised, to the place it has when shown
    /// normally.
    fn restore(&self, hwnd: isize) -> io::Result<()>;
    /// Size the window so that its frame (what [`Facts::frame`] says) is
    /// `width`×`height`, where it is. A window may not take every size.
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

/// The window of the key a lend is recorded under (`<device>#lend-<window>`,
/// [`super::lend_key`]).
fn window_of_key(key: &str) -> Option<&str> {
    key.rsplit_once("#lend-").map(|(_, window)| window)
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

// ---- The environment ----

/// A window lent: its handle and the index a session captures it by.
#[derive(Debug, Clone, Copy)]
struct Screen {
    hwnd: isize,
    index: u32,
}

#[derive(Default)]
struct State {
    /// The lends, by the key the lend path uses.
    screens: BTreeMap<String, Screen>,
    /// Where each lent window was, until it is put back.
    placed: BTreeMap<isize, Placement>,
}

/// Lending as Windows does it: [`Env`] over a [`Desktop`].
pub struct WinEnv<D> {
    desk: D,
    state: Mutex<State>,
}

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

    /// The answer for a window lent: its screen's name and index and the size
    /// its picture has now.
    fn reply(&self, screen: Screen) -> Value {
        match self.desk.facts(screen.hwnd) {
            Some(facts) => json!({
                "name": output_name(screen.index),
                "index": screen.index,
                "width": facts.frame[2].max(0),
                "height": facts.frame[3].max(0),
            }),
            None => error("The window closed"),
        }
    }

    /// A minimised window sends no frames: restore one that is, as often as
    /// it is asked (a view that dials again finds it as it was left).
    fn unminimise(&self, hwnd: isize) {
        if self.desk.facts(hwnd).is_some_and(|f| f.minimized)
            && let Err(e) = self.desk.restore(hwnd)
        {
            tracing::warn!("A lent window could not be brought back from the taskbar: {e}");
        }
    }

    /// Size the window's picture to `width`×`height` if it is not already.
    fn size_to(&self, hwnd: isize, width: u32, height: u32) -> io::Result<()> {
        let Some(facts) = self.desk.facts(hwnd) else {
            return Err(io::Error::other("the window closed"));
        };
        if needs_resize((facts.frame[2], facts.frame[3]), (width, height)) {
            self.desk.size_picture(hwnd, width, height)?;
        }
        Ok(())
    }

    /// Put the window `hwnd` back as `before` says, if it is not as it was.
    fn put_back_as(&self, hwnd: isize, before: &Placement) -> io::Result<()> {
        if self.desk.facts(hwnd).is_none() {
            return Ok(());
        }
        let now = self.desk.placement(hwnd)?;
        if needs_restore(before, &now) {
            self.desk.set_placement(hwnd, before)?;
        }
        Ok(())
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
        Ok(self.desk.facts(hwnd).map(|facts| Seen {
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
            .collect())
    }

    fn add_screen(&self, key: &str, width: u32, height: u32) -> Value {
        let Some(hwnd) = window_of_key(key).and_then(parse_hwnd) else {
            return error("not a window");
        };
        let mut state = self.state();
        if let Some(screen) = state.screens.get(key).copied() {
            // Asked again: the same window, shown if it was minimised since.
            self.unminimise(screen.hwnd);
            return self.reply(screen);
        }
        let Some(facts) = self.desk.facts(hwnd) else {
            return error("No such window on this computer (it may have closed)");
        };
        let Some(index) = window_target::register(hwnd) else {
            return error("Too many windows are lent at once");
        };
        let before = match self.desk.placement(hwnd) {
            Ok(placement) => placement,
            Err(e) => {
                window_target::unregister(index);
                return error(format!("Could not read where the window is: {e}"));
            }
        };
        if facts.minimized
            && let Err(e) = self.desk.restore(hwnd)
        {
            window_target::unregister(index);
            return error(format!(
                "Could not bring the window back from the taskbar: {e}"
            ));
        }
        let screen = Screen { hwnd, index };
        state.placed.insert(hwnd, before);
        state.screens.insert(key.to_string(), screen);
        // A window that will not take the size is lent at the size it has: the
        // answer says which.
        if let Err(e) = self.size_to(hwnd, width, height) {
            tracing::warn!("A lent window was not sized to the borrower's window: {e}");
        }
        self.reply(screen)
    }

    fn drop_screen(&self, key: &str) {
        let mut state = self.state();
        let Some(screen) = state.screens.remove(key) else {
            return;
        };
        window_target::unregister(screen.index);
        // A lend that failed after the window was touched: back as it was.
        if let Some(before) = state.placed.remove(&screen.hwnd)
            && let Err(e) = self.put_back_as(screen.hwnd, &before)
        {
            tracing::warn!("A window that was not lent after all could not be put back: {e}");
        }
    }

    fn resize_screen(&self, key: &str, width: u32, height: u32) -> Value {
        let Some(screen) = self.state().screens.get(key).copied() else {
            return error("That window is not lent");
        };
        self.unminimise(screen.hwnd);
        if let Err(e) = self.size_to(screen.hwnd, width, height) {
            return error(format!("The window was not sized: {e}"));
        }
        self.reply(screen)
    }

    fn output(&self, name: &str) -> Option<Output> {
        let screen = self
            .state()
            .screens
            .values()
            .find(|s| output_name(s.index) == name)
            .copied()?;
        let facts = self.desk.facts(screen.hwnd)?;
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
        let Some(before) = state.placed.get(&hwnd).copied() else {
            return Ok(());
        };
        // A window that will not go back keeps its place in the table, so a
        // later try can.
        self.put_back_as(hwnd, &before)?;
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
    }

    struct Fake {
        wins: Mutex<BTreeMap<isize, Win>>,
        capture: bool,
        /// Every call that changed a window, in order.
        log: Mutex<Vec<String>>,
        /// A window that refuses to go back.
        stuck: Mutex<Vec<isize>>,
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
        }
    }

    impl Fake {
        fn new(wins: &[(isize, Win)]) -> Self {
            Self {
                wins: Mutex::new(wins.iter().cloned().collect()),
                capture: true,
                log: Mutex::new(Vec::new()),
                stuck: Mutex::new(Vec::new()),
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
            })
        }

        fn placement(&self, hwnd: isize) -> io::Result<Placement> {
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
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(|e| e.into_inner())
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
        assert_eq!(window_of_key(&key), Some("0xabc"));
        assert_eq!(window_of_key("no key"), None);
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
        assert_eq!(window_target::hwnd(index), Some(hwnd));
        // Nothing moved or was sized: the window is lent as it stands.
        assert!(env.desk.log().is_empty(), "{:?}", env.desk.log());
        assert_eq!(env.desk.win(hwnd).frame, [100, 80, 800, 600]);
        // Back: the index is free again, and still nothing was touched.
        let back = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(back["returned"], 1, "{back}");
        assert_eq!(window_target::hwnd(index), None);
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
            window_target::hwnd(index_of(&lend_with(&lends, &env, DEVICE, &id))),
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
        let (lends, env) = setup(hwnd);
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
    fn a_maximised_window_is_restored_to_be_sized_and_goes_back_maximised() {
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
        assert_eq!(window_target::hwnd(index), Some(hwnd), "its screen is kept");
        // Free again: the next try puts it back.
        env.desk.stuck.lock().unwrap().clear();
        let ok = return_with(&lends, &env, DEVICE, Some(&id));
        assert_eq!(ok["returned"], 1, "{ok}");
        assert_eq!(env.desk.win(hwnd).placement.normal, [100, 80, 900, 680]);
        assert_eq!(window_target::hwnd(index), None);
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
        assert_eq!(window_target::hwnd(index), None);
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
        assert_eq!(window_target::hwnd(ib), None);
        assert_eq!(
            window_target::hwnd(ia),
            Some(shown),
            "the shown window stays lent"
        );
        // The switch turned off gives the rest back.
        let off = crate::lend_window::sweep_with(&lends, &env, false, &present, &mut looks, now);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].why, Gone::SwitchOff);
        assert_eq!(window_target::hwnd(ia), None);
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
        assert_eq!(window_target::hwnd(index), None);
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
}
