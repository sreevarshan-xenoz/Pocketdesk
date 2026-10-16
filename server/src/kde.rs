use anyhow::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceIdentity, KdeConnectService, TrustStore};
use std::path::PathBuf;

use crate::plugins;
use crate::state;

pub fn host_identity() -> DeviceIdentity {
    DeviceIdentity::new(host_device_id(), host_device_name())
}

/// Build a fully-initialized `KdeConnectService` with all plugins, identity, and trust store.
pub fn build_default_service() -> Result<KdeConnectService> {
    let mut service = KdeConnectService::new();

    // Single source of truth for plugin registration — same list as plugins::register_all()
    service.registry = plugins::register_all();

    let (incoming, outgoing) = service.registry.capability_sets();

    let mut identity = host_identity();
    identity.incoming_capabilities = incoming;
    identity.outgoing_capabilities = outgoing;
    service.set_identity(identity);

    let trust_store = TrustStore::load_or_create(state::trust_store_path()?)?;
    service.set_trust_store(trust_store);

    Ok(service)
}

fn host_device_id() -> String {
    if let Ok(id) = std::env::var("POCKETDESK_DEVICE_ID") {
        return id;
    }

    let path = state::state_dir()
        .map(|d| d.join("device_id"))
        .unwrap_or_else(|_| PathBuf::from("device_id"));
    if let Ok(id) = std::fs::read_to_string(&path) {
        let trimmed = id.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    // Generate and persist a stable UUID
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &id);
    id
}

pub(crate) fn host_device_name() -> String {
    // Windows sets COMPUTERNAME for every process. HOSTNAME is a shell
    // variable bash does not export, so a server started from a script or a
    // systemd unit never sees it: on Linux the kernel's name is read too.
    #[cfg(unix)]
    let kernel = std::fs::read_to_string("/proc/sys/kernel/hostname").ok();
    #[cfg(not(unix))]
    let kernel = None;
    pick_host_name([
        std::env::var("COMPUTERNAME").ok(),
        std::env::var("HOSTNAME").ok(),
        kernel,
    ])
}

/// The first non-blank name, trimmed; a placeholder when there is none.
fn pick_host_name<const N: usize>(candidates: [Option<String>; N]) -> String {
    candidates
        .into_iter()
        .flatten()
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| "pocketdesk-host".to_string())
}

#[cfg(test)]
mod tests {
    use super::pick_host_name;

    #[test]
    fn the_first_name_given_wins_and_blanks_are_skipped() {
        assert_eq!(
            pick_host_name([None, Some(" ".into()), Some("pdtest\n".into())]),
            "pdtest"
        );
        assert_eq!(
            pick_host_name([Some("DESKTOP-1".into()), Some("other".into())]),
            "DESKTOP-1"
        );
    }

    #[test]
    fn with_no_name_at_all_a_placeholder_is_used() {
        assert_eq!(
            pick_host_name([None, Some(String::new())]),
            "pocketdesk-host"
        );
    }
}
