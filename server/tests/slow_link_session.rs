//! R6e: a session on a slow link, and what ends it.
//!
//! On the A71, a Wi-Fi link that had fallen to about 1 Mbit/s made the
//! session reconnect every 30-60 s (checklist §84). Three timers can end a
//! session, and this says which one did:
//! - the phone's stall watch: 10 s with neither a frame nor a byte of video
//!   (`StallWatch.kt`), applied here to the client's own counters;
//! - the server's rule for a client unheard from for 30 s;
//! - QUIC's 45 s idle timeout.
//!
//! The real [`StreamingServer`] (desktop capture and encoder) runs over
//! loopback QUIC through the chaos proxy, whose return path, where the video
//! goes, is a link of `POCKETDESK_SLOW_LINK_KBPS` (default 1000) behind a
//! 64-packet router queue, losing `POCKETDESK_SLOW_LINK_LOSS_PERMILLE` (default
//! 0) of what comes back at random. It asserts what R6f is for: the session
//! lasts `POCKETDESK_SLOW_LINK_SECS` (default 300) and keeps at least 10
//! frames a second moving.
//!
//! First result (Windows, NVENC, 2026-10-17): at 1000 kbit/s and no random
//! loss the session held 300 s but showed 5-12 fps (worst 10 s, 5.2): QUIC's
//! congestion control kept the queue from overflowing (89 datagrams lost in
//! 5 minutes), so the loss controller, which acts on loss alone, held the
//! encoder at 6.4-8 Mbit/s and the transport dropped what the link could
//! not carry. With 3 % random loss too: no end, the loss controller at its
//! 1 Mbit/s floor, QUIC carrying about 300 kbit/s, about 18 fps. Neither
//! reproduces the A71's reconnects (owed: the VM with `tc`).
//!
//! With R6f's delivery ceiling (the transport's waits cap the rate), the
//! clean 1 Mbit/s link held 300 s at 12-24 fps (worst 10 s, 9.7), with 14
//! rate changes; the encoder overshot the lowest targets at 2560x1080.
//! With the frame rate stepping down too (60, 30, 15, then back to 30): it
//! settled within 100 s at 30 fps and about 480 kbit/s, nothing more lost at
//! the queue and no more changes, 18-20 fps delivered (what the screen
//! drew); worst 10 s window 10.5 fps.
//!
//! Ignored, since it captures the real desktop:
//!
//! ```text
//! cargo test -p pocketdesk-server --release --test slow_link_session -- --ignored --nocapture
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::chaos::proxy::{ChaosConfig, ChaosProxy, RateLimit};
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, StreamingClient, StreamingConfig, StreamingServer,
};

/// The phone's stall watch (`StallWatch.kt`): no frame and no byte for this
/// long and it drops the link.
const PHONE_STALL: Duration = Duration::from_secs(10);

