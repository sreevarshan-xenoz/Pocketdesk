//! The phones' media (roadmap T8d): a phone in a session reports what it
//! plays (`kdeconnect.pocketdesk.phone_media`, `{}` when nothing does); the
//! server keeps the newest report per paired phone in memory and writes
//! them to `<state>/phone_media.json` at most every [`WRITE_EVERY`]. The
//! desktop app's presses (`<state>/phone_media_out/`) go to their phone as
//! `kdeconnect.pocketdesk.phone_media_control {command}`. The Media
//! permission gates both: the dispatch gate the reports, [`run_watcher`]
//! the presses.

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
    /// Keep `report` from `device` (a paired phone), at `now_unix`.
    fn take(&mut self, device: &str, report: MediaReport, now_unix: u64) {
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
                    },
                );
                self.dirty = true;
            }
        }
    }

    /// Forget every phone not in `shown`.
    fn keep_only(&mut self, shown: &std::collections::HashSet<String>) {
        let before = self.phones.len();
        self.phones.retain(|id, _| shown.contains(id));
        if self.phones.len() != before {
            self.dirty = true;
        }
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
        REPORTS.lock().unwrap_or_else(|e| e.into_inner()).take(
            sender.device_id(),
            report,
            pocketdesk_core::local::pin::now_unix(),
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
        let phones = {
            let mut reports = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(shown) = &shown {
                reports.keep_only(shown);
            }
            if !reports.dirty {
                continue;
            }
            reports.dirty = false;
            reports.phones.clone()
        };
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
        })
    }

    #[test]
    fn a_report_is_kept_and_nothing_playing_forgets_it() {
        let mut reports = Reports::default();
        reports.take("phone-a", song("One"), 100);
        assert_eq!(reports.phones["phone-a"].media.title, "One");
        assert_eq!(reports.phones["phone-a"].updated_unix, 100);
        reports.dirty = false;
        reports.take("phone-a", MediaReport::Nothing, 101);
        assert!(reports.phones.is_empty());
        assert!(reports.dirty);
        reports.dirty = false;
        reports.take("phone-a", MediaReport::Nothing, 102);
        assert!(!reports.dirty, "nothing to forget, nothing to write");
    }

    #[test]
    fn so_many_phones_are_kept_and_one_no_longer_paired_is_forgotten() {
        let mut reports = Reports::default();
        for n in 0..MAX_PHONES {
            reports.take(&format!("phone-{n}"), song("x"), n as u64);
        }
        reports.take("phone-new", song("x"), 1000);
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
        let dir = pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
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
