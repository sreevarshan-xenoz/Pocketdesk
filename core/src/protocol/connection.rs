use super::kdeconnect::DeviceIdentity;
use super::{HANDSHAKE_HELLO, HANDSHAKE_OK, PROTOCOL_VERSION};
use crate::error::{PocketdeskError, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{Instrument, debug, info_span};
use uuid;

/// A control connection: TLS over TCP (phase B), already past the handshake.
pub type ControlStream = tokio_rustls::client::TlsStream<TcpStream>;

/// Any stream a control connection can run on: TLS over TCP, or a stream of
/// an iroh connection (the internet road, T10a).
pub trait ControlIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> ControlIo for S {}

/// A control connection on either road, past the handshake.
pub type ControlDuplex = Box<dyn ControlIo>;

/// The address a control connection reached.
pub fn control_peer_addr(stream: &ControlStream) -> std::io::Result<std::net::SocketAddr> {
    stream.get_ref().0.peer_addr()
}

/// Probe an idle control connection: after 30 s of silence, then every
/// 10 s. A peer that vanished without closing (it slept, or left the
/// network) is found within about a minute instead of never, so the reader
/// ends and the phone dials again.
pub fn keep_alive(stream: &TcpStream) -> std::io::Result<()> {
    let probing = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10));
    #[cfg(any(target_os = "linux", target_os = "android", windows))]
    let probing = probing.with_retries(3);
    socket2::SockRef::from(stream).set_tcp_keepalive(&probing)
}

#[derive(Debug, Clone)]
pub struct ConnectionManager {
    timeout: Duration,
    /// This device's certificate and its pins of desktops
    /// (`CertManager::control_client_config`).
    tls: Arc<rustls::ClientConfig>,
}

impl ConnectionManager {
    pub fn new(timeout: Duration, tls: Arc<rustls::ClientConfig>) -> Self {
        Self { timeout, tls }
    }

    pub async fn connect(
        &self,
        address: &str,
        port: u16,
        identity: &DeviceIdentity,
    ) -> Result<ControlStream> {
        self.connect_named(address, port, identity)
            .await
            .map(|(stream, _)| stream)
    }

