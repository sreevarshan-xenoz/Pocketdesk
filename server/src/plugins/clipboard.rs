//! The clipboard, both ways: text on the control channel, and images (roadmap
//! N4) through the transfer path inside TLS, since an image is megabytes.
//!
//! The phone drives it. It pushes its text (`kdeconnect.clipboard`) or its
//! image (`clipboard_image`, the desktop dials back for the PNG), and asks
//! for the desktop's (`kdeconnect.clipboard.connect`). The answer carries the
//! text, or, when the clipboard holds an image and no text, the image's id
//! (the hex SHA-256 of its PNG) and size; the phone then fetches it
//! (`clipboard_image_fetch`). Both ends know an image by that id, so neither
//! sends back what it just received.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::clipboard::{
    CLIPBOARD_IMAGE, CLIPBOARD_IMAGE_FETCH, MAX_CLIPBOARD_IMAGE,
};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;

use crate::plugins::share;

#[derive(Debug)]
pub struct ClipboardPlugin;

impl Default for ClipboardPlugin {
    fn default() -> Self {
        Self
    }
}

impl ClipboardPlugin {
    pub fn new() -> Self {
        Self
    }
}

/// What the desktop's clipboard holds, as the phone is told.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Clip {
    Text(String),
    Image(Arc<Image>),
    Empty,
}

/// A clipboard image as PNG, known by its id.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Image {
    pub(crate) id: String,
    pub(crate) png: Vec<u8>,
}

impl Image {
    fn new(png: Vec<u8>) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, &png);
        let id = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        Self { id, png }
    }
}

/// The last image read from or put on the clipboard: what a fetch may take,
/// and on Windows the clipboard sequence number it was read at, so an
/// unchanged clipboard is not read and encoded again every poll. `image` is
/// `None` when what the clipboard held at that number gave no usable image
/// (too large, a bitmap kind not read), so it is not tried again either.
struct Cached {
    // Only Windows has a sequence number to compare; Linux records `None`.
    #[cfg_attr(not(windows), allow(dead_code))]
    sequence: Option<u32>,
    image: Option<Arc<Image>>,
}

fn cache() -> &'static Mutex<Option<Cached>> {
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);
    &CACHE
}

fn remember(sequence: Option<u32>, image: Option<Arc<Image>>) {
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached { sequence, image });
}

/// The clipboard holds text now: no image is the clipboard's any more.
fn forget_image() {
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The answer to `kdeconnect.clipboard.connect`.
fn reply_body(clip: &Clip) -> serde_json::Value {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    match clip {
        Clip::Text(text) => json!({ "content": text, "timestamp": timestamp }),
        Clip::Image(image) => json!({
            "image": { "id": image.id, "size": image.png.len() },
            "timestamp": timestamp,
        }),
        Clip::Empty => json!({ "content": "", "timestamp": timestamp }),
    }
}

/// The image a fetch names, while it is still the clipboard's.
fn image_for_fetch(cached: Option<&Cached>, id: &str) -> std::result::Result<Arc<Image>, String> {
    match cached.and_then(|cached| cached.image.as_ref()) {
        Some(image) if image.id == id => Ok(image.clone()),
        _ => Err("the clipboard no longer holds that image".to_string()),
    }
}

#[async_trait::async_trait]
impl Plugin for ClipboardPlugin {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.clipboard",
            "kdeconnect.clipboard.connect",
            CLIPBOARD_IMAGE,
            CLIPBOARD_IMAGE_FETCH,
        ]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.clipboard", share::FETCH_REFUSED]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        // Another computer (2f) has the clipboard, text or image, read or
        // set, only while this one shares it with its computers (2g); or
        // its text or image as the shared keyboard and mouse's cursor
        // switches, while the clipboard follows the mouse here (BK3).
        if crate::plugins::pair::is_computer_device(sender.device_id())
            && !crate::clipboard_push::with_computers()
            && !follows_the_mouse(packet, sender.device_id())
        {
            tracing::debug!(
                "Another computer's {}, with the clipboard not shared here; ignored",
                packet.packet_type
            );
            // An image's transfer is refused, so that computer stops waiting.
            if matches!(
                packet.packet_type.as_str(),
                CLIPBOARD_IMAGE | CLIPBOARD_IMAGE_FETCH
            ) && let Some(key) = transfer_key(packet)
            {
                share::refuse(sender, &key, "the clipboard is not shared here").await;
            }
            return Ok(());
        }
        match packet.packet_type.as_str() {
            "kdeconnect.clipboard" => {
                // Remote clipboard update - set local clipboard
                if let Some(content) = packet.body.get("content").and_then(|v| v.as_str()) {
                    if crate::plugins::pair::is_computer_device(sender.device_id()) {
                        crate::clipboard_push::follow_arrived(&packet.body);
                    }
                    // Not pushed back to the phone it came from (T10b).
                    crate::clipboard_push::note_from_phone(sender.device_id(), content);
                    if let Err(e) = set_clipboard(content).await {
                        tracing::warn!("Failed to set clipboard: {}", e);
                    }
                }
            }
            "kdeconnect.clipboard.connect" => {
                // Remote requested our clipboard - get and send it. A
                // clipboard that cannot be read is answered as empty, so the
                // phone is not left waiting out its query.
                let clip = match read_clipboard().await {
                    Ok(Clip::Text(text)) if is_pairing_link(&text) => Clip::Empty,
                    // An image this phone put here is not handed back to
                    // it, which would fetch it again (O7 review).
                    Ok(Clip::Image(image))
                        if crate::clipboard_push::image_came_from(
                            sender.device_id(),
                            &image.id,
                        ) =>
                    {
                        Clip::Empty
                    }
                    Ok(clip) => clip,
                    Err(e) => {
                        tracing::debug!("Could not read the clipboard: {e}");
                        Clip::Empty
                    }
                };
                let response =
                    NetworkPacket::new("kdeconnect.clipboard").with_body(reply_body(&clip));
                sender.send_packet(&response).await?;
                // A text handed to the phone that asked crossed too (D3
                // review); the same text again is not written twice.
                if let Clip::Text(text) = &clip
                    && !text.is_empty()
                {
                    crate::clip_history::note(text, None);
                }
            }
            CLIPBOARD_IMAGE => receive_image(packet, sender).await,
            CLIPBOARD_IMAGE_FETCH => send_image(packet, sender).await,
            _ => {}
        }
        Ok(())
    }
}

