//! Linux driving (BK8): what the input-capture portal and its libei stream
//! say, turned into what the [`Driver`] takes, and what the driver wants
//! turned into what is asked of the portal, through a [`Backend`] seam.
//!
//! The driver is the same one Windows' hooks feed. Here it is told:
//! - at home nothing moves it: the compositor watches this computer's edges
//!   as pointer barriers ([`barriers`]), set and enabled while the driver
//!   watches ([`Mode::Watch`]) and disabled while it wants nothing watched
//!   ([`Mode::Off`]: a game in front, the lock, no other computer);
//! - a barrier hit (`Activated`) is a push off that edge
//!   ([`Captured::Edge`]); one that leads nowhere now is let go of at once,
//!   and an edge that flips three times in a second is a wall
//!   ([`FlipBreaker`], xdg-desktop-portal-hyprland #435);
//! - away, libei's relative motion is a move from the parked point (the
//!   driver's park is where its virtual cursor is measured from; the
//!   compositor holds and hides the real cursor), its buttons, wheel and
//!   keys (evdev codes, the switch key spotted by [`Keys`]) go on;
//! - home is `Release` with where the cursor lands ([`Action::WarpHome`]);
//! - the desktop ending the capture by itself (`Deactivated`, `Disabled`,
//!   the libei stream closing) brings the keyboard and mouse home
//!   ([`Captured::CaptureLost`]), as the driver's one-second watchdog does.
//!
//! The portal's zones are this computer's screens while it is used
//! ([`PortalGlue::zones`]). Pure: the real backend (ashpd and reis) is
//! `capture_linux`'s; the tests use a stand-in.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use pocketdesk_core::local::hotkey::Hotkey;

use super::badge;
use super::barriers::{self, Barrier, Flip, FlipBreaker};
use super::drive::{Action, Captured, Driver, Mode};
use super::keys_evdev::Keys;
use super::layout::Layout;
use super::wire::Screen;

/// What the portal and its libei stream said.
#[derive(Debug, Clone, PartialEq)]
pub enum Portal {
    /// How the portal stands now, for the owner.
    Standing(Standing),
    /// This computer's zones (its screens, logical), at the start and each
    /// time they change.
    Zones(Vec<Screen>),
    /// The capture began: the pointer hit `barrier` (if the portal could
    /// tell which) at `at` (in the zones' space, if it said).
    Activated {
        activation: Option<u32>,
        barrier: Option<u32>,
        at: Option<(f64, f64)>,
    },
    /// The capture `activation` ended without this computer letting go.
    Deactivated { activation: Option<u32> },
    /// The session captures nothing until it is enabled again.
    Disabled,
    /// The pointer moved by this much, logical pixels.
    Motion { dx: f64, dy: f64 },
    /// A button, as Linux's `BTN_*` code.
    Button { code: u32, down: bool },
    /// One frame's scroll: smooth (`dx`, `dy`, pixels) and, from a wheel,
    /// in 120ths of a notch (`discrete`), which counts when there is one.
    Scroll {
        dx: f64,
        dy: f64,
        discrete: Option<(i32, i32)>,
    },
    /// A key, as its evdev code.
    Key { code: u32, down: bool },
    /// The session or its libei stream is gone (why, for the log).
    Closed(String),
}

/// How the input-capture portal stands, as the desktop app says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Standing {
    /// Not started yet.
    #[default]
    Starting,
    /// Its permission dialog is up, or was refused (it asks again at the
    /// next start).
    NeedsPermission,
    /// This desktop has no input-capture portal (GNOME 45, Plasma 6.1 and
    /// xdg-desktop-portal-hyprland 1.4 have one).
    Unsupported,
    /// Capturing as asked.
    Ready,
    /// An edge kept flipping and is a wall (fractional scaling on
    /// Hyprland).
    FractionalScale,
}

impl Standing {
    /// How it is said in the status file (empty: nothing to say yet).
    pub fn word(self) -> &'static str {
        use pocketdesk_core::local::shared_input as words;
        match self {
            Standing::Starting => "",
            Standing::NeedsPermission => words::CAPTURE_NEEDS_PERMISSION,
            Standing::Unsupported => words::CAPTURE_UNSUPPORTED,
            Standing::Ready => words::CAPTURE_READY,
            Standing::FractionalScale => words::CAPTURE_FRACTIONAL_SCALE,
        }
    }
}

/// What is asked of the portal.
pub trait Backend {
    /// Watch these barriers (set, and the capture enabled again); none
    /// disables it.
    fn arm(&mut self, barriers: &[Barrier]);
    /// Watch nothing: the capture disabled.
    fn disarm(&mut self);
    /// Let go of the capture `activation`, the cursor put at `at`
    /// (logical, in the zones' space; the compositor may put it elsewhere).
    fn release(&mut self, activation: Option<u32>, at: (i32, i32));
}

