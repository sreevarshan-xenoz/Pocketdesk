//! The v8 handshake with a KDE Connect device that dialed in (3a), from the
//! protocol's behaviour (design §"The protocol"); no KDE code.
//!
//! The device writes a plaintext connection packet; then, the roles being
//! reversed, this computer (the TCP server) runs TLS as the client; inside,
//! each side writes its full identity, and the device's must name the same
//! id and version as its first packet, and the id its certificate's
//! subject (CVE-2025-66270).

use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{NetworkPacket, is_invisible_format};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::rustls;

use super::identity::CompatIdentity;

/// The longest line a device may write.
pub const MAX_LINE: usize = 8192;

/// Each line must come within this.
pub const STEP: Duration = Duration::from_millis(1000);

/// The TLS handshake itself: a slow phone's takes longer than a line.
pub const TLS_STEP: Duration = Duration::from_secs(5);

/// The protocol version spoken, and the oldest taken.
pub const PROTOCOL: u64 = 8;

/// What this computer takes from a KDE Connect device, and sends one
/// (pairing needs no capability). A device turns its plugin for this
/// computer on when one of these meets the plugin's packets.
pub const INCOMING: &[&str] = &[
    "kdeconnect.ping",
    super::link::BATTERY,
    super::link::CLIPBOARD,
    super::share::SHARE,
    super::link::NOTIFICATION,
];
pub const OUTGOING: &[&str] = &["kdeconnect.ping", super::link::CLIPBOARD, super::link::RING];