/// Whether another computer's `packet` is one following the mouse that is
/// taken here with "Clipboard with my computers" off: its text or the
/// offer of its image, marked ([`crate::clipboard_push::takes_follow`]), or
/// the fetch of the image this one told it of at the last switch.
fn follows_the_mouse(packet: &NetworkPacket, device_id: &str) -> bool {
    match packet.packet_type.as_str() {
        "kdeconnect.clipboard" | CLIPBOARD_IMAGE => {
            crate::clipboard_push::takes_follow(device_id, &packet.body)
        }
        CLIPBOARD_IMAGE_FETCH => packet
            .body
            .get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| crate::clipboard_push::fetch_follows(device_id, id)),
        _ => false,
    }
}

/// How an image reaches the phone that asked: dialing it back at home, or
/// on a stream of its own link by the internet road when the request names
/// no port (2e, as files do since 2c).
fn transfer_route(
    packet: &NetworkPacket,
    sender: &dyn DeviceSender,
) -> std::result::Result<share::Route, String> {
    let port = packet
        .body
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0);
    share::route(share::reach(sender, port)?, sender)
}

fn transfer_key(packet: &NetworkPacket) -> Option<String> {
    packet
        .body
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|k| share::is_transfer_key(k))
        .map(str::to_string)
}

/// The phone's clipboard image: reach the phone, read the PNG, put it on
/// the clipboard; the phone hears "saved" only once the clipboard holds it.
async fn receive_image(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(key) = transfer_key(packet) else {
        tracing::warn!("Clipboard image refused: no transfer key");
        return;
    };
    let size = packet
        .body
        .get("size")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let plan = (|| -> std::result::Result<_, String> {
        if size == 0 || size > MAX_CLIPBOARD_IMAGE {
            return Err(format!("an image of {size} bytes is not taken"));
        }
        let route = transfer_route(packet, sender)?;
        let slot = share::transfer_slots().take(sender.device_id())?;
        Ok((route, slot))
    })();
    let (route, slot) = match plan {
        Ok(plan) => plan,
        Err(reason) => {
            tracing::warn!("Clipboard image refused: {reason}");
            share::refuse(sender, &key, &reason).await;
            return;
        }
    };
    let from = sender.device_id().to_string();
    // Another computer's, following the mouse (BK3): a later switch drops it.
    let landing = (crate::plugins::pair::is_computer_device(&from)
        && crate::clipboard_push::marked(&packet.body))
    .then(|| crate::clipboard_push::Landing::begin(&from));
    tokio::spawn(async move {
        let _slot = slot;
        let from = &from;
        let kept = share::receive_bytes(route, &key, size, MAX_CLIPBOARD_IMAGE, |png| async move {
            if landing.as_ref().is_some_and(|l| !l.current()) {
                return Err(share::io_error("clipboard", "a later switch came first"));
            }
            put_image(from, png, None).await
        })
        .await;
        match kept {
            Ok(()) => tracing::info!("Clipboard image received from {from} ({size} bytes)"),
            Err(e) => tracing::warn!("Clipboard image from {from} not taken: {e}"),
        }
    });
}

