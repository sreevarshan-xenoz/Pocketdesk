//! The phone's sound with its screen (K5): a cast session over loopback
//! QUIC whose sound streams reach the player with the phone's "Play its
//! sound here" on, and are dropped with it off; the cast is told which.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
use pocketdesk_core::streaming::cast::{self, CastEvent};
use pocketdesk_core::streaming::client::REMOTE_FEATURE_PHONE_SOUND;
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, SessionMode, StreamingClient, StreamingConfig, StreamingServer,
};
use tokio::sync::mpsc;

/// A cast's sound, as the server hands it to the player, with the phone's
/// permission `allowed`: what the phone heard it may send, and the packets
/// that reached the player.
async fn cast_sound(allowed: bool) -> (u8, Vec<Vec<u8>>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = std::sync::Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert).await.expect("bind");
    let addr = stream_server.local_addr().expect("bound address");
    let (cast_tx, mut cast_events) = mpsc::channel::<CastEvent>(64);
    let (sound_tx, mut sound_rx) = mpsc::channel::<Vec<u8>>(64);
    let cert_for_session = cert.clone();
    let session = tokio::spawn(async move {
        let connection = stream_server
            .accept_connection()
            .await
            .expect("a client")
            .accept()
            .expect("accepted")
            .await
            .expect("handshake");
        let mut server = StreamingServer::new(
            StreamingConfig::default(),
            StreamTransportConfig::default(),
            cert_for_session,
        );
        let (input_tx, _input_rx) = tokio::sync::broadcast::channel(16);
        server.set_input_channel(input_tx);
        server.set_cast_channel(cast_tx);
        server.set_phone_sound_channel(sound_tx);
        server.set_session_gate(move |_claim| {
            Box::pin(async move {
                let mut permissions = DevicePermissions::default();
                permissions.set(Capability::Cast, true);
                permissions.set(Capability::PhoneSound, allowed);
                Some(permissions)
            })
        });
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });
    // The relay's events are not this test's; drain them.
    tokio::spawn(async move { while cast_events.recv().await.is_some() {} });

    let (mut client, _frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert,
        None,
        Some("sound-test"),
        None,
        None,
        SessionMode::Cast,
    )
    .await
    .expect("connect");
    let remote = client.remote();
    let conn = client.connection().cloned().expect("connected");
    let receiving = tokio::spawn(async move { client.start().await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !remote.ready.load(Ordering::Acquire) {
        assert!(tokio::time::Instant::now() < deadline, "never admitted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let features = remote.screen_features.load(Ordering::Acquire);

    for n in 0..5u8 {
        let mut stream = conn.open_uni().await.expect("open stream");
        stream
            .write_all(&cast::audio_bytes(&[0xFC, n, n, n]))
            .await
            .expect("write");
        stream.finish().expect("finish");
    }
    let mut got = Vec::new();
    let until = tokio::time::Instant::now() + Duration::from_secs(if allowed { 5 } else { 1 });
    while got.len() < 5 {
        match tokio::time::timeout_at(until, sound_rx.recv()).await {
            Ok(Some(packet)) => got.push(packet),
            _ => break,
        }
    }
    conn.close(0, b"done");
    receiving.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), session).await;
    got.sort();
    (features, got)
}

#[tokio::test]
async fn a_cast_s_sound_reaches_the_player_with_its_permission() {
    let (features, got) = cast_sound(true).await;
    assert_eq!(
        features & REMOTE_FEATURE_PHONE_SOUND,
        REMOTE_FEATURE_PHONE_SOUND
    );
    let want: Vec<Vec<u8>> = (0..5u8).map(|n| vec![0xFC, n, n, n]).collect();
    assert_eq!(got, want);
}

#[tokio::test]
async fn a_cast_s_sound_is_dropped_without_its_permission() {
    let (features, got) = cast_sound(false).await;
    assert_eq!(
        features & REMOTE_FEATURE_PHONE_SOUND,
        0,
        "told it is not taken"
    );
    assert!(got.is_empty(), "{} packets played", got.len());
}
