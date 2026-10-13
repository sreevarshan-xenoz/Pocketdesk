//! Phase 1's measurement gate: one command produces a session record holding
//! p50/p90/p95/p99/max for latency, encode *and* decode.
//!
//! Every other test of this chain checks one seam in isolation — a sample
//! frame's wire format here, the recorder's arithmetic there. This one drives a
//! whole session: the real [`StreamingServer`] (desktop capture, encoder,
//! telemetry) over loopback QUIC against the real [`StreamingClient`], with the
//! client reporting the durations its decoder measured. What the server writes
//! out when the session ends is what is under test, so the tails in the record
//! are known to be reachable from a live link rather than assumed.
//!
//! Needs a real desktop to capture and an encoder to run on, so it is ignored by
//! default:
//!
//! ```text
//! cargo test -p pocketdesk-server --test session_record -- --ignored --nocapture
//! ```

use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;

use pocketdesk_core::streaming::transport::{CertManager, StreamServer, StreamTransportConfig};
use pocketdesk_core::streaming::{
    QuinnConnection, SAMPLE_DECODE, SAMPLE_RENDER, SampleBatch, SessionReport, StreamingClient,
    StreamingConfig, StreamingServer, report_samples, set_session_telemetry_callback,
};

/// Whatever the session recorder emits while this test runs. The sink is
/// process-global, which is fine for a binary with one test in it.
static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The server's path/RTT sampler period (see streamer.rs task 7). The report
/// drops `rtt_*` and the link block — including the client's own `phone_rtt` —
/// when that sampler never ran, so the session has to outlive one tick.
const TELEMETRY_POLL_SECS: u64 = 5;

#[tokio::test]
#[ignore = "captures the real desktop and runs a real encoder"]
async fn a_live_session_records_every_measured_stage() {
    set_session_telemetry_callback(|report: &SessionReport| {
        RECORDS.lock().unwrap().push(report.format());
    });

    let cert = std::sync::Arc::new(CertManager::new().expect("cert manager"));

    // Port 0: the shipping service already owns 4716 on this box.
    let listen = StreamTransportConfig {
        address: "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
        ..StreamTransportConfig::default()
    };
    let stream_server = StreamServer::new(listen, &cert)
        .await
        .expect("bind streaming port");
    let addr = stream_server.local_addr().expect("bound address");

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
        server.set_session_telemetry(true);
        server
            .run_on_connection(QuinnConnection::shared(connection))
            .await
    });

    let (mut client, mut frames, _audio) = StreamingClient::connect(
        &addr.to_string(),
        cert.clone(),
        None,
        Some("loopback-test"),
        None,
        None,
    )
    .await
    .expect("client connect");
    let connection = client.connection().cloned().expect("connected handle");
    // `start()` consumes the client, so hold the token `stop()` would fire:
    // that is the supported way to end a session. Closing the connection under
    // a running `start()` does NOT end it — observed here as a hang before this
    // line existed, and the same reason the phone needs a frame-gap watchdog to
    // notice a desktop that went away.
    let shutdown = client.cancel_token();
    let receiving = tokio::spawn(async move { client.start().await });

    // Frames arriving is the proof the encode side ran, which is what makes the
    // `enc_*` tail below meaningful rather than merely present.
    let mut got = 0usize;
    for _ in 0..12 {
        match tokio::time::timeout(Duration::from_secs(10), frames.recv()).await {
            Ok(Some(_packet)) => got += 1,
            Ok(None) | Err(_) => break,
        }
    }
    assert!(got > 0, "the session delivered no video frames");

    // The decoder's own numbers, exactly as H264Decoder.kt hands them over:
    // microseconds, one batch per kind.
    for (kind, values) in [
        (SAMPLE_DECODE, [1_200u64, 1_800, 2_400, 31_000]),
        (SAMPLE_RENDER, [33_000u64, 34_000, 40_000, 61_000]),
    ] {
        let batch = SampleBatch::new();
        for value in values {
            assert!(batch.push(value), "sample rejected");
        }
        let sent = tokio::time::timeout(
            Duration::from_secs(5),
            report_samples(&connection, kind, &batch),
        )
        .await
        .expect("report_samples hung");
        assert!(
            sent,
            "sample frame for kind {kind} did not reach the server"
        );
    }

    // Outlive a telemetry tick so the server samples the path, and give the
    // client's one-second feedback loop time to flush its e2e samples.
    tokio::time::sleep(Duration::from_secs(TELEMETRY_POLL_SECS + 3)).await;

    connection.close(0, b"test complete");
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(10), receiving)
        .await
        .expect("the client's receive loop did not stop on cancel")
        .expect("receive task panicked");
    tokio::time::timeout(Duration::from_secs(20), &mut session)
        .await
        .expect("the server pipeline never finished")
        .expect("session task died")
        .expect("streaming session failed");

    let records = RECORDS.lock().unwrap();
    assert_eq!(records.len(), 1, "expected exactly one session record");
    let line = &records[0];

    // "latency" is both halves of it: `rtt_*` is the transport round trip the
    // server sampled, `e2e_*` the capture→display estimate the client measured
    // and reported back. `phone_*` is the client's own reading of the path.
    for key in [
        "rtt_p50=",
        "rtt_p90=",
        "rtt_p95=",
        "rtt_p99=",
        "rtt_max=",
        "enc_p50=",
        "enc_p90=",
        "enc_p95=",
        "enc_p99=",
        "enc_max=",
        "dec_p50=",
        "dec_p90=",
        "dec_p95=",
        "dec_p99=",
        "dec_max=",
        "rnd_p50=",
        "e2e_p50=",
        "phone_rtt=",
    ] {
        assert!(line.contains(key), "record is missing {key}: {line}");
    }
    println!("session record: {line}");
}
