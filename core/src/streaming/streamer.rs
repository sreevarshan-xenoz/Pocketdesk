//! Streaming loop orchestration
//!
//! Wires together capture → encoder → QUIC send into a single coordinated pipeline.
//! Manages lifecycle, error handling, and graceful shutdown.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, info, warn};
use uuid::Uuid;

use super::audio::{AudioConfig, AudioEncoder as AudioOpusEncoder};
use super::audio_capture;
use super::bitrate::LossCeiling;
use super::capture;
use super::connection::{Connection, QuinnConnection, SharedConnection};
use super::encoder::VideoEncoder;
use super::input_packet::{InputPacket, PRESET_AUTO, preset_bitrate_ceil};
use super::session_telemetry::{SessionOutcome, SessionRecorder};
use super::transport::{self, CertManager, StreamServer, StreamTransportConfig};
use super::{EncodedPacket, StreamingConfig, VideoCodec, VideoFrame};
use crate::protocol::permissions::{Capability, DevicePermissions};

/// What a session gate answers, eventually: the session's permissions, or
/// `None` to reject it (see [`StreamingServer::set_session_gate`]).
pub type SessionGateFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<DevicePermissions>> + Send>>;
type SessionGate =
    std::sync::Arc<dyn Fn(SessionClaim) -> SessionGateFuture + Send + Sync + 'static>;

/// What a session says about itself, and what its connection proved, for
/// the session gate to judge.
#[derive(Debug, Clone)]
pub struct SessionClaim {
    /// The device id the client announced (a claim, not a proof).
    pub device_id: Option<String>,
    /// Where it connected from, for the owner's approval prompt.
    pub peer: std::net::SocketAddr,
    /// The key the connection proved it holds (device keys).
    pub peer_key: Option<super::PeerKey>,
    /// The WAN key the client says it dials with, on this session.
    pub iroh_id: Option<String>,
}

/// What a client announced in its config streams.
#[derive(Debug, Default)]
struct ClientClaim {
    device_id: Option<String>,
    iroh_id: Option<String>,
}

/// Where a whole-monitor session's absolute pointer input has to land.
///
/// The phone normalizes such a tap over the video frame, which is one
/// monitor. Windows injects absolute moves over the whole desktop layout,
/// so with two monitors a tap in the middle of the stream landed in the
/// middle of *both* screens. A cropped (single-window) session needs no
/// remap: the phone already normalizes those over the layout box the
/// server reports. Linux keeps the frame-normalized value — whether its
/// uinput absolute device spans the layout on a multi-monitor desktop has
/// not been measured.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PointerSpace {
    monitor: (i32, i32, u32, u32),
    layout: (i32, i32, u32, u32),
}

impl PointerSpace {
    /// The space for a session capturing monitor `index`; `None` when the
    /// injector's space already is the frame.
    fn for_monitor(index: u32) -> Option<Self> {
        if !cfg!(windows) {
            return None;
        }
        let space = Self {
            monitor: capture::monitor_rect(index)?,
            layout: capture::layout_rect()?,
        };
        (space.monitor != space.layout).then_some(space)
    }

    /// A frame-normalized point (0..=65535 per axis) as a layout-normalized one.
    fn remap(&self, x_norm: u16, y_norm: u16) -> (u16, u16) {
        fn axis(norm: u16, start: i32, len: u32, space_start: i32, space_len: u32) -> u16 {
            let max = f64::from(u16::MAX);
            let pixel = f64::from(start) + f64::from(norm) / max * f64::from(len);
            let scaled = (pixel - f64::from(space_start)) / f64::from(space_len.max(1)) * max;
            scaled.round().clamp(0.0, max) as u16
        }
        let (mx, my, mw, mh) = self.monitor;
        let (lx, ly, lw, lh) = self.layout;
        (axis(x_norm, mx, mw, lx, lw), axis(y_norm, my, mh, ly, lh))
    }
}

/// R4 A3: bitrate ceiling applied while an iroh session rides a relay —
/// relay bandwidth is shared and not ours to saturate. Conservative on
/// purpose: a punched-direct path restores the configured rate.
const RELAY_BITRATE_CAP_BPS: u32 = 2_000_000;

/// Controls the streaming server lifecycle
pub struct StreamingServer {
    config: StreamingConfig,
    transport_config: StreamTransportConfig,
    cert_manager: std::sync::Arc<CertManager>,
    cancel: CancellationToken,
    /// Unique session ID for the lifetime of this server instance
    session_id: String,
    /// Watch channel for adaptive bitrate updates
    bitrate_tx: watch::Sender<u32>,
    /// Channel for routing input events received from client over this QUIC connection
    input_tx: Option<tokio::sync::broadcast::Sender<InputPacket>>,
    /// R4 E2: routing for client→server mic audio. `InputPacket::Mic` frames
    /// are intercepted by the monitor task (never injected, never dropped by
    /// view-only) and forwarded here; the server crate's mic relay decodes
    /// them into a PipeWire source. Bounded with drop-on-full: stale mic
    /// audio is worse than gaps. If unset, mic packets are discarded.
    mic_tx: Option<tokio::sync::mpsc::Sender<InputPacket>>,
    /// Optional session gate (pairing, F5 permissions and approval): called
    /// with the device id the client announced in-band (None if it sent
    /// none); `None` rejects the session before capture starts, `Some` is what
    /// it may do. See `set_session_gate`.
    session_gate: Option<SessionGate>,
    /// Record per-session outcome telemetry (R4 A2) through the sink
    /// registered in `session_telemetry`. Off by default.
    telemetry: bool,
    /// R4 D1 view-only latch (per session — a `StreamingServer` instance is
    /// built per connection). Set by `InputPacket::ViewOnly`; while true the
    /// monitor task forwards no injectable input, only control packets.
    view_only: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// R4 A3 user override of the relay bitrate floor: set by
    /// `InputPacket::FullQuality`, read by the relay-guard task.
    full_quality: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// R4 E5 link-profile preset id ([`PRESET_AUTO`]..`PRESET_ECONOMY`),
    /// set by `InputPacket::QualityPreset`. The bitrate-arbiter task folds
    /// it with the A3 relay floor into the live encoder bitrate. Per
    /// session (a `StreamingServer` is built per connection), so it resets
    /// to `Auto` on reconnect.
    preset: std::sync::Arc<AtomicU8>,
    /// R4 C1 server half of the codec negotiation: even when the client
    /// declares HEVC support, H.265 is only picked if the operator allowed
    /// it (`Config::allow_hevc` — HEVC encoder availability is the
    /// operator's box, and the fallback ladder for a failed open is C2).
    hevc_allowed: bool,
    /// R4 B3 capture backend selection: `Auto` detects + falls back, an
    /// explicit variant pins the pipeline (`Config::capture_backend`).
    capture_backend: super::CaptureBackend,
}

impl StreamingServer {
    /// Create a new streaming server with the given configurations
    pub fn new(
        config: StreamingConfig,
        transport_config: StreamTransportConfig,
        cert_manager: std::sync::Arc<CertManager>,
    ) -> Self {
        let (bitrate_tx, _) = watch::channel(config.bitrate_bps);
        Self {
            config,
            transport_config,
            cert_manager,
            cancel: CancellationToken::new(),
            session_id: Uuid::new_v4().to_string(),
            bitrate_tx,
            input_tx: None,
            mic_tx: None,
            session_gate: None,
            telemetry: false,
            view_only: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            full_quality: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            preset: std::sync::Arc::new(AtomicU8::new(PRESET_AUTO)),
            hevc_allowed: false,
            capture_backend: super::CaptureBackend::default(),
        }
    }

