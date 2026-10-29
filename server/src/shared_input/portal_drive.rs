//! Driving another computer from Linux (BK8): the loop that runs a
//! [`Driver`] on what the input-capture portal says ([`PortalGlue`]), and
//! its supervisor. The links to the other computers and the phones, the
//! pings, the sound that follows the cursor, the greeting and the refusals
//! are kept as Windows' loop keeps them (`capture_windows`); what differs:
//! - this computer's own input comes from the portal's session
//!   (`capture_linux`), started with the loop and ended with it;
//! - its screens are the portal's zones once it said them, else what
//!   [`screens::current`] reads;
//! - the gaming lock looks at Hyprland's window in front (full screen),
//!   the only desktop that says it; elsewhere nothing holds the edges;
//! - files dragged across the edge are Windows' only (BK4).
//!
//! The loop is written over any [`Backend`], so it builds and is linted on
//! every system; only Linux starts it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pocketdesk_core::local::shared_input::PHONE_READY;

use super::awake::KeepAwake;
use super::badge;
use super::drive::{Action, Captured, Driver, OpenChannel, SavedLayout, layout_for, reachable};
use super::game_lock::Foreground;
use super::link::{self, Channel};
use super::phone::{self, PhoneChannel};
use super::portal_glue::{Backend, Portal, PortalGlue, Standing};
use super::quick::Nudge;
use super::screens;
use super::sound;
use super::status;
use super::wire::{self, Msg};
use crate::hyprland::HyprWindow;

/// How often what the owner does is told to every computer with a channel.
const ACTIVE_EVERY: Duration = Duration::from_millis(200);
/// How often, while away, the link is looked at.
const LINK_EVERY: Duration = Duration::from_millis(250);
/// How often the screens and the computers are read again (and a capture
/// the desktop disabled is enabled again).
const LAYOUT_EVERY: Duration = Duration::from_secs(2);
/// How long after the driver ended by itself it is started again.
const RESTART_AFTER: Duration = Duration::from_secs(30);
/// How long after it found no input-capture portal it looks again (a
/// desktop updated, or another session).
const LOOK_AGAIN_AFTER: Duration = Duration::from_secs(600);
/// How long after a channel could not be opened it is tried again.
const REOPEN_AFTER: Duration = Duration::from_secs(5);
/// How often what is in front is looked at, for the gaming lock.
const GAME_EVERY: Duration = Duration::from_millis(500);
/// How long Hyprland may take to say what is in front (it answers in a
/// millisecond or two; the loop waits on it).
const PROBE_WAIT: Duration = Duration::from_millis(100);
/// How long, as a driver ends, what it just sent may take to go.
const FLUSH_WAIT: Duration = Duration::from_millis(300);

/// What Hyprland's window in front says for the gaming lock: full screen
/// (Hyprland 0.42 and later say 2 for it, 3 with maximized too; 1 is only
/// maximized) and its class as the program.
pub fn foreground_of(window: Option<&HyprWindow>) -> Foreground {
    window.map_or_else(Foreground::default, |w| Foreground {
        fullscreen: w.fullscreen >= 2,
        cursor_captured: false,
        program: w.class.clone(),
        elevated: false,
        cursor: None,
    })
}

/// Whether one of Hyprland's screens is scaled by a fraction (1.25, 1.5):
/// xdg-desktop-portal-hyprland bug 435 makes an edge there flip.
pub fn fractional(monitors: &[crate::hyprland::HyprMonitor]) -> bool {
    monitors
        .iter()
        .any(|m| m.scale.is_finite() && (m.scale - m.scale.round()).abs() > 1e-3)
}

/// How the portal stands as the owner is told: one that captures on
/// Hyprland with a fractionally scaled screen is warned about it before
/// an edge has flipped.
pub fn shown(standing: Standing, fractional_screen: bool) -> Standing {
    match standing {
        Standing::Ready if fractional_screen => Standing::FractionalScale,
        other => other,
    }
}

/// Whether Hyprland has a fractionally scaled screen now (false off
/// Hyprland, or when it does not say in time).
fn fractional_now(runtime: &tokio::runtime::Handle) -> bool {
    runtime.block_on(async {
        let Ok(ipc) = crate::hyprland::HyprlandIpc::from_env() else {
            return false;
        };
        matches!(
            tokio::time::timeout(PROBE_WAIT, ipc.monitors()).await,
            Ok(Ok(monitors)) if fractional(&monitors)
        )
    })
}