/// The router queue in front of the slow hop, in packets.
const QUEUE_PACKETS: usize = 64;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "captures the real desktop and runs a real encoder for minutes"]
async fn a_session_on_a_slow_link_keeps_going() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "pocketdesk_core=info".into()),
        )
        .with_test_writer()
        .try_init();
    let kbps = env_u64("POCKETDESK_SLOW_LINK_KBPS", 1000);
    // Random loss on the way back, in tenths of a percent (weak Wi-Fi).
    let loss_permille = env_u64("POCKETDESK_SLOW_LINK_LOSS_PERMILLE", 0);
    let run_for = Duration::from_secs(env_u64("POCKETDESK_SLOW_LINK_SECS", 300));

    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert)
        .await
        .expect("bind streaming port");
    let server_addr = stream_server.local_addr().expect("bound address");

    let proxy = Arc::new(
        ChaosProxy::new(
            "127.0.0.1:0",
            &server_addr.to_string(),
            ChaosConfig {
                // The way there: a little delay, as on Wi-Fi.
                base_latency_ms: 10,
                downlink: Some(RateLimit {
                    bits_per_sec: kbps * 1000,
                    queue_packets: QUEUE_PACKETS,
                }),
                downlink_drop_rate: loss_permille as f64 / 1000.0,
                ..Default::default()
            },
        )
        .await
        .expect("proxy"),
    );
    let proxy_addr = proxy.local_addr().unwrap();
    let proxying = {
        let proxy = proxy.clone();
        tokio::spawn(async move { proxy.run().await })
    };

    let cert_for_session = cert.clone();
    let mut session = tokio::spawn(async move {
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
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });

    let (mut client, mut frames, _audio) = StreamingClient::connect(
        &proxy_addr.to_string(),
        cert.clone(),
        None,
        Some("slow-link-test"),
        None,
        None,
        pocketdesk_core::streaming::SessionMode::Screen,
    )
    .await
    .expect("client connect");
    let connection = client.connection().cloned().expect("connected handle");
    let bytes = client.video_bytes();
    let shutdown = client.cancel_token();
    let mut receiving = tokio::spawn(async move { client.start().await });

    let started = Instant::now();
    let mut last_progress = Instant::now();
    let mut last_bytes = 0u64;
    let (mut total_frames, mut window_frames) = (0u64, 0u64);
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;
    let mut worst_fps = f64::MAX;
    let (mut session_done, mut receiving_done) = (false, false);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let ended: Option<String> = loop {
        tokio::select! {
            frame = frames.recv() => match frame {
                Some(_) => {
                    total_frames += 1;
                    window_frames += 1;
                    last_progress = Instant::now();
                }
                None => break Some("the client's frame channel closed".into()),
            },
            result = &mut session, if !session_done => {
                session_done = true;
                break Some(format!("the server ended the session: {result:?}"));
            }
            result = &mut receiving, if !receiving_done => {
                receiving_done = true;
                break Some(format!(
                    "the client's receive loop ended ({result:?}); close reason {:?}",
                    connection.close_reason()
                ));
            }
            _ = tick.tick() => {
                let now_bytes = bytes.load(Ordering::Relaxed);
                if now_bytes != last_bytes {
                    last_bytes = now_bytes;
                    last_progress = Instant::now();
                }
                if last_progress.elapsed() >= PHONE_STALL {
                    break Some(format!(
                        "the phone's stall watch: no frame or byte for {PHONE_STALL:?}"
                    ));
                }
                if window_start.elapsed() >= Duration::from_secs(10) {
                    let secs = window_start.elapsed().as_secs_f64();
                    let fps = window_frames as f64 / secs;
                    let kbit = (now_bytes - window_bytes) as f64 * 8.0 / secs / 1000.0;
                    // The first windows hold the connect and the controllers
                    // finding the link; judge the rest.
                    if started.elapsed() > Duration::from_secs(30) {
                        worst_fps = worst_fps.min(fps);
                    }
                    println!(
                        "{:>4.0} s: {fps:>5.1} fps, {kbit:>6.0} kbit/s delivered, {} lost at the queue",
                        started.elapsed().as_secs_f64(),
                        proxy.downlink_lost(),
                    );
                    window_start = Instant::now();
                    window_frames = 0;
                    window_bytes = now_bytes;
                }
                if started.elapsed() >= run_for {
                    break None;
                }
            }
        }
    };

    println!(
        "at {kbps} kbit/s, {loss_permille} per mille lost: {} after {:.0} s, {total_frames} frames, worst 10 s window {worst_fps:.1} fps",
        ended.as_deref().unwrap_or("still running"),
        started.elapsed().as_secs_f64(),
    );
    connection.close(0, b"test complete");
    shutdown.cancel();
    if !receiving_done {
        let _ = tokio::time::timeout(Duration::from_secs(10), receiving).await;
    }
    if !session_done {
        let _ = tokio::time::timeout(Duration::from_secs(20), session).await;
    }
    proxy.stop();
    let _ = proxying.await;

    assert!(ended.is_none(), "the session ended: {}", ended.unwrap());
    assert!(
        worst_fps >= 10.0,
        "the picture nearly stopped: {worst_fps:.1} fps in the worst 10 s"
    );
}
