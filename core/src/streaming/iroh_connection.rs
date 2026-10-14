//! iroh (noq) implementation of the streaming connection traits (R1 stage 2).
//!
//! iroh 1.x speaks `noq`, whose connection/stream types are unrelated to
//! quinn 0.11 (see `spike/iroh/README.md`), but the operations noq exposes
//! mirror quinn's closely enough that the [`Connection`] abstraction maps
//! one-to-one: same `open_uni`/`accept_uni` shapes, same `write_all` +
//! sync `finish`, and the same `ConnectionError` taxonomy — so close
//! classification is a structural copy of the quinn arm.
//!
//! Behind the `wan` feature: the Android bridge and the default build must
//! not pull iroh into the graph. The v2 multiplexer path additionally needs
//! `open_bi` and datagrams, which the trait does not model yet; only v1
//! streaming can ride this connection today.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use super::dht::DhtAddressLookup;
use anyhow::{Context, Result};
use async_trait::async_trait;
use iroh::endpoint::presets::Minimal;
use iroh::endpoint::{
    ClosedStream, ConnectionError as NoqError, PathId, ReadError, ReadExactError, ReadToEndError,
    RecvStream, SendStream, WriteError,
};
use iroh::{Endpoint, EndpointAddr, RelayMode};

use super::connection::{
    Connection, ConnectionError, ConnectionStats, InStream, OutStream, PeerKey, SharedConnection,
    TransportFamily,
};
use super::transport::StreamTransportConfig;

/// Classify an iroh/noq connection error — identical taxonomy to quinn.
fn classify_conn_error(e: NoqError) -> ConnectionError {
    use NoqError::*;
    match e {
        ApplicationClosed(_) | ConnectionClosed(_) | Reset | TimedOut | LocallyClosed => {
            ConnectionError::Closed
        }
        other => ConnectionError::Other(other.to_string()),
    }
}

/// [`Connection`] over an iroh endpoint connection — the WAN arm next to
/// [`super::QuinnConnection`]'s LAN default.
#[derive(Debug, Clone)]
pub struct IrohConnection {
    inner: iroh::endpoint::Connection,
}

impl IrohConnection {
    pub fn new(connection: iroh::endpoint::Connection) -> Self {
        Self { inner: connection }
    }

    /// Share into the boxed trait object the pipeline expects.
    pub fn shared(connection: iroh::endpoint::Connection) -> SharedConnection {
        Arc::new(Self::new(connection))
    }

    /// Escape hatch for iroh-native plumbing (endpoint close, datagrams,
    /// connection-level close with an error code).
    pub fn inner(&self) -> &iroh::endpoint::Connection {
        &self.inner
    }
}

impl From<iroh::endpoint::Connection> for IrohConnection {
    fn from(connection: iroh::endpoint::Connection) -> Self {
        Self::new(connection)
    }
}

