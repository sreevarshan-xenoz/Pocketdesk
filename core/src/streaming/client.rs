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
use super::input_packet::{InputPacket, SAMPLE_E2E, SAMPLE_RECOVERY, SampleBatch};
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

/// [`SESSION_MODE_MARKER`] mode: the phone's own screen on the computer
/// (T10k). The phone sends its screen ([`super::cast`]); the computer sends
/// nothing back but its ready stream and keyframe requests
/// ([`super::transport::STREAM_KIND_CAST_KEYFRAME`]).
pub const SESSION_MODE_CAST: u8 = 2;

/// [`SESSION_MODE_MARKER`] mode: sound both ways and nothing else (N5),
/// the phone as the computer's speaker and mic. The computer sends its
/// sound (with Audio) and takes the phone's mic (with Mic), and no picture
/// or input; it says when it is admitted with the ready stream.
pub const SESSION_MODE_SOUND: u8 = 3;

/// [`SESSION_MODE_MARKER`] mode: the phone's sound alone on the computer
/// (M7). The phone sends what its apps play as a cast's sound streams
/// ([`super::cast`]) and nothing else; the computer plays it, takes no
/// input, opens no window, and says when it is admitted with the ready
/// stream, whose byte says whether the sound is taken.
pub const SESSION_MODE_PHONE_SOUND: u8 = 4;

/// A sound session's ready stream carries one byte of what the computer
/// allows (N5 review): this bit marks the byte as said, so a computer that
/// says nothing reads as not known.
pub const SOUND_GRANT_SAID: u8 = 0x80;
/// The computer sends its sound (the phone's Audio permission).
pub const SOUND_GRANT_AUDIO: u8 = 1;
/// The computer plays the phone's mic (its Mic permission).
pub const SOUND_GRANT_MIC: u8 = 2;

/// What a sound session's ready stream says the computer allows, from its
/// payload: 0 when it says nothing.
pub fn sound_grant(payload: &[u8]) -> u8 {
    payload
        .first()
        .copied()
        .filter(|b| b & SOUND_GRANT_SAID != 0)
        .unwrap_or(0)
}

/// A screen session's ready stream carries one byte of what the computer
/// takes (E3): this bit marks the byte as said. Computers before E3 send a
/// screen session no ready stream, and read as taking none of it.
pub const SCREEN_FEATURES_SAID: u8 = 0x80;
/// The computer takes pen samples ([`super::InputPacket::Pen`]).
pub const SCREEN_FEATURE_PEN: u8 = 1;
/// The computer takes this session's input (BK7): without it the session
/// is view-only (its device's Input permission is off). Computers before
/// BK7 do not say it; their [`SCREEN_FEATURE_PEN`] meant the same.
pub const SCREEN_FEATURE_INPUT: u8 = 2;
/// The computer says which system it runs (PC4): with it,
/// [`SCREEN_FEATURE_WINDOWS`] is meaningful. Computers before PC4 do not
/// say it.
pub const SCREEN_FEATURE_SYSTEM_SAID: u8 = 4;
/// The computer runs Windows (else Linux), when
/// [`SCREEN_FEATURE_SYSTEM_SAID`] is set: another computer's window offers
/// the keys that system takes (Ctrl+Shift+Esc and Lock in place of
/// Ctrl+Alt+Del, which only a SYSTEM service can send there).
pub const SCREEN_FEATURE_WINDOWS: u8 = 8;
/// The computer says whether it plays this session's microphone (PC10
/// review): with it, [`SCREEN_FEATURE_MIC`] is meaningful. Computers
/// before it do not say it.
pub const SCREEN_FEATURE_MIC_SAID: u8 = 16;
/// The computer plays this session's microphone ([`super::InputPacket::Mic`])
/// when [`SCREEN_FEATURE_MIC_SAID`] is set: its "Use the phone as a
/// microphone" is on for this device, and, for another computer, the
/// session takes its input.
pub const SCREEN_FEATURE_MIC: u8 = 32;
/// The computer lands this session's absolute moves on the screen it shows
/// (PC11 on Linux), so another window onto another of its screens may take
/// control. Windows always has; computers before it do not say it, and a
/// Linux computer whose input backend spans its desktop in a way it cannot
/// place (an X server's enigo) leaves it off.
pub const SCREEN_FEATURE_PER_SCREEN: u8 = 64;

