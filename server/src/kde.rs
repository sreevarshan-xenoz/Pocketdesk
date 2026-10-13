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

fn host_device_name() -> String {
    // Linux shells export HOSTNAME; Windows sets COMPUTERNAME for every process.
    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(name) = std::env::var(var)
            && !name.is_empty()
        {
            return name;
        }
    }

    "pocketdesk-host".to_string()
}