    /// [`ConnectionManager::connect`], with the name the desktop gives
    /// itself in its identity, cleaned (U3).
    pub async fn connect_named(
        &self,
        address: &str,
        port: u16,
        identity: &DeviceIdentity,
    ) -> Result<(ControlStream, Option<String>)> {
        let conn_id = uuid::Uuid::new_v4().to_string();
        let span = info_span!(
            "handshake",
            conn_id = %conn_id,
            peer = %address,
            proto = %PROTOCOL_VERSION
        );

        async move {
            let socket_addr = format!("{address}:{port}");

            let stream = tokio::time::timeout(self.timeout, TcpStream::connect(&socket_addr))
                .await
                .map_err(|_| PocketdeskError::Timeout {
                    operation: "connect",
                    duration_ms: self.timeout.as_millis() as u64,
                })?
                .map_err(|e| PocketdeskError::ConnectionFailed {
                    address: address.to_string(),
                    port,
                    reason: e.to_string(),
                })?;

            stream.set_nodelay(true)?;
            keep_alive(&stream)?;

            // The channel runs inside TLS. The desktop's certificate is
            // checked against the pin for the address dialed (the server
            // name), the same pin the stream uses, and this device presents
            // its own.
            let server_name = rustls::pki_types::ServerName::try_from(address.to_string())
                .map_err(|e| PocketdeskError::ConnectionFailed {
                    address: address.to_string(),
                    port,
                    reason: format!("not an address: {e}"),
                })?;
            let stream = tokio::time::timeout(
                self.timeout,
                tokio_rustls::TlsConnector::from(self.tls.clone()).connect(server_name, stream),
            )
            .await
            .map_err(|_| PocketdeskError::Timeout {
                operation: "TLS handshake",
                duration_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(|e| PocketdeskError::ConnectionFailed {
                address: address.to_string(),
                port,
                reason: format!("TLS: {e}"),
            })?;

            control_handshake_named(stream, address, identity).await
        }
        .instrument(span)
        .await
    }
}

/// The control channel's opening, on a stream already secured (TLS over
/// TCP, or an iroh stream): our HELLO, the desktop's OK and identity, then
/// ours. `peer` names the desktop in errors.
pub async fn control_handshake<S>(stream: S, peer: &str, identity: &DeviceIdentity) -> Result<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    control_handshake_named(stream, peer, identity)
        .await
        .map(|(stream, _)| stream)
}

/// The name a desktop gives itself in its identity line, cleaned (U3):
/// its own claim, for a person to tell computers apart.
fn identity_name(line: &str) -> Option<String> {
    let packet = super::kdeconnect::NetworkPacket::from_wire(line.trim()).ok()?;
    if packet.packet_type != "kdeconnect.identity" {
        return None;
    }
    packet
        .body
        .get("deviceName")
        .and_then(|v| v.as_str())
        .and_then(super::kdeconnect::clean_device_name)
}

/// [`control_handshake`], with the desktop's name from its identity (U3).
pub async fn control_handshake_named<S>(
    stream: S,
    peer: &str,
    identity: &DeviceIdentity,
) -> Result<(S, Option<String>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let address = peer;
    let mut stream = stream;
    // Step 1: Perform Handshake Hello
    stream
        .write_all(format!("{}\n", HANDSHAKE_HELLO).as_bytes())
        .await
        .map_err(|e| PocketdeskError::Io {
            operation: "write handshake hello",
            detail: e.to_string(),
        })?;

    // Step 2: Read handshake response with timeout
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut response))
        .await
        .map_err(|_| PocketdeskError::Timeout {
            operation: "handshake response",
            duration_ms: 5000,
        })?
        .map_err(|e| PocketdeskError::HandshakeFailed {
            peer: address.to_string(),
            response: e.to_string(),
        })?;

    if response.trim() != HANDSHAKE_OK {
        return Err(PocketdeskError::HandshakeFailed {
            peer: address.to_string(),
            response: response.trim().to_string(),
        });
    }

    // Step 3: Read server's identity packet (optional), keeping its name.
    let mut identity_response = String::new();
    match tokio::time::timeout(
        Duration::from_secs(3),
        reader.read_line(&mut identity_response),
    )
    .await
    {
        Ok(Ok(_)) if !identity_response.trim().is_empty() => {
            debug!("Received server identity: {}", identity_response.trim());
        }
        _ => {
            debug!("No server identity packet received or timeout");
        }
    }

    // Step 4: Send client identity packet
    let identity_packet = identity.as_identity_packet();
    let identity_bytes = identity_packet
        .to_wire()
        .map_err(|e| PocketdeskError::Serialization {
            format: "JSON",
            detail: e.to_string(),
        })?;

    let mut stream = reader.into_inner();
    stream
        .write_all(&identity_bytes)
        .await
        .map_err(|e| PocketdeskError::Io {
            operation: "send identity packet",
            detail: e.to_string(),
        })?;
    stream.flush().await.map_err(|e| PocketdeskError::Io {
        operation: "flush identity packet",
        detail: e.to_string(),
    })?;

    Ok((stream, identity_name(&identity_response)))
}

/// The control channel's opening over the internet road (T10a). The desktop
/// names itself only once it has admitted the phone: our HELLO, its OK, our
/// identity, then its identity, which is the admission. A refusal
/// ([`super::HANDSHAKE_REFUSED`]) or a closed stream fails the handshake, so
/// a phone the desktop will not take never counts as connected. Lines are
/// read a byte at a time: the desktop's pushes follow its identity at once,
/// and a buffered reader would swallow them.
pub async fn internet_control_handshake<S>(
    stream: S,
    peer: &str,
    identity: &DeviceIdentity,
) -> Result<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    internet_control_handshake_named(stream, peer, identity)
        .await
        .map(|(stream, _)| stream)
}

