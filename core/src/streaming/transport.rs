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

/// Build the QUIC `TransportConfig` used by every Pocketdesk endpoint.
///
/// Tuned for real-time A/V: large datagram buffers and high stream
/// concurrency (video opens one unidirectional stream per frame). The
/// congestion controller is selectable for A/B testing via the
/// `POCKETDESK_CC` environment variable: `bbr`, `cubic`, or `new_reno`
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
    let requested = std::env::var("POCKETDESK_CC")
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
                "POCKETDESK_CC='{other}' is not one of bbr|cubic|new_reno — falling back to NewReno"
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

        // One identity, however many first loads run at once (the phone
        // loads it for every control connection): checked and made under
        // one lock, each file written whole and renamed into place, the key
        // before the certificate that marks the pair complete.
        let (cert_der, key_der) = {
            static IDENTITY: Mutex<()> = Mutex::new(());
            let _one = IDENTITY.lock().unwrap_or_else(|e| e.into_inner());
            if cert_path.exists() && key_path.exists() {
                (std::fs::read(&cert_path)?, std::fs::read(&key_path)?)
            } else {
                let (cert, key) = generate_identity()?;
                let key_tmp = key_path.with_extension(format!("key.{}.tmp", std::process::id()));
                // The private half of the TOFU-pinned identity: owner-only.
                crate::secure_fs::write_private(&key_tmp, &key)?;
                std::fs::rename(&key_tmp, &key_path)?;
                let cert_tmp = cert_path.with_extension(format!("der.{}.tmp", std::process::id()));
                std::fs::write(&cert_tmp, &cert)?;
                std::fs::rename(&cert_tmp, &cert_path)?;
                (cert, key)
            }
        };

        let peers_path = identity_dir.join("known_peers.json");
        // A file that will not parse is an error: read as empty, it would
        // trust any certificate for every address, and the next write would
        // drop every pin.
        let known_peers = read_pins(&peers_path)?;

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
        let transport = configured_transport();
        let mut crypto = self.tls_server_config()?;

        if !alpns.is_empty() {
            crypto.alpn_protocols = alpns;
        }

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?,
        ));
        server_config.transport_config(Arc::new(transport));

        Ok(server_config)
    }

    /// The TLS half of [`CertManager::server_config`].
    fn tls_server_config(&self) -> Result<rustls::ServerConfig> {
        // A phone proves its own key here (device keys, phase A): the
        // certificate is offered for, not required, so a session without one
        // is refused by the session gate with a reason instead of failing
        // the handshake.
        self.tls_server_config_with(AnyClientCert { mandatory: false })
    }

    /// The control channel's TLS server configuration (phase B): this
    /// device's certificate, and a client certificate required of every
    /// connection, since the control channel holds each device to its key.
    pub fn control_server_config(&self) -> Result<Arc<rustls::ServerConfig>> {
        let mut config = self.tls_server_config_with(AnyClientCert { mandatory: true })?;
        config.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
        Ok(Arc::new(config))
    }

    /// The control channel's TLS client configuration (phase B): the same
    /// certificate pins as the stream, and this device's certificate.
    pub fn control_client_config(&self) -> Arc<rustls::ClientConfig> {
        let mut config = self.tls_client_config();
        config.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
        Arc::new(config)
    }

    fn tls_server_config_with(&self, verifier: AnyClientCert) -> Result<rustls::ServerConfig> {
        let cert = CertificateDer::from(self.cert_der.clone());
        let key = PrivateKeyDer::try_from(self.key_der.clone())
            .map_err(|_| anyhow::anyhow!("Failed to parse private key"))?;

        // Feature unification pulls both aws-lc-rs (iroh/reqwest) and ring into
        // server binaries, so rustls cannot auto-pick a process-level provider.
        let _ = rustls::crypto::ring::default_provider().install_default();

        // TLS 1.3 only: QUIC needs it, and on the control channel both ends
        // are Pocketdesk, so there is nothing older to speak to.
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(vec![cert], key)
            .context("Failed to configure TLS server")
    }

    /// Build a QUIC client TLS configuration with TOFU certificate verification.
    ///
    /// The client verifies the server's certificate against the stored known_peers
    /// map and auto-accepts unknown peers on first use.
    pub fn client_config(&self, alpns: Vec<Vec<u8>>) -> Result<quinn::ClientConfig> {
        let mut crypto = self.tls_client_config();

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
        if let Some(path) = &self.peers_path {
            update_pins(path, |pins| {
                pins.insert(label.to_string(), hash);
            })?;
        }
        Ok(())
    }

    /// Forget the peer `label`, here and on disk. Returns whether it was
    /// pinned.
    pub fn remove_peer(&self, label: &str) -> Result<bool> {
        let mut removed = self.known_peers.lock().unwrap().remove(label).is_some();
        if let Some(path) = &self.peers_path {
            update_pins(path, |pins| removed |= pins.remove(label).is_some())?;
        }
        Ok(removed)
    }

    /// A file transfer's TLS server configuration (phase C; the phone's
    /// side): this device's certificate, and a client certificate required
    /// to be exactly `desktop`, the one pinned for the computer the request
    /// went to.
    pub fn transfer_server_config(&self, desktop: [u8; 32]) -> Result<Arc<rustls::ServerConfig>> {
        let cert = CertificateDer::from(self.cert_der.clone());
        let key = PrivateKeyDer::try_from(self.key_der.clone())
            .map_err(|_| anyhow::anyhow!("Failed to parse private key"))?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config =
            rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
                .with_client_cert_verifier(Arc::new(OnlyCert(desktop)))
                .with_single_cert(vec![cert], key)
                .context("Failed to configure the transfer's TLS")?;
        config.alpn_protocols = vec![crate::protocol::ALPN_TRANSFER.to_vec()];
        Ok(Arc::new(config))
    }

    /// A file transfer's TLS client configuration (phase C; the desktop's
    /// side): this device's certificate, and a server required to be
    /// exactly `phone`, the key the requesting connection proved.
    pub fn transfer_client_config(&self, phone: [u8; 32]) -> Arc<rustls::ClientConfig> {
        let mut config = self.client_with_own_cert(Arc::new(OnlyCert(phone)));
        config.alpn_protocols = vec![crate::protocol::ALPN_TRANSFER.to_vec()];
        Arc::new(config)
    }

    /// The TLS half of [`CertManager::client_config`]: TOFU certificate pins.
    fn tls_client_config(&self) -> rustls::ClientConfig {
        self.client_with_own_cert(Arc::new(TofuVerifier {
            known_peers: self.known_peers.clone(),
            peers_path: self.peers_path.clone(),
        }))
    }

    /// A client that checks the server with `verifier` and presents this
    /// device's certificate.
    fn client_with_own_cert(
        &self,
        verifier: Arc<dyn rustls::client::danger::ServerCertVerifier>,
    ) -> rustls::ClientConfig {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let builder =
            rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
                .dangerous()
                .with_custom_certificate_verifier(verifier);
        // This device's own certificate, so the desktop can bind the device
        // to the key behind it. A key that will not load leaves the client
        // anonymous (the desktop then decides) rather than unable to connect.
        let cert = CertificateDer::from(self.cert_der.clone());
        match PrivateKeyDer::try_from(self.key_der.clone()) {
            Ok(key) => match builder.clone().with_client_auth_cert(vec![cert], key) {
                Ok(config) => config,
                Err(e) => {
                    warn!("Client certificate not usable, connecting without it: {e}");
                    builder.with_no_client_auth()
                }
            },
            Err(_) => {
                warn!("Client key not usable, connecting without it");
                builder.with_no_client_auth()
            }
        }
    }

    /// SHA-256 of this device's own certificate: what a peer sees as its
    /// [`PeerKey::Tls`](super::connection::PeerKey::Tls).
    pub fn cert_fingerprint(&self) -> [u8; 32] {
        Self::cert_hash(&CertificateDer::from(self.cert_der.clone()))
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
    let params = CertificateParams::new(vec!["pocketdesk.local".into()])
        .context("Failed to create certificate params")?;
    let cert = params
        .self_signed(&key_pair)
        .context("Failed to self-sign certificate")?;

    Ok((cert.der().to_vec(), key_pair.serialize_der()))
}

