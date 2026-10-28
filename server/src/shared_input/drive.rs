//! Driving another computer: this computer's own keyboard and mouse carried
//! onto the owner's other computers.
//!
//! [`Driver`] is the state machine, with no I/O of its own: what the input
//! thread captured ([`Captured`]) goes in, what to do comes out
//! ([`Action`]): messages for the other computers, the input thread's
//! [`Mode`], where to park or put the cursor, the hidden cursor, the
//! keep-awake and a word for the owner. Every position is in logical
//! pixels: this computer's own desktop space at home, the other computer's
//! while away (as [`Layout`] has them).
//!
//! At home the mouse is watched: a push off a screen's edge onto another
//! computer ([`Layout::crossing`]) sends it [`Msg::Enter`], and from then on
//! this computer's input is swallowed and sent there. The cursor is parked
//! at the centre of the screen it left and hidden; each move is how far the
//! mouse went from there, carried onto a virtual cursor on the other
//! computer's screens. Crossing back, the switch key, Windows locking or
//! going to sleep, the link turning relayed or failing all bring it home,
//! and everything sent as down is let go of first (except after a failed
//! send: nothing stale is sent again, the other computer lets go of what it
//! held by itself when it stops hearing from this one).
//!
//! While a game is in front ([`game_lock`](super::game_lock)), the
//! keyboard and mouse stay here with no hooks at all ([`Mode::Off`]): the
//! edges lead nowhere, and a game that comes to the front while they are
//! away brings them home. The switch key may still force them off the game
//! (unless a kernel-level anti-cheat runs or the program in front is
//! elevated): the hooks are back only while they are away, and gone again
//! as they come home.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

use super::badge;
use super::game_lock::{self, Foreground, Lock};
use super::held::HeldInput;
use super::keys_windows::evdev_from_scan;
use super::layout::{Layout, Placed};
use super::wire::{Msg, REFUSE_SHARING_OFF, Screen};

/// What the input thread does with this computer's keyboard and mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No hooks: only the switch key is listened for.
    Off,
    /// The mouse is watched (for its edges) and nothing is swallowed.
    Watch,
    /// Every key and the mouse are swallowed and handed on.
    Away,
}

/// What the input thread (or, for the foreground, the loop's probes) saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// The pointer is at (or, away, was moved to) this point, logical.
    Moved {
        x: i32,
        y: i32,
    },
    /// A key, as a set-1 scancode and whether it has the E0 prefix.
    Key {
        scan: u16,
        extended: bool,
        down: bool,
    },
    /// 0 left, 1 middle, 2 right, 3 back, 4 forward.
    Button {
        button: u8,
        down: bool,
    },
    /// Notches, positive down and to the right.
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// The switch key was pressed.
    Hotkey,
    /// The session locked (or the computer is going to sleep).
    Locked,
    Unlocked,
    /// The input went to another desktop (a UAC prompt, Ctrl+Alt+Del, the
    /// lock or screensaver desktop): its switches hide key-ups from the
    /// hooks, so the keyboard and mouse come home as for a lock.
    DesktopSwitch,
    /// What is in front here now, and whether a kernel-level anti-cheat
    /// runs (polled every half second while sharing is on).
    Foreground {
        fg: Foreground,
        kernel_anti_cheat: bool,
    },
    /// Another window came to the front (heard while the hooks are in): no
    /// edge crosses until what is in front has been looked at again.
    ForegroundChanged,
}

/// What the driver wants done.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send this to that computer.
    Send(String, Msg),
    /// Put the input thread in this mode.
    SetMode(Mode),
    /// Hold this computer's cursor here (logical), where it rests while away.
    ParkCursor(i32, i32),
    /// The cursor is home: put it here (logical).
    WarpHome(i32, i32),
    /// Hide this computer's cursor (true) or show it again.
    HideCursor(bool),
    /// Keep this computer awake (true) or let it idle again.
    KeepAwake(bool),
    /// A word for the owner on where the keyboard and mouse are.
    Badge(badge::Badge),
    /// Push this computer's clipboard to that computer: the cursor just
    /// went there, and the clipboard follows it.
    PushClipboard(String),
}

/// Where the keyboard and mouse are while away.
#[derive(Debug, Clone)]
struct Away {
    /// The computer driven.
    to: String,
    /// Its virtual cursor, in its own desktop space.
    at: (i32, i32),
    /// Where this computer's cursor is parked, in its own space.
    park: (i32, i32),
    /// Where the cursor was just before it crossed (this computer's own
    /// space): where it returns if the crossing itself failed.
    from: (i32, i32),
}

/// The driver's word that is no switch, as a badge.
fn note(words: &str) -> Action {
    Action::Badge(badge::of(&badge::Event::Note(words)))
}

/// The state of this computer's keyboard and mouse.
pub struct Driver {
    this: String,
    layout: Layout,
    /// Where the cursor was last seen at home.
    home: Option<(i32, i32)>,
    away: Option<Away>,
    /// What was sent as down to the computer driven.
    held: HeldInput,
    /// The count each [`Msg::Move`] carries.
    seq: u32,
    /// The session is locked: nothing goes anywhere.
    locked: bool,
    /// The mode the input thread was last told.
    mode: Mode,
    /// The computers the cursor may cross onto now: their channel is open
    /// and direct and they answered this run (the glue keeps it up to date).
    reachable: HashSet<String>,
    /// The gaming lock's settings: on, and the programs that always lock
    /// while in front.
    game_lock: bool,
    lock_programs: Vec<String>,
    /// What was last in front here, and whether a kernel-level anti-cheat
    /// ran then.
    foreground: Option<(Foreground, bool)>,
    /// A game holds the keyboard and mouse here (as last looked at).
    game_held: bool,
    /// The keyboard and mouse went off a game here by the switch key: no
    /// snap back while they are away.
    forced: bool,
    /// Another window came to the front and has not been looked at yet.
    foreground_pending: bool,
    /// The mouse was not watched until now (no hooks): the first position
    /// watched only marks where it is, so no stale point measures a move.
    reseed: bool,
    /// The clipboard follows the mouse (`shared_input_clipboard`).
    clipboard: bool,
}

impl Driver {
    /// This computer is `this` in `layout`.
    pub fn new(this: &str, layout: Layout) -> Driver {
        Driver {
            this: this.to_string(),
            layout,
            home: None,
            away: None,
            held: HeldInput::default(),
            seq: 0,
            locked: false,
            mode: Mode::Off,
            reachable: HashSet::new(),
            game_lock: true,
            lock_programs: Vec::new(),
            foreground: None,
            game_held: false,
            forced: false,
            foreground_pending: false,
            reseed: false,
            clipboard: false,
        }
    }

    /// Whether the clipboard follows the mouse (`shared_input_clipboard`):
    /// pushed to each computer the cursor goes to, as it goes.
    pub fn set_clipboard(&mut self, on: bool) {
        self.clipboard = on;
    }

    /// The gaming lock's settings before the driver starts: nothing is
    /// weighed until what is in front is known, so a game already there
    /// never sees a hook.
    pub fn with_game_lock(mut self, on: bool, programs: Vec<String>) -> Driver {
        self.game_lock = on;
        self.lock_programs = programs;
        self
    }

    /// The gaming lock's settings (`shared_input_game_lock`,
    /// `shared_input_lock_programs`), weighed again at once.
    pub fn set_game_lock(&mut self, on: bool, programs: Vec<String>) -> Vec<Action> {
        self.game_lock = on;
        self.lock_programs = programs;
        let mut out = Vec::new();
        self.weigh_game(&mut out);
        out
    }

    /// Whether a game holds the keyboard and mouse here now.
    pub fn game_held(&self) -> bool {
        self.game_held
    }