    /// Enable per-session outcome telemetry (R4 A2). Reports are emitted
    /// through the sink registered via
    /// `session_telemetry::set_session_telemetry_callback`.
    pub fn set_session_telemetry(&mut self, enabled: bool) {
        self.telemetry = enabled;
    }

    /// Permit H.265/HEVC when the connecting client declares it can decode
    /// it (R4 C1). Off by default: sessions stay H.264.
    pub fn set_hevc_allowed(&mut self, allowed: bool) {
        self.hevc_allowed = allowed;
    }

    /// Choose the capture backend (R4 B3). `Auto` (default) detects the
    /// display server and falls back; an explicit variant pins the pipeline.
    pub fn set_capture_backend(&mut self, backend: super::CaptureBackend) {
        self.capture_backend = backend;
    }

    /// Set a channel to receive input events from the remote client.
    ///
    /// When set, the server will parse incoming QUIC streams as binary `InputPacket`
    /// values and forward them through this channel for injection on the host system.
    pub fn set_input_channel(&mut self, tx: tokio::sync::broadcast::Sender<InputPacket>) {
        self.input_tx = Some(tx);
    }

    /// Set a channel to receive `InputPacket::Mic` frames from the client
    /// (R4 E2 reverse audio). See the `mic_tx` field docs.
    pub fn set_mic_channel(&mut self, tx: tokio::sync::mpsc::Sender<InputPacket>) {
        self.mic_tx = Some(tx);
    }

    /// Decide each session before any video/input pipeline starts: `None`
    /// rejects it (not paired, or denied), `Some` is what it may do (F5). The
    /// gate is async so it can wait for the owner's approval.
    ///
    /// The gate judges the device id the client announces in-band against
    /// the key its connection proved (`SessionClaim::peer_key`: the TLS
    /// client certificate on QUIC, the endpoint key on iroh). A client that
    /// announces nothing fails the gate while one is set. The peer address
    /// is for the owner's approval prompt, not for judging the device.
    pub fn set_session_gate<F>(&mut self, gate: F)
    where
        F: Fn(SessionClaim) -> SessionGateFuture + Send + Sync + 'static,
    {
        self.session_gate = Some(std::sync::Arc::new(gate));
    }

    /// Update the target bitrate dynamically (for adaptive bitrate control)
    pub fn update_bitrate(&self, new_bitrate_bps: u32) {
        let _ = self.bitrate_tx.send_if_modified(|current| {
            if *current != new_bitrate_bps {
                *current = new_bitrate_bps;
                info!("Adaptive bitrate: {} bps", new_bitrate_bps);
                true
            } else {
                false
            }
        });
    }

    /// Get a receiver for bitrate updates
    pub fn bitrate_watcher(&self) -> watch::Receiver<u32> {
        self.bitrate_tx.subscribe()
    }

    /// Run the streaming pipeline on an existing connection.
    /// This is useful when the connection was already accepted by a unified
    /// multiplexer; `connection` is transport-agnostic (quinn today, iroh
    /// once R1 lands its endpoint impl).
    pub async fn run_on_connection(&mut self, connection: SharedConnection) -> Result<()> {
        let conn_id = Uuid::new_v4().to_string();
        let span = tracing::info_span!(
            "stream_session",
            session = %self.session_id,
            conn = %conn_id,
            proto = %crate::protocol::PROTOCOL_VERSION,
            peer = tracing::field::Empty,
            transport = "quic"
        );

        async {
            let peer = connection.remote_address();
            tracing::Span::current().record("peer", peer.to_string());
            info!(
                "Streaming client connected (v1 over v2-capable endpoint): {}",
                peer
            );

            // 3. Run the capture → encode → send pipeline
            self.run_pipeline(connection).await
        }
        .instrument(span)
        .await
    }

    /// Start the streaming server — accepts one client connection and runs the pipeline
    pub async fn run(&mut self) -> Result<()> {
        let conn_id = Uuid::new_v4().to_string();
        let span = tracing::info_span!(
            "stream_session",
            session = %self.session_id,
            conn = %conn_id,
            proto = %crate::protocol::PROTOCOL_VERSION,
            peer = tracing::field::Empty,
            transport = "quic"
        );

        use tracing::Instrument;

        async {
            info!(
                "Starting streaming server: {}x{}@{}fps, {}bps",
                self.config.width, self.config.height, self.config.fps, self.config.bitrate_bps
            );

            // 1. Create QUIC server endpoint
            let server = StreamServer::new(self.transport_config.clone(), &self.cert_manager)
                .await
                .context("Failed to create QUIC server")?;

            let local_addr = server.local_addr()?;
            info!("Streaming server listening on {}", local_addr);

            // 2. Wait for incoming connection
            let incoming = tokio::select! {
                Some(conn) = server.accept_connection() => conn,
                _ = self.cancel.cancelled() => {
                    info!("Streaming server cancelled while waiting for connection");
                    return Ok(());
                }
            };

            let connection = incoming.await.context("Failed to accept QUIC connection")?;
            let connection = QuinnConnection::shared(connection);

            let peer = connection.remote_address();
            tracing::Span::current().record("peer", peer.to_string());
            info!("Streaming client connected: {}", peer);

            // 3. Run the capture → encode → send pipeline
            self.run_pipeline(connection).await
        }
        .instrument(span)
        .await
    }

