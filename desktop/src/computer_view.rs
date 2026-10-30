//! Another of the owner's computers' screens in a window (BK7).
//!
//! "Show its screen" in Settings under Other computers asks the server to dial that
//! computer's screen (`computers_out/`, `ComputerAsk::View`); the server
//! relays the picture over loopback and names it in `view.json`
//! (`core::local::view`). A worker thread here connects with the file's
//! token and decodes each access unit through the `ffmpeg` sidecar the
//! phone's cast window uses ([`crate::video`]), into a texture in a window
//! of its own, letterboxed. When the other computer takes this one's input
//! (its Devices page's Control switch for this computer), the window's
//! pointer and keys go to it ([`crate::view_input`]); without it, or with
//! View only on, the window only shows. Closing the window ends the view; a
//! view that ends on its own says why, with Reconnect. The clipboard
//! follows the window's focus while it controls, and files dropped on it go
//! to that computer ([`crate::view_follow`], PC3). Its bar's Send keys
//! menu sends the keys this computer keeps, and on Windows, in full screen,
//! a keyboard grab sends the Windows key and Alt+Tab there
//! ([`crate::view_grab`], PC4). "Hide what I do there" asks that computer
//! for its own privacy mode while the window controls it, and "Lock it when
//! I close the window" locks it as the window closes (PC8).

use std::collections::VecDeque;
use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::config::{COMPUTER_VIEW_QUALITIES, Config};
use pocketdesk_core::local::view::{
    self as wire, Message, Phase, ViewFile, ViewPrivacy, ViewStatus,
};
use pocketdesk_core::streaming::InputPacket;

use crate::video::{
    Accel, Admit, Codec, DecodeWanted, Decoder, KeyGate, Picture, Planes, Recorder, free_name,
    letterbox, now_unix, stamp, user_folder,
};
use crate::view_follow::{self, DropBatch, Edge, FocusEdges};
use crate::view_input::{MOVE_EVERY, SendKey, WindowInput, send_key_choices};

/// The window's id among the app's viewports.
fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pocketdesk-computer-view")
}

/// How often the worker looks for the view.
const POLL: Duration = Duration::from_millis(300);
/// The relay sends a heartbeat every 2 s; silence past this is a dead relay.
const READ_TIMEOUT: Duration = Duration::from_secs(6);
/// A view asked for that the server never names is given up on.
const NO_ANSWER: Duration = Duration::from_secs(20);
/// A window joins its view again at most this many times in
/// [`REJOIN_WINDOW`].
const REJOINS: usize = 3;
const REJOIN_WINDOW: Duration = Duration::from_secs(10);
/// How long a screenshot's or recording's line stays.
const SAVED_NOTE: Duration = Duration::from_secs(8);
/// How long a drop's last word stays (PC3).
const DROP_NOTE: Duration = Duration::from_secs(8);
/// How often a drop on its way looks at the sent list.
const DROP_LOOK: Duration = Duration::from_millis(500);
/// A way of decoding on the graphics card that gives no picture this long
/// after its first access unit is given up for the next (PC5).
const NO_PICTURE: Duration = Duration::from_secs(2);
/// How often the keyboard grab reads where the shared keyboard is (PC4).
#[cfg(windows)]
const ACROSS_LOOK: Duration = Duration::from_millis(500);

/// The computer whose screen is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    device_id: String,
    name: String,
    monitor: u32,
}

#[derive(Default)]
struct State {
    wanted: Option<Wanted>,
    /// When the view was last asked for.
    asked_at: Option<Instant>,
    /// Only a view newer than this one is this window's (one ended, or
    /// replaced by another screen's).
    newer_than: u64,
    /// Counts the asks (a view asked for, a reconnect, another screen) and
    /// closes: a view the worker shows is still this window's while it is
    /// unchanged.
    asked: u64,
    /// When this window last joined its view again (BK7 re-review N3).
    rejoins: Vec<Instant>,
    /// The view being shown.
    session_id: Option<u64>,
    status: ViewStatus,
    /// The picture's size as the stream says it now.
    size: Option<(u16, u16)>,
    picture: Option<Picture>,
    texture: Option<egui::TextureHandle>,
    /// An NV12 picture drawn by the GPU (PC5), and its size while it is
    /// the one drawn (else the texture is).
    gpu: Arc<crate::nv12::Gpu>,
    on_gpu: Option<[usize; 2]>,
    /// The picture shown, for a screenshot.
    shown: Option<Arc<Picture>>,
    /// What keeps the picture from showing (no decoder).
    note: Option<String>,
    /// Why the view ended, until Reconnect.
    ended: Option<String>,
    /// The relay connection, to cut when the window closes, and its
    /// writing half for input.
    stream: Option<TcpStream>,
    writer: Option<TcpStream>,
    input: WindowInput,
    view_only: bool,
    /// The quality preset chosen, and the view it was last sent for.
    quality: u8,
    quality_sent: Option<u64>,
    record_want: bool,
    recording: Option<(PathBuf, Instant)>,
    saved_note: Option<(String, Instant)>,
    fullscreen: bool,
    /// The stats over the picture (PC2), and a choice saved from the bar
    /// that the Settings page has not heard yet.
    stats_on: bool,
    stats_saved: Option<bool>,
    /// The decoder's time and the pictures shown.
    clock: DecodeClock,
    /// Decode on the graphics card when it can (`computer_view_hw_decode`,
    /// PC5), the way decoding now ("d3d11va", "software"), and the way a
    /// view settled on: its session and that way (PR #562 review M1: not
    /// its place, as the ways may differ by the next join).
    hw_decode: bool,
    decoding: String,
    /// Counts the decoders started (PR #562 review I2): a picture from an
    /// older one, still on its way as it is replaced, is dropped.
    decoder_gen: u64,
    way: Option<(u64, Accel)>,
    /// The window's focus, as said to the server for the clipboard (PC3).
    edges: FocusEdges,
    /// Files dropped on the window, on their way; the word on the last
    /// drop; when the sent list was last read for them.
    drop: Option<DropBatch>,
    drop_note: Option<(String, Instant)>,
    drop_look: Option<Instant>,
    /// Windows' keyboard grab (PC4).
    #[cfg(windows)]
    grab: GrabWindow,
    /// "Lock it when I close the window" (PC8), and a choice saved from
    /// the bar that Settings has not heard yet.
    lock_on_close: bool,
    lock_saved: Option<bool>,
    /// When "Hide what I do there" was last asked (PR #561 review): an
    /// ask still unanswered after [`ASKING_WAIT`] is given up here too.
    privacy_asked: Option<Instant>,
}

/// How long the bar shows "Asking…" before it gives up: the relay's own
/// give-up (8 s, said at its next 2 s heartbeat) and a little more.
pub const ASKING_WAIT: Duration = Duration::from_secs(11);

/// The window's keyboard grab on Windows (PC4): the setting, a choice saved
/// from the bar that Settings has not heard yet, the release, the hook
/// while it is in, and where the shared keyboard is, read now and then.
#[cfg(windows)]
#[derive(Default)]
struct GrabWindow {
    setting: bool,
    saved: Option<bool>,
    grab: crate::view_grab::Grab,
    hook: Option<crate::view_grab::Hook>,
    across: bool,
    /// The server's privacy mode is on here (review M11).
    privacy: bool,
    across_read: Option<Instant>,
}

/// What the window measures itself (PC2): pictures shown a second, how
/// long the decoder takes over one (the median and the 95th percentile of
/// the last second's, PC5), and the bytes each picture brings out of the
/// decoder and up to the screen.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub fps: u32,
    pub decode: Option<Duration>,
    pub decode_p95: Option<Duration>,
    pub bytes: u64,
}

/// The `p`th percentile (0 to 1) of `samples`, by nearest rank: `None` for
/// none. The samples are sorted in place.
pub fn percentile(samples: &mut [Duration], p: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let rank = (p.clamp(0.0, 1.0) * samples.len() as f64).ceil() as usize;
    Some(samples[rank.clamp(1, samples.len()) - 1])
}

/// Times the decoder and counts the pictures shown (PC2). The sidecar gives
/// one picture for each access unit, in the order they went in, so each
/// picture is timed against the oldest unit still waiting.
#[derive(Debug, Default)]
pub struct DecodeClock {
    fed: VecDeque<Instant>,
    since: Option<Instant>,
    pictures: u32,
    took: Vec<Duration>,
    bytes: u64,
    shown: Shown,
}

impl DecodeClock {
    /// Past this many units waiting the count has lost its way (the
    /// decoder dropped some): it starts again.
    const MOST_WAITING: usize = 32;

    /// An access unit went to the decoder at `at`.
    pub fn fed(&mut self, at: Instant) {
        if self.fed.len() >= Self::MOST_WAITING {
            self.fed.clear();
        }
        self.fed.push_back(at);
    }

    /// A picture of `bytes` came out at `at`.
    pub fn decoded(&mut self, at: Instant, bytes: usize) {
        if let Some(fed) = self.fed.pop_front() {
            self.took.push(at.saturating_duration_since(fed));
        }
        self.pictures = self.pictures.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes as u64);
        self.roll(at);
    }

    /// The last whole second's pictures and decode time, as of `now`.
    pub fn shown(&mut self, now: Instant) -> Shown {
        self.roll(now);
        self.shown
    }

    /// A new decoder: nothing the old one was fed is waiting.
    pub fn restart(&mut self) {
        self.fed.clear();
    }

    fn roll(&mut self, now: Instant) {
        let since = *self.since.get_or_insert(now);
        let elapsed = now.saturating_duration_since(since);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let ms = elapsed.as_millis().max(1);
        self.shown = Shown {
            fps: u32::try_from(u128::from(self.pictures) * 1000 / ms).unwrap_or(u32::MAX),
            decode: percentile(&mut self.took, 0.5),
            decode_p95: percentile(&mut self.took, 0.95),
            bytes: self.bytes / u64::from(self.pictures.max(1)),
        };
        self.pictures = 0;
        self.took.clear();
        self.bytes = 0;
        self.since = Some(now);
    }
}

/// Bytes in words: B, KB or MB (decimal, as a file's size is said).
pub fn bytes_words(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}

/// The road the picture comes by, as the overlay says it.
pub fn road_words(status: &ViewStatus) -> String {
    let relayed = status.stats.as_ref().is_some_and(|s| s.relayed);
    match status.route.as_str() {
        "" => "not yet".to_string(),
        "internet" if relayed => "internet, by a relay".to_string(),
        "internet" => "internet, direct".to_string(),
        other => other.to_string(),
    }
}

/// A bitrate in words: kb/s, or Mb/s from 1000 kb/s.
pub fn bitrate_words(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mb/s", f64::from(kbps) / 1000.0)
    } else {
        format!("{kbps} kb/s")
    }
}

/// The overlay's lines (PC2): the road and how long it took to reach, the
/// picture's size and codec, the frames that came and were shown, the
/// bitrate, the round trip, the decoder's time, and the frames lost, late
/// and dropped.
pub fn stats_lines(
    status: &ViewStatus,
    size: Option<(u16, u16)>,
    shown: Shown,
    decoding: &str,
) -> Vec<String> {
    let stats = status.stats.clone().unwrap_or_default();
    let mut road = format!("Road: {}", road_words(status));
    if let Some(ms) = stats.dial_ms {
        road.push_str(&format!(" (reached in {ms} ms)"));
    }
    let codec = if stats.codec.is_empty() {
        "H.264"
    } else {
        stats.codec.as_str()
    };
    let picture = match size {
        Some((w, h)) => format!("Picture: {w}\u{d7}{h} {codec}"),
        None => "Picture: not yet".to_string(),
    };
    vec![
        road,
        picture,
        format!("Frames: {} a second in, {} shown", stats.fps, shown.fps),
        format!("Bitrate: {}", bitrate_words(stats.kbps)),
        format!(
            "Round trip: {}",
            stats
                .rtt_ms
                .map_or_else(|| "not known".to_string(), |ms| format!("{ms} ms"))
        ),
        decode_words(shown, decoding),
        format!(
            "Pipe: {}",
            if shown.bytes == 0 {
                "not yet".to_string()
            } else {
                format!("{} a picture", bytes_words(shown.bytes))
            }
        ),
        format!(
            "Lost {} \u{b7} late {} \u{b7} dropped {}",
            stats.lost, stats.late, stats.dropped
        ),
    ]
}

