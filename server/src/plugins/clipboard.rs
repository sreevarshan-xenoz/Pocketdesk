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
                let clip = read_clipboard().await.unwrap_or_else(|e| {
                    tracing::debug!("Could not read the clipboard: {e}");
                    Clip::Empty
                });
                let response =
                    NetworkPacket::new("kdeconnect.clipboard").with_body(reply_body(&clip));
                sender.send_packet(&response).await?;
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

#[cfg(windows)]
fn other(e: impl std::fmt::Display) -> pocketdesk_core::error::PocketdeskError {
    share::io_error("clipboard", e)
}

/// The clipboard's text, if it holds any (T10b's watcher): never an image,
/// and never a copy a password manager marked private.
/// An error says the clipboard could not be read (another app held it).
#[cfg(windows)]
pub(crate) async fn read_text() -> std::io::Result<Option<String>> {
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
pub(crate) async fn read_text() -> std::io::Result<Option<String>> {
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
async fn set_clipboard(content: &str) -> Result<()> {
    let content = content.to_string();
    tokio::task::spawn_blocking(move || crate::platform::clipboard_windows::set_text(&content))
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))??;
    Ok(())
}

/// What the clipboard holds: text when it has any, else an image, read again
/// only when the clipboard's sequence number moved.
#[cfg(windows)]
async fn read_clipboard() -> Result<Clip> {
    use crate::platform::clipboard_windows as win;
    tokio::task::spawn_blocking(|| -> Result<Clip> {
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
            Err(e) => {
                tracing::debug!("The clipboard image is not read: {e}");
                None
            }
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
    let image = image.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let sequence = win::set_image_png(&image.png)?;
        remember(Some(sequence), Some(image));
        Ok(())
    })
    .await
    .map_err(other)?
}

/// Set system clipboard using `wl-copy`.
#[cfg(not(windows))]
async fn set_clipboard(content: &str) -> Result<()> {
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
async fn read_clipboard() -> Result<Clip> {
    let Ok(types) = wl_paste(&["--list-types"]).await else {
        // An empty clipboard makes wl-paste fail.
        return Ok(Clip::Empty);
    };
    let types = String::from_utf8_lossy(&types);
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
    wl_copy(Some("image/png"), &image.png).await?;
    remember(None, Some(image.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
