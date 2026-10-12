//! Client session API ported from the Flutter-era bridge (git 4bf3c73).
//! Plain polling functions; JNI wrappers live in lib.rs.

use linux_link_core::protocol::connection::ConnectionManager;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TcpDeviceSender};
use linux_link_core::protocol::v2::{ALPN_V2, IdentityPacketV2, perform_v2_handshake};
use linux_link_core::streaming::QuinnConnection;
use linux_link_core::streaming::StreamingClient;
use linux_link_core::tailscale::TailscaleClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::sync::broadcast;

use linux_link_core::streaming::InputPacket;
use linux_link_core::streaming::{SAMPLE_DECODE, SAMPLE_RENDER, report_samples};

use crate::{
    CONNECTION_STATE, CONTROL_PEER, CONTROL_WRITER, DECODE_SAMPLES, MAX_AUDIO_PACKETS_PER_RECEIVE,
    MAX_FRAMES_PER_RECEIVE, RATE_SNAPSHOTS, RENDER_SAMPLES, RTT_HISTORY, STREAMING_ACTIVE,
    STREAMING_BYTE_COUNT, STREAMING_FRAME_COUNT, STREAMING_HANDLE, StreamingHandle,
    update_streaming_rtt,
};

/// Initialize the Linux Link backend
pub fn init_app() {
    crate::init_app_impl();
}

/// Set the persistent data directory for certs and state.
/// Must be called before `connect_streaming` for cert persistence.
pub fn set_data_dir(path: String) {
    let cert_path = PathBuf::from(&path).join("linux-link").join("certs");
    let mut guard = crate::CERT_DIR.lock().unwrap();
    *guard = Some(cert_path);
    tracing::info!("Data directory set to: {path}");
}

/// Pin a desktop's certificate before the first streaming connection to
/// `address`, from a scanned pairing code (`fp`: hex SHA-256 of its
/// certificate), so that connection is verified rather than trusted on first
/// use. Only the QUIC streaming channel is covered: the control channel the
/// PIN rides is not encrypted. Refused when a different certificate is
/// already trusted for the address (see below).
pub fn pin_peer_certificate(address: String, fingerprint_hex: String) -> Result<(), String> {
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    pin_peer_certificate_in(&dir, &address, &fingerprint_hex)
}

pub(crate) fn pin_peer_certificate_in(
    cert_dir: &std::path::Path,
    address: &str,
    fingerprint_hex: &str,
) -> Result<(), String> {
    let ip: std::net::IpAddr = address
        .trim()
        .parse()
        .map_err(|_| format!("not an IP address: {address}"))?;
    let hex = fingerprint_hex.trim();
    if hex.len() != 64 {
        return Err("a certificate fingerprint is 64 hex digits".to_string());
    }
    let mut hash = [0u8; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "the fingerprint is not hex".to_string())?;
    }
    let manager = linux_link_core::streaming::transport::CertManager::load_or_create(cert_dir)
        .map_err(|e| e.to_string())?;
    let label = linux_link_core::streaming::transport::peer_label_for_ip(ip);
    // Anything can open a pairing link, so one must never replace the
    // certificate this phone already trusts for an address: a different
    // fingerprint there is a different computer. Removing the computer in
    // the app clears the old trust first.
    if manager
        .known_peers()
        .iter()
        .any(|(known, pinned)| *known == label && *pinned != hash)
    {
        return Err(format!(
            "this phone already trusts a different computer at {ip}; remove it first"
        ));
    }
    manager.pin_peer(&label, hash).map_err(|e| e.to_string())?;
    tracing::info!("Pinned the certificate of {ip} from a pairing code");
    Ok(())
}

/// Forget the certificate this phone trusts for `address` (the computer was
/// removed). Returns whether one was pinned.
pub fn forget_peer_certificate(address: String) -> Result<bool, String> {
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    forget_peer_certificate_in(&dir, &address)
}

pub(crate) fn forget_peer_certificate_in(
    cert_dir: &std::path::Path,
    address: &str,
) -> Result<bool, String> {
    let Ok(ip) = address.trim().parse::<std::net::IpAddr>() else {
        // Hosts saved by name were never pinned by IP.
        return Ok(false);
    };
    let manager = linux_link_core::streaming::transport::CertManager::load_or_create(cert_dir)
        .map_err(|e| e.to_string())?;
    let removed = manager.remove_peer(&linux_link_core::streaming::transport::peer_label_for_ip(
        ip,
    ));
    if removed {
        manager.save_known_peers().map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

/// Check if the LAN discovery service is active.
pub fn is_discovery_active() -> bool {
    crate::MDNS_ACTIVE.load(Ordering::Acquire)
}

/// Get version string
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

use linux_link_core::error::LinuxLinkError;

/// Structured error information for the client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LinuxLinkErrorDto {
    pub code: u32,
    pub message: String,
    pub is_retryable: bool,
}

impl From<LinuxLinkError> for LinuxLinkErrorDto {
    fn from(e: LinuxLinkError) -> Self {
        Self {
            code: e.code() as u32,
            message: e.to_string(),
            is_retryable: e.is_retryable(),
        }
    }
}

/// High-level session status for reconnection state machine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SessionStatus {
    Disconnected,
    Connecting,
    Active,
    /// Connection is alive but RTT is extremely high or packets are being lost.
    Stale {
        rtt_ms: u64,
    },
    /// Attempting to automatically recover a lost connection.
    Reconnecting {
        attempt: u32,
        next_retry_ms: u64,
    },
    /// A fatal error occurred that cannot be automatically recovered.
    Error(LinuxLinkErrorDto),
}

/// Connection state enumeration for the client (legacy control channel state).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connected,
    Disconnected,
    Connecting,
    Error(LinuxLinkErrorDto),
}

/// Peer information for display in the client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerInfoDto {
    pub name: String,
    pub dns_name: String,
    pub ips: Vec<String>,
    pub online: bool,
}

/// Frame data transfer object for the client/MediaCodec.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FrameDto {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub sequence: u64,
}

/// Remote file metadata for the file browser.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoteFileDto {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified: u64,
}
/// Streaming statistics for display in the client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamingStatsDto {
    pub fps: f64,
    pub bitrate_kbps: u64,
    /// Milliseconds of received video the two rates above are divided by — the
    /// sample basis of a number that used to be a lifetime average. Short right
    /// after a connect, and the display is expected to say so rather than quote
    /// it as a steady state (roadmap 2056).
    pub rate_window_ms: u64,
    /// How many one-second RTT polls `get_streaming_rtt` summarised.
    pub rtt_samples: usize,
    pub e2e_latency_ms: u64,
    /// Video frames the link never delivered, measured on the client's own
    /// sequence window — not a placeholder.
    pub frame_drops: u64,
    /// Live link path (R4 A1): "lan" | "wan_direct" | "wan_relayed" |
    /// "wan" (punched/relay not yet observable) | "none" (no session).
    pub link_state: &'static str,
}

/// Monitor information for display in Flutter
#[derive(serde::Deserialize, serde::Serialize)]
pub struct MonitorInfoDto {
    pub index: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

/// Get or create the persistent device identity for this Android client.
fn client_identity() -> linux_link_core::protocol::kdeconnect::DeviceIdentity {
    let mut guard = crate::DEVICE_IDENTITY.lock().unwrap();
    if let Some(id) = &*guard {
        return id.clone();
    }

    // Try to load from disk
    let id_path = {
        let dir_guard = crate::CERT_DIR.lock().unwrap();
        dir_guard.as_ref().map(|d| d.join("device_id"))
    };

    if let Some(ref path) = id_path
        && let Ok(data) = std::fs::read_to_string(path)
    {
        let trimmed = data.trim().to_string();
        if !trimmed.is_empty() {
            let identity = linux_link_core::protocol::kdeconnect::DeviceIdentity::new(
                &trimmed,
                "Linux Link Android Client",
            );
            *guard = Some(identity.clone());
            return identity;
        }
    }

    // Generate and persist new ID
    let new_id = uuid::Uuid::new_v4().to_string();
    if let Some(path) = id_path {
        let _ = std::fs::write(&path, &new_id);
    }

    let identity = linux_link_core::protocol::kdeconnect::DeviceIdentity::new(
        &new_id,
        "Linux Link Android Client",
    );
    *guard = Some(identity.clone());
    identity
}

/// Send Wake-on-LAN magic packet to wake a sleeping peer.
pub fn send_wol(mac_address: String, broadcast_addr: String) -> Result<(), String> {
    linux_link_core::tailscale::wol::send_wol(&mac_address, &broadcast_addr)
        .map_err(|e| e.to_string())
}

/// Ask an always-on LAN peer (the relay) to emit the WoL magic packet for a
/// sleeping target (R3 Tier-2 #12). Broadcast UDP can't cross the wire, so
/// from WAN the phone wakes the desktop through this path instead. Fire-
/// and-forget like find-my-device — not even the relay can confirm the
/// target powered on. An empty `broadcast` uses the relay's default
/// (`255.255.255.255`); pass the directed subnet broadcast for reliability.
pub async fn wake_via_relay(
    address: String,
    port: u16,
    mac: String,
    broadcast: String,
) -> Result<(), String> {
    let conn_mgr = ConnectionManager::new(Duration::from_secs(5));
    let identity = client_identity();
    let stream = conn_mgr
        .connect(&address, port, &identity)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;
    let (_reader, writer) = tokio::io::split(stream);
    let sender = TcpDeviceSender::new(writer, address);
    let request = NetworkPacket::new("kdeconnect.linuxlink.wol").with_body(serde_json::json!({
        "mac": mac,
        "broadcast": broadcast,
    }));
    sender
        .send_packet(&request)
        .await
        .map_err(|e| format!("Failed to send wake request: {e}"))
}

/// Check Tailscale status
pub async fn check_tailscale_status() -> Result<bool, String> {
    let client = TailscaleClient::new().map_err(|e| e.to_string())?;
    match tokio::time::timeout(
        Duration::from_secs(5),
        client.wait_for_ready(Duration::from_secs(3)),
    )
    .await
    {
        Ok(Ok(_)) => Ok(true),
        _ => Ok(false),
    }
}

/// Get list of peers on the tailnet and LAN
pub async fn get_peers() -> Result<Vec<PeerInfoDto>, String> {
    let mut all_peers = Vec::new();

    // 1. Get Tailscale peers
    if let Ok(client) = TailscaleClient::new()
        && let Ok(peers) = client.get_peers().await
    {
        all_peers.extend(peers);
    }

    // 2. Get LAN peers via mDNS
    crate::MDNS_ACTIVE.store(true, Ordering::Release);
    let lan_peers = linux_link_core::tailscale::lan::scan_lan_once().await;
    crate::MDNS_ACTIVE.store(false, Ordering::Release);
    for lp in lan_peers {
        // Avoid duplicates if a peer is found on both Tailscale and LAN
        if !all_peers.iter().any(|p| p.name == lp.name) {
            all_peers.push(lp);
        }
    }

    Ok(all_peers
        .into_iter()
        .map(|p| PeerInfoDto {
            name: p.name,
            dns_name: p.dns_name,
            ips: p.ips,
            online: p.online,
        })
        .collect())
}

/// Connect to a peer
pub async fn connect_to_peer(address: String, port: u16) -> Result<ConnectionState, String> {
    // Kotlin calls this from several poll loops (clipboard sync, quick
    // settings, retry paths) without coordinating. A second call while the
    // first connection is still up would tear down a working control session
    // and re-register the same device_id, which the server reads as a
    // reconnect storm. Reuse what is already connected.
    {
        // Locked in the same order as the control reader (writer, then peer) so
        // the two can never dead-lock against each other.
        let writer_live = (*CONTROL_WRITER).lock().await.is_some();
        let live = (*CONTROL_PEER).lock().await;
        if writer_live
            && live
                .as_ref()
                .is_some_and(|(host, live_port)| *host == address && *live_port == port)
        {
            return Ok(ConnectionState::Connected);
        }
    }

    let mut state_guard = (*CONNECTION_STATE).lock().await;
    *state_guard = ConnectionState::Connecting;

    let conn_mgr = ConnectionManager::new(Duration::from_secs(10));
    let identity = client_identity();

    let conn_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "conn",
        id = %conn_id,
        session = %session_id,
        peer = %address,
        transport = "tcp",
        proto = %linux_link_core::protocol::PROTOCOL_VERSION
    );