/// Put the PNG `png` that the device `from` sent on the clipboard; with
/// `id`, only if it is that image. Not told back to `from` (O7 review of
/// M3), unless the clipboard did not take it.
pub(crate) async fn put_image(from: &str, png: Vec<u8>, id: Option<&str>) -> Result<()> {
    if !crate::platform::clip_image::is_png(&png) {
        return Err(share::io_error("clipboard", "not a PNG"));
    }
    let image = Arc::new(Image::new(png));
    if id.is_some_and(|id| id != image.id) {
        return Err(share::io_error("clipboard", "not the image it named"));
    }
    crate::clipboard_push::note_image_from(from, &image.id);
    if let Err(e) = set_clipboard_image(&image).await {
        crate::clipboard_push::forget_image_from(&image.id);
        return Err(e);
    }
    Ok(())
}

/// The largest image file put on the clipboard as it arrives (O7), as N4's.
pub(crate) const MAX_RECEIVED_IMAGE: u64 = 32 << 20;

/// O7: put the image file `from` sent, saved at `path`, on the clipboard, if
/// it is a PNG or a JPEG of at most [`MAX_RECEIVED_IMAGE`]; read and
/// converted off the runtime. Not told back to `from` (M3's push).
pub(crate) fn put_received_image(path: std::path::PathBuf, size: u64, from: String) {
    if size > MAX_RECEIVED_IMAGE {
        return;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        // One at a time: a decoded photo takes hundreds of megabytes, and a
        // burst of them must not take them all at once (O7 review).
        static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let Ok(_turn) = ONE_AT_A_TIME.acquire().await else {
            return;
        };
        let read = tokio::task::spawn_blocking(move || -> std::result::Result<Vec<u8>, String> {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|e| e.to_string())?
                .take(MAX_RECEIVED_IMAGE + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_RECEIVED_IMAGE {
                return Err("too large".to_string());
            }
            let png = crate::platform::clip_image::png_from_file(&bytes)?;
            // As large as N4's images may be, made PNG (O7 review).
            if png.len() as u64 > MAX_CLIPBOARD_IMAGE {
                return Err("too large as a PNG".to_string());
            }
            Ok(png)
        })
        .await;
        let png = match read {
            Ok(Ok(png)) => png,
            Ok(Err(e)) => {
                tracing::debug!("A received file is not put on the clipboard: {e}");
                return;
            }
            Err(_) => return,
        };
        let image = Arc::new(Image::new(png));
        crate::clipboard_push::note_image_from(&from, &image.id);
        match set_clipboard_image(&image).await {
            Ok(()) => tracing::info!("A received image is on the clipboard"),
            Err(e) => {
                crate::clipboard_push::forget_image_from(&image.id);
                tracing::warn!("A received image could not go onto the clipboard: {e}");
            }
        }
    });
}

/// The image the last clipboard reply named, to the phone that asks.
async fn send_image(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(key) = transfer_key(packet) else {
        tracing::warn!("Clipboard image fetch refused: no transfer key");
        return;
    };
    let id = packet.body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let plan = (|| -> std::result::Result<_, String> {
        let image = {
            let cached = cache().lock().unwrap_or_else(|e| e.into_inner());
            image_for_fetch(cached.as_ref(), id)?
        };
        let route = transfer_route(packet, sender)?;
        let slot = share::transfer_slots().take(sender.device_id())?;
        Ok((image, route, slot))
    })();
    let (image, route, slot) = match plan {
        Ok(plan) => plan,
        Err(reason) => {
            tracing::warn!("Clipboard image fetch refused: {reason}");
            share::refuse(sender, &key, &reason).await;
            return;
        }
    };
    let to = sender.device_id().to_string();
    // To another computer: on its way from now, and never sent to it twice
    // in a row (BK3).
    let computer = crate::plugins::pair::is_computer_device(&to);
    if computer {
        crate::clipboard_push::note_image_sent(&to, &image.id);
    }
    tokio::spawn(async move {
        let _slot = slot;
        let size = image.png.len() as u64;
        let mut reader = image.png.as_slice();
        match share::send_reader(route, &key, &mut reader, size).await {
            Ok(_) => tracing::info!("Clipboard image sent to {to} ({size} bytes)"),
            Err(e) => {
                if computer {
                    crate::clipboard_push::forget_image_sent_if(&to, &image.id);
                }
                tracing::warn!("Clipboard image not sent to {to}: {e}")
            }
        }
    });
}

fn other(e: impl std::fmt::Display) -> pocketdesk_core::error::PocketdeskError {
    share::io_error("clipboard", e)
}

