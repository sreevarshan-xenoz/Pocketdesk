//! A glance at this screen with no session (BF2). A phone with the Peek
//! permission asks `peek` with a transfer key, as it fetches a clipboard
//! image; this computer takes one still of its main screen, shrinks it to
//! [`LONGEST_SIDE`], makes it a JPEG and sends it to that key. It is refused
//! with a reason the phone can say: `off` (this computer's Settings),
//! `paused` (K2a), `busy` (another still being made), `too_soon` (this
//! phone asked under [`MIN_GAP`] ago), `no_screen` (no still could be
//! taken, or not within [`CAPTURE_DEADLINE`]). A phone without the
//! permission is answered `not_allowed`.
//!
//! The still is made in memory and never written here, but on GNOME and KDE
//! under Wayland the desktop's own screenshot service hands it over as a
//! temporary file, which is deleted at once.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::peek::{LONGEST_SIDE, MAX_PEEK, PEEK, fit};

use crate::plugins::share;

/// A phone's stills, at most one in this long.
const MIN_GAP: Duration = Duration::from_secs(2);
/// The JPEG's quality: text on a phone stays readable.
const QUALITY: u8 = 70;
/// How long a still may take. The read of the phone's connection waits on
/// it, and a desktop's screenshot portal can ask someone at the screen and
/// wait forever (BF2 review).
const CAPTURE_DEADLINE: Duration = Duration::from_secs(8);

/// When each phone last had a still made.
static LAST: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// A still that stands in for the screen (`POCKETDESK_PEEK_FILE`, an
/// absolute path to a PNG or JPEG): for throwaway servers, so a test never
/// sends the owner's screen anywhere. The server warns at start when it is
/// set. Set but not absolute, it still stands in, as a path that fails
/// (BF2 review): never the screen instead.
fn stand_in() -> Option<std::path::PathBuf> {
    std::env::var_os("POCKETDESK_PEEK_FILE").map(std::path::PathBuf::from)
}

/// One still of the main screen (or the stand-in), as RGBA.
fn still() -> std::result::Result<image::RgbaImage, String> {
    still_from(stand_in())
}

/// One still of `stand_in`, or of the main screen when there is none.
fn still_from(
    stand_in: Option<std::path::PathBuf>,
) -> std::result::Result<image::RgbaImage, String> {
    if let Some(path) = stand_in {
        if !path.is_absolute() {
            return Err("the stand-in is not an absolute path".to_string());
        }
        return image::open(&path)
            .map(|i| i.to_rgba8())
            .map_err(|e| format!("the stand-in did not read: {e}"));
    }
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or_else(|| monitors.first())
        .ok_or("no screen")?;
    monitor.capture_image().map_err(|e| e.to_string())
}

/// `still` as the phone gets it: shrunk to [`LONGEST_SIDE`], a JPEG.
pub(crate) fn jpeg(still: &image::RgbaImage) -> std::result::Result<Vec<u8>, String> {
    let (width, height) = fit(still.width(), still.height(), LONGEST_SIDE);
    let shrunk =
        image::imageops::resize(still, width, height, image::imageops::FilterType::Triangle);
    let rgb = image::DynamicImage::ImageRgba8(shrunk).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    if out.len() as u64 > MAX_PEEK {
        return Err("too large".to_string());
    }
    Ok(out)
}

/// Why no phone may have a still now, if none may: this computer's
/// Settings, or a pause.
fn held(enabled: bool, paused: bool) -> Option<&'static str> {
    if !enabled {
        Some("off")
    } else if paused {
        Some("paused")
    } else {
        None
    }
}

/// Whether `device` may have a still made at `now`, its last under
/// [`MIN_GAP`] ago; counts it when it may. Asked only once a still is about
/// to be taken, so an ask refused earlier costs the phone no wait (BF2
/// review).
fn past_gap(device: &str, now: Instant) -> bool {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    last.retain(|_, at| now.duration_since(*at) < MIN_GAP);
    if last.contains_key(device) {
        return false;
    }
    last.insert(device.to_string(), now);
    true
}

fn transfer_key(packet: &NetworkPacket) -> Option<String> {
    packet
        .body
        .get("key")
        .and_then(|v| v.as_str())
        .filter(|k| share::is_transfer_key(k))
        .map(str::to_string)
}

/// A phone without the Peek permission hears so, rather than waiting.
pub async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type == PEEK
        && let Some(key) = transfer_key(packet)
    {
        share::refuse(sender, &key, "not_allowed").await;
    }
}

