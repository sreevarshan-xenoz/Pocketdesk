//! The window: status, paired devices, live sessions and the server log.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use pocketdesk_core::local::approval::{ApprovalRequest, Verdict};
use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::pin::now_unix;
use pocketdesk_core::local::sent::SentState;
use pocketdesk_core::local::sessions;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

use crate::model::{self, Snapshot};
use crate::pair_view::PairView;
use crate::settings_view::SettingsForm;
use crate::tray::{Tray, TrayAction};

const REFRESH: Duration = Duration::from_secs(1);
const GOOD: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x6a);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x93, 0x91);

/// The Devices tab's switches, in the order a person thinks of them.
const PERMISSIONS: [(Capability, &str, &str); 11] = [
    (
        Capability::Input,
        "Control the mouse and keyboard",
        "Off: the phone can watch the screen but not touch it.",
    ),
    (
        Capability::Clipboard,
        "Share the clipboard",
        "Copy on one, paste on the other.",
    ),
    (
        Capability::Files,
        "Send and browse files",
        "Send files to this computer and list its folders.",
    ),
    (
        Capability::Audio,
        "Hear and change this computer's sound, and its media",
        "Plays desktop sound on the phone and sets the volume. Also shows the phone what is          playing here (title, artist, app) and lets it pause and skip.",
    ),
    (
        Capability::Mic,
        "Use the phone as a microphone",
        "The phone's microphone becomes \"Pocketdesk Mic\" here.",
    ),
    (
        Capability::Power,
        "Lock, sleep, privacy mode and waking others",
        "Lock the screen, put the computer to sleep or shut it down, blank it for privacy, and send a wake to another computer on this network.",
    ),
    (
        Capability::Exec,
        "Run commands",
        "Run any command line on this computer. Off unless you turn it on.",
    ),
    (
        Capability::Notifications,
        "Show its notifications here",
        "The phone's notifications appear on this computer, when the phone shares them.",
    ),
    (
        Capability::Texts,
        "Show its texts and send replies",
        "Texts the phone receives appear in Messages here, and replies go out through the phone.",
    ),
    (
        Capability::Links,
        "Open its links here",
        "A link shared from the phone opens in this computer's browser (http and https only).",
    ),
    (
        Capability::Media,
        "Show and control its media here",
        "What the phone plays (title, artist, app) shows in Sessions, with Play/Pause and skip.",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Pair,
    Devices,
    Sessions,
    Files,
    Messages,
    Notifications,
    Phone,
    Settings,
    Diagnostics,
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
    /// Held sessions already brought to the owner's attention.
    raised_approvals: Vec<String>,
    /// The phone the Files tab sends to (F7b).
    send_to: Option<String>,
    /// The Files tab's path field.
    send_path: String,
    /// The link the send box hands to the phone (T8a).
    send_link: String,
    /// A "Choose file…" dialog that is open.
    picking: Option<std::thread::JoinHandle<Option<PathBuf>>>,
    /// The Diagnostics tab (F10).
    diagnostics: crate::diagnostics_view::DiagnosticsView,
    /// The Messages tab (N5b).
    messages: crate::messages_view::MessagesView,
    /// The Notifications tab (T10c).
    notes: crate::notes_view::NotesView,
    /// The Phone tab (T10g).
    phone_files: crate::phone_files_view::PhoneFilesView,
    /// Whether "Send to phone" is in the file manager, as last read (T10e).
    send_menu_installed: Option<bool>,
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
            raised_approvals: Vec::new(),
            send_to: None,
            send_path: String::new(),
            send_link: String::new(),
            picking: None,
            diagnostics: Default::default(),
            messages: Default::default(),
            notes: Default::default(),
            phone_files: Default::default(),
            send_menu_installed: None,
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
            self.pair.reset(&self.state_dir);
        }
        if tab == Tab::Settings {
            // Read again on each visit: it may have changed outside the app.
            self.send_menu_installed = None;
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

    /// A phone the owner has not seen yet is waiting: show the window, even
    /// from the tray, so the prompt is where they look.
    fn raise_for_approvals(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self
            .snapshot
            .approvals
            .iter()
            .map(|r| r.id.clone())
            .collect();
        if ids.iter().any(|id| !self.raised_approvals.contains(id)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.raised_approvals = ids;
    }

    fn approval_prompt(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.snapshot.approvals.first().cloned() else {
            return;
        };
        let waiting = self.snapshot.approvals.len();
        let mut verdict = None;
        egui::Window::new("A phone wants to connect")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.label("Phone");
                    ui.monospace(short(&request.device_id))
                        .on_hover_text(&request.device_id);
                });
                ui.label(format!("from {}", request.peer));
                ui.label(
                    RichText::new(format!(
                        "Denied in {}s if you do nothing.",
                        request.remaining(now_unix())
                    ))
                    .color(MUTED),
                );
                if waiting > 1 {
                    ui.label(format!("{} more waiting.", waiting - 1));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Allow once").clicked() {
                        verdict = Some(Verdict::AllowOnce);
                    }
                    if ui
                        .button("Always allow")
                        .on_hover_text("Allow, and stop asking for this phone.")
                        .clicked()
                    {
                        verdict = Some(Verdict::AlwaysAllow);
                    }
                    if ui.button("Deny").clicked() {
                        verdict = Some(Verdict::Deny);
                    }
                });
            });
        if let Some(verdict) = verdict {
            self.answer(&request, verdict);
        }
    }

    fn answer(&mut self, request: &ApprovalRequest, verdict: Verdict) {
        let who = short(&request.device_id);
        // A phone that retried while it waited has several requests held;
        // the owner's answer is about the phone, so it answers them all.
        let ids: Vec<String> = self
            .snapshot
            .approvals
            .iter()
            .filter(|r| r.device_id == request.device_id)
            .map(|r| r.id.clone())
            .collect();
        let result = ids
            .iter()
            .try_for_each(|id| model::answer_approval(&self.state_dir, id, verdict, now_unix()));
        self.notice = Some(match result {
            Ok(()) => match verdict {
                Verdict::AllowOnce => format!("Allowed {who} this once."),
                Verdict::AlwaysAllow => format!("Allowed {who}; it will not be asked again."),
                Verdict::Deny => format!("Denied {who}."),
            },
            Err(e) => format!("Could not answer {who}: {e}"),
        });
        // Off the screen at once; the server drops it from the list when it
        // takes the answer.
        self.snapshot
            .approvals
            .retain(|r| r.device_id != request.device_id);
    }

    fn set_ask_first(&mut self, id: &str, ask_first: bool) {
        if let Err(e) = model::set_ask_first(&self.state_dir, id, ask_first) {
            self.notice = Some(format!("Could not change {}: {e}", short(id)));
        }
        self.refresh();
    }

    fn revoke(&mut self, id: &str) {
        let result = TrustStore::update(self.state_dir.join(TRUST_FILE), |store| {
            store.untrust_device(id)
        });
        // Its keys go too, so the phone can pair again (perhaps reinstalled,
        // with new keys) and bind them.
        let _ = pocketdesk_core::local::device_keys::clear(&self.state_dir, id);
        self.notice = Some(match result {
            Ok(()) => format!("Removed {} — it must pair again to connect.", short(id)),
            Err(e) => format!("Could not remove {}: {e}", short(id)),
        });
        // Removing a phone must not leave it in control of a running session.
        self.end_sessions_of(id);
        self.refresh();
    }

    /// End `id`'s running session, if it has one: a session keeps the
    /// permissions it started with, so a narrower one takes a new session.
    fn end_sessions_of(&mut self, id: &str) {
        let connected = self
            .snapshot
            .sessions
            .iter()
            .any(|s| s.device_id.as_deref() == Some(id));
        if !connected {
            return;
        }
        if let Err(e) = sessions::request_kick(&self.state_dir, id, now_unix()) {
            self.notice = Some(format!("Could not end {}'s session: {e}", short(id)));
        }
    }

    fn set_permission(&mut self, id: &str, capability: Capability, allowed: bool) {
        match model::set_permission(&self.state_dir, id, capability, allowed) {
            Ok(()) if !allowed && model::fixed_at_session_start(capability) => {
                self.end_sessions_of(id);
            }
            Ok(()) => {}
            Err(e) => self.notice = Some(format!("Could not change {}: {e}", short(id))),
        }
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
                 installed (`pocketdesk install` on Windows, the systemd user unit on Linux).",
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
        let now = now_unix();
        egui::Grid::new("devices")
            .num_columns(5)
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
                    ui.label(battery_text(device.battery.as_ref(), now))
                        .on_hover_text("The phone's battery, as it last reported it in a session");
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
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "A change applies at once. Turning off control, sound or the microphone ends that phone's running session, so it reconnects with the change.",
            )
            .color(MUTED),
        );
        let mut change = None;
        let mut ask = None;
        for device in &self.snapshot.devices {
            egui::CollapsingHeader::new(format!("What {} may do", short(&device.id)))
                .id_salt(("permissions", &device.id))
                .show(ui, |ui| {
                    let mut ask_first = device.permissions.ask_first;
                    if ui
                        .checkbox(&mut ask_first, "Ask before each session")
                        .on_hover_text(
                            "Hold each session until you allow it here. Leave this off if \
                             you connect to this computer when nobody is at it.",
                        )
                        .changed()
                    {
                        ask = Some((device.id.clone(), ask_first));
                    }
                    for (capability, label, hint) in PERMISSIONS {
                        let mut allowed = device.permissions.allows(capability);
                        if ui
                            .checkbox(&mut allowed, label)
                            .on_hover_text(hint)
                            .changed()
                        {
                            change = Some((device.id.clone(), capability, allowed));
                        }
                    }
                });
        }
        if let Some((id, capability, allowed)) = change {
            self.set_permission(&id, capability, allowed);
        }
        if let Some((id, ask_first)) = ask {
            self.set_ask_first(&id, ask_first);
        }
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
        let mut give = None;
        let count = self.snapshot.sessions.len();
        egui::Grid::new("sessions")
            .num_columns(5)
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
                        "{} · {}{}",
                        session.transport.to_uppercase(),
                        session.peer,
                        if session.control_only {
                            " · control only"
                        } else {
                            ""
                        }
                    ));
                    ui.label(format!(
                        "for {}",
                        human_duration(now.saturating_sub(session.started_unix))
                    ));
                    match model::control_cell(session, count, self.snapshot.shared_control) {
                        model::ControlCell::Driving => {
                            ui.label(
                                RichText::new("Driving").color(Color32::from_rgb(0x4c, 0xaf, 0x50)),
                            );
                        }
                        model::ControlCell::Give(id) => {
                            if ui
                                .button("Give control")
                                .on_hover_text(
                                    "This phone drives; the others only watch until it goes quiet",
                                )
                                .clicked()
                            {
                                give = Some(id);
                            }
                        }
                        model::ControlCell::Nothing => {
                            ui.label("");
                        }
                    }
                    if ui.button("End").clicked() {
                        end = Some(session.clone());
                    }
                    ui.end_row();
                }
            });
        let mut pressed = None;
        for session in &self.snapshot.sessions {
            let Some(id) = session.device_id.as_deref() else {
                continue;
            };
            let Some(playing) = self.snapshot.phone_media.get(id) else {
                continue;
            };
            let media = &playing.media;
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(format!("Playing on {}:", short(id)));
                ui.label(RichText::new(media_line(media)).strong());
            });
            ui.horizontal(|ui| {
                let buttons = [
                    ("Previous", "previous", media.can_previous),
                    (
                        if media.is_playing { "Pause" } else { "Play" },
                        "playPause",
                        media.can_play_pause,
                    ),
                    ("Next", "next", media.can_next),
                ];
                for (label, command, enabled) in buttons {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        pressed = Some((id.to_string(), command));
                    }
                }
            });
        }
        if let Some((id, command)) = pressed {
            self.notice = Some(
                match model::press_phone_media(&self.state_dir, &id, command, now_unix()) {
                    // Asked, not done: the phone acts while Pocketdesk is
                    // in front there, with its media setting on.
                    Ok(()) => format!(
                        "Asked the phone to {}.",
                        match command {
                            "next" => "skip ahead",
                            "previous" => "go back",
                            _ => "play or pause",
                        }
                    ),
                    Err(e) => e,
                },
            );
        }
        if let Some(session) = end {
            self.end_session(&session);
        }
        if let Some(id) = give {
            self.notice = Some(
                match sessions::request_control(&self.state_dir, id, now_unix()) {
                    Ok(()) => "Handing control over …".to_string(),
                    Err(e) => format!("Could not hand control over: {e}"),
                },
            );
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
        self.send_menu_setting(ui);
    }

    /// "Send to phone" in the file manager (T10e): Explorer's Send To, or the
    /// Nautilus, Nemo and Dolphin menus.
    fn send_menu_setting(&mut self, ui: &mut egui::Ui) {
        use pocketdesk_core::local::send_menu;
        ui.add_space(12.0);
        ui.separator();
        let Ok(data) = send_menu::data_dir() else {
            return;
        };
        // Read once, not every frame; again after a change.
        let installed = *self
            .send_menu_installed
            .get_or_insert_with(|| send_menu::installed_in(&data));
        let mut on = installed;
        ui.checkbox(
            &mut on,
            "\"Send to phone\" in the file manager (sends to the phone connected on this network)",
        );
        if on == installed {
            return;
        }
        let done = if on {
            match crate::restart::server_exe_beside_app() {
                Some(exe) => {
                    send_menu::install_in(&data, &exe).map(|()| "Added to the file manager.")
                }
                None => Err(std::io::Error::other(
                    "the server program is not beside this app",
                )),
            }
        } else {
            send_menu::remove_in(&data).map(|()| "Removed from the file manager.")
        };
        self.send_menu_installed = None;
        self.notice = Some(match done {
            Ok(said) => said.to_string(),
            Err(e) => format!("Not changed: {e}"),
        });
    }

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.send_box(ui);
            ui.add_space(12.0);
            self.sent_list(ui);
            self.received_list(ui);
        });
    }

    /// The phone the send box shows (F7b): the one chosen while it is in a
    /// session, else the first phone that is.
    fn send_target(&self) -> Option<String> {
        let targets = model::send_targets(&self.snapshot);
        self.send_to
            .as_ref()
            .filter(|id| targets.iter().any(|d| &d.id == *id))
            .cloned()
            .or_else(|| targets.first().map(|d| d.id.clone()))
    }

    /// Queue `path` for the chosen phone and say what happened. A chosen
    /// phone that has left is reported, never swapped for another.
    fn send_file(&mut self, path: &std::path::Path) {
        let device = match model::target_for_send(&self.snapshot, self.send_to.as_deref()) {
            Ok(device) => device,
            Err(e) => {
                self.send_to = None;
                self.notice = Some(e);
                return;
            }
        };
        self.notice = Some(
            match model::queue_send(&self.state_dir, &device, path, now_unix()) {
                Ok(name) => format!("Sending {name} to {}…", short(&device)),
                Err(e) => e,
            },
        );
        self.refreshed = None;
    }

    fn send_box(&mut self, ui: &mut egui::Ui) {
        ui.strong("Send to a phone");
        let Some(device) = self.send_target() else {
            ui.label(
                RichText::new(
                    "No phone is in a session. Open a session on the phone, then send files \
                     from here; they land in the phone's Downloads/Pocketdesk.",
                )
                .color(MUTED),
            );
            return;
        };
        let targets: Vec<String> = model::send_targets(&self.snapshot)
            .iter()
            .map(|d| d.id.clone())
            .collect();
        let mut chosen = device.clone();
        ui.horizontal(|ui| {
            ui.label("Phone");
            egui::ComboBox::from_id_salt("send-to")
                .selected_text(short(&chosen))
                .show_ui(ui, |ui| {
                    for id in &targets {
                        ui.selectable_value(&mut chosen, id.clone(), short(id));
                    }
                });
        });
        self.send_to = Some(chosen);
        let mut send = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.send_path)
                    .hint_text("Path to a file or folder")
                    .desired_width(360.0),
            );
            if ui.button("Send").clicked() && !self.send_path.trim().is_empty() {
                send = Some(PathBuf::from(self.send_path.trim()));
            }
            if crate::pick_file::AVAILABLE {
                if ui
                    .add_enabled(self.picking.is_none(), egui::Button::new("Choose file…"))
                    .clicked()
                {
                    self.picking = Some(crate::pick_file::spawn(false));
                }
                if ui
                    .add_enabled(self.picking.is_none(), egui::Button::new("Choose folder…"))
                    .clicked()
                {
                    self.picking = Some(crate::pick_file::spawn(true));
                }
            }
        });
        if let Some(path) = send {
            self.send_file(&path);
            self.send_path.clear();
        }
        let mut link = None;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.send_link)
                    .hint_text("A web link (http or https)")
                    .desired_width(360.0),
            );
            if ui.button("Send link").clicked() && !self.send_link.trim().is_empty() {
                link = Some(self.send_link.clone());
            }
        });
        if let Some(url) = link {
            self.notice = Some(
                match model::queue_link(&self.state_dir, &device, &url, now_unix()) {
                    Ok(said) => {
                        self.send_link.clear();
                        said
                    }
                    Err(e) => e,
                },
            );
        }
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let hint = if hovering {
            RichText::new("Drop to send.").strong()
        } else {
            RichText::new("Or drop files or folders on this window.").color(MUTED)
        };
        ui.label(hint);
    }

    fn sent_list(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.sent.is_empty() {
            return;
        }
        ui.strong("Sent");
        let now = now_unix();
        egui::Grid::new("sent")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for file in &self.snapshot.sent {
                    ui.label(&file.name).on_hover_text(&file.path);
                    ui.label(short(&file.device_id));
                    ui.label(format!(
                        "{} ago",
                        human_duration(now.saturating_sub(file.sent_unix))
                    ));
                    // What a folder send left out goes beside its state.
                    let note = file
                        .note
                        .as_deref()
                        .map(|n| format!(" · {n}"))
                        .unwrap_or_default();
                    match &file.state {
                        SentState::Offered => ui.label(format!("waiting for the phone{note}")),
                        SentState::Sent => ui.label(format!("saved on the phone{note}")),
                        SentState::Failed(reason) => ui.label(
                            RichText::new(format!("not sent: {reason}{note}"))
                                .color(Color32::from_rgb(0xd9, 0x5c, 0x4a)),
                        ),
                    };
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
    }

    fn received_list(&mut self, ui: &mut egui::Ui) {
        ui.strong("Received");
        if self.snapshot.received.is_empty() {
            ui.label("Nothing received yet.");
            ui.label(
                RichText::new(
                    "Share a file to Pocketdesk on your phone; it lands in Downloads and is \
                     listed here.",
                )
                .color(MUTED),
            );
            return;
        }
        let now = now_unix();
        let mut show = None;
        egui::Grid::new("received")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for row in &self.snapshot.received {
                    let file = &row.file;
                    ui.label(&file.name).on_hover_text(&file.path);
                    ui.label(human_size(file.size));
                    ui.label(format!(
                        "{} ago",
                        human_duration(now.saturating_sub(file.received_unix))
                    ));
                    if row.exists {
                        if ui.button("Show in folder").clicked() {
                            show = Some(file.path.clone());
                        }
                    } else {
                        ui.label(RichText::new("moved or deleted").color(MUTED));
                    }
                    ui.end_row();
                }
            });
        if let Some(path) = show
            && let Err(e) = crate::reveal::show_in_folder(std::path::Path::new(&path))
        {
            self.notice = Some(format!("Could not open the folder: {e}"));
        }
    }

    /// Files dropped on the window, and a finished "Choose file…", go to
    /// the chosen phone (F7b).
    fn take_files_to_send(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.switch_to(Tab::Files);
            for path in dropped {
                self.send_file(&path);
            }
        }
        if self.picking.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(Ok(Some(path))) = self.picking.take().map(|h| h.join()) {
                self.send_file(&path);
            }
        } else if self.picking.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
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
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.pair.reset(&self.state_dir);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray(ctx);
        self.finish_job();
        // Closing hides to the tray; Quit (or no tray to come back from) exits.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            // Nobody sees a hidden window, so it must not keep (or, on its
            // next repaint, mint) a live pairing code.
            self.switch_to(Tab::Status);
        }
        if self.pair.take_refresh_request() {
            self.refreshed = None;
            ctx.request_repaint();
        }
        if self.refreshed.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.refresh();
            self.raise_for_approvals(ctx);
        }
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.take_files_to_send(ui.ctx());
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
                    (Tab::Files, "Files"),
                    (Tab::Messages, "Messages"),
                    (Tab::Notifications, "Notifications"),
                    (Tab::Phone, "Phone"),
                    (Tab::Settings, "Settings"),
                    (Tab::Diagnostics, "Diagnostics"),
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
            Tab::Files => self.files_tab(ui),
            Tab::Messages => {
                self.messages
                    .show(ui, &self.state_dir, &self.snapshot.devices, now_unix())
            }
            Tab::Notifications => self.notes.show(ui, &self.state_dir, now_unix()),
            Tab::Phone => {
                self.phone_files
                    .show(ui, &self.state_dir, &self.snapshot.devices, now_unix())
            }
            Tab::Settings => self.settings_tab(ui),
            Tab::Diagnostics => self
                .diagnostics
                .show(ui, &self.state_dir, self.config.as_ref()),
            Tab::Logs => self.logs_tab(ui),
        });
        self.approval_prompt(ui);
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

