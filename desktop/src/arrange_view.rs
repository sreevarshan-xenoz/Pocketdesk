//! The Keyboard and mouse page: one keyboard and mouse across the owner's
//! computers. The sharing switch (`shared_input`); the computers' screens as
//! blocks, each dragged to where it sits on the desk, where it snaps so one
//! of its edges touches another's (this computer stays put), kept in
//! `shared_layout.json`; Identify; where the keyboard and mouse are now; and
//! for each paired computer, its link and the Keyboard and mouse permission
//! both ways. A phone that said its screen (BK5) is a block shaped like it,
//! placed as a computer is, and listed with whether it may be driven and
//! this computer's Keyboard and mouse permission for it. The server writes
//! what this reads (`shared_screens.json`, `shared_input_status.json`) and
//! takes its requests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile};
use pocketdesk_core::local::shared_input::{
    self, Bounds, CAPTURE_FRACTIONAL_SCALE, CAPTURE_NEEDS_PERMISSION, CAPTURE_READY,
    CAPTURE_UNSUPPORTED, LINK_AWAY, LINK_DIRECT, LINK_RELAYED, LayoutFile, PHONE_AWAY,
    PHONE_NEEDS_SERVICE, PHONE_NOT_ALLOWED, PHONE_OFF, PHONE_READY, REFUSED_NOT_ALLOWED,
    REFUSED_SHARING_OFF, Screen, ScreensFile, ShareAsk, StatusFile, bounds,
};
use pocketdesk_core::protocol::permissions::Capability;

use crate::model::Device;

/// How often the files are read again while the page is open.
const REFRESH: Duration = Duration::from_secs(1);

/// The most characters of a computer's name the page shows.
const NAME_CHARS: usize = 48;

/// A computer's name as the page shows it: another computer says it, so it
/// is put on one line, rid of what would turn or hide the text around it,
/// and clipped ([`shared_input::display_name`]).
fn shown_name(name: &str) -> String {
    shared_input::display_name(name, NAME_CHARS)
}

/// The page's sharing switch (UX 2: its one home).
pub const SHARE_SWITCH: &str = "Share my keyboard and mouse";

/// The canvas's height, in points.
const CANVAS_HEIGHT: f32 = 220.0;

/// What the setup checklist knows (UX 2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub server_running: bool,
    /// The computers paired, either way.
    pub paired: usize,
    pub sharing_on: bool,
    /// The paired computers linked directly.
    pub linked: usize,
    /// The computers that take this one's keyboard and mouse.
    pub drives: usize,
    /// An arrangement was saved.
    pub arranged: bool,
}

/// A step's fix, by what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    StartServer,
    PairComputer,
    SharingOn,
    RunDiagnostics,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::StartServer => crate::widgets::START,
            Fix::PairComputer => crate::app::PAIR_A_COMPUTER,
            Fix::SharingOn => "Turn on",
            Fix::RunDiagnostics => "Run diagnostics",
        }
    }
}

/// One step of setting the keyboard and mouse up (UX 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub done: bool,
    /// What it is, at most six words.
    pub label: &'static str,
    /// What to do, while it is not done: one line.
    pub todo: &'static str,
    pub fix: Option<Fix>,
}

/// The steps, in order: each later one waits on those before.
pub fn checklist(facts: Facts) -> Vec<Step> {
    let step = |done, label, todo, fix| Step {
        done,
        label,
        todo,
        fix,
    };
    vec![
        step(
            facts.server_running,
            "Pocketdesk is running",
            "It carries the keyboard and mouse.",
            Some(Fix::StartServer),
        ),
        step(
            facts.paired > 0,
            "Another computer is paired",
            "Pair one from the Pair page.",
            Some(Fix::PairComputer),
        ),
        step(
            facts.sharing_on,
            "Sharing is on here",
            "Turn it on here and on the other.",
            Some(Fix::SharingOn),
        ),
        step(
            facts.linked > 0,
            "Linked directly",
            "Same network, with no relay between.",
            Some(Fix::RunDiagnostics),
        ),
        step(
            facts.drives > 0,
            "This one may drive another",
            "Allow it on the other computer's page.",
            None,
        ),
        step(
            facts.arranged,
            "Arranged on the desk",
            "Optional: drag the computers below.",
            None,
        ),
    ]
}

/// The width of a step's mark, a tick or a dot.
const MARK_WIDTH: f32 = 14.0;

/// The checklist's heading.
pub const SETUP: &str = "Setup";

/// What the page asks the window to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Turn the shared keyboard and mouse on or off here (`shared_input`).
    Sharing(bool),
    /// Let the paired computer `device_id` drive this one, or not: its
    /// Keyboard and mouse permission in the trust store (a computer that
    /// paired with this one). For a phone (BK5), this one driving it.
    Allow { device_id: String, allow: bool },
}

/// A block being dragged: whose, its origin when the drag began, how far
/// the pointer went since (points), and the canvas's scale then (kept for
/// the drag, so the blocks do not shift under the pointer).
struct Drag {
    device_id: String,
    from: (i32, i32),
    moved: Vec2,
    view: View,
}

/// Layout pixels to points: a point is `offset + pixel * scale`.
#[derive(Debug, Clone, Copy)]
struct View {
    scale: f32,
    offset: Pos2,
}

impl View {
    fn rect(&self, b: &Bounds) -> Rect {
        Rect::from_min_max(self.point(b.x0, b.y0), self.point(b.x1, b.y1))
    }

    fn point(&self, x: i64, y: i64) -> Pos2 {
        self.offset + Vec2::new(x as f32 * self.scale, y as f32 * self.scale)
    }

