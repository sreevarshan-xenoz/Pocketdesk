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

/// The address a control connection reached.
pub fn control_peer_addr(stream: &ControlStream) -> std::io::Result<std::net::SocketAddr> {
    stream.get_ref().0.peer_addr()
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
            let mut stream = tokio::time::timeout(
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

            // Step 3: Read server's identity packet (optional)
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
            let identity_bytes =
                identity_packet
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

            Ok(stream)
        }
        .instrument(span)
        .await
    }
}
