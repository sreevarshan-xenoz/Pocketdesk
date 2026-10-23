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
struct Image {
    id: String,
    png: Vec<u8>,
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
        // set, only while this one shares it with its computers (2g).
        if !crate::clipboard_push::with_computers()
            && crate::plugins::pair::is_computer_device(sender.device_id())
        {
            tracing::debug!(
                "Another computer's {}, with the clipboard not shared here; ignored",
                packet.packet_type
            );
            return Ok(());
        }
        match packet.packet_type.as_str() {
            "kdeconnect.clipboard" => {
                // Remote clipboard update - set local clipboard
                if let Some(content) = packet.body.get("content").and_then(|v| v.as_str()) {
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
    tokio::spawn(async move {
        let _slot = slot;
        let kept = share::receive_bytes(route, &key, size, MAX_CLIPBOARD_IMAGE, |png| async {
            if !crate::platform::clip_image::is_png(&png) {
                return Err(share::io_error("clipboard", "not a PNG"));
            }
            let image = Arc::new(Image::new(png));
            set_clipboard_image(&image).await?;
            Ok(())
        })
        .await;
        match kept {
            Ok(()) => tracing::info!("Clipboard image received ({size} bytes)"),
            Err(e) => tracing::warn!("Clipboard image not taken: {e}"),
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
    tokio::spawn(async move {
        let _slot = slot;
        let size = image.png.len() as u64;
        let mut reader = image.png.as_slice();
        match share::send_reader(route, &key, &mut reader, size).await {
            Ok(_) => tracing::info!("Clipboard image sent ({size} bytes)"),
            Err(e) => tracing::warn!("Clipboard image not sent: {e}"),
        }
    });
}

fn other(e: impl std::fmt::Display) -> pocketdesk_core::error::PocketdeskError {
    share::io_error("clipboard", e)
}

/// A file that stands in for the clipboard (`POCKETDESK_CLIPBOARD_FILE`, an
/// absolute path): for tests that run two servers on one machine, which
/// share its real one (2g). The server warns at start when it is set.
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

/// What the stand-ins hold: the text stand-in's text first, as a real
/// clipboard's text wins (M3 review), else the image stand-in's PNG within
/// N4's limit, kept for a phone's fetch, or nothing.
fn stand_in_clip(path: &std::path::Path) -> Clip {
    if let Some(text) = stand_in()
        .and_then(|text| std::fs::read_to_string(text).ok())
        .filter(|text| !text.is_empty())
    {
        forget_image();
        return Clip::Text(text);
    }
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

/// The rules of desktop Settings "Never send copies from" and "… that look
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
    // Text replaces an image, as on a real clipboard (M3 review).
    if let Some(image) = image_stand_in() {
        let _ = std::fs::remove_file(image);
        forget_image();
    }
    if let Some(path) = stand_in() {
        // Clipboard text: owner-only, as the real one is.
        return pocketdesk_core::secure_fs::write_private(&path, content.as_bytes())
            .map_err(|e| share::io_error("clipboard", e));
    }
    set_system_clipboard(content).await
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

/// The clipboard's text, if it holds any (T10b's watcher): `wl-paste`
/// asked for a text type only, so an image is never read, and nothing a
/// password manager marked private.
/// `wl-paste` fails on an empty clipboard, so a failure reads as no text.
#[cfg(not(windows))]
async fn read_system_text() -> std::io::Result<Option<String>> {
    let Ok(types) = wl_paste(&["--list-types"]).await else {
        return Ok(None);
    };
    if crate::clipboard_push::private_types(&String::from_utf8_lossy(&types)) {
        return Ok(None);
    }
    let Ok(bytes) = wl_paste(&["--no-newline", "--type", "text"]).await else {
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
    if let Some(path) = image_stand_in() {
        return Ok(stand_in_clip(&path));
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
#[cfg(windows)]
async fn set_clipboard_image(image: &Arc<Image>) -> Result<()> {
    use crate::platform::clipboard_windows as win;
    if let Some(path) = image_stand_in() {
        return set_stand_in_image(&path, image);
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

/// Write `image` to the image stand-in (M3), owner-only as the clipboard is.
fn set_stand_in_image(path: &std::path::Path, image: &Arc<Image>) -> Result<()> {
    pocketdesk_core::secure_fs::write_private(path, &image.png)
        .map_err(|e| share::io_error("clipboard image", e))?;
    remember(None, Some(image.clone()));
    Ok(())
}

/// Set system clipboard using `wl-copy`.
#[cfg(not(windows))]
async fn set_system_clipboard(content: &str) -> Result<()> {
    wl_copy(None, content.as_bytes()).await
}

/// Run `wl-copy` (with `--type mime` when given) on `bytes`.
#[cfg(not(windows))]
async fn wl_copy(mime: Option<&str>, bytes: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new("wl-copy");
    if let Some(mime) = mime {
        command.args(["--type", mime]);
    }
    // wl-copy forks into the background to retain clipboard ownership, so it
    // must not inherit our stdio — a daemon holding the parent's stdout pipe
    // makes test harnesses (and log capture) wait on EOF forever.
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Io {
            operation: "spawn wl-copy",
            detail: e.to_string(),
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(bytes).await?;
        stdin.flush().await?;
    }

    child.wait().await?;
    Ok(())
}

/// `wl-paste` with `args`, its output on success.
#[cfg(not(windows))]
async fn wl_paste(args: &[&str]) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("wl-paste")
        .args(args)
        .output()
        .await?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(pocketdesk_core::error::PocketdeskError::Io {
            operation: "wl-paste",
            detail: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// What the clipboard holds: text when it offers any, else a PNG.
#[cfg(not(windows))]
async fn read_clipboard_unfiltered() -> Result<Clip> {
    if let Some(path) = image_stand_in() {
        return Ok(stand_in_clip(&path));
    }
    let Ok(types) = wl_paste(&["--list-types"]).await else {
        // An empty clipboard makes wl-paste fail.
        return Ok(Clip::Empty);
    };
    let types = String::from_utf8_lossy(&types);
    // A copy a password manager marked private goes to no phone that asks
    // either, text or image (M3), as the watcher's (T10b).
    if crate::clipboard_push::private_types(&types) {
        forget_image();
        return Ok(Clip::Empty);
    }
    let has = |prefix: &str| types.lines().any(|t| t.trim().starts_with(prefix));
    if has("text/plain") || has("UTF8_STRING") || has("TEXT") || has("STRING") {
        forget_image();
        let text = String::from_utf8_lossy(&wl_paste(&["--no-newline"]).await?).to_string();
        return Ok(Clip::Text(text));
    }
    if !has("image/png") {
        return Ok(Clip::Empty);
    }
    let png = wl_paste(&["--type", "image/png"]).await?;
    if png.len() as u64 > MAX_CLIPBOARD_IMAGE || !crate::platform::clip_image::is_png(&png) {
        return Ok(Clip::Empty);
    }
    let image = Arc::new(Image::new(png));
    remember(None, Some(image.clone()));
    Ok(Clip::Image(image))
}

/// Put `image` on the clipboard with `wl-copy --type image/png`.
#[cfg(not(windows))]
async fn set_clipboard_image(image: &Arc<Image>) -> Result<()> {
    if let Some(path) = image_stand_in() {
        return set_stand_in_image(&path, image);
    }
    wl_copy(Some("image/png"), &image.png).await?;
    remember(None, Some(image.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join(format!("pd-clip-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.png");
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(b"rest of a picture");
        std::fs::write(&path, &png).unwrap();
        let Clip::Image(image) = stand_in_clip(&path) else {
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
        assert_eq!(stand_in_clip(&path), Clip::Empty);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
