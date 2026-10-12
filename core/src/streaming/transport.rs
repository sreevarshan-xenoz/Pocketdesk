//! QUIC-based streaming transport
//!
//! Provides low-latency, unreliable (lossy) QUIC streams for video delivery.
//! Uses quinn with datagram mode for minimal latency over reliability.

use anyhow::{Context, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::EncodedPacket;

/// Our own transport config to avoid name collision with quinn::TransportConfig
#[derive(Debug, Clone)]
pub struct StreamTransportConfig {
    /// Address to listen on (server) or connect to (client)
    pub address: SocketAddr,
    /// ALPN protocol identifier for streaming
    pub alpn: Vec<u8>,
    /// Enable datagram mode (unordered, lower latency)
    pub use_datagrams: bool,
}

impl Default for StreamTransportConfig {
    fn default() -> Self {
        Self {
            address: "0.0.0.0:4716".parse().unwrap(),
            alpn: crate::protocol::ALPN_V1_STREAM.to_vec(),
            use_datagrams: true,
        }
    }
}

// ---------------------------------------------------------------------------
// QUIC transport parameters
// ---------------------------------------------------------------------------

/// Build the QUIC `TransportConfig` used by every Linux Link endpoint.
///
/// Tuned for real-time A/V: large datagram buffers and high stream
/// concurrency (video opens one unidirectional stream per frame). The
/// congestion controller is selectable for A/B testing via the
/// `LINUX_LINK_CC` environment variable: `bbr`, `cubic`, or `new_reno`
/// (the default, matching quinn's own default).
///
/// Roaming hardening (Tier-3 #13): a bounded idle timeout makes dead peers
/// detectable instead of lingering forever, and a keepalive well below half
/// of it holds NAT bindings through video stalls and brief radio sleeps.
/// Address migration (Wi-Fi → LTE mid-session, keeping the live connection
/// via QUIC path validation) is accepted by the server automatically —
/// quinn's `ServerConfig::migration` defaults to `true`.
pub fn configured_transport() -> quinn::TransportConfig {
    const KEEPALIVE_SECS: u64 = 15;
    const IDLE_TIMEOUT_SECS: u64 = 45;
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_send_buffer_size(16 * 1024 * 1024);
    transport.datagram_receive_buffer_size(Some(16 * 1024 * 1024));
    transport.max_concurrent_uni_streams(1024u32.into());
    transport.max_concurrent_bidi_streams(128u32.into());
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(KEEPALIVE_SECS)));
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(IDLE_TIMEOUT_SECS)
            .try_into()
            .expect("idle timeout fits TransportDuration"),
    ));
    apply_congestion_controller(&mut transport);
    transport
}

fn apply_congestion_controller(transport: &mut quinn::TransportConfig) {
    let requested = std::env::var("LINUX_LINK_CC")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let reno: Arc<dyn quinn::congestion::ControllerFactory + Send + Sync + 'static> =
        Arc::new(quinn::congestion::NewRenoConfig::default());
    let (label, factory): (
        &str,
        Arc<dyn quinn::congestion::ControllerFactory + Send + Sync + 'static>,
    ) = match requested.as_str() {
        "bbr" => ("BBR", Arc::new(quinn::congestion::BbrConfig::default())),
        "cubic" => ("CUBIC", Arc::new(quinn::congestion::CubicConfig::default())),
        "" | "new_reno" | "reno" => ("NewReno", reno),
        other => {
            warn!(
                "LINUX_LINK_CC='{other}' is not one of bbr|cubic|new_reno — falling back to NewReno"
            );
            ("NewReno", reno)
        }
    };
    transport.congestion_controller_factory(factory);
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| info!("QUIC congestion controller: {label}"));
}

// ---------------------------------------------------------------------------
// Certificate management with TOFU (Trust On First Use) verification
// ---------------------------------------------------------------------------

/// Manages TLS certificates for E2E encrypted streaming.
///
/// Provides:
/// - A persistent identity certificate for this device (server identity)
/// - TOFU peer certificate verification so connections are trusted
///   automatically on first use and verified on reconnection
/// - Optional on-disk persistence of known peer certificate hashes
#[derive(Debug)]
pub struct CertManager {
    /// Our identity certificate in DER form
    cert_der: Vec<u8>,
    /// Our private key in PKCS#8 DER form
    key_der: Vec<u8>,
    /// Known peer certificate hashes: label → SHA-256 of the DER-encoded cert
    known_peers: Arc<Mutex<HashMap<String, [u8; 32]>>>,
    /// Optional path for persisting `known_peers` across restarts
    peers_path: Option<PathBuf>,
}

