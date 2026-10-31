//! Linux capture backends: XDG Desktop Portal + PipeWire, and direct X11
//! `GetImage`. Moved verbatim out of `capture.rs`, which keeps the
//! OS-independent backend selection; the wlroots screencopy backend lives in
//! `capture_screencopy.rs`.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{Screencast, SelectSourcesOptions, SourceType};
use pipewire::stream::StreamFlags;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use super::capture::{CaptureSession, monitor_rect};
use super::{StreamingConfig, VideoFrame};

/// Shared state between the PipeWire thread and stream callbacks.
struct StreamUserData {
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
/// 4. Sends frames through the provided newest-frame slot
/// 5. Returns a [`CaptureSession`] that manages the capture lifetime
///
/// The capture runs continuously until `cancel` is triggered or the portal
/// session ends.
pub async fn start_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    screen_grant: Option<std::path::PathBuf>,
) -> Result<CaptureSession> {
    info!(
        "Starting screen capture: {}x{}@{}fps",
        config.width, config.height, config.fps
    );
    // N8: input goes through the portal here, so the screen and the input
    // come from one RemoteDesktop session, under one grant.
    // A desktop without RemoteDesktop, or one that refused it, still gets
    // its picture (N8 review); a dialog the owner cancelled, or left
    // unanswered, is not asked again.
    if crate::streaming::portal_input::wanted() {
        match start_remote_desktop_capture(
            config.clone(),
            frame_tx.clone(),
            cancel.clone(),
            screen_grant.clone(),
        )
        .await
        {
            Ok(session) => return Ok(session),
            Err(e) if asked_already(&e) => return Err(e),
            Err(e) => warn!("No remote desktop session ({e:#}): the screen alone, with no input"),
        }
    }

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

    // Steps 2 to 4, the session closed on any failure (N8 review: a dialog
    // left up, and a session nobody held once it was answered).
    let picked = async {
        // Step 2: Select sources (monitor capture)
        // Determine which monitor to capture (F2: multi-monitor).
        // ashpd screencast captures all monitors; we'll select based on config.monitor_index.
        // For now, Portal captures primary monitor by default.
        let source_types = ashpd::enumflags2::BitFlags::from(SourceType::Monitor);
        // N8: with the owner's choice, ask the portal to keep its grant and
        // restore it from the token it gave last; a token it refuses only means
        // the dialog again.
        let saved = screen_grant.as_deref().and_then(crate::portal_grant::read);
        let (persist, token) =
            crate::portal_grant::request(screen_grant.is_some(), saved.as_deref());
        let persist_mode = persist_mode(persist);
        tokio::time::timeout(
            Duration::from_secs(10),
            screencast.select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(Some(source_types))
                    .set_multiple(Some(true))
                    .set_persist_mode(persist_mode)
                    .set_restore_token(token),
            ),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Portal request timed out — is xdg-desktop-portal running?"))?
        .context("Failed to select sources")?;

        // Step 3: Start capture and get PipeWire node ID
        debug!("Starting capture and waiting for PipeWire stream...");
        // The owner answers the dialog here the first time: room to read it.
        let response = tokio::time::timeout(
            START_WAIT,
            screencast.start(&session, None, Default::default()),
        )
        .await
        .map_err(|_| anyhow::Error::new(NoAnswer))?
        .context("Failed to start capture")?;

        let streams = response
            .response()
            .context("No response from capture start")?;
        // Each start answers with a new token, to restore the grant next time.
        if let (Some(dir), Some(token)) = (screen_grant.as_deref(), streams.restore_token()) {
            match crate::portal_grant::write(dir, token) {
                Ok(()) => info!("Screen-sharing grant kept for the next session"),
                Err(e) => warn!(error = %e, "Could not keep the screen-sharing grant"),
            }
        }

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
        Ok::<u32, anyhow::Error>(node_id)
    }
    .await;
    let node_id = match picked {
        Ok(node_id) => node_id,
        Err(e) => {
            let _ = session.close().await;
            return Err(e);
        }
    };
    // Closed when the capture ends, so no session outlives it.
    let closer = cancel.clone();
    tokio::spawn(async move {
        closer.cancelled().await;
        let _ = session.close().await;
    });

    // Step 5: Spawn the PipeWire capture thread
    spawn_pipewire_capture(node_id, config.clone(), frame_tx, cancel.clone())?;

    Ok(CaptureSession::new(config, cancel))
}

