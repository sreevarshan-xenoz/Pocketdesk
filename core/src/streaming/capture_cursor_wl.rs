//! The pointer of a wlroots-style compositor (Sway, Hyprland), through the
//! `ext_image_copy_capture_v1` protocol's cursor session, for the
//! screencopy capture.
//!
//! `zwlr_screencopy` can draw the pointer into the picture or leave it out,
//! and says nothing of it either way. The newer protocol has a cursor
//! session for an output: the compositor says when the pointer enters or
//! leaves the output, where it is, its hotspot, and gives its image as a
//! frame to copy whenever it changes. With it the screencopy asks for the
//! picture without the pointer, and the pointer reaches the viewer as the
//! viewer's own system pointer, drawn with no wait for the picture
//! ([`crate::protocol::cursor`]).
//!
//! It runs on a connection and a thread of its own, so a compositor that
//! stops answering it never holds the picture. [`start`] says whether the
//! session was made: a compositor without the protocol, without a pointer
//! on its seat (nothing to capture), or that refuses it leaves the
//! screencopy drawing the pointer into the picture as before.

use std::os::fd::{AsFd, AsRawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::ext_image_capture_source_v1::ExtImageCaptureSourceV1;
use wayland_protocols::ext::image_capture_source::v1::client::ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_cursor_session_v1::{
    self, ExtImageCopyCaptureCursorSessionV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_frame_v1::{
    self, ExtImageCopyCaptureFrameV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1;
use wayland_protocols::ext::image_copy_capture::v1::client::ext_image_copy_capture_session_v1::{
    self, ExtImageCopyCaptureSessionV1,
};

use super::capture_screencopy::{make_memfd, monitor_rect_physical, poll_fd};
use super::cursor_feed::{CursorFeed, normalized};
use crate::protocol::cursor::{CursorShape, rgba_from_premultiplied_bgra};

/// How long [`start`] waits for the compositor to take the session.
const SETUP_WAIT: Duration = Duration::from_secs(3);
/// Socket poll slice: bounds how soon the thread notices a cancel.
const POLL_TICK: Duration = Duration::from_millis(100);
/// Failed copies in a row after which the session is given up.
const FAILURES_ALLOWED: u32 = 5;

/// What an output said of itself.
#[derive(Default)]
struct OutputInfo {
    x: AtomicI32,
    y: AtomicI32,
    width: AtomicI32,
    height: AtomicI32,
    /// `wl_output.transform`: odd quarter turns swap the picture's sides.
    transform: AtomicI32,
}

impl OutputInfo {
    /// The size of the output's picture (the screencopy buffer's).
    fn picture(&self) -> (u32, u32) {
        let (w, h) = (
            self.width.load(Ordering::Relaxed).max(0) as u32,
            self.height.load(Ordering::Relaxed).max(0) as u32,
        );
        // Transforms 1 and 3 turn by a quarter, and 5 and 7 flip and turn.
        if matches!(self.transform.load(Ordering::Relaxed), 1 | 3 | 5 | 7) {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// Everything the events write to; read by the loop between pumps.
struct Cursor {
    feed: CursorFeed,
    output: Option<Arc<OutputInfo>>,
    /// The pointer is over the output.
    entered: bool,
    position: Option<(i32, i32)>,
    /// A hotspot said, effective with the next ready frame.
    hotspot: (i32, i32),
    /// The capture session's constraints, once it said `done`.
    size: Option<(u32, u32)>,
    format: Option<wl_shm::Format>,
    session_done: bool,
    stopped: bool,
    ready: bool,
    failed: bool,
    capabilities: Option<wl_seat::Capability>,
}

impl Cursor {
    fn say_position(&self) {
        let (Some((x, y)), Some(out)) = (self.position, &self.output) else {
            return;
        };
        let (w, h) = out.picture();
        let (x, y) = normalized(x, y, w, h);
        self.feed.set_place(x, y);
    }
}

macro_rules! no_events {
    ($($proxy:ty),+ $(,)?) => {$(
        impl Dispatch<$proxy, ()> for Cursor {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )+};
}

no_events!(
    WlShm,
    WlShmPool,
    WlBuffer,
    WlPointer,
    ExtImageCaptureSourceV1,
    ExtOutputImageCaptureSourceManagerV1,
    ExtImageCopyCaptureManagerV1,
);

impl Dispatch<WlRegistry, GlobalListContents> for Cursor {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, Arc<OutputInfo>> for Cursor {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        info: &Arc<OutputInfo>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                info.x.store(x, Ordering::Relaxed);
                info.y.store(y, Ordering::Relaxed);
                if let WEnum::Value(t) = transform {
                    info.transform.store(t as i32, Ordering::Relaxed);
                }
            }
            wl_output::Event::Mode {
                flags: WEnum::Value(flags),
                width,
                height,
                ..
            } if flags.contains(wl_output::Mode::Current) => {
                info.width.store(width, Ordering::Relaxed);
                info.height.store(height, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for Cursor {
    fn event(
        state: &mut Self,
        _: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(c),
        } = event
        {
            state.capabilities = Some(c);
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, ()> for Cursor {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureCursorSessionV1,
        event: ext_image_copy_capture_cursor_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_cursor_session_v1::Event;
        match event {
            Event::Enter => {
                state.entered = true;
                state.feed.set_visible(true);
            }
            Event::Leave => {
                state.entered = false;
                state.feed.set_visible(false);
            }
            Event::Position { x, y } => {
                state.position = Some((x, y));
                state.say_position();
            }
            Event::Hotspot { x, y } => state.hotspot = (x, y),
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for Cursor {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => {
                state.size = Some((width, height));
                state.session_done = false;
            }
            Event::ShmFormat {
                format: WEnum::Value(format),
            } => {
                // ARGB first: it is what a compositor draws a pointer in.
                if state.format.is_none() || format == wl_shm::Format::Argb8888 {
                    state.format = Some(format);
                }
            }
            Event::Done => state.session_done = true,
            Event::Stopped => state.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for Cursor {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::Ready => state.ready = true,
            Event::Failed { .. } => state.failed = true,
            _ => {}
        }
    }
}

/// The output the session is for: the one at the screencopy's origin (the
/// same pick as its own), else the monitor's index, else the first.
fn pick_output(outputs: &[(WlOutput, Arc<OutputInfo>)], monitor_index: u32) -> Option<usize> {
    if outputs.is_empty() {
        return None;
    }
    if let Some((x, y)) = monitor_rect_physical(monitor_index)
        && let Some(i) = outputs
            .iter()
            .position(|(_, o)| o.x.load(Ordering::Relaxed) == x && o.y.load(Ordering::Relaxed) == y)
    {
        return Some(i);
    }
    Some((monitor_index as usize).min(outputs.len() - 1))
}

fn bind_outputs(
    globals: &GlobalList,
    qh: &QueueHandle<Cursor>,
) -> Vec<(WlOutput, Arc<OutputInfo>)> {
    let registry = globals.registry();
    let mut outputs = Vec::new();
    globals.contents().with_list(|list| {
        for global in list {
            if global.interface == "wl_output" {
                let info = Arc::<OutputInfo>::default();
                let output: WlOutput =
                    registry.bind(global.name, global.version.min(4), qh, info.clone());
                outputs.push((output, info));
            }
        }
    });
    outputs
}

/// Dispatch what is waiting, then wait up to `wait` for more.
fn pump(
    conn: &Connection,
    queue: &mut EventQueue<Cursor>,
    state: &mut Cursor,
    wait: Duration,
) -> Result<()> {
    queue.dispatch_pending(state)?;
    queue.flush()?;
    if let Some(guard) = conn.prepare_read() {
        poll_fd(guard.connection_fd().as_raw_fd(), wait);
        let _ = guard.read();
    }
    queue.dispatch_pending(state)?;
    Ok(())
}

/// A pointer image's shared memory: a memfd of `len` bytes, mapped to read.
struct Pixels {
    fd: std::os::fd::OwnedFd,
    ptr: *mut u8,
    len: usize,
}

impl Pixels {
    fn new(len: usize) -> Result<Self> {
        let fd = make_memfd(len as u64)?;
        // SAFETY: a fresh memfd of `len` bytes, mapped shared and read-only.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            bail!(
                "mmap of the pointer buffer failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self {
            fd,
            ptr: ptr as *mut u8,
            len,
        })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping covers `len` bytes for as long as `self` lives.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Pixels {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `len` are from the successful mmap above.
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len) };
    }
}

/// Start the pointer capture of the output `monitor_index` names, reporting
/// to `feed` until `cancel`. `Ok` once the compositor took the session; an
/// error says why the screencopy must keep the pointer in the picture, and
/// the thread is stopped.
///
/// From the `Ok` until the thread ends, however it ends (the compositor
/// stopped the session, copies kept failing, any error), the feed holds the
/// pointer out of the picture ([`CursorFeed::apart`]); when it ends the
/// feed says no, and the screencopy asks for the pointer in its picture
/// again ([`CursorFeed::want_out`]).
pub fn start(feed: CursorFeed, monitor_index: u32, cancel: CancellationToken) -> Result<()> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    // Stopped when the answer does not come in time: a thread that took the
    // session after the screencopy gave up on it must not keep it.
    let stop = cancel.child_token();
    let thread_stop = stop.clone();
    std::thread::Builder::new()
        .name("cursor-capture".into())
        .spawn(move || {
            if let Err(e) = run(feed, monitor_index, thread_stop, &ready_tx) {
                // Before the handshake this is the answer; after it, only a
                // line in the log: the picture is not affected.
                if ready_tx.send(Err(anyhow::anyhow!("{e:#}"))).is_err() {
                    warn!("Pointer capture ended: {e:#}");
                }
            }
        })
        .context("Failed to spawn the pointer capture thread")?;
    match ready_rx.recv_timeout(SETUP_WAIT) {
        Ok(result) => result,
        Err(_) => {
            stop.cancel();
            bail!("the compositor did not answer the pointer capture in time")
        }
    }
}

fn run(
    feed: CursorFeed,
    monitor_index: u32,
    cancel: CancellationToken,
    ready_tx: &std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland display")?;
    let (globals, mut queue) =
        registry_queue_init::<Cursor>(&conn).context("wayland registry init failed")?;
    let qh = queue.handle();
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).context("no wl_shm")?;
    let manager: ExtImageCopyCaptureManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .context("the compositor has no ext_image_copy_capture_v1")?;
    let sources: ExtOutputImageCaptureSourceManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .context("the compositor has no ext_output_image_capture_source_v1")?;
    let seat: WlSeat = globals.bind(&qh, 1..=7, ()).context("no wl_seat")?;
    let outputs = bind_outputs(&globals, &qh);

    let mut state = Cursor {
        feed: feed.clone(),
        output: None,
        entered: false,
        position: None,
        hotspot: (0, 0),
        size: None,
        format: None,
        session_done: false,
        stopped: false,
        ready: false,
        failed: false,
        capabilities: None,
    };
    // Geometry, modes and the seat's capabilities arrive as events.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    let chosen = pick_output(&outputs, monitor_index).context("no output to capture")?;
    let (output, info) = &outputs[chosen];
    state.output = Some(info.clone());
    if !state
        .capabilities
        .is_some_and(|c| c.contains(wl_seat::Capability::Pointer))
    {
        bail!("the seat has no pointer");
    }
    let pointer = seat.get_pointer(&qh, ());
    let source = sources.create_source(output, &qh, ());
    let cursor_session = manager.create_pointer_cursor_session(&source, &pointer, &qh, ());
    let session = cursor_session.get_capture_session(&qh, ());
    queue.roundtrip(&mut state)?;
    info!("Pointer capture: a cursor session on the output, the picture without the pointer");
    // Held until this function returns, whichever way it does.
    let _apart = feed.apart();
    let _ = ready_tx.send(Ok(()));

    let mut shape_failures = 0u32;
    let mut asked = false;
    let mut frame: Option<(
        ExtImageCopyCaptureFrameV1,
        WlBuffer,
        WlShmPool,
        Pixels,
        u32,
        u32,
    )> = None;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        if state.stopped {
            debug!("The compositor stopped the pointer capture session");
            break;
        }
        // A copy asked for once the session has its constraints.
        if !asked
            && state.session_done
            && let (Some((w, h)), Some(format)) = (state.size, state.format)
            && w > 0
            && h > 0
            && matches!(format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888)
        {
            let stride = w * 4;
            let pixels = Pixels::new((stride * h) as usize)?;
            let pool = shm.create_pool(pixels.fd.as_fd(), pixels.len as i32, &qh, ());
            let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &qh, ());
            let f = session.create_frame(&qh, ());
            f.attach_buffer(&buffer);
            f.damage_buffer(0, 0, w as i32, h as i32);
            f.capture();
            state.ready = false;
            state.failed = false;
            frame = Some((f, buffer, pool, pixels, w, h));
            asked = true;
        }
        pump(&conn, &mut queue, &mut state, POLL_TICK)?;
        if state.ready || state.failed {
            let Some((f, buffer, pool, pixels, w, h)) = frame.take() else {
                continue;
            };
            if state.ready {
                shape_failures = 0;
                let alpha = state.format == Some(wl_shm::Format::Argb8888);
                // The hotspot said before is effective with this frame.
                let (hx, hy) = state.hotspot;
                if let Some(rgba) = rgba_from_premultiplied_bgra(pixels.bytes(), w, h, w * 4, alpha)
                    && let Some(shape) =
                        CursorShape::fitted(w, h, hx.max(0) as u32, hy.max(0) as u32, rgba)
                {
                    feed.set_shape(shape);
                }
            } else {
                shape_failures += 1;
                if shape_failures >= FAILURES_ALLOWED {
                    warn!(
                        "The compositor failed the pointer's image {shape_failures} times: no more asked"
                    );
                    break;
                }
                // A failed copy may mean new constraints: wait for them.
                state.session_done = false;
                std::thread::sleep(Duration::from_millis(50));
            }
            f.destroy();
            buffer.destroy();
            pool.destroy();
            asked = false;
            state.ready = false;
            state.failed = false;
        }
    }
    cursor_session.destroy();
    let _ = queue.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_output_swaps_its_pictures_sides() {
        let info = OutputInfo::default();
        info.width.store(1920, Ordering::Relaxed);
        info.height.store(1080, Ordering::Relaxed);
        assert_eq!(info.picture(), (1920, 1080));
        info.transform.store(1, Ordering::Relaxed);
        assert_eq!(info.picture(), (1080, 1920));
        info.transform.store(2, Ordering::Relaxed);
        assert_eq!(info.picture(), (1920, 1080), "half a turn keeps them");
        info.transform.store(7, Ordering::Relaxed);
        assert_eq!(info.picture(), (1080, 1920));
    }

    /// The pointer's pixels around `(x, y)` in a BGRA frame: how many
    /// distinct colours a 40 by 40 box holds. A flat background has one;
    /// a pointer drawn over it adds more.
    #[cfg(feature = "injection")]
    fn colours_around(frame: &crate::streaming::VideoFrame, x: u32, y: u32) -> usize {
        let mut seen = std::collections::HashSet::new();
        for row in y.saturating_sub(4)..(y + 36).min(frame.height) {
            for col in x.saturating_sub(4)..(x + 36).min(frame.width) {
                let at = (row * frame.stride + col * 4) as usize;
                seen.insert([
                    frame.data[at],
                    frame.data[at + 1],
                    frame.data[at + 2],
                    frame.data[at + 3],
                ]);
            }
        }
        seen.len()
    }

    /// The whole thing against a compositor of its own: the pointer's
    /// shape and places reach the feed, and the screencopy asked with the
    /// feed says the pointer is out of its picture (and leaves it out
    /// where the compositor has a cursor plane). Run it in a headless sway
    /// (`WLR_BACKENDS=headless`, its own `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`
    /// and `SWAYSOCK` set) with a cursor theme; it moves that sway's pointer
    /// through a virtual one and never touches another session. Ignored: it
    /// needs that compositor.
    #[test]
    #[ignore = "needs a headless sway of its own (WAYLAND_DISPLAY), with a cursor theme"]
    #[cfg(feature = "injection")]
    fn a_headless_sways_pointer_reaches_the_feed_and_is_left_out_of_the_picture() {
        use crate::streaming::StreamingConfig;
        use crate::streaming::capture_screencopy::start_screencopy_capture;
        use crate::streaming::cursor_feed::Marks;
        use std::sync::atomic::AtomicBool;
        use std::time::Instant;
        use tokio::sync::watch;
        assert!(
            std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("SWAYSOCK").is_some(),
            "run this in a headless sway of its own"
        );
        let mut input = crate::input::wlr_input::WlrInput::connect().expect("a virtual pointer");
        // Over the output's middle-left, where nothing else is drawn.
        let (nx, ny) = (20_000u16, 24_000u16);
        input.motion_absolute(nx, ny).unwrap();
        std::thread::sleep(Duration::from_millis(300));

        let feed = CursorFeed::new();
        feed.set_tracking(true);
        let cancel = CancellationToken::new();
        start(feed.clone(), 0, cancel.clone()).expect("a cursor session");
        let mut marks = Marks::default();
        let deadline = Instant::now() + Duration::from_secs(8);
        let (mut shape, mut place) = (None, None);
        let mut step = 0u16;
        while Instant::now() < deadline && (shape.is_none() || place.is_none()) {
            step += 1;
            input.motion_absolute(nx + step % 7, ny).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let got = feed.take(&mut marks);
            shape = shape.or(got.shape);
            place = place.or(got.place);
        }
        let shape = shape.expect("the pointer's shape reached the feed");
        let place = place.expect("the pointer's place reached the feed");
        eprintln!(
            "shape {}x{} hotspot ({},{}), place ({},{}) of 65535",
            shape.width, shape.height, shape.hot_x, shape.hot_y, place.0, place.1
        );
        assert!(shape.width > 0 && shape.height > 0);
        assert!(
            shape.rgba.iter().skip(3).step_by(4).any(|a| *a > 0),
            "an image with something in it"
        );
        cancel.cancel();

        // The picture, with and without the pointer in it.
        let mut frame_of = |feed: Option<CursorFeed>| {
            let (tx, mut rx) =
                crate::streaming::latest_frame::channel::<crate::streaming::VideoFrame>();
            let cancel = CancellationToken::new();
            let (_window_tx, window_rx) = watch::channel(0u64);
            let session = start_screencopy_capture(
                StreamingConfig::default(),
                tx,
                cancel.clone(),
                window_rx,
                Arc::new(AtomicBool::new(false)),
                feed.clone(),
            )
            .expect("screencopy");
            std::thread::sleep(Duration::from_millis(500));
            while rx.try_recv().is_some() {}
            input.motion_absolute(nx, ny).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            let frame = rx.blocking_recv().expect("a frame");
            // Asked before the capture ends: its end puts the pointer back.
            let separate = feed.as_ref().is_some_and(CursorFeed::separate);
            drop(session);
            cancel.cancel();
            (frame, separate)
        };
        let (embedded, _) = frame_of(None);
        let (apart, separate) = frame_of(Some(CursorFeed::new()));
        assert!(separate, "the screencopy took the pointer out");
        let px = (u32::from(nx) * embedded.width) / 65535;
        let py = (u32::from(ny) * embedded.height) / 65535;
        let with = colours_around(&embedded, px, py);
        let without = colours_around(&apart, px, py);
        eprintln!("colours around the pointer: {with} asked with it, {without} asked without it");
        // A compositor with a cursor plane (Hyprland, Sway on a real
        // output) leaves the pointer out of the second: `without` is 1 over
        // a flat background. A headless sway draws its pointer in software,
        // into the output's own buffer, which the screencopy cannot leave
        // it out of: the counts are equal there, and the owed device check
        // (docs/device-verification-checklist.md) is for the real thing.
        assert!(
            without <= with,
            "asking without the pointer never adds to the picture"
        );
    }

    /// Against a compositor that has the protocol (a headless sway the test
    /// is started in, or a developer machine): the session is made, or the
    /// reason it cannot be is said. Never asserts the compositor supports
    /// it; the live test in `server/tests/local_cursor_live.rs` does.
    #[test]
    #[ignore = "connects to the running compositor: run it in a headless sway of its own"]
    fn a_session_is_made_or_its_refusal_is_said() {
        let feed = CursorFeed::new();
        let cancel = CancellationToken::new();
        match start(feed, 0, cancel.clone()) {
            Ok(()) => eprintln!("pointer capture: a session was made"),
            Err(e) => eprintln!("pointer capture: not here ({e:#})"),
        }
        cancel.cancel();
    }
}
