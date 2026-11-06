//! GNOME Shell's windows, as the Pocketdesk GNOME Shell extension tells them
//! (window handoff, phase 6b: input into a lent GNOME window).
//!
//! **Why an extension.** GNOME Shell keeps its window list from ordinary
//! programs (`org.gnome.Shell.Introspect.GetWindows` answers "Access denied"
//! to anything that is not one of GNOME's own apps), and no program can ask
//! GNOME which window covers a spot or has the keyboard. Phase 6 therefore
//! lent a GNOME window to look at only: mutter's RemoteDesktop puts a pointer
//! in a window's rectangle but sends the click to whichever window is topmost
//! there. The extension in `contrib/gnome-shell-extension` is the owner's
//! choice to give those answers to this program (and, by the nature of the
//! session bus, to any other program of the same user): `dev.pocketdesk.Windows`
//! answers `List`, `HitTest`, `Activate` and `Layout`, and nothing else. When
//! it answers, a lent GNOME window takes input through the same guard
//! ([`super::window_guard`]) and the same rules as a Windows one; when it does
//! not, the window stays view-only exactly as before.
//!
//! **What the guard asks, and what answers it** ([`GnomeOs`] is the guard's
//! [`Os`] over an [`Extension`]; the live one is [`bus::Bus`]):
//!
//! - the frame of the lent window: its entry of `List`, only while shown and
//!   sized;
//! - whether it is in front: its entry of `List` says it (or a dialog or popup
//!   of it, of the same program) has the keyboard;
//! - whether the window that takes a click at a pixel is the lent one or
//!   belongs to it: the stage's own pick, as a chain from the window under the
//!   point through the windows it is a dialog or popup of ([`chain_belongs`]);
//!   what is under the point is not a window (the top bar, the overview, a
//!   menu of the Shell, the lock screen) answers no;
//! - raising it: `Activate`.
//!
//! Anything unknown (the extension does not answer, the window is not in the
//! list, a pid is zero) answers no, so the packet is dropped.
//!
//! **The pixel space.** The guard places a picture point on a "layout" box and
//! says the result as a 0..=65535 point of it. Here the layout box is
//! [`GUARD_SPACE`] itself, so that point *is* the pixel, exactly, with every
//! desktop pixel moved by [`BIAS`] so a monitor to the left of or above the
//! primary stays non-negative. [`GnomeOs`] hands the guard frames in that
//! space and takes its pixels back out of it for the pick; the capture's input
//! worker ([`local_pointer`]) turns the same pixel into the window-relative
//! place mutter's RemoteDesktop takes for a window's stream, dropping a pixel
//! the window no longer holds. No monitor layout is asked of anyone, and the
//! pixel the guard tested is the pixel that is sent.
//!
//! The decisions are plain functions, tested on fixtures; the calls to the
//! bus are thin and live in [`bus`] (Linux, with capture).

use super::input_packet::InputPacket;
use super::portal_input::{PortalEvent, text_events};
use super::window_guard::{Os, frame_contains};
use super::window_target::{self, Lease, Rect};

/// The bus name, object and interface the extension serves.
pub const BUS_NAME: &str = "dev.pocketdesk.Windows";
pub const OBJECT_PATH: &str = "/dev/pocketdesk/Windows";
pub const INTERFACE: &str = BUS_NAME;

/// Every desktop pixel is moved by this much (right and down) in the guard's
/// space, so the pixels of a monitor left of or above the primary are not
/// negative.
pub const BIAS: i32 = 16_384;

/// The box the guard is given as the layout: the whole 0..=65535 range, so a
/// point it says is the pixel itself.
pub const GUARD_SPACE: Rect = (0, 0, 65_535, 65_535);

/// How many windows are followed up from one to find the lent one (the
/// extension makes the same cut).
pub const MOST_OWNERS: usize = 8;

/// An ordinary top-level window, as `List` says it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    /// Mutter's id for it, the one `RecordWindow` takes.
    pub id: u64,
    pub title: String,
    /// The class it sets, else its application id.
    pub app: String,
    pub pid: u32,
    /// Its frame on the desktop, in logical pixels.
    pub frame: Rect,
    /// Its buffer on the desktop: the frame and, for a client-side decorated
    /// window, the shadow round it. Mutter's window stream shows this, and
    /// places a pointer in it. `None` when the extension did not say.
    pub buffer: Option<Rect>,
    pub minimized: bool,
    /// Minimised or put away (another workspace).
    pub hidden: bool,
    /// It, or a dialog or popup of it of the same program, has the keyboard.
    pub focus: bool,
    /// Its place in the stacking order, 0 the bottom.
    pub stack: u32,
}

/// Why the extension gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// It is not installed, not enabled, or GNOME Shell is not there: the bus
    /// has nobody under its name.
    NotInstalled,
    /// Something else went wrong (the bus, a timeout, an answer that could not
    /// be read).
    Failed(&'static str),
}

/// How long calls to the extension are not made after one that was not
/// answered (a Shell that is stuck, a bus that hangs).
pub const STALL_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// A circuit breaker for the calls to the extension: the guard asks several
/// questions for a click, on the way of the packet, so a Shell that does not
/// answer must cost one wait, not one for each question. After
/// [`Breaker::trip`] it is [`open`](Breaker::open) for [`STALL_PAUSE`], and
/// the calls in that time answer "failed" at once (which every rule reads as
/// no).
#[derive(Debug, Default)]
pub struct Breaker {
    until: Option<std::time::Instant>,
}

impl Breaker {
    pub const fn new() -> Self {
        Self { until: None }
    }

    /// Whether calls are paused at `now`.
    pub fn open(&self, now: std::time::Instant) -> bool {
        self.until.is_some_and(|until| now < until)
    }

    /// An answer did not come: pause calls from `now`.
    pub fn trip(&mut self, now: std::time::Instant) {
        self.until = Some(now + STALL_PAUSE);
    }
}

/// Run `f`, which blocks on the extension, off an async worker thread where
/// the runtime has several (it runs in place in any other); the streamer's
/// tasks call the guard and ask whether the lend takes input this way.
pub fn blocking<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Which of the packets of a lent GNOME window may be told to mutter, given
/// whether the pointer is known to be inside the window. A button press or a
/// scroll acts at the seat's pointer, wherever that is (the owner's own
/// cursor, in another window), so neither is sent unless the move before it
/// placed the pointer in the window: a move that was dropped (the window moved
/// or shrank, is hidden, the extension was slow) or failed leaves them
/// dropped until one succeeds. Releases always pass, so nothing this session
/// pressed stays down; keys and text do not depend on the pointer (the guard
/// sends them only while the window has the keyboard) and pass.
#[derive(Debug, Default)]
pub struct Placement {
    placed: bool,
}

