//! BG: how smoothly the picture moves on a link that loses packets.
//!
//! The real [`StreamingServer`] (its encoder) runs over loopback QUIC through
//! the chaos proxy, with the test pattern standing in for the screen
//! (`POCKETDESK_TEST_PATTERN`), so every frame differs and no one's desktop is
//! read. The return path, where the video goes, is a link of
//! `POCKETDESK_LOSSY_KBPS` (default 20000) behind a 64-packet router queue,
//! losing `POCKETDESK_LOSSY_PERMILLE` (default 20, 2 %) of what comes back,
//! with `POCKETDESK_LOSSY_DELAY_MS` (default 10) each way. It runs for
//! `POCKETDESK_LOSSY_SECS` (default 60) and prints how evenly frames arrived
//! (the gap between frames: p50, p95, p99, max, and stalls of 200 ms or
//! more) and the session's record: its freezes (BG1), the capture-to-arrival
//! tail (e2e), the frames trimmed and the captures replaced.
//!
//! A measuring stick for the BG items, not a pass or fail on smoothness: it
//! asserts only that frames kept coming. Run it before and after a change:
//!
//! ```text
//! cargo test -p pocketdesk-server --release --test lossy_link_smoothness -- --ignored --nocapture
//! ```

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::chaos::proxy::{ChaosConfig, ChaosProxy, RateLimit};
use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, StreamingClient, StreamingConfig, StreamingServer,
};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The value at quantile `q` of sorted `values`.
fn quantile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs a real encoder for a minute; a measuring stick, run by hand"]
async fn how_smoothly_frames_arrive_on_a_lossy_link() {
    // SAFETY: this test binary has this one test, so nothing else reads the
    // environment while it is set (the runtime's workers are idle here).
    unsafe { std::env::set_var(pocketdesk_core::streaming::test_pattern::VAR, "1") };
    let state = std::env::temp_dir().join(format!("pd-lossy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();
    // Gone however the bench ends, a panic too.
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _remove = Remove(state.clone());
    pocketdesk_core::local::paths::use_state_dir(state.clone());
    pocketdesk_server::session_telemetry::init();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "pocketdesk_core=warn".into()),
        )
        .with_test_writer()
        .try_init();

    let kbps = env_u64("POCKETDESK_LOSSY_KBPS", 20_000);
    let permille = env_u64("POCKETDESK_LOSSY_PERMILLE", 20);
    let delay_ms = env_u64("POCKETDESK_LOSSY_DELAY_MS", 10);
    let run_for = Duration::from_secs(env_u64("POCKETDESK_LOSSY_SECS", 60));

    let cert = Arc::new(CertManager::new().expect("cert manager"));
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert).await.expect("bind");
    let server_addr = stream_server.local_addr().expect("bound address");
    let proxy = Arc::new(
        ChaosProxy::new(
            "127.0.0.1:0",
            &server_addr.to_string(),
            ChaosConfig {
                base_latency_ms: delay_ms,
                downlink: Some(RateLimit {
                    bits_per_sec: kbps * 1000,
                    queue_packets: 64,
                }),
                downlink_drop_rate: permille as f64 / 1000.0,
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
    let session = tokio::spawn(async move {
        let connection = stream_server
            .accept_connection()
            .await
            .expect("no client")
            .accept()
            .expect("rejected")
            .await
            .expect("handshake");
        let mut server = StreamingServer::new(
            StreamingConfig::default(),
            StreamTransportConfig::default(),
            cert_for_session,
        );
        // Its record, for the freezes and the e2e tail.
        server.set_session_telemetry(true);
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });

    let (mut client, mut frames, _audio) = StreamingClient::connect(
        &proxy_addr.to_string(),
        cert.clone(),
        None,
        Some("lossy-link-bench"),
        None,
        None,
        pocketdesk_core::streaming::SessionMode::Screen,
    )
    .await
    .expect("client connect");
    let connection = client.connection().cloned().expect("connected");
    let shutdown = client.cancel_token();
    let receiving = tokio::spawn(async move { client.start().await });

    // The first frames come while the encoder and the link settle.
    let settle = Duration::from_secs(5);
    let started = Instant::now();
    let mut last: Option<Instant> = None;
    let mut gaps_ms: Vec<u64> = Vec::new();
    let mut total = 0u64;
    while started.elapsed() < run_for {
        match tokio::time::timeout(Duration::from_secs(10), frames.recv()).await {
            Ok(Some(_)) => {
                total += 1;
                let now = Instant::now();
                if started.elapsed() >= settle
                    && let Some(previous) = last
                {
                    gaps_ms.push(now.duration_since(previous).as_millis() as u64);
                }
                last = Some(now);
            }
            Ok(None) => panic!("the client's frame channel closed"),
            Err(_) => panic!("no frame for 10 s"),
        }
    }
    connection.close(0, b"bench complete");
    shutdown.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(10), receiving).await;
    let ended = tokio::time::timeout(Duration::from_secs(20), session).await;
    println!(
        "the server's session ended: {}",
        if ended.is_ok() {
            "yes"
        } else {
            "no, within 20 s"
        }
    );
    proxy.stop();
    let _ = proxying.await;

    gaps_ms.sort_unstable();
    let stalls = gaps_ms.iter().filter(|g| **g >= 200).count();
    let secs = (run_for - settle).as_secs_f64();
    println!(
        "{kbps} kbit/s, {permille} per mille lost, {delay_ms} ms each way, {:.0} s: {total} frames, {:.1} fps after settling",
        run_for.as_secs_f64(),
        gaps_ms.len() as f64 / secs,
    );
    println!(
        "gap between frames: p50 {} ms, p95 {} ms, p99 {} ms, max {} ms; stalls of 200 ms or more: {stalls}",
        quantile(&gaps_ms, 0.50),
        quantile(&gaps_ms, 0.95),
        quantile(&gaps_ms, 0.99),
        gaps_ms.last().copied().unwrap_or(0),
    );
    // The session's record, as `pocketdesk sessions` keeps it, written as
    // the session ends.
    let log_path = state.join("streaming_sessions.log");
    let waited = Instant::now();
    while !log_path.exists() && waited.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    if log.is_empty() {
        let names: Vec<String> = std::fs::read_dir(&state)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        println!("no record in {} ({names:?})", state.display());
    }
    if let Some(line) = log.lines().last() {
        let wanted = [
            "freeze_n=",
            "freeze_p50=",
            "freeze_max=",
            "e2e_p50=",
            "e2e_p95=",
            "e2e_p99=",
            "trimmed=",
            "replaced=",
            "encoder=",
        ];
        let kept: Vec<&str> = line
            .split('\t')
            .filter(|f| wanted.iter().any(|w| f.starts_with(w)))
            .collect();
        println!("record: {}", kept.join(" "));
    } else {
        println!("record: none written");
    }
    assert!(total > 0, "no frame arrived");
}
