//! The phone as the computer's speaker and mic (N5): a sound session over
//! loopback QUIC. It captures no picture, so it runs anywhere: the session
//! says it was admitted, its mic frames reach the mic relay, its input is
//! never injected, no picture arrives, and a phone that may neither hear the
//! computer nor be its mic is refused with the reason.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    InputPacket, QuinnConnection, SOUND_GRANT_AUDIO, SOUND_GRANT_MIC, SOUND_GRANT_SAID,
    SessionMode, SharedConnection, StreamingClient, StreamingConfig, StreamingServer,
};
use tokio::sync::{broadcast, mpsc};

/// Send `packet` as the phone does: one uni stream each.
async fn send(conn: &SharedConnection, packet: &InputPacket) {
    let mut stream = conn.open_uni().await.expect("open input stream");
    stream
        .write_all(&packet.encode())
        .await
        .expect("write input");
    stream.finish().expect("finish input");
}

type Session = tokio::task::JoinHandle<anyhow::Result<()>>;

/// A server for one session whose phone has `audio` and `mic`.
async fn serve(
    audio: bool,
    mic: bool,
) -> (
    SocketAddr,
    Arc<CertManager>,
    broadcast::Receiver<(u64, InputPacket)>,
    mpsc::Receiver<InputPacket>,
    Session,
) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert)
        .await
        .expect("bind streaming port");
    let addr = stream_server.local_addr().expect("bound address");

    let (input_tx, input_rx) = broadcast::channel(16);
    let (mic_tx, mic_rx) = mpsc::channel(16);
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
        server.set_mic_channel(mic_tx);
        server.set_session_gate(move |_claim| {
            Box::pin(async move {
                let mut permissions = DevicePermissions::default();
                permissions.set(Capability::Audio, audio);
                permissions.set(Capability::Mic, mic);
                Some(permissions)
            })
        });
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });
    (addr, cert, input_rx, mic_rx, session)
}

#[tokio::test]
async fn a_sound_session_takes_the_mic_and_injects_nothing() {
    let (addr, cert, mut input_rx, mut mic_rx, session) = serve(true, true).await;
    let (mut client, mut frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert.clone(),
        None,
        Some("sound-test"),
        None,
        None,
        SessionMode::Sound,
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

    // It hears that it may do both.
    assert_eq!(
        remote
            .sound_granted
            .load(std::sync::atomic::Ordering::Acquire),
        SOUND_GRANT_SAID | SOUND_GRANT_AUDIO | SOUND_GRANT_MIC
    );

    // The mic reaches the relay.
    let mic = InputPacket::Mic {
        enabled: true,
        opus: vec![0xF8, 0xFF, 0xFE],
    };
    send(&conn, &mic).await;
    let relayed = tokio::time::timeout(Duration::from_secs(5), mic_rx.recv())
        .await
        .expect("the mic frame arrives")
        .expect("mic channel");
    assert!(
        matches!(&relayed, InputPacket::Mic { enabled: true, opus } if opus == &[0xF8, 0xFF, 0xFE]),
        "the phone's frame: {relayed:?}"
    );

    // A speaker drives nothing: input is not injected.
    send(&conn, &InputPacket::MouseMove { dx: 5, dy: -3 }).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), input_rx.recv())
            .await
            .is_err(),
        "a sound session's input is dropped"
    );

    // No picture.
    assert!(
        tokio::time::timeout(Duration::from_millis(500), frames.recv())
            .await
            .is_err(),
        "no video in a sound session"
    );
    assert!(!remote.too_old.load(std::sync::atomic::Ordering::Acquire));

    receiving.abort();
    session.abort();
}

#[tokio::test]
async fn a_sound_session_with_neither_audio_nor_mic_is_refused_with_the_reason() {
    let (addr, cert, _input_rx, mut mic_rx, session) = serve(false, false).await;
    let (client, _frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert,
        None,
        Some("sound-test"),
        None,
        None,
        SessionMode::Sound,
    )
    .await
    .expect("client connect");
    let conn = client.connection().cloned().expect("connected");

    let result = tokio::time::timeout(Duration::from_secs(10), session)
        .await
        .expect("the session ends")
        .expect("the session task");
    assert!(result.is_err(), "refused");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let reason = loop {
        if let Some(reason) = conn.close_reason() {
            break reason;
        }
        assert!(tokio::time::Instant::now() < deadline, "never closed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(reason.contains("sound not allowed"), "the reason: {reason}");
    assert!(mic_rx.try_recv().is_err(), "the relay heard nothing");
}

/// The phone hears which of the two it may use (N5 review): with Audio only,
/// its mic switch is not offered.
#[tokio::test]
async fn a_sound_session_hears_what_it_may_do() {
    let (addr, cert, _input_rx, _mic_rx, session) = serve(true, false).await;
    let (mut client, _frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert,
        None,
        Some("sound-test"),
        None,
        None,
        SessionMode::Sound,
    )
    .await
    .expect("client connect");
    let remote = client.remote();
    let receiving = tokio::spawn(async move { client.start().await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !remote.ready.load(std::sync::atomic::Ordering::Acquire) {
        assert!(tokio::time::Instant::now() < deadline, "never admitted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        remote
            .sound_granted
            .load(std::sync::atomic::Ordering::Acquire),
        SOUND_GRANT_SAID | SOUND_GRANT_AUDIO,
        "sound, no mic"
    );
    receiving.abort();
    session.abort();
}
