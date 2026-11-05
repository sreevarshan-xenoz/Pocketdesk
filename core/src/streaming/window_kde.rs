//! KDE Plasma as the source of a lent window (window handoff, phase 6
//! follow-up): the table of windows KWin reports, the questions the window
//! guard asks of it, and the matching of the portal's stream to the window
//! that was lent.
//!
//! **Where the table comes from.** A resident KWin script (the server's
//! `lend_window::kde_script`) pushes the whole table to the server over
//! D-Bus on each change: each window's uuid, caption, class, pid, frame
//! geometry, whether it is minimised and whether it is shown on the current
//! desktop, its transient-for parent, the stacking order and the active
//! window. [`publish`] parses one message ([`parse_message`], bounded and
//! strict: a message that is not exactly what the script sends is dropped
//! whole and changes nothing) and keeps the newest table with the time it
//! came ([`Snapshot::at`]).
//!
//! **A window's name here is a number.** KWin's uuid is not a window id the
//! lend path takes (it takes digits), so the table gives each uuid a number
//! the first time it is seen ([`KdeWindow::seq`], never reused, counted from
//! 1), and the lend, the lease (`kde:<seq>`, [`kde_native`]) and the guard
//! speak in that number. A uuid KWin never gives to another window, so a
//! number is never another window's.
//!
//! **The guard's questions** ([`KdeOs`], the [`Os`] over the table) follow
//! the rule everything in `window_guard` serves: when in doubt, no.
//!
//! - A table older than [`STALE_AFTER`] answers no to every question (the
//!   script stopped, KWin hung, the bus failed: nothing said what is on the
//!   screen now). The server asks the script for a fresh table about once a
//!   second while a window is lent, so a table that is not stale is current
//!   to a moment.
//! - **Frame:** the lent window's frame geometry, only while it is shown (not
//!   minimised, on the desktop in view).
//! - **In front:** the active window is the lent one or belongs to it
//!   ([`belongs_to`], the same rule as Windows: a window of the same program
//!   whose transient-for chain leads to the lent window, so its dialogs and
//!   menus).
//! - **Under a point:** among the shown windows whose frame holds the point,
//!   the one highest in the stacking order is the lent window or belongs to
//!   it. A window that holds the point but is not in the stacking order has
//!   no place in it and is taken to cover (the point is not the lent
//!   window's). Panels, docks and the desktop are windows of the table like
//!   any other, so a panel over a corner of the lent window covers it.
//! - **Raise:** a request the script takes with its next report
//!   ([`request_raise`]); there is no other way into KWin from here.
//!
//! **Not wired to input yet.** A lent KDE window is view-only for now
//! (`window_target::takes_input` is false for any compositor's window): the
//! portal's RemoteDesktop would have to put the pointer in the window's
//! stream, which no desktop has been shown to confine; so [`KdeOs`] is built
//! and tested but the streamer's guard does not use it. When input is wired,
//! the guard's `Machine` answers for a `kde:` lease through it.
//!
//! **The portal's picture.** KDE's screen-sharing portal shows its owner a
//! dialog the first time a window is chosen and, asked to keep the grant,
//! gives a restore token that brings the same window back with no dialog.
//! Whoever answers the dialog may pick any window, so the stream the portal
//! answers with is matched to the lent window ([`match_stream`]) and refused
//! unless it can be shown to be that window: one stream, of a window, named
//! as the lent window or of its size, and no other window of the same size
//! that it could be mistaken for.
//!
//! Everything here is plain data and decisions, tested on every system; the
//! D-Bus calls are the server's and the portal's are `capture_kde`'s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::window_guard::{Os, belongs_to, frame_contains};
use super::window_target::{Lease, Rect, lease_alive};

/// How old the table may be and still be believed.
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// The largest message taken from the script, in bytes.
pub const MAX_MESSAGE: usize = 512 * 1024;

/// The most windows a message may name.
pub const MOST_WINDOWS: usize = 512;

/// The most a caption keeps, in characters, and a class.
const MOST_CAPTION: usize = 200;
const MOST_CLASS: usize = 64;

/// How far a coordinate or a size may be from zero and be a place on a screen.
const MOST_PIXELS: i64 = 1_000_000;

/// How many uuids are remembered before those not in the newest table are
/// forgotten (their numbers are never given again).
const MOST_REMEMBERED: usize = 2048;

/// How much a portal stream's size may differ from the window's frame, per
/// axis, and still be that window's (a shadow, a rounded edge).
pub const SIZE_TOLERANCE: i32 = 2;

/// The most raise requests waiting for the script's next report.
const MOST_RAISES: usize = 4;

/// What a window of KDE is called to the window table: `kde:` and the number
/// the table gave it, in decimal.
pub fn kde_native(seq: u64) -> String {
    format!("kde:{seq}")
}

/// The number a native name holds: `kde:` and plain decimal digits (no sign,
/// no spaces, no leading zero, one that fits), nothing else.
pub fn kde_seq(native: &str) -> Option<u64> {
    let digits = native.strip_prefix("kde:")?;
    if digits.is_empty()
        || digits.len() > 20
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse::<u64>().ok().filter(|seq| *seq != 0)
}

/// Whether `XDG_CURRENT_DESKTOP` (a colon-separated list) names KDE Plasma.
pub fn is_kde(current_desktop: Option<&str>) -> bool {
    current_desktop.is_some_and(|d| d.split(':').any(|part| part.eq_ignore_ascii_case("KDE")))
}

/// A KWin window uuid as the table keeps it: the 36 characters of a uuid in
/// lower case, without the braces KWin's own text has. `None` for anything
/// else, so only that shape is ever put in a script, a file name or a
/// message.
pub fn bare_uuid(text: &str) -> Option<String> {
    let inner = match text.strip_prefix('{') {
        Some(rest) => rest.strip_suffix('}')?,
        None => text,
    };
    let bytes = inner.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let ok = bytes.iter().enumerate().all(|(i, b)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            *b == b'-'
        } else {
            b.is_ascii_hexdigit()
        }
    });
    ok.then(|| inner.to_ascii_lowercase())
}

/// The name of the file a window's kept screen-sharing grant is in (inside
/// the grants folder, [`set_grant_dir`]); `None` unless `uuid` is a window's.
pub fn grant_name(uuid: &str) -> Option<String> {
    Some(format!("kde-window-{}", bare_uuid(uuid)?))
}

// ---- The message ----

/// Why a message from the script was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    NotJson,
    Version,
    TooMany,
    BadId,
    Duplicate,
    BadGeometry,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooLarge => "the message is too large",
            Self::NotJson => "the message is not what the script sends",
            Self::Version => "the message is of another version",
            Self::TooMany => "the message names too many windows",
            Self::BadId => "a window's id is not a uuid",
            Self::Duplicate => "a window is named twice",
            Self::BadGeometry => "a window's place or size is not one a screen has",
        })
    }
}

impl std::error::Error for ParseError {}

#[derive(Deserialize)]
struct WireWindow {
    id: String,
    #[serde(default)]
    caption: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    pid: i64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    /// The client area's size (without a server-side title bar and border),
    /// in the frame's own logical pixels.
    cw: i64,
    ch: i64,
    /// The scale of the output the window is on; none, or nonsense, reads as
    /// not known.
    #[serde(default)]
    scale: f64,
    min: bool,
    shown: bool,
    normal: bool,
    #[serde(default)]
    parent: String,
}

#[derive(Deserialize)]
struct Wire {
    v: u32,
    windows: Vec<WireWindow>,
    #[serde(default)]
    stack: Vec<String>,
    #[serde(default)]
    active: String,
}

/// A window of a message, checked, before it has a number.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawWindow {
    uuid: String,
    caption: String,
    class: String,
    pid: u32,
    frame: Rect,
    client: (u32, u32),
    scale_milli: Option<u32>,
    minimized: bool,
    shown: bool,
    normal: bool,
    parent: Option<String>,
}

/// A message, checked: windows with their uuids, the stacking order from the
/// bottom, and the active window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    windows: Vec<RawWindow>,
    stack: Vec<String>,
    active: Option<String>,
}

/// Text from a window's own title or class with nothing in it that is not
/// plain text (control, bidirectional and other format characters), cut to
/// `most` characters.
fn clean(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    *c,
                    '\u{200b}'..='\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2060}'..='\u{2064}'
                        | '\u{2066}'..='\u{206f}'
                        | '\u{feff}'
                        | '\u{fff9}'..='\u{fffb}'
                )
        })
        .take(most)
        .collect::<String>()
        .trim()
        .to_string()
}

