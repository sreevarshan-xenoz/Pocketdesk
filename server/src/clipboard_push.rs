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
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// The longest clipboard text pushed. A phone asking in a session still
/// gets longer text; a push goes to every phone, so it stays small.
pub(crate) const MAX_PUSH: usize = 256 * 1024;

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
        text.filter(|t| t.len() <= MAX_PUSH).map(str::to_string)
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
/// text went to it, or it sent this one something of its own.
fn forget_image_sent(device_id: &str) {
    IMAGE_SENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(device_id);
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
/// clipboard is the newer.
pub(crate) async fn push_for_window(device_id: &str) {
    ASKED_BACK.lock().unwrap_or_else(|e| e.into_inner()).take();
    push_following(device_id, Follow::Window).await
}

/// Answer the computer `device_id`'s ask for this computer's clipboard
/// back (PC3, [`ask_back`]), as [`push_now`] sends it, marked as
/// following the window.
pub(crate) async fn answer_window_ask(device_id: &str) {
    push_following(device_id, Follow::Window).await
}

/// [`push_now`], following `follow`.
async fn push_following(device_id: &str, follow: Follow) {
    // That computer's image still on its way here: this clipboard is about
    // to be its own, so nothing older goes back, and the switch drops it.
    let landing = landing_from(device_id);
    let at = switch();
    if landing {
        tracing::debug!(
            "The clipboard did not follow the mouse to {device_id}: its image was still on its way here"
        );
        return;
    }
    let held = match read_held().await {
        Ok(held) => held,
        Err(e) => {
            tracing::debug!("The clipboard did not follow the mouse: not read: {e}");
            return;
        }
    };
    let text_from = FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let image_from = IMAGE_FROM.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let last_image = image_sent_last(device_id);
    match what_follows(
        held,
        device_id,
        text_from.as_ref(),
        image_from.as_ref(),
        last_image.as_deref(),
    ) {
        Held::Text(text) => push_text_now(device_id, &text, follow).await,
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
async fn push_text_now(device_id: &str, text: &str, follow: Follow) {
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
            crate::clip_history::note(text, None);
            tracing::info!(
                "Clipboard followed the mouse to {device_id} ({} chars)",
                text.chars().count()
            );
        }
        Err(e) => tracing::debug!("The clipboard did not follow the mouse to {device_id}: {e}"),
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
            "Clipboard image followed the mouse to {device_id} ({size} bytes, {how})"
        ),
        Err(e) => {
            forget_image_sent_if(device_id, id);
            tracing::debug!("The clipboard image did not follow the mouse to {device_id}: {e}")
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
/// off here ([`follow_taken`]). Reads files (blocking), but only for a
/// marked packet.
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
    )
}

/// Whether a push that follows the window (PC3) is taken: with
/// `computer_view_clipboard` on here (`on`), from one of the owner's paired
/// computers (`is_computer`) that controls this one through its window now
/// (`controls`: its screen session here takes its input), or whose answer
/// this one awaits (`awaited`: this computer's window lost the focus and
/// asked it back; used once). Each later word is asked only when those
/// before it hold.
fn window_taken(
    on: bool,
    is_computer: impl FnOnce() -> bool,
    controls: impl FnOnce() -> bool,
    awaited: impl FnOnce() -> bool,
) -> bool {
    on && is_computer() && (controls() || awaited())
}

/// Whether the computer `device_id` may ask for this computer's clipboard
/// back as its window loses the focus (PC3): with `computer_view_clipboard`
/// on here, a paired computer that controls this one through its window
/// now.
pub(crate) fn takes_window_ask(device_id: &str) -> bool {
    crate::events::current_config().computer_view_clipboard
        && is_paired_computer(device_id)
        && controls_here(device_id)
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

/// The computer this one asked for its clipboard back, and when.
static ASKED_BACK: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);

/// Whether an answer from `device_id` is awaited at `now`, given the ask
/// `asked`: from that computer, within [`ANSWER_WAIT`].
fn awaits(
    asked: Option<&(String, std::time::Instant)>,
    device_id: &str,
    now: std::time::Instant,
) -> bool {
    asked.is_some_and(|(from, at)| {
        from == device_id && now.saturating_duration_since(*at) < ANSWER_WAIT
    })
}

/// Whether an answer from `device_id` is awaited now; the ask is used up.
fn answer_awaited(device_id: &str, now: std::time::Instant) -> bool {
    let mut asked = ASKED_BACK.lock().unwrap_or_else(|e| e.into_inner());
    let awaited = awaits(asked.as_ref(), device_id, now);
    if awaited {
        *asked = None;
    }
    awaited
}

/// The window showing the computer `device_id`'s screen lost the focus
/// while it controlled that computer (PC3): ask that computer for its
/// clipboard, which comes back as a push that follows the window.
pub(crate) async fn ask_back(device_id: &str) {
    *ASKED_BACK.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), std::time::Instant::now()));
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
async fn send_to_computer(device_id: &str, packet: &NetworkPacket) -> Result<(), String> {
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
                            flag.store(true, Ordering::Relaxed)
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
        assert!(window_taken(true, yes, yes, no), "it controls this one");
        assert!(window_taken(true, yes, no, yes), "this one asked it back");
        assert!(!window_taken(true, yes, no, no), "neither");
        assert!(!window_taken(false, yes, yes, yes), "the setting off here");
        assert!(!window_taken(true, no, yes, yes), "not a computer");
        // The ask is used up only when the rest holds.
        let unread = || -> bool { panic!("not asked") };
        assert!(!window_taken(false, unread, unread, unread));
        assert!(!window_taken(true, no, unread, unread));
        assert!(
            window_taken(true, yes, yes, unread),
            "controls: no ask used"
        );
    }

    /// PC3: an answer is awaited from the computer asked, within the wait,
    /// and once.
    #[test]
    fn an_answer_is_awaited_from_the_computer_asked_for_a_while() {
        let at = std::time::Instant::now();
        let asked = ("pc3-desk".to_string(), at);
        assert!(awaits(Some(&asked), "pc3-desk", at));
        assert!(awaits(
            Some(&asked),
            "pc3-desk",
            at + ANSWER_WAIT - Duration::from_millis(1)
        ));
        assert!(
            !awaits(Some(&asked), "pc3-desk", at + ANSWER_WAIT),
            "too late"
        );
        assert!(!awaits(Some(&asked), "pc3-other", at), "another computer");
        assert!(!awaits(None, "pc3-desk", at), "never asked");

        *ASKED_BACK.lock().unwrap() = Some(("pc3-desk-once".to_string(), at));
        assert!(answer_awaited("pc3-desk-once", at));
        assert!(!answer_awaited("pc3-desk-once", at), "used up");
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
