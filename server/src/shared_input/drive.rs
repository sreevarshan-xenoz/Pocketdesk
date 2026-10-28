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

use super::held::HeldInput;
use super::keys_windows::evdev_from_scan;
use super::layout::{Layout, Placed};
use super::wire::{Msg, Screen};

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

/// What the input thread saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Badge(String),
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
        }
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
                    self.go_home(
                        &mut out,
                        park,
                        true,
                        "This computer locked: the keyboard and mouse are back here",
                    );
                }
                self.settle(&mut out);
            }
            Captured::Unlocked => {
                self.locked = false;
                self.settle(&mut out);
            }
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
        if let Some(away) = &self.away {
            let (park, to) = (away.park, self.name_of(&away.to));
            self.go_home(
                &mut out,
                park,
                false,
                &format!("The link to {to} failed: the keyboard and mouse are back here"),
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
            self.go_home(&mut out, park, true, why);
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
                self.go_home(
                    &mut out,
                    away.park,
                    true,
                    &format!("{to} is gone: the keyboard and mouse are back here"),
                );
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
            self.go_home(&mut out, park, true, "The keyboard and mouse are back here");
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
        } else if self.locked || self.targets().is_empty() {
            Mode::Off
        } else {
            Mode::Watch
        }
    }

    /// Tell the input thread the mode, if it changed.
    fn settle(&mut self, out: &mut Vec<Action>) {
        let wanted = self.wanted_mode();
        if wanted != self.mode {
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

    fn name_of(&self, device_id: &str) -> String {
        self.layout
            .computers
            .iter()
            .find(|c| c.device_id == device_id)
            .map_or_else(|| device_id.to_string(), |c| c.name.clone())
    }

    /// At home: the cursor moved to `pos`; off an edge onto another
    /// computer, it goes there.
    fn watch(&mut self, pos: (i32, i32), out: &mut Vec<Action>) {
        let Some(prev) = self.home.replace(pos) else {
            return;
        };
        let (dx, dy) = (pos.0.saturating_sub(prev.0), pos.1.saturating_sub(prev.1));
        // A pointer pushed past the edge is on its last pixel.
        let at = self.layout.clamp(&self.this, pos);
        if let Some(crossing) = self.layout.crossing(&self.this, at, dx, dy)
            && crossing.to != self.this
        {
            let park = self.park_for(at);
            self.go_away(out, &crossing.to, crossing.at, park);
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
                self.go_home(
                    out,
                    crossing.at,
                    true,
                    "The keyboard and mouse are back here",
                );
            }
            Some(crossing) => {
                self.leave(out);
                self.go_away(out, &crossing.to, crossing.at, away.park);
            }
            None => {
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
    /// the next, the last comes home.
    fn hotkey(&mut self, out: &mut Vec<Action>) {
        let targets: Vec<String> = self.targets().iter().map(|c| c.device_id.clone()).collect();
        let next = match &self.away {
            None => targets.first(),
            Some(away) => targets
                .iter()
                .position(|t| *t == away.to)
                .and_then(|i| targets.get(i + 1)),
        }
        .cloned();
        match (next, self.away.clone()) {
            (None, None) => out.push(Action::Badge(
                "No other computer to move the keyboard and mouse to".to_string(),
            )),
            (None, Some(away)) => {
                self.go_home(out, away.park, true, "The keyboard and mouse are back here");
            }
            (Some(next), away) => {
                let park = match &away {
                    Some(away) => {
                        self.leave(out);
                        away.park
                    }
                    None => {
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
                self.go_away(out, &next, at, park);
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
    fn go_away(&mut self, out: &mut Vec<Action>, to: &str, at: (i32, i32), park: (i32, i32)) {
        let was_away = self.away.is_some();
        let (x, y) = self.layout.normalized(to, at);
        self.away = Some(Away {
            to: to.to_string(),
            at,
            park,
        });
        // Parked before the mouse is swallowed, so no movement is measured
        // from where the cursor was before.
        out.push(Action::ParkCursor(park.0, park.1));
        self.settle(out);
        out.push(Action::Send(to.to_string(), Msg::Enter { x, y }));
        if !was_away {
            out.push(Action::HideCursor(true));
            out.push(Action::KeepAwake(true));
        }
        out.push(Action::Badge(format!(
            "The keyboard and mouse are on {}",
            self.name_of(to)
        )));
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
    /// is sent and what was held is forgotten.
    fn go_home(&mut self, out: &mut Vec<Action>, at: (i32, i32), releases: bool, why: &str) {
        if releases {
            self.leave(out);
        } else {
            drop(self.held.releases());
        }
        self.away = None;
        self.home = Some(at);
        // Put back before the mouse is only watched again, so the first
        // position watched is where it came home.
        out.push(Action::WarpHome(at.0, at.1));
        self.settle(out);
        out.push(Action::HideCursor(false));
        out.push(Action::KeepAwake(false));
        out.push(Action::Badge(why.to_string()));
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

/// The layout this computer drives in: its own screens, and to their right
/// the linked computers whose screens `shared_screens.json` holds, in its
/// order (until the owner arranges them).
pub fn layout_for(
    this: &str,
    name: &str,
    own: Vec<Screen>,
    file: &pocketdesk_core::local::shared_input::ScreensFile,
    linked: &[String],
) -> Layout {
    let others = file
        .computers
        .iter()
        .filter(|c| {
            !c.this_computer
                && c.device_id != this
                && !c.screens.is_empty()
                && linked.contains(&c.device_id)
        })
        .map(|c| Placed {
            device_id: c.device_id.clone(),
            name: c.name.clone(),
            origin: (0, 0),
            screens: c.screens.clone(),
        })
        .collect();
    Layout::default_for(
        Placed {
            device_id: this.to_string(),
            name: name.to_string(),
            origin: (0, 0),
            screens: own,
        },
        others,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::shared_input::{ComputerScreens, ScreensFile};

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

    /// A driver whose cursor has just crossed onto the Linux laptop at
    /// (0, 480).
    fn away_on_linux(third: bool) -> Driver {
        let mut d = Driver::new("W", desk(third));
        d.start();
        d.on(moved(3440, 480));
        let crossed = d.on(moved(3455, 480));
        assert_eq!(d.away_on(), Some("L"), "{crossed:?}");
        d
    }

    #[test]
    fn crossing_sends_enter_then_moves() {
        let layout = desk(false);
        let mut d = Driver::new("W", layout.clone());
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

    #[test]
    fn keys_while_away_are_sent_not_local() {
        // At home, keys are this computer's own: nothing is sent.
        let mut home = Driver::new("W", desk(false));
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
            Action::WarpHome(3455, 480),
        ] {
            assert!(back.contains(&wanted), "{wanted:?} in {back:?}");
        }
        // Put back before the mouse is only watched.
        let at = |wanted: &Action| back.iter().position(|a| a == wanted).unwrap();
        assert!(
            at(&Action::WarpHome(3455, 480)) < at(&Action::SetMode(Mode::Watch)),
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
        let mut d = Driver::new("W", layout.clone());
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
        let mut d = Driver::new("W", layout.clone());
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
        assert!(matches!(said.as_slice(), [Action::Badge(_)]), "{said:?}");
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
            Action::WarpHome(PARK.0, PARK.1),
            Action::KeepAwake(false),
        ] {
            assert!(failed.contains(&wanted), "{wanted:?} in {failed:?}");
        }
        assert!(failed.iter().any(|a| matches!(a, Action::Badge(_))));
        assert_eq!(d.away_on(), None);
        // Crossing again starts fresh: the key held before is neither
        // released nor pressed again.
        d.on(moved(3440, 480));
        let again = d.on(moved(3455, 480));
        assert!(matches!(sends(&again).as_slice(), [(to, Msg::Enter { .. })] if to == "L"));
        assert_eq!(sends(&d.on(key(0x1E, false))), vec![]);
        // Failing at home changes nothing.
        let mut home = Driver::new("W", desk(false));
        home.start();
        assert_eq!(home.on_send_error(), vec![]);
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
        let mut d = Driver::new("W", desk(false));
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
        assert!(gone.contains(&Action::Badge(
            "the link to L goes through a relay".to_string()
        )));
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
        let layout = layout_for("W", "W computer", own, &file, &linked);
        // This computer's own screens as read now (not the file's), and only
        // the linked computer that has screens, to their right.
        assert_eq!(layout, desk(false));
    }
}