/// An output's scale (1.0, 1.25, 1.5...) in thousandths, if it is one an
/// output has (a quarter to eight times); anything else is not known.
fn scale_milli(scale: f64) -> Option<u32> {
    (scale.is_finite() && (0.25..=8.0).contains(&scale)).then(|| (scale * 1000.0).round() as u32)
}

/// Read what the script sent. Strict: any window that is not exactly as
/// expected makes the whole message refused (the table stays as it was and
/// ages); the ids it names that it does not list (the active window, a
/// parent, a stacking entry) are the safe direction and are read as none.
pub fn parse_message(json: &str) -> Result<Message, ParseError> {
    if json.len() > MAX_MESSAGE {
        return Err(ParseError::TooLarge);
    }
    let wire: Wire = serde_json::from_str(json).map_err(|_| ParseError::NotJson)?;
    if wire.v != 1 {
        return Err(ParseError::Version);
    }
    if wire.windows.len() > MOST_WINDOWS || wire.stack.len() > MOST_WINDOWS * 2 {
        return Err(ParseError::TooMany);
    }
    let sane = |v: i64| (-MOST_PIXELS..=MOST_PIXELS).contains(&v);
    let mut seen = std::collections::HashSet::new();
    let mut windows = Vec::with_capacity(wire.windows.len());
    for w in wire.windows {
        let uuid = bare_uuid(&w.id).ok_or(ParseError::BadId)?;
        if !seen.insert(uuid.clone()) {
            return Err(ParseError::Duplicate);
        }
        if !(sane(w.x)
            && sane(w.y)
            && (0..=MOST_PIXELS).contains(&w.w)
            && (0..=MOST_PIXELS).contains(&w.h)
            && (0..=MOST_PIXELS).contains(&w.cw)
            && (0..=MOST_PIXELS).contains(&w.ch))
        {
            return Err(ParseError::BadGeometry);
        }
        windows.push(RawWindow {
            uuid,
            caption: clean(&w.caption, MOST_CAPTION),
            class: clean(&w.class, MOST_CLASS),
            pid: u32::try_from(w.pid).unwrap_or(0),
            frame: (w.x as i32, w.y as i32, w.w as u32, w.h as u32),
            client: (w.cw as u32, w.ch as u32),
            scale_milli: scale_milli(w.scale),
            minimized: w.min,
            shown: w.shown,
            normal: w.normal,
            // A parent that is no uuid, or that no window of the message
            // has (looked up when the table is made), is no parent: the
            // window then belongs to nothing.
            parent: bare_uuid(&w.parent),
        });
    }
    let known: std::collections::HashSet<&str> = windows.iter().map(|w| w.uuid.as_str()).collect();
    let stack = wire
        .stack
        .iter()
        .filter_map(|id| bare_uuid(id))
        .filter(|id| known.contains(id.as_str()))
        .collect();
    let active = bare_uuid(&wire.active).filter(|id| known.contains(id.as_str()));
    Ok(Message {
        windows,
        stack,
        active,
    })
}

// ---- The table ----

/// One window of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KdeWindow {
    /// The number this program gave it, which the lend and the lease name it
    /// by.
    pub seq: u64,
    pub uuid: String,
    pub caption: String,
    pub class: String,
    pub pid: u32,
    pub frame: Rect,
    /// The client area's size without a server-side title bar and border, in
    /// the frame's logical pixels.
    pub client: (u32, u32),
    /// The scale of the output the window is on, in thousandths (1500 is
    /// 1.5); `None` when the script could not say.
    pub scale_milli: Option<u32>,
    pub minimized: bool,
    /// On the desktop in view and not minimised, as the script saw it.
    pub shown: bool,
    /// An ordinary window of a program (not a panel, the desktop, a menu, a
    /// notification or a dialog): the only kind that may be offered to lend.
    pub normal: bool,
    /// The window this one is transient for (a dialog's parent), by number.
    pub parent: Option<u64>,
    /// Its place in the stacking order, from the bottom; `None` when the
    /// order did not name it.
    pub z: Option<usize>,
}

/// The table at one moment.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub windows: Vec<KdeWindow>,
    /// The active window, by number.
    pub active: Option<u64>,
    /// When the message came.
    pub at: Instant,
}

impl Snapshot {
    /// Whether the table is recent enough to be believed at `now`.
    pub fn is_fresh(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < STALE_AFTER
    }

    pub fn window(&self, seq: u64) -> Option<&KdeWindow> {
        self.windows.iter().find(|w| w.seq == seq)
    }

    fn owner_of(&self, seq: isize) -> isize {
        u64::try_from(seq)
            .ok()
            .and_then(|seq| self.window(seq))
            .and_then(|w| w.parent)
            .map_or(0, |p| p as isize)
    }

    /// Whether `candidate` is the lent window `lent` or belongs to it.
    fn belongs(&self, candidate: &KdeWindow, lent: &KdeWindow) -> bool {
        belongs_to(
            candidate.seq as isize,
            candidate.pid,
            lent.seq as isize,
            lent.pid,
            |seq| self.owner_of(seq),
        )
    }

    /// The lent window, if the table is fresh and has it shown.
    fn lent_shown(&self, seq: u64, now: Instant) -> Option<&KdeWindow> {
        if !self.is_fresh(now) {
            return None;
        }
        self.window(seq).filter(|w| w.shown && !w.minimized)
    }

    /// The visible frame of the lent window.
    pub fn frame_of(&self, seq: u64, now: Instant) -> Option<Rect> {
        let w = self.lent_shown(seq, now)?;
        (w.frame.2 > 0 && w.frame.3 > 0).then_some(w.frame)
    }

    /// Whether the active window is the lent one or belongs to it.
    pub fn front_is_lent(&self, seq: u64, now: Instant) -> bool {
        let Some(lent) = self.lent_shown(seq, now) else {
            return false;
        };
        self.active
            .and_then(|active| self.window(active))
            .is_some_and(|active| active.shown && self.belongs(active, lent))
    }

    /// Whether the window that takes a click at the desktop pixel `at` is the
    /// lent one or belongs to it.
    pub fn hit_is_lent(&self, seq: u64, at: (i32, i32), now: Instant) -> bool {
        let Some(lent) = self.lent_shown(seq, now) else {
            return false;
        };
        let mut top: Option<(usize, &KdeWindow)> = None;
        for w in &self.windows {
            if !w.shown || w.minimized || !frame_contains(w.frame, at) {
                continue;
            }
            // Something is there that the order does not place: it may be
            // over the lent window, so the spot is not the lent window's.
            let Some(z) = w.z else {
                return false;
            };
            if top.is_none_or(|(best, _)| z > best) {
                top = Some((z, w));
            }
        }
        top.is_some_and(|(_, w)| self.belongs(w, lent))
    }
}

/// The newest table, and the numbers given to uuids.
#[derive(Default)]
pub struct Table {
    next: u64,
    seqs: HashMap<String, u64>,
    snapshot: Option<Arc<Snapshot>>,
}

impl Table {
    /// Take a checked message as the table now.
    pub fn apply(&mut self, message: Message, now: Instant) -> Arc<Snapshot> {
        if self.seqs.len() > MOST_REMEMBERED {
            let live: std::collections::HashSet<&str> =
                message.windows.iter().map(|w| w.uuid.as_str()).collect();
            self.seqs.retain(|uuid, _| live.contains(uuid.as_str()));
        }
        let mut number = |uuid: &str| -> u64 {
            if let Some(seq) = self.seqs.get(uuid) {
                return *seq;
            }
            self.next += 1;
            self.seqs.insert(uuid.to_string(), self.next);
            self.next
        };
        let seqs: HashMap<String, u64> = message
            .windows
            .iter()
            .map(|w| (w.uuid.clone(), number(&w.uuid)))
            .collect();
        let z: HashMap<&str, usize> = message
            .stack
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let windows = message
            .windows
            .iter()
            .map(|w| KdeWindow {
                seq: seqs[&w.uuid],
                uuid: w.uuid.clone(),
                caption: w.caption.clone(),
                class: w.class.clone(),
                pid: w.pid,
                frame: w.frame,
                client: w.client,
                scale_milli: w.scale_milli,
                minimized: w.minimized,
                shown: w.shown,
                normal: w.normal,
                parent: w.parent.as_ref().and_then(|p| seqs.get(p).copied()),
                z: z.get(w.uuid.as_str()).copied(),
            })
            .collect();
        let snapshot = Arc::new(Snapshot {
            windows,
            active: message.active.as_ref().and_then(|a| seqs.get(a).copied()),
            at: now,
        });
        self.snapshot = Some(snapshot.clone());
        snapshot
    }

    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.snapshot.clone()
    }

    /// Forget the table (the script is unloaded): it is not believed from
    /// now. The numbers given are kept, so none is given twice.
    pub fn clear(&mut self) {
        self.snapshot = None;
    }
}

