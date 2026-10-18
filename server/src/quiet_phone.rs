//! Do Not Disturb kept in step with the phones (D2).
//!
//! Each mute the desktop app or `pocketdesk quiet` queued
//! (`core::local::quiet`) goes to its phone as `kdeconnect.pocketdesk.quiet
//! {minutes}` over the phone's live connection, under the phone's Mute
//! permission; a computer is never muted. A phone with "Let the computer
//! mute this phone" on says when it is quiet (`quiet_state`, under the same
//! permission, which the control gate checks before this plugin), kept in
//! `<state>/phone_quiet.json` for the desktop app and the local API.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::phone_quiet::PhoneQuiet;
use pocketdesk_core::local::quiet::QuietRequest;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::quiet::{QUIET, QUIET_STATE};

/// The most phones whose state is kept.
const MAX_PHONES: usize = 64;

/// Daemon task: send each queued mute to its phone.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: muting phones is off");
        return;
    };
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::QUIET_DIR,
            Duration::from_millis(250),
        )
        .await;
        let now = pocketdesk_core::local::pin::now_unix();
        for request in pocketdesk_core::local::quiet::take_all(&dir, now) {
            let clients = crate::state::clone_clients().await;
            let allowed = crate::plugins::pair::allows(&request.device_id, Capability::Mute)
                && !crate::plugins::pair::is_computer_device(&request.device_id);
            match send(&request, &clients, allowed).await {
                Ok(()) => tracing::info!(
                    "Asked {} to be quiet for {} minutes",
                    request.device_id,
                    request.minutes
                ),
                Err(reason) => tracing::info!("{} not muted: {reason}", request.device_id),
            }
        }
    }
}

/// Send one mute (or its end) to its phone: allowed and connected.
async fn send(
    request: &QuietRequest,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
) -> std::result::Result<(), String> {
    if !allowed {
        return Err("Mute is off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .ok_or("the phone is not connected")?;
    let packet =
        NetworkPacket::new(QUIET).with_body(serde_json::json!({ "minutes": request.minutes }));
    phone
        .send_packet(&packet)
        .await
        .map_err(|e| format!("could not send: {e}"))
}

/// Each phone's state, as it last said; kept in memory and written on change.
static STATES: LazyLock<Mutex<BTreeMap<String, PhoneQuiet>>> = LazyLock::new(Default::default);

/// Keep `device`'s `body` in `states`; whether its quiet changed (an event).
/// A full map makes room by dropping the oldest phone.
fn take(
    states: &mut BTreeMap<String, PhoneQuiet>,
    device: &str,
    body: &serde_json::Value,
    now_unix: u64,
) -> Option<bool> {
    let state = PhoneQuiet::from_body(body, now_unix)?;
    if !states.contains_key(device) && states.len() >= MAX_PHONES {
        let oldest = states
            .iter()
            .min_by_key(|(_, s)| s.updated_unix)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            states.remove(&oldest);
        }
    }
    // A first report says something only when quiet (D2 review): a lost
    // file would otherwise log "off" for every phone.
    let changed = match states.get(device) {
        Some(before) => before.quiet != state.quiet,
        None => state.quiet,
    };
    states.insert(device.to_string(), state);
    Some(changed)
}

/// Forget every phone not in `paired`.
fn keep_only(states: &mut BTreeMap<String, PhoneQuiet>, paired: &HashSet<String>) {
    states.retain(|id, _| paired.contains(id));
}

#[derive(Debug, Default)]
pub struct PhoneQuietPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneQuietPlugin {
    fn name(&self) -> &'static str {
        "phone_quiet"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[QUIET_STATE]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[QUIET]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != QUIET_STATE {
            return Ok(());
        }
        let device = sender.device_id();
        // Only a paired phone's, never a computer's.
        if !crate::plugins::pair::is_paired_device(device)
            || crate::plugins::pair::is_computer_device(device)
        {
            return Ok(());
        }
        let now = pocketdesk_core::local::pin::now_unix();
        let (changed, quiet) = {
            let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
            let Some(changed) = take(&mut states, device, &packet.body, now) else {
                tracing::debug!("A quiet state from {device} was not read");
                return Ok(());
            };
            (changed, states.get(device).is_some_and(|s| s.quiet))
        };
        if changed {
            crate::events::emit(
                "phone_quiet",
                Some(device),
                serde_json::json!({ "quiet": quiet }),
                serde_json::json!({}),
            );
        }
        write();
        Ok(())
    }
}

