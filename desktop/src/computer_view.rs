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
//! I close the window" locks it as the window closes (PC8). "Lock the
//! pointer" (or Ctrl+Alt+Shift+M) holds this computer's pointer in the window
//! and sends the mouse's movement as relative moves, for games and 3D
//! ([`crate::view_pointer`], PC7). The bar's numbered screen buttons, laid
//! out as that computer's screens sit, switch the screen (or
//! Ctrl+Alt+Shift+1 to 9), and a screen's "Open in a new window" shows it
//! in a window of its own beside this one, up to four (PC11).

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
use crate::view_mic::{self, MicRun, MicSwitch};
use crate::view_pointer::{self, PointerLock, ReleaseChord};

/// The window `slot`'s id among the app's viewports: the first window's
/// as before PC11, the others' their own.
fn viewport_id(slot: u8) -> egui::ViewportId {
    if slot == 0 {
        egui::ViewportId::from_hash_of("pocketdesk-computer-view")
    } else {
        egui::ViewportId::from_hash_of(("pocketdesk-computer-view", slot))
    }
}

/// Repaint the window whose state is `shared`, its lock not held.
fn repaint(ctx: &egui::Context, shared: &Mutex<State>) {
    let slot = lock(shared).slot;
    ctx.request_repaint_of(viewport_id(slot));
}

/// While a window waits for the view it asked for (a switch, PC11), the
/// worker looks this often, not every [`POLL`]: the server names a view at
/// once, and the picture follows it.
const ASKED_POLL: Duration = Duration::from_millis(20);

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
/// A window whose own write stalled (the relay's reader held up by a slow
/// far link) ends its connection and joins again; those rejoins are counted
/// apart, at most this many in [`STALL_REJOIN_WINDOW`] (PR #563 re-review
/// N3), so a slow link costs some input but keeps the picture.
const STALL_REJOINS: usize = 20;
const STALL_REJOIN_WINDOW: Duration = Duration::from_secs(60);
/// How long a screenshot's or recording's line stays.
const SAVED_NOTE: Duration = Duration::from_secs(8);
/// How long a drop's last word stays (PC3).
const DROP_NOTE: Duration = Duration::from_secs(8);
/// How often a drop on its way looks at the sent list.
const DROP_LOOK: Duration = Duration::from_millis(500);
/// A way of decoding on the graphics card that gives no picture this long
/// after its first access unit is given up for the next (PC5).
const NO_PICTURE: Duration = Duration::from_secs(2);
/// The last way, the processor, gets longer (PR #562 re-review N2): a cold
/// ffmpeg or a large first picture is not taken for a decoder that cannot,
/// which would drop an HEVC view to H.264.
const LAST_NO_PICTURE: Duration = Duration::from_secs(6);
/// A view asked for before the HEVC probe has said waits this long for it,
/// off the UI thread (PR #562 re-review N1).
const HEVC_PROBE_WAIT: Duration = Duration::from_millis(1500);
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
    /// Which of the app's screen windows this is (PC11): 0 the first; its
    /// view is the server's for that window.
    slot: u8,
    /// Another window shows the same computer (PC11): this one's closing
    /// does not lock it.
    same_computer_open: bool,
    /// A screen this window's bar asked to show in a new window (PC11).
    open_new: Option<u32>,
    /// Switching to another screen (PC11): the screen and when it was
    /// asked; the last picture shows, dimmed, until the new one comes.
    switching: Option<(u32, Instant)>,
    /// How long the last switch took, from the ask to the new picture.
    last_switch: Option<Duration>,
    /// A recording going as the screen switched goes on in a new file.
    record_carry: bool,
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
    /// When this window last joined its view again (BK7 re-review N3), and
    /// when it did after its own write stalled (PR #563 re-review N3).
    rejoins: Vec<Instant>,
    stall_rejoins: Vec<Instant>,
    /// This connection was ended because a write to the relay stalled.
    stalled: bool,
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
    /// The ways that gave nothing for a codec, for as long as the app runs
    /// (PR #562 review M3): the next view does not wait on them again.
    failed_ways: std::collections::HashSet<(Codec, Accel)>,
    /// "Use HEVC when both can" as Settings has it (PC5), whether this
    /// view gave up on HEVC (every way gave nothing, PR #562 review I3),
    /// and whether the worker asks for it again without.
    hevc_setting: bool,
    hevc_refused: bool,
    redial_h264: bool,
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
    /// The pointer lock (PC7) and its release chord, and what the relay was
    /// last told of it on this connection (`None`: nothing yet).
    pointer: PointerLock,
    release_chord: ReleaseChord,
    pointer_said: Option<bool>,
    /// When "locked" was last said: it is said again each
    /// [`POINTER_RENEW`], the relay's lease (PR #563 review I1).
    pointer_said_at: Option<Instant>,
    /// "Send my mic" (PC10): the switch; the microphone going, while it
    /// does; its number, so a capture of an older start sends nothing;
    /// what the relay was last told of it on this connection; and why it
    /// last stopped by itself.
    mic: MicSwitch,
    mic_run: Option<MicRun>,
    mic_gen: u64,
    mic_said: Option<bool>,
    mic_note: Option<String>,
    /// Another window sends that computer this computer's mic (PC11): one
    /// window's at a time, so this one's is refused.
    mic_elsewhere: bool,
    /// The mic was on as this window switched screens (PC11): it starts
    /// again once the new screen's view controls that computer.
    mic_resume: bool,
    /// How the microphone is opened: [`view_mic::open`] unless a test
    /// gives a fake (no test opens a real microphone).
    mic_open: Option<MicOpener>,
}

/// Opens the microphone named (or the default), for "Send my mic".
type MicOpener =
    Arc<dyn Fn(Option<String>) -> io::Result<Box<dyn view_mic::MicSource>> + Send + Sync>;

/// How often a locked window says so again: well inside the relay's 3 s
/// lease, so one lost word never frees it there.
pub const POINTER_RENEW: Duration = Duration::from_secs(1);

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
/// one that never gave a picture, and ended or has had `limit`.
pub fn give_up(
    pictures: u64,
    ended: bool,
    first_fed: Option<Instant>,
    now: Instant,
    limit: Duration,
) -> bool {
    pictures == 0
        && (ended || first_fed.is_some_and(|at| now.saturating_duration_since(at) >= limit))
}

/// Whether `event` is Ctrl+Alt+Shift+S pressed: the stats' switch, kept
/// here (PR #551 review M8).
pub fn stats_chord(event: &egui::Event) -> bool {
    matches!(event, egui::Event::Key { pressed: true, .. })
        && view_pointer::chord_is(event, egui::Key::S)
}

/// `events` without the stats' switch, the pointer lock's (PC7) or the
/// screens' (PC11), pressed or let go: they are this window's, not the
/// other computer's. A pass with a screen's chord sends no text either.
pub fn keep_chord_here(events: &mut Vec<egui::Event>) {
    let screens = events.iter().any(|event| screen_chord(event).is_some());
    events.retain(|event| {
        !view_pointer::chord_is(event, egui::Key::S)
            && !view_pointer::is_lock_chord_key(event)
            && view_pointer::chord_key(event)
                .and_then(screen_key)
                .is_none()
            && !(screens && matches!(event, egui::Event::Text(_)))
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

/// The longest side of the bar's screen buttons together, and the least a
/// button is across, in points (PC11).
const SCREENS_HIGH: f32 = 26.0;
const SCREENS_WIDE: f32 = 220.0;
const SCREEN_LEAST: f32 = 16.0;
/// A layout whose buttons would be smaller than this across, or overlap
/// (mirrored screens), is shown as a row instead (PR #570 review M1).
const SCREEN_SMALLEST: f32 = 10.0;

/// The bar's screen buttons (PC11): one for each of `count` screens, laid
/// out as `layout` says they sit on that computer (their corners and sizes,
/// scaled to the bar, at most twice its height), or in a row when it does
/// not say where each is, or the buttons would overlap or be too small to
/// hit. Rectangles from (0, 0), in points.
pub fn screen_buttons(layout: &[wire::ScreenPlace], count: usize) -> Vec<egui::Rect> {
    let row = || {
        let wide = 24.0;
        (0..count)
            .map(|n| {
                egui::Rect::from_min_size(
                    egui::pos2(n as f32 * (wide + 2.0), 0.0),
                    egui::vec2(wide, 18.0),
                )
            })
            .collect()
    };
    if count == 0 {
        return Vec::new();
    }
    if layout.len() != count {
        return row();
    }
    let left = layout.iter().map(|p| p.x).min().unwrap_or(0) as f32;
    let top = layout.iter().map(|p| p.y).min().unwrap_or(0) as f32;
    let right = layout
        .iter()
        .map(|p| p.x as f32 + p.width as f32)
        .fold(left, f32::max);
    let bottom = layout
        .iter()
        .map(|p| p.y as f32 + p.height as f32)
        .fold(top, f32::max);
    let (wide, high) = ((right - left).max(1.0), (bottom - top).max(1.0));
    let least = layout
        .iter()
        .map(|p| (p.width.min(p.height) as f32).max(1.0))
        .fold(f32::MAX, f32::min);
    // As high as the bar, each button big enough to hit, and the lot no
    // wider than the bar can spare.
    let scale = (SCREENS_HIGH / high)
        .max(SCREEN_LEAST / least)
        .min(SCREENS_WIDE / wide)
        .min(2.0 * SCREENS_HIGH / high);
    let rects: Vec<egui::Rect> = layout
        .iter()
        .map(|p| {
            egui::Rect::from_min_size(
                egui::pos2((p.x as f32 - left) * scale, (p.y as f32 - top) * scale),
                egui::vec2(p.width as f32 * scale, p.height as f32 * scale),
            )
        })
        .collect();
    let small = rects
        .iter()
        .any(|r| r.width() < SCREEN_SMALLEST || r.height() < SCREEN_SMALLEST);
    let overlap = rects.iter().enumerate().any(|(n, a)| {
        rects[n + 1..].iter().any(|b| {
            let both = a.intersect(*b);
            both.width() > 0.5 && both.height() > 0.5
        })
    });
    if small || overlap { row() } else { rects }
}

/// The screen Ctrl+Alt+Shift+1 to 9 pressed asks for (PC11), from 0.
pub fn screen_chord(event: &egui::Event) -> Option<u32> {
    if !matches!(event, egui::Event::Key { pressed: true, .. }) {
        return None;
    }
    view_pointer::chord_key(event).and_then(screen_key)
}

/// The screen a number key names, from 0: 1 to 9.
fn screen_key(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9]
        .iter()
        .position(|k| *k == key)
        .map(|n| n as u32)
}

/// The stats' lines about this window among others (PC11): how many views
/// the server holds, of how many, and how long the last switch took.
pub fn window_lines(views: u32, last_switch: Option<Duration>) -> Vec<String> {
    let mut lines = Vec::new();
    if views > 0 {
        lines.push(format!(
            "Windows: {views} of {} shown",
            pocketdesk_core::local::view::MAX_VIEWS
        ));
    }
    if let Some(took) = last_switch {
        lines.push(format!("Last switch: {} ms", took.as_millis()));
    }
    lines
}

/// The words over a screen button (PC11): its name, its chord, and the
/// way to its own window.
pub fn screen_hint(name: &str, n: usize) -> String {
    if n < 9 {
        format!(
            "{name} (Ctrl+Alt+Shift+{}). Right-click to open it in a new window.",
            n + 1
        )
    } else {
        format!("{name}. Right-click to open it in a new window.")
    }
}

/// The bar's screen buttons (PC11), one a screen of `choices`, numbered and
/// laid out as `layout` says they sit, `current` highlighted: the screen
/// clicked, if one was; a screen's "Open in a new window" goes to
/// `open_new`.
fn screen_bar(
    ui: &mut egui::Ui,
    choices: &[String],
    layout: &[wire::ScreenPlace],
    current: u32,
    open_new: &mut Option<u32>,
) -> Option<u32> {
    let rects = screen_buttons(layout, choices.len());
    let size = rects
        .iter()
        .fold(egui::Vec2::ZERO, |size, rect| size.max(rect.max.to_vec2()));
    let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut picked = None;
    for (n, (rect, name)) in rects.iter().zip(choices).enumerate() {
        let rect = rect.translate(area.min.to_vec2()).shrink(1.0);
        let selected = n as u32 == current;
        let response = ui.interact(
            rect,
            ui.id().with(("computer-view-screen", n)),
            egui::Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                ui.is_enabled(),
                selected,
                format!("Screen {}", n + 1),
            )
        });
        let visuals = ui.visuals();
        let (fill, text) = if selected {
            (visuals.selection.bg_fill, visuals.selection.stroke.color)
        } else if response.hovered() {
            (
                visuals.widgets.hovered.weak_bg_fill,
                visuals.widgets.hovered.fg_stroke.color,
            )
        } else {
            (
                visuals.widgets.inactive.weak_bg_fill,
                visuals.widgets.inactive.fg_stroke.color,
            )
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            2.0,
            fill,
            visuals.widgets.inactive.bg_stroke,
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            (n + 1).to_string(),
            egui::FontId::proportional(12.0),
            text,
        );
        let response = response.on_hover_text(screen_hint(name, n));
        if response.clicked() {
            picked = Some(n as u32);
        }
        response.context_menu(|ui| {
            if ui.button("Show here").clicked() {
                picked = Some(n as u32);
                ui.close();
            }
            if ui.button("Open in a new window").clicked() {
                *open_new = Some(n as u32);
                ui.close();
            }
        });
    }
    picked
}

/// The window's title: the computer's name and the screen it shows (PC11,
/// from 0), and "Mic on" first while this computer's microphone goes there
/// (PR #569 review I2), so the taskbar says it too.
pub fn window_title(name: &str, screen: u32, mic_on: bool) -> String {
    let shown = format!("{name} \u{b7} screen {}", screen + 1);
    if mic_on {
        format!("{} \u{b7} {shown}", view_mic::ON_AIR)
    } else {
        shown
    }
}

/// Whether a window after the first, of a computer that runs Windows
/// (`windows`, as its session said) or not, opens View only (PR #570
/// review I4): only Windows maps a session's clicks onto the screen it
/// shows; elsewhere they land over its first screen.
pub fn extra_view_only(windows: Option<bool>) -> bool {
    windows != Some(true)
}

/// What a later window of such a computer says on its bar (review I4).
pub const EXTRA_INPUT_WORDS: &str =
    "This computer's other screens take input in the first window only.";

/// Where the microphone goes, as the main window names it (PC11): the
/// computer, and the window by the screen it shows.
pub fn mic_window_words(name: &str, screen: u32) -> String {
    format!("{name}, from the window of its screen {}", screen + 1)
}

/// The bar's "Mic on", at full colour even while the bar is greyed for a
/// locked pointer (PR #569 review M7): painted with its layer's own
/// painter, which the greyed scope does not fade.
fn on_air_label(ui: &mut egui::Ui) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(view_mic::ON_AIR.to_string(), font, ON_AIR_RED);
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    ui.ctx()
        .layer_painter(ui.layer_id())
        .galley(rect.min, galley, ON_AIR_RED);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, view_mic::ON_AIR));
    response
}

/// What the main window says, on every page, while a screen window sends
/// this computer's microphone (PR #569 review I2).
pub fn mic_on_air_words(name: &str) -> String {
    format!("Mic on: this computer's microphone goes to {name}.")
}

