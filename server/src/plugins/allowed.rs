//! What this computer allows a phone (M2a): a paired phone asks
//! `kdeconnect.pocketdesk.allowed` and is answered, for each of L7's
//! features, whether its own per-phone permission allows it and whether
//! this computer's Settings let any phone have it
//! ([`phone_switches::allowed_body`]). It tells a phone only about itself,
//! so the ask needs no permission; a device not paired gets nothing.

use pocketdesk_core::error::Result;
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::phone_switches::{self, ALLOWED};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use serde_json::json;

use crate::state;

/// The answer for `device_id` from this trust store and config: nothing for
/// a device not paired, or no store to read.
pub fn answer(store: Option<&TrustStore>, device_id: &str, config: &Config) -> serde_json::Value {
    match store {
        Some(store) if store.is_trusted(device_id) => {
            phone_switches::allowed_body(&store.permissions(device_id), config)
        }
        _ => json!({ "allowed": {} }),
    }
}

/// The plugin answering a phone's ask.
pub struct AllowedPlugin;

#[async_trait::async_trait]
impl Plugin for AllowedPlugin {
    fn name(&self) -> &'static str {
        "allowed"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[ALLOWED]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[ALLOWED]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != ALLOWED {
            return Ok(());
        }
        let store = state::trust_store_path()
            .ok()
            .and_then(|path| TrustStore::read(path).ok());
        let body = answer(
            store.as_ref(),
            sender.device_id(),
            &crate::events::current_config(),
        );
        sender
            .send_packet(&NetworkPacket::new(ALLOWED).with_body(body))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::permissions::Capability;

    #[test]
    fn a_paired_phone_hears_what_it_is_allowed_and_another_device_nothing() {
        let dir = std::env::temp_dir().join(format!("pocketdesk-allowed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = TrustStore::read(dir.join("trusted_devices.json")).unwrap();
        store.trust_device("phone-a").unwrap();
        let mut perms = store.permissions("phone-a");
        perms.set(Capability::Clipboard, false);
        store.set_permissions("phone-a", perms).unwrap();
        let config = Config {
            cast_control: false,
            ..Default::default()
        };

        let body = answer(Some(&store), "phone-a", &config);
        assert_eq!(
            body["allowed"]["clipboard"],
            json!({"permission": false, "setting": true})
        );
        assert_eq!(
            body["allowed"]["control"],
            json!({"permission": true, "setting": false})
        );
        assert_eq!(
            body["allowed"]["files"],
            json!({"permission": true, "setting": true})
        );

        assert_eq!(
            answer(Some(&store), "stranger", &config),
            json!({"allowed": {}})
        );
        assert_eq!(answer(None, "phone-a", &config), json!({"allowed": {}}));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