/// Pixels a release lands inside the edge it was taken at.
const INSIDE: i32 = 8;

/// How long input may keep coming with no capture here before it is let
/// go of again (libei can run a little ahead of the portal's signals), and
/// how often it is let go of again after that.
const STRAY_GRACE: Duration = Duration::from_millis(300);

/// A pause in such input this long starts its count over.
const STRAY_GAP: Duration = Duration::from_secs(1);

/// Smooth scroll, in pixels, that makes one notch of a wheel.
const SMOOTH_PER_NOTCH: f64 = 15.0;

/// A wheel's notch in libei's discrete scroll.
const DISCRETE_PER_NOTCH: i32 = 120;

/// The words said once when an edge turns into a wall.
pub const WALL_WORDS: &str = "An edge stopped leading to the next computer: the desktop kept \
     handing the pointer back across it (fractional scaling on Hyprland, \
     xdg-desktop-portal-hyprland bug 435). A screen scale of 1 there crosses again";

/// The portal's side of the driver.
pub struct PortalGlue<B: Backend> {
    backend: B,
    this: String,
    /// This computer's zones, once the portal said them.
    zones: Option<Vec<Screen>>,
    /// The layout and the computers reachable, as last given.
    layout: Layout,
    reachable: HashSet<String>,
    /// The barriers set now, if the capture is enabled.
    armed: Option<Vec<Barrier>>,
    /// The desktop disabled the capture: enabled again only when asked
    /// ([`PortalGlue::rearm`]), not at once.
    disabled: bool,
    /// The driver's mode, as last told.
    mode: Mode,
    /// The capture now, while the keyboard and mouse are taken.
    captured: Option<Option<u32>>,
    /// Where the driver measures moves from while away.
    park: (i32, i32),
    /// What is left of motion and scroll smaller than a whole pixel or
    /// notch.
    motion: (f64, f64),
    smooth: (f64, f64),
    discrete: (i32, i32),
    keys: Keys,
    breaker: FlipBreaker,
    standing: Standing,
    closed: Option<String>,
    /// Input coming with no capture here: since when, and when last.
    stray: Option<(Instant, Instant)>,
    /// When such input was last let go of again.
    rereleased: Option<Instant>,
    /// Where the cursor last came home.
    home: Option<(i32, i32)>,
}