impl CertManager {
    /// Create a new `CertManager` with a freshly generated identity cert.
    ///
    /// Known peers are kept in memory only (not persisted).
    pub fn new() -> Result<Self> {
        let (cert_der, key_der) = generate_identity()?;
        Ok(Self {
            cert_der,
            key_der,
            known_peers: Arc::new(Mutex::new(HashMap::new())),
            peers_path: None,
        })
    }

    /// Create a `CertManager` backed by files in `identity_dir`.
    ///
    /// - If `identity_dir/identity.der` and `identity_dir/identity.key` exist, they
    ///   are loaded; otherwise a new identity is generated and saved.
    /// - Known peers are loaded from `identity_dir/known_peers.json` if it exists.
    pub fn load_or_create(identity_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(identity_dir).context("Failed to create identity directory")?;

        let cert_path = identity_dir.join("identity.der");
        let key_path = identity_dir.join("identity.key");

        let (cert_der, key_der) = if cert_path.exists() && key_path.exists() {
            let cert = std::fs::read(&cert_path)?;
            let key = std::fs::read(&key_path)?;
            (cert, key)
        } else {
            let (cert, key) = generate_identity()?;
            std::fs::write(&cert_path, &cert)?;
            // The private half of the TOFU-pinned identity: owner-only.
            crate::secure_fs::write_private(&key_path, &key)?;
            (cert, key)
        };

        let peers_path = identity_dir.join("known_peers.json");
        let known_peers = if peers_path.exists() {
            let data =
                std::fs::read_to_string(&peers_path).context("Failed to read known peers")?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            cert_der,
            key_der,
            known_peers: Arc::new(Mutex::new(known_peers)),
            peers_path: Some(peers_path),
        })
    }

    /// Build a QUIC server TLS configuration that presents this device's identity
    /// certificate to connecting clients.
    pub fn server_config(&self, alpns: Vec<Vec<u8>>) -> Result<quinn::ServerConfig> {
        let cert = CertificateDer::from(self.cert_der.clone());
        let key = PrivateKeyDer::try_from(self.key_der.clone())
            .map_err(|_| anyhow::anyhow!("Failed to parse private key"))?;

        let transport = configured_transport();

        // Feature unification pulls both aws-lc-rs (iroh/reqwest) and ring into
        // server binaries, so rustls cannot auto-pick a process-level provider.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let mut crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .context("Failed to configure TLS server")?;

        if !alpns.is_empty() {
            crypto.alpn_protocols = alpns;
        }

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
        ));
        server_config.transport_config(Arc::new(transport));

        Ok(server_config)
    }

    /// Build a QUIC client TLS configuration with TOFU certificate verification.
    ///
    /// The client verifies the server's certificate against the stored known_peers
    /// map and auto-accepts unknown peers on first use.
    pub fn client_config(&self, alpns: Vec<Vec<u8>>) -> Result<quinn::ClientConfig> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let mut crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TofuVerifier {
                known_peers: self.known_peers.clone(),
                peers_path: self.peers_path.clone(),
            }))
            .with_no_client_auth();

        if !alpns.is_empty() {
            crypto.alpn_protocols = alpns;
        }

        let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .context("Failed to create QUIC client crypto config")?;

        let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
        client_config.transport_config(Arc::new(configured_transport()));

        Ok(client_config)
    }

    /// Save the known peers map to the configured path.
    pub fn save_known_peers(&self) -> Result<()> {
        if let Some(path) = &self.peers_path {
            let peers = self.known_peers.lock().unwrap();
            let json = serde_json::to_string_pretty(&*peers)?;
            std::fs::write(path, json)?;
        }
        Ok(())
    }

    /// Return a copy of all known peer labels and their certificate hashes.
    pub fn known_peers(&self) -> Vec<(String, [u8; 32])> {
        let peers = self.known_peers.lock().unwrap();
        peers.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    /// Check if a peer with the given label is already trusted.
    pub fn is_peer_trusted(&self, label: &str) -> bool {
        self.known_peers.lock().unwrap().contains_key(label)
    }

    /// Trust `hash` for the peer `label`, replacing whatever was pinned, and
    /// save. Callers decide whether replacing is allowed: the Android bridge
    /// refuses to replace a different pin from an (unauthenticated) link.
    pub fn pin_peer(&self, label: &str, hash: [u8; 32]) -> Result<()> {
        self.known_peers
            .lock()
            .unwrap()
            .insert(label.to_string(), hash);
        self.save_known_peers()
    }

    /// Remove a known peer by label. Returns true if the peer was found and removed.
    pub fn remove_peer(&self, label: &str) -> bool {
        let mut peers = self.known_peers.lock().unwrap();
        peers.remove(label).is_some()
    }

    /// Return the SHA-256 hash of the given DER certificate.
    fn cert_hash(cert: &CertificateDer<'_>) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(cert.as_ref());
        hasher.finalize().into()
    }
}

