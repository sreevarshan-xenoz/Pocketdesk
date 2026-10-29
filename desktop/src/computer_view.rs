//! Another of the owner's computers' screens in a window (BK7).
//!
//! "Show its screen" in Settings under Other computers asks the server to dial that
//! computer's screen (`computers_out/`, `ComputerAsk::View`); the server
//! relays the picture over loopback and names it in `view.json`
//! (`core::local::view`). A worker thread here connects with the file's
//! token and decodes each access unit through the `ffmpeg` sidecar the
//! phone's cast window uses ([`crate::video`]), into a texture in a window
//! of its own, letterboxed. When the other computer takes this one's input
//! (its Devices page's Control switch for this computer), the window's
//! pointer and keys go to it ([`crate::view_input`]); without it, or with
//! View only on, the window only shows. Closing the window ends the view; a
//! view that ends on its own says why, with Reconnect.

use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::config::COMPUTER_VIEW_QUALITIES;
use pocketdesk_core::local::view::{self as wire, Message, Phase, ViewFile, ViewStatus};
use pocketdesk_core::streaming::InputPacket;

use crate::video::{
    Admit, Decoder, KeyGate, Picture, Recorder, free_name, letterbox, now_unix, stamp, user_folder,
};
use crate::view_input::{MOVE_EVERY, WindowInput};

/// The window's id among the app's viewports.
fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pocketdesk-computer-view")
}

/// How often the worker looks for the view.
const POLL: Duration = Duration::from_millis(300);
/// The relay sends a heartbeat every 2 s; silence past this is a dead relay.
const READ_TIMEOUT: Duration = Duration::from_secs(6);
/// A view asked for that the server never names is given up on.
const NO_ANSWER: Duration = Duration::from_secs(20);
/// A window joins its view again at most this many times in
/// [`REJOIN_WINDOW`].
const REJOINS: usize = 3;
const REJOIN_WINDOW: Duration = Duration::from_secs(10);
/// How long a screenshot's or recording's line stays.
const SAVED_NOTE: Duration = Duration::from_secs(8);

/// The computer whose screen is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    device_id: String,
    name: String,
    monitor: u32,
}

#[derive(Default)]
struct State {
    wanted: Option<Wanted>,
    /// When the view was last asked for.
    asked_at: Option<Instant>,
    /// Only a view newer than this one is this window's (one ended, or
    /// replaced by another screen's).
    newer_than: u64,
    /// Counts the asks (a view asked for, a reconnect, another screen) and
    /// closes: a view the worker shows is still this window's while it is
    /// unchanged.
    asked: u64,
    /// When this window last joined its view again (BK7 re-review N3).
    rejoins: Vec<Instant>,
    /// The view being shown.
    session_id: Option<u64>,
    status: ViewStatus,
    /// The picture's size as the stream says it now.
    size: Option<(u16, u16)>,
    picture: Option<Picture>,
    texture: Option<egui::TextureHandle>,
    /// The picture shown, for a screenshot.
    shown: Option<Picture>,
    /// What keeps the picture from showing (no decoder).
    note: Option<String>,
    /// Why the view ended, until Reconnect.
    ended: Option<String>,
    /// The relay connection, to cut when the window closes, and its
    /// writing half for input.
    stream: Option<TcpStream>,
    writer: Option<TcpStream>,
    input: WindowInput,
    view_only: bool,
    /// The quality preset chosen, and the view it was last sent for.
    quality: u8,
    quality_sent: Option<u64>,
    record_want: bool,
    recording: Option<(PathBuf, Instant)>,
    saved_note: Option<(String, Instant)>,
    fullscreen: bool,
}

/// What the window says while there is no picture.
pub fn waiting_words(name: &str, status: &ViewStatus) -> String {
    match status.phase {
        Phase::Connecting => format!("Reaching {name}\u{2026}"),
        Phase::Waiting => format!(
            "Waiting for {name} to let this computer in\u{2026} If it asks first, allow the session there."
        ),
        Phase::Showing => format!("Waiting for {name}'s screen\u{2026}"),
    }
}

