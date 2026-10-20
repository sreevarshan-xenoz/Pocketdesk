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
    /// Where the last touch went, for a release the window did not see.
    last_touch: (u16, u16),
    /// What a drop on the window did, and when (L2).
    drop_note: Option<(String, std::time::Instant)>,
    /// The picture shown, for a screenshot (L5).
    shown: Option<Picture>,
    /// The owner asked to record (L5); the worker starts and stops it.
    record_want: bool,
    /// The recording under way, and since when (L5).
    recording: Option<(PathBuf, std::time::Instant)>,
    /// What the last screenshot or recording did, and when (L5).
    saved_note: Option<(String, std::time::Instant)>,
}

/// How long a drop's line stays at the window's foot.
const DROP_NOTE: std::time::Duration = std::time::Duration::from_secs(6);

/// What a drop of `files` and `folders` on `device`'s window says (L2):
/// sending, or why not; `failed` names what could not be queued.
pub fn drop_words(reaches: bool, files: usize, folders: usize, failed: &[String]) -> String {
    if !reaches {
        return "This phone cannot take files now: it takes them in a session, or with \u{201c}Files from the computer in the background\u{201d} on in its Settings.".to_string();
    }
    if failed.is_empty() {
        return format!(
            "Sending {} to the phone: the Files page's Sent list says when they have gone.",
            crate::model::files_and_folders(files, folders)
        );
    }
    // What went, beside what did not (L2 review: the queued ones were not said).
    let total = files + folders;
    let reasons: Vec<&str> = failed.iter().map(|f| f.trim_end_matches('.')).collect();
    format!(
        "Queued {} of {total}; not queued: {}.",
        total.saturating_sub(failed.len()),
        reasons.join("; ")
    )
}

/// `text` cut to the control road's limit for one text, at a character (L2).
fn pasted(text: &str) -> String {
    let max = pocketdesk_core::streaming::cast_control::MAX_TEXT;
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// How long a screenshot's or recording's line stays (L5).
const SAVED_NOTE: std::time::Duration = std::time::Duration::from_secs(8);

/// `unix` as a name's date and time, UTC: "2026-10-21 01-02-03" (L5).
pub fn stamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}-{:02}-{:02}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The user's own folder `name` ("Pictures", "Videos"), with Pocketdesk in it.
fn user_folder(name: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(name).join("Pocketdesk")
}

/// `dir`/`stem`.`ext`, or the next free "`stem` (2)".
fn free_name(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A recording: the cast's H.264 copied, not encoded again, into an MKV by
/// ffmpeg, with the wall clock's timestamps (the cast's frame rate varies),
/// from a keyframe on (L5).
struct Recorder {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    /// Nothing is written before a keyframe: the file would open grey.
    started: bool,
}

impl Recorder {
    fn start(path: &std::path::Path) -> io::Result<Self> {
        let mut command = Command::new("ffmpeg");
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-use_wallclock_as_timestamps",
                "1",
                "-fflags",
                "+genpts",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                "-c",
                "copy",
                "-y",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            started: false,
        })
    }

    /// Write one access unit, once a keyframe has come.
    fn feed(&mut self, keyframe: bool, access_unit: &[u8]) -> io::Result<()> {
        self.started |= keyframe;
        if !self.started {
            return Ok(());
        }
        match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(access_unit),
            None => Ok(()),
        }
    }
}

