use crate::error::Result;
use crate::protocol::permissions::DevicePermissions;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::PROTOCOL_VERSION;

/// Version of **Linux Link's own** control-channel packet extensions (the
/// `kdeconnect.linuxlink.*` family), tagged on the wire as `llVersion`.
///
/// Deliberately separate from `PROTOCOL_VERSION` (the KDE Connect *identity*
/// handshake version): KDE Connect's own doc is explicitly *not a spec* and
/// their types can drift without notice, so we version the packets WE own and
/// parse unfamiliar fields defensively instead of trusting them. Bump this only
/// for a change that alters the meaning of an existing `kdeconnect.linuxlink.*`
/// field — purely additive fields are forward-compatible by design (unknown
/// fields are ignored) and need no bump.
pub const LL_EXT_VERSION: u32 = 1;

/// Prefix identifying Linux Link's KDE-Connect-compatible control-channel
/// extensions. Every packet sent under this prefix is auto-tagged with
/// [`LL_EXT_VERSION`] by [`NetworkPacket::to_wire`].
pub const LINUXLINK_PACKET_PREFIX: &str = "kdeconnect.linuxlink.";

/// A KDE Connect network packet (JSON, newline-terminated on wire).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPacket {
    #[serde(rename = "type")]
    pub packet_type: String,
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub body: Value,
    #[serde(default)]
    pub payload_size: Option<u64>,
    /// Originating deviceId (KDE Connect convention), stamped by the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Id of the packet this one replies to (notification reply correlation).
    #[serde(default, rename = "replyId", skip_serializing_if = "Option::is_none")]
    pub reply_id: Option<String>,
    /// Version of *our* `kdeconnect.linuxlink.*` extension protocol (R4 D4).
    /// Auto-stamped by [`to_wire`] on every `kdeconnect.linuxlink.*` packet, so
    /// no construction site has to remember it; left absent on KDE Connect's
    /// native types and readable-but-ignored by peers that predate the field.
    #[serde(default, rename = "llVersion", skip_serializing_if = "Option::is_none")]
    pub ll_version: Option<u32>,
}

impl NetworkPacket {
    pub fn new(packet_type: impl Into<String>) -> Self {
        Self {
            packet_type: packet_type.into(),
            id: 0,
            body: Value::Null,
            payload_size: None,
            source: None,
            reply_id: None,
            ll_version: None,
        }
    }

    pub fn with_body(mut self, body: Value) -> Self {
        self.body = body;
        self
    }

    pub fn with_payload_size(mut self, size: u64) -> Self {
        self.payload_size = Some(size);
        self
    }

    /// Serialize to JSON bytes with a trailing newline (wire format).
    ///
    /// Any `kdeconnect.linuxlink.*` packet that has not already carried an
    /// explicit [`ll_version`] is tagged with [`LL_EXT_VERSION`] here — the one
    /// choke point every send path (server + bridge) goes through, so plugin
    /// code never has to set it.
    pub fn to_wire(&self) -> Result<Vec<u8>> {
        let mut pkt = self.clone();
        if pkt.ll_version.is_none() && pkt.packet_type.starts_with(LINUXLINK_PACKET_PREFIX) {
            pkt.ll_version = Some(LL_EXT_VERSION);
        }
        let mut bytes = serde_json::to_vec(&pkt)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Parse from a single wire-format line (trailing newline optional).
    pub fn from_wire(line: &str) -> Result<Self> {
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed.is_empty() {
            return Err(crate::error::LinuxLinkError::ProtocolError {
                detail: "empty packet line".to_string(),
            });
        }
        serde_json::from_str(trimmed).map_err(|e| crate::error::LinuxLinkError::Serialization {
            format: "JSON",
            detail: e.to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub protocol_version: u32,
    pub incoming_capabilities: Vec<String>,
    pub outgoing_capabilities: Vec<String>,
}

impl DeviceIdentity {
    pub fn new(device_id: impl Into<String>, device_name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            device_name: device_name.into(),
            device_type: "desktop".to_string(),
            protocol_version: PROTOCOL_VERSION,
            incoming_capabilities: Vec::new(),
            outgoing_capabilities: Vec::new(),
        }
    }

    pub fn as_identity_packet(&self) -> NetworkPacket {
        NetworkPacket::new("kdeconnect.identity").with_body(serde_json::json!({
            "deviceId": self.device_id,
            "deviceName": self.device_name,
            "deviceType": self.device_type,
            "protocolVersion": self.protocol_version,
            "incomingCapabilities": self.incoming_capabilities,
            "outgoingCapabilities": self.outgoing_capabilities,
        }))
    }
}

/// Trait for KDE Connect plugins.
///
/// Plugins declare capability strings and handle incoming packets asynchronously.
/// To send packets back to the peer, plugins use the `DeviceSender` passed at runtime.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn incoming_capabilities(&self) -> &'static [&'static str];
    fn outgoing_capabilities(&self) -> &'static [&'static str];

    /// Handle an incoming packet. `sender` can be used to reply to the peer.
    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()>;
}

/// Abstraction for sending packets back to the connected peer.
/// Implemented per-connection so plugins can reply without owning the socket.
#[async_trait::async_trait]
pub trait DeviceSender: Send + Sync {
    /// Get the unique ID of the connected device (e.g. IP address or UUID).
    fn device_id(&self) -> &str;