/// The decoder's line: its median time over a picture in the last second,
/// its 95th percentile, and the way it decodes (PC5).
fn decode_words(shown: Shown, decoding: &str) -> String {
    let ms = |d: Duration| format!("{:.1} ms", d.as_secs_f64() * 1000.0);
    let time = match (shown.decode, shown.decode_p95) {
        (Some(p50), Some(p95)) => format!("{} (p95 {})", ms(p50), ms(p95)),
        (Some(p50), None) => ms(p50),
        _ => "not yet".to_string(),
    };
    if decoding.is_empty() {
        format!("Decode: {time}")
    } else {
        format!("Decode: {time} \u{b7} {decoding}")
    }
}

/// Whether a decoder that gave `pictures`, has `ended` or not, and was
/// first fed at `first_fed`, is given up for the next way at `now` (PC5):
/// one that never gave a picture, and ended or has had [`NO_PICTURE`].
pub fn give_up(pictures: u64, ended: bool, first_fed: Option<Instant>, now: Instant) -> bool {
    pictures == 0
        && (ended || first_fed.is_some_and(|at| now.saturating_duration_since(at) >= NO_PICTURE))
}

/// Whether `event` is Ctrl+Alt+Shift+S pressed: the stats' switch, kept
/// here (PR #551 review M8).
pub fn stats_chord(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { key: egui::Key::S, pressed: true, modifiers, .. }
            if modifiers.ctrl && modifiers.alt && modifiers.shift
    )
}

/// `events` without the stats' switch, pressed or let go: it is this
/// window's, not the other computer's.
pub fn keep_chord_here(events: &mut Vec<egui::Event>) {
    events.retain(|event| {
        !matches!(
            event,
            egui::Event::Key { key: egui::Key::S, modifiers, .. }
                if modifiers.ctrl && modifiers.alt && modifiers.shift
        )
    });
}

/// The stats switched on or off from this window: shown at once, and
/// remembered for the next (computer_view_stats).
fn switch_stats(shared: &Mutex<State>, config: Option<&Path>, on: bool) {
    let mut state = lock(shared);
    state.stats_on = on;
    match config.map(|path| save_stats(path, on)) {
        Some(Ok(())) => state.stats_saved = Some(on),
        Some(Err(e)) => {
            state.saved_note = Some((format!("Not saved: {e}"), Instant::now()));
        }
        None => {}
    }
}

/// Save the window's Stats switch as `computer_view_stats` in the config
/// at `path`, that key alone, as Settings writes it.
fn save_stats(path: &Path, on: bool) -> Result<(), String> {
    save_switch(path, "computer_view_stats", |c| c.computer_view_stats = on)
}

/// Save the bar's switch `key`, set by `set`, in the config at `path`, that
/// key alone, as Settings writes it.
fn save_switch(path: &Path, key: &str, set: impl FnOnce(&mut Config)) -> Result<(), String> {
    let mut config = Config::load_from(path).map_err(|e| e.to_string())?;
    set(&mut config);
    config.save_keys_to(path, &[key]).map_err(|e| e.to_string())
}

/// "Send Win and Alt+Tab there" switched from the bar (PC4): at once, and
/// remembered as `computer_view_grab_keys`.
#[cfg(windows)]
fn switch_grab(shared: &Mutex<State>, config: Option<&Path>, on: bool) {
    let mut state = lock(shared);
    state.grab.setting = on;
    let saved = config.map(|path| {
        save_switch(path, "computer_view_grab_keys", |c| {
            c.computer_view_grab_keys = on
        })
    });
    match saved {
        Some(Ok(())) => state.grab.saved = Some(on),
        Some(Err(e)) => {
            state.saved_note = Some((format!("Not saved: {e}"), Instant::now()));
        }
        None => {}
    }
}

/// "Lock it when I close the window" switched from the bar (PC8): at
/// once, and remembered as `computer_view_lock_on_close`.
fn switch_lock(shared: &Mutex<State>, config: Option<&Path>, on: bool) {
    let mut state = lock(shared);
    state.lock_on_close = on;
    let saved = config.map(|path| {
        save_switch(path, "computer_view_lock_on_close", |c| {
            c.computer_view_lock_on_close = on
        })
    });
    match saved {
        Some(Ok(())) => state.lock_saved = Some(on),
        Some(Err(e)) => {
            state.saved_note = Some((format!("Not saved: {e}"), Instant::now()));
        }
        None => {}
    }
}

/// What the window says while there is no picture.
pub fn waiting_words(name: &str, status: &ViewStatus) -> String {
    match status.phase {
        Phase::Connecting => format!("Reaching {name}\u{2026}"),
        Phase::Waiting => format!(
            "Waiting for {name} to let this computer in\u{2026} If it asks first, allow the session there."
        ),
        Phase::Showing => format!("Waiting for {name}'s screen\u{2026}"),
    }
}

/// What the window says while there is no picture: what keeps it from
/// showing when the server says, else how the view stands.
pub fn shown_words(name: &str, status: &ViewStatus) -> String {
    status
        .note
        .clone()
        .unwrap_or_else(|| waiting_words(name, status))
}

/// What the window says of control: `None` while it controls.
pub fn control_words(name: &str, control: Option<bool>, view_only: bool) -> Option<String> {
    match (control, view_only) {
        (Some(false), _) => Some(format!(
            "View only: {name} does not take this computer's clicks and keys. To control it from here, turn on \u{201c}Control the mouse and keyboard\u{201d} for this computer on {name}'s Devices page."
        )),
        (_, true) => Some("View only: this window sends nothing.".to_string()),
        (None, false) => None,
        (Some(true), false) => None,
    }
}

/// Whether the bar's "Asking…", asked at `asked`, has waited too long at
/// `now` (PR #561 review): the relay or that computer never answered.
pub fn asking_lapsed(privacy: &ViewPrivacy, asked: Option<Instant>, now: Instant) -> bool {
    *privacy == ViewPrivacy::Asking
        && asked.is_some_and(|at| now.saturating_duration_since(at) >= ASKING_WAIT)
}

/// Whether "Hide what I do there" shows as on: asked or held there (PC8).
pub fn hiding(privacy: &ViewPrivacy) -> bool {
    matches!(privacy, ViewPrivacy::Asking | ViewPrivacy::On)
}

/// What the window says of privacy on the computer `name` (PC8): `None`
/// while it is off with nothing to say.
pub fn privacy_words(name: &str, privacy: &ViewPrivacy) -> Option<String> {
    match privacy {
        ViewPrivacy::Off => None,
        ViewPrivacy::Asking => Some(format!("Asking {name} to hide what you do there\u{2026}")),
        ViewPrivacy::On => Some(format!(
            "Hidden: {name}'s own keyboard and mouse are blocked and a ring shows on its screen, until this window closes or you turn it off. It does not blank the screen there."
        )),
        ViewPrivacy::Refused(why) => Some(format!("Not hidden on {name}: {why}")),
    }
}

/// The window's last word to the server as it closes (PC8): a lock of the
/// other computer when chosen and the view was let in, else nothing (the
/// server lets privacy go as the view ends).
pub fn closing_ask(lock_on_close: bool, phase: Phase) -> Option<u8> {
    (lock_on_close && phase == Phase::Showing).then_some(wire::PRIVACY_LOCK)
}

/// Whether the window's input goes to the other computer now.
pub fn controls(control: Option<bool>, view_only: bool, connected: bool, current: bool) -> bool {
    control == Some(true) && !view_only && connected && current
}

/// The other computer's screens to choose from: its names, or "Screen n"
/// for as many as the chosen one needs when they are not known.
pub fn screen_choices(status: &ViewStatus) -> Vec<String> {
    if status.monitors.is_empty() {
        return (0..=status.monitor)
            .map(|n| format!("Screen {}", n + 1))
            .collect();
    }
    status.monitors.clone()
}

/// The window and its worker.
pub struct ComputerView {
    shared: Arc<Mutex<State>>,
    state_dir: PathBuf,
    /// The config file the bar's Stats switch is saved to (none in tests).
    config: Option<PathBuf>,
    /// For a repaint when Settings changes the window.
    ctx: egui::Context,
}

fn lock(shared: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ComputerView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf, config: Option<PathBuf>) -> Self {
        let shared = Arc::new(Mutex::new(State::default()));
        let worker = shared.clone();
        let repaint = ctx.clone();
        let ctx = ctx.clone();
        let watched = state_dir.clone();
        let spawned = std::thread::Builder::new()
            .name("computer-view".into())
            .spawn(move || watch(&watched, &worker, &ctx));
        // Without the thread no screen is shown, and nothing else breaks.
        drop(spawned);
        Self {
            shared,
            state_dir,
            config,
            ctx: repaint,
        }
    }

    /// Show the computer `device_id`'s screen `monitor`: asked of the
    /// server, the window opens at once and fills when the picture comes.
    /// `quality` is the preset it starts at, `view_only` whether it sends
    /// nothing and `stats` whether it shows how the picture comes; the
    /// window's own choices then hold.
    pub fn open(
        &self,
        device_id: &str,
        name: &str,
        monitor: u32,
        quality: u8,
        view_only: bool,
        stats: bool,
    ) -> Result<(), String> {
        let wanted = Wanted {
            device_id: device_id.to_string(),
            name: name.to_string(),
            monitor,
        };
        let mut state = lock(&self.shared);
        let same = state
            .wanted
            .as_ref()
            .is_some_and(|w| w.device_id == device_id);
        if !same {
            state.quality = quality;
            state.view_only = view_only;
            state.stats_on = stats;
        }
        ask(&self.state_dir, &mut state, wanted)
    }

    /// Stats switched in Settings (PR #551 review): an open window follows.
    pub fn set_stats(&self, on: bool) {
        lock(&self.shared).stats_on = on;
        self.ctx.request_repaint_of(viewport_id());
    }

    /// "Decode on the graphics card" as Settings has it (PC5): the next
    /// view follows.
    pub fn set_hw_decode(&self, on: bool) {
        lock(&self.shared).hw_decode = on;
    }

    /// The bar's Stats switch, saved since the last call (PC2): Settings
    /// follows it.
    pub fn take_stats_saved(&self) -> Option<bool> {
        lock(&self.shared).stats_saved.take()
    }

    /// "Send Win and Alt+Tab there" as Settings has it (PC4): an open
    /// window follows at once.
    #[cfg(windows)]
    pub fn set_grab_keys(&self, on: bool) {
        lock(&self.shared).grab.setting = on;
        self.ctx.request_repaint_of(viewport_id());
    }

    /// The bar's "Send Win and Alt+Tab there", saved since the last call
    /// (PC4): Settings follows it.
    #[cfg(windows)]
    pub fn take_grab_saved(&self) -> Option<bool> {
        lock(&self.shared).grab.saved.take()
    }

    /// "Lock it when I close the window" as Settings has it (PC8): an open
    /// window follows at once.
    pub fn set_lock_on_close(&self, on: bool) {
        lock(&self.shared).lock_on_close = on;
        self.ctx.request_repaint_of(viewport_id());
    }

    /// The bar's "Lock it when I close the window", saved since the last
    /// call (PC8): Settings follows it.
    pub fn take_lock_saved(&self) -> Option<bool> {
        lock(&self.shared).lock_saved.take()
    }

    /// Call each pass: keeps the window open while a screen is wanted.
    pub fn show(&self, ctx: &egui::Context) {
        let title = {
            let state = lock(&self.shared);
            let Some(wanted) = &state.wanted else { return };
            format!("{} \u{b7} screen", wanted.name)
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([320.0, 240.0]);
        let shared = self.shared.clone();
        let state_dir = self.state_dir.clone();
        let config = self.config.clone();
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            draw(&shared, &state_dir, config.as_deref(), ui);
        });
    }
}

