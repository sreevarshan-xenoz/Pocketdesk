//! R4 E4 — compositor-level privacy shield.
//!
//! While a session is actively driving the desktop, bystanders at the
//! physical machine have no way to tell that somebody remote is in control
//! (Tier-3 #15's `EVIOCGRAB` makes local input *silent*, but a silent
//! keyboard is not an announcement). This module paints an unmistakable
//! full-perimeter "session active" frame on top of everything using the
//! wlroots `zwlr_layer_shell_v1` protocol — the same layer waybar/notifications
//! ride, `OVERLAY` here so nothing covers it.
//!
//! The shield is a **transparent-centre, opaque-border ring**, not a
//! full-screen dim: the remote user still needs to see and work in the
//! desktop, so only the edges carry the signal. Its exact geometry is a pure
//! function ([`render_shield`]) and unit-tested; whether the pixels actually
//! reach the compositor is verified live in an `#[ignore]`d test that grabs a
//! frame through the R4 B1 screencopy backend.
//!
//! Input is already blocked device-side by `EVIOCGRAB` (privacy.rs). The
//! compositor-native alternative (`zwp_input_inhibitor_v1`) is deliberately
//! *not* attempted: this box's Hyprland does not advertise that global, and
//! the grab path works without it.
//!
//! Fallback is explicit and safe, mirroring the B1 capture backend: if there
//! is no Wayland display, or the compositor does not advertise
//! `zwlr_layer_shell_v1`/`wl_shm` (GNOME/KDE, headless, X11-only), [`show`]
//! returns `None` and the caller proceeds with input-grab alone. The shield
//! never panics on a machine that cannot show it.

use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use std::fs::File;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

/// How often the pump thread drains the compositor socket — bounds
/// backpressure while keeping teardown latency low.
const POLL_TICK: Duration = Duration::from_millis(100);

/// Frame colour as native-endian ARGB (`0xAARRGGBB`): a strong red with
/// ~90% alpha. Red is the near-universal "recording / live" signal and reads
/// against any wallpaper.
pub const SHIELD_ARGB: u32 = 0xE6FF_3B30;

/// Border thickness: ~6% of the smaller screen edge, clamped to a visible but
/// unobtrusive 8..=48 px.
pub fn shield_thickness(width: u32, height: u32) -> u32 {
    let pct = (width.min(height) as f32 * 0.06) as u32;
    pct.clamp(8, 48)
}

/// Is `(x, y)` inside the perimeter ring for a `width`×`height` frame of the
/// given `thickness`? Pure and total: a thickness reaching across an axis
/// simply fills that axis.
pub fn is_ring(x: u32, y: u32, width: u32, height: u32, thickness: u32) -> bool {
    x < thickness
        || y < thickness
        || x >= width.saturating_sub(thickness)
        || y >= height.saturating_sub(thickness)
}

/// Render a `width`×`height` ARGB8888 buffer (little-endian bytes, stride =
/// width*4): ring pixels are `argb`, the interior is fully transparent.
pub fn render_shield(width: u32, height: u32, thickness: u32, argb: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
    for y in 0..height {
        for x in 0..width {
            if is_ring(x, y, width, height, thickness) {
                let off = ((y * width + x) as usize) * 4;
                buf[off..off + 4].copy_from_slice(&argb.to_ne_bytes());
            }
        }
    }
    buf
}

/// Newest `configure(serial, width, height)` the compositor sent.
type ConfigureSlot = Arc<Mutex<Option<(u32, u32, u32)>>>;

/// Local Wayland dispatch state (the orphan rule forbids implementing
/// `Dispatch` for the foreign `()` type). Every handler is inert except the
/// layer-surface `configure`, which records the allotted size.
struct ShieldState;

/// A live shield. Dropping it tears the overlay down (the pump thread exits
/// and closes its Wayland connection, which unmaps the surface).
pub struct PrivacyShield {
    stop: Option<Sender<()>>,
    pump: Option<JoinHandle<()>>,
}

impl Drop for PrivacyShield {
    fn drop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        // Detach the pump: it exits on the send above and cleans its Wayland
        // objects. Release must never block on a compositor roundtrip.
        if let Some(pump) = self.pump.take() {
            let _ = pump;
        }
    }
}

