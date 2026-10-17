//! R6g: sessions that overlap, as a reconnect makes them (the new session
//! starts before the old one has ended), against the real capture and the
//! real sound, round after round. The three crashes R6 saw came right after
//! a reconnected session's setup; this is the shape that made them. Each
//! connection gets a `StreamingServer` of its own, as the server's QUIC
//! loop gives it, with the server's own process setup and video settings.
//!
//! Each round opens an old session and then a new one (each to its first
//! frame, the old one checked still alive once the new one runs), then
//! closes them in one of three ways: the old first (a reconnect), the new
//! first (a reconnect that failed after setup), or the new first while the
//! old has stopped reading (a reconnect over a stalled old link). After each
//! round the server must have ended both sessions.
//!
//! Ignored: it captures this computer's screen and sound (over loopback;
//! nothing leaves the box) for minutes. Run it with
//! `cargo test -p pocketdesk-server --test overlapping_sessions -- --ignored --nocapture`;
//! `POCKETDESK_OVERLAP_ROUNDS` sets the rounds (200 by default). A crash
//! takes the test process with it and leaves its report in the folder the
//! test prints first, beside the log of what each session started.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, SessionMode, SharedConnection, StreamingClient, StreamingServer,
};
use tokio_util::sync::CancellationToken;

/// A phone's side of one session.
struct Phone {
    connection: SharedConnection,
    cancel: CancellationToken,
    receiving: tokio::task::JoinHandle<()>,
    /// Frames received since the first.
    frames: Arc<AtomicUsize>,
}

/// Open a screen session and wait for its first frame. `read`: keep reading
/// its frames (else it stalls, as a dropped link's would).
async fn open(addr: SocketAddr, cert: &Arc<CertManager>, name: &str, read: bool) -> Phone {
    let (mut client, mut frame_rx, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert.clone(),
        None,
        Some(name),
        None,
        None,
        SessionMode::Screen,
    )
    .await
    .expect("client connect");
    let connection = client.connection().cloned().expect("connected");
    let cancel = client.cancel_token();
    let receiving = tokio::spawn(async move { client.start().await });
    tokio::time::timeout(Duration::from_secs(15), frame_rx.recv())
        .await
        .expect("no frame in 15 s")
        .expect("the session ended before its first frame");
    let frames = Arc::new(AtomicUsize::new(0));
    if read {
        let count = frames.clone();
        tokio::spawn(async move {
            while frame_rx.recv().await.is_some() {
                count.fetch_add(1, Ordering::Relaxed);
            }
        });
    } else {
        // Held, unread: the session backs up behind it.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            drop(frame_rx);
        });
    }
    Phone {
        connection,
        cancel,
        receiving,
        frames,
    }
}