/// The bar's "Mic on" (PC10): red, as a recording light.
pub const ON_AIR_RED: egui::Color32 = egui::Color32::from_rgb(255, 92, 92);

/// What "Send my mic" says of itself on the bar (PC10).
pub fn mic_hover(name: &str) -> String {
    format!(
        "Send this computer's microphone to {name}, where it becomes the microphone \u{201c}Pocketdesk Mic\u{201d} (on Windows, the virtual cable's \u{201c}CABLE Output\u{201d}). {name} decides: its Devices page needs \u{201c}Use the phone as a microphone\u{201d} on for this computer. Settings \u{2192} Other computers picks the microphone here."
    )
}

/// The windows and their workers: the first, and up to three more that
/// show other screens beside it (PC11).
pub struct ComputerView {
    shared: Arc<Mutex<State>>,
    /// The windows after the first, made as first asked for, kept for
    /// the next.
    extras: Mutex<Vec<Arc<Mutex<State>>>>,
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

/// A window's state and its worker, which follows its view (PC11: one a
/// window).
fn new_window(ctx: &egui::Context, state_dir: &Path, slot: u8) -> Arc<Mutex<State>> {
    let shared = Arc::new(Mutex::new(State {
        slot,
        ..State::default()
    }));
    let worker = shared.clone();
    let ctx = ctx.clone();
    let watched = state_dir.to_path_buf();
    let name = if slot == 0 {
        "computer-view".to_string()
    } else {
        format!("computer-view-{slot}")
    };
    let spawned = std::thread::Builder::new()
        .name(name)
        .spawn(move || watch(&watched, &worker, &ctx));
    // Without the thread no screen is shown, and nothing else breaks.
    drop(spawned);
    shared
}

impl ComputerView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf, config: Option<PathBuf>) -> Self {
        let shared = new_window(ctx, &state_dir, 0);
        Self {
            shared,
            extras: Mutex::new(Vec::new()),
            state_dir,
            config,
            ctx: ctx.clone(),
        }
    }

    /// Every window, the first first.
    fn windows(&self) -> Vec<Arc<Mutex<State>>> {
        let extras = self
            .extras
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::iter::once(self.shared.clone())
            .chain(extras.iter().cloned())
            .collect()
    }

    /// Set something in every window, and repaint each.
    fn each(&self, set: impl Fn(&mut State)) {
        for window in self.windows() {
            let slot = {
                let mut state = lock(&window);
                set(&mut state);
                state.slot
            };
            self.ctx.request_repaint_of(viewport_id(slot));
        }
    }

    /// What every window saved from its bar since the last call: the last.
    fn take_each<T>(&self, take: impl Fn(&mut State) -> Option<T>) -> Option<T> {
        self.windows()
            .iter()
            .filter_map(|window| take(&mut lock(window)))
            .last()
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
            state.hevc_refused = false;
            state.quality = quality;
            state.view_only = view_only;
            state.stats_on = stats;
        }
        ask(&self.state_dir, &mut state, wanted)
    }

    /// Stats switched in Settings (PR #551 review): an open window follows.
    pub fn set_stats(&self, on: bool) {
        self.each(|state| state.stats_on = on);
    }

    /// "Use HEVC when both can" as Settings has it (PC5): the next view
    /// follows.
    pub fn set_hevc(&self, on: bool) {
        self.each(|state| state.hevc_setting = on);
    }

    /// "Decode on the graphics card" as Settings has it (PC5): the next
    /// view follows.
    pub fn set_hw_decode(&self, on: bool) {
        self.each(|state| {
            if state.hw_decode != on {
                // The ways are tried afresh (PR #562 re-review N3).
                state.failed_ways.clear();
            }
            state.hw_decode = on;
        });
    }

    /// The bar's Stats switch, saved since the last call (PC2): Settings
    /// follows it.
    pub fn take_stats_saved(&self) -> Option<bool> {
        self.take_each(|state| state.stats_saved.take())
    }

    /// "Send Win and Alt+Tab there" as Settings has it (PC4): an open
    /// window follows at once.
    #[cfg(windows)]
    pub fn set_grab_keys(&self, on: bool) {
        self.each(|state| state.grab.setting = on);
    }

    /// The bar's "Send Win and Alt+Tab there", saved since the last call
    /// (PC4): Settings follows it.
    #[cfg(windows)]
    pub fn take_grab_saved(&self) -> Option<bool> {
        self.take_each(|state| state.grab.saved.take())
    }

    /// "Lock it when I close the window" as Settings has it (PC8): an open
    /// window follows at once.
    pub fn set_lock_on_close(&self, on: bool) {
        self.each(|state| state.lock_on_close = on);
    }

    /// The bar's "Lock it when I close the window", saved since the last
    /// call (PC8): Settings follows it.
    pub fn take_lock_saved(&self) -> Option<bool> {
        self.take_each(|state| state.lock_saved.take())
    }

    /// The computer this computer's microphone goes to now, and from which
    /// window, if any (PR #569 review I2; PC11: any window's): the main
    /// window says so on every page.
    pub fn mic_on_air(&self) -> Option<String> {
        self.windows().iter().find_map(|window| {
            let state = lock(window);
            state
                .mic
                .on()
                .then(|| {
                    state
                        .wanted
                        .as_ref()
                        .map(|w| mic_window_words(&w.name, w.monitor))
                })
                .flatten()
        })
    }

    /// Stop sending the microphone, from the main window, in every window
    /// (PR #569 review I2, PC11).
    pub fn stop_mic(&self) {
        self.each(|state| {
            let sending = state.mic.on();
            state.mic.stopped();
            if sending {
                stop_mic(state);
            }
        });
    }

    /// Pocketdesk quits with windows open: each closes as it would by its
    /// own close button, the lock on close (PC8) asked first by the last
    /// window of each computer (PC11), each write held to [`QUIT_WRITE`] so
    /// a stalled relay keeps the quit well under a second.
    pub fn quit(&self) {
        let windows = self.windows();
        // Each later window that would lock its computer as it closes: the
        // last of them locks it (review I2).
        let lockers: Vec<Option<String>> = windows.iter().map(|w| would_lock(&lock(w))).collect();
        for (n, window) in windows.iter().enumerate() {
            let mut state = lock(window);
            let device = state.wanted.as_ref().map(|w| w.device_id.clone());
            let later: Vec<Option<String>> = lockers
                .iter()
                .enumerate()
                .map(|(m, l)| if m > n { l.clone() } else { None })
                .collect();
            state.same_computer_open = shown_elsewhere(device.as_deref(), &later, n);
            quit_view(&self.state_dir, &mut state);
        }
    }

    /// Call each pass: keeps each window open while a screen is wanted
    /// there, and opens what a window's bar asked for in a new one (PC11).
    pub fn show(&self, ctx: &egui::Context) {
        self.follow_windows(ctx);
        for window in self.windows() {
            self.show_window(ctx, window);
        }
    }

    /// What each window needs to know of the others (PC11): whether another
    /// shows its computer, or sends it the mic; and a screen a window's bar
    /// asked for opened in a new one.
    fn follow_windows(&self, ctx: &egui::Context) {
        let windows = self.windows();
        let devices: Vec<Option<String>> = windows
            .iter()
            .map(|w| lock(w).wanted.as_ref().map(|w| w.device_id.clone()))
            .collect();
        // The computers each window would lock as it closes (review I2).
        let lockers: Vec<Option<String>> = windows.iter().map(|w| would_lock(&lock(w))).collect();
        // The computers each window sends the mic to (PC11).
        let sending: Vec<Option<String>> = windows
            .iter()
            .zip(&devices)
            .map(|(w, device)| {
                let state = lock(w);
                (state.mic.on() || state.mic_resume)
                    .then(|| device.clone())
                    .flatten()
            })
            .collect();
        for (n, window) in windows.iter().enumerate() {
            let asked = {
                let mut state = lock(window);
                state.mic_elsewhere = devices[n].as_ref().is_some_and(|device| {
                    sending
                        .iter()
                        .enumerate()
                        .any(|(m, to)| m != n && to.as_ref() == Some(device))
                });
                state.same_computer_open = shown_elsewhere(devices[n].as_deref(), &lockers, n);
                state.open_new.take()
            };
            if let Some(monitor) = asked {
                self.open_new_window(ctx, window, monitor);
            }
        }
    }

    /// Show the screen `monitor` of the computer `source` shows in a window
    /// of its own (PC11): a window not in use, else a new one, at most
    /// [`wire::MAX_VIEWS`] in all; it starts with `source`'s choices.
    fn open_new_window(&self, ctx: &egui::Context, source: &Arc<Mutex<State>>, monitor: u32) {
        let target = {
            let mut extras = self
                .extras
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let free = extras.iter().find(|w| lock(w).wanted.is_none()).cloned();
            match free {
                Some(free) => Some(free),
                None if extras.len() + 1 < usize::from(wire::MAX_VIEWS) => {
                    let slot = extras.len() as u8 + 1;
                    let window = new_window(ctx, &self.state_dir, slot);
                    extras.push(window.clone());
                    Some(window)
                }
                None => None,
            }
        };
        let mut from = lock(source);
        let Some(wanted) = from.wanted.clone() else {
            return;
        };
        let Some(target) = target else {
            from.saved_note = Some((
                format!(
                    "At most {} screen windows at once: close one first.",
                    wire::MAX_VIEWS
                ),
                Instant::now(),
            ));
            return;
        };
        let mut state = lock(&target);
        state.quality = from.quality;
        state.view_only = from.view_only;
        state.stats_on = from.stats_on;
        state.hevc_setting = from.hevc_setting;
        state.hw_decode = from.hw_decode;
        state.lock_on_close = from.lock_on_close;
        // A computer that is not Windows takes a window's clicks over its
        // first screen whatever screen the window shows (PR #570 review
        // I4): another window of it opens View only, said on its bar.
        state.view_only = state.view_only || extra_view_only(from.status.windows);
        state.hevc_refused = false;
        #[cfg(windows)]
        {
            state.grab.setting = from.grab.setting;
        }
        drop(from);
        let next = Wanted { monitor, ..wanted };
        if let Err(e) = ask(&self.state_dir, &mut state, next) {
            state.ended = Some(e);
        }
        ctx.request_repaint_of(viewport_id(state.slot));
    }

    /// The window `shared`, while a screen is wanted there.
    fn show_window(&self, ctx: &egui::Context, shared: Arc<Mutex<State>>) {
        let (title, slot) = {
            let state = lock(&shared);
            let Some(wanted) = &state.wanted else { return };
            (
                window_title(&wanted.name, wanted.monitor, state.mic.on()),
                state.slot,
            )
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([320.0, 240.0]);
        let state_dir = self.state_dir.clone();
        let config = self.config.clone();
        ctx.show_viewport_deferred(viewport_id(slot), builder, move |ui, _class| {
            draw(&shared, &state_dir, config.as_deref(), ui);
        });
    }
}

/// Whether `device`, the computer the window `n` shows, is in `others`
/// (each window's computer where it counts, `None` where not) for a window
/// other than `n` (PC11).
pub fn shown_elsewhere(device: Option<&str>, others: &[Option<String>], n: usize) -> bool {
    device.is_some_and(|device| {
        others
            .iter()
            .enumerate()
            .any(|(m, other)| m != n && other.as_deref() == Some(device))
    })
}

/// The computer a window would lock as it closes, if any (PR #570 review
/// I2): one it shows, its view going and shown. A window ended, waiting to
/// be let in, reconnecting or switching would not, so it never keeps
/// another window's close from locking.
fn would_lock(state: &State) -> Option<String> {
    let going = state.ended.is_none()
        && state.status.phase == Phase::Showing
        && state.status.reconnecting == 0
        && state.switching.is_none();
    going
        .then(|| state.wanted.as_ref().map(|w| w.device_id.clone()))
        .flatten()
}

/// A write to the relay as Pocketdesk quits waits at most this long: the
/// close's few writes (the lock, the pointer, the focus, keys let go) stay
/// well under a second even when the relay's reader is held up.
const QUIT_WRITE: Duration = Duration::from_millis(100);

/// Each write to the relay from now waits at most [`QUIT_WRITE`]; set on
/// the writer itself, the handle the writes go through.
fn hold_writes_short(state: &State) {
    if let Some(writer) = &state.writer {
        let _ = writer.set_write_timeout(Some(QUIT_WRITE));
    }
}

/// [`ComputerView::quit`]: with a window open, its last words to the relay
/// (the lock when chosen, then the pointer let go), and the view stopped.
fn quit_view(state_dir: &Path, state: &mut State) {
    if state.wanted.is_none() {
        return;
    }
    hold_writes_short(state);
    let lock = say_closing(state);
    stop_view(state_dir, state, lock);
    state.wanted = None;
    state.switching = None;
    state.record_carry = false;
    // The keyboard grab comes out with the window (PC4).
    #[cfg(windows)]
    {
        state.grab.hook = None;
    }
}

/// Ask the server for `wanted`'s screen; a view shown now is let go first.
fn ask(state_dir: &Path, state: &mut State, wanted: Wanted) -> Result<(), String> {
    ask_with(state_dir, state, wanted, crate::video::hevc_known())
}

