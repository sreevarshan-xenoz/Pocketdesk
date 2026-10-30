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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::view::{self as wire, Message, Phase, ViewFile, ViewPrivacy};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::streaming::transport::{
    CertManager, PacketHeader, STREAM_KIND_READY, STREAM_KIND_VIDEO, StreamServer,
    StreamTransportConfig,
};
use pocketdesk_core::streaming::{
    InputPacket, SCREEN_FEATURE_INPUT, SCREEN_FEATURE_SYSTEM_SAID, SCREEN_FEATURE_WINDOWS,
    SCREEN_FEATURES_SAID,
};
use pocketdesk_server::computer_view::{
    Command, Dialer, PRIVACY_RENEW, PrivacyHost, WINDOW_GRACE, run, run_with,
};
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
            wake: None,
            viewed_unix: 0,
        })
    })
    .unwrap();
}

/// Why the stand-in closes a session (PC2): its End, which is not dialled
/// again, or its server stopping, which is.
const KICKED: &[u8] = b"kicked by user";
const STOPPING: &[u8] = b"session ended";

/// The stand-in for the other computer: one session, admitted with input
/// taken, a keyframe every 50 ms; the input packets it gets go to `input`.
/// It closes the session, with the reason given, when `hang_up` fires.
async fn other_computer(
    cert: Arc<CertManager>,
    input: mpsc::UnboundedSender<InputPacket>,
    hang_up: tokio::sync::oneshot::Receiver<&'static [u8]>,
) -> SocketAddr {
    other_computer_sending(cert, input, hang_up, keyframe()).await
}

/// [`other_computer`], its frames each `frame`.
async fn other_computer_sending(
    cert: Arc<CertManager>,
    input: mpsc::UnboundedSender<InputPacket>,
    hang_up: tokio::sync::oneshot::Receiver<&'static [u8]>,
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
        serve(conn, input, frame, hang_up).await;
    });
    addr
}

/// One session of the stand-in, until `hang_up` says why it closes (or the
/// viewer goes).
async fn serve(
    conn: quinn::Connection,
    input: mpsc::UnboundedSender<InputPacket>,
    frame: Vec<u8>,
    hang_up: tokio::sync::oneshot::Receiver<&'static [u8]>,
) {
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
        // A Windows computer, for the window's Send keys menu (PC4).
        vec![
            SCREEN_FEATURES_SAID
                | SCREEN_FEATURE_INPUT
                | SCREEN_FEATURE_SYSTEM_SAID
                | SCREEN_FEATURE_WINDOWS,
        ],
    )
    .await
    .expect("ready");
    let mut hang_up = hang_up;
    let mut seq = 0;
    let reason = loop {
        tokio::select! {
            reason = &mut hang_up => break reason.unwrap_or(b"done"),
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if send(STREAM_KIND_VIDEO, seq, true, frame.clone()).await.is_err() {
                    break b"done";
                }
                seq += 1;
            }
        }
    };
    conn.close(0u32.into(), reason);
}

