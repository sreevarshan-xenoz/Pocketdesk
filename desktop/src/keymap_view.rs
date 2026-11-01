//! Keys to taps in an app window of the phone: the keymap editor, the
//! overlay of its points, and the keys' and the locked mouse's way to the
//! phone's several fingers.
//!
//! The model, its file and the key to finger state machine are
//! `pocketdesk_core::local::keymap`; this is the window's side of them. Each
//! app window has one [`Keys`] (the phone's own screen window has none: its
//! input goes through accessibility gestures, too slow for a game, and a
//! phone without Shizuku has no app windows). While "Keys to taps" is on and
//! the window is not being edited, the keys and mouse buttons the keymap
//! listens to are kept from the phone as typing, and become fingers
//! ([`translate`]); while the pointer is locked ([`PointerLock`], PC7's lock
//! and chords) its raw movement drags the aim finger. Everything lifts as the
//! window loses the focus, the lock or the keys go off, or control does.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;
use pocketdesk_core::local::keymap::{
    self, Binding, Keymap, MOUSE_LEFT, MOUSE_MIDDLE, MOUSE_RIGHT, Point, RADIUS_RANGE, Runner,
    SENSITIVITY_RANGE, SWIPE_MS_RANGE, token_label,
};
use pocketdesk_core::streaming::cast_control::ControlEvent;

use crate::video::fraction;
use crate::view_pointer::{self, PointerLock, ReleaseChord};

/// What the window says about online games.
pub const WARNING: &str =
    "Some online games flag injected input as cheating. Use keys to taps where that is fine.";

/// What the owner may add to a keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Tap,
    Stick,
    Swipe,
    Aim,
}

/// What the editor waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// A click on the picture (a drag, for a swipe) to place a tool.
    Place(Tool),
    /// A key for a tap just placed.
    TapKey(Point),
    /// A key for a swipe just drawn.
    SwipeKey(Point, Point),
    /// A key for slot `slot` of binding `index`.
    Rebind { index: usize, slot: usize },
}

/// A grab on a marker being dragged: the binding, and whether its end.
type Grab = (usize, bool);

/// One app window's keys to taps.
pub struct Keys {
    package: String,
    dir: PathBuf,
    map: Keymap,
    /// "Keys to taps" is on for this window.
    pub on: bool,
    /// The editor is open: the picture takes clicks to place and move marks.
    pub editing: bool,
    runner: Runner,
    /// The pointer lock, for aiming.
    pub lock: PointerLock,
    chord: ReleaseChord,
    pending: Option<Pending>,
    /// What the editor last said (an error, or what to do next).
    note: Option<String>,
    dirty: bool,
    grab: Option<Grab>,
    swipe_from: Option<Point>,
    /// When fingers were last sent, for the keep-alive.
    last_sent: Instant,
}

/// How often fingers held down are said to be held: the phone lifts any it
/// has heard nothing about for two seconds.
pub const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_millis(500);

/// How near a marker a press grabs it, in points.
const GRAB: f32 = 18.0;

impl Keys {
    /// The keys of `package`'s window, with the keymap saved for it.
    pub fn new(package: &str, dir: PathBuf) -> Self {
        let map = keymap::load(&dir, package).unwrap_or_default();
        Keys {
            package: package.to_string(),
            dir,
            runner: Runner::new(map.clone()),
            map,
            on: false,
            editing: false,
            lock: PointerLock::default(),
            chord: ReleaseChord::default(),
            pending: None,
            note: None,
            dirty: false,
            grab: None,
            swipe_from: None,
            last_sent: Instant::now(),
        }
    }

    /// Say that fingers held down are still held: when `fingers` has
    /// something it is that word; otherwise, after [`KEEP_ALIVE`] without
    /// one, each held finger's move to where it is.
    pub fn pace(&mut self, fingers: &mut Vec<ControlEvent>, now: Instant) {
        if !fingers.is_empty() {
            self.last_sent = now;
        } else if self.runner.busy() && now.saturating_duration_since(self.last_sent) >= KEEP_ALIVE
        {
            fingers.extend(self.runner.keep_alive());
            self.last_sent = now;
        }
    }

    /// The keymap as it is now.
    #[cfg(test)]
    pub fn map(&self) -> &Keymap {
        &self.map
    }

    /// What the editor waits for, for tests and the hint.
    #[cfg(test)]
    pub fn pending(&self) -> Option<Pending> {
        self.pending
    }

    /// What the editor last said.
    #[cfg(test)]
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Whether keys act as fingers now: on and not being edited.
    pub fn acting(&self) -> bool {
        self.on && !self.editing
    }