/// A remote's ready stream carries one byte of what the computer offers
/// it (G4): this bit marks the byte as said. Computers before G4 send a
/// remote's ready stream with no payload, and read as offering none of it.
pub const REMOTE_FEATURES_SAID: u8 = 0x80;
/// The computer draws a laser dot ([`super::InputPacket::Laser`]).
pub const REMOTE_FEATURE_LASER: u8 = 1;
/// A cast: the computer plays the phone's sound (K5), so the phone
/// captures it.
pub const REMOTE_FEATURE_PHONE_SOUND: u8 = 2;

/// What a remote's ready stream says the computer offers, from its
/// payload: 0 when it says nothing.
pub fn remote_features(payload: &[u8]) -> u8 {
    payload
        .first()
        .copied()
        .filter(|b| b & REMOTE_FEATURES_SAID != 0)
        .unwrap_or(0)
}

/// What a screen session's ready stream says the computer takes, from its
/// payload: 0 when it says nothing.
pub fn screen_features(payload: &[u8]) -> u8 {
    payload
        .first()
        .copied()
        .filter(|b| b & SCREEN_FEATURES_SAID != 0)
        .unwrap_or(0)
}

/// Whether the computer takes a screen session's input, from what its ready
/// stream said ([`screen_features`]): `None` until it says (BK7). A
/// computer before BK7 offered a pen exactly when it took input.
pub fn screen_takes_input(features: u8) -> Option<bool> {
    (features & SCREEN_FEATURES_SAID != 0)
        .then_some(features & (SCREEN_FEATURE_INPUT | SCREEN_FEATURE_PEN) != 0)
}

/// Whether the computer runs Windows, from what its screen session's ready
/// stream said ([`screen_features`]): `None` until it says (PC4).
pub fn screen_runs_windows(features: u8) -> Option<bool> {
    (features & SCREEN_FEATURES_SAID != 0 && features & SCREEN_FEATURE_SYSTEM_SAID != 0)
        .then_some(features & SCREEN_FEATURE_WINDOWS != 0)
}

/// Whether the computer lands a screen session's absolute moves on the
/// screen it shows, from what its ready stream said ([`screen_features`]):
/// `None` until it says. Windows always has, so a computer that runs it
/// (PC4) counts though it is from before the bit.
pub fn screen_input_per_screen(features: u8) -> Option<bool> {
    (features & SCREEN_FEATURES_SAID != 0).then_some(
        features & SCREEN_FEATURE_PER_SCREEN != 0 || screen_runs_windows(features) == Some(true),
    )
}

/// Whether the computer plays a screen session's microphone, from what its
/// ready stream said ([`screen_features`]): `None` until it says, or from a
/// computer before the PC10 review.
pub fn screen_takes_mic(features: u8) -> Option<bool> {
    (features & SCREEN_FEATURES_SAID != 0 && features & SCREEN_FEATURE_MIC_SAID != 0)
        .then_some(features & SCREEN_FEATURE_MIC != 0)
}

/// What a session is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionMode {
    /// The computer's screen on the phone.
    #[default]
    Screen,
    /// The phone as a remote: input, no picture (T10i).
    InputOnly,
    /// The phone's screen on the computer (T10k).
    Cast,
    /// Sound both ways, no picture or input (N5).
    Sound,
    /// The phone's sound on the computer, alone (M7).
    PhoneSound,
}