/// A stand-in that takes one session after another, as a computer whose
/// server restarts does (PC2): each closes as `hang_ups` says; the count
/// of sessions it took goes up as each comes.
async fn other_computer_again_and_again(
    cert: Arc<CertManager>,
    refuse_after: Option<usize>,
) -> (
    SocketAddr,
    Arc<AtomicUsize>,
    mpsc::UnboundedSender<&'static [u8]>,
) {
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse().unwrap(),
        ..StreamTransportConfig::default()
    };
    let server = StreamServer::new(listen, &cert).await.expect("bind");
    let addr = server.local_addr().unwrap();
    let sessions = Arc::new(AtomicUsize::new(0));
    let (hang_ups, mut hang_up_rx) = mpsc::unbounded_channel::<&'static [u8]>();
    let counted = sessions.clone();
    tokio::spawn(async move {
        loop {
            let Some(incoming) = server.accept_connection().await else {
                return;
            };
            let Ok(connecting) = incoming.accept() else {
                continue;
            };
            let Ok(conn) = connecting.await else { continue };
            let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
            // Past `refuse_after`, its owner says no at once.
            if refuse_after.is_some_and(|after| n > after) {
                conn.close(0u32.into(), b"session refused");
                continue;
            }
            let (this_tx, this_rx) = tokio::sync::oneshot::channel();
            let (input, _ignored) = mpsc::unbounded_channel();
            let session = tokio::spawn(serve(conn, input, keyframe(), this_rx));
            // The next hang-up closes this session.
            match hang_up_rx.recv().await {
                Some(reason) => {
                    let _ = this_tx.send(reason);
                    let _ = session.await;
                }
                None => return,
            }
        }
    });
    (addr, sessions, hang_ups)
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
            hevc: false,
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
    assert_eq!(showing.windows, Some(true), "its system, said (PC4)");
    assert_eq!(showing.route, "home network");

    // PC2: about once a second, how the picture comes.
    let stats = loop {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("stats within 10 s")
        {
            Message::Status(status) => match status.stats {
                Some(stats) if stats.fps > 0 => break stats,
                _ => continue,
            },
            _ => continue,
        }
    };
    assert_eq!(stats.codec, "H.264");
    assert!(stats.kbps > 0, "{stats:?}");
    assert!(stats.rtt_ms.is_some(), "{stats:?}");
    assert!(stats.dial_ms.is_some(), "{stats:?}");
    assert!(!stats.relayed, "the home network rides no relay");
    assert_eq!((stats.lost, stats.late, stats.dropped), (0, 0, 0));

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

    // The computer ends the session (its End): the window hears so, it is
    // not dialled again, and view.json forgets the view.
    let _ = hang_up_tx.send(KICKED);
    let why = loop {
        match tokio::time::timeout(Duration::from_secs(60), read_message(&mut window))
            .await
            .expect("the end is said")
        {
            Message::End { why } => break why,
            _ => continue,
        }
    };
    assert_eq!(why, "Desk ended the view (kicked by user).");
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

