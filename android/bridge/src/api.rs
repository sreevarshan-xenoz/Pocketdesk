//! Client session API ported from the Flutter-era bridge (git 4bf3c73).
//! Plain polling functions; JNI wrappers live in lib.rs.

use pocketdesk_core::protocol::connection::{ConnectionManager, ControlDuplex};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TcpDeviceSender};
use pocketdesk_core::protocol::transfer::{self, Pace, TransferError};
use pocketdesk_core::protocol::v2::{ALPN_V2, IdentityPacketV2, perform_v2_handshake};
use pocketdesk_core::streaming::QuinnConnection;
use pocketdesk_core::streaming::StreamingClient;
use pocketdesk_core::tailscale::TailscaleClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::sync::broadcast;

use pocketdesk_core::streaming::InputPacket;
use pocketdesk_core::streaming::{SAMPLE_DECODE, SAMPLE_RENDER, report_samples};

use crate::{
    AUDIO_RX, CONNECTION_STATE, CONTROL_PEER, CONTROL_WRITER, DECODE_SAMPLES,
    MAX_AUDIO_PACKETS_PER_RECEIVE, MAX_FRAMES_PER_RECEIVE, RATE_SNAPSHOTS, RENDER_SAMPLES,
    RTT_HISTORY, STREAM_CLOSED_REASON, STREAMING_ACTIVE, STREAMING_BYTE_COUNT,
    STREAMING_FRAME_COUNT, STREAMING_HANDLE, StreamingHandle, VIDEO_RX, update_streaming_rtt,
};

/// Initialize the Pocketdesk backend
pub fn init_app() {
    crate::init_app_impl();
}

/// Set the persistent data directory for certs and state.
/// Must be called before `connect_streaming` for cert persistence.
pub fn set_data_dir(path: String) {
    let cert_path = PathBuf::from(&path).join("pocketdesk").join("certs");
    let mut guard = crate::CERT_DIR.lock().unwrap();
    *guard = Some(cert_path);
    tracing::info!("Data directory set to: {path}");
}

/// Pin a desktop's certificate before the first connection to `address`,
/// from a scanned pairing code (`fp`: hex SHA-256 of its certificate), so
/// that connection is verified rather than trusted on first use. The pin
/// covers the stream and the control channel (phase B), so the PIN that
/// pairs travels to the right computer. Refused when a different
/// certificate is already trusted for the address (see below).
pub fn pin_peer_certificate(address: String, fingerprint_hex: String) -> Result<bool, String> {
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    pin_peer_certificate_in(&dir, &address, &fingerprint_hex)
}

/// Returns whether the pin is new (`false`: this exact pin was already
/// there), so a failed pairing can take back only what it added.
pub(crate) fn pin_peer_certificate_in(
    cert_dir: &std::path::Path,
    address: &str,
    fingerprint_hex: &str,
) -> Result<bool, String> {
    let ip: std::net::IpAddr = address
        .trim()
        .parse()
        .map_err(|_| format!("not an IP address: {address}"))?;
    let hex = fingerprint_hex.trim();
    // ASCII hex only, checked before any slicing: the length is in bytes, so
    // a multi-byte character could put a slice boundary inside it, and a
    // panic here would abort the app (this runs under a JNI export).
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("a certificate fingerprint is 64 hex digits".to_string());
    }
    let mut hash = [0u8; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "the fingerprint is not hex".to_string())?;
    }
    let manager = pocketdesk_core::streaming::transport::CertManager::load_or_create(cert_dir)
        .map_err(|e| e.to_string())?;
    let label = pocketdesk_core::streaming::transport::peer_label_for_ip(ip);
    // Anything can open a pairing link, so one must never replace the
    // certificate this phone already trusts for an address: a different
    // fingerprint there is a different computer. Removing the computer in
    // the app clears the old trust first.
    match manager
        .known_peers()
        .iter()
        .find(|(known, _)| *known == label)
    {
        Some((_, pinned)) if *pinned == hash => return Ok(false),
        Some(_) => {
            return Err(format!(
                "this phone already trusts a different computer at {ip}; remove it first"
            ));
        }
        None => {}
    }
    manager.pin_peer(&label, hash).map_err(|e| e.to_string())?;
    tracing::info!("Pinned the certificate of {ip} from a pairing code");
    Ok(true)
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
    // The label the control channel pinned it under: its IP, or the name a
    // computer saved by name was dialed with.
    let Some(label) = pocketdesk_core::streaming::transport::peer_label_for_address(address) else {
        return Ok(false);
    };
    let manager = pocketdesk_core::streaming::transport::CertManager::load_or_create(cert_dir)
        .map_err(|e| e.to_string())?;
    manager.remove_peer(&label).map_err(|e| e.to_string())
}

/// Check if the LAN discovery service is active.
pub fn is_discovery_active() -> bool {
    crate::MDNS_ACTIVE.load(Ordering::Acquire)
}

/// Get version string
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

use pocketdesk_core::error::PocketdeskError;

/// Structured error information for the client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PocketdeskErrorDto {
    pub code: u32,
    pub message: String,
    pub is_retryable: bool,
}

impl From<PocketdeskError> for PocketdeskErrorDto {
    fn from(e: PocketdeskError) -> Self {
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
    Error(PocketdeskErrorDto),
}

/// Connection state enumeration for the client (legacy control channel state).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Connected,
    Disconnected,
    Connecting,
    Error(PocketdeskErrorDto),
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
pub(crate) fn client_identity() -> pocketdesk_core::protocol::kdeconnect::DeviceIdentity {
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
            let identity = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
                &trimmed,
                "Pocketdesk Android Client",
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

    let identity = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
        &new_id,
        "Pocketdesk Android Client",
    );
    *guard = Some(identity.clone());
    identity
}

/// Wake a sleeping computer from this phone (N6): send the magic packet for
/// `mac` to each `host:port` in `targets_json` (broadcasts on this network,
/// or an outside address whose router forwards it). Names are resolved here;
/// one that cannot be is skipped. Returns how many addresses took it.
pub fn wake(mac: String, targets_json: String) -> Result<usize, String> {
    use std::net::ToSocketAddrs;
    let targets: Vec<String> =
        serde_json::from_str(&targets_json).map_err(|e| format!("bad wake targets: {e}"))?;
    let mut addrs = Vec::new();
    let mut unresolved = Vec::new();
    for target in &targets {
        match target.to_socket_addrs() {
            Ok(found) => addrs.extend(found),
            Err(_) => unresolved.push(target.as_str()),
        }
    }
    if addrs.is_empty() {
        return Err(if unresolved.is_empty() {
            "nowhere to send the wake to".to_string()
        } else {
            format!("could not find {}", unresolved.join(", "))
        });
    }
    pocketdesk_core::tailscale::wol::send_wake(&mac, &addrs).map_err(|e| e.to_string())
}

/// How the computer at `host`:`port` said it can be woken (N6
/// `kdeconnect.pocketdesk.wake_info` body JSON), or `None` when the cached
/// one came from another computer, or none came.
pub async fn get_wake_info(host: &str, port: u16) -> Option<String> {
    wake_info_for(crate::WAKE_INFO.lock().await.as_ref(), host, port)
}

/// Who drives, as the computer at `host`:`port` last said (N7
/// `kdeconnect.pocketdesk.control` body JSON), or `None`.
pub async fn get_control_state(host: &str, port: u16) -> Option<String> {
    wake_info_for(crate::CONTROL_STATE.lock().await.as_ref(), host, port)
}

/// The cached body when it came from `host`:`port`.
fn wake_info_for(slot: Option<&(String, u16, String)>, host: &str, port: u16) -> Option<String> {
    slot.filter(|(from_host, from_port, _)| from_host == host && *from_port == port)
        .map(|(_, _, body)| body.clone())
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
    let request = NetworkPacket::new("kdeconnect.pocketdesk.wol").with_body(serde_json::json!({
        "mac": mac,
        "broadcast": broadcast,
    }));
    relay_outcome(
        query_peer(
            &address,
            port,
            &request,
            "kdeconnect.pocketdesk.wol",
            Duration::from_secs(5),
        )
        .await,
    )
}

