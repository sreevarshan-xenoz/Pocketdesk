//! The phones' media (roadmap T8d): a phone in a session reports what it
//! plays (`kdeconnect.pocketdesk.phone_media`, `{}` when nothing does); the
//! server keeps the newest report per paired phone in memory and writes
//! them to `<state>/phone_media.json` at most every [`WRITE_EVERY`]. The
//! desktop app's presses (`<state>/phone_media_out/`) go to their phone as
//! `kdeconnect.pocketdesk.phone_media_control {command}`. The Media
//! permission gates both: the dispatch gate the reports, [`run_watcher`]
//! the presses. A report's cover (N2) comes once as `art` (base64) beside
//! its `artId`, and is kept in `<state>/phone_media_art/` while the phone
//! shows it ([`cover`]).

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::phone_media::{MediaPress, PhoneMediaState};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::phone_media::{MediaReport, PHONE_MEDIA, PHONE_MEDIA_CONTROL};

/// How often the reports are written, at most.
pub const WRITE_EVERY: Duration = Duration::from_secs(2);
/// The most phones whose reports are kept.
const MAX_PHONES: usize = 64;

/// The reports kept in memory, and whether the file is behind them.
#[derive(Default)]
struct Reports {
    phones: BTreeMap<String, PhoneMediaState>,
    dirty: bool,
}

impl Reports {
    /// Keep `report` from `device` (a paired phone), at `now_unix`, with
    /// the cover kept for it (`art`).
    fn take(&mut self, device: &str, report: MediaReport, now_unix: u64, art: Option<String>) {
        match report {
            MediaReport::Nothing => {
                if self.phones.remove(device).is_some() {
                    self.dirty = true;
                }
            }
            MediaReport::Playing(media) => {
                if !self.phones.contains_key(device) && self.phones.len() >= MAX_PHONES {
                    // Full: the oldest report makes room.
                    let oldest = self
                        .phones
                        .iter()
                        .min_by_key(|(_, s)| s.updated_unix)
                        .map(|(id, _)| id.clone());
                    if let Some(oldest) = oldest {
                        self.phones.remove(&oldest);
                    }
                }
                self.phones.insert(
                    device.to_string(),
                    PhoneMediaState {
                        media,
                        updated_unix: now_unix,
                        art,
                    },
                );
                self.dirty = true;
            }
        }
    }

    /// Forget every phone not in `shown`; the phones forgotten.
    fn keep_only(&mut self, shown: &std::collections::HashSet<String>) -> Vec<String> {
        let gone: Vec<String> = self
            .phones
            .keys()
            .filter(|id| !shown.contains(*id))
            .cloned()
            .collect();
        for id in &gone {
            self.phones.remove(id);
        }
        if !gone.is_empty() {
            self.dirty = true;
        }
        gone
    }
}

/// The cover to show with `device`'s report (N2): the bytes sent with it
/// (`art`, base64) kept as its `artId`, else the one kept by that id, else
/// none, the phone's others forgotten when it `had` one (N2 review: so a
/// phone that never sends covers costs no look at the folder). A cover
/// that is not one is logged and not shown.
fn cover(
    state_dir: &std::path::Path,
    device: &str,
    report: &MediaReport,
    art: Option<&str>,
    had: bool,
) -> Option<String> {
    use base64::Engine;
    use pocketdesk_core::local::phone_media::{art_name, forget_art, has_art, keep_art};
    let id = match report {
        MediaReport::Playing(media) => media.art_id.as_deref(),
        MediaReport::Nothing => None,
    };
    let Some(id) = id else {
        if had {
            forget_art(state_dir, device);
        }
        return None;
    };
    if let Some(text) = art {
        let kept = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            .and_then(|bytes| keep_art(state_dir, device, id, &bytes));
        match kept {
            Ok(name) => return Some(name),
            Err(e) => tracing::debug!("A cover from {device} was not kept: {e}"),
        }
    }
    if has_art(state_dir, device, id) {
        Some(art_name(device, id))
    } else {
        if had {
            forget_art(state_dir, device);
        }
        None
    }
}

static REPORTS: LazyLock<Mutex<Reports>> = LazyLock::new(Default::default);

