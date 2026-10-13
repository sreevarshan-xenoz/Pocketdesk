//! Transport-agnostic connection abstraction for the streaming pipeline.
//!
//! The pipeline (server `streamer.rs`, client `client.rs`, bridge input
//! sends) only ever needs a handful of QUIC operations: open/accept
//! unidirectional streams, write/read whole packets, peer address, and RTT
//! stats. The R1 iroh spike showed iroh 1.x runs on `noq`, whose connection
//! and stream types are incompatible with quinn 0.11 — so the pipeline is
//! written against these traits instead of `quinn::Connection`, with
//! [`QuinnConnection`] as today's implementation and an iroh implementation
//! plugging in for WAN dialing.
//!
//! The packet wire format on top of the streams (`EncodedPacket` headers,
//! `InputPacket`) is defined elsewhere and is transport-agnostic already.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

/// Result alias for trait-method errors: `ConnectionError` is small and
/// `Display`, so `?`/`map_err` at call sites keep working via anyhow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    /// The connection is gone (application/transport close, timeout, reset).
    /// Accept loops treat this as the graceful end of the session.
    Closed,
    /// Anything else (stream misuse, internal errors).
    Other(String),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Closed => write!(f, "connection closed"),
            ConnectionError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// The connection-level facts the pipeline reads (RTT feedback loop, adaptive
/// bitrate) and telemetry records. Deliberately narrow: extend only when a
/// caller actually needs another field.
///
/// The `Option` fields are the honest boundary of what a caller can be told,
/// which is the whole point of this struct: both stacks keep congestion state
/// per path, quinn's `stats()` flattens the current path's numbers into what it
/// returns, and iroh's connection-level `stats()` sums the byte and packet
/// counters while dropping the per-path ones outright — so on iroh they are read
/// off the *selected* path instead (`Connection::paths()`), which is also the
/// only way to get the RTT of the path the video is really riding rather than the
/// initial one. `None` therefore means "no selected path in this snapshot", and
/// never "measured zero" — a zero would read as an uncongested link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionStats {
    pub rtt: Duration,
    /// Packets the transport has declared lost since the handshake.
    pub lost_packets: u64,
    pub lost_bytes: u64,
    /// UDP datagrams and bytes the transport moved, both directions. This is
    /// goodput's noisy neighbour: it counts headers, probes and retransmits,
    /// which is what a link is actually carrying.
    pub datagrams_sent: u64,
    pub datagrams_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    /// Times the congestion controller reacted to loss on the current path.
    pub congestion_events: Option<u64>,
    /// Congestion-window ceiling in bytes: how much the transport will have
    /// outstanding before it starts holding back.
    pub cwnd_bytes: Option<u64>,
    /// Largest UDP payload the current path is known to carry, as found by the
    /// library's own PMTU discovery.
    pub path_mtu: Option<u16>,
    /// Times the path stopped accepting anything at all, as detected by the
    /// PMTU probes.
    pub black_holes_detected: Option<u64>,
    /// True when the selected transport path rides a relay (iroh WAN), i.e.
    /// hole punching has not produced a direct path yet. Always false for
    /// quinn, which has no relay concept.
    pub relayed: bool,
}

/// Write half of a stream handed out by [`Connection::open_uni`].
#[async_trait]
pub trait OutStream: Send {
    async fn write_all(&mut self, data: &[u8]) -> Result<(), ConnectionError>;
    /// Mark the end of the stream. Synchronous in quinn; kept so callers
    /// finish streams deterministically (uni-streams signal end-of-packet).
    fn finish(&mut self) -> Result<(), ConnectionError>;
}

/// Read half of a stream returned by [`Connection::accept_uni`].
#[async_trait]
pub trait InStream: Send {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ConnectionError>;
    async fn read_to_end(&mut self, limit: usize) -> Result<Vec<u8>, ConnectionError>;
}

/// Which transport stack a connection rides. The pipeline is deliberately
/// transport-agnostic, but *telemetry* must classify sessions by it: quinn
/// connections are point-to-point by construction (LAN/Tailscale — no relay
/// concept), while iroh connections may ride a relay until hole punching
/// opens a direct path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFamily {
    Quinn,
    Iroh,
}