    /// Run the full streaming pipeline for a single connection
    async fn run_pipeline(&mut self, connection: SharedConnection) -> Result<()> {
        // Read optional client config streams before starting the pipeline.
        let claim = read_client_config(&connection, &mut self.config, self.hevc_allowed).await;
        let device_id = claim.device_id.clone();

        // R4 A2 session telemetry: a shared recorder observes the connection
        // on a slow poll and an RAII guard emits the outcome report when the
        // pipeline function exits — including via `?`, which is why the
        // guard is armed before the first fallible statement below.
        let telemetry = self.telemetry.then(|| {
            (
                std::sync::Arc::new(SessionRecorder::new(
                    connection.transport_family(),
                    device_id.clone(),
                )),
                std::sync::Arc::new(AtomicU64::new(0)),
            )
        });
        let _telemetry_guard = telemetry.as_ref().map(|(r, b)| r.guard(b.clone()));

        // F5: what this session may do. No gate (pairing not required) means
        // the defaults.
        let permissions = match &self.session_gate {
            None => DevicePermissions::default(),
            Some(gate) => match gate(SessionClaim {
                device_id: device_id.clone(),
                peer: connection.remote_address(),
                peer_key: connection.peer_key(),
                iroh_id: claim.iroh_id.clone(),
            })
            .await
            {
                Some(permissions) => permissions,
                None => {
                    warn!(
                        device_id = device_id.as_deref().unwrap_or("<none announced>"),
                        "Streaming session rejected: device not paired or not approved"
                    );
                    if let Some(guard) = &_telemetry_guard {
                        guard.record(SessionOutcome::Rejected);
                    }
                    connection.close(0u32, b"session refused");
                    return Err(anyhow::anyhow!(
                        "session refused: the device is not paired, or the owner did not approve it"
                    ));
                }
            },
        };
        // Without input, the session is view-only whatever the phone asks:
        // the phone's own view-only switch can tighten this, never lift it.
        let force_view_only = !permissions.allows(Capability::Input);
        let allow_mic = permissions.allows(Capability::Mic);
        let allow_audio = permissions.allows(Capability::Audio);
        if force_view_only || !allow_mic || !allow_audio {
            info!(
                force_view_only,
                allow_mic, allow_audio, "Session limited by device permissions"
            );
        }

        // R4 D2: publish the accepted session in the live registry so the
        // desktop can list it (`linux-link status`) or end it
        // (`linux-link kick`); the handle deregisters when this function
        // exits by any path.
        let _live_session = super::sessions::register(device_id.clone(), &connection);

        let cancel = self.cancel.clone();
        let (frame_tx, mut frame_rx) = mpsc::channel::<VideoFrame>(2);
        let (packet_tx, mut packet_rx) = mpsc::channel::<EncodedPacket>(8);

        // IDR requests: raised by the monitor task when the client reports a
        // sequence gap. The encode task consumes it and forces a keyframe.
        let (keyframe_tx, mut keyframe_rx) = watch::channel(0u64);

        // Window crop: set by the monitor task from client WindowCrop packets.
        // The encode task applies the rect to each frame and rebuilds the
        // encoder at the crop resolution (so the negotiated window size
        // actually reaches the pipeline, not just the session config).
        let (crop_tx, crop_rx) = watch::channel(None::<(u32, u32, u32, u32)>);

        // R4 B2: the same packet's Hyprland window address (0 = geometry
        // crop only). The screencopy capture thread consumes it and flips
        // `window_mode` while it is actually emitting per-window frames —
        // the encode task must then skip the software crop, but only once
        // the compositor confirms (non-Hyprland servers keep the old
        // software-crop behavior with the same packet).
        let (window_tx, window_rx) = watch::channel(0u64);
        let window_mode = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut tasks = JoinSet::new();

        // Task 1: Screen capture (runs on dedicated OS thread internally)
        let capture_config = self.config.clone();
        let _capture_bitrate_rx = self.bitrate_watcher();
        let capture_cancel = cancel.clone();
        let capture_session = capture::start_capture_auto(
            capture_config,
            frame_tx,
            capture_cancel,
            window_rx.clone(),
            window_mode.clone(),
            self.capture_backend,
        )
        .await
        .context("Failed to start screen capture")?;

        info!("Screen capture session started");

        // Clone connection for use across multiple tasks
        let conn_for_transport = connection.clone();
        let conn_for_monitor = connection.clone();
        let _conn_for_audio = connection.clone();

        // Task 2: Video encoding
        // Clone config for encoder (bitrate will be updated via watch channel)
        let mut encoder_config = self.config.clone();
        let mut encoder_bitrate_rx = self.bitrate_watcher();

        // Update encoder config with current bitrate
        encoder_config.bitrate_bps = *encoder_bitrate_rx.borrow();

        let mut encoder =
            VideoEncoder::new(encoder_config).context("Failed to create video encoder")?;
        // R4 C2: the rung we actually landed on after any startup fallback.
        // Resolution rebuilds clone this (not `base_config`) so a session
        // that fell back to hardware→software at startup doesn't oscillate
        // back through the failing hardware probe on every window crop.
        let mut encoder_preferred = encoder.config().clone();
        let encode_crop_rx = crop_rx.clone();
        let encode_window_rx = window_rx.clone();
        let encode_window_mode = window_mode.clone();

        // Spawn encoding task — reads frames, produces packets
        let encode_cancel = cancel.clone();
        let encode_recorder = telemetry.as_ref().map(|t| t.0.clone());
        let encode_span = tracing::info_span!("video_encode");
        tasks.spawn(async move {
            info!("Encoding task started");
            let mut packets_encoded = 0u64;
            let mut frames_dropped = 0u64;
            // The encoder the record names (F10), noted when it changes.
            let mut noted_encoder: Option<&'static str> = None;
            let mut crop: Option<(u32, u32, u32, u32)> = None;
            let mut crop_rx = encode_crop_rx;
            let mut window_rx = encode_window_rx;
            // True while the capture thread is actually emitting
            // compositor-cropped per-window frames (R4 B2).
            let window_mode = encode_window_mode;
            // C2 supervisor state: the encode task used to log an Err and
            // keep looping forever against a dead encoder. Now consecutive
            // failures (or frames consumed with no output for STALL_WINDOW)
            // rebuild the encoder one rung down; the bottom rung gets a
            // hard failure budget before the session ends.
            let mut encode_errs = 0u64;
            let mut frames_since_packet = 0u64;
            let mut last_packet_at = Instant::now();
            let mut used_fallback_rung = false;
            const ERR_REBUILD_AT: u64 = 5;
            const ERR_END_AT: u64 = 90;
            const STALL_FRAMES: u64 = 30;
            const STALL_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

            loop {
                tokio::select! {
                    biased;

                    _ = encode_cancel.cancelled() => {
                        info!("Encoding task cancelled");
                        break;
                    }

                    // Immediate-IDR requests (transport fell behind / client gap)
                    Ok(()) = keyframe_rx.changed() => {
                        info!("Forcing IDR on encoder");
                        encoder.request_keyframe();
                    }

                    // Window crop changes. Size changes are handled by the
                    // frame path (encoder must match incoming frame dims);
                    // same-size rect moves only need a fresh IDR.
                    Ok(()) = crop_rx.changed() => {
                        let new = *crop_rx.borrow_and_update();
                        if new != crop {
                            info!(?new, "Window crop updated");
                            if matches!(
                                (new, crop),
                                (Some((_, _, w, h)), Some((_, _, w2, h2))) if w == w2 && h == h2
                            ) {
                                encoder.request_keyframe();
                            }
                            crop = new;
                        }
                    }

                    // R4 B2: a new window handle switches the capture
                    // thread to compositor-side window capture. Force an
                    // IDR so the client reseeds against the different
                    // frame source; the software crop is skipped while
                    // `window_mode` reports it is live.
                    Ok(()) = window_rx.changed() => {
                        let addr = *window_rx.borrow_and_update();
                        info!(addr, "Window capture target changed");
                        encoder.request_keyframe();
                    }

                    // Bitrate changes (R4 A3 relay floor / E5 HUD preset)
                    // are baked into the encoder's FFmpeg command (rate
                    // control + VBV + GOP), so applying one means a rebuild.
                    // Reuse the sticky preferred config so a rebuild never
                    // re-probes a hardware rung that already failed (C2), and
                    // keep the current frame resolution — only the rate moves.
                    Ok(()) = encoder_bitrate_rx.changed() => {
                        let new_bitrate = *encoder_bitrate_rx.borrow_and_update();
                        if new_bitrate != encoder.config().bitrate_bps {
                            let mut cfg = encoder_preferred.clone();
                            cfg.bitrate_bps = new_bitrate;
                            let rebuilt = match VideoEncoder::new(cfg.clone()) {
                                Ok(e) => Ok(e),
                                Err(e) => {
                                    error!(
                                        error = %e,
                                        "Bitrate rebuild failed; degrading to software"
                                    );
                                    VideoEncoder::new_software(cfg)
                                }
                            };
                            match rebuilt {
                                Ok(e) => {
                                    info!(
                                        bitrate_bps = new_bitrate,
                                        backend = e.backend_name(),
                                        "Encoder rebuilt for new bitrate"
                                    );
                                    encoder_preferred = e.config().clone();
                                    encoder = e;
                                    encode_errs = 0;
                                    frames_since_packet = 0;
                                    last_packet_at = Instant::now();
                                }
                                Err(e) => {
                                    error!(error = %e, "Bitrate rebuild failed; ending session");
                                    encode_cancel.cancel();
                                    break;
                                }
                            }
                        } else {
                            debug!("Bitrate unchanged ({} bps)", new_bitrate);
                        }
                    }

                    // Process next frame
                    Some(mut frame) = frame_rx.recv() => {
                        // In compositor-side window mode the frames already
                        // are exactly the window — the software crop would
                        // cut into it a second time.
                        let software_crop = !window_mode.load(Ordering::Relaxed);
                        if software_crop
                            && let Some((cx, cy, cw, ch)) = crop
                            && !frame.crop_region(cx, cy, cw, ch)
                        {
                            // Crop rect lies outside the captured frame
                            // (stale window geometry) — drop, don't show
                            // the wrong region.
                            frames_dropped += 1;
                            continue;
                        }

                        // Adopt the frame resolution into the encoder: the
                        // crop size (or the real capture size) must be what
                        // FFmpeg was configured with, not the session default.
                        if frame.width != encoder.config().width
                            || frame.height != encoder.config().height
                        {
                            let mut cfg = encoder_preferred.clone();
                            cfg.width = frame.width;
                            cfg.height = frame.height;
                            cfg.bitrate_bps = *encoder_bitrate_rx.borrow_and_update();
                            let rebuilt = match VideoEncoder::new(cfg.clone()) {
                                Ok(e) => Ok(e),
                                Err(e) => {
                                    // C2: a resolution rebuild that can't open
                                    // its preferred rung degrades one rung
                                    // rather than killing the session.
                                    error!(
                                        error = %e,
                                        "Preferred-rung rebuild failed; degrading to software"
                                    );
                                    VideoEncoder::new_software(cfg)
                                }
                            };
                            match rebuilt {
                                Ok(e) => {
                                    info!(
                                        width = frame.width,
                                        height = frame.height,
                                        backend = e.backend_name(),
                                        "Encoder rebuilt at frame resolution"
                                    );
                                    encoder_preferred = e.config().clone();
                                    encoder = e;
                                    encode_errs = 0;
                                    frames_since_packet = 0;
                                    last_packet_at = Instant::now();
                                }
                                Err(e) => {
                                    error!(error = %e, "Failed to rebuild encoder; ending session");
                                    encode_cancel.cancel();
                                    break;
                                }
                            }
                        }

                        // C2: rebuild one rung down after repeated failures
                        // or a sustained no-output stall. Returns false when
                        // the encoder is already on the bottom rung and has
                        // spent its budget — caller must end the session.
                        macro_rules! fallback_rung {
                            ($reason:expr) => {{
                                if used_fallback_rung {
                                    error!(reason = $reason, "Encoder bottom rung exhausted; ending session");
                                    encode_cancel.cancel();
                                    break;
                                }
                                let mut cfg = encoder_preferred.clone();
                                cfg.width = encoder.config().width;
                                cfg.height = encoder.config().height;
                                cfg.bitrate_bps = *encoder_bitrate_rx.borrow();
                                match VideoEncoder::new_software(cfg) {
                                    Ok(e) => {
                                        warn!(
                                            reason = $reason,
                                            backend = e.backend_name(),
                                            "Rebuilt encoder on software rung (C2)"
                                        );
                                        used_fallback_rung = true;
                                        encoder_preferred = e.config().clone();
                                        encoder = e;
                                        encode_errs = 0;
                                        frames_since_packet = 0;
                                        last_packet_at = Instant::now();
                                    }
                                    Err(e) => {
                                        error!(
                                            error = %e,
                                            "Software-rung rebuild failed; ending session"
                                        );
                                        encode_cancel.cancel();
                                        break;
                                    }
                                }
                            }};
                        }

                        // Only successful encodes are sampled: a failed one tells
                        // the C2 ladder's story, not the frame-pacing story.
                        let encode_started = Instant::now();
                        match encoder.encode_frame(&frame) {
                            Ok(packets) if !packets.is_empty() => {
                                if let Some(recorder) = &encode_recorder {
                                    recorder.record_encode(encode_started.elapsed());
                                    let name = encoder.backend_name();
                                    if noted_encoder != Some(name) {
                                        recorder.note_encoder(name);
                                        noted_encoder = Some(name);
                                    }
                                }
                                packets_encoded += packets.len() as u64;
                                encode_errs = 0;
                                frames_since_packet = 0;
                                last_packet_at = Instant::now();
                                let mut receiver_gone = false;
                                for packet in packets {
                                    if packet.is_keyframe {
                                        debug!("Keyframe encoded (seq={}, size={} bytes)", packet.sequence, packet.data.len());
                                    }
                                    trace_packet_stats(&packet);

                                    // Send packet to transport
                                    if packet_tx.send(packet).await.is_err() {
                                        receiver_gone = true;
                                        break;
                                    }
                                }
                                if receiver_gone {
                                    warn!("Packet receiver dropped, stopping encoder");
                                    break;
                                }
                            }
                            Ok(_) => {
                                // Encoder has no output yet (latency/drain phase)
                                frames_dropped += 1;
                                frames_since_packet += 1;
                                if frames_dropped.is_multiple_of(30) {
                                    debug!("Encoder latency: {} frames waiting for output", frames_dropped);
                                }
                                if frames_since_packet >= STALL_FRAMES
                                    && last_packet_at.elapsed() >= STALL_WINDOW
                                {
                                    fallback_rung!("no output for 2s while consuming frames");
                                }
                            }
                            Err(e) => {
                                encode_errs += 1;
                                error!(error = %e, errs = encode_errs, "Encoding failure");
                                if encode_errs >= ERR_END_AT {
                                    error!("Encoder unrecoverable after {ERR_END_AT} failures; ending session");
                                    encode_cancel.cancel();
                                    break;
                                }
                                if encode_errs >= ERR_REBUILD_AT && !used_fallback_rung {
                                    fallback_rung!("repeated encode failures");
                                }
                            }
                        }
                    }
                }
            }

            // Drain remaining packets
            match encoder.drain() {
                Ok(packets) => {
                    let count = packets.len();
                    for packet in packets {
                        if packet_tx.send(packet).await.is_err() {
                            break;
                        }
                    }
                    info!(count, "Drained remaining packets from encoder");
                }
                Err(e) => {
                    warn!(error = %e, "Error draining encoder");
                }
            }

            info!(
                packets_encoded,
                frames_dropped,
                "Encoding task complete"
            );
        }.instrument(encode_span));

        // Task 3: Packet transport — sends packets over QUIC (newest-only)
        let telemetry_bytes_tx = telemetry.as_ref().map(|t| t.1.clone());
        let transport_cancel = cancel.clone();
        let transport_span = tracing::info_span!("video_transport");
        tasks.spawn(async move {
            info!("Transport task started");
            let mut packets_sent = 0u64;
            let mut bytes_sent = 0u64;
            let mut packets_dropped = 0u64;
            let mut connection_closed = false;

            loop {
                tokio::select! {
                    biased;

                    _ = transport_cancel.cancelled() => {
                        info!("Transport task cancelled");
                        break;
                    }

                    // Send the newest frames. When the encoder/queue runs
                    // ahead of what we can push out, drop anything older than
                    // the most recent keyframe so the client renders live
                    // frames instead of a stale backlog.
                    Some(first) = packet_rx.recv() => {
                        let mut batch = vec![first];
                        while let Ok(next) = packet_rx.try_recv() {
                            batch.push(next);
                        }

                        // Resume point: the newest keyframe in the backlog, so
                        // the frames we send form a self-contained decodable
                        // group. If there is no keyframe we can only keep the
                        // whole delta chain (correct) or, if we're badly backed
                        // up, drop it and ask the encoder for an IDR.
                        let start = batch
                            .iter()
                            .rposition(|p| p.is_keyframe)
                            .unwrap_or(0);

                        if start > 0 {
                            // We're trimming to a keyframe boundary — fine.
                            packets_dropped += start as u64;
                            debug!(
                                dropped = start,
                                backlog = batch.len(),
                                "Transport trimmed stale backlog before keyframe"
                            );
                        } else if batch.len() > 1 {
                            // Backed up with no keyframe to cut at: dropping any
                            // delta would break the chain, so send them all.
                            // (Gap-driven IDR requests handle genuine loss.)
                            debug!(backlog = batch.len(), "Transport backlog has no keyframe; sending full delta chain");
                        }

                        for packet in &batch[start..] {
                            if connection_closed {
                                packets_dropped += 1;
                                continue;
                            }

                            // Open a unidirectional stream for this packet
                            match conn_for_transport.open_uni().await {
                                Ok(mut send_stream) => {
                                    // Build and send header
                                    let header = transport::PacketHeader {
                                        sequence: packet.sequence,
                                        stream_kind: transport::STREAM_KIND_VIDEO,
                                        is_keyframe: packet.is_keyframe,
                                        timestamp_us: packet.timestamp.elapsed().as_micros() as u64,
                                    };

                                    if let Err(e) = send_stream.write_all(&header.as_bytes()).await {
                                        error!(error = %e, seq = packet.sequence, "Failed to send packet header");
                                        connection_closed = true;
                                        packets_dropped += 1;
                                        continue;
                                    }

                                    let data_len = packet.data.len();
                                    if let Err(e) = send_stream.write_all(&packet.data).await {
                                        error!(error = %e, seq = packet.sequence, "Failed to send packet data");
                                        connection_closed = true;
                                        packets_dropped += 1;
                                        continue;
                                    }

                                    if let Err(e) = send_stream.finish() {
                                        error!(error = %e, seq = packet.sequence, "Failed to finish stream");
                                        connection_closed = true;
                                        packets_dropped += 1;
                                        continue;
                                    }

                                    packets_sent += 1;
                                    bytes_sent += data_len as u64;
                                    if let Some(counter) = &telemetry_bytes_tx {
                                        counter.fetch_add(data_len as u64, Ordering::Relaxed);
                                    }

                                    if packets_sent.is_multiple_of(60) {
                                        debug!(
                                            sent = packets_sent,
                                            bytes = bytes_sent,
                                            dropped = packets_dropped,
                                            mbps = %format!("{:.1}", (bytes_sent as f64 * 8.0) / 1_000_000.0),
                                            "Transport stats"
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!(error = %e, "Failed to open QUIC stream");
                                    connection_closed = true;
                                    packets_dropped += 1;
                                }
                            }
                        }
                    }

                    // Connection closed
                    else => {
                        info!("All packet senders finished, transport task ending");
                        break;
                    }
                }
            }

            info!(
                packets_sent,
                bytes_sent,
                packets_dropped,
                "Transport task complete"
            );
        }.instrument(transport_span));

        // Task 4: Monitor connection state and handle input streams from client
        let monitor_cancel = cancel.clone();
        let input_tx = self.input_tx.clone();
        let keyframe_req_tx = keyframe_tx.clone();
        let monitor_crop_tx = crop_tx;
        let monitor_window_tx = window_tx;
        let monitor_view_only = self.view_only.clone();
        let monitor_full_quality = self.full_quality.clone();
        let monitor_preset = self.preset.clone();
        let monitor_mic_tx = self.mic_tx.clone();
        let pointer_space = PointerSpace::for_monitor(self.config.monitor_index);
        if let Some(space) = pointer_space {
            info!(
                ?space,
                "Absolute input is remapped from the monitor onto the desktop layout"
            );
        }
        // The client's own measurements land in the session record, so the
        // monitor needs the recorder the encode task shares.
        let monitor_recorder = telemetry.as_ref().map(|t| t.0.clone());
        let monitor_span = tracing::info_span!("connection_monitor");
        tasks.spawn(async move {
            info!("Connection monitor started");
            // A cropped session's taps arrive layout-normalized already.
            let mut cropped = false;

            loop {
                tokio::select! {
                    _ = monitor_cancel.cancelled() => {
                        info!("Connection monitor cancelled");
                        break;
                    }

                    // Accept unidirectional streams from client (input events, acks)
                    result = conn_for_monitor.accept_uni() => {
                        match result {
                            Ok(mut stream) => {
                                debug!("Accepted client stream");
                                // Read the entire payload (max 64KB per input packet)
                                let data = stream.read_to_end(64 * 1024).await;
                                match data {
                                    Ok(data) if !data.is_empty() => {
                                        debug!(size = data.len(), "Received client data");

                                        // Parse as binary InputPacket
                                        match InputPacket::decode(&data) {
                                            Ok(packet) => {
                                                // The client's measurements are
                                                // control-plane: they never reach
                                                // the injector and are not blocked
                                                // by view-only, so a watched-only
                                                // session still reports.
                                                if record_client_measurement(
                                                    monitor_recorder.as_deref(),
                                                    &packet,
                                                ) {
                                                    continue;
                                                }
                                                // IDR requests are control-plane:
                                                // signal the encoder, don't inject input.
                                                if matches!(packet, InputPacket::RequestKeyframe) {
                                                    debug!("Client requested IDR (sequence gap)");
                                                    keyframe_req_tx
                                                        .send_modify(|n| *n = n.wrapping_add(1));
                                                    continue;
                                                }
                                                // Window crop is control-plane too:
                                                // drive the capture-region/encoder
                                                // resolution, don't inject input.
                                                if matches!(packet, InputPacket::WindowCrop { .. }) {
                                                    let rect = packet.crop_rect();
                                                    let addr = packet.window_handle().unwrap_or(0);
                                                    info!(?rect, addr, "Client set window crop");
                                                    cropped = rect.is_some();
                                                    let _ = monitor_crop_tx.send(rect);
                                                    let _ = monitor_window_tx.send(addr);
                                                    continue;
                                                }
                                                // View-only is server-enforced input
                                                // lockdown (R4 D1): latch the flag;
                                                // nothing is injected either way.
                                                if let InputPacket::ViewOnly { enabled } = packet {
                                                    monitor_view_only
                                                        .store(enabled, Ordering::Relaxed);
                                                    info!(enabled, "View-only mode changed");
                                                    continue;
                                                }
                                                // Quality override is control-plane too
                                                // (R4 A3): steer the relay guard, never
                                                // injected, and survives view-only.
                                                if let InputPacket::FullQuality { enabled } = packet
                                                {
                                                    monitor_full_quality
                                                        .store(enabled, Ordering::Relaxed);
                                                    info!(enabled, "Relay quality override changed");
                                                    continue;
                                                }
                                                // Link-profile preset (R4 E5) is
                                                // control-plane too: latch the id
                                                // for the bitrate arbiter, never
                                                // injected, survives view-only.
                                                if let InputPacket::QualityPreset { preset } =
                                                    packet
                                                {
                                                    monitor_preset.store(preset, Ordering::Relaxed);
                                                    info!(preset, "Link-profile preset changed");
                                                    continue;
                                                }
                                                // Mic audio (R4 E2) is session
                                                // media, not injected input — it
                                                // flows to the relay even while
                                                // view-only is latched.
                                                if matches!(packet, InputPacket::Mic { .. }) {
                                                    // F5: no mic permission, no relay.
                                                    if allow_mic
                                                        && let Some(tx) = &monitor_mic_tx
                                                        && tx.try_send(packet).is_err()
                                                    {
                                                        debug!("Mic relay full: dropping frame");
                                                    }
                                                    continue;
                                                }
                                                if force_view_only
                                                    || monitor_view_only.load(Ordering::Relaxed)
                                                {
                                                    debug!("View-only: dropping input packet");
                                                    continue;
                                                }
                                                let packet = match (packet, pointer_space) {
                                                    (
                                                        InputPacket::MouseMoveAbs { x_norm, y_norm },
                                                        Some(space),
                                                    ) if !cropped => {
                                                        let (x_norm, y_norm) = space.remap(x_norm, y_norm);
                                                        InputPacket::MouseMoveAbs { x_norm, y_norm }
                                                    }
                                                    (packet, _) => packet,
                                                };
                                                // Forward to input injector via channel
                                                if let Some(ref tx) = input_tx {
                                                   // Use send() without await (broadcast is sync).
                                                   // If there are no receivers, it returns an error which we can ignore.
                                                   let _ = tx.send(packet);
                                                }
                                            }
                                            Err(e) => {
                                                warn!(error = %e, "Failed to parse input packet");
                                            }
                                        }
                                    }
                                    Ok(_) => {
                                        debug!("Client stream closed gracefully");
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "Error reading client stream");
                                    }
                                }
                            }
                            Err(super::connection::ConnectionError::Closed) => {
                                info!("Client disconnected via application close");
                                break;
                            }
                            Err(e) => {
                                warn!(error = %e, "Connection error in monitor");
                                break;
                            }
                        }
                    }
                }
            }
        }.instrument(monitor_span));

        // Task 6: Audio capture + Opus encoding + QUIC send (F1: Audio Streaming).
        // F5: a device without the audio permission gets no desktop sound.
        if allow_audio {
            let audio_cancel = cancel.clone();
            let audio_conn = connection.clone();
            tasks.spawn(
                async move {
                    if let Err(e) = run_audio_pipeline(audio_conn, audio_cancel).await {
                        error!(error = %e, "Audio pipeline failed");
                    }
                }
                .instrument(tracing::info_span!("audio_pipeline")),
            );
        }

        // Task 7: session telemetry poll (R4 A2) — samples path + RTT every
        // 5 s so the end-of-session report reflects what the link actually
        // did (punching can complete mid-session), not how it started.
        if let Some(shared) = telemetry.as_ref() {
            let recorder = shared.0.clone();
            let poll_cancel = cancel.clone();
            let poll_conn = connection.clone();
            tasks.spawn(async move {
                loop {
                    tokio::select! {
                        _ = poll_cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                    recorder.sample(&poll_conn);
                }
            });
        }

        // Task 7b: live bitrate arbiter (R4 A3 + E5, roadmap 2053). One task
        // owns every link-driven bitrate change so they never fight: it folds
        // the A3 relay floor, the E5 HUD preset ceiling and the loss response
        // together against the session's configured rate and pushes the result
        // to the encoder whenever the effective value changes.
        //
        //   effective = configured
        //                 .min(relay_cap)        // 2 Mbit/s while relayed
        //                 .min(preset_ceil)      // HUD profile, relative to native
        //                 .min(loss_cap)         // 20 % off per congested tick
        //
        // Unlike the other two, `loss_cap` has memory: it is a controller
        // reading the transport's counters once per tick, and it walks back up
        // on its own when the link goes clean.
        //
        // `FullQuality` clears the relay floor (explicit override). The
        // relay term is inert on quinn (LAN / Tailscale never report a
        // relayed path) and on iroh before punching, so this task runs for
        // every transport — that is what lets a LAN session pick a preset.
        {
            let arb_conn = connection.clone();
            let arb_bitrate_tx = self.bitrate_tx.clone();
            let configured_bitrate = self.config.bitrate_bps;
            let arb_cancel = cancel.clone();
            let arb_full_quality = self.full_quality.clone();
            let arb_preset = self.preset.clone();
            tasks.spawn(async move {
                // Seed with the configured rate; the first tick reconciles.
                let mut applied = configured_bitrate;
                let mut loss = LossCeiling::new(configured_bitrate);
                loop {
                    tokio::select! {
                        _ = arb_cancel.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                    let stats = arb_conn.stats();
                    loss.sample(stats.lost_packets, stats.datagrams_sent);
                    let loss_cap = loss.ceiling();
                    let relayed = stats.relayed && !arb_full_quality.load(Ordering::Relaxed);
                    let relay_cap = if relayed {
                        RELAY_BITRATE_CAP_BPS
                    } else {
                        u32::MAX
                    };
                    let preset_ceil =
                        preset_bitrate_ceil(arb_preset.load(Ordering::Relaxed), configured_bitrate);
                    let effective = configured_bitrate
                        .min(relay_cap)
                        .min(preset_ceil)
                        .min(loss_cap);
                    if effective == applied {
                        continue;
                    }
                    info!(
                        effective_bps = effective,
                        configured_bps = configured_bitrate,
                        relayed,
                        preset = arb_preset.load(Ordering::Relaxed),
                        loss_cap = (loss_cap != u32::MAX).then_some(loss_cap),
                        "Encoder bitrate target changed"
                    );
                    let _ = arb_bitrate_tx.send(effective);
                    applied = effective;
                }
            });
        }

        // Wait for all tasks to complete or connection to close
        let result = tasks.join_next().await;
        if let Some(guard) = &_telemetry_guard {
            guard.mark_completed();
        }

        // 4. Shutdown & Cleanup (Mandate 7.1)
        info!("Initiating pipeline shutdown...");
        cancel.cancel();

        // Drop capture session explicitly to trigger immediate PipeWire resource cleanup
        drop(capture_session);

        // Explicitly drain remaining tasks to prevent zombies
        let mut cleanup_count = 0;
        while let Some(res) = tasks.join_next().await {
            cleanup_count += 1;
            if let Err(e) = res {
                warn!(error = %e, "Cleanup: task panicked during shutdown");
            }
        }

        info!(
            tasks_drained = cleanup_count,
            "Streaming pipeline shut down complete"
        );

        match result {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(anyhow::anyhow!("Task failed: {}", e)),
            None => Ok(()),
        }
    }

    /// Signal the server to stop accepting new connections and shut down
    pub fn stop(&self) {
        info!("Stopping streaming server...");
        self.cancel.cancel();
    }

    /// Check if the server is still running
    pub fn is_running(&self) -> bool {
        !self.cancel.is_cancelled()
    }
}

/// Fold the client's own measurements into the session record, and report
/// whether `packet` was one of them. These frames are reporting rather than
/// injection, so the monitor consumes them before the input relay and before
/// the view-only filter: a watched-only session still has to produce a record.
fn record_client_measurement(recorder: Option<&SessionRecorder>, packet: &InputPacket) -> bool {
    match packet {
        InputPacket::LinkFeedback { rtt, lost_packets } => {
            match recorder {
                Some(recorder) => recorder.record_client_link(*rtt, *lost_packets),
                None => debug!("Telemetry off: dropping client link feedback"),
            }
            true
        }
        InputPacket::ClientSamples { kind, values } => {
            match recorder {
                Some(recorder) if recorder.record_client_samples(*kind, values) => {}
                Some(_) => debug!(kind = *kind, "Unknown client sample kind"),
                None => debug!(kind = *kind, "Telemetry off: dropping client samples"),
            }
            true
        }
        _ => false,
    }
}

/// Log packet statistics for monitoring
fn trace_packet_stats(packet: &EncodedPacket) {
    debug!(
        "Packet: seq={}, keyframe={}, size={} bytes, age={}ms",
        packet.sequence,
        packet.is_keyframe,
        packet.data.len(),
        packet.timestamp.elapsed().as_millis()
    );
}

/// Read the client's pre-pipeline config streams from the QUIC connection.
///
/// The client may send up to four small uni-streams immediately after
/// connecting: a monitor-index config (`[0xFF, 0x00] + u32 LE`), a device
/// identity (`[0xFE, 0x00] + len u8 + utf8`, see `client::DEVICE_ID_MARKER`),
/// its WAN key (`[0xFC, 0x00] + len u8 + utf8`, device keys) and codec
/// capabilities (`[0xFD, 0x00] + u8`, R4 C1). Each read is bounded
/// by a short timeout so the pipeline is not blocked when a stream (or the
/// whole handshake, for older clients) is absent.
/// Returns what the client announced about itself.
async fn read_client_config(
    connection: &SharedConnection,
    config: &mut StreamingConfig,
    hevc_allowed: bool,
) -> ClientClaim {
    use super::client::{
        CODEC_CAPS_MARKER, DEVICE_ID_MARKER, IROH_KEY_MARKER, MONITOR_CONFIG_MARKER,
    };

    let mut claim = ClientClaim::default();
    for _ in 0..4 {
        let stream = tokio::select! {
            biased;
            result = connection.accept_uni() => result,
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                debug!("No further client config streams within 200ms");
                break;
            }
        };
        let Ok(mut stream) = stream else { break };

        let mut marker = [0u8; 2];
        if !read_with_timeout(&mut *stream, &mut marker).await {
            break;
        }
        if marker == MONITOR_CONFIG_MARKER {
            let mut buf = [0u8; 4];
            if !read_with_timeout(&mut *stream, &mut buf).await {
                break;
            }
            let monitor_index = u32::from_le_bytes(buf);
            config.monitor_index = monitor_index;
            info!("Client config: monitor_index={}", monitor_index);
        } else if marker == DEVICE_ID_MARKER {
            let mut len = [0u8; 1];
            if !read_with_timeout(&mut *stream, &mut len).await {
                break;
            }
            let mut id = vec![0u8; len[0] as usize];
            if !read_with_timeout(&mut *stream, &mut id).await {
                break;
            }
            match String::from_utf8(id) {
                Ok(id) => {
                    info!("Client identity: deviceId={}", id);
                    claim.device_id = Some(id);
                }
                Err(e) => debug!("Malformed device identity stream: {e}"),
            }
        } else if marker == IROH_KEY_MARKER {
            let mut len = [0u8; 1];
            if !read_with_timeout(&mut *stream, &mut len).await {
                break;
            }
            let mut id = vec![0u8; len[0] as usize];
            if !read_with_timeout(&mut *stream, &mut id).await {
                break;
            }
            match String::from_utf8(id) {
                Ok(id) => claim.iroh_id = Some(id),
                Err(e) => debug!("Malformed WAN key stream: {e}"),
            }
        } else if marker == CODEC_CAPS_MARKER {
            let mut caps = [0u8; 1];
            if !read_with_timeout(&mut *stream, &mut caps).await {
                break;
            }
            if let Some(codec) = negotiate_codec(caps[0], hevc_allowed, config.codec) {
                config.codec = codec;
            }
            info!(
                caps = caps[0],
                codec = config.codec.display_name(),
                "Client codec caps negotiated"
            );
        } else {
            debug!("Unknown config marker: {:02X?}", &marker);
        }
    }
    claim
}

/// R4 C1: pick the session codec from the client's capability bits and the
/// server's policy. Pure so the negotiation is unit-testable without a
/// pipeline. Returns `None` to keep the current config codec (H.264 default).
fn negotiate_codec(caps: u8, hevc_allowed: bool, current: VideoCodec) -> Option<VideoCodec> {
    use super::client::CODEC_CAP_HEVC;
    if hevc_allowed && caps & CODEC_CAP_HEVC != 0 && current == VideoCodec::H264 {
        Some(VideoCodec::H265)
    } else {
        None
    }
}

/// `read_exact` bounded by a short timeout; false on timeout or error.
async fn read_with_timeout(stream: &mut dyn super::connection::InStream, buf: &mut [u8]) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(500), stream.read_exact(buf)).await,
        Ok(Ok(()))
    )
}