    use tracing::Instrument;

    let result = async move {
        match conn_mgr.connect(&address, port, &identity).await {
            Ok(stream) => {
                let (reader, writer) = stream.into_split();
                let writer_arc = Arc::new(Mutex::new(writer));
                {
                    let mut writer_guard = (*CONTROL_WRITER).lock().await;
                    *writer_guard = Some(writer_arc.clone());
                }
                *crate::CONTROL_PEER.lock().await = Some((address.clone(), port));

                // Create a broadcast channel for forwarding incoming packets to Flutter
                let (packet_tx, _) = broadcast::channel(256);
                {
                    let mut incoming = crate::INCOMING_PACKETS.lock().await;
                    *incoming = Some(packet_tx.clone());
                }

                tokio::spawn(
                    async move {
                        let mut reader = tokio::io::BufReader::new(reader);
                        let mut line = String::new();
                        while let Ok(n) = reader.read_line(&mut line).await {
                            if n == 0 {
                                break;
                            }
                            let trimmed = line.trim().to_string();
                            if !trimmed.is_empty() {
                                // Cache the server's iroh WAN identity as it is
                                // announced, independent of Kotlin's poll loop.
                                if trimmed.contains("linuxlink.endpoint")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.linuxlink.endpoint"
                                {
                                    let mut id = crate::WAN_IDENTITY.lock().await;
                                    *id = Some(pkt.body.to_string());
                                }
                                // Find-my-device siren: latch so the UI can ring
                                // even if the poll loop misses the packet.
                                if trimmed.contains("findmydevice")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.findmydevice"
                                    && pkt
                                        .body
                                        .get("ring")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(true)
                                {
                                    crate::SIREN_RINGING
                                        .store(true, std::sync::atomic::Ordering::SeqCst);
                                }
                                // Pairing decisions/announcements from the
                                // desktop latch for the pairing UI (Tier-2 #11b).
                                if trimmed.contains("kdeconnect.pair")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && (pkt.packet_type == "kdeconnect.pair"
                                        || pkt.packet_type == "kdeconnect.linuxlink.pair")
                                {
                                    let mut slot = crate::PAIR_RESULT.lock().await;
                                    *slot = Some(pkt.body.clone());
                                    // A packet carrying the PIN itself must not
                                    // reach Kotlin pollers — the latch consumed it.
                                    if pkt.body.get("pin").is_some() {
                                        line.clear();
                                        continue;
                                    }
                                }
                                // Desktop notifications addressed to us queue
                                // for the notification-posting poller (Tier-2 #11c).
                                if trimmed.contains("kdeconnect.notification")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.notification"
                                    && pkt.source.as_deref() != Some(&client_identity().device_id)
                                    && !pkt
                                        .body
                                        .get("isClear")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                    && pkt.body.get("id").is_some()
                                {
                                    crate::queue_notification(
                                        &pkt.body,
                                        pkt.source.as_deref().unwrap_or("desktop"),
                                    );
                                }
                                let _ = packet_tx.send(trimmed);
                            }
                            line.clear();
                        }
                        tracing::warn!("Control connection lost");
                        // Only retire the shared slots if they still belong to
                        // *this* connection. An older reader that dies late
                        // would otherwise wipe a newer connection's writer,
                        // permanently pushing every query onto the one-shot
                        // fallback path.
                        let still_ours = {
                            let guard = (*CONTROL_WRITER).lock().await;
                            guard
                                .as_ref()
                                .is_some_and(|live| Arc::ptr_eq(live, &writer_arc))
                        };
                        if !still_ours {
                            return;
                        }
                        *crate::CONTROL_PEER.lock().await = None;
                        *crate::INCOMING_PACKETS.lock().await = None;
                        let mut state_guard = (*CONNECTION_STATE).lock().await;
                        *state_guard = ConnectionState::Disconnected;
                        let mut writer_guard = (*CONTROL_WRITER).lock().await;
                        *writer_guard = None;
                        *crate::WAN_IDENTITY.lock().await = None;
                        crate::SIREN_RINGING.store(false, std::sync::atomic::Ordering::SeqCst);
                        *crate::PAIR_RESULT.lock().await = None;
                    }
                    .instrument(tracing::debug_span!("control_reader")),
                );

                tracing::info!("Connected to peer");
                Ok(ConnectionState::Connected)
            }
            Err(e) => {
                tracing::error!(error = %e, "Connection failed");
                Ok(ConnectionState::Error(LinuxLinkErrorDto::from(e)))
            }
        }
    }
    .instrument(span)
    .await;

    if let Ok(ref state) = result {
        *state_guard = match state {
            ConnectionState::Connected => ConnectionState::Connected,
            ConnectionState::Disconnected => ConnectionState::Disconnected,
            ConnectionState::Connecting => ConnectionState::Connecting,
            ConnectionState::Error(e) => ConnectionState::Error(e.clone()),
        };
    }
    result
}

/// Poll for incoming KDE Connect packets from the control connection.
/// Returns up to 16 queued packet JSON strings. Kotlin should call this
/// periodically while connected to process server push messages (notifications,
/// clipboard sync, etc.).
pub async fn poll_incoming_packets() -> Vec<String> {
    let rx = {
        let guard = crate::INCOMING_PACKETS.lock().await;
        guard.as_ref().map(|tx| tx.subscribe())
    };
    let Some(mut rx) = rx else { return vec![] };
    let mut packets = vec![];
    // Try to get at least one packet with a short timeout
    match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
        Ok(Ok(pkt)) => packets.push(pkt),
        _ => return packets,
    }
    // Drain any additional queued packets
    while packets.len() < 16 {
        match rx.try_recv() {
            Ok(pkt) => packets.push(pkt),
            Err(_) => break,
        }
    }
    packets
}

/// The server's cached iroh WAN identity (raw `kdeconnect.linuxlink.endpoint`
/// body JSON), or `None` if the connected desktop has not announced one.
pub async fn get_wan_identity() -> Option<String> {
    crate::WAN_IDENTITY.lock().await.clone()
}