/// What the relay computer's answer means. It answers `{ok, error}` once it
/// has sent the magic packet, or refused to; no answer means it dropped the
/// request, which is what it does when this phone may not use Power there.
fn relay_outcome(answer: Result<serde_json::Value, String>) -> Result<(), String> {
    let body = answer.map_err(|e| {
        format!("The relay computer did not answer ({e}). Is Power allowed for this phone on it?")
    })?;
    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    let reason = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("no reason given");
    Err(format!("The relay computer refused: {reason}"))
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
    let lan_peers = pocketdesk_core::tailscale::lan::scan_lan_once().await;
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

/// A control channel manager: this phone's certificate and its pins of
/// desktops, read fresh so a pin set since (a pairing code, the stream)
/// applies. The control channel runs inside TLS (phase B), so without a data
/// directory there is none.
pub(crate) fn control_manager(timeout: Duration) -> Result<ConnectionManager, String> {
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    let certs = pocketdesk_core::streaming::transport::CertManager::load_or_create(&dir)
        .map_err(|e| format!("This phone's certificate: {e}"))?;
    Ok(ConnectionManager::new(
        timeout,
        certs.control_client_config(),
    ))
}

/// Who answers at `address:port`, compared with this phone's pin for that
/// address, for the Connection Doctor (T1): `matches`, `changed`,
/// `not_pinned`, `not_pocketdesk`, `closed` or `failed`. Never saves a pin.
pub async fn check_identity(address: String, port: u16) -> Result<String, String> {
    use pocketdesk_core::streaming::transport::IdentityProbe;
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    let certs = pocketdesk_core::streaming::transport::CertManager::load_or_create(&dir)
        .map_err(|e| format!("This phone's certificate: {e}"))?;
    let verdict = certs
        .probe_identity(&address, port, Duration::from_secs(3))
        .await;
    Ok(match verdict {
        IdentityProbe::Matches => "matches",
        IdentityProbe::Changed => "changed",
        IdentityProbe::NotPinned => "not_pinned",
        IdentityProbe::NotPocketdesk => "not_pocketdesk",
        IdentityProbe::Closed => "closed",
        IdentityProbe::Failed => "failed",
    }
    .to_string())
}

/// Connect to a peer: TLS over TCP, and when that fails and the computer's
/// WAN identity (`wan_identity`, the `kdeconnect.pocketdesk.endpoint` body
/// cached at home) is known, over the internet road (T10a).
pub async fn connect_to_peer(
    address: String,
    port: u16,
    wan_identity: Option<String>,
) -> Result<ConnectionState, String> {
    // Kotlin calls this from several poll loops (clipboard sync, quick
    // settings, retry paths) without coordinating. A second call while the
    // first connection is still up would tear down a working control session
    // and re-register the same device_id, which the server reads as a
    // reconnect storm. Reuse what is already connected.
    let _one_at_a_time = crate::session::CONNECT_LOCK.lock().await;
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

    let conn_mgr = control_manager(Duration::from_secs(10))?;
    let mut state_guard = (*CONNECTION_STATE).lock().await;
    *state_guard = ConnectionState::Connecting;

    let identity = client_identity();

    let conn_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!(
        "conn",
        id = %conn_id,
        session = %session_id,
        peer = %address,
        transport = "tcp",
        proto = %pocketdesk_core::protocol::PROTOCOL_VERSION
    );

    use tracing::Instrument;

    let result = async move {
        // The stream, and whether it came by the internet road.
        let dialed: Result<(ControlDuplex, bool), _> =
            match conn_mgr.connect(&address, port, &identity).await {
                Ok(stream) => Ok((Box::new(stream) as ControlDuplex, false)),
                Err(home) => match wan_identity.as_deref().map(WanTarget::parse) {
                    Some(Ok(target)) if INTERNET_PAUSE.may_try(&address, Instant::now()) => {
                        tracing::info!(
                            error = %home,
                            "No control link at home; trying the internet road"
                        );
                        match dial_internet_control(&target, &identity, &address, true).await {
                            Ok(stream) => {
                                INTERNET_PAUSE.succeeded(&address);
                                Ok((stream, true))
                            }
                            Err(e) => {
                                tracing::warn!("The internet road failed too: {e}");
                                INTERNET_PAUSE.failed(&address, Instant::now());
                                Err(home)
                            }
                        }
                    }
                    Some(Ok(_)) => {
                        tracing::debug!("The internet road failed a moment ago; not yet again");
                        Err(home)
                    }
                    Some(Err(e)) => {
                        tracing::warn!("Unusable WAN identity: {e}");
                        Err(home)
                    }
                    None => Err(home),
                },
            };
        match dialed {
            Ok((stream, over_internet)) => {
                let (reader, writer) = tokio::io::split(stream);
                let writer_arc = Arc::new(Mutex::new(writer));
                let replaced = (*CONTROL_WRITER).lock().await.replace(writer_arc.clone());
                // A connection this one replaces (to another computer, or one
                // the check above found dead) is closed, so its reader ends
                // instead of holding the socket and a server slot.
                if let Some(old) = replaced {
                    let _ = old.lock().await.shutdown().await;
                }
                *crate::CONTROL_PEER.lock().await = Some((address.clone(), port));
                crate::CONTROL_OVER_INTERNET.store(over_internet, Ordering::SeqCst);

                // Create a broadcast channel for forwarding incoming packets to Flutter
                let (packet_tx, _) = broadcast::channel(256);
                {
                    let mut incoming = crate::INCOMING_PACKETS.lock().await;
                    *incoming = Some(packet_tx.clone());
                }
                // Which computer this connection is to: a reply request is
                // acted on only when it came from the phone's own computer.
                let from_peer = (address.clone(), port);

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
                                if trimmed.contains("pocketdesk.endpoint")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.pocketdesk.endpoint"
                                {
                                    let mut id = crate::WAN_IDENTITY.lock().await;
                                    *id = Some(pkt.body.to_string());
                                }
                                // Who drives, when several phones are
                                // connected (N7).
                                if trimmed.contains("pocketdesk.control")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.pocketdesk.control"
                                {
                                    *crate::CONTROL_STATE.lock().await = Some((
                                        from_peer.0.clone(),
                                        from_peer.1,
                                        pkt.body.to_string(),
                                    ));
                                }
                                // How to wake this desktop (N6), kept for the
                                // app to save with the computer.
                                if trimmed.contains("pocketdesk.wake_info")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.pocketdesk.wake_info"
                                {
                                    *crate::WAKE_INFO.lock().await = Some((
                                        from_peer.0.clone(),
                                        from_peer.1,
                                        pkt.body.to_string(),
                                    ));
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
                                        || pkt.packet_type == "kdeconnect.pocketdesk.pair")
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
                                // A reply the desktop asks us to text (N5b):
                                // latched for the app to send.
                                if trimmed.contains("kdeconnect.sms.request")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.sms.request"
                                {
                                    crate::queue_sms_request(&pkt.body, &from_peer.0, from_peer.1);
                                }
                                // A file the desktop's owner sent us (F7b):
                                // latched for the session to download.
                                if trimmed.contains("file_offer")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == crate::download::FILE_OFFER
                                {
                                    crate::queue_file_offer(&pkt.body);
                                }
                                // A task's result (ecosystem 1c), kept for
                                // the app under its run's id.
                                if trimmed.contains("task_result")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == crate::tasks::TASK_RESULT
                                {
                                    crate::tasks::receive(&pkt.body);
                                }
                                // A link the computer handed over (T8a),
                                // kept for the app to show.
                                if trimmed.contains("pocketdesk.link")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == pocketdesk_core::protocol::link::LINK
                                {
                                    crate::links::receive(&pkt.body);
                                }
                                // The computer's requests to browse this
                                // phone's shared folders (T10g).
                                if trimmed.contains("phone_files.")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                {
                                    crate::phone_file_requests::receive(
                                        &pkt.packet_type,
                                        &pkt.body,
                                        &from_peer.0,
                                        from_peer.1,
                                    );
                                }
                                // The computer's answers to this phone's
                                // notifications (T10c), kept for the app.
                                if trimmed.contains("kdeconnect.notification.")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                {
                                    crate::notification_answers::receive(
                                        &pkt.packet_type,
                                        &pkt.body,
                                        &from_peer.0,
                                        from_peer.1,
                                    );
                                }
                                // The computer's clipboard (T10b), kept
                                // for the app; still passed on, since it is
                                // also the answer to the phone's request.
                                if trimmed.contains("kdeconnect.clipboard")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == "kdeconnect.clipboard"
                                {
                                    crate::clipboard_push::receive(
                                        &pkt.body,
                                        &from_peer.0,
                                        from_peer.1,
                                    );
                                }
                                // A press for the phone's player (T8d),
                                // kept for the app to act on.
                                if trimmed.contains("phone_media_control")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type
                                        == pocketdesk_core::protocol::phone_media::PHONE_MEDIA_CONTROL
                                {
                                    crate::media_presses::receive(&pkt.body);
                                }
                                // The answer to a folder request (T6b),
                                // kept by the path asked.
                                if trimmed.contains("folder_fetch_reply")
                                    && let Ok(pkt) = NetworkPacket::from_wire(&trimmed)
                                    && pkt.packet_type == crate::download::FOLDER_FETCH_REPLY
                                {
                                    crate::latch_folder_reply(&pkt.body);
                                }
                                let latched = NetworkPacket::from_wire(&trimmed)
                                    .is_ok_and(|p| crate::kept_from_pollers(&p.packet_type));
                                if !latched {
                                    let _ = packet_tx.send(trimmed);
                                }
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
                        crate::CONTROL_OVER_INTERNET.store(false, Ordering::SeqCst);
                        *crate::INCOMING_PACKETS.lock().await = None;
                        let mut state_guard = (*CONNECTION_STATE).lock().await;
                        *state_guard = ConnectionState::Disconnected;
                        let mut writer_guard = (*CONTROL_WRITER).lock().await;
                        *writer_guard = None;
                        *crate::WAN_IDENTITY.lock().await = None;
                        *crate::WAKE_INFO.lock().await = None;
                        *crate::CONTROL_STATE.lock().await = None;
                        crate::SIREN_RINGING.store(false, std::sync::atomic::Ordering::SeqCst);
                        *crate::PAIR_RESULT.lock().await = None;
                        // Offers came from this desktop; another one cannot
                        // fill them (F7b).
                        crate::PENDING_OFFERS
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clear();
                        // So did reply requests (N5b): none outlives the
                        // connection that carried it.
                        crate::PENDING_SMS_REQUESTS
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clear();
                        // And links (T8a), shown as from the computer of
                        // the session that takes them.
                        crate::links::forget();
                        crate::media_presses::forget();
                        crate::clipboard_push::forget();
                        crate::notification_answers::forget();
                        crate::phone_file_requests::forget();
                    }
                    .instrument(tracing::debug_span!("control_reader")),
                );

                tracing::info!("Connected to peer");
                Ok(ConnectionState::Connected)
            }
            Err(e) => {
                tracing::error!(error = %e, "Connection failed");
                Ok(ConnectionState::Error(PocketdeskErrorDto::from(e)))
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

/// The server's cached iroh WAN identity (raw `kdeconnect.pocketdesk.endpoint`
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

/// Nothing pairs over the internet road (T10a): the desktop drops pairing
/// packets there, so asking on it would only wait out a timeout.
async fn pairing_road_check(address: &str) -> Result<(), String> {
    let over_internet = crate::CONTROL_OVER_INTERNET.load(Ordering::SeqCst)
        && (*CONTROL_PEER)
            .lock()
            .await
            .as_ref()
            .is_some_and(|(host, _)| host == address);
    if over_internet {
        Err("Pairing needs the same network as the computer, or Tailscale".to_string())
    } else {
        Ok(())
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
/// when `pocketdesk pair` already put a PIN up for manual entry, or `"error"`.
pub async fn request_pair_pin(address: String, port: u16) -> Result<String, String> {
    pairing_road_check(&address).await?;
    let request = NetworkPacket::new("kdeconnect.pocketdesk.pair")
        .with_body(serde_json::json!({ "requestPin": true }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.pair",
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

/// Submit an externally-entered pairing PIN (`pocketdesk pair` flow, or the
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
    pairing_road_check(&address).await?;
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
/// control reader from pushed `kdeconnect.pair` / `kdeconnect.pocketdesk.pair`
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

/// The computer the live control connection is to, as JSON `{host, port}`,
/// or `None` when there is none.
pub async fn control_peer() -> Option<String> {
    let writer_live = (*CONTROL_WRITER).lock().await.is_some();
    let peer = crate::CONTROL_PEER.lock().await.clone();
    peer.filter(|_| writer_live)
        .map(|(host, port)| serde_json::json!({ "host": host, "port": port }).to_string())
}

/// Drain the replies the desktop asked this phone to text (N5b), as a JSON
/// array of `{id, address, body, fromHost, fromPort}`: the computer each
/// came from, so only the phone's own computer's are acted on.
pub fn take_sms_requests() -> String {
    let drained = {
        let mut q = crate::PENDING_SMS_REQUESTS.lock().expect("sms queue");
        std::mem::take(&mut *q)
    };
    serde_json::to_string(&drained).unwrap_or_else(|_| "[]".to_string())
}

/// The packets the app may send through [`send_phone_packet`]: its texts
/// and the results of the replies it sent (N5b), its battery (T8b), and
/// what it plays (T8d).
const PHONE_PACKETS: [&str; 5] = [
    "kdeconnect.sms.messages",
    "kdeconnect.pocketdesk.sms_sent",
    "kdeconnect.battery",
    pocketdesk_core::protocol::phone_media::PHONE_MEDIA,
    pocketdesk_core::protocol::phone_files::LISTING,
];

/// Send one of [`PHONE_PACKETS`] with `body` (a JSON object) to the desktop.
pub async fn send_phone_packet(
    address: String,
    port: u16,
    packet_type: String,
    body: String,
) -> Result<(), String> {
    let packet = phone_packet(packet_type, &body)?;
    push_packet(&address, port, &packet).await
}

/// [`send_phone_packet`] only over the control link already up to that
/// computer, never a connection of its own, within a few seconds: for what
/// the background sends because some other feature keeps the link (the
/// battery), which must not dial when the link has just gone.
pub async fn send_phone_packet_on_link(
    address: String,
    port: u16,
    packet_type: String,
    body: String,
) -> Result<(), String> {
    let packet = phone_packet(packet_type, &body)?;
    tokio::time::timeout(
        Duration::from_secs(3),
        send_on_live(&address, port, &packet),
    )
    .await
    .map_err(|_| "The computer did not take it in time".to_string())?
}

/// A packet the phone may send of itself ([`PHONE_PACKETS`]), with a body
/// that is a JSON object.
fn phone_packet(packet_type: String, body: &str) -> Result<NetworkPacket, String> {
    if !PHONE_PACKETS.contains(&packet_type.as_str()) {
        return Err(format!("{packet_type} is not sent this way"));
    }
    let body: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid {packet_type}: {e}"))?;
    if !body.is_object() {
        return Err(format!("Invalid {packet_type}: not an object"));
    }
    Ok(NetworkPacket::new(packet_type).with_body(body))
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
    let request = NetworkPacket::new("kdeconnect.pocketdesk.notification_reply")
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
/// Whether `reply` answers `request`: a reply that names an action (the
/// audio replies do, T8c) answers only a request that named the same one,
/// since every audio action replies with the same packet type. A reply
/// left over from a request that timed out is skipped, not taken.
fn reply_answers(request: &serde_json::Value, reply: &serde_json::Value) -> bool {
    match (
        request.get("action").and_then(|a| a.as_str()),
        reply.get("action").and_then(|a| a.as_str()),
    ) {
        (Some(asked), Some(answered)) => asked == answered,
        _ => true,
    }
}

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
    // answer as this one's.
    let writer = live_writer_for(address, port).await;
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
                    && reply_answers(&request.body, &packet.body)
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

#[cfg(test)]
mod reply_tests {
    use serde_json::json;

    /// Several audio requests share one reply type: a reply naming another
    /// request's action is not this one's (T8c).
    #[test]
    fn a_reply_naming_another_action_does_not_answer() {
        let media = json!({"action": "media"});
        assert!(super::reply_answers(
            &media,
            &json!({"action": "media", "ok": true})
        ));
        assert!(!super::reply_answers(
            &media,
            &json!({"action": "status", "volume": 40})
        ));
        // Replies that name no action, and requests that name none, as before.
        assert!(super::reply_answers(&media, &json!({"volume": 40})));
        assert!(super::reply_answers(
            &json!({}),
            &json!({"action": "status"})
        ));
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
    let conn_mgr = control_manager(Duration::from_secs(5))?;
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
                    && reply_answers(&request.body, &packet.body)
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
pub(crate) async fn query_peer(
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

/// Send `packet` on the live control connection to `address`:`port` only,
/// never a one-off one: what answers it arrives on the live one.
pub(crate) async fn send_on_live(
    address: &str,
    port: u16,
    packet: &NetworkPacket,
) -> Result<(), String> {
    let writer = live_writer_for(address, port)
        .await
        .ok_or("Not connected to this computer: open a session first")?;
    TcpDeviceSender::from_arc(writer, address.to_string())
        .send_packet(packet)
        .await
        .map_err(|e| format!("Could not ask the computer: {e}"))
}

/// Is the live control connection, made to `live`, the one to `address`:`port`?
fn same_desktop(live: Option<&(String, u16)>, address: &str, port: u16) -> bool {
    live.is_some_and(|(host, live_port)| host == address && *live_port == port)
}

/// The live control connection's writer, if that connection is to
/// `address`:`port`. Locked writer-then-peer, like the control reader.
async fn live_writer_for(
    address: &str,
    port: u16,
) -> Option<Arc<Mutex<crate::session::ControlWriter>>> {
    let guard = (*CONTROL_WRITER).lock().await;
    let peer = (*CONTROL_PEER).lock().await;
    if same_desktop(peer.as_ref(), address, port) {
        guard.as_ref().cloned()
    } else {
        None
    }
}

/// Fire-and-forget push: write `request` on the live control connection, or on
/// a dedicated one when the phone is not connected yet.
///
/// The dedicated connection is half-closed and drained rather than dropped, for
/// the same reason [`oneshot_request`] is: an abrupt close with data still in
/// flight shows up server-side as a `Broken pipe` plugin error.
pub(crate) async fn push_packet(
    address: &str,
    port: u16,
    request: &NetworkPacket,
) -> Result<(), String> {
    // The live connection only if it is to this desktop: a push for another
    // computer (a clipboard, a command) must not reach the connected one.
    if let Some(writer) = live_writer_for(address, port).await {
        let sender = TcpDeviceSender::from_arc(writer, address.to_string());
        return sender
            .send_packet(request)
            .await
            .map_err(|e| format!("Failed to send {}: {e}", request.packet_type));
    }

    let conn_mgr = control_manager(Duration::from_secs(5))?;
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

/// Show one of this phone's notifications on the desktop (N5a): `body` is
/// the `kdeconnect.notification` body as JSON (`{id, app, title, text, time,
/// isClearable}`). Rides the live control connection to that desktop, or a
/// short one of its own.
pub async fn send_notification(address: String, port: u16, body: String) -> Result<(), String> {
    let body = notification_body(&body)?;
    push_packet(
        &address,
        port,
        &NetworkPacket::new("kdeconnect.notification").with_body(body),
    )
    .await
}

/// The notification body the phone sends: a JSON object with an `id`.
fn notification_body(json: &str) -> Result<serde_json::Value, String> {
    let body: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Invalid notification: {e}"))?;
    if !body.is_object() || body.get("id").and_then(|v| v.as_str()).is_none() {
        return Err("A notification needs an id".to_string());
    }
    Ok(body)
}

/// The desktop's clipboard as its reply says (N4), as JSON: `{content}`
/// for text, or `{image: {id, size}}` for an image the phone may fetch with
/// [`crate::download::fetch_clipboard_image`].
pub async fn get_clipboard_state(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new("kdeconnect.clipboard.connect");
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.clipboard",
        Duration::from_secs(5),
    )
    .await?;
    Ok(clipboard_state(&body).to_string())
}

/// The fields of a clipboard reply the phone uses, and nothing else.
fn clipboard_state(body: &serde_json::Value) -> serde_json::Value {
    let mut state = serde_json::json!({});
    if let Some(image) = body.get("image")
        && let (Some(id), Some(size)) = (
            image.get("id").and_then(|v| v.as_str()),
            image.get("size").and_then(|v| v.as_u64()),
        )
    {
        state["image"] = serde_json::json!({ "id": id, "size": size });
    } else {
        state["content"] =
            serde_json::json!(body.get("content").and_then(|v| v.as_str()).unwrap_or(""));
    }
    state
}

/// Put the PNG at `path` on the desktop's clipboard (N4). The desktop dials
/// back for the bytes, inside TLS with the pinned desktop, and answers
/// "saved" only once its clipboard holds the image; a refusal ends the wait.
pub async fn send_clipboard_image(address: String, port: u16, path: String) -> Result<(), String> {
    use pocketdesk_core::protocol::clipboard::{CLIPBOARD_IMAGE, MAX_CLIPBOARD_IMAGE};
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| format!("Failed to open the image: {e}"))?;
    let size = file
        .metadata()
        .await
        .map_err(|e| format!("Failed to read the image: {e}"))?
        .len();
    if size == 0 || size > MAX_CLIPBOARD_IMAGE {
        return Err(format!("An image of {size} bytes is not sent"));
    }
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind port: {e}"))?;
    let transfer_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let key = uuid::Uuid::new_v4().simple().to_string();
    let request = NetworkPacket::new(CLIPBOARD_IMAGE).with_body(serde_json::json!({
        "size": size,
        "port": transfer_port,
        "key": key,
    }));
    let replies = crate::download::send_watching_replies(&address, port, &request)
        .await
        .map_err(|e| format!("Failed to offer the image: {e}"))?;
    let tls = transfer_acceptor(&address)?;
    let refused = crate::download::refusal_on(replies, key.clone());
    let accept = tokio::time::timeout(
        Duration::from_secs(30),
        accept_transfer(&listener, &key, &tls),
    );
    let mut stream = tokio::select! {
        accepted = accept => accepted.map_err(|_| "The computer did not take the image".to_string())??,
        reason = refused => return Err(reason),
    };
    send_over(&mut file, size, &mut stream, false, Pace::DEFAULT)
        .await
        .map(|_| ())
}

/// The running file transfer's bytes sent and total, for the phone's
/// progress notification (F7). Reset as each transfer starts.
pub(crate) static TRANSFER_SENT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static TRANSFER_TOTAL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
/// Tests that move the transfer counters run one at a time: the counters
/// are process-wide.
#[cfg(test)]
pub(crate) static TRANSFER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `(sent, total)` of the running (or last) file transfer.
pub fn transfer_progress() -> (u64, u64) {
    use std::sync::atomic::Ordering;
    (
        TRANSFER_SENT.load(Ordering::Relaxed),
        TRANSFER_TOTAL.load(Ordering::Relaxed),
    )
}

/// Send file to peer using KDE Share protocol.
pub async fn send_file(address: String, port: u16, file_path: String) -> Result<(), String> {
    let file = tokio::fs::File::open(&file_path)
        .await
        .map_err(|e| format!("Failed to open file: {}", e))?;
    let file_size = file
        .metadata()
        .await
        .map_err(|e| format!("Failed to read file metadata: {}", e))?
        .len();
    let filename = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown_file")
        .to_string();
    let modified = std::fs::metadata(&file_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    let source = source_id(&format!("{file_path}\n{file_size}\n{modified}"));
    send_stream(address, port, filename, file_size, file, Some(source), None).await
}

/// Where a file sits in a folder the phone sends (T6b): the send's id, the
/// folder's name, and the file's path under it (its own name last).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPart {
    pub folder_id: String,
    pub folder: String,
    pub path: Vec<String>,
}

/// A file's source id (T6a): 32 hex digits naming the file at its source,
/// the same on every attempt to send it, so the desktop continues what an
/// earlier attempt left. `what` should change whenever the file might.
pub fn source_id(what: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(what.as_bytes())[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Send `size` bytes read from `fd` as `name`, taking ownership of the file
/// descriptor (the phone hands over a content provider's file this way, so
/// it is not copied first).
#[cfg(unix)]
pub async fn send_file_fd(
    address: String,
    port: u16,
    fd: i32,
    name: String,
    size: u64,
    source: String,
    folder: Option<FolderPart>,
) -> Result<(), String> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err("no file descriptor".to_string());
    }
    // SAFETY: the caller gives up `fd` (ParcelFileDescriptor.detachFd), so
    // this File is its only owner and closes it.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let source = pocketdesk_core::protocol::partials::valid_source(&source).then_some(source);
    send_stream(
        address,
        port,
        name,
        size,
        tokio::fs::File::from_std(file),
        source,
        folder,
    )
    .await
}

#[cfg(not(unix))]
pub async fn send_file_fd(
    _address: String,
    _port: u16,
    _fd: i32,
    _name: String,
    _size: u64,
    _source: String,
    _folder: Option<FolderPart>,
) -> Result<(), String> {
    Err("file descriptors are only passed on Android".to_string())
}

/// Offer `file_size` bytes from `file` to the desktop as `filename`, and send
/// them when it connects back with the key: from where the desktop's partial
/// copy of the same `source` ends, if it kept one (T6a).
async fn send_stream<R: tokio::io::AsyncRead + Unpin>(
    address: String,
    port: u16,
    filename: String,
    file_size: u64,
    mut file: R,
    source: Option<String>,
    folder: Option<FolderPart>,
) -> Result<(), String> {
    // This transfer's counters from the start: the phone reads them while
    // waiting for the desktop too, and must not see the last file's.
    {
        use std::sync::atomic::Ordering;
        TRANSFER_TOTAL.store(file_size, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
    }
    let key = uuid::Uuid::new_v4().simple().to_string();
    // At home the computer dials a listener; by the internet road it opens a
    // stream on this phone's link, and the request names no port (2d).
    let arrival = crate::link::Arrival::new(&address, port, &key).await?;
    let by_link = arrival.port().is_none();
    let mut body = serde_json::json!({
        "filename": filename,
        "payloadTransferInfo": crate::link::transfer_info(&arrival, &key),
    });
    if let Some(source) = &source {
        body["sourceId"] = serde_json::json!(source);
    }
    if let Some(part) = &folder {
        body["folderId"] = serde_json::json!(part.folder_id);
        body["folder"] = serde_json::json!(part.folder);
        body["path"] = serde_json::json!(part.path);
    }
    let request = NetworkPacket::new("kdeconnect.share.request")
        .with_body(body)
        .with_payload_size(file_size);
    // The live control connection carries the request when it is to this
    // desktop; otherwise a connection of its own does, read for the whole
    // transfer so an offer the desktop pushes to it is not lost (F7b).
    let replies = crate::download::send_watching_replies(&address, port, &request)
        .await
        .map_err(|e| format!("Failed to send share request: {e}"))?;
    // The replies are read for the whole transfer: a refusal ends the wait
    // at once with its reason, and an offer the desktop pushes to a
    // connection of this transfer's own is not lost (F7b).
    let (refused_tx, refused_rx) = tokio::sync::oneshot::channel();
    let _reading = AbortOnDrop(tokio::spawn({
        let key = key.clone();
        async move {
            let _ = refused_tx.send(crate::download::refusal_on(replies, key).await);
        }
    }));
    // The desktop connects back for the bytes, inside TLS with the pinned
    // desktop's certificate (only the desktop this phone pinned for
    // `address` may take them, phase C), then presents the key from the
    // request; anything else that connects is dropped. (Its source address
    // is no test: a desktop with several addresses may connect back from
    // another.) By the internet road the stream comes on the link instead.
    let accept = tokio::time::timeout(Duration::from_secs(30), arrival.take(&address, &key));
    let mut client_stream = tokio::select! {
        accepted = accept => accepted
            .map_err(|_| "Timeout waiting for file receiver".to_string())??,
        Ok(reason) = refused_rx => return Err(refusal_text("file", &reason, by_link)),
    };
    let from = send_over(
        &mut file,
        file_size,
        &mut client_stream,
        true,
        Pace::DEFAULT,
    )
    .await?;
    if from > 0 {
        tracing::info!("File sent: {filename} ({file_size} bytes, continued from {from})");
    } else {
        tracing::info!("File sent: {filename} ({file_size} bytes)");
    }
    Ok(())
}

/// What the phone says of a computer's refusal of a transfer of `what`
/// ("file", "download"): a computer from before 2c answers a request by the
/// internet road, which names no port, as if it had none to use.
pub(crate) fn refusal_text(what: &str, reason: &str, by_link: bool) -> String {
    if by_link && crate::link::too_old(reason) {
        "This computer is too old for files over the internet: update Pocketdesk on it".to_string()
    } else {
        format!("The computer refused the {what}: {reason}")
    }
}

/// Send `size` bytes of `file` over a transfer the desktop has opened, with
/// the resumable exchange (T6a), counting into the transfer progress when
/// `counted`. Returns where it continued from, once the desktop says it
/// saved the file.
async fn send_over<R, S>(
    file: &mut R,
    size: u64,
    stream: &mut S,
    counted: bool,
    pace: Pace,
) -> Result<u64, String>
where
    R: tokio::io::AsyncRead + Unpin,
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use std::sync::atomic::Ordering;
    if counted {
        TRANSFER_TOTAL.store(size, Ordering::Relaxed);
        TRANSFER_SENT.store(0, Ordering::Relaxed);
    }
    transfer::send(file, size, stream, pace, |n| {
        if counted {
            TRANSFER_SENT.store(n, Ordering::Relaxed);
        }
    })
    .await
    .map_err(|e| match e {
        TransferError::Size(sent) => {
            format!("The file changed while it was sent ({sent} of {size} bytes)")
        }
        TransferError::NotSaved => "The computer could not save the file".to_string(),
        other => format!("The transfer failed: {other}"),
    })
}

/// Aborts a task when dropped, however the function holding it returns.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A transfer connection (phase C): TLS over TCP, the phone's side.
pub(crate) type TransferStream = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;

/// The phone's side of a transfer's TLS (phase C): its own certificate, and
/// a dialer required to present exactly the certificate pinned for the
/// desktop at `address`, the one its control channel proved.
pub(crate) fn transfer_acceptor(address: &str) -> Result<tokio_rustls::TlsAcceptor, String> {
    let dir = crate::CERT_DIR
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "no data directory configured".to_string())?;
    transfer_acceptor_in(&dir, address)
}

/// [`transfer_acceptor`] with this phone's certificates in `dir`.
pub(crate) fn transfer_acceptor_in(
    dir: &std::path::Path,
    address: &str,
) -> Result<tokio_rustls::TlsAcceptor, String> {
    let certs = pocketdesk_core::streaming::transport::CertManager::load_or_create(dir)
        .map_err(|e| format!("This phone's certificate: {e}"))?;
    // The pin's label is the one the control channel used for `address`:
    // its IP, or the name of a computer saved by name.
    let label = pocketdesk_core::streaming::transport::peer_label_for_address(address)
        .ok_or_else(|| format!("{address} is not an address"))?;
    let pinned = certs
        .known_peers()
        .into_iter()
        .find(|(known, _)| *known == label)
        .map(|(_, hash)| hash)
        .ok_or_else(|| format!("This phone has not verified the computer at {address} yet"))?;
    let config = certs
        .transfer_server_config(pinned)
        .map_err(|e| e.to_string())?;
    Ok(tokio_rustls::TlsAcceptor::from(config))
}

/// The desktop's connection for a transfer: TLS, in which it must prove it
/// is the pinned desktop and speak the transfer protocol, then the
/// transfer's `key`. Connections are checked side by side (up to
/// [`MAX_PENDING_TRANSFER_CONNECTIONS`]), so ones that stall cannot hold up
/// the desktop's; anything that fails is dropped, and the wait goes on.
pub(crate) async fn accept_transfer(
    listener: &tokio::net::TcpListener,
    key: &str,
    tls: &tokio_rustls::TlsAcceptor,
) -> Result<TransferStream, String> {
    let mut pending = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept(), if pending.len() < MAX_PENDING_TRANSFER_CONNECTIONS => {
                let (stream, peer) =
                    accepted.map_err(|e| format!("Failed to accept connection: {e}"))?;
                let tls = tls.clone();
                let key = key.to_string();
                pending.spawn(async move {
                    let checked = check_transfer_connection(stream, &tls, &key).await;
                    if let Err(why) = &checked {
                        tracing::warn!("File transfer: dropped a connection from {peer}: {why}");
                    }
                    checked.ok()
                });
            }
            Some(done) = pending.join_next(), if !pending.is_empty() => {
                if let Ok(Some(stream)) = done {
                    // Dropping the set ends the checks still running.
                    return Ok(stream);
                }
            }
        }
    }
}

/// How many incoming connections are checked at once while a transfer waits.
const MAX_PENDING_TRANSFER_CONNECTIONS: usize = 16;

/// TLS with the pinned desktop on the transfer protocol, then the key.
async fn check_transfer_connection(
    stream: tokio::net::TcpStream,
    tls: &tokio_rustls::TlsAcceptor,
    key: &str,
) -> Result<TransferStream, &'static str> {
    let mut stream = tokio::time::timeout(Duration::from_secs(5), tls.accept(stream))
        .await
        .map_err(|_| "no TLS handshake")?
        .map_err(|_| "not the computer")?;
    if stream.get_ref().1.alpn_protocol() != Some(pocketdesk_core::protocol::ALPN_TRANSFER) {
        return Err("not a file transfer");
    }
    if !presents_key(&mut stream, key).await {
        return Err("without the key");
    }
    Ok(stream)
}

/// Whether a connection opens with `key` and a newline, read within 5 s.
pub(crate) async fn presents_key<R: tokio::io::AsyncRead + Unpin>(
    stream: &mut R,
    key: &str,
) -> bool {
    let mut presented = vec![0u8; key.len() + 1];
    matches!(
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut presented)).await,
        Ok(Ok(_))
    ) && presented[..key.len()] == *key.as_bytes()
        && presented[key.len()] == b'\n'
}

/// A folder on the desktop (F7b): the browse reply as the desktop sent it,
/// `{path, parent, files: [{name, isDirectory, size, modified, path}]}`, or
/// with `error` when the folder cannot be listed.
pub async fn list_remote_files(
    address: String,
    port: u16,
    remote_path: String,
) -> Result<serde_json::Value, String> {
    let request = NetworkPacket::new("kdeconnect.filebrowse.request")
        .with_body(serde_json::json!({ "path": remote_path }));
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.filebrowse.response",
        Duration::from_secs(10),
    )
    .await
}

/// Request remote screen streaming.
///
/// `monitor_index` selects which display to stream (0 = primary).
/// Pass `None` to use the default monitor.
/// `codec_caps` advertises the decodable video codecs to the server
/// (R4 C1: bit 0 = HEVC; H.264 is assumed). 0 keeps a plain H.264 session.
/// `mode` says what it is for: the computer's screen, the phone as a remote
/// (T10i), or the phone's own screen shown there (T10k).
pub async fn connect_streaming(
    address: String,
    port: u16,
    monitor_index: Option<u32>,
    codec_caps: u8,
    mode: pocketdesk_core::streaming::SessionMode,
) -> Result<(), String> {
    // If the control port (1716) is passed, automatically switch to the default streaming port (4716).
    // In a real KDE Connect implementation, this would be negotiated or discovery-based.
    let streaming_port = if port == pocketdesk_core::DEFAULT_CONTROL_PORT {
        pocketdesk_core::DEFAULT_STREAMING_PORT
    } else {
        port
    };

    let addr = format!("{address}:{streaming_port}");

    // F2: Idempotent session check — the same desktop *and* monitor; a
    // switch to another monitor is a new session.
    let generation = {
        let mut handle_guard = (*STREAMING_HANDLE).lock().await;
        if let Some(handle) = &*handle_guard {
            if handle.address == address
                && handle.port == streaming_port
                && handle.monitor_index == monitor_index
                && handle.mode == mode
                // One the computer closed (a refusal) is not active: a
                // retry must dial again, not keep the dead session.
                && handle.connection.close_reason().is_none()
            {
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
        crate::CONNECT_GENERATION.begin()
    };

    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Connecting;
    tracing::info!("Connecting to streaming server at {addr}");

    // Create a CertManager with persistent TOFU peer certificate verification.
    // Uses the data directory set via `set_data_dir()`. Falls back to in-memory
    // if no data directory is configured (cert trust lost on restart).
    let cert_manager = {
        let guard = crate::CERT_DIR.lock().unwrap();
        match guard.as_ref() {
            Some(dir) => std::sync::Arc::new(
                pocketdesk_core::streaming::transport::CertManager::load_or_create(dir)
                    .map_err(|e| e.to_string())?,
            ),
            None => {
                tracing::warn!("No data dir configured — cert trust is ephemeral");
                std::sync::Arc::new(
                    pocketdesk_core::streaming::transport::CertManager::new()
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

    // Device keys: on a LAN session, whose TLS certificate proves this
    // phone, name the WAN key it dials with so the desktop can bind it.
    let iroh_id = device_wan_key().map(|key| key.public().to_string());
    let (client, packet_rx, audio_rx) = StreamingClient::connect(
        &addr,
        cert_manager,
        monitor_index,
        Some(&client_identity().device_id),
        iroh_id.as_deref(),
        Some(codec_caps),
        mode,
    )
    .await
    .map_err(|e| {
        let err_dto = PocketdeskErrorDto::from(pocketdesk_core::error::PocketdeskError::from(e));
        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Error(PocketdeskErrorDto {
            code: err_dto.code,
            message: err_dto.message.clone(),
            is_retryable: err_dto.is_retryable,
        });
        err_dto.message
    })?;

    install_streaming(
        generation,
        client,
        packet_rx,
        audio_rx,
        address,
        streaming_port,
        monitor_index,
        None,
    )
    .await
}

/// How long the internet road rests for a computer after a failed dial.
/// Several loops ask for the control link (the session, clipboard, texts,
/// the background sender); without a rest each would pay a full dial (an
/// endpoint, relays, perhaps the DHT) every few seconds while the computer
/// is off.
const INTERNET_REST: Duration = Duration::from_secs(60);

/// When each computer's internet road may be tried again.
pub(crate) struct InternetPause(std::sync::Mutex<std::collections::HashMap<String, Instant>>);

impl InternetPause {
    pub(crate) fn may_try(&self, address: &str, now: Instant) -> bool {
        let rests = self.0.lock().unwrap_or_else(|e| e.into_inner());
        rests.get(address).is_none_or(|until| now >= *until)
    }

    pub(crate) fn failed(&self, address: &str, now: Instant) {
        let mut rests = self.0.lock().unwrap_or_else(|e| e.into_inner());
        rests.retain(|_, until| *until > now);
        rests.insert(address.to_string(), now + INTERNET_REST);
    }

    pub(crate) fn succeeded(&self, address: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(address);
    }
}

pub(crate) static INTERNET_PAUSE: std::sync::LazyLock<InternetPause> =
    std::sync::LazyLock::new(|| InternetPause(std::sync::Mutex::default()));

/// Where a computer is on the internet road: the parts of the
/// `kdeconnect.pocketdesk.endpoint` body a dial needs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WanTarget {
    pub endpoint_id: String,
    pub relay_urls: Vec<String>,
    pub direct_addrs: Vec<String>,
    /// The desktop publishes on the Mainline DHT (N3).
    pub dht: bool,
}

impl WanTarget {
    pub(crate) fn parse(identity_json: &str) -> Result<Self, String> {
        let identity: serde_json::Value = serde_json::from_str(identity_json)
            .map_err(|e| format!("Invalid WAN identity: {e}"))?;
        let endpoint_id = identity["endpointId"]
            .as_str()
            .ok_or_else(|| "WAN identity missing endpointId".to_string())?
            .to_string();
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
        Ok(Self {
            endpoint_id,
            relay_urls: read_list("relayUrls"),
            direct_addrs: read_list("directAddrs"),
            dht: identity["dht"].as_bool() == Some(true),
        })
    }
}

/// The control channel over the internet road (T10a): dial the computer's
/// iroh endpoint as this phone's own iroh key, open the one stream, and run
/// the same handshake as at home. `use_relays` is false only in tests.
pub(crate) async fn dial_internet_control(
    target: &WanTarget,
    identity: &pocketdesk_core::protocol::kdeconnect::DeviceIdentity,
    address: &str,
    use_relays: bool,
) -> Result<ControlDuplex, String> {
    let dht = if target.dht {
        pocketdesk_core::streaming::dht_resolver().await.ok()
    } else {
        None
    };
    let stream = pocketdesk_core::streaming::IrohDial::dial_control(
        &target.endpoint_id,
        &target.relay_urls,
        &target.direct_addrs,
        use_relays,
        Duration::from_secs(20),
        device_wan_key(),
        dht,
    )
    .await
    .map_err(|e| format!("{e:#}"))?;
    // The streams the computer opens on this connection for transfers (2d).
    let acceptor = stream.link_acceptor();
    let stream = pocketdesk_core::protocol::connection::internet_control_handshake(
        stream, address, identity,
    )
    .await
    .map_err(|e| e.to_string())?;
    if let Some(acceptor) = acceptor {
        crate::link::spawn_acceptor(acceptor);
    }
    Ok(Box::new(stream))
}

/// Request remote screen streaming over the iroh WAN path.
///
/// `identity_json` is the cached `kdeconnect.pocketdesk.endpoint` body learned
/// from the desktop over the trusted control channel (fields: endpointId,
/// relayUrls, directAddrs, dht). The identity must come from a prior LAN
/// session; when it says the desktop publishes on the Mainline DHT (`dht`,
/// N3), the dial also looks it up there by id, in case the relay or the
/// addresses changed since.
pub async fn connect_streaming_wan(
    address: String,
    identity_json: String,
    monitor_index: Option<u32>,
    codec_caps: u8,
    mode: pocketdesk_core::streaming::SessionMode,
) -> Result<(), String> {
    let WanTarget {
        endpoint_id,
        relay_urls,
        direct_addrs,
        dht,
    } = WanTarget::parse(&identity_json)?;

    // WAN sessions are keyed by the desktop address with a 0 port sentinel
    // (there is no TCP port on the iroh path).
    let generation = {
        let mut handle_guard = (*STREAMING_HANDLE).lock().await;
        if let Some(handle) = &*handle_guard {
            if handle.address == address
                && handle.port == 0
                && handle.monitor_index == monitor_index
                && handle.mode == mode
                // One the computer closed (a refusal) is not active: a
                // retry must dial again, not keep the dead session.
                && handle.connection.close_reason().is_none()
            {
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
        crate::CONNECT_GENERATION.begin()
    };

    *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Connecting;
    // Built only now that a dial will happen: it resolves the DHT's bootstrap
    // nodes and binds a socket.
    let dht = if dht {
        match pocketdesk_core::streaming::dht_resolver().await {
            Ok(lookup) => Some(lookup),
            Err(e) => {
                // The cached address may still work.
                tracing::warn!("WAN: no DHT lookup this time: {e:#}");
                None
            }
        }
    } else {
        None
    };
    tracing::info!(
        "Dialing {address} over iroh WAN (endpoint {endpoint_id}, DHT lookup: {})",
        dht.is_some()
    );

    let dial = pocketdesk_core::streaming::IrohDial::dial(
        &endpoint_id,
        &relay_urls,
        &direct_addrs,
        true,
        Duration::from_secs(20),
        // Device keys: dial as this phone's own key, the one it named on LAN.
        device_wan_key(),
        dht,
    )
    .await
    .map_err(|e| {
        let err_dto = PocketdeskErrorDto::from(pocketdesk_core::error::PocketdeskError::from(e));
        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Error(PocketdeskErrorDto {
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
        None,
        Some(codec_caps),
        mode,
    )
    .await;

    install_streaming(
        generation,
        client,
        packet_rx,
        audio_rx,
        address,
        0,
        monitor_index,
        Some(dial),
    )
    .await
}

/// This phone's WAN (iroh) key, kept in its data directory across runs so
/// the desktop can bind its WAN sessions to it (device keys). `None` without
/// a data directory, or if the key cannot be read; the dial then uses a
/// throwaway key, which a desktop that has bound one refuses.
fn device_wan_key() -> Option<pocketdesk_core::streaming::IrohSecretKey> {
    // One caller at a time: two first calls at once (a LAN connect and a WAN
    // dial) must not each make a key.
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = crate::CERT_DIR.lock().ok()?.clone()?;
    match pocketdesk_core::streaming::load_or_create_device_key(&dir.join("iroh_device.key")) {
        Ok(key) => Some(key),
        Err(e) => {
            tracing::warn!("WAN key unavailable: {e}");
            None
        }
    }
}

/// Cleanly stop a streaming session: cancel its tasks and close the WAN dial
/// endpoint if it owns one (iroh endpoints must be closed, never dropped).
/// Why a connect that a stop or a newer connect overtook did not install.
const SUPERSEDED: &str = "the stream was stopped or replaced while it connected";

/// The reason a stream the phone stops is closed with, so the desktop ends
/// the session at once (R2).
const STOPPED_BY_PHONE: &[u8] = b"stopped by the phone";

async fn teardown_streaming_handle(handle: StreamingHandle) {
    crate::session::REMOTE.lock().unwrap().take();
    crate::cast::stop();
    VIDEO_RX.lock().await.take();
    AUDIO_RX.lock().await.take();
    // Closed on the wire, so the desktop ends the session at once. Waiting
    // for the last handle to drop is not enough: a clone that outlives this
    // one keeps the connection open on keep-alives, and the desktop went on
    // encoding a stream to nobody (R2).
    handle.connection.close(0, STOPPED_BY_PHONE);
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
#[allow(clippy::too_many_arguments)]
async fn install_streaming(
    generation: u64,
    mut client: StreamingClient,
    packet_rx: tokio::sync::mpsc::Receiver<pocketdesk_core::streaming::EncodedPacket>,
    audio_rx: tokio::sync::mpsc::Receiver<pocketdesk_core::streaming::AudioPacket>,
    address: String,
    port: u16,
    monitor_index: Option<u32>,
    wan_dial: Option<pocketdesk_core::streaming::IrohDial>,
) -> Result<(), String> {
    // A stop or a newer connect came while this one was connecting: it must
    // not take over. Checked again under the handle lock below, where it
    // counts; this early check only saves starting tasks for nothing.
    if !crate::CONNECT_GENERATION.is_current(generation) {
        if let Some(connection) = client.connection() {
            connection.close(0, STOPPED_BY_PHONE);
        }
        client.cancel_token().cancel();
        if let Some(dial) = wan_dial {
            dial.close().await;
        }
        return Err(SUPERSEDED.to_string());
    }
    let Some(connection) = client.connection().cloned() else {
        if let Some(dial) = wan_dial {
            dial.close().await;
        }
        return Err("Connection not available after connect".to_string());
    };

    // Samples the last session's decoder measured must not join this one's
    // record, and nothing is going to report them now that its server is gone.
    DECODE_SAMPLES.clear();
    RENDER_SAMPLES.clear();
    // Same for the HUD's window: a rate or RTT carried over from the computer
    // this session replaced would be quoted as if it were measured here.
    RTT_HISTORY.lock().unwrap().clear();
    RATE_SNAPSHOTS.lock().unwrap().clear();
    *STREAM_CLOSED_REASON.lock().unwrap() = None;

    let cancel = client.cancel_token();
    let client_cancel = cancel.clone();
    // Taken before the client moves into its task.
    let video_bytes = client.video_bytes();
    let remote = client.remote();
    let input_only = remote.input_only;
    let mode = if remote.cast {
        pocketdesk_core::streaming::SessionMode::Cast
    } else if input_only {
        pocketdesk_core::streaming::SessionMode::InputOnly
    } else {
        pocketdesk_core::streaming::SessionMode::Screen
    };
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
                    if let Some(reason) = rtt_connection.close_reason() {
                        tracing::info!("Streaming connection closed: {reason}");
                        *STREAM_CLOSED_REASON.lock().unwrap() = Some(reason);
                        break;
                    }
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
    // Under the lock a stop takes after retiring connects: either the stop
    // retired this one first and it is refused here, or it is installed and
    // the stop tears it down.
    if !crate::CONNECT_GENERATION.is_current(generation) {
        drop(handle);
        connection.close(0, STOPPED_BY_PHONE);
        client_cancel.cancel();
        let _ = task.await;
        let _ = rtt_task.await;
        if let Some(dial) = wan_dial {
            dial.close().await;
        }
        return Err(SUPERSEDED.to_string());
    }
    *VIDEO_RX.lock().await = Some(packet_rx);
    *AUDIO_RX.lock().await = Some(audio_rx);
    *crate::VIDEO_BYTES.lock().unwrap_or_else(|e| e.into_inner()) = Some(video_bytes);
    *crate::session::REMOTE.lock().unwrap() = input_only.then_some(remote);
    if mode == pocketdesk_core::streaming::SessionMode::Cast {
        crate::cast::start(connection.clone(), client_cancel.clone());
    }
    *handle = Some(StreamingHandle {
        address: address.clone(),
        port,
        monitor_index,
        mode,
        cancel: client_cancel,
        task,
        rtt_task,
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
                pocketdesk_core::streaming::transport::CertManager::load_or_create(dir)
                    .map_err(|e| e.to_string())?,
            ),
            None => {
                tracing::warn!("No data dir configured — cert trust is ephemeral");
                std::sync::Arc::new(
                    pocketdesk_core::streaming::transport::CertManager::new()
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
            min_version: pocketdesk_core::protocol::v2::V2_MIN_VERSION,
            max_version: pocketdesk_core::protocol::v2::V2_MAX_VERSION,
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
                if len > pocketdesk_core::protocol::v2::MAX_CONTROL_PAYLOAD_SIZE {
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
                PocketdeskErrorDto::from(pocketdesk_core::error::PocketdeskError::ProtocolError {
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
    connect_streaming(
        address,
        port,
        monitor_index,
        codec_caps,
        pocketdesk_core::streaming::SessionMode::Screen,
    )
    .await
}

/// The phone-as-a-remote session's state (T10i), as JSON: `{"ready"}` once
/// the computer admitted it, `{"tooOld"}` when it streamed to it anyway,
/// `{"none"}` with no input-only session.
pub fn remote_state() -> String {
    let state = state_name(crate::session::REMOTE.lock().unwrap().as_ref(), false);
    format!("{{\"state\":\"{state}\"}}")
}

/// [`remote_state`] of a cast session only (T10k): `none` while the phone's
/// session is anything else, so a cast never takes a remote or a screen
/// session the person opened since for its own.
pub fn cast_state() -> String {
    let state = state_name(crate::session::REMOTE.lock().unwrap().as_ref(), true);
    format!("{{\"state\":\"{state}\"}}")
}

/// The state of an input-only session's `flags`; with `cast`, only of a cast.
fn state_name(flags: Option<&pocketdesk_core::streaming::RemoteFlags>, cast: bool) -> &'static str {
    use std::sync::atomic::Ordering;
    match flags {
        None => "none",
        Some(r) if cast && !r.cast => "none",
        Some(r) if r.too_old.load(Ordering::Acquire) => "tooOld",
        Some(r) if r.ready.load(Ordering::Acquire) => "ready",
        Some(_) => "waiting",
    }
}

/// End the phone's session if it is a cast (T10k), and nothing else: a
/// cast that ended because the person opened another session must not
/// stop that one.
pub async fn stop_cast() {
    let handle = {
        let mut guard = (*STREAMING_HANDLE).lock().await;
        if guard
            .as_ref()
            .is_some_and(|h| h.mode == pocketdesk_core::streaming::SessionMode::Cast)
        {
            guard.take()
        } else {
            None
        }
    };
    if let Some(handle) = handle {
        teardown_streaming_handle(handle).await;
        STREAMING_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
        *crate::SESSION_STATUS.lock().unwrap() = SessionStatus::Disconnected;
        tracing::info!("Cast session stopped");
    }
}

#[cfg(test)]
mod cast_state_tests {
    use super::state_name;
    use pocketdesk_core::streaming::RemoteFlags;

    #[test]
    fn a_cast_sees_only_a_cast_session() {
        let remote = RemoteFlags {
            input_only: true,
            ..Default::default()
        };
        remote
            .ready
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(state_name(Some(&remote), false), "ready");
        assert_eq!(
            state_name(Some(&remote), true),
            "none",
            "a remote is not the cast"
        );
        let cast = RemoteFlags {
            input_only: true,
            cast: true,
            ..Default::default()
        };
        assert_eq!(state_name(Some(&cast), true), "waiting");
        cast.ready.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(state_name(Some(&cast), true), "ready");
        assert_eq!(state_name(None, true), "none");
    }
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

    // Retire connects in flight first, so one that lands after this stop
    // does not install itself (see `install_streaming`).
    crate::CONNECT_GENERATION.retire();
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
    *STREAM_CLOSED_REASON.lock().unwrap() = None;
    // No session, no video counted: the next one installs its own.
    *crate::VIDEO_BYTES.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// Why the live session's connection closed, or `None` while it is open (or
/// no session ever ran).
pub fn stream_closed_reason() -> Option<String> {
    STREAM_CLOSED_REASON.lock().unwrap().clone()
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
        e2e_latency_ms: pocketdesk_core::streaming::client::e2e_estimate_ms(),
        // Frames whose sequence number never arrived, counted by the client's
        // receive loop — see `missed_video_frames` in core. This replaced a
        // literal 0 that asserted a healthy link the phone had never
        // measured (roadmap 2051/2052).
        frame_drops: pocketdesk_core::streaming::client::missed_video_frames(),
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
        Some(cm) => cm.remove_peer(&label).unwrap_or_else(|e| {
            tracing::warn!("Could not forget the certificate of {label}: {e}");
            false
        }),
        None => false,
    }
}

/// Ask the computer for a screen of this phone's shape (T10j), or to take
/// it away: `action` is `add` (with `width`×`height`) or `remove`. The
/// computer's answer: `{name, index, width, height}`, `{removed}` or `{error}`.
pub async fn virtual_screen(
    address: String,
    port: u16,
    action: String,
    width: u32,
    height: u32,
) -> Result<serde_json::Value, String> {
    let request = NetworkPacket::new("kdeconnect.pocketdesk.virtual_screen")
        .with_body(serde_json::json!({ "action": action, "width": width, "height": height }));
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.virtual_screen",
        Duration::from_secs(10),
    )
    .await
}

/// Get detailed list of monitors available on the remote server.
///
/// F2: Multi-monitor support — returns an error if the desktop does not answer.
pub async fn get_monitors(address: String, port: u16) -> Result<Vec<MonitorInfoDto>, String> {
    let request =
        NetworkPacket::new("kdeconnect.pocketdesk.monitors").with_body(serde_json::json!({}));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.monitors",
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
    let request = NetworkPacket::new("kdeconnect.pocketdesk.privacy")
        .with_body(serde_json::json!({ "action": action, "lock": lock }));
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.privacy",
        Duration::from_secs(5),
    )
    .await
}

/// Desktop audio control (Tier-3 #16): send a `kdeconnect.pocketdesk.audio`
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
    let request = NetworkPacket::new("kdeconnect.pocketdesk.audio").with_body(body);
    query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.audio",
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
        NetworkPacket::new("kdeconnect.pocketdesk.windows").with_body(serde_json::json!({}));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.windows",
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
    let packet = NetworkPacket::new("kdeconnect.pocketdesk.power")
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
    let request = NetworkPacket::new("kdeconnect.pocketdesk.exec")
        .with_body(serde_json::json!({ "command": command }));
    let body = query_peer(
        &address,
        port,
        &request,
        "kdeconnect.pocketdesk.exec",
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

/// Receive queued audio packets from the streaming client: each one raw
/// Opus (20 ms @ 48 kHz stereo) with the sequence the desktop gave it, so
/// the player can drop one that arrives late. Up to
/// `MAX_AUDIO_PACKETS_PER_RECEIVE`; waits on `AUDIO_RX` only.
pub async fn receive_audio(timeout_ms: u64) -> Vec<(u64, Vec<u8>)> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut packets = Vec::with_capacity(MAX_AUDIO_PACKETS_PER_RECEIVE);
    let mut guard = AUDIO_RX.lock().await;
    let Some(rx) = guard.as_mut() else {
        return packets;
    };
    if let Ok(Some(packet)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        packets.push((packet.sequence, packet.data));
        while packets.len() < MAX_AUDIO_PACKETS_PER_RECEIVE {
            match rx.try_recv() {
                Ok(packet) => packets.push((packet.sequence, packet.data)),
                Err(_) => break,
            }
        }
    }
    packets
}

/// `receive_audio`'s packets as the JNI payload, big-endian: `u32 count`,
/// then per packet `u64 sequence`, `u32 len`, `len` bytes.
pub fn audio_payload(packets: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let total: usize = packets.iter().map(|(_, data)| 12 + data.len()).sum();
    let mut buf = Vec::with_capacity(4 + total);
    buf.extend_from_slice(&(packets.len() as u32).to_be_bytes());
    for (sequence, data) in packets {
        buf.extend_from_slice(&sequence.to_be_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(data);
    }
    buf
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
        let mut guard = VIDEO_RX.lock().await;
        let Some(rx) = guard.as_mut() else {
            return frames;
        };
        match tokio::time::timeout_at(deadline, rx.recv()).await {
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
                    match rx.try_recv() {
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
async fn stream_connection() -> Result<pocketdesk_core::streaming::SharedConnection, String> {
    (*STREAMING_HANDLE)
        .lock()
        .await
        .as_ref()
        .map(|h| h.connection.clone())
        .ok_or_else(|| "Mic requires an active streaming session".to_string())
}

async fn send_mic_frame(
    conn: &pocketdesk_core::streaming::SharedConnection,
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
        122 => 102, // MOVE_HOME -> KEY_HOME (a keyboard's Home key; KEYCODE_HOME is the phone's)
        123 => 107, // END
        124 => 110, // INSERT
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
        73 => 43, // BACKSLASH
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
mod transfer_pin_tests {
    use pocketdesk_core::streaming::transport::{CertManager, peer_label_for_address};

    fn dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ll-transfer-pin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_computer_saved_by_name_can_take_files_and_be_forgotten() {
        let dir = dir("name");
        let label = peer_label_for_address("mypc.local").unwrap();
        CertManager::load_or_create(&dir)
            .unwrap()
            .pin_peer(&label, [4u8; 32])
            .unwrap();
        assert!(super::transfer_acceptor_in(&dir, "mypc.local").is_ok());
        assert!(super::forget_peer_certificate_in(&dir, "mypc.local").unwrap());
        assert!(
            super::transfer_acceptor_in(&dir, "mypc.local").is_err(),
            "forgotten: no computer to send to"
        );
    }

    #[test]
    fn files_go_to_no_computer_the_phone_has_not_verified() {
        let dir = dir("unpinned");
        assert!(super::transfer_acceptor_in(&dir, "192.0.2.9").is_err());
    }
}

#[cfg(test)]
mod live_connection_tests {
    #[test]
    fn only_the_connection_to_that_desktop_is_reused() {
        let live = ("192.0.2.1".to_string(), 1716);
        assert!(super::same_desktop(Some(&live), "192.0.2.1", 1716));
        assert!(
            !super::same_desktop(Some(&live), "192.0.2.2", 1716),
            "another computer"
        );
        assert!(
            !super::same_desktop(Some(&live), "192.0.2.1", 1717),
            "another port"
        );
        assert!(
            !super::same_desktop(None, "192.0.2.1", 1716),
            "nothing connected"
        );
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
            pocketdesk_core::input::keys::phone_emittable_codes()
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
    use pocketdesk_core::streaming::transport::{CertManager, peer_label_for_ip};

    #[test]
    fn a_pairing_code_fingerprint_is_pinned_under_the_verifiers_label() {
        let dir = std::env::temp_dir().join(format!("bridge-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let hex = "ab".repeat(32);
        pin_peer_certificate_in(&dir, "192.168.1.20", &hex).unwrap();
        let label = peer_label_for_ip("192.168.1.20".parse().unwrap());
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
        assert!(
            pin_peer_certificate_in(&dir, "10.0.0.7", &"ab".repeat(32)).unwrap(),
            "new pin"
        );
        assert!(
            !pin_peer_certificate_in(&dir, "10.0.0.7", &"ab".repeat(32)).unwrap(),
            "the same pin again adds nothing"
        );
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
        // 64 bytes, but a two-byte character straddles a hex-pair boundary:
        // slicing it would panic, and a panic in a JNI export aborts the app.
        let straddling = format!("{}é{}", "a".repeat(61), "b");
        assert_eq!(straddling.len(), 64);
        assert!(pin_peer_certificate_in(&dir, "10.0.0.1", &straddling).is_err());
    }
}

#[cfg(test)]
mod receive_tests {
    use super::*;

    /// Tests that swap the session statics run one at a time.
    pub(super) static SESSION_STATICS: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

    fn frame(sequence: u64) -> pocketdesk_core::streaming::EncodedPacket {
        pocketdesk_core::streaming::EncodedPacket {
            data: vec![0, 0, 0, 1],
            is_keyframe: true,
            timestamp: std::time::Instant::now(),
            sequence,
        }
    }

    #[tokio::test]
    async fn a_waiting_frame_receive_does_not_hold_the_session_lock() {
        let _serial = SESSION_STATICS.lock().await;
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        *crate::VIDEO_RX.lock().await = Some(rx);
        let waiting = tokio::spawn(receive_frames(500));
        tokio::time::sleep(Duration::from_millis(30)).await;

        // What every input send does first: take the session handle.
        let locked =
            tokio::time::timeout(Duration::from_millis(100), (*STREAMING_HANDLE).lock()).await;
        assert!(locked.is_ok(), "an input send waited on the frame poll");
        drop(locked);

        tx.send(frame(7)).await.unwrap();
        let frames = waiting.await.unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].sequence, 7);
        *crate::VIDEO_RX.lock().await = None;
    }

    /// The bytes AudioPayloadTest.kt parses; both tests pin this string.
    const AUDIO_FIXTURE_HEX: &str =
        "00000002000000000000000700000003010203000000000000000800000000";

    #[test]
    fn the_audio_payload_carries_each_sequence() {
        let bytes = audio_payload(&[(7, vec![1, 2, 3]), (8, vec![])]);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, AUDIO_FIXTURE_HEX);
    }

    #[tokio::test]
    async fn a_waiting_audio_receive_blocks_neither_video_nor_input() {
        let _serial = SESSION_STATICS.lock().await;
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(4);
        let (video_tx, video_rx) = tokio::sync::mpsc::channel(4);
        *crate::AUDIO_RX.lock().await = Some(audio_rx);
        *crate::VIDEO_RX.lock().await = Some(video_rx);
        let waiting = tokio::spawn(receive_audio(500));
        tokio::time::sleep(Duration::from_millis(30)).await;

        video_tx.send(frame(1)).await.unwrap();
        let frames = tokio::time::timeout(Duration::from_millis(200), receive_frames(50))
            .await
            .expect("video waited on the audio poll");
        assert_eq!(frames.len(), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), (*STREAMING_HANDLE).lock())
                .await
                .is_ok(),
            "input waited on the audio poll"
        );

        audio_tx
            .send(pocketdesk_core::streaming::AudioPacket {
                data: vec![9],
                sequence: 41,
                timestamp: std::time::Instant::now(),
                is_config: false,
            })
            .await
            .unwrap();
        assert_eq!(waiting.await.unwrap(), vec![(41, vec![9])]);
        *crate::AUDIO_RX.lock().await = None;
        *crate::VIDEO_RX.lock().await = None;
    }
}

#[cfg(test)]
mod wake_relay_tests {
    use super::relay_outcome;

    #[test]
    fn the_relay_s_answer_decides_the_outcome() {
        assert_eq!(relay_outcome(Ok(serde_json::json!({"ok": true}))), Ok(()));
        assert_eq!(
            relay_outcome(Ok(
                serde_json::json!({"ok": false, "error": "not on this computer's network"})
            )),
            Err("The relay computer refused: not on this computer's network".to_string())
        );
        assert!(relay_outcome(Ok(serde_json::json!({}))).is_err());
        let silent = relay_outcome(Err("Timeout waiting".to_string())).unwrap_err();
        assert!(silent.contains("Power"), "{silent}");
    }
}

#[cfg(test)]
mod send_file_tests {
    use super::{presents_key, send_over, source_id, transfer_progress};
    use pocketdesk_core::protocol::transfer::{self, Have, Memory, Pace};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A desktop that receives with the resumable transfer into `partial`,
    /// continuing from `have`, and says saved when `saved`.
    async fn desktop(
        mut stream: tokio::io::DuplexStream,
        mut partial: Memory,
        have: Option<Have>,
        saved: bool,
    ) -> Vec<u8> {
        let received = transfer::receive(
            &mut stream,
            &mut partial,
            have,
            None,
            1 << 30,
            Pace::DEFAULT,
            |_, _| {},
        )
        .await;
        transfer::finish(&mut stream, saved && received.is_ok(), Pace::DEFAULT).await;
        partial.into_bytes()
    }

    /// A receiver that stops reading must not hold the phone's send (and its
    /// foreground service) open for ever.
    #[tokio::test]
    async fn a_send_to_a_receiver_that_stops_reading_fails() {
        let (mut desktop, mut phone) = tokio::io::duplex(1024);
        let quick = Pace {
            idle: Duration::from_millis(100),
            min_bytes_per_sec: 1 << 20,
        };
        let mut file: &[u8] = &[7u8; 64 * 1024];
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            send_over(&mut file, 64 * 1024, &mut phone, false, quick),
        )
        .await
        .expect("the send gives up by itself");
        assert!(outcome.is_err(), "{outcome:?}");
        let mut size = [0u8; 8];
        desktop.read_exact(&mut size).await.unwrap();
    }

    #[tokio::test]
    async fn a_file_is_sent_only_once_the_receiver_says_it_saved_it() {
        for saved in [true, false] {
            let (theirs, mut ours) = tokio::io::duplex(1 << 20);
            let receiver = tokio::spawn(desktop(theirs, Memory::holding(&[]), None, saved));
            let sent = send_over(&mut &b"hello"[..], 5, &mut ours, false, Pace::DEFAULT).await;
            drop(ours);
            assert_eq!(sent.is_ok(), saved, "{sent:?}");
            receiver.await.unwrap();
        }
    }

    #[tokio::test]
    async fn progress_counts_what_the_desktop_already_had() {
        let _counters = super::TRANSFER_TEST_LOCK.lock().await;
        let data: Vec<u8> = (0..300_000u32).map(|i| i as u8).collect();
        let (theirs, mut ours) = tokio::io::duplex(1 << 20);
        let have = Have {
            size: 300_000,
            committed: 100_000,
        };
        let receiver = tokio::spawn(desktop(
            theirs,
            Memory::holding(&data[..100_000]),
            Some(have),
            true,
        ));
        let from = send_over(&mut &data[..], 300_000, &mut ours, true, Pace::DEFAULT)
            .await
            .unwrap();
        drop(ours);
        assert_eq!(from, 100_000);
        assert_eq!(transfer_progress(), (300_000, 300_000));
        assert_eq!(receiver.await.unwrap(), data);
    }

    #[test]
    fn a_source_id_is_32_hex_digits_and_stable() {
        let a = source_id(
            "content://media/1
300
5",
        );
        assert_eq!(a.len(), 32);
        assert!(pocketdesk_core::protocol::partials::valid_source(&a));
        assert_eq!(
            a,
            source_id(
                "content://media/1
300
5"
            )
        );
        assert_ne!(
            a,
            source_id(
                "content://media/1
301
5"
            )
        );
    }

    #[tokio::test]
    async fn only_a_connection_with_the_key_gets_the_file() {
        let key = "0123456789abcdef0123456789abcdef";
        for (sent, ok) in [
            (format!("{key}\n"), true),
            (format!("{key}x"), false),
            ("ffffffffffffffffffffffffffffffff\n".to_string(), false),
            ("short".to_string(), false),
        ] {
            let (mut desktop, mut phone) = tokio::io::duplex(64);
            desktop.write_all(sent.as_bytes()).await.unwrap();
            drop(desktop);
            assert_eq!(presents_key(&mut phone, key).await, ok, "{sent:?}");
        }
    }
}

#[cfg(test)]
mod clipboard_state_tests {
    use super::clipboard_state;
    use serde_json::json;

    #[test]
    fn a_reply_is_text_or_an_image_the_phone_can_fetch() {
        assert_eq!(
            clipboard_state(&json!({"content": "hi", "timestamp": 1})),
            json!({"content": "hi"})
        );
        assert_eq!(
            clipboard_state(&json!({"image": {"id": "ab", "size": 3}, "timestamp": 1})),
            json!({"image": {"id": "ab", "size": 3}})
        );
        assert_eq!(
            clipboard_state(&json!({"image": {"id": "ab"}})),
            json!({"content": ""}),
            "an image without its size is not fetched"
        );
        assert_eq!(clipboard_state(&json!({})), json!({"content": ""}));
    }
}

#[cfg(test)]
mod notification_body_tests {
    use super::notification_body;

    #[test]
    fn a_notification_is_an_object_with_an_id() {
        assert!(notification_body(r#"{"id":"k","app":"Chat","title":"Ana"}"#).is_ok());
        assert!(notification_body(r#"{"app":"Chat"}"#).is_err(), "no id");
        assert!(notification_body("[1]").is_err());
        assert!(notification_body("not json").is_err());
    }
}

#[cfg(test)]
mod sms_tests {
    #[test]
    fn a_reply_request_is_queued_once_and_drained() {
        let _ = super::take_sms_requests();
        let request = serde_json::json!({"id": "r1", "address": "+1", "body": "hi"});
        crate::queue_sms_request(&request, "192.0.2.1", 1716);
        crate::queue_sms_request(&request, "192.0.2.1", 1716);
        crate::queue_sms_request(
            &serde_json::json!({"id": "r2", "address": "+1"}),
            "192.0.2.1",
            1716,
        );
        let drained: Vec<serde_json::Value> =
            serde_json::from_str(&super::take_sms_requests()).unwrap();
        assert_eq!(
            drained,
            vec![serde_json::json!({
                "id": "r1", "address": "+1", "body": "hi",
                "fromHost": "192.0.2.1", "fromPort": 1716,
            })],
            "one per id, only whole requests, with the computer they came from"
        );
        assert_eq!(super::take_sms_requests(), "[]");
    }

    #[test]
    fn only_the_phones_own_packets_go_out_this_way() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let refused = runtime.block_on(super::send_phone_packet(
            "127.0.0.1".into(),
            9,
            "kdeconnect.pocketdesk.exec".into(),
            "{}".into(),
        ));
        assert!(refused.unwrap_err().contains("not sent this way"));
        let bad = runtime.block_on(super::send_phone_packet(
            "127.0.0.1".into(),
            9,
            "kdeconnect.sms.messages".into(),
            "[1]".into(),
        ));
        assert!(bad.unwrap_err().contains("not an object"));
        // The phone's battery (T8b) goes this way: past the list, to the
        // body's check.
        let battery = runtime.block_on(super::send_phone_packet(
            "127.0.0.1".into(),
            9,
            "kdeconnect.battery".into(),
            "[1]".into(),
        ));
        assert!(battery.unwrap_err().contains("not an object"));
        let media = runtime.block_on(super::send_phone_packet(
            "127.0.0.1".into(),
            9,
            "kdeconnect.pocketdesk.phone_media".into(),
            "[1]".into(),
        ));
        assert!(media.unwrap_err().contains("not an object"));
    }
}

#[cfg(test)]
mod wake_tests {
    #[test]
    fn a_wake_goes_to_each_target_the_phone_names() {
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let targets = serde_json::json!([receiver.local_addr().unwrap().to_string()]);

        let reached = super::wake("AA:BB:CC:DD:EE:FF".into(), targets.to_string()).unwrap();
        assert_eq!(reached, 1);
        let mut buf = [0u8; 200];
        let (n, _) = receiver.recv_from(&mut buf).unwrap();
        assert_eq!(n, 102);
    }

    #[test]
    fn wake_info_is_handed_out_only_for_the_computer_it_came_from() {
        let slot = ("192.0.2.1".to_string(), 1716, "{\"mac\":\"x\"}".to_string());
        assert_eq!(
            super::wake_info_for(Some(&slot), "192.0.2.1", 1716).as_deref(),
            Some("{\"mac\":\"x\"}")
        );
        assert_eq!(super::wake_info_for(Some(&slot), "192.0.2.2", 1716), None);
        assert_eq!(super::wake_info_for(Some(&slot), "192.0.2.1", 1717), None);
        assert_eq!(super::wake_info_for(None, "192.0.2.1", 1716), None);
    }

    #[test]
    fn targets_that_name_nothing_are_an_error() {
        assert!(super::wake("AA:BB:CC:DD:EE:FF".into(), "[]".into()).is_err());
        assert!(super::wake("AA:BB:CC:DD:EE:FF".into(), "[\"no port\"]".into()).is_err());
        assert!(super::wake("AA:BB:CC:DD:EE:FF".into(), "not json".into()).is_err());
    }
}

#[cfg(test)]
mod teardown_tests {
    use pocketdesk_core::streaming::{
        Connection, ConnectionError, ConnectionStats, InStream, OutStream, SharedConnection,
        TransportFamily,
    };
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    /// The close a connection was given: its code and reason.
    type Closed = Arc<Mutex<Option<(u32, Vec<u8>)>>>;

    /// Records the close it is given; nothing else is used.
    struct Recording(Closed);

    #[async_trait::async_trait]
    impl Connection for Recording {
        async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
            unimplemented!()
        }
        async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
            unimplemented!()
        }
        fn remote_address(&self) -> SocketAddr {
            "192.0.2.10:4716".parse().unwrap()
        }
        fn stats(&self) -> ConnectionStats {
            ConnectionStats::default()
        }
        fn transport_family(&self) -> TransportFamily {
            TransportFamily::Quinn
        }
        fn close(&self, code: u32, reason: &[u8]) {
            *self.0.lock().unwrap() = Some((code, reason.to_vec()));
        }
    }

    /// R2: a session the phone stops is closed on the wire, not left to the
    /// last handle being dropped. A clone that outlived the handle kept the
    /// desktop encoding a stream to nobody, with keep-alives holding it open.
    #[tokio::test]
    async fn a_stopped_session_closes_its_connection() {
        // Teardown clears the shared frame receivers.
        let _serial = super::receive_tests::SESSION_STATICS.lock().await;
        let closed: Closed = Arc::new(Mutex::new(None));
        let connection: SharedConnection = Arc::new(Recording(closed.clone()));
        let lingering = connection.clone();
        let handle = crate::session::StreamingHandle {
            address: "192.0.2.10".into(),
            port: 4716,
            monitor_index: None,
            mode: pocketdesk_core::streaming::SessionMode::Screen,
            cancel: tokio_util::sync::CancellationToken::new(),
            task: tokio::spawn(async {}),
            rtt_task: tokio::spawn(async {}),
            connection,
            wan_dial: None,
        };
        super::teardown_streaming_handle(handle).await;
        assert!(
            closed.lock().unwrap().is_some(),
            "closed although a clone lives on"
        );
        drop(lingering);
    }
}

#[cfg(test)]
mod internet_road_tests {
    use super::*;
    use pocketdesk_core::protocol::{ALPN_CONTROL, HANDSHAKE_HELLO, HANDSHAKE_OK};
    use tokio::io::BufReader;

    #[tokio::test]
    async fn nothing_asks_to_pair_over_an_internet_link() {
        let _globals = super::TRANSFER_TEST_LOCK.lock().await;
        *crate::CONTROL_PEER.lock().await = Some(("192.0.2.7".to_string(), 1716));
        crate::CONTROL_OVER_INTERNET.store(true, Ordering::SeqCst);
        let refused = pair_with_pin("192.0.2.7".into(), 1716, "123456".into()).await;
        let other = pairing_road_check("198.51.100.3").await;
        crate::CONTROL_OVER_INTERNET.store(false, Ordering::SeqCst);
        let at_home = pairing_road_check("192.0.2.7").await;
        *crate::CONTROL_PEER.lock().await = None;
        assert!(matches!(refused, Err(e) if e.contains("same network")));
        assert!(other.is_ok(), "only the computer that link goes to");
        assert!(at_home.is_ok());
    }

    #[test]
    fn a_failed_internet_dial_rests_that_computer_for_a_minute() {
        let pause = InternetPause(std::sync::Mutex::default());
        let now = Instant::now();
        assert!(pause.may_try("192.0.2.7", now));
        pause.failed("192.0.2.7", now);
        assert!(!pause.may_try("192.0.2.7", now + Duration::from_secs(30)));
        assert!(pause.may_try("198.51.100.3", now), "only that computer");
        assert!(pause.may_try("192.0.2.7", now + INTERNET_REST));
        pause.failed("192.0.2.7", now);
        pause.succeeded("192.0.2.7");
        assert!(
            pause.may_try("192.0.2.7", now),
            "a link that came up clears it"
        );
    }

    #[test]
    fn a_wan_identity_names_the_dial() {
        let target = WanTarget::parse(
            r#"{"endpointId":"abc","relayUrls":["https://relay.example.org/"],"directAddrs":["192.0.2.7:4501"],"dht":true}"#,
        )
        .unwrap();
        assert_eq!(
            target,
            WanTarget {
                endpoint_id: "abc".into(),
                relay_urls: vec!["https://relay.example.org/".into()],
                direct_addrs: vec!["192.0.2.7:4501".into()],
                dht: true,
            }
        );
        let bare = WanTarget::parse(r#"{"endpointId":"abc"}"#).unwrap();
        assert!(bare.relay_urls.is_empty() && bare.direct_addrs.is_empty() && !bare.dht);
        assert!(WanTarget::parse(r#"{"relayUrls":[]}"#).is_err());
        assert!(WanTarget::parse("not json").is_err());
    }

    #[tokio::test]
    async fn the_internet_road_runs_the_control_handshake() {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let desktop = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![ALPN_CONTROL.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let direct = desktop
            .bound_sockets()
            .into_iter()
            .map(|a| a.to_string())
            .collect();
        let serve = {
            let desktop = desktop.clone();
            tokio::spawn(async move {
                let conn = desktop
                    .accept()
                    .await
                    .unwrap()
                    .accept()
                    .unwrap()
                    .await
                    .unwrap();
                let (mut send, recv) = conn.accept_bi().await.unwrap();
                let mut recv = BufReader::new(recv);
                let mut hello = String::new();
                recv.read_line(&mut hello).await.unwrap();
                assert_eq!(hello.trim(), HANDSHAKE_HELLO);
                send.write_all(format!("{HANDSHAKE_OK}\n").as_bytes())
                    .await
                    .unwrap();
                let mut phone = String::new();
                recv.read_line(&mut phone).await.unwrap();
                // Admitted: the desktop names itself.
                send.write_all(b"{\"type\":\"kdeconnect.identity\"}\n")
                    .await
                    .unwrap();
                // Held open while the phone reads the answer.
                tokio::time::sleep(Duration::from_millis(500)).await;
                phone
            })
        };
        let target = WanTarget {
            endpoint_id: desktop.id().to_string(),
            relay_urls: vec![],
            direct_addrs: direct,
            dht: false,
        };
        let identity =
            pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new("road-phone", "Road Phone");
        let _stream = dial_internet_control(&target, &identity, "desktop", false)
            .await
            .expect("the internet road");
        let phone = serve.await.unwrap();
        assert!(
            phone.contains("road-phone"),
            "the phone's identity: {phone}"
        );
        desktop.close().await;
    }
}