/// [`ask`], with the HEVC probe's answer as it stands (`known`).
fn ask_with(
    state_dir: &Path,
    state: &mut State,
    wanted: Wanted,
    known: Option<bool>,
) -> Result<(), String> {
    let could = offer_hevc(
        state.hevc_setting,
        state.hw_decode,
        true,
        state.hevc_refused,
    );
    let release = state.input.release_all();
    send(state, release);
    let slot = state.slot;
    match hevc_ask(could, known) {
        HevcAsk::Now(hevc) => {
            computers::request(state_dir, view_ask(&wanted, hevc, slot), now_unix())
                .map_err(|e| format!("Not asked: {e}"))?;
        }
        HevcAsk::Wait => {
            // The probe has not said yet: the ask waits for it on a thread
            // of its own, at most HEVC_PROBE_WAIT.
            let (dir, request_for) = (state_dir.to_path_buf(), wanted.clone());
            let waited = std::thread::Builder::new()
                .name("computer-view-ask".into())
                .spawn(move || {
                    let hevc = crate::video::wait_for_hevc_probe(HEVC_PROBE_WAIT);
                    if let Err(e) =
                        computers::request(&dir, view_ask(&request_for, hevc, slot), now_unix())
                    {
                        eprintln!("Pocketdesk: a computer's screen was not asked for: {e}");
                    }
                });
            if let Err(e) = waited {
                return Err(format!("Not asked: {e}"));
            }
        }
    }
    // The microphone stops with the view it went to (PR #569 re-review
    // N5): never a frame named for the next computer.
    let sending = state.mic.on();
    state.mic.stopped();
    if sending {
        stop_mic(state);
    }
    if let Some(stream) = state.stream.take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    // The old view's decoder may still give pictures (frames in flight, or
    // read already): they are dropped, never taken for the next screen's
    // (PR #570 review I1).
    state.decoder_gen += 1;
    state.picture = None;
    state.writer = None;
    state.newer_than = state.newer_than.max(state.session_id.unwrap_or(0));
    state.session_id = None;
    // The same computer's screens stay on the bar meanwhile (PC11), so its
    // buttons do not jump as it switches.
    let same = state
        .wanted
        .as_ref()
        .is_some_and(|w| w.device_id == wanted.device_id);
    let (monitors, layout) = if same {
        (
            std::mem::take(&mut state.status.monitors),
            std::mem::take(&mut state.status.layout),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    state.status = ViewStatus {
        monitor: wanted.monitor,
        monitors,
        layout,
        ..ViewStatus::default()
    };
    state.ended = None;
    state.note = None;
    state.asked_at = Some(Instant::now());
    state.wanted = Some(wanted);
    state.asked += 1;
    state.rejoins.clear();
    state.stall_rejoins.clear();
    Ok(())
}

/// Whether a view asks for HEVC (PC5, PR #562 review I3): with "Use HEVC
/// when both can" on, decoding on the graphics card on too (review M10: on
/// the processor HEVC decodes about twice as slowly as H.264), this app's
/// ffmpeg shown to decode it, and the view not given up on it.
pub fn offer_hevc(setting: bool, hw_decode: bool, decodes: bool, refused: bool) -> bool {
    setting && hw_decode && decodes && !refused
}

/// When a view's ask says whether it takes HEVC (PR #562 re-review N1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HevcAsk {
    /// Now, saying this.
    Now(bool),
    /// Once the probe has said, as it could take HEVC.
    Wait,
}

/// When and what an ask says of HEVC: at once when it `could` not take it
/// whatever the probe says, or the probe has said (`known`); else after
/// waiting for the probe.
pub fn hevc_ask(could: bool, known: Option<bool>) -> HevcAsk {
    match (could, known) {
        (false, _) => HevcAsk::Now(false),
        (true, Some(decodes)) => HevcAsk::Now(decodes),
        (true, None) => HevcAsk::Wait,
    }
}

/// The ask for `wanted`'s screen in the window `slot`, taking HEVC or not.
fn view_ask(wanted: &Wanted, hevc: bool, slot: u8) -> ComputerAsk {
    ComputerAsk::View {
        device_id: wanted.device_id.clone(),
        monitor: wanted.monitor,
        hevc,
        window: slot,
    }
}

/// Show another screen of the computer this window shows (PC11), keeping
/// the window's state: its pointer lock let go first (the relay told, the
/// system's grab freed on the next pass), its privacy shown as it was (the
/// server moves the hold with the window), a recording going on in a new
/// file, and the last picture shown dimmed until the new one comes.
fn switch_screen(state_dir: &Path, state: &mut State, monitor: u32) -> Result<(), String> {
    let Some(wanted) = state.wanted.clone() else {
        return Ok(());
    };
    if wanted.monitor == monitor && state.ended.is_none() {
        return Ok(());
    }
    if state.pointer.locked() || state.pointer_said == Some(true) {
        say_pointer(state, false);
    }
    state.pointer.free();
    // The mic goes on with the window (PC11): stopped here, its relay told,
    // and started again on the new screen's view once that controls.
    if state.mic.on() || state.mic.wanted() {
        let sending = state.mic.on();
        state.mic.stopped();
        if sending {
            stop_mic(state);
        }
        state.mic_resume = true;
    }
    let privacy = state.status.privacy.clone();
    state.record_carry = state.record_want;
    let going = state.ended.is_none();
    ask(state_dir, state, Wanted { monitor, ..wanted })?;
    if going {
        state.status.privacy = privacy;
    }
    state.switching = Some((monitor, Instant::now()));
    Ok(())
}

/// Whether a view whose last way just gave up (`step`) is asked for again
/// without HEVC: once, when the stream is HEVC (PR #562 review I3).
pub fn redial_without_hevc(step: Step, codec: Option<Codec>, refused: bool) -> bool {
    step == Step::Exhausted && codec == Some(Codec::Hevc) && !refused
}

/// Let go of what is held there, then end the view on the server: the
/// view this window shows, when it reached one (BK7 review M1). Its end is
/// no longer this window's to say. `lock`: the lock on close did not reach
/// the relay, so the server is asked to lock with the stop (PR #568 review
/// I2), and says so if it cannot.
fn stop_view(state_dir: &Path, state: &mut State, lock: bool) {
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
        ComputerAsk::StopView {
            session_id: shown,
            lock,
            window: state.slot,
        },
        now_unix(),
    );
}

/// Cancel, while the server dials the other computer again after a drop
/// (PC2): the view stops there, and the window says so, with Reconnect.
fn cancel_reconnect(state_dir: &Path, state: &mut State) {
    stop_view(state_dir, state, false);
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
/// ends the connection ([`lost_writer`]).
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
        if let Err(e) = Message::Input(bytes).write(writer) {
            lost_writer(state, &e);
            return;
        }
    }
}

/// Say the window's privacy ask to the server (PC8); a broken write ends
/// the connection, as [`send`]'s.
fn say_privacy(state: &mut State, ask: u8) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if let Err(e) = Message::Privacy(ask).write(writer) {
        lost_writer(state, &e);
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

/// The window's last words to the relay as it closes: the lock, when
/// chosen (PC8), on its own and first, then the pointer let go (PC7). A
/// free whose write stalls ends the connection, so it went second (PR #563
/// re-review): the lock never waits on it, and the connection's end lets go
/// of the pointer at the relay anyway. Whether the lock is still owed: it
/// was chosen but did not reach the relay (no connection, as between joins
/// after a stall, or its write failed), for the stop to carry (PR #568
/// review I2).
fn say_closing(state: &mut State) -> bool {
    let mut owed = false;
    // Another window still shows that computer (PC11): it is not locked
    // under it.
    let lock_it = state.lock_on_close && !state.same_computer_open;
    if let Some(ask) = closing_ask(lock_it, state.status.phase) {
        say_privacy(state, ask);
        owed = lock_owed(state.writer.is_some(), state.ended.is_some());
    }
    if state.pointer.locked() || state.pointer_said == Some(true) {
        say_pointer(state, false);
    }
    state.pointer = PointerLock::default();
    // The microphone stops with the window, and as Pocketdesk quits (PC10).
    let sending = state.mic.on();
    state.mic.stopped();
    if sending {
        stop_mic(state);
    }
    owed
}

/// Whether a lock on close asked is still owed after its write: the
/// connection is gone (never there, or the write failed and ended it), and
/// the view did not end meanwhile.
pub fn lock_owed(connected: bool, ended: bool) -> bool {
    !connected && !ended
}

/// Say the window's pointer lock to the server (PC7): only while it says
/// locked does the relay pass relative moves on. A broken write ends the
/// connection, as [`send`]'s.
fn say_pointer(state: &mut State, locked: bool) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if let Err(e) = Message::PointerLock(locked).write(writer) {
        lost_writer(state, &e);
        return;
    }
    state.pointer_said = Some(locked);
    state.pointer_said_at = Some(Instant::now());
}

/// Say the window's mic to the server (PC10): only while it says on does
/// the relay pass its frames on. A broken write ends the connection, as
/// [`send`]'s.
fn say_mic(state: &mut State, on: bool) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if let Err(e) = Message::Mic(on).write(writer) {
        lost_writer(state, &e);
        return;
    }
    state.mic_said = Some(on);
}

/// The microphone stops going: its capture ends, and the relay hears so
/// (it then sends the other computer the stop itself).
fn stop_mic(state: &mut State) {
    state.mic_run = None;
    state.mic_gen += 1;
    if state.mic_said == Some(true) {
        say_mic(state, false);
    }
}

/// Whether the window may send its microphone now (PR #569 review I2):
/// it controls that computer (Control on there, View only off, connected),
/// and the view neither ended nor reconnects. Checked by the capture on
/// each frame as well as by each pass, so a window that gets no passes
/// (minimised, hidden) still stops.
fn mic_may_go(state: &State) -> bool {
    state.ended.is_none()
        && state.status.reconnecting == 0
        && controls(
            state.status.control,
            state.view_only,
            state.writer.is_some(),
            true,
        )
}

/// Why the microphone stopped by itself, the same whether the capture or
/// the window's pass noticed first (PR #569 re-review N4): the
/// connection went, or the other computer took control back (Control off
/// there); nothing for what this window did itself (View only here, the
/// view ending or reconnecting, which say so themselves).
fn mic_stop_words(state: &State) -> Option<&'static str> {
    if state.writer.is_none() || state.mic_said != Some(true) {
        Some(view_mic::CUT_WORDS)
    } else if state.status.control != Some(true) {
        Some(view_mic::TAKEN_WORDS)
    } else {
        None
    }
}

/// What a window's mic does after it switched screens with the mic on
/// (PC11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// Not switching with it on.
    Nothing,
    /// The new screen's view is not shown yet: nothing yet.
    Wait,
    /// It controls that computer: the mic starts again.
    Start,
    /// It ended, or shows without taking input: the mic stays off, said.
    GiveUp,
}

/// [`Resume`] for a window `resume`-ing its mic, `controlling` that
/// computer or not, its view `ended` or not and standing at `phase`.
pub fn mic_resumes(resume: bool, controlling: bool, ended: bool, phase: Phase) -> Resume {
    match () {
        _ if !resume => Resume::Nothing,
        _ if controlling => Resume::Start,
        _ if ended || phase == Phase::Showing => Resume::GiveUp,
        _ => Resume::Wait,
    }
}

/// Why the mic stayed off after a switch (PC11).
pub const MIC_SWITCH_WORDS: &str = "Mic stopped: the next screen does not take this computer's input. Turn Send my mic on again once it does.";

/// Why a second window's mic is refused (PC11).
pub fn mic_elsewhere_words(name: &str) -> String {
    format!(
        "Another window already sends your microphone to {name}: it has one \u{201c}Pocketdesk Mic\u{201d}. Turn it off there first."
    )
}

/// "Send my mic", each pass (PC10): it starts as [`MicSwitch`] decides
/// (wanted, and the window controls that computer), the relay told before
/// any frame, and stops as it decides otherwise.
fn follow_mic(
    shared: &Arc<Mutex<State>>,
    state: &mut State,
    config: Option<&Path>,
    ctx: &egui::Context,
    controlling: bool,
) {
    let viewport = viewport_id(state.slot);
    // On, but this connection's relay never heard so (a rejoin, another
    // computer asked, a write lost): nothing of it is heard there, so it
    // stops and says why (PR #569 review I1).
    match mic_resumes(
        state.mic_resume,
        controlling,
        state.ended.is_some(),
        state.status.phase,
    ) {
        Resume::Wait => return,
        Resume::Start => {
            state.mic_resume = false;
            if !state.mic.wanted() {
                state.mic.toggle();
            }
        }
        Resume::GiveUp => {
            state.mic_resume = false;
            state.mic_note = Some(MIC_SWITCH_WORDS.to_string());
        }
        Resume::Nothing => {}
    }
    // One window's mic a computer (PC11): asked in a second window, it is
    // refused, saying why.
    if state.mic_elsewhere && state.mic.wanted() && !state.mic.on() {
        state.mic.stopped();
        let name = state
            .wanted
            .as_ref()
            .map_or_else(|| "that computer".to_string(), |w| w.name.clone());
        state.mic_note = Some(mic_elsewhere_words(&name));
        return;
    }
    if state.mic.on() && state.mic_said != Some(true) {
        state.mic.stopped();
        stop_mic(state);
        state.mic_note = Some(view_mic::CUT_WORDS.to_string());
    }
    match state.mic.step(controlling) {
        Some(view_mic::Change::Start) => {
            // The relay first: only then does it pass the frames on.
            say_mic(state, true);
            if state.mic_said != Some(true) {
                state.mic.stopped();
                return;
            }
            state.mic_gen += 1;
            let generation = state.mic_gen;
            let device = config
                .and_then(|path| Config::load_from(path).ok())
                .and_then(|c| c.computer_view_mic_device);
            let sending = shared.clone();
            let send = move |packet: InputPacket| {
                let mut state = lock(&sending);
                if state.mic_gen != generation
                    || !state.mic.on()
                    || state.mic_said != Some(true)
                    || !mic_may_go(&state)
                {
                    return false;
                }
                send(&mut state, vec![packet]);
                state.writer.is_some()
            };
            let ending = shared.clone();
            let repaint = ctx.clone();
            // A capture that ended by itself turns the switch off, whether
            // the microphone failed or the relay took no more (PR #569
            // review I1): the bar never says "Mic on" for nothing.
            let ended = move |how: view_mic::Ended| {
                let mut state = lock(&ending);
                if state.mic_gen == generation {
                    let note = match how {
                        view_mic::Ended::Failed(why) => Some(Some(view_mic::failed_words(&why))),
                        // Why, as the window's pass would say it (re-review
                        // N4), whichever notices first.
                        view_mic::Ended::Refused => {
                            Some(mic_stop_words(&state).map(str::to_string))
                        }
                        view_mic::Ended::Stopped => None,
                    };
                    if let Some(note) = note {
                        state.mic_note = note;
                        state.mic.stopped();
                        stop_mic(&mut state);
                    }
                }
                drop(state);
                // The window's own viewport, and the main window, which
                // says "Mic on" too (PR #569 review M6).
                repaint.request_repaint_of(viewport);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            };
            let opener = state.mic_open.clone();
            let open = move || match opener {
                Some(open) => open(device),
                None => view_mic::open(device),
            };
            match MicRun::start(open, send, ended) {
                Ok(run) => {
                    state.mic_run = Some(run);
                    state.mic_note = None;
                }
                Err(e) => {
                    state.mic_note = Some(view_mic::failed_words(&e.to_string()));
                    state.mic.stopped();
                    stop_mic(state);
                }
            }
        }
        Some(view_mic::Change::Stop) => {
            // Stopped by the view, not the bar: said as the capture would
            // say it (re-review N4).
            if !controlling && let Some(words) = mic_stop_words(state) {
                state.mic_note = Some(words.to_string());
            }
            stop_mic(state);
            // The bar, drawn before, stops saying "Mic on" at once.
            ctx.request_repaint_of(viewport_id(state.slot));
        }
        None => {}
    }
}

/// Whether the relay must be told the lock again: locked and not said so on
/// this connection or not for [`POINTER_RENEW`] (its lease), or freed after
/// it was said locked (PC7).
pub fn pointer_to_say(
    locked: bool,
    said: Option<bool>,
    said_at: Option<Instant>,
    now: Instant,
) -> bool {
    if locked {
        said != Some(true)
            || said_at.is_none_or(|at| now.saturating_duration_since(at) >= POINTER_RENEW)
    } else {
        said == Some(true)
    }
}

/// Say the window's focus to the server (PC3); a broken write ends the
/// connection, as [`send`]'s.
fn say_focus(state: &mut State, edge: Edge) {
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    if let Err(e) = Message::Focus(edge == Edge::Gain).write(writer) {
        lost_writer(state, &e);
    }
}

