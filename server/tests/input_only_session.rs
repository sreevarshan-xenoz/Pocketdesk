//! The phone as a remote (T10i): an input-only session over loopback QUIC.
//! It captures nothing, so it runs anywhere: the phone's input reaches the
//! server's input channel, the session says it was admitted, no picture or
//! sound arrives, and a picture-bound packet is dropped.

use std::net::SocketAddr;
use std::time::Duration;

use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    InputPacket, QuinnConnection, SharedConnection, StreamingClient, StreamingConfig,
    StreamingServer,
};

/// Send `packet` as the phone does: one uni stream each.
async fn send(conn: &SharedConnection, packet: &InputPacket) {
    let mut stream = conn.open_uni().await.expect("open input stream");
    stream
        .write_all(&packet.encode())
        .await
        .expect("write input");
    stream.finish().expect("finish input");
}

#[tokio::test]
async fn an_input_only_session_carries_input_and_no_picture() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = std::sync::Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert)
        .await
        .expect("bind streaming port");
    let addr = stream_server.local_addr().expect("bound address");

    let (input_tx, mut input_rx) = tokio::sync::broadcast::channel(16);
    let cert_for_session = cert.clone();
    let session = tokio::spawn(async move {
        let incoming = stream_server
            .accept_connection()
            .await
            .expect("no client connected");
        let connection = incoming
            .accept()
            .expect("rejected")
            .await
            .expect("handshake failed");
        let mut server = StreamingServer::new(
            StreamingConfig::default(),
            StreamTransportConfig::default(),
            cert_for_session,
        );
        server.set_input_channel(input_tx);
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });

    let (mut client, mut frames, mut audio) = StreamingClient::connect(
        &addr.to_string(),
        cert.clone(),
        None,
        Some("remote-test"),
        None,
        None,
        pocketdesk_core::streaming::SessionMode::InputOnly,
    )
    .await
    .expect("client connect");
    let remote = client.remote();
    let conn = client.connection().cloned().expect("connected");
    let receiving = tokio::spawn(async move { client.start().await });

    // Admitted: the ready stream comes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !remote.ready.load(std::sync::atomic::Ordering::Acquire) {
        assert!(tokio::time::Instant::now() < deadline, "never admitted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A tap at a place in a picture there is none of is dropped; a move is not.
    send(
        &conn,
        &InputPacket::MouseMoveAbs {
            x_norm: 100,
            y_norm: 100,
        },
    )
    .await;
    send(&conn, &InputPacket::MouseMove { dx: 5, dy: -3 }).await;
    let (_, packet) = tokio::time::timeout(Duration::from_secs(5), input_rx.recv())
        .await
        .expect("input arrives")
        .expect("input channel");
    assert!(
        matches!(packet, InputPacket::MouseMove { dx: 5, dy: -3 }),
        "the relative move, not the absolute one: {packet:?}"
    );
    // The two travel on streams of their own: the absolute tap could have
    // come second. It never comes.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), input_rx.recv())
            .await
            .is_err(),
        "a tap at a place in a picture is dropped"
    );

    // No picture and no sound.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), frames.recv())
            .await
            .is_err(),
        "no video on a remote"
    );
    assert!(audio.try_recv().is_err(), "no sound on a remote");
    assert!(!remote.too_old.load(std::sync::atomic::Ordering::Acquire));

    receiving.abort();
    session.abort();
}
