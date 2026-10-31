//! The computer's clipboard to connected phones as it changes (T10b).
//!
//! A phone used to see the computer's clipboard only by asking for it every
//! 4 s during a session. Now the server watches the clipboard and pushes
//! new text to every live control connection whose phone may receive
//! pushes and has the Clipboard permission, so a phone holding a link in the
//! background gets it too. With "Images to phones' clipboards" on (M3), an
//! image the clipboard holds instead is told to those phones as
//! `{image: {id, size}}`, and each fetches it as N4's session does.
//!
//! The owner's other computers (2g) get it the same way when
//! `computer_clipboard` is on: those linked to this one as its clients, and
//! those this one keeps a link to (`peer_computers`). Its image too (BK3),
//! without the phones' switch: told to the first, which fetch it, and sent
//! on a stream each of the others opens. Never back to the device it came
//! from.
//!
//! With the shared keyboard and mouse, the clipboard also follows the
//! mouse ([`push_now`]): its text, or with no text its image (BK3), goes to
//! a computer as the cursor goes there, and comes back as it comes home,
//! marked ([`FOLLOWS`]) so the computer it reaches takes it with
//! "Clipboard follows the mouse" on there, even with `computer_clipboard`
//! off ([`takes_follow`]). An image rides the link's transfer streams, as
//! files do, and a later switch drops one still on its way ([`switch`]).
//!
//! The window that shows another computer's screen (BK7) moves it the same
//! way at its focus (PC3, [`push_for_window`], [`ask_back`]): while it
//! controls that computer, this computer's clipboard goes there as the
//! window gains the focus, and that computer's comes back as it loses it.
//! Those pushes carry [`WINDOW`] beside [`FOLLOWS`], and are taken with
//! `computer_view_clipboard` on, from the computer that controls this one
//! through its window now, or from the computer this one asked back
//! ([`window_taken`]); never by the shared keyboard and mouse's rules.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// The longest clipboard text pushed. A phone asking in a session still
/// gets longer text; a push goes to every phone, so it stays small.
pub(crate) const MAX_PUSH: usize = 256 * 1024;

/// The text a clipboard push from another computer carries in `body`:
/// `Ok(None)` for none (an image, or nothing), `Err(len)` for a text of
/// `len` bytes over [`MAX_PUSH`], which no computer sends.
pub(crate) fn pushed_text(body: &serde_json::Value) -> Result<Option<&str>, usize> {
    match body.get("content").and_then(|v| v.as_str()) {
        Some(text) if text.len() > MAX_PUSH => Err(text.len()),
        text => Ok(text),
    }
}

/// Which way a clipboard text over [`MAX_PUSH`] would have gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TooBig {
    /// Another computer's, arriving on the link.
    In,
    /// This computer's, for another computer (or for every computer it
    /// shares the clipboard with: no device named).
    Out,
}

/// The owner is told of a clipboard too big at most this often, per
/// computer and way: each copy is said, but a loop of them is not.
const TOO_BIG_EVERY: Duration = Duration::from_secs(60);

/// When each notice of a clipboard too big was last shown, by computer
/// (empty: every computer) and way.
static TOO_BIG_SHOWN: LazyLock<Mutex<HashMap<(String, TooBig), Instant>>> =
    LazyLock::new(Default::default);

/// Whether a notice for `key` is due at `now`, noting it shown if it is.
fn too_big_due(
    shown: &mut HashMap<(String, TooBig), Instant>,
    key: (String, TooBig),
    now: Instant,
) -> bool {
    if shown
        .get(&key)
        .is_some_and(|at| now.duration_since(*at) < TOO_BIG_EVERY)
    {
        return false;
    }
    shown.insert(key, now);
    true
}

/// The words of a notice of a clipboard of `len` bytes too big to go
/// `way`, with the computer `name`: its summary and body.
fn too_big_words(way: TooBig, name: &str, len: usize) -> (&'static str, String) {
    let kb = len.div_ceil(1024);
    let limit = MAX_PUSH / 1024;
    match way {
        TooBig::In => (
            "A clipboard was too big to copy",
            format!("A clipboard from {name} was too big to copy ({kb} KB, over {limit} KB)"),
        ),
        TooBig::Out => (
            "The clipboard was too big to share",
            format!("The text copied was not shared with {name} ({kb} KB, over {limit} KB)"),
        ),
    }
}

/// The name the owner knows the computer `device_id` by, else the start
/// of its id. Reads `computers.json`: called only for a notice.
fn computer_name(device_id: &str) -> String {
    crate::state::state_dir()
        .ok()
        .and_then(|dir| {
            pocketdesk_core::local::computers::read(&dir)
                .computers
                .into_iter()
                .find(|c| c.device_id == device_id)
                .map(|c| c.name)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| device_id.chars().take(8).collect())
}

/// A clipboard text of `len` bytes, over [`MAX_PUSH`], that did not go
/// `way` (to or from the computer `device_id`, or with none, to every
/// computer this one shares the clipboard with): logged, and, at most
/// once a minute per computer and way, told to the owner as a desktop
/// notification and in Activity (`clipboard_too_big`).
pub(crate) fn too_big(device_id: Option<&str>, len: usize, way: TooBig) {
    let key = (device_id.unwrap_or_default().to_string(), way);
    let due = too_big_due(
        &mut TOO_BIG_SHOWN.lock().unwrap_or_else(|e| e.into_inner()),
        key,
        Instant::now(),
    );
    if !due {
        tracing::debug!(
            ?device_id,
            len,
            ?way,
            "A clipboard over the limit, not shared"
        );
        return;
    }
    tracing::warn!(
        ?device_id,
        len,
        limit = MAX_PUSH,
        ?way,
        "A clipboard over the limit, not shared"
    );
    if cfg!(test) {
        return;
    }
    crate::events::emit(
        "clipboard_too_big",
        device_id,
        serde_json::json!({ "bytes": len, "limit": MAX_PUSH, "out": way == TooBig::Out }),
        serde_json::json!({}),
    );
    let device_id = device_id.map(str::to_string);
    std::thread::spawn(move || {
        let name = match &device_id {
            Some(id) => computer_name(id),
            None => "your other computers".to_string(),
        };
        let (summary, body) = too_big_words(way, &name, len);
        crate::platform::notify(summary, &body, Duration::from_secs(10));
    });
}

/// How often the clipboard is looked at while a phone could receive it. On
/// Windows a look costs a sequence number; elsewhere it costs a flag set by
/// one `wl-paste --watch` (R6d), or runs `wl-paste` where that cannot run.
#[cfg(windows)]
const TICK: Duration = Duration::from_secs(1);
#[cfg(not(windows))]
const TICK: Duration = Duration::from_secs(2);

/// Change detection over successive reads of the clipboard's text. It keeps
/// a hash of what it saw, not the text: a large copy is not held twice.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    seen: Option<u64>,
    primed: bool,
    /// The length of the last new text left unpushed for being over
    /// [`MAX_PUSH`], until [`Watch::take_skipped`] takes it.
    skipped: Option<usize>,
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Watch {
    /// The text to push after reading `text` (`None`: no text on the
    /// clipboard). The first read only learns what is there: the phone
    /// asks for the current clipboard itself when a session starts.
    pub(crate) fn observe(&mut self, text: Option<&str>) -> Option<String> {
        let first = !self.primed;
        self.primed = true;
        let text = text.filter(|t| !t.is_empty());
        let seen = text.map(hash);
        if seen == self.seen {
            return None;
        }
        self.seen = seen;
        if first {
            return None;
        }
        match text {
            Some(t) if t.len() > MAX_PUSH => {
                self.skipped = Some(t.len());
                None
            }
            text => text.map(str::to_string),
        }
    }

    /// The length of the new text last left unpushed for being over
    /// [`MAX_PUSH`], once.
    pub(crate) fn take_skipped(&mut self) -> Option<usize> {
        self.skipped.take()
    }
}

/// The marks a copy carries on Windows that say it is private: password
/// managers set them so clipboard history, cloud sync and monitors skip it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsMarks {
    /// `ExcludeClipboardContentFromMonitorProcessing` is on the clipboard.
    pub exclude_from_monitors: bool,
    /// `Clipboard Viewer Ignore` is on the clipboard.
    pub viewer_ignore: bool,
    /// The DWORD in `CanIncludeInClipboardHistory`, when present.
    pub history: Option<u32>,
    /// The DWORD in `CanUploadToCloudClipboard`, when present.
    pub cloud: Option<u32>,
    /// One of those two was present but could not be read (another app held
    /// the clipboard): counted as private, never as not (M3 review).
    pub unread: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl WindowsMarks {
    /// Is the copy private? Then it never leaves this computer.
    pub(crate) fn private(self) -> bool {
        self.exclude_from_monitors
            || self.viewer_ignore
            || self.unread
            || self.history == Some(0)
            || self.cloud == Some(0)
    }
}

/// Is a Wayland copy private? KDE's password managers (KeePassXC among
/// them) offer `x-kde-passwordManagerHint` with it.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn private_types(types: &str) -> bool {
    types
        .lines()
        .any(|t| t.trim() == "x-kde-passwordManagerHint")
}

/// Pushes to phones (`clipboard_push`).
static TO_PHONES: AtomicBool = AtomicBool::new(true);

/// Shares with the owner's other computers (`computer_clipboard`, 2g).
static WITH_COMPUTERS: AtomicBool = AtomicBool::new(false);

/// What the watcher sends to, from the config at start; and whether it
/// runs at all.
pub(crate) fn configure(phones: bool, computers: bool) -> bool {
    TO_PHONES.store(phones, Ordering::Relaxed);
    WITH_COMPUTERS.store(computers, Ordering::Relaxed);
    phones || computers
}

/// Whether the clipboard is shared with the owner's other computers.
pub(crate) fn with_computers() -> bool {
    WITH_COMPUTERS.load(Ordering::Relaxed)
}

/// Whether a connection may be pushed the clipboard: a computer while
/// sharing with computers, a phone while pushing to phones, either with
/// the Clipboard permission.
fn may_push(device_id: &str) -> bool {
    let wanted = if crate::plugins::pair::is_computer_device(device_id) {
        with_computers()
    } else {
        TO_PHONES.load(Ordering::Relaxed)
    };
    wanted && crate::plugins::pair::permits(device_id, "kdeconnect.clipboard")
}

/// Which phone last set the computer's clipboard, and to what: that phone
/// is not sent its own text back.
static FROM_PHONE: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Note that `device_id` just set the clipboard to `text`.
pub(crate) fn note_from_phone(device_id: &str, text: &str) {
    // It crossed: kept in the history (D3), with the device it came from.
    crate::clip_history::note(text, Some(device_id));
    forget_image_sent(device_id);
    *FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), text.to_string()));
}

/// The device the clipboard's image came from, with the image's id (O7):
/// M3's push does not tell it back to that device.
static IMAGE_FROM: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Note that `device_id` just put the image `id` on the clipboard.
pub(crate) fn note_image_from(device_id: &str, id: &str) {
    forget_image_sent(device_id);
    *IMAGE_FROM.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), id.to_string()));
}

/// Forget the note: the write it was for did not happen (O7 review).
pub(crate) fn forget_image_from(id: &str) {
    let mut note = IMAGE_FROM.lock().unwrap_or_else(|e| e.into_inner());
    if note.as_ref().is_some_and(|(_, image)| image == id) {
        *note = None;
    }
}

/// Whether `device_id` may put something on this computer's clipboard (O7
/// review): a phone with Clipboard, a computer while `computer_clipboard`
/// is on, a paired KDE Connect device allowed the clipboard. Someone
/// else's phone and a guest (Files and Links only, M2b) may not.
pub(crate) fn may_set_clipboard(device_id: &str) -> bool {
    let permitted = crate::plugins::pair::permits(device_id, "kdeconnect.clipboard");
    if crate::plugins::pair::is_computer_device(device_id) {
        return with_computers() && permitted;
    }
    permitted
        || crate::state::state_dir()
            .ok()
            .and_then(|dir| {
                pocketdesk_core::local::kdeconnect_devices::permissions(&dir, device_id)
            })
            .is_some_and(|p| p.clipboard)
}

/// Whether the image `id` came from `device_id`: not told back to it, by a
/// push or by a session's ask (O7 review).
pub(crate) fn image_came_from(device_id: &str, id: &str) -> bool {
    IMAGE_FROM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|(from, image)| from == device_id && image == id)
}

/// The note, used once: the next change it explains, or not. A copy of the
/// same text later is the computer's own, and goes to that phone too.
fn take_from_phone() -> Option<(String, String)> {
    FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Whether `text` came from `device_id` (the note): not sent back to it.
fn came_from(from: Option<&(String, String)>, device_id: &str, text: &str) -> bool {
    from.is_some_and(|(id, sent)| id == device_id && sent == text)
}

/// The connections to push `text` to: those that may have it (`may`), but
/// not the phone the text came from.
pub(crate) fn targets(
    clients: Vec<Arc<dyn DeviceSender>>,
    text: &str,
    from_phone: Option<&(String, String)>,
    may: impl Fn(&dyn DeviceSender) -> bool,
) -> Vec<Arc<dyn DeviceSender>> {
    clients
        .into_iter()
        .filter(|c| !came_from(from_phone, c.device_id(), text))
        .filter(|c| may(&**c))
        .collect()
}

/// What the watcher compares: the image's marker is its id behind a byte no
/// text starts with, so text and an image never read as the same change.
fn image_key(id: &str) -> String {
    format!("\u{1}image:{id}")
}

/// The packet an image push sends (M3): what the phone's own request is
/// answered with while the clipboard holds an image.
pub(crate) fn image_packet(id: &str, size: u64) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard")
        .with_body(serde_json::json!({ "image": { "id": id, "size": size } }))
}

