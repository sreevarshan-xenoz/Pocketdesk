//! The pointer lock of the window that shows another computer's screen
//! (PC7), for games, CAD and 3D programs that steer by how far the mouse
//! moved rather than where it is.
//!
//! "Lock the pointer" on the window's bar, or Ctrl+Alt+Shift+M, locks this
//! computer's pointer to the window and hides it; the mouse's raw movement
//! then goes to the other computer as relative moves
//! ([`crate::view_input::WindowInput::set_relative`]), and the window tells
//! its server so (the view relay passes relative moves on only while the
//! window says it holds the pointer, and the shared keyboard and mouse leave
//! the screens' edges alone meanwhile). It is freed by the same chord, by
//! Ctrl+Alt+Shift pressed and let go alone (PC4's release chord), and as
//! the window loses the focus, closes, turns View only or stops
//! controlling.
//!
//! Where the windowing library can lock the pointer in place (Wayland) it
//! does; elsewhere (Windows, X11) it is confined to the window and put back
//! in the picture's middle whenever it strays toward an edge.
//!
//! The decisions ([`PointerLock`], [`ReleaseChord`], [`grab_mode`],
//! [`recentre`], [`lock_chord`]) are pure and tested here; the grab itself
//! is a viewport command no test sends.

use std::time::{Duration, Instant};

use eframe::egui;

/// What the window says over the picture as the pointer locks.
pub const HINT: &str = "Ctrl+Alt+Shift+M frees the pointer";
/// How long it says it.
pub const HINT_FOR: Duration = Duration::from_secs(3);

/// The key of a chord with Ctrl+Alt+Shift held, pressed, held or let go:
/// the key where it sits on the keyboard (PR #570 review I3), as Shift and
/// the layout change what it types (Shift+1 types "!", a key egui names
/// apart), else the key it types.
pub fn chord_key(event: &egui::Event) -> Option<egui::Key> {
    match event {
        egui::Event::Key {
            key,
            physical_key,
            modifiers,
            ..
        } if modifiers.ctrl && modifiers.alt && modifiers.shift => {
            Some(physical_key.unwrap_or(*key))
        }
        _ => None,
    }
}

/// Whether `event` is Ctrl+Alt+Shift and the letter `letter`, pressed,
/// held or let go: the key that types it, or the key where it sits (PR
/// #570 review I3), whichever the layout says.
pub fn chord_is(event: &egui::Event, letter: egui::Key) -> bool {
    matches!(
        event,
        egui::Event::Key { key, physical_key, modifiers, .. }
            if modifiers.ctrl && modifiers.alt && modifiers.shift
                && (*key == letter || *physical_key == Some(letter))
    )
}

/// Whether `event` is Ctrl+Alt+Shift+M pressed: the lock's switch.
pub fn lock_chord(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key {
            pressed: true,
            repeat: false,
            ..
        }
    ) && chord_is(event, egui::Key::M)
}

/// Whether `event` is the lock's switch, pressed, held or let go: it is
/// this window's, not the other computer's.
pub fn is_lock_chord_key(event: &egui::Event) -> bool {
    chord_is(event, egui::Key::M)
}

/// What the lock is decided on, each pass of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Situation {
    /// The window has the focus.
    pub focused: bool,
    /// The window controls the other computer: its Control on, View only
    /// off, connected, the view not ended or reconnecting.
    pub controls: bool,
    /// A picture is shown to lock the pointer over.
    pub picture: bool,
}

/// The lock across the window's passes: whether it is wanted, and whether
/// the window locked the pointer (and since when).
#[derive(Debug, Default)]
pub struct PointerLock {
    wanted: bool,
    locked: Option<Instant>,
}

/// What the window does with the pointer this pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Lock,
    Free,
}

impl PointerLock {
    /// The bar's switch or Ctrl+Alt+Shift+M.
    pub fn toggle(&mut self) {
        self.wanted = !self.wanted;
    }

    /// Ctrl+Alt+Shift alone, or the window closing.
    pub fn free(&mut self) {
        self.wanted = false;
    }

    /// Whether the pointer is locked now.
    pub fn locked(&self) -> bool {
        self.locked.is_some()
    }

    /// Whether the bar's switch shows on: the lock is wanted or held.
    pub fn wanted(&self) -> bool {
        self.wanted || self.locked()
    }