    /// Switch the editor; closing it puts the keymap to work and saves it.
    pub fn set_editing(&mut self, editing: bool) {
        if self.editing == editing {
            return;
        }
        self.editing = editing;
        self.pending = None;
        self.grab = None;
        self.swipe_from = None;
        self.note = None;
        if !editing {
            self.save();
            let _ = self.runner.set_map(self.map.clone());
        }
    }

    /// Lift every finger (and the pointer lock goes with the window's).
    pub fn release(&mut self) -> Vec<ControlEvent> {
        self.runner.release_all()
    }

    /// Whether any finger is down, so the window keeps passing.
    pub fn busy(&self) -> bool {
        self.runner.busy()
    }

    /// Time passing: swipes go on, an aim finger the mouse left is lifted.
    pub fn tick(&mut self, now: Instant) -> Vec<ControlEvent> {
        self.runner.tick(now)
    }

    /// Write the keymap if it changed.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Err(e) = keymap::save(&self.dir, &self.package, &self.map) {
            self.note = Some(format!("Could not save the keymap: {e}"));
        }
    }

    fn changed(&mut self) {
        self.dirty = true;
    }

    /// Start placing `tool`: the next click (or drag, for a swipe) on the
    /// picture puts it there.
    pub fn choose(&mut self, tool: Tool) {
        self.pending = Some(Pending::Place(tool));
        self.swipe_from = None;
        self.note = Some(
            match tool {
                Tool::Tap => "Click the picture where the tap goes.",
                Tool::Stick => "Click the picture where the stick's middle goes.",
                Tool::Swipe => "Drag on the picture from where the swipe starts to where it ends.",
                Tool::Aim => "Click the picture where the aim area's middle goes.",
            }
            .into(),
        );
    }

    /// The picture clicked at `at`, with a tool chosen.
    pub fn place(&mut self, at: Point) {
        let Some(Pending::Place(tool)) = self.pending else {
            return;
        };
        match tool {
            Tool::Tap => {
                self.pending = Some(Pending::TapKey(at));
                self.note = Some("Press the key for this tap. Escape cancels.".into());
            }
            Tool::Stick => {
                let result = self.map.add_stick(at).map(|_| ());
                self.finish(result);
            }
            Tool::Aim => {
                let result = self.map.add_aim(at).map(|_| ());
                self.finish(result);
            }
            // A swipe is drawn, not clicked.
            Tool::Swipe => {}
        }
    }

    /// A swipe drawn from `from` to `to`.
    pub fn swipe_drawn(&mut self, from: Point, to: Point) {
        if !matches!(self.pending, Some(Pending::Place(Tool::Swipe))) {
            return;
        }
        let far = i32::from(from.x).abs_diff(i32::from(to.x))
            + i32::from(from.y).abs_diff(i32::from(to.y));
        if far < 1500 {
            self.note = Some("Drag farther to draw a swipe.".into());
            return;
        }
        self.pending = Some(Pending::SwipeKey(from, to));
        self.note = Some("Press the key for this swipe. Escape cancels.".into());
    }

    /// A key (or mouse button) `token` pressed while the editor waits for one.
    pub fn key_chosen(&mut self, token: &str) {
        let result = match self.pending {
            Some(Pending::TapKey(at)) => self.map.add_tap(token, at).map(|_| ()),
            Some(Pending::SwipeKey(from, to)) => self.map.add_swipe(token, from, to).map(|_| ()),
            Some(Pending::Rebind { index, slot }) => self.map.rebind(index, slot, token),
            _ => return,
        };
        self.finish(result);
    }

    /// Escape (or another tool chosen): leave what the editor waited for.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.swipe_from = None;
        self.note = None;
    }

    /// Wait for a key for slot `slot` of binding `index`.
    pub fn rebind(&mut self, index: usize, slot: usize) {
        self.pending = Some(Pending::Rebind { index, slot });
        self.note = Some("Press the new key. Escape cancels.".into());
    }

    /// Take binding `index` away.
    pub fn remove(&mut self, index: usize) {
        self.map.remove(index);
        self.changed();
        self.pending = None;
    }

    fn finish(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.changed();
                self.pending = None;
                self.note = None;
            }
            Err(e) => self.note = Some(e),
        }
    }

    /// The marker of binding `index` near `pos` over `picture`, if any: the
    /// binding, and whether it is a swipe's end.
    pub fn marker_at(&self, picture: egui::Rect, pos: egui::Pos2) -> Option<Grab> {
        let at = |p: Point| to_screen(picture, p);
        self.map
            .bindings
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                let mut hits = vec![(i, false, at(b.anchor()))];
                if let Binding::Swipe { to, .. } = b {
                    hits.push((i, true, at(*to)));
                }
                hits
            })
            .filter(|(_, _, p)| p.distance(pos) <= GRAB)
            .min_by(|a, b| a.2.distance(pos).total_cmp(&b.2.distance(pos)))
            .map(|(i, end, _)| (i, end))
    }

    /// A press on the picture with no tool chosen: a marker under it is
    /// taken up.
    pub fn grab_at(&mut self, picture: egui::Rect, pos: egui::Pos2) {
        self.grab = self.marker_at(picture, pos);
    }

    /// The press goes on to `pos`: the marker taken up follows it.
    pub fn drag_to(&mut self, picture: egui::Rect, pos: egui::Pos2) {
        if let (Some((index, end)), Some((x, y))) = (self.grab, fraction(picture, pos, true)) {
            self.map.move_point(index, end, Point { x, y });
            self.changed();
        }
    }

    /// The press ended.
    pub fn drop_grab(&mut self) {
        self.grab = None;
    }
}