/// What the window says while there is no picture: what keeps it from
/// showing when the server says, else how the view stands.
pub fn shown_words(name: &str, status: &ViewStatus) -> String {
    status
        .note
        .clone()
        .unwrap_or_else(|| waiting_words(name, status))
}

/// What the window says of control: `None` while it controls.
pub fn control_words(name: &str, control: Option<bool>, view_only: bool) -> Option<String> {
    match (control, view_only) {
        (Some(false), _) => Some(format!(
            "View only: {name} does not take this computer's clicks and keys. To control it from here, turn on \u{201c}Control the mouse and keyboard\u{201d} for this computer on {name}'s Devices page."
        )),
        (_, true) => Some("View only: this window sends nothing.".to_string()),
        (None, false) => None,
        (Some(true), false) => None,
    }
}

/// Whether the window's input goes to the other computer now.
pub fn controls(control: Option<bool>, view_only: bool, connected: bool, current: bool) -> bool {
    control == Some(true) && !view_only && connected && current
}

/// The other computer's screens to choose from: its names, or "Screen n"
/// for as many as the chosen one needs when they are not known.
pub fn screen_choices(status: &ViewStatus) -> Vec<String> {
    if status.monitors.is_empty() {
        return (0..=status.monitor)
            .map(|n| format!("Screen {}", n + 1))
            .collect();
    }
    status.monitors.clone()
}

/// The window and its worker.
pub struct ComputerView {
    shared: Arc<Mutex<State>>,
    state_dir: PathBuf,
}

fn lock(shared: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ComputerView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf) -> Self {
        let shared = Arc::new(Mutex::new(State::default()));
        let worker = shared.clone();
        let ctx = ctx.clone();
        let watched = state_dir.clone();
        let spawned = std::thread::Builder::new()
            .name("computer-view".into())
            .spawn(move || watch(&watched, &worker, &ctx));
        // Without the thread no screen is shown, and nothing else breaks.
        drop(spawned);
        Self { shared, state_dir }
    }

    /// Show the computer `device_id`'s screen `monitor`: asked of the
    /// server, the window opens at once and fills when the picture comes.
    /// `quality` is the preset it starts at, `view_only` whether it sends
    /// nothing; the window's own choices then hold.
    pub fn open(
        &self,
        device_id: &str,
        name: &str,
        monitor: u32,
        quality: u8,
        view_only: bool,
    ) -> Result<(), String> {
        let wanted = Wanted {
            device_id: device_id.to_string(),
            name: name.to_string(),
            monitor,
        };
        let mut state = lock(&self.shared);
        let same = state
            .wanted
            .as_ref()
            .is_some_and(|w| w.device_id == device_id);
        if !same {
            state.quality = quality;
            state.view_only = view_only;
        }
        ask(&self.state_dir, &mut state, wanted)
    }

    /// Call each pass: keeps the window open while a screen is wanted.
    pub fn show(&self, ctx: &egui::Context) {
        let title = {
            let state = lock(&self.shared);
            let Some(wanted) = &state.wanted else { return };
            format!("{} \u{b7} screen", wanted.name)
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([320.0, 240.0]);
        let shared = self.shared.clone();
        let state_dir = self.state_dir.clone();
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            draw(&shared, &state_dir, ui);
        });
    }
}

