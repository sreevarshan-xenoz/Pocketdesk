//! R4 A2 — per-stream-session outcome telemetry.
//!
//! Every video pipeline run is observed by a [`SessionRecorder`] and written
//! as one `key=value` line through a host-registered sink (the server appends
//! to a rotating file under its state dir). This is the instrument the relay
//! research says we need: hole punching succeeds ~70% of the time even after
//! prerequisites, so *how* sessions actually connect on real networks —
//! LAN, punched-direct, or riding a relay — must be measured, not guessed.
//!
//! Wire-level goodput (bytes the transport actually pushed) is logged, not
//! encoder output: backlog trims make those numbers differ, and the transport
//! number is what the user experiences.
//!
//! A record also carries what the transport itself reported — loss, the widest
//! congestion window, the path MTU, how many times the selected path moved, and
//! how long a relay it rode — because "that session was slow" only becomes
//! actionable once it can be blamed on a layer. Those are the library's numbers,
//! and where iroh does not expose one it is absent from the line rather than
//! reported as zero.
//!
//! The last stretch of the chain — decode, render, and the client's own view of
//! the link — happens on the other end of the wire and is invisible from here,
//! so the client ships those measurements back over the streaming connection and
//! this recorder turns them into the same kind of distribution as the encode
//! tail. A record whose client never reported is missing those keys entirely.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::metrics::{Samples, Summary};

use super::connection::{ConnectionStats, SharedConnection, TransportFamily};

/// Terminal classification of one streaming session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// Normal end of a pipeline run — graded against transport family and
    /// observed paths by [`SessionRecorder::finish`] (never logged as-is).
    Completed,
    /// Quinn (LAN / Tailscale-routable) connection — always direct.
    LanDirect,
    /// iroh WAN whose sampled paths were all direct (punched).
    WanPunched,
    /// iroh WAN that rode a relay at some point during the session.
    WanRelayed,
    /// Rejected before the pipeline started (e.g. pairing gate).
    Rejected,
    /// Pipeline torn down by an error (capture/encoder/transport failure).
    Failed,
}

impl SessionOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionOutcome::Completed => "completed",
            SessionOutcome::LanDirect => "lan_direct",
            SessionOutcome::WanPunched => "wan_punched",
            SessionOutcome::WanRelayed => "wan_relayed",
            SessionOutcome::Rejected => "rejected",
            SessionOutcome::Failed => "failed",
        }
    }
}

/// One log line, already classified. Built by [`SessionRecorder::finish`].
///
/// Also the retained record: the server writes each report as one JSON line in
/// `streaming_sessions.jsonl` so a later run can be compared against an earlier
/// one (`pocketdesk sessions --json`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionReport {
    pub unix_secs: u64,
    pub outcome: SessionOutcome,
    /// True if any sample during the session rode a relay, even if the final
    /// one was direct — the punching-completes-mid-session case.
    pub ever_relayed: bool,
    pub duration_secs: u64,
    pub rtt_avg_ms: u64,
    pub goodput_kbps: u64,
    pub device_id: Option<String>,
    /// Link RTT distribution, or `None` when the session never sampled the
    /// transport (immediate teardown).
    pub rtt_tail: Option<Summary>,
    /// Per-frame encode time distribution, or `None` when no frame was encoded.
    pub encode_tail: Option<Summary>,
    /// How long each encoder rebuild took (a crop's new size, a bitrate, a
    /// frame rate), or `None` when there was none (AH2): the stall a rebuild
    /// makes is outside `encode_tail`, which times one frame's encode.
    pub rebuild_tail: Option<Summary>,
    /// Client-side distributions, reported back over the streaming connection
    /// by the device doing the decoding. All three are `None` for a session
    /// whose client never sent a sample — which includes every desktop-to-desktop
    /// client today, so `None` here is "nobody measured", never "instant".
    ///
    /// They live on the server's record because the *server* holds the
    /// reservoir that computes the percentiles: the client only ships batches
    /// of durations, so a phone's decode tail is built by the same code that
    /// built the encode tail and is comparable with it.
    ///
    /// - `decode_tail`: MediaCodec feed → drained-frame time per frame.
    /// - `render_tail`: gap between successive frames reaching the panel.
    /// - `e2e_tail`: the R4 E3 compositor-true probe sample (capture → arrival,
    ///   desktop clock plus one network leg), which is the number the HUD shows
    ///   smoothed. Decode and render sit *after* this and are not in it.
    pub decode_tail: Option<Summary>,
    pub render_tail: Option<Summary>,
    pub e2e_tail: Option<Summary>,
    /// What the transport itself said about the link, or `None` for a session
    /// that never got far enough to sample one.
    pub link: Option<LinkReport>,
    /// The encoder that produced the session's frames
    /// (`VideoEncoder::backend_name`), the last one if it changed (a
    /// fallback). `None` when no frame was encoded.
    pub encoder: Option<String>,
}