/// A file that stands in for the clipboard (`POCKETDESK_CLIPBOARD_FILE`, an
/// absolute path): for tests that run two servers on one machine, which
/// share its real one (2g), and throwaway servers beside the owner's. With
/// it or [`image_stand_in`] set, the stand-ins are the whole clipboard and
/// this computer's own is never read or written (S1). The server warns at
/// start when it is set.
pub(crate) fn stand_in() -> Option<std::path::PathBuf> {
    std::env::var_os("POCKETDESK_CLIPBOARD_FILE")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// A PNG that stands in for the clipboard's image
/// (`POCKETDESK_CLIPBOARD_IMAGE_FILE`, an absolute path; M3): read for the
/// phones, and written with an image from one. For tests, beside
/// [`stand_in`]; the server warns at start when it is set.
pub(crate) fn image_stand_in() -> Option<std::path::PathBuf> {
    std::env::var_os("POCKETDESK_CLIPBOARD_IMAGE_FILE")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// What the clipboard holds when a stand-in is set: the stand-ins are the
/// whole clipboard then, and this computer's own is never read (S1: the
/// text's alone had let a phone's ask read it). `None` with neither set.
fn stand_ins_clip(text: Option<&std::path::Path>, image: Option<&std::path::Path>) -> Option<Clip> {
    if text.is_none() && image.is_none() {
        return None;
    }
    Some(stand_in_clip(text, image))
}

/// What the stand-ins hold: the text stand-in's text first, as a real
/// clipboard's text wins (M3 review), else the image stand-in's PNG within
/// N4's limit, kept for a phone's fetch, or nothing.
fn stand_in_clip(text: Option<&std::path::Path>, image: Option<&std::path::Path>) -> Clip {
    if let Some(text) = text
        .and_then(|text| std::fs::read_to_string(text).ok())
        .filter(|text| !text.is_empty())
    {
        forget_image();
        return Clip::Text(text);
    }
    let Some(path) = image else {
        forget_image();
        return Clip::Empty;
    };
    match std::fs::read(path) {
        Ok(png)
            if png.len() as u64 <= MAX_CLIPBOARD_IMAGE
                && crate::platform::clip_image::is_png(&png) =>
        {
            let image = Arc::new(Image::new(png));
            remember(None, Some(image.clone()));
            Clip::Image(image)
        }
        _ => {
            forget_image();
            Clip::Empty
        }
    }
}

/// The clipboard's image for the watcher to push (M3): its id and size,
/// kept for the phone's fetch until the clipboard changes; nothing when the
/// clipboard holds text, nothing usable, or a copy marked private.
/// An error says the clipboard could not be read now (another app held it),
/// for the watcher to look again.
pub(crate) async fn image_for_push() -> Result<Option<(String, u64)>> {
    Ok(match read_clipboard().await? {
        Clip::Image(image) => Some((image.id.clone(), image.png.len() as u64)),
        _ => None,
    })
}

/// The image `id` as the clipboard was last read, while it still holds it:
/// its PNG, for another computer (BK3).
pub(crate) fn kept_image(id: &str) -> Option<Arc<Image>> {
    let cached = cache().lock().unwrap_or_else(|e| e.into_inner());
    image_for_fetch(cached.as_ref(), id).ok()
}

/// Whether `text` is a pairing link (`pocketdesk://pair?...`), which the
/// Pair tab copies for another computer (2f): it holds the PIN, so it goes
/// to no phone or computer from the clipboard.
pub(crate) fn is_pairing_link(text: &str) -> bool {
    // Anywhere in the text, in any case (D3 review): a message that quotes
    // the link carries its PIN just the same.
    text.to_ascii_lowercase().contains("pocketdesk://pair?")
}

/// The clipboard's text, if it holds any (T10b's watcher): never an image,
/// a copy a password manager marked private, or a pairing link.
/// An error says the clipboard could not be read (another app held it).
pub(crate) async fn read_text() -> std::io::Result<Option<String>> {
    let (text, owner) = match stand_in() {
        Some(path) => (
            std::fs::read_to_string(path).ok().filter(|t| !t.is_empty()),
            None,
        ),
        // The image stand-in alone: the stand-ins are the clipboard, and
        // there is no text (M3 review).
        None if image_stand_in().is_some() => (None, None),
        None => {
            let before = clipboard_sequence();
            let text = read_system_text().await?;
            let owner = clipboard_owner();
            same_copy(before)?;
            (text, owner)
        }
    };
    Ok(text.filter(|t| !is_pairing_link(t) && !held_here(Some(t), owner.as_deref())))
}

/// The clipboard's sequence number, where the system keeps one (Windows).
fn clipboard_sequence() -> Option<u32> {
    #[cfg(windows)]
    return Some(crate::platform::clipboard_windows::sequence());
    #[cfg(not(windows))]
    None
}

/// An error when the clipboard changed since `before` (O4 review): its
/// content and its owner were read a moment apart, so a copy made between
/// them would be judged by another copy's program. The read is tried again.
fn same_copy(before: Option<u32>) -> std::io::Result<()> {
    if clipboard_sequence() == before {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "the clipboard changed while it was read",
        ))
    }
}

/// The program that made the clipboard's copy, when the system says
/// (Windows; O4).
fn clipboard_owner() -> Option<String> {
    #[cfg(windows)]
    return crate::platform::clipboard_windows::owner_program();
    #[cfg(not(windows))]
    None
}

