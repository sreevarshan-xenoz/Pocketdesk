//! Another computer's screen in a window (BK7), over loopback: the view
//! relay dials a screen session, relays its picture to a stand-in for the
//! desktop app's window with the size its SPS says, and the window's input
//! reaches the other computer, only what a window may send. The other
//! computer here is a stand-in that speaks the stream's wire (its ready
//! stream, then video frames) and records the input packets it gets: it
//! encodes nothing and injects nothing, so this runs anywhere. The same
//! road with the real streaming server and its encoder is
//! `computer_view_real_session`, ignored.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::view::{self as wire, Message, Phase, ViewFile};
use pocketdesk_core::streaming::transport::{
    CertManager, PacketHeader, STREAM_KIND_READY, STREAM_KIND_VIDEO, StreamServer,
    StreamTransportConfig,
};
use pocketdesk_core::streaming::{InputPacket, SCREEN_FEATURE_INPUT, SCREEN_FEATURES_SAID};
use pocketdesk_server::computer_view::{Command, Dialer, run};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// A 1920x1080 SPS as ffmpeg's libx264 writes it.
const SPS: &str = "67640028acd940780227e5c044000003000400000300503c60c658";

fn keyframe() -> Vec<u8> {
    let mut unit = vec![0, 0, 0, 1];
    unit.extend(
        (0..SPS.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&SPS[i..i + 2], 16).unwrap()),
    );
    unit.extend([0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00]);
    unit
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pd-view-relay-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn paired(dir: &Path) {
    computers::update(dir, |file| {
        file.computers.push(PeerComputer {
            device_id: "desk".into(),
            name: "Desk".into(),
            addresses: vec!["192.0.2.10".into()],
            control_port: 1716,
            stream_port: 4716,
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 0,
            link: String::new(),
            link_unix: 0,
            lets_drive: false,
            mine: false,
        })
    })
    .unwrap();
}

/// The stand-in for the other computer: one session, admitted with input
/// taken, a keyframe every 50 ms; the input packets it gets go to `input`.
/// It closes the session when `hang_up` fires.
async fn other_computer(
    cert: Arc<CertManager>,
    input: mpsc::UnboundedSender<InputPacket>,
    hang_up: tokio::sync::oneshot::Receiver<()>,
) -> SocketAddr {
    other_computer_sending(cert, input, hang_up, keyframe()).await
}

/// [`other_computer`], its frames each `frame`.
async fn other_computer_sending(
    cert: Arc<CertManager>,
    input: mpsc::UnboundedSender<InputPacket>,
    hang_up: tokio::sync::oneshot::Receiver<()>,
    frame: Vec<u8>,
) -> SocketAddr {
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse().unwrap(),
        ..StreamTransportConfig::default()
    };
    let server = StreamServer::new(listen, &cert).await.expect("bind");
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let conn = server
            .accept_connection()
            .await
            .expect("a client")
            .accept()
            .expect("accepted")
            .await
            .expect("handshake");
        // What the client sends: its config streams, then input.
        let reading = conn.clone();
        tokio::spawn(async move {
            while let Ok(mut stream) = reading.accept_uni().await {
                let Ok(bytes) = stream.read_to_end(64 * 1024).await else {
                    continue;
                };
                if bytes.first().is_some_and(|b| *b >= 0xF0) {
                    continue;
                }
                if let Ok(packet) = InputPacket::decode(&bytes)
                    && !matches!(
                        packet,
                        InputPacket::LinkFeedback { .. } | InputPacket::ClientSamples { .. }
                    )
                {
                    let _ = input.send(packet);
                }
            }
        });
        let send = |kind: u8, seq: u64, keyframe: bool, payload: Vec<u8>| {
            let conn = conn.clone();
            async move {
                let mut stream = conn.open_uni().await?;
                let header = PacketHeader {
                    sequence: seq,
                    stream_kind: kind,
                    is_keyframe: keyframe,
                    timestamp_us: 0,
                };
                stream.write_all(&header.as_bytes()).await?;
                stream.write_all(&payload).await?;
                stream.finish()?;
                Ok::<_, anyhow::Error>(())
            }
        };
        send(
            STREAM_KIND_READY,
            0,
            false,
            vec![SCREEN_FEATURES_SAID | SCREEN_FEATURE_INPUT],
        )
        .await
        .expect("ready");
        let mut hang_up = hang_up;
        let mut seq = 0;
        loop {
            tokio::select! {
                _ = &mut hang_up => break,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if send(STREAM_KIND_VIDEO, seq, true, frame.clone()).await.is_err() {
                        break;
                    }
                    seq += 1;
                }
            }
        }
        conn.close(0u32.into(), b"done");
    });
    addr
}

async fn read_message(stream: &mut TcpStream) -> Message {
    let mut head = [0u8; 5];
    stream.read_exact(&mut head).await.expect("head");
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.expect("payload");
    Message::parse(head[0], payload).expect("a message")
}