/// [`internet_control_handshake`], with the desktop's name from the
/// identity that admits the phone (U3).
pub async fn internet_control_handshake_named<S>(
    mut stream: S,
    peer: &str,
    identity: &DeviceIdentity,
) -> Result<(S, Option<String>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let failed = |response: String| PocketdeskError::HandshakeFailed {
        peer: peer.to_string(),
        response,
    };
    stream
        .write_all(format!("{HANDSHAKE_HELLO}\n").as_bytes())
        .await
        .map_err(|e| PocketdeskError::Io {
            operation: "write handshake hello",
            detail: e.to_string(),
        })?;
    let ok = read_line_unbuffered(&mut stream, Duration::from_secs(8))
        .await
        .map_err(failed)?;
    if ok.trim() != HANDSHAKE_OK {
        return Err(failed(ok.trim().to_string()));
    }
    let identity_bytes =
        identity
            .as_identity_packet()
            .to_wire()
            .map_err(|e| PocketdeskError::Serialization {
                format: "JSON",
                detail: e.to_string(),
            })?;
    stream
        .write_all(&identity_bytes)
        .await
        .map_err(|e| PocketdeskError::Io {
            operation: "send identity packet",
            detail: e.to_string(),
        })?;
    stream.flush().await.map_err(|e| PocketdeskError::Io {
        operation: "flush identity packet",
        detail: e.to_string(),
    })?;
    let admitted = read_line_unbuffered(&mut stream, Duration::from_secs(10))
        .await
        .map_err(failed)?;
    match super::kdeconnect::NetworkPacket::from_wire(admitted.trim()) {
        Ok(packet) if packet.packet_type == "kdeconnect.identity" => {
            debug!("Admitted over the internet");
            Ok((stream, identity_name(&admitted)))
        }
        _ => Err(failed(admitted.trim().to_string())),
    }
}