/// Generate a fresh ECDSA P-256 identity certificate and key.
fn generate_identity() -> Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate ECDSA key pair")?;
    let params = CertificateParams::new(vec!["linux-link.local".into()])
        .context("Failed to create certificate params")?;
    let cert = params
        .self_signed(&key_pair)
        .context("Failed to self-sign certificate")?;

    Ok((cert.der().to_vec(), key_pair.serialize_der()))
}

/// The label a peer's certificate pin is stored under: the server name the
/// connection was made to. ServerName has no Display; its DNS name is a
/// string and its IP's Debug output is stable.
fn peer_label(server_name: &ServerName<'_>) -> String {
    match server_name {
        ServerName::DnsName(dns) => dns.as_ref().to_string(),
        ServerName::IpAddress(addr) => format!("{:?}", addr),
        _ => format!("{:?}", server_name),
    }
}

/// [`peer_label`] for a connection made to `ip` (the client connects with
/// the IP as its server name).
pub fn peer_label_for_ip(ip: std::net::IpAddr) -> String {
    peer_label(&ServerName::IpAddress(ip.into()))
}

/// TOFU (Trust On First Use) certificate verifier.
///
/// - **First connection:** auto-accepts any server certificate and stores its
///   SHA-256 hash for future verification.
/// - **Subsequent connections:** verifies that the presented certificate's hash
///   matches the stored value. A mismatch indicates a potential MITM attack.
#[derive(Debug)]
struct TofuVerifier {
    known_peers: Arc<Mutex<HashMap<String, [u8; 32]>>>,
    /// Optional path for persisting known peers to disk on first-accept.
    peers_path: Option<PathBuf>,
}

