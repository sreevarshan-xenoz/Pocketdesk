//! QUIC stream client for receiving H.264 video frames and Opus audio packets.
//!
//! Connects to a `StreamingServer`, receives encoded packets over unidirectional
//! QUIC streams, and demuxes them into video and audio channels.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::connection::{ConnectionError, QuinnConnection, SharedConnection};
use super::input_packet::{InputPacket, SAMPLE_E2E, SampleBatch};
use super::transport::{self, CertManager, StreamTransportConfig};
use super::{AudioPacket, EncodedPacket};

/// Default port for the streaming service.
pub const DEFAULT_STREAMING_PORT: u16 = 4716;

/// Marker bytes for a client-to-server config QUIC stream.
/// Used to transmit settings (e.g. monitor index) before the pipeline starts.
pub(crate) const MONITOR_CONFIG_MARKER: [u8; 2] = [0xFF, 0x00];

/// Marker bytes for a client-to-server identity QUIC stream:
/// `[0xFE, 0x00, len:u8, utf8 deviceId]`. The id says which paired device
/// the session claims to be; the TLS client certificate (or iroh key) is the
/// proof the server holds it to (device keys).
pub(crate) const DEVICE_ID_MARKER: [u8; 2] = [0xFE, 0x00];

/// Marker bytes for a client-to-server WAN key announcement (device keys):
/// `[0xFC, 0x00, len:u8, utf8 iroh endpoint id]`. Sent on a LAN session,
/// where the TLS client certificate already proves the device, so the
/// desktop can trust the WAN key it names.
pub(crate) const IROH_KEY_MARKER: [u8; 2] = [0xFC, 0x00];

/// Marker bytes for a client-to-server codec-capability QUIC stream (R4 C1):
/// `[0xFD, 0x00, caps:u8]`. Absent = legacy client that only decodes H.264.
pub const CODEC_CAPS_MARKER: [u8; 2] = [0xFD, 0x00];

/// Marker bytes for a client-to-server session mode stream (T10i):
/// `[0xFB, 0x00, mode:u8]`. Absent = a session that streams the screen.
pub const SESSION_MODE_MARKER: [u8; 2] = [0xFB, 0x00];

/// [`SESSION_MODE_MARKER`] mode: input only, the phone as a remote. The
/// computer captures, encodes and sends nothing, and says when the session
/// is admitted with a [`super::transport::STREAM_KIND_READY`] stream.
pub const SESSION_MODE_INPUT_ONLY: u8 = 1;

/// `CODEC_CAPS_MARKER` bit 0: the client can decode H.265/HEVC Annex-B.
/// H.264 is assumed for every client; future bits cover AV1 etc.
pub const CODEC_CAP_HEVC: u8 = 0b0000_0001;

/// R4 E3 — compositor-true end-to-end latency probe.
///
/// Every video packet header carries the frame's *age at send* measured on
/// the desktop clock: `Instant::elapsed()` of the capture instant, which on
/// the damage-driven backends is the compositor's copy moment. That makes
/// the dominant, variable part of the latency chain measurable without any
/// cross-device clock sync: sample = capture→send age (pure server-clock
/// interval) + one network leg (transport RTT / 2). What it deliberately
/// does NOT include: the phone's decode→panel present (unmeasurable from
/// software) and any queueing after the client's packet read.
static E2E_EWMA_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One probe sample in microseconds (see [`E2E_EWMA_MS`] docs for
/// semantics/limits): the capture→send age the header carried, plus one
/// network leg. Microseconds are the primary unit because that is what the
/// session record's distribution needs; the millisecond form below is the same
/// number divided down for the HUD.
pub fn e2e_sample_us(capture_age_us: u64, rtt: Duration) -> u64 {
    capture_age_us + (rtt.as_micros() as u64) / 2
}

/// One probe sample in ms (see [`E2E_EWMA_MS`] docs for semantics/limits).
pub fn e2e_sample_ms(capture_age_us: u64, rtt: Duration) -> u64 {
    e2e_sample_us(capture_age_us, rtt) / 1_000
}

/// EWMA (¾ old, ¼ new) so the HUD doesn't jitter per frame. First sample
/// after [`reset_e2e_probe`] seeds it directly.
pub fn record_e2e_sample(sample_ms: u64) {
    use std::sync::atomic::Ordering;
    // A hand-rolled CAS loop rather than `fetch_update`, which rustc 1.99
    // deprecates in favour of a `try_update` older toolchains lack.
    let mut prev = E2E_EWMA_MS.load(Ordering::Relaxed);
    loop {
        let next = if prev == 0 {
            sample_ms.max(1)
        } else {
            (3 * prev + sample_ms) / 4
        };
        match E2E_EWMA_MS.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => prev = actual,
        }
    }
}

