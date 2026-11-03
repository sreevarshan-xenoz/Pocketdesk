//! Dragging a window past the edge (window handoff, phase 4): with the
//! keyboard and mouse shared, a window dragged to the edge that leads to
//! another of the owner's computers goes there, like an extended display.
//!
//! What this computer can know, and what it cannot, shapes the design. The
//! input-capture portal says a barrier was hit and then streams libei's
//! events; it never says a button was held when the pointer arrived. What
//! does show is the end of the drag: the left button (and the modifier the
//! compositor moves a window with) going up with no press of it seen in this
//! visit ([`Evidence`]). So a window is handed over when the drag ends, on
//! the other computer, where the pointer is then: the window that was in
//! front at the edge ([`pick`]) is offered to the other computer
//! ([`Offer`]), which opens it as a borrowed view placed where the drag
//! ended ([`place`]), and lends it to itself as any borrow does (the window
//! leaves this screen at that lend, never before: a refused or unanswered
//! offer moves nothing here).
//!
//! Only a floating window is taken: a tiled window has no place of its own
//! to carry across (a drag of one swaps it with another), and a full-screen
//! window fills a screen the other computer cannot match. Only the left
//! button: the compositor's modifier with the right button resizes. With
//! the trigger `mod` (the default) the compositor's modifier must have been
//! held with the button, so a text selection or a file being dragged, which
//! also ends with a button going up, never takes a window; `any` takes any
//! left-button drag that reaches the edge from a floating window.
//!
//! Back again: the window shown there is dragged back over the edge (the
//! left button held, and the modifier, as it left): the window goes home,
//! placed where the pointer comes in, and the view of it ends.
//!
//! The rules are pure ([`Rules`], [`Evidence`], [`pick`], [`place`]) and
//! tried against fixtures of both compositors' windows; the compositors are
//! read in [`candidates_now`], the other computer is told in [`hand_over`],
//! and the computer that is told opens the window in [`note_offer`].

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

use super::wire::Screen;

// ---- The rules ----

/// What marks a drag as a window being moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trigger {
    /// The compositor's modifier (Super or Alt) was held with the button.
    #[default]
    Modifier,
    /// Any left-button drag that reaches the edge from a floating window.
    Any,
}

impl Trigger {
    /// The setting's word (`drag_window_trigger`): anything but `any` is
    /// the modifier, the stricter.
    pub fn named(word: &str) -> Trigger {
        match word.trim().to_ascii_lowercase().as_str() {
            "any" => Trigger::Any,
            _ => Trigger::Modifier,
        }
    }
}

/// Whether windows are taken across, and how a drag is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rules {
    pub on: bool,
    pub trigger: Trigger,
}

impl Rules {
    /// From the settings: it needs `lend_windows` too, as the other
    /// computer's borrow is a lend.
    pub fn from_settings(drag_windows_across: bool, lend_windows: bool, trigger: &str) -> Rules {
        Rules {
            on: drag_windows_across && lend_windows,
            trigger: Trigger::named(trigger),
        }
    }
}

/// The wire's left button, the only one that moves a window.
pub const LEFT: u8 = 0;

/// Linux's key codes for the keys compositors move windows with.
const MODIFIERS: [u16; 4] = [
    56,  // KEY_LEFTALT
    100, // KEY_RIGHTALT
    125, // KEY_LEFTMETA
    126, // KEY_RIGHTMETA
];

/// Whether the key with this evdev code is a modifier a move is bound to.
pub fn is_modifier(code: u16) -> bool {
    MODIFIERS.contains(&code)
}

/// How long, after the left button went up, the modifier may still be
/// waited for (it is let go of a moment after the button, or before it).
pub const MODIFIER_GRACE: Duration = Duration::from_millis(500);

/// What a visit has shown of a drag that was under way as the pointer
/// crossed: a button or modifier let go of that this computer never saw go
/// down. It is read once a visit: handed over or let go of, it is spent.
#[derive(Debug, Default, Clone)]
pub struct Evidence {
    button: bool,
    since: Option<Instant>,
    modifier: bool,
    spent: bool,
}

impl Evidence {
    /// A new visit.
    pub fn reset(&mut self) {
        *self = Evidence::default();
    }

    /// A button went up that was never seen down. Another than the left
    /// (the right one resizes) spoils the visit's drag.
    pub fn button_up(&mut self, button: u8) {
        if button == LEFT {
            self.button = true;
        } else {
            self.spent = true;
        }
    }

    /// A modifier went up that was never seen down.
    pub fn modifier_up(&mut self) {
        self.modifier = true;
    }

    /// Whether `trigger` is met: the left button, and, for the modifier
    /// trigger, the modifier.
    pub fn ready(&self, trigger: Trigger) -> bool {
        !self.spent && self.button && (trigger == Trigger::Any || self.modifier)
    }

    /// The drag is dealt with: nothing more is read of this visit.
    pub fn spend(&mut self) {
        self.spent = true;
    }

    /// Whether the visit's drag is dealt with.
    pub fn is_spent(&self) -> bool {
        self.spent
    }

    /// The time is `now`: a button let go of that no modifier followed
    /// within [`MODIFIER_GRACE`] was no window being moved.
    pub fn tick(&mut self, now: Instant) {
        if self.spent || !self.button {
            return;
        }
        let since = *self.since.get_or_insert(now);
        if now.saturating_duration_since(since) >= MODIFIER_GRACE && !self.modifier {
            self.spent = true;
        }
    }
}

/// Whether the held keys (`holds`) include a modifier a move is bound to.
pub fn modifier_held(holds: impl Fn(u16) -> bool) -> bool {
    MODIFIERS.iter().any(|code| holds(*code))
}

// ---- Which window ----

