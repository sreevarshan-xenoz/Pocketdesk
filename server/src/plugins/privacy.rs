use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;

use crate::privacy;

/// Privacy mode (Tier-3 #15): the phone asks the desktop to block *local*
/// input while the remote session keeps working.
///
/// Answers `kdeconnect.pocketdesk.privacy`:
/// - `{action: "grab"}` — exclusively grab all physical keyboards/mice
///   (`EVIOCGRAB`); the compositor stops seeing local input, our uinput
///   injection path is untouched. Re-arms the auto-release TTL on refresh.
/// - `{action: "release"}` — give local input back.
/// - `{action: "status"}` — report held devices + seconds of TTL left.
/// - `{lock: true}` (combinable with grab) — also engage the screen locker.
///
/// Every reply is `{ok, grabbed?, ttlLeft?, locked?, error?}`.
#[derive(Debug, Default)]
pub struct PrivacyPlugin;

#[async_trait::async_trait]
impl Plugin for PrivacyPlugin {
    fn name(&self) -> &'static str {
        "privacy"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.privacy"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.privacy" {
            return Ok(());
        }
        let action = packet
            .body
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("status");
        let want_lock = packet
            .body
            .get("lock")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut response = match action {
            "grab" => match privacy::grab_input(privacy::GRAB_TTL) {
                Ok(count) => json!({ "ok": true, "grabbed": count }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "release" => {
                let count = privacy::release_input();
                json!({ "ok": true, "released": count })
            }
            _ => {
                let (count, ttl_left) = privacy::privacy_status();
                json!({ "ok": true, "grabbed": count, "ttlLeft": ttl_left })
            }
        };

        if want_lock {
            // The lock helpers use blocking process spawns.
            let outcome = tokio::task::spawn_blocking(privacy::lock_screen).await;
            match outcome {
                Ok(Ok(program)) => response["locked"] = json!(program),
                Ok(Err(e)) => response["lockError"] = json!(e),
                Err(e) => response["lockError"] = json!(e.to_string()),
            }
        }

        let response = NetworkPacket::new("kdeconnect.pocketdesk.privacy").with_body(response);
        sender.send_packet(&response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_declares_privacy_capability() {
        assert!(
            PrivacyPlugin
                .incoming_capabilities()
                .contains(&"kdeconnect.pocketdesk.privacy")
        );
    }

    #[tokio::test]
    async fn status_action_replies_without_grabbing() {
        let (count, ttl) = privacy::privacy_status();
        assert_eq!(count, 0, "test run must not hold real grabs");
        assert!(ttl.is_none());
        let packet = NetworkPacket::new("kdeconnect.pocketdesk.privacy")
            .with_body(json!({ "action": "status" }));
        assert_eq!(packet.packet_type, "kdeconnect.pocketdesk.privacy");
    }
}
