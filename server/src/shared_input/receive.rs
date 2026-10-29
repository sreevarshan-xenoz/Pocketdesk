//! The computer being driven: what another of the owner's computers sends it
//! on their link, typed and moved here.
//!
//! Nothing is used unless the shared keyboard and mouse is on here
//! (`shared_input`), the sender is one of the owner's paired computers, and
//! it has the Keyboard and mouse permission ([`Capability::SharedInput`]).
//! Only the computer whose cursor came onto this one's screens
//! ([`Msg::Enter`]) moves it, presses its keys and buttons, and turns its
//! wheel; everything it pressed is let go of when it leaves, when it asks,
//! when its link goes, and when its permission does. A computer that may
//! not drive this one is told so ([`Msg::Refuse`]) when it says hello or
//! enters, and a computer that could is told when sharing is turned off
//! here or its permission goes. A computer that may drive this one has it
//! flash its name ([`badge`]) when its cursor arrives, and on Identify.
//!
//! [`Capability::SharedInput`]: pocketdesk_core::protocol::permissions::Capability::SharedInput

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::local::shared_input;
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::streaming::input_packet::InputPacket;

use super::awake::KeepAwake;
use super::badge;
use super::held::HeldInput;
use super::layout;
use super::link::{self, Channel};
use super::quick::{KeepQuick, Nudge};
use super::screens;
use super::sound;
use super::status;
use super::wire::{self, Msg, Screen, Timing};
use crate::input_injector::InputInjector;

/// Where what the driving computer sends ends up.
pub trait Inject: Send {
    /// Press or let go of the key with this evdev code.
    fn key(&mut self, code: u16, pressed: bool);
    /// Press or let go of a button: 0 left, 1 middle, 2 right, 3 back,
    /// 4 forward.
    fn button(&mut self, button: u8, pressed: bool);
    /// Turn the wheel, in notches, positive down and to the right.
    fn wheel(&mut self, dx: i16, dy: i16);
    /// Put the pointer at 0..=65535 across this computer's screens'
    /// bounding box, 65535 being its last pixel.
    fn move_abs(&mut self, x: u16, y: u16);
    /// Get ready to inject before the first event: a uinput device made
    /// at the first event misses it (libinput opens new devices a moment
    /// after they appear).
    fn prepare(&mut self) {}
    /// Flash `badge` on this computer's screens for `how_long` (which
    /// computer has the keyboard and mouse, Identify); on Windows, never
    /// over a full-screen program or a game holding the cursor in front,
    /// as the driving side.
    fn badge(&mut self, badge: &badge::Badge, how_long: Duration) {
        #[cfg(windows)]
        if super::game_lock::in_front(&super::game_lock::foreground(), &[], true) {
            tracing::debug!("Shared input: no badge over the game in front");
            return;
        }
        badge::flash(badge, how_long);
    }
}

impl Inject for InputInjector {
    fn key(&mut self, code: u16, pressed: bool) {
        inject(self, InputPacket::KeyEvent { key: code, pressed });
    }

    fn button(&mut self, button: u8, pressed: bool) {
        inject(self, InputPacket::MouseClick { button, pressed });
    }

    fn wheel(&mut self, dx: i16, dy: i16) {
        // The injector turns it up for a positive `dy`, the wire down.
        inject(
            self,
            InputPacket::MouseScroll {
                dx,
                dy: dy.saturating_neg(),
            },
        );
    }

    fn move_abs(&mut self, x: u16, y: u16) {
        inject(
            self,
            InputPacket::MouseMoveAbs {
                x_norm: x,
                y_norm: y,
            },
        );
    }
}

fn inject(injector: &mut InputInjector, packet: InputPacket) {
    if let Err(e) = injector.handle_input_packet(&packet) {
        // What it was, never which key: no keystroke goes in a log.
        let what = match packet {
            InputPacket::KeyEvent { .. } => "a key",
            InputPacket::MouseClick { .. } => "a button",
            InputPacket::MouseScroll { .. } => "a wheel turn",
            InputPacket::MouseMoveAbs { .. } => "a move",
            _ => "an input",
        };
        tracing::debug!("Shared input: {what} was not injected: {e:#}");
    }
}

/// The highest evdev key code (`KEY_MAX`); one past it is no key.
const KEY_MAX: u16 = 0x2ff;

/// The highest button there is (forward).
const BUTTON_MAX: u8 = 4;

/// Input from the computer driving this recent keeps the link quick (BK10).
pub const QUICK_FOR: Duration = Duration::from_secs(10);

/// An activity note this recent keeps this computer awake.
const AWAKE_FOR: Duration = Duration::from_secs(10);

/// Someone idle this long at the driving computer does not.
const IDLE_MS_AWAKE: u32 = 60_000;

/// How long after the driving computer's last input (or its arrival) this
/// computer's own edges stay unwatched ([`Receiver::driven_by`]).
pub const DRIVEN_FOR: Duration = Duration::from_millis(2500);

/// The driving computer silent this long is taken to have gone.
pub const SILENCE_RELEASE: Duration = Duration::from_secs(1);

/// What this computer does with what other computers send it.
pub struct Receiver {
    inject: Box<dyn Inject>,
    /// The computer whose cursor is on this one's screens now.
    driver: Option<String>,
    /// What this computer has injected as down for it.
    held: HeldInput,
    /// The newest move taken from it since it entered.
    last_seq: Option<u32>,
    /// When the last note came that someone is at a driving computer.
    active_at: Option<Instant>,
    /// When the computer driving last sent input (or arrived).
    input_at: Option<Instant>,
    /// When anything last came off the link from the driving computer.
    heard_at: Option<Instant>,
    /// This computer's name, for its badge.
    name: String,
    /// How long each key and move took here since a computer's cursor
    /// last arrived (that computer), for its `share-test` to ask
    /// ([`Msg::AskTimings`]); at most [`wire::MAX_TIMINGS`].
    timings: Vec<Timing>,
    timed_for: Option<String>,
    /// The keys, buttons and wheel turns from it since it arrived.
    inputs: u32,
    /// It asked for this computer's sound this visit (BK6).
    sound_wanted: bool,
    /// This computer's screens as last looked at (none yet: an arrival is
    /// put where it says).
    screens: Vec<Screen>,
}

impl Receiver {
    pub fn new(inject: Box<dyn Inject>) -> Receiver {
        Receiver {
            inject,
            driver: None,
            held: HeldInput::default(),
            last_seq: None,
            active_at: None,
            input_at: None,
            heard_at: None,
            name: String::new(),
            timings: Vec::new(),
            timed_for: None,
            inputs: 0,
            sound_wanted: false,
            screens: Vec::new(),
        }
    }

    /// This computer's screens, for where an arriving pointer is put
    /// ([`arrival`]).
    pub fn set_screens(&mut self, screens: &[Screen]) {
        if self.screens != screens {
            self.screens = screens.to_vec();
        }
    }

    /// How long what `from` sent took here since its cursor last arrived
    /// (kept after it leaves, until a cursor arrives again).
    pub fn timings_for(&self, from: &str) -> Option<&[Timing]> {
        (self.timed_for.as_deref() == Some(from)).then_some(self.timings.as_slice())
    }

    /// Note how long the move (`moved`, `n` its `seq`) or key (`n` its
    /// count) that came off the link at `arrived_us` took to inject.
    fn time(&mut self, moved: bool, n: u32, arrived_us: Option<u64>) {
        if let Some(arrived_us) = arrived_us
            && self.timings.len() < wire::MAX_TIMINGS
        {
            let took = link::now_us().saturating_sub(arrived_us);
            self.timings.push(Timing {
                moved,
                n,
                arrived_us,
                took_us: u32::try_from(took).unwrap_or(u32::MAX),
            });
        }
    }

    /// This computer's name, as its badge shows it.
    pub fn set_name(&mut self, name: &str) {
        name.clone_into(&mut self.name);
    }

    /// Get the injector ready (sharing is on, or a computer that may
    /// drive this one said hello).
    pub fn prepare(&mut self) {
        self.inject.prepare();
    }

    /// The computer driving this one now.
    pub fn driver(&self) -> Option<&str> {
        self.driver.as_deref()
    }

    /// The computer driving this one at `now` while it sends input: it
    /// arrived, or sent a key, a click, the wheel or a move, within
    /// [`DRIVEN_FOR`]. Pings and activity notes do not count: a cursor left
    /// resting here lets this computer's own edges lead on again.
    pub fn driven_by(&self, now: Instant) -> Option<&str> {
        self.driver.as_deref().filter(|_| {
            self.input_at
                .is_some_and(|at| now.saturating_duration_since(at) < DRIVEN_FOR)
        })
    }

    /// The computer that wants this one's sound now: the one driving it,
    /// when it asked this visit (BK6).
    pub fn sound_for(&self) -> Option<&str> {
        self.driver.as_deref().filter(|_| self.sound_wanted)
    }

    /// Act on `msg` from the computer `from`; `allowed` is whether it may
    /// drive this one (shared input on, one of the owner's computers, with
    /// the permission).
    pub fn handle(&mut self, from: &str, msg: Msg, allowed: bool) {
        self.handle_at(from, msg, allowed, Instant::now());
    }

    /// [`Receiver::handle`], with `now` the time it arrived.
    pub fn handle_at(&mut self, from: &str, msg: Msg, allowed: bool, now: Instant) {
        self.handle_timed(from, msg, allowed, now, None);
    }

    /// [`Receiver::handle`] for what came off the link at `arrived_us`
    /// ([`link::now_us`]): it counts as heard from then, not from when it
    /// is handled, and each key and move is timed.
    pub fn handle_arrived(&mut self, from: &str, msg: Msg, allowed: bool, arrived_us: u64) {
        let arrived = arrived_at(arrived_us, Instant::now(), link::now_us());
        self.handle_timed(from, msg, allowed, arrived, Some(arrived_us));
    }

