use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// Wake-on-LAN relay (Tier-2 #12).
///
/// A WoL magic packet is a UDP broadcast — it can only reach a sleeping
/// machine from a host on the *same* LAN segment. When the phone is off-LAN
/// (cellular / remote) it cannot wake the desktop directly. The relay
/// pattern: an always-on Linux peer on that LAN runs this server, the phone
/// connects to the *relay* over WAN (iroh/Tailscale) and asks it to emit
/// the magic packet for the sleeping target's MAC.
///
/// Answers `kdeconnect.pocketdesk.wol` `{mac, broadcast?, name?}` by sending
/// the magic packet from this host's LAN and replying with the outcome.
#[derive(Debug, Default)]
pub struct WakeRelayPlugin;

/// Limited-broadcast default; callers usually set the directed subnet
/// broadcast (e.g. `192.168.1.255`) for their LAN.
const DEFAULT_BROADCAST: &str = "255.255.255.255";

#[async_trait::async_trait]
impl Plugin for WakeRelayPlugin {
    fn name(&self) -> &'static str {
        "wake_relay"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.wol"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.wol" {
            return Ok(());
        }
        let mac = packet
            .body
            .get("mac")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let broadcast = packet
            .body
            .get("broadcast")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_BROADCAST);
        let name = packet.body.get("name").and_then(|v| v.as_str());

        // Only onto this computer's own networks: a magic packet goes nowhere
        // else, and the relay must not send packets wherever a phone names.
        let allowed = broadcast.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| {
            pocketdesk_core::local::wake::may_relay_to(
                ip,
                &pocketdesk_core::local::wake::adapters(),
            )
        });
        if !allowed {
            tracing::warn!("Wake-relay: refused a target outside this computer's networks");
            let response = NetworkPacket::new("kdeconnect.pocketdesk.wol").with_body(insert_name(
                serde_json::json!({ "ok": false, "mac": mac, "error": "not on this computer's network" }),
                name,
            ));
            return sender.send_packet(&response).await;
        }

        // `send_wol` uses a blocking std::net UDP socket.
        let mac_owned = mac.to_string();
        let broadcast_owned = broadcast.to_string();
        let outcome = tokio::task::spawn_blocking(move || {
            pocketdesk_core::tailscale::wol::send_wol(&mac_owned, &broadcast_owned)
                .map_err(|e| e.to_string())
        })
        .await;

        let response = match outcome {
            Ok(Ok(())) => {
                tracing::info!("Wake-relay: magic packet sent via {broadcast}");
                serde_json::json!({ "ok": true, "mac": mac })
            }
            Ok(Err(e)) => {
                tracing::warn!("Wake-relay: WoL send failed: {e}");
                serde_json::json!({ "ok": false, "mac": mac, "error": e })
            }
            Err(e) => serde_json::json!({ "ok": false, "mac": mac, "error": e.to_string() }),
        };
        let response =
            NetworkPacket::new("kdeconnect.pocketdesk.wol").with_body(insert_name(response, name));
        sender.send_packet(&response).await
    }
}

fn insert_name(mut value: serde_json::Value, name: Option<&str>) -> serde_json::Value {
    if let (Some(name), Some(obj)) = (name, value.as_object_mut()) {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_declares_wol_capability() {
        assert!(
            WakeRelayPlugin
                .incoming_capabilities()
                .contains(&"kdeconnect.pocketdesk.wol")
        );
    }

    #[test]
    fn insert_name_adds_target_label() {
        let v = insert_name(serde_json::json!({ "ok": true }), Some("desk"));
        assert_eq!(v.get("name").and_then(|n| n.as_str()), Some("desk"));
        let v = insert_name(serde_json::json!({ "ok": true }), None);
        assert!(v.get("name").is_none());
    }
}