/// Whether `phone`'s session is still alive and still sending frames.
async fn still_alive(phone: &Phone) -> bool {
    let before = phone.frames.load(Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if phone.connection.close_reason().is_some() {
            return false;
        }
        // A still screen still sends a frame a second (WGC's heartbeat).
        if phone.frames.load(Ordering::Relaxed) > before {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// End a session as the phone does: close the connection, stop receiving.
async fn close(phone: Phone) {
    phone.connection.close(0, b"round over");
    phone.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), phone.receiving)
        .await
        .expect("the phone's receive loop did not stop")
        .expect("receive task panicked");
}

/// The server's live sessions reach none within a few seconds.
async fn server_ended_all() -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if pocketdesk_core::streaming::sessions::list().is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A log file the tracing output goes to.
#[derive(Clone)]
struct LogFile(Arc<Mutex<std::fs::File>>);

impl std::io::Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "captures the real screen and sound for minutes"]
async fn overlapping_sessions_never_bring_the_server_down() {
    let crash_dir = std::env::temp_dir().join(format!("pd-overlap-{}", std::process::id()));
    std::fs::create_dir_all(&crash_dir).unwrap();
    println!("crash reports and the log: {}", crash_dir.display());
    // As the server starts: its process setup, then the crash reporter.
    pocketdesk_server::platform::init_process();
    pocketdesk_server::crash_report::install(&crash_dir);
    let log_path = crash_dir.join("sessions.log");
    let log = LogFile(Arc::new(Mutex::new(
        std::fs::File::create(&log_path).unwrap(),
    )));
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_ansi(false)
        .with_writer(move || log.clone())
        .try_init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let rounds: usize = std::env::var("POCKETDESK_OVERLAP_ROUNDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(200);
    // The video settings the server streams with by default.
    let streaming_config = pocketdesk_core::local::config::Config::default()
        .video_quality
        .to_streaming_config();
    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert)
        .await
        .expect("bind streaming port");
    let addr = stream_server.local_addr().expect("bound address");

    // The server's side: a session per connection, as the QUIC loop does.
    let failed = Arc::new(AtomicUsize::new(0));
    let failed_count = failed.clone();
    let cert_for_sessions = cert.clone();
    tokio::spawn(async move {
        while let Some(incoming) = stream_server.accept_connection().await {
            let (cert, failed, config) = (
                cert_for_sessions.clone(),
                failed_count.clone(),
                streaming_config.clone(),
            );
            tokio::spawn(async move {
                let Ok(connecting) = incoming.accept() else {
                    return;
                };
                let Ok(connection) = connecting.await else {
                    return;
                };
                let mut server =
                    StreamingServer::new(config, StreamTransportConfig::default(), cert);
                if let Err(e) = server
                    .run_on_connection(QuinnConnection::shared(connection))
                    .await
                {
                    println!("a session ended with an error: {e}");
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });

    let start = Instant::now();
    for round in 0..rounds {
        let stalled = round % 3 == 2;
        let old = open(addr, &cert, "overlap-old", !stalled).await;
        // The reconnect: a new session while the old one still runs.
        let new = open(addr, &cert, "overlap-new", true).await;
        assert!(
            still_alive(&new).await,
            "round {round}: the new session died"
        );
        if !stalled {
            assert!(
                still_alive(&old).await,
                "round {round}: the old session died when the new one started"
            );
        }
        match round % 3 {
            0 => {
                // The old one goes first, as a reconnect's does.
                close(old).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                close(new).await;
            }
            1 => {
                // The new one goes first: a reconnect that failed after setup.
                close(new).await;
                close(old).await;
            }
            _ => {
                // The old link stopped reading; the new goes, then the old.
                close(new).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                close(old).await;
            }
        }
        assert!(
            server_ended_all().await,
            "round {round}: the server still runs {} sessions",
            pocketdesk_core::streaming::sessions::list().len()
        );
        if round % 20 == 19 {
            println!(
                "{} rounds in {:.0} s",
                round + 1,
                start.elapsed().as_secs_f64()
            );
        }
    }

    // Capture and sound threads finish after their sessions: let them.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let crash_log = crash_dir.join("crash.log");
    let crashed = std::fs::read_to_string(&crash_log).unwrap_or_default();
    assert!(crashed.is_empty(), "a crash was reported: {crashed}");
    assert_eq!(
        failed.load(Ordering::Relaxed),
        0,
        "sessions ended with an error"
    );

    // What ran: every session's capture, sound and encoder.
    let logged = std::fs::read_to_string(&log_path).unwrap();
    let count = |needle: &str| logged.lines().filter(|l| l.contains(needle)).count();
    let captures = count("Capture started via backend");
    println!(
        "{rounds} rounds, {} sessions in {:.0} s: capture started {captures} times ({} by WGC), \
         the sound's loopback {} times, the software encoder {} times, an FFmpeg encoder process {} times",
        rounds * 2,
        start.elapsed().as_secs_f64(),
        count("Capture started via backend Wgc"),
        count("WASAPI loopback capture started"),
        count("Video encoder: in-process FFmpeg (software)"),
        count("FFmpeg encoder process spawned"),
    );
    assert_eq!(captures, rounds * 2, "every session captured");
    let _ = std::fs::remove_dir_all(&crash_dir);
}
