//! The phone's laser dot on wlroots compositors (G4 on Linux, N10): a small
//! red dot on `zwlr_layer_shell_v1`'s overlay layer, as the privacy shield
//! is drawn ([`super::shield`]), which takes no input (an empty input
//! region, no keyboard) so it never stands in the pointer's way. It is
//! placed by its margins from the top-left of the output under it, so a
//! move is a margin and a commit, never a new picture.
//!
//! One [`Overlay`] lives while the dot shows: opened for a showing, it
//! reads the outputs then (by `xdg_output`'s logical box where offered, so
//! a scaled screen is measured as the compositor lays it out), and closing
//! it takes the dot away. Where there is no Wayland display or no layer
//! shell (GNOME, KDE, X11), [`available`] says so and nothing is drawn.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

/// The dot's colour (a presenter's red, as on Windows) and opacity.
const RGB: (u8, u8, u8) = (230, 30, 40);
const ALPHA: u8 = 230;

/// A screen in the compositor's layout, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

impl Screen {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.left + self.width && y >= self.top && y < self.top + self.height
    }
}

/// What an output said of itself.
#[derive(Debug, Default, Clone)]
struct Output {
    /// `wl_output`'s place, its current mode, scale and whether it is turned
    /// a quarter (its mode's sides then swap).
    at: (i32, i32),
    mode: (i32, i32),
    scale: i32,
    turned: bool,
    /// `xdg_output`'s logical box, when the compositor offers it.
    logical_at: Option<(i32, i32)>,
    logical_size: Option<(i32, i32)>,
}

impl Output {
    /// The output as laid out, or `None` before it said its size.
    fn screen(&self) -> Option<Screen> {
        let (left, top) = self.logical_at.unwrap_or(self.at);
        let (width, height) = self.logical_size.or_else(|| {
            let scale = self.scale.max(1);
            let (w, h) = (self.mode.0 / scale, self.mode.1 / scale);
            (w > 0 && h > 0).then_some(if self.turned { (h, w) } else { (w, h) })
        })?;
        Some(Screen {
            left,
            top,
            width,
            height,
        })
    }
}

#[derive(Default)]
struct State {
    outputs: Vec<Output>,
    /// The newest configure of the dot's surface, not yet acknowledged.
    configure: Option<u32>,
    /// The compositor took the dot's surface away.
    closed: bool,
}

/// The dot on one output, its picture kept for its life.
struct Shown {
    output: usize,
    size: u32,
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    _picture: (OwnedFd, WlShmPool, WlBuffer),
}

/// The dot's connection to the compositor while it shows.
pub struct Overlay {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    outputs: Vec<WlOutput>,
    shown: Option<Shown>,
}

/// Whether this computer can draw the dot: a Wayland compositor with the
/// layer shell.
pub fn available() -> bool {
    let Ok(conn) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _queue)) = registry_queue_init::<State>(&conn) else {
        return false;
    };
    has(&globals, "zwlr_layer_shell_v1") && has(&globals, "wl_shm")
}

fn has(globals: &GlobalList, interface: &str) -> bool {
    globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == interface))
}

