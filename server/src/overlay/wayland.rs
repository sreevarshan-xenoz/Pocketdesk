//! The badge on Linux. On a compositor with the layer shell (Hyprland,
//! Sway, KDE Plasma): a surface on the overlay layer of each output,
//! centred, with an empty input region and no keyboard, so every click and
//! key goes to what is under it; its words drawn with `ab_glyph` from the
//! font `fc-match sans` names (else DejaVu Sans or Noto Sans where they
//! usually are). With no layer shell (GNOME, X11) or no font, a desktop
//! notification says it instead. One thread of its own puts it up and takes
//! it down when its time is up or a newer badge replaces it.

use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont, point};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

/// The badge's sizes at scale 1, in logical pixels.
const FONT_PX: f32 = 26.0;
const LINE_GAP: f32 = 6.0;
const PAD: u32 = 18;
const CORNER: f32 = 16.0;

/// The plate's colour and opacity (the words are white).
const PLATE: (u8, u8, u8, u8) = (0x20, 0x20, 0x20, 0xE6);

/// The largest picture side drawn, in pixels.
const MOST_SIDE: u32 = 8192;

/// How many round trips the surfaces wait for their first configure.
const CONFIGURE_TRIES: usize = 10;

/// Where a sans font usually is when `fc-match` is not there.
const FALLBACK_FONTS: [&str; 6] = [
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
];

/// A badge to show: its lines and until when.
struct Flash {
    lines: Vec<String>,
    until: Instant,
}

static SENDER: OnceLock<Mutex<Option<Sender<Flash>>>> = OnceLock::new();

/// The badge thread, started at the first flash; `None` when it could not
/// start.
fn sender() -> Option<Sender<Flash>> {
    let slot = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("badge".into())
            .spawn(move || run(rx))
            .is_ok();
        Mutex::new(started.then_some(tx))
    });
    slot.lock().ok()?.clone()
}

/// Show `lines` on every output for `how_long`, in place of what shows.
pub fn flash(lines: Vec<String>, how_long: Duration) {
    if let Some(tx) = sender() {
        let _ = tx.send(Flash {
            lines,
            until: Instant::now() + how_long,
        });
    }
}

/// The badge's thread: each flash replaces what shows; what shows goes
/// when its time is up.
fn run(flashes: Receiver<Flash>) {
    let mut shown: Option<Badge> = None;
    let mut until = Instant::now();
    loop {
        let next = if shown.is_none() {
            match flashes.recv() {
                Ok(flash) => flash,
                Err(_) => return,
            }
        } else {
            match flashes.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(flash) => flash,
                Err(RecvTimeoutError::Timeout) => {
                    shown = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        // Only the newest of those waiting is shown, the old one gone first.
        let mut flash = next;
        while let Ok(newer) = flashes.try_recv() {
            flash = newer;
        }
        shown = None;
        match Badge::put_up(&flash.lines) {
            Some(badge) => {
                shown = Some(badge);
                until = flash.until;
            }
            None => {
                let (summary, body) = flash.lines.split_first().map_or_else(
                    || (String::new(), String::new()),
                    |(first, rest)| (first.clone(), rest.join(" ")),
                );
                let left = flash.until.saturating_duration_since(Instant::now());
                crate::platform::notify(&summary, &body, left);
            }
        }
    }
}

/// The font the badge is drawn in, found once; `None` when there is none
/// that reads.
fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        let matched = Command::new("fc-match")
            .args(["-f", "%{file}", "sans"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|path| PathBuf::from(path.trim()));
        matched
            .into_iter()
            .chain(FALLBACK_FONTS.iter().map(PathBuf::from))
            .filter(|path| path.is_file())
            .find_map(|path| FontVec::try_from_vec(std::fs::read(path).ok()?).ok())
    })
    .as_ref()
}