/// PC2: the other computer's session drops as its server stops: the view
/// says it is reconnecting and dials again by itself, and the picture comes
/// back in the same view, with no new window and no click. A Stop while it
/// tries ends it, and nothing dials again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_view_reconnects_by_itself_until_stopped() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("again");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (addr, sessions, hang_ups) = other_computer_again_and_again(cert.clone(), None).await;
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
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    let mut seen = 0;
    while seen < 3 {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("frames within 10 s")
        {
            Message::Frame { .. } => seen += 1,
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    assert_eq!(sessions.load(Ordering::SeqCst), 1);

    // Its server stops: reconnecting is said, then the size and the
    // picture come again in the same view.
    hang_ups.send(STOPPING).unwrap();
    let (mut tries, mut config_again, mut after) = (Vec::new(), false, 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while after < 3 {
        assert!(tokio::time::Instant::now() < deadline, "back within 20 s");
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.reconnecting > 0 => {
                // Dialling, or reached and not let in yet: not shown.
                assert_ne!(status.phase, Phase::Showing, "{status:?}");
                if status.phase == Phase::Connecting {
                    assert_eq!(status.stats, None, "no stats while it dials");
                }
                tries.push(status.reconnecting);
            }
            Message::Status(status) if !tries.is_empty() && status.phase == Phase::Showing => {
                assert_eq!(status.reconnecting, 0, "shown again");
            }
            Message::Config { .. } if !tries.is_empty() => config_again = true,
            Message::Frame { .. } if config_again => after += 1,
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    assert_eq!(tries.first(), Some(&1), "{tries:?}");
    assert_eq!(sessions.load(Ordering::SeqCst), 2, "dialled once more");
    assert_eq!(
        wire::read(&dir).map(|f| f.session_id),
        Some(file.session_id),
        "the same view"
    );

    // It drops again, and the window stops it while it waits to dial.
    hang_ups.send(STOPPING).unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.reconnecting > 0 => break,
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    commands
        .send(Command::Stop {
            session_id: Some(file.session_id),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        sessions.load(Ordering::SeqCst),
        2,
        "never dialled again once stopped"
    );
    assert_eq!(wire::read(&dir), None, "view.json forgets the view");
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Start a view of the stand-in at `addr` through a relay over `dir`, join
/// it as the window, and read until three frames came.
async fn shown_view(
    dir: &Path,
    addr: SocketAddr,
    cert: Arc<CertManager>,
) -> (
    mpsc::Sender<Command>,
    tokio::task::JoinHandle<()>,
    ViewFile,
    TcpStream,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run(
        rx,
        listener,
        dir.to_path_buf(),
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
        })
        .await
        .unwrap();
    let file = view_file(dir).await;
    let mut window = join(&file).await;
    let mut seen = 0;
    while seen < 3 {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("frames within 10 s")
        {
            Message::Frame { .. } => seen += 1,
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    (commands, relay, file, window)
}

/// The window's next end, skipping the rest.
async fn end_of(window: &mut TcpStream) -> String {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), read_message(window))
            .await
            .expect("the end is said")
        {
            Message::End { why } => return why,
            _ => continue,
        }
    }
}

/// PR #551 review I1: a computer forgotten here while its view tries to
/// come back is not dialled again; the view ends saying so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_computer_forgotten_during_the_tries_is_not_dialled_again() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("forgotten-tries");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (addr, sessions, hang_ups) = other_computer_again_and_again(cert.clone(), None).await;
    let (commands, relay, _file, mut window) = shown_view(&dir, addr, cert).await;
    hang_ups.send(STOPPING).unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
            .await
            .expect("a message")
        {
            Message::Status(status) if status.reconnecting > 0 => break,
            Message::End { why } => panic!("ended: {why}"),
            _ => {}
        }
    }
    // Forgotten here before the try (1 s later).
    computers::update(&dir, |file| file.computers.clear()).unwrap();
    assert_eq!(end_of(&mut window).await, "Desk is no longer paired here.");
    assert_eq!(sessions.load(Ordering::SeqCst), 1, "not dialled again");
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PR #551 review I1: forgetting the computer shown ends its view at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forgetting_the_computer_shown_ends_its_view() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("forgotten");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (addr, _sessions, _hang_ups) = other_computer_again_and_again(cert.clone(), None).await;
    let (commands, relay, _file, mut window) = shown_view(&dir, addr, cert).await;
    // Another computer forgotten: the view goes on.
    commands
        .send(Command::Forgotten {
            device_id: "lap".into(),
        })
        .await
        .unwrap();
    commands
        .send(Command::Forgotten {
            device_id: "desk".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        end_of(&mut window).await,
        "That computer is no longer paired with this one."
    );
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

/// PR #551 review M10: a computer that refuses the view as it comes back
/// (its owner said no) ends it, and is not dialled again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_during_the_tries_ends_the_view() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("refused-tries");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (addr, sessions, hang_ups) = other_computer_again_and_again(cert.clone(), Some(1)).await;
    let (commands, relay, _file, mut window) = shown_view(&dir, addr, cert).await;
    hang_ups.send(STOPPING).unwrap();
    let why = end_of(&mut window).await;
    assert!(
        why.starts_with("Desk did not let this computer in (session refused)"),
        "{why}"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(sessions.load(Ordering::SeqCst), 2, "not dialled again");
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
            hevc: false,
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
            hevc: false,
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
            hevc: false,
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
            hevc: false,
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

/// The window's next status whose privacy is `want` (PC8), skipping the
/// rest.
async fn privacy_said(window: &mut TcpStream, want: &ViewPrivacy) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "privacy {want:?} said within 10 s"
        );
        if let Message::Status(status) =
            tokio::time::timeout(Duration::from_secs(10), read_message(window))
                .await
                .expect("a message")
            && &status.privacy == want
        {
            return;
        }
    }
}

/// PC8: the window's "Hide what I do there" goes to the computer it shows
/// as the phone's own privacy packet, to a stand-in for that computer here
/// (nothing is grabbed or locked); the answer decides what the window says;
/// another computer's answer is not taken; and the window's last word, a
/// lock as it closes, lets privacy go and locks, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_windows_privacy_is_asked_of_the_computer_and_let_go_as_it_closes() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;

    let (asked_tx, mut asked) = mpsc::unbounded_channel::<(String, serde_json::Value)>();
    let (answers, answers_rx) = mpsc::channel(8);
    let host = PrivacyHost {
        send: Arc::new(move |id: &str, packet: NetworkPacket| {
            assert_eq!(packet.packet_type, "kdeconnect.pocketdesk.privacy");
            let _ = asked_tx.send((id.to_string(), without_id(packet.body)));
        }),
        answers: answers_rx,
        notice: Arc::new(|_| {}),
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run_with(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr,
            certs: cert,
            id: "viewer".into(),
        },
        host,
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
            hevc: false,
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    // Shown, with control taken there.
    loop {
        if let Message::Status(status) =
            tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
                .await
                .expect("shown within 10 s")
            && status.phase == Phase::Showing
            && status.control == Some(true)
        {
            break;
        }
    }
    let next_ask = async |asked: &mut mpsc::UnboundedReceiver<(String, serde_json::Value)>| {
        tokio::time::timeout(Duration::from_secs(5), asked.recv())
            .await
            .expect("an ask")
            .expect("the host")
    };
    let say = |ask| Message::Privacy(ask).encode();

    // Hide: the grab goes, the window hears it is asked, then held.
    window.write_all(&say(wire::PRIVACY_HIDE)).await.unwrap();
    assert_eq!(
        next_ask(&mut asked).await,
        ("desk".into(), serde_json::json!({"action": "grab"}))
    );
    privacy_said(&mut window, &ViewPrivacy::Asking).await;
    // Another computer's answer is not this view's (review M8): its
    // refusal never reaches the window.
    answers
        .send((
            "other".into(),
            serde_json::json!({"ok": false, "error": "not this view's"}),
        ))
        .await
        .unwrap();
    answers
        .send(("desk".into(), serde_json::json!({"ok": true, "grabbed": 2})))
        .await
        .unwrap();
    privacy_said_never(
        &mut window,
        &ViewPrivacy::On,
        &ViewPrivacy::Refused("not this view's".into()),
    )
    .await;

    // That computer ends it (as a renewal refused says): the window says why.
    let ended = "privacy mode ended there";
    answers
        .send((
            "desk".into(),
            serde_json::json!({"ok": false, "error": ended}),
        ))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::Refused(ended.into())).await;
    // Let go there anyway (review M3): nothing is left held by a refusal.
    assert_eq!(next_ask(&mut asked).await.1["action"], "release");

    // Refused by its Power switch: let go there too, which is nothing.
    window.write_all(&say(wire::PRIVACY_HIDE)).await.unwrap();
    assert_eq!(next_ask(&mut asked).await.1["action"], "grab");
    let power =
        "\u{201c}Lock, sleep, privacy and wake others\u{201d} is off for this computer there";
    answers
        .send((
            "desk".into(),
            serde_json::json!({"ok": false, "error": power}),
        ))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::Refused(power.into())).await;
    assert_eq!(next_ask(&mut asked).await.1["action"], "release");

    // Held again, then the window closes with its lock: one release, with
    // the lock, and nothing after it.
    window.write_all(&say(wire::PRIVACY_HIDE)).await.unwrap();
    assert_eq!(next_ask(&mut asked).await.1["action"], "grab");
    answers
        .send(("desk".into(), serde_json::json!({"ok": true, "grabbed": 2})))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::On).await;
    window.write_all(&say(wire::PRIVACY_LOCK)).await.unwrap();
    // Let go, then the lock as its own status (review I3).
    assert_eq!(
        next_ask(&mut asked).await,
        ("desk".into(), serde_json::json!({"action": "release"}))
    );
    assert_eq!(
        next_ask(&mut asked).await,
        (
            "desk".into(),
            serde_json::json!({"action": "status", "lock": true})
        )
    );
    commands
        .send(Command::Stop { session_id: None })
        .await
        .unwrap();
    drop(window);
    assert!(
        tokio::time::timeout(Duration::from_millis(700), asked.recv())
            .await
            .is_err(),
        "let go once"
    );
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PC8: privacy held when the view ends some other way (Stop, the window
/// gone) is let go there, without a lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn privacy_held_is_let_go_as_the_view_ends() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy-end");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;
    let (asked_tx, mut asked) = mpsc::unbounded_channel::<serde_json::Value>();
    let (answers, answers_rx) = mpsc::channel(8);
    let host = PrivacyHost {
        send: Arc::new(move |_: &str, packet: NetworkPacket| {
            let _ = asked_tx.send(without_id(packet.body));
        }),
        answers: answers_rx,
        notice: Arc::new(|_| {}),
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run_with(
        rx,
        listener,
        dir.clone(),
        Dialer::Direct {
            addr,
            certs: cert,
            id: "viewer".into(),
        },
        host,
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
            hevc: false,
        })
        .await
        .unwrap();
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    loop {
        if let Message::Status(status) =
            tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
                .await
                .expect("shown within 10 s")
            && status.phase == Phase::Showing
            && status.control == Some(true)
        {
            break;
        }
    }
    window
        .write_all(&Message::Privacy(wire::PRIVACY_HIDE).encode())
        .await
        .unwrap();
    let wait = Duration::from_secs(5);
    let grab = tokio::time::timeout(wait, asked.recv()).await.unwrap();
    assert_eq!(grab, Some(serde_json::json!({"action": "grab"})));
    answers
        .send(("desk".into(), serde_json::json!({"ok": true, "grabbed": 2})))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::On).await;
    commands
        .send(Command::Stop { session_id: None })
        .await
        .unwrap();
    let release = tokio::time::timeout(wait, asked.recv()).await.unwrap();
    assert_eq!(release, Some(serde_json::json!({"action": "release"})));
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PR #561 review I2: a window gone without a word (its socket closed, as
/// a crash leaves it) lets privacy go there after the short grace (PR #563
/// re-review N2), not after the view's 30 s without a watcher, and nothing
/// is renewed for nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn privacy_is_let_go_as_the_window_goes() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy-gone");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;
    let (asked_tx, mut asked) = mpsc::unbounded_channel::<serde_json::Value>();
    let (answers, answers_rx) = mpsc::channel(8);
    let host = stand_in_host(
        move |_, body| {
            let _ = asked_tx.send(body);
        },
        answers_rx,
    );
    let (commands, relay, mut window) = controlled_view(&dir, addr, cert, host).await;
    window
        .write_all(&Message::Privacy(wire::PRIVACY_HIDE).encode())
        .await
        .unwrap();
    let wait = Duration::from_secs(5);
    let grab = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab["action"], "grab");
    answers
        .send((
            "desk".into(),
            answer_to(&grab, serde_json::json!({"ok": true, "grabbed": 2})),
        ))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::On).await;
    // The window's socket closes, with no word first.
    drop(window);
    let release = tokio::time::timeout(WINDOW_GRACE + Duration::from_secs(6), asked.recv())
        .await
        .expect("let go once the grace and a heartbeat are past")
        .unwrap();
    assert_eq!(release["action"], "release");
    assert!(
        // Past one renewal period and its heartbeat (re-review): a hold
        // left behind would have renewed by now.
        tokio::time::timeout(PRIVACY_RENEW + Duration::from_secs(3), asked.recv())
            .await
            .is_err(),
        "nothing renewed for nobody"
    );
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PR #563 re-review N2: a window whose write stalled ends its connection
/// and joins the same view again; privacy held there stays through the gap
/// (nothing let go, the window hears it on), and is let go only once the
/// window stays away past the grace.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn privacy_stays_for_a_window_that_joins_again() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy-rejoin");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;
    let (asked_tx, mut asked) = mpsc::unbounded_channel::<serde_json::Value>();
    let (answers, answers_rx) = mpsc::channel(8);
    let host = stand_in_host(
        move |_, body| {
            let _ = asked_tx.send(body);
        },
        answers_rx,
    );
    let (commands, relay, mut window) = controlled_view(&dir, addr, cert, host).await;
    window
        .write_all(&Message::Privacy(wire::PRIVACY_HIDE).encode())
        .await
        .unwrap();
    let grab = tokio::time::timeout(Duration::from_secs(5), asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab["action"], "grab");
    answers
        .send((
            "desk".into(),
            answer_to(&grab, serde_json::json!({"ok": true, "grabbed": 2})),
        ))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::On).await;
    // The window ends its connection, as a stalled write does, waits for a
    // heartbeat to find it gone, and joins the same view again.
    window.shutdown().await.unwrap();
    drop(window);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let file = view_file(&dir).await;
    let mut window = join(&file).await;
    privacy_said(&mut window, &ViewPrivacy::On).await;
    // Past the grace and a heartbeat: nothing was let go meanwhile.
    // A renewal that comes meanwhile is answered, so nothing gives up.
    let quiet = tokio::time::timeout(WINDOW_GRACE + Duration::from_secs(3), async {
        loop {
            let body = asked.recv().await.unwrap();
            if body["action"] != "grab" {
                return body;
            }
            let held = answer_to(&body, serde_json::json!({"ok": true, "grabbed": 2}));
            answers.send(("desk".into(), held)).await.unwrap();
        }
    })
    .await;
    assert!(quiet.is_err(), "kept for the window back: {quiet:?}");
    // Gone for good: let go once the grace is past.
    drop(window);
    let release = tokio::time::timeout(WINDOW_GRACE + Duration::from_secs(6), async {
        loop {
            let body = asked.recv().await.unwrap();
            if body["action"] == "release" {
                return body;
            }
            let held = answer_to(&body, serde_json::json!({"ok": true, "grabbed": 2}));
            answers.send(("desk".into(), held)).await.unwrap();
        }
    })
    .await
    .expect("let go once the window stays away");
    assert_eq!(release["action"], "release");
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// PR #568 review I2: a lock on close that did not reach the relay (the
/// window between joins, or its write failed) comes with the stop: privacy
/// held is let go, then the lock asked, its refusal said here; one for a
/// view already gone is said here as not done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_on_close_carried_by_the_stop_is_asked_or_said() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy-lock-stop");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;
    let (asked_tx, mut asked) = mpsc::unbounded_channel::<serde_json::Value>();
    let (answers, answers_rx) = mpsc::channel(8);
    let (told_tx, mut told) = mpsc::unbounded_channel::<String>();
    let mut host = stand_in_host(
        move |_, body| {
            let _ = asked_tx.send(body);
        },
        answers_rx,
    );
    host.notice = Arc::new(move |words| {
        let _ = told_tx.send(words);
    });
    let (commands, relay, mut window) = controlled_view(&dir, addr, cert, host).await;
    let wait = Duration::from_secs(5);
    window
        .write_all(&Message::Privacy(wire::PRIVACY_HIDE).encode())
        .await
        .unwrap();
    let grab = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(grab["action"], "grab");
    answers
        .send((
            "desk".into(),
            answer_to(&grab, serde_json::json!({"ok": true, "grabbed": 2})),
        ))
        .await
        .unwrap();
    privacy_said(&mut window, &ViewPrivacy::On).await;
    // The window went with no word (between joins); its stop carries the
    // lock.
    drop(window);
    commands
        .send(Command::LockAsItStops { session_id: None })
        .await
        .unwrap();
    commands
        .send(Command::Stop { session_id: None })
        .await
        .unwrap();
    let release = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release["action"], "release", "privacy let go first");
    let lock = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (&lock["action"], &lock["lock"]),
        (&"status".into(), &true.into())
    );
    answers
        .send((
            "desk".into(),
            answer_to(
                &lock,
                serde_json::json!({"ok": false, "error": "that switch is off there"}),
            ),
        ))
        .await
        .unwrap();
    let said = tokio::time::timeout(wait, told.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(said, "Couldn't lock Desk: that switch is off there");
    // No view now: said as not done, nothing asked.
    commands
        .send(Command::LockAsItStops { session_id: None })
        .await
        .unwrap();
    let said = tokio::time::timeout(wait, told.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(said.starts_with("Couldn't lock "), "{said}");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), asked.recv())
            .await
            .is_err(),
        "nothing asked of a computer with no view"
    );
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for the computer shown (PC8): what the relay asks of it goes
/// to `asked`, with the computer's id; its answers come from `answers`.
/// Nothing is grabbed or locked.
fn stand_in_host(
    asked: impl Fn(&str, serde_json::Value) + Send + Sync + 'static,
    answers: mpsc::Receiver<(String, serde_json::Value)>,
) -> PrivacyHost {
    PrivacyHost {
        send: Arc::new(move |id: &str, packet: NetworkPacket| {
            assert_eq!(packet.packet_type, "kdeconnect.pocketdesk.privacy");
            asked(id, packet.body);
        }),
        answers,
        notice: Arc::new(|_| {}),
    }
}