    /// Get a unique ID for this specific connection session.
    fn connection_id(&self) -> &str;

    async fn send_packet(&self, packet: &NetworkPacket) -> Result<()>;
}

/// Concrete sender that wraps the per-connection TCP write half.
pub struct TcpDeviceSender<W> {
    writer: Arc<Mutex<W>>,
    device_id: String,
    connection_id: String,
}

impl<W> Clone for TcpDeviceSender<W> {
    fn clone(&self) -> Self {
        Self {
            writer: self.writer.clone(),
            device_id: self.device_id.clone(),
            connection_id: self.connection_id.clone(),
        }
    }
}

impl<W> TcpDeviceSender<W>
where
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    pub fn new(writer: W, device_id: String) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
            device_id,
            connection_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn from_arc(writer: Arc<Mutex<W>>, device_id: String) -> Self {
        Self {
            writer,
            device_id,
            connection_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    pub fn with_connection_id(mut self, id: String) -> Self {
        self.connection_id = id;
        self
    }
}

#[async_trait::async_trait]
impl<W> DeviceSender for TcpDeviceSender<W>
where
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    fn device_id(&self) -> &str {
        &self.device_id
    }

    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut packet = packet.clone();
        if packet.source.is_none() {
            packet.source = Some(self.device_id.clone());
        }
        let bytes = packet.to_wire()?;
        let mut guard = self.writer.lock().await;
        guard.write_all(&bytes).await?;
        guard.flush().await?;
        Ok(())
    }
}

#[derive(Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, Arc<dyn Plugin>>,
    incoming_map: HashMap<String, Vec<String>>,
}