impl rustls::client::danger::ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let hash = CertManager::cert_hash(end_entity);
        let label = peer_label(server_name);

        let mut peers = self.known_peers.lock().unwrap();

        match peers.get(&label) {
            Some(stored) if *stored == hash => {
                // Known peer with matching cert — verified successfully.
                debug!("TOFU: verified cert for {label}");
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            Some(_) => {
                // Known peer with a DIFFERENT cert — possible MITM!
                warn!("TOFU: certificate hash mismatch for {label}! Possible MITM attack.");
                Err(rustls::Error::General(format!(
                    "Certificate for {label} has changed since the last connection. \
                     This could be a man-in-the-middle attack. \
                     If you recently reinstalled the remote device, \
                     delete its entry and reconnect."
                )))
            }
            None => {
                // New peer — TOFU: auto-accept and store.
                info!("TOFU: first connection to {label}, accepting cert");
                peers.insert(label, hash);
                // Persist to disk so the trust survives restarts.
                if let Some(path) = &self.peers_path
                    && let Ok(json) = serde_json::to_string_pretty(&*peers)
                    && let Err(e) = std::fs::write(path, json)
                {
                    warn!("TOFU: failed to persist known peers: {e}");
                }
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Server-side streaming transport
pub struct StreamServer {
    endpoint: quinn::Endpoint,
    _config: StreamTransportConfig,
}

impl StreamServer {
    /// Create a new streaming server endpoint
    pub async fn new(config: StreamTransportConfig, cert_manager: &CertManager) -> Result<Self> {
        info!(
            "Creating streaming server on {} (datagrams={}, alpn={:?})",
            config.address,
            config.use_datagrams,
            String::from_utf8_lossy(&config.alpn)
        );

        let server_config = cert_manager.server_config(vec![config.alpn.clone()])?;

        let endpoint = quinn::Endpoint::server(server_config, config.address)
            .context("Failed to create endpoint")?;

        Ok(Self {
            endpoint,
            _config: config,
        })
    }

    /// Accept incoming connections
    pub async fn accept_connection(&self) -> Option<quinn::Incoming> {
        self.endpoint.accept().await
    }

    /// Handle a single streaming connection
    pub async fn handle_connection(
        &self,
        connection: quinn::Connection,
        _packet_tx: mpsc::Sender<EncodedPacket>,
    ) -> Result<()> {
        let peer = connection.remote_address();
        info!("Streaming connection from {}", peer);

        loop {
            // Accept incoming streams from client (for input events)
            let mut stream = match connection.accept_uni().await {
                Ok(stream) => stream,
                Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                    debug!("Streaming connection closed by peer");
                    break;
                }
                Err(e) => {
                    warn!("Stream accept error: {}", e);
                    break;
                }
            };

            // Read packets from the stream
            // This would be used if the client sends acknowledgments or feedback
            let _ = stream.read_to_end(1024).await;
        }

        Ok(())
    }

    /// Get the server address
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.endpoint.local_addr().context("No local address")
    }
}

/// Client-side streaming transport
pub struct StreamClient {
    endpoint: quinn::Endpoint,
    _config: StreamTransportConfig,
}

impl StreamClient {
    /// Create a new streaming client
    pub fn new(config: StreamTransportConfig, cert_manager: &CertManager) -> Result<Self> {
        info!("Creating streaming client");

        let client_config = cert_manager.client_config(vec![config.alpn.clone()])?;

        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)
            .context("Failed to create client endpoint")?;
        endpoint.set_default_client_config(client_config);

        Ok(Self {
            endpoint,
            _config: config,
        })
    }

    /// Connect to the streaming server
    pub async fn connect(
        &self,
        server_addr: SocketAddr,
        server_name: &str,
    ) -> Result<quinn::Connection> {
        info!(
            "Connecting to streaming server at {} (identity: {})",
            server_addr, server_name
        );

        let connection = self
            .endpoint
            .connect(server_addr, server_name)
            .context("Failed to initiate connection")?
            .await
            .context("Connection failed")?;

        info!("Streaming connection established");
        Ok(connection)
    }
}

/// Send encoded packets over a QUIC connection
pub async fn send_packets(
    connection: &quinn::Connection,
    mut packets: mpsc::Receiver<EncodedPacket>,
) -> Result<()> {
    info!("Starting packet sender");

    while let Some(packet) = packets.recv().await {
        // Open a unidirectional stream for this packet
        // In production, we'd reuse streams and use datagrams for lower latency
        let mut send_stream = connection.open_uni().await?;

        // Send packet header
        let header = PacketHeader {
            sequence: packet.sequence,
            stream_kind: STREAM_KIND_VIDEO,
            is_keyframe: packet.is_keyframe,
            timestamp_us: packet.timestamp.elapsed().as_micros() as u64,
        };

        send_stream
            .write_all(&header.as_bytes())
            .await
            .context("Failed to send header")?;

        // Send packet data
        send_stream
            .write_all(&packet.data)
            .await
            .context("Failed to send packet")?;

        send_stream.finish().context("Failed to finish stream")?;
    }

    Ok(())
}

/// Receive encoded packets from a QUIC connection
pub async fn receive_packets(
    connection: &quinn::Connection,
    packet_tx: mpsc::Sender<EncodedPacket>,
) -> Result<()> {
    info!("Starting packet receiver");

    loop {
        let mut recv_stream = match connection.accept_uni().await {
            Ok(stream) => stream,
            Err(quinn::ConnectionError::ApplicationClosed(_)) => {
                debug!("Connection closed");
                break;
            }
            Err(e) => {
                warn!("Stream accept error: {}", e);
                break;
            }
        };

        // Read header (18 bytes)
        let mut header_bytes = [0u8; 18];
        recv_stream
            .read_exact(&mut header_bytes)
            .await
            .context("Failed to read header")?;

        let header = PacketHeader::from_bytes(&header_bytes)?;

        // Read packet data
        let data = recv_stream
            .read_to_end(10 * 1024 * 1024) // 10 MB max packet
            .await
            .context("Failed to read packet data")?;

        let packet = EncodedPacket {
            data,
            is_keyframe: header.is_keyframe,
            timestamp: std::time::Instant::now(), // Server timestamp not preserved
            sequence: header.sequence,
        };

        packet_tx
            .send(packet)
            .await
            .context("Failed to forward packet")?;
    }

    Ok(())
}

