//! The phone's screen on the computer (T10k): a cast session over loopback
//! QUIC. It captures nothing on the computer, so it runs anywhere: the
//! phone's config and frames reach the cast channel, the computer's
//! keyframe ask reaches the phone, the phone's input is not injected, and
//! the session's end is told. Without the Cast permission, or with nowhere
//! to show it, the cast is refused with the reason.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
use pocketdesk_core::streaming::cast::{self, CastEvent, CastPayload};
use pocketdesk_core::streaming::cast_control::{ControlBatch, ControlEvent, Global};
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    InputPacket, QuinnConnection, SessionMode, SharedConnection, StreamingClient, StreamingConfig,
    StreamingServer,
};
use tokio::sync::mpsc;

/// Send `bytes` as the phone does: one uni stream each.
async fn send(conn: &SharedConnection, bytes: &[u8]) {
    let mut stream = conn.open_uni().await.expect("open stream");
    stream.write_all(bytes).await.expect("write");
    stream.finish().expect("finish");
}

/// The next event, within a few seconds.
async fn next(events: &mut mpsc::Receiver<CastEvent>) -> CastEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("an event in time")
        .expect("the cast channel")
}

/// A server for one session; `cast` is where its cast goes, `allow_cast`
/// what the phone's permission says.
async fn serve(
    cast: Option<mpsc::Sender<CastEvent>>,
    allow_cast: bool,
) -> (
    SocketAddr,
    std::sync::Arc<CertManager>,
    tokio::sync::broadcast::Receiver<pocketdesk_core::streaming::SessionInput>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
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
    let (input_tx, input_rx) = tokio::sync::broadcast::channel(16);
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
        if let Some(cast) = cast {
            server.set_cast_channel(cast);
        }
        server.set_session_gate(move |_claim| {
            Box::pin(async move {
                let mut permissions = DevicePermissions::default();
                permissions.set(Capability::Cast, allow_cast);
                Some(permissions)
            })
        });
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });
    (addr, cert, input_rx, session)
}

async fn connect_cast(addr: SocketAddr, cert: std::sync::Arc<CertManager>) -> StreamingClient {
    let (client, _frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert,
        None,
        Some("cast-test"),
        None,
        None,
        SessionMode::Cast,
    )
    .await
    .expect("client connect");
    client
}

/// The next control batch the phone took, waiting up to 5 s without
/// blocking the runtime the client receives on.
async fn take_control(remote: &pocketdesk_core::streaming::client::RemoteFlags) -> Option<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(batch) = remote.control.take(Duration::ZERO) {
            return Some(batch);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_cast_reaches_the_relay_and_its_keyframe_asks_reach_the_phone() {
    let (cast_tx, mut events) = mpsc::channel(64);
    let (addr, cert, mut input_rx, session) = serve(Some(cast_tx), true).await;
    let mut client = connect_cast(addr, cert).await;
    let remote = client.remote();
    let conn = client.connection().cloned().expect("connected");
    let receiving = tokio::spawn(async move { client.start().await });

    let CastEvent::Start {
        session_id,
        device_id,
        want_keyframe,
        control,
    } = next(&mut events).await
    else {
        panic!("the cast starts first");
    };
    assert_eq!(device_id.as_deref(), Some("cast-test"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !remote.ready.load(Ordering::Acquire) {
        assert!(tokio::time::Instant::now() < deadline, "never admitted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The phone's config, then a keyframe: in that order, as streams that
    // finish in order (the second is sent once the first is read).
    send(&conn, &cast::config_bytes(864, 1920)).await;
    let CastEvent::Payload { payload, .. } = next(&mut events).await else {
        panic!("the config");
    };
    assert_eq!(
        payload,
        CastPayload::Config {
            width: 864,
            height: 1920
        }
    );
    // A keyframe bigger than any input packet still arrives whole.
    let au: Vec<u8> = [0, 0, 0, 1, 0x65]
        .into_iter()
        .chain(std::iter::repeat_n(7u8, 300 * 1024))
        .collect();
    send(&conn, &cast::frame_bytes(true, 0, &au)).await;
    let CastEvent::Payload {
        session_id: frame_session,
        payload,
    } = next(&mut events).await
    else {
        panic!("the frame");
    };
    assert_eq!(frame_session, session_id);
    assert_eq!(
        payload,
        CastPayload::Frame {
            keyframe: true,
            seq: 0,
            data: au
        }
    );

    // The computer asks for a keyframe; the phone hears it.
    want_keyframe.send(()).await.expect("the ask channel");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !remote.keyframe_wanted.load(Ordering::Acquire) {
        assert!(tokio::time::Instant::now() < deadline, "the ask never came");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Control (C8): nothing reaches a phone that has not said it takes it.
    let batch = ControlBatch {
        sent_ms: 1,
        events: vec![ControlEvent::Global(Global::Home)],
    }
    .encode()
    .unwrap();
    control
        .send(batch.clone())
        .await
        .expect("the control channel");
    assert_eq!(
        {
            tokio::time::sleep(Duration::from_millis(500)).await;
            remote.control.take(Duration::ZERO)
        },
        None,
        "the phone had not said it takes control"
    );
    // It says so; the relay hears it, and control reaches it.
    send(&conn, &cast::control_state_bytes(true)).await;
    let CastEvent::Payload { payload, .. } = next(&mut events).await else {
        panic!("the control state");
    };
    assert_eq!(payload, CastPayload::ControlState { on: true });
    control
        .send(batch.clone())
        .await
        .expect("the control channel");
    assert_eq!(
        take_control(&remote).await,
        Some(batch.clone()),
        "control reached the phone"
    );
    // It says it no longer does: nothing more reaches it.
    send(&conn, &cast::control_state_bytes(false)).await;
    let _ = next(&mut events).await;
    control.send(batch).await.expect("the control channel");
    assert_eq!(
        {
            tokio::time::sleep(Duration::from_millis(500)).await;
            remote.control.take(Duration::ZERO)
        },
        None
    );

    // A cast does not drive the computer.
    send(&conn, &InputPacket::MouseMove { dx: 5, dy: -3 }.encode()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), input_rx.recv())
            .await
            .is_err(),
        "a cast's input is not injected"
    );

    // The phone stops: the relay is told.
    conn.close(0u32, b"done");
    loop {
        match next(&mut events).await {
            CastEvent::End { session_id: ended } => {
                assert_eq!(ended, session_id);
                break;
            }
            CastEvent::Payload { .. } => continue,
            CastEvent::Start { .. } => panic!("a second cast"),
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), session).await;
    receiving.abort();
}

#[tokio::test]
async fn a_cast_without_the_permission_is_refused_with_the_reason() {
    let (cast_tx, mut events) = mpsc::channel(64);
    let (addr, cert, _input_rx, session) = serve(Some(cast_tx), false).await;
    let client = connect_cast(addr, cert).await;
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
    assert!(
        reason.contains("phone screen not allowed"),
        "the reason: {reason}"
    );
    assert!(events.try_recv().is_err(), "the relay heard nothing");
}

#[tokio::test]
async fn a_cast_with_nowhere_to_show_it_is_refused_with_the_reason() {
    let (addr, cert, _input_rx, session) = serve(None, true).await;
    let client = connect_cast(addr, cert).await;
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
    assert!(
        reason.contains("phone screen not shown here"),
        "the reason: {reason}"
    );
}
