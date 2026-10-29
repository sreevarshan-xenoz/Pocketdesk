//! Pair a computer nearby, in the app (`pocketdesk_core::local::nearby`).
//! [`NearbyView`] is the top of Pair → Computer: the Pocketdesk computers
//! found on this network by their announcement, live while the page is
//! open, each with Pair (or "Paired"), and this computer's ask as it goes,
//! its matching code large on screen. [`Prompt`] is the other side: a
//! nearby computer's ask, "<name> wants to pair. Code 482 913", with
//! Accept and Decline, over whatever page is open.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile};
use pocketdesk_core::local::nearby::{self, AskState, NearbyRequest};
use pocketdesk_core::tailscale::lan::Browse;

use crate::model::Device;

/// How often the computers file is read again while the page is open.
const REFRESH: Duration = Duration::from_millis(500);
/// How long the list says it is looking before it says why it may be
/// empty.
const LOOKING: Duration = Duration::from_secs(10);

/// The list's heading.
pub const NEARBY: &str = "Nearby";
/// What the list says while it looks.
pub const LOOKING_WORDS: &str = "Looking on this network…";
/// What it says when it found none.
pub const NONE_FOUND: &str = "No computers found.";
/// The box for the asks made from here (PC1's): its owner ticks one too.
pub const MINE_BOX: &str = "Both are mine: ask for full control";
/// The prompt's box (PC1's, worded as on the link).
pub const PROMPT_MINE_BOX: &str = crate::pair_view::MINE_BOX;
/// The line above the link halves.
pub const NOT_LISTED: &str = "Not listed? Use a link.";
/// A row's word for a computer paired already.
pub const PAIRED: &str = "Paired";

/// Why a computer may not show, each a line (the plan's reasons).
pub fn reasons() -> Vec<&'static str> {
    let mut why = vec![
        "Pocketdesk isn't running there.",
        "\"Visible on this network\" is off there.",
        "It's on another network, or a VPN.",
    ];
    if cfg!(windows) {
        why.push("Windows sees this network as Public.");
    }
    why
}

/// How Windows' Public network is changed, said under the reasons.
const PUBLIC_FIX: &str =
    "Settings → Network & internet → the network → Network profile type → Private.";

/// The words for how an ask from here ended (not Accepted: that one's
/// pairing outcome says the rest).
pub fn ended_words(state: &AskState, name: &str) -> Option<String> {
    Some(match state {
        AskState::Asking | AskState::Waiting | AskState::Accepted => return None,
        AskState::Declined => format!("{name} declined, or didn't answer in time."),
        AskState::Busy => format!("{name} is answering another computer. Try again in a minute."),
        AskState::Unavailable => {
            format!("{name} isn't taking asks now. Its Settings may have them off.")
        }
        AskState::Cancelled => "Cancelled.".to_string(),
        AskState::Failed(_) => {
            format!("Couldn't reach {name}. Check both are on the same network.")
        }
    })
}

/// Pair → Computer's Nearby list and this computer's ask.
#[derive(Default)]
pub struct NearbyView {
    browse: Option<Browse>,
    browse_error: Option<String>,
    file: ComputersFile,
    own_id: Option<String>,
    read_at: Option<Instant>,
    /// The request of the ask made from here, while it is shown.
    asking: Option<String>,
    /// Both are mine, for the next ask (PC1).
    mine: bool,
    /// The Public network's fix is shown.
    how: bool,
}