/// The key a connection's peer proved it holds during the handshake
/// (device keys, phase A): what binds a device to the connection, where the
/// id it announces is only a claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PeerKey {
    /// SHA-256 of the peer's TLS certificate (QUIC on the LAN).
    Tls([u8; 32]),
    /// The peer's iroh endpoint id (WAN).
    Iroh(String),
}

impl PeerKey {
    /// The key of a TLS peer whose certificate chain is `certs` (leaf first).
    pub fn from_tls_certs(certs: &[rustls::pki_types::CertificateDer<'_>]) -> Option<Self> {
        use sha2::{Digest, Sha256};
        let leaf = certs.first()?;
        Some(PeerKey::Tls(Sha256::digest(leaf.as_ref()).into()))
    }
}

impl std::fmt::Display for PeerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerKey::Tls(hash) => {
                f.write_str("tls:")?;
                hash.iter().try_for_each(|b| write!(f, "{b:02x}"))
            }
            PeerKey::Iroh(id) => write!(f, "iroh:{id}"),
        }
    }
}

/// A live remote connection usable by the streaming pipeline.
#[async_trait]
pub trait Connection: Send + Sync {
    async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError>;
    async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError>;
    fn remote_address(&self) -> SocketAddr;
    fn stats(&self) -> ConnectionStats;
    fn transport_family(&self) -> TransportFamily;
    /// Immediately tear down the connection with an application error code
    /// and reason (e.g. rejecting an unpaired session), instead of letting
    /// the peer sit on a live-but-dead connection until the idle timeout.
    fn close(&self, error_code: u32, reason: &[u8]);
    /// Why the connection ended, once it has: the peer's own reason when it
    /// closed deliberately (as the server does when it refuses a session),
    /// otherwise the transport's error. `None` while it is open.
    fn close_reason(&self) -> Option<String> {
        None
    }
    /// The key the peer proved in the handshake, if it presented one.
    fn peer_key(&self) -> Option<PeerKey> {
        None
    }
}

/// Shared handle passed around pipeline tasks (quinn::Connection itself is
/// cheap, but the trait object needs the Arc).
pub type SharedConnection = Arc<dyn Connection>;

// ---------------------------------------------------------------------------
// quinn implementation
// ---------------------------------------------------------------------------

/// Classify a quinn connection error: everything that ends the session
/// collapses to [`ConnectionError::Closed`].
fn classify_conn_error(e: quinn::ConnectionError) -> ConnectionError {
    use quinn::ConnectionError::*;
    match e {
        ApplicationClosed(_) | ConnectionClosed(_) | Reset | TimedOut | LocallyClosed => {
            ConnectionError::Closed
        }
        other => ConnectionError::Other(other.to_string()),
    }
}

/// [`Connection`] over a quinn 0.11 connection — the LAN / current default.
#[derive(Debug, Clone)]
pub struct QuinnConnection {
    inner: quinn::Connection,
}

impl QuinnConnection {
    pub fn new(connection: quinn::Connection) -> Self {
        Self { inner: connection }
    }

    /// Share into the boxed trait object the pipeline expects.
    pub fn shared(connection: quinn::Connection) -> SharedConnection {
        Arc::new(Self::new(connection))
    }

    /// Escape hatch for quinn-native code paths still outside the
    /// abstraction (the v2 multiplexer, cert pinning).
    pub fn inner(&self) -> &quinn::Connection {
        &self.inner
    }
}

impl From<quinn::Connection> for QuinnConnection {
    fn from(connection: quinn::Connection) -> Self {
        Self::new(connection)
    }
}