/// The rules of desktop Settings "Never send copies from these programs" and "… that look
/// like" (O4), built again only when the settings change.
/// The settings a filter was built for, and the filter.
type BuiltFilter = (
    (Vec<String>, Vec<String>),
    Arc<pocketdesk_core::local::clip_filter::ClipFilter>,
);

static FILTER: std::sync::Mutex<Option<BuiltFilter>> = std::sync::Mutex::new(None);

/// Whether a copy stays on this computer by its program or its look (O4):
/// its `text` (`None` for an image, which only its program holds back),
/// made by `owner`.
pub(crate) fn held_here(text: Option<&str>, owner: Option<&str>) -> bool {
    let config = crate::events::current_config();
    if config.clipboard_never_from.is_empty() && config.clipboard_never_like.is_empty() {
        return false;
    }
    let key = (config.clipboard_never_from, config.clipboard_never_like);
    let filter = {
        let mut cached = FILTER.lock().unwrap_or_else(|e| e.into_inner());
        match cached.as_ref() {
            Some((made_for, filter)) if *made_for == key => filter.clone(),
            _ => {
                // A pattern a hand edit left unreadable is said, once a change
                // (O4 review: it was dropped without a word).
                for pattern in &key.1 {
                    if let Some(problem) =
                        pocketdesk_core::local::clip_filter::pattern_problem(pattern.trim())
                        && !pattern.trim().is_empty()
                    {
                        tracing::warn!("Settings' \"Never send copies that look like\": {problem}");
                    }
                }
                let filter = Arc::new(pocketdesk_core::local::clip_filter::ClipFilter::new(
                    &key.0, &key.1,
                ));
                *cached = Some((key, filter.clone()));
                filter
            }
        }
    };
    let held = !filter.lets_out(text, owner);
    if held {
        // Said once a copy, never the copy itself (O4): a copy is read again
        // at each look while it stays.
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (text, owner).hash(&mut hasher);
        let seen = hasher.finish();
        static SAID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if SAID.swap(seen, std::sync::atomic::Ordering::Relaxed) != seen {
            tracing::info!("A copy stays on this computer: Settings keep it here");
        }
    }
    held
}

/// [`read_clipboard_unfiltered`], less what Settings keep here (O4).
async fn read_clipboard() -> Result<Clip> {
    let system = stand_in().is_none() && image_stand_in().is_none();
    let before = clipboard_sequence();
    let clip = read_clipboard_unfiltered().await?;
    let owner = if system { clipboard_owner() } else { None };
    if system {
        same_copy(before).map_err(other)?;
    }
    Ok(match &clip {
        Clip::Text(text) if held_here(Some(text), owner.as_deref()) => Clip::Empty,
        Clip::Image(_) if held_here(None, owner.as_deref()) => Clip::Empty,
        _ => clip,
    })
}

/// Set the clipboard's text.
pub(crate) async fn set_clipboard(content: &str) -> Result<()> {
    if let Some(done) =
        set_stand_ins_text(stand_in().as_deref(), image_stand_in().as_deref(), content)
    {
        return done;
    }
    set_system_clipboard(content).await
}

/// Whether a stand-in is set, so the stand-ins are the whole clipboard.
pub(crate) fn stand_ins_set() -> bool {
    stand_in().is_some() || image_stand_in().is_some()
}

/// Put `content` on the stand-ins when either is set (`None` with neither):
/// the text stand-in takes it, owner-only as the real clipboard is, and it
/// replaces the image, as on a real clipboard (M3 review). With the image's
/// alone the text goes nowhere: never onto this computer's own clipboard (S1).
fn set_stand_ins_text(
    text: Option<&std::path::Path>,
    image: Option<&std::path::Path>,
    content: &str,
) -> Option<Result<()>> {
    if text.is_none() && image.is_none() {
        return None;
    }
    if let Some(image) = image {
        let _ = std::fs::remove_file(image);
    }
    forget_image();
    Some(match text {
        Some(path) => pocketdesk_core::secure_fs::write_private(path, content.as_bytes())
            .map_err(|e| share::io_error("clipboard", e)),
        None => Ok(()),
    })
}

/// [`read_text`] from the system's clipboard.
#[cfg(windows)]
async fn read_system_text() -> std::io::Result<Option<String>> {
    use crate::platform::clipboard_windows as win;
    tokio::task::spawn_blocking(|| {
        if !win::has_text() || win::privacy_marks().private() {
            return Ok(None);
        }
        win::get_text().map(Some)
    })
    .await
    .map_err(|e| std::io::Error::other(e.to_string()))?
}

