//! The window: status, paired devices, live sessions and the server log.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use linux_link_core::local::config::Config;
use linux_link_core::local::paths::TRUST_FILE;
use linux_link_core::local::pin::now_unix;
use linux_link_core::local::sessions;
use linux_link_core::protocol::kdeconnect::TrustStore;

use crate::model::{self, Snapshot};
use crate::pair_view::PairView;
use crate::settings_view::SettingsForm;
use crate::tray::{Tray, TrayAction};

const REFRESH: Duration = Duration::from_secs(1);
const GOOD: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x6a);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x93, 0x91);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Pair,
    Devices,
    Sessions,
    Settings,
    Logs,
}

pub struct PocketdeskApp {
    state_dir: PathBuf,
    tray: Option<Tray>,
    tab: Tab,
    snapshot: Snapshot,
    config: Option<Config>,
    log: Vec<String>,
    refreshed: Option<Instant>,
    /// A device the user asked to revoke, awaiting confirmation.
    confirm_revoke: Option<String>,
    /// The last action's outcome, shown under the tabs.
    notice: Option<String>,
    quitting: bool,
    pair: PairView,
    /// Loaded when the Settings tab opens; an unreadable file is shown, not
    /// overwritten.
    settings: Option<Result<SettingsForm, String>>,
    /// A start or restart running off the UI thread.
    job: Option<(&'static str, std::thread::JoinHandle<Result<(), String>>)>,
    /// Restart was asked for while phones are connected; awaiting a yes.
    confirm_restart: bool,
}

impl PocketdeskApp {
    pub fn new(state_dir: PathBuf, tray: Option<Tray>) -> Self {
        Self {
            state_dir,
            tray,
            tab: Tab::Status,
            snapshot: Snapshot::default(),
            config: None,
            log: Vec::new(),
            refreshed: None,
            confirm_revoke: None,
            notice: None,
            quitting: false,
            pair: PairView::default(),
            settings: None,
            job: None,
            confirm_restart: false,
        }
    }

    /// Run a start or restart off the UI thread; one at a time.
    fn run_job(
        &mut self,
        what: &'static str,
        job: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        if self.job.is_some() {
            return;
        }
        self.notice = Some(format!("{what}…"));
        self.job = Some((what, std::thread::spawn(job)));
    }

    fn finish_job(&mut self) {
        if !self.job.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            return;
        }
        let Some((what, handle)) = self.job.take() else {
            return;
        };
        self.notice = Some(match handle.join() {
            Ok(Ok(())) => format!("{what}: done."),
            Ok(Err(e)) => format!("{what}: {e}"),
            Err(_) => format!("{what}: failed."),
        });
        if let Some(Ok(form)) = &mut self.settings {
            form.restart_needed = false;
        }
        self.refreshed = None;
    }

    fn restart_button(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        if self.confirm_restart {
            ui.label(format!(
                "{} phone(s) are connected; restarting ends their sessions.",
                self.snapshot.sessions.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("Restart anyway").clicked() {
                    self.confirm_restart = false;
                    let dir = self.state_dir.clone();
                    self.run_job("Restarting the server", move || {
                        crate::restart::restart(&dir)
                    });
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_restart = false;
                }
            });
        } else if ui
            .add_enabled(!busy, egui::Button::new("Restart server"))
            .clicked()
        {
            if self.snapshot.sessions.is_empty() {
                let dir = self.state_dir.clone();
                self.run_job("Restarting the server", move || {
                    crate::restart::restart(&dir)
                });
            } else {
                self.confirm_restart = true;
            }
        }
    }

    fn switch_to(&mut self, tab: Tab) {
        if self.tab == Tab::Pair && tab != Tab::Pair {
            self.pair.reset();
        }
        if tab == Tab::Settings && self.settings.is_none() {
            self.settings = Some(
                Config::load()
                    .map(SettingsForm::from_config)
                    .map_err(|e| e.to_string()),
            );
        }
        self.tab = tab;
        self.refreshed = None;
    }