/// The transport's own account of a session's link, read from quinn or iroh
/// rather than measured by us.
///
/// This exists so "it was slow" can be settled after the fact: an encode tail
/// that moved with a congestion window that closed is a bandwidth problem, and
/// one that moved with an untouched window is not. The `Option` fields are
/// `None` on a WAN session because iroh's connection-level statistics sum the
/// byte counters across paths and drop the per-path ones outright — see
/// [`ConnectionStats`]. A key that is `None` is omitted from the log line
/// entirely, so `key=0` always means "measured zero" and a missing key always
/// means "nobody looked".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LinkReport {
    /// Packets and bytes the transport declared lost over the whole connection.
    pub lost_packets: u64,
    pub lost_bytes: u64,
    /// Datagram and byte totals the transport moved, which is what the link
    /// actually carried (headers, probes and retransmits included) rather than
    /// what the encoder handed it.
    pub datagrams_sent: u64,
    pub bytes_sent: u64,
    /// Times the selected path's peer address changed mid-session: a QUIC
    /// migration, or an iroh relay path giving way to a punched direct one.
    pub path_changes: u32,
    /// Seconds spent riding a relay, from the wall clock between observations.
    /// Bounded by the sampling interval on one sample's worth of slop; a
    /// session that never sampled reports no link at all rather than zero.
    pub relayed_secs: u64,
    pub congestion_events: Option<u64>,
    /// Widest congestion window seen, not the last one: the window collapses on
    /// loss and creeps back, so the closing sample is the least informative.
    /// `None` when the transport never reported one, which is not the same
    /// claim as a window of zero bytes.
    pub peak_cwnd_bytes: Option<u64>,
    /// Largest UDP payload the path was found to carry.
    pub path_mtu: Option<u16>,
    pub black_holes_detected: Option<u64>,
    /// RTT as the *client* measured it on the last feedback frame, ms. The
    /// server samples its own end every few seconds; this is the far end's
    /// reading of the same path. `None` means no feedback arrived — and like
    /// every other field here, it is absent whenever the session never sampled
    /// the transport, since that drops the whole block.
    pub client_rtt_ms: Option<u64>,
    /// Packets the client saw lost on *receive*, cumulative. Loss the sender's
    /// statistics cannot see at all — the reason this number is worth a frame.
    pub client_lost_packets: Option<u64>,
}

/// The client's own view of the link, from a [`InputPacket::LinkFeedback`] frame.
///
/// [`InputPacket::LinkFeedback`]: super::input_packet::InputPacket::LinkFeedback
#[derive(Debug, Clone, Copy)]
struct ClientLink {
    rtt_ms: u64,
    lost_packets: u64,
}

impl LinkReport {
    fn from_observed(
        stats: &ConnectionStats,
        peak_cwnd_bytes: Option<u64>,
        relayed_secs: u64,
        path_changes: u32,
        client: Option<ClientLink>,
    ) -> Self {
        Self {
            lost_packets: stats.lost_packets,
            lost_bytes: stats.lost_bytes,
            datagrams_sent: stats.datagrams_sent,
            bytes_sent: stats.bytes_sent,
            path_changes,
            relayed_secs,
            congestion_events: stats.congestion_events,
            peak_cwnd_bytes,
            path_mtu: stats.path_mtu,
            black_holes_detected: stats.black_holes_detected,
            client_rtt_ms: client.map(|c| c.rtt_ms),
            client_lost_packets: client.map(|c| c.lost_packets),
        }
    }

    /// The `key=value` tail of one log line, tab-prefixed and empty when there
    /// is nothing to say.
    fn format(&self) -> String {
        use std::fmt::Write as _;
        let mut tail = String::with_capacity(96);
        write!(
            tail,
            "\tlost_pk={}\tlost_b={}\tdgrams={}\ttx_b={}\tpath_chg={}\trelayed_s={}",
            self.lost_packets,
            self.lost_bytes,
            self.datagrams_sent,
            self.bytes_sent,
            self.path_changes,
            self.relayed_secs,
        )
        .expect("infallible");
        for (key, value) in [
            ("cong_ev", self.congestion_events.map(|v| v.to_string())),
            ("cwnd_pk", self.peak_cwnd_bytes.map(|v| v.to_string())),
            ("mtu", self.path_mtu.map(|v| v.to_string())),
            ("black", self.black_holes_detected.map(|v| v.to_string())),
            ("phone_rtt", self.client_rtt_ms.map(|v| v.to_string())),
            (
                "phone_lost",
                self.client_lost_packets.map(|v| v.to_string()),
            ),
        ] {
            if let Some(value) = value {
                write!(tail, "\t{key}={value}").expect("infallible");
            }
        }
        tail
    }
}

impl SessionReport {
    /// Machine- and human-friendly single line, tab-separated `k=v` fields.
    pub fn format(&self) -> String {
        use std::fmt::Write as _;
        let mut line = String::with_capacity(160);
        write!(
            line,
            "ts={}\toutcome={}\tever_relayed={}\tdur={}\trtt_ms={}\tkbps={}",
            self.unix_secs,
            self.outcome.as_str(),
            self.ever_relayed,
            self.duration_secs,
            self.rtt_avg_ms,
            self.goodput_kbps,
        )
        .expect("writing to String cannot fail");
        if let Some(tail) = &self.rtt_tail {
            write!(line, "\t{}", tail.format_tail("rtt")).expect("infallible");
        }
        if let Some(tail) = &self.encode_tail {
            write!(line, "\t{}", tail.format_tail("enc")).expect("infallible");
        }
        if let Some(tail) = &self.rebuild_tail {
            write!(line, "\t{}", tail.format_tail("rebuild")).expect("infallible");
        }
        for (tail, key) in [
            (self.decode_tail.as_ref(), "dec"),
            (self.render_tail.as_ref(), "rnd"),
            (self.e2e_tail.as_ref(), "e2e"),
        ] {
            if let Some(tail) = tail {
                write!(line, "\t{}", tail.format_tail(key)).expect("infallible");
            }
        }
        if let Some(link) = &self.link {
            write!(line, "{}", link.format()).expect("infallible");
        }
        if let Some(encoder) = &self.encoder {
            write!(line, "\tencoder={encoder}").expect("infallible");
        }
        if let Some(id) = &self.device_id {
            // Defensive: ids are UTF-8 device names from the pairing layer.
            let id = id.replace(['\t', '\n'], " ");
            write!(line, "\tdev={id}").expect("infallible");
        }
        line
    }
}