/// Ask the server for `wanted`'s screen; a view shown now is let go first.
fn ask(state_dir: &Path, state: &mut State, wanted: Wanted) -> Result<(), String> {
    let request = ComputerAsk::View {
        device_id: wanted.device_id.clone(),
        monitor: wanted.monitor,
    };
    let release = state.input.release_all();
    send(state, release);
    computers::request(state_dir, request, now_unix()).map_err(|e| format!("Not asked: {e}"))?;
    if let Some(stream) = state.stream.take() {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    state.writer = None;
    state.newer_than = state.newer_than.max(state.session_id.unwrap_or(0));
    state.session_id = None;
    state.status = ViewStatus {
        monitor: wanted.monitor,
        ..ViewStatus::default()
    };
    state.ended = None;
    state.note = None;
    state.asked_at = Some(Instant::now());
    state.wanted = Some(wanted);
    state.asked += 1;
    state.rejoins.clear();
    Ok(())
}

/// Send `packets` to the other computer through the relay; a broken write
/// ends input here (the reader sees the relay go).
fn send(state: &mut State, packets: Vec<InputPacket>) {
    if packets.is_empty() {
        return;
    }
    let Some(writer) = state.writer.as_mut() else {
        return;
    };
    for packet in packets {
        let bytes = packet.encode();
        if bytes.len() > wire::MAX_INPUT {
            continue;
        }
        if Message::Input(bytes).write(writer).is_err() {
            state.writer = None;
            return;
        }
    }
}

fn draw(shared: &Arc<Mutex<State>>, state_dir: &Path, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut state = lock(shared);
    let Some(wanted) = state.wanted.clone() else {
        return;
    };
    if ctx.input(|i| i.viewport().close_requested()) {
        // Let go of what is held there, then end the view.
        let release = state.input.release_all();
        send(&mut state, release);
        state.wanted = None;
        state.writer = None;
        if let Some(stream) = state.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        // The view this window shows, when it reached one (BK7 review M1).
        let shown = state.session_id;
        state.newer_than = state.newer_than.max(state.session_id.unwrap_or(0));
        state.session_id = None;
        state.asked += 1;
        state.fullscreen = false;
        let _ = computers::request(
            state_dir,
            ComputerAsk::StopView { session_id: shown },
            now_unix(),
        );
        return;
    }
    if let Some(picture) = state.picture.take() {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [picture.width, picture.height],
            &picture.rgba,
        );
        match &mut state.texture {
            Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
            None => {
                state.texture =
                    Some(ctx.load_texture("computer-view", image, egui::TextureOptions::LINEAR));
            }
        }
        state.shown = Some(picture);
    }
    // The window lost focus: whatever is held there is let go.
    let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
    if !focused && state.input.holds() {
        let release = state.input.release_all();
        send(&mut state, release);
    }
    let texture = state.texture.clone();
    let status = state.status.clone();
    let ended = state.ended.clone();
    let note = state.note.clone();
    let connected = state.writer.is_some();
    let mut view_only = state.view_only;
    let mut quality = state.quality;
    let record_want = state.record_want;
    let recording = state.recording.clone();
    let fullscreen = state.fullscreen;
    let saved = state
        .saved_note
        .clone()
        .filter(|(_, at)| at.elapsed() < SAVED_NOTE)
        .map(|(note, _)| note);
    drop(state);

    let mut monitor = wanted.monitor;
    egui::Panel::top("computer-view-bar")
        .frame(
            egui::Frame::NONE
                .fill(egui::Color32::from_gray(24))
                .inner_margin(4.0),
        )
        .show(ui, |ui| {
            // Dark whatever the theme, as the cast window's bar.
            ui.style_mut().visuals = egui::Visuals::dark();
            ui.horizontal_wrapped(|ui| {
                let mut said = wanted.name.clone();
                if !status.route.is_empty() {
                    said.push_str(&format!(" \u{b7} {}", status.route));
                }
                ui.label(egui::RichText::new(said).strong());
                let choices = screen_choices(&status);
                if choices.len() > 1 {
                    let shown = choices
                        .get(monitor as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("Screen {}", monitor + 1));
                    egui::ComboBox::from_id_salt("computer-view-screen")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (n, choice) in choices.iter().enumerate() {
                                ui.selectable_value(&mut monitor, n as u32, choice);
                            }
                        });
                }
                let label = COMPUTER_VIEW_QUALITIES
                    .iter()
                    .find(|(_, id)| *id == quality)
                    .map_or("Auto", |(name, _)| {
                        crate::settings_view::computer_quality_label(name)
                    });
                egui::ComboBox::from_id_salt("computer-view-quality")
                    .selected_text(format!("Quality: {label}"))
                    .show_ui(ui, |ui| {
                        for (name, id) in COMPUTER_VIEW_QUALITIES {
                            ui.selectable_value(
                                &mut quality,
                                id,
                                crate::settings_view::computer_quality_label(name),
                            );
                        }
                    });
                let switch = ui
                    .checkbox(&mut view_only, "View only")
                    .on_hover_text("Show the computer without sending it clicks or keys");
                if switch.clicked() {
                    switch.surrender_focus();
                }
                let shot = ui
                    .add_enabled(texture.is_some(), egui::Button::new("Screenshot"))
                    .on_hover_text("Save the computer's screen as a picture, and copy it");
                if shot.clicked() {
                    shot.surrender_focus();
                    screenshot(shared, &ctx, &wanted.name);
                }
                let record = ui
                    .add_enabled(
                        texture.is_some() || record_want,
                        egui::Button::new(if record_want { "Stop" } else { "Record" }),
                    )
                    .on_hover_text("Record the computer's screen as a video, as it comes");
                if record.clicked() {
                    record.surrender_focus();
                    lock(shared).record_want = !record_want;
                }
                let full = ui.button(if fullscreen {
                    "Exit full screen"
                } else {
                    "Full screen"
                });
                if full.clicked() {
                    full.surrender_focus();
                    lock(shared).fullscreen = !fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                }
                if let Some((_, since)) = &recording {
                    let secs = since.elapsed().as_secs();
                    ui.colored_label(
                        egui::Color32::LIGHT_GRAY,
                        format!("Recording {}:{:02}", secs / 60, secs % 60),
                    );
                    ctx.request_repaint_after(Duration::from_secs(1));
                } else if let Some(note) = &saved {
                    ui.colored_label(egui::Color32::LIGHT_GRAY, note);
                    ctx.request_repaint_after(SAVED_NOTE);
                }
            });
            // What keeps the picture from showing, over a picture that
            // stopped (BK7 review M6).
            if ended.is_none()
                && texture.is_some()
                && let Some(note) = &status.note
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(note).color(egui::Color32::LIGHT_GRAY))
                        .wrap(),
                );
            }
            if ended.is_none()
                && status.phase == Phase::Showing
                && let Some(words) = control_words(&wanted.name, status.control, view_only)
            {
                ui.add(
                    egui::Label::new(egui::RichText::new(words).color(egui::Color32::LIGHT_GRAY))
                        .wrap(),
                );
            }
        });

    // The toolbar's choices.
    let mut reconnect = false;
    if monitor != wanted.monitor {
        let mut state = lock(shared);
        let next = Wanted {
            monitor,
            ..wanted.clone()
        };
        if let Err(e) = ask(state_dir, &mut state, next) {
            state.ended = Some(e);
        }
        return;
    }
    {
        let mut state = lock(shared);
        if view_only && !state.view_only {
            let release = state.input.release_all();
            send(&mut state, release);
        }
        state.view_only = view_only;
        if quality != state.quality {
            state.quality = quality;
            send(
                &mut state,
                vec![InputPacket::QualityPreset { preset: quality }],
            );
        }
    }

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let mut picture_rect = None;
            if let Some(why) = &ended {
                ui.vertical_centered(|ui| {
                    ui.add_space(area.height() / 3.0);
                    ui.colored_label(egui::Color32::LIGHT_GRAY, why);
                    ui.add_space(8.0);
                    reconnect = ui.button("Reconnect").clicked();
                });
            } else if let Some(note) = &note {
                ui.centered_and_justified(|ui| ui.colored_label(egui::Color32::LIGHT_GRAY, note));
            } else if let Some(texture) = &texture {
                let [w, h] = texture.size();
                let rect = letterbox(area, w, h);
                picture_rect = Some(rect);
                let id = ui.id().with("computer-picture");
                // The picture takes the pointer and the keys, so egui's own
                // Tab, arrows and Escape go to the other computer.
                let response = ui.interact(rect, id, egui::Sense::click_and_drag());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Computer screen")
                });
                if response.clicked()
                    || response.drag_started()
                    || !ui.memory(|m| m.focused().is_some())
                {
                    response.request_focus();
                }
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        id,
                        egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: true,
                        },
                    )
                });
                ui.painter().image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ui.centered_and_justified(|ui| {
                    ui.colored_label(
                        egui::Color32::LIGHT_GRAY,
                        shown_words(&wanted.name, &status),
                    )
                });
            }
            // This pass's input, to the other computer.
            let mut state = lock(shared);
            let current = match (&texture, state.size) {
                (Some(t), Some((w, h))) => t.size() == [usize::from(w), usize::from(h)],
                _ => false,
            };
            let on = controls(status.control, state.view_only, connected, current) && focused;
            let packets = match picture_rect {
                Some(rect) if on => {
                    let events = ctx.input(|i| i.events.clone());
                    state.input.take(&events, rect, Instant::now())
                }
                _ => state.input.release_all(),
            };
            send(&mut state, packets);
            if state.input.waiting() {
                ctx.request_repaint_after(MOVE_EVERY);
            }
        });
    if reconnect {
        let mut state = lock(shared);
        if let Err(e) = ask(state_dir, &mut state, wanted) {
            state.ended = Some(e);
        }
    }
}

