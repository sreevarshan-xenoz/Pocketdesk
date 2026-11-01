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
//!
//! Phone apps tier 2: an app opened in a window of its own is shown by a
//! second `CastView` of the same kind, one per window id, reading that
//! window's file (`cast-window-<n>.json`); its window carries the app's
//! name, takes touch and typing only as the phone says that window does, and
//! closing it asks the phone to close the app's window and give its display
//! back.

use std::io::{self, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use pocketdesk_core::local::cast::{self as wire, CastFile, Message};
use pocketdesk_core::streaming::cast_control::{ControlBatch, ControlEvent, Global, TouchAction};

use crate::video::{
    Admit, Decoder, KeyGate, Picture, Recorder, fraction, free_name, letterbox, now_unix, stamp,
    user_folder,
};

/// The window's id among the app's viewports: the phone's screen, or app
/// window `n`.
fn viewport_id(window: Option<u8>) -> egui::ViewportId {
    match window {
        None => egui::ViewportId::from_hash_of("pocketdesk-cast"),
        Some(n) => egui::ViewportId::from_hash_of(("pocketdesk-cast-window", n)),
    }
}

/// Repaint the window `shared` belongs to.
fn repaint(shared: &Mutex<State>, ctx: &egui::Context) {
    let window = lock(shared).window;
    ctx.request_repaint_of(viewport_id(window));
}

/// What tells one cast from the next in a window's state: the session for
/// the phone's screen, and for an app window its token, which is made again
/// each time a window opens (the session outlives its windows).
fn identity(file: &CastFile) -> u64 {
    match (file.window, file.token_bytes()) {
        (Some(_), Some(token)) if token.len() >= 8 => {
            u64::from_le_bytes(token[..8].try_into().unwrap_or([0; 8]))
        }
        _ => file.session_id,
    }
}

/// How often the worker looks for a cast.
const POLL: Duration = Duration::from_millis(500);
/// The relay sends a heartbeat every 2 s; silence past this is a dead relay.
const READ_TIMEOUT: Duration = Duration::from_secs(6);
/// The window's long side when it opens, in points.
const LONG_SIDE: f32 = 800.0;

#[derive(Default)]
struct State {
    /// The app window this state is for (Phone apps tier 2); none for the
    /// phone's screen. Set once and kept.
    window: Option<u8>,
    /// The cast being shown (see [`identity`]), and its phone.
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
    /// Keys to taps for this app window (Phone apps tier 2); none for the
    /// phone's own screen window and for a window with no known app.
    keys: Option<crate::keymap_view::Keys>,
    /// Where the last touch went, for a release the window did not see.
    last_touch: (u16, u16),
    /// What a drop on the window did, and when (L2).
    drop_note: Option<(String, std::time::Instant)>,
    /// A share asked of the phone for a dropped file (Apps part 2): the
    /// phone, the file's name and when it was asked (Unix seconds), until
    /// the phone says how it went.
    share_wait: Option<(String, String, u64)>,
    /// When the phone's word on a share was last looked for.
    share_looked: Option<std::time::Instant>,
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

/// How long the window waits for the phone's word on a share.
const SHARE_WAIT_SECS: u64 = 90;

/// How often the phone's word on a share is looked for.
const SHARE_LOOK: std::time::Duration = std::time::Duration::from_secs(1);

/// What the phone's word on a share says (Apps part 2): `app` is the app it
/// went to or was tried on, as the phone listed it, if the phone said one.
pub fn share_words(
    result: pocketdesk_core::protocol::phone_action::AppResult,
    app: Option<&str>,
    name: &str,
) -> String {
    use pocketdesk_core::protocol::phone_action::AppResult as R;
    let front = app.unwrap_or("The app in front");
    match result {
        R::Shared => format!("Shared {name} into {}.", app.unwrap_or("the app in front")),
        R::NoApp => format!(
            "{name} is in Downloads on the phone: Pocketdesk could not tell which app is in front (or it is not one the phone lists)."
        ),
        R::CannotTake => {
            format!("{front} takes no file like {name}, so it is in Downloads on the phone.")
        }
        R::Off => format!(
            "{name} was not shared: \u{201c}Let the computer open my apps\u{201d} is off on the phone. The file itself is in Downloads."
        ),
        R::ControlOff => format!(
            "{name} was not shared: Pocketdesk control is off on the phone. The file itself is in Downloads."
        ),
        R::Locked | R::LockedSecure => {
            format!("{name} was not shared: the phone is locked. The file itself is in Downloads.")
        }
        R::Asleep => format!(
            "{name} was not shared: the phone's screen is off. The file itself is in Downloads."
        ),
        R::NeedsScreen => format!(
            "{name} was not shared: the phone's screen is not shown to this computer. The file itself is in Downloads."
        ),
        R::TooMany => format!(
            "{name} was not shared: the phone was asked too often. The file itself is in Downloads."
        ),
        _ => {
            format!("Android would not start the share of {name}. The file itself is in Downloads.")
        }
    }
}

/// Look for the phone's word on the share asked for the last drop, once a
/// second, and put it where the drop's line was (Apps part 2).
fn watch_share(shared: &Mutex<State>, ctx: &egui::Context, state_dir: &std::path::Path) {
    let Some((device, name, asked_at)) = lock(shared).share_wait.clone() else {
        return;
    };
    {
        let mut state = lock(shared);
        if state
            .share_looked
            .is_some_and(|at| at.elapsed() < SHARE_LOOK)
        {
            ctx.request_repaint_after(SHARE_LOOK);
            return;
        }
        state.share_looked = Some(std::time::Instant::now());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let word = pocketdesk_core::local::phone_apps::read_results(state_dir)
        .remove(&device)
        .filter(|o| o.answers("share", asked_at) && o.report.name.as_deref() == Some(&name));
    let said = match word {
        Some(outcome) => {
            let label = outcome.report.app.as_deref().map(|id| {
                pocketdesk_core::local::phone_apps::read(state_dir)
                    .remove(&device)
                    .and_then(|l| l.apps.into_iter().find(|a| a.id == id))
                    .map_or_else(|| id.to_string(), |a| a.name)
            });
            Some(share_words(outcome.report.result, label.as_deref(), &name))
        }
        None if now.saturating_sub(asked_at) > SHARE_WAIT_SECS => Some(format!(
            "The phone said nothing about sharing {name}; the file itself is in Downloads if it arrived."
        )),
        None => None,
    };
    let mut state = lock(shared);
    match said {
        Some(said) => {
            state.share_wait = None;
            state.drop_note = Some((said, std::time::Instant::now()));
            ctx.request_repaint();
        }
        None => ctx.request_repaint_after(SHARE_LOOK),
    }
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

/// The cast window and its worker.
pub struct CastView {
    shared: Arc<Mutex<State>>,
    /// Where drops are queued (L2).
    state_dir: PathBuf,
    /// The app window this shows (Phone apps tier 2); none for the screen.
    window: Option<u8>,
}

impl CastView {
    pub fn new(ctx: &egui::Context, state_dir: PathBuf) -> Self {
        Self::spawn(ctx, state_dir, None, None)
    }

    /// The viewer of app window `window` (Phone apps tier 2), for the phone's
    /// app `package` when it is known (it has the window's keymap).
    pub fn for_window(
        ctx: &egui::Context,
        state_dir: PathBuf,
        window: u8,
        package: Option<&str>,
    ) -> Self {
        Self::spawn(ctx, state_dir, Some(window), package)
    }

    fn spawn(
        ctx: &egui::Context,
        state_dir: PathBuf,
        window: Option<u8>,
        package: Option<&str>,
    ) -> Self {
        let keys = window
            .and(package)
            .filter(|p| pocketdesk_core::local::keymap::package_ok(p))
            .map(|p| crate::keymap_view::Keys::new(p, state_dir.clone()));
        let shared = Arc::new(Mutex::new(State {
            window,
            keys,
            ..State::default()
        }));
        let worker = shared.clone();
        let ctx = ctx.clone();
        let watched = state_dir.clone();
        let spawned = std::thread::Builder::new()
            .name("cast-view".into())
            .spawn(move || watch(&watched, &worker, &ctx));
        // Without the thread no cast is ever shown, and nothing else breaks.
        drop(spawned);
        Self {
            shared,
            state_dir,
            window,
        }
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

    /// Bring the screen window to the front (Apps part 1: an app opened on
    /// the phone): a closed one is shown again, and one that is open is
    /// focused. Nothing happens while there is no cast to show.
    pub fn raise(&self, ctx: &egui::Context) {
        {
            let mut state = lock(&self.shared);
            if state.session_id.is_none() {
                return;
            }
            state.dismissed = None;
        }
        ctx.send_viewport_cmd_to(viewport_id(self.window), egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// Call each pass: keeps the window open while there is a cast to show.
    /// `control` is the owner's Settings ("Control phones from their screen
    /// window", C8); `targets`, the phones files reach now (L2); `share`,
    /// Settings' "Share a dropped file into the phone's app in front"
    /// (Apps part 2).
    pub fn show(
        &self,
        ctx: &egui::Context,
        devices: &[crate::model::Device],
        control: bool,
        targets: Vec<String>,
        share: bool,
        keymaps: bool,
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
        ctx.show_viewport_deferred(viewport_id(self.window), builder, move |ui, _class| {
            take_drops(&shared, ui.ctx(), &state_dir, &targets, share);
            watch_share(&shared, ui.ctx(), &state_dir);
            draw(&shared, ui, control, keymaps);
        });
    }

    /// An app window's window, while its cast is there (Phone apps tier 2):
    /// titled with the app's `name` and the phone's. Files dropped on it
    /// are not taken (it is not the phone's screen), and touch and typing
    /// go only as far as the phone says this window takes them.
    pub fn show_window(
        &self,
        ctx: &egui::Context,
        devices: &[crate::model::Device],
        control: bool,
        name: &str,
        keymaps: bool,
    ) {
        let (title, size) = {
            let state = lock(&self.shared);
            let Some(id) = state.session_id else { return };
            if state.dismissed == Some(id) {
                return;
            }
            let phone = devices
                .iter()
                .find(|d| d.id == state.device_id)
                .and_then(|d| d.name.clone())
                .unwrap_or_else(|| short(&state.device_id));
            (format!("{name} \u{b7} {phone}"), state.size)
        };
        let (w, h) = window_size(size);
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([w, h])
            .with_min_inner_size([160.0, 160.0]);
        let shared = self.shared.clone();
        ctx.show_viewport_deferred(viewport_id(self.window), builder, move |ui, _class| {
            draw(&shared, ui, control, keymaps);
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

/// Files or folders dropped on the window go to its phone (L2), as the Files
/// page sends them, and the window says how that went.
fn take_drops(
    shared: &Mutex<State>,
    ctx: &egui::Context,
    state_dir: &std::path::Path,
    targets: &[String],
    share: bool,
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
    // Apps part 2: one file dropped, and the phone asked to share it into
    // the app in front (it says whether that app takes it).
    let mut sharing = None;
    if reaches {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for path in &dropped {
            match crate::model::queue_send(state_dir, &device, path, now) {
                Ok(name) if share && files == 1 && folders == 0 => {
                    let size = std::fs::metadata(path).map_or(0, |m| m.len());
                    if pocketdesk_core::local::phone_actions::request_share(
                        state_dir, &device, &name, size, now,
                    )
                    .is_ok()
                    {
                        sharing = Some((name, now));
                    }
                }
                Ok(_) => {}
                Err(e) => failed.push(e),
            }
        }
    }
    let mut words = drop_words(reaches, files, folders, &failed);
    if let Some((name, now)) = sharing {
        words = format!(
            "Sending {name} to the phone; if the app in front there takes it, it is shared into it, else it stays in Downloads."
        );
        lock(shared).share_wait = Some((device.clone(), name, now));
    }
    lock(shared).drop_note = Some((words, std::time::Instant::now()));
    ctx.request_repaint_after(DROP_NOTE);
}

fn draw(shared: &Arc<Mutex<State>>, ui: &mut egui::Ui, control: bool, keymaps: bool) {
    let ctx = ui.ctx().clone();
    let mut state = lock(shared);
    if ctx.input(|i| i.viewport().close_requested()) {
        state.dismissed = state.session_id;
        // Fingers held for a game are lifted before the window goes, and
        // the pointer lock for aiming goes with it.
        let lifts = state
            .keys
            .as_mut()
            .map(|keys| {
                let lifts = keys.release();
                keys.close(&ctx);
                lifts
            })
            .unwrap_or_default();
        if !lifts.is_empty()
            && let Some(mut writer) = state.writer.as_ref().and_then(|w| w.try_clone().ok())
        {
            let _ = send_control(&mut writer, lifts);
        }
        // An app window closed here is closed on the phone too, and its
        // display given back (best effort: the relay closes one nobody
        // shows after a while anyway).
        if state.window.is_some()
            && let Some(mut writer) = state.writer.take()
        {
            let _ = Message::CloseWindow.write(&mut writer);
        }
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
            &picture.data,
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
    let window = state.window;
    let controlling = control && phone_takes && !state.watch_only;
    let mut watch_only = state.watch_only;
    let recording = state.recording.clone();
    let record_want = state.record_want;
    let saved = state
        .saved_note
        .clone()
        .filter(|(_, at)| at.elapsed() < SAVED_NOTE)
        .map(|(note, _)| note);
    // Keys to taps (an app window's own state, put back below). Off in
    // Settings, they are off here too.
    let mut keys = state.keys.take();
    if !keymaps
        && let Some(k) = keys.as_mut()
        && (k.on || k.editing)
    {
        k.on = false;
        k.set_editing(false);
    }
    // While the pointer is locked for aiming the bar is not clicked.
    let keys_locked = keys.as_ref().is_some_and(|k| k.lock.locked());
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
                    // Read once a second (L5 review: not on every pass).
                    let size = ui.ctx().memory_mut(|m| {
                        let id = egui::Id::new("cast-recording-size");
                        let (at, size): (u64, u64) = m.data.get_temp(id).unwrap_or((u64::MAX, 0));
                        let now = since.elapsed().as_secs();
                        if at == now {
                            size
                        } else {
                            let size = std::fs::metadata(path).map_or(0, |m| m.len());
                            m.data.insert_temp(id, (now, size));
                            size
                        }
                    });
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
                        ui.add_enabled_ui(!watch_only && !keys_locked, |ui| {
                            // An app window has no Home or Recents: only
                            // the phone's screen does.
                            let globals: &[(&str, Global)] = if window.is_some() {
                                &[("Back", Global::Back)]
                            } else {
                                &[
                                    ("Back", Global::Back),
                                    ("Home", Global::Home),
                                    ("Recents", Global::Recents),
                                ]
                            };
                            for &(label, global) in globals {
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
                        // Keys to taps: an app window only (the phone's own
                        // screen takes accessibility gestures, too slow for
                        // a game, and needs no Shizuku).
                        if keymaps {
                            match keys.as_mut() {
                                Some(k) => {
                                    ui.add_enabled_ui(!watch_only && !keys_locked, |ui| k.bar(ui));
                                }
                                None => {
                                    ui.colored_label(
                                        egui::Color32::LIGHT_GRAY,
                                        if window.is_some() {
                                            "Keys to taps: this window's app is not known here, so it has no keymap."
                                        } else {
                                            "Keys to taps works in an app window (Shizuku on the phone), not in the phone's own screen."
                                        },
                                    );
                                }
                            }
                        }
                    } else {
                        ui.colored_label(
                            egui::Color32::LIGHT_GRAY,
                            if window.is_some() {
                                "This window shows the app only: touch and typing go to it when \u{201c}Separate windows (Shizuku)\u{201d} is on in Pocketdesk's Settings on the phone."
                            } else {
                                "To control the phone from here, turn on \u{201c}Let the computer control this phone\u{201d} in Pocketdesk's Settings on the phone."
                            },
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
    if keymaps
        && let Some(k) = keys.as_mut()
        && k.editing
    {
        egui::Panel::right("cast-keys")
            .resizable(false)
            .exact_size(270.0)
            .frame(
                egui::Frame::NONE
                    .fill(egui::Color32::from_gray(24))
                    .inner_margin(8.0),
            )
            .show(ui, |ui| {
                ui.style_mut().visuals = egui::Visuals::dark();
                egui::ScrollArea::vertical().show(ui, |ui| k.edit_panel(ui));
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let mut picture_rect = None;
            let mut picture_response = None;
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
                    let response = ui.interact(
                        rect,
                        ui.id().with("cast-picture"),
                        egui::Sense::click_and_drag(),
                    );
                    // Named for a screen reader (E5).
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Phone screen")
                    });
                    picture_response = Some(response);
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
            // Keys to taps: the editor takes the picture's clicks, and the
            // keys act only on a window with the focus, a picture and control.
            let editing = keymaps && keys.as_ref().is_some_and(|k| k.editing);
            let keys_can = keymaps && controlling && !watch_only && current;
            let focused = ctx.input(|i| i.viewport().focused == Some(true));
            if let Some(k) = keys.as_mut().filter(|_| keymaps) {
                k.follow(&ctx, picture_rect, area, focused, keys_can);
                if k.editing
                    && let Some(rect) = picture_rect
                {
                    let typed = ctx.input(|i| i.events.clone());
                    k.editor_keys(&typed);
                    if let Some(response) = &picture_response {
                        k.editor_pointer(response, rect);
                    }
                    k.overlay(ui.painter(), rect);
                    // Written once the mouse is up, not for each step of a drag.
                    if !ctx.input(|i| i.pointer.any_down()) {
                        k.save();
                    }
                }
            }
            let keys_live = keys_can
                && focused
                && picture_rect.is_some()
                && keys.as_ref().is_some_and(|k| k.acting());
            let mut fingers = Vec::new();
            if controlling && !watch_only && current && !editing {
                events = buttons;
                if let Some(rect) = picture_rect {
                    let (input, hover, down) = ctx.input(|i| {
                        (
                            i.events.clone(),
                            i.pointer.hover_pos(),
                            i.pointer.primary_down(),
                        )
                    });
                    // The keymap takes the keys it is drawn for (and, while
                    // the pointer is locked, the mouse); the rest goes on.
                    let input = match keys.as_mut().filter(|_| keys_live) {
                        Some(k) => {
                            let t = k.translate(&input, rect, std::time::Instant::now());
                            fingers.extend(t.fingers);
                            t.rest
                        }
                        None => input,
                    };
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
            // Swipes go on and a still aim finger lifts with time; and when
            // the keys stop acting (focus lost, control or the keys off, the
            // editor open) every finger lifts.
            if let Some(k) = keys.as_mut() {
                if keys_live {
                    let now = std::time::Instant::now();
                    fingers.extend(k.tick(now));
                    k.pace(&mut fingers, now);
                    if k.busy() {
                        ctx.request_repaint_after(Duration::from_millis(8));
                    }
                } else {
                    fingers.extend(k.release());
                }
            }
            // Written outside the window's lock, so a slow relay holds
            // nothing else; the first broken write ends control here.
            let writer = (!events.is_empty() || !fingers.is_empty())
                .then(|| state.writer.as_ref().and_then(|w| w.try_clone().ok()))
                .flatten();
            drop(state);
            // Fingers go in batches of their own: a phone older than keys
            // to taps drops a batch it cannot read, and then loses only these.
            // Both are always tried: the fingers carry the lifts, which are tried
            // even when the first write failed.
            if let Some(mut writer) = writer {
                let sent = send_control(&mut writer, events);
                let lifted = send_control(&mut writer, fingers);
                if !(sent && lifted) {
                    lock(shared).writer = None;
                }
            }
        });
    lock(shared).keys = keys;
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
        .map(|p| (p.width, p.height, p.data.clone()))
    else {
        return;
    };
    ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &rgba,
    ));
    let stem = format!("Phone screen {}", stamp(now_unix()));
    let shared = shared.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let note = crate::video::save_png(&stem, width, height, rgba);
        lock(&shared).saved_note = Some((note, std::time::Instant::now()));
        repaint(&shared, &ctx);
    });
}

/// The worker: find the cast, show it until it ends, again.
fn watch(state_dir: &std::path::Path, shared: &Arc<Mutex<State>>, ctx: &egui::Context) {
    loop {
        std::thread::sleep(POLL);
        let window = lock(shared).window;
        let found = match window {
            None => wire::read(state_dir),
            Some(n) => wire::read_window(state_dir, n),
        };
        let Some(file) = found else {
            forget(shared, ctx);
            continue;
        };
        let who = identity(&file);
        let dismissed = {
            let mut state = lock(shared);
            if state.session_id != Some(who) {
                *state = State {
                    window: state.window,
                    session_id: Some(who),
                    device_id: file.device_id.clone(),
                    dismissed: state.dismissed,
                    ..State::default()
                };
                ctx.request_repaint();
            }
            state.dismissed == Some(who)
        };
        if dismissed {
            continue;
        }
        // An error is the relay gone or the window closed: the file says
        // which on the next look.
        let _ = view(&file, shared, ctx);
        // A recording ends with its cast, however it ended (L5 review).
        end_recording(shared, Some("the phone's screen stopped"));
        repaint(shared, ctx);
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
            window: state.window,
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
            let started = std::fs::create_dir_all(&dir)
                .and_then(|()| Recorder::start(&path, crate::video::Codec::H264));
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
            repaint(shared, ctx);
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
                        state.note = Some(crate::video::decoder_words(&e, "a phone's screen"));
                    }
                }
                repaint(shared, ctx);
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
                    repaint(shared, ctx);
                }
            }
            Message::ControlState { on } => {
                lock(shared).control_on = on;
                repaint(shared, ctx);
            }
            Message::Heartbeat
            | Message::WantKeyframe
            | Message::Control(_)
            | Message::CloseWindow => {}
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
        repaint(&shared, &ctx);
    })
}

#[cfg(test)]
mod tests {
    /// Apps part 2: the phone's word on a share says where the file went or why not.
    #[test]
    fn a_share_says_where_the_file_went() {
        use pocketdesk_core::protocol::phone_action::AppResult as R;
        assert_eq!(
            share_words(R::Shared, Some("Notes"), "a.png"),
            "Shared a.png into Notes."
        );
        assert!(share_words(R::NoApp, None, "a.png").contains("in Downloads"));
        let cannot = share_words(R::CannotTake, Some("Calculator"), "a.png");
        assert!(cannot.starts_with("Calculator takes no file like a.png"));
        assert!(share_words(R::CannotTake, None, "a.png").starts_with("The app in front takes no"));
        for refused in [
            R::Off,
            R::ControlOff,
            R::LockedSecure,
            R::Asleep,
            R::NeedsScreen,
            R::TooMany,
            R::Failed,
        ] {
            let said = share_words(refused, None, "a.png");
            assert!(said.contains("Downloads"), "{refused:?}: {said}");
            assert!(!said.contains("Shared a.png"), "{refused:?}");
        }
    }

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

    use super::*;

    fn picture() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(200.0, 400.0))
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
    fn a_long_phone_id_is_cut_in_the_title() {
        assert_eq!(short(""), "Phone");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short("0123456789abcdef"), "0123456789ab…");
    }
}