type Sink = std::sync::Arc<dyn Fn(&SessionReport) + Send + Sync + 'static>;

/// The path history one session accumulated, kept off the hot atomics because
/// it is touched once per sample and needs several values to agree. No
/// `Debug`: the connection handle inside it is not printable, and the fields
/// are all in the report.
#[derive(Default)]
struct PathObserver {
    /// The connection last sampled, so teardown can take a final reading.
    connection: Option<SharedConnection>,
    last_stats: Option<ConnectionStats>,
    last_address: Option<SocketAddr>,
    /// When the current relay ride began, `None` while the path is direct.
    relayed_since: Option<Instant>,
    path_changes: u32,
    relayed_us: u64,
    peak_cwnd_bytes: Option<u64>,
}

impl PathObserver {
    /// Fold one transport reading in. `at` is a parameter rather than
    /// `Instant::now()` so the relay accounting can be tested on synthetic
    /// timestamps instead of by sleeping.
    fn observe(&mut self, stats: &ConnectionStats, address: SocketAddr, at: Instant) {
        if self
            .last_address
            .is_some_and(|previous| previous != address)
        {
            self.path_changes += 1;
        }
        self.last_address = Some(address);

        match (stats.relayed, self.relayed_since) {
            (true, None) => self.relayed_since = Some(at),
            (false, Some(since)) => {
                self.relayed_us += at.saturating_duration_since(since).as_micros() as u64;
                self.relayed_since = None;
            }
            _ => {}
        }

        if let Some(cwnd) = stats.cwnd_bytes {
            self.peak_cwnd_bytes = Some(self.peak_cwnd_bytes.map_or(cwnd, |peak| peak.max(cwnd)));
        }
        self.last_stats = Some(*stats);
    }

    /// Close any relay ride still open at `at`, then report the total seconds.
    fn relayed_secs(&mut self, at: Instant) -> u64 {
        if let Some(since) = self.relayed_since.take() {
            self.relayed_us += at.saturating_duration_since(since).as_micros() as u64;
        }
        self.relayed_us / 1_000_000
    }
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Register the process-wide report sink. First caller wins; unset means
/// telemetry is off (every recorder becomes a no-op) — the default for
/// clients and tests that never wire a log up.
pub fn set_session_telemetry_callback<F>(f: F)
where
    F: Fn(&SessionReport) + Send + Sync + 'static,
{
    let _ = SINK.set(std::sync::Arc::new(f));
}

fn sink() -> Option<&'static Sink> {
    SINK.get()
}

/// Live recorder for one pipeline run: passively observes the connection
/// (via the trait's `stats()`), accumulates RTT samples, and emits the
/// report when [`SessionGuard`] is dropped. Cheap enough to poll from any
/// long-running task in the pipeline.
pub struct SessionRecorder {
    started: Instant,
    ever_relayed: AtomicU64,
    rtt_sum_us: AtomicU64,
    rtt_samples: AtomicU64,
    rtt_tail: Samples,
    encode_tail: Samples,
    rebuild_tail: Samples,
    /// Client-reported distributions, filled from `ClientSamples` frames by the
    /// connection monitor. A `Samples` is already internally locked, so these
    /// need no wrapper and the recorder stays `&self` on every path.
    decode_tail: Samples,
    render_tail: Samples,
    e2e_tail: Samples,
    /// The client's last link-feedback frame, if one ever arrived.
    client_link: Mutex<Option<ClientLink>>,
    observed: Mutex<PathObserver>,
    family: TransportFamily,
    device_id: Option<String>,
    /// The encoder last noted, see [`SessionRecorder::note_encoder`].
    encoder: Mutex<Option<&'static str>>,
}

impl SessionRecorder {
    pub fn new(family: TransportFamily, device_id: Option<String>) -> Self {
        Self {
            started: Instant::now(),
            ever_relayed: AtomicU64::new(0),
            rtt_sum_us: AtomicU64::new(0),
            rtt_samples: AtomicU64::new(0),
            rtt_tail: Samples::new(),
            encode_tail: Samples::new(),
            rebuild_tail: Samples::new(),
            decode_tail: Samples::new(),
            render_tail: Samples::new(),
            e2e_tail: Samples::new(),
            client_link: Mutex::new(None),
            observed: Mutex::new(PathObserver::default()),
            family,
            device_id,
            encoder: Mutex::new(None),
        }
    }