/// The PipeWire capture thread for the stream `node_id`.
fn spawn_pipewire_capture(
    node_id: u32,
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<()> {
    std::thread::Builder::new()
        .name("pipewire-capture".into())
        .spawn(move || {
            if let Err(e) = run_pipewire_capture(node_id, &config, frame_tx, cancel) {
                error!("PipeWire capture thread exited with error: {e}");
            } else {
                info!("PipeWire capture thread exited normally");
            }
        })
        .context("Failed to spawn PipeWire capture thread")?;
    Ok(())
}

/// How long the portal's Start may take: the first time, the owner reads
/// and answers its dialog (N8 review: 10 s failed a slow first grant).
const START_WAIT: Duration = Duration::from_secs(60);

/// The portal's dialog went unanswered for [`START_WAIT`].
#[derive(Debug)]
struct NoAnswer;

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("No answer to the screen-sharing dialog within a minute")
    }
}

impl std::error::Error for NoAnswer {}

/// Whether `e` is the owner cancelling the portal's dialog, or no one
/// answering it, which is not asked again in another form: with no one at
/// the desk a second dialog only doubled the wait, to two minutes (KDE
/// Plasma 6.8, 2026-10-31).
fn asked_already(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause.is::<NoAnswer>()
            || matches!(
                cause.downcast_ref::<ashpd::Error>(),
                Some(ashpd::Error::Response(
                    ashpd::desktop::ResponseError::Cancelled
                ))
            )
    })
}

/// What the portal is asked to do with its grant (N8).
fn persist_mode(persist: crate::portal_grant::Persist) -> PersistMode {
    match persist {
        crate::portal_grant::Persist::No => PersistMode::DoNot,
        crate::portal_grant::Persist::UntilRevoked => PersistMode::ExplicitlyRevoked,
    }
}

