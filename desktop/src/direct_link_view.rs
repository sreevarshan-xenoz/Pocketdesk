//! The Pair page's direct link (no shared network): "Make a direct link"
//! makes this computer's own Wi-Fi for one phone, for a train, a café with
//! client isolation or a guest network where the phone cannot reach this
//! computer. The phone scans the code, joins that network for Pocketdesk
//! only (its mobile data stays on for everything else) and pairs.
//!
//! This only asks and shows: the server makes and removes the access point
//! (`pocketdesk_core::local::direct_link` is the contract), and the code is
//! the usual pairing link with the network's name and passphrase on the end.
//!
//! Where the card cannot host its own network beside its own connection (or
//! this program has no right to add a second interface), the hotspot takes
//! over the card and this computer's Wi-Fi goes down. That is said before
//! anything starts, and the ask carries the owner's yes.

use std::path::Path;

use eframe::egui::{self, RichText, TextureHandle, TextureOptions};
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::direct_link::{
    self as dl, Action, Band, Mode, Plan, Request, State, Status, Unavailable,
};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::pin;

use crate::model::Device;
use crate::pair_view::{Whose, link_for, qr_image, qr_modules, whose_for};

/// What the card tells the owner before a link that takes over the Wi-Fi.
pub const DISCONNECT_WARNING: &str =
    "This computer's Wi-Fi will disconnect while the direct link is on.";
/// The button that asks, and the one that asks knowing the Wi-Fi goes down.
pub const MAKE: &str = "Make a direct link";
pub const MAKE_AND_DISCONNECT: &str = "Disconnect Wi-Fi and make the link";
pub const TURN_OFF: &str = "Turn off";

/// How long a start that has not been answered is waited for before it is
/// said to have gone nowhere.
const ASK_WAIT_SECS: u64 = 90;

/// What the card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Settings has direct links off, or this system makes none yet: nothing.
    Hidden,
    /// The server is not running, so nothing could make the link.
    NoServer,
    /// This computer's Wi-Fi card is being looked at.
    Checking,
    /// No link can be made here, and why.
    Unavailable(Unavailable),
    /// Ready to make one.
    Ready,
    /// Asked, and the server is bringing it up.
    Starting,
    /// On: the code is shown.
    On,
    /// The server could not make it, or it did not answer; the words.
    Failed(String),
}

/// The stage from what is known now. Pure, so a test reads it.
pub fn stage(
    offered: bool,
    supported: bool,
    server_running: bool,
    plan: Option<&Result<Plan, Unavailable>>,
    status: Option<&Status>,
    asked_unix: Option<u64>,
    now_unix: u64,
) -> Stage {
    if !offered || !supported {
        return Stage::Hidden;
    }
    // A status a stopped server left says nothing now, nor does one from
    // before the ask just made (an old failure beside the new try).
    let status = status
        .filter(|s| s.is_fresh(now_unix) && server_running)
        .filter(|s| asked_unix.is_none_or(|at| s.updated_unix >= at));
    match status {
        Some(s) if s.state == State::On => return Stage::On,
        Some(s) if s.state == State::Starting => return Stage::Starting,
        Some(s) if s.state == State::Failed => {
            return Stage::Failed(
                s.error
                    .clone()
                    .unwrap_or_else(|| "It could not be made.".into()),
            );
        }
        _ => {}
    }
    if !server_running {
        return Stage::NoServer;
    }
    match asked_unix {
        Some(at) if now_unix.saturating_sub(at) <= ASK_WAIT_SECS => return Stage::Starting,
        Some(_) => {
            return Stage::Failed("The server did not answer. Is it running?".to_string());
        }
        None => {}
    }
    match plan {
        None => Stage::Checking,
        Some(Err(why)) => Stage::Unavailable(*why),
        Some(Ok(_)) => Stage::Ready,
    }
}

/// The card's state between frames.
#[derive(Default)]
pub struct DirectLinkCard {
    /// What the server last said a link could do here (it looks again every
    /// half minute, and when a link ends). The app never asks the system
    /// itself: on Windows that is WinRT, which stays in the server.
    plan: Option<Result<Plan, Unavailable>>,
    /// The owner clicked "Make" on a card that will drop the Wi-Fi, and is
    /// being asked once more.
    confirming: bool,
    /// When a start was asked, until the server answers.
    asked_unix: Option<u64>,
    status: Option<Status>,
    status_read: Option<std::time::Instant>,
    plan_read: Option<std::time::Instant>,
    /// The devices before the link was on; a new one is the phone that
    /// paired over it.
    known: Option<Vec<String>>,
    paired: bool,
    /// The PIN the code on screen was made from.
    shown_pin: Option<String>,
    texture: Option<TextureHandle>,
    error: Option<String>,
}