    /// The encoder now producing frames (`VideoEncoder::backend_name`); the
    /// last one noted is the one the record names. The encode task calls it
    /// only when the encoder changes.
    pub fn note_encoder(&self, name: &'static str) {
        *self.encoder.lock().unwrap_or_else(|p| p.into_inner()) = Some(name);
    }

    /// Observe the connection once. Call every few seconds from a pipeline
    /// task; sessions that never sample (immediate teardown) still log, with
    /// zero-valued samples and no link report.
    pub fn sample(&self, connection: &SharedConnection) {
        // Read the transport outside the lock: `stats()` takes quinn's own
        // connection lock, and a connection that is stalling on its way out
        // must not be able to block whoever holds ours.
        let stats = connection.stats();
        let address = connection.remote_address();
        let now = Instant::now();
        if stats.relayed {
            self.ever_relayed.store(1, Ordering::Relaxed);
        }
        self.rtt_sum_us
            .fetch_add(stats.rtt.as_micros() as u64, Ordering::Relaxed);
        self.rtt_samples.fetch_add(1, Ordering::Relaxed);
        self.rtt_tail.push_micros(stats.rtt.as_micros() as u64);
        let mut observed = self.observed.lock().unwrap_or_else(|p| p.into_inner());
        observed.connection = Some(connection.clone());
        observed.observe(&stats, address, now);
    }

    /// Time one `encode_frame` call. Called per frame from the encode task, so
    /// it must stay cheap: a mutex over a bounded array, no allocation.
    pub fn record_encode(&self, elapsed: Duration) {
        self.encode_tail.push_micros(elapsed.as_micros() as u64);
    }

    /// Time one encoder rebuild (AH2), kept apart from the frames' encodes.
    pub fn record_rebuild(&self, elapsed: Duration) {
        self.rebuild_tail.push_micros(elapsed.as_micros() as u64);
    }

    /// Fold one batch of client-measured durations into the distribution named
    /// by `kind` (a `SAMPLE_*` id). Returns `false` for a kind this record has
    /// no place for — a newer client than server — so the caller can say so
    /// rather than dropping the batch silently.
    pub fn record_client_samples(&self, kind: u8, values: &[u32]) -> bool {
        let tail = match kind {
            super::input_packet::SAMPLE_DECODE => &self.decode_tail,
            super::input_packet::SAMPLE_RENDER => &self.render_tail,
            super::input_packet::SAMPLE_E2E => &self.e2e_tail,
            _ => return false,
        };
        for &micros in values {
            tail.push_micros(u64::from(micros));
        }
        true
    }

    /// Take the client's own reading of the link: the far end's measurement of
    /// the same path, plus a received-loss count that is not observable from
    /// here at all.
    pub fn record_client_link(&self, rtt: Duration, lost_packets: u64) {
        *self.client_link.lock().unwrap_or_else(|p| p.into_inner()) = Some(ClientLink {
            rtt_ms: rtt.as_millis() as u64,
            lost_packets,
        });
    }

    /// The transport's account of the link, or `None` if it was never sampled.
    ///
    /// A session's last seconds are where a stall lives, and the pipeline only
    /// polls every few of them, so the recorder re-reads the handle it holds
    /// instead of reporting the link as it looked at the previous poll.
    fn link_report(&self) -> Option<LinkReport> {
        // Read the transport before taking the lock again, exactly as `sample`
        // does: `stats()` reaches into quinn's own lock.
        let connection = self
            .observed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .connection
            .clone()?;
        let stats = connection.stats();
        let address = connection.remote_address();
        let now = Instant::now();
        let client = *self.client_link.lock().unwrap_or_else(|p| p.into_inner());
        let mut observed = self.observed.lock().unwrap_or_else(|p| p.into_inner());
        observed.observe(&stats, address, now);
        let relayed_secs = observed.relayed_secs(now);
        let last = observed.last_stats?;
        Some(LinkReport::from_observed(
            &last,
            observed.peak_cwnd_bytes,
            relayed_secs,
            observed.path_changes,
            client,
        ))
    }

    pub fn finish(&self, outcome: SessionOutcome, bytes_sent: u64) -> SessionReport {
        let duration = self.started.elapsed();
        let duration_secs = duration.as_secs();
        let samples = self.rtt_samples.load(Ordering::Relaxed).max(1);
        let rtt_avg_ms = self.rtt_sum_us.load(Ordering::Relaxed) / samples / 1000;
        let goodput_kbps = (bytes_sent * 8 / 1000)
            .checked_div(duration_secs)
            .unwrap_or(0);
        let ever_relayed = self.ever_relayed.load(Ordering::Relaxed) == 1;
        SessionReport {
            unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            outcome: self.classify(outcome),
            ever_relayed,
            duration_secs,
            rtt_avg_ms,
            goodput_kbps,
            device_id: self.device_id.clone(),
            rtt_tail: self.rtt_tail.summary_ms(),
            encode_tail: self.encode_tail.summary_ms(),
            rebuild_tail: self.rebuild_tail.summary_ms(),
            decode_tail: self.decode_tail.summary_ms(),
            render_tail: self.render_tail.summary_ms(),
            e2e_tail: self.e2e_tail.summary_ms(),
            link: self.link_report(),
            encoder: self
                .encoder
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(str::to_string),
        }
    }