/// Ask the server for `wanted`'s screen; a view shown now is let go first.
fn ask(state_dir: &Path, state: &mut State, wanted: Wanted) -> Result<(), String> {
    let request = ComputerAsk::View {
        device_id: wanted.device_id.clone(),
        monitor: wanted.monitor,
    };
    let release = state.input.release_all();
    send(state, release);
    computers::request(state_dir, request, now_unix()).map_err(|e| format!("Not asked: {e}"))?;
    if let Some(stream) = state.stream.take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    state.writer = None;
    state.newer_than = state.newer_than.max(state.session_id.unwrap_or(0));
    state.session_id = None;
    state.status = ViewStatus {
        monitor: wanted.monitor,
        ..ViewStatus::default()
    };
    state.ended = None;
    state.note = None;
    state.asked_at = Some(Instant::now());
    state.wanted = Some(wanted);
    state.asked += 1;
    state.rejoins.clear();
    Ok(())
}

/// Let go of what is held there, then end the view on the server: the
/// view this window shows, when it reached one (BK7 review M1). Its end is
/// no longer this window's to say.
fn stop_view(state_dir: &Path, state: &mut State) {
    // The clipboard comes back as the window goes (PC3).
    if let Some(edge) = state.edges.leave() {
        say_focus(state, edge);
    }
    let release = state.input.release_all();
    send(state, release);
    state.writer = None;
    if let Some(stream) = state.stream.take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    let shown = state.session_id;
    state.newer_than = state.newer_than.max(state.session_id.unwrap_or(0));
    state.session_id = None;
    state.asked += 1;
    let _ = computers::request(
        state_dir,
        ComputerAsk::StopView { session_id: shown },
        now_unix(),
    );
}

/// Cancel, while the server dials the other computer again after a drop
/// (PC2): the view stops there, and the window says so, with Reconnect.
fn cancel_reconnect(state_dir: &Path, state: &mut State) {
    stop_view(state_dir, state);
    let name = state
        .wanted
        .as_ref()
        .map_or_else(|| "the computer".to_string(), |w| w.name.clone());
    state.ended = Some(format!("Stopped reconnecting to {name}."));
}

/// What the window says while the server dials the other computer again
/// after a drop (PC2), with the try it is on; `None` when it is not.
pub fn reconnect_words(name: &str, status: &ViewStatus) -> Option<String> {
    let n = status.reconnecting;
    (n > 0).then(|| match status.phase {
        Phase::Waiting => {
            format!("Reconnecting to {name}\u{2026} ({n}) Waiting for it to let this computer in.")
        }
        _ => format!("Reconnecting to {name}\u{2026} ({n})"),
    })
}

/// Send `packets` to the other computer through the relay; a broken write
/// ends input here (the reader sees the relay go).
fn send(state: &mut State, packets: Vec<InputPacket>) {
    if packets.is_empty() {
        return;
    }
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    for packet in packets {
        let bytes = packet.encode();
        if bytes.len() > wire::MAX_INPUT {
            continue;
        }
        if Message::Input(bytes).write(writer).is_err() {
            state.writer = None;
            return;
        }
    }
}

/// Say the window's privacy ask to the server (PC8); a broken write ends
/// input here, as [`send`]'s.
fn say_privacy(state: &mut State, ask: u8) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if Message::Privacy(ask).write(writer).is_err() {
        state.writer = None;
        return;
    }
    // Said at once here; the server's status says how it went.
    match ask {
        wire::PRIVACY_HIDE => {
            state.status.privacy = ViewPrivacy::Asking;
            state.privacy_asked = Some(Instant::now());
        }
        _ => state.status.privacy = ViewPrivacy::Off,
    }
}

/// Say the window's focus to the server (PC3); a broken write ends input
/// here, as [`send`]'s.
fn say_focus(state: &mut State, edge: Edge) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if Message::Focus(edge == Edge::Gain).write(writer).is_err() {
        state.writer = None;
    }
}

/// Files or folders dropped on the window go to its computer (PC3), as the
/// Files page sends them: refused at once when they cannot go now, else
/// queued, and followed on the sent list.
fn take_drops(
    shared: &Mutex<State>,
    ctx: &egui::Context,
    state_dir: &Path,
    config: Option<&Path>,
    wanted: &Wanted,
) {
    let dropped = view_follow::dropped_paths(ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .collect::<Vec<_>>()
    }));
    if dropped.is_empty() {
        return;
    }
    let link = computers::read(state_dir)
        .computers
        .into_iter()
        .find(|c| c.device_id == wanted.device_id)
        .map(|c| c.link)
        .unwrap_or_default();
    let internet_files = config
        .and_then(|path| Config::load_from(path).ok())
        .is_none_or(|c| c.internet_files);
    let now = Instant::now();
    let mut state = lock(shared);
    match view_follow::refusal(&link, internet_files) {
        Some(refusal) => {
            state.drop = None;
            state.drop_note = Some((view_follow::refusal_words(&wanted.name, &refusal), now));
        }
        None => {
            let unix = now_unix();
            let batch = view_follow::queue_all(&dropped, &wanted.name, now, |path| {
                crate::model::queue_send_request(state_dir, &wanted.device_id, path, unix)
                    .map(|(id, _)| id)
            });
            state.drop = Some(batch);
            state.drop_note = None;
            state.drop_look = Some(now);
        }
    }
    ctx.request_repaint();
}

/// What the window says of files dropped on it now (PC3): on their way,
/// where they landed or why not, for a while; `None` otherwise.
fn drop_words(shared: &Mutex<State>, state_dir: &Path, ctx: &egui::Context) -> Option<String> {
    let now = Instant::now();
    let mut state = lock(shared);
    if let Some(batch) = state.drop.clone() {
        if state.drop_look.is_none_or(|at| at.elapsed() >= DROP_LOOK) {
            state.drop_look = Some(now);
            let sent = pocketdesk_core::local::sent::read(state_dir);
            if let Some(landed) = view_follow::landed(&batch, &sent, now) {
                state.drop = None;
                state.drop_note = Some((view_follow::landed_words(&batch, &landed), now));
            }
        }
        if state.drop.is_some() {
            ctx.request_repaint_after(DROP_LOOK);
            return Some(view_follow::sending_words(&batch));
        }
    }
    let note = state
        .drop_note
        .clone()
        .filter(|(_, at)| at.elapsed() < DROP_NOTE)
        .map(|(note, at)| {
            ctx.request_repaint_after(DROP_NOTE.saturating_sub(at.elapsed()));
            note
        });
    if note.is_none() {
        state.drop_note = None;
    }
    note
}

