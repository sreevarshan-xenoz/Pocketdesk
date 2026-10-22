//! Toasts only on the computer in use (N7): a paired phone asks whether
//! this computer is in use, and is answered by how long its keyboard and
//! mouse have been quiet, with desktop Settings "Tell phones when this
//! computer is in use" on. Asked on any link, a copies-only one included
//! (L8), since the phone asks each computer of its owner's.

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::in_use::{ANSWER, ASK, answer_body};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::Value;

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
        // A computer that would not show a phone's notification never says
        // it is in use (N7 review: the others would hold it, and no one
        // would see it).
        // Quiet hours and presenting (P3) hold them too.
        let showing = crate::plugins::notification::shows_phones()
            && !crate::paused::now()
            && crate::quiet_hours::now().is_none();
        let idle = if telling && showing {
            crate::away_lock::idle_secs()
        } else {
            None
        };
        let answer = NetworkPacket::new(ANSWER).with_body(answer_for(telling, showing, idle));
        sender.send_packet(&answer).await
    }
}

/// The answer, by whether this computer tells (`tell_in_use`), would show a
/// phone's notification now (not paused, phones' notifications on), and
/// how long its input has been quiet.
fn answer_for(telling: bool, showing: bool, idle_secs: Option<u64>) -> Value {
    answer_body(telling && showing, idle_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::in_use::in_use;

    /// N7 review: a computer that would not show the notification never
    /// says it is in use.
    #[test]
    fn only_a_computer_that_would_show_it_says_it_is_in_use() {
        assert_eq!(in_use(&answer_for(true, true, Some(3))), Some(true));
        assert_eq!(
            in_use(&answer_for(true, false, Some(3))),
            None,
            "paused, or not showing phones'"
        );
        assert_eq!(
            in_use(&answer_for(false, true, Some(3))),
            None,
            "not telling"
        );
        assert_eq!(in_use(&answer_for(true, true, None)), None, "cannot tell");
    }
}