/// What is in front now, for the driver: nothing probed with the gaming
/// lock off, or off Hyprland.
fn foreground_now(runtime: &tokio::runtime::Handle, game_lock: bool) -> Captured {
    let fg = if game_lock {
        let window = runtime.block_on(async {
            let ipc = crate::hyprland::HyprlandIpc::from_env().ok()?;
            tokio::time::timeout(PROBE_WAIT, ipc.active_window())
                .await
                .ok()?
                .ok()
                .flatten()
        });
        foreground_of(window.as_ref())
    } else {
        Foreground::default()
    };
    Captured::Foreground {
        fg,
        kernel_anti_cheat: false,
    }
}

/// The loop's state besides the driver: this computer's input (the
/// portal's glue), the channels and the keep-awake.
struct Glue<B: Backend> {
    input: PortalGlue<B>,
    channels: HashMap<String, Channel>,
    /// When each channel was opened (before its `Hello`).
    opened: HashMap<String, Instant>,
    /// When opening a channel to a computer last failed, or it last
    /// refused, and how long it waits to be greeted again.
    refused: HashMap<String, (Instant, Duration)>,
    refusals: status::RefusalLog,
    /// This computer's name and screens, for a `Hello`.
    name: String,
    own: Vec<wire::Screen>,
    awake: KeepAwake,
    /// The owner's phones in the layout (BK5), their channels, and how
    /// many pings each had answered when last looked at.
    phone_ids: HashSet<String>,
    phones: HashMap<String, PhoneChannel>,
    pongs_seen: HashMap<String, u64>,
    /// The sound of the computer driven (BK6).
    sound_on: bool,
    asking: sound::Asking,
    listening: Option<sound::Listening>,
    told_unavailable: sound::Unavailable,
    /// A game holds the keyboard and mouse here: no badge over it.
    game_held: bool,
}

impl<B: Backend> Glue<B> {
    /// The channel to `to`, opened (and greeted) if there is none.
    fn channel(&mut self, to: &str) -> Option<&mut Channel> {
        if !self.channels.contains_key(to) {
            let opened = Instant::now();
            match Channel::open(to) {
                Ok(mut channel) => {
                    let hello = Msg::Hello {
                        version: wire::VERSION,
                        name: self.name.clone(),
                        screens: self.own.clone(),
                    };
                    if channel.send(&hello).and(channel.send(&Msg::Leave)).is_err() {
                        self.refused
                            .insert(to.to_string(), (Instant::now(), REOPEN_AFTER));
                        return None;
                    }
                    self.refused.remove(to);
                    self.opened.insert(to.to_string(), opened);
                    self.channels.insert(to.to_string(), channel);
                }
                Err(e) => {
                    tracing::debug!(to, "Shared input: no channel: {e}");
                    self.refused
                        .insert(to.to_string(), (Instant::now(), REOPEN_AFTER));
                    return None;
                }
            }
        }
        self.channels.get_mut(to)
    }

    /// Leave each computer (`only` that one) that refused to be driven
    /// since its channel opened, and greet it again after a wait.
    fn take_refusals(&mut self, driver: &mut Driver, only: Option<&str>) {
        let refusals = link::refusals();
        let refusing: Vec<(String, u8)> = self
            .channels
            .keys()
            .filter(|id| only.is_none_or(|only| only == id.as_str()))
            .filter_map(|id| {
                let (reason, at) = refusals.get(id)?;
                (*at >= *self.opened.get(id)?).then(|| (id.clone(), *reason))
            })
            .collect();
        for (id, reason) in refusing {
            let actions = driver.refused(&id, reason);
            self.apply(driver, actions);
            self.channels.remove(&id);
            self.refusals.note(&id, reason);
            let wait = self.refusals.backoff(&id);
            self.refused.insert(id, (Instant::now(), wait));
        }
    }