/// Consume the find-my-device siren latch (R3 Tier-2 #11). Returns `true`
/// once per `kdeconnect.findmydevice` `{ring:true}` pushed by the desktop.
pub fn check_siren() -> bool {
    crate::SIREN_RINGING.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// Make the remote desktop ring (find-my-device): push `{ring: true}` over the
/// control connection.
pub async fn send_findmydevice(address: String, port: u16) -> Result<(), String> {
    let request = NetworkPacket::new("kdeconnect.findmydevice")
        .with_body(serde_json::json!({ "ring": true }));
    push_packet(&address, port, &request).await
}

// ---------------------------------------------------------------------------
// PIN pairing (R3 Tier-2 #11b)
// ---------------------------------------------------------------------------

fn paired_servers_path() -> Option<PathBuf> {
    crate::CERT_DIR
        .lock()
        .unwrap()
        .as_ref()
        .map(|d| d.join("paired_servers.json"))
}

fn read_paired_servers() -> Vec<String> {
    paired_servers_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn remember_paired_server(server_id: &str) {
    if server_id.is_empty() {
        return;
    }
    let Some(path) = paired_servers_path() else {
        return;
    };
    let mut ids = read_paired_servers();
    if !ids.iter().any(|id| id == server_id) {
        ids.push(server_id.to_string());
        if let Ok(json) = serde_json::to_vec_pretty(&ids)
            && let Some(parent) = path.parent()
        {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(&path, json);
        }
    }
}

/// Request a pairing PIN from the desktop (show-the-PIN flow).
///
/// Pairing rides the live control connection when there is one. A second
/// connection from the same address evicts the live one from the desktop's
/// client registry (it is keyed by peer address), and the evicted session
/// then receives no pushes - HUD, notifications, WAN identity - until the
/// app reconnects. Returns
/// `"pinSent"` when the desktop displayed a PIN to read off it, `"pinReady"`
/// when `linux-link pair` already put a PIN up for manual entry, or `"error"`.
pub async fn request_pair_pin(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new("kdeconnect.linuxlink.pair")
        .with_body(serde_json::json!({ "requestPin": true }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.pair",
        Duration::from_secs(5),
    )
    .await
    .map_err(|_| "Desktop did not respond to the PIN request".to_string())?;
    Ok(body
        .get("pairStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("error")
        .to_string())
}

/// Submit an externally-entered pairing PIN (`linux-link pair` flow, or the
/// code the desktop just displayed). Blocks briefly for the desktop's
/// decision; `Some(desktopDeviceId)` = paired (persisted phone-side).
pub async fn pair_with_pin(
    address: String,
    port: u16,
    pin: String,
) -> Result<Option<String>, String> {
    if pin.len() != 6 || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("PIN must be exactly 6 digits".to_string());
    }
    let request =
        NetworkPacket::new("kdeconnect.pair").with_body(serde_json::json!({ "pin": pin }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pair",
        Duration::from_secs(8),
    )
    .await
    .map_err(|_| "Desktop did not answer the PIN in time".to_string())?;
    if body.get("pair").and_then(|v| v.as_bool()).unwrap_or(false) {
        let id = body
            .get("serverId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        remember_paired_server(&id);
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

/// Wait up to `wait_secs` for the desktop's pairing outcome (latched by the
/// control reader from pushed `kdeconnect.pair` / `kdeconnect.linuxlink.pair`
/// packets) and consume it. `Some(serverId)` = paired (the desktop id is
/// persisted to the phone-side trust list); `None` = still unpaired.
pub async fn check_pair_result(wait_secs: u64) -> Option<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(wait_secs);
    loop {
        let body = crate::PAIR_RESULT.lock().await.take();
        if let Some(b) = body
            && b.get("pair").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            let id = b
                .get("serverId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            remember_paired_server(&id);
            return Some(id);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// JSON array of desktop device ids this phone has paired with.
pub fn paired_servers_json() -> String {
    serde_json::to_string(&read_paired_servers()).unwrap_or_else(|_| "[]".to_string())
}

// ---------------------------------------------------------------------------
// Notification relay (R3 Tier-2 #11c)
// ---------------------------------------------------------------------------

/// Drain the desktop notifications queued by the control reader as a JSON
/// array (`[{id,app,title,text,source}, …]`); Kotlin posts them as Android
/// notifications with an inline-reply action.
pub fn take_pending_notifications() -> String {
    let drained = {
        let mut q = crate::PENDING_NOTIFICATIONS
            .lock()
            .expect("notification queue");
        std::mem::take(&mut *q)
    };
    serde_json::to_string(&drained).unwrap_or_else(|_| "[]".to_string())
}

/// Reply to a desktop notification (R3 Tier-2 #11c) over the control
/// connection; the server's notification_reply plugin relays the text
/// (replies.log + clipboard + on-screen confirmation).
pub async fn send_notification_reply(
    address: String,
    port: u16,
    id: String,
    text: String,
) -> Result<(), String> {
    let request = NetworkPacket::new("kdeconnect.linuxlink.notification_reply")
        .with_body(serde_json::json!({ "id": id, "reply": text, "passive": false }));
    push_packet(&address, port, &request).await
}

/// A control request that got no answer on the shared connection.
///
/// The server is talking to us — it just never sent `reply_type` — so opening
/// a second connection cannot help. Callers must surface this as an error
/// instead of falling back.
#[derive(Debug)]
pub struct NoReply;

impl std::fmt::Display for NoReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The desktop did not answer over the control connection")
    }
}

impl From<NoReply> for String {
    fn from(e: NoReply) -> Self {
        e.to_string()
    }
}

/// Send `request` on the shared control connection and resolve with the body
/// of the first packet whose type is `reply_type`.
///
/// Every query used to open its own throwaway TCP connection carrying the same
/// device_id. The server keys its client registry by device_id, so each of
/// those evicted the real control session (which is how the phone silently
/// stopped receiving desktop notifications), and dropping the socket the moment
/// the answer arrived left the server writing into a dead pipe — the source of
/// the `Broken pipe` plugin errors and the "reconnect storm" kicks. One shared
/// connection for everything instead.
///
/// `Ok(None)` means there is no live control connection: the caller should use
/// [`oneshot_request`]. `Err(NoReply)` means the connection is live but the
/// server never answered.
async fn control_request(
    address: &str,
    port: u16,
    request: &NetworkPacket,
    reply_type: &str,
    timeout: Duration,
) -> Result<Option<serde_json::Value>, NoReply> {
    // Only the live connection *to this desktop* may carry the request: with
    // another computer connected, reusing it would send this desktop's query -
    // or its pairing PIN - to the wrong machine, and record that machine's
    // answer as this one's. Locked writer-then-peer, like the control reader.
    let writer = {
        let guard = (*CONTROL_WRITER).lock().await;
        let peer = (*CONTROL_PEER).lock().await;
        let same_desktop = peer
            .as_ref()
            .is_some_and(|(host, live_port)| host == address && *live_port == port);
        if same_desktop {
            guard.as_ref().cloned()
        } else {
            None
        }
    };
    // Subscribe before sending: the answer can otherwise land in the broadcast
    // before there is anyone listening for it.
    let rx = {
        let guard = crate::INCOMING_PACKETS.lock().await;
        guard.as_ref().map(|tx| tx.subscribe())
    };
    let (Some(writer), Some(mut rx)) = (writer, rx) else {
        return Ok(None);
    };

    let sender = TcpDeviceSender::from_arc(writer, address.to_string());
    sender.send_packet(request).await.map_err(|e| {
        tracing::warn!("Control request failed to send: {e}");
        NoReply
    })?;

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(NoReply);
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(line)) => {
                if let Ok(packet) = NetworkPacket::from_wire(&line)
                    && packet.packet_type == reply_type
                {
                    return Ok(Some(packet.body));
                }
            }
            // The reader loop retires the shared writer when the socket dies,
            // so a lagging subscriber simply means "no answer on this one".
            Ok(Err(_)) | Err(_) => return Err(NoReply),
        }
    }
}

/// Open a control connection for a single request, answer it, and close it
/// politely.
///
/// Only for callers with no live control connection to share. The write half is
/// shut down and the socket drained before returning so the server's last
/// writes land on a closed-but-flushing pipe rather than an aborted one —
/// closing mid-write is what produced the `Broken pipe` plugin errors.
async fn oneshot_request(
    address: &str,
    port: u16,
    request: &NetworkPacket,
    reply_type: &str,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let conn_mgr = ConnectionManager::new(Duration::from_secs(5));
    let identity = client_identity();
    let stream = conn_mgr
        .connect(address, port, &identity)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;
    let (reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));
    let sender = TcpDeviceSender::from_arc(writer.clone(), address.to_string());
    sender
        .send_packet(request)
        .await
        .map_err(|e| format!("Failed to send {reply_type} query: {e}"))?;

    let mut lines = tokio::io::BufReader::new(reader).lines();
    let deadline = tokio::time::Instant::now() + timeout;
    let reply = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("Timeout waiting for {reply_type} response"));
        }
        match tokio::time::timeout(remaining, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                if let Ok(packet) = NetworkPacket::from_wire(&line)
                    && packet.packet_type == reply_type
                {
                    break packet.body;
                }
            }
            Ok(Ok(None)) => return Err("Connection closed by peer".to_string()),
            Ok(Err(e)) => return Err(format!("Read error: {e}")),
            Err(_) => return Err(format!("Timeout waiting for {reply_type} response")),
        }
    };
    let mut writer = writer.lock().await;
    let _ = writer.shutdown().await;
    drop(writer);
    // Read the server's remaining packets to EOF. Closing a socket with unread
    // receive data sends RST, and that is exactly what the server's plugins
    // report as `Broken pipe`.
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        while let Ok(Some(_)) = lines.next_line().await {}
    })
    .await;
    Ok(reply)
}

/// Request/reply for a desktop query: reuse the live control connection, and
/// only open a short-lived one when there is nothing to reuse.
async fn query_peer(
    address: &str,
    port: u16,
    request: &NetworkPacket,
    reply_type: &str,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    match control_request(address, port, request, reply_type, timeout).await? {
        Some(body) => Ok(body),
        None => oneshot_request(address, port, request, reply_type, timeout).await,
    }
}