    /// Normalise a pipeline outcome against what was actually observed:
    /// a completed run is graded by transport family (relaying is an iroh
    /// concept — a quinn session is always direct) and by whether a relay
    /// path was ever selected. Rejections/failures are reported as-is.
    fn classify(&self, outcome: SessionOutcome) -> SessionOutcome {
        if outcome != SessionOutcome::Completed {
            return outcome;
        }
        match self.family {
            TransportFamily::Quinn => SessionOutcome::LanDirect,
            TransportFamily::Iroh => {
                if self.ever_relayed.load(Ordering::Relaxed) == 1 {
                    SessionOutcome::WanRelayed
                } else {
                    SessionOutcome::WanPunched
                }
            }
        }
    }

    /// Arm end-of-session logging for one pipeline run. Drop the guard at
    /// every exit point of `run_pipeline`; the guard defaults to
    /// [`SessionOutcome::Failed`] unless the pipeline marks completion (via
    /// [`SessionGuard::mark_completed`]) or records an explicit terminal
    /// outcome (e.g. the pairing-gate rejection).
    pub fn guard(&self, bytes_sent: Arc<AtomicU64>) -> SessionGuard<'_> {
        SessionGuard {
            recorder: self,
            bytes_sent,
            outcome: std::sync::Mutex::new(SessionOutcome::Failed),
            emitted: AtomicBool::new(false),
        }
    }
}

/// Guard that emits the report when the pipeline function returns.
pub struct SessionGuard<'a> {
    recorder: &'a SessionRecorder,
    bytes_sent: Arc<AtomicU64>,
    outcome: std::sync::Mutex<SessionOutcome>,
    emitted: AtomicBool,
}

impl SessionGuard<'_> {
    /// The pipeline reached its normal join/teardown path; the logged
    /// outcome is graded from the transport family and observed paths.
    pub fn mark_completed(&self) {
        if let Ok(mut g) = self.outcome.lock() {
            *g = SessionOutcome::Completed;
        }
    }

    /// Record a terminal outcome that bypasses the normal exit path (e.g.
    /// the pairing gate rejecting an unpaired device).
    pub fn record(&self, outcome: SessionOutcome) {
        if let Ok(mut g) = self.outcome.lock() {
            *g = outcome;
        }
        self.emit();
        self.emitted.store(true, Ordering::Relaxed);
    }

    fn emit(&self) {
        let Some(sink) = sink() else { return };
        let outcome = *self.outcome.lock().unwrap_or_else(|p| p.into_inner());
        let report = self
            .recorder
            .finish(outcome, self.bytes_sent.load(Ordering::Relaxed));
        sink(&report);
        info_line(&report);
    }
}

impl Drop for SessionGuard<'_> {
    fn drop(&mut self) {
        if !self.emitted.load(Ordering::Relaxed) {
            self.emit();
        }
    }
}