/// The clipboard's text, if it holds any (T10b's watcher): asked for a
/// text type only (`wl-paste`, or X11's `xclip`: T2), so an image is never
/// read, and nothing a password manager marked private.
/// Either fails on an empty clipboard, so a failure reads as no text.
#[cfg(not(windows))]
async fn read_system_text() -> std::io::Result<Option<String>> {
    let tool = ClipTool::current();
    let Ok(types) = paste(tool.list_types()).await else {
        return Ok(None);
    };
    let types = String::from_utf8_lossy(&types);
    if crate::clipboard_push::private_types(&types) {
        return Ok(None);
    }
    let Some(read) = tool.read_text(&types) else {
        return Ok(None);
    };
    let Ok(bytes) = paste(read).await else {
        return Ok(None);
    };
    Ok(String::from_utf8(bytes).ok())
}

/// Set the system clipboard: the Win32 clipboard on Windows.
#[cfg(windows)]
async fn set_system_clipboard(content: &str) -> Result<()> {
    let content = content.to_string();
    tokio::task::spawn_blocking(move || crate::platform::clipboard_windows::set_text(&content))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))??;
    Ok(())
}

/// What the clipboard holds: text when it has any, else an image, read again
/// only when the clipboard's sequence number moved.
#[cfg(windows)]
async fn read_clipboard_unfiltered() -> Result<Clip> {
    use crate::platform::clipboard_windows as win;
    if let Some(clip) = stand_ins_clip(stand_in().as_deref(), image_stand_in().as_deref()) {
        return Ok(clip);
    }
    tokio::task::spawn_blocking(|| -> Result<Clip> {
        // A copy a password manager marked private goes to no phone that
        // asks either, text or image (M3), as the watcher's (T10b).
        if win::privacy_marks().private() {
            forget_image();
            return Ok(Clip::Empty);
        }
        if win::has_text() {
            forget_image();
            let text = win::get_text()?;
            return Ok(if text.is_empty() {
                Clip::Empty
            } else {
                Clip::Text(text)
            });
        }
        let sequence = win::sequence();
        {
            let cached = cache().lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cached.as_ref()
                && cached.sequence == Some(sequence)
            {
                return Ok(cached.image.clone().map_or(Clip::Empty, Clip::Image));
            }
        }
        let image = match win::get_image_png() {
            Ok(Some(png)) if png.len() as u64 <= MAX_CLIPBOARD_IMAGE => {
                Some(Arc::new(Image::new(png)))
            }
            Ok(Some(png)) => {
                tracing::debug!("The clipboard image is too large ({} bytes)", png.len());
                None
            }
            Ok(None) => None,
            // Not remembered (M3 review): another app held the clipboard
            // (Windows' Snipping Tool and clipboard history do, right after
            // a screenshot), so the next look reads it again.
            Err(e) => return Err(e.into()),
        };
        remember(Some(sequence), image.clone());
        Ok(image.map_or(Clip::Empty, Clip::Image))
    })
    .await
    .map_err(other)?
}

/// Put `image` on the Win32 clipboard, and remember it at the sequence
/// number that write made.
/// With a stand-in set, the stand-ins take it instead ([`set_stand_ins_image`]).
#[cfg(windows)]
async fn set_clipboard_image(image: &Arc<Image>) -> Result<()> {
    use crate::platform::clipboard_windows as win;
    if let Some(done) =
        set_stand_ins_image(stand_in().as_deref(), image_stand_in().as_deref(), image)
    {
        return done;
    }
    let image = image.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let sequence = win::set_image_png(&image.png)?;
        remember(Some(sequence), Some(image));
        Ok(())
    })
    .await
    .map_err(other)?
}

/// Put `image` on the stand-ins when either is set (`None` with neither):
/// the image stand-in takes it, owner-only as the clipboard is (M3), and it
/// replaces the text stand-in's text, as on a real clipboard. With the
/// text's alone the text is emptied and the image goes nowhere: never onto
/// this computer's own clipboard (S1).
fn set_stand_ins_image(
    text: Option<&std::path::Path>,
    image_path: Option<&std::path::Path>,
    image: &Arc<Image>,
) -> Option<Result<()>> {
    if text.is_none() && image_path.is_none() {
        return None;
    }
    Some((|| {
        if let Some(path) = image_path {
            pocketdesk_core::secure_fs::write_private(path, &image.png)
                .map_err(|e| share::io_error("clipboard image", e))?;
        }
        if let Some(path) = text {
            pocketdesk_core::secure_fs::write_private(path, b"")
                .map_err(|e| share::io_error("clipboard", e))?;
        }
        if image_path.is_some() {
            remember(None, Some(image.clone()));
        } else {
            forget_image();
        }
        Ok(())
    })())
}

/// Set system clipboard using `wl-copy`, or X11's `xclip` (T2).
#[cfg(not(windows))]
async fn set_system_clipboard(content: &str) -> Result<()> {
    clip_copy(None, content.as_bytes()).await
}

#[cfg(not(windows))]
use pocketdesk_core::local::clip_tool::Tool as ClipTool;