    /// The computers the cursor may cross onto now (an open, direct channel
    /// that answered this run). The glue sets it; a computer that drops out
    /// while it is driven brings the keyboard and mouse home.
    pub fn set_reachable(&mut self, ids: Vec<String>) -> Vec<Action> {
        self.reachable = ids.into_iter().collect();
        let mut out = Vec::new();
        if let Some(away) = self.away.clone()
            && !self.reachable.contains(&away.to)
        {
            let to = self.name_of(&away.to);
            self.go_home(
                &mut out,
                away.from,
                true,
                Some(&format!("{to} is no longer reachable")),
            );
        }
        out
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The computer driven now.
    pub fn away_on(&self) -> Option<&str> {
        self.away.as_ref().map(|a| a.to.as_str())
    }

    /// The input thread's first mode: the mouse watched when there is a
    /// computer to go to.
    pub fn start(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        self.settle(&mut out);
        out
    }

    /// Act on what the input thread saw.
    pub fn on(&mut self, captured: Captured) -> Vec<Action> {
        let mut out = Vec::new();
        match captured {
            Captured::Locked => {
                self.locked = true;
                if let Some(away) = &self.away {
                    let park = away.park;
                    self.go_home(&mut out, park, true, Some("This computer locked"));
                }
                self.settle(&mut out);
            }
            Captured::Unlocked => {
                self.locked = false;
                self.settle(&mut out);
            }
            Captured::DesktopSwitch => {
                if let Some(away) = self.away.clone() {
                    self.go_home(
                        &mut out,
                        away.park,
                        true,
                        Some("A secure prompt took the screen"),
                    );
                }
                self.settle(&mut out);
            }
            Captured::Foreground {
                fg,
                kernel_anti_cheat,
            } => {
                self.foreground = Some((fg, kernel_anti_cheat));
                self.foreground_pending = false;
                self.weigh_game(&mut out);
            }
            Captured::ForegroundChanged => self.foreground_pending = true,
            _ if self.locked => {}
            Captured::Moved { x, y } => match &self.away {
                None => self.watch((x, y), &mut out),
                Some(_) => self.carry((x, y), &mut out),
            },
            Captured::Key {
                scan,
                extended,
                down,
            } => {
                if let (Some(away), Some(code)) = (&self.away, evdev_from_scan(scan, extended)) {
                    // A key held down repeats on the other computer by
                    // itself, and one never sent down is not let go of.
                    if down != self.held.holds_key(code) {
                        let msg = Msg::Key {
                            code,
                            pressed: down,
                        };
                        self.held.note(&msg);
                        out.push(Action::Send(away.to.clone(), msg));
                    }
                }
            }
            Captured::Button { button, down } => {
                if let Some(away) = &self.away
                    && down != self.held.holds_button(button)
                {
                    let msg = Msg::Button {
                        button,
                        pressed: down,
                    };
                    self.held.note(&msg);
                    out.push(Action::Send(away.to.clone(), msg));
                }
            }
            Captured::Wheel { dx, dy } => {
                if let Some(away) = &self.away
                    && (dx != 0 || dy != 0)
                {
                    out.push(Action::Send(away.to.clone(), Msg::Wheel { dx, dy }));
                }
            }
            Captured::Hotkey => self.hotkey(&mut out),
        }
        out
    }

    /// Sending to the computer driven failed: home at once, with nothing
    /// sent (the link is gone or broken) and nothing kept to send again.
    /// The other computer lets go of what it held when it stops hearing
    /// from this one.
    pub fn on_send_error(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(away) = self.away.clone() {
            // Suppress this target until its channel reopens (the glue adds
            // it to `reachable` again), so a push does not re-cross and
            // re-fail at once.
            self.reachable.remove(&away.to);
            let to = self.name_of(&away.to);
            self.go_home(
                &mut out,
                away.from,
                false,
                Some(&format!("The link to {to} failed")),
            );
        }
        out
    }

    /// The link to `device_id` closed or turned relayed (`why`, for the
    /// owner): if it is driven, home, letting go of what was sent down.
    pub fn link_gone(&mut self, device_id: &str, why: &str) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(away) = self.away.as_ref().filter(|a| a.to == device_id) {
            let park = away.park;
            self.go_home(&mut out, park, true, Some(why));
        }
        out
    }

    /// The computer `device_id` refused to be driven (`reason`, as
    /// [`Msg::Refuse`] gives it): no longer one to cross onto (until the
    /// glue finds it reachable again), and if it is driven, home, letting
    /// go of what was sent down.
    pub fn refused(&mut self, device_id: &str, reason: u8) -> Vec<Action> {
        self.reachable.remove(device_id);
        let mut out = Vec::new();
        if let Some(away) = self.away.as_ref().filter(|a| a.to == device_id) {
            let park = away.park;
            let why = match reason {
                REFUSE_SHARING_OFF => "the keyboard and mouse are turned off there",
                _ => "it does not allow this computer (its Keyboard and mouse page)",
            };
            let why = format!("{} won't take input: {why}", self.name_of(device_id));
            self.go_home(&mut out, park, true, Some(&why));
        }
        out
    }

    /// The computers or their screens changed: the computer driven gone
    /// from them brings the keyboard and mouse home, and its cursor stays
    /// on what screens it has.
    pub fn set_layout(&mut self, layout: Layout) -> Vec<Action> {
        self.layout = layout;
        let mut out = Vec::new();
        if let Some(away) = self.away.clone() {
            let gone = self
                .layout
                .computers
                .iter()
                .all(|c| c.device_id != away.to || c.screens.is_empty());
            if gone {
                let to = self.name_of(&away.to);
                self.go_home(&mut out, away.park, true, Some(&format!("{to} is gone")));
            } else {
                let at = self.layout.clamp(&away.to, away.at);
                if let Some(away) = self.away.as_mut() {
                    away.at = at;
                }
            }
        }
        self.settle(&mut out);
        out
    }

    /// Sharing is turning off: home, letting go of what was sent down, and
    /// nothing watched.
    pub fn shutdown(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(away) = &self.away {
            let park = away.park;
            self.go_home(&mut out, park, true, None);
        }
        if self.mode != Mode::Off {
            self.mode = Mode::Off;
            out.push(Action::SetMode(Mode::Off));
        }
        out
    }

    /// The mode the input thread should be in now.
    fn wanted_mode(&self) -> Mode {
        if self.away.is_some() {
            Mode::Away
        } else if self.locked || self.game_held || self.targets().is_empty() {
            Mode::Off
        } else {
            Mode::Watch
        }
    }

    /// What the gaming lock wants now, with what was last in front: a game
    /// that came to the front while away (not by the switch key), or a
    /// refusal that came up while forced away, brings the keyboard and mouse
    /// home; at home, a game in front takes the hooks out.
    fn weigh_game(&mut self, out: &mut Vec<Action>) {
        let (held, lock) = match &self.foreground {
            Some((fg, kernel_anti_cheat)) => (
                game_lock::in_front(fg, &self.lock_programs, self.game_lock),
                game_lock::decide(
                    fg,
                    &self.lock_programs,
                    self.game_lock,
                    *kernel_anti_cheat,
                    self.away.is_some(),
                    self.forced,
                ),
            ),
            None => (false, Lock::Free),
        };
        let was = std::mem::replace(&mut self.game_held, held);
        // While a game holds them here no move is watched: where the cursor
        // is comes from the probe, for a forced switch to park from.
        if held
            && self.away.is_none()
            && let Some(at) = self.foreground.as_ref().and_then(|(fg, _)| fg.cursor)
        {
            self.home = Some(at);
        }
        let why = match lock {
            Lock::SnapBack => Some("A game came to the front here"),
            Lock::ForceRefused(why) => Some(why),
            Lock::Free | Lock::Locked => None,
        };
        if let (Some(why), Some(away)) = (why, self.away.clone()) {
            self.go_home(out, away.park, true, Some(why));
            return;
        }
        if self.away.is_none() && held != was && !self.targets().is_empty() {
            out.push(note(if held {
                "A game is in front: the keyboard and mouse stay here (the switch key still \
                 moves them)"
            } else {
                "No game in front: the screens' edges lead to the other computers again"
            }));
        }
        self.settle(out);
    }

    /// Tell the input thread the mode, if it changed.
    fn settle(&mut self, out: &mut Vec<Action>) {
        let wanted = self.wanted_mode();
        if wanted != self.mode {
            // Watched again after a time unwatched: the last position seen
            // is stale, so the first one now only marks where it is.
            if wanted == Mode::Watch {
                self.reseed = true;
            }
            self.mode = wanted;
            out.push(Action::SetMode(wanted));
        }
    }

    /// The other computers there are to go to, in order.
    fn targets(&self) -> Vec<&Placed> {
        self.layout
            .computers
            .iter()
            .filter(|c| c.device_id != self.this && !c.screens.is_empty())
            .collect()
    }

    /// A computer's name as the owner is shown it (on one line, clipped):
    /// it goes into the badge's reasons as it is.
    fn name_of(&self, device_id: &str) -> String {
        let name = self
            .layout
            .computers
            .iter()
            .find(|c| c.device_id == device_id)
            .map_or(device_id, |c| c.name.as_str());
        badge::clip(name, badge::NAME_CHARS)
    }