    fn handle_timed(
        &mut self,
        from: &str,
        msg: Msg,
        allowed: bool,
        now: Instant,
        arrived_us: Option<u64>,
    ) {
        if !allowed {
            // Its permission went (or the feature did) while it drove.
            self.link_lost(from);
            return;
        }
        // Anything at all from the driving computer says it is still there.
        if self.driver.as_deref() == Some(from) {
            self.heard_at = self.heard_at.max(Some(now));
        }
        if let Msg::Hello { .. } = msg {
            // A computer that may drive this one is there: be ready.
            self.inject.prepare();
            return;
        }
        if let Msg::Active { idle_ms } = msg {
            if idle_ms < IDLE_MS_AWAKE {
                self.active_at = Some(now);
            }
            return;
        }
        if let Msg::Identify { .. } = msg {
            // Only a computer that may drive this one has it show its name.
            let here = badge::Event::Identify { here: &self.name };
            self.inject.badge(&badge::of(&here), badge::IDENTIFY_FOR);
            return;
        }
        if let Msg::Enter { x, y } = msg {
            // A fresh start, whoever enters: what an earlier visit left
            // down is let go of first.
            self.release_held();
            self.driver = Some(from.to_string());
            self.heard_at = Some(now);
            self.input_at = Some(now);
            self.last_seq = None;
            self.timings.clear();
            self.timed_for = Some(from.to_string());
            self.inputs = 0;
            // An ask is for one visit: it asks again after it arrives.
            self.sound_wanted = false;
            tracing::debug!(
                from,
                x,
                y,
                "Shared input: another computer's cursor arrived here at this position (0..=65535)"
            );
            // A few pixels inside the edge it came over, never on it:
            // this computer's own edge there leads back to that computer.
            let (x, y) = arrival(&self.screens, x, y);
            self.inject.move_abs(x, y);
            // Where the cursor arrives, the owner sees whose keyboard and
            // mouse they are now: this computer's.
            let here = badge::Event::Arrived { here: &self.name };
            self.inject.badge(&badge::of(&here), badge::SWITCH_FOR);
            return;
        }
        if self.driver.as_deref() != Some(from) {
            return;
        }
        if matches!(
            msg,
            Msg::Key { .. } | Msg::Button { .. } | Msg::Wheel { .. }
        ) {
            self.inputs = self.inputs.wrapping_add(1);
        }
        if matches!(
            msg,
            Msg::Key { .. } | Msg::Button { .. } | Msg::Wheel { .. } | Msg::Move { .. }
        ) {
            self.input_at = self.input_at.max(Some(now));
        }
        let n = self.inputs;
        match msg {
            Msg::Leave => {
                tracing::debug!(from, "Shared input: the driving computer's cursor left");
                self.stop();
            }
            Msg::Move { seq, x, y } => {
                // Newer by the count, which may wrap.
                let newer = self
                    .last_seq
                    .is_none_or(|last| (seq.wrapping_sub(last) as i32) > 0);
                if newer {
                    self.last_seq = Some(seq);
                    self.inject.move_abs(x, y);
                    self.time(true, seq, arrived_us);
                }
            }
            Msg::Key { code, pressed } if code <= KEY_MAX => {
                self.held.note(&msg);
                self.inject.key(code, pressed);
                self.time(false, n, arrived_us);
            }
            Msg::Button { button, pressed } if button <= BUTTON_MAX => {
                self.held.note(&msg);
                self.inject.button(button, pressed);
                self.time(false, n, arrived_us);
            }
            Msg::Wheel { dx, dy } => {
                self.inject.wheel(dx, dy);
                self.time(false, n, arrived_us);
            }
            Msg::ReleaseAll => {
                tracing::debug!(
                    from,
                    "Shared input: the driving computer let go of everything held"
                );
                self.release_held();
            }
            Msg::SoundWanted { on } => self.sound_wanted = on,
            _ => {}
        }
    }

    /// The link to `from` went (or its permission did): if it was driving,
    /// let go of everything it held down here, and it drives no more.
    pub fn link_lost(&mut self, from: &str) {
        if self.driver.as_deref() == Some(from) {
            self.stop();
        }
    }

    /// The driving computer has sent nothing at all for
    /// [`SILENCE_RELEASE`] by `now` (it sends an activity note several
    /// times a second while it drives): it is taken to have gone, as if it
    /// left. Everything it held down is let go of, and this computer is no
    /// longer kept awake for it; what it sends later is ignored until it
    /// enters again. A link that goes without a word is noticed here long
    /// before the connection times out. Silence is counted from when its
    /// messages came off the link: what it handled last, or `newest`, the
    /// last thing from it to come off the link, handled or not (a loop
    /// held up a moment does not end a session that is still going).
    pub fn check_silence(&mut self, now: Instant, newest: Option<Instant>) {
        let heard = self
            .heard_at
            .max(newest.filter(|_| self.heard_at.is_some()));
        let silent = heard.is_some_and(|at| now.saturating_duration_since(at) >= SILENCE_RELEASE);
        if self.driver.is_some() && silent {
            self.stop();
            self.active_at = None;
        }
    }

    fn stop(&mut self) {
        self.release_held();
        self.driver = None;
        self.heard_at = None;
        self.input_at = None;
        self.last_seq = None;
        self.sound_wanted = false;
    }

    /// Let go of everything injected as down, last pressed first.
    fn release_held(&mut self) {
        for msg in self.held.releases() {
            match msg {
                Msg::Key { code, pressed } => self.inject.key(code, pressed),
                Msg::Button { button, pressed } => self.inject.button(button, pressed),
                _ => {}
            }
        }
    }

    /// Whether this computer should be kept awake at `now`: someone was at a
    /// computer that may drive it within the last 10 s.
    pub fn wants_awake(&self, now: Instant) -> bool {
        self.active_at
            .is_some_and(|at| now.saturating_duration_since(at) < AWAKE_FOR)
    }

    /// Whether the link is kept quick at `now` (BK10): a computer drives
    /// this one and sent a key, a click, the wheel or a move (or arrived)
    /// within [`QUICK_FOR`]. A pointer left resting here lets go, and the
    /// next input takes it again.
    pub fn wants_quick(&self, now: Instant) -> bool {
        self.driver.is_some()
            && self
                .input_at
                .is_some_and(|at| now.saturating_duration_since(at) < QUICK_FOR)
    }
}

/// Pixels inside the edge of the screen it lands on that an arriving
/// pointer is put.
pub const ARRIVE_INSIDE: i64 = 4;

/// Where a pointer arriving at (`x`, `y`) (0..=65535 across the box around
/// `screens`, as [`Msg::Enter`] says it) is put: [`ARRIVE_INSIDE`] pixels
/// inside the edges of the screen it lands on, never on one. The driving
/// computer puts it on the first row or column past its own edge, the very
/// line where this computer's own edge facing it is watched (the portal's
/// barrier on Linux): a pointer put there, as the compositor's virtual
/// pointer puts it, could set it off (the owner's first live test). Only a
/// pixel that is too near an edge moves; with no screens known yet it is
/// put where it says.
fn arrival(screens: &[Screen], x: u16, y: u16) -> (u16, u16) {
    let rects: Vec<(i64, i64, i64, i64)> = screens
        .iter()
        .filter(|s| s.width > 0 && s.height > 0)
        .map(|s| {
            let (x0, y0) = (i64::from(s.x), i64::from(s.y));
            (x0, y0, x0 + i64::from(s.width), y0 + i64::from(s.height))
        })
        .collect();
    let Some((bx0, by0, bx1, by1)) = rects
        .iter()
        .copied()
        .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
    else {
        return (x, y);
    };
    // The pixel it names, as `layout::normalize` names a pixel.
    let pixel = |v: u16, lo: i64, hi: i64| {
        let last = (hi - lo - 1).max(0);
        lo + (i64::from(v) * last + 32767) / 65535
    };
    let (px, py) = (pixel(x, bx0, bx1), pixel(y, by0, by1));
    // The screen it is on, else the nearest.
    let Some(&(x0, y0, x1, y1)) = rects.iter().min_by_key(|r| {
        let dx = (r.0 - px).max(px - (r.2 - 1)).max(0);
        let dy = (r.1 - py).max(py - (r.3 - 1)).max(0);
        dx * dx + dy * dy
    }) else {
        return (x, y);
    };
    let inside = |v: u16, p: i64, lo: i64, hi: i64, blo: i64, bhi: i64| {
        let (min, max) = (lo + ARRIVE_INSIDE, hi - 1 - ARRIVE_INSIDE);
        if min > max || (min..=max).contains(&p) {
            v
        } else {
            layout::normalize(p.clamp(min, max), blo, bhi)
        }
    };
    (
        inside(x, px, x0, x1, bx0, bx1),
        inside(y, py, y0, y1, by0, by1),
    )
}

/// When something that came off the link at `arrived_us` ([`link::now_us`])
/// came, on the monotonic clock: `now` is `now_us`. Never later than now.
fn arrived_at(arrived_us: u64, now: Instant, now_us: u64) -> Instant {
    now.checked_sub(Duration::from_micros(now_us.saturating_sub(arrived_us)))
        .unwrap_or(now)
}

/// A receiver going away (the server stopping or upgrading mid-session)
/// lets go of what it held down first.
impl Drop for Receiver {
    fn drop(&mut self) {
        self.release_held();
    }
}

/// The injector, made when sharing is on or a computer that may drive this
/// one says hello (else at the first event): a server with sharing off
/// makes no virtual devices. One that cannot be made is
/// tried again a while later.
#[derive(Default)]
struct LazyInjector {
    injector: Option<InputInjector>,
    failed_at: Option<Instant>,
    /// Windows: the monitors as last read, and when, to put a position
    /// across the logical screens told to the driving computer onto the
    /// physical ones Windows' absolute input spans.
    #[cfg(windows)]
    desktop: Option<(screens::Desktop, Instant)>,
}

/// How long Windows' monitors are taken as read.
#[cfg(windows)]
const DESKTOP_FOR: Duration = Duration::from_secs(2);

/// How long after the injector could not be made it is tried again.
const INJECTOR_RETRY: Duration = Duration::from_secs(10);

impl LazyInjector {
    fn get(&mut self) -> Option<&mut InputInjector> {
        let waiting = self
            .failed_at
            .is_some_and(|at| at.elapsed() < INJECTOR_RETRY);
        if self.injector.is_none() && !waiting {
            match InputInjector::new() {
                Ok(injector) => self.injector = Some(injector),
                Err(e) => {
                    tracing::warn!("Shared input: nothing to inject with here: {e:#}");
                    self.failed_at = Some(Instant::now());
                }
            }
        }
        self.injector.as_mut()
    }
}

impl Inject for LazyInjector {
    fn prepare(&mut self) {
        self.get();
    }

    fn key(&mut self, code: u16, pressed: bool) {
        if let Some(injector) = self.get() {
            Inject::key(injector, code, pressed);
        }
    }

    fn button(&mut self, button: u8, pressed: bool) {
        if let Some(injector) = self.get() {
            Inject::button(injector, button, pressed);
        }
    }

    fn wheel(&mut self, dx: i16, dy: i16) {
        if let Some(injector) = self.get() {
            Inject::wheel(injector, dx, dy);
        }
    }

    fn move_abs(&mut self, x: u16, y: u16) {
        #[cfg(windows)]
        let (x, y) = {
            if self
                .desktop
                .as_ref()
                .is_none_or(|(_, at)| at.elapsed() >= DESKTOP_FOR)
            {
                let desktop = screens::Desktop::arrange(&crate::windows_desktop::monitors_scaled());
                self.desktop = Some((desktop, Instant::now()));
            }
            self.desktop
                .as_ref()
                .map_or((x, y), |(desktop, _)| desktop.physical_norm(x, y))
        };
        if let Some(injector) = self.get() {
            Inject::move_abs(injector, x, y);
        }
    }
}