impl SessionMode {
    /// The byte the mode stream carries; `None` for [`SessionMode::Screen`],
    /// which sends none.
    pub fn wire(self) -> Option<u8> {
        match self {
            SessionMode::Screen => None,
            SessionMode::InputOnly => Some(SESSION_MODE_INPUT_ONLY),
            SessionMode::Cast => Some(SESSION_MODE_CAST),
            SessionMode::Sound => Some(SESSION_MODE_SOUND),
            SessionMode::PhoneSound => Some(SESSION_MODE_PHONE_SOUND),
        }
    }

    /// Whether the computer sends this session a picture and sound.
    pub fn gets_picture(self) -> bool {
        self == SessionMode::Screen
    }
}

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

/// What the client measures for the server's record, between flushes: the
/// receiver measures, the feedback loop reports.
#[derive(Default)]
struct ClientBatches {
    e2e: SampleBatch,
    /// Freezes (BG1): how long each took to recover.
    recovery: SampleBatch,
}

/// Send what the client measured since the last flush, in frame-sized
/// chunks: a stalled flush loop must not lose the samples it already holds,
/// only delay them.
async fn flush_samples(connection: &SharedConnection, samples: &ClientBatches) {
    for (kind, batch) in [
        (SAMPLE_E2E, &samples.e2e),
        (SAMPLE_RECOVERY, &samples.recovery),
    ] {
        while let Some(values) = batch.take_chunk() {
            send_frame(connection, &InputPacket::ClientSamples { kind, values }).await;
        }
    }
}

/// A freeze's length (BG1): from the first lost frame the client noticed (a
/// gap before a delta) to the next keyframe, which restarts the picture.
/// Gaps while a freeze runs belong to it; a keyframe with no freeze running
/// is a scheduled one and times nothing.
#[derive(Debug, Default)]
struct FreezeClock {
    since: Option<std::time::Instant>,
}

impl FreezeClock {
    /// A lost frame noticed at `now`.
    fn gap(&mut self, now: std::time::Instant) {
        self.since.get_or_insert(now);
    }

    /// A keyframe arrived at `now`: how long the freeze it ends lasted.
    fn keyframe(&mut self, now: std::time::Instant) -> Option<Duration> {
        self.since
            .take()
            .map(|since| now.saturating_duration_since(since))
    }
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

/// The state of a session that gets no picture (T10i, T10k), shared with
/// the receive loop.
#[derive(Debug, Clone, Default)]
pub struct RemoteFlags {
    /// The session gets no picture or sound: a remote or a cast.
    pub input_only: bool,
    /// The session is a cast (T10k), or the phone's sound alone (M7): the
    /// phone sends its stream.
    pub cast: bool,
    /// The session is the phone's sound alone (M7).
    pub phone_sound: bool,
    /// The session is sound only (N5): it takes the computer's sound, never
    /// a picture.
    pub sound: bool,
    /// The computer asked the cast for a keyframe; the sender takes it.
    pub keyframe_wanted: Arc<std::sync::atomic::AtomicBool>,
    /// The computer admitted it (its ready stream came).
    pub ready: Arc<std::sync::atomic::AtomicBool>,
    /// A sound session's grants as the computer said them ([`sound_grant`]):
    /// 0 until said, or from a computer that does not say.
    pub sound_granted: Arc<std::sync::atomic::AtomicU8>,
    /// A screen session's features as the computer said them
    /// ([`screen_features`], E3): 0 until said, or from a computer before E3.
    pub screen_features: Arc<std::sync::atomic::AtomicU8>,
    /// The computer sent a picture or sound anyway: it does not know
    /// input-only sessions, and the session was closed.
    pub too_old: Arc<std::sync::atomic::AtomicBool>,
    /// Control batches the computer sent a cast (C8), for the phone to take.
    pub control: Arc<ControlInbox>,
}

/// Control batches waiting for the phone's control service (C8): bounded,
/// the oldest dropped first, taken with a wait so nothing polls. A batch
/// that waited longer than [`ControlInbox::MAX_AGE`] here (a link that
/// stalled, then delivered a burst) is dropped rather than replayed late;
/// its age is this phone's own clock, never the computer's.
#[derive(Debug, Default)]
pub struct ControlInbox {
    batches: std::sync::Mutex<std::collections::VecDeque<(std::time::Instant, Vec<u8>)>>,
    arrived: std::sync::Condvar,
}

impl ControlInbox {
    /// Most batches kept: a phone that falls behind keeps the newest.
    pub const MAX: usize = 64;
    /// The longest a batch waits here before it is too late to perform.
    pub const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(2);