    /// At home: the cursor moved to `pos`; off an edge onto another
    /// computer, it goes there.
    fn watch(&mut self, pos: (i32, i32), out: &mut Vec<Action>) {
        let prev = self.home.replace(pos);
        if std::mem::take(&mut self.reseed) {
            return;
        }
        let Some(prev) = prev else {
            return;
        };
        let (dx, dy) = (pos.0.saturating_sub(prev.0), pos.1.saturating_sub(prev.1));
        // A pointer pushed past the edge is on its last pixel.
        let at = self.layout.clamp(&self.this, pos);
        // A game in front: the edges lead nowhere (the hooks are out, but a
        // move already on its way may still come).
        if !self.game_held
            && !self.foreground_pending
            && let Some(crossing) = self.layout.crossing(&self.this, at, dx, dy)
            && crossing.to != self.this
            && self.reachable.contains(&crossing.to)
        {
            let park = self.park_for(at);
            self.go_away(out, &crossing.to, crossing.at, park, at);
        }
    }

    /// Away: the mouse was moved to `pos` from where it is parked; carry the
    /// movement onto the virtual cursor.
    fn carry(&mut self, pos: (i32, i32), out: &mut Vec<Action>) {
        let Some(away) = self.away.clone() else {
            return;
        };
        let (dx, dy) = (
            pos.0.saturating_sub(away.park.0),
            pos.1.saturating_sub(away.park.1),
        );
        if dx == 0 && dy == 0 {
            return;
        }
        let to = (away.at.0.saturating_add(dx), away.at.1.saturating_add(dy));
        let at = self.layout.clamp(&away.to, to);
        match self.layout.crossing(&away.to, at, dx, dy) {
            Some(crossing) if crossing.to == self.this => {
                self.go_home(out, crossing.at, true, None);
            }
            Some(crossing) if self.reachable.contains(&crossing.to) => {
                self.leave(out);
                self.go_away(out, &crossing.to, crossing.at, away.park, away.park);
            }
            // An edge onto a computer that is not reachable is a wall: the
            // cursor stays here, clamped to the far screen.
            _ => {
                if at != away.at {
                    self.seq = self.seq.wrapping_add(1);
                    let (x, y) = self.layout.normalized(&away.to, at);
                    out.push(Action::Send(
                        away.to.clone(),
                        Msg::Move {
                            seq: self.seq,
                            x,
                            y,
                        },
                    ));
                    if let Some(away) = self.away.as_mut() {
                        away.at = at;
                    }
                }
                out.push(Action::ParkCursor(away.park.0, away.park.1));
            }
        }
    }

    /// The switch key: home goes to the first other computer, each goes to
    /// the next, the last comes home. With a game in front at home it forces
    /// the keyboard and mouse off it, if the gaming lock allows.
    fn hotkey(&mut self, out: &mut Vec<Action>) {
        let forcing = self.away.is_none() && self.game_held;
        if forcing && let Some((fg, kernel_anti_cheat)) = &self.foreground {
            let lock = game_lock::decide(
                fg,
                &self.lock_programs,
                self.game_lock,
                *kernel_anti_cheat,
                false,
                true,
            );
            if let Lock::ForceRefused(why) = lock {
                out.push(note(why));
                return;
            }
        }
        let targets: Vec<String> = self
            .targets()
            .iter()
            .filter(|c| self.reachable.contains(&c.device_id))
            .map(|c| c.device_id.clone())
            .collect();
        let next = match &self.away {
            None => targets.first(),
            Some(away) => targets
                .iter()
                .position(|t| *t == away.to)
                .and_then(|i| targets.get(i + 1)),
        }
        .cloned();
        match (next, self.away.clone()) {
            (None, None) => out.push(note("No other computer to move the keyboard and mouse to")),
            (None, Some(away)) => {
                self.go_home(out, away.park, true, None);
            }
            (Some(next), away) => {
                let park = match &away {
                    Some(away) => {
                        self.leave(out);
                        away.park
                    }
                    None => {
                        self.forced = forcing;
                        let at = self
                            .home
                            .map(|h| self.layout.clamp(&self.this, h))
                            .unwrap_or_default();
                        self.park_for(at)
                    }
                };
                let at = self
                    .layout
                    .computers
                    .iter()
                    .find(|c| c.device_id == next)
                    .and_then(|c| c.screens.first())
                    .map_or((0, 0), centre);
                self.go_away(out, &next, at, park, park);
            }
        }
    }

    /// The centre of this computer's screen at `at` (or its first screen).
    fn park_for(&self, at: (i32, i32)) -> (i32, i32) {
        let screens = self
            .layout
            .computers
            .iter()
            .find(|c| c.device_id == self.this)
            .map(|c| c.screens.as_slice())
            .unwrap_or_default();
        screens
            .iter()
            .find(|s| contains(s, at))
            .or(screens.first())
            .map_or(at, centre)
    }

    /// Go to `to`, its cursor at `at` (its own space), this computer's
    /// parked at `park`.
    fn go_away(
        &mut self,
        out: &mut Vec<Action>,
        to: &str,
        at: (i32, i32),
        park: (i32, i32),
        from: (i32, i32),
    ) {
        let was_away = self.away.is_some();
        let (x, y) = self.layout.normalized(to, at);
        self.away = Some(Away {
            to: to.to_string(),
            at,
            park,
            from,
        });
        // Parked before the mouse is swallowed, so no movement is measured
        // from where the cursor was before.
        out.push(Action::ParkCursor(park.0, park.1));
        self.settle(out);
        out.push(Action::Send(to.to_string(), Msg::Enter { x, y }));
        // Once as the cursor arrives, never with each move.
        if self.clipboard {
            out.push(Action::PushClipboard(to.to_string()));
        }
        if !was_away {
            out.push(Action::HideCursor(true));
            out.push(Action::KeepAwake(true));
        }
        let to = self.name_of(to);
        out.push(Action::Badge(badge::of(&badge::Event::Went { to: &to })));
    }

    /// Let go of everything sent down to the computer driven, and leave it.
    fn leave(&mut self, out: &mut Vec<Action>) {
        if let Some(away) = &self.away {
            for msg in self.held.releases() {
                out.push(Action::Send(away.to.clone(), msg));
            }
            out.push(Action::Send(away.to.clone(), Msg::Leave));
        }
    }

    /// Back home, the cursor at `at`: with `releases`, everything sent down
    /// is let go of and the computer driven is left first; without, nothing
    /// is sent and what was held is forgotten. The badge names this
    /// computer, with `why` when the owner did not bring them back.
    fn go_home(
        &mut self,
        out: &mut Vec<Action>,
        at: (i32, i32),
        releases: bool,
        why: Option<&str>,
    ) {
        let at = self.nudge_inside(at);
        if releases {
            self.leave(out);
        } else {
            drop(self.held.releases());
        }
        self.away = None;
        self.forced = false;
        self.home = Some(at);
        // Put back before the mouse is only watched again, so the first
        // position watched is where it came home.
        out.push(Action::WarpHome(at.0, at.1));
        self.settle(out);
        out.push(Action::HideCursor(false));
        out.push(Action::KeepAwake(false));
        let here = self.name_of(&self.this);
        out.push(Action::Badge(badge::of(&badge::Event::Home {
            here: &here,
            why,
        })));
    }
}

impl Driver {
    /// `at` moved a few pixels inside this computer's screen, so the cursor
    /// does not come home sitting on the very edge it would cross from.
    fn nudge_inside(&self, at: (i32, i32)) -> (i32, i32) {
        const EDGE: i32 = 4;
        let screens = self
            .layout
            .computers
            .iter()
            .find(|c| c.device_id == self.this)
            .map(|c| c.screens.as_slice())
            .unwrap_or_default();
        let Some(s) = screens.iter().find(|s| contains(s, at)) else {
            return at;
        };
        let inside = |v: i32, lo: i32, len: i32| {
            let (lo, hi) = (lo + EDGE, lo + len - 1 - EDGE);
            if lo <= hi {
                v.clamp(lo, hi)
            } else {
                lo - EDGE + len / 2
            }
        };
        (
            inside(at.0, s.x, s.width as i32),
            inside(at.1, s.y, s.height as i32),
        )
    }
}

fn contains(s: &Screen, (x, y): (i32, i32)) -> bool {
    let (x, y) = (i64::from(x), i64::from(y));
    let (x0, y0) = (i64::from(s.x), i64::from(s.y));
    (x0..x0 + i64::from(s.width)).contains(&x) && (y0..y0 + i64::from(s.height)).contains(&y)
}

