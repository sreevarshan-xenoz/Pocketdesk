//! The computer being driven: what another of the owner's computers sends it
//! on their link, typed and moved here.
//!
//! Nothing is used unless the shared keyboard and mouse is on here
//! (`shared_input`), the sender is one of the owner's paired computers, and
//! it has the Keyboard and mouse permission ([`Capability::SharedInput`]).
//! Only the computer whose cursor came onto this one's screens
//! ([`Msg::Enter`]) moves it, presses its keys and buttons, and turns its
//! wheel; everything it pressed is let go of when it leaves, when it asks,
//! when its link goes, and when its permission does.
//!
//! [`Capability::SharedInput`]: pocketdesk_core::protocol::permissions::Capability::SharedInput

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pocketdesk_core::local::shared_input;
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::streaming::input_packet::InputPacket;

use super::awake::KeepAwake;
use super::held::HeldInput;
use super::link::{self, Channel};
use super::screens;
use super::wire::{self, Msg, Screen};
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
        tracing::debug!("Shared input: {packet:?} was not injected: {e:#}");
    }
}

/// The highest evdev key code (`KEY_MAX`); one past it is no key.
const KEY_MAX: u16 = 0x2ff;

/// The highest button there is (forward).
const BUTTON_MAX: u8 = 4;

/// An activity note this recent keeps this computer awake.
const AWAKE_FOR: Duration = Duration::from_secs(10);

/// Someone idle this long at the driving computer does not.
const IDLE_MS_AWAKE: u32 = 60_000;

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
    /// When anything last came from the driving computer.
    heard_at: Option<Instant>,
}

impl Receiver {
    pub fn new(inject: Box<dyn Inject>) -> Receiver {
        Receiver {
            inject,
            driver: None,
            held: HeldInput::default(),
            last_seq: None,
            active_at: None,
            heard_at: None,
        }
    }

    /// The computer driving this one now.
    pub fn driver(&self) -> Option<&str> {
        self.driver.as_deref()
    }

    /// Act on `msg` from the computer `from`; `allowed` is whether it may
    /// drive this one (shared input on, one of the owner's computers, with
    /// the permission).
    pub fn handle(&mut self, from: &str, msg: Msg, allowed: bool) {
        self.handle_at(from, msg, allowed, Instant::now());
    }