/// One writer of a pin file at a time, in this process. Each write re-reads
/// the file and applies only its own change, so managers loaded at
/// different times (one per connection) never drop each other's pins.
static PINS_FILE: Mutex<()> = Mutex::new(());

/// The pins in `path`; none if there is no file yet. A file that will not
/// parse is an error.
fn read_pins(path: &Path) -> Result<HashMap<String, [u8; 32]>> {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data)
            .with_context(|| format!("The certificate pins in {} are damaged", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(e).context("Failed to read known peers"),
    }
}

/// Apply `change` to the pins on disk and replace the file whole (a temp
/// file, then a rename). Returns the pins as written.
fn update_pins(
    path: &Path,
    change: impl FnOnce(&mut HashMap<String, [u8; 32]>),
) -> Result<HashMap<String, [u8; 32]>> {
    let _one = PINS_FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut pins = read_pins(path)?;
    change(&mut pins);
    let tmp = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&tmp, serde_json::to_string_pretty(&pins)?)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).context("Failed to save known peers");
    }
    Ok(pins)
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

/// [`peer_label`] for a connection made to `address`, as the control
/// channel dials it: an IP address, or a host name (a computer saved by
/// name). `None` for text that is neither.
pub fn peer_label_for_address(address: &str) -> Option<String> {
    ServerName::try_from(address.to_string())
        .ok()
        .map(|name| peer_label(&name))
}

