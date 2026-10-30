//! Another computer's screen in a window (BK7) from the real streaming
//! server, in process over loopback: its encoder runs on the test pattern
//! (`POCKETDESK_TEST_PATTERN`, so no one's desktop is read), the view relay
//! dials it, and a stand-in for the window counts frames at the size the
//! session encodes and sends input. The input reaches the server's input
//! channel, which this test holds: nothing is injected.
//!
//! No sound, either way (BK7 review I4: without a gate the session had the
//! default permissions, Audio among them, so the server captured this
//! computer's real sound and the relay played it on its speakers). The
//! session's gate gives Input and takes away Audio and the mic, so the
//! server starts no sound capture; and the view's sound seam points at a
//! file in the test's folder, which must never be written.
//!
//! Ignored: it runs a real encoder. Run it with
//! `cargo test -p pocketdesk-server --test computer_view_real_session -- --ignored --nocapture`.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::view::{self as wire, Message, Phase};
use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{InputPacket, QuinnConnection, StreamingConfig, StreamingServer};
use pocketdesk_server::computer_view::{Command, Dialer, run};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

async fn read_message(stream: &mut TcpStream) -> Message {
    let mut head = [0u8; 5];
    stream.read_exact(&mut head).await.expect("head");
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.expect("payload");
    Message::parse(head[0], payload).expect("a message")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs a real encoder"]
async fn the_real_servers_screen_reaches_the_window_and_the_window_its_input() {
    // SAFETY: this test binary has this one test, set before anything runs.
    unsafe { std::env::set_var(pocketdesk_core::streaming::test_pattern::VAR, "1") };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = std::env::temp_dir().join(format!("pd-view-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _remove = Remove(dir.clone());
    // A second guard: any sound the view got would go to this file, never
    // the speakers. SAFETY: as above, the one test, before anything runs.
    let sound_seam = dir.join("view-sound.wav");
    unsafe { std::env::set_var(pocketdesk_server::computer_view::SOUND_SEAM, &sound_seam) };
    computers::update(&dir, |file| {
        file.computers.push(PeerComputer {
            device_id: "desk".into(),
            name: "Desk".into(),
            addresses: Vec::new(),
            control_port: 1716,
            stream_port: 4716,
            fingerprint: String::new(),
            endpoint: None,
            paired_unix: 0,
            link: String::new(),
            link_unix: 0,
            lets_drive: false,
            mine: false,
            wake: None,
            viewed_unix: 0,
            keys_by_position: None,
        })
    })
    .unwrap();

    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert).await.expect("bind");
    let addr = stream_server.local_addr().unwrap();
    let (input_tx, mut input_rx) = tokio::sync::broadcast::channel(64);
    let session_cert = cert.clone();
    let session = tokio::spawn(async move {
        let connection = stream_server
            .accept_connection()
            .await
            .expect("a client")
            .accept()
            .expect("accepted")
            .await
            .expect("handshake");
        let config = StreamingConfig {
            width: 640,
            height: 360,
            ..StreamingConfig::default()
        };
        let mut server =
            StreamingServer::new(config, StreamTransportConfig::default(), session_cert);
        server.set_input_channel(input_tx);
        // Input, and no sound either way: no capture starts.
        let mut permissions = DevicePermissions::default();
        permissions.set(Capability::Input, true);
        permissions.set(Capability::Audio, false);
        permissions.set(Capability::Mic, false);
        permissions.ask_first = false;
        assert!(!permissions.allows(Capability::Audio));
        server.set_session_gate(move |_claim| {
            let permissions = permissions.clone();
            Box::pin(async move { Some(permissions) })
        });
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr,
            certs: cert,
            id: "viewer".into(),
        },
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
            hevc: false,
            window: 0,
        })
        .await
        .unwrap();
    let mut file = None;
    for _ in 0..200 {
        file = wire::read(&dir);
        if file.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let file = file.expect("view.json");
    let mut window = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
        .await
        .unwrap();
    window
        .write_all(&file.token_bytes().unwrap())
        .await
        .unwrap();

    let (mut size, mut frames, mut keyframes, mut control) = (None, 0u32, 0u32, None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while frames < 60 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "60 frames within 60 s, {frames} came"
        );
        match tokio::time::timeout(Duration::from_secs(20), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.phase == Phase::Showing => control = status.control,
            Message::Config { width, height } => size = Some((width, height)),
            Message::Frame { keyframe, .. } => {
                assert!(size.is_some(), "the size comes before a frame");
                frames += 1;
                keyframes += u32::from(keyframe);
            }
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    println!("{frames} frames ({keyframes} keyframes) at {size:?}, control {control:?}");
    assert_eq!(size, Some((640, 360)), "the size the session encodes");
    assert_eq!(control, Some(true), "the gate gives input");

    for packet in [
        InputPacket::MouseMoveAbs {
            x_norm: 1000,
            y_norm: 2000,
        },
        InputPacket::KeyEvent {
            key: 30,
            pressed: true,
        },
        InputPacket::KeyEvent {
            key: 30,
            pressed: false,
        },
    ] {
        window
            .write_all(&Message::Input(packet.encode()).encode())
            .await
            .unwrap();
    }
    let mut got = Vec::new();
    while got.len() < 3 {
        let (_, packet) = tokio::time::timeout(Duration::from_secs(10), input_rx.recv())
            .await
            .expect("input reaches the server's channel")
            .expect("input channel");
        match packet {
            InputPacket::MouseMoveAbs { .. } | InputPacket::KeyEvent { .. } => {
                got.push(format!("{packet:?}"))
            }
            _ => {}
        }
    }
    got.sort();
    assert_eq!(
        got,
        [
            "KeyEvent { key: 30, pressed: false }",
            "KeyEvent { key: 30, pressed: true }",
            "MouseMoveAbs { x_norm: 1000, y_norm: 2000 }",
        ]
    );

    // The window closes: the session on the server ends.
    commands
        .send(Command::Stop {
            session_id: Some(file.session_id),
            window: 0,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), session)
        .await
        .expect("the server's session ended")
        .expect("joined")
        .ok();
    drop(commands);
    relay.await.unwrap();
    assert!(
        !sound_seam.exists(),
        "no sound came: the session had no Audio permission"
    );
}