/// Fire-and-forget push: write `request` on the live control connection, or on
/// a dedicated one when the phone is not connected yet.
///
/// The dedicated connection is half-closed and drained rather than dropped, for
/// the same reason [`oneshot_request`] is: an abrupt close with data still in
/// flight shows up server-side as a `Broken pipe` plugin error.
async fn push_packet(address: &str, port: u16, request: &NetworkPacket) -> Result<(), String> {
    let writer = {
        let guard = (*CONTROL_WRITER).lock().await;
        guard.as_ref().cloned()
    };
    if let Some(writer) = writer {
        let sender = TcpDeviceSender::from_arc(writer, address.to_string());
        return sender
            .send_packet(request)
            .await
            .map_err(|e| format!("Failed to send {}: {e}", request.packet_type));
    }

    let conn_mgr = ConnectionManager::new(Duration::from_secs(5));
    let identity = client_identity();
    let stream = conn_mgr
        .connect(address, port, &identity)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));
    let sender = TcpDeviceSender::from_arc(writer.clone(), address.to_string());
    let sent = sender
        .send_packet(request)
        .await
        .map_err(|e| format!("Failed to send {}: {e}", request.packet_type));
    let mut writer = writer.lock().await;
    let _ = writer.shutdown().await;
    drop(writer);
    let mut sink = [0u8; 512];
    let _ = tokio::time::timeout(Duration::from_millis(250), async {
        while let Ok(n) = reader.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
    sent
}

/// Send clipboard content to peer using KDE Connect protocol.
pub async fn send_clipboard(address: String, port: u16, content: String) -> Result<(), String> {
    let packet = NetworkPacket::new("kdeconnect.clipboard").with_body(serde_json::json!({
        "content": content,
    }));
    push_packet(&address, port, &packet).await?;
    tracing::info!(
        "Clipboard sent to {}:{} ({} chars)",
        address,
        port,
        content.len()
    );
    Ok(())
}

/// Get clipboard content from peer.
pub async fn get_clipboard(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new("kdeconnect.clipboard.connect");
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.clipboard",
        Duration::from_secs(5),
    )
    .await?;
    Ok(body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Send file to peer using KDE Share protocol.
pub async fn send_file(address: String, port: u16, file_path: String) -> Result<(), String> {
    let metadata = tokio::fs::metadata(&file_path)
        .await
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;
    let file_size = metadata.len();
    let filename = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown_file")
        .to_string();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind port: {}", e))?;
    let transfer_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let conn_mgr = ConnectionManager::new(Duration::from_secs(10));
    let identity = client_identity();
    let stream = conn_mgr
        .connect(&address, port, &identity)
        .await
        .map_err(|e| e.to_string())?;
    let (_reader, writer) = tokio::io::split(stream);
    let sender = TcpDeviceSender::new(writer, address);
    let request = NetworkPacket::new("kdeconnect.share.request")
        .with_body(serde_json::json!({
            "filename": filename,
            "payloadTransferInfo": { "port": transfer_port },
        }))
        .with_payload_size(file_size as u64);
    sender
        .send_packet(&request)
        .await
        .map_err(|e| format!("Failed to send share request: {}", e))?;
    let (mut client_stream, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
        .await
        .map_err(|_| "Timeout waiting for file receiver".to_string())?
        .map_err(|e| format!("Failed to accept connection: {}", e))?;
    let mut file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| format!("Failed to open file: {}", e))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut sent: u64 = 0;
    loop {
        let n = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("Failed to read file: {}", e))?;
        if n == 0 {
            break;
        }
        client_stream
            .write_all(&buffer[..n])
            .await
            .map_err(|e| format!("Failed to write to stream: {}", e))?;
        sent += n as u64;
    }
    client_stream
        .flush()
        .await
        .map_err(|e: std::io::Error| e.to_string())?;
    tracing::info!("File sent: {} ({} bytes)", filename, sent);
    Ok(())
}

/// List files in a remote directory using the file browse protocol.
pub async fn list_remote_files(
    address: String,
    port: u16,
    remote_path: String,
) -> Result<Vec<RemoteFileDto>, String> {
    let request = NetworkPacket::new("kdeconnect.filebrowse.request")
        .with_body(serde_json::json!({ "path": remote_path }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.filebrowse.response",
        Duration::from_secs(10),
    )
    .await?;
    if let Some(error) = body.get("error").and_then(|v| v.as_str()) {
        return Err(error.to_string());
    }
    let files = body
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "Missing 'files' in response".to_string())?;
    Ok(files
        .iter()
        .filter_map(|f| {
            Some(RemoteFileDto {
                name: f.get("name")?.as_str()?.to_string(),
                is_directory: f.get("isDirectory")?.as_bool()?,
                size: f.get("size")?.as_u64()?,
                modified: f.get("modified")?.as_u64()?,
            })
        })
        .collect())
}

/// Request remote screen streaming.
///
/// `monitor_index` selects which display to stream (0 = primary).
/// Pass `None` to use the default monitor.
/// `codec_caps` advertises the decodable video codecs to the server
/// (R4 C1: bit 0 = HEVC; H.264 is assumed). 0 keeps a plain H.264 session.
pub async fn connect_streaming(
    address: String,
    port: u16,
    monitor_index: Option<u32>,
    codec_caps: u8,
) -> Result<(), String> {
    // If the control port (1716) is passed, automatically switch to the default streaming port (4716).
    // In a real KDE Connect implementation, this would be negotiated or discovery-based.
    let streaming_port = if port == linux_link_core::DEFAULT_CONTROL_PORT {
        linux_link_core::DEFAULT_STREAMING_PORT
    } else {
        port
    };

    let addr = format!("{address}:{streaming_port}");

    // F2: Idempotent session check
    {
        let mut handle_guard = (*STREAMING_HANDLE).lock().await;
        if let Some(handle) = &*handle_guard {
            if handle.address == address && handle.port == streaming_port {
                tracing::info!("Session already active for {addr}, skipping connect");
                return Ok(());
            }
            tracing::info!(
                "Stopping existing session for {} before connecting to {address}",
                handle.address
            );
        }
        if let Some(handle) = handle_guard.take() {
            teardown_streaming_handle(handle).await;
        }
    }

    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Connecting;
    tracing::info!("Connecting to streaming server at {addr}");

    // Create a CertManager with persistent TOFU peer certificate verification.
    // Uses the data directory set via `set_data_dir()`. Falls back to in-memory
    // if no data directory is configured (cert trust lost on restart).
    let cert_manager = {
        let guard = crate::CERT_DIR.lock().unwrap();
        match guard.as_ref() {
            Some(dir) => std::sync::Arc::new(
                linux_link_core::streaming::transport::CertManager::load_or_create(dir)
                    .map_err(|e| e.to_string())?,
            ),
            None => {
                tracing::warn!("No data dir configured — cert trust is ephemeral");
                std::sync::Arc::new(
                    linux_link_core::streaming::transport::CertManager::new()
                        .map_err(|e| e.to_string())?,
                )
            }
        }
    };

    // Store for trust management UI
    {
        let mut cm_guard = crate::CERT_MANAGER.lock().unwrap();
        *cm_guard = Some(cert_manager.clone());
    }

    // F6: Trust revalidation logging
    let peer_label = addr.clone();
    if cert_manager.is_peer_trusted(&peer_label) {
        tracing::info!(peer = %peer_label, "Connecting to already trusted peer");
    } else {
        tracing::info!(peer = %peer_label, "Establishing new trust bond with peer (TOFU)");
    }

    let (client, packet_rx, audio_rx) = StreamingClient::connect(
        &addr,
        cert_manager,
        monitor_index,
        Some(&client_identity().device_id),
        Some(codec_caps),
    )
    .await
    .map_err(|e| {
        let err_dto = LinuxLinkErrorDto::from(linux_link_core::error::LinuxLinkError::from(e));
        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Error(LinuxLinkErrorDto {
            code: err_dto.code,
            message: err_dto.message.clone(),
            is_retryable: err_dto.is_retryable,
        });
        err_dto.message
    })?;

    install_streaming(client, packet_rx, audio_rx, address, streaming_port, None).await
}

/// Request remote screen streaming over the iroh WAN path.
///
/// `identity_json` is the cached `kdeconnect.linuxlink.endpoint` body learned
/// from the desktop over the trusted control channel (fields: endpointId,
/// relayUrls, directAddrs). Relays are used for hole punching only — there is
/// no pkarr discovery, so the identity must come from a prior LAN session.
pub async fn connect_streaming_wan(
    address: String,
    identity_json: String,
    monitor_index: Option<u32>,
    codec_caps: u8,
) -> Result<(), String> {
    let identity: serde_json::Value =
        serde_json::from_str(&identity_json).map_err(|e| format!("Invalid WAN identity: {e}"))?;
    let endpoint_id = identity["endpointId"]
        .as_str()
        .ok_or_else(|| "WAN identity missing endpointId".to_string())?;
    let read_list = |key: &str| -> Vec<String> {
        identity[key]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let relay_urls = read_list("relayUrls");
    let direct_addrs = read_list("directAddrs");

    // WAN sessions are keyed by the desktop address with a 0 port sentinel
    // (there is no TCP port on the iroh path).
    {
        let mut handle_guard = (*STREAMING_HANDLE).lock().await;
        if let Some(handle) = &*handle_guard {
            if handle.address == address && handle.port == 0 {
                tracing::info!("WAN session already active for {address}, skipping connect");
                return Ok(());
            }
            tracing::info!(
                "Stopping existing session for {} before connecting to {address}",
                handle.address
            );
        }
        if let Some(handle) = handle_guard.take() {
            teardown_streaming_handle(handle).await;
        }
    }

    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Connecting;
    tracing::info!("Dialing {address} over iroh WAN (endpoint {endpoint_id})");

    let dial = linux_link_core::streaming::IrohDial::dial(
        endpoint_id,
        &relay_urls,
        &direct_addrs,
        true,
        Duration::from_secs(20),
    )
    .await
    .map_err(|e| {
        let err_dto = LinuxLinkErrorDto::from(linux_link_core::error::LinuxLinkError::from(e));
        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Error(LinuxLinkErrorDto {
            code: err_dto.code,
            message: err_dto.message.clone(),
            is_retryable: err_dto.is_retryable,
        });
        err_dto.message
    })?;

    let (client, packet_rx, audio_rx) = StreamingClient::attach(
        dial.connection(),
        monitor_index,
        Some(&client_identity().device_id),
        Some(codec_caps),
    )
    .await;

    install_streaming(client, packet_rx, audio_rx, address, 0, Some(dial)).await
}

/// Cleanly stop a streaming session: cancel its tasks and close the WAN dial
/// endpoint if it owns one (iroh endpoints must be closed, never dropped).
async fn teardown_streaming_handle(handle: StreamingHandle) {
    handle.cancel.cancel();
    let _ = handle.task.await;
    let _ = handle.rtt_task.await;
    if let Some(dial) = handle.wan_dial {
        dial.close().await;
    }
}

/// Spawn the client start + RTT tasks and install the global streaming handle.
/// Shared by the LAN (`connect_streaming`) and WAN (`connect_streaming_wan`)
/// connect paths.
async fn install_streaming(
    mut client: StreamingClient,
    packet_rx: tokio::sync::mpsc::Receiver<linux_link_core::streaming::EncodedPacket>,
    audio_rx: tokio::sync::mpsc::Receiver<linux_link_core::streaming::AudioPacket>,
    address: String,
    port: u16,
    wan_dial: Option<linux_link_core::streaming::IrohDial>,
) -> Result<(), String> {
    let connection = client
        .connection()
        .ok_or_else(|| "Connection not available after connect".to_string())?
        .clone();

    // Samples the last session's decoder measured must not join this one's
    // record, and nothing is going to report them now that its server is gone.
    DECODE_SAMPLES.clear();
    RENDER_SAMPLES.clear();
    // Same for the HUD's window: a rate or RTT carried over from the computer
    // this session replaced would be quoted as if it were measured here.
    RTT_HISTORY.lock().unwrap().clear();
    RATE_SNAPSHOTS.lock().unwrap().clear();

    let cancel = client.cancel_token();
    let client_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        client.start().await;
        tracing::info!("Streaming client start loop exited");
    });

    let rtt_cancel = cancel.clone();
    let rtt_connection = connection.clone();
    let rtt_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = rtt_cancel.cancelled() => {
                    tracing::info!("RTT polling task cancelled");
                    break;
                }
                _ = interval.tick() => {
                    let stats = rtt_connection.stats();
                    let rtt_us = stats.rtt.as_micros() as u64;
                    update_streaming_rtt(rtt_us);

                    // Hand the decoder's measurements to the desktop, which is
                    // where every percentile tail in the session record is
                    // built. The client's own e2e probes ride core's stats
                    // loop; these two batches are what only the phone knows.
                    report_samples(&rtt_connection, SAMPLE_DECODE, &DECODE_SAMPLES).await;
                    report_samples(&rtt_connection, SAMPLE_RENDER, &RENDER_SAMPLES).await;

                    // Link-path telemetry (R4 A1): quinn LAN is always
                    // direct; an iroh WAN session rides a relay until hole
                    // punching opens a direct path — and iroh keeps trying,
                    // so a relayed→direct transition is expected, not novel.
                    let is_wan = crate::SESSION_IS_WAN.load(Ordering::Relaxed);
                    let state = if !is_wan {
                        1
                    } else if stats.relayed {
                        3
                    } else {
                        2
                    };
                    crate::LINK_STATE.store(state, Ordering::Relaxed);
                    let was_relayed = crate::SESSION_RELAYED.swap(stats.relayed, Ordering::Relaxed);
                    if stats.relayed && !was_relayed {
                        tracing::warn!("WAN session is riding a relay (punching not complete)");
                    } else if !stats.relayed && was_relayed {
                        tracing::info!("WAN session upgraded from relay to direct path");
                    }

                    // Update session status based on RTT (detect staleness)
                    let mut status_guard = crate::SESSION_STATUS.lock().unwrap();
                    let rtt_ms = rtt_us / 1000;
                    if rtt_ms > 2000 {
                        *status_guard = SessionStatus::Stale { rtt_ms };
                    } else if matches!(*status_guard, SessionStatus::Stale { .. }) {
                        *status_guard = SessionStatus::Active;
                    }
                }
            }
        }
    });

    STREAMING_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    crate::SESSION_IS_WAN.store(wan_dial.is_some(), std::sync::atomic::Ordering::Relaxed);
    crate::LINK_STATE.store(
        if wan_dial.is_some() { 0 } else { 1 },
        std::sync::atomic::Ordering::Relaxed,
    );
    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Active;

    let mut handle = (*STREAMING_HANDLE).lock().await;
    *handle = Some(StreamingHandle {
        address: address.clone(),
        port,
        cancel: client_cancel,
        task,
        rtt_task,
        packet_rx,
        audio_rx,
        connection: connection.clone(),
        wan_dial,
    });

    tracing::info!("Streaming session connected to {address}:{port}");
    Ok(())
}

/// Record one duration the app measured, in microseconds, into the batch the
/// streaming poller reports to the desktop. `kind` is a `SAMPLE_*` id.
///
/// This is the whole cost of measuring on a decoder thread: a push into a
/// bounded batch, no allocation, no runtime, no error to handle. A kind the
/// phone does not own (`SAMPLE_E2E` is measured in core's receive loop) is
/// ignored, because the desktop would refuse it too.
pub fn record_sample(kind: u8, value_us: u64) {
    let batch = match kind {
        SAMPLE_DECODE => &DECODE_SAMPLES,
        SAMPLE_RENDER => &RENDER_SAMPLES,
        _ => return,
    };
    if !batch.push(value_us) {
        tracing::debug!("Sample batch full: reporting late, oldest dropped");
    }
}