/// Who answers at an address, judged against the pin this device holds for
/// it (the Connection Doctor, T1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityProbe {
    /// The certificate is the one pinned for this address.
    Matches,
    /// A certificate other than the pinned one: a reinstall, another
    /// computer on that address, or interception. Only pairing again fixes it.
    Changed,
    /// Something speaks TLS there, but no pin is held for the address.
    NotPinned,
    /// Something answered, but not in TLS.
    NotPocketdesk,
    /// The connection was closed before a certificate came: a busy
    /// Pocketdesk (its connection limit), or one that refused the handshake.
    Closed,
    /// Nothing could be learned: no connection, a bad address, or an
    /// unreadable pin file.
    Failed,
}

impl CertManager {
    /// Who answers at `address:port`, compared with the pin held for that
    /// address. Unlike a connection, this never saves a pin, so testing an
    /// address can never trust it. A match needs the handshake signature too,
    /// since anyone can send a copy of a certificate; the handshake stops
    /// once that is checked.
    pub async fn probe_identity(
        &self,
        address: &str,
        port: u16,
        timeout: std::time::Duration,
    ) -> IdentityProbe {
        let Some(label) = peer_label_for_address(address) else {
            return IdentityProbe::Failed;
        };
        let pinned = match &self.peers_path {
            Some(path) => match read_pins(path) {
                Ok(pins) => pins.get(&label).copied(),
                Err(_) => return IdentityProbe::Failed,
            },
            None => self.known_peers.lock().unwrap().get(&label).copied(),
        };
        let record = Arc::new(RecordCert::default());
        // As the other configurations do: the process default is otherwise
        // unset in the app, and rustls refuses to guess between providers.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(record.clone())
            .with_no_client_auth();
        config.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
        let Ok(server_name) = ServerName::try_from(address.to_string()) else {
            return IdentityProbe::Failed;
        };
        // Nothing listening (or no route) is the TCP check's to report.
        let tcp =
            match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((address, port)))
                .await
            {
                Ok(Ok(tcp)) => tcp,
                _ => return IdentityProbe::Failed,
            };
        let handshake = tokio::time::timeout(
            timeout,
            tokio_rustls::TlsConnector::from(Arc::new(config)).connect(server_name, tcp),
        )
        .await;
        let presented = *record.seen.lock().unwrap();
        let proved = record.proved.load(std::sync::atomic::Ordering::SeqCst);
        match (presented, pinned) {
            // The pinned certificate, and proof of its key.
            (Some(cert), Some(pin)) if cert == pin && proved => IdentityProbe::Matches,
            // Another certificate, or the pinned one without its key.
            (Some(_), Some(_)) => IdentityProbe::Changed,
            (Some(_), None) => IdentityProbe::NotPinned,
            (None, _) => match handshake {
                Ok(Err(e)) if not_tls(&e) => IdentityProbe::NotPocketdesk,
                Ok(Err(_)) => IdentityProbe::Closed,
                _ => IdentityProbe::Failed,
            },
        }
    }
}

/// Whether a failed handshake met something that does not speak TLS (as
/// opposed to a connection closed or refused by a TLS server).
fn not_tls(error: &std::io::Error) -> bool {
    matches!(
        error
            .get_ref()
            .and_then(|e| e.downcast_ref::<rustls::Error>()),
        Some(
            rustls::Error::InvalidMessage(_)
                | rustls::Error::InappropriateMessage { .. }
                | rustls::Error::InappropriateHandshakeMessage { .. }
        )
    )
}

/// Records the certificate a server presents and whether the server proved
/// its key with the handshake signature, then ends the handshake: the
/// Connection Doctor needs to know which certificate it is, and that it is
/// not a copy.
#[derive(Debug, Default)]
struct RecordCert {
    seen: Mutex<Option<[u8; 32]>>,
    proved: std::sync::atomic::AtomicBool,
}