/// Stream kind identifiers
pub const STREAM_KIND_VIDEO: u8 = 0;
pub const STREAM_KIND_AUDIO: u8 = 1;

/// Packet header for streaming transport
///
/// Bytes:
///   [0..8]  sequence     (u64 LE)
///   [8..9]  stream_kind  (0=video, 1=audio, etc.)
///   [9..10] flags        (bit 0 = is_keyframe)
///   [10..18] timestamp_us (u64 LE)
/// Total: 18 bytes
#[derive(Debug, Clone, Copy)]
pub struct PacketHeader {
    pub sequence: u64,
    pub stream_kind: u8,
    pub is_keyframe: bool,
    pub timestamp_us: u64,
}

impl PacketHeader {
    pub const SIZE: usize = 18;

    pub fn as_bytes(&self) -> [u8; 18] {
        let mut bytes = [0u8; 18];
        bytes[0..8].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[8] = self.stream_kind;
        bytes[9] = if self.is_keyframe { 1 } else { 0 };
        bytes[10..18].copy_from_slice(&self.timestamp_us.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8; 18]) -> Result<Self> {
        Ok(Self {
            sequence: u64::from_le_bytes(bytes[0..8].try_into()?),
            stream_kind: bytes[8],
            is_keyframe: bytes[9] != 0,
            timestamp_us: u64::from_le_bytes(bytes[10..18].try_into()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pairing code pins the desktop's certificate before the phone ever
    /// connects; the pin must sit under the exact label the verifier looks up
    /// for a connection made to that IP.
    #[test]
    fn a_pinned_ip_is_trusted_under_the_verifiers_label() {
        let dir = std::env::temp_dir().join(format!("ll-pin-peer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ip: std::net::IpAddr = "192.168.1.101".parse().unwrap();
        let label = peer_label_for_ip(ip);
        assert_eq!(
            label,
            peer_label(&ServerName::try_from(ip.to_string()).unwrap())
        );
        let manager = CertManager::load_or_create(&dir).unwrap();
        manager.pin_peer(&label, [7u8; 32]).unwrap();
        assert!(manager.is_peer_trusted(&label));
        // Persisted: a manager made for the next connection sees it.
        let next = CertManager::load_or_create(&dir).unwrap();
        assert!(next.known_peers().contains(&(label, [7u8; 32])));
    }

    #[test]
    fn test_packet_header_roundtrip() {
        let header = PacketHeader {
            sequence: 12345,
            stream_kind: STREAM_KIND_VIDEO,
            is_keyframe: true,
            timestamp_us: 999999,
        };

        let bytes = header.as_bytes();
        let recovered = PacketHeader::from_bytes(&bytes).unwrap();

        assert_eq!(recovered.sequence, 12345);
        assert_eq!(recovered.stream_kind, STREAM_KIND_VIDEO);
        assert!(recovered.is_keyframe);
        assert_eq!(recovered.timestamp_us, 999999);
    }

    #[test]
    fn test_transport_config_default() {
        let config = StreamTransportConfig::default();
        assert_eq!(config.alpn, crate::protocol::ALPN_V1_STREAM);
        assert!(config.use_datagrams);
    }

    #[test]
    fn test_configured_transport_builds() {
        // Unknown values warn and fall back; any accepted value must build.
        // (Serial because LINUX_LINK_CC is process-global.)
        for value in ["", "bbr", "cubic", "new_reno", "bogus"] {
            // Test-only env mutation; no other test in this binary uses the var.
            unsafe { std::env::set_var("LINUX_LINK_CC", value) };
            let _ = configured_transport();
        }
        unsafe { std::env::remove_var("LINUX_LINK_CC") };
    }

    #[test]
    fn roaming_hardening_settings() {
        // TransportConfig has no public getters; its Debug dumps every field.
        let transport = configured_transport();
        let rendered = format!("{transport:?}");
        assert!(
            rendered.contains("keep_alive_interval: Some(15s)"),
            "keepalive missing: {rendered}"
        );
        assert!(
            rendered.contains("max_idle_timeout: Some(45000)"),
            "idle timeout missing: {rendered}"
        );
    }
}