impl Clone for PluginRegistry {
    fn clone(&self) -> Self {
        Self {
            plugins: self.plugins.clone(),
            incoming_map: self.incoming_map.clone(),
        }
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clone the registry for sharing across tasks via Arc.
    pub fn clone_for_dispatch(&self) -> Self {
        self.clone()
    }

    pub fn register<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
    {
        let plugin_arc: Arc<dyn Plugin> = Arc::new(plugin);
        let plugin_name = plugin_arc.name().to_string();

        for packet_type in plugin_arc.incoming_capabilities() {
            self.incoming_map
                .entry((*packet_type).to_string())
                .or_default()
                .push(plugin_name.clone());
        }

        self.plugins.insert(plugin_name, plugin_arc);
    }

    pub fn plugin_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.plugins.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn plugins_for_packet(&self, packet_type: &str) -> Vec<String> {
        self.incoming_map
            .get(packet_type)
            .cloned()
            .unwrap_or_default()
    }

    pub fn capability_sets(&self) -> (Vec<String>, Vec<String>) {
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();

        for plugin in self.plugins.values() {
            incoming.extend(
                plugin
                    .incoming_capabilities()
                    .iter()
                    .map(|v| (*v).to_string()),
            );
            outgoing.extend(
                plugin
                    .outgoing_capabilities()
                    .iter()
                    .map(|v| (*v).to_string()),
            );
        }

        incoming.sort();
        incoming.dedup();
        outgoing.sort();
        outgoing.dedup();

        (incoming, outgoing)
    }

    /// Dispatch an incoming packet to all registered plugins that handle this packet type.
    /// Errors from individual plugins are logged but don't stop dispatch to others.
    pub async fn dispatch_packet(
        &self,
        packet: &NetworkPacket,
        sender: &dyn DeviceSender,
    ) -> Result<()> {
        let plugin_names = self.plugins_for_packet(&packet.packet_type);

        if plugin_names.is_empty() {
            tracing::debug!("no plugin handles packet type: {}", packet.packet_type);
            return Ok(());
        }

        for name in &plugin_names {
            if let Some(plugin) = self.plugins.get(name)
                && let Err(e) = plugin.handle_packet(packet, sender).await
            {
                tracing::warn!("plugin '{}' failed to handle packet: {}", name, e);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
    trusted_device_ids: HashSet<String>,
    /// R4 D3: deviceId -> unix-seconds expiry for *time-boxed* trust
    /// (one-off support grants from `linux-link pair --grant 15m`).
    /// Entries here are trusted only until they expire; expiry is applied
    /// lazily at load, so no scheduler is required anywhere.
    grants: HashMap<String, u64>,
    /// F5: deviceId -> what it may do. Only devices whose permissions differ
    /// from the defaults have an entry.
    permissions: HashMap<String, DevicePermissions>,
}

fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl TrustStore {
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            let empty = TrustStoreFile::default();
            std::fs::write(&path, serde_json::to_vec_pretty(&empty)?)
                .with_context(|| format!("failed to initialize {}", path.display()))?;
            return Ok(Self {
                path,
                trusted_device_ids: HashSet::new(),
                grants: HashMap::new(),
                permissions: HashMap::new(),
            });
        }

        let bytes =
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let decoded: TrustStoreFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let now = unix_now_secs();

        Ok(Self {
            path,
            trusted_device_ids: decoded.trusted_device_ids.into_iter().collect(),
            // Load-time GC: an expired grant is simply not loaded, and the
            // next persist drops it from the file.
            grants: decoded
                .grants
                .into_iter()
                .filter(|(_, expires)| *expires > now)
                .collect(),
            permissions: decoded.permissions,
        })
    }

    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.trusted_device_ids.contains(device_id) || self.grants.contains_key(device_id)
    }

    pub fn trust_device(&mut self, device_id: impl Into<String>) -> Result<()> {
        let id = device_id.into();
        self.grants.remove(&id);
        self.trusted_device_ids.insert(id);
        self.persist()
    }

    /// Trust a device until `ttl` has elapsed (R4 D3). A device that is
    /// already permanently trusted stays permanent — a scoped grant can
    /// never *demote* existing trust.
    pub fn trust_device_with_ttl(
        &mut self,
        device_id: impl Into<String>,
        ttl: std::time::Duration,
    ) -> Result<()> {
        let id = device_id.into();
        if self.trusted_device_ids.contains(&id) {
            return Ok(());
        }
        self.grants
            .insert(id, unix_now_secs() + ttl.as_secs().max(1));
        self.persist()
    }

    pub fn untrust_device(&mut self, device_id: &str) -> Result<()> {
        self.trusted_device_ids.remove(device_id);
        self.grants.remove(device_id);
        // A forgotten device that pairs again starts from the defaults.
        self.permissions.remove(device_id);
        self.persist()
    }

    pub fn trusted_devices(&self) -> Vec<String> {
        let mut values: Vec<String> = self.trusted_device_ids.iter().cloned().collect();
        values.extend(self.grants.keys().cloned());
        values.sort();
        values.dedup();
        values
    }

    /// What [device_id] may do (F5). A device without an entry has the defaults.
    pub fn permissions(&self, device_id: &str) -> DevicePermissions {
        self.permissions.get(device_id).cloned().unwrap_or_default()
    }

    pub fn set_permissions(
        &mut self,
        device_id: &str,
        permissions: DevicePermissions,
    ) -> Result<()> {
        if permissions.is_default() {
            self.permissions.remove(device_id);
        } else {
            self.permissions.insert(device_id.to_string(), permissions);
        }
        self.persist()
    }

    /// Seconds remaining on a device's time-boxed grant (`None` = no grant:
    /// either permanently trusted or not trusted at all).
    pub fn grant_remaining(&self, device_id: &str) -> Option<u64> {
        self.grants
            .get(device_id)
            .map(|expires| expires.saturating_sub(unix_now_secs()))
    }

    fn persist(&self) -> Result<()> {
        let mut permanent: Vec<String> = self.trusted_device_ids.iter().cloned().collect();
        permanent.sort();
        let payload = TrustStoreFile {
            trusted_device_ids: permanent,
            grants: self.grants.clone(),
            permissions: self.permissions.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&payload)?;
        // Whole-file replace: the server and the desktop app both write this
        // file, and a reader must never see one of them halfway through.
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        std::fs::write(&tmp, bytes)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("failed to replace {}", self.path.display()))?;
        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStoreFile {
    #[serde(default)]
    trusted_device_ids: Vec<String>,
    #[serde(default)]
    grants: HashMap<String, u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    permissions: HashMap<String, DevicePermissions>,
}

#[derive(Default)]
pub struct KdeConnectService {
    pub identity: Option<DeviceIdentity>,
    pub registry: PluginRegistry,
    pub trust_store: Option<TrustStore>,
}

impl KdeConnectService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_identity(&mut self, identity: DeviceIdentity) {
        self.identity = Some(identity);
    }

    pub fn set_trust_store(&mut self, store: TrustStore) {
        self.trust_store = Some(store);
    }

    pub fn identity_packet(&self) -> Option<NetworkPacket> {
        self.identity
            .as_ref()
            .map(DeviceIdentity::as_identity_packet)
    }

    pub fn register_plugin<P>(&mut self, plugin: P)
    where
        P: Plugin + 'static,
    {
        self.registry.register(plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server and the desktop app both write the trust store; a save must
    /// replace the file whole (temp + rename) rather than rewrite it in place,
    /// or the other side can read it half-written. A hard link to the old
    /// file tells the two apart: a rename leaves the link on the old bytes,
    /// an in-place write changes them under it.
    #[test]
    fn trust_store_saves_replace_the_file_instead_of_rewriting_it() {
        let dir = std::env::temp_dir().join(format!("ll-trust-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trusted_devices.json");
        let mut store = TrustStore::load_or_create(&path).unwrap();
        store.trust_device("first").unwrap();
        let link = dir.join("before.json");
        std::fs::hard_link(&path, &link).unwrap();
        store.trust_device("second").unwrap();
        let before = std::fs::read_to_string(&link).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("second"));
        assert!(
            !before.contains("second"),
            "the save rewrote the file in place"
        );
        assert!(!dir.join("trusted_devices.json.tmp").exists());
    }

    struct ClipboardPlugin;

    #[async_trait::async_trait]
    impl Plugin for ClipboardPlugin {
        fn name(&self) -> &'static str {
            "clipboard"
        }

        fn incoming_capabilities(&self) -> &'static [&'static str] {
            &["kdeconnect.clipboard"]
        }

        fn outgoing_capabilities(&self) -> &'static [&'static str] {
            &["kdeconnect.clipboard"]
        }

        async fn handle_packet(
            &self,
            _packet: &NetworkPacket,
            _sender: &dyn DeviceSender,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_indexes_plugin_capabilities() {
        let mut registry = PluginRegistry::new();
        registry.register(ClipboardPlugin);

        assert_eq!(registry.plugin_names(), vec!["clipboard".to_string()]);
        assert_eq!(
            registry.plugins_for_packet("kdeconnect.clipboard"),
            vec!["clipboard".to_string()]
        );
    }

    #[test]
    fn trust_store_roundtrip_persists_devices() {
        let unique = format!(
            "linux-link-trust-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);

        let mut store = TrustStore::load_or_create(&path).expect("create trust store");
        store
            .trust_device("device-a")
            .expect("write trusted device");

        let reloaded = TrustStore::load_or_create(&path).expect("reload trust store");
        assert!(reloaded.is_trusted("device-a"));

        let _ = std::fs::remove_file(path);
    }

    fn temp_store_path(tag: &str) -> PathBuf {
        let unique = format!(
            "linux-link-trust-{}-{}-{}.json",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn permissions_are_stored_per_device_and_go_with_it() {
        use crate::protocol::permissions::Capability;
        let path = temp_store_path("perms");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        store.trust_device("phone-p").unwrap();
        assert!(store.permissions("phone-p").is_default());

        let mut permissions = store.permissions("phone-p");
        permissions.set(Capability::Clipboard, false);
        permissions.ask_first = true;
        store
            .set_permissions("phone-p", permissions.clone())
            .unwrap();

        let reloaded = TrustStore::load_or_create(&path).expect("reload");
        assert_eq!(reloaded.permissions("phone-p"), permissions);
        assert!(
            !reloaded
                .permissions("phone-p")
                .allows(Capability::Clipboard)
        );

        // Forgetting a device forgets its permissions; re-pairing starts clean.
        let mut store = reloaded;
        store.untrust_device("phone-p").unwrap();
        store.trust_device("phone-p").unwrap();
        assert!(store.permissions("phone-p").is_default());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_file_without_permissions_loads_and_defaults_are_not_written() {
        let path = temp_store_path("perms-old");
        std::fs::write(&path, r#"{"trusted_device_ids":["phone-o"],"grants":{}}"#).unwrap();
        let mut store = TrustStore::load_or_create(&path).expect("old file");
        assert!(store.permissions("phone-o").is_default());
        store
            .set_permissions(
                "phone-o",
                crate::protocol::permissions::DevicePermissions::default(),
            )
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("permissions"), "{raw}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn grant_is_trusted_until_expiry() {
        let path = temp_store_path("grant");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        store
            .trust_device_with_ttl("phone-x", std::time::Duration::from_secs(60))
            .expect("grant");
        assert!(store.is_trusted("phone-x"));
        assert!(store.grant_remaining("phone-x").unwrap() > 0);

        // Survives a reload while live.
        let reloaded = TrustStore::load_or_create(&path).expect("reload");
        assert!(reloaded.is_trusted("phone-x"));

        // An already-expired grant is dropped at load (lazy expiry).
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let mut expired = raw.as_object().unwrap().clone();
        let grants = expired["grants"].as_object().unwrap().clone();
        let (id, _) = grants.iter().next().unwrap();
        expired.insert(
            "grants".into(),
            serde_json::json!({ id: unix_now_secs() - 1 }),
        );
        std::fs::write(&path, serde_json::to_vec_pretty(&expired).unwrap()).unwrap();
        let stale = TrustStore::load_or_create(&path).expect("reload expired");
        assert!(!stale.is_trusted("phone-x"));
        assert!(stale.trusted_devices().is_empty());

        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn permanent_trust_wins_over_grant_and_clears_it() {
        let path = temp_store_path("permanent");
        let mut store = TrustStore::load_or_create(&path).expect("create");
        store
            .trust_device_with_ttl("phone-y", std::time::Duration::from_secs(60))
            .expect("grant");
        // Granting a device that is permanently trusted is a no-op...
        store.trust_device("phone-y").expect("trust permanent");
        store
            .trust_device_with_ttl("phone-y", std::time::Duration::from_secs(60))
            .expect("grant on permanent");
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(file["grants"].as_object().unwrap().is_empty());
        assert!(
            file["trusted_device_ids"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("phone-y"))
        );
        // ...and untrust removes from both.
        store.untrust_device("phone-y").expect("untrust");
        assert!(!store.is_trusted("phone-y"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn legacy_store_file_without_grants_loads() {
        // Pre-D3 on-disk shape: only trusted_device_ids.
        let path = temp_store_path("legacy");
        std::fs::write(&path, br#"{"trusted_device_ids": ["old-phone"]}"#).unwrap();
        let store = TrustStore::load_or_create(&path).expect("load legacy");
        assert!(store.is_trusted("old-phone"));
        assert_eq!(store.grant_remaining("old-phone"), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn linuxlink_packets_are_version_tagged() {
        let pkt = NetworkPacket::new("kdeconnect.linuxlink.privacy")
            .with_body(serde_json::json!({ "action": "status" }));
        let wire = String::from_utf8(pkt.to_wire().unwrap()).unwrap();
        let parsed: Value = serde_json::from_str(wire.trim()).unwrap();
        assert_eq!(parsed["llVersion"], LL_EXT_VERSION);
        // Round-trips back into the typed field, not just raw JSON.
        let back = NetworkPacket::from_wire(&wire).unwrap();
        assert_eq!(back.ll_version, Some(LL_EXT_VERSION));
    }

    #[test]
    fn native_kde_packets_are_not_tagged() {
        // KDE Connect's own types carry their upstream protocolVersion inside
        // the identity body — we must not graft our llVersion onto them.
        let pkt =
            NetworkPacket::new("kdeconnect.pair").with_body(serde_json::json!({ "pin": "1" }));
        let wire = String::from_utf8(pkt.to_wire().unwrap()).unwrap();
        let parsed: Value = serde_json::from_str(wire.trim()).unwrap();
        assert!(
            parsed.get("llVersion").is_none(),
            "native type must stay untagged"
        );
    }

    #[test]
    fn explicit_ll_version_is_not_overwritten() {
        let mut pkt = NetworkPacket::new("kdeconnect.linuxlink.endpoint");
        pkt.ll_version = Some(99);
        let wire = String::from_utf8(pkt.to_wire().unwrap()).unwrap();
        let parsed: Value = serde_json::from_str(wire.trim()).unwrap();
        assert_eq!(parsed["llVersion"], 99);
    }

    #[test]
    fn unknown_wire_fields_parse_defensively() {
        // The core D4 guarantee: a peer from a future revision (extra fields,
        // even an extra top-level one we do not model) still parses, and the
        // fields we know keep their values.
        let future = r#"{"type":"kdeconnect.linuxlink.privacy","id":7,"llVersion":2,"body":{"ok":true},"futureTopLevel":{"nested":1}}"#;
        let pkt = NetworkPacket::from_wire(future).unwrap();
        assert_eq!(pkt.packet_type, "kdeconnect.linuxlink.privacy");
        assert_eq!(pkt.ll_version, Some(2));
        assert_eq!(pkt.body["ok"], true);
    }
}
