//! The apps a phone lists for its computer (P2): `kdeconnect.pocketdesk.
//! phone_apps {apps}`, answering O2's `apps` ask, kept owner-only in
//! `<state>/phone_apps.json` for the desktop app's "Open an app on the
//! phone…". Held to the phone's Phone actions permission, as the ask is.

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_action::{PHONE_APPS, parse_apps};

#[derive(Debug, Default)]
pub struct PhoneAppsPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneAppsPlugin {
    fn name(&self) -> &'static str {
        "phone_apps"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PHONE_APPS]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != PHONE_APPS {
            return Ok(());
        }
        let device = sender.device_id().to_string();
        if !crate::plugins::pair::allows(&device, Capability::PhoneActions) {
            tracing::debug!("A list of apps from {device} was ignored: Phone actions is off");
            return Ok(());
        }
        let apps = parse_apps(&packet.body);
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        let count = apps.len();
        // Off the runtime: a read and a write of a small file, one writer at
        // a time (P2 review: two lists at once could lose one).
        static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let kept = tokio::task::spawn_blocking(move || {
            let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
            pocketdesk_core::local::phone_apps::keep(
                &dir,
                &device,
                apps,
                pocketdesk_core::local::pin::now_unix(),
                &|id| crate::plugins::pair::is_paired_device(id),
            )
        })
        .await;
        match kept {
            Ok(Ok(())) => tracing::info!("A phone listed {count} apps"),
            Ok(Err(e)) => tracing::warn!("A phone's apps were not kept: {e}"),
            Err(_) => {}
        }
        Ok(())
    }
}