/// Show the shield. Returns `None` (and no side effects) when the environment
/// cannot support it; the caller then relies on the input grab alone.
pub fn show() -> Option<PrivacyShield> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<ShieldState>(&conn).ok()?;
    let qh = queue.handle();
    let mut state = ShieldState;

    let Ok(shm): Result<WlShm, _> = globals.bind(&qh, 1..=1, ()) else {
        return None;
    };
    let Ok(layer_shell): Result<ZwlrLayerShellV1, _> = globals.bind(&qh, 1..=1, ()) else {
        return None;
    };
    let Ok(compositor): Result<WlCompositor, _> = globals.bind(&qh, 1..=1, ()) else {
        return None;
    };

    let configure: ConfigureSlot = Arc::new(Mutex::new(None));
    let surface = compositor.create_surface(&qh, ());
    let layer = layer_shell.get_layer_surface(
        &surface,
        None, // output: None spans the whole compositor space
        Layer::Overlay,
        "pocketdesk-shield".to_string(),
        &qh,
        configure.clone(),
    );
    layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer.set_margin(0, 0, 0, 0);
    layer.set_exclusive_zone(0); // overlay on top, pushes nothing aside
    layer.set_keyboard_interactivity(KeyboardInteractivity::None); // click-through
    surface.commit();

    // The compositor answers the layer request with a configure carrying the
    // size it allotted us (the whole anchored region for a full-edge anchor).
    let (serial, width, height) = loop {
        if queue.roundtrip(&mut state).is_err() {
            return None;
        }
        if let Some(c) = *configure.lock().unwrap() {
            break c;
        }
    };

    let thickness = shield_thickness(width, height);
    let pixels = render_shield(width, height, thickness, SHIELD_ARGB);
    let stride = width * 4;
    let (fd, pool, buffer) = match make_buffer(&shm, &qh, &pixels, width, height, stride) {
        Ok(v) => v,
        Err(_) => return None,
    };

    layer.ack_configure(serial);
    layer.set_size(width, height);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
    if queue.flush().is_err() {
        return None;
    }

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let pump = std::thread::spawn(move || {
        // Own everything the overlay needs for its lifetime; when this closure
        // ends the proxies drop and the connection closes, unmapping the layer.
        let _conn = conn;
        let _shm = shm;
        let _layer_shell = layer_shell;
        let _compositor = compositor;
        let _fd = fd;
        let _pool = pool;
        let _buffer = buffer;
        let _configure = configure;
        pump_events(queue, &stop_rx);
        layer.destroy();
        surface.destroy();
        let _ = _conn.flush();
    });

    Some(PrivacyShield {
        stop: Some(stop_tx),
        pump: Some(pump),
    })
}