fn draw(shared: &Arc<Mutex<State>>, state_dir: &Path, config: Option<&Path>, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut state = lock(shared);
    let Some(wanted) = state.wanted.clone() else {
        return;
    };
    if ctx.input(|i| i.viewport().close_requested()) {
        // The lock, when chosen, goes before the view stops (PC8).
        if let Some(ask) = closing_ask(state.lock_on_close, state.status.phase) {
            say_privacy(&mut state, ask);
        }
        stop_view(state_dir, &mut state);
        state.wanted = None;
        state.fullscreen = false;
        // The keyboard grab comes out with the window (PC4).
        #[cfg(windows)]
        {
            state.grab.hook = None;
        }
        return;
    }
    if let Some(picture) = state.picture.take() {
        let picture = Arc::new(picture);
        if picture.planes == Planes::Nv12 && crate::nv12::planes() == Planes::Nv12 {
            // PC5: its planes go up as they are, and the GPU turns them to
            // RGB as it draws them.
            state.gpu.put(picture.clone());
            state.on_gpu = Some([picture.width, picture.height]);
        } else {
            to_texture(&mut state, &ctx, &picture);
        }
        state.shown = Some(picture);
    } else if state.on_gpu.is_some() && crate::nv12::planes() == Planes::Rgba {
        // The shader failed: the picture shown is drawn as RGBA.
        if let Some(picture) = state.shown.clone() {
            to_texture(&mut state, &ctx, &picture);
        }
    }
    // The window lost focus: whatever is held there is let go.
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    if !focused && state.input.holds() {
        let release = state.input.release_all();
        send(&mut state, release);
    }
    let texture = state.texture.clone();
    let gpu = state.gpu.clone();
    let on_gpu = state.on_gpu;
    // The size of the picture drawn, wherever it is drawn from.
    let drawn = on_gpu.or_else(|| texture.as_ref().map(egui::TextureHandle::size));
    let status = state.status.clone();
    let ended = state.ended.clone();
    let note = state.note.clone();
    let connected = state.writer.is_some();
    let mut view_only = state.view_only;
    let mut quality = state.quality;
    let record_want = state.record_want;
    let recording = state.recording.clone();
    let fullscreen = state.fullscreen;
    let mut stats_on = state.stats_on;
    // Ctrl+Alt+Shift+S, wherever the focus is (PR #551 review M8).
    if ctx.input(|i| i.events.iter().any(stats_chord)) {
        stats_on = !stats_on;
        drop(state);
        switch_stats(shared, config, stats_on);
        state = lock(shared);
    }
    #[cfg(windows)]
    let grab_setting = state.grab.setting;
    let lock_on_close = state.lock_on_close;
    let size = state.size;
    let shown = state.clock.shown(Instant::now());
    let decoding = state.decoding.clone();
    let saved = state
        .saved_note
        .clone()
        .filter(|(_, at)| at.elapsed() < SAVED_NOTE)
        .map(|(note, _)| note);
    drop(state);

    let mut monitor = wanted.monitor;
    let mut send_key: Option<SendKey> = None;
    let mut privacy_ask: Option<u8> = None;
    egui::Panel::top("computer-view-bar")
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(24))
                .inner_margin(4.0),
        )
        .show(ui, |ui| {
            // Dark whatever the theme, as the cast window's bar.
            ui.style_mut().visuals = egui::Visuals::dark();
            ui.horizontal_wrapped(|ui| {
                let mut said = wanted.name.clone();
                if !status.route.is_empty() {
                    said.push_str(&format!(" \u{b7} {}", status.route));
                }
                ui.label(egui::RichText::new(said).strong());
                let choices = screen_choices(&status);
                if choices.len() > 1 {
                    let shown = choices
                        .get(monitor as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("Screen {}", monitor + 1));
                    egui::ComboBox::from_id_salt("computer-view-screen")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (n, choice) in choices.iter().enumerate() {
                                ui.selectable_value(&mut monitor, n as u32, choice);
                            }
                        });
                }
                let label = COMPUTER_VIEW_QUALITIES
                    .iter()
                    .find(|(_, id)| *id == quality)
                    .map_or("Auto", |(name, _)| {
                        crate::settings_view::computer_quality_label(name)
                    });
                egui::ComboBox::from_id_salt("computer-view-quality")
                    .selected_text(format!("Quality: {label}"))
                    .show_ui(ui, |ui| {
                        for (name, id) in COMPUTER_VIEW_QUALITIES {
                            ui.selectable_value(
                                &mut quality,
                                id,
                                crate::settings_view::computer_quality_label(name),
                            );
                        }
                    });
                let switch = ui
                    .checkbox(&mut view_only, "View only")
                    .on_hover_text("Show the computer without sending it clicks or keys");
                if switch.clicked() {
                    switch.surrender_focus();
                }
                // PC4: the keys this computer keeps, or that cannot be
                // pressed through the window, while it controls.
                if ended.is_none() && controls(status.control, view_only, connected, true) {
                    ui.menu_button("Send keys", |ui| {
                        for key in send_key_choices(status.windows) {
                            if ui.button(key.label()).clicked() {
                                send_key = Some(key);
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Send a key or chord this computer would keep to itself");
                    #[cfg(windows)]
                    {
                        let mut grab_on = grab_setting;
                        let grab = ui
                            .checkbox(&mut grab_on, "Send Win and Alt+Tab there")
                            .on_hover_text(format!(
                                "In full screen, the Windows key, Alt+Tab, Alt+Esc and Ctrl+Esc go to {}. Press and let go of Ctrl+Alt+Shift to keep them here.",
                                wanted.name
                            ));
                        if grab.clicked() {
                            grab.surrender_focus();
                            switch_grab(shared, config, grab_on);
                        }
                    }
                    // PC8: the other computer's own privacy mode.
                    let mut hide = hiding(&status.privacy);
                    let hidden = ui.checkbox(&mut hide, "Hide what I do there").on_hover_text(
                        format!(
                            "Ask {} to block its own keyboard and mouse and show a ring on its screen while this window drives it. {} decides: its Devices page needs \u{201c}Lock, sleep, privacy and wake others\u{201d} on for this computer.",
                            wanted.name, wanted.name
                        ),
                    );
                    if hidden.clicked() {
                        hidden.surrender_focus();
                        privacy_ask = Some(if hide {
                            wire::PRIVACY_HIDE
                        } else {
                            wire::PRIVACY_SHOW
                        });
                    }
                }
                if ended.is_none() && status.phase == Phase::Showing {
                    let mut lock_on = lock_on_close;
                    let lock_switch = ui
                        .checkbox(&mut lock_on, "Lock it when I close the window")
                        .on_hover_text(format!(
                            "Lock {} as this window closes, with its own lock. Its Devices page needs \u{201c}Lock, sleep, privacy and wake others\u{201d} on for this computer.",
                            wanted.name
                        ));
                    if lock_switch.clicked() {
                        lock_switch.surrender_focus();
                        switch_lock(shared, config, lock_on);
                    }
                }
                let shot = ui
                    .add_enabled(drawn.is_some(), egui::Button::new("Screenshot"))
                    .on_hover_text("Save the computer's screen as a picture, and copy it");
                if shot.clicked() {
                    shot.surrender_focus();
                    screenshot(shared, &ctx, &wanted.name);
                }
                let record = ui
                    .add_enabled(
                        drawn.is_some() || record_want,
                        egui::Button::new(if record_want { "Stop" } else { "Record" }),
                    )
                    .on_hover_text("Record the computer's screen as a video, as it comes");
                if record.clicked() {
                    record.surrender_focus();
                    lock(shared).record_want = !record_want;
                }
                let full = ui.button(if fullscreen {
                    "Exit full screen"
                } else {
                    "Full screen"
                });
                if full.clicked() {
                    full.surrender_focus();
                    lock(shared).fullscreen = !fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                }
                let stats = ui.checkbox(&mut stats_on, "Stats").on_hover_text(
                    "Show how the picture comes: frames, bitrate, round trip, road and decode time (Ctrl+Alt+Shift+S)",
                );
                if stats.clicked() {
                    stats.surrender_focus();
                    switch_stats(shared, config, stats_on);
                }
                if let Some((_, since)) = &recording {
                    let secs = since.elapsed().as_secs();
                    ui.colored_label(
                        egui::Color32::LIGHT_GRAY,
                        format!("Recording {}:{:02}", secs / 60, secs % 60),
                    );
                    ctx.request_repaint_after(Duration::from_secs(1));
                } else if let Some(note) = &saved {
                    ui.colored_label(egui::Color32::LIGHT_GRAY, note);
                    ctx.request_repaint_after(SAVED_NOTE);
                }
            });
            // What keeps the picture from showing, over a picture that
            // stopped (BK7 review M6).
            if ended.is_none()
                && drawn.is_some()
                && let Some(note) = &status.note
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(note).color(egui::Color32::LIGHT_GRAY))
                        .wrap(),
                );
            }
            if ended.is_none()
                && status.phase == Phase::Showing
                && let Some(words) = control_words(&wanted.name, status.control, view_only)
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(words).color(egui::Color32::LIGHT_GRAY))
                        .wrap(),
                );
            }
            if ended.is_none()
                && let Some(words) = privacy_words(&wanted.name, &status.privacy)
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(words).color(egui::Color32::LIGHT_GRAY))
                        .wrap(),
                );
            }
        });

    // The toolbar's choices.
    let mut reconnect = false;
    let mut cancel = false;
    if monitor != wanted.monitor {
        let mut state = lock(shared);
        let next = Wanted {
            monitor,
            ..wanted.clone()
        };
        if let Err(e) = ask(state_dir, &mut state, next) {
            state.ended = Some(e);
        }
        return;
    }
    {
        let mut state = lock(shared);
        if view_only && !state.view_only {
            let release = state.input.release_all();
            send(&mut state, release);
            // Nothing to hide while this window drives nothing (PC8).
            if hiding(&state.status.privacy) {
                say_privacy(&mut state, wire::PRIVACY_SHOW);
            }
        }
        if let Some(ask) = privacy_ask {
            say_privacy(&mut state, ask);
        }
        // An ask nobody answered is given up here too (review): the bar
        // never stays at "Asking…".
        if asking_lapsed(&state.status.privacy, state.privacy_asked, Instant::now()) {
            say_privacy(&mut state, wire::PRIVACY_SHOW);
            state.status.privacy = ViewPrivacy::Refused(format!(
                "{} did not answer; privacy is off here.",
                wanted.name
            ));
        } else if state.status.privacy == ViewPrivacy::Asking {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        state.view_only = view_only;
        if let Some(key) = send_key
            && controls(status.control, view_only, connected, true)
        {
            let chord = state.input.chord(key.keys(), status.windows);
            send(&mut state, chord);
        }
        if quality != state.quality {
            state.quality = quality;
            send(
                &mut state,
                vec![InputPacket::QualityPreset { preset: quality }],
            );
        }
        // The clipboard follows the focus while the window controls (PC3).
        let inside = focused && ended.is_none() && status.control == Some(true) && !view_only;
        if let Some(edge) = state.edges.observe(inside, Instant::now()) {
            say_focus(&mut state, edge);
        }
        if state.edges.pending() {
            ctx.request_repaint_after(view_follow::SETTLE);
        }
    }
    take_drops(shared, &ctx, state_dir, config, &wanted);
    let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    let drop_said = if hovering {
        Some(format!("Drop to send to {}'s Downloads", wanted.name))
    } else {
        drop_words(shared, state_dir, &ctx)
    };
    if let Some(words) = drop_said {
        egui::Area::new(egui::Id::new("computer-view-drop"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -16.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(&ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_black_alpha(210))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_gray(235), words);
                    });
            });
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let mut picture_rect = None;
            if let Some(why) = &ended {
                ui.vertical_centered(|ui| {
                    ui.add_space(area.height() / 3.0);
                    ui.colored_label(egui::Color32::LIGHT_GRAY, why);
                    ui.add_space(8.0);
                    reconnect = ui.button("Reconnect").clicked();
                });
            } else if let Some(words) = reconnect_words(&wanted.name, &status) {
                // The last picture, dimmed, while it dials again (PC2).
                if let Some([w, h]) = drawn {
                    paint_picture(
                        ui,
                        &gpu,
                        on_gpu.is_some(),
                        texture.as_ref(),
                        letterbox(area, w, h),
                        egui::Color32::from_gray(90),
                    );
                }
                ui.vertical_centered(|ui| {
                    ui.add_space(area.height() / 3.0);
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(200))
                        .corner_radius(6.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.colored_label(egui::Color32::LIGHT_GRAY, words);
                            ui.add_space(8.0);
                            cancel = ui
                                .button("Cancel")
                                .on_hover_text("Stop trying; Reconnect tries again")
                                .clicked();
                        });
                });
            } else if let Some(note) = &note {
                ui.centered_and_justified(|ui| ui.colored_label(egui::Color32::LIGHT_GRAY, note));
            } else if let Some([w, h]) = drawn {
                let rect = letterbox(area, w, h);
                picture_rect = Some(rect);
                let id = ui.id().with("computer-picture");
                // The picture takes the pointer and the keys, so egui's own
                // Tab, arrows and Escape go to the other computer.
                let response = ui.interact(rect, id, egui::Sense::click_and_drag());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Computer screen")
                });
                if response.clicked()
                    || response.drag_started()
                    || !ui.memory(|m| m.focused().is_some())
                {
                    response.request_focus();
                }
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    )
                });
                paint_picture(
                    ui,
                    &gpu,
                    on_gpu.is_some(),
                    texture.as_ref(),
                    rect,
                    egui::Color32::WHITE,
                );
            } else {
                ui.centered_and_justified(|ui| {
                    ui.colored_label(
                        egui::Color32::LIGHT_GRAY,
                        shown_words(&wanted.name, &status),
                    )
                });
            }
            // This pass's input, to the other computer.
            let mut state = lock(shared);
            let current = match (drawn, state.size) {
                (Some(size), Some((w, h))) => size == [usize::from(w), usize::from(h)],
                _ => false,
            };
            let on = controls(status.control, state.view_only, connected, current) && focused;
            let packets = match picture_rect {
                Some(rect) if on => {
                    let mut events = ctx.input(|i| i.events.clone());
                    keep_chord_here(&mut events);
                    state.input.take(&events, rect, Instant::now())
                }
                _ => state.input.release_all(),
            };
            send(&mut state, packets);
            #[cfg(windows)]
            {
                let controlling =
                    ended.is_none() && controls(status.control, state.view_only, connected, true);
                // Only a focus Windows said (review I1): unknown is not
                // focused for the grab.
                let said_focused = ctx.input(|i| i.viewport().focused == Some(true));
                follow_grab(&mut state, state_dir, &ctx, said_focused, controlling);
            }
            if state.input.waiting() {
                ctx.request_repaint_after(MOVE_EVERY);
            }
            drop(state);
            // How the picture comes, over its corner (PC2).
            if stats_on && ended.is_none() {
                let lines = stats_lines(&status, size, shown, &decoding);
                egui::Area::new(egui::Id::new("computer-view-stats"))
                    .fixed_pos(area.min + egui::vec2(8.0, 8.0))
                    .order(egui::Order::Foreground)
                    .interactable(false)
                    .show(&ctx, |ui| {
                        egui::Frame::NONE
                            .fill(egui::Color32::from_black_alpha(180))
                            .corner_radius(4.0)
                            .inner_margin(6.0)
                            .show(ui, |ui| {
                                for line in lines {
                                    ui.label(
                                        egui::RichText::new(line)
                                            .monospace()
                                            .small()
                                            .color(egui::Color32::from_gray(230)),
                                    );
                                }
                            });
                    });
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        });
    if cancel {
        cancel_reconnect(state_dir, &mut lock(shared));
    }
    if reconnect {
        let mut state = lock(shared);
        if let Err(e) = ask(state_dir, &mut state, wanted) {
            state.ended = Some(e);
        }
    }
}

/// `picture` as the window's texture (RGBA, or NV12 turned to RGBA here),
/// drawn from now on in place of the GPU's.
fn to_texture(state: &mut State, ctx: &egui::Context, picture: &Picture) {
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [picture.width, picture.height],
        &picture.to_rgba(),
    );
    match &mut state.texture {
        Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
        None => {
            state.texture =
                Some(ctx.load_texture("computer-view", image, egui::TextureOptions::LINEAR));
        }
    }
    state.on_gpu = None;
}