    /// [`Receiver::handle`], with `now` the time it arrived.
    pub fn handle_at(&mut self, from: &str, msg: Msg, allowed: bool, now: Instant) {
        if !allowed {
            // Its permission went (or the feature did) while it drove.
            self.link_lost(from);
            return;
        }
        // Anything at all from the driving computer says it is still there.
        if self.driver.as_deref() == Some(from) {
            self.heard_at = Some(now);
        }
        if let Msg::Active { idle_ms } = msg {
            if idle_ms < IDLE_MS_AWAKE {
                self.active_at = Some(now);
            }
            return;
        }
        if let Msg::Enter { x, y } = msg {
            // A fresh start, whoever enters: what an earlier visit left
            // down is let go of first.
            self.release_held();
            self.driver = Some(from.to_string());
            self.heard_at = Some(now);
            self.last_seq = None;
            self.inject.move_abs(x, y);
            return;
        }
        if self.driver.as_deref() != Some(from) {
            return;
        }
        match msg {
            Msg::Leave => self.stop(),
            Msg::Move { seq, x, y } => {
                // Newer by the count, which may wrap.
                let newer = self
                    .last_seq
                    .is_none_or(|last| (seq.wrapping_sub(last) as i32) > 0);
                if newer {
                    self.last_seq = Some(seq);
                    self.inject.move_abs(x, y);
                }
            }
            Msg::Key { code, pressed } if code <= KEY_MAX => {
                self.held.note(&msg);
                self.inject.key(code, pressed);
            }
            Msg::Button { button, pressed } if button <= BUTTON_MAX => {
                self.held.note(&msg);
                self.inject.button(button, pressed);
            }
            Msg::Wheel { dx, dy } => self.inject.wheel(dx, dy),
            Msg::ReleaseAll => self.release_held(),
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
    /// before the connection times out.
    pub fn check_silence(&mut self, now: Instant) {
        let silent = self
            .heard_at
            .is_some_and(|at| now.saturating_duration_since(at) >= SILENCE_RELEASE);
        if self.driver.is_some() && silent {
            self.stop();
            self.active_at = None;
        }
    }

    fn stop(&mut self) {
        self.release_held();
        self.driver = None;
        self.heard_at = None;
        self.last_seq = None;
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
}

/// The injector, made when it is first needed: a server whose computers
/// never drive it makes no virtual devices. One that cannot be made is
/// tried again a while later.
#[derive(Default)]
struct LazyInjector {
    injector: Option<InputInjector>,
    failed_at: Option<Instant>,
}

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
    /// And it paired with this one and has the Keyboard and mouse
    /// permission here: it may drive this one.
    allowed: bool,
}

/// The setting and each computer's standing, read again at most once a
/// [`RECHECK`] (they are files: config.toml and the trust store).
#[derive(Default)]
struct Gate {
    on: Option<(bool, Instant)>,
    standings: HashMap<String, (Standing, Instant)>,
}

impl Gate {
    async fn on(&mut self) -> bool {
        if let Some((on, at)) = self.on
            && at.elapsed() < RECHECK
        {
            return on;
        }
        let on = tokio::task::spawn_blocking(|| crate::events::current_config().shared_input)
            .await
            .unwrap_or(false);
        self.on = Some((on, Instant::now()));
        on
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
        let standing = tokio::task::spawn_blocking(move || {
            // A permission profile could carry the flag onto a phone: only
            // the owner's computers drive this one, those that paired with
            // this one (the trust store's).
            let trusted = crate::plugins::pair::is_computer_device(&id);
            // One this computer paired with is heard too (its screens):
            // this one drives it, whether or not it may drive this one.
            let paired_with = trusted
                || crate::state::state_dir().is_ok_and(|dir| {
                    pocketdesk_core::local::computers::read(&dir)
                        .computers
                        .iter()
                        .any(|c| c.device_id == id)
                });
            Standing {
                computer: paired_with,
                allowed: trusted && crate::plugins::pair::allows(&id, Capability::SharedInput),
            }
        })
        .await
        .unwrap_or_default();
        self.standings
            .insert(device_id.to_string(), (standing, Instant::now()));
        standing
    }
}

/// What other computers said of their screens and is not written yet, by
/// computer: the name it gave (when it gave one) and its screens, clipped.
type Heard = HashMap<String, (Option<String>, Vec<Screen>)>;

fn note_heard(heard: &mut Heard, from: &str, name: Option<&str>, screens: &[Screen]) {
    let entry = heard.entry(from.to_string()).or_default();
    if let Some(name) = name {
        entry.0 = Some(shared_input::clean_name(name));
    }
    entry.1 = shared_input::clean_screens(screens);
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
        shared_input::update(&dir, |file| {
            if let (Some(me), Some(own)) = (&me, &own) {
                file.set_own(&me.device_id, &me.device_name, own, now);
            }
            for (id, (name, screens)) in &heard {
                file.heard(id, name.as_deref(), screens, now);
            }
        })
    })
    .await;
    if let Ok(Err(e)) = written {
        tracing::debug!("Shared input: the screens were not written: {e}");
    }
}