    /// A batch arrived.
    pub fn put(&self, batch: Vec<u8>) {
        self.put_at(batch, std::time::Instant::now());
    }

    fn put_at(&self, batch: Vec<u8>, now: std::time::Instant) {
        let mut batches = self.batches.lock().unwrap_or_else(|e| e.into_inner());
        if batches.len() >= Self::MAX {
            batches.pop_front();
        }
        batches.push_back((now, batch));
        self.arrived.notify_one();
    }

    /// The oldest waiting batch still fresh, waiting up to `timeout` for one.
    pub fn take(&self, timeout: std::time::Duration) -> Option<Vec<u8>> {
        let batches = self.batches.lock().unwrap_or_else(|e| e.into_inner());
        let (mut batches, _) = self
            .arrived
            .wait_timeout_while(batches, timeout, |b| b.is_empty())
            .unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        while let Some((at, batch)) = batches.pop_front() {
            if now.saturating_duration_since(at) <= Self::MAX_AGE {
                return Some(batch);
            }
        }
        None
    }
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
    /// `mode` says what the session is for: the computer's screen, the phone
    /// as a remote (T10i) or the phone's own screen (T10k).
    /// Returns the client and receiver channels for consuming video frames and audio packets.
    pub async fn connect(
        addr: &str,
        cert_manager: std::sync::Arc<CertManager>,
        monitor_index: Option<u32>,
        device_id: Option<&str>,
        iroh_id: Option<&str>,
        codec_caps: Option<u8>,
        mode: SessionMode,
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
                mode,
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
        mode: SessionMode,
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
            mode,
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
        mode: SessionMode,
        session_id: String,
    ) -> (
        Self,
        mpsc::Receiver<EncodedPacket>,
        mpsc::Receiver<AudioPacket>,
    ) {
        let input_only = !mode.gets_picture();
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

        // A remote (T10i) or a cast (T10k): say so, and send no codec caps.
        if let Some(wire) = mode.wire() {
            match connection.open_uni().await {
                Ok(mut mode_stream) => {
                    let buf = [SESSION_MODE_MARKER[0], SESSION_MODE_MARKER[1], wire];
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
                cast: matches!(mode, SessionMode::Cast | SessionMode::PhoneSound),
                phone_sound: mode == SessionMode::PhoneSound,
                sound: mode == SessionMode::Sound,
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
        let input_only = remote.input_only;

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
            let samples = Arc::<ClientBatches>::default();

            // Clone connection for the stats task before it gets moved into recv task
            let stats_connection = connection.clone();
            let stats_samples = samples.clone();

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
                        samples,
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
            let stats_every = feedback_interval(input_only);
            let stats_handle = tokio::spawn(
                async move {
                    send_stats_loop(&stats_connection, stats_cancel, &stats_samples, stats_every)
                        .await;
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
    /// The computer asks the cast for a keyframe (T10k).
    KeyframeWanted,
    /// The computer sends the cast control events (C8).
    Control,
    /// A picture or sound on an input-only session, or a picture on a
    /// sound session: the computer does not know the mode.
    TooOld,
    /// A picture or sound for a session that streams.
    Media,
}

fn remote_stream(kind: u8, input_only: bool, sound: bool) -> RemoteStream {
    if kind == transport::STREAM_KIND_READY {
        RemoteStream::Ready
    } else if sound && kind == transport::STREAM_KIND_AUDIO {
        // A sound session's whole point (N5).
        RemoteStream::Media
    } else if input_only && !sound && kind == transport::STREAM_KIND_CAST_KEYFRAME {
        RemoteStream::KeyframeWanted
    } else if input_only && !sound && kind == transport::STREAM_KIND_CAST_CONTROL {
        RemoteStream::Control
    } else if input_only {
        RemoteStream::TooOld
    } else {
        RemoteStream::Media
    }
}

/// The largest control batch a cast reads (C8).
const CONTROL_STREAM_MAX: usize = 16 * 1024;

#[cfg(test)]
mod remote_tests {
    use super::*;

    #[test]
    fn a_remote_takes_its_ready_stream_and_refuses_a_picture() {
        assert_eq!(
            remote_stream(transport::STREAM_KIND_READY, true, false),
            RemoteStream::Ready
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_VIDEO, true, false),
            RemoteStream::TooOld
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_AUDIO, true, false),
            RemoteStream::TooOld
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_VIDEO, false, false),
            RemoteStream::Media
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_CAST_KEYFRAME, true, false),
            RemoteStream::KeyframeWanted
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_CAST_CONTROL, true, false),
            RemoteStream::Control
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_CAST_CONTROL, false, false),
            RemoteStream::Media,
            "a session that streams is not a cast"
        );
    }