impl Placement {
    /// A move the guard sent: `local` is where the window holds it now (`None`
    /// when it does not). Returns it, so the caller tells mutter only then;
    /// a `None` leaves the pointer unplaced.
    pub fn moved(&mut self, local: Option<(i32, i32)>) -> Option<(i32, i32)> {
        self.placed = local.is_some();
        local
    }

    /// The call that moved the pointer failed.
    pub fn failed(&mut self) {
        self.placed = false;
    }

    /// Whether `event` may be told to mutter now.
    pub fn allows(&self, event: &PortalEvent) -> bool {
        match event {
            PortalEvent::Button { pressed: true, .. }
            | PortalEvent::Scroll { .. }
            | PortalEvent::FineScroll { .. } => self.placed,
            _ => true,
        }
    }
}

/// Whether `event` lets go of something (a button, a key): what is still
/// sent as a session ends.
pub fn is_release(event: &PortalEvent) -> bool {
    matches!(
        event,
        PortalEvent::Button { pressed: false, .. }
            | PortalEvent::Key { pressed: false, .. }
            | PortalEvent::Keysym { pressed: false, .. }
    )
}

/// What the guard needs of the extension. [`bus::Bus`] is the real one; the
/// tests have a fake.
pub trait Extension {
    /// Every ordinary top-level window, bottom of the stack first.
    fn list(&self) -> Result<Vec<Window>, Unavailable>;
    /// The window the stage gives a click at the desktop pixel `at` first,
    /// then what it is a dialog or popup of, as `(id, pid)`; empty when
    /// what is there is not a window.
    fn hit(&self, at: (i32, i32)) -> Result<Vec<(u64, u32)>, Unavailable>;
    /// Raise and focus the window `id`; whether a listed window had it.
    fn activate(&self, id: u64) -> Result<bool, Unavailable>;
}

/// Whether the window `lent` is in `chain` (a window then the windows it is a
/// dialog or popup of, as `(id, pid)`), with every window up to it of one
/// program (a pid of zero is none): the lent window itself, or one of its own
/// dialogs and popups. Any other window is not, however it is stacked.
pub fn chain_belongs(chain: &[(u64, u32)], lent: u64) -> bool {
    let Some(&(first, pid)) = chain.first() else {
        return false;
    };
    if lent == 0 || first == 0 || pid == 0 {
        return false;
    }
    for &(id, owner_pid) in chain.iter().take(MOST_OWNERS) {
        if owner_pid != pid || id == 0 {
            return false;
        }
        if id == lent {
            return true;
        }
    }
    false
}

/// Whether a window is one a click or key could reach: shown, with a size.
pub fn visible(window: &Window) -> bool {
    !window.minimized && !window.hidden && window.frame.2 > 0 && window.frame.3 > 0
}

/// A frame in the guard's space ([`BIAS`]); `None` when it would not fit it
/// (a corner left of or above [`BIAS`] pixels short of the origin, or past
/// the range).
pub fn biased(frame: Rect) -> Option<Rect> {
    let x = i64::from(frame.0) + i64::from(BIAS);
    let y = i64::from(frame.1) + i64::from(BIAS);
    let limit = i64::from(GUARD_SPACE.2);
    if x < 0 || y < 0 || x >= limit || y >= limit {
        return None;
    }
    Some((x as i32, y as i32, frame.2, frame.3))
}

/// A pixel of the guard's space as the desktop's.
pub fn unbiased(at: (i32, i32)) -> (i32, i32) {
    (at.0 - BIAS, at.1 - BIAS)
}

/// Where a point the guard sent (`norm` is a pixel of its space) is in the
/// window whose frame is `frame` now (the desktop's pixels): window-relative,
/// which is what mutter's RemoteDesktop takes for a window's stream. `None`
/// when the window does not hold that pixel (it moved or shrank since the
/// guard looked): nothing is sent rather than clamped to its edge.
pub fn local_pointer(norm: (u16, u16), frame: Rect) -> Option<(i32, i32)> {
    let at = unbiased((i32::from(norm.0), i32::from(norm.1)));
    if !frame_contains(frame, at) {
        return None;
    }
    Some((at.0 - frame.0, at.1 - frame.1))
}

/// Where a window's frame starts in its buffer, which is what mutter's
/// window stream shows and places a pointer in: the shadow's width and
/// height above and left of a client-side decorated window, `(0, 0)` for one
/// with none, or when the buffer is not known or does not hold the frame.
pub fn frame_offset(window: &Window) -> (i32, i32) {
    match window.buffer {
        Some(buffer) if holds(buffer, window.frame) => {
            (window.frame.0 - buffer.0, window.frame.1 - buffer.1)
        }
        _ => (0, 0),
    }
}

/// The part of a window stream's picture that is the window's frame, for a
/// picture of `picture` pixels showing the buffer (scaled to it, as a
/// monitor's scale makes it). `None` when that is the whole picture, or when
/// it cannot be told: the picture is then sent as it is.
pub fn frame_in_picture(window: &Window, picture: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let buffer = window.buffer.filter(|b| holds(*b, window.frame))?;
    if buffer == window.frame || buffer.2 == 0 || buffer.3 == 0 {
        return None;
    }
    // A picture that is not the buffer, scaled (its shape is another's): not
    // known to be it, so not cut.
    let (bw, bh) = (u64::from(buffer.2), u64::from(buffer.3));
    let (pw, ph) = (u64::from(picture.0), u64::from(picture.1));
    if (pw * bh).abs_diff(ph * bw) > bw.max(bh) * 2 {
        return None;
    }
    // A picture within a pixel or two of the buffer is the buffer at scale
    // 1 (mutter rounds its rectangles apart by a pixel): the frame's own
    // pixels, unscaled. Otherwise each side is scaled by the picture's.
    let one = |picture: u32, size: u32| picture.abs_diff(size) <= 2;
    let scale = |at: u64, size: u32, picture: u32| {
        if one(picture, size) {
            u32::try_from(at).ok()
        } else {
            u32::try_from(at * u64::from(picture) / u64::from(size)).ok()
        }
    };
    let x = scale(
        u64::from((window.frame.0 - buffer.0).max(0) as u32),
        buffer.2,
        picture.0,
    )?;
    let y = scale(
        u64::from((window.frame.1 - buffer.1).max(0) as u32),
        buffer.3,
        picture.1,
    )?;
    let w = scale(u64::from(window.frame.2), buffer.2, picture.0)?.min(picture.0.saturating_sub(x));
    let h = scale(u64::from(window.frame.3), buffer.3, picture.1)?.min(picture.1.saturating_sub(y));
    (w > 0 && h > 0 && x + w <= picture.0 && y + h <= picture.1).then_some((x, y, w, h))
}