#[async_trait]
impl Connection for QuinnConnection {
    async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
        self.inner
            .open_uni()
            .await
            .map(|s| Box::new(QuinnSendStream { inner: s }) as Box<dyn OutStream>)
            .map_err(classify_conn_error)
    }

    async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
        self.inner
            .accept_uni()
            .await
            .map(|s| Box::new(QuinnRecvStream { inner: s }) as Box<dyn InStream>)
            .map_err(classify_conn_error)
    }

    fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    fn peer_key(&self) -> Option<PeerKey> {
        let certs = self
            .inner
            .peer_identity()?
            .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
            .ok()?;
        PeerKey::from_tls_certs(&certs)
    }

    fn stats(&self) -> ConnectionStats {
        let s = self.inner.stats();
        ConnectionStats {
            rtt: s.path.rtt,
            lost_packets: s.path.lost_packets,
            lost_bytes: s.path.lost_bytes,
            datagrams_sent: s.udp_tx.datagrams,
            datagrams_received: s.udp_rx.datagrams,
            bytes_sent: s.udp_tx.bytes,
            bytes_received: s.udp_rx.bytes,
            congestion_events: Some(s.path.congestion_events),
            cwnd_bytes: Some(s.path.cwnd),
            path_mtu: Some(s.path.current_mtu),
            black_holes_detected: Some(s.path.black_holes_detected),
            relayed: false,
        }
    }

    fn transport_family(&self) -> TransportFamily {
        TransportFamily::Quinn
    }

    fn close(&self, error_code: u32, reason: &[u8]) {
        self.inner.close(error_code.into(), reason);
    }

    fn close_reason(&self) -> Option<String> {
        Some(match self.inner.close_reason()? {
            quinn::ConnectionError::ApplicationClosed(close) => {
                String::from_utf8_lossy(&close.reason).into_owned()
            }
            other => other.to_string(),
        })
    }
}

struct QuinnSendStream {
    inner: quinn::SendStream,
}

#[async_trait]
impl OutStream for QuinnSendStream {
    async fn write_all(&mut self, data: &[u8]) -> Result<(), ConnectionError> {
        self.inner.write_all(data).await.map_err(|e| match e {
            quinn::WriteError::ConnectionLost(e) => classify_conn_error(e),
            other => ConnectionError::Other(other.to_string()),
        })
    }

    fn finish(&mut self) -> Result<(), ConnectionError> {
        // quinn 0.11: `finish()` errors only if the stream was already
        // finished/reset — since callers finish right after a successful
        // write, that means the connection is gone.
        self.inner.finish().map_err(|_| ConnectionError::Closed)
    }
}

struct QuinnRecvStream {
    inner: quinn::RecvStream,
}

#[async_trait]
impl InStream for QuinnRecvStream {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ConnectionError> {
        self.inner.read_exact(buf).await.map_err(|e| match e {
            quinn::ReadExactError::ReadError(quinn::ReadError::ConnectionLost(e)) => {
                classify_conn_error(e)
            }
            other => ConnectionError::Other(other.to_string()),
        })
    }