impl Drop for Recorder {
    /// The file is closed: ffmpeg finishes it once its input ends.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

/// The cast window and its worker.
pub struct CastView {
    shared: Arc<Mutex<State>>,
    /// Where drops are queued (L2).
    state_dir: PathBuf,
}

impl CastView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(State::default()));
        let worker = shared.clone();
        let ctx = ctx.clone();
        let watched = state_dir.clone();
        let spawned = std::thread::Builder::new()
            .name("cast-view".into())
            .spawn(move || watch(&watched, &worker, &ctx));
        // Without the thread no cast is ever shown, and nothing else breaks.
        drop(spawned);
        Self { shared, state_dir }
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
    /// window", C8); `targets`, the phones files reach now (L2).
    pub fn show(
        &self,
        ctx: &egui::Context,
        devices: &[crate::model::Device],
        control: bool,
        targets: Vec<String>,
    ) {
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
        let state_dir = self.state_dir.clone();
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            take_drops(&shared, ui.ctx(), &state_dir, &targets);
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
            // Ctrl+V: the computer's clipboard typed at the phone (L2).
            egui::Event::Paste(text) if !text.is_empty() => {
                out.push(ControlEvent::Text(pasted(text)));
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
    coalesce_moves(out)
}

/// Moves one after another become their last: a 1000 Hz mouse sends one
/// move a pass, not one a millisecond (C8 review). Presses, releases and
/// all else keep their order.
fn coalesce_moves(events: Vec<ControlEvent>) -> Vec<ControlEvent> {
    let mut out: Vec<ControlEvent> = Vec::with_capacity(events.len());
    for event in events {
        let is_move = matches!(
            event,
            ControlEvent::Touch {
                action: TouchAction::Move,
                ..
            }
        );
        if is_move
            && matches!(
                out.last(),
                Some(ControlEvent::Touch {
                    action: TouchAction::Move,
                    ..
                })
            )
        {
            out.pop();
        }
        out.push(event);
    }
    out
}

/// Send `events` to the phone through the relay (C8), in batches; whether
/// all went. A write that fails mid-message leaves the framing broken, so
/// the caller stops writing there (the reader sees the relay go).
fn send_control(writer: &mut TcpStream, events: Vec<ControlEvent>) -> bool {
    let sent_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for chunk in events.chunks(pocketdesk_core::streaming::cast_control::MAX_EVENTS) {
        let batch = ControlBatch {
            sent_ms,
            events: chunk.to_vec(),
        };
        if let Some(bytes) = batch.encode()
            && Message::Control(bytes).write(writer).is_err()
        {
            return false;
        }
    }
    true
}

/// Where a `w`×`h` picture goes in `area`: as large as fits, centred.
fn letterbox(area: egui::Rect, w: usize, h: usize) -> egui::Rect {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (area.width() / w).min(area.height() / h);
    egui::Rect::from_center_size(area.center(), egui::vec2(w * scale, h * scale))
}

/// Files or folders dropped on the window go to its phone (L2), as the Files
/// page sends them, and the window says how that went.
fn take_drops(
    shared: &Mutex<State>,
    ctx: &egui::Context,
    state_dir: &std::path::Path,
    targets: &[String],
) {
    let dropped: Vec<PathBuf> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .collect()
    });
    if dropped.is_empty() {
        return;
    }
    let device = lock(shared).device_id.clone();
    let reaches = !device.is_empty() && targets.contains(&device);
    let folders = dropped.iter().filter(|p| p.is_dir()).count();
    let files = dropped.len() - folders;
    let mut failed = Vec::new();
    if reaches {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for path in &dropped {
            if let Err(e) = crate::model::queue_send(state_dir, &device, path, now) {
                failed.push(e);
            }
        }
    }
    lock(shared).drop_note = Some((
        drop_words(reaches, files, folders, &failed),
        std::time::Instant::now(),
    ));
    ctx.request_repaint_after(DROP_NOTE);
}