/// How often the driver's silence, link and permission, and the
/// keep-awake, are looked at, and what was heard of other computers'
/// screens is written: a silent driver is let go of within a tick of
/// [`SILENCE_RELEASE`].
const TICK: Duration = Duration::from_millis(100);

/// How often this computer's own screens are looked at.
const SCREENS_EVERY: Duration = Duration::from_secs(5);

/// How long the setting and a computer's standing are taken as read.
const RECHECK: Duration = Duration::from_secs(1);

/// What a computer may do here.
#[derive(Debug, Clone, Copy, Default)]
struct Standing {
    /// Shared input is on and it is one of the owner's computers, paired
    /// either way: what it says of its screens is kept.
    computer: bool,
    /// And this computer lets it drive this one: the Keyboard and mouse
    /// permission in the trust store for one that paired with this one,
    /// else what `computers.json` keeps for one this one paired with.
    allowed: bool,
}

/// A computer's standing from the trust store (`trusted`, a computer
/// there, and `permitted`, its Keyboard and mouse permission) and
/// `computers.json` (`paired_with`, whether this one lets it drive, when
/// this one paired with it). The trust store's word wins where both have
/// it.
fn standing_of(trusted: bool, permitted: bool, paired_with: Option<bool>) -> Standing {
    Standing {
        computer: trusted || paired_with.is_some(),
        allowed: if trusted {
            permitted
        } else {
            paired_with.unwrap_or(false)
        },
    }
}

/// What `device_id` may do here, read now (files: blocking), with shared
/// input on.
fn standing_now(device_id: &str) -> Standing {
    // A permission profile could carry the flag onto a phone: only the
    // owner's computers drive this one, those that paired with this one
    // (the trust store's) by its permission there.
    let trusted = crate::plugins::pair::is_computer_device(device_id);
    // One this computer paired with has no trust store entry: what it may
    // do is kept with it in computers.json.
    let paired_with = crate::state::state_dir().ok().and_then(|dir| {
        pocketdesk_core::local::computers::read(&dir)
            .computers
            .into_iter()
            .find(|c| c.device_id == device_id)
    });
    standing_of(
        trusted,
        trusted && crate::plugins::pair::allows(device_id, Capability::SharedInput),
        paired_with.map(|c| c.lets_drive),
    )
}

/// Whether the computer `device_id` may drive this one now: shared input
/// on here, and its standing (files: blocking).
pub(crate) fn may_drive_here(device_id: &str) -> bool {
    crate::events::current_config().shared_input && standing_now(device_id).allowed
}

/// The setting and each computer's standing, read again at most once a
/// [`RECHECK`] (they are files: config.toml and the trust store).
#[derive(Default)]
struct Gate {
    /// `shared_input`, `shared_input_clipboard` and
    /// `shared_input_low_latency`, and when read.
    on: Option<((bool, bool, bool), Instant)>,
    standings: HashMap<String, (Standing, Instant)>,
}

impl Gate {
    async fn settings(&mut self) -> (bool, bool, bool) {
        if let Some((on, at)) = self.on
            && at.elapsed() < RECHECK
        {
            return on;
        }
        let on = tokio::task::spawn_blocking(|| {
            let config = crate::events::current_config();
            (
                config.shared_input,
                config.shared_input_clipboard,
                config.shared_input_low_latency,
            )
        })
        .await
        .unwrap_or((false, false, false));
        self.on = Some((on, Instant::now()));
        on
    }

    async fn on(&mut self) -> bool {
        self.settings().await.0
    }

    /// The clipboard follows the mouse here (`shared_input_clipboard`).
    async fn clipboard_follows(&mut self) -> bool {
        self.settings().await.1
    }

    /// The link is kept quick while driven (`shared_input_low_latency`).
    async fn low_latency(&mut self) -> bool {
        self.settings().await.2
    }

    /// Why `device_id` may not drive this one, as a [`Msg::Refuse`] says
    /// it; none when it may.
    async fn refusal(&mut self, device_id: &str) -> Option<u8> {
        if !self.on().await {
            return Some(wire::REFUSE_SHARING_OFF);
        }
        (!self.standing(device_id).await.allowed).then_some(wire::REFUSE_NOT_ALLOWED)
    }

    async fn standing(&mut self, device_id: &str) -> Standing {
        if !self.on().await {
            return Standing::default();
        }
        if let Some((standing, at)) = self.standings.get(device_id)
            && at.elapsed() < RECHECK
        {
            return *standing;
        }
        let id = device_id.to_string();
        let standing = tokio::task::spawn_blocking(move || standing_now(&id))
            .await
            .unwrap_or_default();
        self.standings
            .insert(device_id.to_string(), (standing, Instant::now()));
        standing
    }
}

/// This computer's screens, read on a task of their own (review M3): a
/// compositor slow to answer (up to the Wayland read's deadline) never
/// holds up the loop that injects another computer's input. The loop asks
/// for a look ([`ScreensLook::ask`]) and takes what was seen when it comes
/// ([`ScreensLook::seen`]); an ask made while a look is on its way is
/// dropped, so no queue builds behind a slow one.
struct ScreensLook {
    ask: tokio::sync::mpsc::Sender<()>,
    seen: tokio::sync::mpsc::Receiver<Vec<Screen>>,
}

impl ScreensLook {
    /// Looking with [`screens::current`].
    fn start() -> Self {
        Self::start_with(screens::current)
    }

    /// Looking with `look`; its task ends with this.
    fn start_with<F, Fut>(look: F) -> Self
    where
        F: Fn() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Vec<Screen>> + Send + 'static,
    {
        let (ask, mut asked) = tokio::sync::mpsc::channel::<()>(1);
        let (tell, seen) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            while asked.recv().await.is_some() {
                if tell.send(look().await).await.is_err() {
                    break;
                }
            }
        });
        ScreensLook { ask, seen }
    }

    /// Ask for a look, unless one is already on its way. Never waits.
    fn ask(&self) {
        let _ = self.ask.try_send(());
    }
}

/// This computer's own screens as last read, so they are written to
/// `shared_screens.json` (and told) when they are first read, whichever
/// way that is, and again only when they change.
#[derive(Debug, Default)]
struct OwnScreens(Option<Vec<Screen>>);

impl OwnScreens {
    /// Not read since sharing was last turned on.
    fn is_unread(&self) -> bool {
        self.0.is_none()
    }

    /// Take `now` as this computer's screens; whether they are new (never
    /// read, or changed), and so to be written and told.
    fn note(&mut self, now: Vec<Screen>) -> bool {
        if self.0.as_ref() == Some(&now) {
            return false;
        }
        self.0 = Some(now);
        true
    }

    fn get(&self) -> &[Screen] {
        self.0.as_deref().unwrap_or_default()
    }

    fn forget(&mut self) {
        self.0 = None;
    }
}

/// What other computers said of their screens and is not written yet, by
/// computer: the name it gave (when it gave one) and its screens (when it
/// gave them), clipped.
type Heard = HashMap<String, (Option<String>, Option<Vec<Screen>>)>;

fn note_heard(heard: &mut Heard, from: &str, name: Option<&str>, screens: &[Screen]) {
    let entry = heard.entry(from.to_string()).or_default();
    if let Some(name) = name {
        entry.0 = Some(shared_input::clean_name(name));
    }
    entry.1 = Some(shared_input::clean_screens(screens));
}

/// A computer said its name alone ([`Msg::Name`]: one that only takes
/// input says it after its screens, as it never says hello).
fn note_name(heard: &mut Heard, from: &str, name: &str) {
    let entry = heard.entry(from.to_string()).or_default();
    entry.0 = Some(shared_input::clean_name(name));
}

/// Keep `heard` in `file`: a computer's screens with its name, when it
/// gave one; a name said alone names the computer already kept (its
/// screens as they were), and makes none without screens.
fn keep_heard(file: &mut shared_input::ScreensFile, heard: &Heard, now: u64) {
    for (id, (name, screens)) in heard {
        match (name, screens) {
            (name, Some(screens)) => file.heard(id, name.as_deref(), screens, now),
            (Some(name), None) => {
                if let Some(screens) = file.get(id).map(|kept| kept.screens.clone()) {
                    file.heard(id, Some(name), &screens, now);
                }
            }
            (None, None) => {}
        }
    }
}

/// Answer `to`'s hello ([`hello_answer`]), on the channel kept for it.
fn answer_hello(channels: &mut HashMap<String, Channel>, to: &str, screens: &[Screen], name: &str) {
    for msg in hello_answer(screens, name) {
        send_to(channels, to, &msg);
    }
}

/// The answer to a hello, in order: this computer's `screens` (the answer
/// the driving side waits for), then its `name` (which a computer that
/// only takes input says nowhere else).
fn hello_answer(screens: &[Screen], name: &str) -> [Msg; 2] {
    [
        Msg::Screens(screens.to_vec()),
        Msg::Name {
            name: name.to_string(),
        },
    ]
}

/// Write `heard` to `shared_screens.json`, and with `own` this computer's
/// screens first.
async fn write_screens(state_dir: Option<&PathBuf>, heard: Heard, own: Option<Vec<Screen>>) {
    let Some(dir) = state_dir.cloned() else {
        return;
    };
    let written = tokio::task::spawn_blocking(move || {
        let me = own.as_ref().map(|_| crate::kde::host_identity());
        let now = pocketdesk_core::local::pin::now_unix();
        // Only the computers and phones paired now are kept (none dropped
        // on a reading that failed).
        let paired = status::kept_devices(&dir);
        shared_input::update(&dir, |file| {
            if let Some(paired) = &paired {
                file.keep_only(paired);
            }
            if let (Some(me), Some(own)) = (&me, &own) {
                file.set_own(&me.device_id, &me.device_name, own, now);
            }
            keep_heard(file, &heard, now);
        })
    })
    .await;
    if let Ok(Err(e)) = written {
        tracing::debug!("Shared input: the screens were not written: {e}");
    }
}

/// Capture and send this computer's sound to the computer driving it
/// while it asks ([`Receiver::sound_for`]), and stop as soon as it does
/// not; a capture that could not give it is told to that computer, once,
/// while it still asks.
fn follow_sound(
    sound: &mut sound::Sending,
    receiver: &Receiver,
    channels: &mut HashMap<String, Channel>,
) {
    sound.set(receiver.sound_for());
    while let Some((to, why)) = sound.take_unavailable() {
        if receiver.sound_for() == Some(to.as_str()) {
            send_to(channels, &to, &Msg::SoundUnavailable { why });
        }
    }
}