    #[test]
    fn control_waits_for_its_service_and_keeps_the_newest() {
        let inbox = ControlInbox::default();
        assert_eq!(inbox.take(std::time::Duration::from_millis(1)), None);
        for n in 0..(ControlInbox::MAX + 3) {
            inbox.put(vec![n as u8]);
        }
        assert_eq!(
            inbox.take(std::time::Duration::ZERO),
            Some(vec![3]),
            "the oldest three were dropped"
        );
        let inbox = std::sync::Arc::new(ControlInbox::default());
        let waiting = {
            let inbox = inbox.clone();
            std::thread::spawn(move || inbox.take(std::time::Duration::from_secs(5)))
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        inbox.put(vec![9]);
        assert_eq!(
            waiting.join().unwrap(),
            Some(vec![9]),
            "a waiting take wakes"
        );
    }

    #[test]
    fn control_that_waited_too_long_is_not_performed() {
        let inbox = ControlInbox::default();
        let long_ago = std::time::Instant::now()
            - ControlInbox::MAX_AGE
            - std::time::Duration::from_millis(10);
        inbox.put_at(vec![1], long_ago);
        inbox.put_at(vec![2], long_ago);
        inbox.put(vec![3]);
        assert_eq!(
            inbox.take(std::time::Duration::ZERO),
            Some(vec![3]),
            "the stale two were dropped, the fresh one kept"
        );
        inbox.put_at(vec![4], long_ago);
        assert_eq!(inbox.take(std::time::Duration::ZERO), None);
    }

    #[test]
    fn a_sound_session_hears_what_the_computer_granted() {
        // The computer says which of its sound and the phone's mic it allows.
        assert_eq!(
            sound_grant(&[SOUND_GRANT_SAID | SOUND_GRANT_AUDIO]),
            SOUND_GRANT_SAID | SOUND_GRANT_AUDIO
        );
        assert_eq!(
            sound_grant(&[SOUND_GRANT_SAID | SOUND_GRANT_AUDIO | SOUND_GRANT_MIC]),
            SOUND_GRANT_SAID | SOUND_GRANT_AUDIO | SOUND_GRANT_MIC
        );
        // Nothing said (an empty ready stream, or a byte without the mark):
        // not known.
        assert_eq!(sound_grant(&[]), 0);
        assert_eq!(sound_grant(&[SOUND_GRANT_AUDIO]), 0);
    }

    #[test]
    fn a_screen_session_hears_whether_the_computer_takes_a_pen() {
        assert_eq!(
            screen_features(&[SCREEN_FEATURES_SAID | SCREEN_FEATURE_PEN]) & SCREEN_FEATURE_PEN,
            SCREEN_FEATURE_PEN
        );
        // BK7: input taken, as said, or as an older computer's pen said it.
        assert_eq!(
            screen_takes_input(SCREEN_FEATURES_SAID | SCREEN_FEATURE_INPUT),
            Some(true)
        );
        assert_eq!(
            screen_takes_input(SCREEN_FEATURES_SAID | SCREEN_FEATURE_PEN),
            Some(true)
        );
        assert_eq!(screen_takes_input(SCREEN_FEATURES_SAID), Some(false));
        assert_eq!(screen_takes_input(0), None, "not said yet");
        // Nothing said: no pen offered.
        assert_eq!(screen_features(&[]), 0);
        assert_eq!(screen_features(&[SCREEN_FEATURE_PEN]), 0);
        // G4: a remote's byte, said or not.
        assert_eq!(
            remote_features(&[REMOTE_FEATURES_SAID | REMOTE_FEATURE_LASER]) & REMOTE_FEATURE_LASER,
            REMOTE_FEATURE_LASER
        );
        assert_eq!(remote_features(&[]), 0);
        assert_eq!(remote_features(&[REMOTE_FEATURE_LASER]), 0);
        // A screen session's ready stream is read, never fed to the decoder.
        assert_eq!(
            remote_stream(transport::STREAM_KIND_READY, false, false),
            RemoteStream::Ready
        );
    }

    /// PC4: a screen session says which system its computer runs; one
    /// before PC4 says nothing of it.
    #[test]
    fn a_screen_session_says_its_system() {
        let said = SCREEN_FEATURES_SAID | SCREEN_FEATURE_SYSTEM_SAID;
        assert_eq!(
            screen_runs_windows(said | SCREEN_FEATURE_WINDOWS),
            Some(true)
        );
        assert_eq!(screen_runs_windows(said), Some(false), "Linux");
        assert_eq!(
            screen_runs_windows(SCREEN_FEATURES_SAID | SCREEN_FEATURE_INPUT),
            None,
            "a computer before PC4"
        );
        assert_eq!(screen_runs_windows(SCREEN_FEATURE_SYSTEM_SAID), None);
    }

    /// PC10 review: a screen session says whether its microphone is
    /// played; one before says nothing of it.
    #[test]
    fn a_screen_session_says_whether_its_mic_is_played() {
        let said = SCREEN_FEATURES_SAID | SCREEN_FEATURE_MIC_SAID;
        assert_eq!(screen_takes_mic(said | SCREEN_FEATURE_MIC), Some(true));
        assert_eq!(screen_takes_mic(said), Some(false));
        assert_eq!(
            screen_takes_mic(SCREEN_FEATURES_SAID | SCREEN_FEATURE_INPUT),
            None,
            "a computer before it"
        );
    }

    #[test]
    fn a_sound_session_takes_sound_and_refuses_a_picture() {
        assert_eq!(
            remote_stream(transport::STREAM_KIND_READY, true, true),
            RemoteStream::Ready
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_AUDIO, true, true),
            RemoteStream::Media
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_VIDEO, true, true),
            RemoteStream::TooOld,
            "a computer that streams the screen does not know the mode"
        );
        assert_eq!(
            remote_stream(transport::STREAM_KIND_CAST_KEYFRAME, true, true),
            RemoteStream::TooOld
        );
    }