    /// The view that fits `all` in `canvas`, with a margin.
    fn fit(all: Bounds, canvas: Rect) -> View {
        let room = canvas.shrink(12.0);
        let (w, h) = (all.width().max(1) as f32, all.height().max(1) as f32);
        let scale = (room.width() / w).min(room.height() / h);
        let size = Vec2::new(w * scale, h * scale);
        let min = room.center() - size / 2.0;
        View {
            scale,
            offset: min - Vec2::new(all.x0 as f32 * scale, all.y0 as f32 * scale),
        }
    }
}

/// One computer as the page shows it.
#[derive(Debug, Clone)]
struct Shown {
    device_id: String,
    name: String,
    screens: Vec<Screen>,
    origin: (i32, i32),
    this: bool,
    /// The owner's phone (BK5): drawn as one.
    phone: bool,
}

#[derive(Default)]
pub struct ArrangeView {
    screens: ScreensFile,
    saved: Option<LayoutFile>,
    status: StatusFile,
    computers: ComputersFile,
    read_at: Option<Instant>,
    drag: Option<Drag>,
    /// What it said happened, until the app takes it.
    notice: Option<crate::notice::Notice>,
}

/// Where `dragged` (a block, in layout pixels) goes when dropped among
/// `others`: its new top-left, so one of its edges touches one of
/// theirs and it overlaps none, sharing at least a quarter of the shorter
/// of the two touching sides; the nearest such place to where it was
/// dropped. With nowhere free next to any (none would be), to the right of
/// them all.
pub fn snap(dragged: Bounds, others: &[Bounds]) -> (i64, i64) {
    let (w, h) = (dragged.width(), dragged.height());
    let at = (dragged.x0, dragged.y0);
    let mut best: Option<((i64, i64), i128)> = None;
    for o in others {
        let need_y = (h.min(o.height()) / 4).max(1);
        let need_x = (w.min(o.width()) / 4).max(1);
        let y = at.1.clamp(o.y0 - h + need_y, o.y1 - need_y);
        let x = at.0.clamp(o.x0 - w + need_x, o.x1 - need_x);
        for place in [(o.x0 - w, y), (o.x1, y), (x, o.y0 - h), (x, o.y1)] {
            let placed = Bounds {
                x0: place.0,
                y0: place.1,
                x1: place.0 + w,
                y1: place.1 + h,
            };
            if others.iter().any(|o| overlaps(&placed, o)) {
                continue;
            }
            let (dx, dy) = (i128::from(place.0 - at.0), i128::from(place.1 - at.1));
            let distance = dx * dx + dy * dy;
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((place, distance));
            }
        }
    }
    best.map_or_else(
        || {
            let right = others.iter().map(|o| o.x1).max().unwrap_or(at.0);
            let top = others.iter().map(|o| o.y0).min().unwrap_or(at.1);
            (right, top)
        },
        |(place, _)| place,
    )
}

/// Whether the two blocks share any pixel; touching edges do not.
pub fn overlaps(a: &Bounds, b: &Bounds) -> bool {
    a.overlaps(b)
}

/// A phone's standing (BK5) as the page says it, and whether it is a
/// warning.
fn phone_words(standing: &str, name: &str, this_name: &str) -> (String, bool) {
    match standing {
        PHONE_READY => (
            format!("Ready: push the pointer off the edge {name} is on to use it"),
            false,
        ),
        PHONE_OFF => (
            format!(
                "Off on {name}: turn on \"Let my computer's keyboard and mouse use this phone\" \
                 in its Settings"
            ),
            true,
        ),
        PHONE_NEEDS_SERVICE => (
            format!(
                "Needs the accessibility service on {name}: turn on Pocketdesk control in \
                 Android's Accessibility settings"
            ),
            true,
        ),
        PHONE_NOT_ALLOWED => (
            format!("Turn on \"{this_name} may drive {name}\" to use it"),
            false,
        ),
        PHONE_AWAY | "" => (format!("Not linked now: open Pocketdesk on {name}"), false),
        _ => (format!("Not linked now: open Pocketdesk on {name}"), false),
    }
}

/// How Linux's input capture stands for driving (BK8) as the page says
/// it, and whether it is a warning; `None` when nothing is known.
fn capture_words(word: &str) -> Option<(&'static str, bool)> {
    Some(match word {
        CAPTURE_READY => (
            "Ready: push the pointer off an edge that leads to another computer",
            false,
        ),
        CAPTURE_NEEDS_PERMISSION => (
            "Needs permission: your desktop shows a dialog asking to let Pocketdesk capture \
             input; allow it (turn sharing off and on to be asked again)",
            true,
        ),
        CAPTURE_UNSUPPORTED => (
            "Your desktop doesn't offer input capture, so this computer's keyboard and mouse \
             stay here (it needs GNOME 45, KDE Plasma 6.1 or Hyprland's portal 1.4 or later); \
             it can still be driven",
            true,
        ),
        CAPTURE_FRACTIONAL_SCALE => (
            "Fractional scaling stops crossing on Hyprland (a bug in its portal): set the \
             screen's scale to a whole number to cross its edges",
            true,
        ),
        _ => return None,
    })
}

/// A link as the page says it.
fn link_words(link: &str) -> &'static str {
    match link {
        LINK_DIRECT => "Linked directly",
        LINK_RELAYED => "Linked through a relay: the keyboard and mouse go only over a direct link",
        REFUSED_SHARING_OFF => "Won't take input: sharing off there",
        REFUSED_NOT_ALLOWED => "Won't take input: not allowed there",
        LINK_AWAY | "" => "Not linked now",
        _ => "Not linked now",
    }
}