/// Audio pipeline: PipeWire loopback capture → Opus encode → QUIC send.
///
/// No capture means no audio for this session, and the task then ends rather
/// than filling the wire: the old fallback synthesized silence at 50 packets/s,
/// which carries nothing, costs the client a demux slot per packet, and is what
/// turned a phone with no audio playout into a stalled picture (see
/// `client::try_deliver_audio`). Runs until cancelled or the connection closes.
async fn run_audio_pipeline(connection: SharedConnection, cancel: CancellationToken) -> Result<()> {
    info!("Audio task started");

    let audio_config = AudioConfig {
        sample_rate: 48000,
        channels: 2,
        bitrate_bps: 64_000,
        frame_duration_ms: 20,
    };

    let mut encoder =
        AudioOpusEncoder::new(audio_config).context("Failed to create Opus encoder")?;

    // Try PipeWire audio loopback capture.
    // The session MUST stay alive for the whole audio task: its Drop
    // cancels the (shared) pipeline token, so dropping it here would tear
    // down capture+encode+transport the instant the session started.
    let (pcm_tx, mut pcm_rx) = mpsc::channel::<audio_capture::PcmBuffer>(8);
    let pw_cancel = cancel.clone();
    let _pw_session =
        match audio_capture::start_audio_capture(48000, 2, 20, pcm_tx, pw_cancel).await {
            Ok(session) => {
                info!("PipeWire audio loopback active");
                Some(session)
            }
            Err(e) => {
                info!(error = %e, "PipeWire audio capture unavailable, sending no audio");
                None
            }
        };
    let Some(_pw_session) = _pw_session else {
        return Ok(());
    };

    let mut packet_seq = 0u64;
    let mut packets_sent = 0u64;
    let mut connection_closed = false;

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!(packets = packets_sent, "Audio task cancelled");
                break;
            }
            pcm = pcm_rx.recv() => {
                let Some(pcm) = pcm else {
                    info!("Audio capture source closed");
                    break;
                };
                if connection_closed { break; }

                let packet = match encoder.encode(&pcm.data) {
                    Ok(Some(p)) => p,
                    Ok(None) => continue,
                    Err(e) => { debug!(error = %e, "Opus encode skip"); continue; }
                };

                if let Err(e) = send_audio_packet(
                    &*connection, packet, packet_seq, &mut connection_closed,
                ).await {
                    warn!(error = %e, "Audio transport failed");
                    connection_closed = true;
                    continue;
                }

                packets_sent += 1;
                packet_seq += 1;

                if packets_sent.is_multiple_of(600) {
                    debug!(sent = packets_sent, "Audio streaming healthy");
                }
            }
        }
    }

    info!(packets_sent, "Audio task complete");
    Ok(())
}