/// Whether a connection is told of an image (M3), with "Images to phones'
/// clipboards" on or not (`phone_images`): [`tells_image`].
fn may_push_image(device_id: &str, phone_images: bool) -> bool {
    tells_image(
        crate::plugins::pair::is_computer_device(device_id),
        phone_images,
        || may_push(device_id),
    )
}

/// Whether a connection is told of the clipboard's image: a phone while
/// "Images to phones' clipboards" is on (`phone_images`, M3), or another
/// computer (BK3), and either only if it may be pushed the clipboard now
/// (`may`: a computer while it is shared with computers), asked last.
fn tells_image(computer: bool, phone_images: bool, may: impl FnOnce() -> bool) -> bool {
    (computer || phone_images) && may()
}

/// The packet a push sends: what the phone's own request is answered with.
pub(crate) fn packet(text: &str) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard").with_body(serde_json::json!({ "content": text }))
}

/// The mark on a push that follows the mouse (shared input), in its body.
pub(crate) const FOLLOWS: &str = "follows";

/// The mark beside [`FOLLOWS`] on a push that follows the window showing
/// another computer's screen (PC3).
pub(crate) const WINDOW: &str = "view";

/// What a push at a switch follows: the shared keyboard and mouse's
/// cursor, or the focus of the window that shows another computer's screen
/// (PC3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Follow {
    Mouse,
    Window,
}

impl Follow {
    /// What it followed, as the log says it.
    fn words(self) -> &'static str {
        match self {
            Follow::Mouse => "the mouse",
            Follow::Window => "the window",
        }
    }
}

/// `body` marked as following `follow`.
fn mark(mut body: serde_json::Value, follow: Follow) -> serde_json::Value {
    body[FOLLOWS] = true.into();
    if follow == Follow::Window {
        body[WINDOW] = true.into();
    }
    body
}

/// Whether a clipboard packet's `body` follows the window (PC3): marked
/// [`FOLLOWS`] and [`WINDOW`], both JSON `true`.
pub(crate) fn of_window(body: &serde_json::Value) -> bool {
    marked(body) && body.get(WINDOW).and_then(|v| v.as_bool()) == Some(true)
}

/// What the clipboard holds at a switch, as read for another computer:
/// its text (none a password manager marked private, nor a pairing link),
/// else its image's id (none marked private), else nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Held {
    Text(String),
    Image(String),
    Nothing,
}

/// What follows the mouse to the computer `to` of what the clipboard
/// `held`: nothing that computer put here itself (`text_from`, the note of
/// the last text a device set; `image_from`, of the last image), no text
/// over [`MAX_PUSH`], and not the image last sent to it again
/// (`last_image`, that computer's).
fn what_follows(
    held: Held,
    to: &str,
    text_from: Option<&(String, String)>,
    image_from: Option<&(String, String)>,
    last_image: Option<&str>,
) -> Held {
    match held {
        Held::Text(text) if text.len() <= MAX_PUSH && !came_from(text_from, to, &text) => {
            Held::Text(text)
        }
        Held::Image(id) if !came_from(image_from, to, &id) && last_image != Some(id.as_str()) => {
            Held::Image(id)
        }
        _ => Held::Nothing,
    }
}

/// The image last sent to each computer, from the start of its send, until
/// its clipboard holds something else: the same image is never sent to a
/// computer twice in a row, nor again while it is on its way (BK3 review:
/// a switch's push and the watcher's sent it twice).
static IMAGE_SENT: LazyLock<Mutex<HashMap<String, Sent>>> = LazyLock::new(Default::default);

/// Note that the image `id` is on its way to the computer `device_id`, or
/// reached it: from the start of its send ([`forget_image_sent_if`] if it
/// fails).
pub(crate) fn note_image_sent(device_id: &str, id: &str) {
    note_sent(device_id, id, None);
}

fn note_sent(device_id: &str, id: &str, told_until: Option<std::time::Instant>) {
    // An image there now: the window's last text no longer is.
    WINDOW_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
    IMAGE_SENT.lock().unwrap_or_else(|e| e.into_inner()).insert(
        device_id.to_string(),
        Sent {
            id: id.to_string(),
            told_until,
        },
    );
}

/// How long an image told to a computer counts as on its way to it before
/// that computer fetches it: one whose settings do not take it never does.
const TOLD_WAIT: Duration = Duration::from_secs(10);

/// An image sent or told to a computer: its id, and while it is only told
/// (not fetched yet), until when it counts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    id: String,
    told_until: Option<std::time::Instant>,
}

/// The id `sent` names at `now`: a told image only until its wait ends.
fn sent_counts(sent: &Sent, now: std::time::Instant) -> Option<&str> {
    sent.told_until
        .is_none_or(|until| now < until)
        .then_some(sent.id.as_str())
}

/// Note that the image `id` was told to the computer `device_id`, for it to
/// fetch: on its way for [`TOLD_WAIT`] unless the fetch comes.
fn note_image_told(device_id: &str, id: &str) {
    note_sent(device_id, id, Some(std::time::Instant::now() + TOLD_WAIT));
}

/// A send or tell of the image `id` to `device_id` failed: its note goes,
/// unless a later one replaced it.
pub(crate) fn forget_image_sent_if(device_id: &str, id: &str) {
    let mut sent = IMAGE_SENT.lock().unwrap_or_else(|e| e.into_inner());
    if sent.get(device_id).is_some_and(|s| s.id == id) {
        sent.remove(device_id);
    }
}

/// The computer `device_id`'s clipboard holds no image of this one's now:
/// text went to it, or it sent this one something of its own. Nor is the
/// window's last text known to be there (range review I2).
fn forget_image_sent(device_id: &str) {
    IMAGE_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
    WINDOW_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
}

/// The text a window push sent each computer last (a hash), with this
/// clipboard's copy as it was read ([`copy_generation`]), while nothing
/// else went there or came from it since (range review I2).
static WINDOW_SENT: LazyLock<Mutex<HashMap<String, (u64, u64)>>> = LazyLock::new(Default::default);

/// [`WINDOW_SENT`]'s entry for `device_id`.
fn window_sent_last(device_id: &str) -> Option<(u64, u64)> {
    WINDOW_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .copied()
}

/// Each change of this computer's clipboard the watcher saw (PR #572
/// review I2), where the system keeps no sequence number (Linux), or the
/// clipboard is a test's stand-in.
static COPIES: AtomicU64 = AtomicU64::new(0);

/// Which copy this computer's clipboard holds: Windows' sequence number,
/// else [`COPIES`]. A copy made again, even of the same text, is another.
fn copy_generation() -> u64 {
    #[cfg(windows)]
    if !crate::plugins::clipboard::stand_ins_set() {
        return u64::from(crate::platform::clipboard_windows::sequence());
    }
    // With no watcher to count the copies (none started, `wl-paste`
    // missing, X11 failing), every read is a new copy, so nothing is
    // skipped as the same text (PR #572 re-review N3).
    #[cfg(not(windows))]
    if !WATCHING.load(Ordering::Relaxed) {
        return COPIES.fetch_add(1, Ordering::Relaxed) + 1;
    }
    COPIES.load(Ordering::Relaxed)
}

/// Whether a clipboard watcher counts [`COPIES`] now.
#[cfg(not(windows))]
static WATCHING: AtomicBool = AtomicBool::new(false);

/// Whether a push of `follows` (read as copy `copy`) to `device_id` is
/// skipped: a window's, not an answer, of the same text it sent there
/// last, from the same copy here (PR #572 review I2: a copy made again
/// goes).
fn window_push_skipped(
    device_id: &str,
    follow: Follow,
    answering: bool,
    follows: &Held,
    copy: u64,
) -> bool {
    follow == Follow::Window
        && !answering
        && matches!(follows, Held::Text(text) if window_sent_last(device_id) == Some((hash(text), copy)))
}

/// The image last sent to the computer `device_id`, while nothing has
/// gone to it since.
fn image_sent_last(device_id: &str) -> Option<String> {
    let now = std::time::Instant::now();
    IMAGE_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .and_then(|sent| sent_counts(sent, now))
        .map(str::to_string)
}

/// The switches of the shared keyboard and mouse this computer has seen: a
/// push for one ([`push_now`]), a follow arriving from another computer,
/// the keyboard and mouse coming home. An image still on its way for an
/// earlier one is dropped.
static SWITCHES: AtomicU64 = AtomicU64::new(0);

/// A switch: what is on its way for an earlier one is dropped. Its number.
pub(crate) fn switch() -> u64 {
    SWITCHES.fetch_add(1, Ordering::SeqCst) + 1
}

/// Whether no switch has come since the one numbered `at`.
pub(crate) fn still(at: u64) -> bool {
    SWITCHES.load(Ordering::SeqCst) == at
}

/// The keyboard and mouse came home (the driving side): an image still on
/// its way to the computer they left is dropped.
pub(crate) fn came_home() {
    switch();
}

/// The image following the mouse here now, from which computer and at
/// which switch: one at a time, the newest.
static LANDING: Mutex<Option<(String, u64)>> = Mutex::new(None);

/// An image following the mouse here, while it lands: dropped by a later
/// switch, and while it lands this computer sends that computer nothing
/// back at a switch (its clipboard is about to be that one's own).
pub(crate) struct Landing {
    from: String,
    at: u64,
}

impl Landing {
    /// An image from the computer `from` starts landing: the cursor came
    /// here, a switch.
    pub(crate) fn begin(from: &str) -> Landing {
        let mut landing = LANDING.lock().unwrap_or_else(|e| e.into_inner());
        let at = switch();
        *landing = Some((from.to_string(), at));
        Landing {
            from: from.to_string(),
            at,
        }
    }

    /// Whether it may still go onto the clipboard: no switch since.
    pub(crate) fn current(&self) -> bool {
        still(self.at)
    }
}

impl Drop for Landing {
    fn drop(&mut self) {
        let mut landing = LANDING.lock().unwrap_or_else(|e| e.into_inner());
        if landing
            .as_ref()
            .is_some_and(|(from, at)| *from == self.from && *at == self.at)
        {
            *landing = None;
        }
    }
}

/// Whether an image from the computer `from` is landing here, with no
/// switch since it began.
fn landing_from(from: &str) -> bool {
    LANDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|(landing, at)| landing == from && still(*at))
}

/// The image told to a computer as following the mouse, at which switch:
/// its fetch is served while no switch has come since.
static TOLD: Mutex<Option<(String, String, u64)>> = Mutex::new(None);

/// Whether a fetch by `device_id` of the image `id` is of the image told
/// to it at the last switch (`told`), with no switch since (`still`).
fn fetch_told(
    told: Option<&(String, String, u64)>,
    device_id: &str,
    id: &str,
    still: impl FnOnce(u64) -> bool,
) -> bool {
    told.is_some_and(|(to, image, at)| to == device_id && image == id && still(*at))
}

/// Whether the computer `device_id` may fetch the image `id` as one
/// following the mouse, with "Clipboard with my computers" off here.
pub(crate) fn fetch_follows(device_id: &str, id: &str) -> bool {
    let told = TOLD.lock().unwrap_or_else(|e| e.into_inner()).clone();
    fetch_told(told.as_ref(), device_id, id, still)
}

/// The packet an image push that follows the mouse sends to a computer
/// linked to this one as its client: [`image_packet`], marked. It fetches
/// the image ([`fetch_follows`]).
fn follow_image_packet(id: &str, size: u64, follow: Follow) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard").with_body(mark(
        serde_json::json!({ "image": { "id": id, "size": size } }),
        follow,
    ))
}

/// The packet that offers another computer this one's clipboard image on
/// the link this one keeps to it, as a phone offers its own (N4): the size
/// and the transfer's key, and no port, since that computer opens a stream
/// on the link for it. Marked when it follows the mouse or the window.
pub(crate) fn image_offer_packet(size: u64, key: &str, follows: Option<Follow>) -> NetworkPacket {
    let mut body = serde_json::json!({ "size": size, "key": key });
    if let Some(follow) = follows {
        body = mark(body, follow);
    }
    NetworkPacket::new(pocketdesk_core::protocol::clipboard::CLIPBOARD_IMAGE).with_body(body)
}

/// The packet a push that follows the mouse or the window sends:
/// [`packet`], marked.
fn follow_packet(text: &str, follow: Follow) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard")
        .with_body(mark(serde_json::json!({ "content": text }), follow))
}

/// Push this computer's clipboard to the computer `device_id` now: the
/// cursor of the shared keyboard and mouse just went there, or came back
/// from there. Its text, or with no text its image (BK3), as
/// [`what_follows`] says: never a copy a password manager marked private,
/// a pairing link, text over [`MAX_PUSH`], nor what that computer itself
/// put here (a switch back with nothing new copied). Each switch runs it
/// on a task of its own, off the input thread; an image a later switch
/// overtakes goes no further ([`switch`]).
pub(crate) async fn push_now(device_id: &str) {
    push_following(device_id, Follow::Mouse).await
}

/// The window showing the computer `device_id`'s screen gained the focus
/// while it controls that computer (PC3): this computer's clipboard goes
/// there, as [`push_now`] sends it, marked as following the window. An
/// answer still awaited from that computer is no longer taken: this
/// clipboard is the newer. An answer awaited from another computer, whose
/// window just lost the focus to this one's, is waited for first, up to
/// [`HANDOVER_WAIT`] (range review I2): its copy is the newest, and goes
/// on to this computer.
pub(crate) async fn push_for_window(device_id: &str) {
    push_for_window_in(
        &HANDING,
        device_id,
        push_following(device_id, Follow::Window),
    )
    .await
}