/// A badge's picture: premultiplied ARGB8888 in the compositor's byte
/// order, `width` x `height` pixels (a whole multiple of its scale).
struct Picture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// `lines` drawn white, centred, on a dark plate with rounded corners, at
/// `scale` pixels to the logical pixel; `None` for a picture too big.
fn render(font: &FontVec, lines: &[String], scale: u32) -> Option<Picture> {
    let s = scale.max(1) as f32;
    let px = PxScale::from(FONT_PX * s);
    let scaled = font.as_scaled(px);
    let line_width = |line: &str| {
        let mut width = 0.0;
        let mut last: Option<GlyphId> = None;
        for c in line.chars() {
            let id = scaled.glyph_id(c);
            if let Some(prev) = last {
                width += scaled.kern(prev, id);
            }
            width += scaled.h_advance(id);
            last = Some(id);
        }
        width
    };
    let widths: Vec<f32> = lines.iter().map(|l| line_width(l)).collect();
    let text_w = widths.iter().copied().fold(0.0, f32::max);
    let gaps = lines.len().saturating_sub(1) as f32;
    let text_h = scaled.height() * lines.len() as f32 + LINE_GAP * s * gaps;
    // The logical size first, so the picture is a whole multiple of it.
    let logical_w = (text_w / s).ceil() as u32 + 2 * PAD;
    let logical_h = (text_h / s).ceil() as u32 + 2 * PAD;
    let (width, height) = (logical_w * scale.max(1), logical_h * scale.max(1));
    if width > MOST_SIDE || height > MOST_SIDE {
        return None;
    }
    let mut cover = vec![0f32; (width as usize) * (height as usize)];
    for (i, line) in lines.iter().enumerate() {
        let mut caret = (width as f32 - widths[i]) / 2.0;
        let baseline =
            (PAD as f32) * s + scaled.ascent() + i as f32 * (scaled.height() + LINE_GAP * s);
        let mut last: Option<GlyphId> = None;
        for c in line.chars() {
            let id = scaled.glyph_id(c);
            if let Some(prev) = last {
                caret += scaled.kern(prev, id);
            }
            let glyph = id.with_scale_and_position(px, point(caret, baseline));
            caret += scaled.h_advance(id);
            last = Some(id);
            let Some(outlined) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, c| {
                let x = bounds.min.x as i64 + i64::from(gx);
                let y = bounds.min.y as i64 + i64::from(gy);
                if (0..i64::from(width)).contains(&x) && (0..i64::from(height)).contains(&y) {
                    let at = (y as usize) * (width as usize) + x as usize;
                    cover[at] = (cover[at] + c).min(1.0);
                }
            });
        }
    }
    let radius = (CORNER * s)
        .min(width as f32 / 2.0)
        .min(height as f32 / 2.0);
    let mut pixels = vec![0u8; cover.len() * 4];
    for y in 0..height {
        for x in 0..width {
            let at = (y as usize) * (width as usize) + x as usize;
            let plate = f32::from(PLATE.3) / 255.0 * plate_cover(x, y, width, height, radius);
            let text = cover[at];
            // White words over the plate, premultiplied.
            let alpha = text + plate * (1.0 - text);
            let channel =
                |c: u8| (255.0 * text + f32::from(c) * plate * (1.0 - text)).round() as u32;
            let argb = ((alpha * 255.0).round() as u32) << 24
                | channel(PLATE.0) << 16
                | channel(PLATE.1) << 8
                | channel(PLATE.2);
            pixels[at * 4..at * 4 + 4].copy_from_slice(&argb.to_le_bytes());
        }
    }
    Some(Picture {
        width,
        height,
        pixels,
    })
}

/// How much of the pixel at `(x, y)` a `width` x `height` plate with
/// corners of `radius` covers, its edge softened by a pixel.
fn plate_cover(x: u32, y: u32, width: u32, height: u32, radius: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let cx = px.clamp(radius, width as f32 - radius);
    let cy = py.clamp(radius, height as f32 - radius);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (radius - d + 0.5).clamp(0.0, 1.0)
}

#[derive(Default)]
struct State {
    /// Each output's scale, as it said.
    scales: Vec<i32>,
    /// Each surface's newest configure, and whether it was taken away.
    configured: Vec<Option<u32>>,
    closed: Vec<bool>,
}

/// A badge up on the outputs, taken down when dropped.
struct Badge {
    conn: Connection,
    queue: EventQueue<State>,
    surfaces: Vec<(WlSurface, ZwlrLayerSurfaceV1)>,
    pictures: Vec<(OwnedFd, WlShmPool, WlBuffer)>,
}