/// Save the picture shown as a PNG in Pictures\\Pocketdesk, and copy it;
/// the file is written off the window's thread.
fn screenshot(shared: &Arc<Mutex<State>>, ctx: &egui::Context, name: &str) {
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
    let stem = format!("{} screen {}", file_safe(name), stamp(now_unix()));
    let shared = shared.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let note = crate::video::save_png(&stem, width, height, rgba);
        lock(&shared).saved_note = Some((note, Instant::now()));
        ctx.request_repaint_of(viewport_id());
    });
}

/// A computer's name fit for a file's: what a file system refuses becomes
/// a space.
pub fn file_safe(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim().trim_end_matches('.').to_string();
    if safe.is_empty() {
        "Computer".to_string()
    } else {
        safe
    }
}

/// The worker: find the view this window wants, show it until it ends,
/// again.
fn watch(state_dir: &Path, shared: &Arc<Mutex<State>>, ctx: &egui::Context) {
    loop {
        std::thread::sleep(POLL);
        let (wanted, newer_than, asked) = {
            let mut state = lock(shared);
            let Some(wanted) = state.wanted.clone() else {
                continue;
            };
            if state.ended.is_some() {
                continue;
            }
            let file = wire::read(state_dir)
                .filter(|f| f.device_id == wanted.device_id && f.session_id > state.newer_than);
            if file.is_none() {
                if state.asked_at.is_some_and(|at| at.elapsed() >= NO_ANSWER) {
                    state.ended = Some(
                        "The server did not answer: is Pocketdesk's server running on this computer?"
                            .to_string(),
                    );
                    ctx.request_repaint_of(viewport_id());
                }
                continue;
            }
            (wanted, state.newer_than, state.asked)
        };
        let Some(file) = wire::read(state_dir)
            .filter(|f| f.device_id == wanted.device_id && f.session_id > newer_than)
        else {
            continue;
        };
        let ended = view(&file, shared, ctx);
        end_recording(shared, Some("the screen stopped"));
        let mut state = lock(shared);
        // The relay dropped this window but the view goes on (its queue
        // was full at a new size, BK7 review M3): it is joined again, with
        // what is held let go meanwhile.
        // Only a connection cut once joined: one refused is a relay gone.
        let cut = matches!(&ended, Err(e) if matches!(
            e.kind(),
            io::ErrorKind::UnexpectedEof
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
        ));
        // Still the one this window wants (not closed, nor asked again or
        // for another screen meanwhile).
        let ours = state.asked == asked && state.wanted.as_ref() == Some(&wanted);
        let dropped =
            cut && ours && wire::read(state_dir).is_some_and(|f| f.session_id == file.session_id);
        // At most this often, then the view ends (BK7 re-review N3): a
        // relay that drops the window each time is not joined for ever.
        state.rejoins.retain(|at| at.elapsed() < REJOIN_WINDOW);
        let rejoin = dropped && state.rejoins.len() < REJOINS;
        if rejoin {
            state.rejoins.push(Instant::now());
            let release = state.input.release_all();
            send(&mut state, release);
            state.stream = None;
            state.writer = None;
            state.session_id = None;
            state.input = WindowInput::default();
            continue;
        }
        state.newer_than = state.newer_than.max(file.session_id);
        // Still the one this window wants: it says why it ended, a relay
        // that would not take it too.
        if ours {
            let why = match ended {
                _ if dropped => format!(
                    "The view of {} kept dropping this window. Reconnect to try again.",
                    wanted.name
                ),
                Ok(why) if !why.is_empty() => why,
                Ok(_) => format!("The view of {} ended.", wanted.name),
                Err(e) => {
                    format!("Lost the connection to this computer's Pocketdesk server ({e}).")
                }
            };
            state.ended = Some(why);
            let release = state.input.release_all();
            send(&mut state, release);
        }
        state.stream = None;
        state.writer = None;
        state.session_id = None;
        state.size = None;
        state.texture = None;
        state.picture = None;
        state.shown = None;
        state.input = WindowInput::default();
        ctx.request_repaint_of(viewport_id());
    }
}