/// `2_100_000` → "2.0 MB".
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// What a phone plays, for the Sessions tab (T8d): "Song · Artist (App)",
/// with what is missing left out.
pub fn media_line(media: &pocketdesk_core::protocol::phone_media::PhoneMedia) -> String {
    let what = [media.title.as_str(), media.artist.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    match (what.is_empty(), media.app.is_empty()) {
        (false, false) => format!("{what} ({})", media.app),
        (false, true) => what,
        (true, false) => media.app.clone(),
        (true, true) => "something".to_string(),
    }
}

/// A phone's last battery report for the Devices tab (T8b): its charge,
/// whether it is charging, and its age once it is over a minute old.
pub fn battery_text(
    battery: Option<&pocketdesk_core::local::phone_status::PhoneBattery>,
    now_unix: u64,
) -> String {
    let Some(battery) = battery else {
        return "—".to_string();
    };
    let mut text = format!("{} %", battery.charge);
    if battery.charging {
        text.push_str(", charging");
    }
    let age = now_unix.saturating_sub(battery.updated_unix);
    if age >= 60 {
        text.push_str(&format!(" · {} ago", human_duration(age)));
    }
    text
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
    #[test]
    fn what_a_phone_plays_reads_as_one_line() {
        use pocketdesk_core::protocol::phone_media::PhoneMedia;
        let song = PhoneMedia {
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            app: "Spotify".to_string(),
            is_playing: true,
            can_play_pause: true,
            can_next: true,
            can_previous: true,
        };
        assert_eq!(media_line(&song), "Song · Artist (Spotify)");
        let video = PhoneMedia {
            artist: String::new(),
            ..song.clone()
        };
        assert_eq!(media_line(&video), "Song (Spotify)");
        let untitled = PhoneMedia {
            title: String::new(),
            artist: String::new(),
            ..song.clone()
        };
        assert_eq!(media_line(&untitled), "Spotify");
        let bare = PhoneMedia {
            title: String::new(),
            artist: String::new(),
            app: String::new(),
            ..song
        };
        assert_eq!(media_line(&bare), "something");
    }

    #[test]
    fn a_phones_battery_reads_with_its_age() {
        use pocketdesk_core::local::phone_status::PhoneBattery;
        let battery = |charge, charging| PhoneBattery {
            charge,
            charging,
            updated_unix: 1000,
        };
        assert_eq!(battery_text(None, 1000), "—");
        assert_eq!(
            battery_text(Some(&battery(54, true)), 1030),
            "54 %, charging"
        );
        assert_eq!(
            battery_text(Some(&battery(54, false)), 1000 + 180),
            "54 % · 3m 0s ago"
        );
        // A clock that went back is not "in the future".
        assert_eq!(battery_text(Some(&battery(9, false)), 900), "9 %");
    }

    #[test]
    fn every_permission_has_a_switch() {
        for capability in Capability::ALL {
            assert!(
                PERMISSIONS.iter().any(|(c, _, _)| *c == capability),
                "{capability:?} has no switch in the Devices tab"
            );
        }
    }

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