/// The answer `body` to the ask `asked`, naming it as a host from PC8's
/// review on does (its `id`, when it had one).
fn answer_to(asked: &serde_json::Value, mut body: serde_json::Value) -> serde_json::Value {
    if let Some(id) = asked.get("id") {
        body["id"] = id.clone();
    }
    body
}

/// A view of the stand-in at `addr` through a relay over `dir` asking its
/// privacy of `host`, joined as the window and read until it is shown with
/// control taken there.
async fn controlled_view(
    dir: &Path,
    addr: SocketAddr,
    cert: Arc<CertManager>,
    host: PrivacyHost,
) -> (
    mpsc::Sender<Command>,
    tokio::task::JoinHandle<()>,
    TcpStream,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let (commands, rx) = mpsc::channel(4);
    let relay = tokio::spawn(run_with(
        rx,
        listener,
        dir.to_path_buf(),
        Dialer::Direct {
            addr,
            certs: cert,
            id: "viewer".into(),
        },
        host,
    ));
    commands
        .send(Command::Start {
            device_id: "desk".into(),
            monitor: 0,
            hevc: false,
        })
        .await
        .unwrap();
    let file = view_file(dir).await;
    let mut window = join(&file).await;
    loop {
        if let Message::Status(status) =
            tokio::time::timeout(Duration::from_secs(10), read_message(&mut window))
                .await
                .expect("shown within 10 s")
            && status.phase == Phase::Showing
            && status.control == Some(true)
        {
            break;
        }
    }
    (commands, relay, window)
}