impl RecordCert {
    fn done(
        &self,
        verified: std::result::Result<
            rustls::client::danger::HandshakeSignatureValid,
            rustls::Error,
        >,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        if verified.is_ok() {
            self.proved.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // Stop here either way: nothing past the proof is needed.
        Err(rustls::Error::General("identity probe: done".into()))
    }
}

impl rustls::client::danger::ServerCertVerifier for RecordCert {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap() = Some(CertManager::cert_hash(end_entity));
        // Accepted for now, so the signature that proves the key is checked.
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.done(rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &signature_algorithms(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.done(rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &signature_algorithms(),
        ))
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
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
        // Not pinned here: it may have been pinned on disk since this
        // manager loaded (another connection, a pairing code). The file
        // decides before anything is accepted on first use.
        if !peers.contains_key(&label)
            && let Some(path) = &self.peers_path
        {
            let on_disk = read_pins(path).map_err(|e| rustls::Error::General(e.to_string()))?;
            if let Some(pinned) = on_disk.get(&label) {
                peers.insert(label.clone(), *pinned);
            }
        }

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
                // Persist first, merged with the file: if another connection
                // pinned this address a moment ago, that pin wins.
                if let Some(path) = &self.peers_path {
                    let pins = update_pins(path, |pins| {
                        pins.entry(label.clone()).or_insert(hash);
                    })
                    .map_err(|e| rustls::Error::General(e.to_string()))?;
                    if pins.get(&label) != Some(&hash) {
                        warn!("TOFU: {label} was pinned to another certificate meanwhile");
                        return Err(rustls::Error::General(format!(
                            "Certificate for {label} does not match its pin."
                        )));
                    }
                }
                info!("TOFU: first connection to {label}, accepting cert");
                peers.insert(label, hash);
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
        }
    }

    // The pin above only says which certificate the desktop has; anyone can
    // present a copy of it (every handshake sends it). The handshake
    // signature is what proves the peer holds that certificate's private
    // key, so it is checked for real, with the same provider the connection
    // uses.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

/// The server's view of a client certificate (device keys, phase A): any
/// certificate is welcome, since the session gate decides whether it is the
/// one bound to the device that claims it, but the handshake signature must
/// prove the client holds its key. `mandatory` refuses a client that sends
/// no certificate at all (the control channel).
#[derive(Debug)]
struct AnyClientCert {
    mandatory: bool,
}

impl rustls::server::danger::ClientCertVerifier for AnyClientCert {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

/// A peer that must present exactly one certificate (a file transfer, phase
/// C: each end knows which key the other proved on the control channel),
/// and prove it holds its key.
#[derive(Debug)]
struct OnlyCert([u8; 32]);

impl OnlyCert {
    fn check(&self, cert: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        if CertManager::cert_hash(cert) == self.0 {
            Ok(())
        } else {
            Err(rustls::Error::General(
                "not the certificate this transfer expects".into(),
            ))
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for OnlyCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

impl rustls::server::danger::ClientCertVerifier for OnlyCert {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

/// The handshake signature algorithms of the provider the connections use.
fn signature_algorithms() -> rustls::crypto::WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
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
/// Server to client, once, on an input-only session (T10i): the session was
/// admitted (the gate and the owner's approval passed), so input sent from
/// now on is injected. No payload.
pub const STREAM_KIND_READY: u8 = 2;
/// Server to client, on a cast session (T10k): send a keyframe now (a viewer
/// joined or lost its way). No payload.
pub const STREAM_KIND_CAST_KEYFRAME: u8 = 3;
/// Server → phone on a cast: a batch of control events (C8,
/// [`super::cast_control`]), sent only after the phone said control is on.
pub const STREAM_KIND_CAST_CONTROL: u8 = 4;

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

    /// A server that presents `cert` but signs the handshake with `signing_key`.
    fn tls_server(cert: Vec<u8>, signing_key: Vec<u8>) -> rustls::ServerConfig {
        #[derive(Debug)]
        struct Fixed(Arc<rustls::sign::CertifiedKey>);
        impl rustls::server::ResolvesServerCert for Fixed {
            fn resolve(
                &self,
                _hello: rustls::server::ClientHello<'_>,
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                Some(self.0.clone())
            }
        }
        let key = rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(
            signing_key.into(),
        ))
        .unwrap();
        let certified = rustls::sign::CertifiedKey::new(vec![CertificateDer::from(cert)], key);
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Fixed(Arc::new(certified))))
    }

    /// Run a TLS handshake in memory; the client's verdict.
    fn handshake(
        client: rustls::ClientConfig,
        server: rustls::ServerConfig,
    ) -> std::result::Result<(), rustls::Error> {
        let name = ServerName::try_from("192.0.2.1").unwrap();
        let mut c = rustls::ClientConnection::new(Arc::new(client), name)?;
        let mut s = rustls::ServerConnection::new(Arc::new(server))?;
        for _ in 0..16 {
            let mut wire = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut wire).unwrap();
            }
            if !wire.is_empty() {
                s.read_tls(&mut &wire[..]).unwrap();
                // The server's own complaints are not the question here.
                let _ = s.process_new_packets();
            }
            let mut wire = Vec::new();
            while s.wants_write() {
                s.write_tls(&mut wire).unwrap();
            }
            if !wire.is_empty() {
                c.read_tls(&mut &wire[..]).unwrap();
                c.process_new_packets()?;
            }
            if !c.is_handshaking() && !s.is_handshaking() {
                return Ok(());
            }
        }
        Err(rustls::Error::General("handshake did not finish".into()))
    }