async fn view_file(dir: &Path) -> ViewFile {
    for _ in 0..200 {
        if let Some(file) = wire::read(dir) {
            return file;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no view.json");
}

async fn join(file: &ViewFile) -> TcpStream {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
        .await
        .unwrap();
    stream
        .write_all(&file.token_bytes().unwrap())
        .await
        .unwrap();
    stream
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_computers_screen_reaches_the_window_and_its_input_the_computer() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("screen");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    let (hang_up_tx, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;

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
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    assert_eq!(
        (file.device_id.as_str(), file.name.as_str()),
        ("desk", "Desk")
    );
    let mut window = join(&file).await;

    // Shown, with control, at the size the SPS says; then frames.
    let (mut showing, mut size, mut frames) = (None, None, 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while frames < 5 {
        assert!(tokio::time::Instant::now() < deadline, "frames within 20 s");
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.phase == Phase::Showing => {
                showing = Some(status);
            }
            Message::Config { width, height } => size = Some((width, height)),
            Message::Frame { data, .. } => {
                assert_eq!(size, Some((1920, 1080)), "the size comes before a frame");
                assert_eq!(data, keyframe());
                frames += 1;
            }
            _ => {}
        }
    }
    let showing = showing.expect("the window heard it is shown");
    assert_eq!(showing.control, Some(true));
    assert_eq!(showing.route, "home network");

    // The window's input reaches the computer; what a window may not send
    // does not.
    for packet in [
        InputPacket::MouseMoveAbs {
            x_norm: 100,
            y_norm: 200,
        },
        InputPacket::Mic {
            enabled: true,
            opus: Vec::new(),
        },
        InputPacket::KeyEvent {
            key: 28,
            pressed: true,
        },
    ] {
        window
            .write_all(&Message::Input(packet.encode()).encode())
            .await
            .unwrap();
    }
    let mut got = Vec::new();
    while got.len() < 2 {
        let packet = tokio::time::timeout(Duration::from_secs(10), input_rx.recv())
            .await
            .expect("input arrives")
            .expect("input channel");
        match packet {
            InputPacket::RequestKeyframe => {}
            other => got.push(format!("{other:?}")),
        }
    }
    got.sort();
    assert_eq!(
        got,
        [
            "KeyEvent { key: 28, pressed: true }",
            "MouseMoveAbs { x_norm: 100, y_norm: 200 }"
        ]
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                match input_rx.recv().await {
                    Some(InputPacket::RequestKeyframe) => continue,
                    other => return other,
                }
            }
        })
        .await
        .is_err(),
        "the mic packet went nowhere"
    );

    // The computer hangs up: the window hears it lost the connection, and
    // view.json forgets the view.
    let _ = hang_up_tx.send(());
    let why = loop {
        match tokio::time::timeout(Duration::from_secs(60), read_message(&mut window))
            .await
            .expect("the end is said")
        {
            Message::End { why } => break why,
            _ => continue,
        }
    };
    assert_eq!(why, "Lost the connection to Desk.");
    for _ in 0..100 {
        if wire::read(&dir).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(wire::read(&dir), None, "view.json forgets the view");

    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// BK7 review M6: frames too large for the window are said in the
/// window, not left at "Waiting for its screen".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_too_large_for_the_window_are_said() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("large");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up_tx, hang_up_rx) = tokio::sync::oneshot::channel();
    let mut huge = keyframe();
    huge.resize(pocketdesk_core::local::cast::MAX_MESSAGE + 1024, 0x11);
    let addr = other_computer_sending(cert.clone(), input_tx, hang_up_rx, huge).await;
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
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    let note = loop {
        match tokio::time::timeout(Duration::from_secs(20), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.note.is_some() => break status.note.unwrap(),
            Message::Frame { .. } => panic!("a frame too large was sent"),
            _ => continue,
        }
    };
    assert!(note.contains("too large"), "{note}");
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_computer_not_paired_here_ends_the_view_with_why() {
    let dir = temp_dir("unpaired");
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr: "127.0.0.1:9".parse().unwrap(),
            certs: cert,
            id: "viewer".into(),
        },
    ));
    commands
        .send(Command::Start {
            device_id: "gone".into(),
            monitor: 0,
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    let why = loop {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::End { why } => break why,
            _ => continue,
        }
    };
    assert_eq!(why, "That computer is no longer paired with this one.");
    // Stopped, the file goes.
    commands
        .send(Command::Stop { session_id: None })
        .await
        .unwrap();
    for _ in 0..100 {
        if wire::read(&dir).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(wire::read(&dir), None);
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A view that ends before its window comes (the computer could not be
/// reached at once) keeps its file, so the window still hears why.
#[tokio::test]
async fn a_window_that_comes_late_still_hears_why_the_view_ended() {
    let dir = temp_dir("late");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    // Nothing listens at port 9: the dial fails at once.
    let relay = tokio::spawn(run(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr: "127.0.0.1:9".parse().unwrap(),
            certs: cert,
            id: "viewer".into(),
        },
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    // Long enough for the dial to fail.
    tokio::time::sleep(Duration::from_secs(12)).await;
    assert_eq!(wire::read(&dir), Some(file.clone()), "kept for the window");
    let mut window = join(&file).await;
    let why = loop {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::End { why } => break why,
            _ => continue,
        }
    };
    assert!(why.starts_with("Could not reach Desk:"), "{why}");
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_window_without_the_token_sees_nothing() {
    let dir = temp_dir("token");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr: "127.0.0.1:9".parse().unwrap(),
            certs: cert,
            id: "viewer".into(),
        },
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port))
        .await
        .unwrap();
    stream.write_all(&[0u8; 32]).await.unwrap();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
        .await
        .expect("closed, not held");
    assert!(matches!(read, Ok(0) | Err(_)), "closed without a byte");
    drop(commands);
    relay.await.unwrap();
    assert_eq!(wire::read(&dir), None, "the relay's end forgets the view");
    let _ = std::fs::remove_dir_all(&dir);
}