    /// Do what the driver wants: this computer's input through the
    /// portal's glue, the rest here.
    fn apply(&mut self, driver: &mut Driver, actions: Vec<Action>) {
        let mut queue = std::collections::VecDeque::from(actions);
        while let Some(action) = queue.pop_front() {
            if matches!(action, Action::WarpHome(..)) {
                // An image still on its way to the computer left goes no
                // further (BK3).
                crate::clipboard_push::came_home();
            }
            for action in self.input.act(vec![action]) {
                match action {
                    Action::Send(to, msg) if self.phone_ids.contains(&to) => {
                        let channel = self
                            .phones
                            .entry(to.clone())
                            .or_insert_with(|| PhoneChannel::open(&to));
                        if let Err(e) = channel.send(&msg) {
                            tracing::debug!("Shared input: not sent to the phone: {e}");
                            self.phones.remove(&to);
                            if driver.away_on() == Some(to.as_str()) {
                                queue.retain(|a| not_for(a, &to));
                                queue.extend(driver.on_send_error());
                            }
                        }
                    }
                    Action::Send(to, msg) => {
                        // The computer driven is never greeted anew
                        // mid-visit: a new channel's leave would end it.
                        let channel = if driver.away_on() != Some(to.as_str()) {
                            self.channel(&to)
                        } else {
                            self.channels.get_mut(&to)
                        };
                        let sent = channel
                            .map(|channel| channel.send(&msg))
                            .unwrap_or_else(|| Err("no channel".to_string()));
                        if let Err(e) = sent {
                            tracing::debug!(to, "Shared input: not sent: {e}");
                            self.channels.remove(&to);
                            if driver.away_on() == Some(to.as_str()) {
                                queue.retain(|a| not_for(a, &to));
                                queue.extend(driver.on_send_error());
                            }
                        }
                    }
                    Action::KeepAwake(true) => self.awake.hold(),
                    Action::KeepAwake(false) => self.awake.release(),
                    Action::Ping(to) => queue.push_front(Action::Send(
                        to,
                        Msg::Ping {
                            t_us: link::now_us(),
                        },
                    )),
                    // A phone takes no clipboard this way (BK5).
                    Action::PushClipboard(to) if self.phone_ids.contains(&to) => {}
                    Action::PushClipboard(to) => {
                        tokio::spawn(async move { crate::clipboard_push::push_now(&to).await });
                    }
                    Action::Badge(said_here) => {
                        let said = said_here.text.lines().collect::<Vec<_>>().join("; ");
                        tracing::info!("Shared input: {said}");
                        if !self.game_held {
                            badge::flash(&said_here, badge::SWITCH_FOR);
                        }
                    }
                    // Win+L is Windows' (its hook spots it); the mode, the
                    // park and the cursor are the portal glue's.
                    Action::LockHere
                    | Action::SetMode(_)
                    | Action::ParkCursor(..)
                    | Action::WarpHome(..)
                    | Action::HideCursor(_) => {}
                }
            }
        }
    }

    /// The sound of the computer driven plays here while the cursor is on
    /// it (BK6), as on Windows.
    fn follow_sound(&mut self, driver: &mut Driver) {
        let want = sound::want_for(driver.away_on(), driver.visit(), self.sound_on, |id| {
            self.phone_ids.contains(id)
        });
        let want = want.as_ref().map(|(to, visit)| (to.as_str(), *visit));
        for step in self.asking.want(want) {
            match step {
                sound::Step::Unask(from) => {
                    self.listening = None;
                    if let Some(channel) = self.channels.get_mut(&from)
                        && let Err(e) = channel.send(&Msg::SoundWanted { on: false })
                    {
                        tracing::debug!(
                            from,
                            "Shared input: the end of the sound was not sent: {e}"
                        );
                    }
                }
                sound::Step::Ask(to) => {
                    self.listening = Some(sound::Listening::start(&to));
                    self.apply(
                        driver,
                        vec![Action::Send(to, Msg::SoundWanted { on: true })],
                    );
                }
            }
        }
        if let Some(to) = self.asking.asked().map(str::to_string)
            && let Some(why) = sound::take_unavailable(&to)
            && self.told_unavailable.worth_saying(
                &to,
                link::connection(&to).map(|conn| conn.stable_id()),
                &why,
            )
        {
            let words = format!("No sound from {}: {why}", driver.name_of(&to));
            let note = badge::of(&badge::Event::Note(&words));
            self.apply(driver, vec![Action::Badge(note)]);
        }
    }
}