/// Helper: send an encoded audio packet over the connection.
async fn send_audio_packet(
    conn: &dyn Connection,
    packet: super::AudioPacket,
    sequence: u64,
    connection_closed: &mut bool,
) -> Result<()> {
    match conn.open_uni().await {
        Ok(mut send_stream) => {
            let header = transport::PacketHeader {
                sequence,
                stream_kind: transport::STREAM_KIND_AUDIO,
                is_keyframe: false,
                timestamp_us: packet.timestamp.elapsed().as_micros() as u64,
            };
            send_stream.write_all(&header.as_bytes()).await?;
            send_stream.write_all(&packet.data).await?;
            send_stream.finish()?;
            Ok(())
        }
        Err(e) => {
            *connection_closed = true;
            Err(anyhow::anyhow!("Failed to open audio stream: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::{EncoderPreset, H264Profile, HardwareEncoder, VideoCodec};

    /// This PC's layout: a 2560x1080 primary at the origin and a 2048x1152
    /// screen to its right, 91 px higher.
    const TWO_SCREENS: PointerSpace = PointerSpace {
        monitor: (0, 0, 2560, 1080),
        layout: (0, -91, 4608, 1171),
    };

    fn to_pixel(norm: (u16, u16), layout: (i32, i32, u32, u32)) -> (i32, i32) {
        let (lx, ly, lw, lh) = layout;
        let px = |n: u16, start: i32, len: u32| {
            start + (f64::from(n) / 65535.0 * f64::from(len)).round() as i32
        };
        (px(norm.0, lx, lw), px(norm.1, ly, lh))
    }

    #[test]
    fn a_frame_point_lands_on_the_same_pixel_of_the_captured_monitor() {
        // The tap that missed on the device: the middle of the stream went to
        // (2560, 281) instead of the monitor's own middle.
        let centre = TWO_SCREENS.remap(32768, 32768);
        let (x, y) = to_pixel(centre, TWO_SCREENS.layout);
        assert!((x - 1280).abs() <= 1 && (y - 540).abs() <= 1, "{x},{y}");
        // The frame's corners are the monitor's corners, not the layout's.
        assert_eq!(
            to_pixel(TWO_SCREENS.remap(0, 0), TWO_SCREENS.layout),
            (0, 0)
        );
        let (x, y) = to_pixel(TWO_SCREENS.remap(65535, 65535), TWO_SCREENS.layout);
        assert!((x - 2560).abs() <= 1 && (y - 1080).abs() <= 1, "{x},{y}");
    }

    #[test]
    fn a_monitor_right_of_the_primary_maps_inside_its_own_rect() {
        let right = PointerSpace {
            monitor: (2560, -91, 2048, 1152),
            ..TWO_SCREENS
        };
        let (x, y) = to_pixel(right.remap(0, 0), right.layout);
        assert_eq!((x, y), (2560, -91));
        let (x, y) = to_pixel(right.remap(65535, 65535), right.layout);
        assert!((x - 4608).abs() <= 1 && (y - 1061).abs() <= 1, "{x},{y}");
    }

    #[test]
    fn test_streaming_server_creation() {
        let config = StreamingConfig::default();
        let transport_config = StreamTransportConfig::default();
        let cert_manager =
            std::sync::Arc::new(CertManager::new().expect("Failed to create CertManager"));
        let server = StreamingServer::new(config, transport_config, cert_manager);

        assert!(server.is_running());
    }

    /// The monitor used to recognise the client's stats frame by its length,
    /// which silently swallowed every 16-byte control frame. Dispatch is by tag
    /// again, and what it dispatches must land in the record rather than the
    /// input channel.
    #[test]
    fn client_measurements_are_recorded_and_never_treated_as_input() {
        use crate::streaming::TransportFamily;
        use crate::streaming::input_packet::{SAMPLE_DECODE, SAMPLE_E2E};
        use std::time::Duration;

        let recorder = SessionRecorder::new(TransportFamily::Quinn, None);
        assert!(record_client_measurement(
            Some(&recorder),
            &InputPacket::LinkFeedback {
                rtt: Duration::from_millis(28),
                lost_packets: 4,
            }
        ));
        assert!(record_client_measurement(
            Some(&recorder),
            &InputPacket::ClientSamples {
                kind: SAMPLE_DECODE,
                values: vec![3_000, 9_000],
            }
        ));
        assert!(record_client_measurement(
            Some(&recorder),
            &InputPacket::ClientSamples {
                kind: SAMPLE_E2E,
                values: vec![40_000],
            }
        ));
        assert!(
            !record_client_measurement(
                Some(&recorder),
                &InputPacket::MouseClick {
                    button: 0,
                    pressed: true,
                }
            ),
            "an input event is not a measurement; the relay must keep filtering it"
        );

        let report = recorder.finish(SessionOutcome::Completed, 0);
        assert_eq!(report.decode_tail.map(|s| s.count), Some(2));
        assert_eq!(report.render_tail.map(|s| s.count), None);
        assert_eq!(report.e2e_tail.map(|s| s.count), Some(1));
        // A recorder that was never handed a connection says so, rather than
        // reporting a link built out of the client's numbers alone.
        assert!(report.link.is_none());
    }

    #[test]
    fn test_bitrate_update() {
        let config = StreamingConfig::default();
        let transport_config = StreamTransportConfig::default();
        let cert_manager =
            std::sync::Arc::new(CertManager::new().expect("Failed to create CertManager"));
        let server = StreamingServer::new(config.clone(), transport_config, cert_manager);

        let new_bitrate = 4_000_000;
        server.update_bitrate(new_bitrate);

        let rx = server.bitrate_watcher();
        assert_eq!(*rx.borrow(), new_bitrate);
    }

    #[test]
    fn test_streaming_config_variants() {
        // Low quality config
        let low = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 2_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };
        assert_eq!(low.bitrate_bps, 2_000_000);

        // High quality config
        let high = StreamingConfig {
            width: 3840,
            height: 2160,
            fps: 60,
            bitrate_bps: 20_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::High,
            preset: EncoderPreset::Medium,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };
        assert_eq!(high.bitrate_bps, 20_000_000);
    }

    #[test]
    fn test_packet_header_export() {
        // Verify PacketHeader is accessible from transport module
        use super::transport::{PacketHeader, STREAM_KIND_VIDEO};

        let header = PacketHeader {
            sequence: 42,
            stream_kind: STREAM_KIND_VIDEO,
            is_keyframe: true,
            timestamp_us: 1_000_000,
        };

        let bytes = header.as_bytes();
        assert_eq!(bytes.len(), 18);
    }

    /// R4 C1 wire round-trip: a real `StreamingClient::connect` with a
    /// monitor index, device id and HEVC capability must land in
    /// `read_client_config` as a negotiated H.265 session.
    #[tokio::test]
    async fn client_config_streams_negotiate_codec_over_loopback() {
        // reqwest (aws-lc-rs) and quinn (ring) link two rustls providers;
        // pick ring explicitly, same as the connection trait test does.
        let _ = rustls::crypto::ring::default_provider().install_default();
        use super::super::client::{CODEC_CAP_HEVC, StreamingClient};

        let certs = std::sync::Arc::new(CertManager::new().expect("certs"));
        let alpn = vec![StreamTransportConfig::default().alpn];
        let server_endpoint = quinn::Endpoint::server(
            certs.server_config(alpn.clone()).expect("server config"),
            "127.0.0.1:0".parse().unwrap(),
        )
        .expect("bind");
        let addr = server_endpoint.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let incoming = server_endpoint.accept().await.expect("incoming");
            QuinnConnection::shared(incoming.await.expect("accept"))
        });

        let (_client, _frames, _audio) = StreamingClient::connect(
            &addr.to_string(),
            certs,
            Some(2),
            Some("device-c1"),
            Some("phone-endpoint"),
            Some(CODEC_CAP_HEVC),
        )
        .await
        .expect("connect");

        let server_conn = accept.await.expect("accept task");
        let mut config = StreamingConfig::default();
        let claim = read_client_config(&server_conn, &mut config, true).await;

        assert_eq!(claim.device_id.as_deref(), Some("device-c1"));
        assert_eq!(
            claim.iroh_id.as_deref(),
            Some("phone-endpoint"),
            "device keys: the WAN key it announces"
        );
        assert_eq!(config.monitor_index, 2);
        assert_eq!(config.codec, VideoCodec::H265);
    }

    #[test]
    fn codec_negotiation_matrix() {
        use super::super::client::CODEC_CAP_HEVC;
        // Absent/zero caps or a server that did not allow HEVC stay H.264.
        assert_eq!(negotiate_codec(0, true, VideoCodec::H264), None);
        assert_eq!(
            negotiate_codec(CODEC_CAP_HEVC, false, VideoCodec::H264),
            None
        );
        // Both ends agree → HEVC.
        assert_eq!(
            negotiate_codec(CODEC_CAP_HEVC, true, VideoCodec::H264),
            Some(VideoCodec::H265)
        );
        // A server already configured for H.265 is untouched; unknown future
        // capability bits alone must not change anything.
        assert_eq!(negotiate_codec(0b10, true, VideoCodec::H265), None);
        assert_eq!(negotiate_codec(0b10, true, VideoCodec::H264), None);
    }
}