/// Smoothed compositor-true e2e estimate, ms. 0 until the first video
/// packet of the current session arrives (per-session seed, not stale data).
pub fn e2e_estimate_ms() -> u64 {
    E2E_EWMA_MS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Clear the probe at session start.
pub fn reset_e2e_probe() {
    E2E_EWMA_MS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Frames the link never delivered.
///
/// Every video frame rides its own uni-stream carrying a sequence number the
/// encoder hands out one-per-packet, so a hole in the range the client has
/// seen is a frame that never arrived — skipped by the transport backlog,
/// reset in flight, or lost with the connection. Wherever the cause lives, the
/// miss is only knowable here, which is why the session HUD's drop counter
/// reads this rather than a desktop-side tally.
///
/// Measured as `highest - lowest + 1 - arrived` instead of "the gap I just
/// saw", because streams complete out of order: a frame still in flight is
/// counted for the moment its successors land and stops being counted when it
/// does, whereas charging every gap on sight would keep counting frames that
/// merely arrived late. The number can therefore come back down — which is
/// what a live count of a self-correcting measurement means, against a
/// ledger of false drops that only ever grows. `lowest` starts at `u64::MAX`
/// so the first packet of a window sets it.
static VIDEO_SEQ_LOWEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
static VIDEO_SEQ_HIGHEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static VIDEO_SEQ_ARRIVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The arithmetic above, as a pure function of one window. `arrived == 0`
/// means nothing has been received yet, so nothing is known — a session that
/// has not delivered a frame reports zero drops rather than a phantom span.
pub fn missed_frames_in_window(lowest: u64, highest: u64, arrived: u64) -> u64 {
    if arrived == 0 || lowest > highest {
        return 0;
    }
    (highest - lowest + 1).saturating_sub(arrived)
}

/// Start a fresh window. Called at session start, and on any packet numbered
/// 0: the encoder counts from 0, so a zero also means a rebuilt encoder — a
/// bitrate or resolution change, or the C2 hardware→software fallback — whose
/// sequence numbers have nothing to do with the run they replaced.
pub fn reset_video_window() {
    use std::sync::atomic::Ordering;
    VIDEO_SEQ_LOWEST.store(u64::MAX, Ordering::Relaxed);
    VIDEO_SEQ_HIGHEST.store(0, Ordering::Relaxed);
    VIDEO_SEQ_ARRIVED.store(0, Ordering::Relaxed);
}

/// Fold one received video packet into the window. Only the receive loop
/// writes these, so a concurrent reader can at worst see a window one packet
/// short for one HUD tick.
pub fn record_video_sequence(sequence: u64) {
    use std::sync::atomic::Ordering;
    if sequence == 0 {
        reset_video_window();
    }
    VIDEO_SEQ_HIGHEST.fetch_max(sequence, Ordering::Relaxed);
    VIDEO_SEQ_LOWEST.fetch_min(sequence, Ordering::Relaxed);
    VIDEO_SEQ_ARRIVED.fetch_add(1, Ordering::Relaxed);
}

/// Frames missed since the window opened. Zero until the first frame arrives.
pub fn missed_video_frames() -> u64 {
    use std::sync::atomic::Ordering;
    missed_frames_in_window(
        VIDEO_SEQ_LOWEST.load(Ordering::Relaxed),
        VIDEO_SEQ_HIGHEST.load(Ordering::Relaxed),
        VIDEO_SEQ_ARRIVED.load(Ordering::Relaxed),
    )
}

/// QUIC Stream Client — connects to a StreamingServer and receives H.264 video frames.
///
/// # Usage
///
/// ```ignore
/// let cert_manager = std::sync::Arc::new(CertManager::new().unwrap());
/// let (mut client, packet_rx) = StreamingClient::connect("100.64.0.1:4716", cert_manager, None, None, None).await?;
/// // Spawn a task to consume packets from packet_rx
/// tokio::spawn(consume_packets(packet_rx));
/// client.start().await; // runs until cancelled
/// ```
pub struct StreamingClient {
    connection: Option<SharedConnection>,
    frame_tx: mpsc::Sender<EncodedPacket>,
    audio_tx: mpsc::Sender<AudioPacket>,
    cancel: CancellationToken,
    /// Unique session ID for tracking this connection
    session_id: String,
    /// Video payload bytes received so far, counted as they arrive rather
    /// than when a frame ends: a large frame on a slow link still shows
    /// the link is alive ([`StreamingClient::video_bytes`]).
    video_bytes: Arc<std::sync::atomic::AtomicU64>,
    /// The session's input-only state (T10i): whether it is one, admitted,
    /// or met a server too old to know it.
    remote: RemoteFlags,
}

/// An input-only session's state (T10i), shared with the receive loop.
#[derive(Debug, Clone, Default)]
pub struct RemoteFlags {
    /// The session is input only: no picture or sound is expected.
    pub input_only: bool,
    /// The computer admitted it (its ready stream came).
    pub ready: Arc<std::sync::atomic::AtomicBool>,
    /// The computer sent a picture or sound anyway: it does not know
    /// input-only sessions, and the session was closed.
    pub too_old: Arc<std::sync::atomic::AtomicBool>,
}

impl StreamingClient {
    /// Create a new `StreamingClient` (not yet connected).
    ///
    /// `channel_capacity` controls the buffer size between the receive task
    /// and the consumer. A capacity of 8 is a reasonable default for real-time
    /// video. Returns the client and receiver channels for consuming frames and audio.
    pub fn new(
        channel_capacity: usize,
        _cert_manager: std::sync::Arc<CertManager>,
    ) -> (
        Self,
        mpsc::Receiver<EncodedPacket>,
        mpsc::Receiver<AudioPacket>,
    ) {
        let (frame_tx, frame_rx) = mpsc::channel(channel_capacity);
        let (audio_tx, audio_rx) = mpsc::channel(channel_capacity);
        let client = Self {
            connection: None,
            frame_tx,
            audio_tx,
            cancel: CancellationToken::new(),
            session_id: String::new(),
            video_bytes: Arc::default(),
            remote: RemoteFlags::default(),
        };
        (client, frame_rx, audio_rx)
    }

    /// The video payload bytes this client has received, counted as they
    /// arrive: a watchdog can tell a slow link (still counting) from a dead
    /// one.
    pub fn video_bytes(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.video_bytes.clone()
    }

    /// The input-only state (T10i), for the caller to watch.
    pub fn remote(&self) -> RemoteFlags {
        self.remote.clone()
    }

    /// Connect to a streaming server at the given address.
    ///
    /// The address should be in the form `"host:port"`, e.g. `"100.64.0.1:4716"`.
    /// Optionally sends a `monitor_index` to the server for multi-monitor selection.
    /// `device_id` is announced in-band so the server can apply its pairing gate.
    /// `codec_caps` (R4 C1, see [`CODEC_CAP_HEVC`]) advertises the client's
    /// decodable set; `None` (older callers) means H.264-only. With
    /// `input_only` (T10i) the session carries input and no picture or sound.
    /// Returns the client and receiver channels for consuming video frames and audio packets.
    pub async fn connect(
        addr: &str,
        cert_manager: std::sync::Arc<CertManager>,
        monitor_index: Option<u32>,
        device_id: Option<&str>,
        iroh_id: Option<&str>,
        codec_caps: Option<u8>,
        input_only: bool,
    ) -> Result<(
        Self,
        mpsc::Receiver<EncodedPacket>,
        mpsc::Receiver<AudioPacket>,
    )> {
        let addr: SocketAddr = addr
            .parse()
            .with_context(|| format!("Invalid server address: {addr}"))?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let conn_id = uuid::Uuid::new_v4().to_string();
        let span = tracing::info_span!(
            "stream_client",
            session = %session_id,
            conn = %conn_id,
            peer = %addr
        );

        use tracing::Instrument;

        async move {
            info!("Connecting to streaming server");

            let transport = super::transport::StreamClient::new(
                StreamTransportConfig::default(),
                &cert_manager,
            )
            .context("Failed to create QUIC transport")?;

            let server_name = addr.ip().to_string();
            let quic_connection = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                transport.connect(addr, &server_name),
            )
            .await
            .context("Connection to streaming server timed out")?
            .context("Failed to connect to streaming server")?;
            info!("Streaming connection established");

            let (client, frame_rx, audio_rx) = Self::attach_with_session(
                QuinnConnection::shared(quic_connection),
                monitor_index,
                device_id,
                iroh_id,
                codec_caps,
                input_only,
                session_id,
            )
            .await;
            Ok((client, frame_rx, audio_rx))
        }
        .instrument(span)
        .await
    }

    /// Adopt an already-established transport-agnostic connection — e.g. an
    /// iroh WAN dial (`super::IrohDial`) — into the same receive pipeline
    /// `connect` builds. The caller keeps ownership of the endpoint/TLS
    /// lifecycle; this only wraps the connection and optionally ships the
    /// monitor-selection config stream.
    pub async fn attach(
        connection: SharedConnection,
        monitor_index: Option<u32>,
        device_id: Option<&str>,
        iroh_id: Option<&str>,
        codec_caps: Option<u8>,
        input_only: bool,
    ) -> (
        Self,
        mpsc::Receiver<EncodedPacket>,
        mpsc::Receiver<AudioPacket>,
    ) {
        Self::attach_with_session(
            connection,
            monitor_index,
            device_id,
            iroh_id,
            codec_caps,
            input_only,
            uuid::Uuid::new_v4().to_string(),
        )
        .await
    }

    async fn attach_with_session(
        connection: SharedConnection,
        monitor_index: Option<u32>,
        device_id: Option<&str>,
        iroh_id: Option<&str>,
        codec_caps: Option<u8>,
        input_only: bool,
        session_id: String,
    ) -> (
        Self,
        mpsc::Receiver<EncodedPacket>,
        mpsc::Receiver<AudioPacket>,
    ) {
        let channel_capacity = 8;
        let (frame_tx, frame_rx) = mpsc::channel(channel_capacity);
        let (audio_tx, audio_rx) = mpsc::channel(channel_capacity);

        // Send optional monitor_index to the server as a config stream (not
        // for a remote: there is no picture to choose).
        if let Some(index) = monitor_index.filter(|_| !input_only) {
            match connection.open_uni().await {
                Ok(mut config_stream) => {
                    // Format: [0xFF, 0x00] config marker + 4 bytes LE monitor_index
                    let mut config_buf = [0u8; 6];
                    config_buf[0..2].copy_from_slice(&MONITOR_CONFIG_MARKER);
                    config_buf[2..6].copy_from_slice(&index.to_le_bytes());
                    if let Err(e) = config_stream.write_all(&config_buf).await {
                        warn!(error = %e, "Failed to send monitor index");
                    }
                    if let Err(e) = config_stream.finish() {
                        warn!(error = %e, "Failed to finish config stream");
                    }
                    info!(index, "Sent monitor selection to server");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to open config stream");
                }
            }
        }

        // Announce our device id so the server's pairing gate can bind this
        // video session to the device that paired over the control channel.
        if let Some(id) = device_id {
            let id_bytes = id.as_bytes();
            if id_bytes.len() <= u8::MAX as usize {
                match connection.open_uni().await {
                    Ok(mut identity_stream) => {
                        let mut buf = Vec::with_capacity(3 + id_bytes.len());
                        buf.extend_from_slice(&DEVICE_ID_MARKER);
                        buf.push(id_bytes.len() as u8);
                        buf.extend_from_slice(id_bytes);
                        if let Err(e) = identity_stream.write_all(&buf).await {
                            warn!(error = %e, "Failed to send device identity");
                        }
                        if let Err(e) = identity_stream.finish() {
                            warn!(error = %e, "Failed to finish identity stream");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to open identity stream");
                    }
                }
            } else {
                warn!("Device id too long to announce — pairing gate will reject");
            }
        }

        // Device keys: name the WAN key this device dials with, on a session
        // whose TLS certificate already proves the device.
        if let Some(iroh) = iroh_id.filter(|id| id.len() <= u8::MAX as usize) {
            match connection.open_uni().await {
                Ok(mut key_stream) => {
                    let mut buf = Vec::with_capacity(3 + iroh.len());
                    buf.extend_from_slice(&IROH_KEY_MARKER);
                    buf.push(iroh.len() as u8);
                    buf.extend_from_slice(iroh.as_bytes());
                    if let Err(e) = key_stream.write_all(&buf).await {
                        warn!(error = %e, "Failed to send the WAN key");
                    }
                    if let Err(e) = key_stream.finish() {
                        warn!(error = %e, "Failed to finish the WAN key stream");
                    }
                }
                Err(e) => warn!(error = %e, "Failed to open the WAN key stream"),
            }
        }

        // The phone as a remote (T10i): say so, and send no codec caps.
        if input_only {
            match connection.open_uni().await {
                Ok(mut mode_stream) => {
                    let buf = [
                        SESSION_MODE_MARKER[0],
                        SESSION_MODE_MARKER[1],
                        SESSION_MODE_INPUT_ONLY,
                    ];
                    if let Err(e) = mode_stream.write_all(&buf).await {
                        warn!(error = %e, "Failed to send the session mode");
                    }
                    if let Err(e) = mode_stream.finish() {
                        warn!(error = %e, "Failed to finish the session mode stream");
                    }
                }
                Err(e) => warn!(error = %e, "Failed to open the session mode stream"),
            }
        }

        // Advertise our decodable codec set (R4 C1). Absent = H.264-only.
        if let Some(caps) = codec_caps
            && caps != 0
            && !input_only
        {
            match connection.open_uni().await {
                Ok(mut caps_stream) => {
                    let buf = [CODEC_CAPS_MARKER[0], CODEC_CAPS_MARKER[1], caps];
                    if let Err(e) = caps_stream.write_all(&buf).await {
                        warn!(error = %e, "Failed to send codec caps");
                    }
                    if let Err(e) = caps_stream.finish() {
                        warn!(error = %e, "Failed to finish codec caps stream");
                    }
                    info!(caps, "Sent codec capabilities to server");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to open codec caps stream");
                }
            }
        }

        let client = Self {
            connection: Some(connection),
            frame_tx,
            audio_tx,
            cancel: CancellationToken::new(),
            session_id,
            video_bytes: Arc::default(),
            remote: RemoteFlags {
                input_only,
                ..Default::default()
            },
        };

        (client, frame_rx, audio_rx)
    }

    /// Start receiving frames. This runs until cancelled or the connection closes.
    pub async fn start(&mut self) {
        let connection = match &self.connection {
            Some(conn) => conn.clone(),
            None => {
                error!("Cannot start: no active connection (call connect first)");
                return;
            }
        };

        let cancel = self.cancel.clone();
        let frame_tx = self.frame_tx.clone();
        let audio_tx = self.audio_tx.clone();
        let video_bytes = self.video_bytes.clone();
        let remote = self.remote.clone();

        let span = tracing::info_span!(
            "client_loop",
            session = %self.session_id,
            transport = "quic"
        );

        use tracing::Instrument;

        async move {
            info!("Starting frame receiver tasks");

            // Samples taken between flushes, shared by the receiver (which
            // measures) and the feedback loop (which reports).
            let e2e_samples = Arc::<SampleBatch>::default();

            // Clone connection for the stats task before it gets moved into recv task
            let stats_connection = connection.clone();
            let stats_samples = e2e_samples.clone();

            // Spawn the receive loop using our cancel-aware receiver
            let recv_cancel = cancel.clone();
            let recv_span = tracing::info_span!("packet_recv");
            let mut recv_handle = tokio::spawn(
                async move {
                    let result = recv_with_cancel(
                        &connection,
                        frame_tx,
                        audio_tx,
                        recv_cancel,
                        e2e_samples,
                        video_bytes,
                        remote,
                    )
                    .await;
                    match result {
                        Ok(()) => debug!("Frame receiver finished normally"),
                        Err(e) => warn!(error = %e, "Frame receiver error"),
                    }
                }
                .instrument(recv_span),
            );

            // Spawn a stats feedback loop that periodically reports the client's
            // own view of the link, plus whatever it measured since the last tick
            let stats_cancel = cancel.clone();
            let stats_span = tracing::info_span!("stats_feedback");
            let stats_handle = tokio::spawn(
                async move {
                    send_stats_loop(&stats_connection, stats_cancel, &stats_samples).await;
                }
                .instrument(stats_span),
            );

            // Wait for cancellation or receive task completion
            let recv_done = tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Streaming client loop cancelled");
                    false
                }
                result = &mut recv_handle => {
                    if let Err(e) = result {
                        warn!(error = %e, "Receive task panicked");
                    }
                    true
                }
            };
            // Neither task outlives the session: each holds the connection,
            // and the stats loop could wait forever on a send, keeping the
            // link alive (and the desktop encoding) after the phone moved on.
            stats_handle.abort();
            let _ = stats_handle.await;
            if !recv_done {
                recv_handle.abort();
                let _ = recv_handle.await;
            }
        }
        .instrument(span)
        .await
    }

    /// Signal the client to stop receiving.
    pub fn stop(&mut self) {
        info!("Stopping streaming client");
        self.cancel.cancel();
        self.connection = None;
    }

    /// Get the current QUIC RTT measurement.
    pub fn current_rtt(&self) -> Duration {
        match &self.connection {
            Some(conn) => conn.stats().rtt,
            None => Duration::ZERO,
        }
    }

    /// Get the transport-agnostic connection handle (for input sends / stats).
    pub fn connection(&self) -> Option<&SharedConnection> {
        self.connection.as_ref()
    }

    /// Check if the client is actively receiving.
    pub fn is_running(&self) -> bool {
        self.connection.is_some() && !self.cancel.is_cancelled()
    }

    /// Return a `CancellationToken` that can be used to cancel this client from outside.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Send an input event over the QUIC connection.
    ///
    /// Encodes the packet as compact binary and sends it over a unidirectional
    /// QUIC stream. The server's Task 4 will parse and inject it.
    pub async fn send_input(&self, packet: &InputPacket) -> Result<()> {
        let connection = self.connection.as_ref().context("No active connection")?;

        let data = packet.encode();
        let mut send_stream = connection
            .open_uni()
            .await
            .context("Failed to open input stream")?;

        send_stream
            .write_all(&data)
            .await
            .context("Failed to send input packet")?;

        send_stream
            .finish()
            .context("Failed to finish input stream")?;

        debug!("Sent input packet: {} bytes over QUIC", data.len());
        Ok(())
    }
}