async fn send_still(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(key) = transfer_key(packet) else {
        tracing::warn!("Peek refused: no transfer key");
        return;
    };
    let config = crate::events::current_config();
    if let Some(reason) = held(config.peek, crate::paused::now()) {
        tracing::info!("Peek refused: {reason}");
        share::refuse(sender, &key, reason).await;
        return;
    }
    let port = packet
        .body
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0);
    let plan = (|| -> std::result::Result<_, String> {
        let route = share::route(share::reach(sender, port)?, sender)?;
        let slot = share::transfer_slots().take(sender.device_id())?;
        Ok((route, slot))
    })();
    let (route, slot) = match plan {
        Ok(plan) => plan,
        Err(reason) => {
            tracing::warn!("Peek refused: {reason}");
            share::refuse(sender, &key, &reason).await;
            return;
        }
    };
    // One still at a time: a 4K screen is tens of megabytes before it
    // shrinks. The turn goes with the capture's thread, so one that outlives
    // its deadline keeps the others `busy` until it ends, never two at once.
    static ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let Ok(turn) = ONE_AT_A_TIME.try_acquire() else {
        share::refuse(sender, &key, "busy").await;
        return;
    };
    if !past_gap(sender.device_id(), Instant::now()) {
        share::refuse(sender, &key, "too_soon").await;
        return;
    }
    let made = tokio::time::timeout(
        CAPTURE_DEADLINE,
        tokio::task::spawn_blocking(move || {
            let _turn = turn;
            still().and_then(|s| jpeg(&s))
        }),
    )
    .await;
    let bytes = match made {
        Ok(Ok(Ok(bytes))) => bytes,
        Ok(Ok(Err(e))) => {
            tracing::warn!("Peek: no still of the screen: {e}");
            share::refuse(sender, &key, "no_screen").await;
            return;
        }
        Ok(Err(_)) => {
            share::refuse(sender, &key, "no_screen").await;
            return;
        }
        Err(_) => {
            tracing::warn!("Peek: no still within {CAPTURE_DEADLINE:?}");
            share::refuse(sender, &key, "no_screen").await;
            return;
        }
    };
    tokio::spawn(async move {
        let _slot = slot;
        let size = bytes.len() as u64;
        let mut reader = bytes.as_slice();
        match share::send_reader(route, &key, &mut reader, size).await {
            Ok(_) => tracing::info!("Peek sent ({size} bytes)"),
            Err(e) => tracing::warn!("Peek not sent: {e}"),
        }
    });
}

/// The plugin answering a phone's peek.
pub struct PeekPlugin;

#[async_trait::async_trait]
impl Plugin for PeekPlugin {
    fn name(&self) -> &'static str {
        "peek"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PEEK]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[share::FETCH_REFUSED]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == PEEK {
            send_still(packet, sender).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_screen_comes_as_a_small_jpeg_its_shape_kept() {
        let screen = image::RgbaImage::from_pixel(3200, 1800, image::Rgba([30, 120, 200, 255]));
        let bytes = jpeg(&screen).unwrap();
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "a JPEG");
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (1600, 900));
        assert!(bytes.len() < 200_000, "{} bytes", bytes.len());
    }

    #[test]
    fn settings_off_and_a_pause_hold_every_phone() {
        assert_eq!(held(false, false), Some("off"));
        assert_eq!(held(false, true), Some("off"));
        assert_eq!(held(true, true), Some("paused"));
        assert_eq!(held(true, false), None);
    }

    #[test]
    fn a_phone_asking_again_at_once_is_told_too_soon_others_are_not() {
        let now = Instant::now();
        assert!(past_gap("phone-a", now));
        assert!(!past_gap("phone-a", now + Duration::from_millis(500)));
        assert!(past_gap("phone-b", now), "its own gap");
        assert!(past_gap("phone-a", now + MIN_GAP));
    }

    /// BF2 review: a stand-in path that is not absolute fails, rather than
    /// letting the screen be taken instead.
    #[test]
    fn a_stand_in_that_is_not_absolute_never_falls_back_to_the_screen() {
        let taken = still_from(Some("peek.png".into()));
        assert_eq!(
            taken.err().as_deref(),
            Some("the stand-in is not an absolute path")
        );
    }

    struct Phone(Mutex<Vec<NetworkPacket>>);

    impl Phone {
        fn sent(&self) -> Vec<NetworkPacket> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-1"
        }
        fn connection_id(&self) -> &str {
            "phone-1"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_phone_without_the_permission_hears_not_allowed() {
        let sender = Phone(Default::default());
        let key = "0123456789abcdef0123456789abcdef";
        let packet = NetworkPacket::new(PEEK).with_body(serde_json::json!({ "key": key }));
        answer_denied(&packet, &sender).await;
        let sent = sender.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].body["key"], key);
        assert_eq!(sent[0].body["reason"], "not_allowed");
        // Another packet denied is not answered here.
        answer_denied(&NetworkPacket::new("kdeconnect.pocketdesk.exec"), &sender).await;
        assert_eq!(sender.sent().len(), 1);
    }
}