/// Whether `id` is a KDE Connect device id: 32 to 38 letters, digits, `_`
/// or `-`.
pub fn valid_id(id: &str) -> bool {
    (32..=38).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A device's name as KDE writes it: without `"',;:.!?()[]<>`, control or
/// invisible characters, at most 32 of them.
pub fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !"\"',;:.!?()[]<>".contains(*c) && !c.is_control() && !is_invisible_format(*c))
        .take(32)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Who a device says it is, from an identity packet held to the rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub device_id: String,
    pub device_name: String,
    pub protocol: u64,
}

/// The identity packet `packet`, held to the rules: an identity, a valid
/// id, version 8 or later, and, if it names one, meant for `our_id`.
pub fn read_hello(packet: &NetworkPacket, our_id: &str) -> Result<Hello, String> {
    if packet.packet_type != "kdeconnect.identity" {
        // The device's own text, kept short and printable for the log.
        let said: String = packet.packet_type.chars().take(40).collect();
        return Err(format!("{said:?} where an identity belongs"));
    }
    let body = &packet.body;
    let device_id = body
        .get("deviceId")
        .and_then(|v| v.as_str())
        .ok_or("no device id")?;
    if !valid_id(device_id) {
        return Err("not a KDE Connect device id".to_string());
    }
    if device_id == our_id {
        return Err("this computer's own id".to_string());
    }
    let protocol = body
        .get("protocolVersion")
        .and_then(|v| v.as_u64())
        .ok_or("no protocol version")?;
    if protocol < PROTOCOL {
        return Err(format!("protocol {protocol}, older than {PROTOCOL}"));
    }
    if let Some(target) = body.get("targetDeviceId").and_then(|v| v.as_str())
        && target != our_id
    {
        return Err("meant for another device".to_string());
    }
    let name = clean_name(
        body.get("deviceName")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    Ok(Hello {
        device_id: device_id.to_string(),
        device_name: if name.is_empty() {
            "KDE Connect device".to_string()
        } else {
            name
        },
        protocol,
    })
}

/// This computer's identity packet, for a broadcast and inside TLS.
pub fn identity_packet(our_id: &str, name: &str, tcp_port: u16) -> NetworkPacket {
    let mut packet = NetworkPacket::new("kdeconnect.identity").with_body(serde_json::json!({
        "deviceId": our_id,
        "deviceName": clean_name(name),
        "deviceType": "desktop",
        "protocolVersion": PROTOCOL,
        "incomingCapabilities": INCOMING,
        "outgoingCapabilities": OUTGOING,
        "tcpPort": tcp_port,
    }));
    packet.id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    packet
}

/// The common name a certificate's subject gives, if any.
pub fn subject_id(der: &[u8]) -> Option<String> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    cert.subject()
        .iter_common_name()
        .next()?
        .as_str()
        .ok()
        .map(str::to_string)
}

/// One line of at most `max` bytes within `within`, read a byte at a time:
/// TLS follows the plaintext packet, and none of it may be taken.
pub async fn read_line<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
    within: Duration,
) -> Result<String, String> {
    tokio::time::timeout(within, async {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if reader.read(&mut byte).await.map_err(|e| e.to_string())? == 0 {
                return Err("it closed the connection".to_string());
            }
            if byte[0] == b'\n' {
                return String::from_utf8(line).map_err(|_| "a line that is not text".to_string());
            }
            if line.len() == max {
                return Err(format!("a line longer than {max} bytes"));
            }
            line.push(byte[0]);
        }
    })
    .await
    .map_err(|_| "no line in time".to_string())?
}

/// The TLS client configuration for a device that said it is `device_id`:
/// this computer's compat certificate, TLS 1.2 or 1.3 (KDE Connect Android
/// caps at 1.2; this path only), and the device's certificate taken only if
/// its subject is that id, and once paired, only the pinned one.
pub fn client_config(
    identity: &CompatIdentity,
    device_id: &str,
    pinned: Option<[u8; 32]>,
) -> Result<Arc<rustls::ClientConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|_| "the compat key does not read".to_string())?;
    let config = rustls::ClientConfig::builder_with_protocol_versions(&[
        &rustls::version::TLS13,
        &rustls::version::TLS12,
    ])
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(DeviceCertificate {
        id: device_id.to_string(),
        pinned,
    }))
    .with_client_auth_cert(
        vec![rustls::pki_types::CertificateDer::from(
            identity.cert_der.clone(),
        )],
        key,
    )
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

/// A device's certificate: its subject names its id; once paired, it is
/// the pinned one; its handshake signature is checked for real.
#[derive(Debug)]
struct DeviceCertificate {
    id: String,
    pinned: Option<[u8; 32]>,
}

fn signature_algorithms() -> rustls::crypto::WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

impl rustls::client::danger::ServerCertVerifier for DeviceCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if subject_id(end_entity.as_ref()).as_deref() != Some(self.id.as_str()) {
            return Err(rustls::Error::General(
                "the certificate does not name the device's id".to_string(),
            ));
        }
        if let Some(pinned) = self.pinned
            && cert_hash(end_entity.as_ref()) != pinned
        {
            return Err(rustls::Error::General(
                "not the certificate this device paired with".to_string(),
            ));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

/// The TLS server configuration for a paired device that comes for a file
/// this computer sends it (T6): this computer's compat certificate, TLS 1.2
/// or 1.3, and the device's certificate required, taken only if its
/// subject is `device_id` and it is the pinned one.
pub fn server_config(
    identity: &CompatIdentity,
    device_id: &str,
    pinned: [u8; 32],
) -> Result<Arc<rustls::ServerConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = rustls::pki_types::PrivateKeyDer::try_from(identity.key_der.clone())
        .map_err(|_| "the compat key does not read".to_string())?;
    let config = rustls::ServerConfig::builder_with_protocol_versions(&[
        &rustls::version::TLS13,
        &rustls::version::TLS12,
    ])
    .with_client_cert_verifier(Arc::new(DeviceClient(DeviceCertificate {
        id: device_id.to_string(),
        pinned: Some(pinned),
    })))
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(
            identity.cert_der.clone(),
        )],
        key,
    )
    .map_err(|e| e.to_string())?;
    Ok(Arc::new(config))
}

/// [`DeviceCertificate`]'s rules for a device on the client's side (T6).
#[derive(Debug)]
struct DeviceClient(DeviceCertificate);

impl rustls::server::danger::ClientCertVerifier for DeviceClient {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        use rustls::client::danger::ServerCertVerifier;
        let name = rustls::pki_types::ServerName::try_from("kdeconnect")
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        self.0
            .verify_server_cert(end_entity, intermediates, &name, &[], now)
            .map(|_| rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &signature_algorithms())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &signature_algorithms())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        signature_algorithms().supported_schemes()
    }
}

/// SHA-256 of a certificate, as pins are kept.
pub fn cert_hash(der: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(digest.as_ref());
    hash
}

/// A device's connection after the handshake.
pub struct DeviceLink<S> {
    pub stream: S,
    pub hello: Hello,
    /// Its certificate, for the pairing code.
    pub cert_der: Vec<u8>,
    /// SHA-256 of its certificate: what pairing pins.
    pub cert_hash: [u8; 32],
}

/// The handshake on a connection a device opened: its connection packet,
/// TLS with this computer as the client, then the identities both ways,
/// the device's held to its first. `pinned` gives a paired device's pin.
pub async fn accept<S>(
    mut stream: S,
    identity: &CompatIdentity,
    ours: &NetworkPacket,
    pinned: impl Fn(&str) -> Option<[u8; 32]>,
) -> Result<DeviceLink<tokio_rustls::client::TlsStream<S>>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let line = read_line(&mut stream, MAX_LINE, STEP).await?;
    let first = NetworkPacket::from_wire(&line).map_err(|e| format!("not a packet: {e}"))?;
    let hello = read_hello(&first, &identity.id)?;
    let config = client_config(identity, &hello.device_id, pinned(&hello.device_id))?;
    // The device's certificate is judged by its subject, not by a name.
    let name = rustls::pki_types::ServerName::try_from("kdeconnect").map_err(|e| e.to_string())?;
    let mut tls = tokio::time::timeout(
        TLS_STEP,
        tokio_rustls::TlsConnector::from(config).connect(name, stream),
    )
    .await
    .map_err(|_| "no TLS handshake in time".to_string())?
    .map_err(|e| format!("TLS: {e}"))?;
    let cert_der = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.as_ref().to_vec())
        .ok_or("no certificate")?;
    let cert_hash = cert_hash(&cert_der);
    let wire = ours.to_wire().map_err(|e| e.to_string())?;
    tls.write_all(&wire).await.map_err(|e| e.to_string())?;
    tls.flush().await.map_err(|e| e.to_string())?;
    let line = read_line(&mut tls, MAX_LINE, STEP).await?;
    let second = NetworkPacket::from_wire(&line).map_err(|e| format!("not a packet: {e}"))?;
    let again = read_hello(&second, &identity.id)?;
    if again.device_id != hello.device_id || again.protocol != hello.protocol {
        return Err("its identity inside TLS is not the one it gave before".to_string());
    }
    Ok(DeviceLink {
        stream: tls,
        hello: again,
        cert_der,
        cert_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway identity in a folder of its own (tests run side by side).
    fn identity(name: &str) -> CompatIdentity {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        CompatIdentity::load_or_create(&dir).unwrap()
    }

    /// A client certificate verifier that takes any, as a device's does for
    /// a computer it has not paired with.
    #[derive(Debug)]
    struct AnyClient;

    impl rustls::server::danger::ClientCertVerifier for AnyClient {
        fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
            &[]
        }
        fn verify_client_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &signature_algorithms())
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &signature_algorithms())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            signature_algorithms().supported_schemes()
        }
    }

    /// A KDE Connect device that dialed in: it writes `first`, runs TLS as
    /// the server with `cert_of`'s certificate (TLS 1.2 only, as KDE
    /// Connect Android), reads the computer's identity and writes `second`.
    async fn device(
        stream: tokio::io::DuplexStream,
        cert_of: CompatIdentity,
        first: NetworkPacket,
        second: NetworkPacket,
    ) -> Option<NetworkPacket> {
        let mut stream = stream;
        stream.write_all(&first.to_wire().unwrap()).await.ok()?;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config =
            rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
                .with_client_cert_verifier(Arc::new(AnyClient))
                .with_single_cert(
                    vec![rustls::pki_types::CertificateDer::from(
                        cert_of.cert_der.clone(),
                    )],
                    rustls::pki_types::PrivateKeyDer::try_from(cert_of.key_der.clone()).unwrap(),
                )
                .unwrap();
        let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(config))
            .accept(stream)
            .await
            .ok()?;
        let line = read_line(&mut tls, MAX_LINE, STEP).await.ok()?;
        tls.write_all(&second.to_wire().unwrap()).await.ok()?;
        tls.flush().await.ok()?;
        NetworkPacket::from_wire(&line).ok()
    }

    fn hello_of(id: &str, protocol: u64) -> NetworkPacket {
        NetworkPacket::new("kdeconnect.identity").with_body(serde_json::json!({
            "deviceId": id,
            "deviceName": "Phone (A71)",
            "deviceType": "phone",
            "protocolVersion": protocol,
        }))
    }

    async fn run(
        phone: CompatIdentity,
        first: NetworkPacket,
        second: NetworkPacket,
        pinned: Option<[u8; 32]>,
    ) -> (Result<Hello, String>, Option<NetworkPacket>) {
        let computer = identity("computer");
        let ours = identity_packet(&computer.id, "Desk", 1716);
        let (a, b) = tokio::io::duplex(64 * 1024);
        let device = tokio::spawn(device(b, phone, first, second));
        let link = accept(a, &computer, &ours, |_| pinned).await;
        if let Ok(link) = &link {
            // The device speaks TLS 1.2 only, as KDE Connect Android: so did we.
            assert_eq!(
                link.stream.get_ref().1.protocol_version(),
                Some(rustls::ProtocolVersion::TLSv1_2)
            );
        }
        let heard = match &link {
            Ok(_) => device.await.unwrap(),
            Err(_) => {
                device.abort();
                None
            }
        };
        (link.map(|l| l.hello), heard)
    }

    #[tokio::test]
    async fn a_v8_device_completes_the_handshake_over_tls_1_2() {
        let phone = identity("phone");
        let id = phone.id.clone();
        let (hello, heard) = run(phone, hello_of(&id, 8), hello_of(&id, 8), None).await;
        let hello = hello.unwrap();
        assert_eq!(hello.device_id, id);
        assert_eq!(hello.device_name, "Phone A71", "KDE's name rules");
        let heard = heard.unwrap();
        assert_eq!(heard.packet_type, "kdeconnect.identity");
        assert_eq!(heard.body["deviceType"], "desktop");
        assert_eq!(heard.body["protocolVersion"], 8);
        assert_eq!(heard.body["tcpPort"], 1716);
    }

    #[tokio::test]
    async fn another_identity_inside_tls_is_refused() {
        // CVE-2025-66270: a first identity, then another inside TLS.
        let phone = identity("phone-cve");
        let id = phone.id.clone();
        let other = "a".repeat(32);
        let (hello, _) = run(phone, hello_of(&id, 8), hello_of(&other, 8), None).await;
        assert!(hello.unwrap_err().contains("not the one it gave"));
    }

    #[tokio::test]
    async fn the_same_id_with_another_version_inside_tls_is_refused() {
        let phone = identity("phone-version");
        let id = phone.id.clone();
        let (hello, _) = run(phone, hello_of(&id, 8), hello_of(&id, 9), None).await;
        assert!(hello.unwrap_err().contains("not the one it gave"));
    }

    #[tokio::test]
    async fn a_certificate_that_does_not_name_the_id_is_refused() {
        let phone = identity("phone-cn");
        let claimed = "b".repeat(32);
        let (hello, _) = run(phone, hello_of(&claimed, 8), hello_of(&claimed, 8), None).await;
        assert!(
            hello.unwrap_err().contains("does not name the device's id"),
            "refused for its certificate, not a timeout"
        );
    }

    #[tokio::test]
    async fn a_paired_device_must_bring_its_pinned_certificate() {
        let phone = identity("phone-pin");
        let id = phone.id.clone();
        let (hello, _) = run(phone, hello_of(&id, 8), hello_of(&id, 8), Some([7; 32])).await;
        assert!(
            hello
                .unwrap_err()
                .contains("not the certificate this device paired with")
        );
    }

    #[test]
    fn an_old_or_odd_identity_is_refused_before_tls() {
        let ours = "c".repeat(36);
        let id = "d".repeat(36);
        assert!(
            read_hello(&hello_of(&id, 7), &ours)
                .unwrap_err()
                .contains("older")
        );
        assert!(read_hello(&hello_of("short", 8), &ours).is_err());
        assert!(read_hello(&NetworkPacket::new("kdeconnect.ping"), &ours).is_err());
        let mut elsewhere = hello_of(&id, 8);
        elsewhere.body["targetDeviceId"] = serde_json::json!("e".repeat(36));
        assert!(
            read_hello(&elsewhere, &ours)
                .unwrap_err()
                .contains("another device")
        );
        elsewhere.body["targetDeviceId"] = serde_json::json!(ours.clone());
        assert!(read_hello(&elsewhere, &ours).is_ok());
        assert!(
            read_hello(&hello_of(&ours, 8), &ours)
                .unwrap_err()
                .contains("own id")
        );
        let mut nameless = hello_of(&id, 8);
        nameless.body["deviceName"] = serde_json::json!("();");
        assert_eq!(
            read_hello(&nameless, &ours).unwrap().device_name,
            "KDE Connect device"
        );
        let long = NetworkPacket::new("x\n".repeat(5000));
        assert!(
            read_hello(&long, &ours).unwrap_err().len() < 120,
            "kept short for the log"
        );
    }

    #[tokio::test]
    async fn a_line_too_long_or_too_slow_is_refused() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        b.write_all(&vec![b'x'; MAX_LINE + 1]).await.unwrap();
        assert!(
            read_line(&mut a, MAX_LINE, STEP)
                .await
                .unwrap_err()
                .contains("longer")
        );
        let (mut a, _b) = tokio::io::duplex(64);
        assert!(
            read_line(&mut a, MAX_LINE, Duration::from_millis(50))
                .await
                .unwrap_err()
                .contains("in time")
        );
    }

    /// The capabilities are KDE Connect's own packet types, as KDE Connect
    /// 1.36 for Android names them (read from its app): a type it does not
    /// know turns no plugin on, as Ring's first one did (3c review).
    #[test]
    fn the_capabilities_are_kde_connects_own_packet_types() {
        assert_eq!(
            INCOMING,
            [
                "kdeconnect.ping",
                "kdeconnect.battery",
                "kdeconnect.clipboard",
                "kdeconnect.share.request",
                "kdeconnect.notification"
            ]
        );
        assert_eq!(
            OUTGOING,
            [
                "kdeconnect.ping",
                "kdeconnect.clipboard",
                "kdeconnect.findmyphone.request"
            ]
        );
    }

    #[test]
    fn ids_and_names_follow_kdes_rules() {
        assert!(valid_id(&"a".repeat(32)) && valid_id(&"a_b-".repeat(9)));
        assert!(!valid_id(&"a".repeat(31)) && !valid_id(&"a".repeat(39)));
        assert!(!valid_id(&format!("{}!", "a".repeat(32))));
        assert_eq!(clean_name("My \"Phone\"; (2)"), "My Phone 2");
        assert_eq!(clean_name(&"n".repeat(40)).len(), 32);
    }
}
