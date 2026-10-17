//! KDE Connect devices paired with this computer (3b), a pairing waiting
//! for the owner, and the desktop app's requests about them.
//!
//! - `<state>/kdeconnect_devices.json` (owner-only, written only by the
//!   server): each paired device's KDE id, name, the SHA-256 of the
//!   certificate it paired with (pinned), and when.
//! - `<state>/kdeconnect_pairing.json` (owner-only, the server's): the
//!   pairings waiting, each with the code both screens show.
//! - `<state>/kdeconnect_out/<id>.json` (the app's): pair with a device,
//!   allow or deny a device's request, unpair, ping; taken within 60 s.
//!
//! Apart from Pocketdesk's trust store: nothing here makes a phone paired.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{KDECONNECT_DEVICES_FILE, KDECONNECT_OUT_DIR, KDECONNECT_PAIRING_FILE};

/// How long a request waits for the server.
pub const REQUEST_TTL_SECS: u64 = 60;

/// How long a pairing waits for an answer, on either side.
pub const PAIRING_TTL_SECS: u64 = 30;

/// One paired KDE Connect device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdeDevice {
    pub device_id: String,
    pub name: String,
    /// Hex SHA-256 of the certificate it paired with: the only one taken
    /// from it afterwards.
    pub cert_hash: String,
    pub paired_unix: u64,
}

/// A pairing waiting: asked by the device (the owner answers here) or from
/// here (the device's owner answers there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub device_id: String,
    pub name: String,
    /// The code both screens show.
    pub code: String,
    pub asked_unix: u64,
    /// The device asked; else this computer did.
    pub from_device: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DevicesFile {
    #[serde(default)]
    devices: Vec<KdeDevice>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PendingFile {
    #[serde(default)]
    pending: Vec<Pending>,
}

/// What the app asks of the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum KdeAsk {
    /// Ask a device that is connected to pair.
    Pair { device_id: String },
    /// Allow a device's pairing request.
    Allow { device_id: String },
    /// Deny it.
    Deny { device_id: String },
    /// Unpair a device.
    Unpair { device_id: String },
    /// Ping a paired, connected device.
    Ping { device_id: String },
}

/// One request, as a file in `kdeconnect_out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdeRequest {
    pub id: String,
    #[serde(flatten)]
    pub ask: KdeAsk,
    pub created_unix: u64,
}

/// The server's writes, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// The paired devices. A missing file is none; one that does not read is
/// an error (never taken for "none paired").
pub fn try_devices(state_dir: &Path) -> io::Result<Vec<KdeDevice>> {
    match std::fs::read_to_string(state_dir.join(KDECONNECT_DEVICES_FILE)) {
        Ok(raw) => serde_json::from_str::<DevicesFile>(&raw)
            .map(|f| f.devices)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The paired devices; none when the file does not read.
pub fn devices(state_dir: &Path) -> Vec<KdeDevice> {
    try_devices(state_dir).unwrap_or_default()
}

/// The pin for `device_id`, when it is paired.
pub fn pinned(state_dir: &Path, device_id: &str) -> Option<[u8; 32]> {
    devices(state_dir)
        .into_iter()
        .find(|d| d.device_id == device_id)
        .and_then(|d| parse_hash(&d.cert_hash))
}

/// Pair `device`, replacing an earlier pairing of its id (the server's side).
pub fn add(state_dir: &Path, device: KdeDevice) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = try_devices(state_dir)?;
    devices.retain(|d| d.device_id != device.device_id);
    devices.push(device);
    write_private_json(
        &state_dir.join(KDECONNECT_DEVICES_FILE),
        &DevicesFile { devices },
    )
}

/// Unpair `device_id`; whether it was paired.
pub fn remove(state_dir: &Path, device_id: &str) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = try_devices(state_dir)?;
    let before = devices.len();
    devices.retain(|d| d.device_id != device_id);
    if devices.len() == before {
        return Ok(false);
    }
    write_private_json(
        &state_dir.join(KDECONNECT_DEVICES_FILE),
        &DevicesFile { devices },
    )?;
    Ok(true)
}