#[derive(Debug, Default)]
pub struct PhoneMediaPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneMediaPlugin {
    fn name(&self) -> &'static str {
        "phone_media"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PHONE_MEDIA]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[PHONE_MEDIA_CONTROL]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != PHONE_MEDIA {
            return Ok(());
        }
        // Only a paired phone's: with `pairing_required = false` anyone
        // could otherwise fill the reports under made-up ids.
        if !crate::plugins::pair::is_paired_device(sender.device_id()) {
            return Ok(());
        }
        let Some(report) = MediaReport::from_body(&packet.body) else {
            tracing::debug!("A media report from {} was not read", sender.device_id());
            return Ok(());
        };
        // A stat for a cover's id, a cover's write when its bytes come, and
        // a look at the folder only when the phone's last report had one:
        // small owner-only files, on the handler as the icons' are (K3).
        let had = REPORTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .phones
            .get(sender.device_id())
            .is_some_and(|s| s.art.is_some());
        let art = crate::state::state_dir().ok().and_then(|dir| {
            cover(
                &dir,
                sender.device_id(),
                &report,
                packet.body.get("art").and_then(|v| v.as_str()),
                had,
            )
        });
        REPORTS.lock().unwrap_or_else(|e| e.into_inner()).take(
            sender.device_id(),
            report,
            pocketdesk_core::local::pin::now_unix(),
            art,
        );
        // Written out now, not at the inbox's next sweep (R6c).
        crate::inbox::nudge(pocketdesk_core::local::paths::PHONE_MEDIA_DIR);
        Ok(())
    }
}

/// Daemon task: keep `<state>/phone_media.json` up to date with the
/// reports, at most every [`WRITE_EVERY`], and send the desktop app's
/// presses to their phones. The file starts empty: what played in an
/// earlier run is not playing now.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: the phones' media is not shown");
        return;
    };
    REPORTS.lock().unwrap_or_else(|e| e.into_inner()).dirty = true;
    // Nor are an earlier run's covers.
    let _ = std::fs::remove_dir_all(dir.join(pocketdesk_core::local::paths::PHONE_MEDIA_ART_DIR));
    let mut last_write = tokio::time::Instant::now() - WRITE_EVERY;
    loop {
        // Wait for a press, or, with a report waiting, for the throttle to
        // allow it; a press is sent at once either way.
        let dirty = REPORTS.lock().unwrap_or_else(|e| e.into_inner()).dirty;
        let throttled = WRITE_EVERY.saturating_sub(last_write.elapsed());
        if !dirty || !throttled.is_zero() {
            crate::inbox::wait_at_most(
                pocketdesk_core::local::paths::PHONE_MEDIA_DIR,
                if dirty {
                    throttled
                } else {
                    crate::inbox::SWEEP
                },
                Duration::from_millis(250),
            )
            .await;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        for press in pocketdesk_core::local::phone_media::take_presses(&dir, now) {
            let clients = crate::state::clone_clients().await;
            // Paired too: the permissions of an id that is not paired are
            // the defaults, and a phone just unpaired may still be connected.
            let allowed = crate::plugins::pair::is_paired_device(&press.device_id)
                && crate::plugins::pair::permits(&press.device_id, PHONE_MEDIA);
            if let Err(reason) = push(&press, &clients, allowed).await {
                tracing::info!("A media press for {} not sent: {reason}", press.device_id);
            }
        }
        // A report waiting is written once the throttle allows (the wait
        // above ends then), not at the next sweep.
        if last_write.elapsed() < WRITE_EVERY {
            continue;
        }
        last_write = tokio::time::Instant::now();
        let connected: Vec<String> = crate::state::clone_clients()
            .await
            .iter()
            .map(|c| c.device_id().to_string())
            .collect();
        // An unreadable store forgets no one.
        let shown = TrustStore::read(dir.join(TRUST_FILE))
            .ok()
            .map(|store| showable(&connected, &store));
        let (phones, gone) = {
            let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
            let gone = shown
                .as_ref()
                .map(|shown| reports.keep_only(shown))
                .unwrap_or_default();
            if !reports.dirty {
                continue;
            }
            reports.dirty = false;
            (reports.phones.clone(), gone)
        };
        for device in gone {
            pocketdesk_core::local::phone_media::forget_art(&dir, &device);
        }
        if let Err(e) = pocketdesk_core::local::phone_media::write(&dir, &phones) {
            tracing::warn!("Could not keep the phones' media: {e}");
        }
    }
}

