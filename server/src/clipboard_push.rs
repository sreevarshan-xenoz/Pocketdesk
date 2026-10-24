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
//! those this one keeps a link to (`peer_computers`). Never back to the
//! device it came from.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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
    *FROM_PHONE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((device_id.to_string(), text.to_string()));
}

/// The device the clipboard's image came from, with the image's id (O7):
/// M3's push does not tell it back to that device.
static IMAGE_FROM: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Note that `device_id` just put the image `id` on the clipboard.
pub(crate) fn note_image_from(device_id: &str, id: &str) {
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

/// Whether a connection is told of an image (M3): a phone, never another
/// computer (they share text only), that may be pushed the clipboard.
fn may_push_image(device_id: &str) -> bool {
    !crate::plugins::pair::is_computer_device(device_id) && may_push(device_id)
}

/// The packet a push sends: what the phone's own request is answered with.
pub(crate) fn packet(text: &str) -> NetworkPacket {
    NetworkPacket::new("kdeconnect.clipboard").with_body(serde_json::json!({ "content": text }))
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
                Ok(text) => match image_now(text.is_none(), &clients).await {
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
            let Ok(image) = image_now(text.is_none(), &clients).await else {
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

/// The clipboard's image to tell phones of (M3), when it holds no text,
/// "Images to phones' clipboards" is on (read live from the config) and a
/// phone among `clients` may have it: never read and encoded for no one
/// (M3 review). An error says the clipboard could not be read now.
async fn image_now(
    no_text: bool,
    clients: &[Arc<dyn DeviceSender>],
) -> pocketdesk_core::error::Result<Option<(String, u64)>> {
    if !no_text
        || !crate::events::current_config().clipboard_images
        || !TO_PHONES.load(Ordering::Relaxed)
        || !clients.iter().any(|c| may_push_image(c.device_id()))
    {
        return Ok(None);
    }
    crate::plugins::clipboard::image_for_push().await
}

/// Push `text` to `clients`, and to the computers this one keeps a link to
/// (`linked`), if it is a change; or, with no text, tell the phones of
/// `image` (M3).
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
            push_image(&id, size, clients).await;
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
            Ok(()) => tracing::info!(
                "Clipboard sent to {} ({} chars)",
                target.device_id(),
                text.chars().count()
            ),
            Err(e) => tracing::debug!("Clipboard not pushed to {}: {e}", target.device_id()),
        }
    }
    for computer in linked {
        if !came_from(from_phone.as_ref(), &computer.device_id, &text) {
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

/// Tell the phones that may have it of the clipboard's image (M3).
async fn push_image(id: &str, size: u64, clients: Vec<Arc<dyn DeviceSender>>) {
    let packet = image_packet(id, size);
    for target in clients
        .into_iter()
        .filter(|c| may_push_image(c.device_id()) && !image_came_from(c.device_id(), id))
    {
        if let Err(e) = target.send_packet(&packet).await {
            tracing::debug!("Clipboard image not told to {}: {e}", target.device_id());
        }
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

    #[test]
    fn an_image_push_names_it_by_id_and_size() {
        let packet = image_packet("ab12", 2048);
        assert_eq!(packet.packet_type, "kdeconnect.clipboard");
        assert_eq!(packet.body["image"]["id"], "ab12");
        assert_eq!(packet.body["image"]["size"], 2048);
        assert!(packet.body.get("content").is_none());
    }
}
