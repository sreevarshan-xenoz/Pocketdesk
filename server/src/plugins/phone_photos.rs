//! The phone's recent photos on the computer (C7): a phone's answer
//! (`kdeconnect.pocketdesk.phone_photos`) is kept as its last, with its
//! thumbnails, for the desktop app; the app's requests (`phone_photos_out/`)
//! go to their phone as `phone_photos.list` or `phone_photos.fetch`. The
//! Files permission gates both: the dispatch gate the answers, the watcher
//! the requests.

use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::phone_photos::{self as store, Ask};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::phone_photos::{
    FETCH, LIST, PHOTOS, Photos, TAKE, fetch_body, list_body,
};

pub struct PhonePhotosPlugin;

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Plugin for PhonePhotosPlugin {
    fn name(&self) -> &'static str {
        "phone_photos"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PHOTOS]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[LIST, FETCH, TAKE]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != PHOTOS {
            return Ok(());
        }
        let Some(photos) = Photos::from_body(&packet.body) else {
            tracing::info!("A phone's photos were dropped: not an answer");
            return Ok(());
        };
        // Only a paired phone's photos are kept (an unpaired one passes the
        // gate when pairing is not required), never a computer's, whose
        // default permissions include Files (C7 review).
        if !crate::plugins::pair::is_paired_device(sender.device_id())
            || crate::plugins::pair::is_computer_device(sender.device_id())
        {
            tracing::info!("Photos from a device that is not a paired phone were dropped");
            return Ok(());
        }
        let device = sender.device_id().to_string();
        let dir = crate::state::state_dir().map_err(|e| {
            pocketdesk_core::error::PocketdeskError::Other {
                detail: e.to_string(),
            }
        })?;
        tokio::task::spawn_blocking(move || {
            store::keep(&dir, &device, photos, unix_now(), &|id| {
                crate::plugins::pair::is_paired_device(id)
            })
        })
        .await
        .ok()
        .and_then(|r| r.err())
        .map_or(Ok(()), |e| {
            tracing::warn!("A phone's photos were not kept: {e}");
            Ok(())
        })
    }
}

/// The packet a request sends.
pub(crate) fn request_packet(request: &store::Request) -> NetworkPacket {
    match request.ask {
        Ask::List => NetworkPacket::new(LIST).with_body(list_body(None)),
        Ask::ListBefore(id) => NetworkPacket::new(LIST).with_body(list_body(Some(id))),
        Ask::Fetch(id) => NetworkPacket::new(FETCH).with_body(fetch_body(id, false)),
        Ask::Copy(id) => NetworkPacket::new(FETCH).with_body(fetch_body(id, true)),
        Ask::Take => NetworkPacket::new(TAKE).with_body(serde_json::json!({})),
    }
}

/// Send the desktop app's requests to their phones, each only to a
/// connected phone with Files on. The photos of an earlier run are dropped
/// first. Runs for the server's life.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let _ = store::clear(&dir);
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::PHONE_PHOTOS_OUT_DIR,
            Duration::from_millis(300),
        )
        .await;
        let requests = store::take_all(&dir, unix_now());
        if requests.is_empty() {
            continue;
        }
        let clients = crate::state::clone_clients().await;
        for request in requests {
            let packet = request_packet(&request);
            if !crate::plugins::pair::permits(&request.device_id, &packet.packet_type) {
                tracing::info!("A request for a phone's photos was dropped: Files is off");
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == request.device_id) else {
                tracing::info!(
                    "A request for a phone's photos was dropped: the phone is not connected"
                );
                continue;
            };
            // The photo that answers is not one sent unasked (O7 review).
            if matches!(request.ask, Ask::Fetch(_) | Ask::Copy(_) | Ask::Take) {
                crate::plugins::share::asked()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .note(&request.device_id, std::time::Instant::now());
            }
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("A request for a phone's photos was not sent: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_becomes_its_packet() {
        let request = |ask| store::Request {
            id: "00000000-0000-0000-0000-000000000000".into(),
            device_id: "phone".into(),
            ask,
            created_unix: 1,
        };
        let list = request_packet(&request(Ask::List));
        assert_eq!(list.packet_type, LIST);
        let fetch = request_packet(&request(Ask::Fetch(7)));
        assert_eq!(fetch.packet_type, FETCH);
        assert_eq!(fetch.body["id"], 7);
        assert!(fetch.body.get("clipboard").is_none());
        // T4: an older page, and a copy the phone puts on the clipboard.
        let older = request_packet(&request(Ask::ListBefore(9)));
        assert_eq!(
            (older.packet_type.as_str(), &older.body["before"]),
            (LIST, &serde_json::json!(9))
        );
        let copy = request_packet(&request(Ask::Copy(7)));
        assert_eq!(copy.packet_type, FETCH);
        assert_eq!(copy.body["clipboard"], true);
        // F3: the camera, offered to the phone's owner.
        assert_eq!(request_packet(&request(Ask::Take)).packet_type, TAKE);
    }

    #[test]
    fn the_photos_packets_need_files() {
        use pocketdesk_core::protocol::permissions::{Capability, capability_for};
        for t in [LIST, PHOTOS, FETCH, TAKE] {
            assert_eq!(capability_for(t), Some(Capability::Files), "{t}");
        }
    }
}