/// N8: the screen with the keyboard and pointer, through one RemoteDesktop
/// session, where the input injector has no other way in (GNOME and KDE
/// without uinput). One grant covers both, kept as the screen's is when the
/// owner chose so. A worker owns the session for the capture's life and
/// takes the injector's events ([`crate::streaming::portal_input`]).
async fn start_remote_desktop_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
    screen_grant: Option<std::path::PathBuf>,
) -> Result<CaptureSession> {
    use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions};
    let unavailable =
        || anyhow::anyhow!("Portal request timed out — is xdg-desktop-portal running?");
    let remote = tokio::time::timeout(Duration::from_secs(10), RemoteDesktop::new())
        .await
        .map_err(|_| unavailable())?
        .context("Failed to create the RemoteDesktop portal")?;
    let screencast = tokio::time::timeout(Duration::from_secs(10), Screencast::new())
        .await
        .map_err(|_| unavailable())?
        .context("Failed to create Screencast portal")?;
    let session = tokio::time::timeout(
        Duration::from_secs(10),
        remote.create_session(Default::default()),
    )
    .await
    .map_err(|_| unavailable())?
    .context("Failed to create the remote desktop session")?;

    // The session closed on any failure from here (N8 review).
    let picked = async {
        let saved = screen_grant
            .as_deref()
            .and_then(crate::portal_grant::read_remote);
        let (persist, token) =
            crate::portal_grant::request(screen_grant.is_some(), saved.as_deref());
        tokio::time::timeout(
            Duration::from_secs(10),
            remote.select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(persist_mode(persist))
                    .set_restore_token(token),
            ),
        )
        .await
        .map_err(|_| unavailable())?
        .context("Failed to select the keyboard and pointer")?;
        tokio::time::timeout(
            Duration::from_secs(10),
            screencast.select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(Some(ashpd::enumflags2::BitFlags::from(SourceType::Monitor)))
                    .set_multiple(Some(true)),
            ),
        )
        .await
        .map_err(|_| unavailable())?
        .context("Failed to select sources")?;
        // The owner answers the dialog here the first time: room to read it.
        let started =
            tokio::time::timeout(START_WAIT, remote.start(&session, None, Default::default()))
                .await
                .map_err(|_| anyhow::Error::new(NoAnswer))?
                .context("Failed to start the remote desktop session")?
                .response()
                .context("No response from the remote desktop start")?;
        if let (Some(dir), Some(token)) = (screen_grant.as_deref(), started.restore_token()) {
            match crate::portal_grant::write_remote(dir, token) {
                Ok(()) => info!("Screen-sharing grant kept for the next session"),
                Err(e) => warn!(error = %e, "Could not keep the screen-sharing grant"),
            }
        }

        let stream_infos: Vec<_> = started
            .streams()
            .iter()
            .map(|s| (s.pipe_wire_node_id(), s.position()))
            .collect();
        let want = monitor_rect(config.monitor_index).map(|(x, y, _, _)| (x, y));
        let node_id = pick_stream_node(&stream_infos, want)
            .context("No PipeWire stream available from the remote desktop session")?;
        let size = started
            .streams()
            .iter()
            .find(|s| s.pipe_wire_node_id() == node_id)
            .and_then(|s| s.size())
            .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32))
            .unwrap_or((config.width, config.height));
        info!(
            "Remote desktop session: node_id={node_id}, {}x{}, input through the portal",
            size.0, size.1
        );
        Ok::<(u32, (u32, u32)), anyhow::Error>((node_id, size))
    }
    .await;
    let (node_id, size) = match picked {
        Ok(picked) => picked,
        Err(e) => {
            let _ = session.close().await;
            return Err(e);
        }
    };

    // The worker: the injector's events, in order, until the capture ends.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registered = crate::streaming::portal_input::register(tx);
    let worker_cancel = cancel.clone();
    tokio::spawn(async move {
        let _registered = registered;
        let mut said = false;
        loop {
            tokio::select! {
                _ = worker_cancel.cancelled() => break,
                event = rx.recv() => match event {
                    Some(event) => {
                        if let Err(e) = apply_portal_event(&remote, &session, node_id, size, event).await {
                            // Said once a session: input dead and silent
                            // is worse than one line (N8 review).
                            if !std::mem::replace(&mut said, true) {
                                warn!(error = %e, "Portal input not taken");
                            }
                        }
                    }
                    // Only its own capture's end closes the session.
                    None => {
                        worker_cancel.cancelled().await;
                        break;
                    }
                },
            }
        }
        let _ = session.close().await;
    });

    spawn_pipewire_capture(node_id, config.clone(), frame_tx, cancel.clone())?;
    Ok(CaptureSession::new(config, cancel))
}

/// Tell the portal one input event (N8).
async fn apply_portal_event(
    remote: &ashpd::desktop::remote_desktop::RemoteDesktop,
    session: &ashpd::desktop::Session<ashpd::desktop::remote_desktop::RemoteDesktop>,
    stream: u32,
    size: (u32, u32),
    event: crate::streaming::portal_input::PortalEvent,
) -> ashpd::Result<()> {
    use crate::streaming::portal_input::{PortalEvent, absolute_position};
    use ashpd::desktop::remote_desktop::{Axis, KeyState};
    let state = |pressed: bool| {
        if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        }
    };
    match event {
        PortalEvent::Motion { dx, dy } => {
            remote
                .notify_pointer_motion(session, dx, dy, Default::default())
                .await
        }
        PortalEvent::Absolute { x_norm, y_norm } => {
            let (x, y) = absolute_position(x_norm, y_norm, size);
            remote
                .notify_pointer_motion_absolute(session, stream, x, y, Default::default())
                .await
        }
        PortalEvent::Button { code, pressed } => {
            remote
                .notify_pointer_button(session, code, state(pressed), Default::default())
                .await
        }
        PortalEvent::Scroll { axis, steps } => {
            let axis = if axis == 0 {
                Axis::Vertical
            } else {
                Axis::Horizontal
            };
            remote
                .notify_pointer_axis_discrete(session, axis, steps, Default::default())
                .await
        }
        PortalEvent::Key { code, pressed } => {
            remote
                .notify_keyboard_keycode(session, code, state(pressed), Default::default())
                .await
        }
        PortalEvent::Keysym { sym, pressed } => {
            remote
                .notify_keyboard_keysym(session, sym, state(pressed), Default::default())
                .await
        }
    }
}