    /// Run a TLS handshake in memory; the server's verdict, and the client
    /// certificate it was shown.
    fn handshake_seen_by_server(
        client: rustls::ClientConfig,
        server: rustls::ServerConfig,
    ) -> std::result::Result<Option<Vec<u8>>, rustls::Error> {
        let name = ServerName::try_from("192.0.2.1").unwrap();
        let mut c = rustls::ClientConnection::new(Arc::new(client), name)?;
        let mut s = rustls::ServerConnection::new(Arc::new(server))?;
        for _ in 0..16 {
            let mut wire = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut wire).unwrap();
            }
            if !wire.is_empty() {
                s.read_tls(&mut &wire[..]).unwrap();
                s.process_new_packets()?;
            }
            let mut wire = Vec::new();
            while s.wants_write() {
                s.write_tls(&mut wire).unwrap();
            }
            if !wire.is_empty() {
                c.read_tls(&mut &wire[..]).unwrap();
                let _ = c.process_new_packets();
            }
            if !c.is_handshaking() && !s.is_handshaking() && !c.wants_write() {
                return Ok(s
                    .peer_certificates()
                    .and_then(|certs| certs.first())
                    .map(|cert| cert.to_vec()));
            }
        }
        Err(rustls::Error::General("handshake did not finish".into()))
    }

    /// A client that presents `cert` but signs with `signing_key`.
    fn tls_client(cert: Vec<u8>, signing_key: Vec<u8>) -> rustls::ClientConfig {
        #[derive(Debug)]
        struct Fixed(Arc<rustls::sign::CertifiedKey>);
        impl rustls::client::ResolvesClientCert for Fixed {
            fn resolve(
                &self,
                _hints: &[&[u8]],
                _schemes: &[rustls::SignatureScheme],
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                Some(self.0.clone())
            }
            fn has_certs(&self) -> bool {
                true
            }
        }
        let key = rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(
            signing_key.into(),
        ))
        .unwrap();
        let certified = rustls::sign::CertifiedKey::new(vec![CertificateDer::from(cert)], key);
        let any_server = CertManager::new().unwrap();
        let mut config = any_server.tls_client_config();
        config.client_auth_cert_resolver = Arc::new(Fixed(Arc::new(certified)));
        config
    }

    #[test]
    fn the_phone_proves_its_certificate_to_the_desktop() {
        let desktop = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let shown = handshake_seen_by_server(
            phone.tls_client_config(),
            desktop.tls_server_config().unwrap(),
        )
        .unwrap();
        assert_eq!(
            shown.as_deref(),
            Some(&phone.cert_der[..]),
            "the phone's own certificate"
        );

        // A copy of the phone's certificate, signed with another key: the
        // certificate alone proves nothing, so it must be refused.
        let (_, other_key) = generate_identity().unwrap();
        let forged = tls_client(phone.cert_der.clone(), other_key);
        assert!(
            handshake_seen_by_server(forged, desktop.tls_server_config().unwrap()).is_err(),
            "a copied client certificate without its key is refused"
        );
    }

    #[test]
    fn the_desktop_must_hold_the_key_of_the_certificate_it_presents() {
        let (cert, key) = generate_identity().unwrap();
        let (_, other_key) = generate_identity().unwrap();
        let phone = CertManager::new().unwrap();
        // Pinned, as a pairing code pins it.
        let label = peer_label(&ServerName::try_from("192.0.2.1").unwrap());
        phone
            .pin_peer(
                &label,
                CertManager::cert_hash(&CertificateDer::from(cert.clone())),
            )
            .unwrap();

        assert!(
            handshake(phone.tls_client_config(), tls_server(cert.clone(), key)).is_ok(),
            "the real desktop connects"
        );
        // Anyone can copy the certificate (every handshake sends it); only
        // the desktop has its key. A pin is worth nothing if a signature by
        // another key passes.
        assert!(
            handshake(phone.tls_client_config(), tls_server(cert, other_key)).is_err(),
            "a copied certificate without its key is refused"
        );
    }

    /// A pairing code pins the desktop's certificate before the phone ever
    /// connects; the pin must sit under the exact label the verifier looks up
    /// for a connection made to that IP.
    #[test]
    fn a_pinned_ip_is_trusted_under_the_verifiers_label() {
        let dir = std::env::temp_dir().join(format!("ll-pin-peer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ip: std::net::IpAddr = "192.168.1.20".parse().unwrap();
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

    fn pin_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-pins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn first_loads_at_once_make_one_identity() {
        let dir = pin_dir("identity-race");
        let loads: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || CertManager::load_or_create(&dir).unwrap())
            })
            .collect();
        let managers: Vec<CertManager> = loads.into_iter().map(|t| t.join().unwrap()).collect();
        let on_disk = CertManager::load_or_create(&dir).unwrap();
        for manager in &managers {
            assert_eq!(manager.cert_der, on_disk.cert_der, "one certificate");
            assert_eq!(manager.key_der, on_disk.key_der, "and its own key");
        }
    }

    #[test]
    fn the_pin_label_for_an_address_is_the_one_its_connection_uses() {
        let ip: std::net::IpAddr = "192.0.2.5".parse().unwrap();
        assert_eq!(
            peer_label_for_address("192.0.2.5").as_deref(),
            Some(peer_label_for_ip(ip).as_str())
        );
        assert_eq!(
            peer_label_for_address("mypc.local").as_deref(),
            Some(peer_label(&ServerName::try_from("mypc.local").unwrap()).as_str()),
            "a computer saved by name is pinned under its name"
        );
        assert_eq!(peer_label_for_address(""), None);
    }

    #[test]
    fn a_pin_one_manager_writes_is_not_lost_by_another() {
        let dir = pin_dir("merge");
        let control = CertManager::load_or_create(&dir).unwrap();
        let stream = CertManager::load_or_create(&dir).unwrap();
        control.pin_peer("a", [1u8; 32]).unwrap();
        stream.pin_peer("b", [2u8; 32]).unwrap();
        let next = CertManager::load_or_create(&dir).unwrap();
        let peers = next.known_peers();
        assert!(peers.contains(&("a".to_string(), [1u8; 32])), "{peers:?}");
        assert!(peers.contains(&("b".to_string(), [2u8; 32])), "{peers:?}");
    }

    #[test]
    fn a_removed_pin_is_not_written_back_by_a_manager_that_still_holds_it() {
        let dir = pin_dir("remove");
        let old = CertManager::load_or_create(&dir).unwrap();
        old.pin_peer("a", [1u8; 32]).unwrap();
        let ui = CertManager::load_or_create(&dir).unwrap();
        assert!(ui.remove_peer("a").unwrap());
        old.pin_peer("b", [2u8; 32]).unwrap();
        let peers = CertManager::load_or_create(&dir).unwrap().known_peers();
        assert!(!peers.iter().any(|(label, _)| label == "a"), "{peers:?}");
    }

    #[test]
    fn a_damaged_pin_file_is_an_error_not_an_empty_list() {
        let dir = pin_dir("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("known_peers.json"), "{not json").unwrap();
        assert!(
            CertManager::load_or_create(&dir).is_err(),
            "an empty list would trust any certificate for every address"
        );
    }

    #[test]
    fn a_manager_loaded_before_a_pin_does_not_accept_another_certificate_for_it() {
        let dir = pin_dir("stale");
        let stale = CertManager::load_or_create(&dir).unwrap();
        let ip: std::net::IpAddr = "192.0.2.30".parse().unwrap();
        let (desktop, _) = generate_identity().unwrap();
        CertManager::load_or_create(&dir)
            .unwrap()
            .pin_peer(
                &peer_label_for_ip(ip),
                CertManager::cert_hash(&CertificateDer::from(desktop)),
            )
            .unwrap();
        let (impostor, _) = generate_identity().unwrap();
        let verifier = TofuVerifier {
            known_peers: stale.known_peers.clone(),
            peers_path: stale.peers_path.clone(),
        };
        use rustls::client::danger::ServerCertVerifier;
        assert!(
            verifier
                .verify_server_cert(
                    &CertificateDer::from(impostor),
                    &[],
                    &ServerName::IpAddress(ip.into()),
                    &[],
                    rustls::pki_types::UnixTime::now(),
                )
                .is_err(),
            "the address was pinned since this manager loaded"
        );
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
        // (Serial because POCKETDESK_CC is process-global.)
        for value in ["", "bbr", "cubic", "new_reno", "bogus"] {
            // Test-only env mutation; no other test in this binary uses the var.
            unsafe { std::env::set_var("POCKETDESK_CC", value) };
            let _ = configured_transport();
        }
        unsafe { std::env::remove_var("POCKETDESK_CC") };
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

    /// Accepts any server certificate, for tests that only care whether a
    /// handshake gets as far as a certificate at all.
    #[derive(Debug)]
    struct AnyCert;

    impl rustls::client::danger::ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error>
        {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
        {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            signature_algorithms().supported_schemes()
        }
    }

    /// SECURITY.md's "TLS 1.3": a client that offers only TLS 1.2 is refused
    /// by the control channel, which speaks nothing older, while the same
    /// client (with a certificate, as the channel requires) gets in on 1.3.
    #[tokio::test]
    async fn the_control_channel_refuses_tls_1_2() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let desktop = CertManager::new().expect("desktop certs");
        let phone = CertManager::new().expect("phone certs");
        let attempt = |version: &'static rustls::SupportedProtocolVersion| {
            let mut config = rustls::ClientConfig::builder_with_protocol_versions(&[version])
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyCert))
                .with_client_auth_cert(
                    vec![CertificateDer::from(phone.cert_der.clone())],
                    PrivateKeyDer::try_from(phone.key_der.clone()).unwrap(),
                )
                .unwrap();
            config.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
            Arc::new(config)
        };
        for (version, ok) in [
            (&rustls::version::TLS12, false),
            (&rustls::version::TLS13, true),
        ] {
            let port = one_control_server(&desktop).await;
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let name = ServerName::try_from("127.0.0.1").unwrap();
            let result = tokio_rustls::TlsConnector::from(attempt(version))
                .connect(name, tcp)
                .await;
            assert_eq!(result.is_ok(), ok, "{version:?}");
        }
    }

    /// A control-channel TLS server on loopback for one connection; its port.
    async fn one_control_server(desktop: &CertManager) -> u16 {
        let config = desktop.control_server_config().expect("server config");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let _ = tokio_rustls::TlsAcceptor::from(config).accept(tcp).await;
            }
        });
        port
    }

    /// A TLS server on loopback that presents `desktop`'s certificate but
    /// signs with another key, as someone replaying it would; its port.
    async fn replaying_server(desktop: &CertManager) -> u16 {
        #[derive(Debug)]
        struct Fixed(Arc<rustls::sign::CertifiedKey>);
        impl rustls::server::ResolvesServerCert for Fixed {
            fn resolve(
                &self,
                _hello: rustls::server::ClientHello<'_>,
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                Some(self.0.clone())
            }
        }
        let impostor = CertManager::new().expect("impostor certs");
        let key = PrivateKeyDer::try_from(impostor.key_der.clone()).unwrap();
        let signer = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
        let certified = rustls::sign::CertifiedKey::new(
            vec![CertificateDer::from(desktop.cert_der.clone())],
            signer,
        );
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Fixed(Arc::new(certified))));
        config.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
        let config = Arc::new(config);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                let _ = tokio_rustls::TlsAcceptor::from(config).accept(tcp).await;
            }
        });
        port
    }

    fn pin_file(dir: &Path, pin: Option<[u8; 32]>) {
        let mut pins = std::collections::HashMap::new();
        if let Some(pin) = pin {
            pins.insert(
                peer_label_for_ip("127.0.0.1".parse().unwrap()),
                pin.to_vec(),
            );
        }
        std::fs::write(
            dir.join("known_peers.json"),
            serde_json::to_string(&pins).unwrap(),
        )
        .unwrap();
    }

    /// T1: the Connection Doctor tells the pinned computer from a changed or
    /// unknown one, and from something that is not TLS, and saves nothing.
    #[tokio::test]
    async fn an_identity_probe_compares_with_the_pin_and_never_saves_one() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let desktop = CertManager::new().expect("desktop certs");
        let dir = std::env::temp_dir().join(format!("pd-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wait = std::time::Duration::from_secs(5);

        let pins = dir.join("known_peers.json");
        let unchanged =
            |before: &[u8]| assert_eq!(std::fs::read(&pins).unwrap(), before, "never saves");

        pin_file(&dir, None);
        let phone = CertManager::load_or_create(&dir).unwrap();
        let before = std::fs::read(&pins).unwrap();
        let port = one_control_server(&desktop).await;
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::NotPinned
        );
        unchanged(&before);
        assert!(phone.known_peers.lock().unwrap().is_empty());

        pin_file(&dir, Some(desktop.cert_fingerprint()));
        let before = std::fs::read(&pins).unwrap();
        let port = one_control_server(&desktop).await;
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::Matches
        );
        unchanged(&before);

        // The desktop's certificate replayed by someone without its key.
        let port = replaying_server(&desktop).await;
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::Changed
        );
        unchanged(&before);

        pin_file(&dir, Some([7; 32]));
        let before = std::fs::read(&pins).unwrap();
        let port = one_control_server(&desktop).await;
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::Changed
        );
        unchanged(&before);

        // A Pocketdesk at its connection limit accepts, then drops.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((tcp, _)) = listener.accept().await {
                drop(tcp);
            }
        });
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::Closed
        );
        // A plain TCP service that answers in clear text.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut tcp, _)) = listener.accept().await {
                // As a web server would: read the request, answer, linger.
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut hello = [0u8; 512];
                let _ = tcp.read(&mut hello).await;
                let _ = tcp.write_all(b"HTTP/1.0 400 Bad Request\r\n\r\n").await;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        });
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::NotPocketdesk
        );

        // Nothing listening is not "not Pocketdesk": the TCP check says why.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        assert_eq!(
            phone.probe_identity("127.0.0.1", port, wait).await,
            IdentityProbe::Failed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A TLS handshake over a loopback TCP socket with the control channel's
    /// configurations; what each end learned of the other.
    async fn control_handshake(
        server: Arc<rustls::ServerConfig>,
        client: Arc<rustls::ClientConfig>,
    ) -> (
        std::result::Result<Option<super::super::PeerKey>, std::io::Error>,
        std::result::Result<Option<Vec<u8>>, std::io::Error>,
    ) {
        loopback_handshake(server, client, crate::protocol::ALPN_CONTROL).await
    }

    /// A TLS handshake over a loopback TCP socket, negotiating `alpn`; what
    /// each end learned of the other.
    async fn loopback_handshake(
        server: Arc<rustls::ServerConfig>,
        client: Arc<rustls::ClientConfig>,
        alpn: &'static [u8],
    ) -> (
        std::result::Result<Option<super::super::PeerKey>, std::io::Error>,
        std::result::Result<Option<Vec<u8>>, std::io::Error>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_side = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await?;
            let tls = tokio_rustls::TlsAcceptor::from(server).accept(tcp).await?;
            let (_, session) = tls.get_ref();
            assert_eq!(session.alpn_protocol(), Some(alpn));
            Ok(session
                .peer_certificates()
                .and_then(super::super::PeerKey::from_tls_certs))
        });
        let client_side = async {
            let tcp = tokio::net::TcpStream::connect(addr).await?;
            let name = ServerName::IpAddress(addr.ip().into());
            let mut tls = tokio_rustls::TlsConnector::from(client)
                .connect(name, tcp)
                .await?;
            // TLS 1.3 settles the client's certificate after the client's
            // handshake ends; a read is where a refusal surfaces.
            use tokio::io::AsyncReadExt;
            let mut byte = [0u8; 1];
            let _ =
                tokio::time::timeout(std::time::Duration::from_millis(300), tls.read(&mut byte))
                    .await;
            let (_, session) = tls.get_ref();
            Ok(session
                .peer_certificates()
                .and_then(|c| c.first())
                .map(|c| c.as_ref().to_vec()))
        };
        let client_seen = client_side.await;
        let server_seen = server_side.await.unwrap();
        (server_seen, client_seen)
    }

    #[tokio::test]
    async fn each_end_of_the_control_channel_proves_its_key() {
        let desktop = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let (server_seen, client_seen) = control_handshake(
            desktop.control_server_config().unwrap(),
            phone.control_client_config(),
        )
        .await;
        assert_eq!(
            server_seen.unwrap(),
            Some(super::super::PeerKey::Tls(phone.cert_fingerprint())),
            "the desktop knows the phone's key"
        );
        assert_eq!(client_seen.unwrap().as_deref(), Some(&desktop.cert_der[..]));
    }

    #[tokio::test]
    async fn a_transfer_runs_between_the_desktop_and_phone_each_side_expects() {
        let desktop = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let (phone_saw, desktop_saw) = loopback_handshake(
            phone
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
            desktop.transfer_client_config(phone.cert_fingerprint()),
            crate::protocol::ALPN_TRANSFER,
        )
        .await;
        assert_eq!(
            phone_saw.unwrap(),
            Some(super::super::PeerKey::Tls(desktop.cert_fingerprint()))
        );
        assert_eq!(desktop_saw.unwrap().as_deref(), Some(&phone.cert_der[..]));
    }

    #[tokio::test]
    async fn the_phone_sends_files_to_no_desktop_but_the_one_it_pinned() {
        let desktop = CertManager::new().unwrap();
        let impostor = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let (phone_saw, _) = loopback_handshake(
            phone
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
            impostor.transfer_client_config(phone.cert_fingerprint()),
            crate::protocol::ALPN_TRANSFER,
        )
        .await;
        assert!(phone_saw.is_err(), "another computer dialed in");
    }

    #[tokio::test]
    async fn the_desktop_sends_files_to_no_phone_but_the_one_that_asked() {
        let desktop = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let impostor = CertManager::new().unwrap();
        let (_, desktop_saw) = loopback_handshake(
            impostor
                .transfer_server_config(desktop.cert_fingerprint())
                .unwrap(),
            desktop.transfer_client_config(phone.cert_fingerprint()),
            crate::protocol::ALPN_TRANSFER,
        )
        .await;
        assert!(desktop_saw.is_err(), "another device answered");
    }

    #[tokio::test]
    async fn the_control_channel_refuses_a_client_without_a_certificate() {
        let desktop = CertManager::new().unwrap();
        let phone = CertManager::new().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut anonymous = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TofuVerifier {
                known_peers: phone.known_peers.clone(),
                peers_path: None,
            }))
            .with_no_client_auth();
        anonymous.alpn_protocols = vec![crate::protocol::ALPN_CONTROL.to_vec()];
        let (server_seen, _) = control_handshake(
            desktop.control_server_config().unwrap(),
            Arc::new(anonymous),
        )
        .await;
        assert!(server_seen.is_err(), "no certificate, no control channel");
    }
}
