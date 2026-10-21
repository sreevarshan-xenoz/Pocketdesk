//! What a phone's connection is for (L8 part 2): a phone that keeps a link
//! to another computer of its owner's only to carry copies of its
//! notifications says so as the link opens, with
//! `kdeconnect.pocketdesk.link_role {role: "copies"}`. That connection then
//! takes this computer's answers and clears to its notifications and
//! nothing else: it is no push target, and the phone is not shown as linked
//! by it (`state::mark_copy_link`).
//!
//! It only narrows what the connection is used for, so any connection may
//! say it, paired or not.

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// The packet a phone names a connection's role with.
pub const LINK_ROLE: &str = "kdeconnect.pocketdesk.link_role";

#[derive(Debug, Default)]
pub struct LinkRolePlugin;

#[async_trait::async_trait]
impl Plugin for LinkRolePlugin {
    fn name(&self) -> &'static str {
        "link_role"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[LINK_ROLE]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type == LINK_ROLE
            && packet.body.get("role").and_then(|v| v.as_str()) == Some("copies")
        {
            crate::state::mark_copy_link(sender.connection_id());
            tracing::debug!(
                device = %sender.device_id(),
                "A phone's link carries copies of its notifications only"
            );
        }
        Ok(())
    }
}