/// Connect to a peer using the v2 multiplexed protocol (QUIC).
pub async fn connect_v2(address: String, port: u16) -> Result<(), String> {
    let addr_str = format!("{address}:{port}");
    let addr: std::net::SocketAddr = addr_str
        .parse()
        .map_err(|e| format!("Invalid address {addr_str}: {e}"))?;

    // Create a CertManager with persistent TOFU peer certificate verification.
    let cert_manager = {
        let guard = crate::CERT_DIR.lock().unwrap();
        match guard.as_ref() {
            Some(dir) => std::sync::Arc::new(
                linux_link_core::streaming::transport::CertManager::load_or_create(dir)
                    .map_err(|e| e.to_string())?,
            ),
            None => {
                tracing::warn!("No data dir configured — cert trust is ephemeral");
                std::sync::Arc::new(
                    linux_link_core::streaming::transport::CertManager::new()
                        .map_err(|e| e.to_string())?,
                )
            }
        }
    };

    // Store for trust management UI
    {
        let mut cm_guard = crate::CERT_MANAGER.lock().unwrap();
        *cm_guard = Some(cert_manager.clone());
    }

    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Connecting;

    let span = tracing::info_span!("connect_v2", peer = %addr_str);

    let result = async {
        let client_config = cert_manager
            .client_config(vec![ALPN_V2.to_vec()])
            .map_err(|e| format!("Failed to create QUIC config: {e}"))?;

        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
            .map_err(|e| format!("Failed to create endpoint: {e}"))?;
        endpoint.set_default_client_config(client_config);

        let server_name = addr.ip().to_string();
        let connection = endpoint
            .connect(addr, &server_name)
            .map_err(|e| format!("Failed to initiate connection: {e}"))?
            .await
            .map_err(|e| format!("Connection failed: {e}"))?;

        tracing::info!("QUIC connection established, performing v2 handshake");

        let (mut send0, mut recv0) = connection
            .open_bi()
            .await
            .map_err(|e| format!("Failed to open stream 0: {e}"))?;

        let identity = client_identity();
        let local_identity = IdentityPacketV2 {
            device_id: identity.device_id,
            device_name: identity.device_name,
            min_version: linux_link_core::protocol::v2::V2_MIN_VERSION,
            max_version: linux_link_core::protocol::v2::V2_MAX_VERSION,
            capabilities: vec!["streaming".to_string(), "input".to_string()],
        };

        let peer_identity = perform_v2_handshake(&mut send0, &mut recv0, &local_identity)
            .await
            .map_err(|e| format!("v2 Handshake failed: {e}"))?;

        tracing::info!("v2 Handshake successful with {}", peer_identity.device_name);

        // Create a broadcast channel for forwarding incoming packets to Flutter
        let (packet_tx, _) = broadcast::channel(256);
        {
            let mut incoming = crate::INCOMING_PACKETS.lock().await;
            *incoming = Some(packet_tx.clone());
        }

        let mut control_recv = recv0;
        let task = tokio::spawn(async move {
            loop {
                let mut len_buf = [0u8; 4];
                if control_recv.read_exact(&mut len_buf).await.is_err() {
                    break;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                if len > linux_link_core::protocol::v2::MAX_CONTROL_PAYLOAD_SIZE {
                    tracing::error!("v2 payload exceeds limit: {}", len);
                    break;
                }
                let mut buf = vec![0u8; len];
                if control_recv.read_exact(&mut buf).await.is_err() {
                    break;
                }
                if let Ok(json_str) = String::from_utf8(buf) {
                    let _ = packet_tx.send(json_str);
                }
            }
            tracing::warn!("v2 control stream closed");
            let mut handle_guard = crate::V2_HANDLE.lock().await;
            *handle_guard = None;
            let mut incoming = crate::INCOMING_PACKETS.lock().await;
            *incoming = None;
        });

        let mut handle_guard = crate::V2_HANDLE.lock().await;
        *handle_guard = Some(crate::V2Handle {
            connection,
            control_send: Arc::new(Mutex::new(send0)),
            task,
        });

        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Active;
        Ok(())
    };

    use tracing::Instrument;
    match result.instrument(span).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_msg: String = e;
            let err_dto =
                LinuxLinkErrorDto::from(linux_link_core::error::LinuxLinkError::ProtocolError {
                    detail: err_msg.clone(),
                });
            *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Error(err_dto);
            Err(err_msg)
        }
    }
}

/// Reset the reconnection backoff timer (e.g. after a successful manual connect).
pub fn reset_reconnect_backoff() {
    crate::RECONNECT_BACKOFF.lock().unwrap().reset();
}

/// Attempt to reconnect the current streaming session using exponential backoff.
pub async fn reconnect_streaming(
    address: String,
    port: u16,
    monitor_index: Option<u32>,
    attempt: u32,
    codec_caps: u8,
) -> Result<(), String> {
    let next_retry = crate::RECONNECT_BACKOFF.lock().unwrap().next_delay();
    let next_retry_ms = next_retry.as_millis() as u64;

    {
        let mut status_guard = crate::SESSION_STATUS.lock().unwrap();
        *status_guard = SessionStatus::Reconnecting {
            attempt,
            next_retry_ms,
        };
    }

    tracing::info!(attempt, next_retry_ms, "Reconnecting streaming session...");

    // Perform the delay in Rust to own the timing semantics
    tokio::time::sleep(next_retry).await;

    // Delegate to idempotent connect_streaming
    connect_streaming(address, port, monitor_index, codec_caps).await
}

/// Stop the v2 multiplexed connection and clear its handle.
pub async fn stop_v2() -> Result<(), String> {
    let handle = {
        let mut guard = crate::V2_HANDLE.lock().await;
        guard.take()
    };
    if let Some(handle) = handle {
        handle.task.abort();
        let _ = handle.task.await;
        handle.connection.close(0u32.into(), b"Stopped");
        tracing::info!("v2 session stopped");
    }
    Ok(())
}

/// Stop remote screen streaming.
pub async fn stop_streaming() -> Result<(), String> {
    // Reset streaming metrics
    STREAMING_FRAME_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    STREAMING_BYTE_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
    crate::LINK_STATE.store(0, std::sync::atomic::Ordering::Relaxed);
    crate::SESSION_IS_WAN.store(false, std::sync::atomic::Ordering::Relaxed);
    crate::SESSION_RELAYED.store(false, std::sync::atomic::Ordering::Relaxed);
    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Disconnected;

    let handle = {
        let mut guard = (*STREAMING_HANDLE).lock().await;
        guard.take()
    };
    if let Some(handle) = handle {
        teardown_streaming_handle(handle).await;
        tracing::info!("Streaming session stopped");
    }

    // F4: Also clear v2 handle if it exists during stop_streaming
    let _ = stop_v2().await;

    STREAMING_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// Check if streaming is active using an atomic flag (no lock contention).
pub fn is_streaming_active() -> bool {
    STREAMING_ACTIVE.load(std::sync::atomic::Ordering::Acquire)
}

/// Get the current RTT to the streaming server in microseconds.
///
/// The median of this session's recent one-second polls, not whichever poll was
/// current when the HUD looked: the transport reports one smoothed value per
/// sample and no variance at all, so a median over a stated number of samples is
/// the most confidence a link figure can carry (roadmap 2056). Use
/// [`rtt_samples`] alongside it before believing the number.
pub fn get_streaming_rtt() -> u64 {
    let history = RTT_HISTORY.lock().unwrap();
    crate::rates::median(&history[..]).unwrap_or(0)
}

/// How many RTT polls [`get_streaming_rtt`] summarised, as the DTO reports it.
/// Zero after a reconnect, which is when the HUD must not present a link figure
/// as fact.
pub fn rtt_samples() -> usize {
    RTT_HISTORY.lock().unwrap().len()
}

/// Get the current high-level status of the streaming session.
pub fn get_session_status() -> SessionStatus {
    crate::SESSION_STATUS.lock().unwrap().clone()
}

/// Get detailed streaming session statistics.
///
/// The rates are measured over the last [`crate::rates::RATE_WINDOW`], not
/// divided by the session's lifetime: a picture that stalled three seconds ago
/// has to read as stalled, which a total-averaged figure cannot say.
pub fn get_streaming_stats() -> StreamingStatsDto {
    let frame_count = STREAMING_FRAME_COUNT.load(Ordering::Relaxed);
    let byte_count = STREAMING_BYTE_COUNT.load(Ordering::Relaxed);

    let rates = {
        let mut history = RATE_SNAPSHOTS.lock().unwrap();
        crate::rates::push_snapshot(&mut history, Instant::now(), frame_count, byte_count);
        crate::rates::rates_over(&history, crate::rates::RATE_WINDOW)
    };

    StreamingStatsDto {
        fps: (rates.fps * 10.0).round() / 10.0,
        bitrate_kbps: rates.kbps,
        rate_window_ms: rates.span.as_millis() as u64,
        rtt_samples: rtt_samples(),
        // R4 E3: real compositor-true estimate (capture→send age measured
        // on the desktop clock + one network leg), EWMA'd in the core
        // client. 0 until the session's first video packet.
        e2e_latency_ms: linux_link_core::streaming::client::e2e_estimate_ms(),
        // Frames whose sequence number never arrived, counted by the client's
        // receive loop — see `missed_video_frames` in core. This replaced a
        // literal 0 that asserted a healthy link the phone had never
        // measured (roadmap 2051/2052).
        frame_drops: linux_link_core::streaming::client::missed_video_frames(),
        link_state: match crate::LINK_STATE.load(Ordering::Relaxed) {
            1 => "lan",
            2 => "wan_direct",
            3 => "wan_relayed",
            _ if is_streaming_active() => "wan",
            _ => "none",
        },
    }
}

/// List all trusted peers (certificate labels) from the active streaming session's
/// CertManager. Returns an empty list if no streaming session is active.
pub fn list_trusted_peers() -> Vec<String> {
    let guard = crate::CERT_MANAGER.lock().unwrap();
    match guard.as_ref() {
        Some(cm) => {
            // Can't access known_peers directly — it's behind Arc<Mutex<HashMap>>.
            // We provide the list from a best-effort basis.
            let peers = cm.known_peers();
            peers.into_iter().map(|(label, _hash)| label).collect()
        }
        None => vec![],
    }
}

/// Remove a trusted peer certificate by label. Returns true if the peer was
/// found and removed, false otherwise. Persists the change to disk.
pub fn forget_trusted_peer(label: String) -> bool {
    let guard = crate::CERT_MANAGER.lock().unwrap();
    match guard.as_ref() {
        Some(cm) => {
            let removed = cm.remove_peer(&label);
            if removed {
                let _ = cm.save_known_peers();
            }
            removed
        }
        None => false,
    }
}

/// Get detailed list of monitors available on the remote server.
///
/// F2: Multi-monitor support — returns an error if the desktop does not answer.
pub async fn get_monitors(address: String, port: u16) -> Result<Vec<MonitorInfoDto>, String> {
    let request =
        NetworkPacket::new("kdeconnect.linuxlink.monitors").with_body(serde_json::json!({}));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.monitors",
        Duration::from_secs(5),
    )
    .await?;
    Ok(body
        .get("monitors")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| {
            // Legacy fallback if server only returns count
            let count = body.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
            (0..count)
                .map(|i| MonitorInfoDto {
                    index: i,
                    name: format!("Monitor {i}"),
                    width: 1920,
                    height: 1080,
                    is_primary: i == 0,
                })
                .collect()
        }))
}

