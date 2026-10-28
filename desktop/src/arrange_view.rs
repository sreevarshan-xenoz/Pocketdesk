//! The Keyboard and mouse page: one keyboard and mouse across the owner's
//! computers. The sharing switch (`shared_input`); the computers' screens as
//! blocks, each dragged to where it sits on the desk, where it snaps so one
//! of its edges touches another's (this computer stays put), kept in
//! `shared_layout.json`; Identify; where the keyboard and mouse are now; and
//! for each paired computer, its link and the Keyboard and mouse permission
//! both ways. The server writes what this reads (`shared_screens.json`,
//! `shared_input_status.json`) and takes its requests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile};
use pocketdesk_core::local::shared_input::{
    self, Bounds, LINK_AWAY, LINK_DIRECT, LINK_RELAYED, LayoutFile, REFUSED_NOT_ALLOWED,
    REFUSED_SHARING_OFF, Screen, ScreensFile, ShareAsk, StatusFile, bounds,
};
use pocketdesk_core::protocol::permissions::Capability;

use crate::model::Device;

/// How often the files are read again while the page is open.
const REFRESH: Duration = Duration::from_secs(1);

/// The canvas's height, in points.
const CANVAS_HEIGHT: f32 = 220.0;

/// What the page asks the window to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Turn the shared keyboard and mouse on or off here (`shared_input`).
    Sharing(bool),
    /// Let the paired computer `device_id` drive this one, or not: its
    /// Keyboard and mouse permission in the trust store (a computer that
    /// paired with this one).
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

    fn name_of(&self, devices: &[Device], id: &str) -> String {
        if let Some(c) = self.screens.get(id).filter(|c| !c.name.is_empty()) {
            return c.name.clone();
        }
        if let Some(c) = self.computers.computers.iter().find(|c| c.device_id == id) {
            return c.name.clone();
        }
        crate::model::device_label(devices, id)
    }

    /// This computer and the paired computers that said their screens,
    /// where they sit.
    fn shown(&self, devices: &[Device]) -> Vec<Shown> {
        let Some(me) = self.screens.this_computer() else {
            return Vec::new();
        };
        let paired = self.paired(devices);
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
            name: me.name.clone(),
            screens: me.screens.clone(),
            origin: (0, 0),
            this: true,
        }];
        shown.extend(
            others
                .into_iter()
                .zip(origins)
                .map(|((id, screens), origin)| Shown {
                    name: self.name_of(devices, &id),
                    device_id: id,
                    screens,
                    origin,
                    this: false,
                }),
        );
        shown
    }

    /// Keep where each computer is, with `id` dropped at `origin`.
    fn save(&mut self, state_dir: &Path, shown: &[Shown], id: &str, origin: (i32, i32)) {
        let origins: BTreeMap<String, (i32, i32)> = shown
            .iter()
            .map(|c| {
                let at = if c.device_id == id { origin } else { c.origin };
                (c.device_id.clone(), at)
            })
            .collect();
        let layout = LayoutFile {
            origins,
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
    fn canvas(&mut self, ui: &mut egui::Ui, state_dir: &Path, shown: &[Shown]) {
        let width = ui.available_width().max(200.0);
        let (canvas, _) = ui.allocate_exact_size(Vec2::new(width, CANVAS_HEIGHT), Sense::hover());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 6.0, ui.visuals().extreme_bg_color);
        // Where each block is now, the one dragged where the pointer has it.
        let mut placed: Vec<(usize, Bounds)> = Vec::new();
        for (i, c) in shown.iter().enumerate() {
            let origin = match &self.drag {
                Some(d) if d.device_id == c.device_id => (
                    d.from.0 + (d.moved.x / d.view.scale).round() as i32,
                    d.from.1 + (d.moved.y / d.view.scale).round() as i32,
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
                painter.rect(r, 2.0, fill, edge, StrokeKind::Inside);
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
                self.save(state_dir, shown, &id, origin);
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
        let mut on = sharing_on;
        if ui
            .checkbox(&mut on, "Share my keyboard and mouse with my computers")
            .on_hover_text(
                "One keyboard and mouse across your own paired computers, on Windows and Linux. \
                 The same switch is in Settings, Keyboard and mouse.",
            )
            .changed()
        {
            action = Some(Action::Sharing(on));
        }
        if !server_running {
            ui.label(
                RichText::new(
                    "The server is not running: start it on the Overview for any of this to work.",
                )
                .color(crate::theme::warn(ui)),
            );
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
            self.canvas(ui, state_dir, &shown);
            ui.label(
                RichText::new(
                    "Drag a computer to where it sits on your desk: it snaps to the nearest edge \
                     of another. This computer stays where it is.",
                )
                .color(muted),
            );
            if shown.len() == 1 {
                ui.label(
                    RichText::new(
                        "No other computer has said its screens yet: pair one (Settings, Other \
                         computers), turn sharing on there too, and it shows up here.",
                    )
                    .color(muted),
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
        ui.add_space(8.0);
        ui.heading("Your computers");
        let paired = self.paired(devices);
        if paired.is_empty() {
            ui.label(
                RichText::new(
                    "No other computer is paired with this one. Pair one in Settings, Other \
                     computers, with its pairing link.",
                )
                .color(muted),
            );
            return action;
        }
        let mut ask: Option<(String, bool)> = None;
        egui::Grid::new("keyboard_mouse_computers")
            .num_columns(2)
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
                        if ui
                            .checkbox(&mut allowed, format!("{name} may drive {this_name}"))
                            .on_hover_text(
                                "Its keyboard and mouse may move onto this computer's screens \
                                 (the Keyboard and mouse permission).",
                            )
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
            LINK_DIRECT if !cfg!(windows) => (
                "Driving another computer from here comes later; this one can be driven"
                    .to_string(),
                false,
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
}