    async fn read_to_end(&mut self, limit: usize) -> Result<Vec<u8>, ConnectionError> {
        self.inner.read_to_end(limit).await.map_err(|e| match e {
            quinn::ReadToEndError::Read(quinn::ReadError::ConnectionLost(e)) => {
                classify_conn_error(e)
            }
            other => ConnectionError::Other(other.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::transport::{CertManager, StreamTransportConfig};

    /// Device keys (phase A): each end learns the key the other proved in
    /// the handshake; the desktop, the phone's.
    #[tokio::test]
    async fn each_end_knows_the_key_the_other_proved() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let desktop = CertManager::new().expect("desktop certs");
        let phone = CertManager::new().expect("phone certs");
        let alpn = vec![StreamTransportConfig::default().alpn];
        let server_endpoint = quinn::Endpoint::server(
            desktop.server_config(alpn.clone()).expect("server config"),
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("bind");
        let addr = server_endpoint.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let conn = server_endpoint
                .accept()
                .await
                .expect("incoming")
                .await
                .expect("accept");
            QuinnConnection::shared(conn)
        });
        let mut client_endpoint =
            quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client endpoint");
        client_endpoint
            .set_default_client_config(phone.client_config(alpn).expect("client config"));
        let client = QuinnConnection::shared(
            client_endpoint
                .connect(addr, "pocketdesk.local")
                .expect("dial")
                .await
                .expect("connect"),
        );
        let server = accept.await.expect("accept task");

        assert_eq!(
            server.peer_key(),
            Some(PeerKey::Tls(phone.cert_fingerprint()))
        );
        assert_eq!(
            client.peer_key(),
            Some(PeerKey::Tls(desktop.cert_fingerprint()))
        );
        assert_eq!(
            PeerKey::Tls([0xab; 32]).to_string(),
            format!("tls:{}", "ab".repeat(32))
        );
    }

    /// Full trait exercise over a real quinn loopback connection: open/accept
    /// uni, split writes across read_exact + read_to_end, finish ends the
    /// stream, and a graceful close surfaces as `ConnectionError::Closed`.
    #[tokio::test]
    async fn quinn_connection_roundtrip() {
        // reqwest (aws-lc-rs) and quinn (ring) link two rustls providers;
        // pick ring explicitly, same as the integration tests do.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let certs = Arc::new(CertManager::new().expect("certs"));
        let alpn = vec![StreamTransportConfig::default().alpn];

        let server_endpoint = quinn::Endpoint::server(
            certs.server_config(alpn.clone()).expect("server config"),
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("bind");
        let addr = server_endpoint.local_addr().unwrap();

        let accept = {
            let endpoint = server_endpoint.clone();
            tokio::spawn(async move {
                let incoming = endpoint.accept().await.expect("incoming");
                let conn = incoming.await.expect("accept");
                QuinnConnection::shared(conn)
            })
        };

        let mut client_endpoint =
            quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client endpoint");
        client_endpoint
            .set_default_client_config(certs.client_config(alpn).expect("client config"));
        let raw_client = client_endpoint
            .connect(addr, "pocketdesk.local")
            .expect("dial")
            .await
            .expect("connect");
        // Dropping the client endpoint handle later leaves the established
        // connection running (the Connection keeps the driver alive).

        let client_q = QuinnConnection::new(raw_client);
        let client: SharedConnection = Arc::new(client_q.clone());
        let server: SharedConnection = accept.await.expect("accept task");

        assert_eq!(client.remote_address(), addr);
        assert_eq!(
            server.remote_address().ip().to_string(),
            "127.0.0.1",
            "server should see the client's loopback address"
        );
        assert!(client.stats().rtt < Duration::from_secs(5));
        assert!(
            !client.stats().relayed && !server.stats().relayed,
            "quinn has no relay concept — traffic is always direct"
        );
        let stats = client.stats();
        assert!(
            stats.bytes_sent > 0 && stats.datagrams_sent > 0 && stats.bytes_received > 0,
            "the handshake alone moves datagrams: {stats:?}"
        );
        assert!(
            stats.cwnd_bytes.unwrap_or(0) > 0,
            "a connection with no congestion window is not sending: {stats:?}"
        );
        assert!(
            stats.path_mtu.unwrap_or(0) >= 500,
            "quinn reports a path MTU even before probing pays off: {stats:?}"
        );
        assert!(
            stats.congestion_events.is_some(),
            "the counter exists on a quinn path, even at zero on a loaded box: {stats:?}"
        );

        let payload = tokio::join!(
            async {
                let mut reader = server.accept_uni().await.expect("accept_uni");
                let mut head = [0u8; 6];
                reader.read_exact(&mut head).await.expect("read_exact");
                let mut body = reader.read_to_end(64).await.expect("read_to_end");
                let mut all = head.to_vec();
                all.append(&mut body);
                all
            },
            async {
                let mut writer = client.open_uni().await.expect("open_uni");
                writer.write_all(b"hello ").await.expect("write 1");
                writer.write_all(b"stream").await.expect("write 2");
                writer.finish().expect("finish");
            },
        )
        .0;
        assert_eq!(payload, b"hello stream");

        assert_eq!(client.close_reason(), None, "open");

        // After the connection is closed, accept_uni reports Closed rather
        // than hanging — this is what the receive loops break on.
        client_q.inner().close(quinn::VarInt::from_u32(0), b"bye");
        match tokio::time::timeout(Duration::from_secs(5), server.accept_uni()).await {
            Ok(Err(ConnectionError::Closed)) => {}
            Ok(Ok(_)) => panic!("unexpected stream after close"),
            Ok(Err(other)) => panic!("expected Closed from accept_uni, got {other}"),
            Err(_) => panic!("accept_uni hung after close"),
        }
        // The side that was closed learns the closer's own words.
        assert_eq!(server.close_reason().as_deref(), Some("bye"));
    }
}