/// The phones whose media may be shown: connected now, paired, and allowed
/// Media. A report from any other is forgotten, so a phone whose session
/// ended, or whose Media was turned off, leaves no stale song behind.
fn showable(connected: &[String], store: &TrustStore) -> std::collections::HashSet<String> {
    connected
        .iter()
        .filter(|id| {
            store.is_trusted(id)
                && store
                    .permissions(id)
                    .allows(pocketdesk_core::protocol::permissions::Capability::Media)
        })
        .cloned()
        .collect()
}

/// Send one press to its phone.
async fn push(
    press: &MediaPress,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
) -> std::result::Result<(), String> {
    if !allowed {
        return Err("Media is off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == press.device_id)
        .ok_or("the phone is not connected")?;
    let packet = NetworkPacket::new(PHONE_MEDIA_CONTROL)
        .with_body(serde_json::json!({ "command": press.command }));
    phone
        .send_packet(&packet)
        .await
        .map_err(|e| format!("could not send: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_media::PhoneMedia;

    fn song(title: &str) -> MediaReport {
        MediaReport::Playing(PhoneMedia {
            title: title.to_string(),
            artist: String::new(),
            app: "Spotify".to_string(),
            is_playing: true,
            can_play_pause: true,
            can_next: true,
            can_previous: true,
            ..Default::default()
        })
    }

    fn with_cover(title: &str, id: &str) -> MediaReport {
        let MediaReport::Playing(mut media) = song(title) else {
            unreachable!()
        };
        media.art_id = Some(id.to_string());
        MediaReport::Playing(media)
    }

    /// A JPEG's start declaring 64 x 64, then `tail`, and its id.
    fn jpeg(tail: &[u8]) -> (Vec<u8>, String) {
        use sha2::{Digest, Sha256};
        let mut bytes = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0, 64, 0, 64, 3, 1, 0x22, 0, 2, 0x11, 1, 3,
            0x11, 1, 0xFF, 0xD9,
        ];
        bytes.extend_from_slice(tail);
        let id = Sha256::digest(&bytes)[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        (bytes, id)
    }

    #[test]
    fn a_cover_is_kept_once_and_shown_by_its_id_until_the_track_has_none() {
        use base64::Engine;
        let dir = std::env::temp_dir().join(format!("pd-media-cover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (bytes, id) = jpeg(b"cover");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let first = cover(&dir, "phone", &with_cover("One", &id), Some(&b64), true);
        assert!(first.is_some(), "the bytes kept");
        assert_eq!(cover(&dir, "phone", &song("Plain"), None, false), None);
        assert!(
            pocketdesk_core::local::phone_media::has_art(&dir, "phone", &id),
            "no cover before: the folder is not looked at"
        );
        assert_eq!(
            cover(&dir, "phone", &with_cover("One", &id), None, true),
            first,
            "said again by id"
        );
        assert_eq!(
            cover(
                &dir,
                "phone",
                &with_cover("Two", "0123456789abcdef"),
                None,
                true
            ),
            None,
            "another id with no bytes shows none"
        );
        assert_eq!(
            cover(&dir, "phone", &with_cover("One", &id), None, true),
            None,
            "and the old one went"
        );
        let again = cover(&dir, "phone", &with_cover("One", &id), Some(&b64), true);
        assert!(again.is_some());
        assert_eq!(
            cover(&dir, "phone", &song("Three"), None, true),
            None,
            "a track with none"
        );
        assert!(!pocketdesk_core::local::phone_media::has_art(
            &dir, "phone", &id
        ));
        assert_eq!(
            cover(
                &dir,
                "phone",
                &with_cover("Bad", &id),
                Some("not base64!"),
                true
            ),
            None,
            "bad bytes and nothing kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_is_kept_and_nothing_playing_forgets_it() {
        let mut reports = Reports::default();
        reports.take("phone-a", song("One"), 100, None);
        assert_eq!(reports.phones["phone-a"].media.title, "One");
        assert_eq!(reports.phones["phone-a"].updated_unix, 100);
        reports.dirty = false;
        reports.take("phone-a", MediaReport::Nothing, 101, None);
        assert!(reports.phones.is_empty());
        assert!(reports.dirty);
        reports.dirty = false;
        reports.take("phone-a", MediaReport::Nothing, 102, None);
        assert!(!reports.dirty, "nothing to forget, nothing to write");
    }

    #[test]
    fn so_many_phones_are_kept_and_one_no_longer_paired_is_forgotten() {
        let mut reports = Reports::default();
        for n in 0..MAX_PHONES {
            reports.take(&format!("phone-{n}"), song("x"), n as u64, None);
        }
        reports.take("phone-new", song("x"), 1000, None);
        assert_eq!(reports.phones.len(), MAX_PHONES);
        assert!(reports.phones.contains_key("phone-new"));
        assert!(!reports.phones.contains_key("phone-0"), "the oldest went");
        reports.dirty = false;
        reports.keep_only(&["phone-new".to_string()].into());
        assert_eq!(reports.phones.len(), 1);
        assert!(reports.dirty);
    }

    /// Only a phone that is connected, paired and allowed Media has its
    /// media shown: one whose session ended, or whose Media is off, keeps
    /// no stale song with buttons that would be refused.
    #[test]
    fn only_connected_paired_phones_with_media_on_are_shown() {
        let dir = std::env::temp_dir().join(format!("pd-media-showable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        for id in ["here", "media-off", "away"] {
            store.trust_device(id).unwrap();
        }
        let mut off = store.permissions("media-off");
        off.media = Some(false);
        store.set_permissions("media-off", off).unwrap();
        let connected = ["here", "media-off", "stranger"].map(str::to_string);
        let shown = showable(&connected, &store);
        assert_eq!(shown, ["here".to_string()].into());
    }

    struct Phone {
        id: &'static str,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_press_goes_to_its_phone_only_with_media_on() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let b = Arc::new(Phone {
            id: "phone-b",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone(), b.clone()];
        let press = MediaPress {
            id: "x".to_string(),
            device_id: "phone-b".to_string(),
            command: "next".to_string(),
            created_unix: 1,
        };
        push(&press, &clients, true).await.unwrap();
        assert!(a.sent.lock().unwrap().is_empty());
        let sent = b.sent.lock().unwrap().clone();
        assert_eq!(sent[0].packet_type, PHONE_MEDIA_CONTROL);
        assert_eq!(sent[0].body["command"], "next");
        assert!(
            push(&press, &clients, false)
                .await
                .unwrap_err()
                .contains("off")
        );
        let away = MediaPress {
            device_id: "phone-c".to_string(),
            ..press
        };
        assert!(
            push(&away, &clients, true)
                .await
                .unwrap_err()
                .contains("not connected")
        );
    }

    /// Only a paired phone's report is kept, and none is answered.
    #[tokio::test]
    async fn only_a_paired_phones_report_is_kept() {
        let dir = crate::state::use_test_dirs();
        std::fs::create_dir_all(dir).unwrap();
        TrustStore::update(dir.join(TRUST_FILE), |store| {
            store.trust_device("media-paired")
        })
        .unwrap();
        let report = NetworkPacket::new(PHONE_MEDIA)
            .with_body(serde_json::json!({"title": "Song", "isPlaying": true}));
        for id in ["media-paired", "media-stranger"] {
            let phone = Phone {
                id,
                sent: Mutex::default(),
            };
            PhoneMediaPlugin
                .handle_packet(&report, &phone)
                .await
                .unwrap();
            assert!(phone.sent.lock().unwrap().is_empty(), "{id} answered");
        }
        let reports = REPORTS.lock().unwrap();
        assert_eq!(reports.phones["media-paired"].media.title, "Song");
        assert!(!reports.phones.contains_key("media-stranger"));
    }
}
