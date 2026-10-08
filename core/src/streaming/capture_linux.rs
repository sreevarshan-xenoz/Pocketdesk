//! Linux capture backends: XDG Desktop Portal + PipeWire, and direct X11
//! `GetImage`. Moved verbatim out of `capture.rs`, which keeps the
//! OS-independent backend selection; the wlroots screencopy backend lives in
//! `capture_screencopy.rs`.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ashpd::desktop::screencast::{Screencast, SelectSourcesOptions, SourceType};
use pipewire::stream::StreamFlags;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::capture::{CaptureSession, monitor_rect};
use super::{StreamingConfig, VideoFrame};

/// SPA_PARAM_Format property ID (from SPA param type enum).
const SPA_PARAM_FORMAT: u32 = 0;

/// Shared state between the PipeWire thread and stream callbacks.
struct StreamUserData {
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
    /// Actual negotiated width (updated by param_changed callback).
    width: AtomicU32,
    /// Actual negotiated height (updated by param_changed callback).
    height: AtomicU32,
    /// Frame counter for logging.
    frame_count: AtomicU64,
}

/// Choose the PipeWire stream for the requested monitor from
/// `(node_id, position)` pairs: exact position match against the target
/// rect's origin, else the first stream (single-share dialogs and backends
/// that don't report positions).
fn pick_stream_node(
    streams: &[(u32, Option<(i32, i32)>)],
    want: Option<(i32, i32)>,
) -> Option<u32> {
    if let Some(pos) = want
        && let Some((node_id, _)) = streams.iter().find(|(_, p)| *p == Some(pos))
    {
        return Some(*node_id);
    }
    streams.first().map(|(node_id, _)| *node_id)
}