/// Whether `outer` holds all of `inner`.
fn holds(outer: Rect, inner: Rect) -> bool {
    let end = |at: i32, size: u32| i64::from(at) + i64::from(size);
    inner.0 >= outer.0
        && inner.1 >= outer.1
        && end(inner.0, inner.2) <= end(outer.0, outer.2)
        && end(inner.1, inner.3) <= end(outer.1, outer.3)
}

/// The evdev codes of the mouse buttons a packet names (left, middle, right,
/// back, forward).
const BUTTONS: [i32; 5] = [0x110, 0x112, 0x111, 0x113, 0x114];

/// What mutter's RemoteDesktop is told for a packet the guard let through,
/// apart from a move (which [`local_pointer`] places): a button, a wheel turn,
/// a key (by its evdev code), text (as keysyms). A pen, a gamepad, the laser
/// dot and everything else are none; the guard drops them anyway.
pub fn window_events(packet: &InputPacket) -> Vec<PortalEvent> {
    match packet {
        InputPacket::MouseClick { button, pressed } => BUTTONS
            .get(usize::from(*button))
            .map(|&code| PortalEvent::Button {
                code,
                pressed: *pressed,
            })
            .into_iter()
            .collect(),
        // The portal's steps go down and right; the wheel's value goes up.
        InputPacket::MouseScroll { dx, dy, fine } => {
            let mut events = Vec::new();
            match fine {
                Some((x, y)) => {
                    if *y != 0 {
                        events.push(PortalEvent::FineScroll {
                            axis: 0,
                            v120: i32::from(*y).saturating_neg(),
                        });
                    }
                    if *x != 0 {
                        events.push(PortalEvent::FineScroll {
                            axis: 1,
                            v120: i32::from(*x),
                        });
                    }
                }
                None => {
                    if *dy != 0 {
                        events.push(PortalEvent::Scroll {
                            axis: 0,
                            steps: i32::from(*dy).saturating_neg(),
                        });
                    }
                    if *dx != 0 {
                        events.push(PortalEvent::Scroll {
                            axis: 1,
                            steps: i32::from(*dx),
                        });
                    }
                }
            }
            events
        }
        InputPacket::KeyEvent { key, pressed } => vec![PortalEvent::Key {
            code: i32::from(*key),
            pressed: *pressed,
        }],
        InputPacket::Text(text) => text_events(text),
        _ => Vec::new(),
    }
}

/// The guard's [`Os`] for a lent GNOME window, over an [`Extension`].
pub struct GnomeOs<E>(pub E);

impl<E: Extension> GnomeOs<E> {
    /// The listed window the lease holds, if it still stands and is listed.
    fn lent(&self, lease: &Lease) -> Option<Window> {
        let id = window_target::gnome_id(&lease.native)?;
        if !window_target::lease_alive(lease) {
            return None;
        }
        self.0.list().ok()?.into_iter().find(|w| w.id == id)
    }
}

impl<E: Extension> Os for GnomeOs<E> {
    fn lease_holds(&self, lease: &Lease) -> bool {
        window_target::lease_alive(lease)
    }

    fn frame(&self, lease: &Lease) -> Option<Rect> {
        self.lent(lease)
            .filter(visible)
            .and_then(|w| biased(w.frame))
    }

    fn front_is_lent(&self, lease: &Lease) -> bool {
        self.lent(lease)
            .is_some_and(|w| visible(&w) && w.focus && w.pid != 0)
    }

    fn hit_is_lent(&self, lease: &Lease, at: (i32, i32)) -> bool {
        let Some(id) = window_target::gnome_id(&lease.native) else {
            return false;
        };
        match self.0.hit(unbiased(at)) {
            Ok(chain) => chain_belongs(&chain, id),
            Err(_) => false,
        }
    }

    fn raise(&self, lease: &Lease) {
        let Some(id) = window_target::gnome_id(&lease.native) else {
            return;
        };
        if window_target::lease_alive(lease) {
            let _ = self.0.activate(id);
        }
    }
}

// ---- What the guard's `Machine` answers for a GNOME window ----

/// The frame the guard places pointers in, for a native lease (see
/// [`GnomeOs`]): `None` without the extension.
pub fn frame(lease: &Lease) -> Option<Rect> {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        GnomeOs(bus::Bus).frame(lease)
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    {
        let _ = lease;
        None
    }
}

/// Whether the lent window has the keyboard: false without the extension.
pub fn front_is_lent(lease: &Lease) -> bool {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        GnomeOs(bus::Bus).front_is_lent(lease)
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    {
        let _ = lease;
        false
    }
}

/// Whether the window that takes a click at `at` (a pixel of the guard's
/// space) is the lent one or belongs to it: false without the extension.
pub fn hit_is_lent(lease: &Lease, at: (i32, i32)) -> bool {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        GnomeOs(bus::Bus).hit_is_lent(lease, at)
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    {
        let _ = (lease, at);
        false
    }
}

/// Bring the lent window to the front: nothing without the extension.
pub fn raise(lease: &Lease) {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        GnomeOs(bus::Bus).raise(lease)
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    {
        let _ = lease;
    }
}

/// Whether the extension answers on this computer's session bus now (a
/// session of a lent GNOME window takes input only if it does).
pub fn answers() -> bool {
    #[cfg(all(feature = "capture", target_os = "linux"))]
    {
        bus::Bus.list().is_ok()
    }
    #[cfg(not(all(feature = "capture", target_os = "linux")))]
    {
        false
    }
}

