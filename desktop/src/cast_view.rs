//! The phone's screen on the computer (T10k): the window that shows a cast.
//!
//! A worker thread watches `cast.json` (written by the server's cast relay),
//! connects to the relay with its token, and feeds each access unit to an
//! `ffmpeg` sidecar that decodes it to RGBA; the newest picture goes to a
//! texture in a window of its own, at the phone's shape. Closing the window
//! stops showing that cast (the phone keeps casting until it stops);
//! the Sessions tab can show it again.

use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use pocketdesk_core::local::cast::{self as wire, CastFile, Message};

/// The window's id among the app's viewports.
fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pocketdesk-cast")
}

/// How often the worker looks for a cast.
const POLL: Duration = Duration::from_millis(500);
/// The relay sends a heartbeat every 2 s; silence past this is a dead relay.
const READ_TIMEOUT: Duration = Duration::from_secs(6);
/// The window's long side when it opens, in points.
const LONG_SIDE: f32 = 800.0;
/// An access unit delimiter: ffmpeg's parser emits a frame once the next
/// one begins, and this begins one at once.
const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0xF0];

/// One decoded picture.
struct Picture {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

#[derive(Default)]
struct State {
    /// The cast being shown, and its phone.
    session_id: Option<u64>,
    device_id: String,
    /// The phone's screen as it is encoded now.
    size: Option<(u16, u16)>,
    /// The size the window was last made for.
    sized_for: Option<(u16, u16)>,
    /// The newest picture, until the window takes it.
    picture: Option<Picture>,
    texture: Option<egui::TextureHandle>,
    /// What stops it showing, said in the window.
    note: Option<String>,
    /// The cast the owner closed the window on.
    dismissed: Option<u64>,
    /// The relay connection, to cut when the window closes.
    stream: Option<TcpStream>,
}

/// The cast window and its worker.
pub struct CastView {
    shared: Arc<Mutex<State>>,
}

impl CastView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(State::default()));
        let worker = shared.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("cast-view".into())
            .spawn(move || watch(&state_dir, &worker, &ctx));
        // Without the thread no cast is ever shown, and nothing else breaks.
        drop(spawned);
        Self { shared }
    }

    /// The cast now, and whether its window was closed.
    pub fn showing(&self) -> Option<(u64, bool)> {
        let state = lock(&self.shared);
        state.session_id.map(|id| (id, state.dismissed == Some(id)))
    }

    /// Show a cast whose window was closed again.
    pub fn reopen(&self) {
        lock(&self.shared).dismissed = None;
    }

    /// Call each pass: keeps the window open while there is a cast to show.
    pub fn show(&self, ctx: &egui::Context, devices: &[crate::model::Device]) {
        let (title, size) = {
            let state = lock(&self.shared);
            let Some(id) = state.session_id else { return };
            if state.dismissed == Some(id) {
                return;
            }
            (
                // The phone's name (U3), else its id.
                format!(
                    "{} · phone screen",
                    devices
                        .iter()
                        .find(|d| d.id == state.device_id)
                        .and_then(|d| d.name.clone())
                        .unwrap_or_else(|| short(&state.device_id))
                ),
                state.size,
            )
        };
        let (w, h) = window_size(size);
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([w, h])
            .with_min_inner_size([160.0, 160.0]);
        let shared = self.shared.clone();
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            draw(&shared, ui);
        });
    }
}

fn lock(shared: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn short(device_id: &str) -> String {
    if device_id.is_empty() {
        "Phone".into()
    } else if device_id.chars().count() > 12 {
        format!("{}…", device_id.chars().take(12).collect::<String>())
    } else {
        device_id.into()
    }
}

/// The window at the phone's shape, its long side [`LONG_SIDE`].
fn window_size(size: Option<(u16, u16)>) -> (f32, f32) {
    let (w, h) = size.map_or((9.0, 16.0), |(w, h)| {
        (f32::from(w.max(1)), f32::from(h.max(1)))
    });
    if h >= w {
        (LONG_SIDE * w / h, LONG_SIDE)
    } else {
        (LONG_SIDE, LONG_SIDE * h / w)
    }
}

/// Where a `w`×`h` picture goes in `area`: as large as fits, centred.
fn letterbox(area: egui::Rect, w: usize, h: usize) -> egui::Rect {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (area.width() / w).min(area.height() / h);
    egui::Rect::from_center_size(area.center(), egui::vec2(w * scale, h * scale))
}

fn draw(shared: &Mutex<State>, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut state = lock(shared);
    if ctx.input(|i| i.viewport().close_requested()) {
        state.dismissed = state.session_id;
        if let Some(stream) = state.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        return;
    }
    // A turned phone: the window turns with it.
    if state.size.is_some() && state.size != state.sized_for {
        if state.sized_for.is_some() {
            let (w, h) = window_size(state.size);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
        }
        state.sized_for = state.size;
    }
    if let Some(picture) = state.picture.take() {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [picture.width, picture.height],
            &picture.rgba,
        );
        match &mut state.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                state.texture = Some(ctx.load_texture("cast", image, egui::TextureOptions::LINEAR));
            }
        }
    }
    let texture = state.texture.clone();
    let note = state.note.clone();
    drop(state);
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            let area = ui.max_rect();
            match (&texture, note) {
                (_, Some(note)) => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::LIGHT_GRAY, note)
                    });
                }
                (Some(texture), None) => {
                    let [w, h] = texture.size();
                    ui.painter().image(
                        texture.id(),
                        letterbox(area, w, h),
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                (None, None) => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            egui::Color32::LIGHT_GRAY,
                            "Waiting for the phone's screen…",
                        )
                    });
                }
            }
        });
}