/// Start PipeWire screen capture.
///
/// This function:
/// 1. Creates an XDG Portal screencast session
/// 2. Extracts the PipeWire node ID from the portal response
/// 3. Spawns a background PipeWire client thread that receives frames
/// 4. Sends frames through the provided `mpsc::Sender<VideoFrame>` channel
/// 5. Returns a [`CaptureSession`] that manages the capture lifetime
///
/// The capture runs continuously until `cancel` is triggered or the portal
/// session ends.
pub async fn start_capture(
    config: StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<CaptureSession> {
    info!(
        "Starting screen capture: {}x{}@{}fps",
        config.width, config.height, config.fps
    );

    // Step 1: Create XDG Portal screencast session (with timeout to handle missing portal backends)
    let screencast = tokio::time::timeout(Duration::from_secs(10), Screencast::new())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Screencast portal unavailable (timeout after 10s) — is xdg-desktop-portal running?"
            )
        })?
        .context("Failed to create Screencast portal")?;

    debug!("Creating screencast session...");
    let session = tokio::time::timeout(
        Duration::from_secs(10),
        screencast.create_session(Default::default()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Portal request timed out — is xdg-desktop-portal running?"))?
    .context("Failed to create capture session")?;

    // Step 2: Select sources (monitor capture)
    // Determine which monitor to capture (F2: multi-monitor).
    // ashpd screencast captures all monitors; we'll select based on config.monitor_index.
    // For now, Portal captures primary monitor by default.
    let source_types = ashpd::enumflags2::BitFlags::from(SourceType::Monitor);
    tokio::time::timeout(
        Duration::from_secs(10),
        screencast.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_sources(Some(source_types))
                .set_multiple(Some(true)),
        ),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Portal request timed out — is xdg-desktop-portal running?"))?
    .context("Failed to select sources")?;

    // Step 3: Start capture and get PipeWire node ID
    debug!("Starting capture and waiting for PipeWire stream...");
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        screencast.start(&session, None, Default::default()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Portal request timed out — is xdg-desktop-portal running?"))?
    .context("Failed to start capture")?;

    let streams = response
        .response()
        .context("No response from capture start")?;

    // Step 4: Pick the stream for the requested monitor. The portal grants one
    // stream per monitor the desktop user shared; the position match against
    // the xcap enumeration is what makes `monitor_index` mean the same thing
    // on both sides of the wire.
    let stream_infos: Vec<_> = streams
        .streams()
        .iter()
        .map(|s| {
            let size = s.size().unwrap_or((0, 0));
            info!(
                "Available stream: node_id={}, size={}x{}, position={:?}",
                s.pipe_wire_node_id(),
                size.0,
                size.1,
                s.position()
            );
            (s.pipe_wire_node_id(), s.position())
        })
        .collect();
    let want = monitor_rect(config.monitor_index).map(|(x, y, _, _)| (x, y));
    if want.is_some()
        && !stream_infos
            .iter()
            .any(|(_, p)| want.is_some_and(|w| *p == Some(w)))
    {
        warn!("Monitor {want:?} not among the shared streams, falling back to the first one");
    }
    let node_id = pick_stream_node(&stream_infos, want)
        .context("No PipeWire stream available from screencast session")?;
    info!(
        "Selected node_id={} for monitor_index={}",
        node_id, config.monitor_index
    );

    // Clone config for the capture thread
    let thread_config = config.clone();

    // Step 5: Spawn the PipeWire capture thread
    let pw_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("pipewire-capture".into())
        .spawn(move || {
            if let Err(e) = run_pipewire_capture(node_id, &thread_config, frame_tx, pw_cancel) {
                error!("PipeWire capture thread exited with error: {e}");
            } else {
                info!("PipeWire capture thread exited normally");
            }
        })
        .context("Failed to spawn PipeWire capture thread")?;

    Ok(CaptureSession::new(config, cancel))
}

/// Run the PipeWire capture loop in a dedicated thread.
fn run_pipewire_capture(
    node_id: u32,
    config: &StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<()> {
    info!("PipeWire capture thread started, connecting to node {node_id}");

    // Initialize PipeWire
    pipewire::init();

    // Create the main loop (event loop)
    let mainloop = pipewire::main_loop::MainLoopBox::new(None)
        .context("Failed to create PipeWire main loop")?;

    // Create the context
    let context = pipewire::context::ContextBox::new(mainloop.loop_(), None)
        .context("Failed to create PipeWire context")?;

    // Connect to the PipeWire daemon
    let core = context
        .connect(None)
        .context("Failed to connect to PipeWire daemon")?;

    // Create stream properties for video capture.
    // The stream properties guide the format negotiation.
    // We don't pass explicit format params to connect(); instead,
    // we learn the negotiated format from the param_changed callback.
    let stream_props = pipewire::properties::properties! {
        *pipewire::keys::MEDIA_TYPE => "Video",
        *pipewire::keys::MEDIA_CATEGORY => "Capture",
        *pipewire::keys::MEDIA_ROLE => "Screen",
    };

    // Create the stream
    let stream = pipewire::stream::StreamBox::new(&core, "linux-link-capture", stream_props)
        .context("Failed to create PipeWire stream")?;

    // Set up shared state for callbacks
    let user_data = StreamUserData {
        frame_tx,
        cancel: cancel.clone(),
        width: AtomicU32::new(config.width),
        height: AtomicU32::new(config.height),
        frame_count: AtomicU64::new(0),
    };

    // Create local listener with param_changed and process callbacks
    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .param_changed(|_stream, ud, id, param| {
            on_param_changed(id, param, ud);
        })
        .process(|stream, ud| {
            on_process(stream, ud);
        })
        .register()
        .context("Failed to register stream listener")?;

    // Connect the stream to the specific PipeWire node.
    // We pass an empty params array -- the format will be negotiated
    // automatically based on stream properties and server capabilities.
    // The actual format is learned via the param_changed callback.
    let mut params: [&libspa::pod::Pod; 0] = [];
    stream
        .connect(
            libspa::utils::Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .with_context(|| format!("Failed to connect stream to node {node_id}"))?;

    // Activate the stream to start receiving frames
    stream
        .set_active(true)
        .context("Failed to activate stream")?;

    info!("PipeWire stream connected and active, entering main loop");

    // Run the main loop until cancellation
    loop {
        mainloop.loop_().iterate(Duration::from_millis(100));
        if cancel.is_cancelled() {
            info!("Cancellation received, exiting PipeWire main loop");
            break;
        }
    }

    info!("PipeWire capture thread shutting down");

    // Clean up
    let _ = stream.set_active(false);
    let _ = stream.disconnect();

    Ok(())
}

/// Handle the param_changed callback to learn the negotiated video format.
fn on_param_changed(id: u32, param: Option<&libspa::pod::Pod>, user_data: &StreamUserData) {
    if id != SPA_PARAM_FORMAT {
        return;
    }

    let Some(param) = param else {
        debug!("param_changed: format removed");
        return;
    };

    // Parse the negotiated format
    let mut video_info = libspa::param::video::VideoInfoRaw::new();
    if video_info.parse(param).is_err() {
        warn!("Failed to parse video format from param_changed");
        return;
    }

    let format = video_info.format();
    let size = video_info.size();
    let framerate = video_info.framerate();

    info!(
        "Negotiated video format: {:?}, {}x{}@{}/{}fps",
        format, size.width, size.height, framerate.num, framerate.denom
    );

    // Update the actual dimensions for frame capture
    user_data.width.store(size.width, Ordering::Relaxed);
    user_data.height.store(size.height, Ordering::Relaxed);
}

/// Handle the process callback -- dequeue frames and send them through the channel.
fn on_process(stream: &pipewire::stream::Stream, user_data: &StreamUserData) {
    // Dequeue an available buffer
    let mut buffer = match stream.dequeue_buffer() {
        Some(b) => b,
        None => {
            trace!("No buffer available in process callback");
            return;
        }
    };

    // Access the buffer data
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        warn!("Buffer has no data planes");
        return;
    }

    let data = &mut datas[0];

    // Get chunk info first (size and stride)
    let (size, stride) = {
        let chunk = data.chunk();
        (chunk.size() as usize, chunk.stride())
    };
    if size == 0 {
        return;
    }

    // Then get the data slice
    let Some(data_slice) = data.data() else {
        trace!("Buffer data pointer is null");
        return;
    };

    if data_slice.is_empty() {
        return;
    }

    // Get the actual negotiated dimensions
    let width = user_data.width.load(Ordering::Relaxed);
    let height = user_data.height.load(Ordering::Relaxed);

    // Copy the frame data (cap at actual data slice length)
    let frame_data = data_slice[..size.min(data_slice.len())].to_vec();

    // Update frame counter
    let count = user_data.frame_count.fetch_add(1, Ordering::Relaxed);

    if count.is_multiple_of(30) {
        debug!("Captured frame #{count}: size={size}B, {width}x{height}, stride={stride}");
    }

    let frame = VideoFrame {
        data: frame_data,
        width,
        height,
        stride: stride as u32,
        timestamp: Instant::now(),
    };
    // Send the frame through the channel.
    // Use try_send to implement Latest-Frame-Wins by dropping the newest frame if full.
    match user_data.frame_tx.try_send(frame) {
        Ok(_) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            // Ignore Full error to drop the frame if encoder is busy
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            // Channel closed -- receiver dropped, stop capturing.
            debug!("Frame channel closed, stopping capture");
            user_data.cancel.cancel();
        }
    }
}

/// Quick check whether PipeWire portal is available (lightweight probe).
pub(super) async fn try_portal_available() -> bool {
    // Try creating a screencast session — if it fails, portal isn't available
    match Screencast::new().await {
        Ok(screencast) => match screencast.create_session(Default::default()).await {
            Ok(_session) => true,
            Err(e) => {
                debug!("Portal session creation failed: {e}");
                false
            }
        },
        Err(e) => {
            debug!("Portal unavailable: {e}");
            false
        }
    }
}

/// Quick check whether X11 display is accessible.
pub(super) fn check_x11_available() -> bool {
    use x11rb::rust_connection::RustConnection;

    match RustConnection::connect(None) {
        Ok((_conn, _screen_num)) => true,
        Err(e) => {
            debug!("X11 connection failed: {e}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// X11 screen capture (fallback when PipeWire portal isn't available)
// ---------------------------------------------------------------------------

/// Start X11 screen capture using X GetImage for screen capture.
///
/// This is a fallback when PipeWire/Portal isn't available (e.g., bare X11
/// sessions without a working portal). For Wayland, use the PipeWire path via
/// `start_capture()`.
pub(super) async fn start_x11_capture(
    config: StreamingConfig,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<CaptureSession> {
    use x11rb::connection::Connection;
    use x11rb::rust_connection::RustConnection;

    info!("Starting X11 screen capture");

    let (conn, screen_num) =
        RustConnection::connect(None).context("Failed to connect to X11 display")?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    // Get the actual screen dimensions
    let screen_width = screen.width_in_pixels as u32;
    let screen_height = screen.height_in_pixels as u32;
    info!("X11 screen: {}x{}", screen_width, screen_height);

    // Per-monitor capture (F2: multi-monitor): grab the xcap-enumerated rect
    // for `config.monitor_index`; frames then reach the encoder through the
    // same size-rebuild path as window crops. Out-of-range or a failed
    // enumeration keeps the legacy whole-root grab clamped to the config size.
    let whole_screen = || CaptureRegion {
        x: 0,
        y: 0,
        width: config.width.min(screen_width),
        height: config.height.min(screen_height),
    };
    let region = match monitor_rect(config.monitor_index) {
        Some((x, y, width, height)) => match (i16::try_from(x), i16::try_from(y)) {
            (Ok(x), Ok(y)) => {
                info!(
                    "X11 capture target: monitor {} at ({x},{y}) {width}x{height}",
                    config.monitor_index
                );
                CaptureRegion {
                    x,
                    y,
                    width,
                    height,
                }
            }
            _ => {
                warn!("Monitor offset {x},{y} outside i16 GetImage range, capturing whole screen");
                whole_screen()
            }
        },
        None => whole_screen(),
    };

    // Spawn the capture loop on a blocking thread
    let cap_cancel = cancel.clone();
    std::thread::Builder::new()
        .name("x11-capture".into())
        .spawn(move || {
            if let Err(e) =
                run_x11_capture_loop(conn, root, region, config.fps, frame_tx, cap_cancel)
            {
                error!("X11 capture thread exited with error: {e}");
            }
        })
        .context("Failed to spawn X11 capture thread")?;

    Ok(CaptureSession::new(config, cancel))
}

/// Rectangle of the virtual screen to grab each frame (monitor-local on
/// multi-monitor setups, whole-root as the legacy fallback).
#[derive(Clone, Copy)]
struct CaptureRegion {
    x: i16,
    y: i16,
    width: u32,
    height: u32,
}

/// Run the X11 capture loop, sending frames through the channel.
///
/// Frame pacing is variable-rate: an unchanged grab is not forwarded to the
/// encoder at all, and once the screen goes static the loop backs its polling
/// off to `IDLE_FPS` so a still desktop costs ~10 GetImage round-trips per
/// second instead of 60 encodes. The Wayland/PipeWire path is already
/// damage-driven; this mirrors that behaviour for X11.
fn run_x11_capture_loop(
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    region: CaptureRegion,
    fps: u32,
    frame_tx: mpsc::Sender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<()> {
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

    /// Poll rate for a static screen — matches the "5–15 fps when idle" target.
    const IDLE_FPS: u64 = 10;

    let frame_interval = Duration::from_micros(1_000_000 / fps as u64);
    let idle_interval = Duration::from_micros(1_000_000 / IDLE_FPS.min(fps as u64));
    let mut frame_count = 0u64;
    let mut idle = false;
    let mut prev_frame: Option<Vec<u8>> = None;

    loop {
        if cancel.is_cancelled() {
            info!("X11 capture cancelled");
            break;
        }

        let frame_start = Instant::now();

        // Capture the screen using GetImage
        let result = conn.get_image(
            ImageFormat::Z_PIXMAP,
            root,
            region.x, // monitor x in the virtual screen
            region.y, // monitor y in the virtual screen
            region.width as u16,
            region.height as u16,
            !0, // plane mask (all planes)
        );

        match result {
            Ok(cookie) => match cookie.reply() {
                Ok(reply) => {
                    let frame_data = reply.data.to_vec();
                    let stride = region.width * 4;

                    // Change detection: only forward (and stay at full fps) when
                    // pixels actually moved. A cheap byte compare vs. encoding a
                    // whole frame.
                    if prev_frame.as_ref() != Some(&frame_data) {
                        if idle {
                            debug!("X11 capture: screen active, resuming {fps} fps pacing");
                            idle = false;
                        }
                        frame_count += 1;
                        if frame_count.is_multiple_of(30) {
                            debug!(
                                "X11 capture: frame #{}, {}x{}",
                                frame_count, region.width, region.height
                            );
                        }

                        let frame = VideoFrame {
                            data: frame_data.clone(),
                            width: region.width,
                            height: region.height,
                            stride,
                            timestamp: Instant::now(),
                        };
                        // Send frame to pipeline. prev_frame only advances when
                        // the frame actually reached the pipeline — if the
                        // encoder backlog forced a drop, the next poll must
                        // still see it as a change and retry.
                        match frame_tx.try_send(frame) {
                            Ok(_) => prev_frame = Some(frame_data),
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                debug!("Frame channel closed, stopping capture");
                                break;
                            }
                        }
                    } else if !idle {
                        info!("X11 capture: screen static, dropping to {IDLE_FPS} fps idle pacing");
                        idle = true;
                    }

                    // Pace: full rate while frames are being forwarded, idle
                    // rate once the screen is static.
                    let target = if idle { idle_interval } else { frame_interval };
                    let elapsed = frame_start.elapsed();
                    if elapsed < target {
                        std::thread::sleep(target - elapsed);
                    }
                }
                Err(e) => {
                    error!("X11 GetImage reply error: {e}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            },
            Err(e) => {
                error!("X11 GetImage request error: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    info!("X11 capture thread shut down after {} frames", frame_count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_stream_matches_position() {
        let streams = [(10u32, Some((0, 0))), (20, Some((1920, 0)))];
        assert_eq!(pick_stream_node(&streams, Some((1920, 0))), Some(20));
        assert_eq!(pick_stream_node(&streams, Some((0, 0))), Some(10));
    }

    #[test]
    fn pick_stream_falls_back_to_first() {
        let streams = [(10u32, Some((0, 0))), (20, Some((1920, 0)))];
        // Unknown target position → first stream.
        assert_eq!(pick_stream_node(&streams, Some((3840, 0))), Some(10));
        // No target (enumeration failed / index out of range) → first stream.
        assert_eq!(pick_stream_node(&streams, None), Some(10));
        // Backend that reports no positions → first stream.
        let unpositioned = [(7u32, None), (8, None)];
        assert_eq!(pick_stream_node(&unpositioned, Some((0, 0))), Some(7));
        assert_eq!(pick_stream_node(&[], Some((0, 0))), None);
    }
}