/// Where `p` is over `picture`.
fn to_screen(picture: egui::Rect, p: Point) -> egui::Pos2 {
    egui::pos2(
        picture.left() + f32::from(p.x) / 65535.0 * picture.width(),
        picture.top() + f32::from(p.y) / 65535.0 * picture.height(),
    )
}

/// The mouse button a press is, as a keymap's token.
fn button_token(button: egui::PointerButton) -> Option<&'static str> {
    match button {
        egui::PointerButton::Primary => Some(MOUSE_LEFT),
        egui::PointerButton::Secondary => Some(MOUSE_RIGHT),
        egui::PointerButton::Middle => Some(MOUSE_MIDDLE),
        _ => None,
    }
}

/// The key an event is, where it sits on the keyboard (so a French layout's
/// W is still the key above S), as a keymap's token.
pub fn key_token(key: egui::Key, physical: Option<egui::Key>) -> String {
    physical.unwrap_or(key).name().to_string()
}

/// What one pass of input makes of the keymap.
#[derive(Debug, Default)]
pub struct Translated {
    /// The fingers, to go in batches of their own.
    pub fingers: Vec<ControlEvent>,
    /// The events the phone's ordinary touch and typing still take.
    pub rest: Vec<egui::Event>,
}

/// This pass's `events` through the keymap in `runner`: the keys and (while
/// `locked`) the mouse buttons it listens to become fingers and are kept
/// from the ordinary path; typed text is never sent while keys are acting;
/// the locked mouse's raw movement is the aim's. Keys held with Ctrl, Alt
/// or the system key are shortcuts, not the keymap's, and the ordinary path
/// keeps them.
pub fn translate(
    runner: &mut Runner,
    events: &[egui::Event],
    picture: egui::Rect,
    locked: bool,
    now: Instant,
) -> Translated {
    runner.set_picture(picture.width(), picture.height());
    let mut out = Translated::default();
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    for event in events {
        match event {
            egui::Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers,
            } => {
                let token = key_token(*key, *physical_key);
                if !runner.listens(&token) {
                    out.rest.push(event.clone());
                    continue;
                }
                if *pressed && (modifiers.ctrl || modifiers.alt || modifiers.command) {
                    out.rest.push(event.clone());
                } else if !*repeat {
                    out.fingers.extend(runner.key(&token, *pressed, now));
                }
            }
            // Games do not type.
            egui::Event::Text(_) | egui::Event::Paste(_) => {}
            egui::Event::PointerButton {
                button, pressed, ..
            } if locked => {
                if let Some(token) = button_token(*button)
                    && runner.listens(token)
                {
                    out.fingers.extend(runner.key(token, *pressed, now));
                }
            }
            egui::Event::PointerMoved(_) | egui::Event::MouseWheel { .. } if locked => {}
            egui::Event::MouseMoved(delta) if locked => {
                dx += delta.x;
                dy += delta.y;
            }
            other => out.rest.push(other.clone()),
        }
    }
    if locked && (dx != 0.0 || dy != 0.0) && picture.width() > 0.0 && picture.height() > 0.0 {
        out.fingers
            .extend(runner.mouse(dx / picture.width(), dy / picture.height(), now));
    }
    out
}

impl Keys {
    /// This pass's input, through the keymap (see [`translate`]).
    pub fn translate(
        &mut self,
        events: &[egui::Event],
        picture: egui::Rect,
        now: Instant,
    ) -> Translated {
        let locked = self.lock.locked();
        translate(&mut self.runner, events, picture, locked, now)
    }