/// Outcome of handing one audio packet to the consumer.
#[derive(Debug, PartialEq, Eq)]
enum AudioDelivery {
    Queued,
    /// The queue was full, so this packet was not queued.
    Dropped,
    /// Nothing is receiving audio any more.
    NoConsumer,
}

/// What a stream of `kind` is to a session (T10i).
#[derive(Debug, PartialEq, Eq)]
enum RemoteStream {
    /// The input-only session was admitted.
    Ready,
    /// A picture or sound on an input-only session: the computer does not
    /// know the mode.
    TooOld,
    /// A picture or sound for a session that streams.
    Media,
}

fn remote_stream(kind: u8, input_only: bool) -> RemoteStream {
    if kind == transport::STREAM_KIND_READY {
        RemoteStream::Ready
    } else if input_only {
        RemoteStream::TooOld
    } else {
        RemoteStream::Media
    }
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    #[test]
    fn a_remote_takes_its_ready_stream_and_refuses_a_picture() {
        assert_eq!(
            remote_stream(transport::STREAM_KIND_READY, true),
            RemoteStream::Ready
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_VIDEO, true),
            RemoteStream::TooOld
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_AUDIO, true),
            RemoteStream::TooOld
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_VIDEO, false),
            RemoteStream::Media
        );
    }
}