/// A window as the compositor shows it, for the pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Sway's container id or Hyprland's address, as a lend names it.
    pub id: String,
    pub title: String,
    pub floating: bool,
    pub focused: bool,
    pub fullscreen: bool,
    /// On a workspace no screen shows (Sway's scratchpad, Hyprland's
    /// special ones) or not mapped.
    pub hidden: bool,
    /// Its place and size as the compositor draws it (a Sway container's
    /// title bar and border included), in the layout's coordinates.
    pub rect: [i32; 4],
}

/// The window a drag holds, and where the pointer held it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub id: String,
    pub title: String,
    pub rect: [i32; 4],
    /// Where the pointer was in the window as it crossed: the window is put
    /// down with the pointer at the same place in it.
    pub grab: (i32, i32),
}

/// Why no window was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// No window has the focus.
    NoFocus,
    /// The window in front is tiled.
    Tiled,
    /// The window in front fills its screen.
    Fullscreen,
    /// The window in front is not where the pointer crossed.
    NotUnderPointer,
}

/// How far from the window's edge a crossing still counts as being on it:
/// the pointer is held at the screen's last pixel, a window dragged to the
/// edge may end a few pixels short of it.
pub const SLACK: i32 = 8;

/// The window a drag that crossed at `crossed_at` (this computer's own
/// desktop space) holds: the one with the focus, if it floats, is not full
/// screen and the pointer is on it.
pub fn pick(windows: &[Candidate], crossed_at: (i32, i32)) -> Result<Picked, Why> {
    let front = windows
        .iter()
        .find(|w| w.focused && !w.hidden)
        .ok_or(Why::NoFocus)?;
    if front.fullscreen {
        return Err(Why::Fullscreen);
    }
    if !front.floating {
        return Err(Why::Tiled);
    }
    let [x, y, w, h] = front.rect;
    let (px, py) = (i64::from(crossed_at.0), i64::from(crossed_at.1));
    let slack = i64::from(SLACK);
    let inside = |p: i64, start: i32, len: i32| {
        p >= i64::from(start) - slack && p < i64::from(start) + i64::from(len) + slack
    };
    if w <= 0 || h <= 0 || !inside(px, x, w) || !inside(py, y, h) {
        return Err(Why::NotUnderPointer);
    }
    Ok(Picked {
        id: front.id.clone(),
        title: front.title.clone(),
        rect: front.rect,
        grab: (
            (crossed_at.0 - x).clamp(0, w.saturating_sub(1)),
            (crossed_at.1 - y).clamp(0, h.saturating_sub(1)),
        ),
    })
}

// ---- Where it lands ----

/// Where a window of `size` goes on `screen` for the pointer at `at` to be
/// where `grab` says in it, its top left kept on the screen (the window
/// slid in as far as it must, never off a side it fits inside; one larger
/// than the screen has its top left at the screen's).
pub fn place(screen: &Screen, at: (i32, i32), grab: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    let fit = |want: i64, start: i32, room: u32, len: i32| -> i32 {
        let lo = i64::from(start);
        let hi = lo + i64::from(room) - i64::from(len.max(1));
        let v = if hi < lo { lo } else { want.clamp(lo, hi) };
        i32::try_from(v).unwrap_or(start)
    };
    (
        fit(
            i64::from(at.0) - i64::from(grab.0),
            screen.x,
            screen.width,
            size.0,
        ),
        fit(
            i64::from(at.1) - i64::from(grab.1),
            screen.y,
            screen.height,
            size.1,
        ),
    )
}

// ---- The compositors ----

/// A Hyprland window of `j/clients`, with what the pick reads.
#[derive(Debug, Deserialize)]
struct HyprClient {
    address: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    at: [i32; 2],
    #[serde(default)]
    size: [i32; 2],
    #[serde(default)]
    fullscreen: i32,
    #[serde(default)]
    workspace: crate::hyprland::HyprWorkspaceRef,
}

/// Hyprland's windows (`j/clients`) with the focus from `j/activewindow`.
pub fn candidates_from_hypr(clients: &str, active: &str) -> Vec<Candidate> {
    let Ok(clients) = serde_json::from_str::<Vec<HyprClient>>(clients) else {
        return Vec::new();
    };
    let active = serde_json::from_str::<Value>(active)
        .ok()
        .and_then(|v| v.get("address").and_then(Value::as_str).map(str::to_string));
    clients
        .into_iter()
        .map(|c| Candidate {
            focused: active.as_deref() == Some(c.address.as_str()),
            id: c.address,
            title: c.title,
            floating: c.floating,
            fullscreen: c.fullscreen >= 2,
            hidden: !c.mapped || c.hidden || c.workspace.name.starts_with("special"),
            rect: [c.at[0], c.at[1], c.size[0], c.size[1]],
        })
        .collect()
}

/// Sway's windows from a `get_tree` reply.
pub fn candidates_from_sway(tree: &Value) -> Vec<Candidate> {
    crate::sway_tree::windows(tree)
        .into_iter()
        .map(|w| Candidate {
            id: w.id.to_string(),
            title: w.title,
            floating: w.floating,
            focused: w.focused,
            fullscreen: w.fullscreen != 0,
            hidden: w.workspace.starts_with("__i3"),
            rect: [w.origin[0], w.origin[1], w.frame[0], w.frame[1]],
        })
        .collect()
}

/// The windows of the compositor this server runs in. Blocking; an error
/// when it is neither Sway nor Hyprland, or does not answer.
pub fn candidates_now() -> std::io::Result<Vec<Candidate>> {
    use crate::virtual_screen::{Desk, desk};
    match desk() {
        Some(Desk::Sway) => Ok(candidates_from_sway(&crate::sway_tree::tree()?)),
        Some(Desk::Hyprland) => Ok(candidates_from_hypr(
            &crate::virtual_screen::hypr("j/clients")?,
            &crate::virtual_screen::hypr("j/activewindow")?,
        )),
        None => Err(std::io::Error::other("not running under Sway or Hyprland")),
    }
}

// ---- The offer ----