/// Write the states, without the phones no longer paired, off the packet loop.
fn write() {
    if cfg!(test) {
        return;
    }
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    std::thread::spawn(move || {
        // One write at a time, each of the newest states (D2 review): a
        // thread per report, each with its own snapshot, could write an
        // older one last.
        static WRITING: Mutex<()> = Mutex::new(());
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let paired: Option<HashSet<String>> =
            pocketdesk_core::protocol::kdeconnect::TrustStore::read(dir.join(TRUST_FILE))
                .ok()
                .map(|store| store.trusted_devices().into_iter().collect());
        let states = {
            let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(paired) = &paired {
                keep_only(&mut states, paired);
            }
            states.clone()
        };
        if let Err(e) = pocketdesk_core::local::phone_quiet::write(&dir, &states) {
            tracing::warn!("Could not keep the phones' quiet state: {e}");
        }
    });
}

/// Load what an earlier run kept, so the desktop app shows it until each
/// phone says again.
pub fn load() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let kept = pocketdesk_core::local::phone_quiet::read(&dir);
    let mut states = STATES.lock().unwrap_or_else(|e| e.into_inner());
    for (device, state) in kept {
        states.entry(device).or_insert(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    fn request(device: &str, minutes: u32) -> QuietRequest {
        QuietRequest {
            id: "x".to_string(),
            device_id: device.to_string(),
            minutes,
            created_unix: 1,
        }
    }

    #[tokio::test]
    async fn a_mute_goes_to_its_phone_only_with_mute_on() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone()];
        send(&request("phone-a", 60), &clients, true).await.unwrap();
        send(&request("phone-a", 0), &clients, true).await.unwrap();
        let sent = a.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].packet_type, QUIET);
        assert_eq!(sent[0].body["minutes"], 60);
        assert_eq!(sent[1].body["minutes"], 0, "0 ends the mute");
        assert!(
            send(&request("phone-a", 60), &clients, false)
                .await
                .is_err()
        );
        assert!(send(&request("phone-b", 60), &clients, true).await.is_err());
        assert_eq!(a.sent.lock().unwrap().len(), 2, "nothing more sent");
    }

    #[test]
    fn a_state_is_kept_and_a_change_is_told() {
        let mut states = BTreeMap::new();
        assert_eq!(
            take(&mut states, "q", &json!({"quiet": false}), 9),
            Some(false),
            "a first report of not quiet tells nothing"
        );
        assert_eq!(
            take(&mut states, "p", &json!({"quiet": true}), 10),
            Some(true)
        );
        assert_eq!(
            take(&mut states, "p", &json!({"quiet": true}), 11),
            Some(false),
            "no change"
        );
        assert_eq!(
            take(&mut states, "p", &json!({"quiet": false}), 12),
            Some(true)
        );
        assert_eq!(take(&mut states, "p", &json!({"nope": 1}), 13), None);
        assert!(!states["p"].quiet);
        assert_eq!(states["p"].updated_unix, 12);
    }

    #[test]
    fn the_oldest_phone_makes_room_and_unpaired_ones_go() {
        let mut states = BTreeMap::new();
        for i in 0..MAX_PHONES {
            take(
                &mut states,
                &format!("p{i}"),
                &json!({"quiet": false}),
                100 + i as u64,
            );
        }
        take(&mut states, "new", &json!({"quiet": true}), 1_000);
        assert_eq!(states.len(), MAX_PHONES);
        assert!(!states.contains_key("p0"), "the oldest made room");
        assert!(states.contains_key("new"));
        let paired: HashSet<String> = ["new".to_string()].into();
        keep_only(&mut states, &paired);
        assert_eq!(states.keys().collect::<Vec<_>>(), ["new"]);
    }
}