/// An ask's body without the id it names itself by (PR #561 review M3).
fn without_id(mut body: serde_json::Value) -> serde_json::Value {
    assert!(
        body["id"].as_u64().is_some(),
        "every ask names itself: {body}"
    );
    body.as_object_mut().unwrap().remove("id");
    body
}

/// PR #561 review I3: a lock asked as the window closes that is not done
/// there (refused, or its locker failed) is said to the owner here; one
/// done says nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_on_close_not_done_is_said() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = temp_dir("privacy-lock");
    paired(&dir);
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let (input_tx, _input_rx) = mpsc::unbounded_channel();
    let (_hang_up, hang_up_rx) = tokio::sync::oneshot::channel();
    let addr = other_computer(cert.clone(), input_tx, hang_up_rx).await;
    let (asked_tx, mut asked) = mpsc::unbounded_channel::<serde_json::Value>();
    let (answers, answers_rx) = mpsc::channel(8);
    let (told_tx, mut told) = mpsc::unbounded_channel::<String>();
    let mut host = stand_in_host(
        move |_, body| {
            let _ = asked_tx.send(body);
        },
        answers_rx,
    );
    host.notice = Arc::new(move |words| {
        let _ = told_tx.send(words);
    });
    let (commands, relay, mut window) = controlled_view(&dir, addr, cert, host).await;
    let wait = Duration::from_secs(5);
    let lock = Message::Privacy(wire::PRIVACY_LOCK).encode();

    // Refused there: said here, by the computer's name and why.
    window.write_all(&lock).await.unwrap();
    let ask = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (&ask["action"], &ask["lock"]),
        (&"status".into(), &true.into())
    );
    let why = "\u{201c}Lock, sleep, privacy and wake others\u{201d} is off for this computer there";
    answers
        .send((
            "desk".into(),
            answer_to(&ask, serde_json::json!({"ok": false, "error": why})),
        ))
        .await
        .unwrap();
    let said = tokio::time::timeout(wait, told.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(said, format!("Couldn't lock Desk: {why}"));

    // Its locker failed: said too.
    window.write_all(&lock).await.unwrap();
    let ask = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    answers
        .send((
            "desk".into(),
            answer_to(
                &ask,
                serde_json::json!({"ok": true, "grabbed": 0, "ttlLeft": null, "lockError": "no screen locker found"}),
            ),
        ))
        .await
        .unwrap();
    let said = tokio::time::timeout(wait, told.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(said, "Couldn't lock Desk: no screen locker found");

    // Done: nothing to say.
    window.write_all(&lock).await.unwrap();
    let ask = tokio::time::timeout(wait, asked.recv())
        .await
        .unwrap()
        .unwrap();
    answers
        .send((
            "desk".into(),
            answer_to(&ask, serde_json::json!({"ok": true, "grabbed": 0, "ttlLeft": null, "locked": "loginctl"})),
        ))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), told.recv())
            .await
            .is_err(),
        "a lock done says nothing"
    );
    drop(window);
    drop(commands);
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// [`privacy_said`], failing if the window is told `never` first.
async fn privacy_said_never(window: &mut TcpStream, want: &ViewPrivacy, never: &ViewPrivacy) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "privacy {want:?} said within 10 s"
        );
        if let Message::Status(status) =
            tokio::time::timeout(Duration::from_secs(10), read_message(window))
                .await
                .expect("a message")
        {
            assert_ne!(&status.privacy, never);
            if &status.privacy == want {
                return;
            }
        }
    }
}