impl<B: Backend> PortalGlue<B> {
    /// The glue for this computer (`this`), asking `backend`, with the
    /// switch key `hotkey`.
    pub fn new(this: &str, backend: B, hotkey: Option<Hotkey>) -> Self {
        PortalGlue {
            backend,
            this: this.to_string(),
            zones: None,
            layout: Layout::default(),
            reachable: HashSet::new(),
            armed: None,
            disabled: false,
            mode: Mode::Off,
            captured: None,
            park: (0, 0),
            motion: (0.0, 0.0),
            smooth: (0.0, 0.0),
            discrete: (0, 0),
            keys: Keys::new(hotkey),
            breaker: FlipBreaker::default(),
            standing: Standing::Starting,
            closed: None,
            stray: None,
            rereleased: None,
            home: None,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// This computer's screens as the portal has them, once it said.
    pub fn zones(&self) -> Option<&[Screen]> {
        self.zones.as_deref()
    }

    pub fn standing(&self) -> Standing {
        self.standing
    }

    /// Why the session ended, once it did: the loop ends with it.
    pub fn closed(&self) -> Option<&str> {
        self.closed.as_deref()
    }

    /// The keyboard and mouse are taken now.
    pub fn captured(&self) -> bool {
        self.captured.is_some()
    }

    /// The layout (built on the zones) and the computers reachable now:
    /// the barriers follow them, set again if watching (never mid-capture:
    /// setting barriers ends it).
    pub fn set_targets(&mut self, layout: &Layout, reachable: &[String]) {
        self.layout = layout.clone();
        self.reachable = reachable.iter().cloned().collect();
        self.arm();
    }

    /// The capture enabled again if the driver watches and it is not (the
    /// desktop disabled it): the loop asks every few seconds.
    pub fn rearm(&mut self) {
        self.disabled = false;
        self.arm();
    }

    /// What the portal said: the driver told, and what it wants done here
    /// done; the rest (sends, the badge, the keep-awake) is returned.
    pub fn portal(&mut self, event: Portal, driver: &mut Driver, now: Instant) -> Vec<Action> {
        if self.captured.is_none()
            && matches!(
                event,
                Portal::Motion { .. }
                    | Portal::Button { .. }
                    | Portal::Scroll { .. }
                    | Portal::Key { .. }
            )
        {
            self.stray(now);
            return Vec::new();
        }
        match event {
            Portal::Standing(standing) => {
                self.standing = standing;
                Vec::new()
            }
            Portal::Zones(zones) => {
                if self.zones.as_ref() != Some(&zones) {
                    // New screens: every edge is tried again.
                    self.breaker.clear();
                    self.zones = Some(zones);
                }
                if self.standing == Standing::Starting || self.standing == Standing::NeedsPermission
                {
                    self.standing = Standing::Ready;
                }
                Vec::new()
            }
            Portal::Activated {
                activation,
                barrier,
                at,
            } => self.activated(activation, barrier, at, driver, now),
            Portal::Deactivated { activation } => {
                match self.captured {
                    // A stale activation's end is no news.
                    Some(Some(now_id)) if activation.is_some_and(|id| id != now_id) => Vec::new(),
                    Some(_) => self.lost(driver),
                    None => Vec::new(),
                }
            }
            Portal::Disabled => {
                self.armed = None;
                self.disabled = true;
                self.lost(driver)
            }
            Portal::Closed(why) => {
                self.armed = None;
                self.closed = Some(why);
                self.lost(driver)
            }
            Portal::Motion { dx, dy } => {
                if self.captured.is_none() || driver.away_on().is_none() {
                    return Vec::new();
                }
                let (wx, wy) = whole(&mut self.motion, (dx, dy));
                if wx == 0 && wy == 0 {
                    return Vec::new();
                }
                let moved = Captured::Moved {
                    x: self.park.0.saturating_add(wx),
                    y: self.park.1.saturating_add(wy),
                };
                self.tell(driver, moved)
            }
            Portal::Button { code, down } => match (self.captured, button_of(code)) {
                (Some(_), Some(button)) => self.tell(driver, Captured::Button { button, down }),
                _ => Vec::new(),
            },
            Portal::Scroll { dx, dy, discrete } => {
                if self.captured.is_none() {
                    return Vec::new();
                }
                let (nx, ny) = match discrete {
                    Some((x, y)) => {
                        self.discrete.0 = self.discrete.0.saturating_add(x);
                        self.discrete.1 = self.discrete.1.saturating_add(y);
                        let n = (
                            self.discrete.0 / DISCRETE_PER_NOTCH,
                            self.discrete.1 / DISCRETE_PER_NOTCH,
                        );
                        self.discrete.0 -= n.0 * DISCRETE_PER_NOTCH;
                        self.discrete.1 -= n.1 * DISCRETE_PER_NOTCH;
                        n
                    }
                    None => {
                        let (x, y) = whole(
                            &mut self.smooth,
                            (dx / SMOOTH_PER_NOTCH, dy / SMOOTH_PER_NOTCH),
                        );
                        (x, y)
                    }
                };
                if nx == 0 && ny == 0 {
                    return Vec::new();
                }
                let notch = |n: i32| n.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
                self.tell(
                    driver,
                    Captured::Wheel {
                        dx: notch(nx),
                        dy: notch(ny),
                    },
                )
            }
            Portal::Key { code, down } => {
                if self.captured.is_none() {
                    return Vec::new();
                }
                let Ok(code) = u16::try_from(code) else {
                    return Vec::new();
                };
                match self.keys.key(code, down) {
                    Some(captured) => self.tell(driver, captured),
                    None => Vec::new(),
                }
            }
        }
    }

    /// Input came with no capture here: libei only sends it while the
    /// desktop captures, so if it keeps coming past the grace the desktop
    /// still holds this computer's keyboard and mouse (a release it ignored
    /// or that failed, an activation that never came): let go of them
    /// again, where the cursor last came home.
    fn stray(&mut self, now: Instant) {
        let first = match self.stray {
            Some((first, last)) if now.saturating_duration_since(last) < STRAY_GAP => first,
            _ => now,
        };
        self.stray = Some((first, now));
        if now.saturating_duration_since(first) < STRAY_GRACE
            || self
                .rereleased
                .is_some_and(|at| now.saturating_duration_since(at) < STRAY_GRACE)
        {
            return;
        }
        // Said once a spell, not at each try.
        if self.rereleased.is_none_or(|at| at < first) {
            tracing::warn!(
                "Shared input: the desktop still sends input with nothing captured here: letting go of it again"
            );
        }
        self.rereleased = Some(now);
        let home = self.home.unwrap_or_else(|| {
            self.zones
                .as_deref()
                .and_then(|zones| zones.first())
                .map_or((0, 0), |z| {
                    let half = |len: u32| i32::try_from(len / 2).unwrap_or(0);
                    (
                        z.x.saturating_add(half(z.width)),
                        z.y.saturating_add(half(z.height)),
                    )
                })
        });
        self.backend.release(None, home);
        self.keys.switched();
    }

    /// Tell the driver `captured`, doing here what it wants done here.
    fn tell(&mut self, driver: &mut Driver, captured: Captured) -> Vec<Action> {
        let actions = driver.on(captured);
        self.act(actions)
    }

    /// The pointer hit a barrier: across it to the computer past it, or
    /// let go of at once if it leads nowhere now (or keeps flipping).
    fn activated(
        &mut self,
        activation: Option<u32>,
        barrier: Option<u32>,
        at: Option<(f64, f64)>,
        driver: &mut Driver,
        now: Instant,
    ) -> Vec<Action> {
        let mut out = Vec::new();
        self.stray = None;
        // What was down before the capture is not this computer's to send.
        self.keys.switched();
        self.reset_rests();
        if self.captured.is_some() && driver.away_on().is_some() {
            // A new activation mid-capture: the same capture goes on.
            self.captured = Some(activation);
            return out;
        }
        self.captured = Some(activation);
        let hit = self
            .armed
            .as_deref()
            .and_then(|armed| barriers::activated(armed, barrier, at))
            .cloned();
        let Some(hit) = hit.filter(|_| self.mode == Mode::Watch) else {
            // Nothing armed that it could be (a stale arming, a game in
            // front): let go where it is.
            let at = at.map_or((0, 0), |(x, y)| (x as i32, y as i32));
            self.let_go(at);
            return out;
        };
        let push = hit.push(at);
        if let Flip::Wall { first } = self.breaker.activated(hit.key(), now) {
            if first {
                tracing::warn!(
                    "Shared input: an edge flips between capturing and not (xdph #435, \
                     fractional scaling?): it is a wall now"
                );
                out.push(Action::Badge(badge::of(&badge::Event::Note(WALL_WORDS))));
            }
            self.standing = Standing::FractionalScale;
            self.let_go(inside(&push));
            // Set again without it.
            self.arm();
            return out;
        }
        out.extend(self.tell(driver, push.clone()));
        if driver.away_on().is_none() {
            // It led nowhere (that computer stopped answering, or a game):
            // the pointer back where it was.
            if self.captured.is_some() {
                self.let_go(inside(&push));
            }
        }
        out
    }

    /// The desktop ended the capture by itself: the keyboard and mouse
    /// come home (nothing to let go of, it is gone).
    fn lost(&mut self, driver: &mut Driver) -> Vec<Action> {
        if self.captured.take().is_none() {
            return Vec::new();
        }
        let held = self.keys.switched();
        self.reset_rests();
        let mut out = Vec::new();
        // Keys held as it ended are let go of there before the leave.
        for code in held {
            out.extend(self.tell(driver, Captured::EvKey { code, down: false }));
        }
        out.extend(self.tell(driver, Captured::CaptureLost));
        out
    }

    /// Let go of the capture, the cursor at `at`.
    fn let_go(&mut self, at: (i32, i32)) {
        self.home = Some(at);
        if let Some(activation) = self.captured.take() {
            self.backend.release(activation, at);
        }
        self.keys.switched();
        self.reset_rests();
    }

    fn reset_rests(&mut self) {
        self.motion = (0.0, 0.0);
        self.smooth = (0.0, 0.0);
        self.discrete = (0, 0);
    }

    /// Set the barriers there should be now, if they changed: while the
    /// driver watches and nothing is captured; none, disabled.
    fn arm(&mut self) {
        if self.mode != Mode::Watch
            || self.captured.is_some()
            || self.zones.is_none()
            || self.disabled
            || self.closed.is_some()
        {
            return;
        }
        let wanted = barriers::barriers(
            &self.layout,
            &self.this,
            |to| self.reachable.contains(to),
            self.breaker.walls(),
        );
        if wanted.is_empty() {
            self.disarm();
        } else if self.armed.as_ref() != Some(&wanted) {
            self.backend.arm(&wanted);
            self.armed = Some(wanted);
        }
    }

    fn disarm(&mut self) {
        if self.armed.take().is_some() {
            self.backend.disarm();
        }
    }

    /// Do what the driver wants done to this computer's own input (its
    /// mode, the park, home); the rest is returned for the loop.
    pub fn act(&mut self, actions: Vec<Action>) -> Vec<Action> {
        let mut rest = Vec::new();
        for action in actions {
            match action {
                Action::SetMode(mode) => {
                    self.mode = mode;
                    match mode {
                        Mode::Watch => self.arm(),
                        Mode::Off => self.disarm(),
                        Mode::Away => {}
                    }
                }
                Action::ParkCursor(x, y) => {
                    if self.park != (x, y) {
                        self.park = (x, y);
                        self.motion = (0.0, 0.0);
                    }
                }
                Action::WarpHome(x, y) => self.let_go((x, y)),
                // The compositor hides the cursor while it captures.
                Action::HideCursor(_) => {}
                other => rest.push(other),
            }
        }
        rest
    }
}

/// Whole units of `rest + d`, keeping what is left over in `rest`.
fn whole(rest: &mut (f64, f64), d: (f64, f64)) -> (i32, i32) {
    let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
    rest.0 += finite(d.0);
    rest.1 += finite(d.1);
    let w = (rest.0.trunc(), rest.1.trunc());
    rest.0 -= w.0;
    rest.1 -= w.1;
    let to_i32 = |v: f64| v.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    (to_i32(w.0), to_i32(w.1))
}

/// A few pixels back inside from where a push was taken.
fn inside(push: &Captured) -> (i32, i32) {
    match push {
        Captured::Edge { x, y, dx, dy } => (
            x.saturating_sub(dx.saturating_mul(INSIDE)),
            y.saturating_sub(dy.saturating_mul(INSIDE)),
        ),
        _ => (0, 0),
    }
}

/// The wire's button (0 left, 1 middle, 2 right, 3 back, 4 forward) for
/// Linux's `BTN_*` code.
pub fn button_of(code: u32) -> Option<u8> {
    match code {
        0x110 => Some(0),         // BTN_LEFT
        0x111 => Some(2),         // BTN_RIGHT
        0x112 => Some(1),         // BTN_MIDDLE
        0x113 | 0x116 => Some(3), // BTN_SIDE, BTN_BACK
        0x114 | 0x115 => Some(4), // BTN_EXTRA, BTN_FORWARD
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_input::drive::ANSWER_WITHIN;
    use crate::shared_input::layout::Placed;
    use crate::shared_input::wire::Msg;
    use pocketdesk_core::local::hotkey::parse;
    use std::time::Duration;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        /// The barriers' lines and where they lead.
        Arm(Vec<((i32, i32, i32, i32), String)>),
        Disarm,
        Release(Option<u32>, (i32, i32)),
    }

    #[derive(Default)]
    struct Fake {
        calls: Vec<Call>,
    }

    impl Backend for Fake {
        fn arm(&mut self, barriers: &[Barrier]) {
            self.calls.push(Call::Arm(
                barriers.iter().map(|b| (b.line, b.to.clone())).collect(),
            ));
        }
        fn disarm(&mut self) {
            self.calls.push(Call::Disarm);
        }
        fn release(&mut self, activation: Option<u32>, at: (i32, i32)) {
            self.calls.push(Call::Release(activation, at));
        }
    }

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    /// The Linux laptop ("L", one 1600x1000 zone) driving, Windows ("W")
    /// to its right and the phone ("P") below it.
    fn desk() -> Layout {
        let placed = |id: &str, origin, screens| Placed {
            device_id: id.to_string(),
            name: id.to_string(),
            origin,
            screens,
        };
        Layout {
            computers: vec![
                placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)]),
                placed("W", (1600, 0), vec![screen(0, 0, 1920, 1080)]),
                placed("P", (400, 1000), vec![screen(0, 0, 400, 800)]),
            ],
        }
    }

    const RIGHT: (i32, i32, i32, i32) = (1600, 0, 1600, 999);
    const BOTTOM: (i32, i32, i32, i32) = (400, 1000, 799, 1000);
    /// The centre of L's screen, where the driver parks.
    const PARK: (i32, i32) = (800, 500);

    struct Rig {
        glue: PortalGlue<Fake>,
        driver: Driver,
        now: Instant,
    }

    impl Rig {
        fn new(reachable: &[&str]) -> Rig {
            let reachable: Vec<String> = reachable.iter().map(|s| s.to_string()).collect();
            let mut driver = Driver::new("L", desk());
            driver.set_reachable(reachable.clone());
            let mut glue = PortalGlue::new("L", Fake::default(), parse("ctrl+alt+right"));
            let now = Instant::now();
            glue.portal(
                Portal::Zones(vec![screen(0, 0, 1600, 1000)]),
                &mut driver,
                now,
            );
            glue.set_targets(&desk(), &reachable);
            let start = driver.start();
            let rest = glue.act(start);
            assert_eq!(rest, vec![]);
            Rig { glue, driver, now }
        }

        fn say(&mut self, event: Portal) -> Vec<Action> {
            self.glue.portal(event, &mut self.driver, self.now)
        }

        fn calls(&mut self) -> Vec<Call> {
            std::mem::take(&mut self.glue.backend.calls)
        }

        fn hit(&mut self, activation: u32, barrier: u32, at: (f64, f64)) -> Vec<Action> {
            self.say(Portal::Activated {
                activation: Some(activation),
                barrier: Some(barrier),
                at: Some(at),
            })
        }
    }

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

    #[test]
    fn watching_sets_the_barriers_onto_the_computers_reachable() {
        let mut rig = Rig::new(&["W", "P"]);
        assert_eq!(rig.glue.standing(), Standing::Ready);
        assert_eq!(rig.glue.zones(), Some(&[screen(0, 0, 1600, 1000)][..]));
        assert_eq!(
            rig.calls(),
            vec![Call::Arm(vec![
                (RIGHT, "W".to_string()),
                (BOTTOM, "P".to_string())
            ])]
        );
        // The same again sets nothing; one gone sets them again.
        rig.glue
            .set_targets(&desk(), &["W".to_string(), "P".to_string()]);
        assert_eq!(rig.calls(), vec![]);
        rig.glue.set_targets(&desk(), &["W".to_string()]);
        assert_eq!(rig.calls(), vec![Call::Arm(vec![(RIGHT, "W".to_string())])]);
        // None reachable: no barrier, and the capture is disabled.
        let none = rig.driver.set_reachable(Vec::new());
        assert_eq!(rig.glue.act(none), vec![]);
        rig.glue.set_targets(&desk(), &[]);
        assert_eq!(rig.calls(), vec![Call::Disarm]);
    }

    #[test]
    fn a_barrier_hit_crosses_and_libei_input_goes_on() {
        let mut rig = Rig::new(&["W"]);
        rig.calls();
        let crossed = rig.hit(7, 1, (1600.0, 500.0));
        let layout = desk();
        let (x, y) = layout.normalized("W", (0, 500));
        assert_eq!(sends(&crossed), vec![sent("W", Msg::Enter { x, y })]);
        assert!(crossed.contains(&Action::KeepAwake(true)));
        // The mode, park and cursor are the glue's own, not the loop's.
        assert!(!crossed.iter().any(|a| matches!(
            a,
            Action::SetMode(_) | Action::ParkCursor(..) | Action::HideCursor(_)
        )));
        assert_eq!(rig.driver.away_on(), Some("W"));
        assert!(rig.glue.captured());
        assert_eq!(rig.calls(), vec![], "nothing asked of the portal");

        // Motion, its fractions kept until they make a pixel.
        let moved = rig.say(Portal::Motion { dx: 10.6, dy: 0.5 });
        let (x, y) = layout.normalized("W", (10, 500));
        assert_eq!(sends(&moved), vec![sent("W", Msg::Move { seq: 1, x, y })]);
        let moved = rig.say(Portal::Motion { dx: 0.5, dy: 0.6 });
        let (x, y) = layout.normalized("W", (11, 501));
        assert_eq!(sends(&moved), vec![sent("W", Msg::Move { seq: 2, x, y })]);
        assert_eq!(rig.say(Portal::Motion { dx: 0.2, dy: 0.2 }), vec![]);

        // Buttons, the wheel (a notch is 120 of libei's discrete scroll;
        // smooth scroll alone counts 15 pixels a notch), keys.
        assert_eq!(
            sends(&rig.say(Portal::Button {
                code: 0x111,
                down: true
            })),
            vec![sent(
                "W",
                Msg::Button {
                    button: 2,
                    pressed: true
                }
            )]
        );
        assert_eq!(
            sends(&rig.say(Portal::Scroll {
                dx: 30.0,
                dy: 30.0,
                discrete: Some((0, 240))
            })),
            vec![sent("W", Msg::Wheel { dx: 0, dy: 2 })]
        );
        assert_eq!(
            rig.say(Portal::Scroll {
                dx: 0.0,
                dy: 0.0,
                discrete: Some((0, 60))
            }),
            vec![]
        );
        assert_eq!(
            sends(&rig.say(Portal::Scroll {
                dx: 0.0,
                dy: 0.0,
                discrete: Some((0, 60))
            })),
            vec![sent("W", Msg::Wheel { dx: 0, dy: 1 })]
        );
        assert_eq!(
            sends(&rig.say(Portal::Scroll {
                dx: -31.0,
                dy: 0.0,
                discrete: None
            })),
            vec![sent("W", Msg::Wheel { dx: -2, dy: 0 })]
        );
        assert_eq!(
            sends(&rig.say(Portal::Key {
                code: 30,
                down: true
            })),
            vec![sent(
                "W",
                Msg::Key {
                    code: 30,
                    pressed: true
                }
            )]
        );
        assert_eq!(rig.calls(), vec![]);
    }

    #[test]
    fn coming_home_lets_go_of_the_capture_where_the_cursor_lands() {
        let mut rig = Rig::new(&["W"]);
        rig.calls();
        rig.hit(7, 1, (1600.0, 500.0));
        rig.say(Portal::Key {
            code: 42,
            down: true,
        });
        // 30 left from W's first column is back across onto L.
        let back = rig.say(Portal::Motion { dx: -30.0, dy: 0.0 });
        assert_eq!(
            sends(&back),
            vec![
                sent(
                    "W",
                    Msg::Key {
                        code: 42,
                        pressed: false
                    }
                ),
                sent("W", Msg::Leave)
            ]
        );
        assert_eq!(rig.driver.away_on(), None);
        assert!(!rig.glue.captured());
        // Let go 4 pixels inside the edge; the barriers stay as they were.
        assert_eq!(rig.calls(), vec![Call::Release(Some(7), (1595, 500))]);
        // Input after it is stale: nothing.
        assert_eq!(rig.say(Portal::Motion { dx: 5.0, dy: 0.0 }), vec![]);
        assert_eq!(
            rig.say(Portal::Key {
                code: 30,
                down: true
            }),
            vec![]
        );
    }

    #[test]
    fn the_switch_key_goes_on_and_comes_home_from_libeis_keys() {
        let mut rig = Rig::new(&["W", "P"]);
        rig.calls();
        rig.hit(3, 1, (1600.0, 100.0));
        let key = |rig: &mut Rig, code, down| rig.say(Portal::Key { code, down });
        key(&mut rig, 29, true);
        key(&mut rig, 56, true);
        // On to the phone: Ctrl and Alt let go of on W first.
        let next = key(&mut rig, 106, true);
        let release = |code| {
            sent(
                "W",
                Msg::Key {
                    code,
                    pressed: false,
                },
            )
        };
        assert_eq!(
            &sends(&next)[..3],
            &[release(56), release(29), sent("W", Msg::Leave)]
        );
        assert_eq!(rig.driver.away_on(), Some("P"));
        assert!(rig.glue.captured(), "still captured, on the phone now");
        assert_eq!(key(&mut rig, 106, false), vec![]);
        // Ctrl and Alt were never down on the phone: their key-ups stay.
        assert_eq!(sends(&key(&mut rig, 29, false)), vec![]);
        // Pressed again: the last computer goes home.
        key(&mut rig, 29, true);
        let home = key(&mut rig, 106, true);
        assert_eq!(rig.driver.away_on(), None);
        assert_eq!(sends(&home).last(), Some(&sent("P", Msg::Leave)));
        assert!(matches!(rig.calls()[..], [Call::Release(Some(3), _)]));
    }

    #[test]
    fn a_silent_computer_brings_them_home_and_lets_go() {
        let mut rig = Rig::new(&["W"]);
        rig.calls();
        rig.hit(9, 1, (1600.0, 500.0));
        let actions = rig.driver.tick(rig.now);
        assert_eq!(rig.glue.act(actions), vec![Action::Ping("W".to_string())]);
        let actions = rig.driver.tick(rig.now + ANSWER_WITHIN);
        let rest = rig.glue.act(actions);
        assert!(sends(&rest).contains(&sent("W", Msg::Leave)), "{rest:?}");
        assert_eq!(rig.calls(), vec![Call::Release(Some(9), PARK)]);
    }

    #[test]
    fn the_desktop_ending_the_capture_brings_them_home() {
        for end in [
            Portal::Deactivated {
                activation: Some(4),
            },
            Portal::Deactivated { activation: None },
            Portal::Disabled,
            Portal::Closed("the libei stream closed".to_string()),
        ] {
            let mut rig = Rig::new(&["W"]);
            rig.calls();
            rig.hit(4, 1, (1600.0, 500.0));
            rig.say(Portal::Key {
                code: 56,
                down: true,
            });
            let home = rig.say(end.clone());
            assert_eq!(
                sends(&home),
                vec![
                    sent(
                        "W",
                        Msg::Key {
                            code: 56,
                            pressed: false
                        }
                    ),
                    sent("W", Msg::Leave)
                ],
                "{end:?}"
            );
            assert_eq!(rig.driver.away_on(), None, "{end:?}");
            // It is gone already: nothing to let go of.
            assert_eq!(rig.calls(), vec![], "{end:?}");
            match end {
                Portal::Disabled => {
                    // Not enabled again at once (the desktop said no for
                    // now), only when asked.
                    rig.glue.rearm();
                    assert_eq!(rig.calls(), vec![Call::Arm(vec![(RIGHT, "W".to_string())])]);
                }
                Portal::Closed(_) => assert_eq!(rig.glue.closed(), Some("the libei stream closed")),
                _ => {
                    rig.glue.rearm();
                    assert_eq!(rig.calls(), vec![], "still armed");
                }
            }
        }
        // A stale capture's end changes nothing.
        let mut rig = Rig::new(&["W"]);
        rig.hit(5, 1, (1600.0, 500.0));
        assert_eq!(
            rig.say(Portal::Deactivated {
                activation: Some(4)
            }),
            vec![]
        );
        assert_eq!(rig.driver.away_on(), Some("W"));
    }

    #[test]
    fn a_barrier_that_leads_nowhere_now_is_let_go_of_at_once() {
        // W stopped answering since the barriers were set: the driver
        // will not cross, and the pointer is let go of inside the edge.
        let mut rig = Rig::new(&["W"]);
        let actions = rig.driver.refused("W", 0);
        rig.glue.act(actions);
        rig.calls();
        let hit = rig.hit(2, 1, (1600.0, 300.0));
        assert_eq!(sends(&hit), vec![]);
        assert_eq!(rig.calls(), vec![Call::Release(Some(2), (1591, 300))]);
        // A game in front: nothing is watched, and a late hit is let go of.
        let mut rig = Rig::new(&["W"]);
        let fg = crate::shared_input::game_lock::Foreground {
            fullscreen: true,
            ..Default::default()
        };
        let actions = rig.driver.on(Captured::Foreground {
            fg,
            kernel_anti_cheat: false,
        });
        rig.glue.act(actions);
        assert_eq!(rig.calls()[1..], [Call::Disarm]);
        rig.hit(3, 1, (1600.0, 300.0));
        assert_eq!(rig.calls(), vec![Call::Release(Some(3), (1600, 300))]);
        assert_eq!(rig.driver.away_on(), None);
    }

    #[test]
    fn an_edge_that_keeps_flipping_turns_into_a_wall() {
        let mut rig = Rig::new(&["W", "P"]);
        rig.calls();
        let t0 = rig.now;
        for (n, ms) in [(1u32, 0u64), (2, 300)] {
            rig.now = t0 + Duration::from_millis(ms);
            rig.hit(n, 1, (1600.0, 500.0));
            assert_eq!(rig.driver.away_on(), Some("W"));
            // The compositor hands it straight back (xdph #435).
            rig.say(Portal::Deactivated {
                activation: Some(n),
            });
            assert_eq!(rig.driver.away_on(), None);
        }
        rig.now = t0 + Duration::from_millis(600);
        let third = rig.hit(3, 1, (1600.0, 500.0));
        // Not crossed: let go inside, the barriers set again without that
        // edge, and said once.
        assert_eq!(rig.driver.away_on(), None);
        assert_eq!(sends(&third), vec![]);
        assert_eq!(
            third,
            vec![Action::Badge(badge::of(&badge::Event::Note(WALL_WORDS)))]
        );
        assert_eq!(
            rig.calls(),
            vec![
                Call::Release(Some(3), (1591, 500)),
                Call::Arm(vec![(BOTTOM, "P".to_string())])
            ]
        );
        assert_eq!(rig.glue.standing(), Standing::FractionalScale);
        // New zones (a scale changed): the edge is tried again.
        rig.say(Portal::Zones(vec![
            screen(0, 0, 1600, 1000),
            screen(1600, 0, 0, 0),
        ]));
        rig.glue
            .set_targets(&desk(), &["W".to_string(), "P".to_string()]);
        assert_eq!(
            rig.calls(),
            vec![Call::Arm(vec![
                (RIGHT, "W".to_string()),
                (BOTTOM, "P".to_string())
            ])]
        );
    }

    #[test]
    fn libeis_buttons_are_the_wires() {
        assert_eq!(button_of(0x110), Some(0));
        assert_eq!(button_of(0x112), Some(1));
        assert_eq!(button_of(0x111), Some(2));
        assert_eq!(button_of(0x113), Some(3));
        assert_eq!(button_of(0x114), Some(4));
        assert_eq!(button_of(0x117), None);
    }

    /// BK8 review I1: the desktop still captures (a Release it ignored or
    /// that failed, an Activated that never came) while the glue thinks it
    /// does not: input that keeps coming for 300 ms is let go of again,
    /// at most every 300 ms, never in the moment after a release.
    #[test]
    fn input_with_no_capture_is_let_go_of_again() {
        let mut rig = Rig::new(&["W"]);
        rig.calls();
        let t0 = rig.now;
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let motion = || Portal::Motion { dx: 3.0, dy: 0.0 };
        for ms in [0, 100, 250] {
            rig.now = at(ms);
            assert_eq!(rig.say(motion()), vec![]);
        }
        assert_eq!(rig.calls(), vec![], "within the grace: libei may run ahead");
        rig.now = at(300);
        rig.say(Portal::Key {
            code: 30,
            down: true,
        });
        // Let go of at this computer's centre, where it would come home.
        assert_eq!(rig.calls(), vec![Call::Release(None, PARK)]);
        rig.now = at(450);
        rig.say(motion());
        assert_eq!(rig.calls(), vec![], "not again at once");
        rig.now = at(600);
        rig.say(Portal::Button {
            code: 0x110,
            down: true,
        });
        assert_eq!(rig.calls(), vec![Call::Release(None, PARK)]);
        assert_eq!(rig.driver.away_on(), None, "nothing was sent anywhere");

        // After a real crossing home, libei's last events are no cause.
        let mut rig = Rig::new(&["W"]);
        rig.hit(7, 1, (1600.0, 500.0));
        rig.say(Portal::Motion { dx: -30.0, dy: 0.0 });
        rig.calls();
        let t0 = rig.now;
        for ms in [10, 100, 200] {
            rig.now = t0 + Duration::from_millis(ms);
            rig.say(Portal::Motion { dx: -1.0, dy: 0.0 });
        }
        assert_eq!(rig.calls(), vec![]);
        // Kept up past the grace, it is let go of where it came home.
        rig.now = t0 + Duration::from_millis(400);
        rig.say(Portal::Motion { dx: -1.0, dy: 0.0 });
        assert_eq!(rig.calls(), vec![Call::Release(None, (1595, 500))]);
    }
}