/// The worker: find the cast, show it until it ends, again.
fn watch(state_dir: &std::path::Path, shared: &Arc<Mutex<State>>, ctx: &egui::Context) {
    loop {
        std::thread::sleep(POLL);
        let Some(file) = wire::read(state_dir) else {
            forget(shared, ctx);
            continue;
        };
        let dismissed = {
            let mut state = lock(shared);
            if state.session_id != Some(file.session_id) {
                *state = State {
                    session_id: Some(file.session_id),
                    device_id: file.device_id.clone(),
                    dismissed: state.dismissed,
                    ..State::default()
                };
                ctx.request_repaint();
            }
            state.dismissed == Some(file.session_id)
        };
        if dismissed {
            continue;
        }
        // An error is the relay gone or the window closed: the file says
        // which on the next look.
        let _ = view(&file, shared, ctx);
        let mut state = lock(shared);
        state.stream = None;
        state.size = None;
        state.texture = None;
        state.picture = None;
    }
}

/// No cast: no window.
fn forget(shared: &Mutex<State>, ctx: &egui::Context) {
    let mut state = lock(shared);
    if state.session_id.is_some() {
        let dismissed = state.dismissed;
        *state = State {
            dismissed,
            ..State::default()
        };
        ctx.request_repaint();
    }
}

/// Show one cast until its end.
fn view(file: &CastFile, shared: &Arc<Mutex<State>>, ctx: &egui::Context) -> io::Result<()> {
    let token = file
        .token_bytes()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a bad token in cast.json"))?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, file.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.write_all(&token)?;
    lock(shared).stream = Some(stream.try_clone()?);
    let mut asks = stream.try_clone()?;
    let mut reader = BufReader::with_capacity(256 * 1024, stream);

    let mut decoder: Option<Decoder> = None;
    let mut gate = KeyGate::default();
    loop {
        match Message::read(&mut reader)? {
            Message::Config { width, height } => {
                decoder = None;
                gate = KeyGate::default();
                let started = Decoder::start(width, height, publisher(shared, ctx));
                let mut state = lock(shared);
                state.size = Some((width, height));
                match started {
                    Ok(d) => {
                        decoder = Some(d);
                        state.note = None;
                    }
                    Err(e) => {
                        state.note = Some(format!(
                            "Showing a phone's screen needs ffmpeg on this computer ({e})."
                        ));
                    }
                }
                ctx.request_repaint_of(viewport_id());
            }
            Message::Frame {
                keyframe,
                seq,
                data,
            } => {
                let Some(d) = decoder.as_mut() else { continue };
                match gate.admit(keyframe, seq) {
                    Admit::Feed => {}
                    Admit::Skip => continue,
                    Admit::SkipAndAsk => {
                        Message::WantKeyframe.write(&mut asks)?;
                        continue;
                    }
                }
                if let Err(e) = d.feed(&data) {
                    decoder = None;
                    lock(shared).note = Some(format!("The decoder stopped ({e})."));
                    ctx.request_repaint_of(viewport_id());
                }
            }
            Message::Heartbeat | Message::WantKeyframe => {}
            Message::End => return Ok(()),
        }
    }
}

/// Where a decoded picture goes: the window's next pass takes it.
fn publisher(shared: &Arc<Mutex<State>>, ctx: &egui::Context) -> Box<dyn FnMut(Picture) + Send> {
    let shared = shared.clone();
    let ctx = ctx.clone();
    Box::new(move |picture| {
        lock(&shared).picture = Some(picture);
        ctx.request_repaint_of(viewport_id());
    })
}

/// What to do with a frame after a decoder (re)started, or after a gap.
#[derive(Debug, PartialEq, Eq)]
enum Admit {
    Feed,
    Skip,
    /// Skip it, and ask the phone for a keyframe (once).
    SkipAndAsk,
}

/// A decoder starts from a keyframe: deltas before one would only draw
/// garbage. So does one after a gap in the sequence (a frame dropped on
/// the way, by the phone or the relay), until the next keyframe.
#[derive(Debug, Default)]
struct KeyGate {
    fed: bool,
    asked: bool,
    last_seq: Option<u64>,
}

impl KeyGate {
    fn admit(&mut self, keyframe: bool, seq: u64) -> Admit {
        let gap = self
            .last_seq
            .is_some_and(|last| seq > last.saturating_add(1));
        self.last_seq = Some(seq);
        if gap && !keyframe {
            self.fed = false;
            self.asked = false;
        }
        if keyframe || self.fed {
            self.fed = true;
            return Admit::Feed;
        }
        if self.asked {
            Admit::Skip
        } else {
            self.asked = true;
            Admit::SkipAndAsk
        }
    }
}