/// Get the number of monitors available on the remote server (legacy).
pub async fn get_monitor_count(address: String, port: u16) -> Result<u32, String> {
    get_monitors(address, port).await.map(|m| m.len() as u32)
}

/// Desktop battery state as `{currentCharge, isCharging}`, or `noBattery` on
/// desktops without one (KDE Connect parity, Tier-2 #11).
pub async fn get_battery(address: String, port: u16) -> Result<serde_json::Value, String> {
    let request = NetworkPacket::new("kdeconnect.battery.request").with_body(serde_json::json!({}));
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.battery",
        Duration::from_secs(5),
    )
    .await
}

/// Desktop privacy mode (Tier-3 #15): ask the server to grab/release the
/// physical keyboard+mouse and/or lock its screen, then wait for the
/// plugin's reply body (`{ok, grabbed?, locked?, error?}`). Replies are
/// matched by packet type within the timeout because other pushes may
/// overtake them on the control connection.
pub async fn desktop_privacy(
    address: String,
    port: u16,
    action: String,
    lock: bool,
) -> Result<serde_json::Value, String> {
    let request = NetworkPacket::new("kdeconnect.linuxlink.privacy")
        .with_body(serde_json::json!({ "action": action, "lock": lock }));
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.privacy",
        Duration::from_secs(5),
    )
    .await
}

/// Desktop audio control (Tier-3 #16): send a `kdeconnect.linuxlink.audio`
/// request built from `body_json` (e.g. `{"action":"setVolume","volume":45}`)
/// and wait for the plugin's reply, matched by packet type within the
/// timeout. Same shared-control-connection shape as [desktop_privacy].
pub async fn audio_control(
    address: String,
    port: u16,
    body_json: String,
) -> Result<serde_json::Value, String> {
    let body: serde_json::Value =
        serde_json::from_str(&body_json).map_err(|e| format!("Bad request JSON: {e}"))?;
    let request = NetworkPacket::new("kdeconnect.linuxlink.audio").with_body(body);
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.audio",
        Duration::from_secs(8),
    )
    .await
}

/// A desktop window reported by the server's Hyprland windows plugin
/// (R3#7 picker). `at`/`size` are global desktop coordinates; `local_at` is
/// the window origin within its monitor — the space `InputPacket::WindowCrop`
/// crop rects are expressed in (the capture stream is monitor-local).
#[derive(serde::Deserialize, serde::Serialize)]
pub struct WindowInfoDto {
    pub address: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "class", default)]
    pub class: String,
    #[serde(default)]
    pub at: [i32; 2],
    #[serde(default)]
    pub local_at: [i32; 2],
    #[serde(default)]
    pub size: [i32; 2],
    /// Size of the monitor this window lives on `[w, h]`.
    #[serde(default)]
    pub monitor_size: [i32; 2],
    #[serde(default)]
    pub monitor: i32,
    #[serde(default)]
    pub fullscreen: u8,
    #[serde(default)]
    pub workspace: WorkspaceRefDto,
    /// Set from the response's `activeAddress`, not per-window.
    #[serde(default)]
    pub active: bool,
}

#[derive(serde::Deserialize, serde::Serialize, Default)]
pub struct WorkspaceRefDto {
    #[serde(default)]
    pub id: i32,
    #[serde(default)]
    pub name: String,
}

/// Enumerate the server's visible windows over the control channel.
///
/// Returns the list, the active window address, and the monitor layout
/// bounding box `[x, y, w, h]` (desktop space normalized input spans;
/// `None` if the server predates the field). `Err` on transport errors or
/// when the server's plugin reports no Hyprland session.
pub async fn get_windows(
    address: String,
    port: u16,
) -> Result<(Vec<WindowInfoDto>, String, Option<[i32; 4]>), String> {
    let request =
        NetworkPacket::new("kdeconnect.linuxlink.windows").with_body(serde_json::json!({}));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.windows",
        Duration::from_secs(5),
    )
    .await?;
    if !body
        .get("available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err("Server is not running Hyprland (no window list)".to_string());
    }

    let active = body
        .get("activeAddress")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut windows: Vec<WindowInfoDto> = body
        .get("windows")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    for w in &mut windows {
        w.active = w.address == active;
    }
    let screen = body
        .get("screen")
        .cloned()
        .and_then(|v| serde_json::from_value::<[i32; 4]>(v).ok());
    Ok((windows, active, screen))
}

/// Execute a power management command on the remote server.
/// Supported actions: "sleep", "shutdown", "restart", "hibernate".
pub async fn send_power_command(address: String, port: u16, action: String) -> Result<(), String> {
    let packet = NetworkPacket::new("kdeconnect.linuxlink.power")
        .with_body(serde_json::json!({ "action": action }));
    push_packet(&address, port, &packet).await?;
    tracing::info!("Power command sent: {action}");
    Ok(())
}

/// Execute a shell command on the remote server and return stdout + stderr + exit code.
pub async fn execute_remote_command(
    address: String,
    port: u16,
    command: String,
) -> Result<String, String> {
    let request = NetworkPacket::new("kdeconnect.linuxlink.exec")
        .with_body(serde_json::json!({ "command": command }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.linuxlink.exec",
        Duration::from_secs(10),
    )
    .await?;
    let stdout = body.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
    let stderr = body.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
    let exit_code = body.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1);
    Ok(format!(
        "{stdout}\n---END-OUTPUT---\n{stderr}\n---END-ERROR---\n{exit_code}"
    ))
}