#[async_trait]
impl Connection for IrohConnection {
    async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
        self.inner
            .open_uni()
            .await
            .map(|s| Box::new(IrohSendStream { inner: s }) as Box<dyn OutStream>)
            .map_err(classify_conn_error)
    }

    async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
        self.inner
            .accept_uni()
            .await
            .map(|s| Box::new(IrohRecvStream { inner: s }) as Box<dyn InStream>)
            .map_err(classify_conn_error)
    }

    fn peer_key(&self) -> Option<PeerKey> {
        Some(PeerKey::Iroh(self.inner.remote_id().to_string()))
    }

    fn remote_address(&self) -> SocketAddr {
        // Established iroh connections expose their network paths, not a
        // single socket address. The selected direct path (if any) is the
        // quinn-equivalent "peer"; relay/custom paths have no IP to report
        // (the pipeline only logs this — a proper transport-address type
        // lands with the WAN dialing wiring).
        let unspecified = SocketAddr::from(([0, 0, 0, 0], 0));
        let paths = self.inner.paths();
        paths
            .iter()
            .find(|p| p.is_selected() && p.is_ip())
            .or_else(|| paths.iter().find(|p| p.is_ip()))
            .map(|p| match p.remote_addr() {
                iroh::TransportAddr::Ip(addr) => *addr,
                _ => unspecified,
            })
            .unwrap_or(unspecified)
    }

    fn stats(&self) -> ConnectionStats {
        // noq keeps RTT, congestion state and MTU *per path*. The
        // connection-level `stats()` is an aggregate over every path that ever
        // existed and deliberately drops those three (summing a congestion
        // window across a relay and a direct path means nothing), so the only
        // way to read them is the path that is actually carrying the session —
        // which is also the honest source for the RTT: `PathId::ZERO` is the
        // initial path, and once DCUtR has punched a direct one the traffic
        // rides a different id, so reading ZERO would report the latency of a
        // path the video no longer uses.
        //
        // `relayed` mirrors iroh's path selection: DCUtR keeps trying to
        // punch a direct path while traffic rides the relay, so this flips
        // false on its own once punching succeeds — callers surface it as
        // "relayed — trying direct…", never as a dead end.
        let paths = self.inner.paths();
        let mut relayed = false;
        // Stays `None` — absent, not zero — when the snapshot has no selected
        // path (mid-handshake, or a path that closed between snapshots); a zero
        // here would read as an uncongested link.
        let mut path_stats = None;
        if let Some(path) = paths.iter().find(|path| path.is_selected()) {
            relayed = path.is_relay();
            path_stats = Some(path.stats());
        }
        let s = self.inner.stats();
        ConnectionStats {
            rtt: path_stats
                .as_ref()
                .map(|path| path.rtt)
                .or_else(|| self.inner.rtt(PathId::ZERO))
                .unwrap_or(Duration::ZERO),
            lost_packets: s.lost_packets,
            lost_bytes: s.lost_bytes,
            datagrams_sent: s.udp_tx.datagrams,
            datagrams_received: s.udp_rx.datagrams,
            bytes_sent: s.udp_tx.bytes,
            bytes_received: s.udp_rx.bytes,
            congestion_events: path_stats.as_ref().map(|p| p.congestion_events),
            cwnd_bytes: path_stats.as_ref().map(|p| p.cwnd),
            path_mtu: path_stats.as_ref().map(|p| p.current_mtu),
            black_holes_detected: path_stats.as_ref().map(|p| p.black_holes_detected),
            relayed,
        }
    }

    fn transport_family(&self) -> TransportFamily {
        TransportFamily::Iroh
    }

    fn close(&self, error_code: u32, reason: &[u8]) {
        self.inner
            .close(iroh::endpoint::VarInt::from_u32(error_code), reason);
    }

    fn close_reason(&self) -> Option<String> {
        Some(match self.inner.close_reason()? {
            iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
                String::from_utf8_lossy(&close.reason).into_owned()
            }
            other => other.to_string(),
        })
    }
}

/// Client-side WAN dial (R1 stage 3).
///
/// Owns the ephemeral iroh `Endpoint` for the life of the session: iroh
/// aborts its socket task with an error if an `Endpoint` is dropped while
/// live, so callers must `close()` the dial (or at least keep the endpoint
/// alive until the connection ends). The identity the phone learned from the
/// server's `kdeconnect.pocketdesk.endpoint` announcement is reassembled here
/// from its string parts — relays are dial-reachable fallbacks, direct
/// addresses the LAN/Tailscale-routable set.
pub struct IrohDial {
    endpoint: Endpoint,
    raw: iroh::endpoint::Connection,
    connection: SharedConnection,
}

/// The relays the dialing end uses for itself. A desktop that announces only
/// relays of its own (N2) is dialed with only those, so n0's relays see
/// neither end; a desktop on n0's relays keeps n0's defaults.
fn dial_relay_mode(announced: &[iroh::RelayUrl], use_relays: bool) -> RelayMode {
    if !use_relays {
        return RelayMode::Disabled;
    }
    let n0 = RelayMode::Default.relay_map();
    if announced.is_empty() || announced.iter().any(|url| n0.contains(url)) {
        RelayMode::Default
    } else {
        RelayMode::Custom(announced.iter().cloned().collect())
    }
}