impl Overlay {
    /// Connect and read the outputs; `None` where the dot cannot be drawn.
    pub fn open() -> Option<Self> {
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).ok()?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).ok()?;
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        let manager: Option<ZxdgOutputManagerV1> = globals.bind(&qh, 1..=3, ()).ok();
        let mut state = State::default();
        let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| (g.name, g.version))
                .collect()
        });
        let mut outputs = Vec::new();
        for (index, (name, version)) in found.into_iter().enumerate() {
            let output: WlOutput = globals.registry().bind(name, version.min(4), &qh, index);
            if let Some(manager) = &manager {
                manager.get_xdg_output(&output, &qh, index);
            }
            state.outputs.push(Output::default());
            outputs.push(output);
        }
        // Two round trips: the outputs' events, then xdg_output's done.
        queue.roundtrip(&mut state).ok()?;
        queue.roundtrip(&mut state).ok()?;
        Some(Overlay {
            conn,
            queue,
            state,
            compositor,
            shm,
            layer_shell,
            outputs,
            shown: None,
        })
    }

    /// The outputs as laid out, in the compositor's order.
    pub fn screens(&self) -> Vec<Screen> {
        self.state
            .outputs
            .iter()
            .filter_map(Output::screen)
            .collect()
    }

    /// Show the dot, `size` logical pixels wide, centred on `(x, y)` of the
    /// layout. False when the compositor would not take it (the caller
    /// closes the overlay and opens a new one at the next move).
    pub fn show_at(&mut self, x: i32, y: i32, size: u32) -> bool {
        self.read();
        if self.state.closed {
            return false;
        }
        let screens: Vec<(usize, Screen)> = self
            .state
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.screen().map(|s| (i, s)))
            .collect();
        let Some(&(output, screen)) = screens
            .iter()
            .find(|(_, s)| s.contains(x, y))
            .or_else(|| screens.first())
        else {
            return false;
        };
        let (left, top) = margins(screen, x, y, size);
        let fits = self
            .shown
            .as_ref()
            .is_some_and(|s| s.output == output && s.size == size);
        if fits {
            let shown = self.shown.as_ref().expect("checked above");
            if let Some(serial) = self.state.configure.take() {
                shown.layer.ack_configure(serial);
            }
            shown.layer.set_margin(top, 0, 0, left);
            shown.surface.commit();
            return self.conn.flush().is_ok();
        }
        self.take_down();
        match self.put_up(output, size, left, top) {
            Some(shown) => {
                self.shown = Some(shown);
                true
            }
            None => false,
        }
    }

    /// Read what the compositor sent, without waiting.
    fn read(&mut self) {
        let _ = self.conn.flush();
        if let Some(guard) = self.queue.prepare_read() {
            let _ = guard.read();
        }
        let _ = self.queue.dispatch_pending(&mut self.state);
    }

    fn take_down(&mut self) {
        if let Some(shown) = self.shown.take() {
            shown.layer.destroy();
            shown.surface.destroy();
            let _ = self.conn.flush();
        }
        self.state.configure = None;
    }

    /// A surface for the dot on `output` with its first picture, placed by
    /// its margins.
    fn put_up(&mut self, output: usize, size: u32, left: i32, top: i32) -> Option<Shown> {
        let qh = self.queue.handle();
        let surface = self.compositor.create_surface(&qh, ());
        // An empty input region: every press goes to what is under the dot.
        let region: WlRegion = self.compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            self.outputs.get(output),
            Layer::Overlay,
            "pocketdesk-laser".to_string(),
            &qh,
            (),
        );
        layer.set_anchor(Anchor::Top | Anchor::Left);
        layer.set_size(size, size);
        // Placed from the output's own edge, not pushed by a panel's zone.
        layer.set_exclusive_zone(-1);
        layer.set_margin(top, 0, 0, left);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        surface.commit();
        self.state.configure = None;
        let serial = loop {
            self.queue.roundtrip(&mut self.state).ok()?;
            if self.state.closed {
                return None;
            }
            if let Some(serial) = self.state.configure.take() {
                break serial;
            }
        };
        let picture = picture(&self.shm, &qh, size)?;
        layer.ack_configure(serial);
        surface.attach(Some(&picture.2), 0, 0);
        surface.damage_buffer(0, 0, size as i32, size as i32);
        surface.commit();
        self.conn.flush().ok()?;
        Some(Shown {
            output,
            size,
            surface,
            layer,
            _picture: picture,
        })
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // Its surface goes now; the connection's close takes the rest.
        self.take_down();
    }
}

/// The dot's margins from the top-left of `screen`, centred on `(x, y)`
/// and kept on it.
fn margins(screen: Screen, x: i32, y: i32, size: u32) -> (i32, i32) {
    let half = size as i32 / 2;
    let most = |side: i32| (side - size as i32).max(0);
    (
        (x - screen.left - half).clamp(0, most(screen.width)),
        (y - screen.top - half).clamp(0, most(screen.height)),
    )
}