/// [`push_for_window`] with the hand-over's state `handing`, and `push` the
/// push itself, run once the wait is over.
async fn push_for_window_in(
    handing: &Handing,
    device_id: &str,
    push: impl std::future::Future<Output = ()>,
) {
    lock(&handing.asked).remove(device_id);
    wait_for_other_answers(handing, device_id).await;
    push.await
}

/// How close one window's focus loss and another's gain are, to be one
/// hand-over between windows (range review I2).
const HANDOVER: Duration = Duration::from_millis(300);

/// The longest a window's push waits for another computer's answer.
const HANDOVER_WAIT: Duration = Duration::from_millis(1500);

/// The longest a push waits for an answer taken to be written here.
const WRITE_WAIT: Duration = Duration::from_secs(2);

/// After another computer's answer was written to this computer's
/// clipboard, a moment for the system to settle before a push reads it.
const ANSWER_SETTLE: Duration = Duration::from_millis(100);

/// The windows' focus and the asks back, as the hand-over between windows
/// sees them (range review I2, PR #572 review I2).
#[derive(Debug, Default)]
struct Handing {
    moves: Mutex<FocusMoves>,
    /// The computers this one asked for their clipboard back, and when: one
    /// each, so a push to one does not undo an ask to another.
    asked: Mutex<HashMap<String, std::time::Instant>>,
    /// When an answer to an ask was last taken.
    answered: Mutex<Option<std::time::Instant>>,
    /// When an answer taken was last written to this clipboard.
    written: Mutex<Option<std::time::Instant>>,
}