fn draw(shared: &Arc<Mutex<State>>, ui: &mut egui::Ui, control: bool) {
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
        // Kept for a screenshot (L5).
        state.shown = Some(picture);
    }
    let texture = state.texture.clone();
    let note = state.note.clone();
    let phone_takes = state.control_on && state.writer.is_some();
    let controlling = control && phone_takes && !state.watch_only;
    let mut watch_only = state.watch_only;
    let recording = state.recording.clone();
    let record_want = state.record_want;
    let saved = state
        .saved_note
        .clone()
        .filter(|(_, at)| at.elapsed() < SAVED_NOTE)
        .map(|(note, _)| note);
    drop(state);
    let mut buttons = Vec::new();
    // L5: a screenshot and a recording, whatever control says.
    egui::Panel::top("cast-capture")
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(24))
                .inner_margin(4.0),
        )
        .show(ui, |ui| {
            ui.style_mut().visuals = egui::Visuals::dark();
            ui.horizontal_wrapped(|ui| {
                let shot = ui
                    .add_enabled(texture.is_some(), egui::Button::new("Screenshot"))
                    .on_hover_text("Save the phone's screen as a picture, and copy it");
                if shot.clicked() {
                    shot.surrender_focus();
                    screenshot(shared, &ctx);
                }
                let label = if record_want { "Stop" } else { "Record" };
                let record = ui
                    .add_enabled(texture.is_some() || record_want, egui::Button::new(label))
                    .on_hover_text("Record the phone's screen as a video, as it comes");
                if record.clicked() {
                    record.surrender_focus();
                    lock(shared).record_want = !record_want;
                }
                if let Some((path, since)) = &recording {
                    let size = std::fs::metadata(path).map_or(0, |m| m.len());
                    let secs = since.elapsed().as_secs();
                    ui.colored_label(
                        egui::Color32::LIGHT_GRAY,
                        format!(
                            "Recording {}:{:02} · {}",
                            secs / 60,
                            secs % 60,
                            crate::words::human_size(size)
                        ),
                    );
                    ctx.request_repaint_after(Duration::from_secs(1));
                } else if let Some(note) = &saved {
                    ui.colored_label(egui::Color32::LIGHT_GRAY, note);
                    ctx.request_repaint_after(SAVED_NOTE);
                }
            });
        });
    if control {
        egui::Panel::top("cast-control")
            .frame(egui::Frame::NONE.fill(egui::Color32::from_gray(24)).inner_margin(4.0))
            .show(ui, |ui| {
                // The bar is dark whatever the theme: in the light one its
                // buttons were dark text on near black, about 1.5:1 (E5).
                ui.style_mut().visuals = egui::Visuals::dark();
                ui.horizontal_wrapped(|ui| {
                    if phone_takes {
                        ui.add_enabled_ui(!watch_only, |ui| {
                            for (label, global) in [
                                ("Back", Global::Back),
                                ("Home", Global::Home),
                                ("Recents", Global::Recents),
                            ] {
                                let button = ui.button(label);
                                if button.clicked() {
                                    // Enter and Space go to the phone, not
                                    // to this button again (C8 review).
                                    button.surrender_focus();
                                    buttons.push(ControlEvent::Global(global));
                                }
                            }
                        });
                        let switch = ui
                            .checkbox(&mut watch_only, "Watch only")
                            .on_hover_text("Show the phone without touching it");
                        if switch.clicked() {
                            switch.surrender_focus();
                        }
                    } else {
                        ui.colored_label(
                            egui::Color32::LIGHT_GRAY,
                            "To control the phone from here, turn on \u{201c}Let the computer control this phone\u{201d} in Pocketdesk's Settings on the phone.",
                        );
                    }
                });
            });
    }
    let drop_note = lock(shared)
        .drop_note
        .clone()
        .filter(|(_, at)| at.elapsed() < DROP_NOTE)
        .map(|(note, _)| note);
    // Over the picture, never beside it (L2 review: a panel shrank the
    // picture, and a drag under way jumped).
    if let Some(note) = drop_note {
        egui::Area::new(egui::Id::new("cast-drop"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -8.0))
            .interactable(false)
            .show(&ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_gray(24))
                    .inner_margin(6.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ctx.content_rect().width() - 32.0);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(note).color(egui::Color32::LIGHT_GRAY),
                            )
                            .wrap(),
                        );
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
                    )
                    // Named for a screen reader (E5).
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Phone screen")
                    });
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
            // A picture of the old shape (the phone just turned) would put
            // a tap in the wrong place: nothing goes until the new one shows.
            let current = match (&texture, state.size) {
                (Some(t), Some((w, h))) => t.size() == [usize::from(w), usize::from(h)],
                _ => false,
            };
            let mut events = Vec::new();
            if controlling && !watch_only && current {
                events = buttons;
                if let Some(rect) = picture_rect {
                    let (input, hover, down) = ctx.input(|i| {
                        (
                            i.events.clone(),
                            i.pointer.hover_pos(),
                            i.pointer.primary_down(),
                        )
                    });
                    events.extend(control_events(&input, rect, hover, &mut state.pressing));
                    // A paste cut to the road's limit says so (L2 review).
                    let max = pocketdesk_core::streaming::cast_control::MAX_TEXT;
                    if input
                        .iter()
                        .any(|e| matches!(e, egui::Event::Paste(text) if text.len() > max))
                    {
                        state.drop_note = Some((
                            format!("Pasted the first {max} bytes of the clipboard."),
                            std::time::Instant::now(),
                        ));
                        ctx.request_repaint_after(DROP_NOTE);
                    }
                    // A release the window never saw (focus went mid-drag):
                    // the finger lifts where it last was (C8 review).
                    if state.pressing && !down {
                        state.pressing = false;
                        let (x, y) = state.last_touch;
                        events.push(ControlEvent::Touch {
                            action: TouchAction::Up,
                            x,
                            y,
                        });
                    }
                }
            } else if state.pressing {
                // Control went off mid-press: the finger lifts.
                state.pressing = false;
                let (x, y) = state.last_touch;
                events.push(ControlEvent::Touch {
                    action: TouchAction::Up,
                    x,
                    y,
                });
            }
            if let Some(ControlEvent::Touch { x, y, .. }) = events
                .iter()
                .rev()
                .find(|e| matches!(e, ControlEvent::Touch { .. }))
            {
                state.last_touch = (*x, *y);
            }
            // Written outside the window's lock, so a slow relay holds
            // nothing else; the first broken write ends control here.
            let writer = (!events.is_empty())
                .then(|| state.writer.as_ref().and_then(|w| w.try_clone().ok()))
                .flatten();
            drop(state);
            if let Some(mut writer) = writer
                && !send_control(&mut writer, events)
            {
                lock(shared).writer = None;
            }
        });
}