/// Run the PipeWire capture loop in a dedicated thread.
fn run_pipewire_capture(
    node_id: u32,
    config: &StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
    let stream = pipewire::stream::StreamBox::new(&core, "pocketdesk-capture", stream_props)
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

    // Connect the stream to the specific PipeWire node, offering the formats
    // the encoder takes. A stream that offers none is never given one: the
    // link stays unnegotiated and no frame ever comes (seen on KDE Plasma
    // 6.8, 2026-10-31). Memory buffers, as no modifier is offered; the
    // format chosen is learned in the param_changed callback.
    let format = video_format_pod(config)?;
    let format =
        libspa::pod::Pod::from_bytes(&format).context("The capture's video format is not a pod")?;
    let mut params = [format];
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

/// The `EnumFormat` the capture offers: raw video in the byte orders the
/// encoder reads as `bgra` (BGRx first, as compositors share no alpha), any
/// size, any rate up to 1000 fps, preferring the configured ones.
fn video_format_pod(config: &StreamingConfig) -> Result<Vec<u8>> {
    use libspa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use libspa::param::video::VideoFormat;
    use libspa::utils::{Fraction, Rectangle};
    let object = libspa::pod::object!(
        libspa::utils::SpaTypes::ObjectParamFormat,
        libspa::param::ParamType::EnumFormat,
        libspa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        libspa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        libspa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA
        ),
        libspa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: config.width.max(1),
                height: config.height.max(1)
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 16384,
                height: 16384
            }
        ),
        libspa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction {
                num: config.fps.max(1),
                denom: 1
            },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    let (cursor, _) = libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(object),
    )
    .map_err(|e| anyhow::anyhow!("The capture's video format did not serialize: {e:?}"))?;
    Ok(cursor.into_inner())
}