    /// The lock in `now`'s situation: locked as it is wanted and the window
    /// can, freed as it is not wanted or the window can no longer (and then
    /// not wanted any more: a focus that comes back does not lock again).
    pub fn step(&mut self, now: Situation, at: Instant) -> Option<Change> {
        if !(now.focused && now.controls && now.picture) {
            self.wanted = false;
        }
        match (self.wanted, self.locked.is_some()) {
            (true, false) => {
                self.locked = Some(at);
                Some(Change::Lock)
            }
            (false, true) => {
                self.locked = None;
                Some(Change::Free)
            }
            _ => None,
        }
    }

    /// Whether the window says [`HINT`] at `at`: for [`HINT_FOR`] after it
    /// locked.
    pub fn hint(&self, at: Instant) -> bool {
        self.locked
            .is_some_and(|since| at.saturating_duration_since(since) < HINT_FOR)
    }
}

/// Ctrl+Alt+Shift pressed and let go with nothing else (PC4's release
/// chord), as the window's passes see it: the modifiers each key carried
/// and the modifiers at the pass's end.
#[derive(Debug, Default)]
pub struct ReleaseChord {
    /// All three were held at once since the first went down.
    armed: bool,
    /// Another key was pressed since the first went down.
    spoiled: bool,
}

impl ReleaseChord {
    /// This pass's `events`, then `held` (the modifiers now): whether the
    /// chord was let go just now.
    pub fn observe(&mut self, events: &[egui::Event], held: egui::Modifiers) -> bool {
        let mut fired = false;
        for event in events {
            if let egui::Event::Key {
                key,
                physical_key,
                pressed,
                modifiers,
                ..
            } = event
            {
                if *pressed && !is_chord_key(physical_key.unwrap_or(*key)) {
                    self.spoiled = true;
                }
                fired |= self.look(*modifiers);
            }
        }
        fired | self.look(held)
    }

    fn look(&mut self, mods: egui::Modifiers) -> bool {
        if !(mods.ctrl || mods.alt || mods.shift) {
            let fired = self.armed && !self.spoiled;
            *self = ReleaseChord::default();
            return fired;
        }
        if mods.ctrl && mods.alt && mods.shift {
            self.armed = true;
        }
        false
    }
}

/// Ctrl, Alt or Shift, left or right.
fn is_chord_key(key: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        key,
        ShiftLeft | ShiftRight | ControlLeft | ControlRight | AltLeft | AltRight
    )
}

/// How the pointer is held in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabMode {
    /// Held where it is (Wayland's pointer constraints).
    Locked,
    /// Kept inside the window, and put back in the picture's middle as it
    /// strays (Windows and X11, which cannot lock it).
    Confined,
}

/// The grab the windowing library can give on this system: `windows`,
/// Windows; `wayland`, a Wayland session (else X11 on Linux).
pub fn grab_mode(windows: bool, wayland: bool) -> GrabMode {
    if !windows && wayland {
        GrabMode::Locked
    } else {
        GrabMode::Confined
    }
}

/// Whether winit runs on Wayland, by its own rule (winit 0.30's Linux
/// event loop, which eframe never forces): `WAYLAND_DISPLAY`, else
/// `WAYLAND_SOCKET`, set and not empty; else X11. (No variable picks X11
/// over a Wayland session any more: winit 0.29 dropped
/// `WINIT_UNIX_BACKEND`.) PR #563 review M4.
pub fn winit_on_wayland(
    wayland_display: Option<&std::ffi::OsStr>,
    wayland_socket: Option<&std::ffi::OsStr>,
) -> bool {
    wayland_display
        .filter(|v| !v.is_empty())
        .or(wayland_socket)
        .is_some_and(|v| !v.is_empty())
}

/// The grab on this computer.
pub fn grab_here() -> GrabMode {
    let display = std::env::var_os("WAYLAND_DISPLAY");
    let socket = std::env::var_os("WAYLAND_SOCKET");
    grab_mode(
        cfg!(windows),
        winit_on_wayland(display.as_deref(), socket.as_deref()),
    )
}

/// Where to put a confined pointer at `pointer` back, over `picture`: its
/// middle, once the pointer is outside the middle half (or not known);
/// `None` while it is well inside.
pub fn recentre(pointer: Option<egui::Pos2>, picture: egui::Rect) -> Option<egui::Pos2> {
    let middle = picture.shrink2(picture.size() / 4.0);
    match pointer {
        Some(at) if middle.contains(at) => None,
        _ => Some(picture.center()),
    }
}

