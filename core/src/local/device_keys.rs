//! The keys paired devices prove (device keys, phase A of mutual auth).
//!
//! A paired phone used to be known only by the `deviceId` it announces,
//! which anyone who has seen it can repeat. Here each device is bound to the
//! keys it proves on a connection: its TLS client certificate on LAN QUIC,
//! and its iroh endpoint key on the WAN path. `<state>/device_keys.json` is
//! its own file: the trust store's writers rebuild that file from their
//! struct and would drop a field they do not know.
//!
//! The rules:
//! - [`check`] only reads. A session's key either matches, is refused, or
//!   would bind ([`Verdict::FirstUse`]); the caller binds it with
//!   [`bind_first`] only after the whole session gate has allowed the
//!   session (an owner who denies an impostor must not leave it bound).
//! - Only a LAN (TLS) key binds by first use, and only on a device with no
//!   key at all. A WAN key is never bound by first use - anyone on the
//!   internet who knows the phone's id could otherwise claim it there - it
//!   is [`register`]ed from a session the device's TLS key already proves.
//! - Writes take a lock file shared with every process (the server, the
//!   CLI and the desktop app change this file), and are atomic and synced.
//! - An unreadable store refuses rather than forgets; [`clear`] (pairing
//!   again, removing the device) sets it aside and starts afresh, which is
//!   the owner's way back.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::DEVICE_KEYS_FILE;
use crate::protocol::kdeconnect::StoreLock;

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
    fn slot(&self, slot: Slot) -> &Option<String> {
        match slot {
            Slot::Tls => &self.tls,
            Slot::Iroh => &self.iroh,
        }
    }

    fn slot_mut(&mut self, slot: Slot) -> &mut Option<String> {
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
    /// The device has no key; this LAN key would be bound to it, once the
    /// session is allowed ([`bind_first`]).
    FirstUse,
    /// Nothing bound and nothing to bind: a client from before device keys,
    /// or a WAN session from a device that has not proved a LAN key yet.
    Unbound,
    /// Not this device, and why.
    Refused(Refusal),
}

/// Why a claim was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A key other than the one the device proved.
    WrongKey,
    /// The device has proved a key, but none was presented.
    NoKey,
    /// A key on a connection the device never registered one for.
    NotRegistered,
    /// The store could not be read (it is not the claimant's fault).
    StoreUnreadable,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::WrongKey => "a different key than the one this device proved",
            Refusal::NoKey => "no key, but this device has proved one",
            Refusal::NotRegistered => "this device's key for this connection was never registered",
            Refusal::StoreUnreadable => "the device key store cannot be read",
        }
    }
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