/// Mirror the report into the tracing log at a sensible level, so journal
/// readers see the same truth as the file.
fn info_line(report: &SessionReport) {
    let msg = format!("streaming session {}", report.outcome.as_str());
    match report.outcome {
        SessionOutcome::Rejected | SessionOutcome::Failed => tracing::warn!("{msg}"),
        _ => tracing::info!("{msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::connection::{Connection, ConnectionError, InStream, OutStream};
    use crate::streaming::input_packet::{SAMPLE_DECODE, SAMPLE_E2E, SAMPLE_RENDER};
    use async_trait::async_trait;
    use std::net::{IpAddr, Ipv4Addr};

    /// A connection whose transport statistics the test drives by hand — the
    /// only way to observe a relay ride, a path change and a closing congestion
    /// window without renting a network.
    struct FakeLink {
        stats: Mutex<ConnectionStats>,
        address: SocketAddr,
    }

    impl FakeLink {
        fn set(&self, stats: ConnectionStats) {
            *self.stats.lock().unwrap() = stats;
        }
    }

    #[async_trait]
    impl Connection for FakeLink {
        async fn open_uni(&self) -> Result<Box<dyn OutStream>, ConnectionError> {
            unimplemented!()
        }
        async fn accept_uni(&self) -> Result<Box<dyn InStream>, ConnectionError> {
            unimplemented!()
        }
        fn remote_address(&self) -> SocketAddr {
            self.address
        }
        fn stats(&self) -> ConnectionStats {
            *self.stats.lock().unwrap()
        }
        fn transport_family(&self) -> TransportFamily {
            TransportFamily::Iroh
        }
        fn close(&self, _code: u32, _reason: &[u8]) {}
    }

    fn addr(last: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 64, 0, last)), 4433)
    }

    fn relayed(stats: ConnectionStats) -> ConnectionStats {
        ConnectionStats {
            relayed: true,
            ..stats
        }
    }

    fn report(outcome: SessionOutcome) -> SessionReport {
        SessionReport {
            unix_secs: 123,
            outcome,
            ever_relayed: outcome == SessionOutcome::WanRelayed,
            duration_secs: 10,
            rtt_avg_ms: 42,
            goodput_kbps: 8_000,
            device_id: Some("phone-1".into()),
            rtt_tail: None,
            encode_tail: None,
            rebuild_tail: None,
            decode_tail: None,
            render_tail: None,
            e2e_tail: None,
            link: None,
            encoder: None,
        }
    }

    #[test]
    fn report_line_has_all_fields() {
        let mut r = report(SessionOutcome::WanRelayed);
        let tails = Samples::new();
        for i in 1..=100u64 {
            tails.push_micros(i * 1_000);
        }
        let tail = tails.summary_ms();
        r.rtt_tail = tail;
        r.encode_tail = tail;
        let line = r.format();
        assert!(line.contains("outcome=wan_relayed"));
        assert!(line.contains("ever_relayed=true"));
        assert!(line.contains("dev=phone-1"));
        assert!(line.contains("kbps=8000"));
        // Both tails present, each under its own prefix, so a reader can tell a
        // 99th-percentile encode hitch from a 99th-percentile link stall.
        assert!(line.contains("rtt_p99=99\trtt_max=100"), "{line}");
        assert!(line.contains("enc_p99=99\tenc_max=100"), "{line}");
    }

    #[test]
    fn the_record_names_the_encoder_the_session_ended_on() {
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        assert_eq!(
            rec.finish(SessionOutcome::Completed, 0).encoder,
            None,
            "none opened"
        );
        rec.note_encoder("ffmpeg-sidecar-nvenc");
        rec.note_encoder("ffmpeg-sidecar");
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert_eq!(
            report.encoder.as_deref(),
            Some("ffmpeg-sidecar"),
            "the last one, after a fallback"
        );
        assert!(
            report.format().contains("encoder=ffmpeg-sidecar"),
            "{}",
            report.format()
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["encoder"], "ffmpeg-sidecar");
    }

    #[test]
    fn a_session_that_never_encoded_reports_no_encode_tail() {
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert!(report.encode_tail.is_none());
        assert!(report.rtt_tail.is_none());
        // The line stays parsable: no dangling `enc_p50=` with a zero in it.
        let line = report.format();
        assert!(!line.contains("enc_"), "{line}");
        assert!(!line.contains("rtt_p"), "{line}");
    }

    #[test]
    fn encode_samples_reach_the_report_as_a_distribution() {
        // The exact failure the average hides: 10% of frames stall at 30x the
        // typical encode time. Mean 15 ms still reads as "fine" in a log line.
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        for _ in 0..90 {
            rec.record_encode(Duration::from_millis(4));
        }
        for _ in 0..10 {
            rec.record_encode(Duration::from_millis(120));
        }
        let report = rec.finish(SessionOutcome::Completed, 0);
        let tail = report.encode_tail.expect("samples were taken");
        assert_eq!(tail.count, 100);
        assert_eq!(
            tail.mean_ms, 15,
            "the mean the old telemetry would have quoted"
        );
        assert_eq!(tail.p50_ms, 4);
        assert_eq!(tail.p95_ms, 120, "a one-in-ten stall has to be visible");
        assert_eq!(tail.p99_ms, 120);
        assert_eq!(tail.max_ms, 120);
    }

    #[test]
    fn encoder_rebuilds_reach_the_record_apart_from_the_frames() {
        // AH2: a crop's or a bitrate's rebuild stalls the stream, but the
        // encode timer covers one frame's encode only, so the record showed
        // nothing; its own tail does.
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        for _ in 0..50 {
            rec.record_encode(Duration::from_millis(10));
        }
        rec.record_rebuild(Duration::from_millis(180));
        rec.record_rebuild(Duration::from_millis(240));
        let report = rec.finish(SessionOutcome::Completed, 0);
        let rebuilds = report.rebuild_tail.expect("rebuilds were timed");
        assert_eq!(rebuilds.count, 2);
        assert_eq!(rebuilds.max_ms, 240);
        assert_eq!(
            report.encode_tail.expect("frames").max_ms,
            10,
            "the frames' tail untouched"
        );
        let line = report.format();
        assert!(line.contains("rebuild_n=2\t"), "{line}");
        assert!(line.contains("rebuild_max=240"), "{line}");
        // A session with no rebuild says nothing of them.
        let quiet =
            SessionRecorder::new(TransportFamily::Quinn, None).finish(SessionOutcome::Completed, 0);
        assert!(quiet.rebuild_tail.is_none());
        assert!(!quiet.format().contains("rebuild_"));
    }

    #[test]
    fn the_client_measures_join_the_servers_distributions() {
        // A phone that decodes in 4 ms usually and 200 ms on one frame in fifty
        // is the same invisible failure class the encode tail exists for, and
        // the server cannot see it at all — it has to be reported in.
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        for _ in 0..90 {
            assert!(rec.record_client_samples(SAMPLE_DECODE, &[4_000]));
        }
        assert!(rec.record_client_samples(SAMPLE_DECODE, &[200_000; 10]));
        assert!(rec.record_client_samples(SAMPLE_RENDER, &[16_000; 60]));
        assert!(rec.record_client_samples(SAMPLE_E2E, &[55_000; 60]));
        let report = rec.finish(SessionOutcome::Completed, 0);
        let decode = report.decode_tail.expect("decode samples");
        assert_eq!(decode.count, 100);
        assert_eq!(decode.p50_ms, 4);
        assert_eq!(decode.p95_ms, 200);
        assert_eq!(report.render_tail.expect("render samples").p50_ms, 16);
        assert_eq!(report.e2e_tail.expect("e2e samples").p50_ms, 55);
        let line = report.format();
        for key in ["dec_p99=", "rnd_p50=", "e2e_p50="] {
            assert!(line.contains(key), "missing {key} in {line}");
        }
    }

    #[test]
    fn a_batch_that_arrives_in_pieces_still_makes_one_distribution() {
        // The client flushes in frame-sized chunks, so the record must be built
        // from many batches rather than assuming one call holds the session.
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        for value in [1_000u32, 2_000, 3_000] {
            rec.record_client_samples(SAMPLE_DECODE, &[value]);
        }
        let tail = rec
            .finish(SessionOutcome::Completed, 0)
            .decode_tail
            .unwrap();
        assert_eq!((tail.count, tail.p50_ms, tail.max_ms), (3, 2, 3));
    }

    #[test]
    fn a_sample_kind_with_no_distribution_is_refused_not_swallowed() {
        // A newer client can invent a kind this server has no place for. The
        // caller has to be told, because `true` is what "recorded" means here.
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        assert!(!rec.record_client_samples(9, &[4_000]));
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert!(report.decode_tail.is_none() && report.render_tail.is_none());
    }

    #[test]
    fn the_clients_link_reading_lands_in_the_link_block() {
        let (_link, connection) = fake(
            ConnectionStats {
                lost_packets: 2,
                ..Default::default()
            },
            addr(11),
        );
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        // Before any feedback frame: the server's own numbers are there, the
        // phone's are absent rather than zero.
        rec.sample(&connection);
        let before = rec.finish(SessionOutcome::Completed, 0);
        let link = before.link.expect("sampled");
        assert_eq!(link.lost_packets, 2);
        assert!(link.client_rtt_ms.is_none() && link.client_lost_packets.is_none());
        assert!(
            !before.format().contains("phone_rtt="),
            "{}",
            before.format()
        );

        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        rec.sample(&connection);
        rec.record_client_link(Duration::from_millis(31), 7);
        let after = rec.finish(SessionOutcome::Completed, 0);
        let link = after.link.expect("sampled");
        assert_eq!(
            (link.client_rtt_ms, link.client_lost_packets),
            (Some(31), Some(7)),
            "receive-side loss is only ever knowable by the receiver"
        );
        assert!(
            after.format().contains("phone_rtt=31"),
            "{}",
            after.format()
        );
        assert!(
            after.format().contains("phone_lost=7"),
            "{}",
            after.format()
        );
    }

    #[test]
    fn json_and_text_agree_on_the_outcome_vocabulary() {
        // The record is kept for comparison, so a `wan_punched` in the log line
        // must not become "WanPunched" in the JSON — two vocabularies is how a
        // grep stops being a query.
        for outcome in [
            SessionOutcome::LanDirect,
            SessionOutcome::WanPunched,
            SessionOutcome::WanRelayed,
            SessionOutcome::Rejected,
            SessionOutcome::Failed,
        ] {
            let json = serde_json::to_string(&outcome).unwrap();
            assert_eq!(json, format!("\"{}\"", outcome.as_str()));
        }
    }

    #[test]
    fn a_record_carries_every_stat_a_regression_check_needs() {
        let rec = SessionRecorder::new(TransportFamily::Quinn, Some("phone-1".into()));
        for i in 0..200u64 {
            rec.record_encode(Duration::from_micros(4_000 + i * 1_000));
        }
        let report = rec.finish(SessionOutcome::Completed, 0);
        let json = serde_json::to_string(&report).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["outcome"], "lan_direct");
        assert_eq!(parsed["device_id"], "phone-1");
        assert_eq!(parsed["encode_tail"]["count"], 200);
        assert_eq!(parsed["encode_tail"]["max_ms"], 203);
        assert!(parsed["rtt_tail"].is_null(), "rtt was never sampled");
    }

    #[test]
    fn completed_quinn_session_is_lan_direct() {
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert_eq!(report.outcome, SessionOutcome::LanDirect);
    }

    #[test]
    fn completed_iroh_session_is_graded_by_observed_paths() {
        let punched = SessionRecorder::new(TransportFamily::Iroh, None);
        assert_eq!(
            punched.finish(SessionOutcome::Completed, 0).outcome,
            SessionOutcome::WanPunched
        );
        let relayed = SessionRecorder::new(TransportFamily::Iroh, None);
        relayed.ever_relayed.store(1, Ordering::Relaxed);
        assert_eq!(
            relayed.finish(SessionOutcome::Completed, 0).outcome,
            SessionOutcome::WanRelayed
        );
    }

    #[test]
    fn explicit_outcomes_are_not_reclassified() {
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        for outcome in [SessionOutcome::Rejected, SessionOutcome::Failed] {
            let report = rec.finish(outcome, 0);
            assert_eq!(report.outcome, outcome);
        }
        // guard() is what the pipeline arms; it must be constructible.
        let _guard = rec.guard(Arc::new(AtomicU64::new(0)));
    }

    fn fake(stats: ConnectionStats, address: SocketAddr) -> (Arc<FakeLink>, SharedConnection) {
        let link = Arc::new(FakeLink {
            stats: Mutex::new(stats),
            address,
        });
        let connection: SharedConnection = link.clone();
        (link, connection)
    }

    #[test]
    fn a_session_that_never_touched_a_transport_reports_no_link() {
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert!(report.link.is_none());
        // Not a row of zeros pretending the link was pristine.
        let line = report.format();
        assert!(!line.contains("lost_pk="), "{line}");
        assert!(!line.contains("cwnd_pk="), "{line}");
    }

    #[test]
    fn the_link_is_read_again_at_teardown_not_frozen_at_the_last_poll() {
        // The pipeline polls every few seconds. Loss in the final stretch is
        // exactly where a stall lives, so it must not be discarded with the
        // gap between the last sample and the session ending.
        let (link, connection) = fake(
            ConnectionStats {
                lost_packets: 5,
                ..Default::default()
            },
            addr(7),
        );
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        rec.sample(&connection);
        link.set(ConnectionStats {
            lost_packets: 9,
            ..Default::default()
        });
        let report = rec.finish(SessionOutcome::Completed, 0);
        assert_eq!(report.link.expect("sampled").lost_packets, 9);
    }

    #[test]
    fn a_relay_that_gives_way_to_direct_is_counted_and_timed() {
        // Pure over the observer's clock: an `Instant` is a parameter precisely
        // so this does not have to sleep to prove the arithmetic.
        let start = Instant::now() + Duration::from_secs(1_000);
        let mut observer = PathObserver::default();
        let relayed = relayed(ConnectionStats::default());
        observer.observe(&relayed, addr(1), start);
        observer.observe(&relayed, addr(1), start + Duration::from_secs(4));
        observer.observe(
            &ConnectionStats::default(),
            addr(2),
            start + Duration::from_secs(5),
        );
        assert_eq!(
            observer.path_changes, 1,
            "the relay-to-direct hop is a path change"
        );
        assert_eq!(
            observer.relayed_secs(start + Duration::from_secs(5)),
            5,
            "riding the relay from the first sample to the punched one"
        );
    }

    #[test]
    fn a_relay_ride_still_open_at_teardown_is_billed_to_the_end() {
        let start = Instant::now() + Duration::from_secs(1_000);
        let mut observer = PathObserver::default();
        let relayed = relayed(ConnectionStats::default());
        observer.observe(&relayed, addr(1), start);
        observer.observe(&relayed, addr(1), start + Duration::from_secs(3));
        assert_eq!(observer.relayed_secs(start + Duration::from_secs(6)), 6);
    }

    #[test]
    fn the_widest_congestion_window_is_the_one_kept() {
        let mut observer = PathObserver::default();
        let wide = ConnectionStats {
            cwnd_bytes: Some(50_000),
            ..Default::default()
        };
        let collapsed = ConnectionStats {
            cwnd_bytes: Some(12_000),
            ..Default::default()
        };
        let at = Instant::now() + Duration::from_secs(1_000);
        observer.observe(&collapsed, addr(1), at);
        observer.observe(&wide, addr(1), at + Duration::from_secs(1));
        observer.observe(&collapsed, addr(1), at + Duration::from_secs(2));
        assert_eq!(
            observer.peak_cwnd_bytes,
            Some(50_000),
            "a window that closed and is still recovering must not read as never-congested"
        );
    }

    #[test]
    fn an_unreported_field_stays_absent_in_both_renderings() {
        // The recorder's shape, not a claim about a library: a `None` here means
        // the transport's snapshot had no selected path to read from, and the
        // honest rendering of "not reported" is the key being absent. A `0`
        // would be read as a link that never lost a packet or never grew a
        // window. (Both families can report these four — quinn off the
        // connection, iroh off the selected path — so absence is a gap in a
        // sample, not a WAN-only property.)
        let rec = SessionRecorder::new(TransportFamily::Iroh, None);
        let (_, connection) = fake(
            ConnectionStats {
                lost_packets: 3,
                bytes_sent: 4_000_000,
                datagrams_sent: 3_000,
                relayed: true,
                ..Default::default()
            },
            addr(9),
        );
        rec.sample(&connection);
        let report = rec.finish(SessionOutcome::Completed, 0);
        let line = report.format();
        let link = report.link.expect("sampled");
        assert_eq!(
            (link.path_mtu, link.peak_cwnd_bytes, link.congestion_events),
            (None, None, None)
        );
        for absent in ["mtu=", "cwnd_pk=", "cong_ev=", "black="] {
            assert!(!line.contains(absent), "{absent} in {line}");
        }
        assert!(
            line.contains("relayed_s=0"),
            "a ride too short to round to a second still counts the relay: {line}"
        );
        assert!(line.contains("path_chg=0"), "{line}");
        let json: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert!(json["link"]["path_mtu"].is_null(), "{json}");
        assert_eq!(json["link"]["lost_packets"], 3);

        // The same record with the fields present: both renderings must show them.
        let rec = SessionRecorder::new(TransportFamily::Quinn, None);
        let (_, connection) = fake(
            ConnectionStats {
                cwnd_bytes: Some(48_000),
                path_mtu: Some(1420),
                congestion_events: Some(2),
                black_holes_detected: Some(0),
                ..Default::default()
            },
            addr(9),
        );
        rec.sample(&connection);
        let report = rec.finish(SessionOutcome::Completed, 0);
        let line = report.format();
        assert!(line.contains("mtu=1420"), "{line}");
        assert!(line.contains("cwnd_pk=48000"), "{line}");
        assert!(line.contains("cong_ev=2"), "{line}");
        let json: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert_eq!(json["link"]["path_mtu"], 1420);
        assert_eq!(json["link"]["peak_cwnd_bytes"], 48_000);
    }
}