/// The `ffmpeg` sidecar that decodes one encoder's stream.
struct Decoder {
    child: Child,
    stdin: ChildStdin,
}

impl Decoder {
    fn start(width: u16, height: u16, mut out: Box<dyn FnMut(Picture) + Send>) -> io::Result<Self> {
        let (w, h) = (usize::from(width), usize::from(height));
        if w == 0 || h == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an empty screen",
            ));
        }
        let mut command = Command::new("ffmpeg");
        // Each flag here removes a frame of delay ffmpeg would otherwise add
        // (measured with ffmpeg 8): `nobuffer` is left out, as it drops the
        // packets read while probing (the keyframe), and the rawvideo
        // encoder holds a frame unless it runs on one thread.
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-flags",
                "low_delay",
                "-probesize",
                "32",
                "-analyzeduration",
                "0",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                "-vf",
            ])
            // Whatever the stream's crop, frames come out the size read.
            .arg(format!("scale={w}:{h}"))
            .args([
                "-vsync",
                "passthrough",
                "-threads",
                "1",
                // A broken stretch is waited out, not given up on.
                "-max_error_rate",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        std::thread::Builder::new()
            .name("cast-decode".into())
            .spawn(move || {
                loop {
                    let mut rgba = vec![0u8; w * h * 4];
                    if stdout.read_exact(&mut rgba).is_err() {
                        break;
                    }
                    out(Picture {
                        width: w,
                        height: h,
                        rgba,
                    });
                }
            })?;
        Ok(Self { child, stdin })
    }

    /// One access unit, decoded at once.
    fn feed(&mut self, access_unit: &[u8]) -> io::Result<()> {
        self.stdin.write_all(access_unit)?;
        self.stdin.write_all(&AUD)?;
        self.stdin.flush()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_takes_the_phones_shape() {
        assert_eq!(window_size(Some((900, 1600))), (450.0, 800.0));
        assert_eq!(window_size(Some((1600, 900))), (800.0, 450.0));
        assert_eq!(window_size(None), (450.0, 800.0), "a phone, until it says");
    }

    #[test]
    fn a_picture_is_letterboxed_in_the_window() {
        let area = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 400.0));
        let placed = letterbox(area, 100, 200);
        assert_eq!(placed.size(), egui::vec2(200.0, 400.0));
        assert_eq!(placed.center(), area.center());
    }

    #[test]
    fn a_decoder_starts_from_a_keyframe_and_asks_for_one_once() {
        let mut gate = KeyGate::default();
        assert_eq!(gate.admit(false, 3), Admit::SkipAndAsk);
        assert_eq!(gate.admit(false, 4), Admit::Skip);
        assert_eq!(gate.admit(true, 5), Admit::Feed);
        assert_eq!(gate.admit(false, 6), Admit::Feed, "deltas after it are fed");
    }

    #[test]
    fn a_gap_in_the_frames_waits_for_the_next_keyframe() {
        let mut gate = KeyGate::default();
        assert_eq!(gate.admit(true, 0), Admit::Feed);
        assert_eq!(gate.admit(false, 1), Admit::Feed);
        assert_eq!(
            gate.admit(false, 3),
            Admit::SkipAndAsk,
            "frame 2 was dropped"
        );
        assert_eq!(gate.admit(false, 4), Admit::Skip);
        assert_eq!(
            gate.admit(true, 9),
            Admit::Feed,
            "a keyframe after a gap is fed"
        );
        assert_eq!(gate.admit(false, 10), Admit::Feed);
    }

    #[test]
    fn a_long_phone_id_is_cut_in_the_title() {
        assert_eq!(short(""), "Phone");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short("0123456789abcdef"), "0123456789ab…");
    }

    /// The sidecar turns an H.264 stream into pictures of the size read.
    /// Skipped where ffmpeg is not installed.
    #[test]
    fn the_sidecar_decodes_what_ffmpeg_encodes() {
        let encoded = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x128:rate=10",
                "-frames:v",
                "3",
                "-c:v",
                "libx264",
                "-bf",
                "0",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stderr(Stdio::null())
            .output();
        let Ok(encoded) = encoded else {
            eprintln!("no ffmpeg here: skipped");
            return;
        };
        if !encoded.status.success() || encoded.stdout.is_empty() {
            eprintln!("this ffmpeg cannot encode H.264: skipped");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut decoder = Decoder::start(
            64,
            128,
            Box::new(move |p| {
                let _ = tx.send((p.width, p.height, p.rgba.len()));
            }),
        )
        .expect("ffmpeg starts");
        decoder.feed(&encoded.stdout).expect("feed");
        for _ in 0..3 {
            let got = rx.recv_timeout(Duration::from_secs(10)).expect("a picture");
            assert_eq!(got, (64, 128, 64 * 128 * 4));
        }
    }
}
