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
    /// What it may do (3c).
    #[serde(default)]
    pub permissions: KdePermissions,
}

/// What a paired KDE Connect device may do (3c), narrower than a phone's:
/// the clipboard both ways, its battery, ringing it from here, and, only
/// when the owner turns it on, sending files here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KdePermissions {
    pub clipboard: bool,
    pub battery: bool,
    pub find: bool,
    pub files: bool,
    /// Its notifications shown here (N9), as a phone's are.
    pub notifications: bool,
}

impl Default for KdePermissions {
    fn default() -> Self {
        Self {
            clipboard: true,
            battery: true,
            find: true,
            files: false,
            // Sent only if the owner turned notification sync on in the
            // device's own KDE Connect app.
            notifications: true,
        }
    }
}

/// One of a paired KDE Connect device's switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KdePermission {
    Clipboard,
    Battery,
    Find,
    Files,
    Notifications,
}

impl KdePermissions {
    /// These, with `which` set to `on`.
    pub fn with(mut self, which: KdePermission, on: bool) -> Self {
        *match which {
            KdePermission::Clipboard => &mut self.clipboard,
            KdePermission::Battery => &mut self.battery,
            KdePermission::Find => &mut self.find,
            KdePermission::Files => &mut self.files,
            KdePermission::Notifications => &mut self.notifications,
        } = on;
        self
    }

    /// Whether `which` is on.
    pub fn allows(&self, which: KdePermission) -> bool {
        match which {
            KdePermission::Clipboard => self.clipboard,
            KdePermission::Battery => self.battery,
            KdePermission::Find => self.find,
            KdePermission::Files => self.files,
            KdePermission::Notifications => self.notifications,
        }
    }
}

/// What `device_id` may do, if it is paired.
pub fn permissions(state_dir: &Path, device_id: &str) -> Option<KdePermissions> {
    devices(state_dir)
        .into_iter()
        .find(|d| d.device_id == device_id)
        .map(|d| d.permissions)
}

/// Set one of a paired `device_id`'s switches (the server's side), the
/// others as they are; whether it is paired.
pub fn set_permission(
    state_dir: &Path,
    device_id: &str,
    which: KdePermission,
    on: bool,
) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = try_devices(state_dir)?;
    let Some(device) = devices.iter_mut().find(|d| d.device_id == device_id) else {
        return Ok(false);
    };
    device.permissions = device.permissions.with(which, on);
    write_private_json(
        &state_dir.join(KDECONNECT_DEVICES_FILE),
        &DevicesFile { devices },
    )?;
    Ok(true)
}

/// Set what a paired `device_id` may do (the server's side); whether it is
/// paired.
pub fn set_permissions(
    state_dir: &Path,
    device_id: &str,
    permissions: KdePermissions,
) -> io::Result<bool> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut devices = try_devices(state_dir)?;
    let Some(device) = devices.iter_mut().find(|d| d.device_id == device_id) else {
        return Ok(false);
    };
    device.permissions = permissions;
    write_private_json(
        &state_dir.join(KDECONNECT_DEVICES_FILE),
        &DevicesFile { devices },
    )?;
    Ok(true)
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
    /// This computer asked and the device accepted: it waits for the owner
    /// to confirm the code here (3b review), since a device answering for
    /// another could otherwise pair with no one comparing codes.
    #[serde(default)]
    pub accepted: bool,
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
    /// Allow a device's pairing request, naming the code the owner saw: an
    /// answer to another request (a device with the same id that asked
    /// since) is not taken (3b review).
    Allow {
        device_id: String,
        #[serde(default)]
        code: String,
    },
    /// Deny it.
    Deny {
        device_id: String,
        #[serde(default)]
        code: String,
    },
    /// Unpair a device.
    Unpair { device_id: String },
    /// Ping a paired, connected device.
    Ping { device_id: String },
    /// Ring it, to find it (3c).
    Ring { device_id: String },
    /// Turn one of its switches on or off (3c): one at a time, so two
    /// changes made close together never undo each other (3c review).
    Permission {
        device_id: String,
        permission: KdePermission,
        on: bool,
    },
}