/// Receive queued audio packets from the streaming client (F1: Audio Streaming).
///
/// Each audio packet contains raw Opus-encoded data (typically 20ms @ 48kHz stereo).
/// Returns up to `MAX_AUDIO_PACKETS_PER_RECEIVE` packets.
pub async fn receive_audio(timeout_ms: u64) -> Vec<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut packets = Vec::with_capacity(MAX_AUDIO_PACKETS_PER_RECEIVE);
    {
        let mut guard = (*STREAMING_HANDLE).lock().await;
        let Some(handle) = guard.as_mut() else {
            return packets;
        };
        match tokio::time::timeout_at(deadline, handle.audio_rx.recv()).await {
            Ok(Some(packet)) => {
                packets.push(packet.data);
                while packets.len() < MAX_AUDIO_PACKETS_PER_RECEIVE {
                    match handle.audio_rx.try_recv() {
                        Ok(packet) => {
                            packets.push(packet.data);
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                        | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                packets
            }
            _ => packets,
        }
    }
}

/// Receive queued H.264 frames from the streaming client.
///
/// The `timeout_ms` parameter only applies to waiting for the FIRST frame.
/// After the first frame arrives, any additional queued frames are drained
/// immediately with `try_recv()` (no further timeout). Returns up to
/// `MAX_FRAMES_PER_RECEIVE` frames per call.
pub async fn receive_frames(timeout_ms: u64) -> Vec<FrameDto> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut frames = Vec::with_capacity(MAX_FRAMES_PER_RECEIVE);
    {
        let mut guard = (*STREAMING_HANDLE).lock().await;
        let Some(handle) = guard.as_mut() else {
            return frames;
        };
        match tokio::time::timeout_at(deadline, handle.packet_rx.recv()).await {
            Ok(Some(packet)) => {
                STREAMING_FRAME_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                STREAMING_BYTE_COUNT.fetch_add(
                    packet.data.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                frames.push(FrameDto {
                    data: packet.data,
                    is_keyframe: packet.is_keyframe,
                    sequence: packet.sequence,
                });
                while frames.len() < MAX_FRAMES_PER_RECEIVE {
                    match handle.packet_rx.try_recv() {
                        Ok(packet) => {
                            STREAMING_FRAME_COUNT
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            STREAMING_BYTE_COUNT.fetch_add(
                                packet.data.len() as u64,
                                std::sync::atomic::Ordering::Relaxed,
                            );
                            frames.push(FrameDto {
                                data: packet.data,
                                is_keyframe: packet.is_keyframe,
                                sequence: packet.sequence,
                            });
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                        | Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                frames
            }
            _ => frames,
        }
    }
}

/// Send mouse event to remote, preferring the low-latency QUIC streaming channel.
///
/// Falls back to KDE Connect TCP protocol if streaming is not active.
pub async fn send_mouse_event(
    _address: String,
    _port: u16,
    x: f32,
    y: f32,
    button: i32,
    is_pressed: bool,
) -> Result<(), String> {
    // Try QUIC streaming channel first (lower latency, compact binary protocol)
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    if let Some(conn) = quic_conn {
        let packet = if button == 2 {
            // Scroll event (button=2 is reserved for scroll on the Flutter side)
            InputPacket::MouseScroll {
                dx: x as i16,
                dy: y as i16,
            }
        } else if button != 0 {
            InputPacket::MouseClick {
                button: button as u8,
                pressed: is_pressed,
            }
        } else {
            InputPacket::MouseMove {
                dx: x as i16,
                dy: y as i16,
            }
        };

        let data = packet.encode();
        let mut send_stream = conn
            .open_uni()
            .await
            .map_err(|e| format!("QUIC open stream: {e}"))?;
        send_stream
            .write_all(&data)
            .await
            .map_err(|e| format!("QUIC write: {e}"))?;
        send_stream
            .finish()
            .map_err(|e| format!("QUIC finish: {e}"))?;
        return Ok(());
    }

    // Fall back to TCP/KDE Connect protocol
    let writer_arc = {
        let guard = (*CONTROL_WRITER).lock().await;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "Not connected".to_string())?
    };
    let sender = TcpDeviceSender::from_arc(writer_arc, _address.clone());
    let mut body = serde_json::json!({});
    if x != 0.0 || y != 0.0 {
        body["dx"] = serde_json::json!(x);
        body["dy"] = serde_json::json!(y);
    }
    if button != 0 {
        body["isPressed"] = serde_json::json!(is_pressed);
        body["button"] = serde_json::json!(button);
    }
    let packet = NetworkPacket::new("kdeconnect.mousepad.request").with_body(body);
    sender
        .send_packet(&packet)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Send a normalized absolute pointer position (direct-touch mode).
///
/// Coordinates are 0..=65535 on both axes, resolution-independent. QUIC-only:
/// the legacy KDE Connect TCP protocol has no absolute-position concept.
pub async fn send_mouse_abs(x_norm: u16, y_norm: u16) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn = quic_conn.ok_or_else(|| {
        tracing::warn!("send_mouse_abs: no active QUIC streaming connection");
        "Absolute input requires an active QUIC streaming connection".to_string()
    })?;

    let data = InputPacket::MouseMoveAbs { x_norm, y_norm }.encode();
    let send = async {
        let mut send_stream = conn
            .open_uni()
            .await
            .map_err(|e| format!("QUIC open stream: {e}"))?;
        send_stream
            .write_all(&data)
            .await
            .map_err(|e| format!("QUIC write: {e}"))?;
        send_stream
            .finish()
            .map_err(|e| format!("QUIC finish: {e}"))?;
        Ok(())
    }
    .await;
    if let Err(e) = &send {
        tracing::warn!(error = %e, "send_mouse_abs failed");
    }
    send
}

/// Send a mouse button press/release over QUIC.
///
/// `button` is the wire encoding: 0=Left, 1=Middle, 2=Right, 3=Back, 4=Forward.
/// This exists because `send_mouse_event`'s legacy `button` parameter reserves
/// 0 for "movement", which makes a left click unaddressable through it.
/// QUIC-only, like `send_mouse_abs`.
pub async fn send_mouse_click(button: u8, pressed: bool) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn = quic_conn
        .ok_or_else(|| "Mouse click requires an active QUIC streaming connection".to_string())?;

    let data = InputPacket::MouseClick { button, pressed }.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

/// Parse a Hyprland window address as the windows plugin reports it
/// (`"0x55f6c2d3"`, optionally bare decimal) into the handle the server
/// hands to `hyprland_toplevel_export_v1`. Empty or unparseable → 0, which
/// means "geometry crop only" and keeps the pre-B2 software-crop behavior.
fn parse_window_address(address: &str) -> u64 {
    let s = address.trim();
    if s.is_empty() {
        return 0;
    }
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).unwrap_or(0),
        None => s.parse::<u64>().unwrap_or(0),
    }
}

/// Restrict the server's capture to a window (R3#7 single-window streaming,
/// R4 B2 compositor-side capture). `address` is the Hyprland window address
/// from `get_windows`; when the server understands it, the compositor crops
/// to that window itself (occlusion-correct, no wasted bandwidth). The rect
/// (monitor-local `WindowInfoDto::local_at` + `size`) always travels too —
/// it is what non-Hyprland servers crop with, and what the input mapping
/// uses. Zero width or height clears the crop and restores the full desktop.
/// The server rebuilds the encoder at the captured resolution, so the video
/// stream size follows the window. QUIC-only, like `send_mouse_abs`.
pub async fn send_window_crop(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    address: &str,
) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn = quic_conn
        .ok_or_else(|| "Window crop requires an active QUIC streaming connection".to_string())?;

    let packet = if width == 0 || height == 0 {
        InputPacket::WindowCrop {
            x: None,
            y: None,
            width: None,
            height: None,
            window: 0,
        }
    } else {
        InputPacket::WindowCrop {
            x: Some(x),
            y: Some(y),
            width: Some(width),
            height: Some(height),
            window: parse_window_address(address),
        }
    };
    let data = packet.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

/// Toggle R4 D1 view-only mode on the server: while enabled, the server
/// drops every injectable input packet from this session (mouse, keyboard,
/// gamepad, text) — video keeps streaming. Enforcement is server-side, so
/// a lagging or buggy client cannot inject while latched. The state is
/// per-session: a new stream starts interactive.
pub async fn send_view_only(enabled: bool) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn = quic_conn
        .ok_or_else(|| "View-only requires an active QUIC streaming connection".to_string())?;

    let data = InputPacket::ViewOnly { enabled }.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

/// Toggle R4 A3 full-quality override: while enabled the server skips its
/// relayed-path bitrate clamp and encodes at the configured bitrate even if
/// video is riding an iroh relay (shared, scarce bandwidth). Control-plane
/// packet — never injected as input; per-session like view-only.
pub async fn send_full_quality(enabled: bool) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn = quic_conn.ok_or_else(|| {
        "Full-quality override requires an active QUIC streaming connection".to_string()
    })?;

    let data = InputPacket::FullQuality { enabled }.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

/// R4 E5: pick a named link-profile preset (see `RustCore` id constants:
/// 0 Auto, 1 Quality, 2 Balanced, 3 Economy). Control-plane — the server
/// folds it with the A3 relay floor into the live encoder bitrate. Never
/// injected, and it survives view-only.
pub async fn send_quality_preset(preset: u8) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let conn =
        quic_conn.ok_or_else(|| "Quality preset requires an active QUIC session".to_string())?;

    let data = InputPacket::QualityPreset { preset }.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

// R4 E2 mic-share plumbing: the phone-side Opus encoder is Kotlin's
// MediaCodec (`c2.android.opus.encoder`) — the `opus` crate can't
// cross-compile here (audiopus_sys builds libopus via CMake, which fights
// cargo-ndk) — so the bridge only frames and forwards finished packets.
// Format both ends agree on: 48 kHz mono, 20 ms frames.
async fn stream_connection() -> Result<linux_link_core::streaming::SharedConnection, String> {
    (*STREAMING_HANDLE)
        .lock()
        .await
        .as_ref()
        .map(|h| h.connection.clone())
        .ok_or_else(|| "Mic requires an active streaming session".to_string())
}

async fn send_mic_frame(
    conn: &linux_link_core::streaming::SharedConnection,
    enabled: bool,
    opus: Vec<u8>,
) -> Result<(), String> {
    let data = InputPacket::Mic { enabled, opus }.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;
    Ok(())
}

/// Open the phone→desktop mic: sends the start frame, which makes the
/// server create its "Pocketdesk Mic" PipeWire source. Pairs with
/// `send_mic_opus`/`stop_mic`.
pub async fn start_mic() -> Result<(), String> {
    let conn = stream_connection().await?;
    send_mic_frame(&conn, true, Vec::new()).await
}

/// Push one encoded Opus frame (20 ms, 48 kHz mono) to the desktop.
pub async fn send_mic_opus(opus: Vec<u8>) -> Result<(), String> {
    let conn = stream_connection().await?;
    send_mic_frame(&conn, true, opus).await
}

/// Close the phone→desktop mic: the server removes its virtual source.
/// Best-effort — succeeds silently with no session.
pub async fn stop_mic() -> Result<(), String> {
    if let Ok(conn) = stream_connection().await {
        send_mic_frame(&conn, false, Vec::new()).await?;
    }
    Ok(())
}

/// Send keyboard event to remote, preferring the low-latency QUIC streaming channel.
///
/// Falls back to KDE Connect TCP protocol if streaming is not active.
pub async fn send_keyboard_event(
    _address: String,
    _port: u16,
    key_code: i32,
    text: String,
) -> Result<(), String> {
    // Try QUIC streaming channel first (lower latency, compact binary protocol)
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    if let Some(conn) = quic_conn {
        if !text.is_empty() {
            // Send as text packet
            let packet = InputPacket::Text(text);
            let data = packet.encode();
            let mut send_stream = conn
                .open_uni()
                .await
                .map_err(|e| format!("QUIC open stream: {e}"))?;
            send_stream
                .write_all(&data)
                .await
                .map_err(|e| format!("QUIC write: {e}"))?;
            send_stream
                .finish()
                .map_err(|e| format!("QUIC finish: {e}"))?;
            return Ok(());
        }

        if key_code != 0 {
            // Decode modifier encoding:
            //   key_code > 100000 = modifier release   (subtract 100000)
            //   key_code > 50000  = modifier press      (subtract 50000)
            //   otherwise         = regular key          (press + release)
            const MOD_RELEASE_OFFSET: i32 = 100000;
            const MOD_PRESS_OFFSET: i32 = 50000;

            let evdev_key = android_to_evdev_keycode(if key_code > MOD_RELEASE_OFFSET {
                key_code - MOD_RELEASE_OFFSET
            } else if key_code > MOD_PRESS_OFFSET {
                key_code - MOD_PRESS_OFFSET
            } else {
                key_code
            });

            if key_code > MOD_RELEASE_OFFSET {
                // Modifier release: send single release packet
                let packet = InputPacket::KeyEvent {
                    key: evdev_key,
                    pressed: false,
                };
                let data = packet.encode();
                let mut send_stream = conn
                    .open_uni()
                    .await
                    .map_err(|e| format!("QUIC open stream: {e}"))?;
                send_stream
                    .write_all(&data)
                    .await
                    .map_err(|e| format!("QUIC write: {e}"))?;
                send_stream
                    .finish()
                    .map_err(|e| format!("QUIC finish: {e}"))?;
            } else if key_code > MOD_PRESS_OFFSET {
                // Modifier press: send single press packet
                let packet = InputPacket::KeyEvent {
                    key: evdev_key,
                    pressed: true,
                };
                let data = packet.encode();
                let mut send_stream = conn
                    .open_uni()
                    .await
                    .map_err(|e| format!("QUIC open stream: {e}"))?;
                send_stream
                    .write_all(&data)
                    .await
                    .map_err(|e| format!("QUIC write: {e}"))?;
                send_stream
                    .finish()
                    .map_err(|e| format!("QUIC finish: {e}"))?;
            } else {
                // Regular key: press + release (current behavior)
                for pressed in [true, false] {
                    let packet = InputPacket::KeyEvent {
                        key: evdev_key,
                        pressed,
                    };
                    let data = packet.encode();
                    let mut send_stream = conn
                        .open_uni()
                        .await
                        .map_err(|e| format!("QUIC open stream: {e}"))?;
                    send_stream
                        .write_all(&data)
                        .await
                        .map_err(|e| format!("QUIC write: {e}"))?;
                    send_stream
                        .finish()
                        .map_err(|e| format!("QUIC finish: {e}"))?;
                }
            }
            return Ok(());
        }

        return Ok(());
    }

    // Fall back to TCP/KDE Connect protocol
    let writer_arc = {
        let guard = (*CONTROL_WRITER).lock().await;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| "Not connected".to_string())?
    };
    let sender = TcpDeviceSender::from_arc(writer_arc, _address.clone());
    let mut body = serde_json::json!({});
    if !text.is_empty() {
        body["text"] = serde_json::json!(text);
    }
    if key_code != 0 {
        let key_str: String = match key_code {
            66 => "Enter".to_string(),
            67 => "Backspace".to_string(),
            19 => "Up".to_string(),
            20 => "Down".to_string(),
            21 => "Left".to_string(),
            22 => "Right".to_string(),
            62 => "Space".to_string(),
            _ => format!("Key{}", key_code),
        };
        body["key"] = serde_json::json!(key_str);
    }
    let packet = NetworkPacket::new("kdeconnect.mousepad.request").with_body(body);
    sender
        .send_packet(&packet)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Send gamepad state over the QUIC streaming channel.
///
/// `axes` contains 6 i16 values: [LX, LY, RX, RY, L2, R2] in range -32768..32767.
/// `buttons` is a 16-bit bitmask (A=0, B=1, X=2, Y=3, LB=4, RB=5,
/// Select=6, Start=7, Home=8, LSB=9, RSB=10, DPadUp=11..DPadRight=14).
/// Requires an active streaming session — no TCP fallback for gamepad.
pub async fn send_gamepad_event(axes: Vec<i16>, buttons: u32) -> Result<(), String> {
    let quic_conn = {
        let guard = (*STREAMING_HANDLE).lock().await;
        if let Some(h) = guard.as_ref() {
            Some(h.connection.clone())
        } else {
            let v2_guard = (*crate::V2_HANDLE).lock().await;
            v2_guard
                .as_ref()
                .map(|h| QuinnConnection::shared(h.connection.clone()))
        }
    };

    let Some(conn) = quic_conn else {
        return Err("No active streaming session for gamepad input".to_string());
    };

    // Pad or truncate axes to 6
    let mut axis_array = [0i16; 6];
    for (i, &val) in axes.iter().enumerate().take(6) {
        axis_array[i] = val;
    }

    let packet = InputPacket::Gamepad {
        axes: axis_array,
        buttons: buttons as u16,
    };

    let data = packet.encode();
    let mut send_stream = conn
        .open_uni()
        .await
        .map_err(|e| format!("QUIC open stream: {e}"))?;
    send_stream
        .write_all(&data)
        .await
        .map_err(|e| format!("QUIC write: {e}"))?;
    send_stream
        .finish()
        .map_err(|e| format!("QUIC finish: {e}"))?;

    Ok(())
}

/// Map Android `KeyEvent.KEYCODE_*` to Linux evdev `KEY_*` codes.
///
/// Values verified against `android.view.KeyEvent` (SDK 37) and
/// `linux/input-event-codes.h`. Unknown codes map to 0 (KEY_RESERVED), which
/// the server ignores — safer than passing an Android number through as a
/// different evdev key.
fn android_to_evdev_keycode(android_keycode: i32) -> u16 {
    match android_keycode {
        // Navigation / system
        3 => 102,   // HOME -> KEY_HOME
        4 => 1,     // BACK -> KEY_ESC
        19 => 103,  // DPAD_UP
        20 => 108,  // DPAD_DOWN
        21 => 105,  // DPAD_LEFT
        22 => 106,  // DPAD_RIGHT
        23 => 28,   // DPAD_CENTER -> KEY_ENTER
        24 => 115,  // VOLUME_UP
        25 => 114,  // VOLUME_DOWN
        82 => 139,  // MENU -> KEY_MENU (the application-keys key; KEY_COMPOSE is 127)
        85 => 164,  // MEDIA_PLAY_PAUSE
        87 => 163,  // MEDIA_NEXT
        88 => 165,  // MEDIA_PREVIOUS
        92 => 104,  // PAGE_UP
        93 => 109,  // PAGE_DOWN
        111 => 1,   // ESCAPE
        120 => 99,  // SYSRQ
        123 => 107, // END
        323 => 99,  // PRINT -> KEY_SYSRQ
        // Editing
        61 => 15,   // TAB
        62 => 57,   // SPACE
        66 => 28,   // ENTER
        67 => 14,   // DEL (backspace) -> KEY_BACKSPACE
        112 => 111, // FORWARD_DEL -> KEY_DELETE
        115 => 58,  // CAPS_LOCK
        // Modifiers
        57 => 56,   // ALT_LEFT
        58 => 100,  // ALT_RIGHT
        59 => 42,   // SHIFT_LEFT
        60 => 54,   // SHIFT_RIGHT
        113 => 29,  // CTRL_LEFT
        114 => 97,  // CTRL_RIGHT
        117 => 125, // META_LEFT (Super)
        118 => 126, // META_RIGHT (Super)
        // Digits (Android 0..9 = 7..16 -> evdev 11, 2..10)
        7 => 11,
        8 => 2,
        9 => 3,
        10 => 4,
        11 => 5,
        12 => 6,
        13 => 7,
        14 => 8,
        15 => 9,
        16 => 10,
        // Punctuation
        17 => 55, // STAR -> KEY_KPASTERISK
        55 => 51, // COMMA
        56 => 52, // PERIOD
        68 => 41, // GRAVE
        69 => 12, // MINUS
        70 => 13, // EQUALS
        71 => 26, // LEFT_BRACKET
        72 => 27, // RIGHT_BRACKET
        74 => 39, // SEMICOLON
        75 => 40, // APOSTROPHE
        76 => 53, // SLASH
        78 => 69, // NUM -> KEY_NUMLOCK
        81 => 78, // PLUS -> KEY_KPPLUS
        // Letters A..Z (Android 29..54, evdev non-contiguous)
        29 => 30, // A
        30 => 48, // B
        31 => 46, // C
        32 => 32, // D
        33 => 18, // E
        34 => 33, // F
        35 => 34, // G
        36 => 35, // H
        37 => 23, // I
        38 => 36, // J
        39 => 37, // K
        40 => 38, // L
        41 => 50, // M
        42 => 49, // N
        43 => 24, // O
        44 => 25, // P
        45 => 16, // Q
        46 => 19, // R
        47 => 31, // S
        48 => 20, // T
        49 => 22, // U
        50 => 47, // V
        51 => 17, // W
        52 => 45, // X
        53 => 21, // Y
        54 => 44, // Z
        // F1..F10 (Android 131..140 -> evdev 59..68). The evdev function keys
        // stop being contiguous there: F11 and F12 are 87 and 88, while 69 and
        // 70 are NumLock and F1 — so extending the range would have turned the
        // phone's F11 into a NumLock press on the desktop.
        131..=140 => (android_keycode - 72) as u16,
        141 => 87, // F11 -> KEY_F11
        142 => 88, // F12 -> KEY_F12
        _ => 0,
    }
}

#[cfg(test)]
mod keymap_tests {
    use super::android_to_evdev_keycode as k;

    #[test]
    fn letters_map_to_qwerty_evdev() {
        assert_eq!(k(29), 30); // A -> KEY_A
        assert_eq!(k(30), 48); // B -> KEY_B
        assert_eq!(k(54), 44); // Z -> KEY_Z
    }

    #[test]
    fn digits_map_to_evdev() {
        assert_eq!(k(8), 2); // 1
        assert_eq!(k(16), 10); // 9
        assert_eq!(k(7), 11); // 0
    }

    #[test]
    fn modifiers_map_to_evdev() {
        assert_eq!(k(117), 125); // META_LEFT -> KEY_LEFTMETA
        assert_eq!(k(57), 56); // ALT_LEFT -> KEY_LEFTALT
        assert_eq!(k(113), 29); // CTRL_LEFT -> KEY_LEFTCTRL
    }

    #[test]
    fn function_keys_map_to_evdev() {
        assert_eq!(k(131), 59); // F1
        assert_eq!(k(140), 68); // F10 — where evdev stops being contiguous
        assert_eq!(k(141), 87, "F11 is not KEY_NUMLOCK");
        assert_eq!(k(142), 88, "F12 is not KEY_SCROLLLOCK");
    }

    /// The client half of the key contract in `core::input::keys`: whatever this
    /// table produces must be inside the set the server is required to cover,
    /// and the set must not claim keys this table cannot produce. A new row here
    /// without a mapping there (or vice versa) fails a test instead of arriving
    /// on a desktop as a dead key.
    #[test]
    fn emitted_codes_are_exactly_the_contracted_set() {
        let mut emitted: Vec<u16> = (0..1000)
            .map(k)
            .filter(|&code| code != 0) // KEY_RESERVED: an unknown Android key, ignored
            .collect();
        emitted.sort_unstable();
        emitted.dedup();
        assert_eq!(
            emitted,
            linux_link_core::input::keys::phone_emittable_codes()
        );
    }

    #[test]
    fn delete_vs_backspace() {
        assert_eq!(k(67), 14); // DEL -> KEY_BACKSPACE
        assert_eq!(k(112), 111); // FORWARD_DEL -> KEY_DELETE
        assert_eq!(k(24), 115); // VOLUME_UP -> KEY_VOLUMEUP
    }

    #[test]
    fn unknown_codes_are_reserved() {
        assert_eq!(k(9999), 0);
    }
}

#[cfg(test)]
mod pin_certificate_tests {
    use super::{forget_peer_certificate_in, pin_peer_certificate_in};
    use linux_link_core::streaming::transport::{CertManager, peer_label_for_ip};

    #[test]
    fn a_pairing_code_fingerprint_is_pinned_under_the_verifiers_label() {
        let dir = std::env::temp_dir().join(format!("bridge-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let hex = "ab".repeat(32);
        pin_peer_certificate_in(&dir, "192.168.1.101", &hex).unwrap();
        let label = peer_label_for_ip("192.168.1.101".parse().unwrap());
        let peers = CertManager::load_or_create(&dir).unwrap().known_peers();
        assert!(peers.contains(&(label, [0xab; 32])));
    }

    /// A pairing link must not silently replace the certificate this phone
    /// already trusts for an address: anything can open a link, and a
    /// different fingerprint there means a different (possibly hostile)
    /// computer. The same fingerprint again is fine.
    #[test]
    fn a_link_cannot_replace_the_certificate_already_trusted_for_an_address() {
        let dir = std::env::temp_dir().join(format!("bridge-pin-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pin_peer_certificate_in(&dir, "10.0.0.7", &"ab".repeat(32)).unwrap();
        pin_peer_certificate_in(&dir, "10.0.0.7", &"ab".repeat(32)).unwrap();
        let err = pin_peer_certificate_in(&dir, "10.0.0.7", &"cd".repeat(32)).unwrap_err();
        assert!(err.contains("different"), "{err}");
        let label = peer_label_for_ip("10.0.0.7".parse().unwrap());
        let peers = CertManager::load_or_create(&dir).unwrap().known_peers();
        assert!(
            peers.contains(&(label, [0xab; 32])),
            "the trusted pin is kept"
        );
    }

    #[test]
    fn removing_a_computer_lets_a_new_code_pin_its_address_again() {
        let dir = std::env::temp_dir().join(format!("bridge-pin-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        pin_peer_certificate_in(&dir, "10.0.0.8", &"ab".repeat(32)).unwrap();
        assert!(forget_peer_certificate_in(&dir, "10.0.0.8").unwrap());
        pin_peer_certificate_in(&dir, "10.0.0.8", &"cd".repeat(32)).unwrap();
        assert!(
            !forget_peer_certificate_in(&dir, "10.0.0.9").unwrap(),
            "nothing pinned there"
        );
    }

    #[test]
    fn malformed_input_is_refused() {
        let dir = std::env::temp_dir().join(format!("bridge-pin-bad-{}", std::process::id()));
        assert!(pin_peer_certificate_in(&dir, "not-an-ip", &"ab".repeat(32)).is_err());
        assert!(pin_peer_certificate_in(&dir, "10.0.0.1", "abcd").is_err());
        assert!(pin_peer_certificate_in(&dir, "10.0.0.1", &"zz".repeat(32)).is_err());
    }
}
