//! The keys paired devices prove (device keys, phase A of mutual auth).
//!
//! A paired phone used to be known only by the `deviceId` it announces,
//! which anyone who has seen it can repeat. Here each device is bound to the
//! keys it proves on a connection: its TLS client certificate on LAN QUIC,
//! and its iroh endpoint key on the WAN path. `<state>/device_keys.json` is
//! its own file, written only by the server: the trust store's writers
//! rebuild that file from their struct and would drop a field they do not
//! know.
//!
//! Binding is first use after pairing, with one rule that keeps it from
//! being a race on every transport: only a device with no key at all binds
//! by first use. Once it has one, another transport's key is registered
//! only from a session already authenticated by the first (see
//! [`register`]).

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::paths::DEVICE_KEYS_FILE;

/// Which key a connection proved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// SHA-256 of the TLS client certificate, hex (LAN QUIC).
    Tls,
    /// iroh endpoint id (WAN).
    Iroh,
}

/// What one device has proved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iroh: Option<String>,
}

impl DeviceKeys {
    fn slot(&mut self, slot: Slot) -> &mut Option<String> {
        match slot {
            Slot::Tls => &mut self.tls,
            Slot::Iroh => &mut self.iroh,
        }
    }

    fn is_empty(&self) -> bool {
        self.tls.is_none() && self.iroh.is_none()
    }
}

/// The verdict on a session's claim to be a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The key matches the one bound to the device.
    Matches,
    /// The device had no key; this one is now bound to it.
    Bound,
    /// Neither a key presented nor one bound: a client from before device
    /// keys, allowed until the device binds one.
    Unbound,
    /// Not this device, and why.
    Refused(&'static str),
}

impl Verdict {
    pub fn allows(&self) -> bool {
        !matches!(self, Verdict::Refused(_))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct KeysFile {
    #[serde(default)]
    devices: HashMap<String, DeviceKeys>,
}

/// Reads, changes and replaces the file one caller at a time.
static WRITE: Mutex<()> = Mutex::new(());

/// Every device's keys. A missing file is none; one that cannot be read is
/// an error, since it could be hiding a binding.
pub fn read(state_dir: &Path) -> io::Result<HashMap<String, DeviceKeys>> {
    match std::fs::read_to_string(state_dir.join(DEVICE_KEYS_FILE)) {
        Ok(raw) => serde_json::from_str::<KeysFile>(&raw)
            .map(|f| f.devices)
            .map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e),
    }
}

fn write(state_dir: &Path, devices: HashMap<String, DeviceKeys>) -> io::Result<()> {
    let json = serde_json::to_string_pretty(&KeysFile { devices }).map_err(io::Error::other)?;
    let path = state_dir.join(DEVICE_KEYS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// Decide a session that claims `device_id` and proved `presented` (or
/// nothing) on `slot`, binding the key when the device has none yet.
pub fn check(state_dir: &Path, device_id: &str, slot: Slot, presented: Option<&str>) -> Verdict {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(mut devices) = read(state_dir) else {
        return Verdict::Refused("the device key store cannot be read");
    };
    let keys = devices.get(device_id).cloned().unwrap_or_default();
    let mut bound = keys.clone();
    match (presented, bound.slot(slot).clone()) {
        (Some(key), Some(known)) if known == key => Verdict::Matches,
        (Some(_), Some(_)) => Verdict::Refused("a different key than the one this device proved"),
        (Some(key), None) if keys.is_empty() => {
            *bound.slot(slot) = Some(key.to_string());
            devices.insert(device_id.to_string(), bound);
            match write(state_dir, devices) {
                Ok(()) => Verdict::Bound,
                Err(_) => Verdict::Refused("the device key store cannot be written"),
            }
        }
        (Some(_), None) => {
            Verdict::Refused("this device's key for this connection was never registered")
        }
        (None, _) if keys.is_empty() => Verdict::Unbound,
        (None, _) => Verdict::Refused("no key, but this device has proved one"),
    }
}

/// Record `key` for `device_id` on `slot`, replacing what was there. Only
/// for a session already authenticated as that device by another key.
pub fn register(state_dir: &Path, device_id: &str, slot: Slot, key: &str) -> io::Result<()> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = read(state_dir)?;
    *devices.entry(device_id.to_string()).or_default().slot(slot) = Some(key.to_string());
    write(state_dir, devices)
}

/// Forget `device_id`'s keys: it paired again (perhaps from a reinstalled
/// phone, with new keys) or was removed.
pub fn clear(state_dir: &Path, device_id: &str) -> io::Result<()> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = read(state_dir)?;
    if devices.remove(device_id).is_some() {
        write(state_dir, devices)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_device_binds_its_first_key_and_must_keep_it() {
        let dir = temp_dir("bind");
        assert_eq!(check(&dir, "phone", Slot::Tls, Some("aa")), Verdict::Bound);
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("aa")),
            Verdict::Matches
        );
        assert!(
            !check(&dir, "phone", Slot::Tls, Some("bb")).allows(),
            "another key"
        );
        assert!(
            !check(&dir, "phone", Slot::Tls, None).allows(),
            "no key at all"
        );
        assert_eq!(read(&dir).unwrap()["phone"].tls.as_deref(), Some("aa"));
    }

    #[test]
    fn a_client_without_keys_passes_only_while_its_device_has_none() {
        let dir = temp_dir("legacy");
        assert_eq!(check(&dir, "phone", Slot::Tls, None), Verdict::Unbound);
        assert!(read(&dir).unwrap().is_empty(), "nothing recorded");
    }

    #[test]
    fn a_second_transport_is_not_bound_by_first_use() {
        // Otherwise anyone could claim the phone on the WAN before the phone
        // ever used it there.
        let dir = temp_dir("second");
        check(&dir, "phone", Slot::Tls, Some("aa"));
        assert!(!check(&dir, "phone", Slot::Iroh, Some("attacker")).allows());
        register(&dir, "phone", Slot::Iroh, "phone-endpoint").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Iroh, Some("phone-endpoint")),
            Verdict::Matches
        );
    }

    #[test]
    fn pairing_again_lets_a_new_key_bind() {
        let dir = temp_dir("clear");
        check(&dir, "phone", Slot::Tls, Some("old"));
        check(&dir, "other", Slot::Tls, Some("other-key"));
        clear(&dir, "phone").unwrap();
        assert_eq!(check(&dir, "phone", Slot::Tls, Some("new")), Verdict::Bound);
        assert_eq!(
            check(&dir, "other", Slot::Tls, Some("other-key")),
            Verdict::Matches,
            "untouched"
        );
    }

    #[test]
    fn an_unreadable_store_refuses_rather_than_forgets() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(DEVICE_KEYS_FILE), "{not json").unwrap();
        assert!(!check(&dir, "phone", Slot::Tls, Some("aa")).allows());
        assert!(!check(&dir, "phone", Slot::Tls, None).allows());
    }
}
