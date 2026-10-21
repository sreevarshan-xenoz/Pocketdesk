//! Typing on a phone from this computer's keyboard (M4): the desktop app's
//! typing window writes batches to `typing_out/`, and [`run`] sends each to
//! its phone, in order, as `kdeconnect.pocketdesk.type`. Only while
//! "Control phones from their screen window" is on and the phone's "Show
//! its screen or camera here" permission allows it, the two that gate C8's
//! control. A batch for a phone not connected is dropped: late keys would
//! land in another field.

use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::typing::{self, TypeBatch};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::typing::{TYPE, body};

/// The packet for `batch`, if this computer may type on that phone.
pub fn packet_for(batch: &TypeBatch, control_on: bool, allowed: bool) -> Option<NetworkPacket> {
    (control_on && allowed).then(|| NetworkPacket::new(TYPE).with_body(body(&batch.events)))
}

/// Send the batches the typing window writes, for the server's life.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::TYPING_DIR,
            Duration::from_millis(500),
        )
        .await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let batches = typing::take_all(&dir, now_ms);
        if batches.is_empty() {
            continue;
        }
        let control_on = crate::events::current_config().cast_control;
        let clients = crate::state::clone_clients().await;
        for batch in batches {
            let allowed = crate::plugins::pair::allows(
                &batch.device_id,
                pocketdesk_core::protocol::permissions::Capability::Cast,
            );
            let Some(packet) = packet_for(&batch, control_on, allowed) else {
                tracing::info!(
                    "Typing not sent: control from this computer or the phone's permission is off"
                );
                continue;
            };
            let Some(phone) = clients.iter().find(|c| c.device_id() == batch.device_id) else {
                tracing::debug!("Typing not sent: the phone is not connected");
                continue;
            };
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::debug!("Typing not sent to the phone: {e}");
            }
        }
    }
}

/// Announces what this computer may send a phone to type (M4).
#[derive(Debug, Default)]
pub struct TypingPlugin;

#[async_trait::async_trait]
impl Plugin for TypingPlugin {
    fn name(&self) -> &'static str {
        "typing"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[TYPE]
    }

    async fn handle_packet(
        &self,
        _packet: &NetworkPacket,
        _sender: &dyn DeviceSender,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::typing::TypeEvent;

    #[test]
    fn a_batch_goes_only_with_control_on_and_the_phone_allowing_it() {
        let batch = TypeBatch {
            device_id: "phone".to_string(),
            seq: 1,
            created_ms: 1,
            events: vec![TypeEvent::Text("hi".to_string()), TypeEvent::Enter],
        };
        let packet = packet_for(&batch, true, true).expect("sent");
        assert_eq!(packet.packet_type, TYPE);
        assert_eq!(packet.body["events"][0]["text"], "hi");
        assert_eq!(packet.body["events"][1]["key"], "enter");
        assert!(
            packet_for(&batch, false, true).is_none(),
            "control off here"
        );
        assert!(
            packet_for(&batch, true, false).is_none(),
            "the phone's permission off"
        );
    }
}