fn centre(s: &Screen) -> (i32, i32) {
    let half = |len: u32| i32::try_from(len / 2).unwrap_or(i32::MAX);
    (
        s.x.saturating_add(half(s.width)),
        s.y.saturating_add(half(s.height)),
    )
}

/// An open channel to another computer, as [`reachable`] weighs it.
#[derive(Debug, Clone)]
pub struct OpenChannel {
    pub device_id: String,
    /// When this channel was opened (before its `Hello` went).
    pub opened: Instant,
    /// Whether the link to that computer is direct now.
    pub direct: bool,
}

/// Whether `msg` from another computer answers this one's `Hello`: only its
/// screens do (it sends them only to a computer it lets drive it). A
/// `Hello` from that computer's own driver, or its activity notes, are not.
pub fn is_answer(msg: &Msg) -> bool {
    matches!(msg, Msg::Screens(_))
}

/// The computers the cursor may cross onto: an open channel whose link is
/// direct, to a computer that sent its screens (`screens_heard`, when each
/// last did) since that channel opened. There is no time window: a quiet
/// computer stays reachable for as long as its channel is open; a channel
/// opened anew needs a new answer.
pub fn reachable(
    channels: &[OpenChannel],
    screens_heard: &HashMap<String, Instant>,
) -> BTreeSet<String> {
    channels
        .iter()
        .filter(|c| {
            c.direct
                && screens_heard
                    .get(&c.device_id)
                    .is_some_and(|heard| *heard >= c.opened)
        })
        .map(|c| c.device_id.clone())
        .collect()
}

/// The layout this computer drives in: its own screens, and the linked
/// computers whose screens `shared_screens.json` holds where the owner put
/// them (`saved`, the Keyboard and mouse page's `shared_layout.json`), or,
/// with no arrangement or one that would put screens over each other, to
/// their right in the file's order. The places are worked out over every
/// computer in the file, linked or not, as the page draws them.
pub fn layout_for(
    this: &str,
    name: &str,
    own: Vec<Screen>,
    file: &pocketdesk_core::local::shared_input::ScreensFile,
    linked: &[String],
    saved: Option<&pocketdesk_core::local::shared_input::LayoutFile>,
) -> Layout {
    // Arranged as the Keyboard and mouse page does, over every computer
    // whose screens are known, so the two agree; then only the linked ones
    // are kept, each where it was put.
    let others: Vec<(String, Vec<Screen>)> = file
        .computers
        .iter()
        .filter(|c| !c.this_computer && c.device_id != this && !c.screens.is_empty())
        .map(|c| (c.device_id.clone(), c.screens.clone()))
        .collect();
    let origins = pocketdesk_core::local::shared_input::arrange(&own, &others, saved);
    let (others, origins): (Vec<_>, Vec<_>) = others
        .into_iter()
        .zip(origins)
        .filter(|((id, _), _)| linked.contains(id))
        .unzip();
    let mut computers = vec![Placed {
        device_id: this.to_string(),
        name: name.to_string(),
        origin: (0, 0),
        screens: own,
    }];
    computers.extend(
        others
            .into_iter()
            .zip(origins)
            .map(|((id, screens), origin)| {
                let name = file.get(&id).map(|c| c.name.clone()).unwrap_or_default();
                Placed {
                    device_id: id,
                    name,
                    origin,
                    screens,
                }
            }),
    );
    Layout { computers }
}

/// The owner's arrangement (`shared_layout.json`), read again only when
/// the file changed.
#[derive(Default)]
pub struct SavedLayout {
    stamp: Option<std::time::SystemTime>,
    file: Option<pocketdesk_core::local::shared_input::LayoutFile>,
}