/// The pairings waiting, without those past [`PAIRING_TTL_SECS`].
pub fn pending(state_dir: &Path, now_unix: u64) -> Vec<Pending> {
    std::fs::read_to_string(state_dir.join(KDECONNECT_PAIRING_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<PendingFile>(&raw).ok())
        .map(|f| f.pending)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.asked_unix <= now_unix && now_unix - p.asked_unix <= PAIRING_TTL_SECS)
        .collect()
}

/// Note a pairing waiting (the server's side), in place of another for the
/// same device.
pub fn set_pending(state_dir: &Path, entry: Pending, now_unix: u64) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = pending(state_dir, now_unix);
    list.retain(|p| p.device_id != entry.device_id);
    list.push(entry);
    write_private_json(
        &state_dir.join(KDECONNECT_PAIRING_FILE),
        &PendingFile { pending: list },
    )
}

/// The pairing with `device_id` is no longer waiting.
pub fn clear_pending(state_dir: &Path, device_id: &str, now_unix: u64) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = pending(state_dir, now_unix);
    list.retain(|p| p.device_id != device_id);
    write_private_json(
        &state_dir.join(KDECONNECT_PAIRING_FILE),
        &PendingFile { pending: list },
    )
}

/// Leave a request for the server (the app's side).
pub fn request(state_dir: &Path, ask: KdeAsk, now_unix: u64) -> io::Result<KdeRequest> {
    let request = KdeRequest {
        id: uuid::Uuid::new_v4().to_string(),
        ask,
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(KDECONNECT_OUT_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first; a stale,
/// future, unreadable or misnamed one is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<KdeRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(KDECONNECT_OUT_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue;
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<KdeRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn request_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir
        .join(KDECONNECT_OUT_DIR)
        .join(format!("{id}.json")))
}

/// Hex SHA-256, as kept.
pub fn hash_hex(hash: [u8; 32]) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hash(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-kde-devices-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn device(id: &str, hash: [u8; 32]) -> KdeDevice {
        KdeDevice {
            device_id: id.to_string(),
            name: "Phone".to_string(),
            cert_hash: hash_hex(hash),
            paired_unix: 1,
        }
    }

    #[test]
    fn a_device_is_paired_with_its_certificate_and_unpaired() {
        let dir = temp_dir("pair");
        assert_eq!(pinned(&dir, "a"), None);
        add(&dir, device("a", [3; 32])).unwrap();
        add(&dir, device("b", [4; 32])).unwrap();
        add(&dir, device("a", [5; 32])).unwrap();
        assert_eq!(
            pinned(&dir, "a"),
            Some([5; 32]),
            "paired again, its new key"
        );
        assert_eq!(devices(&dir).len(), 2);
        assert!(remove(&dir, "a").unwrap());
        assert!(!remove(&dir, "a").unwrap());
        assert_eq!(pinned(&dir, "a"), None);
        std::fs::write(dir.join(KDECONNECT_DEVICES_FILE), "{bad").unwrap();
        assert!(
            add(&dir, device("c", [6; 32])).is_err(),
            "a bad file is never wiped"
        );
    }

    #[test]
    fn a_pairing_waits_its_time_and_one_per_device() {
        let dir = temp_dir("pending");
        let entry = |id: &str, at: u64| Pending {
            device_id: id.to_string(),
            name: "Phone".to_string(),
            code: "1ACBAC75".to_string(),
            asked_unix: at,
            from_device: true,
        };
        set_pending(&dir, entry("a", 100), 100).unwrap();
        set_pending(&dir, entry("a", 105), 105).unwrap();
        set_pending(&dir, entry("b", 106), 106).unwrap();
        assert_eq!(pending(&dir, 110).len(), 2);
        assert!(
            pending(&dir, 106 + PAIRING_TTL_SECS + 1).is_empty(),
            "too old"
        );
        clear_pending(&dir, "a", 110).unwrap();
        assert_eq!(pending(&dir, 110)[0].device_id, "b");
    }

    #[test]
    fn a_request_is_taken_once() {
        let dir = temp_dir("requests");
        let asked = request(
            &dir,
            KdeAsk::Allow {
                device_id: "a".to_string(),
            },
            100,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert!(take_all(&dir, 101).is_empty());
        request(
            &dir,
            KdeAsk::Ping {
                device_id: "a".to_string(),
            },
            100,
        )
        .unwrap();
        assert!(
            take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty(),
            "stale"
        );
    }
}