/// The dot's picture, `size` pixels a side: a round dot, premultiplied
/// ARGB8888 in the compositor's byte order, its edge softened by a pixel.
pub fn render_dot(size: u32) -> Vec<u8> {
    let mut picture = vec![0u8; (size as usize) * (size as usize) * 4];
    let radius = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - radius;
            let dy = y as f32 + 0.5 - radius;
            let cover = (radius - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            let alpha = (f32::from(ALPHA) * cover) as u32;
            let premultiplied = |c: u8| u32::from(c) * alpha / 255;
            let argb = alpha << 24
                | premultiplied(RGB.0) << 16
                | premultiplied(RGB.1) << 8
                | premultiplied(RGB.2);
            let at = ((y * size + x) as usize) * 4;
            picture[at..at + 4].copy_from_slice(&argb.to_le_bytes());
        }
    }
    picture
}

/// The dot's picture in shared memory, kept for the surface's life.
fn picture(
    shm: &WlShm,
    qh: &QueueHandle<State>,
    size: u32,
) -> Option<(OwnedFd, WlShmPool, WlBuffer)> {
    let pixels = render_dot(size);
    let name = std::ffi::CString::new("pocketdesk-laser").ok()?;
    // SAFETY: a valid C string; the descriptor is checked below.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    // SAFETY: a fresh descriptor from memfd_create, owned from here.
    let mut file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    file.write_all(&pixels).ok()?;
    let fd = OwnedFd::from(file);
    let pool = shm.create_pool(fd.as_fd(), pixels.len() as i32, qh, ());
    let side = size as i32;
    let buffer = pool.create_buffer(0, side, side, side * 4, wl_shm::Format::Argb8888, qh, ());
    Some((fd, pool, buffer))
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
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

impl Dispatch<WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: wl_output::Event,
        index: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                output.at = (x, y);
                output.turned = matches!(
                    transform,
                    WEnum::Value(
                        wl_output::Transform::_90
                            | wl_output::Transform::_270
                            | wl_output::Transform::Flipped90
                            | wl_output::Transform::Flipped270
                    )
                );
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if let WEnum::Value(flags) = flags
                    && flags.contains(wl_output::Mode::Current)
                {
                    output.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => output.scale = factor,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.logical_at = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.logical_size = (width > 0 && height > 0).then_some((width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                state.configure = Some(serial);
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlRegion);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore ZxdgOutputManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Screen = Screen {
        left: 1920,
        top: 0,
        width: 1280,
        height: 1024,
    };

    #[test]
    fn the_dot_is_centred_and_kept_on_its_screen() {
        assert_eq!(margins(SCREEN, 2560, 512, 24), (628, 500));
        // At the screen's corners it stays whole on it.
        assert_eq!(margins(SCREEN, 1920, 0, 24), (0, 0));
        assert_eq!(margins(SCREEN, 3199, 1023, 24), (1256, 1000));
    }

    #[test]
    fn an_output_is_measured_as_laid_out() {
        let scaled = Output {
            at: (0, 0),
            mode: (3840, 2160),
            scale: 2,
            ..Default::default()
        };
        assert_eq!(
            scaled.screen(),
            Some(Screen {
                left: 0,
                top: 0,
                width: 1920,
                height: 1080
            })
        );
        // A turned one swaps its sides; xdg_output's box, where said, wins.
        let turned = Output {
            mode: (1920, 1080),
            scale: 1,
            turned: true,
            ..Default::default()
        };
        assert_eq!(
            turned.screen().map(|s| (s.width, s.height)),
            Some((1080, 1920))
        );
        let fractional = Output {
            mode: (2560, 1600),
            scale: 2,
            logical_at: Some((1920, 0)),
            logical_size: Some((1706, 1066)),
            ..Default::default()
        };
        assert_eq!(
            fractional.screen(),
            Some(Screen {
                left: 1920,
                top: 0,
                width: 1706,
                height: 1066
            })
        );
        assert_eq!(Output::default().screen(), None, "no size said yet");
    }

    #[test]
    fn the_dot_is_round_and_premultiplied() {
        let picture = render_dot(24);
        let px = |x: u32, y: u32| {
            let at = ((y * 24 + x) as usize) * 4;
            u32::from_le_bytes([
                picture[at],
                picture[at + 1],
                picture[at + 2],
                picture[at + 3],
            ])
        };
        assert_eq!(px(0, 0) >> 24, 0, "a corner is clear");
        let middle = px(12, 12);
        assert_eq!(middle >> 24, u32::from(ALPHA));
        // Premultiplied: no channel above its alpha.
        assert!((middle >> 16 & 0xff) <= middle >> 24);
    }
}
