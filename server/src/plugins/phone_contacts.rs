//! The contacts a phone lists for its computer (P10): `kdeconnect.pocketdesk.
//! phone_contacts {contacts}`, answering the `contacts` ask, names and
//! numbers only, kept owner-only in `<state>/phone_contacts.json` for the
//! desktop app's Messages page. Held to the phone's Texts permission, as
//! the ask is.

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_action::{PHONE_CONTACTS, parse_contacts};

#[derive(Debug, Default)]
pub struct PhoneContactsPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneContactsPlugin {
    fn name(&self) -> &'static str {
        "phone_contacts"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PHONE_CONTACTS]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != PHONE_CONTACTS {
            return Ok(());
        }
        let device = sender.device_id().to_string();
        if !crate::plugins::pair::allows(&device, Capability::Texts) {
            tracing::debug!("A phone's contacts from {device} were ignored: Texts is off");
            return Ok(());
        }
        let contacts = parse_contacts(&packet.body);
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        let count = contacts.len();
        // Off the runtime: a read and a write of a small file, one writer at
        // a time (as P2's: two lists at once could lose one).
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let kept = tokio::task::spawn_blocking(move || {
            let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            pocketdesk_core::local::phone_contacts::keep(
                &dir,
                &device,
                contacts,
                pocketdesk_core::local::pin::now_unix(),
                &|id| crate::plugins::pair::is_paired_device(id),
            )
        })
        .await;
        match kept {
            Ok(Ok(())) => tracing::info!("A phone listed {count} contacts"),
            Ok(Err(e)) => tracing::warn!("A phone's contacts were not kept: {e}"),
            Err(_) => {}
        }
        Ok(())
    }
}