/// Replace the file: a temp file of this writer's own, synced, renamed.
fn write(state_dir: &Path, devices: HashMap<String, DeviceKeys>) -> io::Result<()> {
    let json = serde_json::to_string_pretty(&KeysFile { devices }).map_err(io::Error::other)?;
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(DEVICE_KEYS_FILE);
    let tmp = state_dir.join(format!(
        "{DEVICE_KEYS_FILE}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    drop(file);
    crate::atomic_file::replace(&tmp, &path)
}

fn lock(state_dir: &Path) -> io::Result<StoreLock> {
    StoreLock::acquire(&state_dir.join(DEVICE_KEYS_FILE)).map_err(io::Error::other)
}

/// Judge a session that claims `device_id` and proved `presented` (or
/// nothing) on `slot`. Reads only.
pub fn check(state_dir: &Path, device_id: &str, slot: Slot, presented: Option<&str>) -> Verdict {
    let Ok(devices) = read(state_dir) else {
        return Verdict::Refused(Refusal::StoreUnreadable);
    };
    let keys = devices.get(device_id).cloned().unwrap_or_default();
    match (presented, keys.slot(slot)) {
        (Some(key), Some(known)) if known == key => Verdict::Matches,
        (Some(_), Some(_)) => Verdict::Refused(Refusal::WrongKey),
        (Some(_), None) if keys.is_empty() => match slot {
            Slot::Tls => Verdict::FirstUse,
            // Only a LAN key binds by first use; see the module doc.
            Slot::Iroh => Verdict::Unbound,
        },
        (Some(_), None) => Verdict::Refused(Refusal::NotRegistered),
        (None, _) if keys.is_empty() => Verdict::Unbound,
        (None, _) => Verdict::Refused(Refusal::NoKey),
    }
}

/// Bind `key` as `device_id`'s LAN key, if the device still has no key at
/// all. `Ok(true)` when it is (now) the device's key; `Ok(false)` when
/// another session bound a different one first.
pub fn bind_first(state_dir: &Path, device_id: &str, key: &str) -> io::Result<bool> {
    let _lock = lock(state_dir)?;
    let mut devices = read(state_dir)?;
    let keys = devices.entry(device_id.to_string()).or_default();
    if keys.tls.as_deref() == Some(key) {
        return Ok(true);
    }
    if !keys.is_empty() {
        return Ok(false);
    }
    keys.tls = Some(key.to_string());
    write(state_dir, devices)?;
    Ok(true)
}

/// Record `key` for `device_id` on `slot`, replacing what was there. Only
/// for a session already authenticated as that device by another key.
pub fn register(state_dir: &Path, device_id: &str, slot: Slot, key: &str) -> io::Result<()> {
    let _lock = lock(state_dir)?;
    let mut devices = read(state_dir)?;
    let entry = devices.entry(device_id.to_string()).or_default();
    if entry.slot(slot).as_deref() == Some(key) {
        return Ok(());
    }
    *entry.slot_mut(slot) = Some(key.to_string());
    write(state_dir, devices)
}

/// Forget `device_id`'s keys: it was removed, or paired again (perhaps from
/// a reinstalled phone, with new keys). A store that cannot be read is set
/// aside (as `device_keys.json.unreadable`) and started afresh: this is the
/// owner's own action, and it is the way back from a damaged file.
pub fn clear(state_dir: &Path, device_id: &str) -> io::Result<()> {
    let _lock = lock(state_dir)?;
    let mut devices = read_or_set_aside(state_dir)?;
    if devices.remove(device_id).is_some() || !state_dir.join(DEVICE_KEYS_FILE).exists() {
        write(state_dir, devices)?;
    }
    Ok(())
}

/// Make `tls_key` the device's only key (phase B: pairing with the owner's
/// PIN, inside TLS, proves it). The WAN key is dropped until a LAN session
/// that key authenticates registers one. Like [`clear`], this is the owner's
/// own action, so an unreadable store is set aside.
pub fn replace(state_dir: &Path, device_id: &str, tls_key: &str) -> io::Result<()> {
    let _lock = lock(state_dir)?;
    let mut devices = read_or_set_aside(state_dir)?;
    devices.insert(
        device_id.to_string(),
        DeviceKeys {
            tls: Some(tls_key.to_string()),
            iroh: None,
        },
    );
    write(state_dir, devices)
}

/// The store, or, if it cannot be read, an empty one with the damaged file
/// set aside as `device_keys.json.unreadable`. Only for the owner's own
/// actions (removing or pairing a device).
fn read_or_set_aside(state_dir: &Path) -> io::Result<HashMap<String, DeviceKeys>> {
    match read(state_dir) {
        Ok(devices) => Ok(devices),
        Err(_) => {
            std::fs::rename(
                state_dir.join(DEVICE_KEYS_FILE),
                state_dir.join(format!("{DEVICE_KEYS_FILE}.unreadable")),
            )?;
            Ok(HashMap::new())
        }
    }
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
    fn a_device_binds_its_first_lan_key_and_must_keep_it() {
        let dir = temp_dir("bind");
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("aa")),
            Verdict::FirstUse
        );
        assert!(read(&dir).unwrap().is_empty(), "check only reads");
        assert!(bind_first(&dir, "phone", "aa").unwrap());
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("aa")),
            Verdict::Matches
        );
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("bb")),
            Verdict::Refused(Refusal::WrongKey)
        );
        assert_eq!(
            check(&dir, "phone", Slot::Tls, None),
            Verdict::Refused(Refusal::NoKey)
        );
    }

    #[test]
    fn of_two_first_sessions_only_one_binds() {
        let dir = temp_dir("race");
        assert!(bind_first(&dir, "phone", "aa").unwrap());
        assert!(
            !bind_first(&dir, "phone", "bb").unwrap(),
            "another key came second"
        );
        assert!(
            bind_first(&dir, "phone", "aa").unwrap(),
            "the same key again is fine"
        );
    }

    #[test]
    fn a_client_without_keys_passes_only_while_its_device_has_none() {
        let dir = temp_dir("legacy");
        assert_eq!(check(&dir, "phone", Slot::Tls, None), Verdict::Unbound);
        assert!(read(&dir).unwrap().is_empty(), "nothing recorded");
    }

    #[test]
    fn a_wan_key_never_binds_by_first_use() {
        // A phone whose first session after the update rides the WAN must
        // still bind its LAN key next; and nobody on the internet may bind
        // their iroh key to a phone that has none yet.
        let dir = temp_dir("wanfirst");
        assert!(check(&dir, "phone", Slot::Iroh, Some("somebody")).allows());
        assert!(read(&dir).unwrap().is_empty(), "nothing bound from the WAN");
        assert!(
            check(&dir, "phone", Slot::Tls, Some("aa")).allows(),
            "the phone's LAN key still binds"
        );
    }

    #[test]
    fn a_wan_key_is_registered_from_an_authenticated_session() {
        let dir = temp_dir("second");
        bind_first(&dir, "phone", "aa").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Iroh, Some("attacker")),
            Verdict::Refused(Refusal::NotRegistered)
        );
        register(&dir, "phone", Slot::Iroh, "phone-endpoint").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Iroh, Some("phone-endpoint")),
            Verdict::Matches
        );
    }

    #[test]
    fn registering_the_same_key_again_writes_nothing() {
        let dir = temp_dir("same");
        register(&dir, "phone", Slot::Iroh, "e").unwrap();
        let file = dir.join(DEVICE_KEYS_FILE);
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        register(&dir, "phone", Slot::Iroh, "e").unwrap();
        assert_eq!(
            before,
            std::fs::metadata(&file).unwrap().modified().unwrap()
        );
    }

    #[test]
    fn pairing_again_lets_a_new_key_bind() {
        let dir = temp_dir("clear");
        bind_first(&dir, "phone", "old").unwrap();
        bind_first(&dir, "other", "other-key").unwrap();
        clear(&dir, "phone").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("new")),
            Verdict::FirstUse
        );
        assert_eq!(
            check(&dir, "other", Slot::Tls, Some("other-key")),
            Verdict::Matches,
            "untouched"
        );
    }

    #[test]
    fn pairing_with_the_owners_pin_makes_the_proved_key_the_only_one() {
        let dir = temp_dir("replace");
        bind_first(&dir, "phone", "old").unwrap();
        register(&dir, "phone", Slot::Iroh, "old-wan").unwrap();
        bind_first(&dir, "other", "other-key").unwrap();
        replace(&dir, "phone", "new").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("new")),
            Verdict::Matches
        );
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("old")),
            Verdict::Refused(Refusal::WrongKey)
        );
        assert_eq!(
            read(&dir).unwrap()["phone"].iroh,
            None,
            "the WAN key waits for an authenticated LAN session"
        );
        assert_eq!(
            check(&dir, "other", Slot::Tls, Some("other-key")),
            Verdict::Matches,
            "untouched"
        );
    }

    #[test]
    fn pairing_again_recovers_an_unreadable_store() {
        let dir = temp_dir("replace-corrupt");
        std::fs::write(dir.join(DEVICE_KEYS_FILE), "{not json").unwrap();
        replace(&dir, "phone", "new").unwrap();
        assert!(dir.join(format!("{DEVICE_KEYS_FILE}.unreadable")).exists());
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("new")),
            Verdict::Matches
        );
    }

    #[test]
    fn an_unreadable_store_refuses_until_the_owner_clears_it() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join(DEVICE_KEYS_FILE), "{not json").unwrap();
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("aa")),
            Verdict::Refused(Refusal::StoreUnreadable)
        );
        // Removing or re-pairing the phone is the way back.
        clear(&dir, "phone").unwrap();
        assert!(
            dir.join(format!("{DEVICE_KEYS_FILE}.unreadable")).exists(),
            "set aside"
        );
        assert_eq!(
            check(&dir, "phone", Slot::Tls, Some("aa")),
            Verdict::FirstUse
        );
    }
}