/// The key of the answer-shaped virtual-screen packet that offers a window:
/// the body is `{drag_window: {window, title, x, y, width, height}}`, no
/// `action`, so it is heard before any permission and gated by the computer
/// that hears it ([`note_offer`]).
pub const OFFER: &str = "drag_window";

/// The most a title is cut to, as the window list's.
const TITLE_CHARS: usize = 200;

/// A window offered to the computer the pointer went to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The window's id on the computer that offers it.
    pub window: String,
    pub title: String,
    /// Where its top left goes, in the receiving computer's own desktop
    /// space, and its size.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The longest side an offered window may have.
const MOST_SIDE: u32 = 16384;

impl Offer {
    /// The packet body.
    pub fn body(&self) -> Value {
        json!({ OFFER: {
            "window": self.window,
            "title": self.title,
            "x": self.x,
            "y": self.y,
            "width": self.width,
            "height": self.height,
        }})
    }

    /// The offer in `body`, if it is one, read as strictly as an ask is: a
    /// window id that may reach a command, a title without control
    /// characters, sizes and places a screen can have.
    pub fn read(body: &Value) -> Option<Offer> {
        let o = body.get(OFFER)?;
        let window = o.get("window")?.as_str()?;
        if !crate::lend_window::is_window_id(window) {
            return None;
        }
        let num = |key: &str| o.get(key).and_then(Value::as_i64);
        let side = |key: &str| {
            u32::try_from(num(key)?)
                .ok()
                .filter(|v| (64..=MOST_SIDE).contains(v))
        };
        let spot = |key: &str| {
            i32::try_from(num(key)?)
                .ok()
                .filter(|v| v.unsigned_abs() <= 1_000_000)
        };
        Some(Offer {
            window: window.to_string(),
            title: clean_title(o.get("title").and_then(Value::as_str).unwrap_or("")),
            x: spot("x")?,
            y: spot("y")?,
            width: side("width")?,
            height: side("height")?,
        })
    }
}

/// `text` without control, bidirectional and other format characters (a
/// title is another program's, and is put before the owner), at most
/// [`TITLE_CHARS`] characters.
fn clean_title(text: &str) -> String {
    text.chars()
        .filter(|c| !pocketdesk_core::local::shared_input::is_hidden(*c))
        .take(TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

// ---- Handing a window over ----

/// What a drag that ended on the other computer says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    /// The computer the pointer went to.
    pub to: String,
    /// Where the pointer crossed, in this computer's desktop space.
    pub crossed_at: (i32, i32),
    /// Where the drag ended, in the other computer's.
    pub drop_at: (i32, i32),
    /// The other computer's screen the drag ended on.
    pub screen: Screen,
}

/// A window handed over and not yet taken back.
#[derive(Debug, Clone)]
struct Dragged {
    window: String,
    grab: (i32, i32),
    size: (i32, i32),
    at: Instant,
}

/// How long a window handed over may be dragged back: after this the
/// ordinary rules of a lend (its borrower, the lease) are all that holds it.
const BACK_WITHIN: Duration = Duration::from_secs(6 * 3600);

/// What was handed over, by the computer it went to.
fn dragged() -> &'static Mutex<HashMap<String, Dragged>> {
    static DRAGGED: std::sync::LazyLock<Mutex<HashMap<String, Dragged>>> =
        std::sync::LazyLock::new(Default::default);
    &DRAGGED
}

/// The window handed to `device`, taken off the list (once).
fn take_dragged(device: &str) -> Option<Dragged> {
    let found = dragged()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device)?;
    (found.at.elapsed() < BACK_WITHIN).then_some(found)
}

/// What deciding a handover needs of this computer.
trait Source: Sync {
    /// Whether the other computer may borrow a window (the lend's gate).
    fn may_lend_to(&self, device: &str) -> Result<(), &'static str>;
    fn candidates(&self) -> std::io::Result<Vec<Candidate>>;
}

/// What a handover came to, for the log (never a title).
#[derive(Debug, PartialEq, Eq)]
enum Decided {
    Refused(&'static str),
    Nothing(Why),
    Unreadable,
    Offer(Offer, Picked),
}

/// The offer a drag that ended is to make: the lend's gate, the window in
/// front, and where it lands.
fn decide(env: &dyn Source, h: &Handover) -> Decided {
    if let Err(why) = env.may_lend_to(&h.to) {
        return Decided::Refused(why);
    }
    let Ok(windows) = env.candidates() else {
        return Decided::Unreadable;
    };
    let picked = match pick(&windows, h.crossed_at) {
        Ok(picked) => picked,
        Err(why) => return Decided::Nothing(why),
    };
    let size = (picked.rect[2], picked.rect[3]);
    let (x, y) = place(&h.screen, h.drop_at, picked.grab, size);
    let offer = Offer {
        window: picked.id.clone(),
        title: clean_title(&picked.title),
        x,
        y,
        width: u32::try_from(size.0).unwrap_or(0),
        height: u32::try_from(size.1).unwrap_or(0),
    };
    Decided::Offer(offer, picked)
}

struct Live;

impl Source for Live {
    fn may_lend_to(&self, device: &str) -> Result<(), &'static str> {
        crate::lend_window::may_lend(device)
    }

    fn candidates(&self) -> std::io::Result<Vec<Candidate>> {
        candidates_now()
    }
}