/// Draw the picture over `rect`, tinted: the GPU's NV12 (PC5) or the
/// texture.
fn paint_picture(
    ui: &egui::Ui,
    gpu: &Arc<crate::nv12::Gpu>,
    on_gpu: bool,
    texture: Option<&egui::TextureHandle>,
    rect: egui::Rect,
    tint: egui::Color32,
) {
    if on_gpu {
        ui.painter().add(gpu.paint(rect, tint));
    } else if let Some(texture) = texture {
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            tint,
        );
    }
}

/// Windows' keyboard grab, each pass (PC4): the keys the hook took go to
/// the other computer after the window's own (Alt before its Tab); the hook
/// goes in or comes out as [`crate::view_grab::Grab`] decides, and while it
/// is in the window beats for its watchdog. A hook the watchdog took out,
/// or the release chord, gives the keys back until the window leaves the
/// front; whatever the hook sent is let go as it comes out.
#[cfg(windows)]
fn follow_grab(
    state: &mut State,
    state_dir: &Path,
    ctx: &egui::Context,
    focused: bool,
    controlling: bool,
) {
    use crate::view_grab::{BEAT_EVERY, Heard, INSTALL, Situation};
    let now = Instant::now();
    let fullscreen = ctx
        .input(|i| i.viewport().fullscreen)
        .unwrap_or(state.fullscreen);
    // Where the shared keyboard is and whether privacy is on, read only
    // while the rest would put the hook in, every half second (review M5).
    let in_reach = focused && fullscreen && state.grab.setting && controlling;
    if in_reach
        && state
            .grab
            .across_read
            .is_none_or(|at| now.saturating_duration_since(at) >= ACROSS_LOOK)
    {
        look_around(state, state_dir, now);
    }
    let heard: Vec<Heard> = state
        .grab
        .hook
        .as_ref()
        .map(|hook| hook.heard.try_iter().collect())
        .unwrap_or_default();
    let mut packets = Vec::new();
    for heard in heard {
        match heard {
            Heard::Key(code, pressed) if focused && controlling => {
                packets.extend(state.input.grabbed(code, pressed));
            }
            Heard::Key(..) => {}
            Heard::Release => state.grab.grab.release(),
        }
    }
    let died = state
        .grab
        .hook
        .as_ref()
        .is_some_and(|hook| !hook.is_alive());
    if died {
        state.grab.grab.release();
    }
    let situation = |state: &State| Situation {
        focused,
        fullscreen,
        setting: state.grab.setting,
        controls: controlling,
        shared_across: state.grab.across,
        privacy: state.grab.privacy,
    };
    let mut wanted = state.grab.grab.wanted(situation(state));
    if wanted && state.grab.hook.is_none() {
        // Read afresh just before it goes in: a hook put in after shared
        // input's would come first in the chain (review M5).
        look_around(state, state_dir, now);
        wanted = state.grab.grab.wanted(situation(state));
    }
    if wanted && state.grab.hook.is_none() {
        state.grab.hook = INSTALL(ctx, viewport_id());
        if state.grab.hook.is_none() {
            // Not tried again each pass: the keys stay here until the
            // window leaves the front.
            state.grab.grab.release();
        }
    } else if !wanted && state.grab.hook.take().is_some() {
        packets.extend(state.input.release_all());
    }
    if let Some(hook) = &state.grab.hook {
        hook.beat();
        ctx.request_repaint_after(BEAT_EVERY);
    }
    send(state, packets);
}

/// Read where the shared keyboard is and whether privacy mode is on here,
/// for the keyboard grab (PC4).
#[cfg(windows)]
fn look_around(state: &mut State, state_dir: &Path, now: Instant) {
    state.grab.across_read = Some(now);
    state.grab.across = !pocketdesk_core::local::shared_input::read_status(state_dir)
        .keyboard_on
        .trim()
        .is_empty();
    state.grab.privacy = crate::view_grab::privacy_shown();
}

/// Save the picture shown as a PNG in Pictures\\Pocketdesk, and copy it;
/// the file is written off the window's thread.
fn screenshot(shared: &Arc<Mutex<State>>, ctx: &egui::Context, name: &str) {
    let Some((width, height, rgba)) = lock(shared)
        .shown
        .as_ref()
        .map(|p| (p.width, p.height, p.to_rgba()))
    else {
        return;
    };
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &rgba,
    ));
    let stem = format!("{} screen {}", file_safe(name), stamp(now_unix()));
    let shared = shared.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let note = crate::video::save_png(&stem, width, height, rgba);
        lock(&shared).saved_note = Some((note, Instant::now()));
        ctx.request_repaint_of(viewport_id());
    });
}

/// A computer's name fit for a file's: what a file system refuses becomes
/// a space.
pub fn file_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_end_matches('.').to_string();
    if safe.is_empty() {
        "Computer".to_string()
    } else {
        safe
    }
}

/// The worker: find the view this window wants, show it until it ends,
/// again.
fn watch(state_dir: &Path, shared: &Arc<Mutex<State>>, ctx: &egui::Context) {
    loop {
        std::thread::sleep(POLL);
        let (wanted, newer_than, asked) = {
            let mut state = lock(shared);
            let Some(wanted) = state.wanted.clone() else {
                continue;
            };
            if state.ended.is_some() {
                continue;
            }
            let file = wire::read(state_dir)
                .filter(|f| f.device_id == wanted.device_id && f.session_id > state.newer_than);
            if file.is_none() {
                if state.asked_at.is_some_and(|at| at.elapsed() >= NO_ANSWER) {
                    state.ended = Some(
                        "The server did not answer: is Pocketdesk's server running on this computer?"
                            .to_string(),
                    );
                    ctx.request_repaint_of(viewport_id());
                }
                continue;
            }
            (wanted, state.newer_than, state.asked)
        };
        let Some(file) = wire::read(state_dir)
            .filter(|f| f.device_id == wanted.device_id && f.session_id > newer_than)
        else {
            continue;
        };
        let ended = view(&file, shared, ctx);
        end_recording(shared, Some("the screen stopped"));
        let mut state = lock(shared);
        // The relay dropped this window but the view goes on (its queue
        // was full at a new size, BK7 review M3): it is joined again, with
        // what is held let go meanwhile.
        // Only a connection cut once joined: one refused is a relay gone.
        let cut = matches!(&ended, Err(e) if matches!(
            e.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        ));
        // Still the one this window wants (not closed, nor asked again or
        // for another screen meanwhile).
        let ours = state.asked == asked && state.wanted.as_ref() == Some(&wanted);
        let dropped =
            cut && ours && wire::read(state_dir).is_some_and(|f| f.session_id == file.session_id);
        // At most this often, then the view ends (BK7 re-review N3): a
        // relay that drops the window each time is not joined for ever.
        state.rejoins.retain(|at| at.elapsed() < REJOIN_WINDOW);
        let rejoin = dropped && state.rejoins.len() < REJOINS;
        if rejoin {
            state.rejoins.push(Instant::now());
            let release = state.input.release_all();
            send(&mut state, release);
            state.stream = None;
            state.writer = None;
            state.session_id = None;
            state.input = WindowInput::default();
            continue;
        }
        state.newer_than = state.newer_than.max(file.session_id);
        // Still the one this window wants: it says why it ended, a relay
        // that would not take it too.
        if ours {
            let why = match ended {
                _ if dropped => format!(
                    "The view of {} kept dropping this window. Reconnect to try again.",
                    wanted.name
                ),
                Ok(why) if !why.is_empty() => why,
                Ok(_) => format!("The view of {} ended.", wanted.name),
                Err(e) => {
                    format!("Lost the connection to this computer's Pocketdesk server ({e}).")
                }
            };
            state.ended = Some(why);
            let release = state.input.release_all();
            send(&mut state, release);
        }
        state.stream = None;
        state.writer = None;
        state.session_id = None;
        state.size = None;
        state.texture = None;
        state.on_gpu = None;
        state.picture = None;
        state.shown = None;
        state.input = WindowInput::default();
        ctx.request_repaint_of(viewport_id());
    }
}

/// Show one view until its end: why it ended, as the server said it.
fn view(file: &ViewFile, shared: &Arc<Mutex<State>>, ctx: &egui::Context) -> io::Result<String> {
    let token = file
        .token_bytes()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a bad token in view.json"))?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, file.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.write_all(&token)?;
    // A stuck write must not hold the window: input is best effort.
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    {
        let mut state = lock(shared);
        state.stream = Some(stream.try_clone()?);
        state.writer = Some(stream.try_clone()?);
        state.session_id = Some(file.session_id);
        state.asked_at = None;
    }
    let mut asks = stream.try_clone()?;
    let mut reader = BufReader::with_capacity(256 * 1024, stream);
    let mut decoder: Option<Decoder> = None;
    let mut gate = KeyGate::default();
    // The ways to decode, and the one this view settled on, if it did
    // (PC5): a rejoin of the same view keeps it.
    let ways = crate::video::local_accel_chain(lock(shared).hw_decode);
    let settled_on = match &lock(shared).way {
        Some((session, way)) if *session == file.session_id => Some(way.clone()),
        _ => None,
    };
    let mut ladder = Ladder::new(ways, settled_on.as_ref());
    // The stream's codec and size, for a decoder started again.
    let mut stream_is: Option<(Codec, Option<(u16, u16)>)> = None;
    let mut recorder: Option<Recorder> = None;
    loop {
        let message = Message::read(&mut reader)?;
        let want = lock(shared).record_want
            && matches!(message, Message::Frame { .. } | Message::Heartbeat);
        let stop = !lock(shared).record_want;
        if want && recorder.is_none() {
            let dir = user_folder("Videos");
            let name = file_safe(&file.name);
            let path = free_name(&dir, &format!("{name} screen {}", stamp(now_unix())), "mkv");
            // The stream's own codec (PC5 review I1): HEVC is copied as
            // HEVC.
            let codec = stream_is.map_or(Codec::H264, |(codec, _)| codec);
            let started =
                std::fs::create_dir_all(&dir).and_then(|()| Recorder::start(&path, codec));
            let mut state = lock(shared);
            match started {
                Ok(r) => {
                    recorder = Some(r);
                    state.recording = Some((path, Instant::now()));
                    Message::WantKeyframe.write(&mut asks)?;
                }
                Err(e) => {
                    state.record_want = false;
                    let why = if e.kind() == io::ErrorKind::NotFound {
                        format!("Not recording: {e} (it needs ffmpeg).")
                    } else {
                        format!("Not recording: {e}.")
                    };
                    state.saved_note = Some((why, Instant::now()));
                }
            }
        } else if stop && recorder.is_some() {
            recorder = None;
            end_recording(shared, None);
            ctx.request_repaint_of(viewport_id());
        }
        match message {
            Message::Status(status) => {
                let mut state = lock(shared);
                // The quality chosen goes once the computer has let it in.
                if status.phase == Phase::Showing && state.quality_sent != Some(file.session_id) {
                    state.quality_sent = Some(file.session_id);
                    let preset = state.quality;
                    send(&mut state, vec![InputPacket::QualityPreset { preset }]);
                }
                state.status = status;
                ctx.request_repaint_of(viewport_id());
            }
            Message::Config { width, height } => {
                // Another size: the decoder starts again at it, from the
                // next keyframe.
                if recorder.take().is_some() {
                    end_recording(shared, Some("it stopped as the screen changed size"));
                }
                stream_is = Some((Codec::H264, Some((width, height))));
                decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                gate = KeyGate::default();
                ladder.started();
            }
            Message::HevcConfig => {
                // PC5: HEVC from the next keyframe, its size the first
                // picture's.
                if recorder.take().is_some() {
                    end_recording(shared, Some("it stopped as the picture changed"));
                }
                stream_is = Some((Codec::Hevc, None));
                decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                gate = KeyGate::default();
                ladder.started();
            }
            Message::Frame {
                keyframe,
                seq,
                data,
            } => {
                climb(
                    shared,
                    ctx,
                    file.session_id,
                    &mut ladder,
                    &mut decoder,
                    &mut gate,
                    stream_is,
                );
                // The GPU's shader failed (PC5): the decoder gives RGBA.
                if decoder
                    .as_ref()
                    .is_some_and(|d| d.planes() != crate::nv12::planes())
                {
                    decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                    gate = KeyGate::default();
                    ladder.started();
                }
                let Some(d) = decoder.as_mut() else { continue };
                match gate.admit(keyframe, seq) {
                    Admit::Feed => {}
                    Admit::Skip => continue,
                    Admit::SkipAndAsk => {
                        Message::WantKeyframe.write(&mut asks)?;
                        continue;
                    }
                }
                if let Some(r) = recorder.as_mut()
                    && let Err(e) = r.feed(keyframe, &data)
                {
                    recorder = None;
                    end_recording(shared, Some(&format!("it stopped ({e})")));
                }
                lock(shared).clock.fed(Instant::now());
                ladder.fed(Instant::now());
                if let Err(e) = d.feed(&data) {
                    decoder = None;
                    if ladder.failed() == Step::Next {
                        // The next way, from the next keyframe (PC5).
                        decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                        gate = KeyGate::default();
                    } else {
                        lock(shared).note = Some(format!("The decoder stopped ({e})."));
                        ctx.request_repaint_of(viewport_id());
                    }
                }
            }
            Message::End { why } => return Ok(why),
            // A still screen sends no frames, only the relay's heartbeat
            // every 2 s: a card that stalled on its first keyframe is left
            // then too, not at the next movement (PR #562 review M2).
            Message::Heartbeat => {
                let step = climb(
                    shared,
                    ctx,
                    file.session_id,
                    &mut ladder,
                    &mut decoder,
                    &mut gate,
                    stream_is,
                );
                // The next way starts from a keyframe, asked for now.
                if step == Step::Next {
                    Message::WantKeyframe.write(&mut asks)?;
                }
            }
            Message::WantKeyframe | Message::Input(_) | Message::Focus(_) | Message::Privacy(_) => {
            }
        }
    }
}