    fn refresh(&mut self) {
        self.snapshot = model::snapshot(&self.state_dir, now_unix());
        self.config = Config::load().ok();
        if self.tab == Tab::Logs {
            self.log = model::log_tail(&self.state_dir, 400);
        }
        self.refreshed = Some(Instant::now());
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        for action in tray.take_actions() {
            match action {
                TrayAction::Open | TrayAction::Pair => {
                    if action == TrayAction::Pair {
                        self.switch_to(Tab::Pair);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn revoke(&mut self, id: &str) {
        let result = TrustStore::load_or_create(self.state_dir.join(TRUST_FILE))
            .and_then(|mut store| store.untrust_device(id));
        self.notice = Some(match result {
            Ok(()) => format!("Removed {} — it must pair again to connect.", short(id)),
            Err(e) => format!("Could not remove {}: {e}", short(id)),
        });
        self.refresh();
    }

    fn end_session(&mut self, session: &sessions::MirrorSession) {
        // A device id names it exactly; a session that never announced one
        // is ended by its address.
        let target = session.device_id.clone().unwrap_or_else(|| {
            session
                .peer
                .rsplit_once(':')
                .map_or(session.peer.clone(), |(ip, _)| ip.to_string())
        });
        self.notice = Some(
            match sessions::request_kick(&self.state_dir, &target, now_unix()) {
                Ok(()) => format!("Ending the session from {} …", short(&target)),
                Err(e) => format!("Could not end the session: {e}"),
            },
        );
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if self.snapshot.running {
            status_line(ui, GOOD, "The server is running");
            self.restart_button(ui);
        } else {
            status_line(ui, MUTED, "The server is not running");
            ui.label(
                "Phones cannot connect until it starts. It starts at logon once \
                 installed (`linux-link install` on Windows, the systemd user unit on Linux).",
            );
            if ui
                .add_enabled(self.job.is_none(), egui::Button::new("Start server"))
                .clicked()
            {
                self.run_job("Starting the server", crate::restart::start);
            }
        }
        ui.add_space(8.0);
        egui::Grid::new("status")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                ui.label("Paired phones");
                ui.label(self.snapshot.devices.len().to_string());
                ui.end_row();
                ui.label("Live sessions");
                ui.label(self.snapshot.sessions.len().to_string());
                ui.end_row();
                if let Some(config) = &self.config {
                    ui.label("Control port");
                    ui.label(format!("TCP {}", config.control_port));
                    ui.end_row();
                    ui.label("Streaming port");
                    ui.label(format!("UDP {}", config.streaming_port));
                    ui.end_row();
                    ui.label("Pairing required");
                    ui.label(if config.pairing_required { "yes" } else { "no" });
                    ui.end_row();
                }
            });
    }

    fn devices_tab(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.devices.is_empty() {
            ui.label("No phone is paired yet.");
            return;
        }
        let mut revoke = None;
        let mut confirmed = None;
        egui::Grid::new("devices")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for device in &self.snapshot.devices {
                    ui.monospace(short(&device.id)).on_hover_text(&device.id);
                    ui.label(match device.grant_remaining {
                        None => "permanent".to_string(),
                        Some(secs) => format!("{} left", human_duration(secs)),
                    });
                    if device.connected {
                        ui.label(RichText::new("connected").color(GOOD));
                    } else {
                        ui.label(RichText::new("—").color(MUTED));
                    }
                    if self.confirm_revoke.as_deref() == Some(&device.id) {
                        ui.horizontal(|ui| {
                            if ui.button("Remove").clicked() {
                                confirmed = Some(device.id.clone());
                            }
                            if ui.button("Cancel").clicked() {
                                revoke = Some(None);
                            }
                        });
                    } else if ui.button("Remove…").clicked() {
                        revoke = Some(Some(device.id.clone()));
                    }
                    ui.end_row();
                }
            });
        if let Some(choice) = revoke {
            self.confirm_revoke = choice;
        }
        if let Some(id) = confirmed {
            self.confirm_revoke = None;
            self.revoke(&id);
        }
    }

    fn sessions_tab(&mut self, ui: &mut egui::Ui) {
        if !self.snapshot.running {
            ui.label("The server is not running, so no phone is connected.");
            return;
        }
        if self.snapshot.sessions.is_empty() {
            ui.label("No phone is viewing this desktop right now.");
            return;
        }
        let now = now_unix();
        let mut end = None;
        egui::Grid::new("sessions")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for session in &self.snapshot.sessions {
                    let who = session
                        .device_id
                        .as_deref()
                        .map_or("unannounced".to_string(), short);
                    ui.monospace(who);
                    ui.label(format!(
                        "{} · {}",
                        session.transport.to_uppercase(),
                        session.peer
                    ));
                    ui.label(format!(
                        "for {}",
                        human_duration(now.saturating_sub(session.started_unix))
                    ));
                    if ui.button("End").clicked() {
                        end = Some(session.clone());
                    }
                    ui.end_row();
                }
            });
        if let Some(session) = end {
            self.end_session(&session);
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let form = match &mut self.settings {
            Some(Ok(form)) => form,
            Some(Err(error)) => {
                ui.label(
                    RichText::new(format!("config.toml could not be read: {error}"))
                        .color(Color32::from_rgb(0xd9, 0x5c, 0x4a)),
                );
                ui.label("Fix or remove the file, then reopen this tab. Nothing is saved over it.");
                return;
            }
            None => return,
        };
        if form.show(ui) {
            match Config::path() {
                Ok(path) => match form.save_to(&path) {
                    Ok(()) => self.notice = Some("Saved.".to_string()),
                    Err(e) => self.notice = Some(format!("Not saved: {e}")),
                },
                Err(e) => self.notice = Some(format!("Not saved: {e}")),
            }
        }
        if form.restart_needed {
            ui.add_space(8.0);
            ui.label(RichText::new("Restart the server to apply the saved settings.").strong());
            if self.snapshot.running {
                self.restart_button(ui);
            }
        }
    }