impl DirectLinkCard {
    /// The page was left: the code's PIN is taken off file (a code nobody
    /// looks at stays valid for no one). The link itself stays on until it
    /// is turned off or goes idle.
    pub fn leave(&mut self, state_dir: &Path) {
        self.drop_code(state_dir);
        self.confirming = false;
    }

    /// Forget the code on screen and take its PIN off file.
    fn drop_code(&mut self, state_dir: &Path) {
        if let Some(shown) = self.shown_pin.take() {
            let _ = pin::remove_if(state_dir, &shown);
        }
        self.texture = None;
    }

    /// Draw the card.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: Option<&Config>,
        devices: &[Device],
        server_running: bool,
        now_unix: u64,
    ) {
        let default = Config::default();
        let config = config.unwrap_or(&default);
        // Linux makes it through NetworkManager, Windows through the Mobile
        // Hotspot.
        let supported = cfg!(any(target_os = "linux", windows));
        if !config.direct_link || !supported {
            // Off in Settings while one is on: the server turns it off.
            self.leave(state_dir);
            return;
        }
        self.read_status(state_dir);
        self.read_plan(state_dir, now_unix);
        if self.plan.is_none() {
            // Waiting for the server's first word.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(1000));
        }
        let stage = stage(
            config.direct_link,
            supported,
            server_running,
            self.plan.as_ref(),
            self.status.as_ref(),
            self.asked_unix,
            now_unix,
        );
        // A status (or the wait running out) answers the ask.
        if matches!(stage, Stage::On | Stage::Failed(_)) {
            self.asked_unix = None;
        }
        ui.add_space(14.0);
        ui.separator();
        ui.add_space(8.0);
        crate::theme::section(ui, "Direct link");
        match stage {
            Stage::Hidden => {}
            Stage::NoServer => {
                ui.label("Start the server to make a direct link.");
            }
            Stage::Checking => {
                ui.label("Looking at this computer's Wi-Fi…");
            }
            Stage::Unavailable(why) => {
                ui.label(why.words());
            }
            Stage::Ready | Stage::Failed(_) => {
                self.draw_ready(ui, state_dir, config, now_unix, &stage);
            }
            Stage::Starting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Making the direct link…");
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(500));
            }
            Stage::On => self.draw_on(ui, state_dir, config, devices, now_unix),
        }
        if !matches!(stage, Stage::On) {
            self.known = None;
            self.paired = false;
            self.drop_code(state_dir);
        }
    }

    /// The server's plan on file, read about once a second: it is recent or
    /// it says nothing (a stopped server's is not believed).
    fn read_plan(&mut self, state_dir: &Path, now_unix: u64) {
        let due = self
            .plan_read
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(1000));
        if due {
            self.plan_read = Some(std::time::Instant::now());
            self.plan = dl::read_plan(state_dir, now_unix);
        }
    }

    fn read_status(&mut self, state_dir: &Path) {
        let due = self
            .status_read
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(1000));
        if due {
            self.status_read = Some(std::time::Instant::now());
            self.status = dl::read_status(state_dir);
        }
    }

    fn ask(
        &mut self,
        state_dir: &Path,
        config: &Config,
        action: Action,
        takeover: bool,
        now_unix: u64,
    ) {
        let request = Request::new(
            action,
            Band::from_setting(&config.direct_link_band),
            config.direct_link_idle_minutes,
            takeover,
            now_unix,
        );
        match dl::request(state_dir, &request) {
            Ok(()) => {
                self.error = None;
                if action == Action::Start {
                    self.asked_unix = Some(now_unix);
                    // The old failure is not shown beside the new try.
                    self.status = None;
                    self.status_read = Some(std::time::Instant::now());
                }
            }
            Err(e) => self.error = Some(format!("Could not ask the server: {e}")),
        }
    }

    fn draw_ready(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: &Config,
        now_unix: u64,
        stage: &Stage,
    ) {
        ui.label(
            "No shared network, a café with client isolation or a guest Wi-Fi? Make this \
             computer's own Wi-Fi for one phone: it joins for Pocketdesk only, and keeps its \
             mobile data for everything else.",
        );
        ui.label(
            RichText::new(format!(
                "It turns itself off after {} minute{} with no phone using it.",
                config.direct_link_idle_minutes,
                if config.direct_link_idle_minutes == 1 {
                    ""
                } else {
                    "s"
                },
            ))
            .color(crate::theme::muted(ui)),
        );
        if let Stage::Failed(why) = stage {
            ui.add_space(4.0);
            ui.label(RichText::new(why).color(crate::theme::bad(ui)));
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(crate::theme::bad(ui)));
        }
        let disconnects = self
            .plan
            .as_ref()
            .and_then(|p| p.as_ref().ok())
            .is_some_and(|p| p.disconnects);
        ui.add_space(6.0);
        // Said before anything starts, not after.
        if disconnects {
            ui.label(
                RichText::new(DISCONNECT_WARNING)
                    .color(crate::theme::warn(ui))
                    .strong(),
            );
            ui.add_space(4.0);
        }
        if self.confirming && disconnects {
            ui.horizontal(|ui| {
                if ui.button(MAKE_AND_DISCONNECT).clicked() {
                    self.confirming = false;
                    self.ask(state_dir, config, Action::Start, true, now_unix);
                }
                if ui.button("Cancel").clicked() {
                    self.confirming = false;
                }
            });
        } else if ui.button(MAKE).clicked() {
            if disconnects {
                self.confirming = true;
            } else {
                self.ask(state_dir, config, Action::Start, false, now_unix);
            }
        }
    }

    fn draw_on(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: &Config,
        devices: &[Device],
        now_unix: u64,
    ) {
        let Some(status) = self.status.clone() else {
            return;
        };
        let known = self
            .known
            .get_or_insert_with(|| devices.iter().map(|d| d.id.clone()).collect());
        let new_phone = devices
            .iter()
            .any(|d| !d.computer && !known.contains(&d.id));
        if !self.paired && new_phone {
            // One code, one phone: the server took the PIN when it paired.
            self.paired = true;
            self.drop_code(state_dir);
        }
        if status.mode == Some(Mode::TakesOverCard) {
            ui.label(
                RichText::new("This computer's Wi-Fi is off while the direct link is on.")
                    .color(crate::theme::warn(ui)),
            );
            ui.add_space(4.0);
        }
        if self.paired {
            ui.label(
                RichText::new("Paired ✓  The phone can connect now.")
                    .color(crate::theme::good(ui))
                    .strong()
                    .size(18.0),
            );
        } else {
            self.draw_code(ui, state_dir, config, &status, now_unix);
        }
        ui.add_space(6.0);
        let left = status.idle_left_secs.unwrap_or(0);
        if status.in_use {
            ui.label("A phone is using it.");
        } else {
            ui.label(
                RichText::new(format!(
                    "No phone is using it. It turns off in {}:{:02}.",
                    left / 60,
                    left % 60
                ))
                .color(crate::theme::muted(ui)),
            );
        }
        ui.add_space(4.0);
        if ui.button(TURN_OFF).clicked() {
            self.leave(state_dir);
            self.ask(state_dir, config, Action::Stop, false, now_unix);
            // Shown as off at once; the server's next word confirms it.
            self.status = None;
            self.status_read = Some(std::time::Instant::now());
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// The code: the pairing link with the network on the end. Made again
    /// when its PIN ran out.
    fn draw_code(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        config: &Config,
        status: &Status,
        now_unix: u64,
    ) {
        let on_file = pin::read(state_dir, now_unix);
        let stale = self.texture.is_none()
            || match (&self.shown_pin, &on_file) {
                (Some(shown), Some(file)) => *shown != file.pin,
                _ => true,
            };
        if stale {
            self.make_code(ui, state_dir, config, status, now_unix);
        }
        if let Some(texture) = &self.texture {
            let size = ui.available_width().clamp(180.0, 260.0);
            ui.horizontal_top(|ui| {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(egui::vec2(size, size))
                        .alt_text("Direct link code to scan with the phone"),
                );
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.label("In the app, tap \"Scan pairing code\".");
                    ui.label(
                        "The phone asks to join this computer's Wi-Fi. Allow it: it is used \
                         for Pocketdesk only.",
                    );
                    if let Some(file) = &on_file {
                        let remaining = pin::remaining(file, now_unix).as_secs();
                        ui.label(
                            RichText::new(format!(
                                "PIN {}  ·  valid for {}:{:02}",
                                crate::pair_view::spaced_pin(&file.pin),
                                remaining / 60,
                                remaining % 60
                            ))
                            .color(crate::theme::muted(ui)),
                        );
                    }
                });
            });
        }
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(crate::theme::bad(ui)));
        }
    }

    fn make_code(
        &mut self,
        ui: &egui::Ui,
        state_dir: &Path,
        config: &Config,
        status: &Status,
        now_unix: u64,
    ) {
        let (Some(wifi), Some(address)) = (status.wifi.clone(), status.address.clone()) else {
            return;
        };
        // The code on file's own kind of phone is kept (a guest's code made
        // on the page above is not replaced by this one's), else the
        // owner's.
        let whose = whose_for(false, Whose::MINE, pin::read(state_dir, now_unix).as_ref());
        match link_for(state_dir, config, now_unix, vec![address], whose) {
            Ok(link) => {
                let link = PairLink {
                    mine: false,
                    wifi: Some(wifi),
                    ..link
                };
                match qr_modules(&link.to_uri()) {
                    Ok(modules) => {
                        self.texture = Some(ui.ctx().load_texture(
                            "direct-link-qr",
                            qr_image(&modules, 4),
                            TextureOptions::NEAREST,
                        ));
                        self.shown_pin = Some(link.pin.clone());
                        self.error = None;
                    }
                    Err(e) => self.error = Some(format!("Could not draw the code: {e}")),
                }
            }
            Err(e) => self.error = Some(format!("Could not make a pairing code: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::direct_link::WifiJoin;

    fn plan(disconnects: bool) -> Result<Plan, Unavailable> {
        Ok(Plan {
            device: "wlan0".into(),
            mode: if disconnects {
                Mode::TakesOverCard
            } else {
                Mode::SecondInterface
            },
            channel: None,
            disconnects,
        })
    }

    fn status(state: State, updated: u64) -> Status {
        Status {
            state,
            mode: Some(Mode::TakesOverCard),
            wifi: Some(WifiJoin {
                ssid: "pocketdesk-ab12".into(),
                passphrase: "Abcdef123456".into(),
            }),
            address: Some("192.0.2.1".into()),
            started_unix: 100,
            updated_unix: updated,
            idle_minutes: 10,
            idle_left_secs: Some(300),
            in_use: false,
            error: (state == State::Failed).then(|| "No card.".to_string()),
        }
    }

    #[test]
    fn the_card_is_hidden_when_off_or_not_here() {
        let p = plan(false);
        assert_eq!(
            stage(false, true, true, Some(&p), None, None, 1_000),
            Stage::Hidden
        );
        assert_eq!(
            stage(true, false, true, Some(&p), None, None, 1_000),
            Stage::Hidden
        );
    }

    #[test]
    fn the_card_says_what_it_is_waiting_for() {
        let p = plan(true);
        assert_eq!(
            stage(true, true, false, Some(&p), None, None, 1_000),
            Stage::NoServer
        );
        assert_eq!(
            stage(true, true, true, None, None, None, 1_000),
            Stage::Checking
        );
        assert_eq!(
            stage(
                true,
                true,
                true,
                Some(&Err(Unavailable::NoWifi)),
                None,
                None,
                1_000
            ),
            Stage::Unavailable(Unavailable::NoWifi)
        );
        assert_eq!(
            stage(true, true, true, Some(&p), None, None, 1_000),
            Stage::Ready
        );
        // Asked: starting, until the server's word or the wait runs out.
        assert_eq!(
            stage(true, true, true, Some(&p), None, Some(990), 1_000),
            Stage::Starting
        );
        assert!(matches!(
            stage(
                true,
                true,
                true,
                Some(&p),
                None,
                Some(1_000 - ASK_WAIT_SECS - 1),
                1_000
            ),
            Stage::Failed(_)
        ));
    }

    #[test]
    fn the_servers_word_wins_while_it_is_fresh_and_running() {
        let p = plan(true);
        let on = status(State::On, 995);
        assert_eq!(
            stage(true, true, true, Some(&p), Some(&on), None, 1_000),
            Stage::On
        );
        let starting = status(State::Starting, 999);
        assert_eq!(
            stage(true, true, true, Some(&p), Some(&starting), None, 1_000),
            Stage::Starting
        );
        let failed = status(State::Failed, 999);
        assert_eq!(
            stage(true, true, true, Some(&p), Some(&failed), Some(990), 1_000),
            Stage::Failed("No card.".to_string())
        );
        // A failure from before the ask just made is not shown for it.
        assert_eq!(
            stage(
                true,
                true,
                true,
                Some(&p),
                Some(&failed),
                Some(1_000),
                1_001
            ),
            Stage::Starting
        );
        // A status a stopped server left (old, or no server) is not "on".
        assert_eq!(
            stage(true, true, true, Some(&p), Some(&on), None, 2_000),
            Stage::Ready
        );
        assert_eq!(
            stage(true, true, false, Some(&p), Some(&on), None, 1_000),
            Stage::NoServer
        );
    }
}
