//! The phone's screen on the computer (T10k): the window that shows a cast.
//!
//! A worker thread watches `cast.json` (written by the server's cast relay),
//! connects to the relay with its token, and feeds each access unit to an
//! `ffmpeg` sidecar that decodes it to RGBA; the newest picture goes to a
//! texture in a window of its own, at the phone's shape. Closing the window
//! stops showing that cast (the phone keeps casting until it stops);
//! the Sessions tab can show it again.
//!
//! Once the phone says it takes control (C8), the window controls it: the
//! primary button presses, drags and releases on the phone, the wheel
//! scrolls, the right button is Back and the middle Home, typing goes to
//! the phone's focused field, and a bar holds Back, Home, Recents and a
//! switch to watch without touching.

use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use pocketdesk_core::local::cast::{self as wire, CastFile, Message};
use pocketdesk_core::streaming::cast_control::{ControlBatch, ControlEvent, Global, TouchAction};

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
    /// Where control goes: the relay connection's writing half (C8).
    writer: Option<TcpStream>,
    /// The phone takes control now, as it last said (C8).
    control_on: bool,
    /// The owner switched control off in this window, to watch only.
    watch_only: bool,
    /// The primary button is down on the phone.
    pressing: bool,
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
    /// `control` is the owner's Settings ("Control phones from their screen
    /// window", C8).
    pub fn show(&self, ctx: &egui::Context, devices: &[crate::model::Device], control: bool) {
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
            draw(&shared, ui, control);
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

/// The point `pos` as fractions of `picture` (C8), 0 to 65535 on each
/// axis; `None` outside it, unless `clamp` (a drag that left the picture
/// keeps to its edge).
fn fraction(picture: egui::Rect, pos: egui::Pos2, clamp: bool) -> Option<(u16, u16)> {
    if picture.width() <= 0.0 || picture.height() <= 0.0 {
        return None;
    }
    if !clamp && !picture.contains(pos) {
        return None;
    }
    let fx = ((pos.x - picture.left()) / picture.width()).clamp(0.0, 1.0);
    let fy = ((pos.y - picture.top()) / picture.height()).clamp(0.0, 1.0);
    Some(((fx * 65535.0).round() as u16, (fy * 65535.0).round() as u16))
}

/// The wheel's `delta` as notches (C8): positive scrolls the phone's
/// content down. Lines count one each; points, one per 50.
fn notches(delta: egui::Vec2, unit: egui::MouseWheelUnit) -> i8 {
    let lines = match unit {
        egui::MouseWheelUnit::Line => delta.y,
        egui::MouseWheelUnit::Page => delta.y * 3.0,
        egui::MouseWheelUnit::Point => delta.y / 50.0,
    };
    (-lines).round().clamp(-5.0, 5.0) as i8
}

/// This pass's input in the window as control for the phone (C8), with the
/// picture at `picture`; `pressing` follows the primary button.
fn control_events(
    events: &[egui::Event],
    picture: egui::Rect,
    hover: Option<egui::Pos2>,
    pressing: &mut bool,
) -> Vec<ControlEvent> {
    let mut out = Vec::new();
    for event in events {
        match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                ..
            } => {
                if *pressed {
                    if let Some((x, y)) = fraction(picture, *pos, false) {
                        *pressing = true;
                        out.push(ControlEvent::Touch {
                            action: TouchAction::Down,
                            x,
                            y,
                        });
                    }
                } else if *pressing {
                    *pressing = false;
                    if let Some((x, y)) = fraction(picture, *pos, true) {
                        out.push(ControlEvent::Touch {
                            action: TouchAction::Up,
                            x,
                            y,
                        });
                    }
                }
            }
            egui::Event::PointerMoved(pos) if *pressing => {
                if let Some((x, y)) = fraction(picture, *pos, true) {
                    out.push(ControlEvent::Touch {
                        action: TouchAction::Move,
                        x,
                        y,
                    });
                }
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                ..
            } if fraction(picture, *pos, false).is_some() => match button {
                egui::PointerButton::Secondary => out.push(ControlEvent::Global(Global::Back)),
                egui::PointerButton::Middle => out.push(ControlEvent::Global(Global::Home)),
                _ => {}
            },
            egui::Event::MouseWheel { unit, delta, .. } => {
                let dy = notches(*delta, *unit);
                if dy != 0 && !*pressing {
                    // At the pointer; the picture's centre when it is not over it.
                    let at = hover
                        .filter(|p| picture.contains(*p))
                        .unwrap_or(picture.center());
                    if let Some((x, y)) = fraction(picture, at, false) {
                        out.push(ControlEvent::Scroll { x, y, dx: 0, dy });
                    }
                }
            }
            egui::Event::Text(text) if !text.is_empty() => {
                out.push(ControlEvent::Text(text.clone()));
            }
            egui::Event::Key {
                key, pressed: true, ..
            } => match key {
                egui::Key::Backspace => out.push(ControlEvent::Backspace),
                egui::Key::Enter => out.push(ControlEvent::Enter),
                egui::Key::Escape => out.push(ControlEvent::Global(Global::Back)),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

/// Send `events` to the phone through the relay (C8), in batches; a write
/// that fails ends nothing (the reader sees the relay go).
fn send_control(writer: &mut TcpStream, events: Vec<ControlEvent>) {
    let sent_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for chunk in events.chunks(pocketdesk_core::streaming::cast_control::MAX_EVENTS) {
        let batch = ControlBatch {
            sent_ms,
            events: chunk.to_vec(),
        };
        if let Some(bytes) = batch.encode() {
            let _ = Message::Control(bytes).write(writer);
        }
    }
}

/// Where a `w`×`h` picture goes in `area`: as large as fits, centred.
fn letterbox(area: egui::Rect, w: usize, h: usize) -> egui::Rect {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (area.width() / w).min(area.height() / h);
    egui::Rect::from_center_size(area.center(), egui::vec2(w * scale, h * scale))
}

fn draw(shared: &Mutex<State>, ui: &mut egui::Ui, control: bool) {
    let ctx = ui.ctx().clone();
    let mut state = lock(shared);
    if ctx.input(|i| i.viewport().close_requested()) {
        state.dismissed = state.session_id;
        state.writer = None;
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
    let phone_takes = state.control_on && state.writer.is_some();
    let controlling = control && phone_takes && !state.watch_only;
    let mut watch_only = state.watch_only;
    drop(state);
    let mut buttons = Vec::new();
    if control {
        egui::Panel::top("cast-control")
            .frame(egui::Frame::NONE.fill(egui::Color32::from_gray(24)).inner_margin(4.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if phone_takes {
                        ui.add_enabled_ui(!watch_only, |ui| {
                            if ui.button("Back").clicked() {
                                buttons.push(ControlEvent::Global(Global::Back));
                            }
                            if ui.button("Home").clicked() {
                                buttons.push(ControlEvent::Global(Global::Home));
                            }
                            if ui.button("Recents").clicked() {
                                buttons.push(ControlEvent::Global(Global::Recents));
                            }
                        });
                        ui.checkbox(&mut watch_only, "Watch only")
                            .on_hover_text("Show the phone without touching it");
                    } else {
                        ui.colored_label(
                            egui::Color32::LIGHT_GRAY,
                            "To control the phone from here, turn on \u{201c}Let the computer control this phone\u{201d} in its Pocketdesk Settings.",
                        );
                    }
                });
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let mut picture_rect = None;
            match (&texture, note) {
                (_, Some(note)) => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(egui::Color32::LIGHT_GRAY, note)
                    });
                }
                (Some(texture), None) => {
                    let [w, h] = texture.size();
                    let rect = letterbox(area, w, h);
                    picture_rect = Some(rect);
                    // The picture takes the pointer, so egui does not.
                    ui.interact(
                        rect,
                        ui.id().with("cast-picture"),
                        egui::Sense::click_and_drag(),
                    );
                    ui.painter().image(
                        texture.id(),
                        rect,
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
            // C8: this pass's input, to the phone.
            let mut state = lock(shared);
            state.watch_only = watch_only;
            let mut events = Vec::new();
            if controlling && !watch_only {
                events = buttons;
                if let Some(rect) = picture_rect {
                    let (input, hover) = ctx.input(|i| (i.events.clone(), i.pointer.hover_pos()));
                    events.extend(control_events(&input, rect, hover, &mut state.pressing));
                }
            } else if state.pressing {
                state.pressing = false;
            }
            if !events.is_empty()
                && let Some(writer) = state.writer.as_mut()
            {
                send_control(writer, events);
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
        state.writer = None;
        state.control_on = false;
        state.pressing = false;
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
    // A stuck write must not hold the window (C8): control is best effort.
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    {
        let mut state = lock(shared);
        state.stream = Some(stream.try_clone()?);
        state.writer = Some(stream.try_clone()?);
    }
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
            Message::ControlState { on } => {
                lock(shared).control_on = on;
                ctx.request_repaint_of(viewport_id());
            }
            Message::Heartbeat | Message::WantKeyframe | Message::Control(_) => {}
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

    fn picture() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(200.0, 400.0))
    }

    #[test]
    fn a_point_is_a_fraction_of_the_picture_and_outside_it_is_nothing() {
        assert_eq!(
            fraction(picture(), egui::pos2(100.0, 0.0), false),
            Some((0, 0))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(300.0, 400.0), false),
            Some((65535, 65535))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(200.0, 200.0), false),
            Some((32768, 32768))
        );
        assert_eq!(
            fraction(picture(), egui::pos2(50.0, 200.0), false),
            None,
            "the letterbox's bar"
        );
        assert_eq!(
            fraction(picture(), egui::pos2(50.0, 500.0), true),
            Some((0, 65535)),
            "a drag that left keeps to the edge"
        );
    }

    #[test]
    fn a_press_drag_and_release_on_the_picture_are_one_touch() {
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut pressing = false;
        let events = control_events(
            &[
                egui::Event::PointerMoved(egui::pos2(150.0, 100.0)),
                press(egui::pos2(150.0, 100.0), true),
                egui::Event::PointerMoved(egui::pos2(160.0, 120.0)),
                egui::Event::PointerMoved(egui::pos2(500.0, 120.0)),
                press(egui::pos2(500.0, 120.0), false),
            ],
            picture(),
            None,
            &mut pressing,
        );
        let actions: Vec<_> = events
            .iter()
            .map(|e| match e {
                ControlEvent::Touch { action, x, .. } => (*action, *x),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            actions[0].0,
            TouchAction::Down,
            "a move before the press is nothing"
        );
        assert_eq!(actions[1].0, TouchAction::Move);
        assert_eq!(
            actions[2],
            (TouchAction::Move, 65535),
            "clamped to the edge"
        );
        assert_eq!(actions[3], (TouchAction::Up, 65535));
        assert!(!pressing);

        // A press on the bar is not a touch, nor its release.
        let events = control_events(
            &[
                press(egui::pos2(10.0, 10.0), true),
                press(egui::pos2(150.0, 10.0), false),
            ],
            picture(),
            None,
            &mut pressing,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn buttons_wheel_and_keys_are_what_the_phone_does_with_them() {
        let mut pressing = false;
        let at = egui::pos2(200.0, 100.0);
        let events = control_events(
            &[
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Middle,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -2.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("hé".into()),
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            picture(),
            Some(at),
            &mut pressing,
        );
        assert_eq!(
            events,
            vec![
                ControlEvent::Global(Global::Back),
                ControlEvent::Global(Global::Home),
                ControlEvent::Scroll {
                    x: 32768,
                    y: 16384,
                    dx: 0,
                    dy: 2
                },
                ControlEvent::Text("hé".into()),
                ControlEvent::Backspace,
                ControlEvent::Enter,
                ControlEvent::Global(Global::Back),
            ]
        );
    }

    #[test]
    fn the_wheel_counts_lines_and_points() {
        assert_eq!(
            notches(egui::vec2(0.0, 1.0), egui::MouseWheelUnit::Line),
            -1
        );
        assert_eq!(
            notches(egui::vec2(0.0, -100.0), egui::MouseWheelUnit::Point),
            2
        );
        assert_eq!(
            notches(egui::vec2(0.0, -99.0), egui::MouseWheelUnit::Line),
            5,
            "bounded"
        );
        assert_eq!(
            notches(egui::vec2(0.0, 10.0), egui::MouseWheelUnit::Point),
            0
        );
    }

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