impl SavedLayout {
    /// The arrangement in `state_dir`, if there is one that reads.
    pub fn get(
        &mut self,
        state_dir: &std::path::Path,
    ) -> Option<&pocketdesk_core::local::shared_input::LayoutFile> {
        let path = state_dir.join(pocketdesk_core::local::paths::SHARED_LAYOUT_FILE);
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp.is_none() || stamp != self.stamp {
            self.stamp = stamp;
            self.file = stamp.and(pocketdesk_core::local::shared_input::read_layout(state_dir));
        }
        self.file.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::shared_input::{ComputerScreens, LayoutFile, ScreensFile};
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    fn placed(id: &str, screens: Vec<Screen>) -> Placed {
        Placed {
            device_id: id.to_string(),
            name: format!("{id} computer"),
            origin: (0, 0),
            screens,
        }
    }

    /// The owner's desk: Windows ("W", a monitor and the laptop's screen to
    /// its right) and the Linux laptop ("L") to the right of that, and with
    /// `third`, another ("M") to the right of the Linux one.
    fn desk(third: bool) -> Layout {
        let mut others = vec![placed("L", vec![screen(0, 0, 1600, 1000)])];
        if third {
            others.push(placed("M", vec![screen(0, 0, 1280, 800)]));
        }
        Layout::default_for(
            placed(
                "W",
                vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)],
            ),
            others,
        )
    }

    /// The centre of the laptop's screen, where the cursor parks when it
    /// leaves from there.
    const PARK: (i32, i32) = (1920 + 768, 480);

    fn sends(actions: &[Action]) -> Vec<(String, Msg)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(to, msg) => Some((to.clone(), msg.clone())),
                _ => None,
            })
            .collect()
    }

    fn sent(to: &str, msg: Msg) -> (String, Msg) {
        (to.to_string(), msg)
    }

    fn moved(x: i32, y: i32) -> Captured {
        Captured::Moved { x, y }
    }

    fn key(scan: u16, down: bool) -> Captured {
        Captured::Key {
            scan,
            extended: false,
            down,
        }
    }

    fn evdev(code: u16, pressed: bool) -> Msg {
        Msg::Key { code, pressed }
    }

    /// The badge for the keyboard and mouse coming home to W, for `why`.
    fn home_badge(why: Option<&str>) -> Action {
        Action::Badge(badge::of(&badge::Event::Home {
            here: "W computer",
            why,
        }))
    }

    /// A driver in `layout` with the given computers reachable.
    fn driver(layout: Layout, reachable: &[&str]) -> Driver {
        let mut d = Driver::new("W", layout);
        d.set_reachable(reachable.iter().map(|s| s.to_string()).collect());
        d
    }

    /// A driver whose cursor has just crossed onto the Linux laptop at
    /// (0, 480).
    fn away_on_linux(third: bool) -> Driver {
        let mut d = driver(desk(third), &["L", "M"]);
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"), "{crossed:?}");
        d
    }

    #[test]
    fn crossing_sends_enter_then_moves() {
        let layout = desk(false);
        let mut d = driver(layout.clone(), &["L"]);
        assert_eq!(d.start(), vec![Action::SetMode(Mode::Watch)]);
        // The first sighting has no direction yet; moving inside a screen
        // goes nowhere.
        assert_eq!(d.on(moved(3400, 480)), vec![]);
        assert_eq!(d.on(moved(3440, 480)), vec![]);
        // The last pixel of the laptop's screen, heading right: onto the
        // Linux laptop at the same height.
        let crossed = d.on(moved(3455, 480));
        let (x, y) = layout.normalized("L", (0, 480));
        assert_eq!(sends(&crossed), vec![sent("L", Msg::Enter { x, y })]);
        for wanted in [
            Action::SetMode(Mode::Away),
            Action::ParkCursor(PARK.0, PARK.1),
            Action::HideCursor(true),
        ] {
            assert!(crossed.contains(&wanted), "{wanted:?} in {crossed:?}");
        }
        assert_eq!(d.mode(), Mode::Away);
        // Parked before anything is swallowed.
        let at = |wanted: &Action| crossed.iter().position(|a| a == wanted).unwrap();
        assert!(
            at(&Action::ParkCursor(PARK.0, PARK.1)) < at(&Action::SetMode(Mode::Away)),
            "{crossed:?}"
        );
        // The mouse moving 10 right and 5 down from where it is parked moves
        // the cursor there on the other computer, and parks it again.
        let step = d.on(moved(PARK.0 + 10, PARK.1 + 5));
        let (x, y) = layout.normalized("L", (10, 485));
        assert_eq!(sends(&step), vec![sent("L", Msg::Move { seq: 1, x, y })]);
        assert!(step.contains(&Action::ParkCursor(PARK.0, PARK.1)));
        // Each move counts on.
        let step = d.on(moved(PARK.0 + 1, PARK.1));
        let (x, y) = layout.normalized("L", (11, 485));
        assert_eq!(sends(&step), vec![sent("L", Msg::Move { seq: 2, x, y })]);
        // The park point itself is no movement.
        assert_eq!(sends(&d.on(moved(PARK.0, PARK.1))), vec![]);
    }

    fn pushes(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::PushClipboard(to) => Some(to.clone()),
                _ => None,
            })
            .collect()
    }

    /// The clipboard goes with the cursor once as it arrives, never with
    /// each move or key, and not as it comes home (the other computer
    /// pushes its own back then).
    #[test]
    fn clipboard_pushed_once_per_enter() {
        let mut d = driver(desk(true), &["L", "M"]);
        d.set_clipboard(true);
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert_eq!(pushes(&crossed), ["L"], "{crossed:?}");
        // After the Enter, so the other computer is driven when it lands.
        let at = |wanted: &dyn Fn(&Action) -> bool| crossed.iter().position(wanted).unwrap();
        assert!(
            at(&|a| matches!(a, Action::Send(_, Msg::Enter { .. })))
                < at(&|a| matches!(a, Action::PushClipboard(_)))
        );
        let mut later = Vec::new();
        for i in 1..50 {
            later.extend(d.on(moved(PARK.0 + i % 7, PARK.1)));
            later.extend(d.on(key(0x1e, i % 2 == 1)));
        }
        assert_eq!(pushes(&later), Vec::<String>::new(), "not per move or key");
        // The switch key on to the next computer: one push there.
        let next = d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), Some("M"));
        assert_eq!(pushes(&next), ["M"]);
        // And home: none from here.
        let home = d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), None);
        assert_eq!(pushes(&home), Vec::<String>::new());

        // With the setting off, nothing follows.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));
        assert_eq!(pushes(&crossed), Vec::<String>::new());
    }

    #[test]
    fn each_switch_names_the_computer_that_has_them() {
        let mut d = driver(desk(true), &["L", "M"]);
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert!(
            crossed.contains(&Action::Badge(badge::of(&badge::Event::Went {
                to: "L computer"
            }))),
            "{crossed:?}"
        );
        // The switch key on to the next names it.
        let next = d.on(Captured::Hotkey);
        assert!(
            next.contains(&Action::Badge(badge::of(&badge::Event::Went {
                to: "M computer"
            }))),
            "{next:?}"
        );
        // Home by the switch key names this computer, with no why.
        let home = d.on(Captured::Hotkey);
        assert!(home.contains(&home_badge(None)), "{home:?}");
        assert_eq!(
            home_badge(None),
            Action::Badge(badge::Badge {
                text: "Keyboard and mouse: W computer".to_string(),
                fallback: crate::overlay::Fallback::Quiet,
            })
        );
        // Home the owner did not make says why under the name.
        let mut d = away_on_linux(false);
        let locked = d.on(Captured::Locked);
        assert!(
            locked.contains(&home_badge(Some("This computer locked"))),
            "{locked:?}"
        );

        // A peer's name in a reason is shown tame and clipped.
        let mut sly = desk(false);
        for c in &mut sly.computers {
            if c.device_id == "L" {
                c.name = format!("Lap\u{202E}top\n{}", "n".repeat(300));
            }
        }
        let mut d = driver(sly, &["L"]);
        d.start();
        d.on(moved(3440, 480));
        d.on(moved(3455, 480));
        let failed = d.on_send_error();
        let said = failed
            .iter()
            .find_map(|a| match a {
                Action::Badge(b) => Some(b.text.clone()),
                _ => None,
            })
            .expect("a badge");
        let why = said.lines().nth(1).expect("why on a second line");
        assert!(why.starts_with("The link to Lap top n"), "{why}");
        assert!(why.chars().count() < 100, "{why}");
        assert_eq!(said.lines().count(), 2, "{said}");
    }

    #[test]
    fn keys_while_away_are_sent_not_local() {
        // At home, keys are this computer's own: nothing is sent.
        let mut home = driver(desk(false), &["L"]);
        home.start();
        assert_eq!(home.on(key(0x1E, true)), vec![]);
        assert_eq!(
            home.on(Captured::Button {
                button: 0,
                down: true
            }),
            vec![]
        );

        let mut d = away_on_linux(false);
        assert_eq!(d.mode(), Mode::Away, "the input thread swallows them");
        assert_eq!(
            sends(&d.on(key(0x1E, true))),
            vec![sent("L", evdev(30, true))]
        );
        // A held key's repeats are the other computer's to make.
        assert_eq!(sends(&d.on(key(0x1E, true))), vec![]);
        assert_eq!(
            sends(&d.on(key(0x1E, false))),
            vec![sent("L", evdev(30, false))]
        );
        // A key let go of that was never sent down is not sent.
        assert_eq!(sends(&d.on(key(0x1E, false))), vec![]);
        // The E0 prefix tells the arrows from the keypad.
        assert_eq!(
            sends(&d.on(Captured::Key {
                scan: 0x4D,
                extended: true,
                down: true
            })),
            vec![sent("L", evdev(106, true))]
        );
        assert_eq!(
            sends(&d.on(Captured::Button {
                button: 2,
                down: true
            })),
            vec![sent(
                "L",
                Msg::Button {
                    button: 2,
                    pressed: true
                }
            )]
        );
        assert_eq!(
            sends(&d.on(Captured::Wheel { dx: 0, dy: 2 })),
            vec![sent("L", Msg::Wheel { dx: 0, dy: 2 })]
        );
        // A key Windows names but the wire does not is dropped.
        assert_eq!(sends(&d.on(key(0x7F, true))), vec![]);
    }

    #[test]
    fn crossing_back_sends_leave_and_releases() {
        let mut d = away_on_linux(false);
        d.on(key(0x2A, true)); // left Shift
        d.on(Captured::Button {
            button: 0,
            down: true,
        });
        // 20 left from (0, 480) is past the Linux laptop's left edge: back
        // onto the Windows laptop's last column, at the same height.
        let back = d.on(moved(PARK.0 - 20, PARK.1));
        assert_eq!(
            sends(&back),
            vec![
                sent("L", evdev(42, false)),
                sent(
                    "L",
                    Msg::Button {
                        button: 0,
                        pressed: false
                    }
                ),
                sent("L", Msg::Leave),
            ]
        );
        for wanted in [
            Action::SetMode(Mode::Watch),
            Action::HideCursor(false),
            Action::WarpHome(3451, 480),
        ] {
            assert!(back.contains(&wanted), "{wanted:?} in {back:?}");
        }
        // Put back before the mouse is only watched.
        let at = |wanted: &Action| back.iter().position(|a| a == wanted).unwrap();
        assert!(
            at(&Action::WarpHome(3451, 480)) < at(&Action::SetMode(Mode::Watch)),
            "{back:?}"
        );
        assert_eq!(d.away_on(), None);
        assert_eq!(d.mode(), Mode::Watch);
        // Home again, keys are this computer's.
        assert_eq!(d.on(key(0x1E, true)), vec![]);
    }

    #[test]
    fn hotkey_toggles_to_next_and_home() {
        // Two computers: the key goes over and comes back.
        let layout = desk(false);
        let mut d = driver(layout.clone(), &["L"]);
        d.start();
        d.on(moved(500, 500));
        let over = d.on(Captured::Hotkey);
        // To the centre of the Linux laptop's screen.
        let (x, y) = layout.normalized("L", (800, 500));
        assert_eq!(sends(&over), vec![sent("L", Msg::Enter { x, y })]);
        // Parked at the centre of the monitor the cursor was on.
        assert!(over.contains(&Action::ParkCursor(960, 540)), "{over:?}");
        d.on(key(0x1D, true)); // Ctrl, held for the key itself
        let back = d.on(Captured::Hotkey);
        assert_eq!(
            sends(&back),
            vec![sent("L", evdev(29, false)), sent("L", Msg::Leave)]
        );
        assert!(back.contains(&Action::WarpHome(960, 540)), "{back:?}");
        assert_eq!(d.away_on(), None);

        // Three: home, the next, the one after, home.
        let layout = desk(true);
        let mut d = driver(layout.clone(), &["L", "M"]);
        d.start();
        d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), Some("L"));
        let next = d.on(Captured::Hotkey);
        let (x, y) = layout.normalized("M", (640, 400));
        assert_eq!(
            sends(&next),
            vec![sent("L", Msg::Leave), sent("M", Msg::Enter { x, y })]
        );
        assert_eq!(d.away_on(), Some("M"));
        let home = d.on(Captured::Hotkey);
        assert_eq!(sends(&home), vec![sent("M", Msg::Leave)]);
        assert_eq!(d.away_on(), None);

        // Alone, the key says so and goes nowhere.
        let mut alone = Driver::new(
            "W",
            Layout::default_for(placed("W", vec![screen(0, 0, 1920, 1080)]), Vec::new()),
        );
        assert_eq!(alone.start(), vec![]);
        let said = alone.on(Captured::Hotkey);
        assert!(sends(&said).is_empty());
        assert!(matches!(said.as_slice(), [Action::Badge(..)]), "{said:?}");
    }

    #[test]
    fn locked_or_suspended_snaps_back_with_releases() {
        let mut d = away_on_linux(false);
        d.on(key(0x5B, true)); // Win, as Win+L begins
        d.on(Captured::Key {
            scan: 0x5B,
            extended: true,
            down: true,
        });
        let locked = d.on(Captured::Locked);
        assert_eq!(
            sends(&locked),
            vec![sent("L", evdev(125, false)), sent("L", Msg::Leave)]
        );
        assert!(locked.contains(&Action::SetMode(Mode::Off)), "{locked:?}");
        assert!(locked.contains(&Action::HideCursor(false)));
        assert!(locked.contains(&Action::KeepAwake(false)));
        assert_eq!(d.away_on(), None);
        // While locked nothing goes anywhere, by the edge or the key.
        assert_eq!(d.on(moved(3440, 480)), vec![]);
        assert_eq!(d.on(moved(3455, 480)), vec![]);
        assert_eq!(d.on(Captured::Hotkey), vec![]);
        assert_eq!(d.on(Captured::Unlocked), vec![Action::SetMode(Mode::Watch)]);
        // Locked at home is only the mode.
        assert_eq!(d.on(Captured::Locked), vec![Action::SetMode(Mode::Off)]);
    }

    #[test]
    fn send_error_goes_home_without_replay() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        let failed = d.on_send_error();
        assert_eq!(sends(&failed), vec![], "nothing is sent on a failed link");
        for wanted in [
            Action::SetMode(Mode::Watch),
            Action::HideCursor(false),
            // Back to where it left (the edge, a few pixels inside), not the
            // park point.
            Action::WarpHome(3451, 480),
            Action::KeepAwake(false),
        ] {
            assert!(failed.contains(&wanted), "{wanted:?} in {failed:?}");
        }
        assert!(failed.iter().any(|a| matches!(a, Action::Badge(..))));
        assert_eq!(d.away_on(), None);
        // That target is suppressed until its channel reopens: a push at the
        // edge does not cross again.
        d.on(moved(3440, 480));
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        // Once the glue says it is reachable again, it crosses, fresh: the
        // key held before is neither released nor pressed again.
        d.set_reachable(vec!["L".to_string()]);
        d.on(moved(3440, 480));
        let again = d.on(moved(3455, 480));
        assert!(matches!(sends(&again).as_slice(), [(to, Msg::Enter { .. })] if to == "L"));
        assert_eq!(sends(&d.on(key(0x1E, false))), vec![]);
        // Failing at home changes nothing.
        let mut home = driver(desk(false), &["L"]);
        home.start();
        assert_eq!(home.on_send_error(), vec![]);
    }

    #[test]
    fn failed_enter_returns_to_where_it_left() {
        // The crossing is sent, then the glue finds the Enter could not go
        // (the channel refused): the cursor goes back where it left, not to
        // the park point, and the target is suppressed.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert!(
            sends(&crossed)
                .iter()
                .any(|(_, m)| matches!(m, Msg::Enter { .. }))
        );
        let failed = d.on_send_error();
        assert_eq!(sends(&failed), vec![], "nothing is sent on a dead link");
        // The edge pixel it left from (3455), a few pixels inside.
        assert!(failed.contains(&Action::WarpHome(3451, 480)), "{failed:?}");
        assert_eq!(d.away_on(), None);
        // Suppressed: the same push does not cross until reachable again.
        d.on(moved(3440, 480));
        assert_eq!(d.on(moved(3455, 480)), vec![]);
    }

    #[test]
    fn relayed_target_is_not_a_crossing() {
        // The Linux laptop is in the layout but not reachable (its link is
        // relayed, or its channel will not open): the edge is a wall.
        let mut d = driver(desk(false), &[]);
        d.start();
        d.on(moved(3440, 480));
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        // The switch key has nowhere to go either.
        assert!(matches!(
            d.on(Captured::Hotkey).as_slice(),
            [Action::Badge(..)]
        ));
    }

    #[test]
    fn computer_that_did_not_answer_is_not_a_target() {
        // L has screens in the layout but did not answer this run; only M
        // did. The edge toward L is a wall; the switch key reaches M.
        let mut d = driver(desk(true), &["M"]);
        d.start();
        d.on(moved(3440, 480));
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        let over = d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), Some("M"), "{over:?}");
    }

    fn open(id: &str, opened: Instant, direct: bool) -> OpenChannel {
        OpenChannel {
            device_id: id.to_string(),
            opened,
            direct,
        }
    }

    #[test]
    fn a_quiet_direct_computer_stays_reachable() {
        // Opened, answered once a second later; then a minute of nothing
        // but activity notes (which are no answer). Nothing ages it out.
        let opened = Instant::now();
        let heard = HashMap::from([("L".to_string(), opened + Duration::from_secs(1))]);
        assert!(!is_answer(&Msg::Active { idle_ms: 0 }));
        let channels = [open("L", opened, true)];
        assert_eq!(
            reachable(&channels, &heard),
            BTreeSet::from(["L".to_string()])
        );
        // A minute on, with the same map (nothing more heard), still so.
        let later = opened + Duration::from_secs(61);
        assert!(later > opened);
        assert_eq!(reachable(&channels, &heard).len(), 1);
        // A link turned relayed is not.
        assert!(reachable(&[open("L", opened, false)], &heard).is_empty());
    }

    #[test]
    fn a_reopened_channel_needs_a_new_answer() {
        let first = Instant::now();
        let answered = first + Duration::from_secs(1);
        let reopened = first + Duration::from_secs(10);
        let mut heard = HashMap::from([("L".to_string(), answered)]);
        // The answer came before this channel opened: not reachable yet.
        assert!(reachable(&[open("L", reopened, true)], &heard).is_empty());
        // Once it answers on the new channel, it is.
        heard.insert("L".to_string(), reopened + Duration::from_millis(5));
        assert_eq!(reachable(&[open("L", reopened, true)], &heard).len(), 1);
        // Never answered at all: not reachable.
        assert!(reachable(&[open("M", first, true)], &heard).is_empty());
    }

    #[test]
    fn a_peer_drivers_hello_is_not_an_answer() {
        assert!(!is_answer(&Msg::Hello {
            version: crate::shared_input::wire::VERSION,
            name: "L".to_string(),
            screens: vec![screen(0, 0, 1600, 1000)],
        }));
        assert!(!is_answer(&Msg::Enter { x: 0, y: 0 }));
        assert!(!is_answer(&Msg::Leave));
        assert!(is_answer(&Msg::Screens(vec![screen(0, 0, 1600, 1000)])));
    }

    #[test]
    fn desktop_switch_snaps_back_with_releases() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        let back = d.on(Captured::DesktopSwitch);
        assert_eq!(
            sends(&back),
            vec![sent("L", evdev(30, false)), sent("L", Msg::Leave)]
        );
        assert!(back.contains(&Action::SetMode(Mode::Watch)), "{back:?}");
        assert!(back.contains(&Action::HideCursor(false)));
        assert!(back.contains(&Action::KeepAwake(false)));
        assert_eq!(d.away_on(), None);
        // At home it is a no-op.
        assert_eq!(d.on(Captured::DesktopSwitch), vec![]);
    }

    #[test]
    fn virtual_cursor_is_clamped_to_remote_screens() {
        let layout = desk(false);
        let mut d = away_on_linux(false);
        // Far down and far right: the Linux screen's bottom right corner
        // (nothing is right of it, so no crossing).
        let step = d.on(moved(PARK.0 + 9000, PARK.1 + 5000));
        let (x, y) = layout.normalized("L", (1599, 999));
        assert_eq!((x, y), (65535, 65535));
        assert_eq!(sends(&step), vec![sent("L", Msg::Move { seq: 1, x, y })]);
        assert_eq!(d.away_on(), Some("L"));
        // Up past the top: held at the top.
        let step = d.on(moved(PARK.0, PARK.1 - 5000));
        let (x, y) = layout.normalized("L", (1599, 0));
        assert_eq!(sends(&step), vec![sent("L", Msg::Move { seq: 2, x, y })]);
        // Pushing on against the wall moves nothing.
        assert_eq!(sends(&d.on(moved(PARK.0 + 5, PARK.1))), vec![]);
    }

    #[test]
    fn holds_awake_while_away_only() {
        let awake = |actions: &[Action]| {
            actions
                .iter()
                .filter_map(|a| match a {
                    Action::KeepAwake(on) => Some(*on),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mut d = driver(desk(false), &["L"]);
        assert_eq!(awake(&d.start()), Vec::<bool>::new());
        assert_eq!(awake(&d.on(moved(3440, 480))), Vec::<bool>::new());
        assert_eq!(awake(&d.on(moved(3455, 480))), vec![true]);
        assert_eq!(awake(&d.on(moved(PARK.0 + 3, PARK.1))), Vec::<bool>::new());
        assert_eq!(awake(&d.on(moved(PARK.0 - 50, PARK.1))), vec![false]);
        assert_eq!(awake(&d.on(moved(3400, 480))), Vec::<bool>::new());
    }

    #[test]
    fn a_relayed_link_or_a_lost_computer_goes_home_with_releases() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        // Another computer's link is none of this one's business.
        assert_eq!(d.link_gone("M", "relayed"), vec![]);
        let gone = d.link_gone("L", "the link to L goes through a relay");
        assert_eq!(
            sends(&gone),
            vec![sent("L", evdev(30, false)), sent("L", Msg::Leave)]
        );
        assert!(gone.contains(&home_badge(Some("the link to L goes through a relay"))));
        assert_eq!(d.away_on(), None);

        // The computer driven leaves the layout: home too.
        let mut d = away_on_linux(false);
        let alone = Layout::default_for(
            placed(
                "W",
                vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)],
            ),
            Vec::new(),
        );
        let left = d.set_layout(alone);
        assert_eq!(sends(&left), vec![sent("L", Msg::Leave)]);
        assert!(left.contains(&Action::SetMode(Mode::Off)), "{left:?}");
        assert_eq!(d.away_on(), None);
        // And back: there is somewhere to go, so the mouse is watched.
        assert_eq!(
            d.set_layout(desk(false)),
            vec![Action::SetMode(Mode::Watch)]
        );
    }

    #[test]
    fn a_refusing_computer_is_left_with_releases_and_not_crossed_onto() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        // Another computer refusing is none of this one's business.
        assert_eq!(d.refused("M", REFUSE_SHARING_OFF), vec![]);
        let back = d.refused("L", REFUSE_SHARING_OFF);
        assert_eq!(
            sends(&back),
            vec![sent("L", evdev(30, false)), sent("L", Msg::Leave)]
        );
        assert!(back.contains(&home_badge(Some(
            "L computer won't take input: the keyboard and mouse are turned off there"
        ))));
        assert_eq!(d.away_on(), None);
        // Its edge is a wall now, until the glue says it is reachable again.
        d.on(moved(3440, 480));
        let pushed = d.on(moved(3455, 480));
        assert_eq!(sends(&pushed), vec![], "{pushed:?}");
        assert_eq!(d.away_on(), None);
    }

    #[test]
    fn an_offline_computer_keeps_its_place_in_the_row() {
        // L is offline, M linked: with no arrangement M sits where the
        // page draws it, after L, not in L's place.
        let other = |id: &str, screens: Vec<Screen>| ComputerScreens {
            device_id: id.to_string(),
            name: format!("{id} computer"),
            screens,
            changed_unix: 0,
            this_computer: false,
        };
        let file = ScreensFile {
            computers: vec![
                ComputerScreens {
                    this_computer: true,
                    ..other("W", Vec::new())
                },
                other("L", vec![screen(0, 0, 1600, 1000)]),
                other("M", vec![screen(0, 0, 1280, 800)]),
            ],
        };
        let own = vec![screen(0, 0, 1920, 1080)];
        let page = pocketdesk_core::local::shared_input::arrange(
            &own,
            &[
                ("L".to_string(), vec![screen(0, 0, 1600, 1000)]),
                ("M".to_string(), vec![screen(0, 0, 1280, 800)]),
            ],
            None,
        );
        let layout = layout_for("W", "W computer", own, &file, &["M".to_string()], None);
        let ids: Vec<&str> = layout
            .computers
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(ids, ["W", "M"], "only the linked one");
        assert_eq!(layout.computers[1].origin, page[1]);
        assert_eq!(page[1], (1920 + 1600, 0));
    }

    #[test]
    fn the_owners_arrangement_is_used_when_there_is_one() {
        let file = ScreensFile {
            computers: vec![
                ComputerScreens {
                    device_id: "W".to_string(),
                    name: "W computer".to_string(),
                    screens: Vec::new(),
                    changed_unix: 0,
                    this_computer: true,
                },
                ComputerScreens {
                    device_id: "L".to_string(),
                    name: "L computer".to_string(),
                    screens: vec![screen(0, 0, 1600, 1000)],
                    changed_unix: 0,
                    this_computer: false,
                },
            ],
        };
        let own = vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)];
        let linked = ["L".to_string()];
        // The laptop to the left, its bottom level with the monitor's.
        let saved = LayoutFile {
            origins: BTreeMap::from([("L".to_string(), (-1600, 80))]),
            rev: 1,
        };
        let layout = layout_for("W", "W computer", own.clone(), &file, &linked, Some(&saved));
        assert_eq!(layout.computers[1].origin, (-1600, 80));
        // Crossing off the monitor's left edge lands on the laptop's right.
        let crossing = layout.crossing("W", (0, 500), -5, 0).unwrap();
        assert_eq!(crossing.to, "L");
        assert_eq!(crossing.at, (1599, 420));
        // None, or one that would overlap, is the row to the right.
        let overlapping = LayoutFile {
            origins: BTreeMap::from([("L".to_string(), (10, 10))]),
            rev: 2,
        };
        assert_eq!(
            layout_for(
                "W",
                "W computer",
                own.clone(),
                &file,
                &linked,
                Some(&overlapping)
            ),
            desk(false)
        );
        assert_eq!(
            layout_for("W", "W computer", own, &file, &linked, None),
            desk(false)
        );
    }

    #[test]
    fn shutdown_goes_home_with_releases_and_stops_watching() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        let off = d.shutdown();
        assert_eq!(
            sends(&off),
            vec![sent("L", evdev(30, false)), sent("L", Msg::Leave)]
        );
        assert_eq!(off.last(), Some(&Action::SetMode(Mode::Off)));
        assert_eq!(d.mode(), Mode::Off);
    }

    /// What is in front: a full-screen game, or an ordinary window.
    fn in_front(game: bool, kernel_anti_cheat: bool) -> Captured {
        Captured::Foreground {
            fg: Foreground {
                fullscreen: game,
                cursor_captured: false,
                program: if game {
                    "eldenring.exe"
                } else {
                    "explorer.exe"
                }
                .to_string(),
                elevated: false,
                cursor: None,
            },
            kernel_anti_cheat,
        }
    }

    fn modes(actions: &[Action]) -> Vec<Mode> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::SetMode(mode) => Some(*mode),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_game_in_front_takes_the_hooks_out_and_the_edges_lead_nowhere() {
        let mut d = driver(desk(false), &["L"]);
        assert_eq!(d.start(), vec![Action::SetMode(Mode::Watch)]);
        let locked = d.on(in_front(true, false));
        assert_eq!(modes(&locked), vec![Mode::Off], "{locked:?}");
        assert!(locked.iter().any(|a| matches!(a, Action::Badge(..))));
        assert!(d.game_held());
        // Still in front half a second later: nothing more.
        assert_eq!(d.on(in_front(true, false)), vec![]);
        // A move already on its way at the edge crosses nothing.
        d.on(moved(3440, 480));
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        // The game leaves the front: the mouse is watched again.
        let free = d.on(in_front(false, false));
        assert_eq!(modes(&free), vec![Mode::Watch], "{free:?}");
        d.on(moved(3440, 480));
        d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));
    }

    #[test]
    fn a_single_move_after_the_game_leaves_does_not_cross() {
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(500, 500));
        d.on(in_front(true, false));
        assert_eq!(d.mode(), Mode::Off);
        // Unwatched meanwhile, the cursor went to the edge; the first move
        // watched again is measured from nowhere stale.
        assert_eq!(modes(&d.on(in_front(false, false))), vec![Mode::Watch]);
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        // A push from there crosses as ever.
        d.on(moved(3450, 480));
        d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));

        // The same after a lock and unlock.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(500, 500));
        d.on(Captured::Locked);
        d.on(Captured::Unlocked);
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
    }

    #[test]
    fn a_foreground_change_holds_the_edges_until_it_is_looked_at() {
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(3440, 480));
        // A window comes to the front: until the probe answers, an edge
        // scroll does not cross.
        assert_eq!(d.on(Captured::ForegroundChanged), vec![]);
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);
        // It was a game: locked before any move can cross.
        assert_eq!(modes(&d.on(in_front(true, false))), vec![Mode::Off]);
        d.on(moved(3450, 480));
        assert_eq!(sends(&d.on(moved(3455, 480))), vec![]);
        assert_eq!(d.away_on(), None);

        // An ordinary window: the edges work again once it is looked at.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(moved(3440, 480));
        d.on(Captured::ForegroundChanged);
        assert_eq!(d.on(in_front(false, false)), vec![]);
        d.on(moved(3450, 480));
        d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));
    }

    #[test]
    fn a_forced_switch_parks_where_the_cursor_is_in_the_game() {
        let mut d = driver(desk(false), &["L"]);
        d.start();
        // Last watched on the monitor; the game then has the cursor on the
        // laptop's screen, where the probe sees it.
        d.on(moved(500, 500));
        let mut game = in_front(true, false);
        if let Captured::Foreground { fg, .. } = &mut game {
            fg.cursor = Some((3000, 500));
        }
        d.on(game);
        let over = d.on(Captured::Hotkey);
        assert!(
            over.contains(&Action::ParkCursor(PARK.0, PARK.1)),
            "{over:?}"
        );
        assert_eq!(d.away_on(), Some("L"));
    }

    #[test]
    fn a_game_already_in_front_at_start_never_sees_a_hook() {
        // As the loop starts it: the settings, what is in front, then start.
        let mut d = Driver::new("W", desk(false)).with_game_lock(true, vec!["blender".into()]);
        d.set_reachable(vec!["L".to_string()]);
        let mut blender = in_front(false, false);
        if let Captured::Foreground { fg, .. } = &mut blender {
            fg.program = "Blender.exe".to_string();
        }
        assert_eq!(modes(&d.on(blender)), vec![]);
        assert_eq!(d.start(), vec![]);
        assert_eq!(d.mode(), Mode::Off);
    }

    #[test]
    fn a_game_coming_to_the_front_while_away_snaps_back_with_releases() {
        let mut d = away_on_linux(false);
        d.on(key(0x1E, true));
        let back = d.on(in_front(true, false));
        assert_eq!(
            sends(&back),
            vec![sent("L", evdev(30, false)), sent("L", Msg::Leave)]
        );
        // Home with no hooks: the game has the mouse.
        assert_eq!(modes(&back), vec![Mode::Off], "{back:?}");
        assert!(back.contains(&Action::HideCursor(false)));
        assert!(back.contains(&Action::KeepAwake(false)));
        assert_eq!(d.away_on(), None);
    }

    #[test]
    fn the_switch_key_forces_off_a_game_and_the_hooks_go_on_the_way_back() {
        let layout = desk(false);
        let mut d = driver(layout.clone(), &["L"]);
        d.start();
        d.on(moved(500, 500));
        d.on(in_front(true, false));
        assert_eq!(d.mode(), Mode::Off);
        // The switch key (heard with no hooks) takes them to Linux: the hooks
        // are back while away.
        let over = d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), Some("L"), "{over:?}");
        assert_eq!(modes(&over), vec![Mode::Away]);
        let (x, y) = layout.normalized("L", (800, 500));
        assert_eq!(sends(&over), vec![sent("L", Msg::Enter { x, y })]);
        // The game still in front does not snap them back: it was forced.
        assert_eq!(d.on(in_front(true, false)), vec![]);
        assert_eq!(d.away_on(), Some("L"));
        // The key again: home, and the hooks go at once.
        d.on(key(0x1D, true));
        let back = d.on(Captured::Hotkey);
        assert_eq!(
            sends(&back),
            vec![sent("L", evdev(29, false)), sent("L", Msg::Leave)]
        );
        assert_eq!(modes(&back), vec![Mode::Off], "{back:?}");
        assert_eq!(d.away_on(), None);
        // Home, the force is spent: a game coming to the front after an edge
        // crossing snaps back again.
        d.on(in_front(false, false));
        d.on(moved(3440, 480));
        d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));
        d.on(in_front(true, false));
        assert_eq!(d.away_on(), None);

        // Crossing back by the edge from a forced switch is home too, hooks
        // out.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(in_front(true, false));
        d.on(Captured::Hotkey);
        let park = (960, 540);
        let back = d.on(moved(park.0 - 900, park.1));
        assert_eq!(d.away_on(), None, "{back:?}");
        assert_eq!(modes(&back), vec![Mode::Off]);
    }

    #[test]
    fn the_switch_key_refused_mid_game_says_why_and_stays() {
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(in_front(true, true));
        let refused = d.on(Captured::Hotkey);
        assert_eq!(refused, vec![note(game_lock::ANTI_CHEAT_REFUSED)]);
        assert_eq!(d.away_on(), None);
        assert_eq!(d.mode(), Mode::Off);

        // An elevated game in front: refused with the way to lift it.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(Captured::Foreground {
            fg: Foreground {
                fullscreen: true,
                cursor_captured: false,
                program: "game.exe".to_string(),
                elevated: true,
                cursor: None,
            },
            kernel_anti_cheat: false,
        });
        let refused = d.on(Captured::Hotkey);
        assert_eq!(refused, vec![note(game_lock::ELEVATED_REFUSED)]);
        assert_eq!(d.away_on(), None);

        // An anti-cheat starting while forced away brings them home.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.on(in_front(true, false));
        d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), Some("L"));
        let back = d.on(in_front(true, true));
        assert_eq!(sends(&back), vec![sent("L", Msg::Leave)]);
        assert!(back.contains(&home_badge(Some(game_lock::ANTI_CHEAT_REFUSED))));
        assert_eq!(d.away_on(), None);
        assert_eq!(d.mode(), Mode::Off);
    }

    #[test]
    fn the_lock_turned_off_never_locks() {
        let mut d = driver(desk(false), &["L"]);
        d.start();
        assert_eq!(modes(&d.set_game_lock(false, Vec::new())), vec![]);
        assert_eq!(modes(&d.on(in_front(true, true))), vec![]);
        assert_eq!(d.mode(), Mode::Watch);
        // Away by the edge, a game coming to the front leaves them there.
        d.on(moved(3440, 480));
        d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"));
        assert_eq!(sends(&d.on(in_front(true, true))), vec![]);
        assert_eq!(d.away_on(), Some("L"));
        // Turned on again at home, the game in front locks at once.
        d.on(Captured::Hotkey);
        assert_eq!(d.away_on(), None);
        assert_eq!(modes(&d.set_game_lock(true, Vec::new())), vec![Mode::Off]);
        // A program on the list locks only while in front.
        let mut d = driver(desk(false), &["L"]);
        d.start();
        d.set_game_lock(true, vec!["Blender".to_string()]);
        let mut blender = in_front(false, false);
        if let Captured::Foreground { fg, .. } = &mut blender {
            fg.program = "blender.exe".to_string();
        }
        assert_eq!(modes(&d.on(blender)), vec![Mode::Off]);
        assert_eq!(modes(&d.on(in_front(false, false))), vec![Mode::Watch]);
    }

    #[test]
    fn the_layout_puts_linked_computers_with_screens_to_the_right() {
        let other = |id: &str, screens: Vec<Screen>| ComputerScreens {
            device_id: id.to_string(),
            name: format!("{id} computer"),
            screens,
            changed_unix: 0,
            this_computer: false,
        };
        let file = ScreensFile {
            computers: vec![
                ComputerScreens {
                    this_computer: true,
                    ..other("W", vec![screen(0, 0, 10, 10)])
                },
                other("gone", vec![screen(0, 0, 800, 600)]),
                other("L", vec![screen(0, 0, 1600, 1000)]),
                other("blank", Vec::new()),
            ],
        };
        let linked = ["L".to_string(), "blank".to_string()];
        let own = vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)];
        let layout = layout_for("W", "W computer", own, &file, &linked, None);
        // This computer's own screens as read now (not the file's), and only
        // the linked computer that has screens, to their right, after the
        // place of the one not linked now (as the page draws them).
        let mut expected = desk(false);
        expected.computers[1].origin = (3456 + 800, 0);
        assert_eq!(layout, expected);
    }
}