/// Tell the computer `to` this computer's `screens`, on the channel kept
/// for it (opened the first time; dropped when a send fails, so the next
/// opens it again).
fn tell_screens(channels: &mut HashMap<String, Channel>, to: &str, screens: &[Screen]) {
    if !channels.contains_key(to) {
        match Channel::open(to) {
            Ok(channel) => {
                channels.insert(to.to_string(), channel);
            }
            Err(e) => {
                tracing::debug!(to, "Shared input: this computer's screens not sent: {e}");
                return;
            }
        }
    }
    let msg = Msg::Screens(screens.to_vec());
    if let Some(channel) = channels.get_mut(to)
        && let Err(e) = channel.send(&msg)
    {
        tracing::debug!(to, "Shared input: this computer's screens not sent: {e}");
        channels.remove(to);
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
    let mut awake = KeepAwake::default();
    let mut gate = Gate::default();
    // This computer's screens as last looked at (none while shared input
    // is off), and the computers told them.
    let mut own: Option<Vec<Screen>> = None;
    let mut channels: HashMap<String, Channel> = HashMap::new();
    let mut heard = Heard::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut look = tokio::time::interval(SCREENS_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            arrival = arriving.recv() => {
                let Some((from, msg)) = arrival else { break };
                let standing = gate.standing(&from).await;
                match &msg {
                    Msg::Hello { version, name, screens } if standing.computer => {
                        note_heard(&mut heard, &from, Some(name), screens);
                        if standing.allowed && *version == wire::VERSION {
                            if own.is_none() {
                                own = Some(screens::current().await);
                            }
                            tell_screens(&mut channels, &from, own.as_deref().unwrap_or_default());
                        }
                    }
                    Msg::Screens(screens) if standing.computer => {
                        note_heard(&mut heard, &from, None, screens);
                    }
                    _ => {}
                }
                if standing.allowed && matches!(msg, Msg::Enter { .. }) {
                    tracing::info!(from, "Shared input: another computer drives this one");
                }
                receiver.handle(&from, msg, standing.allowed);
            }
            _ = tick.tick() => {
                if let Some(driver) = receiver.driver().map(str::to_string) {
                    receiver.check_silence(Instant::now());
                    if receiver.driver().is_none() {
                        tracing::info!(
                            driver,
                            "Shared input: the driving computer went silent; what it held is let go of"
                        );
                    }
                }
                if let Some(driver) = receiver.driver().map(str::to_string) {
                    let linked = link::linked().contains(&driver);
                    if !linked || !gate.standing(&driver).await.allowed {
                        tracing::info!(
                            driver,
                            linked,
                            "Shared input: the driving computer's link or permission went; \
                             what it held is let go of"
                        );
                        receiver.link_lost(&driver);
                    }
                }
                if receiver.wants_awake(Instant::now()) {
                    awake.hold();
                } else {
                    awake.release();
                }
                if !heard.is_empty() {
                    write_screens(state_dir.as_ref(), std::mem::take(&mut heard), None).await;
                }
            }
            _ = look.tick() => {
                if !gate.on().await {
                    own = None;
                    channels.clear();
                    continue;
                }
                let now = screens::current().await;
                if own.as_ref() != Some(&now) {
                    write_screens(state_dir.as_ref(), Heard::new(), Some(now.clone())).await;
                    own = Some(now);
                    let told: Vec<String> = channels.keys().cloned().collect();
                    for to in told {
                        if gate.standing(&to).await.allowed {
                            tell_screens(&mut channels, &to, own.as_deref().unwrap_or_default());
                        } else {
                            channels.remove(&to);
                        }
                    }
                }
            }
        }
    }
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
    struct Fake(Arc<Mutex<Vec<Call>>>);

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
    }

    fn receiver() -> (Receiver, Fake) {
        let fake = Fake::default();
        (Receiver::new(Box::new(fake.clone())), fake)
    }

    fn key(code: u16, pressed: bool) -> Msg {
        Msg::Key { code, pressed }
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

    #[test]
    fn silent_sender_is_released_after_a_second() {
        let (mut r, fake) = receiver();
        let t0 = Instant::now();
        r.handle_at("pc", Msg::Enter { x: 0, y: 0 }, true, t0);
        r.handle_at("pc", key(42, true), true, t0);
        r.handle_at("pc", Msg::Active { idle_ms: 0 }, true, t0);
        fake.take();
        // Not yet: a little short of a second of silence.
        r.check_silence(t0 + SILENCE_RELEASE - Duration::from_millis(1));
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), Some("pc"));
        // A second of nothing: let go of once, as a Leave, and no longer
        // kept awake for it.
        r.check_silence(t0 + SILENCE_RELEASE);
        assert_eq!(fake.take(), vec![Call::Key(42, false)]);
        assert_eq!(r.driver(), None);
        assert!(!r.wants_awake(t0 + SILENCE_RELEASE));
        r.check_silence(t0 + SILENCE_RELEASE * 5);
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
            r.check_silence(now);
        }
        assert_eq!(fake.take(), vec![]);
        assert_eq!(r.driver(), Some("pc"));
        assert!(r.wants_awake(t0 + step * 30));
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
}