    /// The lock and chords, each pass: Ctrl+Alt+Shift+M switches the lock,
    /// Ctrl+Alt+Shift alone frees it; it locks only while keys are acting
    /// and the window has the focus and a picture, and frees itself when
    /// they stop. Viewport commands go out here; the lifted fingers of a
    /// freed lock are not this pass's business (the window lifts all when
    /// keys stop acting).
    pub fn follow(
        &mut self,
        ctx: &egui::Context,
        picture: Option<egui::Rect>,
        area: egui::Rect,
        focused: bool,
        can: bool,
    ) {
        let (events, held) = ctx.input(|i| (i.events.clone(), i.modifiers));
        if events.iter().any(view_pointer::lock_chord) {
            self.lock.toggle();
        }
        if self.chord.observe(&events, held) {
            self.lock.free();
        }
        let situation = view_pointer::Situation {
            focused,
            controls: can && self.acting() && self.map.has_aim(),
            picture: picture.is_some(),
        };
        let over = picture.unwrap_or(area);
        if let Some(change) = self.lock.step(situation, Instant::now()) {
            for command in view_pointer::commands(change, view_pointer::grab_here(), over) {
                ctx.send_viewport_cmd(command);
            }
        }
        if !self.lock.locked() {
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::None);
        if let Some(back) = view_pointer::recentre(ctx.input(|i| i.pointer.latest_pos()), over) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(back));
        }
        // Said for as long as the pointer is locked, not for a moment: a
        // hidden pointer is how a window is lost.
        egui::Area::new(egui::Id::new("cast-keys-pointer-note"))
            .fixed_pos(egui::pos2(over.center().x - 140.0, over.top() + 16.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_black_alpha(210))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_gray(235), view_pointer::HINT);
                    });
            });
    }

    /// The window closed: the lock is let go.
    pub fn close(&mut self, ctx: &egui::Context) {
        if self.lock.locked() {
            self.lock.free();
            ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(
                egui::viewport::CursorGrab::None,
            ));
        }
    }

    /// The bar's controls for keys to taps.
    pub fn bar(&mut self, ui: &mut egui::Ui) {
        let on = ui
            .checkbox(&mut self.on, "Keys to taps")
            .on_hover_text("Play this app from the keyboard and mouse, as this app's keymap says");
        if on.clicked() {
            on.surrender_focus();
            if !self.on {
                self.set_editing(false);
                if self.lock.wanted() {
                    self.lock.free();
                }
            }
        }
        if !self.on {
            return;
        }
        let label = if self.editing { "Done" } else { "Edit keys" };
        let edit = ui.button(label).on_hover_text(
            "Draw where keys tap, the stick, swipes and the aim area on the picture",
        );
        if edit.clicked() {
            edit.surrender_focus();
            self.set_editing(!self.editing);
        }
        if !self.editing && self.map.has_aim() {
            let mut wanted = self.lock.wanted();
            let aim = ui
                .toggle_value(&mut wanted, "Aim with the mouse")
                .on_hover_text("Lock the pointer to the window: its movement drags the aim area. Ctrl+Alt+Shift+M switches it");
            if aim.clicked() {
                aim.surrender_focus();
                self.lock.toggle();
            }
        }
        ui.colored_label(egui::Color32::from_rgb(230, 190, 90), WARNING);
    }

    /// The editor's list beside the picture: what to add, and each binding
    /// with its keys and settings.
    pub fn edit_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Keys");
        ui.label("Add to the picture:");
        ui.horizontal_wrapped(|ui| {
            for (label, tool) in [
                ("Tap", Tool::Tap),
                ("Stick", Tool::Stick),
                ("Swipe", Tool::Swipe),
                ("Aim", Tool::Aim),
            ] {
                let chosen = self.pending == Some(Pending::Place(tool));
                if ui.selectable_label(chosen, label).clicked() {
                    self.choose(tool);
                }
            }
        });
        if let Some(note) = self.note.clone() {
            ui.colored_label(egui::Color32::from_rgb(230, 190, 90), note);
        }
        ui.separator();
        if self.map.bindings.is_empty() {
            ui.label("Nothing yet: choose a tool, then click or drag on the picture.");
        }
        let mut remove = None;
        let mut rebind = None;
        let mut changed = false;
        for (index, binding) in self.map.bindings.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(binding.name());
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                let slots: &[&str] = match binding {
                    Binding::Stick { .. } => &["Up", "Down", "Left", "Right"],
                    Binding::Aim { .. } => &[],
                    _ => &["Key"],
                };
                ui.horizontal_wrapped(|ui| {
                    for (slot, name) in slots.iter().enumerate() {
                        let token = binding
                            .keys()
                            .get(slot)
                            .map(|k| token_label(k))
                            .unwrap_or_default();
                        if ui
                            .button(format!("{name}: {token}"))
                            .on_hover_text("Click, then press the key")
                            .clicked()
                        {
                            rebind = Some((index, slot));
                        }
                    }
                });
                match binding {
                    Binding::Stick { radius, .. } => {
                        changed |= ui
                            .add(egui::Slider::new(radius, RADIUS_RANGE).text("Reach"))
                            .changed();
                    }
                    Binding::Aim {
                        radius,
                        sensitivity,
                        ..
                    } => {
                        changed |= ui
                            .add(egui::Slider::new(radius, RADIUS_RANGE).text("Area"))
                            .changed();
                        changed |= ui
                            .add(egui::Slider::new(sensitivity, SENSITIVITY_RANGE).text("Speed %"))
                            .changed();
                    }
                    Binding::Swipe { ms, .. } => {
                        changed |= ui
                            .add(egui::Slider::new(ms, SWIPE_MS_RANGE).text("Time ms"))
                            .changed();
                    }
                    Binding::Tap { .. } => {}
                }
                ui.add_space(4.0);
            });
        }
        if let Some(index) = remove {
            self.remove(index);
        }
        if let Some((index, slot)) = rebind {
            self.rebind(index, slot);
        }
        if changed {
            self.changed();
        }
        // A mouse button as a key, for the key being chosen.
        if matches!(
            self.pending,
            Some(Pending::TapKey(_) | Pending::SwipeKey(..) | Pending::Rebind { .. })
        ) {
            ui.separator();
            ui.label("Or a mouse button (used while the pointer is locked):");
            ui.horizontal_wrapped(|ui| {
                for token in [MOUSE_LEFT, MOUSE_RIGHT, MOUSE_MIDDLE] {
                    if ui.button(token_label(token)).clicked() {
                        self.key_chosen(token);
                    }
                }
            });
        }
        ui.separator();
        ui.colored_label(egui::Color32::from_rgb(230, 190, 90), WARNING);
        ui.label("The keymap is saved on this computer, for this app only.");
    }

    /// While the editor waits for a key: the first plain key pressed is it;
    /// Escape leaves.
    pub fn editor_keys(&mut self, events: &[egui::Event]) {
        if !matches!(
            self.pending,
            Some(Pending::TapKey(_) | Pending::SwipeKey(..) | Pending::Rebind { .. })
        ) {
            return;
        }
        for event in events {
            if let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers,
            } = event
            {
                if modifiers.ctrl || modifiers.alt || modifiers.command {
                    continue;
                }
                if *key == egui::Key::Escape {
                    self.cancel();
                } else {
                    let token = key_token(*key, *physical_key);
                    self.key_chosen(&token);
                }
                return;
            }
        }
    }

    /// The editor's input on the picture: a click or drag with a tool chosen
    /// places it, one without takes a marker up and moves it.
    pub fn editor_pointer(&mut self, response: &egui::Response, picture: egui::Rect) {
        let pos = response.interact_pointer_pos();
        match self.pending {
            Some(Pending::Place(Tool::Swipe)) => {
                if response.drag_started()
                    && let Some((x, y)) = pos.and_then(|p| fraction(picture, p, true))
                {
                    self.swipe_from = Some(Point { x, y });
                }
                if response.drag_stopped()
                    && let (Some(from), Some((x, y))) = (
                        self.swipe_from.take(),
                        pos.and_then(|p| fraction(picture, p, true)),
                    )
                {
                    self.swipe_drawn(from, Point { x, y });
                }
            }
            Some(Pending::Place(_)) => {
                if response.clicked()
                    && let Some((x, y)) = pos.and_then(|p| fraction(picture, p, true))
                {
                    self.place(Point { x, y });
                }
            }
            Some(_) => {}
            None => {
                if response.drag_started()
                    && let Some(p) = pos
                {
                    self.grab_at(picture, p);
                }
                if response.dragged()
                    && let Some(p) = pos
                {
                    self.drag_to(picture, p);
                }
                if response.drag_stopped() {
                    self.drop_grab();
                }
            }
        }
    }

    /// The marks of the keymap over `picture`: a ring and the key for a tap,
    /// the stick's area and keys, a swipe's arrow, the aim's area.
    pub fn overlay(&self, painter: &egui::Painter, picture: egui::Rect) {
        let ring = egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 200, 255));
        let fill = egui::Color32::from_rgba_unmultiplied(20, 60, 90, 120);
        let text = egui::Color32::WHITE;
        let label = |at: egui::Pos2, words: &str| {
            painter.text(
                at,
                egui::Align2::CENTER_CENTER,
                words,
                egui::FontId::proportional(12.0),
                text,
            );
        };
        for binding in &self.map.bindings {
            match binding {
                Binding::Tap { key, at } => {
                    let c = to_screen(picture, *at);
                    painter.circle(c, 14.0, fill, ring);
                    label(c, &token_label(key));
                }
                Binding::Stick {
                    up,
                    down,
                    left,
                    right,
                    center,
                    radius,
                } => {
                    let c = to_screen(picture, *center);
                    let r = f32::from(*radius) / 65535.0 * picture.width();
                    painter.circle(c, r, fill, ring);
                    label(c + egui::vec2(0.0, -r + 10.0), &token_label(up));
                    label(c + egui::vec2(0.0, r - 10.0), &token_label(down));
                    label(c + egui::vec2(-r + 12.0, 0.0), &token_label(left));
                    label(c + egui::vec2(r - 12.0, 0.0), &token_label(right));
                    label(c, "Stick");
                }
                Binding::Swipe { key, from, to, .. } => {
                    let a = to_screen(picture, *from);
                    let b = to_screen(picture, *to);
                    painter.circle(a, 12.0, fill, ring);
                    painter.arrow(a, b - a, ring);
                    painter.circle_filled(b, 5.0, ring.color);
                    label(a, &token_label(key));
                }
                Binding::Aim { center, radius, .. } => {
                    let c = to_screen(picture, *center);
                    let r = f32::from(*radius) / 65535.0 * picture.width();
                    painter.circle(
                        c,
                        r,
                        egui::Color32::from_rgba_unmultiplied(90, 60, 20, 90),
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 190, 90)),
                    );
                    label(c, "Aim");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn picture() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 800.0))
    }

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-keymap-view-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(name: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: name,
            physical_key: Some(name),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn runner() -> Runner {
        let mut map = Keymap::default();
        map.add_stick(Point { x: 10000, y: 40000 }).unwrap();
        map.add_tap("Space", Point { x: 60000, y: 50000 }).unwrap();
        map.add_aim(Point { x: 45000, y: 25000 }).unwrap();
        map.add_tap(MOUSE_LEFT, Point { x: 55000, y: 40000 })
            .unwrap();
        Runner::new(map)
    }

    #[test]
    fn a_mapped_key_is_a_finger_and_never_typed() {
        let now = Instant::now();
        let mut run = runner();
        let out = translate(
            &mut run,
            &[
                key(egui::Key::W, true),
                egui::Event::Text("w".into()),
                key(egui::Key::Space, true),
                key(egui::Key::Enter, true),
                egui::Event::Paste("pasted".into()),
            ],
            picture(),
            false,
            now,
        );
        // W: the stick's down and move; Space: a tap. Typing never goes.
        assert_eq!(out.fingers.len(), 3);
        assert!(
            out.fingers
                .iter()
                .all(|e| matches!(e, ControlEvent::Pointer { .. }))
        );
        assert_eq!(
            out.rest.len(),
            1,
            "Enter, which no key is mapped to, goes on"
        );
        assert!(matches!(
            out.rest[0],
            egui::Event::Key {
                key: egui::Key::Enter,
                ..
            }
        ));
        // The release of a mapped key is the finger's, and a repeat is nothing.
        let repeat = egui::Event::Key {
            key: egui::Key::Space,
            physical_key: Some(egui::Key::Space),
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers::NONE,
        };
        let out = translate(
            &mut run,
            &[repeat, key(egui::Key::Space, false)],
            picture(),
            false,
            now,
        );
        assert_eq!(out.fingers.len(), 1);
        assert!(out.rest.is_empty());
    }

    #[test]
    fn a_key_with_ctrl_or_alt_is_a_shortcut_and_not_the_keymaps() {
        let now = Instant::now();
        let mut run = runner();
        let ctrl = egui::Event::Key {
            key: egui::Key::W,
            physical_key: Some(egui::Key::W),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        let out = translate(&mut run, &[ctrl], picture(), false, now);
        assert!(out.fingers.is_empty());
        assert_eq!(out.rest.len(), 1);
        assert!(!run.busy());
    }

    #[test]
    fn the_layouts_own_letter_does_not_matter_only_the_key_where_it_sits() {
        // A key that types Z where a W sits (a French layout): the physical key wins.
        assert_eq!(key_token(egui::Key::Z, Some(egui::Key::W)), "W");
        assert_eq!(key_token(egui::Key::Z, None), "Z");
    }

    #[test]
    fn unlocked_the_mouse_is_the_ordinary_touch_and_locked_it_aims() {
        let now = Instant::now();
        let mut run = runner();
        let click = egui::Event::PointerButton {
            pos: egui::pos2(200.0, 300.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let moved = egui::Event::PointerMoved(egui::pos2(210.0, 310.0));
        let raw = egui::Event::MouseMoved(egui::vec2(8.0, 4.0));
        let out = translate(
            &mut run,
            &[click.clone(), moved.clone(), raw.clone()],
            picture(),
            false,
            now,
        );
        assert!(
            out.fingers.is_empty(),
            "unlocked, the mouse aims at nothing"
        );
        assert_eq!(
            out.rest.len(),
            3,
            "its click and moves are the ordinary touch's"
        );
        // Locked: the click is the mapped Mouse left tap, the raw movement is the aim's.
        let out = translate(&mut run, &[click, moved, raw], picture(), true, now);
        assert!(
            out.rest.is_empty(),
            "locked, nothing of the mouse is a touch"
        );
        let downs = out
            .fingers
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ControlEvent::Pointer {
                        action: pocketdesk_core::streaming::cast_control::TouchAction::Down,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(downs, 2, "the click's tap and the aim's finger");
        // A button nothing is mapped to does nothing locked, and the wheel is not sent.
        let right = egui::Event::PointerButton {
            pos: egui::pos2(0.0, 0.0),
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let out = translate(
            &mut run,
            &[right, wheel],
            picture(),
            true,
            now + Duration::from_millis(1),
        );
        assert!(out.fingers.is_empty() && out.rest.is_empty());
    }

    #[test]
    fn a_pass_with_all_the_mouse_moves_is_one_aim_move() {
        let now = Instant::now();
        let mut run = runner();
        let moves: Vec<egui::Event> = (0..10)
            .map(|_| egui::Event::MouseMoved(egui::vec2(1.0, 0.0)))
            .collect();
        let out = translate(&mut run, &moves, picture(), true, now);
        // The finger goes down, then one move for the pass, not ten.
        assert_eq!(out.fingers.len(), 2);
    }

    #[test]
    fn the_editor_places_each_tool_and_asks_for_a_key_where_one_is_needed() {
        let dir = dir("place");
        let mut keys = Keys::new("org.example.game", dir.clone());
        keys.set_editing(true);
        // A tap: click, then a key.
        keys.choose(Tool::Tap);
        keys.place(Point { x: 100, y: 200 });
        assert_eq!(
            keys.pending(),
            Some(Pending::TapKey(Point { x: 100, y: 200 }))
        );
        keys.key_chosen("E");
        assert_eq!(keys.pending(), None);
        assert!(keys.map().uses("E"));
        // A key already used is said, and the tap waits for another.
        keys.choose(Tool::Tap);
        keys.place(Point { x: 5, y: 5 });
        keys.key_chosen("E");
        assert!(keys.note().unwrap().contains("used twice"));
        assert!(matches!(keys.pending(), Some(Pending::TapKey(_))));
        keys.cancel();
        assert_eq!(keys.map().bindings.len(), 1);
        // A stick and an aim need no key; a second aim is refused with the reason.
        keys.choose(Tool::Stick);
        keys.place(Point { x: 9000, y: 9000 });
        keys.choose(Tool::Aim);
        keys.place(Point { x: 40000, y: 20000 });
        keys.choose(Tool::Aim);
        keys.place(Point { x: 41000, y: 21000 });
        assert!(keys.note().unwrap().contains("one aim"));
        assert_eq!(keys.map().bindings.len(), 3);
        // A swipe is drawn: too short is asked again, then it asks for a key.
        keys.choose(Tool::Swipe);
        keys.swipe_drawn(Point { x: 100, y: 100 }, Point { x: 120, y: 110 });
        assert!(keys.note().unwrap().contains("farther"));
        keys.swipe_drawn(Point { x: 1000, y: 1000 }, Point { x: 30000, y: 1000 });
        keys.key_chosen("Q");
        assert!(keys.map().uses("Q"));
        // Leaving the editor saves the keymap, and a new window of the app reads it.
        keys.set_editing(false);
        let again = Keys::new("org.example.game", dir.clone());
        assert_eq!(again.map(), keys.map());
        assert_eq!(again.map().bindings.len(), 4);
        // Another app has its own.
        assert!(
            Keys::new("org.example.other", dir.clone())
                .map()
                .bindings
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_editor_rebinds_removes_and_moves_marks() {
        let dir = dir("edit");
        let mut keys = Keys::new("org.example.game", dir.clone());
        keys.set_editing(true);
        keys.choose(Tool::Stick);
        keys.place(Point { x: 20000, y: 20000 });
        // Rebind the up key; Escape leaves the wait.
        keys.rebind(0, 0);
        keys.editor_keys(&[key(egui::Key::ArrowUp, true)]);
        let arrow = key_token(egui::Key::ArrowUp, None);
        assert!(keys.map().uses(&arrow) && !keys.map().uses("W"));
        keys.rebind(0, 1);
        keys.editor_keys(&[key(egui::Key::Escape, true)]);
        assert_eq!(keys.pending(), None);
        assert!(keys.map().uses("S"));
        // A key pressed with no wait pending is nothing for the editor.
        keys.editor_keys(&[key(egui::Key::X, true)]);
        assert!(!keys.map().uses("X"));
        // A mark is taken up by its marker, followed and dropped.
        let pic = picture();
        let on_it = to_screen(pic, Point { x: 20000, y: 20000 });
        assert_eq!(keys.marker_at(pic, on_it), Some((0, false)));
        assert_eq!(keys.marker_at(pic, on_it + egui::vec2(60.0, 0.0)), None);
        keys.grab_at(pic, on_it);
        keys.drag_to(pic, to_screen(pic, Point { x: 30000, y: 40000 }));
        keys.drop_grab();
        assert_eq!(
            keys.map().bindings[0].anchor(),
            Point { x: 30000, y: 40000 }
        );
        keys.drag_to(pic, on_it);
        assert_eq!(
            keys.map().bindings[0].anchor(),
            Point { x: 30000, y: 40000 },
            "dropped, so not followed"
        );
        keys.remove(0);
        assert!(keys.map().bindings.is_empty());
        keys.set_editing(false);
        // Nothing left: the file goes.
        assert!(keymap::load(&dir, "org.example.game").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_act_only_while_on_and_not_being_edited_and_all_lifts_when_they_stop() {
        let dir = dir("act");
        let mut keys = Keys::new("org.example.game", dir.clone());
        keys.set_editing(true);
        keys.choose(Tool::Tap);
        keys.place(Point { x: 100, y: 100 });
        keys.key_chosen("E");
        keys.set_editing(false);
        assert!(!keys.acting(), "off until the window turns it on");
        keys.on = true;
        assert!(keys.acting());
        let now = Instant::now();
        let out = keys.translate(&[key(egui::Key::E, true)], picture(), now);
        assert_eq!(out.fingers.len(), 1);
        assert!(keys.busy());
        let lifted = keys.release();
        assert_eq!(lifted.len(), 1);
        assert!(!keys.busy());
        keys.set_editing(true);
        assert!(!keys.acting(), "the editor is open");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_finger_held_is_said_to_be_held_every_half_second_and_never_when_none_is() {
        let dir = dir("alive");
        let mut keys = Keys::new("org.example.game", dir.clone());
        keys.set_editing(true);
        keys.choose(Tool::Tap);
        keys.place(Point { x: 100, y: 100 });
        keys.key_chosen("E");
        keys.set_editing(false);
        keys.on = true;
        let t0 = Instant::now();
        let mut said = Vec::new();
        keys.pace(&mut said, t0);
        assert!(said.is_empty(), "nothing held, nothing said");
        let out = keys.translate(&[key(egui::Key::E, true)], picture(), t0);
        let mut fingers = out.fingers;
        keys.pace(&mut fingers, t0);
        assert_eq!(fingers.len(), 1, "the press itself is the word");
        // Held with nothing new: quiet for a while, then a move to where it is.
        let mut quiet = Vec::new();
        keys.pace(&mut quiet, t0 + KEEP_ALIVE / 2);
        assert!(quiet.is_empty());
        keys.pace(&mut quiet, t0 + KEEP_ALIVE);
        assert_eq!(quiet.len(), 1);
        assert!(matches!(
            quiet[0],
            ControlEvent::Pointer {
                action: pocketdesk_core::streaming::cast_control::TouchAction::Move,
                x: 100,
                y: 100,
                ..
            }
        ));
        let mut next = Vec::new();
        keys.pace(&mut next, t0 + KEEP_ALIVE + KEEP_ALIVE / 2);
        assert!(next.is_empty(), "said again only after another half second");
        keys.release();
        let mut none = Vec::new();
        keys.pace(&mut none, t0 + KEEP_ALIVE * 10);
        assert!(none.is_empty(), "released: nothing to say");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_that_is_no_package_has_no_keymap_and_saves_nothing() {
        let dir = dir("bad");
        let mut keys = Keys::new("../escape", dir.clone());
        keys.set_editing(true);
        keys.choose(Tool::Tap);
        keys.place(Point { x: 1, y: 1 });
        keys.key_chosen("E");
        keys.set_editing(false);
        assert!(keys.note().unwrap().contains("Could not save"));
        assert!(!dir.join("keymaps").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