/// Keep the connection serviced until `stop_rx` fires. We dispatch pending
/// compositor events (so the socket never backs up) but never re-render: the
/// frame geometry is fixed at [`show`] time, a documented limitation.
fn pump_events(mut queue: wayland_client::EventQueue<ShieldState>, stop_rx: &Receiver<()>) {
    let mut state = ShieldState;
    loop {
        match stop_rx.recv_timeout(POLL_TICK) {
            Ok(()) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let _ = queue.dispatch_pending(&mut state);
    }
}

/// Create an `Argb8888` `wl_buffer` over a memfd carrying `pixels`. The
/// returned fd is kept open for the buffer's lifetime (the compositor maps
/// the shared memory itself).
fn make_buffer(
    shm: &WlShm,
    qh: &QueueHandle<ShieldState>,
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: u32,
) -> anyhow::Result<(OwnedFd, WlShmPool, WlBuffer)> {
    let len = pixels.len();
    let fd = new_memfd()?;
    let mut file = File::from(fd);
    file.set_len(len as u64)?;
    file.write_all(pixels)?;
    file.flush()?;
    let fd = OwnedFd::from(file);
    let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    Ok((fd, pool, buffer))
}

fn new_memfd() -> anyhow::Result<OwnedFd> {
    let name = std::ffi::CString::new("pocketdesk-shield").unwrap();
    // SAFETY: memfd_create with a valid C string; fd checked below.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: raw is a fresh valid fd from memfd_create.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

// ---- Dispatch plumbing (all inert: the shield is a static surface). --------

impl Dispatch<WlRegistry, GlobalListContents> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        _event: <WlShm as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        _event: <WlBuffer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &WlSurface,
        _event: <WlSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ConfigureSlot> for ShieldState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as Proxy>::Event,
        data: &ConfigureSlot,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                *data.lock().unwrap() = Some((serial, width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thickness_grows_with_screen_and_clamps() {
        assert_eq!(shield_thickness(1920, 1080), 48); // 6% of 1080 ≈ 64 → clamp 48
        assert_eq!(shield_thickness(600, 400), 24); // 6% of 400 = 24, within range
        assert_eq!(shield_thickness(100, 100), 8); // 6 → clamp low
    }

    #[test]
    fn ring_is_edges_only() {
        let (w, h, t) = (20, 10, 3);
        // corners and edges are ring; interior is not.
        assert!(is_ring(0, 0, w, h, t));
        assert!(is_ring(w - 1, h - 1, w, h, t));
        assert!(is_ring(10, 1, w, h, t));
        assert!(!is_ring(10, 5, w, h, t));
    }

    #[test]
    fn render_marks_ring_and_leaves_center_transparent() {
        let (w, h, t) = (8, 6, 2);
        let buf = render_shield(w, h, t, SHIELD_ARGB);
        assert_eq!(buf.len(), (w * h * 4) as usize);
        let px = |x: u32, y: u32| {
            let o = ((y * w + x) as usize) * 4;
            u32::from_ne_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]])
        };
        assert_eq!(px(0, 0), SHIELD_ARGB); // top-left ring
        assert_eq!(px(4, 3), 0); // interior transparent
        // Oversized thickness fills the whole frame.
        let full = render_shield(4, 4, 4, SHIELD_ARGB);
        assert!(
            (0..full.len())
                .step_by(4)
                .all(|o| u32::from_ne_bytes(full[o..o + 4].try_into().unwrap()) == SHIELD_ARGB)
        );
    }

    /// Live check against the running compositor: show the shield, grab a
    /// composited frame through the B1 backend, assert the perimeter is shield
    /// red and the centre is untouched. Skips without a display / layer-shell;
    /// `#[ignore]` so a normal `cargo test` never paints over the user's screen.
    #[test]
    #[ignore = "paints a full-screen overlay; run explicitly on a wlroots desktop"]
    fn shield_is_visible_in_screencopy() {
        use super::super::capture_screencopy::start_screencopy_capture;
        use crate::streaming::{StreamingConfig, VideoFrame};
        use tokio::sync::watch;
        use tokio_util::sync::CancellationToken;

        let Some(_shield) = show() else {
            eprintln!("skip: no layer-shell / wayland display here");
            return;
        };
        // Give the compositor a beat to map + paint the layer.
        std::thread::sleep(Duration::from_millis(400));

        let (tx, mut rx) = crate::streaming::latest_frame::channel::<VideoFrame>();
        let cancel = CancellationToken::new();
        let (_wtx, wrx) = watch::channel(0u64);
        let window_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let session = match start_screencopy_capture(
            StreamingConfig::default(),
            tx,
            cancel.clone(),
            wrx,
            window_mode,
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip: no screencopy capture here: {e:#}");
                return;
            }
        };
        let frame = rx.blocking_recv().expect("no frame from screencopy");
        let px = |x: u32, y: u32| {
            let o = ((y * frame.width + x) as usize) * 4; // BGRA, stride == w*4
            (frame.data[o], frame.data[o + 1], frame.data[o + 2])
        };
        let (b, g, r) = px(2, frame.height / 2); // left ring
        assert!(
            r > 150 && g < 110 && b < 110,
            "expected shield red on the left edge, got B{b} G{g} R{r}"
        );
        let (cb, cg, cr) = px(frame.width / 2, frame.height / 2); // centre
        assert!(
            !(cr > 150 && cg < 110 && cb < 110),
            "centre should not be painted shield red (B{cb} G{cg} R{cr})"
        );
        drop(session);
    }
}
