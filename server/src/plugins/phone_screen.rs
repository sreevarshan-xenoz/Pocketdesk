//! The phone as another screen for this computer's keyboard and mouse
//! (BK5): what a paired phone says of itself
//! ([`SCREEN`](pocketdesk_core::protocol::phone_screen::SCREEN)), kept for
//! the driver and, as a block, in `shared_screens.json` for the Keyboard
//! and mouse page; and its answers to the driver's pings
//! ([`PONG`](pocketdesk_core::protocol::phone_screen::PONG)). Neither is an
//! action, so no permission gates them; only a paired phone's screen is
//! taken (another computer is never a phone).

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::phone_screen::{INPUT, PONG, SCREEN, ScreenReport, pong_of};

use crate::shared_input::phone;

/// Takes what a phone says as a screen.
#[derive(Debug, Default)]
pub struct PhoneScreenPlugin;

/// Whether `device_id` is a paired phone.
fn paired_phone(device_id: &str) -> bool {
    crate::plugins::pair::is_paired_device(device_id)
        && !crate::plugins::pair::is_computer_device(device_id)
}

/// Keep the phone `device_id`'s screen in `shared_screens.json`, under the
/// name it has here. Blocking file work.
fn keep_screen(device_id: &str, report: &ScreenReport) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let name = crate::state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::read(path).ok())
        .and_then(|store| store.name(device_id).map(str::to_string))
        .unwrap_or_default();
    let now = pocketdesk_core::local::pin::now_unix();
    if let Err(e) = pocketdesk_core::local::shared_input::update(&dir, |file| {
        file.heard_phone(device_id, &name, &report.screen, now)
    }) {
        tracing::debug!("Shared input: the phone's screen was not kept: {e}");
    }
}

#[async_trait::async_trait]
impl Plugin for PhoneScreenPlugin {
    fn name(&self) -> &'static str {
        "phone_screen"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[SCREEN, PONG]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[INPUT]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let id = sender.device_id().to_string();
        match packet.packet_type.as_str() {
            SCREEN => {
                let Some(report) = ScreenReport::from_body(&packet.body) else {
                    return Ok(());
                };
                let _ = tokio::task::spawn_blocking(move || {
                    if !paired_phone(&id) {
                        tracing::debug!(
                            "A phone screen from {id}, not a paired phone, was ignored"
                        );
                        return;
                    }
                    phone::heard(&id, report);
                    keep_screen(&id, &report);
                })
                .await;
            }
            // Counted for whoever sends it: the driver looks only at the
            // phones it drives, which are paired.
            PONG if pong_of(&packet.body).is_some() => phone::ponged(&id),
            _ => {}
        }
        Ok(())
    }
}