/// A drag ended on the computer `h.to`: pick the window it held, and offer
/// it there. Nothing moves here: the window leaves this screen when the
/// other computer borrows it. Off the input loop (the compositor is read).
pub async fn hand_over(h: Handover) {
    let held = h.clone();
    let decided = tokio::task::spawn_blocking(move || decide(&Live, &held))
        .await
        .unwrap_or(Decided::Unreadable);
    let (offer, picked) = match decided {
        Decided::Offer(offer, picked) => (offer, picked),
        Decided::Refused(why) => {
            tracing::debug!("Shared input: a dragged window stays: {why}");
            return;
        }
        Decided::Nothing(why) => {
            tracing::debug!("Shared input: no window to hand over: {why:?}");
            return;
        }
        Decided::Unreadable => {
            tracing::debug!("Shared input: the compositor's windows could not be read");
            return;
        }
    };
    let packet = NetworkPacket::new(crate::virtual_screen::PACKET).with_body(offer.body());
    if !crate::remote_devices::send_to_computer(&h.to, packet).await {
        tracing::debug!("Shared input: a dragged window could not be offered: no link");
        return;
    }
    tracing::info!("Shared input: a dragged window is offered to the computer the pointer is on");
    dragged().lock().unwrap_or_else(|e| e.into_inner()).insert(
        h.to,
        Dragged {
            window: picked.id,
            grab: picked.grab,
            size: (picked.rect[2], picked.rect[3]),
            at: Instant::now(),
        },
    );
}

/// The pointer came back over the edge from `from` with the window's
/// button held (and its modifier, as the rules ask), at `at` on this
/// computer's `screen`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Return {
    pub from: String,
    pub at: (i32, i32),
    pub screen: Screen,
}

/// The window handed to `r.from` goes home, with the pointer where it comes
/// in at the same place in it, and the view of it there ends.
pub async fn bring_back(r: Return) {
    let Some(was) = take_dragged(&r.from) else {
        tracing::debug!("Shared input: nothing was handed over there to bring back");
        return;
    };
    let (x, y) = place(&r.screen, r.at, was.grab, was.size);
    let (device, window) = (r.from.clone(), was.window.clone());
    let back = tokio::task::spawn_blocking(move || {
        crate::lend_window::return_dragged(&device, &window, [x, y])
    })
    .await;
    match back {
        Ok(answer) if answer.get("error").is_none() => {
            tracing::info!("Shared input: a dragged window went home");
            crate::lend_window::tell_returned(&r.from, &was.window).await;
        }
        _ => {
            tracing::warn!("Shared input: a dragged window could not be brought home");
            // Still lent: dragged again, or returned by closing its view.
            dragged()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(r.from, was);
        }
    }
}

// ---- Taking an offer ----

/// What the computer that is offered a window checks and does.
trait Here: Sync {
    /// Whether this computer takes offers at all (Windows does not yet).
    fn supported(&self) -> bool;
    /// Whether `device` is one of the owner's computers, paired both ways.
    fn is_mine(&self, device: &str) -> bool;
    /// The computer whose keyboard and mouse are here now.
    fn driven_by(&self) -> Option<String>;
    /// Whether an offer came a moment ago (one is taken, then a pause).
    fn too_soon(&self) -> bool;
    /// Ask the app to open the borrowed window.
    fn open(&self, device: &str, offer: &Offer) -> std::io::Result<()>;
}

/// Why an offer is not taken, for the log.
#[derive(Debug, PartialEq, Eq)]
enum Declined {
    NotSupported,
    NotMine,
    NotDriving,
    TooSoon,
    Unreadable,
    NoApp,
}

/// Take the offer in `body` from `device`: it must be from one of this
/// computer's own (paired both ways), the one whose pointer is here now,
/// and a well-formed offer.
fn accept(env: &dyn Here, device: &str, body: &Value) -> Result<Offer, Declined> {
    if !env.supported() {
        return Err(Declined::NotSupported);
    }
    if !env.is_mine(device) {
        return Err(Declined::NotMine);
    }
    if env.driven_by().as_deref() != Some(device) {
        return Err(Declined::NotDriving);
    }
    let offer = Offer::read(body).ok_or(Declined::Unreadable)?;
    if env.too_soon() {
        return Err(Declined::TooSoon);
    }
    env.open(device, &offer).map_err(|_| Declined::NoApp)?;
    Ok(offer)
}

/// How long after an offer is taken the next is not.
const PAUSE: Duration = Duration::from_secs(2);

impl Here for Live {
    fn supported(&self) -> bool {
        !cfg!(windows)
    }

    fn is_mine(&self, device: &str) -> bool {
        crate::lend_window::is_owner_computer(device)
    }

    fn driven_by(&self) -> Option<String> {
        super::receive::driven_by()
    }

    fn too_soon(&self) -> bool {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let soon = last.is_some_and(|at| at.elapsed() < PAUSE);
        if !soon {
            *last = Some(Instant::now());
        }
        soon
    }

