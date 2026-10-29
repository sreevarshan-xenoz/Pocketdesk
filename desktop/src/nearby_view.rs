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
/// How long an ask waits for the server to take it.
const TAKEN_WITHIN: Duration = Duration::from_secs(15);

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
/// What the asking computer's owner is asked once both show the code.
pub const SAME_CODE: &str = "Same code on both?";
/// The button that confirms it.
pub const CONFIRM: &str = "Confirm";

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

/// The owner's note when a nearby pairing ask failed here (at `tried`,
/// unix seconds) within the last ten minutes. It accuses nobody: an
/// honest computer whose connection dropped fails the same way.
pub fn tried_note(tried: Option<u64>, now_unix: u64) -> Option<String> {
    let tried = tried?;
    let ago = now_unix.saturating_sub(tried);
    (ago < nearby::BUDGET_WINDOW.as_secs()).then(|| {
        let minutes = ago / 60;
        let when = match minutes {
            0 => "just now".to_string(),
            1 => "a minute ago".to_string(),
            m => format!("{m} minutes ago"),
        };
        format!("A nearby pairing ask failed (a wrong code or a dropped connection), {when}.")
    })
}

/// The owner's note while nearby asks are paused here (until `until`, unix
/// seconds), with when they start again: as a local clock time when this
/// computer's local time is known (`local_minute`, minutes past midnight
/// now), else in minutes. And how to turn them off.
pub fn paused_note(until: Option<u64>, now_unix: u64, local_minute: Option<u16>) -> Option<String> {
    let left = until?.checked_sub(now_unix).filter(|left| *left > 0)?;
    let minutes = left.div_ceil(60);
    let when = match local_minute {
        Some(at) => {
            let then = (u64::from(at) + minutes) % (24 * 60);
            format!("until {:02}:{:02}", then / 60, then % 60)
        }
        None => format!("for about {minutes} more minutes"),
    };
    Some(format!(
        "Nearby asks are paused {when}. \"{OFF_SWITCH}\" in Settings turns them off."
    ))
}

/// Settings' switch for nearby asks, as the notes name it.
pub const OFF_SWITCH: &str = "Let computers nearby ask to pair";

