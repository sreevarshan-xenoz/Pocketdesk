//! The phone's storage on the computer (T10g): a phone's listing of a
//! folder it shares (`kdeconnect.pocketdesk.phone_files.listing`) is kept as
//! its last, for the desktop app; the app's requests (`phone_files_out/`)
//! go to their phone as `phone_files.list` or `phone_files.fetch`. The
//! Files permission gates both: the dispatch gate the listings, the
//! watcher the requests.

use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::phone_files::{self as store, Kind};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::phone_files::{FETCH, LIST, LISTING, Listing, request_body};

pub struct PhoneFilesPlugin;

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Plugin for PhoneFilesPlugin {
    fn name(&self) -> &'static str {
        "phone_files"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[LISTING]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[LIST, FETCH]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != LISTING {
            return Ok(());
        }
        let Some(listing) = Listing::from_body(&packet.body) else {
            tracing::info!("A phone's file listing was dropped: out of bounds");
            return Ok(());
        };
        // Only a paired phone's listing is kept (an unpaired one passes the
        // gate when pairing is not required).
        if !crate::plugins::pair::is_paired_device(sender.device_id()) {
            tracing::info!("A file listing from a phone that is not paired was dropped");
            return Ok(());
        }
        let device = sender.device_id().to_string();
        // The drive may be waiting for it (D8).
        crate::drive::live::listing_came(&device, &listing);
        let dir = crate::state::state_dir().map_err(|e| {
            pocketdesk_core::error::PocketdeskError::Other {
                detail: e.to_string(),
            }
        })?;
        tokio::task::spawn_blocking(move || {
            store::keep(&dir, &device, listing, unix_now(), &|id| {
                crate::plugins::pair::is_paired_device(id)
            })
        })
        .await
        .ok()
        .and_then(|r| r.err())
        .map_or(Ok(()), |e| {
            tracing::warn!("A phone's file listing was not kept: {e}");
            Ok(())
        })
    }
}

/// The packet a request sends.
pub(crate) fn request_packet(request: &store::Request) -> Option<NetworkPacket> {
    let kind = match request.kind {
        Kind::List => LIST,
        Kind::Fetch => FETCH,
    };
    request_body(request.root, &request.path).map(|body| NetworkPacket::new(kind).with_body(body))
}

/// Send the desktop app's requests to their phones, each only to a
/// connected phone with Files on. The listings of an earlier run are
/// dropped first. Runs for the server's life.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let _ = store::clear(&dir);
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::PHONE_FILES_DIR,
            Duration::from_millis(300),
        )
        .await;
        let requests = store::take_all(&dir, unix_now());
        if requests.is_empty() {
            continue;
        }
        let clients = crate::state::clone_clients().await;
        for request in requests {
            let Some(packet) = request_packet(&request) else {
                continue;
            };
            if !crate::plugins::pair::permits(&request.device_id, &packet.packet_type) {
                tracing::info!("A request for a phone's files was dropped: Files is off");
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == request.device_id) else {
                tracing::info!(
                    "A request for a phone's files was dropped: the phone is not connected"
                );
                continue;
            };
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("A request for a phone's files was not sent: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_becomes_its_packet() {
        let request = store::Request {
            id: "00000000-0000-0000-0000-000000000000".into(),
            device_id: "phone".into(),
            kind: Kind::Fetch,
            root: 2,
            path: vec!["Docs".into(), "a.pdf".into()],
            created_unix: 1,
        };
        let packet = request_packet(&request).unwrap();
        assert_eq!(packet.packet_type, FETCH);
        assert_eq!(packet.body["root"], 2);
        assert_eq!(packet.body["path"][1], "a.pdf");
    }
}