#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod bus {
    //! The extension over the session bus.

    use std::collections::HashMap;
    use std::time::Duration;

    use ashpd::zbus::zvariant::{OwnedValue, Value};
    use ashpd::zbus::{self, Connection};

    use super::{BUS_NAME, Extension, INTERFACE, OBJECT_PATH, Unavailable, Window};

    /// The error the blocking calls report when the call itself cannot be run.
    fn gone() -> Unavailable {
        Unavailable::Failed("the call to the GNOME Shell extension failed")
    }

    /// How long the extension may take: it answers at once or not at all.
    /// Short, because a guard's question is asked on the way of a packet.
    const CALL_WAIT: Duration = Duration::from_millis(500);

    /// Whether calls are paused after an answer that did not come
    /// ([`super::Breaker`]), shared by every caller.
    static BREAKER: std::sync::Mutex<super::Breaker> = std::sync::Mutex::new(super::Breaker::new());

    fn paused() -> bool {
        BREAKER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open(std::time::Instant::now())
    }

    fn pause() {
        BREAKER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .trip(std::time::Instant::now());
    }

    /// What a call says when it is not made because the last one stalled.
    const PAUSED: Unavailable =
        Unavailable::Failed("the GNOME Shell extension did not answer a moment ago");

    /// `List`'s answer as it is on the wire: `aa{sv}`.
    pub type RawList = Vec<HashMap<String, OwnedValue>>;

    /// The extension on the session bus, asked from blocking code.
    pub struct Bus;

    impl Extension for Bus {
        fn list(&self) -> Result<Vec<Window>, Unavailable> {
            on_a_thread(async { list(&connection().await?).await })
        }

        fn hit(&self, at: (i32, i32)) -> Result<Vec<(u64, u32)>, Unavailable> {
            on_a_thread(async move { hit_test(&connection().await?, at).await })
        }

        fn activate(&self, id: u64) -> Result<bool, Unavailable> {
            on_a_thread(async move { activate(&connection().await?, id).await })
        }
    }

    fn text(props: &HashMap<String, OwnedValue>, key: &str) -> String {
        match props.get(key).map(|v| &**v) {
            Some(Value::Str(s)) => s.as_str().to_string(),
            _ => String::new(),
        }
    }

    fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
        matches!(props.get(key).map(|v| &**v), Some(Value::Bool(true)))
    }

    fn unsigned(props: &HashMap<String, OwnedValue>, key: &str) -> u32 {
        match props.get(key).map(|v| &**v) {
            Some(Value::U32(n)) => *n,
            Some(Value::I32(n)) => u32::try_from(*n).unwrap_or(0),
            _ => 0,
        }
    }

    fn signed(props: &HashMap<String, OwnedValue>, key: &str) -> i32 {
        match props.get(key).map(|v| &**v) {
            Some(Value::I32(n)) => *n,
            Some(Value::U32(n)) => i32::try_from(*n).unwrap_or(0),
            _ => 0,
        }
    }

    /// The windows of `List`'s answer, in the order said (bottom first). An
    /// entry with no id is dropped; any other property of the wrong kind or
    /// missing reads as nothing (no size, not focused, no pid), never as
    /// something else, so every rule that wants more than that says no.
    pub fn parse_list(raw: RawList) -> Vec<Window> {
        raw.into_iter()
            .filter_map(|props| {
                let id = match props.get("id").map(|v| &**v) {
                    Some(Value::U64(id)) if *id != 0 => *id,
                    _ => return None,
                };
                Some(Window {
                    id,
                    title: text(&props, "title"),
                    app: text(&props, "app"),
                    pid: unsigned(&props, "pid"),
                    frame: (
                        signed(&props, "x"),
                        signed(&props, "y"),
                        unsigned(&props, "width"),
                        unsigned(&props, "height"),
                    ),
                    buffer: {
                        let (w, h) = (
                            unsigned(&props, "buffer-width"),
                            unsigned(&props, "buffer-height"),
                        );
                        (w > 0 && h > 0)
                            .then(|| (signed(&props, "buffer-x"), signed(&props, "buffer-y"), w, h))
                    },
                    minimized: flag(&props, "minimized"),
                    hidden: flag(&props, "hidden"),
                    focus: flag(&props, "focus"),
                    stack: unsigned(&props, "stack"),
                })
            })
            .collect()
    }

    /// Whether a D-Bus error says nobody serves the extension's name.
    pub fn is_missing(error_name: &str) -> bool {
        matches!(
            error_name,
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
                | "org.freedesktop.DBus.Error.UnknownObject"
                | "org.freedesktop.DBus.Error.UnknownInterface"
                | "org.freedesktop.DBus.Error.UnknownMethod"
        )
    }

    async fn ask<T, B>(conn: &Connection, method: &str, body: &B) -> Result<T, Unavailable>
    where
        B: serde::ser::Serialize + zbus::zvariant::DynamicType,
        T: serde::de::DeserializeOwned + zbus::zvariant::Type,
    {
        if paused() {
            return Err(PAUSED);
        }
        let reply = tokio::time::timeout(
            CALL_WAIT,
            conn.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body),
        )
        .await
        .map_err(|_| {
            pause();
            Unavailable::Failed("the GNOME Shell extension did not answer")
        })?;
        match reply {
            Ok(message) => message.body().deserialize::<T>().map_err(|_| {
                Unavailable::Failed("the GNOME Shell extension's answer was unreadable")
            }),
            Err(zbus::Error::MethodError(name, _, _)) if is_missing(name.as_str()) => {
                Err(Unavailable::NotInstalled)
            }
            Err(zbus::Error::MethodError(_, _, _)) => {
                Err(Unavailable::Failed("the GNOME Shell extension refused"))
            }
            Err(_) => Err(Unavailable::Failed(
                "the GNOME Shell extension could not be asked",
            )),
        }
    }

    /// `List`, on a connection.
    pub async fn list(conn: &Connection) -> Result<Vec<Window>, Unavailable> {
        Ok(parse_list(ask::<RawList, _>(conn, "List", &()).await?))
    }

    /// `HitTest`, on a connection.
    pub async fn hit_test(
        conn: &Connection,
        at: (i32, i32),
    ) -> Result<Vec<(u64, u32)>, Unavailable> {
        ask::<Vec<(u64, u32)>, _>(conn, "HitTest", &(at.0, at.1)).await
    }

    /// `Activate`, on a connection.
    pub async fn activate(conn: &Connection, id: u64) -> Result<bool, Unavailable> {
        ask::<bool, _>(conn, "Activate", &(id,)).await
    }

    /// The one session-bus connection the blocking calls share, made on
    /// first use on [`runtime`] (which it needs to live as long as) and
    /// dropped when a call fails any way but a D-Bus error from the
    /// extension, so the next call connects afresh.
    static CONNECTION: tokio::sync::Mutex<Option<Connection>> = tokio::sync::Mutex::const_new(None);

    async fn connection() -> Result<Connection, Unavailable> {
        let mut held = CONNECTION.lock().await;
        if let Some(conn) = &*held {
            return Ok(conn.clone());
        }
        let conn = tokio::time::timeout(CALL_WAIT, Connection::session())
            .await
            .map_err(|_| Unavailable::Failed("the session bus did not answer"))?
            .map_err(|_| Unavailable::Failed("no session bus"))?;
        *held = Some(conn.clone());
        Ok(conn)
    }

    /// The runtime the shared connection lives on: one for the life of the
    /// program, with a worker of its own, so a call can be made from blocking
    /// code that is or is not inside another runtime.
    fn runtime() -> Option<&'static tokio::runtime::Runtime> {
        static RUNTIME: std::sync::OnceLock<Option<tokio::runtime::Runtime>> =
            std::sync::OnceLock::new();
        RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("gnome-ext-call")
                    .enable_all()
                    .build()
                    .ok()
            })
            .as_ref()
    }

    /// Run `fut` to its end on [`runtime`] and wait for it here. A bus that
    /// fails (not the extension's own error) is dropped so the next call
    /// connects afresh.
    fn on_a_thread<T: Send + 'static>(
        fut: impl std::future::Future<Output = Result<T, Unavailable>> + Send + 'static,
    ) -> Result<T, Unavailable> {
        if paused() {
            return Err(PAUSED);
        }
        let runtime = runtime().ok_or_else(gone)?;
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let result = fut.await;
            if matches!(result, Err(Unavailable::Failed(_))) {
                *CONNECTION.lock().await = None;
            }
            let _ = tx.send(result);
        });
        // The connect and the call each wait at most CALL_WAIT.
        match rx.recv_timeout(CALL_WAIT * 3) {
            Ok(result) => {
                if matches!(result, Err(Unavailable::Failed(_))) {
                    pause();
                }
                result
            }
            Err(_) => {
                pause();
                Err(gone())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn owned(value: Value<'_>) -> OwnedValue {
            OwnedValue::try_from(value).unwrap()
        }

        #[allow(clippy::too_many_arguments)]
        fn entry(
            id: u64,
            title: &str,
            app: &str,
            pid: u32,
            frame: (i32, i32, u32, u32),
            minimized: bool,
            focus: bool,
            stack: u32,
        ) -> HashMap<String, OwnedValue> {
            HashMap::from([
                ("id".to_string(), owned(Value::from(id))),
                ("title".to_string(), owned(Value::from(title))),
                ("app".to_string(), owned(Value::from(app))),
                ("pid".to_string(), owned(Value::from(pid))),
                ("x".to_string(), owned(Value::from(frame.0))),
                ("y".to_string(), owned(Value::from(frame.1))),
                ("width".to_string(), owned(Value::from(frame.2))),
                ("height".to_string(), owned(Value::from(frame.3))),
                ("minimized".to_string(), owned(Value::from(minimized))),
                ("hidden".to_string(), owned(Value::from(minimized))),
                ("focus".to_string(), owned(Value::from(focus))),
                ("stack".to_string(), owned(Value::from(stack))),
            ])
        }

        #[test]
        fn a_windows_buffer_is_read_when_said_and_none_when_not() {
            let mut said = entry(4, "t", "app", 1, (319, 101, 640, 480), false, true, 0);
            said.insert("buffer-x".into(), owned(Value::from(305i32)));
            said.insert("buffer-y".into(), owned(Value::from(87i32)));
            said.insert("buffer-width".into(), owned(Value::from(668u32)));
            said.insert("buffer-height".into(), owned(Value::from(508u32)));
            let not_said = entry(5, "t", "app", 1, (0, 0, 640, 480), false, false, 1);
            let windows = parse_list(vec![said, not_said]);
            assert_eq!(windows[0].buffer, Some((305, 87, 668, 508)));
            assert_eq!(windows[1].buffer, None);
        }

        #[test]
        fn the_extensions_list_is_read_in_the_order_said() {
            let windows = parse_list(vec![
                entry(
                    5,
                    "Files",
                    "org.gnome.Nautilus",
                    410,
                    (-20, 30, 800, 600),
                    false,
                    false,
                    0,
                ),
                entry(9, "wev", "wev", 77, (100, 120, 640, 480), true, true, 1),
            ]);
            assert_eq!(windows.len(), 2);
            assert_eq!(windows[0].id, 5, "bottom of the stack first, as said");
            assert_eq!(
                windows[0].frame,
                (-20, 30, 800, 600),
                "a negative corner is kept"
            );
            assert_eq!(windows[0].app, "org.gnome.Nautilus");
            assert!(!windows[0].focus && !windows[0].minimized);
            assert_eq!(windows[1].title, "wev");
            assert_eq!(windows[1].pid, 77);
            assert!(windows[1].focus && windows[1].minimized && windows[1].hidden);
            assert_eq!(windows[1].stack, 1);
        }

        #[test]
        fn what_is_missing_or_of_the_wrong_kind_reads_as_nothing_and_a_window_without_an_id_is_none()
         {
            let odd: HashMap<String, OwnedValue> = HashMap::from([
                ("id".to_string(), owned(Value::from(7u64))),
                ("title".to_string(), owned(Value::from(5u32))),
                ("focus".to_string(), owned(Value::from("true"))),
                ("width".to_string(), owned(Value::from(-4i32))),
                ("height".to_string(), owned(Value::from("tall"))),
                ("pid".to_string(), owned(Value::from("x"))),
            ]);
            let no_id: HashMap<String, OwnedValue> =
                HashMap::from([("title".to_string(), owned(Value::from("x")))]);
            let zero_id: HashMap<String, OwnedValue> =
                HashMap::from([("id".to_string(), owned(Value::from(0u64)))]);
            let wrong_id: HashMap<String, OwnedValue> =
                HashMap::from([("id".to_string(), owned(Value::from(7u32)))]);
            let windows = parse_list(vec![odd, no_id, zero_id, wrong_id]);
            assert_eq!(
                windows.len(),
                1,
                "only an id that is a t and not zero is a window"
            );
            let w = &windows[0];
            assert_eq!((w.title.as_str(), w.pid, w.focus), ("", 0, false));
            assert_eq!((w.frame.2, w.frame.3), (0, 0));
            assert!(parse_list(Vec::new()).is_empty());
        }

        #[test]
        fn only_a_name_nobody_serves_is_not_installed() {
            for gone in [
                "org.freedesktop.DBus.Error.ServiceUnknown",
                "org.freedesktop.DBus.Error.NameHasNoOwner",
                "org.freedesktop.DBus.Error.UnknownObject",
                "org.freedesktop.DBus.Error.UnknownInterface",
                "org.freedesktop.DBus.Error.UnknownMethod",
            ] {
                assert!(is_missing(gone), "{gone}");
            }
            assert!(!is_missing("org.freedesktop.DBus.Error.AccessDenied"));
            assert!(!is_missing("org.freedesktop.DBus.Error.NoReply"));
            assert!(!is_missing(""));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::window_guard::{WindowGuard, aim};
    use std::cell::RefCell;
    use std::time::Instant;

    fn win(id: u64, pid: u32, frame: Rect, focus: bool) -> Window {
        Window {
            id,
            title: format!("w{id}"),
            app: "app".into(),
            pid,
            frame,
            focus,
            ..Window::default()
        }
    }

    /// A 640x480 window with a client-side shadow of 14 pixels each side.
    fn shadowed() -> Window {
        Window {
            buffer: Some((305, 87, 668, 508)),
            ..win(1, 7, (319, 101, 640, 480), true)
        }
    }

    #[test]
    fn a_shadowed_windows_picture_is_cut_to_its_frame_and_its_pointer_moved_by_the_shadow() {
        let w = shadowed();
        assert_eq!(frame_offset(&w), (14, 14));
        assert_eq!(frame_in_picture(&w, (668, 508)), Some((14, 14, 640, 480)));
        // Mutter's buffer a pixel off the picture (509 tall, 508 drawn): the
        // frame's own size, not scaled down to an odd one.
        let rounded = Window {
            buffer: Some((305, 89, 668, 509)),
            ..win(1, 7, (319, 101, 640, 480), true)
        };
        assert_eq!(
            frame_in_picture(&rounded, (668, 508)),
            Some((14, 12, 640, 480))
        );
        // At twice the scale the cut is twice as far in and as big.
        assert_eq!(
            frame_in_picture(&w, (1336, 1016)),
            Some((28, 28, 1280, 960))
        );
    }

    #[test]
    fn a_picture_is_left_whole_when_it_is_the_frame_or_cannot_be_told() {
        // No shadow: the buffer is the frame.
        let plain = Window {
            buffer: Some((319, 101, 640, 480)),
            ..win(1, 7, (319, 101, 640, 480), true)
        };
        assert_eq!(frame_offset(&plain), (0, 0));
        assert_eq!(frame_in_picture(&plain, (640, 480)), None);
        // The buffer not said (an older extension).
        let unknown = win(1, 7, (319, 101, 640, 480), true);
        assert_eq!(frame_offset(&unknown), (0, 0));
        assert_eq!(frame_in_picture(&unknown, (668, 508)), None);
        // A buffer that does not hold the frame is not believed.
        let wrong = Window {
            buffer: Some((400, 101, 100, 100)),
            ..win(1, 7, (319, 101, 640, 480), true)
        };
        assert_eq!(frame_offset(&wrong), (0, 0));
        assert_eq!(frame_in_picture(&wrong, (100, 100)), None);
        // A picture of another shape (the whole screen's stream) is not cut.
        assert_eq!(frame_in_picture(&shadowed(), (1280, 720)), None);
    }

    /// An extension that says what it was told, and notes what it was asked.
    #[derive(Default)]
    struct Fake {
        windows: RefCell<Vec<Window>>,
        chain: RefCell<Vec<(u64, u32)>>,
        down: RefCell<bool>,
        asked_hits: RefCell<Vec<(i32, i32)>>,
        activated: RefCell<Vec<u64>>,
    }

    impl Extension for &Fake {
        fn list(&self) -> Result<Vec<Window>, Unavailable> {
            if *self.down.borrow() {
                return Err(Unavailable::NotInstalled);
            }
            Ok(self.windows.borrow().clone())
        }
        fn hit(&self, at: (i32, i32)) -> Result<Vec<(u64, u32)>, Unavailable> {
            if *self.down.borrow() {
                return Err(Unavailable::NotInstalled);
            }
            self.asked_hits.borrow_mut().push(at);
            Ok(self.chain.borrow().clone())
        }
        fn activate(&self, id: u64) -> Result<bool, Unavailable> {
            self.activated.borrow_mut().push(id);
            Ok(true)
        }
    }

    fn lease(id: u64, device: &str) -> Lease {
        let (lease, _) =
            window_target::register_native(&window_target::gnome_native(id), device, None).unwrap();
        lease
    }

    #[test]
    fn a_window_belongs_to_itself_and_to_a_dialog_or_popup_of_its_own_program_and_to_nothing_else()
    {
        // The lent window under the point.
        assert!(chain_belongs(&[(5, 100)], 5));
        // A dialog of it (then it) of the same program.
        assert!(chain_belongs(&[(8, 100), (5, 100)], 5));
        assert!(chain_belongs(&[(9, 100), (8, 100), (5, 100)], 5));
        // Another window.
        assert!(!chain_belongs(&[(6, 100)], 5));
        // A dialog of another window.
        assert!(!chain_belongs(&[(8, 100), (6, 100)], 5));
        // A window of another program that names it as its parent.
        assert!(!chain_belongs(&[(8, 200), (5, 100)], 5));
        // Not a window under the point.
        assert!(!chain_belongs(&[], 5));
        // A program that is not known (pid zero), or an id that is none.
        assert!(!chain_belongs(&[(5, 0)], 5));
        assert!(!chain_belongs(&[(0, 100)], 0));
        assert!(!chain_belongs(&[(5, 100)], 0));
        // The lent window past the cut is not found.
        let mut deep: Vec<(u64, u32)> = (100..100 + MOST_OWNERS as u64).map(|id| (id, 7)).collect();
        deep.push((5, 7));
        assert!(!chain_belongs(&deep, 5));
        deep.pop();
        deep[MOST_OWNERS - 1] = (5, 7);
        assert!(chain_belongs(&deep, 5));
    }

    #[test]
    fn the_guards_space_holds_every_pixel_exactly_and_a_frame_that_does_not_fit_is_none() {
        assert_eq!(biased((10, 20, 300, 200)), Some((16394, 16404, 300, 200)));
        assert_eq!(biased((-16384, -16384, 5, 5)), Some((0, 0, 5, 5)));
        assert_eq!(biased((-16385, 0, 5, 5)), None);
        assert_eq!(biased((0, -16385, 5, 5)), None);
        assert_eq!(biased((49150, 0, 5, 5)), Some((65534, 16384, 5, 5)));
        assert_eq!(biased((49151, 0, 5, 5)), None, "past the range");
        assert_eq!(unbiased((16394, 16404)), (10, 20));
        // The guard's own placement, over this space, says the pixel itself.
        let frame = biased((100, 50, 640, 480));
        for (px, py) in [(0u16, 0u16), (65535, 65535), (32768, 12345)] {
            let at = aim(frame, Some(GUARD_SPACE), (px, py)).unwrap();
            assert_eq!((i32::from(at.norm.0), i32::from(at.norm.1)), at.pixel);
            // And the pixel is in the window, inside its frame.
            let desktop = unbiased(at.pixel);
            assert!(frame_contains((100, 50, 640, 480), desktop));
        }
    }

    #[test]
    fn a_pixel_the_guard_sent_is_placed_in_the_window_and_one_it_no_longer_holds_is_dropped() {
        let frame = (100, 50, 640, 480);
        let norm = |x: i32, y: i32| ((x + BIAS) as u16, (y + BIAS) as u16);
        assert_eq!(local_pointer(norm(100, 50), frame), Some((0, 0)));
        assert_eq!(local_pointer(norm(739, 529), frame), Some((639, 479)));
        assert_eq!(local_pointer(norm(300, 200), frame), Some((200, 150)));
        // The window moved or shrank: not clamped to an edge, dropped.
        assert_eq!(local_pointer(norm(740, 529), frame), None);
        assert_eq!(local_pointer(norm(99, 50), frame), None);
        assert_eq!(local_pointer(norm(300, 200), (400, 50, 640, 480)), None);
        // A monitor left of the primary.
        assert_eq!(
            local_pointer(norm(-1900, 10), (-1920, 0, 1920, 1080)),
            Some((20, 10))
        );
    }

    #[test]
    fn the_guards_questions_are_answered_from_the_list_and_the_pick_and_unknown_is_no() {
        let device = "gnome-windows-os-device";
        let lease = lease(41_001, device);
        let fake = Fake::default();
        *fake.windows.borrow_mut() = vec![
            win(41_000, 90, (0, 0, 1280, 720), false),
            win(41_001, 100, (100, 50, 640, 480), true),
        ];
        let os = GnomeOs(&fake);
        assert!(os.lease_holds(&lease));
        assert_eq!(os.frame(&lease), Some((16484, 16434, 640, 480)));
        assert!(os.front_is_lent(&lease));
        // The pick asked about the desktop's own pixel, not the guard's.
        *fake.chain.borrow_mut() = vec![(41_001, 100)];
        assert!(os.hit_is_lent(&lease, (16484 + 10, 16434 + 20)));
        assert_eq!(*fake.asked_hits.borrow(), [(110, 70)]);
        // A window over it, one of another program that names it, and
        // something that is no window at all.
        *fake.chain.borrow_mut() = vec![(41_000, 90)];
        assert!(!os.hit_is_lent(&lease, (16500, 16500)));
        *fake.chain.borrow_mut() = vec![(41_002, 200), (41_001, 100)];
        assert!(!os.hit_is_lent(&lease, (16500, 16500)));
        fake.chain.borrow_mut().clear();
        assert!(!os.hit_is_lent(&lease, (16500, 16500)));
        // Raising asks for this window only, and only while the lease stands.
        os.raise(&lease);
        assert_eq!(*fake.activated.borrow(), [41_001]);
        // Not focused: not in front.
        fake.windows.borrow_mut()[1].focus = false;
        assert!(!os.front_is_lent(&lease));
        // Minimised or put away: no frame, nothing in front.
        fake.windows.borrow_mut()[1].focus = true;
        fake.windows.borrow_mut()[1].minimized = true;
        assert_eq!(os.frame(&lease), None);
        assert!(!os.front_is_lent(&lease));
        fake.windows.borrow_mut()[1].minimized = false;
        fake.windows.borrow_mut()[1].hidden = true;
        assert_eq!(os.frame(&lease), None);
        fake.windows.borrow_mut()[1].hidden = false;
        // A program with no pid is not trusted to be in front.
        fake.windows.borrow_mut()[1].pid = 0;
        assert!(!os.front_is_lent(&lease));
        fake.windows.borrow_mut()[1].pid = 100;
        // A window of no size has no frame.
        fake.windows.borrow_mut()[1].frame = (100, 50, 0, 480);
        assert_eq!(os.frame(&lease), None);
        fake.windows.borrow_mut()[1].frame = (100, 50, 640, 480);
        // The window is not in the list: closed.
        fake.windows.borrow_mut().pop();
        assert_eq!(os.frame(&lease), None);
        assert!(!os.front_is_lent(&lease));
        // The extension gone: every answer is no.
        fake.windows
            .borrow_mut()
            .push(win(41_001, 100, (100, 50, 640, 480), true));
        assert!(os.front_is_lent(&lease));
        *fake.down.borrow_mut() = true;
        assert_eq!(os.frame(&lease), None);
        assert!(!os.front_is_lent(&lease));
        assert!(!os.hit_is_lent(&lease, (16500, 16500)));
        // A lease that does not stand any more holds, finds and raises nothing.
        window_target::unregister(&lease);
        *fake.down.borrow_mut() = false;
        let before = fake.activated.borrow().len();
        assert!(!os.lease_holds(&lease));
        assert_eq!(os.frame(&lease), None);
        os.raise(&lease);
        assert_eq!(fake.activated.borrow().len(), before);
        // A Windows lease is no GNOME window.
        let handle = Lease {
            native: String::new(),
            ..lease.clone()
        };
        assert!(!os.hit_is_lent(&handle, (16500, 16500)));
    }

    /// The whole of it through the guard: a click aimed at a spot another
    /// window covers is dropped, one inside the lent window is sent as the
    /// pixel the guard tested.
    #[test]
    fn a_click_through_the_guard_goes_where_the_lent_window_is_and_not_where_another_covers_it() {
        let device = "gnome-windows-guard-device";
        let lease = lease(42_001, device);
        let fake = Fake::default();
        *fake.windows.borrow_mut() = vec![win(42_001, 100, (100, 50, 640, 480), true)];
        let mut guard = WindowGuard::new(lease.clone(), GnomeOs(&fake), Some(GUARD_SPACE));
        let now = Instant::now();
        // The point the viewer shows at the picture's middle.
        let middle = InputPacket::MouseMoveAbs {
            x_norm: 32768,
            y_norm: 32768,
        };
        // Another window covers it: the move is dropped, so is the click.
        *fake.chain.borrow_mut() = vec![(42_000, 90)];
        assert!(guard.admit(middle.clone(), now).send.is_empty());
        let out = guard.admit(
            InputPacket::MouseClick {
                button: 0,
                pressed: true,
            },
            now,
        );
        assert!(
            !out.send
                .iter()
                .any(|p| matches!(p, InputPacket::MouseClick { pressed: true, .. })),
            "a click through a covering window is not sent"
        );
        // Nothing covers it: the lent window is there. The move says the
        // pixel itself, which the worker places in the window.
        *fake.chain.borrow_mut() = vec![(42_001, 100)];
        let out = guard.admit(middle, now);
        let InputPacket::MouseMoveAbs { x_norm, y_norm } = out.send[0] else {
            panic!("a move is sent: {:?}", out.send);
        };
        let local = local_pointer((x_norm, y_norm), (100, 50, 640, 480)).unwrap();
        assert_eq!(local, (320, 240), "the middle of a 640x480 window");
        let out = guard.admit(
            InputPacket::MouseClick {
                button: 0,
                pressed: true,
            },
            now,
        );
        assert!(
            out.send
                .iter()
                .any(|p| matches!(p, InputPacket::MouseClick { pressed: true, .. }))
        );
        // The keyboard is with another window: a key is not sent, and the
        // window is asked to come forward.
        fake.windows.borrow_mut()[0].focus = false;
        let out = guard.admit(
            InputPacket::KeyEvent {
                key: 30,
                pressed: true,
            },
            now,
        );
        assert!(
            !out.send
                .iter()
                .any(|p| matches!(p, InputPacket::KeyEvent { pressed: true, .. }))
        );
        assert_eq!(*fake.activated.borrow(), [42_001]);
        window_target::unregister(&lease);
    }

    #[test]
    fn a_packet_the_guard_let_through_is_told_to_mutter_as_the_injector_tells_the_portal() {
        let click = |button, pressed| window_events(&InputPacket::MouseClick { button, pressed });
        assert_eq!(
            click(0, true),
            [PortalEvent::Button {
                code: 0x110,
                pressed: true
            }]
        );
        assert_eq!(
            click(2, false),
            [PortalEvent::Button {
                code: 0x111,
                pressed: false
            }],
            "right is BTN_RIGHT"
        );
        assert_eq!(click(1, true).len(), 1);
        assert_eq!(click(4, true).len(), 1);
        assert!(click(5, true).is_empty(), "no such button");
        // Notches: up is the wheel's positive, down is the portal's.
        assert_eq!(
            window_events(&InputPacket::MouseScroll {
                dx: 1,
                dy: 2,
                fine: None
            }),
            [
                PortalEvent::Scroll { axis: 0, steps: -2 },
                PortalEvent::Scroll { axis: 1, steps: 1 }
            ]
        );
        // Fine turns go by the 120ths, and the notches are not counted twice.
        assert_eq!(
            window_events(&InputPacket::MouseScroll {
                dx: 3,
                dy: 3,
                fine: Some((0, -60))
            }),
            [PortalEvent::FineScroll { axis: 0, v120: 60 }]
        );
        assert!(
            window_events(&InputPacket::MouseScroll {
                dx: 0,
                dy: 0,
                fine: None
            })
            .is_empty()
        );
        assert_eq!(
            window_events(&InputPacket::KeyEvent {
                key: 30,
                pressed: true
            }),
            [PortalEvent::Key {
                code: 30,
                pressed: true
            }]
        );
        assert_eq!(window_events(&InputPacket::Text("a".into())).len(), 2);
        // A move is placed by `local_pointer`, not here; nothing else is input.
        assert!(
            window_events(&InputPacket::MouseMoveAbs {
                x_norm: 1,
                y_norm: 1
            })
            .is_empty()
        );
        assert!(window_events(&InputPacket::MouseMove { dx: 1, dy: 1 }).is_empty());
        assert!(window_events(&InputPacket::RequestKeyframe).is_empty());
    }

    #[test]
    fn a_press_or_a_scroll_waits_for_a_move_that_placed_the_pointer_and_a_release_never_does() {
        let press = PortalEvent::Button {
            code: 0x110,
            pressed: true,
        };
        let release = PortalEvent::Button {
            code: 0x110,
            pressed: false,
        };
        let scroll = PortalEvent::Scroll { axis: 0, steps: 1 };
        let fine = PortalEvent::FineScroll { axis: 0, v120: 30 };
        let key = PortalEvent::Key {
            code: 30,
            pressed: true,
        };
        let sym = PortalEvent::Keysym {
            sym: 97,
            pressed: true,
        };
        let mut placement = Placement::default();
        // Nothing has placed the pointer yet: it may be anywhere, in another
        // window of the owner's. Presses and scrolls wait; releases, keys
        // (which go where the keyboard is, and the guard sends them only
        // while it is in the lent window) and text do not.
        for event in [&press, &scroll, &fine] {
            assert!(!placement.allows(event), "{event:?}");
        }
        for event in [&release, &key, &sym] {
            assert!(placement.allows(event), "{event:?}");
        }
        // A move the window does not hold (dropped, the extension slow, the
        // window hidden) places nothing, and the answer says not to tell mutter.
        assert_eq!(placement.moved(None), None);
        assert!(!placement.allows(&press));
        // One that lands places the pointer.
        assert_eq!(placement.moved(Some((3, 4))), Some((3, 4)));
        for event in [&press, &scroll, &fine, &release, &key] {
            assert!(placement.allows(event), "{event:?}");
        }
        // A later move that is dropped takes the placing back.
        assert_eq!(placement.moved(None), None);
        assert!(!placement.allows(&press));
        assert!(!placement.allows(&scroll));
        assert!(placement.allows(&release));
        // A move mutter did not take takes it back too.
        placement.moved(Some((1, 1)));
        assert!(placement.allows(&press));
        placement.failed();
        assert!(!placement.allows(&press));
    }

    #[test]
    fn only_what_lets_go_is_told_as_a_session_ends() {
        let ev = |pressed| {
            [
                PortalEvent::Button { code: 1, pressed },
                PortalEvent::Key { code: 1, pressed },
                PortalEvent::Keysym { sym: 1, pressed },
            ]
        };
        assert!(ev(false).iter().all(is_release));
        assert!(!ev(true).iter().any(is_release));
        assert!(!is_release(&PortalEvent::Scroll { axis: 0, steps: 1 }));
        assert!(!is_release(&PortalEvent::Absolute {
            x_norm: 0,
            y_norm: 0
        }));
    }

    #[test]
    fn a_stalled_extension_costs_one_wait_then_nothing_for_a_while() {
        use std::time::{Duration, Instant};
        let mut breaker = Breaker::new();
        let t0 = Instant::now();
        assert!(!breaker.open(t0), "calls are made until one stalls");
        breaker.trip(t0);
        assert!(breaker.open(t0), "paused at once");
        assert!(breaker.open(t0 + STALL_PAUSE - Duration::from_millis(1)));
        assert!(
            !breaker.open(t0 + STALL_PAUSE),
            "asked again once the pause is over"
        );
        // A second stall pauses again from then.
        let later = t0 + STALL_PAUSE + Duration::from_secs(1);
        breaker.trip(later);
        assert!(breaker.open(later + Duration::from_secs(1)));
        assert!(!breaker.open(later + STALL_PAUSE));
        // Short enough to matter: the pause is a couple of seconds.
        assert!(STALL_PAUSE >= Duration::from_secs(1) && STALL_PAUSE <= Duration::from_secs(5));
    }

    #[test]
    fn work_that_blocks_runs_in_place_or_off_the_worker_and_gives_its_answer() {
        // No runtime.
        assert_eq!(blocking(|| 5), 5);
        // A runtime with one thread: in place.
        let one = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert_eq!(one.block_on(async { blocking(|| 6) }), 6);
        // A runtime with several: off the worker, inside a task.
        let many = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .build()
            .unwrap();
        assert_eq!(
            many.block_on(async { tokio::spawn(async { blocking(|| 7) }).await.unwrap() }),
            7
        );
    }

    #[test]
    fn without_the_extension_nothing_is_answered_on_this_build() {
        // The test machine has no extension on any bus it can reach, and the
        // free functions are the ones the guard's machine uses.
        let device = "gnome-windows-free-fns-device";
        let lease = lease(43_001, device);
        if !answers() {
            assert_eq!(frame(&lease), None);
            assert!(!front_is_lent(&lease));
            assert!(!hit_is_lent(&lease, (16500, 16500)));
            raise(&lease);
        }
        window_target::unregister(&lease);
    }
}