impl ArrangeView {
    /// What this page said happened since the last call.
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }

    /// What Setup's steps are judged by (UX 2).
    fn facts(&self, devices: &[Device], sharing_on: bool, server_running: bool) -> Facts {
        let paired_now = self.paired(devices);
        Facts {
            server_running,
            paired: paired_now.len(),
            sharing_on,
            linked: paired_now
                .iter()
                .filter(|id| self.status.link.get(*id).is_some_and(|l| l == LINK_DIRECT))
                .count(),
            // Only computers paired now (PR #555 review m5). Like the
            // links, it is fresh only while sharing is on (the status is
            // written every 30 s while off): the steps after Sharing may
            // lag then, and Sharing's own step comes first.
            drives: paired_now
                .iter()
                .filter(|id| self.status.answered.contains(*id))
                .count(),
            arranged: self.saved.is_some(),
        }
    }

    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.screens = shared_input::read(state_dir);
        self.saved = shared_input::read_layout(state_dir);
        self.status = shared_input::read_status(state_dir);
        self.computers = computers::read(state_dir);
        self.read_at = Some(Instant::now());
    }

    /// The computers paired now, either way: those paired with this one
    /// (`devices`' computers) and those this one paired with.
    fn paired(&self, devices: &[Device]) -> BTreeSet<String> {
        devices
            .iter()
            .filter(|d| d.computer)
            .map(|d| d.id.clone())
            .chain(self.computers.computers.iter().map(|c| c.device_id.clone()))
            .collect()
    }

    /// The phones paired now that said their screen (BK5).
    fn phones(&self, devices: &[Device]) -> BTreeSet<String> {
        devices
            .iter()
            .filter(|d| !d.computer)
            .filter(|d| self.screens.get(&d.id).is_some_and(|c| c.phone.is_some()))
            .map(|d| d.id.clone())
            .collect()
    }

    /// What is placed on the desk: the computers and the phones paired now.
    fn placeable(&self, devices: &[Device]) -> BTreeSet<String> {
        let mut all = self.paired(devices);
        all.extend(self.phones(devices));
        all
    }

    fn name_of(&self, devices: &[Device], id: &str) -> String {
        if let Some(c) = self.screens.get(id).filter(|c| !c.name.is_empty()) {
            return shown_name(&c.name);
        }
        if let Some(c) = self.computers.computers.iter().find(|c| c.device_id == id) {
            return shown_name(&c.name);
        }
        shown_name(&crate::model::device_label(devices, id))
    }

    /// This computer and the paired computers that said their screens,
    /// where they sit.
    fn shown(&self, devices: &[Device]) -> Vec<Shown> {
        let Some(me) = self.screens.this_computer() else {
            return Vec::new();
        };
        let paired = self.placeable(devices);
        let others: Vec<(String, Vec<Screen>)> = self
            .screens
            .computers
            .iter()
            .filter(|c| !c.this_computer && paired.contains(&c.device_id) && !c.screens.is_empty())
            .map(|c| (c.device_id.clone(), c.screens.clone()))
            .collect();
        let origins = shared_input::arrange(&me.screens, &others, self.saved.as_ref());
        let mut shown = vec![Shown {
            device_id: me.device_id.clone(),
            name: shown_name(&me.name),
            screens: me.screens.clone(),
            origin: (0, 0),
            this: true,
            phone: false,
        }];
        shown.extend(
            others
                .into_iter()
                .zip(origins)
                .map(|((id, screens), origin)| Shown {
                    name: self.name_of(devices, &id),
                    phone: self.screens.get(&id).is_some_and(|c| c.phone.is_some()),
                    device_id: id,
                    screens,
                    origin,
                    this: false,
                }),
        );
        shown
    }

    /// Keep where each computer is, with `id` dropped at `origin`.
    fn save(
        &mut self,
        state_dir: &Path,
        shown: &[Shown],
        paired: &BTreeSet<String>,
        id: &str,
        origin: (i32, i32),
    ) {
        let placed: Vec<(String, (i32, i32))> = shown
            .iter()
            .map(|c| {
                let at = if c.device_id == id { origin } else { c.origin };
                (c.device_id.clone(), at)
            })
            .collect();
        let layout = LayoutFile {
            origins: origins_to_save(&placed, self.saved.as_ref(), paired),
            rev: self.saved.as_ref().map_or(0, |l| l.rev) + 1,
        };
        match shared_input::write_layout(state_dir, &layout) {
            Ok(()) => self.saved = Some(layout),
            Err(e) => {
                self.notice =
                    crate::notice::Notice::error(format!("Could not keep the arrangement: {e}"))
            }
        }
    }

    /// The canvas: each computer's screens, scaled to fit, its name on its
    /// block; the others dragged into place.
    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        shown: &[Shown],
        paired: &BTreeSet<String>,
    ) {
        // A computer dragged that has gone (unpaired meanwhile) drags no more.
        if self
            .drag
            .as_ref()
            .is_some_and(|d| !shown.iter().any(|c| c.device_id == d.device_id))
        {
            self.drag = None;
        }
        let width = ui.available_width().max(200.0);
        let (canvas, _) = ui.allocate_exact_size(Vec2::new(width, CANVAS_HEIGHT), Sense::hover());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 6.0, ui.visuals().extreme_bg_color);
        // Where each block is now, the one dragged where the pointer has it.
        let mut placed: Vec<(usize, Bounds)> = Vec::new();
        for (i, c) in shown.iter().enumerate() {
            let origin = match &self.drag {
                Some(d) if d.device_id == c.device_id => (
                    d.from
                        .0
                        .saturating_add((d.moved.x / d.view.scale).round() as i32),
                    d.from
                        .1
                        .saturating_add((d.moved.y / d.view.scale).round() as i32),
                ),
                _ => c.origin,
            };
            if let Some(b) = bounds(&c.screens) {
                placed.push((i, b.shifted(origin)));
            }
        }
        let Some(all) = placed.iter().map(|(_, b)| *b).reduce(|a, b| Bounds {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        }) else {
            return;
        };
        let view = self
            .drag
            .as_ref()
            .map_or_else(|| View::fit(all, canvas), |d| d.view);
        let text = ui.visuals().text_color();
        let mut dropped: Option<(String, Bounds)> = None;
        for (i, block) in &placed {
            let c = &shown[*i];
            let shift = (
                (block.x0 - bounds(&c.screens).map_or(0, |b| b.x0)) as i32,
                (block.y0 - bounds(&c.screens).map_or(0, |b| b.y0)) as i32,
            );
            let fill = if c.this {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };
            let edge = Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color);
            for s in &c.screens {
                let r = view.rect(&Bounds::of(s).shifted(shift));
                if c.phone {
                    phone_glyph(&painter, r, fill, edge);
                } else {
                    painter.rect(r, 2.0, fill, edge, StrokeKind::Inside);
                }
            }
            let rect = view.rect(block);
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if c.this {
                    format!("{} (this computer)", c.name)
                } else {
                    c.name.clone()
                },
                egui::FontId::proportional(13.0),
                if c.this {
                    ui.visuals().selection.stroke.color
                } else {
                    text
                },
            );
            if c.this {
                continue;
            }
            let response = ui
                .interact(rect, ui.id().with(("arrange", &c.device_id)), Sense::drag())
                .on_hover_cursor(egui::CursorIcon::Grab);
            if response.drag_started() {
                self.drag = Some(Drag {
                    device_id: c.device_id.clone(),
                    from: c.origin,
                    moved: Vec2::ZERO,
                    view,
                });
            }
            if response.dragged()
                && let Some(d) = self.drag.as_mut().filter(|d| d.device_id == c.device_id)
            {
                d.moved += response.drag_delta();
            }
            if response.drag_stopped() {
                dropped = Some((c.device_id.clone(), *block));
            }
        }
        if let Some((id, block)) = dropped {
            self.drag = None;
            let others: Vec<Bounds> = placed
                .iter()
                .filter(|(i, _)| shown[*i].device_id != id)
                .map(|(_, b)| *b)
                .collect();
            let (x, y) = snap(block, &others);
            if let Some(c) = shown.iter().find(|c| c.device_id == id)
                && let Some(b) = bounds(&c.screens)
            {
                let origin = (clamp_i32(x - b.x0), clamp_i32(y - b.y0));
                self.save(state_dir, shown, paired, &id, origin);
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        devices: &[Device],
        sharing_on: bool,
        server_running: bool,
        now_unix: u64,
    ) -> Option<Action> {
        self.refresh(state_dir);
        let mut action = None;
        let muted = crate::theme::muted(ui);
        // UX 2: each step, done or not, with its fix.
        let facts = self.facts(devices, sharing_on, server_running);
        ui.label(RichText::new(SETUP).strong());
        for step in checklist(facts) {
            ui.horizontal_wrapped(|ui| {
                // The mark in a box of one width, so the labels line up
                // (PR #555 review m8).
                let mark = if step.done {
                    RichText::new("✔").color(crate::theme::good(ui))
                } else {
                    RichText::new("•").color(crate::theme::warn(ui))
                };
                ui.add_sized(
                    [MARK_WIDTH, ui.spacing().interact_size.y],
                    egui::Label::new(mark.strong()),
                );
                ui.label(step.label);
                if !step.done {
                    ui.label(RichText::new(step.todo).color(muted));
                    if let Some(fix) = step.fix
                        && ui.small_button(fix.label()).clicked()
                    {
                        match fix {
                            Fix::StartServer => {
                                crate::widgets::ask(ui.ctx(), crate::widgets::Ask::StartServer)
                            }
                            Fix::PairComputer => {
                                crate::widgets::ask(ui.ctx(), crate::widgets::Ask::PairComputer)
                            }
                            Fix::RunDiagnostics => {
                                crate::widgets::ask(ui.ctx(), crate::widgets::Ask::RunDiagnostics)
                            }
                            Fix::SharingOn => action = Some(Action::Sharing(true)),
                        }
                    }
                }
            });
        }
        ui.add_space(8.0);
        let mut on = sharing_on;
        if ui
            .checkbox(&mut on, SHARE_SWITCH)
            .on_hover_text(
                "One keyboard and mouse across your own paired computers, on Windows and Linux.",
            )
            .changed()
        {
            action = Some(Action::Sharing(on));
        }
        // A stopped server is said by the bar at the top (UX 1).
        // Linux drives through its desktop's input capture (BK8): how that
        // stands, while sharing is on.
        if !cfg!(windows)
            && sharing_on
            && let Some((words, warn)) = capture_words(&self.status.input_capture)
        {
            let words = RichText::new(words);
            ui.label(if warn {
                words.color(crate::theme::warn(ui))
            } else {
                words.color(muted)
            });
        }
        let shown = self.shown(devices);
        let this_name = shown
            .first()
            .map_or_else(|| "this computer".to_string(), |c| c.name.clone());
        let on_now = &self.status.keyboard_on;
        ui.label(if on_now.is_empty() {
            "The keyboard and mouse are on this computer.".to_string()
        } else {
            format!(
                "The keyboard and mouse are on {} now.",
                self.name_of(devices, on_now)
            )
        });
        ui.add_space(6.0);
        if shown.is_empty() {
            ui.label(
                RichText::new(
                    "This computer's screens show here once sharing is on and the server has \
                     looked at them.",
                )
                .color(muted),
            );
        } else {
            let paired = self.placeable(devices);
            self.canvas(ui, state_dir, &shown, &paired);
            ui.label(
                RichText::new(
                    "Drag a computer to where it sits on your desk: it snaps to the nearest edge \
                     of another. This computer stays where it is.",
                )
                .color(muted),
            );
            if shown.len() == 1 {
                ui.label(
                    RichText::new("Others show here once paired, with sharing on.").color(muted),
                );
            }
        }
        ui.horizontal(|ui| {
            let reachable = self.status.answered.len();
            if ui
                .add_enabled(reachable > 0, egui::Button::new("Identify"))
                .on_hover_text("Each computer this one can drive shows who it is.")
                .on_disabled_hover_text("No computer takes this one's keyboard and mouse now.")
                .clicked()
            {
                self.notice = crate::notice::Notice::outcome(
                    shared_input::request(state_dir, ShareAsk::Identify, now_unix)
                        .map(|_| "Asked your computers to show who they are.".to_string())
                        .map_err(|e| format!("Not asked: {e}")),
                );
            }
            if self.saved.is_some()
                && ui
                    .button("Put them in a row")
                    .on_hover_text("Forget the arrangement: the others to the right of this one.")
                    .clicked()
            {
                match std::fs::remove_file(
                    state_dir.join(pocketdesk_core::local::paths::SHARED_LAYOUT_FILE),
                ) {
                    Ok(()) => self.saved = None,
                    Err(e) => {
                        self.notice = crate::notice::Notice::error(format!(
                            "Could not forget the arrangement: {e}"
                        ))
                    }
                }
            }
        });
        // UX 1: why Identify is off, beside it, with the fix.
        if self.status.answered.is_empty() {
            if !sharing_on {
                if crate::widgets::why_off(
                    ui,
                    "Identify needs sharing on: turn it on above.",
                    Some("Turn sharing on"),
                ) {
                    action = Some(Action::Sharing(true));
                }
            } else if !server_running {
                crate::widgets::why_off_ask(
                    ui,
                    "Identify needs the server running.",
                    crate::widgets::START,
                    crate::widgets::Ask::StartServer,
                );
            } else if self.paired(devices).is_empty() {
                crate::widgets::why_off_ask(
                    ui,
                    "Identify needs another computer paired with this one.",
                    crate::app::PAIR_A_COMPUTER,
                    crate::widgets::Ask::PairComputer,
                );
            } else {
                crate::widgets::why_off(
                    ui,
                    "No computer takes this one's keyboard and mouse yet: turn sharing on there, and let this one drive it.",
                    None,
                );
            }
        }
        ui.add_space(8.0);
        ui.heading("Your computers");
        let paired = self.paired(devices);
        if paired.is_empty() {
            ui.label(RichText::new("No other computer is paired yet.").color(muted));
            if let Some(phones) = self.phones_section(ui, devices, &this_name) {
                action = Some(phones);
            }
            return action;
        }
        let mut ask: Option<(String, bool)> = None;
        egui::Grid::new("keyboard_mouse_computers")
            .num_columns(2)
            // Its name on a line of its own, not a narrow column (UX 2).
            .min_col_width(180.0)
            .spacing([16.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                for id in &paired {
                    let name = self.name_of(devices, id);
                    let link = self.status.link.get(id).map_or(LINK_AWAY, String::as_str);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).strong());
                        let linked = matches!(link, LINK_DIRECT);
                        let words = RichText::new(link_words(link));
                        ui.label(if linked {
                            words.color(crate::theme::good(ui))
                        } else if link.starts_with("refused") {
                            words.color(crate::theme::warn(ui))
                        } else {
                            words.color(muted)
                        });
                        if !shown.iter().any(|c| c.device_id == *id) {
                            ui.label(
                                RichText::new("It has not said its screens yet.").color(muted),
                            );
                        }
                    });
                    ui.vertical(|ui| {
                        // This computer letting it drive this one: its trust
                        // store entry when it paired with this one, else what
                        // this one keeps for a computer it paired with.
                        let device = devices.iter().find(|d| d.computer && d.id == *id);
                        let mut allowed = match device {
                            Some(d) => d.permissions.allows(Capability::SharedInput),
                            None => self
                                .computers
                                .computers
                                .iter()
                                .any(|c| c.device_id == *id && c.lets_drive),
                        };
                        // Kept by the server for a computer this one paired
                        // with, so asked of it: not while it is stopped.
                        let can = device.is_some() || server_running;
                        if ui
                            .add_enabled(
                                can,
                                egui::Checkbox::new(
                                    &mut allowed,
                                    format!("{name} may drive {this_name}"),
                                ),
                            )
                            .on_hover_text(
                                "Its keyboard and mouse may move onto this computer's screens \
                                 (the Keyboard and mouse permission).",
                            )
                            .on_disabled_hover_text(STOPPED)
                            .changed()
                        {
                            if device.is_some() {
                                action = Some(Action::Allow {
                                    device_id: id.clone(),
                                    allow: allowed,
                                });
                            } else {
                                ask = Some((id.clone(), allowed));
                            }
                        }
                        let (words, warn) = self.drives_it(link, id, &name, &this_name, sharing_on);
                        let words = RichText::new(words);
                        ui.label(if warn {
                            words.color(crate::theme::warn(ui))
                        } else {
                            words.color(muted)
                        });
                    });
                    ui.end_row();
                }
            });
        if let Some((device_id, allow)) = ask {
            let done = if allow {
                "Asked the server to let it drive this computer."
            } else {
                "Asked the server to stop it driving this computer."
            };
            self.notice = crate::notice::Notice::outcome(
                computers::request(
                    state_dir,
                    ComputerAsk::LetDrive { device_id, allow },
                    now_unix,
                )
                .map(|_| done.to_string())
                .map_err(|e| format!("Not asked: {e}")),
            );
            // Read again soon, for the server's answer.
            self.read_at = None;
        }
        if let Some(phones) = self.phones_section(ui, devices, &this_name) {
            action = Some(phones);
        }
        action
    }

    /// Each phone that said its screen (BK5): whether the keyboard and
    /// mouse may go onto it now, and this computer's Keyboard and mouse
    /// permission for it.
    fn phones_section(
        &self,
        ui: &mut egui::Ui,
        devices: &[Device],
        this_name: &str,
    ) -> Option<Action> {
        let phones = self.phones(devices);
        if phones.is_empty() {
            return None;
        }
        let muted = crate::theme::muted(ui);
        let mut action = None;
        ui.add_space(8.0);
        ui.heading("Your phones");
        egui::Grid::new("keyboard_mouse_phones")
            .num_columns(2)
            .min_col_width(180.0)
            .spacing([16.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                for id in &phones {
                    let name = self.name_of(devices, id);
                    let standing = self.status.phones.get(id).map_or("", String::as_str);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&name).strong());
                        let (words, warn) = phone_words(standing, &name, this_name);
                        let words = RichText::new(words);
                        ui.label(if standing == PHONE_READY {
                            words.color(crate::theme::good(ui))
                        } else if warn {
                            words.color(crate::theme::warn(ui))
                        } else {
                            words.color(muted)
                        });
                    });
                    ui.vertical(|ui| {
                        let Some(device) = devices.iter().find(|d| !d.computer && d.id == *id)
                        else {
                            return;
                        };
                        let mut allowed = device.permissions.allows(Capability::SharedInput);
                        if ui
                            .checkbox(&mut allowed, format!("{this_name} may drive {name}"))
                            .on_hover_text(
                                "This computer's keyboard and mouse may move onto the phone, \
                                 used as another screen (its Keyboard and mouse permission). \
                                 The phone must allow it too.",
                            )
                            .changed()
                        {
                            action = Some(Action::Allow {
                                device_id: id.clone(),
                                allow: allowed,
                            });
                        }
                    });
                    ui.end_row();
                }
            });
        action
    }

    /// Whether that computer lets this one drive it, in words, and whether
    /// it is a warning.
    fn drives_it(
        &self,
        link: &str,
        id: &str,
        name: &str,
        this_name: &str,
        sharing_on: bool,
    ) -> (String, bool) {
        if self.status.answered.contains(id) {
            return (format!("{this_name} may drive {name}"), false);
        }
        match link {
            REFUSED_NOT_ALLOWED => (
                format!(
                    "{name} does not let {this_name} drive it: on {name}, turn on \"{this_name} \
                     may drive {name}\" on its Keyboard and mouse page"
                ),
                true,
            ),
            REFUSED_SHARING_OFF => (
                format!("{name} has sharing off: turn it on there, on its Keyboard and mouse page"),
                true,
            ),
            LINK_DIRECT if !sharing_on => (format!("Turn sharing on above to drive {name}"), false),
            LINK_DIRECT => (format!("Waiting for {name} to answer"), false),
            LINK_RELAYED => (
                "Not over a relay: the computers must be on the same network".to_string(),
                false,
            ),
            _ => (format!("Not known until {name} is linked"), false),
        }
    }
}