/// Run `wl-copy` or `xclip` (as `mime` when given) on `bytes`.
#[cfg(not(windows))]
async fn clip_copy(mime: Option<&str>, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let (program, args) = ClipTool::current().write(mime);
    let mut command = tokio::process::Command::new(program);
    command.args(&args);
    // wl-copy and xclip fork into the background to keep the clipboard, so
    // they must not inherit our stdio — a daemon holding the parent's stdout
    // pipe makes test harnesses (and log capture) wait on EOF forever.
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Io {
            operation: "spawn the clipboard's copy program",
            detail: format!("{program}: {e}"),
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(bytes).await?;
        stdin.flush().await?;
    }

    // Its exit says whether the copy took (T2 review: an image was
    // remembered as on the clipboard when xclip found no display).
    let status = child.wait().await?;
    if !status.success() {
        return Err(pocketdesk_core::error::PocketdeskError::Io {
            operation: "set the clipboard",
            detail: format!("{program} exited with {status}"),
        });
    }
    Ok(())
}

/// How long a read of the clipboard is given: `xclip` waits on the copy's
/// program, which may hang (T2 review, as P7's for copied files).
#[cfg(not(windows))]
const PASTE_TIME: std::time::Duration = std::time::Duration::from_secs(3);

/// `wl-paste` or `xclip` as [`ClipTool`] says, its output on success.
#[cfg(not(windows))]
async fn paste((program, args): pocketdesk_core::local::clip_tool::Command) -> Result<Vec<u8>> {
    let read = tokio::process::Command::new(program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(PASTE_TIME, read).await.map_err(|_| {
        pocketdesk_core::error::PocketdeskError::Io {
            operation: "read the clipboard",
            detail: format!("{program} did not answer within {PASTE_TIME:?}"),
        }
    })??;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(pocketdesk_core::error::PocketdeskError::Io {
            operation: "read the clipboard",
            detail: format!("{program}: {}", String::from_utf8_lossy(&output.stderr)),
        })
    }
}

/// What the clipboard holds: text when it offers any, else a PNG.
#[cfg(not(windows))]
async fn read_clipboard_unfiltered() -> Result<Clip> {
    if let Some(clip) = stand_ins_clip(stand_in().as_deref(), image_stand_in().as_deref()) {
        return Ok(clip);
    }
    let tool = ClipTool::current();
    let Ok(types) = paste(tool.list_types()).await else {
        // An empty clipboard makes wl-paste (and xclip) fail.
        return Ok(Clip::Empty);
    };
    let types = String::from_utf8_lossy(&types);
    // A copy a password manager marked private goes to no phone that asks
    // either, text or image (M3), as the watcher's (T10b).
    if crate::clipboard_push::private_types(&types) {
        forget_image();
        return Ok(Clip::Empty);
    }
    if let Some(read) = tool.read_text(&types) {
        forget_image();
        let text = String::from_utf8_lossy(&paste(read).await?).to_string();
        return Ok(Clip::Text(text));
    }
    if !types.lines().any(|t| t.trim().starts_with("image/png")) {
        return Ok(Clip::Empty);
    }
    let png = paste(tool.read("image/png")).await?;
    if png.len() as u64 > MAX_CLIPBOARD_IMAGE || !crate::platform::clip_image::is_png(&png) {
        return Ok(Clip::Empty);
    }
    let image = Arc::new(Image::new(png));
    remember(None, Some(image.clone()));
    Ok(Clip::Image(image))
}

