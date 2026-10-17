//! R6g: sessions that overlap, as a reconnect makes them (the new session
//! starts before the old one has ended), against the real capture and the
//! real sound, round after round. The three crashes R6 saw came right after
//! a reconnected session's setup; this is the shape that made them. Each
//! connection gets a `StreamingServer` of its own, as the server's QUIC
//! loop gives it.
//!
//! Ignored: it captures this computer's screen and sound (over loopback;
//! nothing leaves the box) for minutes. Run it with
//! `cargo test -p pocketdesk-server --test overlapping_sessions -- --ignored --nocapture`;
//! `POCKETDESK_OVERLAP_ROUNDS` sets the rounds (200 by default). A crash
//! takes the test process with it and leaves its report in the folder the
//! test prints first.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, SessionMode, SharedConnection, StreamingClient, StreamingConfig,
    StreamingServer,
};
use tokio_util::sync::CancellationToken;

/// A phone's side of one session.
struct Phone {
    connection: SharedConnection,
    cancel: CancellationToken,
    receiving: tokio::task::JoinHandle<()>,
}

/// Open a screen session and wait for its first frame.
async fn open(addr: SocketAddr, cert: &Arc<CertManager>, name: &str) -> Phone {
    let (mut client, mut frames, _audio) = StreamingClient::connect(
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
    let receiving = tokio::spawn(async move {
        let _ = client.start().await;
    });
    tokio::time::timeout(Duration::from_secs(15), frames.recv())
        .await
        .expect("no frame in 15 s")
        .expect("the session ended before its first frame");
    // Keep the frames flowing while the other session starts.
    tokio::spawn(async move { while frames.recv().await.is_some() {} });
    Phone {
        connection,
        cancel,
        receiving,
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "captures the real screen and sound for minutes"]
async fn overlapping_sessions_never_bring_the_server_down() {
    let crash_dir = std::env::temp_dir().join(format!("pd-overlap-{}", std::process::id()));
    std::fs::create_dir_all(&crash_dir).unwrap();
    println!("crash reports, if any: {}", crash_dir.display());
    pocketdesk_server::crash_report::install(&crash_dir);
    let _ = rustls::crypto::ring::default_provider().install_default();

    let rounds: usize = std::env::var("POCKETDESK_OVERLAP_ROUNDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(200);
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
    let ended = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let (ended_count, failed_count) = (ended.clone(), failed.clone());
    let cert_for_sessions = cert.clone();
    tokio::spawn(async move {
        while let Some(incoming) = stream_server.accept_connection().await {
            let (cert, ended, failed) = (
                cert_for_sessions.clone(),
                ended_count.clone(),
                failed_count.clone(),
            );
            tokio::spawn(async move {
                let Ok(connecting) = incoming.accept() else {
                    return;
                };
                let Ok(connection) = connecting.await else {
                    return;
                };
                let mut server = StreamingServer::new(
                    StreamingConfig::default(),
                    StreamTransportConfig::default(),
                    cert,
                );
                if let Err(e) = server
                    .run_on_connection(QuinnConnection::shared(connection))
                    .await
                {
                    println!("a session ended with an error: {e}");
                    failed.fetch_add(1, Ordering::Relaxed);
                }
                ended.fetch_add(1, Ordering::Relaxed);
            });
        }
    });

    let start = Instant::now();
    for round in 0..rounds {
        let old = open(addr, &cert, "overlap-old").await;
        // The reconnect: a new session while the old one still runs.
        let new = open(addr, &cert, "overlap-new").await;
        if round % 2 == 0 {
            // The old one goes first, as a reconnect's does.
            close(old).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            close(new).await;
        } else {
            // The new one goes first: a reconnect that failed after setup.
            close(new).await;
            close(old).await;
        }
        if round % 20 == 19 {
            println!(
                "{} rounds in {:.0} s, {} sessions ended",
                round + 1,
                start.elapsed().as_secs_f64(),
                ended.load(Ordering::Relaxed)
            );
        }
    }

    // Every session the server ran ends.
    let deadline = Instant::now() + Duration::from_secs(30);
    while ended.load(Ordering::Relaxed) < rounds * 2 {
        assert!(
            Instant::now() < deadline,
            "{} of {} sessions ended",
            ended.load(Ordering::Relaxed),
            rounds * 2
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let crash_log = crash_dir.join("crash.log");
    assert!(
        !crash_log.exists() || std::fs::read_to_string(&crash_log).unwrap().is_empty(),
        "a crash was reported: {}",
        std::fs::read_to_string(&crash_log).unwrap_or_default()
    );
    println!(
        "{rounds} rounds, {} sessions, {} ended with an error, in {:.0} s",
        rounds * 2,
        failed.load(Ordering::Relaxed),
        start.elapsed().as_secs_f64()
    );
}