/// The words for how an ask from here ended (not Accepted: that one's
/// pairing outcome says the rest).
pub fn ended_words(state: &AskState, name: &str) -> Option<String> {
    Some(match state {
        AskState::Asking | AskState::Confirm | AskState::Waiting | AskState::Accepted => {
            return None;
        }
        AskState::Declined => format!("{name} declined, or didn't answer in time."),
        AskState::Busy => format!("{name} is answering another computer. Try again in a minute."),
        AskState::Unavailable => {
            format!("{name} isn't taking asks now (off, or paused). Pair with a link below.")
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
    /// The request of the ask made from here, while it is shown, and when
    /// it was made.
    asking: Option<(String, Instant)>,
    /// What the tests list in place of a look on the network (no test
    /// opens mDNS sockets on the machine it runs on).
    #[cfg(test)]
    pub found_for_tests: Vec<nearby::Found>,
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
        if self.browse.is_none() && self.browse_error.is_none() && !cfg!(test) {
            let ctx = ui.ctx().clone();
            match Browse::start(move || ctx.request_repaint()) {
                Ok(browse) => self.browse = Some(browse),
                Err(e) => self.browse_error = Some(e.to_string()),
            }
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        ui.label(RichText::new(NEARBY).heading());
        if let Some(note) = tried_note(self.file.nearby_tried, now_unix) {
            ui.label(RichText::new(note).color(crate::theme::warn(ui)));
        }
        let local_minute = pocketdesk_core::local::quiet_hours::local_now().map(|t| t.minute);
        if let Some(note) = paused_note(self.file.nearby_paused_until, now_unix, local_minute) {
            ui.label(RichText::new(note).color(crate::theme::warn(ui)));
        }
        let under_way = self.ask_progress(ui, state_dir, now_unix);
        let paired: Vec<String> = self
            .file
            .computers
            .iter()
            .map(|c| c.device_id.clone())
            .chain(devices.iter().filter(|d| d.computer).map(|d| d.id.clone()))
            .collect();
        #[cfg(not(test))]
        let found = self.browse.as_ref().map(Browse::found).unwrap_or_default();
        #[cfg(test)]
        let found = self.found_for_tests.clone();
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
                    self.asking = Some((request.id, Instant::now()));
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
        let Some((request, asked_at)) = self.asking.clone() else {
            return false;
        };
        let Some(out) = self
            .file
            .nearby_out
            .clone()
            .filter(|o| o.request == request)
        else {
            // Not taken by the server yet: it may have stopped.
            if asked_at.elapsed() > TAKEN_WITHIN {
                let done = crate::widgets::why_off(
                    ui,
                    "The server didn't take the ask. Is it running?",
                    Some("Done"),
                );
                if done {
                    self.asking = None;
                }
                return false;
            }
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
                AskState::Confirm => {
                    ui.label(RichText::new(SAME_CODE).strong());
                    ui.label(
                        RichText::new(format!("Confirm only if {name} shows it too."))
                            .color(crate::theme::muted(ui)),
                    );
                    if ui.button(CONFIRM).clicked() {
                        let _ = computers::request(
                            state_dir,
                            ComputerAsk::ConfirmNearby {
                                nearby: request.clone(),
                            },
                            now_unix,
                        );
                        self.read_at = None;
                    }
                }
                AskState::Waiting => {
                    ui.label(format!("Confirmed. Waiting for {name} to accept."));
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
                ui.label(format!(
                    "Accept only if {} shows the same code.",
                    crate::computers_view::shown(&asked.name)
                ));
                ui.label(
                    RichText::new(format!("Asked from {}.", asked.address))
                        .color(crate::theme::muted(ui)),
                );
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
            // The newest few: an id is never asked again.
            if self.answered.len() > 16 {
                self.answered.remove(0);
            }
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

    fn found(id: &str, name: &str, address: &str) -> nearby::Found {
        nearby::Found {
            device_id: id.to_string(),
            name: name.to_string(),
            addresses: vec![address.to_string()],
            control_port: 1716,
            stream_port: 4716,
        }
    }

    /// The list shows each computer found, a paired one as "Paired"; Pair
    /// asks the server for that computer as found, with both-are-mine
    /// when ticked; then its code shows large, with whom it waits for.
    #[test]
    fn pair_on_a_nearby_computer_asks_and_shows_the_code() {
        let dir = temp("list");
        computers::update(&dir, |file| {
            file.computers.push(computers::PeerComputer {
                device_id: "00000000-0000-4000-8000-0000000000e2".to_string(),
                name: "Old Desk".to_string(),
                addresses: Vec::new(),
                control_port: 1716,
                stream_port: 4716,
                fingerprint: String::new(),
                endpoint: None,
                paired_unix: 0,
                link: String::new(),
                link_unix: 0,
                lets_drive: false,
                mine: false,
            });
        })
        .unwrap();
        let mut view = NearbyView {
            found_for_tests: vec![
                found(
                    "00000000-0000-4000-8000-0000000000e1",
                    "Test Tower",
                    "192.0.2.80",
                ),
                found(
                    "00000000-0000-4000-8000-0000000000e2",
                    "Old Desk",
                    "192.0.2.81",
                ),
            ],
            ..NearbyView::default()
        };
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            view.show(
                ui,
                &state,
                &[],
                true,
                pocketdesk_core::local::pin::now_unix(),
            );
        });
        harness.run_steps(2);
        assert!(harness.query_by_label("Test Tower").is_some());
        assert!(
            harness.query_by_label(PAIRED).is_some(),
            "Old Desk is paired"
        );
        harness.get_by_label(MINE_BOX).click();
        harness.run_steps(2);
        harness.get_by_label("Pair").click();
        harness.run_steps(2);
        let now = pocketdesk_core::local::pin::now_unix();
        let taken = computers::take_all(&dir, now);
        assert_eq!(taken.len(), 1);
        assert_eq!(
            taken[0].ask,
            ComputerAsk::PairNearby {
                device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
                name: "Test Tower".to_string(),
                address: "192.0.2.80".to_string(),
                port: 1716,
                mine: true,
            }
        );
        // The server shows the code: the owner here is asked to confirm it.
        let says = |state: AskState| {
            computers::update(&dir, |file| {
                file.nearby_out = Some(nearby::NearbyAsk {
                    request: taken[0].id.clone(),
                    device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
                    name: "Test Tower".to_string(),
                    code: Some("482913".to_string()),
                    state,
                    unix: now,
                });
            })
            .unwrap();
        };
        says(AskState::Confirm);
        std::thread::sleep(REFRESH);
        harness.run_steps(2);
        assert!(harness.query_by_label("482 913").is_some());
        assert!(harness.query_by_label(SAME_CODE).is_some());
        harness.get_by_label(CONFIRM).click();
        harness.run_steps(2);
        let confirmed: Vec<ComputerAsk> =
            computers::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .into_iter()
                .map(|r| r.ask)
                .collect();
        assert_eq!(
            confirmed,
            [ComputerAsk::ConfirmNearby {
                nearby: taken[0].id.clone(),
            }]
        );
        // Confirmed: it waits for the other's owner.
        says(AskState::Waiting);
        std::thread::sleep(REFRESH);
        harness.run_steps(2);
        assert!(harness.query_by_label(CONFIRM).is_none());
        assert!(
            harness
                .query_by_label("Confirmed. Waiting for Test Tower to accept.")
                .is_some()
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

    /// A failed ask here shows as a note for ten minutes, accusing nobody.
    #[test]
    fn a_failed_ask_here_shows_a_note() {
        assert_eq!(tried_note(None, 1_000), None);
        assert!(
            tried_note(Some(1_000), 1_010)
                .unwrap()
                .starts_with("A nearby pairing ask failed (a wrong code or a dropped connection)")
        );
        assert!(
            tried_note(Some(1_000), 1_130)
                .unwrap()
                .contains("2 minutes ago")
        );
        assert_eq!(tried_note(Some(1_000), 1_600), None);
    }

    /// While asks are paused the note says until when, by the clock when
    /// it is known, and names the switch that turns them off; none after.
    #[test]
    fn a_pause_says_until_when_and_how_to_turn_asks_off() {
        let note = paused_note(Some(1_600), 1_000, Some(14 * 60 + 5)).unwrap();
        assert!(note.contains("until 14:15"), "{note}");
        assert!(note.contains(OFF_SWITCH));
        let late = paused_note(Some(1_600), 1_000, Some(23 * 60 + 55)).unwrap();
        assert!(late.contains("until 00:05"), "{late}");
        let unknown = paused_note(Some(1_600), 1_000, None).unwrap();
        assert!(unknown.contains("for about 10 more minutes"), "{unknown}");
        assert_eq!(paused_note(Some(1_600), 1_600, Some(0)), None);
        assert_eq!(paused_note(None, 1_000, Some(0)), None);
    }

    /// Refused as unavailable, the asking side points to the link.
    #[test]
    fn unavailable_points_to_the_link() {
        let words = ended_words(&AskState::Unavailable, "Test Tower").unwrap();
        assert!(
            words.contains("paused") && words.contains("link"),
            "{words}"
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