fn table() -> std::sync::MutexGuard<'static, Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE
        .get_or_init(|| Mutex::new(Table::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// A receiver that changes whenever a message is taken.
pub fn updates() -> tokio::sync::watch::Receiver<u64> {
    updates_tx().subscribe()
}

fn updates_tx() -> &'static tokio::sync::watch::Sender<u64> {
    static TX: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// Take a message from the script as the table now. A message that is not
/// taken changes nothing.
pub fn publish(json: &str) -> Result<(), ParseError> {
    let message = parse_message(json)?;
    table().apply(message, Instant::now());
    updates_tx().send_modify(|n| *n = n.wrapping_add(1));
    Ok(())
}

/// The newest table, however old (look at [`Snapshot::is_fresh`]).
pub fn snapshot() -> Option<Arc<Snapshot>> {
    table().snapshot()
}

/// Forget the table: the script is gone.
pub fn clear() {
    table().clear();
}

/// The uuid of the window with the number `seq` in the newest table.
pub fn uuid_of(seq: u64) -> Option<String> {
    snapshot()?.window(seq).map(|w| w.uuid.clone())
}

// ---- Raising ----

fn raises() -> std::sync::MutexGuard<'static, Vec<String>> {
    static RAISES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    RAISES.lock().unwrap_or_else(|e| e.into_inner())
}

type Poker = Arc<dyn Fn() + Send + Sync>;

fn poker() -> std::sync::MutexGuard<'static, Option<Poker>> {
    static POKER: Mutex<Option<Poker>> = Mutex::new(None);
    POKER.lock().unwrap_or_else(|e| e.into_inner())
}

/// What to do to make the script report at once (the server sets it while
/// the script is loaded and clears it when it is not).
pub fn set_poker(poke: Option<Poker>) {
    *poker() = poke;
}

/// Ask the script, with its next report, to raise and activate the window
/// numbered `seq`. Only a window the newest table has is asked for, at most
/// [`MOST_RAISES`] at a time.
pub fn request_raise(seq: u64) {
    let Some(uuid) = uuid_of(seq) else {
        return;
    };
    {
        let mut waiting = raises();
        if waiting.contains(&uuid) || waiting.len() >= MOST_RAISES {
            return;
        }
        waiting.push(uuid);
    }
    poke();
}

/// Ask the script to report now, if it is loaded (the same call a raise
/// makes). Nothing says whether a report follows; [`updates`] does.
pub fn poke() {
    let poke = poker().clone();
    if let Some(poke) = poke {
        poke();
    }
}

/// The uuids waiting to be raised, taken (the reply to the script's report).
pub fn take_raises() -> Vec<String> {
    std::mem::take(&mut *raises())
}

// ---- Where the grants are kept ----

fn grant_dir_slot() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
    DIR.lock().unwrap_or_else(|e| e.into_inner())
}

/// The folder a lent window's kept screen-sharing grant is written in (the
/// server sets it from its state folder; nothing is kept without it).
pub fn set_grant_dir(dir: Option<PathBuf>) {
    *grant_dir_slot() = dir;
}

pub fn grant_dir() -> Option<PathBuf> {
    grant_dir_slot().clone()
}

// ---- The guard's Os over the table ----

/// What the window guard asks of the windows of a KDE desktop, answered from
/// the table (see the module's notes). Built and tested; not yet the guard's
/// Os, because a lent KDE window is view-only.
pub struct KdeOs;

impl KdeOs {
    fn ask<T>(lease: &Lease, otherwise: T, look: impl FnOnce(&Snapshot, u64, Instant) -> T) -> T {
        let (Some(seq), Some(snapshot)) = (kde_seq(&lease.native), snapshot()) else {
            return otherwise;
        };
        look(&snapshot, seq, Instant::now())
    }
}

impl Os for KdeOs {
    fn lease_holds(&self, lease: &Lease) -> bool {
        lease_alive(lease)
    }

    fn frame(&self, lease: &Lease) -> Option<Rect> {
        Self::ask(lease, None, |s, seq, now| s.frame_of(seq, now))
    }

    fn front_is_lent(&self, lease: &Lease) -> bool {
        Self::ask(lease, false, |s, seq, now| s.front_is_lent(seq, now))
    }

    fn hit_is_lent(&self, lease: &Lease, at: (i32, i32)) -> bool {
        Self::ask(lease, false, |s, seq, now| s.hit_is_lent(seq, at, now))
    }

    fn raise(&self, lease: &Lease) {
        if lease_alive(lease)
            && let Some(seq) = kde_seq(&lease.native)
        {
            request_raise(seq);
        }
    }
}

// ---- The portal's stream ----

/// What a portal stream is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Window,
    Monitor,
    Virtual,
    /// The portal did not say.
    Unknown,
}

/// A stream the portal answered with, reduced to what shows whose it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalStream {
    pub node: u32,
    pub kind: StreamKind,
    pub size: Option<(i32, i32)>,
    /// What the portal calls it (its id and mapping id), if it says.
    pub names: Vec<String>,
}

/// Why a stream was not taken for the lent window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    NoStream,
    SeveralStreams,
    NotAWindow,
    SizeUnknown,
    WrongSize,
    /// The scale of the lent window's output is not known, so its size in
    /// device pixels cannot be said.
    ScaleUnknown,
    /// It names another window of the table.
    Another,
    /// Another window has the same size: the stream could be either.
    Ambiguous,
}

fn same_size(a: (i32, i32), b: (u32, u32)) -> bool {
    let (bw, bh) = (i64::from(b.0), i64::from(b.1));
    (i64::from(a.0) - bw).abs() <= i64::from(SIZE_TOLERANCE)
        && (i64::from(a.1) - bh).abs() <= i64::from(SIZE_TOLERANCE)
}

/// The scales (in thousandths) another window is tried at when its own is
/// not known: the ones outputs have.
const PLAUSIBLE_SCALES: [u32; 8] = [1000, 1250, 1500, 1750, 2000, 2250, 2500, 3000];

/// A size in logical pixels as device pixels at `scale_milli`.
fn device(logical: u32, scale_milli: u32) -> u32 {
    u32::try_from((u64::from(logical) * u64::from(scale_milli) + 500) / 1000).unwrap_or(u32::MAX)
}

/// Whether a stream of `size` (device pixels) could be of the window `w` on an
/// output of `scale_milli`: the portal's window stream is the window's client
/// area or its whole frame (a server-side title bar and border), in device
/// pixels. The compared sizes are device pixels on both sides, never the
/// window's logical size.
fn could_be(size: (i32, i32), w: &KdeWindow, scale_milli: u32) -> bool {
    let of = |(width, height): (u32, u32)| {
        let sized = (device(width, scale_milli), device(height, scale_milli));
        sized.0 > 0 && sized.1 > 0 && same_size(size, sized)
    };
    of(w.client) || of((w.frame.2, w.frame.3))
}

/// The node of the stream that is shown to be the lent window `lent`, or why
/// none is. One stream of a window (the owner picks one in the portal's
/// dialog, and any window may be picked), that names the lent window, or, if
/// it names none, has the size of the lent window in device pixels (its client
/// area or its frame times its output's scale) and no other window of the
/// table could have that size at its own scale (it could be either). A stream
/// that names the lent window is the only unconditional accept; a window whose
/// output scale is not known is refused.
pub fn match_stream(
    streams: &[PortalStream],
    lent: &KdeWindow,
    table: &Snapshot,
) -> Result<u32, Mismatch> {
    let stream = match streams {
        [] => return Err(Mismatch::NoStream),
        [one] => one,
        _ => return Err(Mismatch::SeveralStreams),
    };
    if stream.kind != StreamKind::Window {
        return Err(Mismatch::NotAWindow);
    }
    let named: Vec<String> = stream.names.iter().filter_map(|n| bare_uuid(n)).collect();
    if named.contains(&lent.uuid) {
        return Ok(stream.node);
    }
    if named
        .iter()
        .any(|n| table.windows.iter().any(|w| w.uuid == *n))
    {
        return Err(Mismatch::Another);
    }
    let size = stream
        .size
        .filter(|(w, h)| *w > 0 && *h > 0)
        .ok_or(Mismatch::SizeUnknown)?;
    let scale = lent.scale_milli.ok_or(Mismatch::ScaleUnknown)?;
    if !could_be(size, lent, scale) {
        return Err(Mismatch::WrongSize);
    }
    // Another window that the stream could be of, at that window's own scale
    // (at every scale an output has, where it is not known).
    let other_like = table.windows.iter().any(|w| {
        w.seq != lent.seq
            && match w.scale_milli {
                Some(own) => could_be(size, w, own),
                None => PLAUSIBLE_SCALES.iter().any(|own| could_be(size, w, *own)),
            }
    });
    if other_like {
        return Err(Mismatch::Ambiguous);
    }
    Ok(stream.node)
}