/// A write to the relay failed (it timed out, or the relay went): the
/// connection is ended, not left half open, so the relay reads its end and
/// lets go of what this window held there (its pointer lock, PC7 review
/// I1), and the worker joins the view again (PC2) with a fresh one. A write
/// that timed out is a stall, its rejoin counted apart ([`STALL_REJOINS`]).
fn lost_writer(state: &mut State, error: &io::Error) {
    state.stalled |= write_stalled(error.kind());
    state.writer = None;
    if let Some(stream) = state.stream.take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Whether a write that failed with `kind` timed out (the relay's reader
/// held up), not a relay gone: Windows says `TimedOut`, Unix `WouldBlock`.
pub fn write_stalled(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

/// Whether one more rejoin fits: fewer than `most` in `times` within
/// `within` of `now`, and if so it is counted.
pub fn may_rejoin(times: &mut Vec<Instant>, most: usize, within: Duration, now: Instant) -> bool {
    times.retain(|at| now.saturating_duration_since(*at) < within);
    let room = times.len() < most;
    if room {
        times.push(now);
    }
    room
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
        let lock = say_closing(&mut state);
        state.mic_note = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(
            egui::viewport::CursorGrab::None,
        ));
        stop_view(state_dir, &mut state, lock);
        state.wanted = None;
        // A switch under way ends with the window (PC11).
        state.switching = None;
        state.record_carry = false;
        state.fullscreen = false;
        // The GPU's picture goes with the window (PR #562 review M5): now,
        // if this pass is drawn, else as the window next opens.
        state.on_gpu = None;
        if state.gpu.holds() {
            ui.painter().add(state.gpu.free(ui.max_rect()));
        }
        // The keyboard grab comes out with the window (PC4).
        #[cfg(windows)]
        {
            state.grab.hook = None;
        }
        return;
    }
    if let Some(picture) = state.picture.take() {
        // The new screen's first picture ends a switch (PC11).
        if let Some((_, since)) = state.switching.take() {
            state.last_switch = Some(since.elapsed());
        }
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
    // No picture on the GPU now (the view ended, or RGBA instead): its GL
    // objects go (PR #562 review M5).
    if on_gpu.is_none() && gpu.holds() {
        ui.painter().add(gpu.free(ui.max_rect()));
    }
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
    let slot = state.slot;
    let switching = state.switching.map(|(to, _)| to);
    let last_switch = state.last_switch;
    let mut stats_on = state.stats_on;
    // Ctrl+Alt+Shift+S, wherever the focus is (PR #551 review M8).
    if ctx.input(|i| i.events.iter().any(stats_chord)) {
        stats_on = !stats_on;
        drop(state);
        switch_stats(shared, config, stats_on);
        state = lock(shared);
    }
    // PC7: Ctrl+Alt+Shift+M switches the pointer lock; Ctrl+Alt+Shift
    // pressed and let go alone frees it (PC4's release chord).
    let (events, held) = ctx.input(|i| (i.events.clone(), i.modifiers));
    if events.iter().any(view_pointer::lock_chord) {
        state.pointer.toggle();
    }
    if state.release_chord.observe(&events, held) {
        state.pointer.free();
    }
    let pointer_wanted = state.pointer.wanted();
    // While the pointer is locked the hidden pointer may rest on the bar
    // (Wayland keeps it where it was): its clicks are the other computer's,
    // never the bar's (PR #563 review I2). The chords free it.
    let pointer_locked = state.pointer.locked();
    #[cfg(windows)]
    let grab_setting = state.grab.setting;
    let lock_on_close = state.lock_on_close;
    let mic_wanted = state.mic.wanted();
    let mic_on = state.mic.on();
    let mic_note = state.mic_note.clone();
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
    let choices = screen_choices(&status);
    // PC11: Ctrl+Alt+Shift+1 to 9 shows that screen, wherever the focus is.
    if let Some(n) = events.iter().find_map(screen_chord)
        && (n as usize) < choices.len()
    {
        monitor = n;
    }
    let mut open_new: Option<u32> = None;
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
            ui.add_enabled_ui(!pointer_locked, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut said = wanted.name.clone();
                    if !status.route.is_empty() {
                        said.push_str(&format!(" \u{b7} {}", status.route));
                    }
                    ui.label(egui::RichText::new(said).strong());
                    if choices.len() > 1 {
                        // PC11: a button a screen, laid out as they sit.
                        let current = switching.unwrap_or(wanted.monitor);
                        if let Some(pick) =
                            screen_bar(ui, &choices, &status.layout, current, &mut open_new)
                        {
                            monitor = pick;
                        }
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
                        // PC7: the pointer held in the window, its movement sent.
                        let mut pointer_on = pointer_wanted;
                        let pointer = ui
                            .checkbox(&mut pointer_on, "Lock the pointer")
                            .on_hover_text(format!(
                                "Hold the pointer in this window and send {} how far the mouse moves, for games and 3D. Ctrl+Alt+Shift+M switches it; Ctrl+Alt+Shift alone, or leaving the window, frees it.",
                                wanted.name
                            ));
                        if pointer.clicked() {
                            pointer.surrender_focus();
                            lock(shared).pointer.toggle();
                        }
                        // The way out, said on the greyed bar for as long
                        // as it is locked (PR #563 re-review N5).
                        if pointer_locked {
                            ui.label(view_pointer::HINT);
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
                        // PC10: this computer's microphone, there.
                        if view_mic::available() {
                            let mut mic_switch = mic_wanted;
                            let mic = ui
                                .checkbox(&mut mic_switch, "Send my mic")
                                .on_hover_text(mic_hover(&wanted.name));
                            if mic.clicked() {
                                mic.surrender_focus();
                                lock(shared).mic.toggle();
                            }
                        }
                    }
                    if mic_on {
                        on_air_label(ui).on_hover_text(format!(
                            "This computer's microphone goes to {} now",
                            wanted.name
                        ));
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
                if ended.is_none() && slot != 0 && extra_view_only(status.windows) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(EXTRA_INPUT_WORDS).color(egui::Color32::LIGHT_GRAY),
                        )
                        .wrap(),
                    );
                }
                if let Some(words) = &mic_note {
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
        });

    // The toolbar's choices.
    let mut reconnect = false;
    let mut cancel = false;
    if let Some(n) = open_new {
        // The app opens it, on its next pass (PC11).
        lock(shared).open_new = Some(n);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    if monitor != wanted.monitor {
        let mut state = lock(shared);
        if let Err(e) = switch_screen(state_dir, &mut state, monitor) {
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
        // PC10: the microphone, while the window controls that computer.
        let controlling = ended.is_none()
            && status.reconnecting == 0
            && controls(status.control, view_only, connected, true);
        follow_mic(shared, &mut state, config, &ctx, controlling);
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
            } else if let (Some(to), Some([w, h])) = (switching, drawn) {
                // The last screen's picture, dimmed, until the next one's
                // comes (PC11); nothing goes there meanwhile.
                paint_picture(
                    ui,
                    &gpu,
                    on_gpu.is_some(),
                    texture.as_ref(),
                    letterbox(area, w, h),
                    egui::Color32::from_gray(90),
                );
                let to_name = choices
                    .get(to as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("Screen {}", to + 1));
                ui.vertical_centered(|ui| {
                    ui.add_space(area.height() / 3.0);
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(200))
                        .corner_radius(6.0)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::LIGHT_GRAY,
                                format!("Switching to {to_name}\u{2026}"),
                            );
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
                // Locked, a click is the other computer's alone.
                if (!pointer_locked && (response.clicked() || response.drag_started()))
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
            let going = ended.is_none();
            follow_pointer(
                &mut state,
                &ctx,
                picture_rect,
                area,
                &status,
                going,
                connected,
            );
            // Relative moves need no picture of the right size: a size
            // change while locked keeps the movement and what is held
            // (PR #563 review M6).
            let current = current || state.pointer.locked();
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
                let mut lines = stats_lines(&status, size, shown, &decoding);
                lines.extend(window_lines(status.views, last_switch));
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

/// The pointer lock, each pass (PC7): it locks or lets go as
/// [`PointerLock`] decides (a focus the system said, control, a picture),
/// the relay hears it before this pass's moves, the window's input turns
/// relative, and while locked the pointer is hidden and, confined, put back
/// in the picture's middle as it strays.
fn follow_pointer(
    state: &mut State,
    ctx: &egui::Context,
    picture: Option<egui::Rect>,
    area: egui::Rect,
    status: &ViewStatus,
    going: bool,
    connected: bool,
) {
    let now = Instant::now();
    let situation = view_pointer::Situation {
        // Only a focus the system said: unknown is not focused.
        focused: ctx.input(|i| i.viewport().focused == Some(true)),
        controls: going
            && status.reconnecting == 0
            && controls(status.control, state.view_only, connected, true),
        picture: picture.is_some(),
    };
    let mode = view_pointer::grab_here();
    let over = picture.unwrap_or(area);
    if let Some(change) = state.pointer.step(situation, now) {
        for command in view_pointer::commands(change, mode, over) {
            ctx.send_viewport_cmd(command);
        }
    }
    let locked = state.pointer.locked();
    if pointer_to_say(locked, state.pointer_said, state.pointer_said_at, now) {
        say_pointer(state, locked);
    }
    state.input.set_relative(locked);
    if !locked {
        return;
    }
    ctx.set_cursor_icon(egui::CursorIcon::None);
    // Said again in time for the relay's lease, mouse moving or not.
    ctx.request_repaint_after(POINTER_RENEW);
    // Re-centred whatever the mode: should Locked have failed (winit only
    // logs it), the confined pointer is kept in the picture still; a
    // locked one takes this only as where it shows on unlock.
    if let Some(back) = view_pointer::recentre(ctx.input(|i| i.pointer.latest_pos()), over) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(back));
    }
    if state.pointer.hint(now) {
        egui::Area::new(egui::Id::new("computer-view-pointer-hint"))
            .fixed_pos(egui::pos2(over.center().x - 140.0, over.top() + 16.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_black_alpha(210))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_gray(235), view_pointer::HINT);
                    });
            });
        ctx.request_repaint_after(view_pointer::HINT_FOR);
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
        state.grab.hook = INSTALL(ctx, viewport_id(state.slot));
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
        repaint(&ctx, &shared);
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
    let slot = lock(shared).slot;
    let viewport = viewport_id(slot);
    let mut wait = POLL;
    loop {
        std::thread::sleep(wait);
        wait = POLL;
        let (wanted, newer_than, asked) = {
            let mut state = lock(shared);
            let Some(wanted) = state.wanted.clone() else {
                continue;
            };
            if state.ended.is_some() {
                continue;
            }
            let file = wire::read_window(state_dir, slot)
                .filter(|f| f.device_id == wanted.device_id && f.session_id > state.newer_than);
            if file.is_none() {
                if state.asked_at.is_some_and(|at| at.elapsed() >= NO_ANSWER) {
                    state.ended = Some(
                        "The server did not answer: is Pocketdesk's server running on this computer?"
                            .to_string(),
                    );
                    ctx.request_repaint_of(viewport);
                } else if state.asked_at.is_some() {
                    // Asked (a switch, PC11): the server names it at once.
                    wait = ASKED_POLL;
                }
                continue;
            }
            (wanted, state.newer_than, state.asked)
        };
        let Some(file) = wire::read_window(state_dir, slot)
            .filter(|f| f.device_id == wanted.device_id && f.session_id > newer_than)
        else {
            continue;
        };
        let ended = view(&file, shared, ctx);
        // A switch (PC11): a recording goes on in a new file for the next
        // screen.
        let carry = std::mem::take(&mut lock(shared).record_carry);
        if carry {
            end_recording(shared, Some("it goes on in a new file for the next screen"));
            lock(shared).record_want = true;
        } else {
            end_recording(shared, Some("the screen stopped"));
        }
        let mut state = lock(shared);
        // The next view is looked for at once.
        wait = ASKED_POLL;
        // HEVC gave nothing here (PR #562 review I3): the same screen is
        // asked for again as H.264, once, if the window still wants it.
        if std::mem::take(&mut state.redial_h264)
            && state.asked == asked
            && state.wanted.as_ref() == Some(&wanted)
        {
            let release = state.input.release_all();
            send(&mut state, release);
            state.input = WindowInput::default();
            if let Err(e) = ask(state_dir, &mut state, wanted) {
                state.ended = Some(e);
            }
            ctx.request_repaint_of(viewport);
            continue;
        }
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
        let dropped = cut
            && ours
            && wire::read_window(state_dir, slot).is_some_and(|f| f.session_id == file.session_id);
        // At most this often, then the view ends (BK7 re-review N3): a
        // relay that drops the window each time is not joined for ever. A
        // cut this window made as its write stalled has its own, gentler
        // count (PR #563 re-review N3): a slow link degrades the input, not
        // the view.
        let stalled = std::mem::take(&mut state.stalled);
        let now = Instant::now();
        let rejoin = dropped
            && if stalled {
                may_rejoin(
                    &mut state.stall_rejoins,
                    STALL_REJOINS,
                    STALL_REJOIN_WINDOW,
                    now,
                )
            } else {
                may_rejoin(&mut state.rejoins, REJOINS, REJOIN_WINDOW, now)
            };
        if rejoin {
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
                _ if dropped && stalled => format!(
                    "The link to {} stayed too slow for this window's input. Reconnect to try again.",
                    wanted.name
                ),
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
        state.picture = None;
        // A switch keeps the last picture, shown dimmed until the next
        // screen's comes (PC11).
        if state.switching.is_none() || state.ended.is_some() {
            state.switching = None;
            state.texture = None;
            state.on_gpu = None;
            state.shown = None;
        }
        state.input = WindowInput::default();
        ctx.request_repaint_of(viewport);
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
        // A new connection's relay knows no lock yet (PC7), nor mic
        // (PC10), and has not stalled.
        state.pointer_said = None;
        state.mic_said = None;
        state.stalled = false;
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
            repaint(ctx, shared);
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
                ctx.request_repaint_of(viewport_id(state.slot));
            }
            Message::Config { width, height } => {
                // Another size: the decoder starts again at it, from the
                // next keyframe.
                if recorder.take().is_some() {
                    end_recording(shared, Some("it stopped as the screen changed size"));
                }
                stream_is = Some((Codec::H264, Some((width, height))));
                skip_failed(shared, &mut ladder, Codec::H264);
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
                skip_failed(shared, &mut ladder, Codec::Hevc);
                decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                gate = KeyGate::default();
                ladder.started();
            }
            Message::Frame {
                keyframe,
                seq,
                data,
            } => {
                let step = climb(
                    shared,
                    ctx,
                    file.session_id,
                    &mut ladder,
                    &mut decoder,
                    &mut gate,
                    stream_is,
                );
                if give_up_hevc(shared, step, stream_is) {
                    return Ok(String::new());
                }
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
                    let gave = d.pictures();
                    decoder = None;
                    let tried = ladder.way().clone();
                    let step = ladder.failed();
                    remember_failed(shared, stream_is, tried, step, gave);
                    if give_up_hevc(shared, step, stream_is) {
                        return Ok(String::new());
                    }
                    if step == Step::Next {
                        // The next way, from the next keyframe (PC5).
                        decoder = start_decoder(shared, ctx, stream_is, ladder.way());
                        gate = KeyGate::default();
                    } else {
                        lock(shared).note = Some(format!("The decoder stopped ({e})."));
                        repaint(ctx, shared);
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
                if give_up_hevc(shared, step, stream_is) {
                    return Ok(String::new());
                }
                // The next way starts from a keyframe, asked for now.
                if step == Step::Next {
                    Message::WantKeyframe.write(&mut asks)?;
                }
            }
            Message::WantKeyframe
            | Message::Input(_)
            | Message::Focus(_)
            | Message::Privacy(_)
            | Message::PointerLock(_)
            | Message::Mic(_) => {}
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
        let last = self.at + 1 >= self.ways.len();
        let limit = if last { LAST_NO_PICTURE } else { NO_PICTURE };
        if give_up(pictures, ended, self.first_fed, now, limit) {
            return self.next();
        }
        Step::Stay
    }

    /// Pass over the ways `failed` says gave nothing before, unless one is
    /// settled; never past the last.
    pub fn skip(&mut self, failed: impl Fn(&Accel) -> bool) {
        if self.settled {
            return;
        }
        while self.at + 1 < self.ways.len() && failed(&self.ways[self.at]) {
            self.at += 1;
            self.first_fed = None;
        }
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
    let tried = ladder.way().clone();
    let gave = d.pictures();
    let step = ladder.check(gave, d.ended(), Instant::now());
    remember_failed(shared, stream_is, tried, step, gave);
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

/// The last way gave nothing on HEVC (PC5, PR #562 review I3): the view
/// is asked for again without it, once, by the worker as this one ends.
fn give_up_hevc(
    shared: &Mutex<State>,
    step: Step,
    stream_is: Option<(Codec, Option<(u16, u16)>)>,
) -> bool {
    let mut state = lock(shared);
    let codec = stream_is.map(|(codec, _)| codec);
    if !redial_without_hevc(step, codec, state.hevc_refused) {
        return false;
    }
    state.hevc_refused = true;
    state.redial_h264 = true;
    true
}

/// A way that gave up (`step` from it) is remembered as failed for the
/// stream's codec (PR #562 review M3), only when it never gave a picture
/// (`gave`, re-review N3: one that worked and then stopped, a driver reset
/// or a crash, is tried again next time); the processor never is.
fn remember_failed(
    shared: &Mutex<State>,
    stream_is: Option<(Codec, Option<(u16, u16)>)>,
    tried: Accel,
    step: Step,
    gave: u64,
) {
    if matches!(step, Step::Next | Step::Exhausted)
        && gave == 0
        && tried != Accel::Software
        && let Some((codec, _)) = stream_is
    {
        lock(shared).failed_ways.insert((codec, tried));
    }
}

/// Before a decoder of `codec` starts, the ways that failed for it in this
/// app are passed over.
fn skip_failed(shared: &Mutex<State>, ladder: &mut Ladder, codec: Codec) {
    let state = lock(shared);
    ladder.skip(|way| state.failed_ways.contains(&(codec, way.clone())));
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
    ctx.request_repaint_of(viewport_id(state.slot));
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
        let slot = state.slot;
        drop(state);
        ctx.request_repaint_of(viewport_id(slot));
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

    /// PR #563 re-review: as the window closes, the lock goes first and on
    /// its own, so a free that stalls never keeps it from the relay.
    #[test]
    fn the_lock_on_close_goes_before_the_pointer_free() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let mut state = State {
            writer: Some(writer),
            lock_on_close: true,
            status: ViewStatus {
                phase: Phase::Showing,
                ..ViewStatus::default()
            },
            pointer_said: Some(true),
            ..State::default()
        };
        relay
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(!say_closing(&mut state), "the lock reached the relay");
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::Privacy(wire::PRIVACY_LOCK)
        );
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::PointerLock(false)
        );
        assert!(!state.pointer.locked());
    }

    /// PC8: Pocketdesk quitting with the window open locks the other
    /// computer as its close would, the lock first, each write held short,
    /// and stops the view; with no window open it does nothing.
    #[test]
    fn quitting_with_the_window_open_locks_it_and_stops_the_view() {
        let dir = std::env::temp_dir().join(format!("pd-computer-quit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let mut state = State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            session_id: Some(9),
            writer: Some(writer),
            lock_on_close: true,
            status: ViewStatus {
                phase: Phase::Showing,
                ..ViewStatus::default()
            },
            pointer_said: Some(true),
            ..State::default()
        };
        relay
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        quit_view(&dir, &mut state);
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::Privacy(wire::PRIVACY_LOCK)
        );
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::PointerLock(false)
        );
        assert!(state.wanted.is_none() && state.writer.is_none());
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::StopView {
                session_id: Some(9),
                lock: false,
                window: 0,
            }]
        );
        // Nothing open: nothing said, nothing asked.
        quit_view(&dir, &mut state);
        assert!(computers::take_all(&dir, now_unix()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #568 review I2: a window closed between joins (its stall ended the
    /// connection, the next join not made yet) still asks the lock, on the
    /// stop; one whose view ended, or without the option, does not.
    #[test]
    fn a_lock_on_close_with_no_connection_rides_the_stop() {
        assert!(lock_owed(false, false));
        assert!(!lock_owed(true, false), "it reached the relay");
        assert!(!lock_owed(false, true), "the view ended");
        let dir = std::env::temp_dir().join(format!("pd-computer-lockgap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let gap = |lock_on_close: bool, ended: Option<String>| State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            lock_on_close,
            ended,
            status: ViewStatus {
                phase: Phase::Showing,
                ..ViewStatus::default()
            },
            ..State::default()
        };
        let stops = |dir: &Path| {
            computers::take_all(dir, now_unix())
                .into_iter()
                .map(|r| r.ask)
                .collect::<Vec<_>>()
        };
        quit_view(&dir, &mut gap(true, None));
        assert_eq!(
            stops(&dir),
            [ComputerAsk::StopView {
                session_id: None,
                lock: true,
                window: 0,
            }]
        );
        quit_view(&dir, &mut gap(false, None));
        quit_view(&dir, &mut gap(true, Some("The view of Desk ended.".into())));
        assert_eq!(
            stops(&dir),
            [
                ComputerAsk::StopView {
                    session_id: None,
                    lock: false,
                    window: 0,
                },
                ComputerAsk::StopView {
                    session_id: None,
                    lock: false,
                    window: 0,
                }
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A writer to a relay whose reader is held up: its peer never reads and
    /// its send buffer is full, so the next write waits out its timeout, set
    /// here long (as the window's own 200 ms would be, made plain). The peer
    /// is kept open alongside.
    fn stalled_writer() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (peer, _) = listener.accept().unwrap();
        writer.set_nonblocking(true).unwrap();
        let chunk = vec![0u8; 64 * 1024];
        let mut full = false;
        for _ in 0..16_384 {
            match (&writer).write(&chunk) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    full = true;
                    break;
                }
                Err(e) => panic!("filling the buffer: {e}"),
            }
        }
        assert!(full, "the send buffer filled");
        writer.set_nonblocking(false).unwrap();
        writer
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        (writer, peer)
    }

    /// PR #568 review I1: quitting while the relay's reader is held up
    /// returns within about a second: quit holds each write short itself.
    #[test]
    fn quitting_while_the_relay_stalls_is_quick() {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-quit-stall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (writer, _peer) = stalled_writer();
        let mut state = State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            session_id: Some(9),
            writer: Some(writer),
            lock_on_close: true,
            status: ViewStatus {
                phase: Phase::Showing,
                ..ViewStatus::default()
            },
            pointer_said: Some(true),
            ..State::default()
        };
        let started = Instant::now();
        quit_view(&dir, &mut state);
        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "quit took {took:?}");
        assert!(state.wanted.is_none() && state.writer.is_none());
        // The lock's write failed: the stop carries it (PR #568 review I2).
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::StopView {
                session_id: Some(9),
                lock: true,
                window: 0,
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let two = NO_PICTURE;
        assert!(!give_up(0, false, None, at(9_000), two), "not fed yet");
        assert!(!give_up(0, false, Some(t), at(1_999), two));
        assert!(give_up(0, false, Some(t), at(2_000), two));
        assert!(give_up(0, true, None, t, two), "ffmpeg ended at once");
        assert!(
            !give_up(1, true, Some(t), at(9_000), two),
            "it gave a picture"
        );
        assert!(!give_up(5, false, Some(t), at(9_000), two));
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
        // The processor, last, is given 6 s, not 2 (re-review N2).
        assert_eq!(ladder.check(0, false, at(8_000)), Step::Stay);
        assert_eq!(ladder.check(1, false, at(8_010)), Step::Settled);
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

    /// PR #562 review M3: a way that failed for a codec is remembered for
    /// the app's life and passed over for that codec, never the processor,
    /// never a settled way, and not for the other codec.
    #[test]
    fn a_way_that_failed_is_passed_over_next_time() {
        let shared = Mutex::new(State::default());
        let hevc = Some((Codec::Hevc, None));
        remember_failed(&shared, hevc, Accel::D3d11va, Step::Next, 0);
        remember_failed(&shared, hevc, Accel::Auto, Step::Stay, 0);
        remember_failed(&shared, hevc, Accel::Software, Step::Exhausted, 0);
        remember_failed(&shared, None, Accel::Auto, Step::Next, 0);
        // A way that worked, then stopped (re-review N3), is not.
        remember_failed(&shared, hevc, Accel::Auto, Step::Next, 120);
        assert_eq!(
            lock(&shared).failed_ways,
            [(Codec::Hevc, Accel::D3d11va)].into_iter().collect()
        );
        let ways = vec![Accel::D3d11va, Accel::Auto, Accel::Software];
        let mut ladder = Ladder::new(ways.clone(), None);
        skip_failed(&shared, &mut ladder, Codec::Hevc);
        assert_eq!(ladder.way(), &Accel::Auto);
        let mut ladder = Ladder::new(ways.clone(), None);
        skip_failed(&shared, &mut ladder, Codec::H264);
        assert_eq!(ladder.way(), &Accel::D3d11va, "H.264's own");
        let mut settled = Ladder::new(ways.clone(), Some(&Accel::D3d11va));
        skip_failed(&shared, &mut settled, Codec::Hevc);
        assert_eq!(settled.way(), &Accel::D3d11va, "settled, kept");
        let mut all = Ladder::new(ways, None);
        all.skip(|_| true);
        assert_eq!(all.way(), &Accel::Software, "never past the last");
        // Turning decoding on the card off or on tries every way afresh
        // (re-review N3).
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join(format!("pd-pc5-n3-{}", std::process::id()));
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.set_hw_decode(true);
        lock(&view.shared)
            .failed_ways
            .insert((Codec::H264, Accel::D3d11va));
        view.set_hw_decode(true);
        assert_eq!(lock(&view.shared).failed_ways.len(), 1, "unchanged, kept");
        view.set_hw_decode(false);
        assert!(lock(&view.shared).failed_ways.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #562 review I3: a view asks for HEVC only with the setting on,
    /// this app's ffmpeg decoding it and the view not given up on it; it
    /// is given up once, when the last way gives nothing on HEVC.
    #[test]
    fn hevc_is_asked_for_only_when_this_app_decodes_it_and_given_up_once() {
        assert!(offer_hevc(true, true, true, false));
        assert!(!offer_hevc(false, true, true, false), "the setting off");
        assert!(
            !offer_hevc(true, false, true, false),
            "no graphics card decode (review M10)"
        );
        assert!(!offer_hevc(true, true, false, false), "this ffmpeg cannot");
        assert!(
            !offer_hevc(true, true, true, true),
            "given up for this view"
        );
        let hevc = Some(Codec::Hevc);
        assert!(redial_without_hevc(Step::Exhausted, hevc, false));
        assert!(!redial_without_hevc(Step::Exhausted, hevc, true), "once");
        assert!(!redial_without_hevc(Step::Next, hevc, false), "a way left");
        assert!(!redial_without_hevc(
            Step::Exhausted,
            Some(Codec::H264),
            false
        ));
        let shared = Mutex::new(State::default());
        assert!(give_up_hevc(
            &shared,
            Step::Exhausted,
            Some((Codec::Hevc, None))
        ));
        assert!(lock(&shared).redial_h264 && lock(&shared).hevc_refused);
        assert!(
            !give_up_hevc(&shared, Step::Exhausted, Some((Codec::Hevc, None))),
            "not twice"
        );
    }

    /// PR #562 re-review N1: an ask made after the probe said this app
    /// decodes HEVC says hevc: true; one made before it waits; one that
    /// could not take HEVC says false at once.
    #[test]
    fn an_ask_after_the_probe_says_hevc() {
        assert_eq!(hevc_ask(true, Some(true)), HevcAsk::Now(true));
        assert_eq!(hevc_ask(true, Some(false)), HevcAsk::Now(false));
        assert_eq!(hevc_ask(true, None), HevcAsk::Wait);
        assert_eq!(hevc_ask(false, None), HevcAsk::Now(false), "no wait");
        let dir = std::env::temp_dir().join(format!("pd-pc5-n1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State {
            hevc_setting: true,
            hw_decode: true,
            ..State::default()
        };
        let wanted = Wanted {
            device_id: "desk".into(),
            name: "Desk".into(),
            monitor: 0,
        };
        ask_with(&dir, &mut state, wanted, Some(true)).unwrap();
        let asked = computers::take_all(&dir, now_unix());
        assert!(
            matches!(
                asked.as_slice(),
                [r] if r.ask == ComputerAsk::View { device_id: "desk".into(), monitor: 0, hevc: true, window: 0 }
            ),
            "{asked:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #562 review I3: the ask says HEVC as the window offers it.
    #[test]
    fn the_view_ask_says_whether_it_takes_hevc() {
        let dir = std::env::temp_dir().join(format!("pd-pc5-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State {
            hevc_setting: true,
            hw_decode: true,
            hevc_refused: true,
            ..State::default()
        };
        let wanted = Wanted {
            device_id: "desk".into(),
            name: "Desk".into(),
            monitor: 0,
        };
        ask(&dir, &mut state, wanted).unwrap();
        let asked = computers::take_all(&dir, now_unix());
        assert!(
            matches!(
                asked.as_slice(),
                [r] if r.ask == ComputerAsk::View { device_id: "desk".into(), monitor: 0, hevc: false, window: 0 }
            ),
            "{asked:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
            // Two slow pictures in thirty-one: the median keeps to the
            // rest, the 95th percentile (the 30th by nearest rank) shows
            // them (PC5; one alone would sit above it, review M8).
            let slow = n == 7 || n == 20;
            clock.decoded(ms(n * 33 + if slow { 40 } else { 5 }), 100);
        }
        clock.fed(ms(1000));
        clock.decoded(ms(1006), 400);
        let shown = clock.shown(ms(1006));
        assert_eq!(shown.fps, 30);
        assert_eq!(shown.decode.map(|d| d.as_millis()), Some(5));
        assert_eq!(shown.decode_p95.map(|d| d.as_millis()), Some(40));
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
                session_id: Some(41),
                lock: false,
                window: 0,
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
        // PC7: the pointer lock's switch stays here too.
        let mut events = vec![
            key(egui::Key::M, true, all),
            key(egui::Key::M, false, all),
            key(egui::Key::M, true, egui::Modifiers::NONE),
        ];
        keep_chord_here(&mut events);
        assert_eq!(events.len(), 1, "only the plain M goes on");
    }

    /// PR #563 review I2: while the pointer is locked the bar is off, so a
    /// click where its "Lock the pointer" sits (where Wayland's locked
    /// pointer rests after the bar locked it) goes to the other computer
    /// alone and leaves the lock on. Nothing is grabbed: a test window
    /// sends its viewport commands nowhere.
    #[test]
    fn a_click_on_the_bar_while_locked_is_the_other_computers() {
        use egui_kittest::kittest::Queryable;
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("pd-computer-lockbar-{}", std::process::id()));
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let (w, h) = (64usize, 36usize);
        let shared = Arc::new(Mutex::new(State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            status: ViewStatus {
                phase: Phase::Showing,
                control: Some(true),
                ..ViewStatus::default()
            },
            writer: Some(writer),
            picture: Some(Picture {
                width: w,
                height: h,
                planes: Planes::Rgba,
                data: vec![0; w * h * 4],
                colour: Default::default(),
            }),
            size: Some((w as u16, h as u16)),
            ..State::default()
        }));
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        harness.step();
        let switch = harness.get_by_label("Lock the pointer").rect().center();
        let all = egui::Modifiers {
            alt: true,
            ctrl: true,
            shift: true,
            mac_cmd: false,
            command: true,
        };
        harness.key_press_modifiers(all, egui::Key::M);
        harness.step();
        harness.step();
        assert!(lock(&shared).pointer.locked(), "the chord locked it");
        // The greyed bar says the way out (re-review N5), beside the
        // picture's own hint of the first seconds.
        let said = harness.query_all_by_label(view_pointer::HINT).count();
        assert_eq!(said, 2, "the bar and the picture say how to free it");
        // The picture's size changing (a new size said, its picture not
        // come yet) leaves locked input going (PR #563 review M6).
        lock(&shared).size = Some((w as u16 + 2, h as u16));
        harness.hover_at(switch);
        harness.drag_at(switch);
        harness.step();
        harness.drop_at(switch);
        harness.step();
        harness.step();
        assert!(
            lock(&shared).pointer.locked(),
            "the bar's switch did nothing"
        );
        // PR #563 review M9: the chord frees it, and closing the window
        // frees a lock taken again.
        harness.key_press_modifiers(all, egui::Key::M);
        harness.step();
        assert!(!lock(&shared).pointer.locked(), "the chord freed it");
        harness.key_press_modifiers(all, egui::Key::M);
        harness.step();
        assert!(lock(&shared).pointer.locked());
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        harness.step();
        assert!(!lock(&shared).pointer.locked(), "closing freed it");
        // The relay heard each, in order: the lock before the click, both
        // halves of the click, then each release (the closing one before
        // the connection's end).
        drop(harness);
        drop(shared);
        relay
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut heard = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = relay.read(&mut buf) {
            if n == 0 {
                break;
            }
            heard.extend_from_slice(&buf[..n]);
        }
        let mut reader = io::Cursor::new(heard);
        let mut said: Vec<&str> = Vec::new();
        while let Ok(message) = Message::read(&mut reader) {
            let word = match message {
                Message::PointerLock(true) => "lock",
                Message::PointerLock(false) => "free",
                Message::Input(bytes) => match InputPacket::decode(&bytes) {
                    Ok(InputPacket::MouseClick { pressed: true, .. }) => "down",
                    Ok(InputPacket::MouseClick { pressed: false, .. }) => "up",
                    _ => continue,
                },
                _ => continue,
            };
            // A lock said again each second (the relay's lease) is the same
            // lock, wherever it falls: only a lock after a free is a new one
            // (PR #563 re-review N6).
            let renewal = word == "lock"
                && said.iter().rev().find(|w| matches!(**w, "lock" | "free")) == Some(&"lock");
            if !renewal {
                said.push(word);
            }
        }
        assert_eq!(said, ["lock", "down", "up", "free", "lock", "free"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A window shown and controlled, its relay a socket here, and its
    /// microphone `open` (never a real one).
    fn mic_window(
        name: &str,
        open: MicOpener,
    ) -> (Arc<Mutex<State>>, std::net::TcpStream, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-mic-{name}-{}", std::process::id()));
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (relay, _) = listener.accept().unwrap();
        let (w, h) = (64usize, 36usize);
        let shared = Arc::new(Mutex::new(State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            status: ViewStatus {
                phase: Phase::Showing,
                control: Some(true),
                ..ViewStatus::default()
            },
            writer: Some(writer),
            picture: Some(Picture {
                width: w,
                height: h,
                planes: Planes::Rgba,
                data: vec![0; w * h * 4],
                colour: Default::default(),
            }),
            size: Some((w as u16, h as u16)),
            mic_open: Some(open),
            ..State::default()
        }));
        (shared, relay, dir)
    }

    /// PC10: "Send my mic" tells the relay before any frame, sends the
    /// start and then frames from the (fake) microphone, says "Mic on" on
    /// the bar, and stops as View only goes on, telling the relay; turned
    /// on again it stops as the window closes. No real microphone opens.
    #[test]
    fn send_my_mic_tells_the_relay_first_and_stops_with_view_only_and_close() {
        use egui_kittest::kittest::Queryable;
        use std::io::Read;
        use std::sync::atomic::{AtomicUsize, Ordering};
        // Half a frame a read, each read counted.
        struct Fake(Arc<AtomicUsize>);
        impl view_mic::MicSource for Fake {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                std::thread::sleep(Duration::from_millis(5));
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(vec![100; view_mic::FRAME_SAMPLES / 2])
            }
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let (shared, mut relay, dir) = mic_window(
            "mic",
            Arc::new(move |device| {
                assert_eq!(device, None, "no config: the default");
                Ok(Box::new(Fake(counted.clone())) as Box<dyn view_mic::MicSource>)
            }),
        );
        // A third read starts only after the first frame went (one more
        // for a read of the last run still finishing).
        let a_frame_went = |from: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while reads.load(Ordering::SeqCst) < from + 4 {
                assert!(Instant::now() < deadline, "TIMEOUT: no frame");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness.step();
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 0);
        harness.get_by_label("Send my mic").click();
        harness.step();
        harness.step();
        assert!(lock(&shared).mic.on());
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 1);
        a_frame_went(0);
        harness.get_by_label("View only").click();
        harness.step();
        harness.step();
        assert!(!lock(&shared).mic.on(), "View only stopped it");
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 0);
        // On again (View only off first), then the window closes.
        harness.get_by_label("View only").click();
        harness.step();
        harness.get_by_label("Send my mic").click();
        harness.step();
        harness.step();
        assert!(lock(&shared).mic.on(), "on again");
        let before = reads.load(Ordering::SeqCst);
        a_frame_went(before);
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        harness.step();
        assert!(!lock(&shared).mic.on(), "closing stopped it");
        drop(harness);
        drop(shared);
        relay
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut heard = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = relay.read(&mut buf) {
            if n == 0 {
                break;
            }
            heard.extend_from_slice(&buf[..n]);
        }
        let mut reader = io::Cursor::new(heard);
        let mut said: Vec<&str> = Vec::new();
        while let Ok(message) = Message::read(&mut reader) {
            let word = match message {
                Message::Mic(true) => "on",
                Message::Mic(false) => "off",
                Message::Input(bytes) => match InputPacket::decode(&bytes) {
                    Ok(InputPacket::Mic {
                        enabled: true,
                        opus,
                    }) if opus.is_empty() => "start",
                    Ok(InputPacket::Mic { enabled: true, .. }) => "frame",
                    Ok(InputPacket::Mic { enabled: false, .. }) => "stop",
                    _ => continue,
                },
                _ => continue,
            };
            // Frames in a row are one "frame".
            if word == "frame" && said.last() == Some(&"frame") {
                continue;
            }
            said.push(word);
        }
        assert_eq!(
            said,
            ["on", "start", "frame", "off", "on", "start", "frame", "off"],
            "the relay hears on before any frame, and off as it stops"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC10: a microphone that cannot open says why on the bar, and the
    /// switch goes off.
    #[test]
    fn a_microphone_that_cannot_open_says_why_and_goes_off() {
        use egui_kittest::kittest::Queryable;
        let (shared, _relay, dir) = mic_window(
            "nomic",
            Arc::new(|_| Err(io::Error::other("no microphone"))),
        );
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness.step();
        harness.get_by_label("Send my mic").click();
        harness.step();
        let words = view_mic::failed_words("no microphone");
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.query_all_by_label(&words).count() == 0 {
            assert!(Instant::now() < deadline, "TIMEOUT: the bar never said why");
            std::thread::sleep(Duration::from_millis(10));
            harness.step();
        }
        harness.step();
        assert!(!lock(&shared).mic.wanted(), "the switch went off");
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #569 review I1: the window joins its view again (a new
    /// connection, whose relay never heard "on"): the mic stops, the bar
    /// says why and no longer "Mic on", and the new relay hears nothing of
    /// the mic.
    #[test]
    fn a_rejoin_stops_the_mic_and_says_so() {
        use egui_kittest::kittest::Queryable;
        use std::io::Read;
        struct Quiet;
        impl view_mic::MicSource for Quiet {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                std::thread::sleep(Duration::from_millis(5));
                Ok(Vec::new())
            }
        }
        let (shared, _relay, dir) = mic_window(
            "mic-rejoin",
            Arc::new(|_| Ok(Box::new(Quiet) as Box<dyn view_mic::MicSource>)),
        );
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness.step();
        harness.get_by_label("Send my mic").click();
        harness.step();
        harness.step();
        assert!(lock(&shared).mic.on());
        // A new connection, as `view` makes one.
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        {
            let mut state = lock(&shared);
            state.writer = Some(writer);
            state.pointer_said = None;
            state.mic_said = None;
        }
        harness.step();
        harness.step();
        assert!(!lock(&shared).mic.on(), "stopped");
        assert!(!lock(&shared).mic.wanted(), "and off");
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 0);
        assert_eq!(harness.query_all_by_label(view_mic::CUT_WORDS).count(), 1);
        drop(harness);
        drop(shared);
        relay
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut heard = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = relay.read(&mut buf) {
            if n == 0 {
                break;
            }
            heard.extend_from_slice(&buf[..n]);
        }
        let mut reader = io::Cursor::new(heard);
        while let Ok(message) = Message::read(&mut reader) {
            assert!(!matches!(message, Message::Mic(_)), "{message:?}");
            if let Message::Input(bytes) = message {
                assert!(
                    !matches!(InputPacket::decode(&bytes), Ok(InputPacket::Mic { .. })),
                    "no mic on the new connection"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A microphone that gives a frame every 10 ms, its reads counted.
    struct Counted(Arc<std::sync::atomic::AtomicUsize>);
    impl view_mic::MicSource for Counted {
        fn read(&mut self) -> io::Result<Vec<i16>> {
            std::thread::sleep(Duration::from_millis(5));
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![100; view_mic::FRAME_SAMPLES / 2])
        }
    }

    /// Every message the relay heard, as words, until the socket ends.
    fn heard_mic(relay: &mut std::net::TcpStream) -> Vec<&'static str> {
        use std::io::Read;
        relay
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut heard = Vec::new();
        let mut buf = [0u8; 65536];
        while let Ok(n) = relay.read(&mut buf) {
            if n == 0 {
                break;
            }
            heard.extend_from_slice(&buf[..n]);
        }
        let mut reader = io::Cursor::new(heard);
        let mut said = Vec::new();
        while let Ok(message) = Message::read(&mut reader) {
            let word = match message {
                Message::Mic(true) => "on",
                Message::Mic(false) => "off",
                Message::Input(bytes) => match InputPacket::decode(&bytes) {
                    Ok(InputPacket::Mic {
                        enabled: true,
                        opus,
                    }) if opus.is_empty() => "start",
                    Ok(InputPacket::Mic { enabled: true, .. }) => "frame",
                    _ => continue,
                },
                _ => continue,
            };
            if word == "frame" && said.last() == Some(&"frame") {
                continue;
            }
            said.push(word);
        }
        said
    }

    /// PR #569 review I2: control lost (Control off there, the view
    /// reconnecting or ending, View only) stops the microphone from the
    /// capture itself, with no pass of the window drawn: it stops sending,
    /// the switch goes off, and the relay hears off.
    #[test]
    fn the_mic_stops_with_no_pass_drawn_as_control_goes() {
        use egui_kittest::kittest::Queryable;
        use std::sync::atomic::{AtomicUsize, Ordering};
        type Lose = fn(&mut State);
        let ways: [(&str, Lose); 4] = [
            ("control off there", |s| s.status.control = Some(false)),
            ("reconnecting", |s| s.status.reconnecting = 1),
            ("ended", |s| s.ended = Some("gone".into())),
            ("view only", |s| s.view_only = true),
        ];
        for (way, lose) in ways {
            let reads = Arc::new(AtomicUsize::new(0));
            let counted = reads.clone();
            let (shared, mut relay, dir) = mic_window(
                "nodraw",
                Arc::new(move |_| {
                    Ok(Box::new(Counted(counted.clone())) as Box<dyn view_mic::MicSource>)
                }),
            );
            let mut harness = egui_kittest::Harness::new_ui({
                let (shared, dir) = (shared.clone(), dir.clone());
                move |ui| draw(&shared, &dir, None, ui)
            });
            harness.step();
            harness.get_by_label("Send my mic").click();
            harness.step();
            harness.step();
            let deadline = Instant::now() + Duration::from_secs(10);
            while reads.load(Ordering::SeqCst) < 4 {
                assert!(Instant::now() < deadline, "TIMEOUT: {way}: no frame");
                std::thread::sleep(Duration::from_millis(5));
            }
            // Control goes; no pass is drawn from here.
            lose(&mut lock(&shared));
            let deadline = Instant::now() + Duration::from_secs(10);
            while lock(&shared).mic.on() {
                assert!(Instant::now() < deadline, "TIMEOUT: {way}: still on");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(!lock(&shared).mic.wanted(), "{way}");
            // Re-review N4: the same words as the pass would give.
            let expected = (way == "control off there").then(|| view_mic::TAKEN_WORDS.to_string());
            assert_eq!(lock(&shared).mic_note, expected, "{way}");
            assert_eq!(
                window_title("Desk", 0, lock(&shared).mic.on()),
                "Desk \u{b7} screen 1"
            );
            drop(harness);
            drop(shared);
            assert_eq!(
                heard_mic(&mut relay),
                ["on", "start", "frame", "off"],
                "{way}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// PR #569 review M9: with a microphone that gives nothing yet, the
    /// window's own pass stops it as the view reconnects or Control goes
    /// off there.
    #[test]
    fn a_pass_stops_the_mic_as_the_view_reconnects_or_control_goes() {
        use egui_kittest::kittest::Queryable;
        struct Silent;
        impl view_mic::MicSource for Silent {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                std::thread::sleep(Duration::from_millis(5));
                Ok(Vec::new())
            }
        }
        type Lose = fn(&mut State);
        let ways: [(&str, Lose); 2] = [
            ("reconnecting", |s| s.status.reconnecting = 2),
            ("control off there", |s| s.status.control = Some(false)),
        ];
        for (way, lose) in ways {
            let (shared, mut relay, dir) = mic_window(
                "pass",
                Arc::new(|_| Ok(Box::new(Silent) as Box<dyn view_mic::MicSource>)),
            );
            let mut harness = egui_kittest::Harness::new_ui({
                let (shared, dir) = (shared.clone(), dir.clone());
                move |ui| draw(&shared, &dir, None, ui)
            });
            harness.step();
            harness.get_by_label("Send my mic").click();
            harness.step();
            harness.step();
            assert!(lock(&shared).mic.on(), "{way}");
            lose(&mut lock(&shared));
            harness.step();
            assert!(!lock(&shared).mic.on(), "{way}: the pass stopped it");
            // Re-review N4: the same words as the capture would give.
            let expected = (way == "control off there").then(|| view_mic::TAKEN_WORDS.to_string());
            assert_eq!(lock(&shared).mic_note, expected, "{way}");
            // The bar drawn before it stopped says so until the next pass.
            harness.step();
            assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 0);
            drop(harness);
            drop(shared);
            // The start may or may not have gone before it stopped.
            let mut heard = heard_mic(&mut relay);
            heard.retain(|w| *w != "start");
            assert_eq!(heard, ["on", "off"], "{way}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// PR #569 review I2: "Mic on" is in the window's title and said to
    /// the main window, and the main window's Stop stops it.
    #[test]
    fn the_title_and_the_main_window_say_mic_on() {
        use egui_kittest::kittest::Queryable;
        assert_eq!(window_title("Desk", 0, false), "Desk \u{b7} screen 1");
        assert_eq!(
            window_title("Desk", 1, true),
            "Mic on \u{b7} Desk \u{b7} screen 2"
        );
        assert!(mic_on_air_words("Desk").contains("goes to Desk"));
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = reads.clone();
        let (shared, mut relay, dir) = mic_window(
            "onair",
            Arc::new(move |_| {
                Ok(Box::new(Counted(counted.clone())) as Box<dyn view_mic::MicSource>)
            }),
        );
        let ctx = egui::Context::default();
        let view = ComputerView {
            shared: shared.clone(),
            extras: Mutex::new(Vec::new()),
            state_dir: dir.clone(),
            config: None,
            ctx: ctx.clone(),
        };
        assert_eq!(view.mic_on_air(), None);
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness.step();
        harness.get_by_label("Send my mic").click();
        harness.step();
        harness.step();
        assert_eq!(view.mic_on_air(), Some(mic_window_words("Desk", 0)));
        view.stop_mic();
        assert_eq!(view.mic_on_air(), None);
        assert!(!lock(&shared).mic.wanted());
        // Re-review T2: the relay heard the main window's Stop.
        drop(harness);
        drop(view);
        drop(shared);
        let mut heard = heard_mic(&mut relay);
        heard.retain(|w| !matches!(*w, "start" | "frame"));
        assert_eq!(heard, ["on", "off"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #569 re-review N5: asking for another computer's screen stops
    /// the microphone at once, the relay hearing off.
    #[test]
    fn asking_for_another_screen_stops_the_mic() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let dir = std::env::temp_dir().join(format!("pd-computer-mic-ask-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut state = State {
            writer: Some(writer.try_clone().unwrap()),
            stream: Some(writer),
            mic_said: Some(true),
            ..State::default()
        };
        state.mic.toggle();
        assert_eq!(state.mic.step(true), Some(view_mic::Change::Start));
        let other = Wanted {
            device_id: "lab".into(),
            name: "Lab".into(),
            monitor: 0,
        };
        let _ = ask_with(&dir, &mut state, other, Some(false));
        assert!(!state.mic.on() && !state.mic.wanted());
        assert_eq!(heard_mic(&mut relay), ["off"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #569 review M7: "Mic on" stays on the bar while the pointer is
    /// locked and the bar greyed.
    #[test]
    fn mic_on_stays_on_the_greyed_bar_while_the_pointer_is_locked() {
        use egui_kittest::kittest::Queryable;
        struct Silent;
        impl view_mic::MicSource for Silent {
            fn read(&mut self) -> io::Result<Vec<i16>> {
                std::thread::sleep(Duration::from_millis(5));
                Ok(Vec::new())
            }
        }
        let (shared, _relay, dir) = mic_window(
            "locked",
            Arc::new(|_| Ok(Box::new(Silent) as Box<dyn view_mic::MicSource>)),
        );
        let mut harness = egui_kittest::Harness::new_ui({
            let (shared, dir) = (shared.clone(), dir.clone());
            move |ui| draw(&shared, &dir, None, ui)
        });
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(true);
        harness.step();
        harness.get_by_label("Send my mic").click();
        harness.step();
        harness.step();
        let all = egui::Modifiers {
            alt: true,
            ctrl: true,
            shift: true,
            mac_cmd: false,
            command: true,
        };
        harness.key_press_modifiers(all, egui::Key::M);
        harness.step();
        harness.step();
        assert!(lock(&shared).pointer.locked(), "locked");
        assert!(lock(&shared).mic.on(), "the mic goes on");
        assert_eq!(harness.query_all_by_label(view_mic::ON_AIR).count(), 1);
        // Re-review T1: painted red at full opacity, not faded with the bar.
        fn texts<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
            match shape {
                egui::Shape::Text(text) => out.push(text),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &harness.output().shapes {
            texts(&clipped.shape, &mut found);
        }
        let on_air: Vec<_> = found
            .into_iter()
            .filter(|t| t.galley.text() == view_mic::ON_AIR)
            .collect();
        assert_eq!(on_air.len(), 1, "painted once");
        let text = on_air[0];
        assert_eq!(text.opacity_factor, 1.0, "not faded");
        assert_eq!(text.fallback_color, ON_AIR_RED);
        assert!(text.override_text_color.is_none_or(|c| c == ON_AIR_RED));
        assert!(
            text.galley
                .job
                .sections
                .iter()
                .all(|section| section.format.color == ON_AIR_RED),
            "its glyphs are red"
        );
        // The bar's other words, beside it, are faded while it is locked.
        let mut all = Vec::new();
        for clipped in &harness.output().shapes {
            texts(&clipped.shape, &mut all);
        }
        assert!(
            all.iter().any(
                |t| t.galley.text() == "Send my mic" && t.opacity_factor < 1.0
                    || t.galley.text() == "Send my mic"
                        && t.galley
                            .job
                            .sections
                            .iter()
                            .all(|s| s.format.color.a() < 255)
            ),
            "the greyed bar is greyed (the check above is not vacuous)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_mic_switch_names_the_computer_and_its_permission() {
        let said = mic_hover("Desk");
        assert!(said.contains("to Desk"));
        assert!(said.contains("Use the phone as a microphone"));
        assert!(said.contains("Pocketdesk Mic"));
    }

    /// PR #563 review I1: a write to the relay that fails ends the
    /// connection, so the relay reads its end and lets go of the window's
    /// lock, and the worker joins again.
    #[test]
    fn a_failed_write_ends_the_connection() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let reading = TcpStream::connect(addr).unwrap();
        let (mut relay_side, _) = listener.accept().unwrap();
        // The writer is a connection whose writes fail.
        let broken = TcpStream::connect(addr).unwrap();
        let _other = listener.accept().unwrap();
        broken.shutdown(std::net::Shutdown::Write).unwrap();
        let mut state = State {
            stream: Some(reading),
            writer: Some(broken),
            ..State::default()
        };
        say_focus(&mut state, Edge::Gain);
        assert!(state.writer.is_none() && state.stream.is_none());
        relay_side
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0u8; 1];
        assert_eq!(
            relay_side.read(&mut byte).unwrap(),
            0,
            "the relay reads its end"
        );
    }

    /// PR #563 re-review N3: a write that timed out is a stall, its rejoin
    /// counted apart; a relay gone is not.
    #[test]
    fn a_stalled_write_is_told_from_a_relay_gone() {
        assert!(write_stalled(io::ErrorKind::TimedOut), "Windows");
        assert!(write_stalled(io::ErrorKind::WouldBlock), "Unix");
        for kind in [
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
        ] {
            assert!(!write_stalled(kind), "{kind:?}");
        }
        let mut state = State::default();
        lost_writer(&mut state, &io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(!state.stalled);
        lost_writer(&mut state, &io::Error::from(io::ErrorKind::TimedOut));
        assert!(state.stalled);
        // A later broken write does not take the stall back.
        lost_writer(&mut state, &io::Error::from(io::ErrorKind::BrokenPipe));
        assert!(state.stalled);
    }

    /// PR #563 re-review N3: the stall rejoins' count is gentler than the
    /// plain one, and both forget rejoins past their window.
    #[test]
    fn rejoins_are_counted_within_their_window() {
        let t = Instant::now();
        let mut plain = Vec::new();
        for i in 0..REJOINS {
            assert!(may_rejoin(&mut plain, REJOINS, REJOIN_WINDOW, t), "{i}");
        }
        assert!(!may_rejoin(&mut plain, REJOINS, REJOIN_WINDOW, t));
        assert_eq!(plain.len(), REJOINS, "a refusal is not counted");
        assert!(may_rejoin(
            &mut plain,
            REJOINS,
            REJOIN_WINDOW,
            t + REJOIN_WINDOW
        ));
        // Three stalls in a few seconds still rejoin, many times over.
        let mut stalls = Vec::new();
        for i in 0..STALL_REJOINS {
            let at = t + Duration::from_millis(500) * i as u32;
            assert!(
                may_rejoin(&mut stalls, STALL_REJOINS, STALL_REJOIN_WINDOW, at),
                "{i}"
            );
        }
        const { assert!(STALL_REJOINS > REJOINS * 3) };
        assert!(STALL_REJOIN_WINDOW > REJOIN_WINDOW);
        // A link that never comes back still ends the view in the end.
        let at = t + Duration::from_secs(11);
        assert!(!may_rejoin(
            &mut stalls,
            STALL_REJOINS,
            STALL_REJOIN_WINDOW,
            at
        ));
    }

    /// PC7: the relay hears the lock once per connection and again each
    /// second, and the release only after it heard the lock.
    #[test]
    fn the_relay_is_told_the_lock_once_a_connection() {
        let t = Instant::now();
        let just = Some(t);
        assert!(pointer_to_say(true, None, None, t), "a new connection");
        assert!(pointer_to_say(true, Some(false), just, t));
        assert!(!pointer_to_say(true, Some(true), just, t), "said already");
        assert!(pointer_to_say(false, Some(true), just, t));
        assert!(!pointer_to_say(false, Some(false), just, t));
        assert!(!pointer_to_say(false, None, None, t), "never said locked");
        // PR #563 review I1: said again each second, for the relay's lease.
        let later = t + POINTER_RENEW;
        assert!(!pointer_to_say(
            true,
            Some(true),
            just,
            t + POINTER_RENEW / 2
        ));
        assert!(pointer_to_say(true, Some(true), just, later));
        assert!(!pointer_to_say(false, Some(false), just, later));
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
                monitor: 0,
                hevc: false,
                window: 0,
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

    /// PC11: the screen buttons sit as the screens do: side by side, one
    /// higher, scaled to the bar; a row when the layout is not known.
    #[test]
    fn the_screen_buttons_sit_as_the_screens_do() {
        let place = |x, y, width, height| wire::ScreenPlace {
            x,
            y,
            width,
            height,
        };
        let layout = [place(0, 0, 1920, 1080), place(1920, -200, 2560, 1440)];
        let rects = screen_buttons(&layout, 2);
        assert_eq!(rects.len(), 2);
        let (a, b) = (rects[0], rects[1]);
        assert!(a.right() <= b.left() + 0.01, "the second to the right");
        assert!(b.top() < a.top(), "the second higher, as it sits");
        assert!(a.left().abs() < 0.01 && b.top().abs() < 0.01, "from (0, 0)");
        let bottom = rects.iter().map(|r| r.bottom()).fold(0.0, f32::max);
        assert!(
            (bottom - SCREENS_HIGH).abs() < 0.5,
            "as high as the bar: {bottom}"
        );
        assert!(
            (a.width() / a.height() - 1920.0 / 1080.0).abs() < 0.01,
            "each its shape"
        );
        // Two stacked: each still big enough to hit.
        let stacked = [place(0, 0, 1920, 1080), place(0, 1080, 1920, 1080)];
        for rect in screen_buttons(&stacked, 2) {
            assert!(rect.height() >= SCREEN_LEAST - 0.01, "{rect:?}");
        }
        // Many wide ones: no wider than the bar spares.
        let wide: Vec<_> = (0..6).map(|n| place(n * 3840, 0, 3840, 1080)).collect();
        let right = screen_buttons(&wide, 6)
            .iter()
            .map(|r| r.right())
            .fold(0.0, f32::max);
        assert!(right <= SCREENS_WIDE + 0.5, "{right}");
        // PR #570 review M1: mirrored screens overlap, a tiny one beside a
        // huge one cannot be hit: a row; four stacked stay at most twice
        // the bar's height.
        let is_row =
            |rects: &[egui::Rect]| rects.iter().all(|r| r.top() == 0.0 && r.height() == 18.0);
        let mirrored = [place(0, 0, 1920, 1080), place(0, 0, 1920, 1080)];
        assert!(is_row(&screen_buttons(&mirrored, 2)));
        let tiny = [place(0, 0, 7680, 4320), place(7680, 0, 320, 240)];
        assert!(is_row(&screen_buttons(&tiny, 2)));
        let tall: Vec<_> = (0..4).map(|n| place(0, n * 1080, 1920, 1080)).collect();
        let tall = screen_buttons(&tall, 4);
        assert!(!is_row(&tall));
        let bottom = tall.iter().map(|r| r.bottom()).fold(0.0, f32::max);
        assert!(bottom <= 2.0 * SCREENS_HIGH + 0.5, "{bottom}");
        // Not known (a computer before PC11), or not for every screen: a row.
        let row = screen_buttons(&[], 3);
        assert_eq!(row.len(), 3);
        assert!(row.windows(2).all(|p| p[0].right() < p[1].left()));
        assert!(row.iter().all(|r| r.top() == 0.0));
        assert_eq!(screen_buttons(&layout[..1], 2).len(), 2);
        assert!(screen_buttons(&layout, 0).is_empty());
    }

    /// PC11: Ctrl+Alt+Shift+1 to 9 names a screen, kept here with what it
    /// would type; nothing else does.
    #[test]
    fn ctrl_alt_shift_and_a_number_picks_a_screen_here() {
        // As egui-winit says a key: what it types, and where it sits (PR
        // #570 review I3: with Shift, the 1 key types "!").
        let typed = |key, physical, pressed, modifiers| egui::Event::Key {
            key,
            physical_key: Some(physical),
            pressed,
            repeat: false,
            modifiers,
        };
        let key = |key, pressed, modifiers| typed(key, key, pressed, modifiers);
        let all = egui::Modifiers {
            ctrl: true,
            alt: true,
            shift: true,
            ..egui::Modifiers::NONE
        };
        assert_eq!(screen_chord(&key(egui::Key::Num1, true, all)), Some(0));
        assert_eq!(screen_chord(&key(egui::Key::Num9, true, all)), Some(8));
        assert_eq!(screen_chord(&key(egui::Key::Num2, false, all)), None);
        assert_eq!(screen_chord(&key(egui::Key::Num0, true, all)), None);
        let ctrl_alt = egui::Modifiers {
            shift: false,
            ..all
        };
        assert_eq!(screen_chord(&key(egui::Key::Num2, true, ctrl_alt)), None);
        let bang = typed(egui::Key::Exclamationmark, egui::Key::Num1, true, all);
        assert_eq!(screen_chord(&bang), Some(0), "Shift+1 types !");
        let slash = typed(egui::Key::Slash, egui::Key::Num7, true, all);
        assert_eq!(screen_chord(&slash), Some(6), "a German Shift+7");
        let mut kept = vec![bang.clone(), slash];
        keep_chord_here(&mut kept);
        assert!(kept.is_empty(), "kept here, never sent");
        // The stats' and the pointer's chords, where a layout moves S or M.
        let s = typed(egui::Key::O, egui::Key::S, true, all);
        assert!(stats_chord(&s));
        let m = typed(egui::Key::Comma, egui::Key::M, true, all);
        assert!(view_pointer::lock_chord(&m));
        let mut events = vec![
            key(egui::Key::Num2, true, all),
            egui::Event::Text("@".into()),
            key(egui::Key::Num2, false, all),
            key(egui::Key::A, true, egui::Modifiers::NONE),
        ];
        keep_chord_here(&mut events);
        assert_eq!(events, [key(egui::Key::A, true, egui::Modifiers::NONE)]);
        // Typing on its own still goes.
        let mut typed = vec![egui::Event::Text("2".into())];
        keep_chord_here(&mut typed);
        assert_eq!(typed.len(), 1);
    }

    /// PC11: the stats count the windows and time the last switch; a
    /// screen's button says its chord and the way to its own window.
    #[test]
    fn the_stats_count_the_windows_and_time_the_switch() {
        assert!(window_lines(0, None).is_empty());
        assert_eq!(
            window_lines(2, Some(Duration::from_millis(840))),
            ["Windows: 2 of 4 shown", "Last switch: 840 ms"]
        );
        assert_eq!(
            screen_hint("eDP-1 (1920x1080)", 0),
            "eDP-1 (1920x1080) (Ctrl+Alt+Shift+1). Right-click to open it in a new window."
        );
        assert!(!screen_hint("Screen 10", 9).contains("Ctrl"));
        // PR #570 review I4: later windows of a computer not Windows (or
        // not said) open View only.
        assert!(!extra_view_only(Some(true)));
        assert!(extra_view_only(Some(false)));
        assert!(extra_view_only(None));
    }

    /// PC11: a computer two windows show is told apart from one shown once.
    #[test]
    fn a_computer_shown_in_another_window_is_seen() {
        let devices = [Some("desk".to_string()), None, Some("desk".to_string())];
        let at = |n: usize| devices[n].as_deref();
        assert!(shown_elsewhere(at(0), &devices, 0));
        assert!(shown_elsewhere(at(2), &devices, 2));
        assert!(!shown_elsewhere(at(1), &devices, 1));
        let once = [Some("desk".to_string()), Some("lap".to_string())];
        assert!(!shown_elsewhere(once[0].as_deref(), &once, 0));
        // PR #570 review I2: a window that would not lock (ended, not
        // shown) keeps no other's close from locking.
        let showing = |device: &str| State {
            wanted: Some(Wanted {
                device_id: device.into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            status: ViewStatus {
                phase: Phase::Showing,
                ..ViewStatus::default()
            },
            ..State::default()
        };
        assert_eq!(would_lock(&showing("desk")).as_deref(), Some("desk"));
        let ended = State {
            ended: Some("Desk refused it.".into()),
            ..showing("desk")
        };
        assert_eq!(would_lock(&ended), None);
        let waiting = State {
            status: ViewStatus::default(),
            ..showing("desk")
        };
        assert_eq!(would_lock(&waiting), None);
    }

    /// PC11: switching screens keeps the window's state: the pointer let go
    /// (the relay told first), privacy shown as it was, a recording carried
    /// to the next screen, the screens kept on the bar, and the ask for the
    /// window's own view.
    #[test]
    fn a_switch_keeps_the_windows_state() {
        let dir = std::env::temp_dir().join(format!("pd-computer-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        relay
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut state = State {
            slot: 2,
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            session_id: Some(9),
            writer: Some(writer),
            status: ViewStatus {
                phase: Phase::Showing,
                control: Some(true),
                privacy: ViewPrivacy::On,
                monitors: vec!["A".into(), "B".into()],
                ..ViewStatus::default()
            },
            pointer_said: Some(true),
            record_want: true,
            ..State::default()
        };
        state.pointer.toggle();
        switch_screen(&dir, &mut state, 1).unwrap();
        assert_eq!(
            Message::read(&mut relay).unwrap(),
            Message::PointerLock(false),
            "the relay hears the pointer let go"
        );
        assert!(!state.pointer.wanted(), "the lock let go here");
        assert_eq!(state.status.privacy, ViewPrivacy::On);
        assert_eq!(state.status.monitors, ["A", "B"], "the screens stay");
        assert_eq!(state.status.monitor, 1);
        assert!(state.record_carry && state.record_want);
        assert_eq!(state.switching.map(|(to, _)| to), Some(1));
        assert!(state.writer.is_none() && state.session_id.is_none());
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::View {
                device_id: "desk".into(),
                monitor: 1,
                hevc: false,
                window: 2,
            }]
        );
        // The same screen again asks nothing.
        switch_screen(&dir, &mut state, 1).unwrap();
        assert!(computers::take_all(&dir, now_unix()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC11: "Open in a new window" shows the screen in a window of its
    /// own, with the first one's choices, its own view asked; its close
    /// stops only its view; at most four windows, a fifth said; and on quit
    /// only the last window of a computer locks it.
    #[test]
    fn a_screen_opens_in_a_window_of_its_own() {
        let dir = std::env::temp_dir().join(format!("pd-computer-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 3, true, true).unwrap();
        computers::take_all(&dir, now_unix());
        view.open_new_window(&ctx, &view.shared, 1);
        let windows = view.windows();
        assert_eq!(windows.len(), 2);
        {
            let second = lock(&windows[1]);
            assert_eq!(second.slot, 1);
            assert_eq!(second.wanted.as_ref().map(|w| w.monitor), Some(1));
            assert_eq!(
                (second.quality, second.view_only, second.stats_on),
                (3, true, true),
                "the first window's choices"
            );
        }
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::View {
                device_id: "desk".into(),
                monitor: 1,
                hevc: false,
                window: 1,
            }]
        );
        // Its close stops its own view alone.
        stop_view(&dir, &mut lock(&windows[1]), false);
        lock(&windows[1]).wanted = None;
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::StopView {
                session_id: None,
                lock: false,
                window: 1,
            }]
        );
        assert!(lock(&view.shared).wanted.is_some(), "the first stays");
        // A window not in use is used again; then up to four in all.
        for monitor in [1, 0, 1] {
            view.open_new_window(&ctx, &view.shared, monitor);
        }
        assert_eq!(view.windows().len(), 4);
        assert_eq!(lock(&view.windows()[1]).slot, 1, "used again");
        computers::take_all(&dir, now_unix());
        view.open_new_window(&ctx, &view.shared, 0);
        assert_eq!(view.windows().len(), 4, "no fifth");
        assert!(computers::take_all(&dir, now_unix()).is_empty());
        assert!(
            lock(&view.shared)
                .saved_note
                .as_ref()
                .is_some_and(|(note, _)| note.starts_with("At most 4")),
            "said"
        );
        // Quit with the lock on close chosen: only the last window of the
        // computer asks it, so it never locks under another.
        for window in view.windows() {
            let mut state = lock(&window);
            state.lock_on_close = true;
            state.status.phase = Phase::Showing;
        }
        view.quit();
        let locks: Vec<(u8, bool)> = computers::take_all(&dir, now_unix())
            .into_iter()
            .filter_map(|r| match r.ask {
                ComputerAsk::StopView { lock, window, .. } => Some((window, lock)),
                _ => None,
            })
            .collect();
        let mut locks = locks;
        locks.sort();
        assert_eq!(locks, [(0, false), (1, false), (2, false), (3, true)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC11: a window after the first follows its own view's file, never
    /// the first window's; and after a switch it joins the next view
    /// quickly (it looks every ASKED_POLL, not every POLL, while it waits):
    /// the join's delay is measured here, the far computer's own start
    /// aside.
    #[test]
    fn a_window_joins_its_own_view_and_a_switch_at_once() {
        let dir = std::env::temp_dir().join(format!("pd-computer-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let file = |session_id, token: [u8; 32]| ViewFile {
            session_id,
            device_id: "desk".into(),
            name: "Desk".into(),
            port: listener.local_addr().unwrap().port(),
            token: pocketdesk_core::local::cast::token_hex(&token),
        };
        // The first window's file names another view: never joined here.
        wire::write(&dir, &file(50, [6u8; 32])).unwrap();
        let ctx = egui::Context::default();
        let window = new_window(&ctx, &dir, 1);
        {
            let mut state = lock(&window);
            ask_with(
                &dir,
                &mut state,
                Wanted {
                    device_id: "desk".into(),
                    name: "Desk".into(),
                    monitor: 0,
                },
                Some(false),
            )
            .unwrap();
        }
        listener.set_nonblocking(true).unwrap();
        let join = |want: [u8; 32]| {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut relay = loop {
                match listener.accept() {
                    Ok((relay, _)) => break relay,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "joined within 10 s");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            let joined = Instant::now();
            relay.set_nonblocking(false).unwrap();
            relay
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut given = [0u8; 32];
            std::io::Read::read_exact(&mut relay, &mut given).unwrap();
            assert_eq!(given, want, "its own view's token");
            (relay, joined)
        };
        wire::write_window(&dir, 1, &file(60, [7u8; 32])).unwrap();
        let (mut relay, _) = join([7u8; 32]);
        // Switches: the next view is named as the old one ends, as the
        // server does; the window joins it within a few polls.
        let mut delays = Vec::new();
        for (n, session) in (61..66).enumerate() {
            {
                let mut state = lock(&window);
                switch_screen(&dir, &mut state, (n % 2) as u32 + 1).unwrap();
            }
            let token = [8 + n as u8; 32];
            wire::write_window(&dir, 1, &file(session, token)).unwrap();
            let named = Instant::now();
            let _ = Message::End { why: String::new() }.write(&mut relay);
            let (next, joined) = join(token);
            delays.push(joined.saturating_duration_since(named));
            relay = next;
        }
        let median = percentile(&mut delays, 0.5).unwrap();
        eprintln!("a switch's join after the view is named: {delays:?}, median {median:?}");
        assert!(median < Duration::from_millis(150), "{delays:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// PC11: after a switch with the mic on, it waits for the new screen's
    /// view, starts again once that controls, and is given up, said, if it
    /// ends or shows without taking input.
    #[test]
    fn the_mic_goes_on_across_a_switch_or_says_why_not() {
        assert_eq!(
            mic_resumes(false, true, false, Phase::Showing),
            Resume::Nothing
        );
        assert_eq!(
            mic_resumes(true, false, false, Phase::Connecting),
            Resume::Wait
        );
        assert_eq!(
            mic_resumes(true, false, false, Phase::Waiting),
            Resume::Wait
        );
        assert_eq!(
            mic_resumes(true, true, false, Phase::Showing),
            Resume::Start
        );
        assert_eq!(
            mic_resumes(true, false, false, Phase::Showing),
            Resume::GiveUp
        );
        assert_eq!(
            mic_resumes(true, false, true, Phase::Connecting),
            Resume::GiveUp
        );
        // The switch stops it here, to start again on the new view.
        let dir =
            std::env::temp_dir().join(format!("pd-computer-mic-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            ..State::default()
        };
        state.mic.toggle();
        switch_screen(&dir, &mut state, 1).unwrap();
        assert!(state.mic_resume && !state.mic.wanted());
        let shared = Arc::new(Mutex::new(State::default()));
        let ctx = egui::Context::default();
        // Waiting for the new view: nothing yet.
        follow_mic(&shared, &mut state, None, &ctx, false);
        assert!(state.mic_resume && state.mic_note.is_none());
        // Shown, but without control there: given up, said.
        state.status.phase = Phase::Showing;
        follow_mic(&shared, &mut state, None, &ctx, false);
        assert!(!state.mic_resume && !state.mic.wanted());
        assert_eq!(state.mic_note.as_deref(), Some(MIC_SWITCH_WORDS));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC11: one window's mic a computer: a second window's "Send my mic"
    /// for it is refused, saying why, and its switch goes off; the app sees
    /// which window sends where.
    #[test]
    fn a_second_windows_mic_for_one_computer_is_refused() {
        let mut state = State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 1,
            }),
            mic_elsewhere: true,
            ..State::default()
        };
        state.mic.toggle();
        let shared = Arc::new(Mutex::new(State::default()));
        follow_mic(&shared, &mut state, None, &egui::Context::default(), true);
        assert!(!state.mic.wanted() && !state.mic.on());
        assert_eq!(state.mic_note, Some(mic_elsewhere_words("Desk")));
        assert!(state.mic_said.is_none(), "the relay was told nothing");
        // The app marks a window whose computer another window sends to.
        let dir = std::env::temp_dir().join(format!("pd-computer-mic-two-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        view.open_new_window(&ctx, &view.shared, 1);
        lock(&view.shared).mic_resume = true;
        let windows = view.windows();
        let marked = |view: &ComputerView| {
            view.windows()
                .iter()
                .map(|w| lock(w).mic_elsewhere)
                .collect::<Vec<_>>()
        };
        // What show() works out each pass, before it shows any window.
        view.follow_windows(&ctx);
        assert_eq!(marked(&view), [false, true]);
        assert_eq!(windows.len(), 2);
        view.quit();
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// PR #570 review I1: a picture the old screen's decoder still gives
    /// after a switch is dropped: it neither ends the switch (nor its
    /// time) nor shows; the next screen's first picture does.
    #[test]
    fn an_old_screens_picture_after_a_switch_is_dropped() {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-old-picture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let shared = Arc::new(Mutex::new(State {
            wanted: Some(Wanted {
                device_id: "desk".into(),
                name: "Desk".into(),
                monitor: 0,
            }),
            decoder_gen: 4,
            ..State::default()
        }));
        let ctx = egui::Context::default();
        let picture = || Picture {
            width: 8,
            height: 8,
            planes: Planes::Rgba,
            data: vec![0; 8 * 8 * 4],
            colour: crate::video::Colour::default(),
        };
        let mut old = publisher(&shared, &ctx, 4);
        switch_screen(&dir, &mut lock(&shared), 1).unwrap();
        old(picture());
        {
            let state = lock(&shared);
            assert!(state.picture.is_none(), "the old screen's picture");
            assert!(state.switching.is_some(), "still switching");
        }
        // The next screen's decoder starts with a newer number.
        let next = lock(&shared).decoder_gen + 1;
        lock(&shared).decoder_gen = next;
        publisher(&shared, &ctx, next)(picture());
        assert!(lock(&shared).picture.is_some(), "the new screen's shows");
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// PR #570 review I2: with "Lock it when I close the window" on, a
    /// window whose view of the same computer ended does not keep the
    /// shown window from locking it as Pocketdesk quits.
    #[test]
    fn a_window_ended_keeps_no_lock_from_the_one_shown() {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-lock-ended-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        view.open_new_window(&ctx, &view.shared, 1);
        computers::take_all(&dir, now_unix());
        for window in view.windows() {
            let mut state = lock(&window);
            state.lock_on_close = true;
            state.status.phase = Phase::Showing;
        }
        lock(&view.windows()[1]).ended = Some("Desk refused it.".into());
        view.quit();
        let mut locks: Vec<(u8, bool)> = computers::take_all(&dir, now_unix())
            .into_iter()
            .filter_map(|r| match r.ask {
                ComputerAsk::StopView { lock, window, .. } => Some((window, lock)),
                _ => None,
            })
            .collect();
        locks.sort();
        assert_eq!(locks, [(0, true), (1, false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// PR #570 review I4: a later window of a computer that is not Windows
    /// opens View only; of a Windows computer, with the first's choice.
    #[test]
    fn a_later_window_of_a_computer_not_windows_opens_view_only() {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-extra-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone(), None);
        view.open("desk", "Desk", 0, 0, false, false).unwrap();
        lock(&view.shared).status.windows = Some(false);
        view.open_new_window(&ctx, &view.shared, 1);
        assert!(lock(&view.windows()[1]).view_only, "Linux: View only");
        assert!(!lock(&view.shared).view_only, "the first window as it was");
        lock(&view.shared).status.windows = Some(true);
        view.open_new_window(&ctx, &view.shared, 1);
        assert!(!lock(&view.windows()[2]).view_only, "Windows: as chosen");
        view.quit();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