/// Whether `action` is not something for `to` (dropped once a send to it
/// failed).
fn not_for(action: &Action, to: &str) -> bool {
    match action {
        Action::Send(t, _) | Action::PushClipboard(t) => t != to,
        _ => true,
    }
}

/// Set once the server is stopping: no driver starts again, and the one
/// running brings the keyboard and mouse home and ends ([`stop`]).
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Set while a driver's thread runs.
static DRIVING: AtomicBool = AtomicBool::new(false);

/// `DRIVING` set while this is held, and cleared however its holder ends.
struct Driving;

impl Driving {
    fn raise() -> Driving {
        DRIVING.store(true, Ordering::SeqCst);
        Driving
    }
}

impl Drop for Driving {
    fn drop(&mut self) {
        DRIVING.store(false, Ordering::SeqCst);
    }
}

/// The server is stopping: the driver running brings the keyboard and
/// mouse home and ends, and none starts again. Waits at most `within`;
/// whether it ended in time.
pub async fn stop(within: Duration) -> bool {
    STOPPING.store(true, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + within;
    while DRIVING.load(Ordering::SeqCst) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// How a portal session is started: its backend, what it says going to
/// the sender (`capture_linux::start` on Linux).
pub type Start<B> = fn(Option<PathBuf>, Sender<Portal>) -> Option<B>;

/// The driver while `stop` is not set, on a thread of its own inside
/// `runtime`: how the portal stood as it ended.
fn drive<B: Backend>(
    stop: Arc<AtomicBool>,
    runtime: tokio::runtime::Handle,
    start: Start<B>,
) -> Standing {
    let _driving = Driving::raise();
    let _runtime = runtime.enter();
    let config = crate::events::current_config();
    let hotkey = pocketdesk_core::local::hotkey::parse(&config.shared_input_hotkey);
    let state_dir = crate::state::state_dir().ok();
    let (said, events): (Sender<Portal>, Receiver<Portal>) = mpsc::channel();
    let Some(backend) = start(state_dir.clone(), said) else {
        return Standing::Starting;
    };
    let me = crate::kde::host_identity();
    let own = runtime.block_on(screens::current());
    let mut glue = Glue {
        input: PortalGlue::new(&me.device_id, backend, hotkey),
        channels: HashMap::new(),
        opened: HashMap::new(),
        refused: HashMap::new(),
        refusals: status::RefusalLog::default(),
        name: me.device_name.clone(),
        own,
        awake: KeepAwake::default(),
        phone_ids: HashSet::new(),
        phones: HashMap::new(),
        pongs_seen: HashMap::new(),
        sound_on: config.shared_input_sound,
        asking: sound::Asking::default(),
        listening: None,
        told_unavailable: sound::Unavailable::default(),
        game_held: false,
    };
    let mut saved = SavedLayout::default();
    let mut read_layout = |glue: &mut Glue<B>| {
        let file = state_dir
            .as_deref()
            .map(pocketdesk_core::local::shared_input::read)
            .unwrap_or_default();
        glue.phone_ids = file
            .computers
            .iter()
            .filter(|c| c.phone.is_some() && !c.this_computer)
            .map(|c| c.device_id.clone())
            .collect();
        let mut linked = link::linked();
        linked.extend(
            glue.phone_ids
                .iter()
                .filter(|id| phone::is_linked(id))
                .cloned(),
        );
        let mut layout = layout_for(
            &me.device_id,
            &me.device_name,
            glue.own.clone(),
            &file,
            &linked,
            state_dir.as_deref().and_then(|dir| saved.get(dir)),
        );
        // Each named as the owner paired it, whatever it calls itself
        // (one that only takes input never said its name): the badge
        // never says "this computer" of another.
        if layout.computers.len() > 1 {
            let paired = state_dir
                .as_deref()
                .map(status::paired_names)
                .unwrap_or_default();
            layout.name_as_paired(&paired);
        }
        layout
    };
    let mut game_lock_on = config.shared_input_game_lock;
    // This computer's Wi-Fi kept awake toward the computer driven while the
    // link is kept quick (BK10).
    let mut quick_on = config.shared_input_low_latency;
    let mut nudge = Nudge::default();
    let mut driver = Driver::new(&me.device_id, read_layout(&mut glue))
        .with_game_lock(game_lock_on, config.shared_input_lock_programs.clone());
    driver.set_clipboard(config.shared_input_clipboard);
    let actions = driver.on(foreground_now(&runtime, game_lock_on));
    glue.apply(&mut driver, actions);
    let started = driver.start();
    glue.apply(&mut driver, started);
    tracing::info!("Shared input: this computer's keyboard and mouse may go to the others");

    let mut last_input = Instant::now();
    let (mut active_at, mut link_at, mut game_at) =
        (Instant::now(), Instant::now(), Instant::now());
    // The first layout pass at once, with the computers' screens.
    let mut layout_at: Option<Instant> = None;
    let mut greeted_at: Option<Instant> = None;
    let mut fractional_screen = false;
    while !stop.load(Ordering::SeqCst) && !STOPPING.load(Ordering::SeqCst) {
        match events.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                if matches!(
                    event,
                    Portal::Motion { .. }
                        | Portal::Button { .. }
                        | Portal::Scroll { .. }
                        | Portal::Key { .. }
                        | Portal::Activated { .. }
                ) {
                    last_input = Instant::now();
                }
                // New zones: this computer's screens, said and laid out
                // again at once.
                if matches!(event, Portal::Zones(_)) {
                    layout_at = None;
                }
                let actions = glue.input.portal(event, &mut driver, Instant::now());
                glue.apply(&mut driver, actions);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Some(why) = glue.input.closed() {
            tracing::warn!(
                "Shared input: input capture ended ({why}); the keyboard and mouse stay here"
            );
            break;
        }
        let now = Instant::now();
        let answered: Vec<String> = glue
            .channels
            .iter_mut()
            .filter_map(|(id, channel)| {
                let mut any = false;
                while channel.try_pong().is_some() {
                    any = true;
                }
                any.then(|| id.clone())
            })
            .collect();
        for id in answered {
            driver.answered(&id, now);
        }
        if let Some(phone) = driver.away_on().and_then(|to| glue.phones.get_mut(to)) {
            phone.repeat(now);
        }
        for id in &glue.phone_ids {
            if let Some((_, count)) = phone::pongs(id)
                && glue.pongs_seen.insert(id.clone(), count) != Some(count)
            {
                driver.answered(id, now);
            }
        }
        let actions = driver.tick(now);
        glue.apply(&mut driver, actions);
        nudge.follow(driver.away_on().filter(|_| quick_on));
        glue.follow_sound(&mut driver);
        // Looked at only at home: away, the desktop gives every key and
        // click to Pocketdesk, so nothing here comes to the front, and the
        // loop is not held up while it carries input.
        if driver.away_on().is_none() && game_at.elapsed() >= GAME_EVERY {
            game_at = Instant::now();
            let actions = driver.on(foreground_now(&runtime, game_lock_on));
            glue.apply(&mut driver, actions);
            glue.game_held = driver.game_held();
        }
        // Who is at this computer, as libei last said (only while away is
        // anything said), to every computer with a channel.
        if active_at.elapsed() >= ACTIVE_EVERY {
            active_at = Instant::now();
            let idle = u32::try_from(last_input.elapsed().as_millis()).unwrap_or(u32::MAX);
            let to: Vec<String> = glue.channels.keys().cloned().collect();
            let actions = to
                .into_iter()
                .map(|to| Action::Send(to, Msg::Active { idle_ms: idle }))
                .collect();
            glue.apply(&mut driver, actions);
        }
        status::set_keyboard_on(driver.away_on());
        status::set_input_capture(Some(shown(glue.input.standing(), fractional_screen)));
        // Away, a link that turned relayed or closed brings it home.
        if let Some(to) = driver.away_on().map(str::to_string)
            && link_at.elapsed() >= LINK_EVERY
        {
            link_at = Instant::now();
            let why = match link::connection(&to) {
                _ if glue.phone_ids.contains(&to) => {
                    (!phone::is_linked(&to)).then_some("The link to the phone closed")
                }
                None => Some("The link to that computer closed"),
                Some(conn) if !link::is_direct(&conn) => Some(
                    "The link to that computer went through a relay: the keyboard and mouse are \
                     shared only on a direct link",
                ),
                Some(_) => None,
            };
            if let Some(why) = why {
                let actions = driver.link_gone(&to, why);
                glue.apply(&mut driver, actions);
                glue.channels.remove(&to);
            }
            glue.take_refusals(&mut driver, Some(&to));
        }
        if layout_at.is_none_or(|at| at.elapsed() >= LAYOUT_EVERY) {
            layout_at = Some(Instant::now());
            let config = crate::events::current_config();
            game_lock_on = config.shared_input_game_lock;
            quick_on = config.shared_input_low_latency;
            driver.set_clipboard(config.shared_input_clipboard);
            glue.sound_on = config.shared_input_sound;
            let actions = driver.set_game_lock(game_lock_on, config.shared_input_lock_programs);
            glue.apply(&mut driver, actions);
            fractional_screen = fractional_now(&runtime);
            // The portal's zones are this computer's screens once it said
            // them; until then, what the desktop says.
            let own = match glue.input.zones() {
                Some(zones) => zones.to_vec(),
                None => runtime.block_on(screens::current()),
            };
            if own != glue.own {
                glue.own = own;
                let screens = Msg::Screens(glue.own.clone());
                let to: Vec<String> = glue.channels.keys().cloned().collect();
                let actions = to
                    .into_iter()
                    .map(|to| Action::Send(to, screens.clone()))
                    .collect();
                glue.apply(&mut driver, actions);
            }
            if greeted_at.is_none_or(|at| at.elapsed() >= REOPEN_AFTER) {
                greeted_at = Some(Instant::now());
                let linked = link::linked();
                glue.refusals.keep_linked(&linked);
                for id in linked {
                    let refused = glue
                        .refused
                        .get(&id)
                        .is_some_and(|(at, wait)| at.elapsed() < *wait);
                    if !refused && driver.away_on() != Some(id.as_str()) {
                        let _ = glue.channel(&id);
                    }
                }
                if let Some(to) = driver.away_on().map(str::to_string)
                    && !glue.phone_ids.contains(&to)
                    && link::connection(&to).is_none()
                {
                    let actions = driver.link_gone(&to, "The link to that computer closed");
                    glue.apply(&mut driver, actions);
                }
                glue.channels.retain(|id, _| link::connection(id).is_some());
                glue.phones.retain(|_, channel| !channel.is_closed());
            }
            let layout = read_layout(&mut glue);
            let actions = driver.set_layout(layout.clone());
            glue.apply(&mut driver, actions);
            glue.take_refusals(&mut driver, None);
            let open: Vec<OpenChannel> = glue
                .channels
                .keys()
                .filter_map(|id| {
                    Some(OpenChannel {
                        device_id: id.clone(),
                        opened: *glue.opened.get(id)?,
                        direct: link::connection(id).is_some_and(|conn| link::is_direct(&conn)),
                    })
                })
                .collect();
            let mut reachable = reachable(&open, &link::screens_heard());
            reachable.extend(
                glue.phone_ids
                    .iter()
                    .filter(|id| phone::standing_now(id) == PHONE_READY)
                    .cloned(),
            );
            for id in &reachable {
                glue.refusals.clear(id);
            }
            let reachable: Vec<String> = reachable.into_iter().collect();
            let actions = driver.set_reachable(reachable.clone());
            glue.apply(&mut driver, actions);
            // The barriers follow the computers there are to go to.
            glue.input.set_targets(&layout, &reachable);
            glue.input.rearm(Instant::now());
        }
    }
    let actions = driver.shutdown();
    glue.apply(&mut driver, actions);
    glue.follow_sound(&mut driver);
    status::set_keyboard_on(None);
    let deadline = Instant::now() + FLUSH_WAIT;
    for (_, channel) in glue.channels.drain() {
        channel.finish_within(deadline.saturating_duration_since(Instant::now()));
    }
    tracing::info!("Shared input: this computer's keyboard and mouse stay here");
    // How it ended stays said (a refusal, no portal) until sharing is off.
    let standing = glue.input.standing();
    status::set_input_capture(Some(standing));
    // Dropping the glue drops the backend: the portal's session ends.
    standing
}

/// The driver for the server's life: running while shared input and
/// driving are both on (read again every second), its portal session
/// started by `start`. Turning either off, or the server stopping
/// ([`stop`]), brings the keyboard and mouse home and ends it. One that
/// ended by itself starts again after a while, except after the owner
/// said no to the desktop's dialog: then only once sharing is turned off
/// and on again (or the server restarts), so the dialog does not nag.
pub async fn supervise<B: Backend + 'static>(start: Start<B>) {
    let runtime = tokio::runtime::Handle::current();
    let mut running: Option<(Arc<AtomicBool>, JoinHandle<Standing>)> = None;
    let mut retry_at: Option<Instant> = None;
    let mut refused = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let want = tokio::task::spawn_blocking(|| {
            let config = crate::events::current_config();
            config.shared_input && config.shared_input_drive
        })
        .await
        .unwrap_or(false);
        if !want {
            refused = false;
            if running.is_none() {
                status::set_input_capture(None);
            }
        }
        if running.as_ref().is_some_and(|(_, t)| t.is_finished()) {
            let mut wait = RESTART_AFTER;
            if let Some((_, thread)) = running.take() {
                let standing = thread.join().unwrap_or_default();
                refused = standing == Standing::NeedsPermission;
                if standing == Standing::Unsupported {
                    wait = LOOK_AGAIN_AFTER;
                }
            }
            retry_at = Some(Instant::now() + wait);
            continue;
        }
        match (
            &running,
            want && !refused && !STOPPING.load(Ordering::SeqCst),
        ) {
            (None, true) if retry_at.is_none_or(|at| Instant::now() >= at) => {
                retry_at = None;
                let stop = Arc::new(AtomicBool::new(false));
                let (flag, handle) = (Arc::clone(&stop), runtime.clone());
                match std::thread::Builder::new()
                    .name("pd-shared-drive".into())
                    .spawn(move || drive(flag, handle, start))
                {
                    Ok(thread) => running = Some((stop, thread)),
                    Err(e) => tracing::warn!("Shared input: the driver did not start: {e}"),
                }
            }
            (Some(_), false) => {
                if let Some((stop, thread)) = running.take() {
                    stop.store(true, Ordering::SeqCst);
                    let _ = tokio::task::spawn_blocking(move || thread.join()).await;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_input::game_lock;

    fn window(fullscreen: u8, class: &str) -> HyprWindow {
        serde_json::from_value(serde_json::json!({
            "address": "0x1", "class": class, "fullscreen": fullscreen,
        }))
        .unwrap()
    }

    #[test]
    fn a_fractionally_scaled_hyprland_screen_is_warned_of() {
        let monitor = |scale: f64| -> crate::hyprland::HyprMonitor {
            serde_json::from_value(serde_json::json!({ "name": "eDP-1", "scale": scale })).unwrap()
        };
        assert!(!fractional(&[]));
        assert!(!fractional(&[monitor(1.0), monitor(2.0)]));
        assert!(fractional(&[monitor(1.0), monitor(1.25)]));
        assert!(fractional(&[monitor(1.5)]));
        assert_eq!(shown(Standing::Ready, true), Standing::FractionalScale);
        assert_eq!(shown(Standing::Ready, false), Standing::Ready);
        // What it cannot do yet comes first.
        assert_eq!(
            shown(Standing::NeedsPermission, true),
            Standing::NeedsPermission
        );
        assert_eq!(shown(Standing::Unsupported, true), Standing::Unsupported);
    }

    #[test]
    fn hyprlands_window_in_front_is_weighed_for_the_gaming_lock() {
        assert_eq!(foreground_of(None), Foreground::default());
        let fg = foreground_of(Some(&window(0, "steam_app_1245620")));
        assert!(!fg.fullscreen);
        assert_eq!(fg.program, "steam_app_1245620");
        // Maximized is not full screen; full screen is, with or without it.
        assert!(!foreground_of(Some(&window(1, "kitty"))).fullscreen);
        assert!(foreground_of(Some(&window(2, "game"))).fullscreen);
        assert!(foreground_of(Some(&window(3, "game"))).fullscreen);
        // A listed program locks by its class.
        let fg = foreground_of(Some(&window(0, "eldenring.exe")));
        assert!(game_lock::in_front(&fg, &["eldenring".to_string()], true));
    }
}
