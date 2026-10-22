//! Toasts only on the computer in use (N7): a paired phone asks whether
//! this computer is in use, and is answered by how long its keyboard and
//! mouse have been quiet, with desktop Settings "Tell phones when this
//! computer is in use" on. Asked on any link, a copies-only one included
//! (L8), since the phone asks each computer of its owner's.

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::in_use::{ANSWER, ASK, answer_body};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

#[derive(Debug, Default)]
pub struct InUsePlugin;

#[async_trait::async_trait]
impl Plugin for InUsePlugin {
    fn name(&self) -> &'static str {
        "in_use"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[ASK]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[ANSWER]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != ASK || !crate::plugins::pair::is_paired_device(sender.device_id())
        {
            return Ok(());
        }
        let telling = crate::events::current_config().tell_in_use;
        let idle = if telling {
            crate::away_lock::idle_secs()
        } else {
            None
        };
        let answer = NetworkPacket::new(ANSWER).with_body(answer_body(telling, idle));
        sender.send_packet(&answer).await
    }
}