/// Tell the computer `to` this computer's `screens`, on the channel kept
/// for it.
fn tell_screens(channels: &mut HashMap<String, Channel>, to: &str, screens: &[Screen]) {
    send_to(channels, to, &Msg::Screens(screens.to_vec()));
}

/// Send `msg` to the computer `to` on the channel kept for it (opened the
/// first time; dropped when a send fails, so the next opens it again).
fn send_to(channels: &mut HashMap<String, Channel>, to: &str, msg: &Msg) {
    // A channel on a link since replaced (the other computer restarted and
    // linked again) would carry this nowhere: it goes on the new link.
    if channels
        .get(to)
        .is_some_and(|channel| !channel.is_current())
    {
        channels.remove(to);
    }
    if !channels.contains_key(to) {
        match Channel::open(to) {
            Ok(channel) => {
                channels.insert(to.to_string(), channel);
            }
            Err(e) => {
                tracing::debug!(to, "Shared input: not sent to another computer: {e}");
                return;
            }
        }
    }
    if let Some(channel) = channels.get_mut(to)
        && let Err(e) = channel.send(msg)
    {
        tracing::debug!(to, "Shared input: not sent to another computer: {e}");
        channels.remove(to);
    }
}

/// Whether this computer pushes its clipboard back to `from` for `msg`
/// (the clipboard following the mouse home): a leave from the computer
/// driving this one now (`driver`), allowed to (`allowed`), with
/// `shared_input_clipboard` on here (`follows`). Not the leave a driver
/// sends as it greets (it drives nothing then), nor one from another
/// computer.
fn pushes_back(msg: &Msg, from: &str, allowed: bool, driver: Option<&str>, follows: bool) -> bool {
    matches!(msg, Msg::Leave) && allowed && follows && driver == Some(from)
}

/// Whether a computer that may not drive this one is told so for `msg`:
/// its hello and its entering are, and anything at all while it drives
/// (`driving`, its permission or the setting having just gone). Its moves
/// and notes otherwise are not, one refusal each time it tries.
fn refuses(msg: &Msg, driving: bool) -> bool {
    driving || matches!(msg, Msg::Hello { .. } | Msg::Enter { .. })
}

/// Whether `msg` is a cursor arriving over a link that is not direct now
/// (`direct`, looked at only for an arrival: what else a computer sends is
/// taken only once its arrival was).
fn arrives_relayed(msg: &Msg, direct: impl FnOnce() -> bool) -> bool {
    matches!(msg, Msg::Enter { .. }) && !direct()
}

/// Why the computer driving this one is refused now: `refusal`, its
/// permission's or the setting's word, or its link, still up (`linked`),
/// riding a relay (not `direct`).
fn refusal_now(refusal: Option<u8>, linked: bool, direct: bool) -> Option<u8> {
    refusal.or((linked && !direct).then_some(wire::REFUSE_RELAYED))
}

/// Whether the link to `device_id` is up and direct now.
fn direct_now(device_id: &str) -> bool {
    link::connection(device_id).is_some_and(|conn| link::is_direct(&conn))
}

/// Tell the computer `to` that this one will not be driven by it
/// (`reason`), on a channel of its own, dropped once sent; said in the log
/// at info only when `told` has not said it already. A refusal goes over a
/// relay too (it carries no input), so a computer refused for its relayed
/// link hears why.
fn refuse(to: &str, reason: u8, told: &mut status::RefusalLog) {
    let sent =
        Channel::open_for_refusal(to).and_then(|mut channel| channel.send(&Msg::Refuse { reason }));
    match sent {
        Ok(()) => say_refused(told, to, reason),
        Err(e) => tracing::debug!(to, "Shared input: the refusal was not sent: {e}"),
    }
}

/// Say in the log that this one refused `to` (`reason`): at info the
/// first time, at debug again.
fn say_refused(told: &mut status::RefusalLog, to: &str, reason: u8) {
    if told.note(to, reason) {
        tracing::info!(
            to,
            reason,
            "Shared input: told another computer this one won't take its input"
        );
    } else {
        tracing::debug!(to, reason, "Shared input: refused another computer again");
    }
}

/// How often `shared_input_status.json` is brought up to date (and
/// `shared_screens.json` kept to the computers paired now) while sharing is
/// on ([`STATUS_IDLE_EVERY`] while it is off).
const STATUS_EVERY: Duration = Duration::from_secs(1);

/// How often the status is kept while sharing is off here: the page still
/// shows each paired computer's link then, but reading the computers and
/// the trust store every second for the server's life is not worth it.
const STATUS_IDLE_EVERY: Duration = Duration::from_secs(30);

/// Whether the status is kept now: sharing on (`on`) or just turned off
/// (`was_on`, as it was last kept; none yet), or `since` it was last kept
/// is [`STATUS_IDLE_EVERY`].
fn status_due(on: bool, was_on: Option<bool>, since: Option<Duration>) -> bool {
    on || was_on != Some(false) || since.is_none_or(|since| since >= STATUS_IDLE_EVERY)
}

/// The computer driving this one now, while its input is recent
/// ([`Receiver::driven_by`]): this computer's own driver watches no edge
/// meanwhile ([`driven_by`]).
static DRIVEN_BY: Mutex<Option<String>> = Mutex::new(None);

/// The computer driving this one now, while it sends input. This
/// computer's own driver (Windows' hooks, Linux's input-capture portal)
/// watches no edge meanwhile: the pointer here is that computer's, and put
/// on the edge facing it, an edge watched would carry this computer's
/// keyboard and mouse straight back (the owner's first live test, Windows
/// driving the Linux laptop). Watched again once it leaves, goes home,
/// falls silent, or sends no input for [`DRIVEN_FOR`], so its cursor left
/// idle here never keeps this computer's own mouse from its edges (PR #552
/// review I1: Linux has no switch key at home).
pub fn driven_by() -> Option<String> {
    DRIVEN_BY.lock().map(|by| by.clone()).unwrap_or_default()
}

fn set_driven_by(by: Option<&str>) {
    let Ok(mut now) = DRIVEN_BY.lock() else {
        return;
    };
    if now.as_deref() != by {
        tracing::debug!(
            by,
            "Shared input: driven by another computer now, or no longer (no edge watched meanwhile)"
        );
        *now = by.map(str::to_string);
    }
}

/// Whether `msg` from `from` makes it drive this one now, said before it is
/// acted on so no edge its pointer lands on is watched: its arrival, or
/// input from the computer whose cursor is here (`driver`).
fn drives_now(msg: &Msg, from: &str, allowed: bool, driver: Option<&str>) -> bool {
    allowed
        && match msg {
            Msg::Enter { .. } => true,
            Msg::Move { .. } | Msg::Key { .. } | Msg::Button { .. } | Msg::Wheel { .. } => {
                driver == Some(from)
            }
            _ => false,
        }
}