/// Put `image` on the clipboard as `image/png` (`wl-copy`, or `xclip`).
/// With a stand-in set, the stand-ins take it instead ([`set_stand_ins_image`]).
#[cfg(not(windows))]
async fn set_clipboard_image(image: &Arc<Image>) -> Result<()> {
    if let Some(done) =
        set_stand_ins_image(stand_in().as_deref(), image_stand_in().as_deref(), image)
    {
        return done;
    }
    clip_copy(Some("image/png"), &image.png).await?;
    remember(None, Some(image.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The image cache is one per process; the tests that read it take turns.
    static CACHE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_pairing_link_is_told_from_other_text() {
        assert!(is_pairing_link("pocketdesk://pair?v=1&id=b&pin=123456"));
        assert!(is_pairing_link("  pocketdesk://pair?v=1"));
        assert!(
            is_pairing_link("see pocketdesk://pair?v=1"),
            "quoted, its PIN goes with it"
        );
        assert!(is_pairing_link("PocketDesk://PAIR?v=1&pin=123456"));
        assert!(!is_pairing_link("https://example.org/"));
    }

    #[test]
    fn an_image_is_known_by_the_sha256_of_its_png() {
        let image = Image::new(b"abc".to_vec());
        assert_eq!(
            image.id,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_reply_carries_text_or_the_image_id_never_both() {
        let text = reply_body(&Clip::Text("hello".into()));
        assert_eq!(text["content"], "hello");
        assert!(text.get("image").is_none());
        let image = Arc::new(Image::new(vec![1, 2, 3]));
        let body = reply_body(&Clip::Image(image.clone()));
        assert_eq!(body["image"]["id"], image.id.as_str());
        assert_eq!(body["image"]["size"], 3);
        assert!(body.get("content").is_none(), "an older phone sees no text");
        assert_eq!(reply_body(&Clip::Empty)["content"], "");
    }

    #[test]
    fn a_fetch_takes_only_the_image_the_clipboard_still_holds() {
        let image = Arc::new(Image::new(vec![7; 10]));
        let cached = Cached {
            sequence: Some(4),
            image: Some(image.clone()),
        };
        assert_eq!(image_for_fetch(Some(&cached), &image.id).unwrap(), image);
        assert!(image_for_fetch(Some(&cached), "0000").is_err(), "stale id");
        assert!(image_for_fetch(None, &image.id).is_err(), "nothing cached");
        let unusable = Cached {
            sequence: Some(5),
            image: None,
        };
        assert!(
            image_for_fetch(Some(&unusable), &image.id).is_err(),
            "the clipboard moved on to something that is not an image"
        );
    }

    /// M3: the image stand-in is read as the clipboard's image and kept for
    /// a fetch; anything else in it is no image.
    #[test]
    fn the_image_stand_in_is_the_clipboards_image() {
        let _turn = CACHE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("pd-clip-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.png");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(b"rest of a picture");
        std::fs::write(&path, &png).unwrap();
        let Clip::Image(image) = stand_in_clip(None, Some(&path)) else {
            panic!("a PNG is the clipboard's image");
        };
        assert_eq!(image.id, Image::new(png.clone()).id);
        let kept = cache().lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            image_for_fetch(kept.as_ref(), &image.id).is_ok(),
            "kept for a fetch"
        );
        drop(kept);
        std::fs::write(&path, b"not a picture").unwrap();
        assert_eq!(stand_in_clip(None, Some(&path)), Clip::Empty);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// S1: with either stand-in set, the stand-ins are the whole clipboard.
    /// A throwaway server with the text's alone had answered a phone's ask
    /// with this computer's real clipboard, and put a phone's image on it.
    #[test]
    fn either_stand_in_keeps_the_system_clipboard_out() {
        let _turn = CACHE_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("pd-clip-stand-ins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("clip.txt");
        let image = dir.join("clip.png");
        assert_eq!(stand_ins_clip(None, None), None, "neither: the system's");

        std::fs::write(&text, "from the stand-in").unwrap();
        assert_eq!(
            stand_ins_clip(Some(&text), None),
            Some(Clip::Text("from the stand-in".into())),
            "the text's alone is read"
        );
        std::fs::write(&text, "").unwrap();
        assert_eq!(
            stand_ins_clip(Some(&text), None),
            Some(Clip::Empty),
            "an empty text stand-in is an empty clipboard, not the system's"
        );

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(b"a picture");
        let picture = Arc::new(Image::new(png.clone()));
        std::fs::write(&text, "older text").unwrap();
        assert!(
            set_stand_ins_image(Some(&text), Some(&image), &picture)
                .expect("a stand-in is set")
                .is_ok()
        );
        assert_eq!(std::fs::read(&image).unwrap(), png);
        assert_eq!(
            stand_ins_clip(Some(&text), Some(&image)),
            Some(Clip::Image(picture.clone())),
            "an image replaces the text, as on a real clipboard"
        );

        std::fs::write(&text, "text again").unwrap();
        assert!(
            set_stand_ins_image(Some(&text), None, &picture)
                .expect("a stand-in is set")
                .is_ok(),
            "the text's alone takes an image without this computer's clipboard"
        );
        assert_eq!(stand_ins_clip(Some(&text), None), Some(Clip::Empty));
        assert!(set_stand_ins_image(None, None, &picture).is_none());

        // Text: onto the text's stand-in, replacing the image; with the
        // image's alone, nowhere; with neither, the system's.
        std::fs::write(&image, &png).unwrap();
        assert!(
            set_stand_ins_text(Some(&text), Some(&image), "new text")
                .expect("a stand-in is set")
                .is_ok()
        );
        assert!(!image.exists(), "text replaces the image");
        assert_eq!(
            stand_ins_clip(Some(&text), Some(&image)),
            Some(Clip::Text("new text".into()))
        );
        std::fs::write(&image, &png).unwrap();
        assert!(
            set_stand_ins_text(None, Some(&image), "goes nowhere")
                .expect("a stand-in is set")
                .is_ok(),
            "the image's alone takes text without this computer's clipboard"
        );
        assert_eq!(stand_ins_clip(None, Some(&image)), Some(Clip::Empty));
        assert!(set_stand_ins_text(None, None, "x").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