/// The viewport commands that lock the pointer over `picture` with `mode`,
/// or free it. The pointer is hidden by the window each pass while locked
/// (`CursorIcon::None`), which the windowing glue keeps hidden.
pub fn commands(change: Change, mode: GrabMode, picture: egui::Rect) -> Vec<egui::ViewportCommand> {
    use egui::viewport::CursorGrab;
    match change {
        Change::Lock => {
            // Confined first: where Locked fails (winit only logs it), the
            // pointer stays confined, and the window re-centres it anyway
            // (PR #563 review M4); where it works, Locked replaces it.
            let mut grabs = vec![egui::ViewportCommand::CursorGrab(CursorGrab::Confined)];
            if mode == GrabMode::Locked {
                grabs.push(egui::ViewportCommand::CursorGrab(CursorGrab::Locked));
            }
            grabs
                .into_iter()
                .chain([
                    // Into the picture where the system moves it (Windows, X11).
                    // Wayland takes this only as where the pointer shows again on
                    // unlock: a locked pointer stays where it was, so the window
                    // turns its bar off while locked.
                    egui::ViewportCommand::CursorPosition(picture.center()),
                ])
                .collect()
        }
        Change::Free => vec![egui::ViewportCommand::CursorGrab(CursorGrab::None)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    const ALL: egui::Modifiers = egui::Modifiers {
        alt: true,
        ctrl: true,
        shift: true,
        mac_cmd: false,
        command: true,
    };

    fn able() -> Situation {
        Situation {
            focused: true,
            controls: true,
            picture: true,
        }
    }

    #[test]
    fn ctrl_alt_shift_m_is_the_switch() {
        assert!(lock_chord(&key(egui::Key::M, true, ALL)));
        assert!(!lock_chord(&key(egui::Key::M, false, ALL)), "its release");
        assert!(!lock_chord(&key(egui::Key::M, true, egui::Modifiers::CTRL)));
        assert!(!lock_chord(&key(egui::Key::S, true, ALL)), "the stats'");
        let mut repeat = key(egui::Key::M, true, ALL);
        if let egui::Event::Key { repeat: r, .. } = &mut repeat {
            *r = true;
        }
        assert!(!lock_chord(&repeat), "held, it switches once");
        assert!(is_lock_chord_key(&key(egui::Key::M, false, ALL)));
        assert!(!is_lock_chord_key(&key(
            egui::Key::M,
            true,
            egui::Modifiers::NONE
        )));
    }

    /// The switch locks and frees; the lock goes as the focus, control or
    /// the picture goes, and a focus that comes back does not lock again.
    #[test]
    fn the_lock_follows_the_switch_and_lets_go_when_the_window_cannot_hold_it() {
        let t = Instant::now();
        let mut lock = PointerLock::default();
        assert_eq!(lock.step(able(), t), None);
        lock.toggle();
        assert_eq!(lock.step(able(), t), Some(Change::Lock));
        assert!(lock.locked() && lock.wanted());
        assert_eq!(lock.step(able(), t), None, "held, nothing more");
        lock.toggle();
        assert_eq!(lock.step(able(), t), Some(Change::Free));
        assert!(!lock.locked());
        for lost in [
            Situation {
                focused: false,
                ..able()
            },
            Situation {
                controls: false,
                ..able()
            },
            Situation {
                picture: false,
                ..able()
            },
        ] {
            lock.toggle();
            assert_eq!(lock.step(able(), t), Some(Change::Lock));
            assert_eq!(lock.step(lost, t), Some(Change::Free), "{lost:?}");
            assert_eq!(lock.step(able(), t), None, "not again by itself");
            assert!(!lock.wanted());
        }
        // Asked while the window cannot hold it: nothing, and not later.
        lock.toggle();
        assert_eq!(
            lock.step(
                Situation {
                    controls: false,
                    ..able()
                },
                t
            ),
            None
        );
        assert_eq!(lock.step(able(), t), None);
        // The release chord frees it.
        lock.toggle();
        lock.step(able(), t);
        lock.free();
        assert_eq!(lock.step(able(), t), Some(Change::Free));
    }

    #[test]
    fn the_hint_shows_for_a_while_after_the_lock() {
        let t = Instant::now();
        let mut lock = PointerLock::default();
        assert!(!lock.hint(t));
        lock.toggle();
        lock.step(able(), t);
        assert!(lock.hint(t + Duration::from_secs(1)));
        assert!(!lock.hint(t + HINT_FOR));
    }

    /// Ctrl+Alt+Shift pressed and let go alone frees it; with another key
    /// pressed meanwhile (the switch's M, Ctrl+Alt+Shift+S) it does not.
    #[test]
    fn ctrl_alt_shift_alone_is_the_release() {
        use egui::Key::{AltLeft, ControlLeft, M, ShiftLeft};
        let ctrl = egui::Modifiers::CTRL;
        let ctrl_alt = egui::Modifiers {
            alt: true,
            ..egui::Modifiers::CTRL
        };
        let none = egui::Modifiers::NONE;
        let mut chord = ReleaseChord::default();
        // Over several passes, as fingers are slow.
        assert!(!chord.observe(&[key(ControlLeft, true, ctrl)], ctrl));
        assert!(!chord.observe(&[key(AltLeft, true, ctrl_alt)], ctrl_alt));
        assert!(!chord.observe(&[key(ShiftLeft, true, ALL)], ALL));
        assert!(!chord.observe(&[key(ShiftLeft, false, ctrl_alt)], ctrl_alt));
        assert!(chord.observe(&[key(ControlLeft, false, none)], none));
        assert!(!chord.observe(&[], none), "once");
        // All in one pass, said only by the keys' own modifiers.
        assert!(chord.observe(
            &[key(ControlLeft, true, ALL), key(ControlLeft, false, none),],
            none
        ));
        // With M pressed meanwhile: the switch, not the release.
        assert!(!chord.observe(&[key(M, true, ALL)], ALL));
        assert!(!chord.observe(&[key(M, false, ALL)], ALL));
        assert!(!chord.observe(&[], none));
        // Only two of the three: nothing.
        assert!(!chord.observe(&[], ctrl_alt));
        assert!(!chord.observe(&[], none));
        // Another key pressed before the third came down spoils it too.
        assert!(!chord.observe(&[key(egui::Key::A, true, ctrl)], ctrl));
        assert!(!chord.observe(&[], ALL));
        assert!(!chord.observe(&[], none));
    }

    #[test]
    fn winit_is_on_wayland_by_its_own_rule() {
        use std::ffi::OsStr;
        let set = Some(OsStr::new("wayland-1"));
        let empty = Some(OsStr::new(""));
        assert!(winit_on_wayland(set, None));
        assert!(winit_on_wayland(None, Some(OsStr::new("5"))), "a socket");
        assert!(winit_on_wayland(empty, Some(OsStr::new("5"))));
        assert!(!winit_on_wayland(empty, None), "empty is not set");
        assert!(!winit_on_wayland(None, empty));
        assert!(!winit_on_wayland(None, None), "X11");
    }

    #[test]
    fn wayland_locks_and_the_rest_confine() {
        assert_eq!(grab_mode(false, true), GrabMode::Locked);
        assert_eq!(grab_mode(false, false), GrabMode::Confined, "X11");
        assert_eq!(grab_mode(true, false), GrabMode::Confined, "Windows");
        assert_eq!(grab_mode(true, true), GrabMode::Confined);
    }

    #[test]
    fn a_confined_pointer_is_put_back_once_it_strays_from_the_middle() {
        let picture = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 200.0));
        let centre = egui::pos2(300.0, 150.0);
        assert_eq!(recentre(None, picture), Some(centre));
        assert_eq!(recentre(Some(centre), picture), None);
        assert_eq!(recentre(Some(egui::pos2(220.0, 110.0)), picture), None);
        assert_eq!(
            recentre(Some(egui::pos2(190.0, 150.0)), picture),
            Some(centre)
        );
        assert_eq!(
            recentre(Some(egui::pos2(300.0, 60.0)), picture),
            Some(centre)
        );
        assert_eq!(
            recentre(Some(egui::pos2(600.0, 150.0)), picture),
            Some(centre)
        );
    }

    #[test]
    fn the_grab_is_asked_with_the_pointer_put_in_the_picture() {
        use egui::viewport::CursorGrab;
        let picture = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 400.0));
        assert_eq!(
            commands(Change::Lock, GrabMode::Locked, picture),
            vec![
                egui::ViewportCommand::CursorGrab(CursorGrab::Confined),
                egui::ViewportCommand::CursorGrab(CursorGrab::Locked),
                egui::ViewportCommand::CursorPosition(egui::pos2(400.0, 240.0)),
            ],
            "confined first, should the lock fail"
        );
        assert_eq!(
            commands(Change::Lock, GrabMode::Confined, picture),
            vec![
                egui::ViewportCommand::CursorGrab(CursorGrab::Confined),
                egui::ViewportCommand::CursorPosition(egui::pos2(400.0, 240.0)),
            ]
        );
        assert_eq!(
            commands(Change::Free, GrabMode::Confined, picture),
            vec![egui::ViewportCommand::CursorGrab(CursorGrab::None)]
        );
    }

    /// Set to `1` on a test machine to let the live test grab its pointer.
    const POINTER_TEST_MACHINE: &str = "POCKETDESK_POINTER_TEST_MACHINE";

    /// winit's event loop off the main thread, as a test runs.
    fn any_thread(builder: &mut eframe::EventLoopBuilder<eframe::UserEvent>) {
        #[cfg(windows)]
        {
            use winit::platform::windows::EventLoopBuilderExtWindows;
            builder.with_any_thread(true);
        }
        #[cfg(target_os = "linux")]
        {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_any_thread(true);
        }
    }

    /// The owed desk check (checklist §235): a real window locks this
    /// computer's pointer as the view does, sums the raw movement for five
    /// seconds while the tester moves the mouse, then frees it and closes.
    /// The pointer must stay hidden and held, and come back where the
    /// window let it go. Never on the owner's own computer.
    #[test]
    #[ignore = "grabs this computer's pointer in a real window: run only on a test machine, with POCKETDESK_POINTER_TEST_MACHINE=1 (owed desk check)"]
    fn the_real_grab_locks_and_frees() {
        use std::sync::{Arc, Mutex};
        if std::env::var(POINTER_TEST_MACHINE).as_deref() != Ok("1") {
            eprintln!("skipped: set {POINTER_TEST_MACHINE}=1 on a test machine");
            return;
        }
        struct Live {
            start: Instant,
            lock: PointerLock,
            asked: bool,
            input: crate::view_input::WindowInput,
            moves: Arc<Mutex<(usize, i64)>>,
        }
        impl eframe::App for Live {
            fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
                let ctx = ui.ctx().clone();
                let area = ui.max_rect();
                let elapsed = self.start.elapsed();
                // Asked once, as the bar would; a lock that lapsed (the
                // focus lost) is not asked again, as in the window (PR
                // #563 review M10).
                if !self.asked && elapsed > Duration::from_millis(500) {
                    self.asked = true;
                    self.lock.toggle();
                } else if elapsed >= Duration::from_secs(6) {
                    self.lock.free();
                }
                let now = Situation {
                    focused: ctx.input(|i| i.viewport().focused == Some(true)),
                    controls: true,
                    picture: true,
                };
                let mode = grab_here();
                if let Some(change) = self.lock.step(now, Instant::now()) {
                    for command in commands(change, mode, area) {
                        ctx.send_viewport_cmd(command);
                    }
                }
                self.input.set_relative(self.lock.locked());
                let events = ctx.input(|i| i.events.clone());
                let packets = self.input.take(&events, area, Instant::now());
                if let Ok(mut moves) = self.moves.lock() {
                    for packet in packets {
                        if let pocketdesk_core::streaming::InputPacket::MouseMove { dx, dy } =
                            packet
                        {
                            moves.0 += 1;
                            moves.1 += i64::from(dx).abs() + i64::from(dy).abs();
                        }
                    }
                }
                if self.lock.locked() {
                    ctx.set_cursor_icon(egui::CursorIcon::None);
                    // As the window does, whatever the mode.
                    if let Some(back) = recentre(ctx.input(|i| i.pointer.latest_pos()), area) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(back));
                    }
                }
                ui.label(HINT);
                if elapsed > Duration::from_secs(7) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        let moves = Arc::new(Mutex::new((0usize, 0i64)));
        let app = Live {
            start: Instant::now(),
            lock: PointerLock::default(),
            asked: false,
            input: crate::view_input::WindowInput::default(),
            moves: moves.clone(),
        };
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Pocketdesk pointer lock test")
                .with_inner_size([640.0, 400.0]),
            event_loop_builder: Some(Box::new(any_thread)),
            ..Default::default()
        };
        eframe::run_native(
            "pocketdesk-pointer-test",
            options,
            Box::new(|_| Ok(Box::new(app))),
        )
        .expect("the window ran");
        let (count, distance) = *moves.lock().unwrap();
        eprintln!("relative moves: {count}, distance {distance}");
        assert!(count > 0, "the mouse was moved while the window held it");
    }
}