impl IrohDial {
    /// Dial a server from its announced identity parts.
    ///
    /// `use_relays` selects relay assistance (hole punching): the desktop's
    /// own relays when it announces only those, n0's otherwise. `false`
    /// restricts the dial to the direct addresses — loopback tests and
    /// air-gapped direct paths.
    ///
    /// `dht` looks the desktop up by id alongside the announced addresses
    /// (N3), for when those went stale; iroh merges what it finds.
    pub async fn dial(
        endpoint_id: &str,
        relay_urls: &[String],
        direct_addrs: &[String],
        use_relays: bool,
        timeout: Duration,
        device_key: Option<iroh::SecretKey>,
        dht: Option<DhtAddressLookup>,
    ) -> Result<Self> {
        let id: iroh::EndpointId = endpoint_id.parse().context("invalid iroh EndpointId")?;
        let relay_urls = relay_urls
            .iter()
            .map(|url| {
                url.parse::<iroh::RelayUrl>()
                    .with_context(|| format!("invalid relay url: {url}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut addr = EndpointAddr::new(id);
        for url in &relay_urls {
            addr = addr.with_relay_url(url.clone());
        }
        for a in direct_addrs {
            let a: SocketAddr = a
                .parse()
                .with_context(|| format!("invalid direct address: {a}"))?;
            addr = addr.with_ip_addr(a);
        }

        let alpn = StreamTransportConfig::default().alpn;
        let mut builder = Endpoint::builder(Minimal);
        // Device keys: dial as this device's own key, so the desktop can bind
        // the session to it; without one, iroh makes a throwaway key.
        if let Some(key) = device_key {
            builder = builder.secret_key(key);
        }
        if let Some(dht) = dht {
            builder = builder.address_lookup(dht);
        }
        let endpoint = builder
            .alpns(vec![alpn.clone()])
            .relay_mode(dial_relay_mode(&relay_urls, use_relays))
            .bind_addr(SocketAddr::from(([0, 0, 0, 0], 0)))
            .context("iroh bind_addr rejected wildcard socket addr")?
            .bind()
            .await
            .context("iroh client endpoint bind failed")?;

        let raw = tokio::time::timeout(timeout, endpoint.connect(addr, &alpn))
            .await
            .context("iroh dial timed out")?
            .context("iroh dial failed")?;
        // Connected: the DHT has done its job. Left in place, its node would
        // keep up a routing table for the whole session.
        if let Ok(lookup) = endpoint.address_lookup() {
            lookup.clear();
        }

        Ok(Self {
            endpoint,
            connection: IrohConnection::shared(raw.clone()),
            raw,
        })
    }

    /// The trait handle `StreamingClient::attach` adopts.
    pub fn connection(&self) -> SharedConnection {
        self.connection.clone()
    }

    /// Gracefully close the connection and the endpoint. Mandatory before
    /// drop to avoid iroh's dropped-live-endpoint error log.
    pub async fn close(self) {
        self.raw
            .close(iroh::endpoint::VarInt::from_u32(0), b"session end");
        self.endpoint.close().await;
    }
}

struct IrohSendStream {
    inner: SendStream,
}

#[async_trait]
impl OutStream for IrohSendStream {
    async fn write_all(&mut self, data: &[u8]) -> Result<(), ConnectionError> {
        self.inner.write_all(data).await.map_err(|e| match e {
            WriteError::ConnectionLost(e) => classify_conn_error(e),
            WriteError::ClosedStream => ConnectionError::Closed,
            other => ConnectionError::Other(other.to_string()),
        })
    }

    fn finish(&mut self) -> Result<(), ConnectionError> {
        // Same contract as quinn 0.11: ClosedStream means the stream (and
        // effectively the session for our one-packet-per-stream use) is done.
        self.inner
            .finish()
            .map_err(|_: ClosedStream| ConnectionError::Closed)
    }
}

struct IrohRecvStream {
    inner: RecvStream,
}

#[async_trait]
impl InStream for IrohRecvStream {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ConnectionError> {
        self.inner.read_exact(buf).await.map_err(|e| match e {
            ReadExactError::ReadError(ReadError::ConnectionLost(e)) => classify_conn_error(e),
            other => ConnectionError::Other(other.to_string()),
        })
    }

    async fn read_to_end(&mut self, limit: usize) -> Result<Vec<u8>, ConnectionError> {
        self.inner.read_to_end(limit).await.map_err(|e| match e {
            ReadToEndError::Read(ReadError::ConnectionLost(e)) => classify_conn_error(e),
            ReadToEndError::Read(other) => ConnectionError::Other(other.to_string()),
            ReadToEndError::TooLong => ConnectionError::Other("stream exceeds limit".into()),
        })
    }
}

/// This device's WAN key, loaded from `path` or created there (device keys).
/// A damaged file is replaced with a fresh key: the desktop only accepts a
/// WAN key the device named on a LAN session, and the next LAN session names
/// this one, where keeping the damaged file would end WAN for good. A new
/// key is written to a temp file and renamed into place, so a crash cannot
/// leave a half-written key behind.
pub fn load_or_create_device_key(path: &std::path::Path) -> Result<iroh::SecretKey> {
    if let Ok(bytes) = std::fs::read(path) {
        if let Ok(raw) = <[u8; 32]>::try_from(bytes.as_slice()) {
            return Ok(iroh::SecretKey::from_bytes(&raw));
        }
        tracing::warn!("{} is damaged; making a new WAN key", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let key = iroh::SecretKey::generate();
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    let tmp = std::path::PathBuf::from(tmp);
    crate::secure_fs::write_private(&tmp, &key.to_bytes())
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("placing {}", path.display()))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::transport::StreamTransportConfig;
    use iroh::endpoint::VarInt;

    /// Device keys: the phone dials the WAN with one key, kept across runs,
    /// so the desktop can bind its WAN sessions to it.
    #[test]
    fn a_device_keeps_its_wan_key_across_runs() {
        let dir = std::env::temp_dir().join(format!("ll-irohkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("iroh_device.key");
        let first = load_or_create_device_key(&path).unwrap();
        let again = load_or_create_device_key(&path).unwrap();
        assert_eq!(first.public().to_string(), again.public().to_string());
        // A damaged file is replaced: the next LAN session registers the new
        // key with the desktop, where an error here would end WAN for good.
        std::fs::write(&path, b"short").unwrap();
        let fresh = load_or_create_device_key(&path).unwrap();
        assert_ne!(fresh.public().to_string(), first.public().to_string());
        assert_eq!(std::fs::read(&path).unwrap().len(), 32);
    }

    #[test]
    fn a_phone_uses_the_desktops_own_relay_and_not_n0s() {
        let n0: Vec<iroh::RelayUrl> = RelayMode::Default.relay_map().urls();
        assert!(matches!(dial_relay_mode(&[], true), RelayMode::Default));
        assert!(
            matches!(dial_relay_mode(&n0[..1], true), RelayMode::Default),
            "a desktop on n0's relays"
        );
        let mine: iroh::RelayUrl = "http://192.0.2.10:3340".parse().unwrap();
        let RelayMode::Custom(map) = dial_relay_mode(std::slice::from_ref(&mine), true) else {
            panic!("a desktop on its own relay: only that relay");
        };
        assert!(map.contains(&mine));
        assert!(!n0.iter().any(|url| map.contains(url)), "nothing of n0's");
        assert!(matches!(
            dial_relay_mode(std::slice::from_ref(&mine), false),
            RelayMode::Disabled
        ));
    }

    /// Loopback iroh endpoint with relays/DNS disabled — the spike's
    /// zero-infrastructure setup, dialing the direct `EndpointAddr` only.
    async fn bind_loopback_endpoint(alpn: &[u8]) -> (iroh::endpoint::Endpoint, SocketAddr) {
        use iroh::RelayMode;
        use iroh::endpoint::Endpoint;
        use iroh::endpoint::presets::Minimal;
        let addr: SocketAddr = ([127, 0, 0, 1], 0).into();
        // The spike dials an explicitly-bound port; reserve one the same way.
        let port = {
            let sock = std::net::UdpSocket::bind(addr).unwrap();
            sock.local_addr().unwrap().port()
        };
        let addr = SocketAddr::new(addr.ip(), port);
        let endpoint = Endpoint::builder(Minimal)
            .alpns(vec![alpn.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .bind_addr(addr)
            .expect("bind_addr")
            .bind()
            .await
            .expect("endpoint bind");
        (endpoint, addr)
    }

    /// Same exercise as `connection::tests::quinn_connection_roundtrip`,
    /// over iroh's noq connection: uni round-trip through the traits and
    /// graceful close surfacing as `ConnectionError::Closed`.
    #[tokio::test]
    async fn iroh_connection_roundtrip() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let alpn = StreamTransportConfig::default().alpn;
        let (server, server_addr) = bind_loopback_endpoint(&alpn).await;
        let (client, _) = bind_loopback_endpoint(&alpn).await;

        let accept = {
            let server = server.clone();
            tokio::spawn(async move {
                let incoming = server.accept().await.expect("incoming");
                let conn = incoming.accept().expect("reject").await.expect("handshake");
                IrohConnection::shared(conn)
            })
        };

        let raw_client = client
            .connect(server.addr(), &alpn)
            .await
            .expect("dial loopback");
        let client_q = IrohConnection::new(raw_client);
        let client_conn: SharedConnection = Arc::new(client_q.clone());
        let server_conn: SharedConnection = accept.await.expect("accept task");

        assert_eq!(
            client_q.remote_address(),
            server_addr,
            "direct loopback connection reports the dialed socket address"
        );
        assert!(
            !client_q.stats().relayed && !server_conn.stats().relayed,
            "relay-less loopback traffic is direct"
        );
        let stats = client_q.stats();
        assert!(
            stats.bytes_sent > 0 && stats.datagrams_sent > 0,
            "the aggregate counters do fill in: {stats:?}"
        );
        // The connection-level aggregate throws the per-path fields away, but a
        // live connection always has a selected path to read them from, and this
        // is the on-host proof that a WAN session is not structurally blind to
        // congestion state or MTU. An earlier revision asserted the opposite —
        // it had only looked at `Connection::stats()`, which really does drop
        // them — and encoded that as a permanent gap in every WAN record.
        assert!(
            stats.cwnd_bytes.is_some()
                && stats.path_mtu.is_some()
                && stats.congestion_events.is_some()
                && stats.black_holes_detected.is_some(),
            "the selected path reports its congestion window, MTU and loss state: {stats:?}"
        );
        assert!(
            stats.path_mtu.unwrap_or(0) >= 1200,
            "a path MTU below the QUIC floor means it was not measured: {stats:?}"
        );
        assert!(
            stats.rtt > Duration::ZERO,
            "the RTT is the selected path's, not a zero because path 0 was queried: {stats:?}"
        );

        let payload = tokio::join!(
            async {
                let mut reader = server_conn.accept_uni().await.expect("accept_uni");
                let mut head = [0u8; 6];
                reader.read_exact(&mut head).await.expect("read_exact");
                let mut body = reader.read_to_end(64).await.expect("read_to_end");
                let mut all = head.to_vec();
                all.append(&mut body);
                all
            },
            async {
                let mut writer = client_conn.open_uni().await.expect("open_uni");
                writer.write_all(b"hello ").await.expect("write 1");
                writer.write_all(b"stream").await.expect("write 2");
                writer.finish().expect("finish");
            },
        )
        .0;
        assert_eq!(payload, b"hello stream");

        // Same exercise over the client dial path: reassemble the announced
        // identity strings into an `EndpointAddr`, dial with relays off, and
        // round-trip a stream pair through the traits. (The server handle is
        // held open — dropping the last iroh Connection handle closes the
        // connection, unlike quinn.)
        {
            let accept2 = {
                let server = server.clone();
                tokio::spawn(async move {
                    let incoming = server.accept().await.expect("incoming 2");
                    let conn = incoming
                        .accept()
                        .expect("reject 2")
                        .await
                        .expect("handshake 2");
                    IrohConnection::shared(conn)
                })
            };
            let dial = IrohDial::dial(
                &server.id().to_string(),
                &[],
                &[server_addr.to_string()],
                false,
                Duration::from_secs(10),
                None,
                None,
            )
            .await
            .expect("loopback dial over direct addr");
            let dialed = dial.connection();
            let server2 = accept2.await.expect("accept task 2");

            let echoed = tokio::join!(
                async {
                    let mut r = server2.accept_uni().await.expect("server accept_uni");
                    let data = r.read_to_end(64 * 1024).await.expect("server read");
                    let mut s = server2.open_uni().await.expect("server open_uni");
                    s.write_all(&data).await.expect("server echo");
                    s.finish().expect("server finish");
                },
                async {
                    let mut w = dialed.open_uni().await.expect("client open_uni");
                    w.write_all(b"over the wan").await.expect("client write");
                    w.finish().expect("client finish");
                    let mut r = dialed.accept_uni().await.expect("client accept_uni");
                    r.read_to_end(1024).await.expect("client read")
                },
            )
            .1;
            assert_eq!(echoed, b"over the wan");

            drop(server2);
            dial.close().await;
        }

        // Close surfaces as ConnectionError::Closed on the accept loop —
        // the sentinel every streaming receive task breaks on.
        client_q.inner().close(VarInt::from_u32(0), b"bye");
        match tokio::time::timeout(Duration::from_secs(5), server_conn.accept_uni()).await {
            Ok(Err(ConnectionError::Closed)) => {}
            Ok(Ok(_)) => panic!("unexpected stream after close"),
            Ok(Err(other)) => panic!("expected Closed from accept_uni, got {other}"),
            Err(_) => panic!("accept_uni hung after close"),
        }

        // iroh lesson from the spike: dropping a live endpoint aborts its
        // socket task with an ERROR log — close both explicitly.
        client.close().await;
        server.close().await;
    }

    /// N3 on a local Mainline DHT: the desktop publishes its record, and a
    /// phone that knows only the id (no relay, no address) finds it there and
    /// opens a session. Relays are off on both ends, so the DHT is the only
    /// way the phone can learn where the desktop is.
    #[tokio::test]
    async fn a_phone_with_only_the_id_finds_the_desktop_on_the_dht() {
        use futures_lite::StreamExt as _;
        use iroh::address_lookup::{AddrFilter, AddressLookup as _};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let testnet = n0_mainline::Testnet::new(5).await.expect("testnet");
        let on_testnet = || {
            DhtAddressLookup::builder()
                .dht_builder(super::super::dht::dht_builder(&testnet.bootstrap))
        };

        let alpn = StreamTransportConfig::default().alpn;
        let desktop = Endpoint::builder(Minimal)
            .alpns(vec![alpn.clone()])
            .relay_mode(RelayMode::Disabled)
            // Loopback is never published by the server; the test's filter
            // lets it through, since loopback is all a test has.
            .address_lookup(on_testnet().addr_filter(AddrFilter::unfiltered()))
            .bind_addr(SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("bind_addr")
            .bind()
            .await
            .expect("desktop bind");
        let id = desktop.id();

        // Publishing is fire-and-forget: wait until a reader sees the record.
        let reader = on_testnet().no_publish().build().expect("reader");
        let published = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let items: Vec<_> = reader.resolve(id).expect("stream").collect().await;
                if items
                    .into_iter()
                    .flatten()
                    .any(|item| !item.into_endpoint_addr().addrs.is_empty())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await;
        assert!(published.is_ok(), "the desktop's record reached the DHT");
        assert!(
            IrohDial::dial(
                &id.to_string(),
                &[],
                &[],
                false,
                Duration::from_secs(3),
                None,
                None
            )
            .await
            .is_err(),
            "the control: without the DHT, the id alone leads nowhere"
        );

        let accept = {
            let desktop = desktop.clone();
            tokio::spawn(async move {
                let incoming = desktop.accept().await.expect("incoming");
                incoming.accept().expect("reject").await.expect("handshake")
            })
        };
        let dial = IrohDial::dial(
            &id.to_string(),
            &[],
            &[],
            false,
            Duration::from_secs(20),
            None,
            Some(on_testnet().no_publish().build().expect("phone lookup")),
        )
        .await
        .expect("the phone finds the desktop by id alone");
        let accepted = accept.await.expect("accept task");
        assert_eq!(
            accepted.remote_id(),
            dial.endpoint.id(),
            "the session is the phone's"
        );
        assert!(
            dial.endpoint.address_lookup().unwrap().is_empty(),
            "the phone's DHT node stops once connected"
        );
        drop(accepted);
        dial.close().await;
        desktop.close().await;
    }
}