/// What the ladder of ways says to do now (PC5 review M9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Keep the decoder.
    Stay,
    /// Start the next way, from a keyframe.
    Next,
    /// The way gave its first picture: it holds for the view.
    Settled,
    /// The last way gave up too.
    Exhausted,
}

/// The ways to decode in order, the one tried now, and whether it gave a
/// picture (PC5): a way that ends or gives nothing for [`NO_PICTURE`] after
/// its first access unit, or cannot take one, gives way to the next.
#[derive(Debug, Clone)]
pub struct Ladder {
    ways: Vec<Accel>,
    at: usize,
    settled: bool,
    first_fed: Option<Instant>,
    exhausted: bool,
}

impl Ladder {
    /// The ways, settled already on `settled_on` (a rejoin of the same
    /// view) when it is still among them, else trying the first.
    pub fn new(ways: Vec<Accel>, settled_on: Option<&Accel>) -> Self {
        let ways = if ways.is_empty() {
            vec![Accel::Software]
        } else {
            ways
        };
        let settled = settled_on.and_then(|on| ways.iter().position(|w| w == on));
        Self {
            ways,
            at: settled.unwrap_or(0),
            settled: settled.is_some(),
            first_fed: None,
            exhausted: false,
        }
    }

    /// The way tried now.
    pub fn way(&self) -> &Accel {
        &self.ways[self.at]
    }

    /// A decoder of this way started again (another size or codec).
    pub fn started(&mut self) {
        self.first_fed = None;
    }

    /// An access unit went to the decoder at `at`.
    pub fn fed(&mut self, at: Instant) {
        self.first_fed.get_or_insert(at);
    }

    /// The decoder has given `pictures` and `ended` or not, at `now`.
    pub fn check(&mut self, pictures: u64, ended: bool, now: Instant) -> Step {
        if self.settled {
            return Step::Stay;
        }
        if pictures > 0 {
            self.settled = true;
            return Step::Settled;
        }
        if give_up(pictures, ended, self.first_fed, now) {
            return self.next();
        }
        Step::Stay
    }

    /// The decoder could not take an access unit.
    pub fn failed(&mut self) -> Step {
        self.settled = false;
        self.next()
    }

    fn next(&mut self) -> Step {
        self.first_fed = None;
        if self.at + 1 < self.ways.len() {
            self.at += 1;
            Step::Next
        } else if !self.exhausted {
            self.exhausted = true;
            Step::Exhausted
        } else {
            Step::Stay
        }
    }
}

/// Follow the ladder for the decoder now (PC5): the next way started, or
/// the way that gave a picture kept for the view and said.
#[allow(clippy::too_many_arguments)]
fn climb(
    shared: &Arc<Mutex<State>>,
    ctx: &egui::Context,
    session_id: u64,
    ladder: &mut Ladder,
    decoder: &mut Option<Decoder>,
    gate: &mut KeyGate,
    stream_is: Option<(Codec, Option<(u16, u16)>)>,
) -> Step {
    let Some(d) = decoder.as_ref() else {
        return Step::Stay;
    };
    let step = ladder.check(d.pictures(), d.ended(), Instant::now());
    match step {
        Step::Next => {
            *decoder = start_decoder(shared, ctx, stream_is, ladder.way());
            *gate = KeyGate::default();
        }
        Step::Settled => {
            let words = way_words(ladder.way(), d.auto_took());
            let mut state = lock(shared);
            state.way = Some((session_id, ladder.way().clone()));
            state.decoding = words;
        }
        Step::Stay | Step::Exhausted => {}
    }
    step
}

/// A decoder for a stream that `is` this codec and size, decoding `way`
/// (PC5); the window says the way, or why none started.
fn start_decoder(
    shared: &Arc<Mutex<State>>,
    ctx: &egui::Context,
    is: Option<(Codec, Option<(u16, u16)>)>,
    way: &Accel,
) -> Option<Decoder> {
    let (codec, size) = is?;
    let wanted = DecodeWanted {
        codec,
        size,
        accel: way.clone(),
        planes: crate::nv12::planes(),
    };
    // A new decoder: the stream's size, or none until its first picture,
    // and only its own pictures from now on (PR #562 review I2).
    let decoder_gen = {
        let mut state = lock(shared);
        state.decoder_gen += 1;
        state.size = size;
        state.decoder_gen
    };
    let started = Decoder::open(&wanted, publisher(shared, ctx, decoder_gen));
    let mut state = lock(shared);
    state.clock.restart();
    state.decoding = way.words();
    let decoder = match started {
        Ok(d) => {
            state.note = None;
            Some(d)
        }
        Err(e) => {
            state.note = Some(crate::video::decoder_words(&e, "another computer's screen"));
            None
        }
    };
    ctx.request_repaint_of(viewport_id());
    decoder
}

/// The overlay's word for the way a view settled on: `auto` with what it
/// took, when ffmpeg said (PC5).
pub fn way_words(way: &Accel, auto_took: Option<String>) -> String {
    match (way, auto_took) {
        (Accel::Auto, Some(took)) => format!("auto ({took})"),
        (Accel::Auto, None) => "auto (software)".to_string(),
        _ => way.words(),
    }
}

/// A recording ended: the window says where it went, and why it stopped.
fn end_recording(shared: &Mutex<State>, why: Option<&str>) {
    let mut state = lock(shared);
    state.record_want = false;
    let Some((path, _)) = state.recording.take() else {
        return;
    };
    let kept = std::fs::metadata(&path).is_ok_and(|m| m.len() > 0);
    let note = match (kept, why) {
        (true, None) => format!("Recorded {}.", path.display()),
        (true, Some(why)) => format!("Recorded {}; {why}.", path.display()),
        (false, Some(why)) => format!("Nothing recorded: {why}."),
        (false, None) => "Nothing recorded: no picture came.".to_string(),
    };
    state.saved_note = Some((note, Instant::now()));
}