static HANDING: LazyLock<Handing> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wait, up to [`HANDOVER_WAIT`], while a window of another computer than
/// `device_id` is losing the focus or its answer is awaited; then, if an
/// answer was taken meanwhile, until it is written here (up to
/// [`WRITE_WAIT`]) and [`ANSWER_SETTLE`] after.
async fn wait_for_other_answers(handing: &Handing, device_id: &str) {
    let start = std::time::Instant::now();
    loop {
        let now = std::time::Instant::now();
        // The focus first: an ask is noted before its loss is let go.
        let losing = lock(&handing.moves).other_losing(device_id, now);
        let pending = losing || asked_elsewhere(&lock(&handing.asked), device_id, now);
        if !pending {
            break;
        }
        if now.saturating_duration_since(start) >= HANDOVER_WAIT {
            tracing::debug!(
                "The clipboard follows the window to {device_id} without the answer it waited for"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let Some(answered) = lock(&handing.answered).filter(|at| *at >= start) else {
        return;
    };
    let waiting = std::time::Instant::now();
    while lock(&handing.written).is_none_or(|at| at < answered) {
        if waiting.elapsed() >= WRITE_WAIT {
            tracing::debug!("The answer taken was not written here in time");
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(ANSWER_SETTLE).await;
}

/// A window answer from another computer (its body `body`) was written to
/// this computer's clipboard: a push waiting on it may read the clipboard
/// now (PR #572 review I2).
pub(crate) fn window_text_landed(body: &serde_json::Value) {
    if of_window(body) {
        *lock(&HANDING.written) = Some(std::time::Instant::now());
    }
}

/// Another device's image was written to this computer's clipboard: a push
/// waiting on an answer that was that image reads the clipboard now (PR
/// #572 re-review N4), not after [`WRITE_WAIT`].
pub(crate) fn image_landed() {
    *lock(&HANDING.written) = Some(std::time::Instant::now());
}

/// Whether an answer from a computer other than `device_id` is awaited at
/// `now` from an ask made within [`HANDOVER_WAIT`]: its window lost the
/// focus just now. An older ask (its window lost the focus to another
/// program) holds no push up.
fn asked_elsewhere(
    asked: &HashMap<String, std::time::Instant>,
    device_id: &str,
    now: std::time::Instant,
) -> bool {
    asked
        .iter()
        .any(|(from, at)| from != device_id && now.saturating_duration_since(*at) < HANDOVER_WAIT)
}

/// The windows' focus, per computer they show (range review I2): a loss
/// waits [`HANDOVER`] before its ask, and a gain waits as long for a loss,
/// so a window of the same computer taking or giving the focus then is a
/// hand-over that asks and pushes nothing, whichever is heard first.
#[derive(Debug, Default)]
struct FocusMoves {
    by_device: HashMap<String, FocusMove>,
}

#[derive(Debug, Default, Clone, Copy)]
struct FocusMove {
    /// Moves on with each loss or gain, so a waiting one knows it stands.
    turn: u64,
    /// A window of it lost the focus then, its ask not sent yet.
    lost: Option<std::time::Instant>,
    /// A window of it gained the focus then.
    gained: Option<std::time::Instant>,
    /// At its last gain, whether a window was known to hold the focus
    /// (one of its own or another computer's): only then can a loss still
    /// follow (PR #572 re-review N5).
    held_before: bool,
}

impl FocusMoves {
    /// A window of `device_id` gained (`focused`) or lost the focus at
    /// `now`, in the order the windows said so: the turn to check once its
    /// wait is over, or `None` when a window of the same computer moved the
    /// other way within [`HANDOVER`] before (a hand-over between its
    /// windows), which ends that one's wait too.
    fn note(&mut self, device_id: &str, focused: bool, now: std::time::Instant) -> Option<u64> {
        let held_elsewhere = self
            .by_device
            .iter()
            .any(|(id, m)| id != device_id && m.gained.is_some());
        let m = self.by_device.entry(device_id.to_string()).or_default();
        m.turn += 1;
        if focused {
            m.held_before = held_elsewhere || m.gained.is_some();
        }
        let within = |at: Option<std::time::Instant>| {
            at.is_some_and(|at| now.saturating_duration_since(at) < HANDOVER)
        };
        let handed = if focused {
            let handed = within(m.lost.take());
            m.gained = (!handed).then_some(now);
            handed
        } else {
            let handed = within(m.gained.take());
            m.lost = (!handed).then_some(now);
            handed
        };
        (!handed).then_some(m.turn)
    }

    /// Whether the move of `device_id`'s window at `turn` still stands:
    /// none of that computer's windows moved since.
    fn stands(&self, device_id: &str, turn: u64) -> bool {
        self.by_device
            .get(device_id)
            .is_some_and(|m| m.turn == turn)
    }

    /// The loss at `turn` was asked for: no longer losing.
    fn asked(&mut self, device_id: &str, turn: u64) {
        if let Some(m) = self.by_device.get_mut(device_id).filter(|m| m.turn == turn) {
            m.lost = None;
        }
    }

    /// Whether a window of a computer other than `device_id` lost the focus
    /// and its ask is not sent yet (bounded, twice [`HANDOVER`]).
    fn other_losing(&self, device_id: &str, now: std::time::Instant) -> bool {
        self.by_device.iter().any(|(id, m)| {
            id != device_id
                && m.lost
                    .is_some_and(|at| now.saturating_duration_since(at) < HANDOVER * 2)
        })
    }
}

/// The window showing `device_id`'s screen gained (`focused`) or lost the
/// focus at `now`: noted at once, in the order the windows said it (PR #572
/// review I2), before anything is read or awaited. The turn for
/// [`window_gained_focus`] or [`window_lost_focus`].
pub(crate) fn note_window_focus(
    device_id: &str,
    focused: bool,
    now: std::time::Instant,
) -> Option<u64> {
    lock(&HANDING.moves).note(device_id, focused, now)
}

/// Whether a window gain of `device_id`, noted as `turn` at `at`, still
/// pushes once it waited up to [`HANDOVER`] for a loss not heard yet: not
/// if a window of the same computer lost the focus meanwhile; at once when
/// another computer's window did (its answer is then waited for).
async fn gain_goes(handing: &Handing, device_id: &str, turn: u64, at: std::time::Instant) -> bool {
    {
        let moves = lock(&handing.moves);
        let held_before = moves
            .by_device
            .get(device_id)
            .is_some_and(|m| m.held_before);
        // From another program (no window held the focus), no loss can
        // follow; after another computer's loss, its answer is waited for
        // anyway: no wait either way (PR #572 re-review N5).
        if !held_before || moves.other_losing(device_id, at) {
            return moves.stands(device_id, turn);
        }
    }
    loop {
        {
            let moves = lock(&handing.moves);
            if !moves.stands(device_id, turn) {
                return false;
            }
            if moves
                .by_device
                .iter()
                .any(|(id, m)| id != device_id && m.lost.is_some_and(|lost| lost >= at))
            {
                return true;
            }
        }
        if at.elapsed() >= HANDOVER {
            return lock(&handing.moves).stands(device_id, turn);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The window showing `device_id`'s screen gained the focus while it
/// controls that computer (PC3), noted as `turn` at `at`
/// ([`note_window_focus`]): [`push_for_window`], unless a window of the
/// same computer lost the focus just before or just after (range review
/// I2: two windows on one computer's screens share its clipboard already).
pub(crate) async fn window_gained_focus(
    device_id: &str,
    turn: Option<u64>,
    at: std::time::Instant,
) {
    let goes = match turn {
        Some(turn) => gain_goes(&HANDING, device_id, turn, at).await,
        None => false,
    };
    if !goes {
        tracing::debug!("The focus moved between windows of {device_id}: nothing pushed");
        return;
    }
    push_for_window(device_id).await
}

/// The window showing `device_id`'s screen lost the focus while it
/// controlled that computer (PC3), noted as `turn` at `at`
/// ([`note_window_focus`]): [`ask_back`] [`HANDOVER`] after, unless a
/// window of the same computer took the focus meanwhile (range review I2).
pub(crate) async fn window_lost_focus(device_id: &str, turn: Option<u64>, at: std::time::Instant) {
    let Some(turn) = turn else {
        tracing::debug!("The focus moved between windows of {device_id}: nothing asked");
        return;
    };
    tokio::time::sleep(HANDOVER.saturating_sub(at.elapsed())).await;
    if !lock(&HANDING.moves).stands(device_id, turn) {
        tracing::debug!("The focus moved between windows of {device_id}: nothing asked");
        return;
    }
    ask_back(device_id).await;
    lock(&HANDING.moves).asked(device_id, turn);
}

/// Answer the computer `device_id`'s ask for this computer's clipboard
/// back (PC3, [`ask_back`]), as [`push_now`] sends it, marked as
/// following the window.
pub(crate) async fn answer_window_ask(device_id: &str) {
    // Asked again here, as the ask was judged (PR #554 review I1): the
    // answer goes on whichever road reaches that computer.
    if !takes_window_ask(device_id) {
        tracing::debug!("Not answered {device_id}'s ask for the clipboard back");
        return;
    }
    // At most once each ANSWER_EVERY (PR #554 review I2): nothing on this
    // side shows that the window's focus moved, so asks in a loop are not
    // read for.
    let may = ANSWERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .may_answer(device_id, std::time::Instant::now());
    if !may {
        tracing::debug!("Not answered {device_id}'s ask for the clipboard back: too soon");
        return;
    }
    push_following_answer(device_id).await
}

/// How often a computer's asks for this clipboard back are answered, at
/// most (PR #554 review I2).
const ANSWER_EVERY: Duration = Duration::from_secs(2);

/// The answers given to each computer's asks: when it was last tried, and
/// what the last answer held (a hash), so the same copy is not sent again.
#[derive(Debug, Default)]
struct Answers {
    last: HashMap<String, (Option<std::time::Instant>, Option<u64>)>,
}

impl Answers {
    /// Whether an ask from `device_id` may be answered at `now`: not within
    /// [`ANSWER_EVERY`] of its last try. A try is noted either way it ends.
    fn may_answer(&mut self, device_id: &str, now: std::time::Instant) -> bool {
        let entry = self.last.entry(device_id.to_string()).or_default();
        let may = entry
            .0
            .is_none_or(|at| now.saturating_duration_since(at) >= ANSWER_EVERY);
        if may {
            entry.0 = Some(now);
        }
        may
    }

    /// Whether `held` (a hash of what the clipboard holds) is new for
    /// `device_id` since its last answer; noted when it is.
    fn fresh(&mut self, device_id: &str, held: u64) -> bool {
        let entry = self.last.entry(device_id.to_string()).or_default();
        if entry.1 == Some(held) {
            return false;
        }
        entry.1 = Some(held);
        true
    }
}

static ANSWERS: LazyLock<Mutex<Answers>> = LazyLock::new(Default::default);

/// [`push_now`], following `follow`.
async fn push_following(device_id: &str, follow: Follow) {
    push_held(device_id, follow, false).await
}

/// An answer to an ask for this clipboard back: [`push_held`] following the
/// window, only with something new since the last answer to that computer.
async fn push_following_answer(device_id: &str) {
    push_held(device_id, Follow::Window, true).await
}

/// [`push_now`], following `follow`; when `answering` an ask, only what is
/// new for that computer since its last answer ([`Answers::fresh`]).
async fn push_held(device_id: &str, follow: Follow, answering: bool) {
    // That computer's image still on its way here: this clipboard is about
    // to be its own, so nothing older goes back, and the switch drops it.
    let landing = landing_from(device_id);
    let at = switch();
    if landing {
        tracing::debug!(
            "The clipboard did not follow {} to {device_id}: its image was still on its way here",
            follow.words()
        );
        return;
    }
    // The copy read, as it was before the read: one made during it is
    // another.
    let copy = copy_generation();
    let held = match read_held().await {
        Ok(held) => held,
        Err(e) => {
            tracing::debug!(
                "The clipboard did not follow {}: not read: {e}",
                follow.words()
            );
            return;
        }
    };
    let text_from = FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let image_from = IMAGE_FROM.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let last_image = image_sent_last(device_id);
    if let Held::Text(text) = &held
        && text.len() > MAX_PUSH
    {
        too_big(Some(device_id), text.len(), TooBig::Out);
    }
    let follows = what_follows(
        held,
        device_id,
        text_from.as_ref(),
        image_from.as_ref(),
        last_image.as_deref(),
    );
    if answering {
        let key = match &follows {
            Held::Text(text) => hash(text),
            Held::Image(id) => hash(&image_key(id)),
            Held::Nothing => return,
        };
        let fresh = ANSWERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fresh(device_id, key);
        if !fresh {
            tracing::debug!("Not answered {device_id}: nothing new copied since the last answer");
            return;
        }
    }
    // The same text the window sent that computer last is there already,
    // unless something else went there or came from it since (range
    // review I2).
    if window_push_skipped(device_id, follow, answering, &follows, copy) {
        tracing::debug!(
            "The clipboard did not follow the window to {device_id}: sent there already"
        );
        return;
    }
    match follows {
        Held::Text(text) => push_text_now(device_id, &text, follow, copy).await,
        Held::Image(id) => push_image_now(device_id, &id, at, follow).await,
        Held::Nothing => {}
    }
}

/// What the clipboard holds for another computer: its text, else its image
/// (read only when there is no text).
async fn read_held() -> std::io::Result<Held> {
    if let Some(text) = crate::plugins::clipboard::read_text().await? {
        return Ok(Held::Text(text));
    }
    match crate::plugins::clipboard::image_for_push().await {
        Ok(Some((id, _))) => Ok(Held::Image(id)),
        Ok(None) => Ok(Held::Nothing),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    }
}

/// The text of [`push_now`].
async fn push_text_now(device_id: &str, text: &str, follow: Follow, copy: u64) {
    let packet = follow_packet(text, follow);
    let client = crate::state::clone_clients()
        .await
        .into_iter()
        .find(|c| c.device_id() == device_id);
    let sent = match client {
        // One linked to this one as its client: with the Clipboard
        // permission, as any push to it.
        Some(client) if crate::plugins::pair::permits(device_id, "kdeconnect.clipboard") => {
            client.send_packet(&packet).await.map_err(|e| e.to_string())
        }
        Some(_) => Err("it does not have the Clipboard permission here".to_string()),
        // One this one keeps a link to.
        None => match crate::peer_computers::live_link(device_id) {
            Some(link) => {
                link.send(packet);
                Ok(())
            }
            None => Err("no link to it".to_string()),
        },
    };
    match sent {
        // Who and how long, never the text (Z1).
        Ok(()) => {
            forget_image_sent(device_id);
            if follow == Follow::Window {
                WINDOW_SENT
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(device_id.to_string(), (hash(text), copy));
            }
            crate::clip_history::note(text, None);
            tracing::info!(
                "Clipboard followed {} to {device_id} ({} chars)",
                follow.words(),
                text.chars().count()
            );
        }
        Err(e) => tracing::debug!(
            "The clipboard did not follow {} to {device_id}: {e}",
            follow.words()
        ),
    }
}

/// The image of [`push_now`], `id`, for the switch numbered `at` (BK3). To
/// a computer linked to this one as its client, it is told, and it fetches
/// the image while no switch has come since ([`fetch_follows`]); to one
/// this one keeps a link to, it goes on a stream that computer opens.
async fn push_image_now(device_id: &str, id: &str, at: u64, follow: Follow) {
    // Kept as the clipboard was read; gone if it changed since.
    let Some(image) = crate::plugins::clipboard::kept_image(id) else {
        return;
    };
    let size = image.png.len() as u64;
    // The other computer takes none larger (N4's limit): none is sent.
    if size > pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE {
        tracing::debug!("The clipboard image did not follow to {device_id}: {size} bytes");
        return;
    }
    let client = crate::state::clone_clients()
        .await
        .into_iter()
        .find(|c| c.device_id() == device_id);
    let sent = match client {
        Some(client) if crate::plugins::pair::permits(device_id, "kdeconnect.clipboard") => {
            *TOLD.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((device_id.to_string(), id.to_string(), at));
            note_image_told(device_id, id);
            client
                .send_packet(&follow_image_packet(id, size, follow))
                .await
                .map(|()| "told")
                .map_err(|e| e.to_string())
        }
        Some(_) => Err("it does not have the Clipboard permission here".to_string()),
        None => {
            note_image_sent(device_id, id);
            crate::peer_computers::send_clipboard_image(device_id, image, Some(follow), || {
                still(at)
            })
            .await
            .map(|()| "sent")
        }
    };
    match sent {
        // Who and how large, never the image.
        Ok(how) => tracing::info!(
            "Clipboard image followed {} to {device_id} ({size} bytes, {how})",
            follow.words()
        ),
        Err(e) => {
            forget_image_sent_if(device_id, id);
            tracing::debug!(
                "The clipboard image did not follow {} to {device_id}: {e}",
                follow.words()
            )
        }
    }
}

/// A packet marked as following the mouse arrived here (`body`): the
/// cursor came here, a switch.
pub(crate) fn follow_arrived(body: &serde_json::Value) {
    if marked(body) {
        switch();
    }
}

/// The image a clipboard packet's `body` tells of (M3's `{image: {id,
/// size}}`): an id as this computer makes one (hex SHA-256), and a size
/// within N4's limit.
pub(crate) fn told_image(body: &serde_json::Value) -> Option<(String, u64)> {
    let image = body.get("image")?;
    let id = image.get("id")?.as_str()?;
    let size = image.get("size")?.as_u64()?;
    let valid = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    (valid && size > 0 && size <= pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE)
        .then(|| (id.to_ascii_lowercase(), size))
}

/// Whether a clipboard packet's `body` is marked as following the mouse:
/// [`FOLLOWS`] is JSON `true`, nothing else.
pub(crate) fn marked(body: &serde_json::Value) -> bool {
    body.get(FOLLOWS).and_then(|v| v.as_bool()) == Some(true)
}

/// Whether a push marked as following the mouse is taken (`marked`): with
/// the shared keyboard and mouse (`shared_input`) and "Clipboard follows
/// the mouse" (`follows_on`) on here, from one of the owner's paired
/// computers (`is_computer`) that may drive this one (`may_drive`) or that
/// answered this one's hello with its screens on the link up now
/// (`answered`: this one may drive it). Each later word is asked only when
/// those before it hold (they read files).
fn follow_taken(
    marked: bool,
    shared_input: bool,
    follows_on: bool,
    is_computer: impl FnOnce() -> bool,
    may_drive: impl FnOnce() -> bool,
    answered: impl FnOnce() -> bool,
) -> bool {
    marked && shared_input && follows_on && is_computer() && (may_drive() || answered())
}

/// Whether a clipboard packet's `body` from the computer `device_id` is
/// taken as one following the mouse, with "Clipboard with my computers"
/// off here ([`follow_taken`]), and, from a computer paired into this one,
/// with its Clipboard permission here (PR #557 review). Reads files
/// (blocking), but only for a marked packet.
pub(crate) fn takes_follow(device_id: &str, body: &serde_json::Value) -> bool {
    if !marked(body) {
        return false;
    }
    let config = crate::events::current_config();
    // One that follows the window, by the window's rules alone (PC3).
    if of_window(body) {
        return window_taken(
            config.computer_view_clipboard,
            || is_paired_computer(device_id),
            || controls_here(device_id),
            || clipboard_allowed(device_id),
            || answer_awaited(device_id, std::time::Instant::now()),
        );
    }
    follow_taken(
        true,
        config.shared_input,
        config.shared_input_clipboard,
        || {
            crate::plugins::pair::is_computer_device(device_id)
                || crate::peer_computers::is_paired(device_id)
        },
        || crate::shared_input::receive::may_drive_here(device_id),
        || crate::shared_input::link::answered(device_id),
    ) && crate::plugins::pair::kept_link_clipboard(device_id)
}

/// Whether a push that follows the window (PC3) is taken: with
/// `computer_view_clipboard` on here (`on`), from one of the owner's paired
/// computers (`is_computer`) that controls this one through its window now
/// (`controls`: its screen session here takes its input) and has the
/// Clipboard permission here (`permitted`, PR #554 review I1: on the link
/// this computer keeps no other gate asks), or whose answer this one awaits
/// (`awaited`: this computer's window lost the focus and asked it back;
/// used once). Each later word is asked only when those before it hold.
fn window_taken(
    on: bool,
    is_computer: impl FnOnce() -> bool,
    controls: impl FnOnce() -> bool,
    permitted: impl FnOnce() -> bool,
    awaited: impl FnOnce() -> bool,
) -> bool {
    // The awaited answer first, so it is used up even from a computer that
    // also controls this one (PR #572 review I2): a push waiting on it
    // goes on.
    on && is_computer() && (awaited() || (controls() && permitted()))
}

/// Whether an ask for this computer's clipboard back is answered (PC3):
/// with `computer_view_clipboard` on here (`on`), from a paired computer
/// (`is_computer`) that controls this one through its window now
/// (`controls`) and has the Clipboard permission here (`permitted`).
fn window_ask_taken(
    on: bool,
    is_computer: impl FnOnce() -> bool,
    controls: impl FnOnce() -> bool,
    permitted: impl FnOnce() -> bool,
) -> bool {
    on && is_computer() && controls() && permitted()
}

/// Whether the computer `device_id` may ask for this computer's clipboard
/// back as its window loses the focus: [`window_ask_taken`], whichever road
/// the ask came by.
pub(crate) fn takes_window_ask(device_id: &str) -> bool {
    window_ask_taken(
        crate::events::current_config().computer_view_clipboard,
        || is_paired_computer(device_id),
        || controls_here(device_id),
        || clipboard_allowed(device_id),
    )
}

/// Whether the computer `device_id` has the Clipboard permission here.
fn clipboard_allowed(device_id: &str) -> bool {
    crate::plugins::pair::allows(
        device_id,
        pocketdesk_core::protocol::permissions::Capability::Clipboard,
    )
}

/// One of the owner's paired computers, either way.
fn is_paired_computer(device_id: &str) -> bool {
    crate::plugins::pair::is_computer_device(device_id)
        || crate::peer_computers::is_paired(device_id)
}

/// Whether the computer `device_id` controls this one through its window
/// now: a live session of its shows this screen and takes its input.
fn controls_here(device_id: &str) -> bool {
    session_controls(&pocketdesk_core::streaming::sessions::list(), device_id)
}

/// Whether one of `live` is `device_id`'s, showing the screen and taking
/// its input.
fn session_controls(
    live: &[pocketdesk_core::streaming::sessions::LiveSession],
    device_id: &str,
) -> bool {
    live.iter()
        .any(|s| s.device_id.as_deref() == Some(device_id) && s.streamed.video && s.streamed.input)
}

/// The packet that asks another computer for its clipboard back (PC3): the
/// window showing its screen lost the focus. It answers with a push that
/// follows the window, as [`push_for_window`] sends one.
pub(crate) const ASK_BACK: &str = pocketdesk_core::protocol::clipboard::CLIPBOARD_VIEW;

/// How long an answer to [`ask_back`] is awaited.
const ANSWER_WAIT: Duration = Duration::from_secs(15);

/// Whether an answer is awaited at `now` from a computer asked at `asked`:
/// within [`ANSWER_WAIT`].
fn awaits(asked: Option<&std::time::Instant>, now: std::time::Instant) -> bool {
    asked.is_some_and(|at| now.saturating_duration_since(*at) < ANSWER_WAIT)
}

/// Whether an answer from `device_id` is awaited now; the ask is used up.
fn answer_awaited(device_id: &str, now: std::time::Instant) -> bool {
    answer_awaited_in(&HANDING, device_id, now)
}

/// [`answer_awaited`] with the hand-over's state `handing`.
fn answer_awaited_in(handing: &Handing, device_id: &str, now: std::time::Instant) -> bool {
    let mut asked = lock(&handing.asked);
    let awaited = awaits(asked.get(device_id), now);
    if awaited {
        asked.remove(device_id);
        *lock(&handing.answered) = Some(now);
    }
    awaited
}

/// The window showing the computer `device_id`'s screen lost the focus
/// while it controlled that computer (PC3): ask that computer for its
/// clipboard, which comes back as a push that follows the window.
pub(crate) async fn ask_back(device_id: &str) {
    lock(&HANDING.asked).insert(device_id.to_string(), std::time::Instant::now());
    // An image of this computer's still on its way there goes no further.
    switch();
    let ask = NetworkPacket::new(ASK_BACK).with_body(serde_json::json!({}));
    match send_to_computer(device_id, &ask).await {
        Ok(()) => tracing::debug!("Asked {device_id} for its clipboard back"),
        Err(e) => tracing::debug!("Not asked {device_id} for its clipboard back: {e}"),
    }
}

/// Send `packet` to the computer `device_id`: on its connection here when it
/// is linked to this one as its client, else on the link this one keeps to
/// it.
pub(crate) async fn send_to_computer(
    device_id: &str,
    packet: &NetworkPacket,
) -> Result<(), String> {
    let client = crate::state::clone_clients()
        .await
        .into_iter()
        .find(|c| c.device_id() == device_id);
    match client {
        Some(client) => client.send_packet(packet).await.map_err(|e| e.to_string()),
        None => match crate::peer_computers::live_link(device_id) {
            Some(link) => {
                link.send(packet.clone());
                Ok(())
            }
            None => Err("no link to it".to_string()),
        },
    }
}

/// What the window's focus does now (PC3), on the side whose window it is:
/// nothing with `computer_view_clipboard` off (`on`), with "Clipboard with
/// my computers" on (`sync`: every copy goes both ways already, and the
/// window does not fight it), or while the other computer does not take
/// this one's input (`control`).
pub(crate) fn window_focus_does(
    focused: bool,
    on: bool,
    sync: bool,
    control: Option<bool>,
) -> Option<WindowFocus> {
    if !on || sync || control != Some(true) {
        return None;
    }
    Some(if focused {
        WindowFocus::Push
    } else {
        WindowFocus::AskBack
    })
}

/// What the window's focus does: push this computer's clipboard there, or
/// ask for that computer's back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowFocus {
    Push,
    AskBack,
}

/// One `wl-paste --watch` while a phone could receive the clipboard (R6d):
/// it runs `echo` at each change, so a line on its output says the
/// clipboard changed, and the loop reads it only then, instead of spawning
/// `wl-paste` every tick. On X11, XFixes says when the clipboard changes
/// owner (T2), on a thread of its own.
#[cfg(not(windows))]
struct ChangeWatch {
    source: WatchSource,
    changed: Arc<AtomicBool>,
}

#[cfg(not(windows))]
enum WatchSource {
    Process(tokio::process::Child),
    /// The X11 thread: whether it still runs, and the word to stop.
    #[cfg(target_os = "linux")]
    X11 {
        alive: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
    },
}

#[cfg(target_os = "linux")]
impl Drop for ChangeWatch {
    fn drop(&mut self) {
        WATCHING.store(false, Ordering::Relaxed);
        if let WatchSource::X11 { stop, .. } = &self.source {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// After a watcher could not start or ended, how long the loop reads every
/// tick before it tries one again (a compositor restarting, or an old
/// `wl-clipboard` without `--watch`).
#[cfg(not(windows))]
const WATCH_RETRY: Duration = Duration::from_secs(60);

/// How long the X11 watcher's connection is waited for (T2).
#[cfg(target_os = "linux")]
const X11_SETUP: Duration = Duration::from_secs(2);

/// A connection to the X server that hears the clipboard change owner (T2).
#[cfg(target_os = "linux")]
fn x11_watch() -> std::io::Result<x11rb::rust_connection::RustConnection> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).map_err(std::io::Error::other)?;
    let root = conn.setup().roots[screen].root;
    conn.xfixes_query_version(5, 0)
        .map_err(std::io::Error::other)?
        .reply()
        .map_err(std::io::Error::other)?;
    let clipboard = conn
        .intern_atom(false, b"CLIPBOARD")
        .map_err(std::io::Error::other)?
        .reply()
        .map_err(std::io::Error::other)?
        .atom;
    conn.xfixes_select_selection_input(
        root,
        clipboard,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .map_err(std::io::Error::other)?;
    conn.flush().map_err(std::io::Error::other)?;
    Ok(conn)
}

#[cfg(not(windows))]
impl ChangeWatch {
    fn start() -> std::io::Result<Self> {
        let started = Self::start_any();
        if started.is_ok() {
            WATCHING.store(true, Ordering::Relaxed);
        }
        started
    }

    fn start_any() -> std::io::Result<Self> {
        match pocketdesk_core::local::clip_tool::Tool::current() {
            pocketdesk_core::local::clip_tool::Tool::Wayland => {
                Self::start_with("wl-paste", &["--watch", "echo"])
            }
            #[cfg(target_os = "linux")]
            pocketdesk_core::local::clip_tool::Tool::X11 => Self::start_x11(),
            #[cfg(not(target_os = "linux"))]
            pocketdesk_core::local::clip_tool::Tool::X11 => {
                Err(std::io::Error::other("no clipboard watcher here"))
            }
        }
    }

    /// X11 (T2): XFixes' selection events on the clipboard, polled on a
    /// thread that ends at the [`Drop`]'s word or a lost connection. The
    /// connection is made on that thread, waited for [`X11_SETUP`] at most
    /// (T2 review: a stalled X server held the async loop).
    #[cfg(target_os = "linux")]
    fn start_x11() -> std::io::Result<Self> {
        // What is there when it starts is read once.
        let changed = Arc::new(AtomicBool::new(true));
        let alive = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, running, stopping) = (changed.clone(), alive.clone(), stop.clone());
        let (ready, started) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("clipboard-x11".into())
            .spawn(move || {
                let conn = match x11_watch() {
                    Ok(conn) => {
                        let _ = ready.send(Ok(()));
                        conn
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        running.store(false, Ordering::Relaxed);
                        return;
                    }
                };
                use x11rb::connection::Connection;
                while !stopping.load(Ordering::Relaxed) {
                    match conn.poll_for_event() {
                        Ok(Some(x11rb::protocol::Event::XfixesSelectionNotify(_))) => {
                            flag.store(true, Ordering::Relaxed);
                            COPIES.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                        Err(_) => break,
                    }
                }
                running.store(false, Ordering::Relaxed);
            })?;
        let watch = Self {
            source: WatchSource::X11 { alive, stop },
            changed,
        };
        match started.recv_timeout(X11_SETUP) {
            Ok(Ok(())) => Ok(watch),
            Ok(Err(e)) => Err(e),
            // Dropped here: its thread stops once the setup ends.
            Err(_) => Err(std::io::Error::other("the X server did not answer in time")),
        }
    }

    fn start_with(program: &str, args: &[&str]) -> std::io::Result<Self> {
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("no output to read"))?;
        // What is there when it starts is read once.
        let changed = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&changed);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(_)) = lines.next_line().await {
                flag.store(true, Ordering::Relaxed);
                COPIES.fetch_add(1, Ordering::Relaxed);
            }
        });
        Ok(Self {
            source: WatchSource::Process(child),
            changed,
        })
    }

    /// Whether the clipboard changed since the last look; `None` once the
    /// watcher has ended.
    fn take_change(&mut self) -> Option<bool> {
        let running = match &mut self.source {
            WatchSource::Process(child) => matches!(child.try_wait(), Ok(None)),
            #[cfg(target_os = "linux")]
            WatchSource::X11 { alive, .. } => alive.load(Ordering::Relaxed),
        };
        running.then(|| self.changed.swap(false, Ordering::Relaxed))
    }
}

/// Watch the clipboard while any connection could receive it, and push
/// what changes. Runs for the server's life when `clipboard_push` is on.
pub(crate) async fn run() {
    let mut watch = Watch::default();
    #[cfg(windows)]
    let mut sequence = None;
    #[cfg(not(windows))]
    let mut change_watch: Option<ChangeWatch> = None;
    #[cfg(not(windows))]
    let mut watch_again_at: Option<tokio::time::Instant> = None;
    #[cfg(not(windows))]
    let mut said = false;
    #[cfg(not(windows))]
    let mut image_read_at: Option<tokio::time::Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let clients = crate::state::clone_clients().await;
        let linked = crate::peer_computers::linked();
        // A paired KDE Connect device's link counts too (3c): it is no
        // client of the control channel's.
        if clients.is_empty()
            && linked.is_empty()
            && crate::kdeconnect_compat::link::live_paired().is_empty()
        {
            // No phone to tell: what is copied meanwhile is not pushed when
            // one comes back (a phone in a session asks for it itself).
            watch = Watch::default();
            #[cfg(windows)]
            {
                sequence = None;
            }
            // No one to tell: the watcher ends with the last of them.
            #[cfg(not(windows))]
            {
                change_watch = None;
            }
            continue;
        }
        // On Windows the clipboard's sequence number says whether anything
        // changed, without opening the clipboard.
        #[cfg(windows)]
        {
            // A stand-in clipboard (tests), text or image, has no sequence:
            // read each tick.
            let now = (crate::plugins::clipboard::stand_in().is_none()
                && crate::plugins::clipboard::image_stand_in().is_none())
            .then(crate::platform::clipboard_windows::sequence);
            if now.is_some() && sequence == now {
                continue;
            }
            // Noted only once the clipboard was read, its image too (M3
            // review): a read that failed (another app held it) is tried
            // again next tick.
            match crate::plugins::clipboard::read_text().await {
                Ok(text) => match image_now(text.is_none(), &clients, &linked).await {
                    Ok(image) => {
                        sequence = now;
                        push_change(&mut watch, text, image, clients, linked).await;
                    }
                    Err(e) => tracing::debug!("The clipboard's image was not read: {e}"),
                },
                Err(e) => tracing::debug!("The clipboard was not read: {e}"),
            }
        }
        #[cfg(not(windows))]
        {
            let may_watch = crate::plugins::clipboard::stand_in().is_none()
                && crate::plugins::clipboard::image_stand_in().is_none()
                && watch_again_at.is_none_or(|at| tokio::time::Instant::now() >= at);
            if change_watch.is_none() && may_watch {
                match ChangeWatch::start() {
                    Ok(started) => change_watch = Some(started),
                    Err(e) => {
                        if !std::mem::replace(&mut said, true) {
                            tracing::info!(
                                "No clipboard watcher ({e}): the clipboard is read every 2 s"
                            );
                        }
                        watch_again_at = Some(tokio::time::Instant::now() + WATCH_RETRY);
                    }
                }
            }
            if let Some(watcher) = change_watch.as_mut() {
                match watcher.take_change() {
                    Some(false) => continue,
                    Some(true) => {}
                    None => {
                        if !std::mem::replace(&mut said, true) {
                            tracing::info!(
                                "The clipboard watcher ended: the clipboard is read every 2 s for a while"
                            );
                        }
                        change_watch = None;
                        watch_again_at = Some(tokio::time::Instant::now() + WATCH_RETRY);
                    }
                }
            }
            let text = crate::plugins::clipboard::read_text().await.ok().flatten();
            // Without `--watch` (GNOME), an image is read at most every
            // IMAGE_FALLBACK, not every tick (M3 review): it is a whole PNG
            // and its hash. Until then nothing is decided, so the watch
            // keeps what it saw.
            if text.is_none() && change_watch.is_none() {
                let due = image_read_at
                    .is_none_or(|at: tokio::time::Instant| at.elapsed() >= IMAGE_FALLBACK);
                if !due {
                    continue;
                }
                image_read_at = Some(tokio::time::Instant::now());
            }
            let Ok(image) = image_now(text.is_none(), &clients, &linked).await else {
                continue;
            };
            push_change(&mut watch, text, image, clients, linked).await;
        }
    }
}

/// How often an image is read where no `wl-paste --watch` says the
/// clipboard changed (M3 review).
#[cfg(not(windows))]
const IMAGE_FALLBACK: Duration = Duration::from_secs(30);

/// The clipboard's image to tell of, when it holds no text and a phone or
/// computer among `clients` may have it ([`may_push_image`]: a phone with
/// "Images to phones' clipboards" on, M3; a computer while the clipboard is
/// shared with computers, BK3), or a computer this one keeps a link to is
/// up (`linked`, only while it is shared): never read and encoded for no one
/// (M3 review). An error says the clipboard could not be read now.
async fn image_now(
    no_text: bool,
    clients: &[Arc<dyn DeviceSender>],
    linked: &[crate::peer_computers::LinkedComputer],
) -> pocketdesk_core::error::Result<Option<(String, u64)>> {
    if !no_text {
        return Ok(None);
    }
    let phone_images = phone_images();
    if linked.is_empty()
        && !clients
            .iter()
            .any(|c| may_push_image(c.device_id(), phone_images))
    {
        return Ok(None);
    }
    crate::plugins::clipboard::image_for_push().await
}

/// Whether phones are told of an image: pushing to phones, with "Images to
/// phones' clipboards" on (M3; read live from the config).
fn phone_images() -> bool {
    TO_PHONES.load(Ordering::Relaxed) && crate::events::current_config().clipboard_images
}

/// Push `text` to `clients`, and to the computers this one keeps a link to
/// (`linked`), if it is a change; or, with no text, `image` (M3, BK3).
async fn push_change(
    watch: &mut Watch,
    text: Option<String>,
    image: Option<(String, u64)>,
    clients: Vec<Arc<dyn DeviceSender>>,
    linked: Vec<crate::peer_computers::LinkedComputer>,
) {
    let seen = text
        .clone()
        .or_else(|| image.as_ref().map(|(id, _)| image_key(id)));
    let Some(changed) = watch.observe(seen.as_deref()) else {
        // Too big for any computer it would have gone to: the owner is
        // told (phones are not: one in a session still asks for it).
        if let Some(len) = watch.take_skipped()
            && (!linked.is_empty()
                || clients.iter().any(|c| {
                    crate::plugins::pair::is_computer_device(c.device_id())
                        && may_push(c.device_id())
                }))
        {
            too_big(None, len, TooBig::Out);
        }
        return;
    };
    if text.is_none() {
        if let Some((id, size)) = image.filter(|(id, _)| image_key(id) == changed) {
            push_image(&id, size, clients, linked).await;
        }
        return;
    }
    let text = changed;
    let from_phone = take_from_phone();
    let targets = targets(clients, &text, from_phone.as_ref(), |c| {
        may_push(c.device_id())
    });
    let packet = packet(&text);
    // Paired KDE Connect devices, as phones (3c): each link sends it only
    // to a device allowed the clipboard.
    let kde: Vec<String> = if TO_PHONES.load(Ordering::Relaxed) {
        crate::kdeconnect_compat::link::live_paired()
            .into_iter()
            .filter(|id| !came_from(from_phone.as_ref(), id, &text))
            .collect()
    } else {
        Vec::new()
    };
    // This computer's own copy, gone to anyone, a phone, a computer or a KDE
    // device: kept in the history (D3 review). A phone's own text was kept
    // as it arrived.
    let goes = !targets.is_empty()
        || !kde.is_empty()
        || linked
            .iter()
            .any(|c| !came_from(from_phone.as_ref(), &c.device_id, &text));
    if from_phone.as_ref().is_none_or(|(_, from)| from != &text) && goes {
        crate::clip_history::note(&text, None);
    }
    for target in targets {
        match target.send_packet(&packet).await {
            // Z1: who and how long, never the text: a copy sent once, and
            // never back to the phone it came from, can be seen.
            Ok(()) => {
                forget_image_sent(target.device_id());
                tracing::info!(
                    "Clipboard sent to {} ({} chars)",
                    target.device_id(),
                    text.chars().count()
                )
            }
            Err(e) => tracing::debug!("Clipboard not pushed to {}: {e}", target.device_id()),
        }
    }
    for computer in linked {
        if !came_from(from_phone.as_ref(), &computer.device_id, &text) {
            forget_image_sent(&computer.device_id);
            computer.send(packet.clone());
        }
    }
    for device_id in kde {
        crate::kdeconnect_compat::link::tell(
            &device_id,
            crate::kdeconnect_compat::link::Event::Clipboard(text.clone()),
        );
    }
}

/// Whether the image `id` goes to `device_id`: not back to where it came
/// from, nor to a computer that was sent it last.
fn image_goes_to(device_id: &str, id: &str) -> bool {
    !image_came_from(device_id, id) && image_sent_last(device_id).as_deref() != Some(id)
}

/// Tell the phones and computers that may have it of the clipboard's image
/// (M3), each then fetching it; and send it to the computers this one keeps
/// a link to (`linked`, BK3), each on a stream it opens for it.
async fn push_image(
    id: &str,
    size: u64,
    clients: Vec<Arc<dyn DeviceSender>>,
    linked: Vec<crate::peer_computers::LinkedComputer>,
) {
    let packet = image_packet(id, size);
    let phone_images = phone_images();
    for target in clients
        .into_iter()
        .filter(|c| may_push_image(c.device_id(), phone_images) && image_goes_to(c.device_id(), id))
    {
        // A computer's is on its way from now (BK3 review).
        let computer = crate::plugins::pair::is_computer_device(target.device_id());
        if computer {
            note_image_told(target.device_id(), id);
        }
        match target.send_packet(&packet).await {
            // AD1: who and how large, never the image: a phone's switch or
            // permission keeping it from the phone can be told apart.
            Ok(()) => tracing::info!(
                "Clipboard image told to {} ({size} bytes)",
                target.device_id()
            ),
            Err(e) => {
                if computer {
                    forget_image_sent_if(target.device_id(), id);
                }
                tracing::debug!("Clipboard image not told to {}: {e}", target.device_id())
            }
        }
    }
    let linked: Vec<String> = linked
        .into_iter()
        .map(|c| c.device_id)
        .filter(|device_id| image_goes_to(device_id, id))
        .collect();
    if linked.is_empty() {
        return;
    }
    // Kept as the clipboard was read; gone if it changed since.
    let Some(image) = crate::plugins::clipboard::kept_image(id) else {
        return;
    };
    for device_id in linked {
        let (image, id) = (image.clone(), id.to_string());
        // On its way from now (BK3 review).
        note_image_sent(&device_id, &id);
        // Its own task: one computer slow to open its stream holds up none.
        tokio::spawn(async move {
            match crate::peer_computers::send_clipboard_image(&device_id, image, None, || true)
                .await
            {
                Ok(()) => tracing::info!("Clipboard image sent to {device_id} ({size} bytes)"),
                Err(e) => {
                    forget_image_sent_if(&device_id, &id);
                    tracing::debug!("Clipboard image not sent to {device_id}: {e}")
                }
            }
        });
    }
}

#[cfg(test)]
mod image_note_tests {
    use super::*;

    /// O7: an image is not told back to the device it came from, and a note
    /// for a write that failed is forgotten.
    #[test]
    fn an_image_is_not_told_back_to_where_it_came_from() {
        note_image_from("phone-a", "img1");
        assert!(image_came_from("phone-a", "img1"));
        assert!(!image_came_from("phone-b", "img1"), "another device");
        assert!(!image_came_from("phone-a", "img2"), "another image");
        forget_image_from("img2");
        assert!(
            image_came_from("phone-a", "img1"),
            "another image's failure"
        );
        forget_image_from("img1");
        assert!(!image_came_from("phone-a", "img1"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::kdeconnect::TcpDeviceSender;

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_watcher_says_a_change_once_and_none_once_it_ends() {
        // Two changes, then it stays: `sh` stands in for `wl-paste --watch`.
        let mut watcher = ChangeWatch::start_with("sh", &["-c", "echo; echo; sleep 5"]).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(watcher.take_change(), Some(true));
        assert_eq!(watcher.take_change(), Some(false), "said once");
        let mut ended = ChangeWatch::start_with("sh", &["-c", "exit 1"]).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(ended.take_change(), None, "an old wl-paste without --watch");
    }

    /// Only a body whose mark is JSON `true` is marked.
    #[test]
    fn only_a_true_mark_is_a_follow() {
        assert!(marked(
            &serde_json::json!({"content": "x", "follows": true})
        ));
        for body in [
            serde_json::json!({"content": "x"}),
            serde_json::json!({"content": "x", "follows": "true"}),
            serde_json::json!({"content": "x", "follows": 1}),
            serde_json::json!({"content": "x", "follows": false}),
            serde_json::json!({"content": "x", "follows": null}),
        ] {
            assert!(!marked(&body), "{body}");
        }
        assert!(follow_packet("x", Follow::Mouse).body[FOLLOWS] == true);
    }

    /// A marked push is taken with both settings on, from a paired computer
    /// that may drive this one or that this one may drive; never from a
    /// phone, a computer sharing nothing, or with either setting off.
    #[test]
    fn a_follow_is_taken_only_from_a_computer_sharing_input() {
        let yes = || true;
        let no = || false;
        // Both ways in.
        assert!(
            follow_taken(true, true, true, yes, yes, no),
            "it drives here"
        );
        assert!(
            follow_taken(true, true, true, yes, no, yes),
            "this drives it"
        );
        // A phone (no paired computer), even with the permission flag.
        assert!(!follow_taken(true, true, true, no, yes, yes));
        // A paired computer sharing nothing with this one.
        assert!(!follow_taken(true, true, true, yes, no, no));
        // Unmarked.
        assert!(!follow_taken(false, true, true, yes, yes, yes));
        // Either setting off here.
        assert!(!follow_taken(true, false, true, yes, yes, yes));
        assert!(!follow_taken(true, true, false, yes, yes, yes));
        // Nothing is read for a push turned away early.
        let unread = || -> bool { panic!("read for nothing") };
        assert!(!follow_taken(false, true, true, unread, unread, unread));
        assert!(!follow_taken(true, true, true, no, unread, unread));
    }

    #[test]
    fn a_copy_is_not_sent_back_to_the_device_it_came_from() {
        let from = ("desk-b".to_string(), "hello".to_string());
        assert!(came_from(Some(&from), "desk-b", "hello"));
        assert!(
            !came_from(Some(&from), "desk-b", "a later copy"),
            "its own copy goes"
        );
        assert!(!came_from(Some(&from), "phone", "hello"), "others get it");
        assert!(!came_from(None, "desk-b", "hello"));
    }

    #[test]
    fn a_copy_a_password_manager_marks_stays_on_the_computer() {
        assert!(!WindowsMarks::default().private());
        for marks in [
            WindowsMarks {
                exclude_from_monitors: true,
                ..Default::default()
            },
            WindowsMarks {
                viewer_ignore: true,
                ..Default::default()
            },
            WindowsMarks {
                history: Some(0),
                ..Default::default()
            },
            WindowsMarks {
                cloud: Some(0),
                ..Default::default()
            },
            // A mark there but unread fails closed (M3 review).
            WindowsMarks {
                unread: true,
                ..Default::default()
            },
        ] {
            assert!(marks.private(), "{marks:?}");
        }
        assert!(
            !WindowsMarks {
                history: Some(1),
                cloud: Some(1),
                ..Default::default()
            }
            .private(),
            "allowed explicitly"
        );
        assert!(private_types("text/plain\nx-kde-passwordManagerHint\n"));
        assert!(!private_types("text/plain;charset=utf-8\nTEXT\n"));
    }

    #[test]
    fn the_note_of_a_phone_copy_is_used_once() {
        note_from_phone("a", "hello");
        assert_eq!(take_from_phone(), Some(("a".into(), "hello".into())));
        assert_eq!(take_from_phone(), None);
    }

    #[test]
    fn only_a_change_after_the_first_read_is_pushed() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("there at start")), None);
        assert_eq!(watch.observe(Some("copied")), Some("copied".into()));
        assert_eq!(watch.observe(Some("copied")), None, "once");
        assert_eq!(watch.observe(None), None, "an image or nothing");
        assert_eq!(
            watch.observe(Some("copied")),
            Some("copied".into()),
            "the same text copied again after something else"
        );
        assert_eq!(watch.observe(Some("")), None);
        let long = "x".repeat(MAX_PUSH + 1);
        assert_eq!(watch.observe(Some(&long)), None, "too long to push");
        assert_eq!(watch.take_skipped(), Some(MAX_PUSH + 1), "said once");
        assert_eq!(watch.take_skipped(), None);
        assert_eq!(watch.observe(Some(&long)), None, "the same copy again");
        assert_eq!(watch.take_skipped(), None, "is no new copy");
    }

    /// Another computer's push is its text up to [`MAX_PUSH`], not past it.
    #[test]
    fn a_pushed_text_over_the_limit_is_its_length() {
        let at = "x".repeat(MAX_PUSH);
        assert_eq!(
            pushed_text(&serde_json::json!({ "content": at })),
            Ok(Some(at.as_str()))
        );
        let over = "x".repeat(MAX_PUSH + 1);
        assert_eq!(
            pushed_text(&serde_json::json!({ "content": over })),
            Err(MAX_PUSH + 1)
        );
        assert_eq!(pushed_text(&serde_json::json!({ "image": {} })), Ok(None));
    }

    /// I1 (PR #597 review): a clipboard too big is told once a minute per
    /// computer and way, in words that name the computer and the limit.
    #[test]
    fn a_clipboard_too_big_is_told_once_a_minute_per_computer_and_way() {
        let mut shown = HashMap::new();
        let now = Instant::now();
        let key = |id: &str, way| (id.to_string(), way);
        assert!(too_big_due(&mut shown, key("desk", TooBig::In), now));
        assert!(!too_big_due(
            &mut shown,
            key("desk", TooBig::In),
            now + Duration::from_secs(30)
        ));
        assert!(
            too_big_due(&mut shown, key("desk", TooBig::Out), now),
            "the other way"
        );
        assert!(
            too_big_due(&mut shown, key("laptop", TooBig::In), now),
            "another computer"
        );
        assert!(too_big_due(
            &mut shown,
            key("desk", TooBig::In),
            now + TOO_BIG_EVERY
        ));
        let (summary, body) = too_big_words(TooBig::In, "Desk", MAX_PUSH + 1);
        assert_eq!(summary, "A clipboard was too big to copy");
        assert_eq!(
            body,
            "A clipboard from Desk was too big to copy (257 KB, over 256 KB)"
        );
        let (_, body) = too_big_words(TooBig::Out, "your other computers", 300 * 1024);
        assert_eq!(
            body,
            "The text copied was not shared with your other computers (300 KB, over 256 KB)"
        );
    }

    fn phone(id: &str) -> Arc<dyn DeviceSender> {
        Arc::new(TcpDeviceSender::from_arc(
            Arc::new(tokio::sync::Mutex::new(tokio::io::sink())),
            id.to_string(),
        ))
    }

    #[test]
    fn the_text_goes_to_permitted_phones_but_not_back_to_its_sender() {
        let clients = vec![phone("a"), phone("b"), phone("c")];
        let ids = |v: Vec<Arc<dyn DeviceSender>>| -> Vec<String> {
            v.iter().map(|c| c.device_id().to_string()).collect()
        };
        let from_a = ("a".to_string(), "hello".to_string());
        assert_eq!(
            ids(targets(clients.clone(), "hello", Some(&from_a), |c| c
                .device_id()
                != "c")),
            ["b"],
            "not back to a; c may not have it"
        );
        assert_eq!(
            ids(targets(clients, "other", Some(&from_a), |_| true)),
            ["a", "b", "c"],
            "a gets text it did not send"
        );
    }

    #[test]
    fn an_image_is_one_change_and_text_after_it_another() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(Some("before")), None, "the first read primes");
        let key = image_key("ab12");
        assert_eq!(watch.observe(Some(&key)), Some(key.clone()));
        assert_eq!(watch.observe(Some(&key)), None, "once per change");
        assert_eq!(watch.observe(Some("after")), Some("after".to_string()));
        assert!(!key.starts_with("image:"), "no text reads as an image");
    }

    /// BK3: at a switch, the clipboard's text goes, else its image, but
    /// nothing the computer reached put here itself, no text too long, and
    /// not the image last sent to it again.
    #[test]
    fn text_follows_else_an_image_but_not_back_or_twice() {
        let text = || Held::Text("hello".to_string());
        let image = || Held::Image("ab12".to_string());
        assert_eq!(what_follows(text(), "desk", None, None, None), text());
        assert_eq!(what_follows(image(), "desk", None, None, None), image());
        assert_eq!(
            what_follows(Held::Nothing, "desk", None, None, None),
            Held::Nothing
        );
        let long = Held::Text("x".repeat(MAX_PUSH + 1));
        assert_eq!(what_follows(long, "desk", None, None, None), Held::Nothing);
        // Not back to where it came from.
        let text_from = ("desk".to_string(), "hello".to_string());
        assert_eq!(
            what_follows(text(), "desk", Some(&text_from), None, None),
            Held::Nothing
        );
        assert_eq!(
            what_follows(text(), "other", Some(&text_from), None, None),
            text()
        );
        let image_from = ("desk".to_string(), "ab12".to_string());
        assert_eq!(
            what_follows(image(), "desk", None, Some(&image_from), None),
            Held::Nothing
        );
        assert_eq!(
            what_follows(image(), "other", None, Some(&image_from), None),
            image(),
            "another computer gets it"
        );
        assert_eq!(
            what_follows(text(), "desk", None, Some(&image_from), None),
            text(),
            "the image's note says nothing of text"
        );
        // Not the same image twice in a row.
        assert_eq!(
            what_follows(image(), "desk", None, None, Some("ab12")),
            Held::Nothing
        );
        assert_eq!(
            what_follows(image(), "desk", None, None, Some("cd34")),
            image(),
            "another image was the last"
        );
        assert_eq!(
            what_follows(text(), "desk", None, None, Some("ab12")),
            text()
        );
    }

    /// BK3 review: an image on its way to a computer counts from the start
    /// of its send, so a switch's push and the watcher's never send it
    /// twice; a failed send lets the next one go, and only its own note.
    #[test]
    fn an_image_on_its_way_is_not_sent_again_and_a_failure_lets_it_go() {
        let image = || Held::Image("ab12".to_string());
        note_image_sent("bk3-flight", "ab12");
        assert!(!image_goes_to("bk3-flight", "ab12"), "the watcher's");
        let last = image_sent_last("bk3-flight");
        assert_eq!(
            what_follows(image(), "bk3-flight", None, None, last.as_deref()),
            Held::Nothing,
            "a switch's"
        );
        forget_image_sent_if("bk3-flight", "cd34");
        assert!(!image_goes_to("bk3-flight", "ab12"), "another's failure");
        forget_image_sent_if("bk3-flight", "ab12");
        assert!(image_goes_to("bk3-flight", "ab12"), "the next one goes");
        let last = image_sent_last("bk3-flight");
        assert_eq!(
            what_follows(image(), "bk3-flight", None, None, last.as_deref()),
            image()
        );
        // Told, it waits for the fetch only so long.
        note_image_told("bk3-flight", "ab12");
        assert!(!image_goes_to("bk3-flight", "ab12"), "told, on its way");
        forget_image_sent("bk3-flight");
        let now = std::time::Instant::now();
        let told = Sent {
            id: "ab12".to_string(),
            told_until: Some(now + TOLD_WAIT),
        };
        assert_eq!(sent_counts(&told, now), Some("ab12"));
        assert_eq!(sent_counts(&told, now + TOLD_WAIT), None, "never fetched");
        let sent = Sent {
            id: "ab12".to_string(),
            told_until: None,
        };
        assert_eq!(sent_counts(&sent, now + TOLD_WAIT * 100), Some("ab12"));
    }

    /// BK3: the image last sent to a computer is kept, each computer's own,
    /// until its clipboard holds something else.
    #[test]
    fn the_image_last_sent_is_each_computers_until_its_clipboard_changes() {
        assert_eq!(image_sent_last("bk3-desk-a"), None);
        note_image_sent("bk3-desk-a", "ab12");
        note_image_sent("bk3-desk-b", "cd34");
        assert_eq!(image_sent_last("bk3-desk-a").as_deref(), Some("ab12"));
        assert_eq!(image_sent_last("bk3-desk-b").as_deref(), Some("cd34"));
        note_image_sent("bk3-desk-a", "ef56");
        assert_eq!(image_sent_last("bk3-desk-a").as_deref(), Some("ef56"));
        forget_image_sent("bk3-desk-a");
        assert_eq!(image_sent_last("bk3-desk-a"), None, "something else since");
        assert_eq!(image_sent_last("bk3-desk-b").as_deref(), Some("cd34"));
        forget_image_sent("bk3-desk-b");
    }

    /// BK3: an image that follows the mouse is marked as text is, told to a
    /// computer linked as this one's client with its id and size, and
    /// offered on a kept link with its size and key, never a port.
    #[test]
    fn an_image_that_follows_is_marked_and_offered_without_a_port() {
        let told = follow_image_packet("ab12", 2048, Follow::Mouse);
        assert_eq!(told.packet_type, "kdeconnect.clipboard");
        assert_eq!(told.body["image"]["id"], "ab12");
        assert_eq!(told.body["image"]["size"], 2048);
        assert!(marked(&told.body));
        assert!(told.body.get("content").is_none());
        assert!(!marked(&image_packet("ab12", 2048).body), "M3's is not");

        let key = "0123456789abcdef0123456789abcdef";
        let offer = image_offer_packet(4096, key, Some(Follow::Mouse));
        assert_eq!(
            offer.packet_type,
            pocketdesk_core::protocol::clipboard::CLIPBOARD_IMAGE
        );
        assert_eq!(offer.body["size"], 4096);
        assert_eq!(offer.body["key"], key);
        assert!(offer.body.get("port").is_none(), "the link, not a dial");
        assert!(marked(&offer.body));
        let shared = image_offer_packet(4096, key, None);
        assert!(!marked(&shared.body));
        assert!(shared.body.get(FOLLOWS).is_none());
    }

    /// BK3: the image a computer tells of is read as this computer names
    /// one (64 hex digits) within N4's limit; nothing else is fetched.
    #[test]
    fn a_told_image_is_read_only_as_an_id_and_a_size_within_the_limit() {
        let id = "ab".repeat(32);
        let told = |id: &str, size: u64| serde_json::json!({"image": {"id": id, "size": size}});
        assert_eq!(told_image(&told(&id, 9)), Some((id.clone(), 9)));
        assert_eq!(
            told_image(&follow_image_packet(&id.to_uppercase(), 9, Follow::Mouse).body),
            Some((id.clone(), 9)),
            "either case"
        );
        assert_eq!(told_image(&told("ab12", 9)), None, "too short");
        assert_eq!(told_image(&told(&"zz".repeat(32), 9)), None, "not hex");
        assert_eq!(told_image(&told(&id, 0)), None, "empty");
        let max = pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE;
        assert_eq!(told_image(&told(&id, max + 1)), None, "too large");
        assert_eq!(told_image(&packet("text").body), None, "text");
    }

    /// BK3: a marked image is taken under the same rule as marked text.
    #[test]
    fn a_marked_image_is_taken_as_marked_text_is() {
        let yes = || true;
        let no = || false;
        let image = serde_json::json!({"image": {"id": "ab12", "size": 3}, "follows": true});
        let offer = serde_json::json!({"size": 3, "key": "k", "follows": true});
        for body in [&image, &offer] {
            assert!(follow_taken(marked(body), true, true, yes, yes, no));
            assert!(!follow_taken(marked(body), true, false, yes, yes, yes));
            assert!(!follow_taken(marked(body), true, true, no, yes, yes));
        }
        let unmarked = serde_json::json!({"image": {"id": "ab12", "size": 3}});
        assert!(!follow_taken(marked(&unmarked), true, true, yes, yes, yes));
    }

    /// BK3: a computer fetches only the image told to it at the last
    /// switch, and only while no switch has come since.
    #[test]
    fn a_fetch_is_served_only_for_the_image_told_at_the_last_switch() {
        let told = ("desk".to_string(), "ab12".to_string(), 7);
        let now = |at: u64| at == 7;
        assert!(fetch_told(Some(&told), "desk", "ab12", now));
        assert!(!fetch_told(Some(&told), "other", "ab12", now), "another");
        assert!(!fetch_told(Some(&told), "desk", "cd34", now), "another id");
        assert!(!fetch_told(None, "desk", "ab12", now), "nothing told");
        assert!(
            !fetch_told(Some(&told), "desk", "ab12", |_| false),
            "a later switch"
        );
    }

    /// The tests that move the process-wide switch counter ([`SWITCHES`])
    /// take turns: one moving it between another's steps would read as a
    /// later switch. Any test that moves it holds this.
    static SWITCH_TESTS: Mutex<()> = Mutex::new(());

    /// BK3: a later switch drops an image on its way, and while one from a
    /// computer lands, nothing goes back to it.
    #[test]
    fn a_later_switch_drops_an_image_on_its_way() {
        let _turn = SWITCH_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let at = switch();
        assert!(still(at));
        let landing = Landing::begin("bk3-desk");
        assert!(!still(at), "an image arriving is a switch");
        assert!(landing.current());
        assert!(landing_from("bk3-desk"));
        assert!(!landing_from("bk3-other"));
        switch();
        assert!(!landing.current(), "dropped by a later switch");
        assert!(!landing_from("bk3-desk"));
        let next = Landing::begin("bk3-desk");
        assert!(landing_from("bk3-desk"));
        drop(landing);
        assert!(
            landing_from("bk3-desk"),
            "an older one ending clears nothing"
        );
        drop(next);
        assert!(!landing_from("bk3-desk"), "landed or not, it is over");
        let before = switch();
        came_home();
        assert!(!still(before), "coming home is a switch");
    }

    /// BK3: another computer is told of an image as a phone is, without
    /// the phones' switch; neither without leave to be pushed the clipboard.
    #[test]
    fn computers_are_told_of_an_image_as_phones_are_without_their_switch() {
        let yes = || true;
        let no = || false;
        assert!(tells_image(true, false, yes), "a computer");
        assert!(tells_image(true, true, yes));
        assert!(tells_image(false, true, yes), "a phone, images on");
        assert!(!tells_image(false, false, yes), "a phone, images off");
        assert!(!tells_image(true, true, no), "not shared with computers");
        assert!(!tells_image(false, true, no), "no Clipboard permission");
        let unread = || -> bool { panic!("asked for nothing") };
        assert!(!tells_image(false, false, unread));
    }

    #[test]
    fn an_image_push_names_it_by_id_and_size() {
        let packet = image_packet("ab12", 2048);
        assert_eq!(packet.packet_type, "kdeconnect.clipboard");
        assert_eq!(packet.body["image"]["id"], "ab12");
        assert_eq!(packet.body["image"]["size"], 2048);
        assert!(packet.body.get("content").is_none());
    }

    /// PC3: a push that follows the window carries both marks; one that
    /// follows the mouse only its own, and is never read as the window's.
    #[test]
    fn a_push_that_follows_the_window_is_marked_as_the_windows() {
        let text = follow_packet("x", Follow::Window);
        assert!(marked(&text.body) && of_window(&text.body));
        assert_eq!(text.body["content"], "x");
        let image = follow_image_packet("ab12", 9, Follow::Window);
        assert!(of_window(&image.body));
        assert_eq!(image.body["image"]["size"], 9);
        let key = "0123456789abcdef0123456789abcdef";
        assert!(of_window(
            &image_offer_packet(9, key, Some(Follow::Window)).body
        ));
        for mouse in [
            follow_packet("x", Follow::Mouse).body,
            follow_image_packet("ab12", 9, Follow::Mouse).body,
            image_offer_packet(9, key, Some(Follow::Mouse)).body,
        ] {
            assert!(marked(&mouse) && !of_window(&mouse), "{mouse}");
            assert!(mouse.get(WINDOW).is_none());
        }
        // The window's mark alone is not a follow.
        assert!(!of_window(
            &serde_json::json!({"content": "x", "view": true})
        ));
        assert!(!of_window(
            &serde_json::json!({"content": "x", "follows": true, "view": 1})
        ));
    }

    /// PC3: a push that follows the window is taken with the setting on,
    /// from a paired computer that controls this one through its window or
    /// whose answer this one awaits; never with the setting off, nor from a
    /// phone.
    #[test]
    fn a_window_push_is_taken_from_the_computer_that_controls_or_was_asked() {
        let yes = || true;
        let no = || false;
        assert!(
            window_taken(true, yes, yes, yes, no),
            "it controls this one"
        );
        assert!(
            window_taken(true, yes, no, no, yes),
            "this one asked it back"
        );
        assert!(!window_taken(true, yes, no, yes, no), "neither");
        assert!(
            !window_taken(false, yes, yes, yes, yes),
            "the setting off here"
        );
        assert!(!window_taken(true, no, yes, yes, yes), "not a computer");
        // The ask is used up only when the rest holds.
        let unread = || -> bool { panic!("not asked") };
        assert!(!window_taken(false, unread, unread, unread, unread));
        assert!(!window_taken(true, no, unread, unread, unread));
        // The awaited answer is looked at first, and used up, even from a
        // computer that also controls this one (PR #572 review I2).
        let looked = std::cell::Cell::new(false);
        assert!(window_taken(true, yes, yes, yes, || {
            looked.set(true);
            false
        }));
        assert!(looked.get());
        assert!(window_taken(true, yes, unread, unread, yes), "asked: taken");
    }

    /// PR #554 review I1: with Clipboard off here for the computer that
    /// controls this one, its window neither sets this clipboard nor reads
    /// it, whichever road the packet came by.
    #[test]
    fn clipboard_off_here_takes_no_window_push_and_answers_no_ask() {
        let yes = || true;
        let no = || false;
        assert!(
            !window_taken(true, yes, yes, no, no),
            "it controls, but Clipboard is off for it"
        );
        assert!(window_ask_taken(true, yes, yes, yes));
        assert!(!window_ask_taken(true, yes, yes, no), "Clipboard off");
        assert!(!window_ask_taken(true, yes, no, yes), "not controlling");
        assert!(!window_ask_taken(true, no, yes, yes), "not a computer");
        assert!(!window_ask_taken(false, yes, yes, yes), "the setting off");
        let unread = || -> bool { panic!("not asked") };
        assert!(!window_ask_taken(false, unread, unread, unread));
        assert!(!window_ask_taken(true, yes, no, unread));
    }

    /// PR #554 review M5: the log says what a push followed.
    #[test]
    fn the_log_says_whether_the_mouse_or_the_window_was_followed() {
        assert_eq!(Follow::Mouse.words(), "the mouse");
        assert_eq!(Follow::Window.words(), "the window");
    }

    /// PR #554 review I2: a computer's asks are answered at most once each
    /// ANSWER_EVERY, and only with something new since its last answer.
    #[test]
    fn asks_are_answered_now_and_then_and_only_with_something_new() {
        let t = std::time::Instant::now();
        let mut answers = Answers::default();
        assert!(answers.may_answer("desk", t), "the first");
        assert!(answers.fresh("desk", 7));
        assert!(
            !answers.may_answer("desk", t + Duration::from_millis(500)),
            "too soon"
        );
        assert!(answers.may_answer("other", t), "each computer its own");
        assert!(
            answers.may_answer("desk", t + ANSWER_EVERY),
            "after the wait"
        );
        assert!(!answers.fresh("desk", 7), "the same again: no answer");
        assert!(answers.fresh("desk", 8), "a new copy");
        assert!(
            answers.fresh("other", 7),
            "another computer's last is its own"
        );
        // A refused try still counts against the wait.
        assert!(!answers.may_answer("desk", t + ANSWER_EVERY + Duration::from_millis(1)));
    }

    /// PC3: an answer is awaited from the computer asked, within the wait,
    /// and once.
    #[test]
    fn an_answer_is_awaited_from_the_computer_asked_for_a_while() {
        let at = std::time::Instant::now();
        assert!(awaits(Some(&at), at));
        assert!(awaits(
            Some(&at),
            at + ANSWER_WAIT - Duration::from_millis(1)
        ));
        assert!(!awaits(Some(&at), at + ANSWER_WAIT), "too late");
        assert!(!awaits(None, at), "never asked");

        let handing = Handing::default();
        lock(&handing.asked).insert("desk".to_string(), at);
        assert!(
            !answer_awaited_in(&handing, "other", at),
            "another computer"
        );
        assert!(answer_awaited_in(&handing, "desk", at));
        assert!(!answer_awaited_in(&handing, "desk", at), "used up");
        assert_eq!(*lock(&handing.answered), Some(at));
    }

    /// Range review I2: a push to C undoes C's own ask, not B's.
    #[tokio::test]
    async fn a_push_to_one_computer_leaves_anothers_ask() {
        let handing = Handing::default();
        let long_ago = std::time::Instant::now() - HANDOVER_WAIT;
        lock(&handing.asked).insert("b".to_string(), long_ago);
        lock(&handing.asked).insert("c".to_string(), long_ago);
        let pushed = std::sync::atomic::AtomicBool::new(false);
        push_for_window_in(&handing, "c", async {
            pushed.store(true, Ordering::Relaxed);
        })
        .await;
        assert!(pushed.load(Ordering::Relaxed));
        assert!(lock(&handing.asked).contains_key("b"), "B's ask stands");
        assert!(!lock(&handing.asked).contains_key("c"));
    }

    /// Range review I2: a push to C waits only for a recent ask to another
    /// computer.
    #[test]
    fn a_push_waits_only_for_another_computers_recent_ask() {
        let at = std::time::Instant::now();
        let mut asked = HashMap::new();
        asked.insert("b".to_string(), at);
        assert!(asked_elsewhere(&asked, "c", at));
        assert!(!asked_elsewhere(&asked, "b", at), "its own");
        assert!(
            !asked_elsewhere(&asked, "c", at + HANDOVER_WAIT),
            "an old ask holds nothing up"
        );
    }

    /// PR #572 review I2: a push waits for B's answer, then for it to be
    /// written here, then the settle; and no longer than the wait.
    #[tokio::test]
    async fn a_push_waits_for_the_answer_to_be_written() {
        let handing = std::sync::Arc::new(Handing::default());
        lock(&handing.asked).insert("b".to_string(), std::time::Instant::now());
        let start = std::time::Instant::now();
        let waiting = {
            let handing = handing.clone();
            tokio::spawn(async move { wait_for_other_answers(&handing, "c").await })
        };
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!waiting.is_finished(), "B's answer is awaited");
        assert!(answer_awaited_in(&handing, "b", std::time::Instant::now()));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(!waiting.is_finished(), "taken, not written yet");
        *lock(&handing.written) = Some(std::time::Instant::now());
        waiting.await.unwrap();
        let took = start.elapsed();
        assert!(
            took >= Duration::from_millis(120) + ANSWER_SETTLE,
            "{took:?}"
        );
        assert!(took < HANDOVER_WAIT, "{took:?}");

        // No answer at all: the wait ends with HANDOVER_WAIT.
        let handing = Handing::default();
        lock(&handing.asked).insert("b".to_string(), std::time::Instant::now());
        let start = std::time::Instant::now();
        wait_for_other_answers(&handing, "c").await;
        let took = start.elapsed();
        assert!(
            took >= HANDOVER_WAIT - Duration::from_millis(30),
            "{took:?}"
        );
        assert!(
            took < HANDOVER_WAIT + Duration::from_millis(500),
            "{took:?}"
        );
    }

    /// Range review I2, PR #572 review I2: the focus moving between two
    /// windows of the same computer asks and pushes nothing, whichever is
    /// heard first; a loss long before is no hand-over.
    #[test]
    fn the_focus_between_windows_of_one_computer_does_nothing() {
        let t = std::time::Instant::now();
        let ms = Duration::from_millis;
        let mut moves = FocusMoves::default();
        // The loss first.
        let turn = moves
            .note("b", false, t)
            .expect("a loss alone waits to ask");
        assert_eq!(
            moves.note("b", true, t + ms(5)),
            None,
            "a window of B took it"
        );
        assert!(!moves.stands("b", turn), "so B is not asked");
        // The gain first.
        let turn = moves
            .note("b", true, t + ms(1000))
            .expect("a gain alone waits");
        assert_eq!(moves.note("b", false, t + ms(1005)), None);
        assert!(!moves.stands("b", turn), "so nothing is pushed");
        // B to C: both go, C's push after B's loss is asked.
        let turn = moves.note("b", false, t + ms(2000)).unwrap();
        assert!(moves.other_losing("c", t + ms(2005)), "C waits for B");
        assert!(moves.note("c", true, t + ms(2005)).is_some());
        assert!(moves.stands("b", turn), "and B is asked");
        moves.asked("b", turn);
        assert!(!moves.other_losing("c", t + ms(2310)));
        // A loss long before is no hand-over.
        let turn = moves.note("b", false, t + ms(3000)).unwrap();
        assert!(moves.stands("b", turn));
        assert!(moves.note("b", true, t + ms(3000) + HANDOVER).is_some());
    }

    /// PR #572 review I2: a gain heard before its loss waits for it. From
    /// C's window to B's own other window, nothing is pushed; from B's
    /// window to C's, C's push goes once B's loss is heard, before the wait
    /// is out, and then waits for B's answer.
    #[tokio::test]
    async fn a_gain_heard_first_waits_for_the_loss() {
        let handing = std::sync::Arc::new(Handing::default());
        // B's window holds the focus.
        lock(&handing.moves).note("b", true, std::time::Instant::now() - HANDOVER);
        let at = std::time::Instant::now();
        let turn = lock(&handing.moves).note("c", true, at).unwrap();
        let going = {
            let handing = handing.clone();
            tokio::spawn(async move { gain_goes(&handing, "c", turn, at).await })
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(!going.is_finished(), "waiting for a loss");
        lock(&handing.moves).note("b", false, std::time::Instant::now());
        assert!(going.await.unwrap(), "C's push goes");
        assert!(at.elapsed() < HANDOVER, "as soon as B's loss is heard");
        assert!(
            lock(&handing.moves).other_losing("c", std::time::Instant::now()),
            "and waits for B"
        );

        let handing = std::sync::Arc::new(Handing::default());
        // B's first window holds the focus.
        lock(&handing.moves).note("b", true, std::time::Instant::now() - HANDOVER);
        let at = std::time::Instant::now();
        let turn = lock(&handing.moves).note("b", true, at).unwrap();
        let going = {
            let handing = handing.clone();
            tokio::spawn(async move { gain_goes(&handing, "b", turn, at).await })
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            lock(&handing.moves).note("b", false, std::time::Instant::now()),
            None,
            "B's other window's loss asks nothing"
        );
        assert!(!going.await.unwrap(), "and nothing is pushed");

        // From another program (no window held the focus): at once (PR
        // #572 re-review N5).
        let handing = Handing::default();
        let at = std::time::Instant::now();
        let turn = lock(&handing.moves).note("b", true, at).unwrap();
        assert!(gain_goes(&handing, "b", turn, at).await);
        assert!(at.elapsed() < HANDOVER);
        // After another computer's loss heard first: at once too.
        let handing = Handing::default();
        lock(&handing.moves).note("c", true, std::time::Instant::now() - HANDOVER * 2);
        lock(&handing.moves).note("c", false, std::time::Instant::now());
        let at = std::time::Instant::now();
        let turn = lock(&handing.moves).note("b", true, at).unwrap();
        assert!(gain_goes(&handing, "b", turn, at).await);
        assert!(at.elapsed() < HANDOVER);
        // A window holding the focus that never says it lost it: the wait
        // runs out and the push goes.
        let handing = Handing::default();
        lock(&handing.moves).note("c", true, std::time::Instant::now());
        let at = std::time::Instant::now();
        let turn = lock(&handing.moves).note("b", true, at).unwrap();
        assert!(gain_goes(&handing, "b", turn, at).await);
        assert!(at.elapsed() >= HANDOVER);
    }

    /// PR #572 re-review N3: with no watcher counting copies, each read is
    /// a new copy, so a copy made again is never skipped as the same text.
    #[cfg(not(windows))]
    #[test]
    fn without_a_watcher_every_read_is_a_new_copy() {
        WATCHING.store(false, Ordering::Relaxed);
        let first = copy_generation();
        assert_ne!(copy_generation(), first);
    }

    /// Range review I2, PR #572 review I2: the text a window sent a
    /// computer last is not sent again from the same copy; a copy made
    /// again here goes, and so does anything after something else went
    /// there or came from it.
    #[test]
    fn the_same_text_is_not_sent_twice_but_a_new_copy_is() {
        let id = "pr572-same-text";
        let hello = Held::Text("hello".to_string());
        WINDOW_SENT
            .lock()
            .unwrap()
            .insert(id.to_string(), (hash("hello"), 7));
        assert!(window_push_skipped(id, Follow::Window, false, &hello, 7));
        assert!(
            !window_push_skipped(id, Follow::Window, false, &hello, 8),
            "copied again"
        );
        assert!(
            !window_push_skipped(id, Follow::Window, true, &hello, 7),
            "an answer"
        );
        assert!(!window_push_skipped(id, Follow::Mouse, false, &hello, 7));
        let other = Held::Text("other".to_string());
        assert!(!window_push_skipped(id, Follow::Window, false, &other, 7));
        forget_image_sent(id);
        assert!(
            !window_push_skipped(id, Follow::Window, false, &hello, 7),
            "something else went there"
        );
        WINDOW_SENT
            .lock()
            .unwrap()
            .insert(id.to_string(), (hash("hello"), 7));
        note_image_sent(id, "ab12");
        assert_eq!(window_sent_last(id), None, "an image went there");
        forget_image_sent(id);
    }

    /// PC3: a computer controls this one through its window when a live
    /// session of its shows the screen and takes its input.
    #[test]
    fn a_computer_controls_this_one_with_a_screen_session_that_takes_input() {
        use pocketdesk_core::streaming::sessions::{LiveSession, Streamed};
        let session = |device: &str, video: bool, input: bool| LiveSession {
            id: 1,
            streamed: Streamed {
                video,
                input,
                ..Streamed::default()
            },
            device_id: Some(device.to_string()),
            peer: "192.0.2.4:4000".parse().unwrap(),
            lan: true,
            started_unix: 0,
        };
        assert!(session_controls(&[session("desk", true, true)], "desk"));
        assert!(
            !session_controls(&[session("desk", true, false)], "desk"),
            "view-only"
        );
        assert!(
            !session_controls(&[session("desk", false, true)], "desk"),
            "a remote"
        );
        assert!(!session_controls(&[session("other", true, true)], "desk"));
        assert!(!session_controls(&[], "desk"));
    }

    /// PC3: the window's focus pushes as it comes and asks back as it goes,
    /// only with the setting on, the always-on sharing off, and the other
    /// computer taking this one's input.
    #[test]
    fn the_windows_focus_pushes_or_asks_back_only_while_it_controls() {
        assert_eq!(
            window_focus_does(true, true, false, Some(true)),
            Some(WindowFocus::Push)
        );
        assert_eq!(
            window_focus_does(false, true, false, Some(true)),
            Some(WindowFocus::AskBack)
        );
        for focused in [true, false] {
            assert_eq!(window_focus_does(focused, false, false, Some(true)), None);
            assert_eq!(
                window_focus_does(focused, true, true, Some(true)),
                None,
                "Clipboard with my computers carries it already"
            );
            assert_eq!(window_focus_does(focused, true, false, Some(false)), None);
            assert_eq!(window_focus_does(focused, true, false, None), None);
        }
    }
}
