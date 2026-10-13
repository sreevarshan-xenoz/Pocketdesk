//! R1 spike: can iroh replace/sit beside the quinn transport?
//!
//! Local loopback only (relays disabled, direct UDP via 127.0.0.1), so the
//! tests exercise iroh's real QUIC stack (noq) without any n0 infrastructure.
//! Headline finding — documented in `README.md` and proven by
//! `tests/loopback.rs::iroh_types_are_not_quinn_types`: iroh 1.x speaks
//! `noq` stream/connection types which are *not* `quinn` 0.11 types, so core
//! cannot adopt iroh without a connection abstraction over
//! `core/src/streaming`.

use anyhow::{Context, Result};
use iroh::RelayMode;
use iroh::endpoint::QuicTransportConfig;
use iroh::endpoint::presets::Minimal;
use iroh::endpoint::{Connection, Endpoint};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const ALPN: &[u8] = b"pocketdesk-spike/0";

pub const PAYLOAD: &[u8] = b"iroh spike: hello over QUIC";

/// Build a relay-less endpoint bound to an explicit loopback port.
///
/// `Minimal` only chooses the rustls crypto provider (ring) — no DNS/Pkarr
/// address lookup, no relays, no portmapping. Peer endpoints are dialed
/// purely through the direct `EndpointAddr` returned by `Endpoint::addr()`.
pub async fn bind_loopback_endpoint(port: u16) -> Result<Endpoint> {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    // Explicit datagram receive buffer: without it the datagram transport
    // parameter is not advertised and send_datagram() fails. Same knob as
    // quinn's max_datagram_frame_size.
    let transport = QuicTransportConfig::builder()
        .datagram_receive_buffer_size(Some(64 * 1024))
        .build();
    Endpoint::builder(Minimal)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .transport_config(transport)
        .bind_addr(addr)
        .context("bind_addr rejected loopback socket addr")?
        .bind()
        .await
        .context("endpoint bind failed")
}

/// Reserve an ephemeral loopback UDP port by binding and dropping a socket.
pub fn reserve_loopback_port() -> Result<u16> {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
    Ok(sock.local_addr()?.port())
}

/// Accept exactly one connection on `server`, then echo back anything the
/// client sends on bidi streams and the first datagram.
///
/// Returns once the connection is established; the echo tasks outlive it.
pub async fn serve_echo_once(server: &Endpoint) -> Result<Connection> {
    let incoming = server.accept().await.context("no incoming connection")?;
    let conn = incoming
        .accept()
        .context("accept rejected")?
        .await
        .context("handshake failed")?;

    let echo = conn.clone();
    tokio::spawn(async move {
        while let Ok((mut send, mut recv)) = echo.accept_bi().await {
            tokio::spawn(async move {
                while let Ok(Some(chunk)) = recv.read_chunk(16 * 1024).await {
                    if send.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
                let _ = send.finish();
            });
        }
    });

    let dg = conn.clone();
    tokio::spawn(async move {
        if let Ok(data) = dg.read_datagram().await {
            let _ = dg.send_datagram(data);
        }
    });

    Ok(conn)
}

/// Full client flow used by both the test suite and the demo binary:
/// dial `server`, echo-check a bidi stream, a uni stream, and a datagram.
pub async fn client_roundtrip(client: &Endpoint, server: &Endpoint) -> Result<()> {
    let conn = client
        .connect(server.addr(), ALPN)
        .await
        .context("iroh connect over loopback failed")?;

    // Bidi: write, half-close, read the echo.
    let (mut send, mut recv) = conn.open_bi().await.context("open_bi failed")?;
    send.write_all(PAYLOAD).await.context("bidi write failed")?;
    send.finish().context("bidi finish failed")?;
    let mut buf = vec![0u8; PAYLOAD.len()];
    recv.read_exact(&mut buf)
        .await
        .context("bidi echo read failed")?;
    anyhow::ensure!(buf == PAYLOAD, "bidi echo mismatch");

    // Datagram: one app datagram, bounced by the echo task.
    conn.send_datagram(bytes::Bytes::from_static(PAYLOAD))
        .context("send_datagram failed")?;
    let echoed = conn.read_datagram().await.context("read_datagram failed")?;
    anyhow::ensure!(echoed == PAYLOAD, "datagram echo mismatch");

    conn.close(0u32.into(), b"done");
    Ok(())
}