/// A phone's outline in `r`: rounded, with its earpiece's slot near the
/// top edge of its longer side (BK5).
fn phone_glyph(painter: &egui::Painter, r: Rect, fill: egui::Color32, edge: Stroke) {
    let radius = (r.width().min(r.height()) * 0.18).clamp(2.0, 10.0);
    painter.rect(r, radius, fill, edge, StrokeKind::Inside);
    let slot = (r.width().min(r.height()) * 0.3).clamp(3.0, 16.0);
    let inset = (radius * 0.6).max(2.0);
    let (a, b) = if r.height() >= r.width() {
        let y = r.top() + inset;
        (
            Pos2::new(r.center().x - slot / 2.0, y),
            Pos2::new(r.center().x + slot / 2.0, y),
        )
    } else {
        let x = r.left() + inset;
        (
            Pos2::new(x, r.center().y - slot / 2.0),
            Pos2::new(x, r.center().y + slot / 2.0),
        )
    };
    painter.line_segment([a, b], Stroke::new(1.5, edge.color));
}

/// Why a computer this one paired with cannot be let drive it now.
const STOPPED: &str = "Start the server first: it keeps this for a computer this one paired with";

/// The arrangement to keep: where each computer shown is (`placed`), and
/// where `saved` put a computer still `paired` that is not shown now (its
/// screens not known); any other id is dropped.
pub fn origins_to_save(
    placed: &[(String, (i32, i32))],
    saved: Option<&LayoutFile>,
    paired: &BTreeSet<String>,
) -> BTreeMap<String, (i32, i32)> {
    let mut origins: BTreeMap<String, (i32, i32)> = saved
        .map(|s| {
            s.origins
                .iter()
                .filter(|(id, _)| paired.contains(*id))
                .map(|(id, at)| (id.clone(), *at))
                .collect()
        })
        .unwrap_or_default();
    origins.extend(placed.iter().cloned());
    origins
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(x0: i64, y0: i64, w: i64, h: i64) -> Bounds {
        Bounds {
            x0,
            y0,
            x1: x0 + w,
            y1: y0 + h,
        }
    }

    /// Whether `a` touches `b` along an edge they share for some length.
    fn touches(a: &Bounds, b: &Bounds) -> bool {
        let across = |a0: i64, a1: i64, b0: i64, b1: i64| a0.max(b0) < a1.min(b1);
        ((a.x1 == b.x0 || a.x0 == b.x1) && across(a.y0, a.y1, b.y0, b.y1))
            || ((a.y1 == b.y0 || a.y0 == b.y1) && across(a.x0, a.x1, b.x0, b.x1))
    }

    /// A computer's name as another computer gave it is shown tamed: on
    /// one line, with no character that turns or hides the text around it,
    /// and clipped; this computer's own name too.
    /// UX 2: each step is done by its own fact, and only those not done
    /// say what to do; a label is at most six words.
    #[test]
    fn the_setup_steps_follow_the_facts() {
        let none = checklist(Facts::default());
        assert!(none.iter().all(|s| !s.done));
        let fixes: Vec<Option<Fix>> = none.iter().map(|s| s.fix).collect();
        assert_eq!(
            fixes,
            [
                Some(Fix::StartServer),
                Some(Fix::PairComputer),
                Some(Fix::SharingOn),
                Some(Fix::RunDiagnostics),
                None,
                None
            ]
        );
        let all = checklist(Facts {
            server_running: true,
            paired: 1,
            sharing_on: true,
            linked: 1,
            drives: 1,
            arranged: true,
        });
        assert!(all.iter().all(|s| s.done));
        let some = checklist(Facts {
            server_running: true,
            paired: 2,
            ..Facts::default()
        });
        let done: Vec<bool> = some.iter().map(|s| s.done).collect();
        assert_eq!(done, [true, true, false, false, false, false]);
        for step in none {
            assert!(step.label.split_whitespace().count() <= 6, "{}", step.label);
        }
    }

    /// PR #555 review m6: Setup's fixes do what they say: Turn on turns
    /// sharing on, Start the server asks the app to start it.
    #[test]
    fn setup_fixes_do_what_they_say() {
        use egui_kittest::kittest::Queryable;
        let dir = std::env::temp_dir().join(format!("pd-setup-fix-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let acted = std::rc::Rc::new(std::cell::RefCell::new(None));
        let seen = acted.clone();
        let state = dir.clone();
        let mut view = ArrangeView::default();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            if let Some(action) = view.show(ui, &state, &[], false, false, 0) {
                *seen.borrow_mut() = Some(action);
            }
        });
        harness.run();
        harness.get_by_label("Turn on").click();
        harness.run();
        assert_eq!(*acted.borrow(), Some(Action::Sharing(true)));
        harness.get_by_label(crate::widgets::START).click();
        harness.run();
        assert_eq!(
            crate::widgets::take_ask(&harness.ctx),
            Some(crate::widgets::Ask::StartServer)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PR #555 review m5: a computer that answered but is no longer paired
    /// does not count as one this computer may drive, nor as linked.
    #[test]
    fn setup_counts_only_computers_paired_now() {
        let mut view = ArrangeView::default();
        view.status.answered = BTreeSet::from(["gone".to_string(), "laptop".to_string()]);
        view.status
            .link
            .insert("gone".to_string(), LINK_DIRECT.to_string());
        let none = view.facts(&[], true, true);
        assert_eq!((none.paired, none.linked, none.drives), (0, 0, 0));
        let devices = [Device {
            id: "laptop".to_string(),
            computer: true,
            ..Default::default()
        }];
        let one = view.facts(&devices, true, true);
        assert_eq!((one.paired, one.linked, one.drives), (1, 0, 1));
    }

    #[test]
    fn computers_names_are_shown_tamed() {
        let screens = |id: &str, name: &str, this: bool| shared_input::ComputerScreens {
            device_id: id.to_string(),
            name: name.to_string(),
            screens: vec![Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
            changed_unix: 0,
            this_computer: this,
            phone: None,
        };
        let long = "x".repeat(200);
        let view = ArrangeView {
            screens: ScreensFile {
                computers: vec![
                    screens("me", "Desk\u{2028}top", true),
                    screens("laptop", "Lap\u{202E}top\nevil", false),
                    screens("long", &long, false),
                ],
            },
            ..Default::default()
        };
        assert_eq!(view.name_of(&[], "laptop"), "Lap top evil");
        assert_eq!(view.name_of(&[], "long").chars().count(), NAME_CHARS);
        let devices = [Device {
            id: "laptop".to_string(),
            computer: true,
            ..Default::default()
        }];
        let shown = view.shown(&devices);
        assert_eq!(shown[0].name, "Desk top");
        assert_eq!(shown[1].name, "Lap top evil");
    }

    #[test]
    fn dragged_block_snaps_to_touching_edge() {
        let monitor = block(0, 0, 1920, 1080);
        // Dropped a little off to the right: it closes the gap.
        assert_eq!(snap(block(2000, 30, 1600, 1000), &[monitor]), (1920, 30));
        // Dropped overlapping the left edge: out to the left of it.
        assert_eq!(
            snap(block(-1500, 100, 1600, 1000), &[monitor]),
            (-1600, 100)
        );
        // Dropped below: up against the bottom.
        assert_eq!(snap(block(500, 1500, 1600, 1000), &[monitor]), (500, 1080));
        // Dropped far up to the right: it keeps a quarter of its side
        // against the monitor's, not a corner.
        let (x, y) = snap(block(3000, -2000, 1600, 1000), &[monitor]);
        assert_eq!(x, 1920);
        assert_eq!(y, -1000 + 250);
        assert!(touches(&block(x, y, 1600, 1000), &monitor));
    }

    #[test]
    fn blocks_never_overlap_after_a_drop() {
        let others = [block(0, 0, 1920, 1080), block(1920, 60, 1600, 1000)];
        for x in (-5000..7000).step_by(250) {
            for y in (-3000..4000).step_by(250) {
                let (sx, sy) = snap(block(x, y, 1280, 800), &others);
                let dropped = block(sx, sy, 1280, 800);
                assert!(
                    others.iter().all(|o| !overlaps(&dropped, o)),
                    "{x},{y} -> {sx},{sy}"
                );
                assert!(
                    others.iter().any(|o| touches(&dropped, o)),
                    "{x},{y} -> {sx},{sy} touches one"
                );
            }
        }
        // Touching edges are not overlapping; one pixel over is.
        assert!(!overlaps(&block(0, 0, 10, 10), &block(10, 0, 10, 10)));
        assert!(overlaps(&block(0, 0, 10, 10), &block(9, 9, 10, 10)));
    }

    #[test]
    fn only_computers_paired_now_are_kept_in_the_arrangement() {
        let saved = LayoutFile {
            origins: BTreeMap::from([
                ("me".to_string(), (0, 0)),
                ("asleep".to_string(), (-1600, 0)),
                ("unpaired".to_string(), (0, 1080)),
                ("laptop".to_string(), (1920, 0)),
            ]),
            rev: 4,
        };
        let paired = BTreeSet::from(["asleep".to_string(), "laptop".to_string()]);
        let placed = [
            ("me".to_string(), (0, 0)),
            ("laptop".to_string(), (0, -1000)),
        ];
        assert_eq!(
            origins_to_save(&placed, Some(&saved), &paired),
            BTreeMap::from([
                ("me".to_string(), (0, 0)),
                // Paired, its screens not known now: where it was put.
                ("asleep".to_string(), (-1600, 0)),
                ("laptop".to_string(), (0, -1000)),
            ])
        );
        assert_eq!(
            origins_to_save(&placed, None, &paired),
            BTreeMap::from(placed.clone())
        );
    }

    /// BK5: a paired phone that said its screen is placed with the
    /// computers, drawn as a phone; one not paired is not.
    #[test]
    fn a_paired_phone_is_placed_as_a_phone() {
        use pocketdesk_core::local::shared_input::PhoneScreen;
        let phone = PhoneScreen {
            width_px: 1080,
            height_px: 2400,
            width_dp: 400,
            height_dp: 900,
            rotation: 0,
        };
        let mut screens = ScreensFile::default();
        screens.set_own(
            "me",
            "Desk",
            &[Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
            1,
        );
        screens.heard_phone("oppo", "OPPO", &phone, 1);
        screens.heard_phone("stranger", "Other", &phone, 1);
        let view = ArrangeView {
            screens,
            ..Default::default()
        };
        let devices = [Device {
            id: "oppo".to_string(),
            computer: false,
            ..Default::default()
        }];
        let shown = view.shown(&devices);
        assert_eq!(shown.len(), 2);
        assert!(shown[1].phone && shown[1].device_id == "oppo");
        assert_eq!(shown[1].origin, (1920, 0));
        assert_eq!(view.phones(&devices), BTreeSet::from(["oppo".to_string()]));
        // Its place is kept with the computers'.
        assert!(view.placeable(&devices).contains("oppo"));
    }

    #[test]
    fn a_phones_standing_reads_in_words() {
        let (ready, warn) = phone_words(PHONE_READY, "OPPO", "Desk");
        assert!(ready.starts_with("Ready") && !warn);
        assert!(phone_words(PHONE_OFF, "OPPO", "Desk").1);
        let (service, warn) = phone_words(PHONE_NEEDS_SERVICE, "OPPO", "Desk");
        assert!(service.contains("accessibility service") && warn);
        let (here, warn) = phone_words(PHONE_NOT_ALLOWED, "OPPO", "Desk");
        assert_eq!(here, "Turn on \"Desk may drive OPPO\" to use it");
        assert!(!warn);
        assert!(phone_words("", "OPPO", "Desk").0.starts_with("Not linked"));
        assert!(
            phone_words(PHONE_AWAY, "OPPO", "Desk")
                .0
                .starts_with("Not linked")
        );
    }

    #[test]
    fn each_link_reads_in_words() {
        assert_eq!(link_words(LINK_DIRECT), "Linked directly");
        assert_eq!(
            link_words(REFUSED_SHARING_OFF),
            "Won't take input: sharing off there"
        );
        assert_eq!(
            link_words(REFUSED_NOT_ALLOWED),
            "Won't take input: not allowed there"
        );
        assert_eq!(link_words(LINK_AWAY), "Not linked now");
        assert_eq!(link_words("something new"), "Not linked now");
    }

    /// BK8: how Linux's input capture stands reads in words, a warning
    /// when crossing cannot happen; nothing said is nothing shown.
    #[test]
    fn linuxs_input_capture_reads_in_words() {
        let (ready, warn) = capture_words(CAPTURE_READY).unwrap();
        assert!(ready.starts_with("Ready"), "{ready}");
        assert!(!warn);
        let (ask, warn) = capture_words(CAPTURE_NEEDS_PERMISSION).unwrap();
        assert!(ask.contains("dialog"), "{ask}");
        assert!(warn);
        let (none, warn) = capture_words(CAPTURE_UNSUPPORTED).unwrap();
        assert!(
            none.contains("GNOME 45") && none.contains("Plasma 6.1"),
            "{none}"
        );
        assert!(warn);
        let (scale, warn) = capture_words(CAPTURE_FRACTIONAL_SCALE).unwrap();
        assert!(scale.contains("Fractional scaling"), "{scale}");
        assert!(warn);
        assert_eq!(capture_words(""), None);
        assert_eq!(capture_words("something new"), None);
    }
}