/// Show one view until its end: why it ended, as the server said it.
fn view(file: &ViewFile, shared: &Arc<Mutex<State>>, ctx: &egui::Context) -> io::Result<String> {
    let token = file
        .token_bytes()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a bad token in view.json"))?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, file.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.write_all(&token)?;
    // A stuck write must not hold the window: input is best effort.
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    {
        let mut state = lock(shared);
        state.stream = Some(stream.try_clone()?);
        state.writer = Some(stream.try_clone()?);
        state.session_id = Some(file.session_id);
        state.asked_at = None;
    }
    let mut asks = stream.try_clone()?;
    let mut reader = BufReader::with_capacity(256 * 1024, stream);
    let mut decoder: Option<Decoder> = None;
    let mut gate = KeyGate::default();
    let mut recorder: Option<Recorder> = None;
    loop {
        let message = Message::read(&mut reader)?;
        let want = lock(shared).record_want
            && matches!(message, Message::Frame { .. } | Message::Heartbeat);
        let stop = !lock(shared).record_want;
        if want && recorder.is_none() {
            let dir = user_folder("Videos");
            let name = file_safe(&file.name);
            let path = free_name(&dir, &format!("{name} screen {}", stamp(now_unix())), "mkv");
            let started = std::fs::create_dir_all(&dir).and_then(|()| Recorder::start(&path));
            let mut state = lock(shared);
            match started {
                Ok(r) => {
                    recorder = Some(r);
                    state.recording = Some((path, Instant::now()));
                    Message::WantKeyframe.write(&mut asks)?;
                }
                Err(e) => {
                    state.record_want = false;
                    let why = if e.kind() == io::ErrorKind::NotFound {
                        format!("Not recording: {e} (it needs ffmpeg).")
                    } else {
                        format!("Not recording: {e}.")
                    };
                    state.saved_note = Some((why, Instant::now()));
                }
            }
        } else if stop && recorder.is_some() {
            recorder = None;
            end_recording(shared, None);
            ctx.request_repaint_of(viewport_id());
        }
        match message {
            Message::Status(status) => {
                let mut state = lock(shared);
                // The quality chosen goes once the computer has let it in.
                if status.phase == Phase::Showing && state.quality_sent != Some(file.session_id) {
                    state.quality_sent = Some(file.session_id);
                    let preset = state.quality;
                    send(&mut state, vec![InputPacket::QualityPreset { preset }]);
                }
                state.status = status;
                ctx.request_repaint_of(viewport_id());
            }
            Message::Config { width, height } => {
                // Another size: the decoder starts again at it, from the
                // next keyframe.
                decoder = None;
                gate = KeyGate::default();
                if recorder.take().is_some() {
                    end_recording(shared, Some("it stopped as the screen changed size"));
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
                        state.note =
                            Some(crate::video::decoder_words(&e, "another computer's screen"));
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
            Message::End { why } => return Ok(why),
            Message::Heartbeat | Message::WantKeyframe | Message::Input(_) => {}
        }
    }
}

/// A recording ended: the window says where it went, and why it stopped.
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
    state.saved_note = Some((note, Instant::now()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_says_how_the_view_stands() {
        let mut status = ViewStatus::default();
        assert_eq!(waiting_words("Desk", &status), "Reaching Desk\u{2026}");
        status.phase = Phase::Waiting;
        assert!(waiting_words("Desk", &status).contains("let this computer in"));
        status.phase = Phase::Showing;
        assert_eq!(
            waiting_words("Desk", &status),
            "Waiting for Desk's screen\u{2026}"
        );
    }

    /// BK7 review M6: a note from the server takes the waiting words'
    /// place.
    #[test]
    fn the_servers_note_is_said_in_place_of_waiting() {
        let mut status = ViewStatus {
            phase: Phase::Showing,
            ..ViewStatus::default()
        };
        assert_eq!(shown_words("Desk", &status), waiting_words("Desk", &status));
        status.note = Some("Desk's screen comes in frames too large to show here.".into());
        assert_eq!(
            shown_words("Desk", &status),
            "Desk's screen comes in frames too large to show here."
        );
    }

    /// View-only when the other computer refuses input says where control
    /// is turned on; the window's own switch says it sends nothing.
    #[test]
    fn view_only_says_why_and_where_control_is_turned_on() {
        let refused = control_words("Desk", Some(false), false).unwrap();
        assert!(refused.contains("Devices page") && refused.contains("Desk"));
        assert!(
            control_words("Desk", Some(false), true)
                .unwrap()
                .contains("Devices page")
        );
        assert_eq!(
            control_words("Desk", Some(true), true).unwrap(),
            "View only: this window sends nothing."
        );
        assert_eq!(control_words("Desk", Some(true), false), None);
        assert_eq!(control_words("Desk", None, false), None);
    }

    #[test]
    fn input_goes_only_with_control_taken_and_the_picture_current() {
        assert!(controls(Some(true), false, true, true));
        assert!(
            !controls(Some(false), false, true, true),
            "the host refuses input"
        );
        assert!(!controls(None, false, true, true), "not said yet");
        assert!(!controls(Some(true), true, true, true), "view only here");
        assert!(!controls(Some(true), false, false, true), "no relay");
        assert!(
            !controls(Some(true), false, true, false),
            "a picture of the old size would put a click in the wrong place"
        );
    }

    #[test]
    fn the_screens_are_named_or_numbered() {
        let mut status = ViewStatus::default();
        assert_eq!(screen_choices(&status), ["Screen 1"]);
        status.monitor = 1;
        assert_eq!(screen_choices(&status), ["Screen 1", "Screen 2"]);
        status.monitors = vec!["eDP-1 (1920x1080)".into(), "HDMI-A-1 (2560x1440)".into()];
        assert_eq!(screen_choices(&status).len(), 2);
    }

    #[test]
    fn a_name_is_fit_for_a_file() {
        assert_eq!(file_safe("Desk"), "Desk");
        assert_eq!(file_safe("a/b:c"), "a b c");
        assert_eq!(file_safe("..."), "Computer");
    }

    /// BK7 review M3: a relay that drops the window (its queue was full at a
    /// new size) while `view.json` still names the view is joined again,
    /// not taken for the view's end.
    #[test]
    fn a_window_dropped_by_the_relay_joins_the_view_again() {
        let dir = std::env::temp_dir().join(format!("pd-computer-rejoin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        struct Remove(PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _remove = Remove(dir.clone());
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = [4u8; pocketdesk_core::local::cast::TOKEN_LEN];
        wire::write(
            &dir,
            &ViewFile {
                session_id: 20,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone());
        view.open("desk", "Desk", 0, 0, false).unwrap();
        listener.set_nonblocking(true).unwrap();
        let join = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut relay = loop {
                match listener.accept() {
                    Ok((relay, _)) => break relay,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "joined within 10 s");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            relay.set_nonblocking(false).unwrap();
            relay
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut given = [0u8; 32];
            std::io::Read::read_exact(&mut relay, &mut given).unwrap();
            assert_eq!(given, token);
            relay
        };
        let first = join();
        drop(first);
        // Joined again, and nothing said ended meanwhile.
        let mut second = join();
        assert_eq!(lock(&view.shared).ended, None);
        Message::End {
            why: "Lost the connection to Desk.".into(),
        }
        .write(&mut second)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            lock(&view.shared).ended.as_deref(),
            Some("Lost the connection to Desk.")
        );

        // BK7 re-review N3: a relay that drops the window each time it
        // joins is joined at most three times in ten seconds, then the view
        // ends, saying so.
        wire::write(
            &dir,
            &ViewFile {
                session_id: 21,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        view.open("desk", "Desk", 0, 0, false).unwrap();
        for _ in 0..4 {
            drop(join());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            lock(&view.shared)
                .ended
                .as_deref()
                .is_some_and(|why| why.contains("kept dropping")),
            "{:?}",
            lock(&view.shared).ended
        );
        std::thread::sleep(Duration::from_millis(1000));
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "not joined a fifth time"
        );
    }

    /// A relay that is gone (nothing at its port) ends the view with why,
    /// rather than being tried for ever.
    #[test]
    fn a_relay_gone_ends_the_view() {
        let dir = std::env::temp_dir().join(format!("pd-computer-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let port = {
            let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.local_addr().unwrap().port()
        };
        wire::write(
            &dir,
            &ViewFile {
                session_id: 30,
                device_id: "desk".into(),
                name: "Desk".into(),
                port,
                token: pocketdesk_core::local::cast::token_hex(&[5u8; 32]),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone());
        view.open("desk", "Desk", 0, 0, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(lock(&view.shared).ended.as_deref().is_some_and(|why| {
            why.starts_with("Lost the connection to this computer's Pocketdesk server")
        }),);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The worker against a stand-in relay: it finds the view the window
    /// asked for, decodes its frames to pictures of the size the relay
    /// says, sends the chosen quality once the view is shown, and keeps the
    /// reason it ended. Needs ffmpeg with libx264; skipped without.
    #[test]
    fn the_worker_shows_a_view_and_keeps_why_it_ended() {
        let encoded = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=64x48:rate=10",
                "-frames:v",
                "3",
                "-c:v",
                "libx264",
                "-bf",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "h264",
                "pipe:1",
            ])
            .stderr(std::process::Stdio::null())
            .output();
        let Some(encoded) = encoded
            .ok()
            .filter(|o| o.status.success() && !o.stdout.is_empty())
        else {
            eprintln!("no ffmpeg with libx264 here: skipped");
            return;
        };
        let dir = std::env::temp_dir().join(format!("pd-computer-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let token = [3u8; pocketdesk_core::local::cast::TOKEN_LEN];
        wire::write(
            &dir,
            &ViewFile {
                session_id: 10,
                device_id: "desk".into(),
                name: "Desk".into(),
                port: listener.local_addr().unwrap().port(),
                token: pocketdesk_core::local::cast::token_hex(&token),
            },
        )
        .unwrap();
        let ctx = egui::Context::default();
        let view = ComputerView::new(&ctx, dir.clone());
        view.open("desk", "Desk", 0, 3, false).unwrap();
        let asked = computers::take_all(&dir, now_unix());
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::View {
                device_id: "desk".into(),
                monitor: 0
            }]
        );
        let (mut relay, _) = listener.accept().unwrap();
        relay
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut given = [0u8; 32];
        std::io::Read::read_exact(&mut relay, &mut given).unwrap();
        assert_eq!(given, token);
        Message::Status(ViewStatus {
            phase: Phase::Showing,
            control: Some(true),
            ..ViewStatus::default()
        })
        .write(&mut relay)
        .unwrap();
        Message::Config {
            width: 64,
            height: 48,
        }
        .write(&mut relay)
        .unwrap();
        Message::Frame {
            keyframe: true,
            seq: 0,
            data: encoded.stdout.clone(),
        }
        .write(&mut relay)
        .unwrap();
        // The quality chosen (3, Economy) goes once shown.
        match Message::read(&mut relay).unwrap() {
            Message::Input(bytes) => assert!(matches!(
                InputPacket::decode(&bytes).unwrap(),
                InputPacket::QualityPreset { preset: 3 }
            )),
            other => panic!("{other:?}"),
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let size = lock(&view.shared)
                .picture
                .as_ref()
                .map(|p| (p.width, p.height));
            if let Some(size) = size {
                assert_eq!(size, (64, 48));
                break;
            }
            assert!(Instant::now() < deadline, "a picture within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        Message::End {
            why: "Lost the connection to Desk.".into(),
        }
        .write(&mut relay)
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lock(&view.shared).ended.is_none() {
            assert!(Instant::now() < deadline, "the end within 10 s");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            lock(&view.shared).ended.as_deref(),
            Some("Lost the connection to Desk.")
        );
        assert!(
            lock(&view.shared).wanted.is_some(),
            "the window stays open, saying why, until closed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