/// One request, as a file in `kdeconnect_out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdeRequest {
    pub id: String,
    #[serde(flatten)]
    pub ask: KdeAsk,
    pub created_unix: u64,
    /// When, in milliseconds: the order of requests made in one second.
    #[serde(default)]
    pub created_ms: u64,
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
    crate::atomic_file::replace(&tmp, path)
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

/// The pairing with `device_id` that showed `code` is no longer waiting;
/// another for the device (a newer link's, 3b review) is left.
pub fn clear_pending(
    state_dir: &Path,
    device_id: &str,
    code: &str,
    now_unix: u64,
) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = pending(state_dir, now_unix);
    list.retain(|p| p.device_id != device_id || p.code != code);
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
        created_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(KDECONNECT_OUT_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &file)?;
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
    taken.sort_by_key(|r| (r.created_unix, r.created_ms));
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
            permissions: KdePermissions::default(),
        }
    }

    #[test]
    fn a_paired_device_may_do_little_by_default_and_the_owner_changes_it() {
        let dir = temp_dir("permissions");
        assert_eq!(permissions(&dir, "a"), None, "not paired");
        add(&dir, device("a", [1; 32])).unwrap();
        let given = permissions(&dir, "a").unwrap();
        assert!(given.clipboard && given.battery && given.find);
        assert!(!given.files, "files only when the owner turns it on");
        let files = KdePermissions {
            files: true,
            ..given
        };
        assert!(set_permissions(&dir, "a", files).unwrap());
        assert!(permissions(&dir, "a").unwrap().files);
        assert!(!set_permissions(&dir, "b", files).unwrap(), "not paired");
        // One switch at a time leaves the others as they are.
        assert!(set_permission(&dir, "a", KdePermission::Clipboard, false).unwrap());
        let now = permissions(&dir, "a").unwrap();
        assert!(!now.clipboard && now.files && now.battery);
        assert!(now.allows(KdePermission::Files));
        // A file from before 3c reads with the defaults.
        let old = r#"{"devices":[{"device_id":"c","name":"P","cert_hash":"00","paired_unix":1}]}"#;
        std::fs::write(dir.join(KDECONNECT_DEVICES_FILE), old).unwrap();
        assert_eq!(permissions(&dir, "c"), Some(KdePermissions::default()));
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
            accepted: false,
        };
        set_pending(&dir, entry("a", 100), 100).unwrap();
        set_pending(&dir, entry("a", 105), 105).unwrap();
        set_pending(&dir, entry("b", 106), 106).unwrap();
        assert_eq!(pending(&dir, 110).len(), 2);
        assert!(
            pending(&dir, 106 + PAIRING_TTL_SECS + 1).is_empty(),
            "too old"
        );
        clear_pending(&dir, "a", "00000000", 110).unwrap();
        assert_eq!(pending(&dir, 110).len(), 2, "another request's code");
        clear_pending(&dir, "a", "1ACBAC75", 110).unwrap();
        assert_eq!(pending(&dir, 110)[0].device_id, "b");
    }

    #[test]
    fn a_request_is_taken_once() {
        let dir = temp_dir("requests");
        let asked = request(
            &dir,
            KdeAsk::Allow {
                device_id: "a".to_string(),
                code: "1ACBAC75".to_string(),
            },
            100,
        )
        .unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        // A request from before the code was named reads with none.
        let old: KdeRequest =
            serde_json::from_str(r#"{"id":"x","ask":"allow","device_id":"a","created_unix":1}"#)
                .unwrap();
        assert_eq!(
            old.ask,
            KdeAsk::Allow {
                device_id: "a".to_string(),
                code: String::new()
            }
        );
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

    #[test]
    fn requests_made_in_one_second_come_in_the_order_made() {
        let dir = temp_dir("order");
        let switch = |on| KdeAsk::Permission {
            device_id: "a".to_string(),
            permission: KdePermission::Files,
            on,
        };
        for on in [true, false, true, false] {
            request(&dir, switch(on), 100).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        let taken: Vec<_> = take_all(&dir, 100).into_iter().map(|r| r.ask).collect();
        assert_eq!(
            taken,
            [switch(true), switch(false), switch(true), switch(false)]
        );
    }
}