// ---- What the owner is told ----

/// Why a window of KDE cannot be lent, in the owner's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotWayland,
    /// KWin would not take the script.
    ScriptRefused,
    /// The script loaded and reported nothing.
    ScriptSilent,
    /// The script reported, then stopped.
    TableStale,
    /// The screen-sharing portal is not there.
    NoPortal,
    /// The portal is there and cannot share one window.
    PortalNoWindows,
}

/// What `refusal` says to the owner.
pub fn say(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::NotWayland => {
            "This computer lends a window of KDE Plasma only in a Wayland session (this one is not)"
        }
        Refusal::ScriptRefused => {
            "KWin did not take the window script Pocketdesk gives it to list windows (check System Settings, Window Management, KWin Scripts)"
        }
        Refusal::ScriptSilent => {
            "KWin took the window script but it reported no windows (it may have been refused by KWin's script settings)"
        }
        Refusal::TableStale => "KWin's window script has stopped reporting",
        Refusal::NoPortal => {
            "KDE's screen-sharing portal is not there (xdg-desktop-portal and xdg-desktop-portal-kde), so a window cannot be shown"
        }
        Refusal::PortalNoWindows => {
            "KDE's screen-sharing portal cannot share a single window on this computer"
        }
    }
}

/// What the session says when the portal's answer is not the lent window.
pub fn say_mismatch(mismatch: Mismatch) -> &'static str {
    match mismatch {
        Mismatch::NoStream => "The screen-sharing dialog gave no window",
        Mismatch::SeveralStreams => {
            "The screen-sharing dialog gave more than one picture, so none was taken for the lent window"
        }
        Mismatch::NotAWindow => {
            "The screen-sharing dialog was answered with a screen, not the lent window: nothing is shown"
        }
        Mismatch::SizeUnknown | Mismatch::WrongSize | Mismatch::Another => {
            "The screen-sharing dialog was answered with a window that cannot be shown to be the lent one: nothing is shown"
        }
        Mismatch::ScaleUnknown => {
            "The lent window's screen scale is not known, so the picture cannot be shown to be the lent window's: nothing is shown"
        }
        Mismatch::Ambiguous => {
            "Another window has the same size as the lent one, so the picture cannot be shown to be the lent window's: nothing is shown"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::input_packet::InputPacket;
    use crate::streaming::window_guard::WindowGuard;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// The tests that publish to the process's table take turns.
    static TURN: Mutex<()> = Mutex::new(());

    // Uuids of the shape KWin gives (made up).
    const A: &str = "00000000-0000-4000-8000-00000000000a";
    const B: &str = "00000000-0000-4000-8000-00000000000b";
    const C: &str = "00000000-0000-4000-8000-00000000000c";
    const D: &str = "00000000-0000-4000-8000-00000000000d";

    fn win(id: &str, pid: i64, rect: (i64, i64, i64, i64)) -> serde_json::Value {
        serde_json::json!({
            "id": format!("{{{id}}}"), "caption": format!("title {id}"), "class": "app",
            "pid": pid, "x": rect.0, "y": rect.1, "w": rect.2, "h": rect.3,
            "cw": rect.2, "ch": rect.3, "scale": 1.0,
            "min": false, "shown": true, "normal": true, "parent": ""
        })
    }

    fn message(windows: Vec<serde_json::Value>, stack: &[&str], active: &str) -> String {
        serde_json::json!({
            "v": 1, "windows": windows,
            "stack": stack.iter().map(|s| format!("{{{s}}}")).collect::<Vec<_>>(),
            "active": if active.is_empty() { String::new() } else { format!("{{{active}}}") },
        })
        .to_string()
    }

    fn table_of(json: &str, now: Instant) -> Snapshot {
        let message = parse_message(json).unwrap();
        let snapshot = Table::default().apply(message, now);
        Arc::try_unwrap(snapshot).unwrap_or_else(|s| (*s).clone())
    }

    fn seq(snapshot: &Snapshot, uuid: &str) -> u64 {
        snapshot
            .windows
            .iter()
            .find(|w| w.uuid == uuid)
            .unwrap()
            .seq
    }

    /// A is lent (pid 10, 100,100 400x300); B is a window of another program
    /// over its corner; C is A's dialog; D is another window of A's program
    /// that is not its dialog. Stack, bottom to top: A, D, C, B.
    fn desk(now: Instant) -> Snapshot {
        let mut c = win(C, 10, (150, 150, 100, 100));
        c["parent"] = format!("{{{A}}}").into();
        let json = message(
            vec![
                win(A, 10, (100, 100, 400, 300)),
                win(B, 20, (450, 350, 200, 200)),
                c,
                win(D, 10, (0, 0, 50, 50)),
            ],
            &[A, D, C, B],
            A,
        );
        table_of(&json, now)
    }

    // ---- Names ----

    #[test]
    fn a_native_name_is_kde_and_plain_digits_only() {
        assert_eq!(kde_native(7), "kde:7");
        assert_eq!(kde_seq("kde:7"), Some(7));
        assert_eq!(kde_seq("kde:18446744073709551615"), Some(u64::MAX));
        for bad in [
            "",
            "kde:",
            "kde:0",
            "kde:07",
            "kde:-1",
            "kde:+1",
            "kde: 1",
            "kde:1 ",
            "gnome:7",
            "kde:18446744073709551616",
            "kde:1;2",
            "KDE:7",
        ] {
            assert_eq!(kde_seq(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn kde_is_named_in_the_desktop_list() {
        assert!(is_kde(Some("KDE")));
        assert!(is_kde(Some("plasma:KDE")));
        assert!(is_kde(Some("kde")));
        assert!(!is_kde(Some("GNOME")));
        assert!(!is_kde(Some("KDEish")));
        assert!(!is_kde(Some("")));
        assert!(!is_kde(None));
    }

    #[test]
    fn a_uuid_is_thirty_six_hex_and_dash_characters_with_or_without_braces() {
        assert_eq!(bare_uuid(&format!("{{{A}}}")).as_deref(), Some(A));
        assert_eq!(bare_uuid(A).as_deref(), Some(A));
        assert_eq!(
            bare_uuid("{00000000-0000-4000-8000-0000000000AB}").as_deref(),
            Some("00000000-0000-4000-8000-0000000000ab")
        );
        for bad in [
            "",
            "{}",
            "{00000000-0000-4000-8000-00000000000a",
            "00000000-0000-4000-8000-00000000000a}",
            "00000000-0000-4000-8000-00000000000",
            "00000000-0000-4000-8000-00000000000g",
            "00000000_0000-4000-8000-00000000000a",
            "00000000-0000-4000-8000-00000000000a\n",
            "\"; os.system(1) //-4000-8000-00000000000a",
        ] {
            assert_eq!(bare_uuid(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_grant_is_kept_under_a_name_made_only_from_a_uuid() {
        assert_eq!(
            grant_name(&format!("{{{A}}}")).as_deref(),
            Some("kde-window-00000000-0000-4000-8000-00000000000a")
        );
        assert_eq!(grant_name("../../etc/passwd"), None);
        assert_eq!(grant_name(""), None);
    }

    // ---- The message ----

    #[test]
    fn a_message_is_read_with_its_windows_stack_and_active_window() {
        let now = Instant::now();
        let s = desk(now);
        assert_eq!(s.windows.len(), 4);
        let a = s.window(seq(&s, A)).unwrap();
        assert_eq!(a.frame, (100, 100, 400, 300));
        assert_eq!(a.pid, 10);
        assert_eq!(a.caption, format!("title {A}"));
        assert_eq!(a.z, Some(0));
        assert_eq!(s.window(seq(&s, B)).unwrap().z, Some(3));
        assert_eq!(s.active, Some(seq(&s, A)));
        let c = s.window(seq(&s, C)).unwrap();
        assert_eq!(c.parent, Some(seq(&s, A)));
    }

    #[test]
    fn a_window_keeps_its_number_across_messages_and_a_new_one_gets_the_next() {
        let mut table = Table::default();
        let now = Instant::now();
        let one = table.apply(
            parse_message(&message(
                vec![win(A, 1, (0, 0, 5, 5)), win(B, 1, (0, 0, 5, 5))],
                &[A, B],
                "",
            ))
            .unwrap(),
            now,
        );
        let two = table.apply(
            parse_message(&message(
                vec![win(B, 1, (1, 1, 5, 5)), win(C, 1, (0, 0, 5, 5))],
                &[B, C],
                "",
            ))
            .unwrap(),
            now,
        );
        let n = |s: &Snapshot, id: &str| s.windows.iter().find(|w| w.uuid == id).map(|w| w.seq);
        assert_eq!((n(&one, A), n(&one, B)), (Some(1), Some(2)));
        assert_eq!(n(&two, B), Some(2), "the same window, the same number");
        assert_eq!(n(&two, C), Some(3));
        // A window that was left out of a message and comes back keeps its
        // number (the numbers are forgotten only for a long list).
        let three = table.apply(
            parse_message(&message(vec![win(A, 1, (0, 0, 5, 5))], &[A], "")).unwrap(),
            now,
        );
        assert_eq!(n(&three, A), Some(1));
    }

    #[test]
    fn a_message_that_is_not_what_the_script_sends_is_refused_whole() {
        let ok = message(vec![win(A, 1, (0, 0, 5, 5))], &[A], A);
        assert!(parse_message(&ok).is_ok());
        assert_eq!(parse_message("not json"), Err(ParseError::NotJson));
        assert_eq!(parse_message("{}"), Err(ParseError::NotJson));
        assert_eq!(
            parse_message(&ok.replace("\"v\":1", "\"v\":2")),
            Err(ParseError::Version)
        );
        // A window with a field missing is not read as anything: not as
        // shown or hidden, nor as ordinary or not.
        for field in ["shown", "min", "normal", "x", "w", "cw", "ch", "id"] {
            let mut missing = win(A, 1, (0, 0, 5, 5));
            missing.as_object_mut().unwrap().remove(field);
            assert_eq!(
                parse_message(&message(vec![missing], &[A], "")),
                Err(ParseError::NotJson),
                "{field}"
            );
        }
        // An id that is no uuid is refused, whatever it holds.
        let mut bad = win(A, 1, (0, 0, 5, 5));
        bad["id"] = "{x\"); //}".into();
        assert_eq!(
            parse_message(&message(vec![bad], &[], "")),
            Err(ParseError::BadId)
        );
        assert_eq!(
            parse_message(&message(
                vec![win(A, 1, (0, 0, 5, 5)), win(A, 1, (0, 0, 5, 5))],
                &[],
                ""
            )),
            Err(ParseError::Duplicate)
        );
        for rect in [
            (0, 0, -1, 5),
            (0, 0, 5, -1),
            (2_000_000, 0, 5, 5),
            (0, 0, 5_000_000, 5),
        ] {
            assert_eq!(
                parse_message(&message(vec![win(A, 1, rect)], &[], "")),
                Err(ParseError::BadGeometry),
                "{rect:?}"
            );
        }
        let many: Vec<_> = (0..=MOST_WINDOWS)
            .map(|i| {
                win(
                    &format!("00000000-0000-4000-8000-{i:012x}"),
                    1,
                    (0, 0, 5, 5),
                )
            })
            .collect();
        assert_eq!(
            parse_message(&message(many, &[], "")),
            Err(ParseError::TooMany)
        );
        let big = format!(
            "{{\"v\":1,\"windows\":[],\"pad\":\"{}\"}}",
            "x".repeat(MAX_MESSAGE)
        );
        assert_eq!(parse_message(&big), Err(ParseError::TooLarge));
    }

    #[test]
    fn what_the_message_names_but_does_not_list_is_read_as_none() {
        let mut w = win(A, 1, (0, 0, 5, 5));
        w["parent"] = format!("{{{B}}}").into();
        let json = message(vec![w], &[A, B, "not a uuid"], B);
        let s = table_of(&json, Instant::now());
        assert_eq!(s.active, None, "an active window that is not listed");
        assert_eq!(s.windows[0].parent, None, "a parent that is not listed");
        assert_eq!(s.windows[0].z, Some(0));
    }

    #[test]
    fn a_title_keeps_no_control_or_direction_characters_and_is_cut() {
        let mut w = win(A, 1, (0, 0, 5, 5));
        w["caption"] = "a\u{202e}b\u{0007}c\nd\u{200b}e\u{2066}f".into();
        w["class"] = "x".repeat(500).into();
        let s = table_of(&message(vec![w], &[A], ""), Instant::now());
        assert_eq!(s.windows[0].caption, "abcdef");
        assert_eq!(s.windows[0].class.chars().count(), MOST_CLASS);
        let mut long = win(B, 1, (0, 0, 5, 5));
        long["caption"] = "y".repeat(500).into();
        let s = table_of(&message(vec![long], &[B], ""), Instant::now());
        assert_eq!(s.windows[0].caption.chars().count(), MOST_CAPTION);
    }

    // ---- Staleness ----

    #[test]
    fn a_stale_table_answers_no_to_everything() {
        let then = Instant::now();
        let s = desk(then);
        let a = seq(&s, A);
        let inside = (120, 120);
        let fresh = then + STALE_AFTER - Duration::from_millis(1);
        assert!(s.frame_of(a, fresh).is_some());
        assert!(s.front_is_lent(a, fresh));
        assert!(s.hit_is_lent(a, inside, fresh));
        let old = then + STALE_AFTER;
        assert_eq!(s.frame_of(a, old), None);
        assert!(!s.front_is_lent(a, old));
        assert!(!s.hit_is_lent(a, inside, old));
        assert!(!s.is_fresh(old));
        // A clock that went back is not "fresh for ever": it is read as now.
        assert!(s.is_fresh(then - Duration::from_secs(1)));
    }

    // ---- The rules ----

    #[test]
    fn the_frame_is_the_lent_windows_while_it_is_shown_and_not_otherwise() {
        let now = Instant::now();
        let mut s = desk(now);
        let a = seq(&s, A);
        assert_eq!(s.frame_of(a, now), Some((100, 100, 400, 300)));
        assert_eq!(s.frame_of(999, now), None, "no such window");
        let w = s.windows.iter_mut().find(|w| w.seq == a).unwrap();
        w.shown = false;
        assert_eq!(s.frame_of(a, now), None, "on another desktop");
        let w = s.windows.iter_mut().find(|w| w.seq == a).unwrap();
        w.shown = true;
        w.minimized = true;
        assert_eq!(s.frame_of(a, now), None, "minimised");
        let w = s.windows.iter_mut().find(|w| w.seq == a).unwrap();
        w.minimized = false;
        w.frame.2 = 0;
        assert_eq!(s.frame_of(a, now), None, "no size");
    }

    #[test]
    fn the_front_is_the_lent_window_or_one_that_belongs_to_it() {
        let now = Instant::now();
        let mut s = desk(now);
        let (a, b, c, d) = (seq(&s, A), seq(&s, B), seq(&s, C), seq(&s, D));
        assert!(s.front_is_lent(a, now), "the window itself");
        s.active = Some(c);
        assert!(s.front_is_lent(a, now), "its dialog, of the same program");
        s.active = Some(d);
        assert!(
            !s.front_is_lent(a, now),
            "another window of the same program"
        );
        s.active = Some(b);
        assert!(!s.front_is_lent(a, now), "another program's window");
        s.active = None;
        assert!(!s.front_is_lent(a, now), "nothing is active");
        s.active = Some(999);
        assert!(!s.front_is_lent(a, now), "an active window the table lacks");
    }

    #[test]
    fn a_dialog_of_another_program_is_not_the_lent_windows_even_with_it_as_parent() {
        let now = Instant::now();
        let mut c = win(C, 99, (150, 150, 100, 100));
        c["parent"] = format!("{{{A}}}").into();
        let s = table_of(
            &message(vec![win(A, 10, (0, 0, 400, 300)), c], &[A, C], C),
            now,
        );
        assert!(!s.front_is_lent(seq(&s, A), now));
        assert!(!s.hit_is_lent(seq(&s, A), (160, 160), now));
    }

    #[test]
    fn a_window_with_no_program_is_never_the_lent_ones_front() {
        let now = Instant::now();
        let s = table_of(&message(vec![win(A, 0, (0, 0, 400, 300))], &[A], A), now);
        assert!(!s.front_is_lent(seq(&s, A), now), "no pid, so no proof");
    }

    #[test]
    fn a_dialog_chain_is_followed_a_few_owners_deep_and_a_loop_ends() {
        let now = Instant::now();
        let mut c = win(C, 10, (0, 0, 10, 10));
        c["parent"] = format!("{{{D}}}").into();
        let mut d = win(D, 10, (0, 0, 10, 10));
        d["parent"] = format!("{{{A}}}").into();
        let s = table_of(
            &message(vec![win(A, 10, (0, 0, 400, 300)), c, d], &[A, D, C], C),
            now,
        );
        assert!(s.front_is_lent(seq(&s, A), now), "a dialog of a dialog");
        let mut a = win(A, 10, (0, 0, 400, 300));
        a["parent"] = format!("{{{B}}}").into();
        let mut b = win(B, 10, (0, 0, 10, 10));
        b["parent"] = format!("{{{A}}}").into();
        // A and B are each other's parent: asking whether A is in front of
        // the lent C follows the loop a few steps and says no.
        let looped = table_of(
            &message(vec![a, b, win(C, 10, (0, 0, 5, 5))], &[A, B, C], A),
            now,
        );
        assert!(!looped.front_is_lent(seq(&looped, C), now));
    }

    #[test]
    fn a_point_is_the_lent_windows_only_where_nothing_covers_it() {
        let now = Instant::now();
        let s = desk(now);
        let a = seq(&s, A);
        assert!(s.hit_is_lent(a, (300, 200), now), "plain window");
        assert!(
            s.hit_is_lent(a, (160, 160), now),
            "its own dialog is over it"
        );
        assert!(
            !s.hit_is_lent(a, (460, 360), now),
            "another program's window covers"
        );
        assert!(
            !s.hit_is_lent(a, (600, 500), now),
            "outside the lent window"
        );
        assert!(
            !s.hit_is_lent(a, (10, 10), now),
            "another window of the program"
        );
        assert!(
            !s.hit_is_lent(a, (500, 300), now),
            "the frame's far edge is outside"
        );
        assert!(s.hit_is_lent(a, (499, 300), now));
    }

    #[test]
    fn a_covering_window_that_is_not_shown_does_not_cover() {
        let now = Instant::now();
        let mut s = desk(now);
        let a = seq(&s, A);
        let b = seq(&s, B);
        s.windows.iter_mut().find(|w| w.seq == b).unwrap().shown = false;
        assert!(s.hit_is_lent(a, (460, 360), now), "on another desktop");
        let w = s.windows.iter_mut().find(|w| w.seq == b).unwrap();
        w.shown = true;
        w.minimized = true;
        assert!(s.hit_is_lent(a, (460, 360), now), "minimised");
    }

    #[test]
    fn a_window_the_stacking_order_does_not_name_is_taken_to_cover() {
        let now = Instant::now();
        let mut s = desk(now);
        let a = seq(&s, A);
        let b = seq(&s, B);
        s.windows.iter_mut().find(|w| w.seq == b).unwrap().z = None;
        assert!(!s.hit_is_lent(a, (460, 360), now));
        // Away from it the lent window is still found.
        assert!(s.hit_is_lent(a, (300, 200), now));
    }

    #[test]
    fn the_top_of_the_stacking_order_wins_not_the_order_of_the_list() {
        let now = Instant::now();
        // B listed first but stacked under A: A's click.
        let s = table_of(
            &message(
                vec![win(B, 20, (0, 0, 100, 100)), win(A, 10, (0, 0, 100, 100))],
                &[B, A],
                A,
            ),
            now,
        );
        assert!(s.hit_is_lent(seq(&s, A), (50, 50), now));
        let s = table_of(
            &message(
                vec![win(B, 20, (0, 0, 100, 100)), win(A, 10, (0, 0, 100, 100))],
                &[A, B],
                A,
            ),
            now,
        );
        assert!(!s.hit_is_lent(seq(&s, A), (50, 50), now));
    }

    #[test]
    fn a_panel_over_a_corner_covers_it() {
        let now = Instant::now();
        let panel = win(D, 5, (0, 380, 1000, 40));
        let s = table_of(
            &message(vec![win(A, 10, (100, 100, 400, 300)), panel], &[A, D], A),
            now,
        );
        let a = seq(&s, A);
        assert!(s.hit_is_lent(a, (300, 200), now));
        assert!(!s.hit_is_lent(a, (300, 390), now));
    }

    // ---- The guard over the rules ----

    /// What the guard's Os reads in the test: a table and a clock the test
    /// moves, and the raises asked for.
    struct Shared {
        snapshot: RefCell<Snapshot>,
        clock: Cell<Instant>,
        seq: u64,
        raised: RefCell<Vec<u64>>,
    }

    /// The table's rules as the guard's Os.
    struct Over(Rc<Shared>);

    impl Os for Over {
        fn lease_holds(&self, _lease: &Lease) -> bool {
            true
        }
        fn frame(&self, _lease: &Lease) -> Option<Rect> {
            let me = &self.0;
            me.snapshot.borrow().frame_of(me.seq, me.clock.get())
        }
        fn front_is_lent(&self, _lease: &Lease) -> bool {
            let me = &self.0;
            me.snapshot.borrow().front_is_lent(me.seq, me.clock.get())
        }
        fn hit_is_lent(&self, _lease: &Lease, at: (i32, i32)) -> bool {
            let me = &self.0;
            me.snapshot.borrow().hit_is_lent(me.seq, at, me.clock.get())
        }
        fn raise(&self, _lease: &Lease) {
            self.0.raised.borrow_mut().push(self.0.seq);
        }
    }

    fn lease_for(seq: u64) -> Lease {
        Lease {
            index: 1000,
            hwnd: 7,
            generation: 1,
            identity: Default::default(),
            device: "device".to_string(),
            native: kde_native(seq),
        }
    }

    fn pressed(button_down: bool) -> InputPacket {
        InputPacket::MouseClick {
            button: 0,
            pressed: button_down,
        }
    }

    fn presses(out: &crate::streaming::window_guard::Out) -> usize {
        out.send
            .iter()
            .filter(|p| matches!(p, InputPacket::MouseClick { pressed: true, .. }))
            .count()
    }

    #[test]
    fn the_guard_over_the_kde_table_sends_a_click_only_where_the_window_is_in_front() {
        let now = Instant::now();
        let snapshot = desk(now);
        let a = seq(&snapshot, A);
        let b = seq(&snapshot, B);
        let shared = Rc::new(Shared {
            snapshot: RefCell::new(snapshot),
            clock: Cell::new(now),
            seq: a,
            raised: RefCell::new(Vec::new()),
        });
        let lease = lease_for(a);
        // The layout is the screen: 1000x700 at the origin.
        let mut guard = WindowGuard::new(lease, Over(shared.clone()), Some((0, 0, 1000, 700)));
        let middle = InputPacket::MouseMoveAbs {
            x_norm: 32768,
            y_norm: 32768,
        };
        // The middle of the lent window, nothing over it: the click goes.
        assert_eq!(guard.admit(middle.clone(), now).send.len(), 1);
        let out = guard.admit(pressed(true), now);
        assert!(!out.end);
        assert_eq!(presses(&out), 1);
        assert_eq!(
            guard.admit(pressed(false), now).send.len(),
            1,
            "its release"
        );
        // Another program's window comes over the whole screen: the pointer
        // is not moved there and a click is not sent.
        shared
            .snapshot
            .borrow_mut()
            .windows
            .iter_mut()
            .find(|w| w.seq == b)
            .unwrap()
            .frame = (0, 0, 1000, 700);
        assert!(guard.admit(middle, now).send.is_empty());
        assert_eq!(presses(&guard.admit(pressed(true), now)), 0);
        // Another window active: a key asks for a raise once, and with the
        // table still saying it is not in front, is dropped.
        shared.snapshot.borrow_mut().active = Some(b);
        let key = guard.admit(
            InputPacket::KeyEvent {
                key: 30,
                pressed: true,
            },
            now,
        );
        assert!(key.send.is_empty());
        assert_eq!(shared.raised.borrow().as_slice(), &[a], "raised once");
        // A table gone stale: nothing is sent at all.
        let later = now + STALE_AFTER + Duration::from_secs(1);
        shared.clock.set(later);
        shared.snapshot.borrow_mut().active = Some(a);
        let moved = guard.admit(
            InputPacket::MouseMoveAbs {
                x_norm: 100,
                y_norm: 100,
            },
            later,
        );
        assert!(moved.send.is_empty());
        let typed = guard.admit(
            InputPacket::Text("x".into()),
            later + Duration::from_secs(1),
        );
        assert!(typed.send.is_empty());
    }

    // ---- The global table ----

    #[test]
    fn the_table_the_server_publishes_is_the_one_the_guards_os_reads() {
        // The table is the process's: the tests that publish take turns.
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        assert!(snapshot().is_none());
        assert!(publish("nonsense").is_err());
        assert!(snapshot().is_none(), "a refused message changes nothing");
        let updates = updates();
        let json = message(
            vec![
                win(A, 10, (100, 100, 400, 300)),
                win(B, 20, (450, 350, 200, 200)),
            ],
            &[A, B],
            A,
        );
        publish(&json).unwrap();
        assert!(updates.has_changed().unwrap());
        let s = snapshot().expect("a table");
        assert!(s.is_fresh(Instant::now()));
        let a = seq(&s, A);
        let lease = lease_for(a);
        let os = KdeOs;
        assert_eq!(os.frame(&lease), Some((100, 100, 400, 300)));
        assert!(os.front_is_lent(&lease));
        assert!(os.hit_is_lent(&lease, (150, 150)));
        assert!(!os.hit_is_lent(&lease, (460, 360)));
        // A lease that is not a KDE one has no answers.
        let mut other = lease.clone();
        other.native = "gnome:5".to_string();
        assert_eq!(os.frame(&other), None);
        assert!(!os.front_is_lent(&other));
        assert!(!os.hit_is_lent(&other, (150, 150)));
        assert_eq!(uuid_of(a).as_deref(), Some(A));
        // The lease does not stand (nothing is registered under it).
        assert!(!os.lease_holds(&lease));
        clear();
        assert_eq!(os.frame(&lease), None, "no table, no answer");
    }

    #[test]
    fn a_raise_is_asked_of_the_script_with_its_next_report_and_only_for_a_window_the_table_has() {
        // One test owns the queue and the poker (process-wide).
        let poked = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = poked.clone();
        set_poker(Some(Arc::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })));
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        take_raises();
        clear();
        request_raise(424_242);
        assert!(take_raises().is_empty(), "a window the table has not");
        let json = message(vec![win(D, 10, (0, 0, 5, 5))], &[D], D);
        publish(&json).unwrap();
        let d = seq(&snapshot().unwrap(), D);
        request_raise(d);
        request_raise(d);
        assert_eq!(take_raises(), vec![D.to_string()], "asked once");
        assert!(poked.load(std::sync::atomic::Ordering::Relaxed) >= 1);
        assert!(take_raises().is_empty(), "taken");
        for i in 0..8u64 {
            // Not windows of the table: nothing queues.
            request_raise(1000 + i);
        }
        assert!(take_raises().is_empty());
        set_poker(None);
        clear();
    }

    // ---- The portal's stream ----

    fn stream(
        node: u32,
        kind: StreamKind,
        size: Option<(i32, i32)>,
        names: &[&str],
    ) -> PortalStream {
        PortalStream {
            node,
            kind,
            size,
            names: names.iter().map(|n| n.to_string()).collect(),
        }
    }

    fn lent_of(s: &Snapshot) -> KdeWindow {
        s.window(seq(s, A)).unwrap().clone()
    }

    #[test]
    fn the_stream_of_the_lent_windows_size_is_taken_when_no_other_window_has_it() {
        let now = Instant::now();
        let s = desk(now);
        let lent = lent_of(&s);
        let good = stream(41, StreamKind::Window, Some((400, 300)), &[]);
        assert_eq!(match_stream(&[good], &lent, &s), Ok(41));
        let near = stream(42, StreamKind::Window, Some((402, 298)), &[]);
        assert_eq!(match_stream(&[near], &lent, &s), Ok(42));
        let off = stream(43, StreamKind::Window, Some((403, 300)), &[]);
        assert_eq!(match_stream(&[off], &lent, &s), Err(Mismatch::WrongSize));
    }

    #[test]
    fn a_stream_that_cannot_be_shown_to_be_the_lent_window_is_refused() {
        let now = Instant::now();
        let s = desk(now);
        let lent = lent_of(&s);
        assert_eq!(match_stream(&[], &lent, &s), Err(Mismatch::NoStream));
        let two = [
            stream(1, StreamKind::Window, Some((400, 300)), &[]),
            stream(2, StreamKind::Window, Some((400, 300)), &[]),
        ];
        assert_eq!(match_stream(&two, &lent, &s), Err(Mismatch::SeveralStreams));
        for kind in [
            StreamKind::Monitor,
            StreamKind::Virtual,
            StreamKind::Unknown,
        ] {
            assert_eq!(
                match_stream(&[stream(1, kind, Some((400, 300)), &[])], &lent, &s),
                Err(Mismatch::NotAWindow),
                "{kind:?}"
            );
        }
        for size in [None, Some((0, 0)), Some((-1, 300))] {
            assert_eq!(
                match_stream(&[stream(1, StreamKind::Window, size, &[])], &lent, &s),
                Err(Mismatch::SizeUnknown),
                "{size:?}"
            );
        }
        // A window of the same size elsewhere: the stream could be either.
        let twin = table_of(
            &message(
                vec![win(A, 10, (0, 0, 400, 300)), win(B, 20, (500, 0, 400, 300))],
                &[A, B],
                A,
            ),
            now,
        );
        let lent = lent_of(&twin);
        assert_eq!(
            match_stream(
                &[stream(1, StreamKind::Window, Some((400, 300)), &[])],
                &lent,
                &twin
            ),
            Err(Mismatch::Ambiguous)
        );
    }

    #[test]
    fn a_stream_that_names_the_window_is_that_window_whatever_its_size() {
        let now = Instant::now();
        let twin = table_of(
            &message(
                vec![win(A, 10, (0, 0, 400, 300)), win(B, 20, (500, 0, 400, 300))],
                &[A, B],
                A,
            ),
            now,
        );
        let lent = lent_of(&twin);
        let named = stream(
            9,
            StreamKind::Window,
            Some((390, 290)),
            &[&format!("{{{A}}}")],
        );
        assert_eq!(
            match_stream(&[named], &lent, &twin),
            Ok(9),
            "named, so no tie"
        );
        let other = stream(9, StreamKind::Window, Some((400, 300)), &[B]);
        assert_eq!(
            match_stream(&[other], &lent, &twin),
            Err(Mismatch::Another),
            "it names another window of the table"
        );
        // A name that is no uuid says nothing: the size decides (and ties).
        let noise = stream(9, StreamKind::Window, Some((400, 300)), &["window-1", ""]);
        assert_eq!(
            match_stream(&[noise], &lent, &twin),
            Err(Mismatch::Ambiguous)
        );
        // Named for a window the table has not: no help, no harm.
        let s = desk(now);
        let lent = lent_of(&s);
        let stranger = stream(
            5,
            StreamKind::Window,
            Some((400, 300)),
            &["00000000-0000-4000-8000-0000000000ff"],
        );
        assert_eq!(match_stream(&[stranger], &lent, &s), Ok(5));
    }

    // ---- The portal's stream, in device pixels ----

    /// A window with its client area and the scale of its output.
    fn scaled(
        id: &str,
        frame: (i64, i64, i64, i64),
        client: (i64, i64),
        scale: f64,
    ) -> serde_json::Value {
        let mut w = win(id, 10, frame);
        w["cw"] = client.0.into();
        w["ch"] = client.1.into();
        w["scale"] = scale.into();
        w
    }

    fn only(w: serde_json::Value) -> (Snapshot, KdeWindow) {
        let s = table_of(&message(vec![w], &[A], A), Instant::now());
        let lent = s.windows[0].clone();
        (s, lent)
    }

    fn of_size(width: i32, height: i32) -> [PortalStream; 1] {
        [stream(7, StreamKind::Window, Some((width, height)), &[])]
    }

    #[test]
    fn the_scale_is_read_in_thousandths_and_nonsense_is_not_known() {
        for (scale, milli) in [
            (1.0, Some(1000)),
            (1.25, Some(1250)),
            (1.5, Some(1500)),
            (2.0, Some(2000)),
            (0.0, None),
            (-1.0, None),
            (0.1, None),
            (99.0, None),
        ] {
            let (s, _) = only(scaled(A, (0, 0, 100, 100), (100, 100), scale));
            assert_eq!(s.windows[0].scale_milli, milli, "{scale}");
        }
        // No scale at all is not known either.
        let mut none = scaled(A, (0, 0, 100, 100), (100, 100), 1.0);
        none.as_object_mut().unwrap().remove("scale");
        let (s, _) = only(none);
        assert_eq!(s.windows[0].scale_milli, None);
        assert_eq!(s.windows[0].client, (100, 100));
    }

    #[test]
    fn a_stream_is_compared_in_device_pixels_not_logical_ones() {
        // Scale 1: the frame's own size.
        let (s, lent) = only(scaled(A, (0, 0, 800, 600), (800, 600), 1.0));
        assert_eq!(match_stream(&of_size(800, 600), &lent, &s), Ok(7));
        // Scale 1.25: 800x600 logical is 1000x750 device; the logical size
        // is not the picture's size.
        let (s, lent) = only(scaled(A, (0, 0, 800, 600), (800, 600), 1.25));
        assert_eq!(match_stream(&of_size(1000, 750), &lent, &s), Ok(7));
        assert_eq!(
            match_stream(&of_size(800, 600), &lent, &s),
            Err(Mismatch::WrongSize)
        );
        // Scale 1.5: a window of 533x400 is 800x600 device pixels (799.5
        // rounds up), and its logical size is not what a stream has.
        let (s, lent) = only(scaled(A, (0, 0, 533, 400), (533, 400), 1.5));
        assert_eq!(match_stream(&of_size(800, 600), &lent, &s), Ok(7));
        assert_eq!(
            match_stream(&of_size(533, 400), &lent, &s),
            Err(Mismatch::WrongSize)
        );
        // 800x600 logical at 1.5 is 1200x900 device, not 800x600.
        let (s, lent) = only(scaled(A, (0, 0, 800, 600), (800, 600), 1.5));
        assert_eq!(match_stream(&of_size(1200, 900), &lent, &s), Ok(7));
        assert_eq!(
            match_stream(&of_size(800, 600), &lent, &s),
            Err(Mismatch::WrongSize)
        );
    }

    #[test]
    fn a_server_side_title_bar_makes_the_client_area_or_the_frame_the_streams_size() {
        // Frame 800x628 (a 28 pixel title bar), client area 800x600, at 1.5.
        let (s, lent) = only(scaled(A, (0, 0, 800, 628), (800, 600), 1.5));
        assert_eq!(
            match_stream(&of_size(1200, 900), &lent, &s),
            Ok(7),
            "client"
        );
        assert_eq!(match_stream(&of_size(1200, 942), &lent, &s), Ok(7), "frame");
        assert_eq!(
            match_stream(&of_size(1200, 920), &lent, &s),
            Err(Mismatch::WrongSize),
            "neither"
        );
        // At scale 1 as well.
        let (s, lent) = only(scaled(A, (0, 0, 800, 628), (800, 600), 1.0));
        assert_eq!(match_stream(&of_size(800, 600), &lent, &s), Ok(7));
        assert_eq!(match_stream(&of_size(800, 628), &lent, &s), Ok(7));
    }

    #[test]
    fn a_window_whose_device_size_is_the_lent_windows_logical_size_is_a_twin_not_a_match() {
        let now = Instant::now();
        // Lent: 800x600 at scale 1. Another: 533x400 at 1.5, which is also
        // 800x600 device pixels. A stream of 800x600 could be either.
        let both = table_of(
            &message(
                vec![
                    scaled(A, (0, 0, 800, 600), (800, 600), 1.0),
                    scaled(B, (900, 0, 533, 400), (533, 400), 1.5),
                ],
                &[A, B],
                A,
            ),
            now,
        );
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_of(&both), &both),
            Err(Mismatch::Ambiguous)
        );
        // And the other way round: lent is the 533x400 window at 1.5.
        let lent_b = both.window(seq(&both, B)).unwrap().clone();
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_b, &both),
            Err(Mismatch::Ambiguous)
        );
        // Without the twin the same stream is the lent window's.
        let (alone, lent) = only(scaled(A, (0, 0, 533, 400), (533, 400), 1.5));
        assert_eq!(match_stream(&of_size(800, 600), &lent, &alone), Ok(7));
        // A window that is a twin only in logical pixels is not one: 800x600
        // at 1.5 is 1200x900 device, so a stream of 800x600 of the lent
        // 800x600@1 window is not mistaken for it.
        let apart = table_of(
            &message(
                vec![
                    scaled(A, (0, 0, 800, 600), (800, 600), 1.0),
                    scaled(B, (900, 0, 800, 600), (800, 600), 1.5),
                ],
                &[A, B],
                A,
            ),
            now,
        );
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_of(&apart), &apart),
            Ok(7)
        );
        // Another window's title bar can make it a twin too.
        let barred = table_of(
            &message(
                vec![
                    scaled(A, (0, 0, 800, 600), (800, 600), 1.0),
                    scaled(B, (900, 0, 800, 628), (800, 600), 1.0),
                ],
                &[A, B],
                A,
            ),
            now,
        );
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_of(&barred), &barred),
            Err(Mismatch::Ambiguous)
        );
    }

    #[test]
    fn a_scale_that_is_not_known_refuses_unless_the_stream_names_the_window() {
        let now = Instant::now();
        let (s, lent) = only(scaled(A, (0, 0, 800, 600), (800, 600), 0.0));
        assert_eq!(
            match_stream(&of_size(800, 600), &lent, &s),
            Err(Mismatch::ScaleUnknown)
        );
        let named = [stream(
            3,
            StreamKind::Window,
            Some((1, 1)),
            &[&format!("{{{A}}}")],
        )];
        assert_eq!(match_stream(&named, &lent, &s), Ok(3));
        // Another window with no known scale is a twin if it could be one at
        // any scale an output has: 640x480 at 1.25 is 800x600.
        let unknown_twin = table_of(
            &message(
                vec![
                    scaled(A, (0, 0, 800, 600), (800, 600), 1.0),
                    scaled(B, (900, 0, 640, 480), (640, 480), 0.0),
                ],
                &[A, B],
                A,
            ),
            now,
        );
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_of(&unknown_twin), &unknown_twin),
            Err(Mismatch::Ambiguous)
        );
        let unknown_far = table_of(
            &message(
                vec![
                    scaled(A, (0, 0, 800, 600), (800, 600), 1.0),
                    scaled(B, (900, 0, 100, 100), (100, 100), 0.0),
                ],
                &[A, B],
                A,
            ),
            now,
        );
        assert_eq!(
            match_stream(&of_size(800, 600), &lent_of(&unknown_far), &unknown_far),
            Ok(7)
        );
    }

    // ---- What the owner is told ----

    #[test]
    fn the_refusals_are_said_in_the_owners_terms() {
        assert!(say(Refusal::NotWayland).contains("Wayland"));
        assert!(say(Refusal::ScriptRefused).contains("KWin Scripts"));
        assert!(say(Refusal::ScriptSilent).contains("no windows"));
        assert!(say(Refusal::TableStale).contains("stopped"));
        assert!(say(Refusal::NoPortal).contains("screen-sharing portal"));
        assert!(say(Refusal::PortalNoWindows).contains("single window"));
        for mismatch in [
            Mismatch::NoStream,
            Mismatch::SeveralStreams,
            Mismatch::NotAWindow,
            Mismatch::SizeUnknown,
            Mismatch::WrongSize,
            Mismatch::ScaleUnknown,
            Mismatch::Another,
            Mismatch::Ambiguous,
        ] {
            let words = say_mismatch(mismatch);
            assert!(
                words.starts_with("The ") || words.starts_with("Another"),
                "{words}"
            );
            assert!(!words.is_empty());
        }
        assert!(say_mismatch(Mismatch::Ambiguous).contains("same size"));
        assert!(say_mismatch(Mismatch::NotAWindow).contains("a screen"));
        assert!(ParseError::BadId.to_string().contains("uuid"));
    }
}