    #[test]
    fn each_mode_says_itself_and_whether_it_gets_a_picture() {
        assert_eq!(SessionMode::Screen.wire(), None);
        assert_eq!(SessionMode::InputOnly.wire(), Some(SESSION_MODE_INPUT_ONLY));
        assert_eq!(SessionMode::Cast.wire(), Some(SESSION_MODE_CAST));
        assert_eq!(SessionMode::Sound.wire(), Some(SESSION_MODE_SOUND));
        assert_eq!(
            SessionMode::PhoneSound.wire(),
            Some(SESSION_MODE_PHONE_SOUND)
        );
        assert!(!SessionMode::PhoneSound.gets_picture());
        assert!(SessionMode::Screen.gets_picture());
        assert!(!SessionMode::Cast.gets_picture());
        assert!(!SessionMode::Sound.gets_picture());
    }

    #[test]
    fn a_remote_reports_its_link_less_often_but_well_within_the_silence_limit() {
        assert_eq!(feedback_interval(false), Duration::from_secs(1));
        // The server ends a session it has not heard from in 30 s.
        assert!(feedback_interval(true) > feedback_interval(false));
        assert!(feedback_interval(true) * 3 < Duration::from_secs(30));
    }
}

/// How often the client reports its link: every second for a picture, whose
/// rate the server steers by it; every 5 s for a remote (T10i), which only
/// has to stay well inside the server's 30 s silence limit, so the phone's
/// radio rests between reports.
fn feedback_interval(input_only: bool) -> Duration {
    Duration::from_secs(if input_only { 5 } else { 1 })
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
    samples: Arc<ClientBatches>,
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
    let mut freeze = FreezeClock::default();
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
                        match remote_stream(header.stream_kind, remote.input_only, remote.sound) {
                            RemoteStream::Ready => {
                                // A screen session hears what the computer
                                // takes (E3); it was admitted already, by its
                                // picture.
                                if !remote.input_only {
                                    if let Ok(payload) = recv_stream.read_to_end(16).await {
                                        remote.screen_features.store(
                                            screen_features(&payload),
                                            std::sync::atomic::Ordering::Release,
                                        );
                                    }
                                    continue;
                                }
                                // A sound session hears what it may do (N5).
                                if remote.sound
                                    && let Ok(payload) = recv_stream.read_to_end(16).await
                                {
                                    remote.sound_granted.store(
                                        sound_grant(&payload),
                                        std::sync::atomic::Ordering::Release,
                                    );
                                } else if !remote.sound
                                    && let Ok(payload) = recv_stream.read_to_end(16).await
                                {
                                    // A remote hears what it is offered (G4),
                                    // kept where a screen session keeps its own.
                                    remote.screen_features.store(
                                        remote_features(&payload),
                                        std::sync::atomic::Ordering::Release,
                                    );
                                }
                                remote.ready.store(true, std::sync::atomic::Ordering::Release);
                                info!("The computer admitted the input-only session");
                                continue;
                            }
                            RemoteStream::KeyframeWanted => {
                                remote
                                    .keyframe_wanted
                                    .store(true, std::sync::atomic::Ordering::Release);
                                continue;
                            }
                            RemoteStream::Control => {
                                // Only a cast takes control (C8), and only
                                // what it can read.
                                if remote.cast
                                    && let Ok(batch) =
                                        recv_stream.read_to_end(CONTROL_STREAM_MAX).await
                                    && !batch.is_empty()
                                {
                                    remote.control.put(batch);
                                }
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
                                freeze.gap(now);
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
                            if header.is_keyframe
                                && let Some(took) = freeze.keyframe(std::time::Instant::now())
                            {
                                debug!(ms = took.as_millis() as u64, "Recovered from a freeze");
                                samples.recovery.push(took.as_micros() as u64);
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
                            samples.e2e.push(e2e_sample_us(header.timestamp_us, rtt));

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
    samples: &ClientBatches,
    every: Duration,
) {
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // What was measured since the last tick goes too (BG1
                // review: a freeze that ended in the last second was lost).
                flush_samples(connection, samples).await;
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

                flush_samples(connection, samples).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BG1: a freeze runs from the first lost frame to the next keyframe;
    /// later gaps belong to it, and a scheduled keyframe times nothing.
    #[test]
    fn a_freeze_is_timed_from_its_first_gap_to_the_next_keyframe() {
        let start = std::time::Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut freeze = FreezeClock::default();
        assert_eq!(freeze.keyframe(at(10)), None, "a scheduled keyframe");
        freeze.gap(at(100));
        freeze.gap(at(250));
        assert_eq!(freeze.keyframe(at(400)), Some(Duration::from_millis(300)));
        assert_eq!(freeze.keyframe(at(450)), None, "over");
        freeze.gap(at(500));
        assert_eq!(freeze.keyframe(at(520)), Some(Duration::from_millis(20)));
    }

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