impl NearbyView {
    /// Stop looking (the page was left): nothing listens while nobody
    /// looks.
    pub fn stop(&mut self) {
        self.browse = None;
        self.browse_error = None;
    }

    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.file = computers::read(state_dir);
        self.own_id = pocketdesk_core::local::identity::device_id(state_dir);
        self.read_at = Some(Instant::now());
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        devices: &[Device],
        server_running: bool,
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        if self.browse.is_none() && self.browse_error.is_none() {
            let ctx = ui.ctx().clone();
            match Browse::start(move || ctx.request_repaint()) {
                Ok(browse) => self.browse = Some(browse),
                Err(e) => self.browse_error = Some(e.to_string()),
            }
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        ui.label(RichText::new(NEARBY).heading());
        let under_way = self.ask_progress(ui, state_dir, now_unix);
        let paired: Vec<String> = self
            .file
            .computers
            .iter()
            .map(|c| c.device_id.clone())
            .chain(devices.iter().filter(|d| d.computer).map(|d| d.id.clone()))
            .collect();
        let found = self.browse.as_ref().map(Browse::found).unwrap_or_default();
        let rows = nearby::nearby_list(&found, self.own_id.as_deref().unwrap_or(""), &paired);
        if let Some(e) = &self.browse_error {
            ui.label(
                RichText::new(format!("Can't look on this network: {e}"))
                    .color(crate::theme::bad(ui)),
            );
        } else if rows.is_empty() {
            let looked = self.browse.as_ref().map_or(LOOKING, Browse::looked_for);
            if looked < LOOKING {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(LOOKING_WORDS);
                });
            } else {
                self.none_found(ui);
            }
        }
        let mut ask = None;
        for row in &rows {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(crate::computers_view::shown(&row.found.name)).strong());
                ui.label(
                    RichText::new(format!(
                        "on this network · {}",
                        row.found.addresses.first().map_or("", String::as_str)
                    ))
                    .color(crate::theme::muted(ui)),
                );
                if row.paired {
                    ui.label(RichText::new(PAIRED).color(crate::theme::good(ui)));
                } else if ui
                    .add_enabled(server_running && !under_way, egui::Button::new("Pair"))
                    .on_hover_text("It shows a code; check it matches the one here.")
                    .on_disabled_hover_text(if server_running {
                        "One ask at a time."
                    } else {
                        "Start the server first: it asks."
                    })
                    .clicked()
                {
                    ask = Some(row.found.clone());
                }
            });
        }
        if !rows.is_empty() && !server_running {
            crate::widgets::why_off_ask(
                ui,
                crate::computers_view::PAIR_NEEDS_SERVER,
                crate::widgets::START,
                crate::widgets::Ask::StartServer,
            );
        }
        if !rows.iter().all(|r| r.paired) {
            ui.checkbox(&mut self.mine, MINE_BOX)
                .on_hover_text("Its owner must tick it too. Off for others' computers.");
        }
        if let Some(found) = ask {
            let asked = computers::request(
                state_dir,
                ComputerAsk::PairNearby {
                    device_id: found.device_id,
                    name: found.name,
                    address: found.addresses.first().cloned().unwrap_or_default(),
                    port: found.control_port,
                    mine: self.mine,
                },
                now_unix,
            );
            match asked {
                Ok(request) => {
                    self.asking = Some(request.id);
                    self.read_at = None;
                }
                Err(e) => {
                    ui.label(RichText::new(format!("Not asked: {e}")).color(crate::theme::bad(ui)));
                }
            }
        }
        ui.add_space(8.0);
        ui.separator();
        ui.label(RichText::new(NOT_LISTED).color(crate::theme::muted(ui)));
        ui.add_space(4.0);
    }

    /// The reasons a computer may not show, and what to do.
    fn none_found(&mut self, ui: &mut egui::Ui) {
        ui.label(NONE_FOUND);
        for why in reasons() {
            ui.label(RichText::new(format!("• {why}")).color(crate::theme::muted(ui)));
        }
        ui.horizontal(|ui| {
            if cfg!(windows) && ui.small_button("How to make it Private").clicked() {
                self.how = !self.how;
            }
            if ui.small_button("Run diagnostics").clicked() {
                crate::widgets::ask(ui.ctx(), crate::widgets::Ask::RunDiagnostics);
            }
        });
        if self.how {
            ui.label(RichText::new(PUBLIC_FIX).color(crate::theme::muted(ui)));
        }
    }

    /// This computer's ask, while it goes and as it ended; whether one is
    /// under way.
    fn ask_progress(&mut self, ui: &mut egui::Ui, state_dir: &Path, now_unix: u64) -> bool {
        let Some(request) = self.asking.clone() else {
            return false;
        };
        let Some(out) = self
            .file
            .nearby_out
            .clone()
            .filter(|o| o.request == request)
        else {
            // Not taken by the server yet.
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Asking…");
            });
            return true;
        };
        let name = crate::computers_view::shown(&out.name);
        let mut under_way = !out.state.is_over();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(code) = &out.code {
                ui.label(
                    RichText::new(nearby::spaced(code))
                        .monospace()
                        .size(32.0)
                        .strong(),
                );
            }
            match &out.state {
                AskState::Asking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Asking {name}…"));
                    });
                }
                AskState::Waiting => {
                    ui.label(format!("Waiting for {name} to accept."));
                    ui.label(
                        RichText::new("It must show the same code.").color(crate::theme::muted(ui)),
                    );
                }
                AskState::Accepted => {
                    match self
                        .file
                        .last_pairing
                        .as_ref()
                        .filter(|l| l.request == request)
                    {
                        Some(last) => {
                            let words = crate::computers_view::outcome_words(&last.outcome, &name);
                            let color = if words.ok {
                                crate::theme::good(ui)
                            } else {
                                crate::theme::bad(ui)
                            };
                            ui.label(RichText::new(&words.said).color(color));
                            if words.ok && ui.button("Set up keyboard and mouse").clicked() {
                                crate::widgets::ask(
                                    ui.ctx(),
                                    crate::widgets::Ask::Go(crate::app::Tab::KeyboardMouse),
                                );
                            }
                        }
                        None => {
                            under_way = true;
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("{name} accepted. Pairing…"));
                            });
                        }
                    }
                }
                ended => {
                    if let Some(words) = ended_words(ended, &name) {
                        ui.label(RichText::new(words).color(crate::theme::warn(ui)));
                    }
                    if let AskState::Failed(why) = ended {
                        ui.label(
                            RichText::new(format!("Details: {why}"))
                                .small()
                                .color(crate::theme::muted(ui)),
                        );
                    }
                }
            }
            ui.horizontal(|ui| {
                if !out.state.is_over() && ui.button("Cancel").clicked() {
                    let _ = computers::request(state_dir, ComputerAsk::CancelNearby, now_unix);
                    self.read_at = None;
                }
                if out.state.is_over() && !under_way && ui.button("Done").clicked() {
                    self.asking = None;
                }
            });
        });
        under_way
    }
}