    fn logs_tab(&self, ui: &mut egui::Ui) {
        if self.log.is_empty() {
            ui.label(
                "No log file yet. The server writes one when it starts at logon \
                 (`start --background`); started from a terminal it logs there instead.",
            );
            return;
        }
        egui::ScrollArea::both()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in &self.log {
                    ui.monospace(line);
                }
            });
    }
}

impl eframe::App for PocketdeskApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray(ctx);
        self.finish_job();
        // Closing hides to the tray; Quit (or no tray to come back from) exits.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if self.refreshed.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("Pocketdesk");
                ui.add_space(12.0);
                for (tab, label) in [
                    (Tab::Status, "Status"),
                    (Tab::Pair, "Pair"),
                    (Tab::Devices, "Devices"),
                    (Tab::Sessions, "Sessions"),
                    (Tab::Settings, "Settings"),
                    (Tab::Logs, "Logs"),
                ] {
                    if ui.selectable_label(self.tab == tab, label).clicked() {
                        self.switch_to(tab);
                    }
                }
            });
            if let Some(notice) = &self.notice {
                ui.label(RichText::new(notice).italics());
            }
            ui.add_space(4.0);
        });
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Status => self.status_tab(ui),
            Tab::Pair => self.pair.show(
                ui,
                &self.state_dir,
                self.config.as_ref(),
                &self.snapshot.devices,
                now_unix(),
            ),
            Tab::Devices => self.devices_tab(ui),
            Tab::Sessions => self.sessions_tab(ui),
            Tab::Settings => self.settings_tab(ui),
            Tab::Logs => self.logs_tab(ui),
        });
    }
}

/// A coloured dot and a bold line. The dot is painted: the bundled fonts
/// have no "●" glyph.
fn status_line(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(RichText::new(text).color(color).strong());
    });
}

/// A device id short enough for a table: the first 12 characters.
pub fn short(id: &str) -> String {
    if id.chars().count() > 12 {
        format!("{}…", id.chars().take(12).collect::<String>())
    } else {
        id.to_string()
    }
}

/// `75` → "1m 15s", `7200` → "2h 0m".
pub fn human_duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        3600..86_400 => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
        _ => format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(75), "1m 15s");
        assert_eq!(human_duration(7_200), "2h 0m");
        assert_eq!(human_duration(90_000), "1d 1h");
    }

    #[test]
    fn long_ids_are_shortened_on_a_char_boundary() {
        assert_eq!(short("abcdef"), "abcdef");
        assert_eq!(short("0a1b2c3d-4e5f-6789"), "0a1b2c3d-4e5…");
        assert_eq!(short("ééééééééééééé"), "éééééééééééé…");
    }
}