/// Where a decoded picture of the decoder `decoder_gen` goes: the window's
/// next pass takes it, unless a newer decoder has started since.
fn publisher(
    shared: &Arc<Mutex<State>>,
    ctx: &egui::Context,
    decoder_gen: u64,
) -> Box<dyn FnMut(Picture) + Send> {
    let shared = shared.clone();
    let ctx = ctx.clone();
    Box::new(move |picture| {
        let mut state = lock(&shared);
        if state.decoder_gen != decoder_gen {
            return;
        }
        state.clock.decoded(Instant::now(), picture.data.len());
        // HEVC's size is the first picture's (PC5).
        if state.size.is_none()
            && let (Ok(w), Ok(h)) = (u16::try_from(picture.width), u16::try_from(picture.height))
        {
            state.size = Some((w, h));
        }
        state.picture = Some(picture);
        drop(state);
        ctx.request_repaint_of(viewport_id());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::view::ViewStats;

    #[test]
    fn the_window_says_how_the_view_stands() {
        let mut status = ViewStatus::default();
        assert_eq!(waiting_words("Desk", &status), "Reaching Desk\u{2026}");
        status.phase = Phase::Waiting;
        assert!(waiting_words("Desk", &status).contains("let this computer in"));
        status.phase = Phase::Showing;
        assert_eq!(
            waiting_words("Desk", &status),
            "Waiting for Desk's screen\u{2026}"
        );
    }

    /// BK7 review M6: a note from the server takes the waiting words'
    /// place.
    #[test]
    fn the_servers_note_is_said_in_place_of_waiting() {
        let mut status = ViewStatus {
            phase: Phase::Showing,
            ..ViewStatus::default()
        };
        assert_eq!(shown_words("Desk", &status), waiting_words("Desk", &status));
        status.note = Some("Desk's screen comes in frames too large to show here.".into());
        assert_eq!(
            shown_words("Desk", &status),
            "Desk's screen comes in frames too large to show here."
        );
    }

    /// View-only when the other computer refuses input says where control
    /// is turned on; the window's own switch says it sends nothing.
    #[test]
    fn view_only_says_why_and_where_control_is_turned_on() {
        let refused = control_words("Desk", Some(false), false).unwrap();
        assert!(refused.contains("Devices page") && refused.contains("Desk"));
        assert!(
            control_words("Desk", Some(false), true)
                .unwrap()
                .contains("Devices page")
        );
        assert_eq!(
            control_words("Desk", Some(true), true).unwrap(),
            "View only: this window sends nothing."
        );
        assert_eq!(control_words("Desk", Some(true), false), None);
        assert_eq!(control_words("Desk", None, false), None);
    }

    /// PC8: the bar's switch shows asked or held as on, and the window says
    /// how privacy stands there, never that it blanks the screen.
    #[test]
    fn the_window_says_how_privacy_stands_there() {
        assert!(!hiding(&ViewPrivacy::Off));
        assert!(hiding(&ViewPrivacy::Asking));
        assert!(hiding(&ViewPrivacy::On));
        assert!(!hiding(&ViewPrivacy::Refused("no".into())));
        assert_eq!(privacy_words("Desk", &ViewPrivacy::Off), None);
        assert!(
            privacy_words("Desk", &ViewPrivacy::Asking)
                .unwrap()
                .starts_with("Asking Desk")
        );
        let on = privacy_words("Desk", &ViewPrivacy::On).unwrap();
        assert!(on.contains("keyboard and mouse are blocked"), "{on}");
        assert!(on.contains("does not blank"), "{on}");
        assert_eq!(
            privacy_words("Desk", &ViewPrivacy::Refused("it is off there".into())).unwrap(),
            "Not hidden on Desk: it is off there"
        );
    }

    /// PR #561 review: "Asking…" is given up after the relay's own wait,
    /// and only while it is still asking.
    #[test]
    fn asking_is_given_up_after_the_relays_wait() {
        let at = Instant::now();
        let later = at + ASKING_WAIT;
        assert!(!asking_lapsed(&ViewPrivacy::Asking, Some(at), at));
        assert!(asking_lapsed(&ViewPrivacy::Asking, Some(at), later));
        assert!(
            !asking_lapsed(&ViewPrivacy::On, Some(at), later),
            "answered"
        );
        assert!(!asking_lapsed(&ViewPrivacy::Asking, None, later));
        assert!(
            ASKING_WAIT > Duration::from_secs(10),
            "past the relay's give-up"
        );
    }

    /// PC8: the window locks the other computer as it closes only when
    /// chosen and once it was let in.
    #[test]
    fn the_window_locks_it_on_close_only_when_chosen_and_let_in() {
        assert_eq!(closing_ask(true, Phase::Showing), Some(wire::PRIVACY_LOCK));
        assert_eq!(closing_ask(false, Phase::Showing), None);
        assert_eq!(closing_ask(true, Phase::Waiting), None);
        assert_eq!(closing_ask(true, Phase::Connecting), None);
    }

    /// PC8: the ask goes to the relay as its one-byte message and the bar
    /// shows it asked at once; with no relay nothing is said.
    #[test]
    fn a_privacy_ask_goes_to_the_relay() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let mut state = State {
            writer: Some(writer),
            ..State::default()
        };
        say_privacy(&mut state, wire::PRIVACY_HIDE);
        assert_eq!(state.status.privacy, ViewPrivacy::Asking);
        say_privacy(&mut state, wire::PRIVACY_LOCK);
        assert_eq!(state.status.privacy, ViewPrivacy::Off);
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::Privacy(wire::PRIVACY_HIDE)
        );
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::Privacy(wire::PRIVACY_LOCK)
        );
        let mut alone = State::default();
        say_privacy(&mut alone, wire::PRIVACY_HIDE);
        assert_eq!(
            alone.status.privacy,
            ViewPrivacy::Off,
            "no relay, not asked"
        );
    }

    #[test]
    fn input_goes_only_with_control_taken_and_the_picture_current() {
        assert!(controls(Some(true), false, true, true));
        assert!(
            !controls(Some(false), false, true, true),
            "the host refuses input"
        );
        assert!(!controls(None, false, true, true), "not said yet");
        assert!(!controls(Some(true), true, true, true), "view only here");
        assert!(!controls(Some(true), false, false, true), "no relay");
        assert!(
            !controls(Some(true), false, true, false),
            "a picture of the old size would put a click in the wrong place"
        );
    }

    #[test]
    fn the_screens_are_named_or_numbered() {
        let mut status = ViewStatus::default();
        assert_eq!(screen_choices(&status), ["Screen 1"]);
        status.monitor = 1;
        assert_eq!(screen_choices(&status), ["Screen 1", "Screen 2"]);
        status.monitors = vec!["eDP-1 (1920x1080)".into(), "HDMI-A-1 (2560x1440)".into()];
        assert_eq!(screen_choices(&status).len(), 2);
    }

    /// PC2: the overlay says the road and how it was reached, the picture,
    /// the frames that came and were shown, the bitrate, the round trip,
    /// the decoder's time and the frames lost, late and dropped.
    #[test]
    fn the_stats_say_the_road_the_picture_and_the_numbers() {
        let mut status = ViewStatus {
            phase: Phase::Showing,
            route: "internet".into(),
            stats: Some(ViewStats {
                fps: 59,
                kbps: 8_400,
                rtt_ms: Some(23),
                relayed: true,
                dial_ms: Some(640),
                lost: 2,
                late: 1,
                dropped: 0,
                codec: "H.264".into(),
            }),
            ..ViewStatus::default()
        };
        let shown = Shown {
            fps: 58,
            decode: Some(Duration::from_micros(6_100)),
            decode_p95: Some(Duration::from_micros(9_400)),
            bytes: 3_110_400,
        };
        assert_eq!(
            stats_lines(&status, Some((1920, 1080)), shown, "d3d11va"),
            [
                "Road: internet, by a relay (reached in 640 ms)",
                "Picture: 1920\u{d7}1080 H.264",
                "Frames: 59 a second in, 58 shown",
                "Bitrate: 8.4 Mb/s",
                "Round trip: 23 ms",
                "Decode: 6.1 ms (p95 9.4 ms) \u{b7} d3d11va",
                "Pipe: 3.1 MB a picture",
                "Lost 2 \u{b7} late 1 \u{b7} dropped 0",
            ]
        );
        status.stats.as_mut().unwrap().relayed = false;
        assert_eq!(
            stats_lines(&status, Some((1920, 1080)), shown, "d3d11va")[0],
            "Road: internet, direct (reached in 640 ms)"
        );
        status.route = "home network".into();
        assert_eq!(
            stats_lines(&status, Some((1920, 1080)), shown, "d3d11va")[0],
            "Road: home network (reached in 640 ms)"
        );
        // Nothing known yet.
        let lines = stats_lines(&ViewStatus::default(), None, Shown::default(), "");
        assert_eq!(lines[0], "Road: not yet");
        assert_eq!(lines[1], "Picture: not yet");
        assert_eq!(lines[4], "Round trip: not known");
        assert_eq!(lines[5], "Decode: not yet");
        assert_eq!(lines[6], "Pipe: not yet");
        assert_eq!(bitrate_words(950), "950 kb/s");
        assert_eq!(bytes_words(8_294_400), "8.3 MB");
        assert_eq!(bytes_words(4_200), "4 KB");
        assert_eq!(bytes_words(12), "12 B");
    }

    /// PR #562 review I2: a picture from a decoder already replaced is
    /// dropped, so it cannot give the new stream its size; the current
    /// decoder's first picture does.
    #[test]
    fn only_the_current_decoders_pictures_count() {
        let shared = Arc::new(Mutex::new(State::default()));
        let ctx = egui::Context::default();
        let picture = |width: usize, height: usize| Picture {
            width,
            height,
            planes: Planes::Rgba,
            data: vec![0; width * height * 4],
            colour: crate::video::Colour::default(),
        };
        lock(&shared).decoder_gen = 2;
        let mut old = publisher(&shared, &ctx, 1);
        old(picture(64, 36));
        {
            let state = lock(&shared);
            assert!(state.picture.is_none(), "the old decoder's picture");
            assert_eq!(state.size, None, "and not its size");
        }
        let mut current = publisher(&shared, &ctx, 2);
        current(picture(32, 18));
        assert_eq!(lock(&shared).size, Some((32, 18)));
        current(picture(16, 8));
        assert_eq!(lock(&shared).size, Some((32, 18)), "the first picture's");
        old(picture(64, 36));
        assert_eq!(
            lock(&shared).picture.as_ref().map(|p| p.width),
            Some(16),
            "a late old picture replaces nothing"
        );
    }

    /// PC5: a way is given up when it never gave a picture and ended, or
    /// had two seconds since its first access unit; one that gave a picture
    /// is kept.
    #[test]
    fn a_way_that_gives_no_picture_is_given_up() {
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        assert!(!give_up(0, false, None, at(9_000)), "not fed yet");
        assert!(!give_up(0, false, Some(t), at(1_999)));
        assert!(give_up(0, false, Some(t), at(2_000)));
        assert!(give_up(0, true, None, t), "ffmpeg ended at once");
        assert!(!give_up(1, true, Some(t), at(9_000)), "it gave a picture");
        assert!(!give_up(5, false, Some(t), at(9_000)));
        assert_eq!(way_words(&Accel::D3d11va, None), "d3d11va");
        assert_eq!(way_words(&Accel::Software, None), "software");
        assert_eq!(
            way_words(&Accel::Auto, Some("dxva2".into())),
            "auto (dxva2)"
        );
        assert_eq!(way_words(&Accel::Auto, None), "auto (software)");
    }

    /// PC5 review M9: the ladder goes to the next way when one gives no
    /// picture in time, ends or cannot take a unit; settles on the one
    /// that gives a picture and keeps it; says once that the last gave up.
    #[test]
    fn the_ladder_climbs_settles_and_runs_out() {
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        let ways = vec![Accel::D3d11va, Accel::Auto, Accel::Software];
        let mut ladder = Ladder::new(ways.clone(), None);
        assert_eq!(ladder.way(), &Accel::D3d11va);
        assert_eq!(ladder.check(0, false, at(9_000)), Step::Stay, "not fed");
        ladder.fed(at(0));
        ladder.fed(at(500));
        assert_eq!(ladder.check(0, false, at(1_999)), Step::Stay);
        assert_eq!(ladder.check(0, false, at(2_000)), Step::Next);
        assert_eq!(ladder.way(), &Accel::Auto);
        // The new way's clock starts at its own first unit.
        assert_eq!(ladder.check(0, false, at(5_000)), Step::Stay);
        assert_eq!(ladder.check(0, true, at(5_000)), Step::Next, "ended");
        assert_eq!(ladder.way(), &Accel::Software);
        ladder.fed(at(6_000));
        assert_eq!(ladder.check(1, false, at(6_010)), Step::Settled);
        assert_eq!(ladder.check(0, true, at(99_000)), Step::Stay, "settled");
        // A feed error leaves the last way: said once.
        assert_eq!(ladder.failed(), Step::Exhausted);
        assert_eq!(ladder.failed(), Step::Stay);
        // A feed error on a card goes to the next.
        let mut ladder = Ladder::new(ways.clone(), None);
        assert_eq!(ladder.failed(), Step::Next);
        assert_eq!(ladder.way(), &Accel::Auto);
        // A rejoin starts settled on the way the view settled on (review
        // M1); one no longer among the ways (the setting changed) starts
        // afresh, unsettled.
        let mut ladder = Ladder::new(ways.clone(), Some(&Accel::Auto));
        assert_eq!(ladder.way(), &Accel::Auto);
        assert_eq!(ladder.check(0, true, at(0)), Step::Stay);
        let software_only = vec![Accel::Software];
        let mut ladder = Ladder::new(ways.clone(), Some(&Accel::Software));
        assert_eq!(ladder.way(), &Accel::Software);
        let mut afresh = Ladder::new(ways, Some(&Accel::Vaapi("/dev/dri/renderD128".into())));
        assert_eq!(afresh.way(), &Accel::D3d11va);
        afresh.fed(at(0));
        assert_eq!(afresh.check(0, false, at(2_000)), Step::Next, "not settled");
        let mut card = Ladder::new(software_only, Some(&Accel::D3d11va));
        assert_eq!(card.way(), &Accel::Software);
        assert_eq!(ladder.check(0, true, at(0)), Step::Stay);
        assert_eq!(card.check(0, true, at(0)), Step::Exhausted);
        assert_eq!(Ladder::new(Vec::new(), None).way(), &Accel::Software);
    }

    /// PC5: the percentiles by nearest rank.
    #[test]
    fn a_percentile_is_by_nearest_rank() {
        let d = Duration::from_millis;
        assert_eq!(percentile(&mut [], 0.5), None);
        assert_eq!(percentile(&mut [d(3)], 0.95), Some(d(3)));
        let mut samples: Vec<Duration> = (1..=20).rev().map(d).collect();
        assert_eq!(percentile(&mut samples, 0.5), Some(d(10)));
        assert_eq!(percentile(&mut samples, 0.95), Some(d(19)));
        assert_eq!(percentile(&mut samples, 1.0), Some(d(20)));
        assert_eq!(percentile(&mut samples, 0.0), Some(d(1)));
    }

    /// PC2: each picture is timed against the access unit it came from,
    /// and the last whole second's pictures and decode time are what the
    /// overlay says.
    #[test]
    fn the_decoder_is_timed_and_the_pictures_counted() {
        let t = Instant::now();
        let ms = |n: u64| t + Duration::from_millis(n);
        let mut clock = DecodeClock::default();
        assert_eq!(clock.shown(t), Shown::default());
        for n in 0..30u64 {
            clock.fed(ms(n * 33));
            // One slow picture in thirty: the median keeps to the rest,
            // the 95th percentile shows it (PC5).
            clock.decoded(ms(n * 33 + if n == 7 { 40 } else { 5 }), 100);
        }
        clock.fed(ms(1000));
        clock.decoded(ms(1006), 400);
        let shown = clock.shown(ms(1006));
        assert_eq!(shown.fps, 30);
        assert_eq!(shown.decode.map(|d| d.as_millis()), Some(5));
        assert_eq!(shown.decode_p95.map(|d| d.as_millis()), Some(6));
        assert_eq!(shown.bytes, (30 * 100 + 400) / 31);
        // Nothing for two seconds: none shown, no decode time.
        assert_eq!(clock.shown(ms(3100)), Shown::default());
        // A new decoder forgets the units the old one was fed: a picture
        // with none waiting is counted, not timed.
        clock.fed(ms(3200));
        clock.restart();
        clock.decoded(ms(3900), 1);
        clock.fed(ms(4000));
        clock.decoded(ms(4002), 1);
        clock.decoded(ms(4004), 1);
        let shown = clock.shown(ms(4200));
        assert_eq!(shown.fps, 2, "{shown:?}");
        assert_eq!(shown.decode.map(|d| d.as_millis()), Some(2));
        // A decoder that drops units does not grow the waiting list for
        // ever.
        for n in 0..1000u64 {
            clock.fed(ms(5000 + n));
        }
        assert!(clock.fed.len() <= 32);
    }

    /// PC2: while the server dials again after a drop the window says so,
    /// with the try it is on.
    #[test]
    fn the_window_says_it_is_reconnecting() {
        let mut status = ViewStatus::default();
        assert_eq!(reconnect_words("Desk", &status), None);
        status.reconnecting = 2;
        assert_eq!(
            reconnect_words("Desk", &status).as_deref(),
            Some("Reconnecting to Desk\u{2026} (2)")
        );
        status.phase = Phase::Waiting;
        assert!(
            reconnect_words("Desk", &status)
                .unwrap()
                .contains("let this computer in")
        );
    }

    /// PC2: Cancel while it reconnects stops the view on the server (its
    /// StopView names the view), and the window says so, with Reconnect.
    #[test]
    fn cancel_stops_the_reconnecting_view() {
        let dir = std::env::temp_dir().join(format!("pd-computer-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            session_id: Some(41),
            status: ViewStatus {
                reconnecting: 3,
                ..ViewStatus::default()
            },
            ..State::default()
        };
        let asked = state.asked;
        cancel_reconnect(&dir, &mut state);
        assert_eq!(
            state.ended.as_deref(),
            Some("Stopped reconnecting to Desk.")
        );
        assert!(state.wanted.is_some(), "the window stays, with Reconnect");
        assert_eq!(state.session_id, None);
        assert!(state.asked > asked, "the view's end is not this window's");
        assert_eq!(state.newer_than, 41);
        let sent = computers::take_all(&dir, now_unix());
        assert_eq!(
            sent.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::StopView {
                session_id: Some(41)
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC4: the keyboard grab is asked for only with the window in front,
    /// full screen, controlling and the setting on, never with shared input
    /// across; in a test the stand-in installer is asked, so no hook goes
    /// in, and one that does not go in is not asked for again each pass.
    #[cfg(windows)]
    #[test]
    fn the_grab_is_asked_for_only_in_front_and_never_hooks_in_a_test() {
        let dir = std::env::temp_dir().join(format!("pd-computer-grab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let asks = crate::view_grab::refused_here;
        let fresh = |fullscreen: bool, setting: bool| {
            let mut state = State {
                fullscreen,
                ..State::default()
            };
            state.grab.setting = setting;
            state
        };
        for (what, mut state, focused, controlling) in [
            ("windowed", fresh(false, true), true, true),
            ("setting off", fresh(true, false), true, true),
            ("no focus", fresh(true, true), false, true),
            ("not controlling", fresh(true, true), true, false),
        ] {
            let before = asks();
            follow_grab(&mut state, &dir, &ctx, focused, controlling);
            assert_eq!(asks(), before, "{what}");
        }
        let mut state = fresh(true, true);
        let before = asks();
        follow_grab(&mut state, &dir, &ctx, true, true);
        assert_eq!(asks(), before + 1, "in front: asked");
        assert!(state.grab.hook.is_none(), "and nothing went in");
        follow_grab(&mut state, &dir, &ctx, true, true);
        assert_eq!(asks(), before + 1, "not asked again each pass");
        // Shared input has the keyboard across: the server's hook has it.
        pocketdesk_core::local::shared_input::write_status(
            &dir,
            &pocketdesk_core::local::shared_input::StatusFile {
                keyboard_on: "laptop".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mut state = fresh(true, true);
        let before = asks();
        follow_grab(&mut state, &dir, &ctx, true, true);
        assert_eq!(asks(), before, "shared input across");
        // Review M5: read just now as home, it is read again before the
        // hook would go in.
        let mut state = fresh(true, true);
        state.grab.across_read = Some(Instant::now());
        let before = asks();
        follow_grab(&mut state, &dir, &ctx, true, true);
        assert_eq!(asks(), before, "read afresh before going in");
        // Windowed, the file is not read at all.
        let mut state = fresh(false, true);
        follow_grab(&mut state, &dir, &ctx, true, true);
        assert!(state.grab.across_read.is_none(), "not read while windowed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #551 review: Stats switched in Settings reaches a window already
    /// open; opening the same computer again keeps the window's own.
    #[test]
    fn the_settings_stats_reach_an_open_window() {
        let dir = std::env::temp_dir().join(format!("pd-computer-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        assert!(!lock(&view.shared).stats_on);
        view.set_stats(true);
        assert!(lock(&view.shared).stats_on);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        assert!(lock(&view.shared).stats_on, "the same window keeps it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #551 review M8: Ctrl+Alt+Shift+S switches the stats, as
    /// Moonlight's does; it stays here, never reaching the other computer,
    /// and S alone or with fewer keys is the other computer's.
    #[test]
    fn ctrl_alt_shift_s_switches_the_stats_here() {
        let key = |key, pressed, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let all = egui::Modifiers {
            alt: true,
            ctrl: true,
            shift: true,
            mac_cmd: false,
            command: true,
        };
        assert!(stats_chord(&key(egui::Key::S, true, all)));
        assert!(!stats_chord(&key(egui::Key::S, false, all)), "on the press");
        assert!(!stats_chord(&key(
            egui::Key::S,
            true,
            egui::Modifiers::CTRL
        )));
        assert!(!stats_chord(&key(egui::Key::D, true, all)));
        let mut events = vec![
            key(egui::Key::S, true, all),
            key(egui::Key::S, false, all),
            key(egui::Key::A, true, egui::Modifiers::NONE),
        ];
        keep_chord_here(&mut events);
        assert_eq!(events.len(), 1, "only the A goes on");
    }

    #[test]
    fn a_name_is_fit_for_a_file() {
        assert_eq!(file_safe("Desk"), "Desk");
        assert_eq!(file_safe("a/b:c"), "a b c");
        assert_eq!(file_safe("..."), "Computer");
    }

    /// BK7 review M3: a relay that drops the window (its queue was full at a
    /// new size) while `view.json` still names the view is joined again,
    /// not taken for the view's end.
    #[test]
    fn a_window_dropped_by_the_relay_joins_the_view_again() {
        let dir = std::env::temp_dir().join(format!("pd-computer-rejoin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        struct Remove(PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _remove = Remove(dir.clone());
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = [4u8; pocketdesk_core::local::cast::TOKEN_LEN];
        wire::write(
            &dir,
            &ViewFile {
                session_id: 20,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        listener.set_nonblocking(true).unwrap();
        let join = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut relay = loop {
                match listener.accept() {
                    Ok((relay, _)) => break relay,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "joined within 10 s");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            relay.set_nonblocking(false).unwrap();
            relay
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut given = [0u8; 32];
            std::io::Read::read_exact(&mut relay, &mut given).unwrap();
            assert_eq!(given, token);
            relay
        };
        let first = join();
        drop(first);
        // Joined again, and nothing said ended meanwhile.
        let mut second = join();
        assert_eq!(lock(&view.shared).ended, None);
        Message::End {
            why: "Lost the connection to Desk.".into(),
        }
        .write(&mut second)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            lock(&view.shared).ended.as_deref(),
            Some("Lost the connection to Desk.")
        );

        // BK7 re-review N3: a relay that drops the window each time it
        // joins is joined at most three times in ten seconds, then the view
        // ends, saying so.
        wire::write(
            &dir,
            &ViewFile {
                session_id: 21,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        for _ in 0..4 {
            drop(join());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            lock(&view.shared)
                .ended
                .as_deref()
                .is_some_and(|why| why.contains("kept dropping")),
            "{:?}",
            lock(&view.shared).ended
        );
        std::thread::sleep(Duration::from_millis(1000));
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "not joined a fifth time"
        );
    }

    /// A relay that is gone (nothing at its port) ends the view with why,
    /// rather than being tried for ever.
    #[test]
    fn a_relay_gone_ends_the_view() {
        let dir = std::env::temp_dir().join(format!("pd-computer-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let port = {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        wire::write(
            &dir,
            &ViewFile {
                session_id: 30,
                device_id: "desk".into(),
                name: "Desk".into(),
                port,
                token: pocketdesk_core::local::cast::token_hex(&[5u8; 32]),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(lock(&view.shared).ended.as_deref().is_some_and(|why| {
            why.starts_with("Lost the connection to this computer's Pocketdesk server")
        }),);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The worker against a stand-in relay: it finds the view the window
    /// asked for, decodes its frames to pictures of the size the relay
    /// says, sends the chosen quality once the view is shown, and keeps the
    /// reason it ended. Needs ffmpeg with libx264; skipped without.
    #[test]
    fn the_worker_shows_a_view_and_keeps_why_it_ended() {
        let encoded = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=10",
                "-frames:v",
                "3",
                "-c:v",
                "libx264",
                "-bf",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stderr(std::process::Stdio::null())
            .output();
        let Some(encoded) = encoded
            .ok()
            .filter(|o| o.status.success() && !o.stdout.is_empty())
        else {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("pd-computer-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = [3u8; pocketdesk_core::local::cast::TOKEN_LEN];
        wire::write(
            &dir,
            &ViewFile {
                session_id: 10,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 3, false, false).unwrap();
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::View {
                device_id: "desk".into(),
                monitor: 0
            }]
        );
        let (mut relay, _) = listener.accept().unwrap();
        relay
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut given = [0u8; 32];
        std::io::Read::read_exact(&mut relay, &mut given).unwrap();
        assert_eq!(given, token);
        Message::Status(ViewStatus {
            phase: Phase::Showing,
            control: Some(true),
            ..ViewStatus::default()
        })
        .write(&mut relay)
        .unwrap();
        Message::Config {
            width: 64,
            height: 48,
        }
        .write(&mut relay)
        .unwrap();
        Message::Frame {
            keyframe: true,
            seq: 0,
            data: encoded.stdout.clone(),
        }
        .write(&mut relay)
        .unwrap();
        // The quality chosen (3, Economy) goes once shown.
        match Message::read(&mut relay).unwrap() {
            Message::Input(bytes) => assert!(matches!(
                InputPacket::decode(&bytes).unwrap(),
                InputPacket::QualityPreset { preset: 3 }
            )),
            other => panic!("{other:?}"),
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let size = lock(&view.shared)
                .picture
                .as_ref()
                .map(|p| (p.width, p.height));
            if let Some(size) = size {
                assert_eq!(size, (64, 48));
                break;
            }
            assert!(Instant::now() < deadline, "a picture within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        Message::End {
            why: "Lost the connection to Desk.".into(),
        }
        .write(&mut relay)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            lock(&view.shared).ended.as_deref(),
            Some("Lost the connection to Desk.")
        );
        assert!(
            lock(&view.shared).wanted.is_some(),
            "the window stays open, saying why, until closed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