/// A nearby computer's ask on this computer's screen, with Accept and
/// Decline (Escape declines).
#[derive(Default)]
pub struct Prompt {
    shown: Option<NearbyRequest>,
    /// The asks answered here, so one is not shown again before the server
    /// takes the answer.
    answered: Vec<String>,
    /// Both are mine (PC1), off for each new ask.
    mine: bool,
}

/// What the prompt's title says.
pub fn prompt_title(name: &str) -> String {
    format!("{} wants to pair", crate::computers_view::shown(name))
}

impl Prompt {
    /// The ask now on file (read with the app's other state); true when it
    /// is new, for the window to come forward.
    pub fn update(&mut self, asked: Option<NearbyRequest>) -> bool {
        let asked = asked.filter(|r| !self.answered.contains(&r.id));
        let new = asked
            .as_ref()
            .is_some_and(|r| self.shown.as_ref().is_none_or(|s| s.id != r.id));
        if new {
            self.mine = false;
        }
        self.shown = asked;
        new
    }

    /// Draw it while an ask waits; the owner's answer goes to the server.
    pub fn show(&mut self, ctx: &egui::Context, state_dir: &Path, now_unix: u64) {
        let Some(asked) = self.shown.clone() else {
            return;
        };
        let mut answer = None;
        let left = nearby::ANSWER_WITHIN
            .as_secs()
            .saturating_sub(now_unix.saturating_sub(asked.unix));
        egui::Window::new(prompt_title(&asked.name))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("Code");
                ui.label(
                    RichText::new(nearby::spaced(&asked.code))
                        .monospace()
                        .size(32.0)
                        .strong(),
                );
                ui.label(format!("Accept only if {} shows the same.", asked.address));
                ui.checkbox(&mut self.mine, PROMPT_MINE_BOX)
                    .on_hover_text("Both must tick it. Off for others' computers.");
                ui.label(
                    RichText::new(format!("Declined in {left}s if you do nothing."))
                        .color(crate::theme::muted(ui)),
                );
                ui.horizontal(|ui| {
                    if ui.button("Accept").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Decline").clicked() {
                        answer = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        if let Some(accept) = answer {
            let _ = computers::request(
                state_dir,
                ComputerAsk::AnswerNearby {
                    nearby: asked.id.clone(),
                    accept,
                    mine: accept && self.mine,
                },
                now_unix,
            );
            self.answered.push(asked.id);
            self.answered.truncate(16);
            self.shown = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn request(id: &str) -> NearbyRequest {
        NearbyRequest {
            id: id.to_string(),
            device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
            name: "Test Tower".to_string(),
            address: "192.0.2.50".to_string(),
            code: "482913".to_string(),
            unix: pocketdesk_core::local::pin::now_unix(),
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-nearby-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The prompt names the computer and shows its code, spaced; Accept
    /// asks the server once, with both-are-mine only when ticked; the ask
    /// is not shown again.
    #[test]
    fn the_prompt_shows_the_code_and_accept_answers_once() {
        let dir = temp("prompt");
        let mut prompt = Prompt::default();
        assert!(prompt.update(Some(request("a1"))), "new");
        assert!(!prompt.update(Some(request("a1"))), "the same");
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            prompt.show(ui.ctx(), &state, pocketdesk_core::local::pin::now_unix());
        });
        harness.run();
        assert!(
            harness
                .query_by_label(&prompt_title("Test Tower"))
                .is_some()
        );
        assert!(harness.query_by_label("482 913").is_some());
        harness.get_by_label("Accept").click();
        harness.run();
        let now = pocketdesk_core::local::pin::now_unix();
        let asks: Vec<ComputerAsk> = computers::take_all(&dir, now)
            .into_iter()
            .map(|r| r.ask)
            .collect();
        assert_eq!(
            asks,
            [ComputerAsk::AnswerNearby {
                nearby: "a1".to_string(),
                accept: true,
                mine: false,
            }]
        );
        assert!(harness.query_by_label("482 913").is_none(), "answered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Escape declines.
    #[test]
    fn escape_declines_the_prompt() {
        let dir = temp("escape");
        let mut prompt = Prompt::default();
        prompt.update(Some(request("b2")));
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            prompt.show(ui.ctx(), &state, pocketdesk_core::local::pin::now_unix());
        });
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        let now = pocketdesk_core::local::pin::now_unix();
        let asks: Vec<ComputerAsk> = computers::take_all(&dir, now)
            .into_iter()
            .map(|r| r.ask)
            .collect();
        assert_eq!(
            asks,
            [ComputerAsk::AnswerNearby {
                nearby: "b2".to_string(),
                accept: false,
                mine: false,
            }]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How an ask ended reads as words; Accepted says nothing here (its
    /// pairing's outcome does).
    #[test]
    fn an_ended_ask_reads_as_words() {
        assert_eq!(ended_words(&AskState::Accepted, "Test Tower"), None);
        assert!(
            ended_words(&AskState::Busy, "Test Tower")
                .unwrap()
                .contains("another computer")
        );
        assert!(
            ended_words(&AskState::Failed("timeout".into()), "Test Tower")
                .unwrap()
                .contains("same network")
        );
    }

    /// The empty list's reasons each fit a line.
    #[test]
    fn the_reasons_are_short() {
        for why in reasons() {
            assert!(why.split_whitespace().count() <= 8, "{why}");
        }
    }
}