impl Badge {
    /// `lines` on every output; `None` where it cannot be drawn (no
    /// Wayland, no layer shell, no font, no output that took it).
    fn put_up(lines: &[String]) -> Option<Badge> {
        let font = font()?;
        let conn = Connection::connect_to_env().ok()?;
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).ok()?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).ok()?;
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| (g.name, g.version))
                .collect()
        });
        let mut state = State::default();
        let mut outputs = Vec::new();
        for (index, (name, version)) in found.into_iter().enumerate() {
            let output: WlOutput = globals.registry().bind(name, version.min(4), &qh, index);
            outputs.push(output);
            state.scales.push(1);
        }
        let mut badge = Badge {
            conn,
            queue,
            surfaces: Vec::new(),
            pictures: Vec::new(),
        };
        // The outputs say their scales.
        badge.queue.roundtrip(&mut state).ok()?;
        let mut drawn = Vec::new();
        for (index, output) in outputs.iter().enumerate() {
            let scale = state.scales.get(index).copied().unwrap_or(1).clamp(1, 4);
            let Some(picture) = render(font, lines, scale as u32) else {
                continue;
            };
            let surface = compositor.create_surface(&qh, ());
            // An empty input region: every click goes to what is under it.
            let region: WlRegion = compositor.create_region(&qh, ());
            surface.set_input_region(Some(&region));
            region.destroy();
            if compositor.version() >= 3 {
                surface.set_buffer_scale(scale);
            }
            let surface_index = badge.surfaces.len();
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(output),
                Layer::Overlay,
                "pocketdesk-badge".to_string(),
                &qh,
                surface_index,
            );
            // No anchor: centred on its output.
            layer.set_size(picture.width / scale as u32, picture.height / scale as u32);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            surface.commit();
            state.configured.push(None);
            state.closed.push(false);
            badge.surfaces.push((surface, layer));
            drawn.push(picture);
        }
        if badge.surfaces.is_empty() {
            return None;
        }
        // A compositor that neither configures nor closes them is let go.
        for _ in 0..CONFIGURE_TRIES {
            badge.queue.roundtrip(&mut state).ok()?;
            let settled = state
                .configured
                .iter()
                .zip(&state.closed)
                .all(|(serial, closed)| serial.is_some() || *closed);
            if settled {
                break;
            }
        }
        for (index, picture) in drawn.iter().enumerate() {
            let (Some(serial), false) = (state.configured[index], state.closed[index]) else {
                continue;
            };
            let Some(buffer) = buffer(&shm, &qh, picture) else {
                continue;
            };
            let (surface, layer) = &badge.surfaces[index];
            layer.ack_configure(serial);
            surface.attach(Some(&buffer.2), 0, 0);
            surface.damage_buffer(0, 0, picture.width as i32, picture.height as i32);
            surface.commit();
            badge.pictures.push(buffer);
        }
        badge.conn.flush().ok()?;
        (!badge.pictures.is_empty()).then_some(badge)
    }
}

impl Drop for Badge {
    fn drop(&mut self) {
        for (surface, layer) in self.surfaces.drain(..) {
            layer.destroy();
            surface.destroy();
        }
        // A proxy is not destroyed when dropped.
        for (_, pool, buffer) in self.pictures.drain(..) {
            buffer.destroy();
            pool.destroy();
        }
        let _ = self.conn.flush();
    }
}

/// `picture` in shared memory, kept for its surface's life.
fn buffer(
    shm: &WlShm,
    qh: &QueueHandle<State>,
    picture: &Picture,
) -> Option<(OwnedFd, WlShmPool, WlBuffer)> {
    let name = std::ffi::CString::new("pocketdesk-badge").ok()?;
    // SAFETY: a valid C string; the descriptor is checked below.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return None;
    }
    // SAFETY: a fresh descriptor from memfd_create, owned from here.
    let mut file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    file.write_all(&picture.pixels).ok()?;
    let fd = OwnedFd::from(file);
    let pool = shm.create_pool(fd.as_fd(), picture.pixels.len() as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        picture.width as i32,
        picture.height as i32,
        picture.width as i32 * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
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
        if let wl_output::Event::Scale { factor } = event
            && let Some(scale) = state.scales.get_mut(*index)
        {
            *scale = factor;
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        index: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                if let Some(configured) = state.configured.get_mut(*index) {
                    *configured = Some(serial);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(closed) = state.closed.get_mut(*index) {
                    *closed = true;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plate_has_soft_round_corners() {
        // The very corner is clear, the middle and an edge's middle full.
        assert_eq!(plate_cover(0, 0, 200, 80, 16.0), 0.0);
        assert_eq!(plate_cover(100, 40, 200, 80, 16.0), 1.0);
        assert_eq!(plate_cover(100, 0, 200, 80, 16.0), 1.0);
    }

    /// Drawn with the font this computer has (none on a bare CI image:
    /// then there is nothing to draw with, and the badge is a
    /// notification).
    #[test]
    fn a_badge_picture_has_its_plate_and_words() {
        let Some(font) = font() else {
            return;
        };
        let lines = vec!["Keyboard and mouse: Laptop".to_string()];
        let one = render(font, &lines, 1).expect("a picture");
        let two = render(font, &lines, 2).expect("a picture");
        assert_eq!((two.width % 2, two.height % 2), (0, 0));
        assert_eq!(one.pixels.len(), (one.width * one.height * 4) as usize);
        let px = |p: &Picture, x: u32, y: u32| {
            let at = ((y * p.width + x) * 4) as usize;
            u32::from_le_bytes([
                p.pixels[at],
                p.pixels[at + 1],
                p.pixels[at + 2],
                p.pixels[at + 3],
            ])
        };
        assert_eq!(px(&one, 0, 0) >> 24, 0, "a corner is clear");
        // Some of the middle row is white words.
        let row = one.height / 2;
        assert!((0..one.width).any(|x| px(&one, x, row) & 0xff > 0xc0));
        // Premultiplied: no channel above its alpha.
        assert!(
            (0..one.width)
                .map(|x| px(&one, x, row))
                .all(|p| (p >> 16 & 0xff) <= p >> 24)
        );
    }
}