/// A recording ended (its ffmpeg has finished the file): the window says
/// where it went, if it holds anything, and why it stopped (L5 review: a
/// recording's state had outlived it, and named files never written).
fn end_recording(shared: &Mutex<State>, why: Option<&str>) {
    let mut state = lock(shared);
    state.record_want = false;
    let Some((path, _)) = state.recording.take() else {
        return;
    };
    let kept = std::fs::metadata(&path).is_ok_and(|m| m.len() > 0);
    let note = match (kept, why) {
        (true, None) => format!("Recorded {}.", path.display()),
        (true, Some(why)) => format!("Recorded {}; {why}.", path.display()),
        (false, Some(why)) => format!("Nothing recorded: {why}."),
        (false, None) => "Nothing recorded: no picture came.".to_string(),
    };
    state.saved_note = Some((note, std::time::Instant::now()));
}

/// Save the picture shown as a PNG in Pictures\\Pocketdesk, and copy it
/// (L5); the file is written off the window's thread.
fn screenshot(shared: &Arc<Mutex<State>>, ctx: &egui::Context) {
    let Some((width, height, rgba)) = lock(shared)
        .shown
        .as_ref()
        .map(|p| (p.width, p.height, p.rgba.clone()))
    else {
        return;
    };
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &rgba,
    ));
    let dir = user_folder("Pictures");
    let path = free_name(&dir, &format!("Phone screen {}", stamp(now_unix())), "png");
    let shared = shared.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let saved = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                    .ok_or_else(|| "a picture of the wrong size".to_string())?
                    .save(&path)
                    .map_err(|e| e.to_string())
            });
        lock(&shared).saved_note = Some((
            match saved {
                Ok(()) => format!("Saved {} and copied it.", path.display()),
                Err(e) => format!("Copied, not saved: {e}."),
            },
            std::time::Instant::now(),
        ));
        ctx.request_repaint_of(viewport_id());
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
        // A recording ends with its cast, however it ended (L5 review).
        end_recording(shared, Some("the phone's screen stopped"));
        ctx.request_repaint_of(viewport_id());
        let mut state = lock(shared);
        state.stream = None;
        state.writer = None;
        state.control_on = false;
        state.pressing = false;
        state.size = None;
        state.texture = None;
        state.picture = None;
        state.shown = None;
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
    let mut recorder: Option<Recorder> = None;
    loop {
        let message = Message::read(&mut reader)?;
        // L5: the owner's Record and Stop, between frames or heartbeats; a
        // config (a reconnect, the phone turned) starts a new stream first.
        let want = lock(shared).record_want
            && matches!(message, Message::Frame { .. } | Message::Heartbeat);
        let stop = !lock(shared).record_want;
        if want && recorder.is_none() {
            let dir = user_folder("Videos");
            let path = free_name(&dir, &format!("Phone screen {}", stamp(now_unix())), "mkv");
            let started = std::fs::create_dir_all(&dir).and_then(|()| Recorder::start(&path));
            let mut state = lock(shared);
            match started {
                Ok(r) => {
                    recorder = Some(r);
                    state.recording = Some((path, std::time::Instant::now()));
                    // It starts on the next keyframe.
                    Message::WantKeyframe.write(&mut asks)?;
                }
                Err(e) => {
                    state.record_want = false;
                    let why = if e.kind() == io::ErrorKind::NotFound {
                        format!("Not recording: {e} (it needs ffmpeg).")
                    } else {
                        format!("Not recording: {e}.")
                    };
                    state.saved_note = Some((why, std::time::Instant::now()));
                }
            }
        } else if stop && recorder.is_some() {
            recorder = None;
            end_recording(shared, None);
            ctx.request_repaint_of(viewport_id());
        }
        match message {
            Message::Config { width, height } => {
                decoder = None;
                gate = KeyGate::default();
                // A new size is a new stream: the recording so far is kept.
                if recorder.take().is_some() {
                    end_recording(shared, Some("it stopped as the phone's screen changed"));
                }
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
                // L5: the recording takes it as it is.
                if let Some(r) = recorder.as_mut()
                    && let Err(e) = r.feed(keyframe, &data)
                {
                    recorder = None;
                    end_recording(shared, Some(&format!("it stopped ({e})")));
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
    /// L2: a drop says it is sending, why it cannot, or what failed.
    #[test]
    fn a_drop_says_what_it_did() {
        assert!(drop_words(true, 2, 0, &[]).starts_with("Sending 2 files to the phone"));
        assert!(drop_words(true, 1, 1, &[]).contains("1 file and 1 folder"));
        assert!(drop_words(false, 1, 0, &[]).starts_with("This phone cannot take files now"));
        assert_eq!(
            drop_words(
                true,
                3,
                0,
                &["c.txt is not a file or a folder.".to_string()]
            ),
            "Queued 2 of 3; not queued: c.txt is not a file or a folder."
        );
    }

    /// L2: a paste is typed, cut at a character within the road's limit.
    #[test]
    fn a_paste_is_typed_within_its_limit() {
        let max = pocketdesk_core::streaming::cast_control::MAX_TEXT;
        assert_eq!(pasted("héllo"), "héllo");
        let long = "é".repeat(max);
        let cut = pasted(&long);
        assert!(cut.len() <= max && cut.chars().all(|c| c == 'é'));
        let mut pressing = false;
        let events = control_events(
            &[egui::Event::Paste("from here".into())],
            picture(),
            None,
            &mut pressing,
        );
        assert_eq!(events, vec![ControlEvent::Text("from here".into())]);
    }

    /// L5: a recording copies the stream into an MKV of the wall clock's
    /// length, from its first keyframe. Needs ffmpeg and ffprobe; skipped
    /// without them.
    #[test]
    fn a_recording_lasts_as_long_as_the_cast() {
        let dir = std::env::temp_dir().join(format!("pd-l5-rec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("t.h264");
        let made = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("testsrc=size=320x240:rate=10")
            .args([
                "-t",
                "2",
                "-c:v",
                "libx264",
                "-x264-params",
                "aud=1:keyint=10",
            ])
            .args(["-bsf:v", "h264_mp4toannexb", "-f", "h264"])
            .arg(&source)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        }
        let data = std::fs::read(&source).unwrap();
        let aud: &[u8] = &[0, 0, 0, 1, 9];
        let mut starts: Vec<usize> = data
            .windows(aud.len())
            .enumerate()
            .filter(|(_, w)| *w == aud)
            .map(|(i, _)| i)
            .collect();
        starts.push(data.len());
        let units: Vec<&[u8]> = starts.windows(2).map(|w| &data[w[0]..w[1]]).collect();
        assert_eq!(units.len(), 20);
        let out = dir.join("out.mkv");
        let mut recorder = Recorder::start(&out).unwrap();
        // A delta frame before the first keyframe is left out.
        recorder.feed(false, units[1]).unwrap();
        for (i, unit) in units.iter().enumerate() {
            recorder.feed(i % 10 == 0, unit).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(recorder);
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_name,width",
            ])
            .args(["-of", "default=nw=1"])
            .arg(&out)
            .output();
        let Ok(probe) = probe else {
            eprintln!("no ffprobe here: skipped");
            return;
        };
        let text = String::from_utf8_lossy(&probe.stdout).to_string();
        assert!(
            text.contains("codec_name=h264") && text.contains("width=320"),
            "{text}"
        );
        let duration: f64 = text
            .lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.parse().ok())
            .unwrap();
        assert!(
            (1.6..2.4).contains(&duration),
            "{duration} s for 2 s of frames"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L5: names carry the date and time, UTC.
    #[test]
    fn a_name_carries_the_date() {
        assert_eq!(super::stamp(0), "1970-01-01 00-00-00");
        assert_eq!(super::stamp(1_792_070_523), "2026-10-15 13-22-03");
        assert_eq!(
            super::stamp(951_782_400),
            "2000-02-29 00-00-00",
            "a leap day"
        );
    }

    /// L5: a name taken gets the next free one.
    #[test]
    fn a_name_taken_gets_the_next() {
        let dir = std::env::temp_dir().join(format!("pd-l5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            super::free_name(&dir, "Phone", "png"),
            dir.join("Phone.png")
        );
        std::fs::write(dir.join("Phone.png"), b"x").unwrap();
        assert_eq!(
            super::free_name(&dir, "Phone", "png"),
            dir.join("Phone (2).png")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert_eq!(
            actions[1],
            (TouchAction::Move, 65535),
            "two moves in a pass are their last, clamped to the edge"
        );
        assert_eq!(actions[2], (TouchAction::Up, 65535));
        assert_eq!(actions.len(), 3);
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
    fn control_goes_to_the_relay_as_batches_it_can_read() {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut relay, _) = listener.accept().unwrap();
        let many: Vec<_> = (0..70)
            .map(|n| ControlEvent::Touch {
                action: TouchAction::Move,
                x: n,
                y: n,
            })
            .collect();
        send_control(&mut writer, many.clone());
        let mut got = Vec::new();
        for _ in 0..2 {
            let Message::Control(bytes) = Message::read(&mut relay).unwrap() else {
                panic!("a control message");
            };
            got.extend(ControlBatch::decode(&bytes).expect("readable").events);
        }
        assert_eq!(got, many, "split at the batch limit, nothing lost");
    }

    #[test]
    fn moves_in_a_pass_become_their_last_and_nothing_else_moves() {
        let mv = |x| ControlEvent::Touch {
            action: TouchAction::Move,
            x,
            y: 0,
        };
        let down = ControlEvent::Touch {
            action: TouchAction::Down,
            x: 0,
            y: 0,
        };
        let up = ControlEvent::Touch {
            action: TouchAction::Up,
            x: 9,
            y: 0,
        };
        assert_eq!(
            coalesce_moves(vec![
                down.clone(),
                mv(1),
                mv(2),
                mv(3),
                up.clone(),
                mv(4),
                mv(5)
            ]),
            vec![down, mv(3), up, mv(5)]
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