/// The receiving side, for the server's life: take what other computers
/// send ([`link::incoming`]) and act on it, keep this computer awake while
/// someone drives it, and keep `shared_screens.json`. Not run with
/// `POCKETDESK_SHARE_TEST` set, whose recorder takes what arrives instead.
pub async fn run() {
    let mut arriving = link::incoming();
    let state_dir = crate::state::state_dir().ok();
    let mut receiver = Receiver::new(Box::<LazyInjector>::default());
    receiver.set_name(&crate::kde::host_device_name());
    let mut awake = KeepAwake::default();
    // Held while another computer's cursor is on this one (BK10).
    let mut quick = KeepQuick::default();
    let mut nudge = Nudge::default();
    let mut gate = Gate::default();
    // This computer's screens as last looked at (none while shared input
    // is off), and the computers told them.
    let mut own = OwnScreens::default();
    let mut channels: HashMap<String, Channel> = HashMap::new();
    let mut heard = Heard::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut look = tokio::time::interval(SCREENS_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Read off this loop: a slow compositor never delays an injection.
    let mut looks = ScreensLook::start();
    let mut status_every = tokio::time::interval(STATUS_EVERY);
    status_every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_status: Option<shared_input::StatusFile> = None;
    // The link the driving computer's cursor arrived on.
    let mut driver_link: Option<usize> = None;
    // Whether sharing was on when the status was last kept, and when.
    let mut status_kept: Option<(bool, Instant)> = None;
    // The refusals heard from other computers, and told to them, for the
    // log to say each once.
    let mut heard_refusals = status::RefusalLog::default();
    let mut told_refusals = status::RefusalLog::default();
    // This computer's sound, sent to the computer driving it while that
    // one asks (BK6).
    let mut sound = sound::Sending::new(sound::desktop_source());
    loop {
        // Whatever changed last time round (an arrival, a leave, a silence,
        // a permission or a link gone), the sound follows it.
        follow_sound(&mut sound, &receiver, &mut channels);
        // And whether this computer's own driver may watch its edges.
        set_driven_by(receiver.driven_by(Instant::now()));
        tokio::select! {
            arrival = arriving.recv() => {
                let Some((from, msg, arrived_us, link_id)) = arrival else { break };
                if let Msg::Refuse { reason } = msg {
                    // Its answer to this computer driving it.
                    if heard_refusals.note(&from, reason) {
                        tracing::info!(
                            from,
                            reason,
                            "Shared input: another computer won't take this one's input"
                        );
                    } else {
                        tracing::debug!(from, reason, "Shared input: refused again");
                    }
                    link::note_refused(&from, reason);
                    continue;
                }
                let standing = gate.standing(&from).await;
                if !standing.allowed && refuses(&msg, receiver.driver() == Some(from.as_str())) {
                    let reason = gate.refusal(&from).await.unwrap_or(wire::REFUSE_NOT_ALLOWED);
                    refuse(&from, reason, &mut told_refusals);
                    channels.remove(&from);
                }
                if standing.allowed {
                    told_refusals.clear(&from);
                }
                // Its cursor arriving over a relay is refused, and it comes
                // home: the keyboard and mouse go over a direct link only.
                if standing.allowed && arrives_relayed(&msg, || link::direct_on(link_id) == Some(true)) {
                    tracing::info!(
                        from,
                        "Shared input: another computer's cursor came over a relay; refused"
                    );
                    refuse(&from, wire::REFUSE_RELAYED, &mut told_refusals);
                    receiver.link_lost(&from);
                    continue;
                }
                // Its screens are the answer the driving side waits for
                // before it treats this computer as one to cross onto (a
                // hello from its own driver is not).
                if standing.computer
                    && super::drive::is_answer(&msg)
                    && link::note_screens(&from, link_id)
                {
                    heard_refusals.clear(&from);
                }
                match &msg {
                    Msg::Hello { version, name, screens } if standing.computer => {
                        note_heard(&mut heard, &from, Some(name), screens);
                        if standing.allowed && *version == wire::VERSION {
                            // Not looked at since sharing came on: a look
                            // is asked for, and its screens go to every
                            // computer told when it comes. This answer
                            // says what is known now.
                            if own.is_unread() {
                                looks.ask();
                            }
                            let name = crate::kde::host_device_name();
                            answer_hello(&mut channels, &from, own.get(), &name);
                        }
                    }
                    Msg::Screens(screens) if standing.computer => {
                        note_heard(&mut heard, &from, None, screens);
                    }
                    Msg::Name { name } if standing.computer => {
                        note_name(&mut heard, &from, name);
                    }
                    Msg::Identify { name } if standing.allowed => {
                        // Shown on screen by the receiver, below.
                        tracing::info!(
                            from,
                            name,
                            "Shared input: another computer asks this one to show who it is"
                        );
                    }
                    _ => {}
                }
                if standing.allowed && matches!(msg, Msg::Enter { .. }) {
                    tracing::info!(from, "Shared input: another computer drives this one");
                    driver_link = Some(link_id);
                }
                // Said before its pointer is put here or moved, so no edge
                // it lands on is watched.
                if drives_now(&msg, &from, standing.allowed, receiver.driver()) {
                    set_driven_by(Some(&from));
                }
                if standing.allowed && matches!(msg, Msg::AskTimings) {
                    let timings = receiver.timings_for(&from).unwrap_or_default().to_vec();
                    send_to(&mut channels, &from, &Msg::Timings(timings));
                    continue;
                }
                // The cursor going home from here: this computer's
                // clipboard goes with it, when it follows the mouse here.
                let follows = matches!(msg, Msg::Leave) && gate.clipboard_follows().await;
                let back = pushes_back(&msg, &from, standing.allowed, receiver.driver(), follows);
                receiver.handle_arrived(&from, msg, standing.allowed, arrived_us);
                if back {
                    let to = from.clone();
                    tokio::spawn(async move { crate::clipboard_push::push_now(&to).await });
                }
            }
            _ = tick.tick() => {
                // A computer told this one's screens (one that may cross
                // onto it) whose permission went, or with sharing turned
                // off here, is told it may not.
                let mut refused_now = Vec::new();
                let told: Vec<String> = channels.keys().cloned().collect();
                for to in told {
                    if let Some(reason) = gate.refusal(&to).await {
                        if let Some(mut channel) = channels.remove(&to)
                            && let Err(e) = channel.send(&Msg::Refuse { reason })
                        {
                            tracing::debug!(to, "Shared input: the refusal was not sent: {e}");
                        }
                        say_refused(&mut told_refusals, &to, reason);
                        refused_now.push(to);
                    }
                }
                if let Some(driver) = receiver.driver().map(str::to_string) {
                    receiver.check_silence(Instant::now(), link::last_heard(&driver));
                    if receiver.driver().is_none() {
                        tracing::info!(
                            driver,
                            "Shared input: the driving computer went silent; what it held is let go of"
                        );
                    }
                }
                if let Some(driver) = receiver.driver().map(str::to_string) {
                    let linked = link::linked().contains(&driver);
                    // The link its cursor came on while it is still read,
                    // else the one up now.
                    let direct = driver_link
                        .and_then(link::direct_on)
                        .unwrap_or_else(|| direct_now(&driver));
                    let refusal = refusal_now(gate.refusal(&driver).await, linked, direct);
                    if linked
                        && let Some(reason) = refusal
                        && !refused_now.contains(&driver)
                    {
                        refuse(&driver, reason, &mut told_refusals);
                    }
                    if !linked || refusal.is_some() {
                        tracing::info!(
                            driver,
                            linked,
                            "Shared input: the driving computer's link or permission went, or \
                             its link turned relayed; what it held is let go of"
                        );
                        receiver.link_lost(&driver);
                    }
                }
                if receiver.wants_awake(Instant::now()) {
                    awake.hold();
                } else {
                    awake.release();
                }
                // Held while the computer driving sends input, let go
                // after ten seconds without (BK10 review M3).
                let quick_now = receiver.wants_quick(Instant::now()) && gate.low_latency().await;
                quick.set(quick_now);
                nudge.follow(receiver.driver().filter(|_| quick_now));
                if !heard.is_empty() {
                    write_screens(state_dir.as_ref(), std::mem::take(&mut heard), None).await;
                }
            }
            _ = look.tick() => {
                if !gate.on().await {
                    own.forget();
                    receiver.set_screens(&[]);
                    // Those that could cross onto this one are told they
                    // cannot now.
                    for (to, mut channel) in channels.drain() {
                        let refusal = Msg::Refuse {
                            reason: wire::REFUSE_SHARING_OFF,
                        };
                        if let Err(e) = channel.send(&refusal) {
                            tracing::debug!(to, "Shared input: the refusal was not sent: {e}");
                        }
                    }
                    continue;
                }
                // Sharing is on: the injector is made now, not at the first
                // event, which it would miss.
                receiver.prepare();
                // The name its badge shows, kept to a rename.
                receiver.set_name(&crate::kde::host_device_name());
                looks.ask();
            }
            Some(seen) = looks.seen.recv() => {
                // Looked at while sharing was on; off since, it is no news.
                if !gate.on().await {
                    continue;
                }
                if own.note(seen) {
                    receiver.set_screens(own.get());
                    write_screens(state_dir.as_ref(), Heard::new(), Some(own.get().to_vec())).await;
                    let told: Vec<String> = channels.keys().cloned().collect();
                    for to in told {
                        if gate.standing(&to).await.allowed {
                            tell_screens(&mut channels, &to, own.get());
                        } else {
                            channels.remove(&to);
                        }
                    }
                }
            }
            _ = status_every.tick() => {
                let linked = link::linked();
                heard_refusals.keep_linked(&linked);
                told_refusals.keep_linked(&linked);
                let Some(dir) = state_dir.clone() else { continue };
                let sharing_on = gate.on().await;
                let due = status_due(
                    sharing_on,
                    status_kept.map(|(on, _)| on),
                    status_kept.map(|(_, at)| at.elapsed()),
                );
                if !due {
                    continue;
                }
                status_kept = Some((sharing_on, Instant::now()));
                let mut last = last_status.take();
                last_status = tokio::task::spawn_blocking(move || {
                    let paired = status::paired_computers(&dir);
                    let mut now = status::now(paired.as_ref().unwrap_or(&BTreeSet::new()));
                    // BK5: each phone said as a screen, and whether it
                    // may be driven now.
                    let trust = crate::state::trust_store_path().ok();
                    let phones = status::paired_phones_at(trust.as_deref());
                    if let Some(phones) = &phones {
                        now.phones = status::phones_from(
                            &shared_input::read(&dir),
                            phones,
                            super::phone::standing_now,
                        );
                    } else if let Some(last) = &last {
                        now.phones = last.phones.clone();
                    }
                    let kept = paired.as_ref().zip(phones.as_ref()).map(|(c, p)| {
                        c.union(p).cloned().collect::<BTreeSet<String>>()
                    });
                    status::write_if_changed(&dir, now, kept.as_ref(), sharing_on, &mut last);
                    last
                })
                .await
                .unwrap_or(None);
            }
        }
    }
    set_driven_by(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Key(u16, bool),
        Button(u8, bool),
        Wheel(i16, i16),
        Move(u16, u16),
    }

    #[derive(Clone, Default)]
    struct Fake(
        Arc<Mutex<Vec<Call>>>,
        Arc<Mutex<usize>>,
        Arc<Mutex<Vec<String>>>,
    );

    impl Fake {
        fn badges(&self) -> Vec<String> {
            std::mem::take(&mut *self.2.lock().unwrap())
        }
    }

    impl Fake {
        fn prepared(&self) -> usize {
            *self.1.lock().unwrap()
        }
    }

    impl Fake {
        fn take(&self) -> Vec<Call> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Inject for Fake {
        fn key(&mut self, code: u16, pressed: bool) {
            self.0.lock().unwrap().push(Call::Key(code, pressed));
        }
        fn button(&mut self, button: u8, pressed: bool) {
            self.0.lock().unwrap().push(Call::Button(button, pressed));
        }
        fn wheel(&mut self, dx: i16, dy: i16) {
            self.0.lock().unwrap().push(Call::Wheel(dx, dy));
        }
        fn move_abs(&mut self, x: u16, y: u16) {
            self.0.lock().unwrap().push(Call::Move(x, y));
        }
        fn prepare(&mut self) {
            *self.1.lock().unwrap() += 1;
        }
        fn badge(&mut self, badge: &badge::Badge, _how_long: Duration) {
            self.2.lock().unwrap().push(badge.text.clone());
        }
    }

    fn receiver() -> (Receiver, Fake) {
        let fake = Fake::default();
        (Receiver::new(Box::new(fake.clone())), fake)
    }

    fn key(code: u16, pressed: bool) -> Msg {
        Msg::Key { code, pressed }
    }

    /// The clipboard goes back only on a leave from the computer driving
    /// this one now, allowed to, with the setting on: never the leave a
    /// driver sends as it greets, nor one from another computer.
    #[test]
    fn only_the_drivers_leave_pushes_the_clipboard_back() {
        let leave = Msg::Leave;
        assert!(pushes_back(&leave, "pc", true, Some("pc"), true));
        // The greeting leave: nothing is driven.
        assert!(!pushes_back(&leave, "pc", true, None, true));
        // Another computer's leave while "pc" drives.
        assert!(!pushes_back(&leave, "other", true, Some("pc"), true));
        // Not allowed to drive this one, or the setting off here.
        assert!(!pushes_back(&leave, "pc", false, Some("pc"), true));
        assert!(!pushes_back(&leave, "pc", true, Some("pc"), false));
        // Anything but a leave.
        assert!(!pushes_back(
            &Msg::Enter { x: 0, y: 0 },
            "pc",
            true,
            Some("pc"),
            true
        ));
        assert!(!pushes_back(&Msg::ReleaseAll, "pc", true, Some("pc"), true));
    }

    /// Each key and move injected for the driving computer is timed from
    /// when it came off the link, and kept for it after it leaves: a stale
    /// move, a key out of range (counted, not timed) and what another
    /// computer sends are not; the next arrival starts over.
    #[test]
    fn keys_and_moves_are_timed_for_the_visit() {
        let (mut r, _fake) = receiver();
        let at = link::now_us();
        r.handle_arrived("pc", Msg::Enter { x: 0, y: 0 }, true, at);
        let mv = |seq| Msg::Move { seq, x: 1, y: 1 };
        r.handle_arrived("pc", mv(5), true, at);
        r.handle_arrived("pc", mv(4), true, at);
        r.handle_arrived("pc", key(30, true), true, at);
        r.handle_arrived("pc", key(0x300, true), true, at);
        r.handle_arrived("pc", key(30, false), true, at);
        r.handle_arrived("other", key(31, true), true, at);
        r.handle_arrived("pc", Msg::Leave, true, at);
        let timed: Vec<(bool, u32)> = r
            .timings_for("pc")
            .unwrap()
            .iter()
            .map(|t| (t.moved, t.n))
            .collect();
        assert_eq!(timed, [(true, 5), (false, 1), (false, 3)]);
        let first = r.timings_for("pc").unwrap()[0];
        assert_eq!(first.arrived_us, at);
        assert!(first.took_us < 1_000_000, "{first:?}");
        assert_eq!(r.timings_for("other"), None);
        // Untimed (the tests' way in) and a new arrival: nothing kept.
        r.handle("other", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("other", key(30, true), true);
        assert_eq!(r.timings_for("pc"), None);
        assert_eq!(r.timings_for("other"), Some(&[][..]));
    }

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    /// The owner's first live test: the pointer of the computer arriving
    /// was put on the very edge facing it, where this computer's own
    /// barrier sits; it is put a few pixels inside instead.
    #[test]
    fn an_arriving_pointer_is_put_inside_the_edge() {
        let one = [screen(0, 0, 1920, 1080)];
        let x = |px| layout::normalize(px, 0, 1920);
        let y = |px| layout::normalize(px, 0, 1080);
        // The left edge, halfway down: 4 pixels in, the height as it was.
        assert_eq!(arrival(&one, 0, 32768), (x(4), 32768));
        // The right edge and the top.
        assert_eq!(arrival(&one, 65535, 0), (x(1915), y(4)));
        // The bottom-left corner.
        assert_eq!(arrival(&one, 0, 65535), (x(4), y(1075)));
        // Inside already: as it came.
        assert_eq!(arrival(&one, 32768, 32768), (32768, 32768));
        assert_eq!(arrival(&one, x(4), y(4)), (x(4), y(4)));
        // Screens not known yet: as it came.
        assert_eq!(arrival(&[], 0, 0), (0, 0));
    }

    /// The edge is that of the screen it lands on, not the box around them:
    /// a shorter screen to the right of a taller one has its bottom edge
    /// halfway down the box.
    #[test]
    fn an_arrival_is_put_inside_the_screen_it_lands_on() {
        let two = [screen(0, 0, 1920, 1080), screen(1920, 0, 1280, 720)];
        let x = |px| layout::normalize(px, 0, 3200);
        let y = |px| layout::normalize(px, 0, 1080);
        // Onto the short screen's last row, from below.
        assert_eq!(arrival(&two, x(2500), y(719)), (x(2500), y(715)));
        // Onto the tall screen's last row: its own bottom.
        assert_eq!(arrival(&two, x(100), y(1079)), (x(100), y(1075)));
    }

    #[test]
    fn the_receiver_puts_an_arrival_inside_but_moves_as_sent() {
        let (mut r, fake) = receiver();
        r.set_screens(&[screen(0, 0, 1920, 1080)]);
        r.handle("pc", Msg::Enter { x: 0, y: 32768 }, true);
        r.handle(
            "pc",
            Msg::Move {
                seq: 1,
                x: 0,
                y: 32768,
            },
            true,
        );
        let inside = layout::normalize(4, 0, 1920);
        assert_eq!(
            fake.take(),
            vec![Call::Move(inside, 32768), Call::Move(0, 32768)]
        );
    }

    /// PR #552 review I1: a cursor left resting here (its computer still
    /// pinging and saying it is active) stops counting as driving after
    /// [`DRIVEN_FOR`], so this computer's own edges lead on again; its next
    /// input counts again.
    #[test]
    fn driven_counts_only_while_the_driving_computer_sends_input() {
        let (mut r, _fake) = receiver();
        let t0 = Instant::now();
        assert_eq!(r.driven_by(t0), None);
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        assert_eq!(r.driven_by(t0), Some("pc"));
        let resting = t0 + DRIVEN_FOR - Duration::from_millis(100);
        r.handle_at("pc", Msg::Ping { t_us: 1 }, true, resting);
        r.handle_at("pc", Msg::Active { idle_ms: 0 }, true, resting);
        assert_eq!(r.driven_by(resting), Some("pc"));
        let later = t0 + DRIVEN_FOR;
        assert_eq!(
            r.driven_by(later),
            None,
            "pings and activity notes do not count"
        );
        assert_eq!(r.driver(), Some("pc"), "its cursor is still here");
        r.handle_at("pc", Msg::Move { seq: 1, x: 9, y: 9 }, true, later);
        assert_eq!(r.driven_by(later), Some("pc"));
        r.handle_at("pc", Msg::Leave, true, later);
        assert_eq!(r.driven_by(later), None);
    }

    /// What says, before it is acted on, that a computer drives this one.
    #[test]
    fn its_arrival_and_its_input_say_it_drives_before_they_are_acted_on() {
        let key = Msg::Key {
            code: 30,
            pressed: true,
        };
        assert!(drives_now(&Msg::Enter { x: 0, y: 0 }, "pc", true, None));
        assert!(!drives_now(&Msg::Enter { x: 0, y: 0 }, "pc", false, None));
        assert!(drives_now(&key, "pc", true, Some("pc")));
        assert!(!drives_now(&key, "pc", true, Some("other")));
        assert!(!drives_now(&key, "pc", true, None));
        assert!(!drives_now(&Msg::Ping { t_us: 1 }, "pc", true, Some("pc")));
        assert!(!drives_now(&Msg::Leave, "pc", true, Some("pc")));
    }

    #[test]
    fn injects_keys_and_moves_from_the_driver() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 100, y: 200 }, true);
        r.handle(
            "pc",
            Msg::Move {
                seq: 1,
                x: 65535,
                y: 0,
            },
            true,
        );
        r.handle("pc", key(30, true), true);
        r.handle("pc", key(30, false), true);
        r.handle(
            "pc",
            Msg::Button {
                button: 2,
                pressed: true,
            },
            true,
        );
        r.handle("pc", Msg::Wheel { dx: -1, dy: 3 }, true);
        assert_eq!(r.driver(), Some("pc"));
        assert_eq!(
            fake.take(),
            vec![
                Call::Move(100, 200),
                Call::Move(65535, 0),
                Call::Key(30, true),
                Call::Key(30, false),
                Call::Button(2, true),
                Call::Wheel(-1, 3),
            ]
        );
        // A key or a button that is none is not passed on.
        r.handle("pc", key(KEY_MAX + 1, true), true);
        r.handle(
            "pc",
            Msg::Button {
                button: BUTTON_MAX + 1,
                pressed: true,
            },
            true,
        );
        assert_eq!(fake.take(), vec![]);
    }

    #[test]
    fn identify_only_from_an_allowed_computer() {
        let (mut r, fake) = receiver();
        r.set_name("Laptop");
        // A computer that may not drive this one cannot make it put
        // anything up.
        r.handle(
            "stranger",
            Msg::Identify {
                name: "Someone".to_string(),
            },
            false,
        );
        assert_eq!(fake.badges(), Vec::<String>::new());
        // One that may: this computer shows its own name.
        r.handle(
            "pc",
            Msg::Identify {
                name: "Desk".to_string(),
            },
            true,
        );
        assert_eq!(fake.badges(), ["This is Laptop"]);
        // It moves nothing and drives nothing.
        assert_eq!(r.driver(), None);
        assert_eq!(fake.take(), vec![]);
    }

    #[test]
    fn a_computer_entering_flashes_this_ones_name() {
        let (mut r, fake) = receiver();
        r.set_name("Laptop");
        r.handle("pc", Msg::Enter { x: 1, y: 2 }, true);
        assert_eq!(fake.badges(), ["Keyboard and mouse: Laptop"]);
        // Moves and keys flash nothing.
        r.handle("pc", Msg::Move { seq: 1, x: 3, y: 4 }, true);
        r.handle("pc", key(30, true), true);
        assert_eq!(fake.badges(), Vec::<String>::new());
        // Nor does one that may not drive this one.
        r.handle("stranger", Msg::Enter { x: 1, y: 2 }, false);
        assert_eq!(fake.badges(), Vec::<String>::new());
    }

    #[test]
    fn nothing_without_the_permission() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 1, y: 2 }, false);
        r.handle("pc", Msg::Move { seq: 1, x: 3, y: 4 }, false);
        r.handle("pc", key(30, true), false);
        r.handle("pc", Msg::Active { idle_ms: 0 }, false);
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), None);
        assert!(!r.wants_awake(Instant::now()));

        // Allowed, then the permission goes mid-drive: what it held is let
        // go of, and it drives no more.
        r.handle("pc", Msg::Enter { x: 1, y: 2 }, true);
        r.handle("pc", key(42, true), true);
        fake.take();
        r.handle("pc", key(30, true), false);
        assert_eq!(fake.take(), vec![Call::Key(42, false)]);
        assert_eq!(r.driver(), None);
    }

    #[test]
    fn stale_move_seq_is_dropped() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        fake.take();
        r.handle("pc", Msg::Move { seq: 5, x: 5, y: 5 }, true);
        r.handle("pc", Msg::Move { seq: 4, x: 4, y: 4 }, true);
        r.handle("pc", Msg::Move { seq: 5, x: 9, y: 9 }, true);
        r.handle("pc", Msg::Move { seq: 6, x: 6, y: 6 }, true);
        // The count wraps: u32::MAX comes before 0.
        r.handle(
            "pc",
            Msg::Move {
                seq: u32::MAX,
                x: 7,
                y: 7,
            },
            true,
        );
        assert_eq!(fake.take(), vec![Call::Move(5, 5), Call::Move(6, 6)]);
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        r.handle(
            "pc",
            Msg::Move {
                seq: u32::MAX,
                x: 1,
                y: 1,
            },
            true,
        );
        r.handle("pc", Msg::Move { seq: 0, x: 2, y: 2 }, true);
        assert_eq!(
            fake.take(),
            vec![Call::Move(0, 0), Call::Move(1, 1), Call::Move(2, 2)]
        );
    }

    #[test]
    fn leave_releases_what_was_held() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("pc", key(42, true), true);
        r.handle("pc", key(30, true), true);
        r.handle(
            "pc",
            Msg::Button {
                button: 0,
                pressed: true,
            },
            true,
        );
        fake.take();
        r.handle("pc", Msg::Leave, true);
        assert_eq!(
            fake.take(),
            vec![
                Call::Key(30, false),
                Call::Key(42, false),
                Call::Button(0, false)
            ]
        );
        assert_eq!(r.driver(), None);
        // Gone: nothing more of it is used.
        r.handle("pc", key(31, true), true);
        r.handle("pc", Msg::Move { seq: 9, x: 1, y: 1 }, true);
        assert_eq!(fake.take(), vec![]);

        // ReleaseAll lets go too, and it still drives.
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("pc", key(29, true), true);
        fake.take();
        r.handle("pc", Msg::ReleaseAll, true);
        assert_eq!(fake.take(), vec![Call::Key(29, false)]);
        assert_eq!(r.driver(), Some("pc"));
        // Leaving after it has nothing left to let go of.
        r.handle("pc", Msg::Leave, true);
        assert_eq!(fake.take(), vec![]);
    }

    #[test]
    fn link_loss_releases_injected() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("pc", key(42, true), true);
        r.handle(
            "pc",
            Msg::Button {
                button: 2,
                pressed: true,
            },
            true,
        );
        fake.take();
        // Another computer's link going changes nothing.
        r.link_lost("other");
        assert_eq!(fake.take(), vec![]);
        r.link_lost("pc");
        assert_eq!(
            fake.take(),
            vec![Call::Key(42, false), Call::Button(2, false)]
        );
        assert_eq!(r.driver(), None);
        r.link_lost("pc");
        assert_eq!(fake.take(), vec![]);
    }

    #[test]
    fn moves_only_from_the_computer_that_entered() {
        let (mut r, fake) = receiver();
        // Before anyone entered, nothing moves.
        r.handle("a", Msg::Move { seq: 1, x: 1, y: 1 }, true);
        r.handle("a", key(30, true), true);
        assert_eq!(fake.take(), vec![]);

        r.handle("a", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("a", key(42, true), true);
        r.handle("b", Msg::Move { seq: 2, x: 2, y: 2 }, true);
        r.handle("b", key(30, true), true);
        r.handle("b", Msg::Wheel { dx: 0, dy: 1 }, true);
        r.handle("b", Msg::Leave, true);
        r.handle("b", Msg::ReleaseAll, true);
        assert_eq!(fake.take(), vec![Call::Move(0, 0), Call::Key(42, true)]);
        assert_eq!(r.driver(), Some("a"));

        // Another computer entering takes over; what the first held is
        // let go of first.
        r.handle("b", Msg::Enter { x: 9, y: 9 }, true);
        assert_eq!(fake.take(), vec![Call::Key(42, false), Call::Move(9, 9)]);
        assert_eq!(r.driver(), Some("b"));
        r.handle("a", key(30, true), true);
        assert_eq!(fake.take(), vec![]);
    }

    /// BK10 review M3: the quick link is held while the computer driving
    /// sends input, let go after ten seconds without any (a pointer left
    /// resting here overnight), taken again by the next input, and let go
    /// as it leaves.
    #[test]
    fn the_quick_link_lets_go_after_ten_idle_seconds() {
        let (mut r, _fake) = receiver();
        let t0 = Instant::now();
        assert!(!r.wants_quick(t0));
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        assert!(r.wants_quick(t0));
        r.handle_at("pc", key(30, true), true, t0 + Duration::from_secs(5));
        // Pings and activity notes are not input.
        r.handle_at("pc", Msg::Active { idle_ms: 0 }, true, t0 + QUICK_FOR);
        assert!(r.wants_quick(t0 + QUICK_FOR));
        let idle = t0 + Duration::from_secs(5) + QUICK_FOR;
        assert!(!r.wants_quick(idle));
        r.handle_at("pc", Msg::Move { seq: 1, x: 5, y: 5 }, true, idle);
        assert!(r.wants_quick(idle));
        r.handle_at("pc", Msg::Leave, true, idle);
        assert!(!r.wants_quick(idle));
    }

    #[test]
    fn silent_sender_is_released_after_a_second() {
        let (mut r, fake) = receiver();
        let t0 = Instant::now();
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        r.handle_at("pc", key(42, true), true, t0);
        r.handle_at("pc", Msg::Active { idle_ms: 0 }, true, t0);
        fake.take();
        // Not yet: a little short of a second of silence.
        r.check_silence(t0 + SILENCE_RELEASE - Duration::from_millis(1), None);
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), Some("pc"));
        // A second of nothing: let go of once, as a Leave, and no longer
        // kept awake for it.
        r.check_silence(t0 + SILENCE_RELEASE, None);
        assert_eq!(fake.take(), vec![Call::Key(42, false)]);
        assert_eq!(r.driver(), None);
        assert!(!r.wants_awake(t0 + SILENCE_RELEASE));
        r.check_silence(t0 + SILENCE_RELEASE * 5, None);
        assert_eq!(fake.take(), vec![]);
        // What it sends later is ignored until it enters again.
        let later = t0 + SILENCE_RELEASE * 6;
        r.handle_at("pc", Msg::Move { seq: 9, x: 1, y: 1 }, true, later);
        r.handle_at("pc", key(30, true), true, later);
        assert_eq!(fake.take(), vec![]);
        r.handle_at("pc", Msg::Enter { x: 2, y: 2 }, true, later);
        r.handle_at("pc", key(30, true), true, later);
        assert_eq!(fake.take(), vec![Call::Move(2, 2), Call::Key(30, true)]);
    }

    /// Silence is counted from when the driving computer's messages came
    /// off the link, not from when this loop got to them: one that came
    /// while the loop was busy keeps the session, and one handled late
    /// counts from when it came.
    #[test]
    fn silence_is_counted_from_arrival() {
        let (mut r, fake) = receiver();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        r.handle_at("pc", key(42, true), true, t0);
        fake.take();
        // The loop stalled; a note came off the link at 0.9 s and waits.
        r.check_silence(ms(1500), Some(ms(900)));
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), Some("pc"));
        r.check_silence(ms(1900), Some(ms(900)));
        assert_eq!(fake.take(), vec![Call::Key(42, false)]);
        assert_eq!(r.driver(), None);

        // A note handled now that came off the link 1.5 s ago, and nothing
        // since: a second and more of silence already.
        let (mut r, fake) = receiver();
        let now_us = link::now_us();
        r.handle_arrived("pc", Msg::Enter { x: 0, y: 0 }, true, now_us - 1_600_000);
        r.handle_arrived("pc", key(42, true), true, now_us - 1_600_000);
        r.handle_arrived("pc", Msg::Active { idle_ms: 0 }, true, now_us - 1_500_000);
        fake.take();
        r.check_silence(Instant::now(), None);
        assert_eq!(fake.take(), vec![Call::Key(42, false)]);
        assert_eq!(r.driver(), None);
    }

    #[test]
    fn an_arrival_is_put_on_the_monotonic_clock() {
        let now = Instant::now();
        let at = arrived_at(1_000_300_000, now, 1_000_600_000);
        assert_eq!(now - at, Duration::from_millis(300));
        // A wall clock stepped back: never later than now.
        assert_eq!(arrived_at(1_000_600_000, now, 1_000_300_000), now);
    }

    #[test]
    fn steady_active_keeps_the_session() {
        let (mut r, fake) = receiver();
        let t0 = Instant::now();
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        r.handle_at("pc", key(42, true), true, t0);
        fake.take();
        let step = Duration::from_millis(100);
        for i in 1..=30u32 {
            let now = t0 + step * i;
            if i % 2 == 0 {
                r.handle_at("pc", Msg::Active { idle_ms: 0 }, true, now);
            }
            r.check_silence(now, None);
        }
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), Some("pc"));
        assert!(r.wants_awake(t0 + step * 30));
    }

    #[test]
    fn dropping_the_receiver_releases_what_is_held() {
        let (mut r, fake) = receiver();
        r.handle("pc", Msg::Enter { x: 0, y: 0 }, true);
        r.handle("pc", key(42, true), true);
        r.handle(
            "pc",
            Msg::Button {
                button: 0,
                pressed: true,
            },
            true,
        );
        fake.take();
        // The server stopping (or upgrading) mid-session.
        drop(r);
        assert_eq!(
            fake.take(),
            vec![Call::Key(42, false), Call::Button(0, false)]
        );
    }

    #[test]
    fn an_allowed_hello_prepares_the_injector() {
        let (mut r, fake) = receiver();
        let hello = || Msg::Hello {
            version: wire::VERSION,
            name: "pc".to_string(),
            screens: Vec::new(),
        };
        r.handle("pc", hello(), false);
        assert_eq!(fake.prepared(), 0);
        r.handle("pc", hello(), true);
        assert_eq!(fake.prepared(), 1);
        // Nothing is injected for it.
        assert_eq!(fake.take(), vec![]);
        r.prepare();
        assert_eq!(fake.prepared(), 2);
    }

    #[test]
    fn this_computers_screens_are_written_the_first_time_whichever_way() {
        let screen = Screen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // A Hello reads them first: written then, so the next look that
        // finds the same does not need to.
        let mut own = OwnScreens::default();
        assert!(own.is_unread());
        assert!(own.note(vec![screen]));
        assert!(!own.note(vec![screen]));
        assert_eq!(own.get(), &[screen]);
        // None at all is written too, the first time.
        let mut own = OwnScreens::default();
        assert!(own.note(Vec::new()));
        assert!(!own.note(Vec::new()));
        // A change is written; sharing turned off forgets them.
        assert!(own.note(vec![screen]));
        own.forget();
        assert!(own.is_unread());
        assert!(own.note(vec![screen]));
    }

    #[test]
    fn either_side_of_a_pairing_can_let_the_other_drive() {
        // Paired with this one: its permission in the trust store.
        let s = standing_of(true, true, None);
        assert!(s.computer && s.allowed);
        assert!(!standing_of(true, false, None).allowed);
        // This one paired with it: what computers.json keeps.
        let s = standing_of(false, false, Some(true));
        assert!(s.computer && s.allowed);
        let s = standing_of(false, false, Some(false));
        assert!(s.computer && !s.allowed);
        // Paired both ways: the trust store's word.
        assert!(!standing_of(true, false, Some(true)).allowed);
        // Neither: no computer of the owner's.
        let s = standing_of(false, false, None);
        assert!(!s.computer && !s.allowed);
    }

    #[test]
    fn a_computer_that_may_not_drive_is_told_when_it_tries() {
        let hello = Msg::Hello {
            version: wire::VERSION,
            name: "pc".to_string(),
            screens: Vec::new(),
        };
        assert!(refuses(&hello, false));
        assert!(refuses(&Msg::Enter { x: 0, y: 0 }, false));
        // Its moves and notes alone are not answered, one each.
        assert!(!refuses(&Msg::Move { seq: 1, x: 0, y: 0 }, false));
        assert!(!refuses(&Msg::Active { idle_ms: 0 }, false));
        assert!(!refuses(&key(30, true), false));
        // While it drives, anything is: its permission just went.
        assert!(refuses(&Msg::Move { seq: 1, x: 0, y: 0 }, true));
    }

    /// A cursor arriving over a relay is refused (the keyboard and mouse
    /// go over a direct link only); whether the link is direct is looked at
    /// only for an arrival.
    #[test]
    fn an_arrival_over_a_relay_is_refused() {
        let enter = Msg::Enter { x: 0, y: 0 };
        assert!(arrives_relayed(&enter, || false));
        assert!(!arrives_relayed(&enter, || true));
        let key = key(30, true);
        assert!(!arrives_relayed(&key, || panic!("not looked at for a key")));
    }

    /// The computer driving this one is let go of, and told why, when its
    /// link turns relayed, as when its permission or the setting goes.
    #[test]
    fn a_driver_whose_link_turned_relayed_is_refused() {
        assert_eq!(refusal_now(None, true, true), None);
        assert_eq!(refusal_now(None, true, false), Some(wire::REFUSE_RELAYED));
        // Its permission's word comes first.
        assert_eq!(
            refusal_now(Some(wire::REFUSE_NOT_ALLOWED), true, false),
            Some(wire::REFUSE_NOT_ALLOWED)
        );
        // No link at all is no refusal: it is let go of as gone.
        assert_eq!(refusal_now(None, false, false), None);
    }

    /// The status (computers.json and the trust store read, the file
    /// written) is kept each second while sharing is on, once as it turns
    /// off, and then only every half minute.
    #[test]
    fn the_status_is_kept_rarely_while_sharing_is_off() {
        let s = Duration::from_secs;
        // The first time, whatever the setting.
        assert!(status_due(false, None, None));
        assert!(status_due(true, None, None));
        // On: each second.
        assert!(status_due(true, Some(true), Some(s(1))));
        // Turned off: at once, to say so.
        assert!(status_due(false, Some(true), Some(s(1))));
        // Off: every half minute.
        assert!(!status_due(false, Some(false), Some(s(1))));
        assert!(!status_due(false, Some(false), Some(s(29))));
        assert!(status_due(false, Some(false), Some(STATUS_IDLE_EVERY)));
        // Turned on: at once.
        assert!(status_due(true, Some(false), Some(s(1))));
    }

    #[test]
    fn activity_keeps_awake_for_ten_seconds() {
        let (mut r, _fake) = receiver();
        let now = Instant::now();
        r.handle("pc", Msg::Active { idle_ms: 70_000 }, true);
        assert!(!r.wants_awake(now));
        r.handle("pc", Msg::Active { idle_ms: 200 }, true);
        assert!(r.wants_awake(Instant::now()));
        assert!(!r.wants_awake(Instant::now() + AWAKE_FOR));
    }

    /// BK6: only the computer driving this one asks for its sound, and its
    /// ask lasts its visit: a leave, a new arrival, its silence, its link
    /// or its permission going all end it, and it never moves anything.
    #[test]
    fn only_the_driver_asks_for_sound_and_for_its_visit_only() {
        let wants = |on| Msg::SoundWanted { on };
        let enter = Msg::Enter { x: 0, y: 0 };
        let (mut r, fake) = receiver();
        r.handle("pc", wants(true), true);
        assert_eq!(r.sound_for(), None, "not driving this one");
        r.handle("pc", enter.clone(), true);
        assert_eq!(r.sound_for(), None, "not asked this visit");
        r.handle("pc", wants(true), true);
        assert_eq!(r.sound_for(), Some("pc"));
        r.handle("other", wants(false), true);
        assert_eq!(r.sound_for(), Some("pc"), "another computer's word");
        r.handle("pc", wants(false), true);
        assert_eq!(r.sound_for(), None, "no longer");
        // The leave ends it.
        r.handle("pc", wants(true), true);
        r.handle("pc", Msg::Leave, true);
        assert_eq!(r.sound_for(), None, "left");
        // A new arrival forgets the last visit's ask, and another
        // computer arriving takes over without it.
        r.handle("pc", enter.clone(), true);
        r.handle("pc", wants(true), true);
        r.handle("pc", enter.clone(), true);
        assert_eq!(r.sound_for(), None, "arrived anew");
        r.handle("pc", wants(true), true);
        r.handle("other", enter.clone(), true);
        assert_eq!(r.sound_for(), None, "another computer drives now");
        // Its silence, its link and its permission end it.
        let t0 = Instant::now();
        r.handle_at("pc", enter.clone(), true, t0);
        r.handle_at("pc", wants(true), true, t0);
        r.check_silence(t0 + SILENCE_RELEASE, None);
        assert_eq!(r.sound_for(), None, "silent");
        r.handle("pc", enter.clone(), true);
        r.handle("pc", wants(true), true);
        r.link_lost("pc");
        assert_eq!(r.sound_for(), None, "its link went");
        r.handle("pc", enter, true);
        r.handle("pc", wants(true), true);
        r.handle("pc", wants(true), false);
        assert_eq!(r.sound_for(), None, "its permission went");
        // Asking moves and presses nothing.
        assert!(
            fake.take()
                .iter()
                .all(|call| matches!(call, Call::Move(0, 0))),
            "only the arrivals moved"
        );
    }

    /// Review M8: a hello is answered with the screens first (the answer
    /// the driving side waits for) and then the name, in that order.
    #[test]
    fn a_hello_is_answered_with_the_screens_then_the_name() {
        let screen = Screen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(
            hello_answer(&[screen], "Arch laptop"),
            [
                Msg::Screens(vec![screen]),
                Msg::Name {
                    name: "Arch laptop".to_string()
                }
            ]
        );
    }

    /// Review M3: this computer's screens are read on a task of their own,
    /// so asking never waits on the compositor, and asks made while a look
    /// is on its way make no queue behind it.
    #[tokio::test]
    async fn a_look_at_the_screens_never_holds_up_the_asker() {
        let looked = Arc::new(Mutex::new(0u32));
        let counted = looked.clone();
        let screen = Screen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let mut looks = ScreensLook::start_with(move || {
            let counted = counted.clone();
            async move {
                *counted.lock().unwrap() += 1;
                // A compositor slow to answer.
                tokio::time::sleep(Duration::from_millis(300)).await;
                vec![screen]
            }
        });
        let asked = Instant::now();
        for _ in 0..5 {
            looks.ask();
        }
        assert!(asked.elapsed() < Duration::from_millis(50), "asking waited");
        let seen = tokio::time::timeout(Duration::from_secs(2), looks.seen.recv())
            .await
            .unwrap();
        assert_eq!(seen, Some(vec![screen]));
        // The asks made while it looked were one at most, not five.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(*looked.lock().unwrap() <= 2, "{}", *looked.lock().unwrap());
    }

    /// BK8 live: where another computer's cursor arrived, its leave and its
    /// letting go are logged at debug, so "it came home where it crossed"
    /// can be checked from the log; a key never is (its code or anything).
    #[test]
    fn arrivals_leaves_and_letting_go_are_logged_and_keys_never() {
        let (mut r, _fake) = receiver();
        let (caught, guard) = super::super::caught_log::catch();
        r.handle("pc", Msg::Enter { x: 1234, y: 4321 }, true);
        r.handle("pc", key(47, true), true);
        r.handle("pc", Msg::ReleaseAll, true);
        r.handle("pc", Msg::Leave, true);
        drop(guard);
        let log = caught.text();
        assert!(log.contains("x=1234") && log.contains("y=4321"), "{log}");
        assert!(log.contains("let go of everything"), "{log}");
        assert!(log.contains("cursor left"), "{log}");
        assert!(!log.contains("47") && !log.contains("code"), "{log}");
    }

    /// BK8 live: a computer that only takes input never says hello, so its
    /// name comes in its own message, after its screens: it names the
    /// computer already kept, its screens left as they were; screens heard
    /// alone keep the name.
    #[test]
    fn a_name_heard_alone_names_the_computer_and_keeps_its_screens() {
        let screen = Screen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let mut file = shared_input::ScreensFile::default();
        let mut heard = Heard::new();
        note_heard(&mut heard, "laptop", None, &[screen]);
        keep_heard(&mut file, &heard, 1);
        assert_eq!(file.get("laptop").map(|c| c.name.as_str()), Some(""));
        let mut heard = Heard::new();
        note_name(&mut heard, "laptop", "Arch laptop");
        keep_heard(&mut file, &heard, 2);
        let kept = file.get("laptop").unwrap();
        assert_eq!(kept.name, "Arch laptop");
        assert_eq!(kept.screens, vec![screen]);
        // Its screens again, alone: the name stays.
        let mut heard = Heard::new();
        note_heard(&mut heard, "laptop", None, &[screen]);
        keep_heard(&mut file, &heard, 3);
        assert_eq!(file.get("laptop").unwrap().name, "Arch laptop");
        // A name from a computer not kept yet makes no entry without screens.
        let mut heard = Heard::new();
        note_name(&mut heard, "stranger", "Desk");
        keep_heard(&mut file, &heard, 4);
        assert!(file.get("stranger").is_none());
        // Both in one batch: kept together.
        let mut heard = Heard::new();
        note_heard(&mut heard, "desk", None, &[screen]);
        note_name(&mut heard, "desk", "Desk");
        keep_heard(&mut file, &heard, 5);
        assert_eq!(file.get("desk").unwrap().name, "Desk");
    }

    /// Both ends of two links on this machine, each direct (an answer goes
    /// only on a direct link).
    async fn two_direct_links() -> (
        Vec<iroh::Endpoint>,
        [iroh::endpoint::Connection; 2],
        iroh::endpoint::Connection,
    ) {
        let (a, b, first, _first_back) = link::tests::linked_pair().await;
        let (c, d, second, second_back) = link::tests::linked_pair().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !(link::is_direct(&first) && link::is_direct(&second)) {
            assert!(tokio::time::Instant::now() < deadline, "direct in time");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (vec![a, b, c, d], [first, second], second_back)
    }

    /// When another computer restarts and links again, its new link
    /// replaces the old one, which lives on here until it times out (about
    /// 45 s, the other end gone without a word). This computer's answers to
    /// it (screens, refusals, timings) go on the new link at once, not on
    /// the old one into nothing.
    #[tokio::test]
    async fn an_answer_goes_on_the_link_that_replaced_the_old_one() {
        let id = "test-answer-relinked";
        let (ends, [first, second], second_back) = two_direct_links().await;
        link::register(id, first);
        let mut channels = HashMap::new();
        send_to(&mut channels, id, &Msg::Leave);
        assert!(
            channels.contains_key(id),
            "a channel kept for the next answer"
        );
        link::register(id, second);
        send_to(&mut channels, id, &Msg::Leave);
        let mut stream = tokio::time::timeout(Duration::from_secs(5), second_back.accept_uni())
            .await
            .expect("the answer on the new link at once")
            .unwrap();
        let mut head = [0u8; 7];
        stream.read_exact(&mut head).await.unwrap();
        assert_eq!(head[..4], wire::STREAM_MAGIC);
        assert_eq!(head[4], wire::VERSION);
        let len = usize::from(u16::from_be_bytes([head[5], head[6]]));
        let mut body = vec![0u8; len];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(wire::decode(&body).unwrap(), Msg::Leave);
        link::unregister(id);
        drop(channels);
        for end in ends {
            end.close().await;
        }
    }
}