/// Handle the param_changed callback to learn the negotiated video format.
fn on_param_changed(id: u32, param: Option<&libspa::pod::Pod>, user_data: &StreamUserData) {
    if id != libspa::param::ParamType::Format.as_raw() {
        return;
    }

    let Some(param) = param else {
        debug!("param_changed: format removed");
        return;
    };

    let Some(video_info) = negotiated(id, param) else {
        warn!("Failed to parse video format from param_changed");
        return;
    };
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

/// The video format a `param_changed` of `id` settled on, when it is the
/// stream's Format (`SPA_PARAM_Format`, 4; this was checked against 0,
/// `SPA_PARAM_Invalid`, so the frames kept the configured size whatever the
/// compositor sent, 2026-10-31).
fn negotiated(id: u32, param: &libspa::pod::Pod) -> Option<libspa::param::video::VideoInfoRaw> {
    if id != libspa::param::ParamType::Format.as_raw() {
        return None;
    }
    let mut video_info = libspa::param::video::VideoInfoRaw::new();
    video_info.parse(param).ok()?;
    Some(video_info)
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
        gpu: None,
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
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
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
                            gpu: None,
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
    fn the_capture_offers_raw_bgr_video_at_the_configured_size_and_rate() {
        use libspa::param::format::{FormatProperties, MediaSubtype, MediaType};
        use libspa::param::video::VideoFormat;
        use libspa::pod::{ChoiceValue, Value};
        use libspa::utils::{Choice, ChoiceEnum, Fraction, Id, Rectangle};
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            ..StreamingConfig::default()
        };
        let bytes = video_format_pod(&config).unwrap();
        let (_, value) =
            libspa::pod::deserialize::PodDeserializer::deserialize_any_from(&bytes).unwrap();
        let Value::Object(object) = value else {
            panic!("not an object: {value:?}");
        };
        assert_eq!(object.id, libspa::param::ParamType::EnumFormat.as_raw());
        let find = |key: FormatProperties| {
            object
                .properties
                .iter()
                .find(|p| p.key == key.as_raw())
                .map(|p| p.value.clone())
                .unwrap_or_else(|| panic!("no {key:?}"))
        };
        assert_eq!(
            find(FormatProperties::MediaType),
            Value::Id(Id(MediaType::Video.as_raw()))
        );
        assert_eq!(
            find(FormatProperties::MediaSubtype),
            Value::Id(Id(MediaSubtype::Raw.as_raw()))
        );
        let Value::Choice(ChoiceValue::Id(Choice(
            _,
            ChoiceEnum::Enum {
                default,
                alternatives,
            },
        ))) = find(FormatProperties::VideoFormat)
        else {
            panic!("the formats are not an enum");
        };
        assert_eq!(default, Id(VideoFormat::BGRx.as_raw()));
        assert!(alternatives.contains(&Id(VideoFormat::BGRA.as_raw())));
        let Value::Choice(ChoiceValue::Rectangle(Choice(_, ChoiceEnum::Range { default, .. }))) =
            find(FormatProperties::VideoSize)
        else {
            panic!("the size is not a range");
        };
        assert_eq!(
            default,
            Rectangle {
                width: 1280,
                height: 720
            }
        );
        let Value::Choice(ChoiceValue::Fraction(Choice(_, ChoiceEnum::Range { default, min, .. }))) =
            find(FormatProperties::VideoFramerate)
        else {
            panic!("the rate is not a range");
        };
        assert_eq!(default, Fraction { num: 30, denom: 1 });
        assert_eq!(min, Fraction { num: 0, denom: 1 });
    }

    #[test]
    fn the_size_the_compositor_settled_on_is_read_from_the_format_param() {
        use libspa::param::ParamType;
        use libspa::param::format::{FormatProperties, MediaSubtype, MediaType};
        use libspa::param::video::VideoFormat;
        use libspa::utils::{Fraction, Rectangle};
        let object = libspa::pod::object!(
            libspa::utils::SpaTypes::ObjectParamFormat,
            ParamType::Format,
            libspa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            libspa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            libspa::pod::property!(FormatProperties::VideoFormat, Id, VideoFormat::BGRx),
            libspa::pod::property!(
                FormatProperties::VideoSize,
                Rectangle,
                Rectangle {
                    width: 1280,
                    height: 720
                }
            ),
            libspa::pod::property!(
                FormatProperties::VideoFramerate,
                Fraction,
                Fraction { num: 0, denom: 1 }
            ),
        );
        let (cursor, _) = libspa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &libspa::pod::Value::Object(object),
        )
        .unwrap();
        let bytes = cursor.into_inner();
        let pod = libspa::pod::Pod::from_bytes(&bytes).unwrap();

        let info = negotiated(ParamType::Format.as_raw(), pod).expect("the format");
        assert_eq!(
            info.size(),
            Rectangle {
                width: 1280,
                height: 720
            }
        );
        assert_eq!(info.format(), VideoFormat::BGRx);
        // Only the Format param is the one settled on.
        assert!(negotiated(ParamType::EnumFormat.as_raw(), pod).is_none());
        assert!(negotiated(0, pod).is_none());
    }

    #[test]
    fn a_dialog_cancelled_or_unanswered_is_not_asked_again() {
        let unanswered = anyhow::Error::new(NoAnswer).context("Failed to start");
        assert!(asked_already(&unanswered));
        let cancelled = anyhow::Error::new(ashpd::Error::Response(
            ashpd::desktop::ResponseError::Cancelled,
        ))
        .context("Failed to start");
        assert!(asked_already(&cancelled));
        // No RemoteDesktop portal, or one that failed: the screen alone.
        let failed =
            anyhow::Error::new(ashpd::Error::Response(ashpd::desktop::ResponseError::Other));
        assert!(!asked_already(&failed));
        assert!(!asked_already(&anyhow::anyhow!("Portal request timed out")));
    }

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