/// Queue one audio packet without ever making the caller wait for it.
///
/// Video and audio share this one demux loop, and the loop is the only thing
/// reading the connection: if it stops for audio, the *picture* stops too. That
/// is not hypothetical — a client whose audio consumer is not polling (today's
/// Android app: `nativeReceiveAudio` has no caller, because the phone has no
/// Opus playout path) fills the 8-slot queue 160 ms into a session and the
/// `await`-ing send then wedges video behind it. It was not rare either: the
/// desktop used to stream synthesized silence at 50 packets/s whenever PipeWire
/// capture was unavailable, so any session on such a machine hit it.
///
/// So audio is offered, never insisted on. A full queue means the consumer is
/// at least 160 ms behind and this packet is already too late to be worth
/// bumping an older one out of; a closed receiver ends the *audio* path and
/// nothing else.
fn try_deliver_audio(tx: &mpsc::Sender<AudioPacket>, packet: AudioPacket) -> AudioDelivery {
    match tx.try_send(packet) {
        Ok(()) => AudioDelivery::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => AudioDelivery::Dropped,
        Err(mpsc::error::TrySendError::Closed(_)) => AudioDelivery::NoConsumer,
    }
}

/// Receive packets with cancellation support — demuxes video and audio streams.
///
/// Reads packet headers and routes to the appropriate channel based on `stream_kind`:
/// - `stream_kind == 0` → video frames on `frame_tx`
/// - `stream_kind == 1` → audio packets on `audio_tx`
async fn recv_with_cancel(
    connection: &SharedConnection,
    frame_tx: mpsc::Sender<EncodedPacket>,
    audio_tx: mpsc::Sender<AudioPacket>,
    cancel: CancellationToken,
    e2e_samples: Arc<SampleBatch>,
    video_bytes: Arc<std::sync::atomic::AtomicU64>,
    remote: RemoteFlags,
) -> Result<()> {
    info!("Starting packet receiver (video + audio)");
    reset_e2e_probe();
    reset_video_window();

    // Video sequence-gap tracking for IDR requests. Because each frame is sent
    // on its own unidirectional stream, frames can complete out of order; we
    // only ask for a fresh IDR when a *non-keyframe* arrives with a gap before
    // it (a broken delta chain), and rate-limit so we don't spam the server.
    let mut last_video_seq: Option<u64> = None;
    let mut last_idr_request: Option<std::time::Instant> = None;
    const IDR_REQUEST_INTERVAL: Duration = Duration::from_millis(250);

    let mut audio_dropped = 0u64;
    let mut audio_consumer_gone = false;

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                debug!("Packet receiver cancelled");
                break;
            }

            result = connection.accept_uni() => {
                match result {
                    Ok(mut recv_stream) => {
                        // Read the 18-byte packet header
                        let mut header_bytes = [0u8; 18];
                        if let Err(e) = recv_stream.read_exact(&mut header_bytes).await {
                            debug!("Failed to read packet header: {e}");
                            continue;
                        }

                        let header = match transport::PacketHeader::from_bytes(&header_bytes) {
                            Ok(h) => h,
                            Err(e) => {
                                warn!("Invalid packet header: {e}");
                                continue;
                            }
                        };

                        // T10i: an input-only session is admitted, or met a
                        // computer that streams to it anyway (too old to know
                        // the mode): that one is closed, not read.
                        match remote_stream(header.stream_kind, remote.input_only) {
                            RemoteStream::Ready => {
                                remote.ready.store(true, std::sync::atomic::Ordering::Release);
                                info!("The computer admitted the input-only session");
                                continue;
                            }
                            RemoteStream::TooOld => {
                                remote.too_old.store(true, std::sync::atomic::Ordering::Release);
                                warn!("The computer streams to an input-only session: it is too old for it");
                                connection.close(0u32, b"input-only session not known here");
                                break;
                            }
                            RemoteStream::Media => {}
                        }

                        // Read the payload (up to 10 MB for video, 64 KB for audio)
                        let max_size = if header.stream_kind == transport::STREAM_KIND_AUDIO {
                            64 * 1024
                        } else {
                            10 * 1024 * 1024
                        };
                        // Video is counted as it arrives (a slow keyframe is
                        // not a dead link); audio flows even when video does
                        // not, so it is not counted.
                        let read = if header.stream_kind == transport::STREAM_KIND_AUDIO {
                            recv_stream.read_to_end(max_size).await
                        } else {
                            recv_stream.read_to_end_counting(max_size, &video_bytes).await
                        };
                        let data = match read {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Failed to read packet data (seq={}, kind={}): {e}", header.sequence, header.stream_kind);
                                continue;
                            }
                        };

                        if header.stream_kind == transport::STREAM_KIND_AUDIO {
                            // Offered, never insisted on: see `try_deliver_audio`.
                            let packet = AudioPacket {
                                data,
                                sequence: header.sequence,
                                timestamp: std::time::Instant::now(),
                                is_config: false,
                            };

                            match try_deliver_audio(&audio_tx, packet) {
                                AudioDelivery::Queued => {}
                                AudioDelivery::Dropped => {
                                    audio_dropped += 1;
                                    if audio_dropped.is_multiple_of(100) {
                                        debug!(
                                            dropped = audio_dropped,
                                            "Audio consumer behind — packets skipped"
                                        );
                                    }
                                }
                                AudioDelivery::NoConsumer => {
                                    if !audio_consumer_gone {
                                        audio_consumer_gone = true;
                                        info!(
                                            "Audio consumer gone — audio skipped, video continues"
                                        );
                                    }
                                }
                            }
                        } else {
                            // Gap detection: a missing frame between the last
                            // delta we got and this one breaks the decode chain.
                            // A keyframe restarts the chain, so it never needs a
                            // request; a delta after a gap does.
                            let gap = !header.is_keyframe
                                && last_video_seq
                                    .is_some_and(|last| header.sequence > last + 1);
                            if gap {
                                let now = std::time::Instant::now();
                                let throttled = last_idr_request
                                    .is_some_and(|t| now.duration_since(t) < IDR_REQUEST_INTERVAL);
                                if !throttled {
                                    last_idr_request = Some(now);
                                    debug!(
                                        seq = header.sequence,
                                        last = ?last_video_seq,
                                        "Video sequence gap — requesting IDR"
                                    );
                                    send_keyframe_request(connection).await;
                                }
                            }
                            last_video_seq = Some(header.sequence);
                            record_video_sequence(header.sequence);

                            // R4 E3: compositor-true latency sample — the
                            // header's age is capture→send on the desktop
                            // clock, plus half the transport RTT for the
                            // wire leg. The EWMA feeds the HUD; the raw
                            // sample joins the batch the server turns into
                            // the session's e2e tail.
                            let rtt = connection.stats().rtt;
                            record_e2e_sample(e2e_sample_ms(header.timestamp_us, rtt));
                            e2e_samples.push(e2e_sample_us(header.timestamp_us, rtt));

                            // Route to video channel
                            let packet = EncodedPacket {
                                data,
                                is_keyframe: header.is_keyframe,
                                timestamp: std::time::Instant::now(),
                                sequence: header.sequence,
                            };

                            if frame_tx.send(packet).await.is_err() {
                                debug!("Frame receiver dropped — channel closed");
                                break;
                            }
                        }
                    }
                    Err(ConnectionError::Closed) => {
                        debug!("Connection closed by peer");
                        break;
                    }
                    Err(e) => {
                        warn!("Stream accept error: {e}");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Best-effort IDR request to the server after a detected video gap.
async fn send_keyframe_request(connection: &SharedConnection) {
    let data = InputPacket::RequestKeyframe.encode();
    match connection.open_uni().await {
        Ok(mut stream) => {
            if let Err(e) = stream.write_all(&data).await {
                debug!("Failed to send IDR request: {e}");
            } else {
                let _ = stream.finish();
            }
        }
        Err(e) => {
            debug!("Failed to open IDR request stream: {e}");
        }
    }
}

/// Send one frame to the server on its own unidirectional stream. Best effort:
/// a feedback frame that cannot be sent is not worth an error path, since the
/// next tick reports again.
async fn send_frame(connection: &SharedConnection, packet: &InputPacket) {
    let data = packet.encode();
    match connection.open_uni().await {
        Ok(mut stream) => {
            if let Err(e) = stream.write_all(&data).await {
                debug!("Failed to send feedback to server: {e}");
            } else {
                let _ = stream.finish();
            }
        }
        Err(e) => {
            debug!("Failed to open feedback stream: {e}");
        }
    }
}

/// Report one frame's worth of a client-measured duration batch over an
/// existing streaming connection.
///
/// For a measurement source that lives outside [`StreamingClient`] — the Android
/// decoder is a separate object holding only the connection handle — so its
/// samples join the same server-side reservoir the client's own e2e probes
/// already feed. Best effort like the stats loop: a frame that cannot be sent
/// costs nothing, because the batch keeps the samples until the next tick.
/// Returns whether a frame went out.
pub async fn report_samples(connection: &SharedConnection, kind: u8, batch: &SampleBatch) -> bool {
    let Some(values) = batch.take_chunk() else {
        return false;
    };
    send_frame(connection, &InputPacket::ClientSamples { kind, values }).await;
    true
}

/// Periodically report the client's own account of the session: the link as its
/// transport sees it, and every latency sample it took since the last tick.
///
/// The two travel as ordinary [`InputPacket`] frames so the server dispatches
/// them by tag like any other client control. This loop used to write a bare
/// 16-byte buffer that the server matched on length and discarded.
async fn send_stats_loop(
    connection: &SharedConnection,
    cancel: CancellationToken,
    e2e_samples: &SampleBatch,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Stats feedback cancelled");
                break;
            }

            _ = interval.tick() => {
                let stats = connection.stats();
                send_frame(
                    connection,
                    &InputPacket::LinkFeedback {
                        rtt: stats.rtt,
                        lost_packets: stats.lost_packets,
                    },
                )
                .await;

                // Drain in frame-sized chunks: a stalled flush loop must not
                // lose the samples it already holds, only delay them.
                while let Some(values) = e2e_samples.take_chunk() {
                    send_frame(
                        connection,
                        &InputPacket::ClientSamples {
                            kind: SAMPLE_E2E,
                            values,
                        },
                    )
                    .await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_packet(sequence: u64) -> AudioPacket {
        AudioPacket {
            data: vec![0xf8],
            sequence,
            timestamp: std::time::Instant::now(),
            is_config: false,
        }
    }

    #[test]
    fn a_full_audio_queue_is_a_drop_not_a_wait() {
        // The demux loop shares its progress with video, so nothing on the
        // audio side may block: a full queue means this packet is skipped and
        // the older ones stay queued for whoever finally reads them.
        let (tx, mut rx) = mpsc::channel(2);
        assert_eq!(
            try_deliver_audio(&tx, audio_packet(0)),
            AudioDelivery::Queued
        );
        assert_eq!(
            try_deliver_audio(&tx, audio_packet(1)),
            AudioDelivery::Queued
        );
        assert_eq!(
            try_deliver_audio(&tx, audio_packet(2)),
            AudioDelivery::Dropped,
            "the third packet must return immediately, not wait for a consumer"
        );
        assert_eq!(rx.try_recv().ok().map(|p| p.sequence), Some(0));
        assert_eq!(rx.try_recv().ok().map(|p| p.sequence), Some(1));
        assert!(
            rx.try_recv().is_err(),
            "the skipped packet is gone for good, not queued behind the others"
        );
    }

    #[test]
    fn a_gone_audio_consumer_ends_audio_not_the_session() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        assert_eq!(
            try_deliver_audio(&tx, audio_packet(0)),
            AudioDelivery::NoConsumer,
            "no listener is a reason to stop sending audio, not to stop the link"
        );
    }

    #[test]
    fn e2e_sample_math() {
        // 12.34 ms capture→send age + 51 ms RTT (25.5 → 25 ms one-way).
        assert_eq!(e2e_sample_ms(12_340, Duration::from_millis(51)), 12 + 25);
        assert_eq!(e2e_sample_ms(0, Duration::ZERO), 0);
    }

    #[test]
    fn a_clean_link_reports_no_drops() {
        // In order, nothing missing: the span and the arrival count agree.
        assert_eq!(missed_frames_in_window(0, 9, 10), 0);
        // Frames 3 and 4 never arrived.
        assert_eq!(missed_frames_in_window(0, 9, 8), 2);
        // Nothing received yet, so nothing is known. A session that produced
        // no video must read as zero drops, not as a phantom span.
        assert_eq!(missed_frames_in_window(u64::MAX, 0, 0), 0);
    }

    #[test]
    fn a_late_frame_corrects_the_count() {
        // Streams complete out of order, so 12 can land while 9-11 are still
        // in flight: those three read as missing for that moment.
        assert_eq!(missed_frames_in_window(8, 12, 2), 3);
        // The late arrivals correct it with no other bookkeeping. Charging the
        // gap on sight instead would have left three permanent phantom drops
        // on a link that lost nothing.
        assert_eq!(missed_frames_in_window(8, 12, 5), 0);
    }

    #[test]
    fn a_rebuilt_encoder_starts_a_new_window() {
        // The window is process-global state, so this test owns it outright:
        // nothing else in this module records sequences.
        reset_video_window();
        assert_eq!(missed_video_frames(), 0, "an empty window is not a loss");
        for seq in 0..5 {
            record_video_sequence(seq);
        }
        record_video_sequence(6);
        assert_eq!(missed_video_frames(), 1, "frame 5 never arrived");
        // A bitrate rebuild: the new encoder counts from 0 again, and the
        // old run's 6 must not read as six missing frames.
        record_video_sequence(0);
        assert_eq!(missed_video_frames(), 0, "sequence 0 must reset the run");
        record_video_sequence(1);
        assert_eq!(missed_video_frames(), 0, "and the new run counts cleanly");
    }

    #[test]
    fn e2e_ewma_seed_smooth_and_reset() {
        reset_e2e_probe();
        assert_eq!(
            e2e_estimate_ms(),
            0,
            "fresh session must not show stale data"
        );
        record_e2e_sample(100);
        assert_eq!(e2e_estimate_ms(), 100, "first sample seeds directly");
        record_e2e_sample(0);
        assert_eq!(e2e_estimate_ms(), 75, "EWMA must be 3/4 old + 1/4 new");
        reset_e2e_probe();
        assert_eq!(e2e_estimate_ms(), 0);
    }

    #[test]
    fn test_streaming_client_new() {
        use super::transport::CertManager;
        let cert_manager =
            std::sync::Arc::new(CertManager::new().expect("Failed to create CertManager"));
        let (client, _frame_rx, _audio_rx) = StreamingClient::new(8, cert_manager);
        assert!(!client.is_running());
        assert_eq!(client.current_rtt(), Duration::ZERO);
    }

    #[test]
    fn feedback_frames_are_what_the_stats_loop_puts_on_the_wire() {
        // The loop's two exports, checked as shapes the server dispatches by
        // tag: a link reading and one batch of e2e samples. This is the frame
        // format that replaced an untagged 16-byte buffer the server discarded.
        let feedback = InputPacket::LinkFeedback {
            rtt: Duration::from_millis(28),
            lost_packets: 4,
        };
        let decoded = InputPacket::decode(&feedback.encode()).expect("feedback decodes");
        assert!(
            matches!(
                decoded,
                InputPacket::LinkFeedback {
                    lost_packets: 4,
                    ..
                }
            ),
            "{decoded:?}"
        );

        let batch = SampleBatch::default();
        batch.push(e2e_sample_us(12_340, Duration::from_millis(51)));
        batch.push(e2e_sample_us(9_000, Duration::from_millis(30)));
        let values = batch.take_chunk().expect("two samples pending");
        assert_eq!(values, vec![37_840, 24_000]);
        let samples = InputPacket::ClientSamples {
            kind: SAMPLE_E2E,
            values,
        };
        match InputPacket::decode(&samples.encode()).unwrap() {
            InputPacket::ClientSamples { kind, values } => {
                assert_eq!(kind, SAMPLE_E2E);
                assert_eq!(
                    values.iter().map(|&v| v as u64 / 1_000).collect::<Vec<_>>(),
                    vec![
                        e2e_sample_ms(12_340, Duration::from_millis(51)),
                        e2e_sample_ms(9_000, Duration::from_millis(30))
                    ],
                    "the tail and the HUD must be the same number, divided"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
        assert!(batch.take_chunk().is_none());
    }
}