    fn open(&self, device: &str, offer: &Offer) -> std::io::Result<()> {
        let dir = crate::state::state_dir().map_err(std::io::Error::other)?;
        pocketdesk_core::local::app_open::request_borrow_drop(
            &dir,
            device,
            &pocketdesk_core::local::app_open::DroppedWindow {
                window: offer.window.clone(),
                title: offer.title.clone(),
                x: offer.x,
                y: offer.y,
                width: offer.width,
                height: offer.height,
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        )
    }
}

/// Whether `body` is an offer of a window, handled here: the app is asked to
/// open it when the computer that sent it is one of this one's own and its
/// pointer is here. Answer-shaped, so it is heard before any permission and
/// never answered; anything that does not pass is dropped with a line in the
/// log (the window's title is never logged).
pub fn note_offer(device_id: &str, body: &Value) -> bool {
    if body.get(OFFER).is_none() {
        return false;
    }
    let (device, body) = (device_id.to_string(), body.clone());
    let take = move || match accept(&Live, &device, &body) {
        Ok(_) => tracing::info!("Shared input: a window dragged here is opened"),
        Err(why) => tracing::debug!("Shared input: an offered window is not opened: {why:?}"),
    };
    // The trust files are read: off the packet loop where there is one.
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(take);
        }
        Err(_) => take(),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
        *START + Duration::from_millis(ms)
    }

    // ---- the trigger ----

    #[test]
    fn the_setting_needs_the_lend_and_names_its_trigger() {
        assert!(!Rules::from_settings(false, true, "mod").on);
        assert!(!Rules::from_settings(true, false, "mod").on, "lend is off");
        assert!(Rules::from_settings(true, true, "mod").on);
        assert_eq!(Trigger::named("mod"), Trigger::Modifier);
        assert_eq!(Trigger::named(" ANY "), Trigger::Any);
        assert_eq!(
            Trigger::named("whatever"),
            Trigger::Modifier,
            "the stricter one"
        );
        assert_eq!(Rules::default().trigger, Trigger::Modifier);
    }

    #[test]
    fn a_button_let_go_is_a_window_drag_when_the_modifier_came_with_it() {
        let mut e = Evidence::default();
        assert!(!e.ready(Trigger::Any), "nothing let go of yet");
        e.button_up(LEFT);
        assert!(e.ready(Trigger::Any), "any drag: the button is enough");
        assert!(!e.ready(Trigger::Modifier), "the modifier trigger waits");
        e.modifier_up();
        assert!(e.ready(Trigger::Modifier));
        e.spend();
        assert!(!e.ready(Trigger::Any), "read once a visit");
        assert!(e.is_spent());
        e.reset();
        assert!(!e.is_spent() && !e.ready(Trigger::Any), "a new visit");
    }

    #[test]
    fn the_modifier_may_come_before_the_button() {
        let mut e = Evidence::default();
        e.modifier_up();
        assert!(!e.ready(Trigger::Modifier), "no button yet");
        e.button_up(LEFT);
        assert!(e.ready(Trigger::Modifier));
    }

    #[test]
    fn the_right_or_middle_button_never_moves_a_window() {
        // The compositor's modifier with the right button resizes.
        for button in [1, 2, 3, 4] {
            let mut e = Evidence::default();
            e.modifier_up();
            e.button_up(button);
            e.button_up(LEFT);
            assert!(!e.ready(Trigger::Any), "button {button} spoils the drag");
        }
    }

    #[test]
    fn a_button_with_no_modifier_after_it_is_let_go_of_after_the_grace() {
        let mut e = Evidence::default();
        e.button_up(LEFT);
        e.tick(at(0));
        e.tick(at(499));
        assert!(!e.is_spent(), "still waiting for the modifier");
        e.tick(at(500));
        assert!(e.is_spent(), "a text selection, not a window");
        assert!(!e.ready(Trigger::Modifier));
        // Late is too late: the modifier after the grace takes nothing.
        e.modifier_up();
        assert!(!e.ready(Trigger::Modifier));
        // The grace is for the modifier trigger's wait: a modifier that came
        // in time holds.
        let mut e = Evidence::default();
        e.button_up(LEFT);
        e.tick(at(0));
        e.modifier_up();
        e.tick(at(900));
        assert!(e.ready(Trigger::Modifier));
        // No button: no clock runs.
        let mut e = Evidence::default();
        e.tick(at(0));
        e.tick(at(10_000));
        assert!(!e.is_spent());
    }

    #[test]
    fn the_keys_a_move_is_bound_to() {
        for code in [56, 100, 125, 126] {
            assert!(is_modifier(code), "{code}");
        }
        for code in [29, 42, 30, 28] {
            assert!(!is_modifier(code), "ctrl, shift, a, enter: {code}");
        }
        assert!(modifier_held(|c| c == 125));
        assert!(!modifier_held(|c| c == 29));
    }

    // ---- the pick ----

    fn window(id: &str, rect: [i32; 4]) -> Candidate {
        Candidate {
            id: id.to_string(),
            title: format!("window {id}"),
            floating: true,
            focused: false,
            fullscreen: false,
            hidden: false,
            rect,
        }
    }

    #[test]
    fn the_focused_floating_window_under_the_pointer_is_picked() {
        let mut front = window("0xaa", [1200, 300, 800, 600]);
        front.focused = true;
        let behind = window("0xbb", [0, 0, 1920, 1080]);
        // Crossed on the right edge of a 1920 wide screen, in the window.
        let picked = pick(&[behind, front], (1919, 450)).unwrap();
        assert_eq!(picked.id, "0xaa");
        assert_eq!(picked.rect, [1200, 300, 800, 600]);
        assert_eq!(picked.grab, (719, 150), "where in the window it was held");
    }

    #[test]
    fn a_window_a_few_pixels_short_of_the_edge_is_still_under_the_pointer() {
        let mut front = window("7", [1000, 100, 912, 400]);
        front.focused = true;
        assert!(
            pick(std::slice::from_ref(&front), (1919, 200)).is_ok(),
            "8 short"
        );
        front.rect = [1000, 100, 900, 400];
        assert_eq!(
            pick(&[front], (1919, 200)).unwrap_err(),
            Why::NotUnderPointer,
            "19 short"
        );
    }

    #[test]
    fn what_is_not_a_floating_window_in_front_is_left_alone() {
        let mut front = window("0xaa", [1200, 300, 800, 600]);
        assert_eq!(pick(&[], (1919, 450)).unwrap_err(), Why::NoFocus);
        assert_eq!(
            pick(std::slice::from_ref(&front), (1919, 450)).unwrap_err(),
            Why::NoFocus,
            "nothing has the focus"
        );
        front.focused = true;
        front.floating = false;
        assert_eq!(
            pick(std::slice::from_ref(&front), (1919, 450)).unwrap_err(),
            Why::Tiled
        );
        front.floating = true;
        front.fullscreen = true;
        assert_eq!(
            pick(std::slice::from_ref(&front), (1919, 450)).unwrap_err(),
            Why::Fullscreen
        );
        front.fullscreen = false;
        front.hidden = true;
        assert_eq!(
            pick(std::slice::from_ref(&front), (1919, 450)).unwrap_err(),
            Why::NoFocus,
            "a hidden window is not in front"
        );
        front.hidden = false;
        assert_eq!(
            pick(std::slice::from_ref(&front), (10, 10)).unwrap_err(),
            Why::NotUnderPointer,
            "the pointer crossed elsewhere"
        );
    }

    #[test]
    fn a_window_with_no_size_is_never_picked() {
        let mut front = window("1", [1900, 100, 0, 0]);
        front.focused = true;
        assert_eq!(
            pick(&[front], (1919, 100)).unwrap_err(),
            Why::NotUnderPointer
        );
    }

    // ---- both compositors' windows ----

    const HYPR_CLIENTS: &str = r#"[
      {"address":"0x55d1","mapped":true,"hidden":false,"at":[1280,200],"size":[640,480],
       "workspace":{"id":1,"name":"1"},"floating":true,"monitor":0,"class":"term","title":"one",
       "pid":1,"fullscreen":0,"fullscreenClient":0},
      {"address":"0x55d2","mapped":true,"hidden":false,"at":[0,0],"size":[1920,1080],
       "workspace":{"id":1,"name":"1"},"floating":false,"monitor":0,"class":"term","title":"two",
       "pid":2,"fullscreen":0,"fullscreenClient":0},
      {"address":"0x55d3","mapped":true,"hidden":false,"at":[0,0],"size":[1920,1080],
       "workspace":{"id":2,"name":"2"},"floating":true,"monitor":0,"class":"vid","title":"three",
       "pid":3,"fullscreen":2,"fullscreenClient":0},
      {"address":"0x55d4","mapped":true,"hidden":false,"at":[100,100],"size":[300,300],
       "workspace":{"id":-99,"name":"special:magic"},"floating":true,"monitor":0,"class":"x",
       "title":"four","pid":4,"fullscreen":0,"fullscreenClient":0},
      {"address":"0x55d5","mapped":false,"hidden":false,"at":[0,0],"size":[10,10],
       "workspace":{"id":1,"name":"1"},"floating":true,"monitor":0,"class":"x","title":"five",
       "pid":5,"fullscreen":0,"fullscreenClient":0}
    ]"#;

    #[test]
    fn hyprlands_windows_are_read_with_the_focus_from_the_active_one() {
        let all = candidates_from_hypr(HYPR_CLIENTS, r#"{"address":"0x55d1","title":"one"}"#);
        assert_eq!(all.len(), 5);
        let one = &all[0];
        assert!(one.floating && one.focused && !one.fullscreen && !one.hidden);
        assert_eq!(one.rect, [1280, 200, 640, 480]);
        assert!(!all[1].floating && !all[1].focused);
        assert!(all[2].fullscreen, "Hyprland's 2 is full screen");
        assert!(all[3].hidden, "a special workspace is not shown");
        assert!(all[4].hidden, "an unmapped window is not there");
        // The pick over them: the focused floating one, crossed at its edge.
        assert_eq!(pick(&all, (1919, 300)).unwrap().id, "0x55d1");
        // Another focused: the tiled one, the full-screen one.
        let tiled = candidates_from_hypr(HYPR_CLIENTS, r#"{"address":"0x55d2"}"#);
        assert_eq!(pick(&tiled, (1919, 300)).unwrap_err(), Why::Tiled);
        let full = candidates_from_hypr(HYPR_CLIENTS, r#"{"address":"0x55d3"}"#);
        assert_eq!(pick(&full, (1919, 300)).unwrap_err(), Why::Fullscreen);
        // No window in front: Hyprland says {}.
        let none = candidates_from_hypr(HYPR_CLIENTS, "{}");
        assert_eq!(pick(&none, (1919, 300)).unwrap_err(), Why::NoFocus);
    }

    #[test]
    fn what_hyprland_says_badly_picks_nothing() {
        assert!(candidates_from_hypr("not json", "{}").is_empty());
        assert!(candidates_from_hypr("[]", "{}").is_empty());
        assert!(candidates_from_hypr("{}", "{}").is_empty());
    }

    /// A Sway tree: a tiled terminal, a floating window being dragged (its
    /// title bar above its content), and a window in the scratchpad.
    fn sway_tree() -> Value {
        json!({
          "id": 1, "type": "root", "name": "root", "floating_nodes": [],
          "nodes": [
            { "id": 2147483647, "type": "output", "name": "__i3", "floating_nodes": [],
              "nodes": [ { "id": 9, "type": "workspace", "name": "__i3_scratch", "num": -1,
                "nodes": [], "floating_nodes": [
                  { "id": 99, "type": "floating_con", "name": "hidden", "app_id": "hid",
                    "focused": false, "nodes": [], "floating_nodes": [],
                    "rect": {"x":0,"y":0,"width":100,"height":100},
                    "window_rect": {"x":0,"y":0,"width":100,"height":100},
                    "deco_rect": {"x":0,"y":0,"width":100,"height":0} } ] } ] },
            { "id": 3, "type": "output", "name": "HEADLESS-1", "floating_nodes": [],
              "nodes": [ { "id": 4, "type": "workspace", "name": "1", "num": 1, "floating_nodes": [
                  { "id": 11, "type": "floating_con", "name": "dragged", "app_id": "foot",
                    "focused": true, "nodes": [], "floating_nodes": [],
                    "rect": {"x":1300,"y":327,"width":620,"height":400},
                    "window_rect": {"x":0,"y":0,"width":620,"height":400},
                    "deco_rect": {"x":1300,"y":300,"width":620,"height":27},
                    "fullscreen_mode": 0 } ],
                "nodes": [
                  { "id": 10, "type": "con", "name": "tiled", "app_id": "foot",
                    "focused": false, "nodes": [], "floating_nodes": [],
                    "rect": {"x":0,"y":0,"width":1920,"height":1080},
                    "window_rect": {"x":0,"y":0,"width":1920,"height":1080},
                    "deco_rect": {"x":0,"y":0,"width":0,"height":0},
                    "fullscreen_mode": 0 } ] } ] }
          ]
        })
    }

    #[test]
    fn sways_container_is_picked_with_its_title_bar_in() {
        let all = candidates_from_sway(&sway_tree());
        assert_eq!(all.len(), 2, "the scratchpad's is not on a screen");
        let dragged = all.iter().find(|w| w.id == "11").unwrap();
        assert!(dragged.floating && dragged.focused);
        // The container: the title bar's top, its content's height with it.
        assert_eq!(dragged.rect, [1300, 300, 620, 427]);
        let picked = pick(&all, (1919, 310)).unwrap();
        assert_eq!(picked.id, "11");
        assert_eq!(picked.grab, (619, 10), "held by its title bar");
        // The tiled one with the focus is left alone.
        let mut tiled = all.clone();
        for w in &mut tiled {
            w.focused = w.id == "10";
        }
        assert_eq!(pick(&tiled, (1919, 310)).unwrap_err(), Why::Tiled);
    }

    // ---- where it lands ----

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_window_lands_with_the_pointer_where_it_held_it() {
        let s = screen(0, 0, 2560, 1440);
        // Dropped in the middle of the screen, held 719 across and 150 down.
        assert_eq!(place(&s, (1000, 700), (719, 150), (800, 600)), (281, 550));
    }

    #[test]
    fn a_window_is_slid_in_to_stay_on_its_screen() {
        let s = screen(0, 0, 2560, 1440);
        // Dropped on the left edge: its left side would be off.
        assert_eq!(place(&s, (0, 700), (719, 150), (800, 600)), (0, 550));
        // On the bottom edge: its bottom would be off.
        assert_eq!(place(&s, (1000, 1439), (100, 20), (800, 600)), (900, 840));
        // Past the far corner: the last place it fits.
        assert_eq!(place(&s, (2559, 1439), (0, 0), (800, 600)), (1760, 840));
    }

    #[test]
    fn a_screen_not_at_the_origin_and_a_window_too_big_for_it() {
        let right = screen(1920, -200, 1280, 1024);
        assert_eq!(place(&right, (2000, 100), (50, 50), (400, 300)), (1950, 50));
        assert_eq!(
            place(&right, (1920, -200), (50, 50), (400, 300)),
            (1920, -200)
        );
        // Larger than the screen: its top left is the screen's.
        let small = screen(100, 50, 640, 480);
        assert_eq!(place(&small, (300, 300), (10, 10), (1000, 900)), (100, 50));
    }

    // ---- the handover ----

    struct Fake {
        gate: Result<(), &'static str>,
        windows: Mutex<Option<Vec<Candidate>>>,
    }

    impl Source for Fake {
        fn may_lend_to(&self, _device: &str) -> Result<(), &'static str> {
            self.gate
        }

        fn candidates(&self) -> std::io::Result<Vec<Candidate>> {
            self.windows
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| std::io::Error::other("no compositor"))
        }
    }

    fn handover() -> Handover {
        Handover {
            to: "desk".to_string(),
            crossed_at: (1919, 450),
            drop_at: (1000, 700),
            screen: screen(0, 0, 2560, 1440),
        }
    }

    fn fake(gate: Result<(), &'static str>, windows: Option<Vec<Candidate>>) -> Fake {
        Fake {
            gate,
            windows: Mutex::new(windows),
        }
    }

    #[test]
    fn a_drag_that_ends_over_there_offers_the_window_where_it_lands() {
        let mut front = window("0xaa", [1200, 300, 800, 600]);
        front.focused = true;
        let decided = decide(&fake(Ok(()), Some(vec![front])), &handover());
        let Decided::Offer(offer, picked) = decided else {
            panic!("an offer: {decided:?}");
        };
        assert_eq!(picked.id, "0xaa");
        assert_eq!(
            offer,
            Offer {
                window: "0xaa".into(),
                title: "window 0xaa".into(),
                x: 281,
                y: 550,
                width: 800,
                height: 600
            }
        );
    }

    #[test]
    fn a_refused_lend_is_a_plain_crossing() {
        let mut front = window("0xaa", [1200, 300, 800, 600]);
        front.focused = true;
        let env = fake(Err("not lending"), Some(vec![front]));
        assert_eq!(decide(&env, &handover()), Decided::Refused("not lending"));
    }

    #[test]
    fn nothing_is_offered_for_a_tiled_window_or_an_unreadable_compositor() {
        let mut tiled = window("0xaa", [0, 0, 1920, 1080]);
        tiled.focused = true;
        tiled.floating = false;
        assert_eq!(
            decide(&fake(Ok(()), Some(vec![tiled])), &handover()),
            Decided::Nothing(Why::Tiled)
        );
        assert_eq!(
            decide(&fake(Ok(()), None), &handover()),
            Decided::Unreadable
        );
    }

    // ---- the packet ----

    #[test]
    fn an_offer_crosses_as_a_packet_body_and_reads_back() {
        let offer = Offer {
            window: "0x55d1".into(),
            title: "A title".into(),
            x: -40,
            y: 12,
            width: 800,
            height: 600,
        };
        let body = offer.body();
        assert!(body.get("action").is_none(), "answer-shaped");
        assert_eq!(Offer::read(&body), Some(offer));
    }

    #[test]
    fn an_offer_is_read_as_strictly_as_an_ask() {
        let good = json!({ OFFER: {
            "window": "42", "title": "t", "x": 1, "y": 2, "width": 300, "height": 200 }});
        assert!(Offer::read(&good).is_some());
        let with = |key: &str, value: Value| {
            let mut body = good.clone();
            body[OFFER][key] = value;
            Offer::read(&body)
        };
        // A window id that could reach a command.
        for bad in ["", "0x", "42; exec x", "-1", "0xZZ", "9999999999999"] {
            assert_eq!(with("window", json!(bad)), None, "{bad:?}");
        }
        assert_eq!(with("window", json!(42)), None, "not a string");
        // Sizes and places a screen cannot have.
        for key in ["width", "height"] {
            for bad in [json!(0), json!(63), json!(16385), json!(-5), json!("9")] {
                assert_eq!(with(key, bad.clone()), None, "{key} {bad}");
            }
        }
        for key in ["x", "y"] {
            assert_eq!(with(key, json!(1_000_001)), None);
            assert_eq!(with(key, json!(i64::MAX)), None);
            assert_eq!(with(key, json!(null)), None);
            assert!(with(key, json!(-1_000_000)).is_some());
        }
        assert_eq!(Offer::read(&json!({})), None);
        assert_eq!(Offer::read(&json!({ "lent_gone": "42" })), None);
        // A title is cleaned and cut.
        let long = "x".repeat(500);
        let read = with("title", json!(format!("a\u{7}b\n{long}"))).unwrap();
        assert!(!read.title.contains('\u{7}') && !read.title.contains('\n'));
        assert_eq!(read.title.chars().count(), TITLE_CHARS);
    }

    #[test]
    fn a_title_loses_bidi_overrides_isolates_and_other_format_characters() {
        let sly = "a\u{202E}b\u{2066}c\u{2069}d\u{200B}e\u{2060}f\u{AD}g\u{E0041}h\u{7}";
        assert_eq!(clean_title(sly), "abcdefgh");
        // The title of an offer read off the wire, and of one decided here.
        let mut body = json!({ OFFER: {
            "window": "42", "title": "t", "x": 1, "y": 2, "width": 300, "height": 200 }});
        body[OFFER]["title"] = json!(sly);
        assert_eq!(Offer::read(&body).unwrap().title, "abcdefgh");
        let mut front = window("0xaa", [1200, 300, 800, 600]);
        front.title = sly.to_string();
        front.focused = true;
        let Decided::Offer(offer, _) = decide(&fake(Ok(()), Some(vec![front])), &handover()) else {
            panic!("an offer");
        };
        assert_eq!(offer.title, "abcdefgh");
    }

    // ---- taking an offer ----

    struct There {
        supported: bool,
        mine: bool,
        driven_by: Option<&'static str>,
        soon: bool,
        opened: Mutex<Vec<(String, Offer)>>,
        fails: bool,
    }

    impl There {
        fn ready() -> There {
            There {
                supported: true,
                mine: true,
                driven_by: Some("lap"),
                soon: false,
                opened: Mutex::new(Vec::new()),
                fails: false,
            }
        }
    }

    impl Here for There {
        fn supported(&self) -> bool {
            self.supported
        }

        fn is_mine(&self, _device: &str) -> bool {
            self.mine
        }

        fn driven_by(&self) -> Option<String> {
            self.driven_by.map(str::to_string)
        }

        fn too_soon(&self) -> bool {
            self.soon
        }

        fn open(&self, device: &str, offer: &Offer) -> std::io::Result<()> {
            if self.fails {
                return Err(std::io::Error::other("no state folder"));
            }
            self.opened
                .lock()
                .unwrap()
                .push((device.to_string(), offer.clone()));
            Ok(())
        }
    }

    fn offered() -> Value {
        json!({ OFFER: {
            "window": "0x55d1", "title": "t", "x": 10, "y": 20, "width": 800, "height": 600 }})
    }

    #[test]
    fn an_offer_from_the_computer_whose_pointer_is_here_opens_the_window() {
        let there = There::ready();
        let offer = accept(&there, "lap", &offered()).unwrap();
        assert_eq!(offer.window, "0x55d1");
        let opened = there.opened.lock().unwrap();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].0, "lap");
        assert_eq!((opened[0].1.x, opened[0].1.y), (10, 20));
    }

    #[test]
    fn an_offer_that_does_not_pass_the_gates_opens_nothing() {
        let case = |there: There, want: Declined| {
            assert_eq!(accept(&there, "lap", &offered()).unwrap_err(), want);
            assert!(there.opened.lock().unwrap().is_empty());
        };
        // Windows takes none yet: a plain crossing.
        case(
            There {
                supported: false,
                ..There::ready()
            },
            Declined::NotSupported,
        );
        // A computer paired one way, or a stranger.
        case(
            There {
                mine: false,
                ..There::ready()
            },
            Declined::NotMine,
        );
        // Its pointer is not here (or another's is).
        case(
            There {
                driven_by: None,
                ..There::ready()
            },
            Declined::NotDriving,
        );
        case(
            There {
                driven_by: Some("other"),
                ..There::ready()
            },
            Declined::NotDriving,
        );
        // Too soon after the last.
        case(
            There {
                soon: true,
                ..There::ready()
            },
            Declined::TooSoon,
        );
        case(
            There {
                fails: true,
                ..There::ready()
            },
            Declined::NoApp,
        );
        // Not an offer that reads.
        let there = There::ready();
        assert_eq!(
            accept(&there, "lap", &json!({ OFFER: { "window": "bad id" } })).unwrap_err(),
            Declined::Unreadable
        );
    }

    #[test]
    fn only_an_offer_is_taken_by_the_offer_check() {
        assert!(!note_offer("lap", &json!({ "lent_gone": "5" })));
        assert!(!note_offer(
            "lap",
            &json!({ "id": 3, "name": "HEADLESS-2" })
        ));
    }

    // ---- what was handed over ----

    #[test]
    fn a_window_handed_over_is_taken_back_once() {
        let mut map = dragged().lock().unwrap();
        map.insert(
            "taken-back".to_string(),
            Dragged {
                window: "0xaa".into(),
                grab: (3, 4),
                size: (800, 600),
                at: Instant::now(),
            },
        );
        drop(map);
        let got = take_dragged("taken-back").unwrap();
        assert_eq!((got.window.as_str(), got.grab), ("0xaa", (3, 4)));
        assert!(take_dragged("taken-back").is_none(), "once");
        assert!(take_dragged("never-handed-over").is_none());
    }
}