/// One line, read a byte at a time within `deadline`, at most 64 KiB.
async fn read_line_unbuffered<S>(
    stream: &mut S,
    deadline: Duration,
) -> std::result::Result<String, String>
where
    S: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    const MAX: usize = 64 * 1024;
    tokio::time::timeout(deadline, async {
        let mut line = Vec::new();
        loop {
            match stream.read_u8().await {
                Ok(b'\n') => break,
                Ok(byte) if line.len() < MAX => line.push(byte),
                Ok(_) => return Err("a handshake line too long".to_string()),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err("the desktop closed the connection".to_string());
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        String::from_utf8(line).map_err(|_| "a handshake line that is not text".to_string())
    })
    .await
    .map_err(|_| "no answer in time".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn the_handshake_runs_on_any_stream() {
        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        let identity = DeviceIdentity::new("duplex-phone", "Duplex Phone");
        let desktop = tokio::spawn(async move {
            let mut desktop = BufReader::new(desktop);
            let mut hello = String::new();
            desktop.read_line(&mut hello).await.unwrap();
            assert_eq!(hello.trim(), HANDSHAKE_HELLO);
            let inner = desktop.get_mut();
            inner
                .write_all(
                    format!("{HANDSHAKE_OK}\n{{\"type\":\"kdeconnect.identity\"}}\n").as_bytes(),
                )
                .await
                .unwrap();
            let mut identity = String::new();
            desktop.read_line(&mut identity).await.unwrap();
            identity
        });
        let _stream = control_handshake(phone, "duplex", &identity).await.unwrap();
        let sent = desktop.await.unwrap();
        assert!(sent.contains("duplex-phone"), "our identity: {sent}");
    }

    /// The desktop's side of the internet road's opening, answering
    /// `after_identity` once the phone has named itself.
    fn internet_desktop(
        desktop: tokio::io::DuplexStream,
        after_identity: &'static str,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut desktop = BufReader::new(desktop);
            let mut hello = String::new();
            desktop.read_line(&mut hello).await.unwrap();
            assert_eq!(hello.trim(), HANDSHAKE_HELLO);
            desktop
                .get_mut()
                .write_all(format!("{HANDSHAKE_OK}\n").as_bytes())
                .await
                .unwrap();
            let mut phone = String::new();
            desktop.read_line(&mut phone).await.unwrap();
            assert!(phone.contains("road-phone"));
            desktop
                .get_mut()
                .write_all(after_identity.as_bytes())
                .await
                .unwrap();
            // Held open: only what was said decides.
            tokio::time::sleep(Duration::from_secs(1)).await;
        })
    }

    #[tokio::test]
    async fn the_internet_road_counts_only_an_admitted_phone_as_connected() {
        let identity = DeviceIdentity::new("road-phone", "Road Phone");
        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        internet_desktop(
            desktop,
            "{\"type\":\"kdeconnect.identity\"}\n{\"type\":\"kdeconnect.pocketdesk.endpoint\"}\n",
        );
        let mut stream = internet_control_handshake(phone, "desktop", &identity)
            .await
            .expect("admitted");
        // The push right behind the identity is still there to read.
        let mut next = String::new();
        BufReader::new(&mut stream)
            .read_line(&mut next)
            .await
            .unwrap();
        assert!(next.contains("pocketdesk.endpoint"), "{next}");

        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        internet_desktop(desktop, "POCKETDESK_ERR 1 refused\n");
        assert!(matches!(
            internet_control_handshake(phone, "desktop", &identity).await,
            Err(PocketdeskError::HandshakeFailed { .. })
        ));

        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        let closing = internet_desktop(desktop, "");
        closing.abort();
        assert!(
            internet_control_handshake(phone, "desktop", &identity)
                .await
                .is_err()
        );
    }

    /// U3: the computer's name comes with its identity, cleaned, on both
    /// roads; an identity without one is no name.
    #[tokio::test]
    async fn the_handshake_returns_the_computers_name() {
        let identity = DeviceIdentity::new("duplex-phone", "Duplex Phone");
        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            let mut desktop = BufReader::new(desktop);
            let mut hello = String::new();
            desktop.read_line(&mut hello).await.unwrap();
            desktop
                .get_mut()
                .write_all(
                    format!(
                        "{HANDSHAKE_OK}\n{{\"type\":\"kdeconnect.identity\",\"body\":{{\"deviceName\":\"Desk\u{202E}top\"}}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut ours = String::new();
            desktop.read_line(&mut ours).await.unwrap();
        });
        let (_stream, name) = control_handshake_named(phone, "duplex", &identity)
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("Desktop"));

        let identity = DeviceIdentity::new("road-phone", "Road Phone");
        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        internet_desktop(
            desktop,
            "{\"type\":\"kdeconnect.identity\",\"body\":{\"deviceName\":\"Study PC\"}}\n",
        );
        let (_stream, name) = internet_control_handshake_named(phone, "desktop", &identity)
            .await
            .expect("admitted");
        assert_eq!(name.as_deref(), Some("Study PC"));

        let (phone, desktop) = tokio::io::duplex(64 * 1024);
        internet_desktop(desktop, "{\"type\":\"kdeconnect.identity\"}\n");
        let (_stream, name) = internet_control_handshake_named(phone, "desktop", &identity)
            .await
            .expect("admitted");
        assert_eq!(name, None);
    }

    #[tokio::test]
    async fn a_refusal_in_place_of_ok_fails_the_handshake() {
        let (phone, mut desktop) = tokio::io::duplex(4096);
        let identity = DeviceIdentity::new("duplex-phone", "Duplex Phone");
        tokio::spawn(async move {
            let mut hello = [0u8; 64];
            let _ = desktop.read(&mut hello).await;
            desktop.write_all(b"POCKETDESK_ERR 1\n").await.unwrap();
        });
        assert!(matches!(
            control_handshake(phone, "duplex", &identity).await,
            Err(PocketdeskError::HandshakeFailed { .. })
        ));
    }
}
