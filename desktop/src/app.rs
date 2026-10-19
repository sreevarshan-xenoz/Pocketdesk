//! The window: status, paired devices, live sessions and the server log.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
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
use crate::tray::Tray;
use crate::tray_menu::TrayAction;
pub use crate::words::{human_size, short};

const REFRESH: Duration = Duration::from_secs(1);

/// Where the last page shown is kept between runs.
pub const PAGE_KEY: &str = "page";

/// The Devices tab's switches, in the order a person thinks of them.
const PERMISSIONS: [(Capability, &str, &str); 16] = [
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
        "Plays this computer's sound on the phone and sets the volume. Also shows the phone what is playing here (title, artist, app) and lets it pause and skip.",
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
    (
        Capability::Cast,
        "Show its screen here",
        "The phone can show its own screen in a window here, when its owner starts it on the phone.",
    ),
    (
        Capability::Tasks,
        "Start my tasks",
        "The phone can list the tasks named in Settings and start them (each one may ask you first).",
    ),
    (
        Capability::Ring,
        "Ring this phone from here",
        "Ring it from here when it is lost, at most once a minute. The phone rings only with \"Ring from the computer\" on in Pocketdesk's Settings on the phone.",
    ),
    (
        Capability::Mute,
        "Mute this phone from here",
        "Turn on the phone's Do Not Disturb from here for a while, and see when it is quiet. The phone allows it only with \"Let the computer mute this phone\" on in Pocketdesk's Settings on the phone.",
    ),
    (
        Capability::AwayLock,
        "Lock this computer when this phone leaves",
        "When the phone's link has been gone for the time in Settings (Phones) and nobody has used the keyboard or mouse meanwhile, this computer locks, once per departure. A phone asleep may lose its link, so reading or watching without touching anything can lock too. Windows only for now: on Linux it never locks.",
    ),
];

/// Whether a device is shown `capability`'s switch (E5): a paired computer
/// is not a phone to ring, mute, cast or lock this computer by.
fn shown_for(capability: Capability, computer: bool) -> bool {
    !(computer
        && matches!(
            capability,
            Capability::Ring | Capability::Mute | Capability::AwayLock | Capability::Cast
        ))
}

/// Why "Lock this computer when this phone leaves" cannot be turned on
/// here, if it cannot: this computer cannot tell yet whether someone is at
/// it (E5).
fn away_lock_unavailable() -> Option<&'static str> {
    (!cfg!(windows))
        .then_some("Linux cannot tell yet whether someone is at the keyboard, so it never locks.")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Activity,
    Pair,
    Devices,
    Sessions,
    Files,
    Messages,
    Notifications,
    Phone,
    Clipboard,
    Settings,
    Diagnostics,
    Logs,
}

pub struct PocketdeskApp {
    state_dir: PathBuf,
    tray: Option<Tray>,
    tab: Tab,
    /// The page last chosen, kept for the next run: not the Overview the
    /// window goes back to when hidden to the tray (U1 review).
    chosen: Tab,
    snapshot: Snapshot,
    config: Option<Config>,
    log: Vec<String>,
    refreshed: Option<Instant>,
    /// A device the user asked to revoke, awaiting confirmation.
    confirm_revoke: Option<String>,
    /// The Name boxes on the Devices page, by device id (U3).
    renaming: std::collections::HashMap<String, String>,
    /// The last action's outcome, shown under the tabs.
    notice: Option<crate::notice::Notice>,
    quitting: bool,
    /// When this window started: a tray's Quit from before it is not for it (E2 review).
    started_unix: u64,
    pair: PairView,
    /// Loaded when the Settings tab opens; an unreadable file is shown, not
    /// overwritten.
    settings: Option<Result<SettingsForm, String>>,
    /// A start or restart running off the UI thread.
    job: Option<(&'static str, std::thread::JoinHandle<Result<(), String>>)>,
    /// A setting that runs a program (the file manager's menu, the
    /// browser's host), off the UI thread: what it says once done.
    setting_job: Option<std::thread::JoinHandle<Result<String, String>>>,
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
    /// Each phone's last session, for the Overview (N6), read while it shows.
    last_sessions: std::collections::HashMap<String, u64>,
    /// The session log as last read for [`Self::last_sessions`].
    last_sessions_stamp: Option<(u64, std::time::SystemTime)>,
    /// The Overview's link box: the phone it is for, and the link typed.
    overview_link: Option<(String, String)>,
    /// The link box was just opened: its field takes the cursor once.
    overview_link_focus: bool,
    /// A "Choose file…" dialog that is open.
    picking: Option<std::thread::JoinHandle<Option<PathBuf>>>,
    /// The Diagnostics tab (F10).
    diagnostics: crate::diagnostics_view::DiagnosticsView,
    /// The Messages tab (N5b).
    messages: crate::messages_view::MessagesView,
    /// D3: the texts that crossed, to copy or send again.
    clipboard: crate::clipboard_view::ClipboardView,
    /// The Notifications tab (T10c).
    notes: crate::notes_view::NotesView,
    /// Settings → Other computers (2f).
    computers: crate::computers_view::ComputersView,
    /// Settings → KDE Connect devices (3a).
    kdeconnect: crate::kdeconnect_view::KdeConnectView,
    /// The Phone tab (T10g).
    phone_files: crate::phone_files_view::PhoneFilesView,
    photos: crate::photos_view::PhotosView,
    /// Whether "Send to phone" is in the file manager, as last read (T10e).
    send_menu_installed: Option<bool>,
    /// The browsers the extension's host is registered with, as last read
    /// (ecosystem 1d); `Some(None)` when it is not.
    browser_registered: Option<Option<Vec<String>>>,
    /// The window showing a phone's screen (T10k), started on the first pass.
    cast: Option<crate::cast_view::CastView>,
    /// The Activity tab (ecosystem 1b).
    activity: crate::activity_view::ActivityView,
    /// Task starts waiting for the owner's answer (ecosystem 1c).
    task_requests: Vec<pocketdesk_core::local::tasks::TaskRequest>,
    /// Those already brought to the owner's attention.
    raised_tasks: Vec<String>,
    /// Those answered here, until the server takes them off its list (it
    /// looks every half second; the list could show one again before).
    answered_tasks: Vec<String>,
}

impl PocketdeskApp {
    /// The app, open on `page` (`--page`) or the Overview.
    pub fn new(state_dir: PathBuf, tray: Option<Tray>, page: Option<Tab>) -> Self {
        let mut app = Self {
            state_dir,
            tray,
            tab: Tab::Status,
            chosen: Tab::Status,
            snapshot: Snapshot::default(),
            config: None,
            log: Vec::new(),
            refreshed: None,
            confirm_revoke: None,
            renaming: std::collections::HashMap::new(),
            notice: None,
            quitting: false,
            started_unix: now_unix(),
            pair: PairView::default(),
            settings: None,
            job: None,
            setting_job: None,
            confirm_restart: false,
            raised_approvals: Vec::new(),
            send_to: None,
            send_path: String::new(),
            send_link: String::new(),
            last_sessions: Default::default(),
            last_sessions_stamp: None,
            overview_link: None,
            overview_link_focus: false,
            picking: None,
            diagnostics: Default::default(),
            messages: Default::default(),
            clipboard: Default::default(),
            notes: Default::default(),
            computers: Default::default(),
            kdeconnect: Default::default(),
            phone_files: Default::default(),
            photos: Default::default(),
            send_menu_installed: None,
            browser_registered: None,
            cast: None,
            activity: Default::default(),
            task_requests: Vec::new(),
            raised_tasks: Vec::new(),
            answered_tasks: Vec::new(),
        };
        if let Some(page) = page {
            app.switch_to(page);
            app.chosen = page;
        }
        app
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
        self.notice = crate::notice::Notice::working(format!("{what}…"));
        self.job = Some((what, std::thread::spawn(job)));
    }

    /// Run a setting's program off the UI thread (U1: PowerShell and
    /// `browser-host` froze the window for a second or two); one at a time.
    fn run_setting(
        &mut self,
        doing: &str,
        job: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) {
        if self.setting_job.is_some() {
            return;
        }
        self.notice = crate::notice::Notice::working(format!("{doing}…"));
        self.setting_job = Some(std::thread::spawn(job));
    }

    fn finish_setting(&mut self) {
        if !self.setting_job.as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let Some(handle) = self.setting_job.take() else {
            return;
        };
        self.notice = crate::notice::Notice::outcome(match handle.join() {
            Ok(Ok(said)) => Ok(said),
            Ok(Err(e)) => Err(format!("Not changed: {e}")),
            Err(_) => Err("Not changed: the change failed.".to_string()),
        });
        // Read again: they changed.
        self.send_menu_installed = None;
        self.browser_registered = None;
    }

    fn finish_job(&mut self) {
        if !self.job.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            return;
        }
        let Some((what, handle)) = self.job.take() else {
            return;
        };
        let outcome = match handle.join() {
            Ok(Ok(())) => Ok(format!("{what}: done.")),
            Ok(Err(e)) => Err(format!("{what}: {e}")),
            Err(_) => Err(format!("{what}: failed.")),
        };
        // Only a server that started again has read what is saved (U5
        // review: a failed restart cleared the notice of what waits).
        if outcome.is_ok()
            && let Some(Ok(form)) = &mut self.settings
        {
            form.restarted();
        }
        self.notice = crate::notice::Notice::outcome(outcome);
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
        // The Overview's link box is for the moment it was opened in.
        if tab != self.tab {
            self.overview_link = None;
        }
        if self.tab == Tab::Pair && tab != Tab::Pair {
            self.pair.reset(&self.state_dir);
        }
        if tab == Tab::Activity && self.tab != Tab::Activity {
            self.activity.reopen();
        }
        if tab == Tab::Settings {
            // Read again on each visit: it may have changed outside the app.
            self.send_menu_installed = None;
            self.browser_registered = None;
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
        let requests = pocketdesk_core::local::tasks::read_requests(&self.state_dir, now_unix());
        self.answered_tasks
            .retain(|id| requests.iter().any(|r| &r.id == id));
        self.task_requests = requests
            .into_iter()
            .filter(|r| !self.answered_tasks.contains(&r.id))
            .collect();
        self.config = Config::load().ok();
        if self.tab == Tab::Logs {
            self.log = model::log_tail(&self.state_dir, 400);
        }
        if self.tab == Tab::Status {
            // Read again only when the log changed, not every second.
            let stamp = model::session_log_stamp(&self.state_dir);
            if stamp != self.last_sessions_stamp {
                self.last_sessions =
                    model::last_sessions(&model::session_log_tail(&self.state_dir));
                self.last_sessions_stamp = stamp;
            }
        }
        if self.tab == Tab::Activity {
            self.activity.refresh(&self.state_dir);
        }
        self.refreshed = Some(Instant::now());
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        for action in tray.take_actions() {
            let show = match action {
                TrayAction::Open => true,
                TrayAction::Pair => {
                    self.switch_to(Tab::Pair);
                    true
                }
                TrayAction::SendFile(id) => {
                    self.send_to = Some(id);
                    self.switch_to(Tab::Files);
                    true
                }
                // D5: asked from the tray, the window stays as it is.
                TrayAction::Ring(id) => {
                    self.ring(&id);
                    false
                }
                TrayAction::Mute(id, minutes) => {
                    self.quiet(&id, minutes);
                    false
                }
                TrayAction::SendClipboard(id) => {
                    self.send_clipboard(&id);
                    false
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    false
                }
            };
            if show {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
    }

    /// Go to the page the tray program or a second launch asked for, and
    /// come forward; or quit, as the tray program's Quit asks (E2).
    fn open_asked(
        &mut self,
        ctx: &egui::Context,
        open: pocketdesk_core::local::app_open::OpenRequest,
    ) {
        if open.page == pocketdesk_core::local::app_open::QUIT {
            if open.quits(self.started_unix) {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        if let Some(tab) = Tab::from_key(&open.page) {
            if tab == Tab::Files && open.phone.is_some() {
                self.send_to = open.phone;
            }
            self.switch_to(tab);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// The tray's menu for the phones as last read (D5); rebuilt by the
    /// tray only when it changes.
    fn update_tray(&mut self) {
        let Some(tray) = self.tray.as_mut() else {
            return;
        };
        let rows = model::phone_rows(&self.snapshot, &self.last_sessions);
        tray.show(crate::tray_menu::menu(
            &rows,
            &self.snapshot.devices,
            now_unix(),
        ));
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

    /// A phone's task start that asks first: show the window, even from the
    /// tray, as a held session does (ecosystem 1c).
    fn raise_for_tasks(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.task_requests.iter().map(|r| r.id.clone()).collect();
        if ids.iter().any(|id| !self.raised_tasks.contains(id)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.raised_tasks = ids;
    }

    fn task_prompt(&mut self, ui: &mut egui::Ui) {
        use pocketdesk_core::local::tasks::{self, TaskVerdict};
        let Some(request) = self.task_requests.first().cloned() else {
            return;
        };
        let mut verdict = None;
        let from_home_assistant =
            request.device_id == pocketdesk_core::local::home_assistant::HOME_ASSISTANT_DEVICE;
        let title = if from_home_assistant {
            "Home Assistant wants to run a task"
        } else {
            "A phone wants to run a task"
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.label(RichText::new(&request.task).strong());
                if from_home_assistant {
                    ui.label("Asked from a button in Home Assistant.");
                } else {
                    phone_row(ui, &self.snapshot.devices, &request.device_id);
                }
                ui.label(
                    RichText::new(format!(
                        "Denied in {}s if you do nothing.",
                        request.remaining(now_unix())
                    ))
                    .color(crate::theme::muted(ui)),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Allow").clicked() {
                        verdict = Some(TaskVerdict::Allow);
                    }
                    if ui.button("Deny").clicked() {
                        verdict = Some(TaskVerdict::Deny);
                    }
                });
            });
        if let Some(verdict) = verdict {
            if let Err(e) = tasks::answer(&self.state_dir, &request.id, verdict, now_unix()) {
                self.notice =
                    crate::notice::Notice::error(format!("Could not answer the task: {e}"));
            }
            self.task_requests.retain(|r| r.id != request.id);
            self.answered_tasks.push(request.id.clone());
        }
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
                phone_row(ui, &self.snapshot.devices, &request.device_id);
                ui.label(format!("from {}", request.peer));
                ui.label(
                    RichText::new(format!(
                        "Denied in {}s if you do nothing.",
                        request.remaining(now_unix())
                    ))
                    .color(crate::theme::muted(ui)),
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
        let who = self.label(&request.device_id);
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
        self.notice = crate::notice::Notice::outcome(match result {
            Ok(()) => Ok(match verdict {
                Verdict::AllowOnce => format!("Allowed {who} this once."),
                Verdict::AlwaysAllow => format!("Allowed {who}; it will not be asked again."),
                Verdict::Deny => format!("Denied {who}."),
            }),
            Err(e) => Err(format!("Could not answer {who}: {e}")),
        });
        // Off the screen at once; the server drops it from the list when it
        // takes the answer.
        self.snapshot
            .approvals
            .retain(|r| r.device_id != request.device_id);
    }

    fn set_ask_first(&mut self, id: &str, ask_first: bool) {
        if let Err(e) = model::set_ask_first(&self.state_dir, id, ask_first) {
            self.notice =
                crate::notice::Notice::error(format!("Could not change {}: {e}", self.label(id)));
        }
        self.refresh();
    }

    /// A paired device by its name, else its short id (U3).
    fn label(&self, id: &str) -> String {
        model::device_label(&self.snapshot.devices, id)
    }

    /// The owner names a paired device (U3); blank clears it.
    fn rename_device(&mut self, id: &str, name: &str) {
        let before = self.label(id);
        self.notice =
            crate::notice::Notice::outcome(match model::rename_device(&self.state_dir, id, name) {
                Ok(()) if name.trim().is_empty() => {
                    Ok(format!("{before} goes by its own name again."))
                }
                Ok(()) => Ok(format!("Renamed {before}.")),
                Err(e) => Err(format!("Could not rename {before}: {e}")),
            });
        self.refresh();
    }

    fn revoke(&mut self, id: &str) {
        // Named before it goes, since the name goes with it.
        let who = self.label(id);
        let result = TrustStore::update(self.state_dir.join(TRUST_FILE), |store| {
            store.untrust_device(id)
        });
        // Its keys go too, so the phone can pair again (perhaps reinstalled,
        // with new keys) and bind them.
        let _ = pocketdesk_core::local::device_keys::clear(&self.state_dir, id);
        self.notice = crate::notice::Notice::outcome(match result {
            Ok(()) => Ok(format!("Unpaired {who}: it must pair again to connect.")),
            Err(e) => Err(format!("Could not unpair {who}: {e}")),
        });
        // Unpairing a phone must not leave it in control of a running session.
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
            self.notice = crate::notice::Notice::error(format!(
                "Could not end {}'s session: {e}",
                self.label(id)
            ));
        }
    }

    fn set_permission(&mut self, id: &str, capability: Capability, allowed: bool) {
        match model::set_permission(&self.state_dir, id, capability, allowed) {
            Ok(()) if !allowed && model::fixed_at_session_start(capability) => {
                self.end_sessions_of(id);
            }
            Ok(()) => {}
            Err(e) => {
                self.notice = crate::notice::Notice::error(format!(
                    "Could not change {}: {e}",
                    self.label(id)
                ))
            }
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
        self.notice = crate::notice::Notice::outcome(
            match sessions::request_kick(&self.state_dir, &target, now_unix()) {
                Ok(()) => Ok(format!(
                    "Asked the session from {} to end.",
                    self.label(&target)
                )),
                Err(e) => Err(format!("Could not end the session: {e}")),
            },
        );
    }

    /// The page shown, under its heading.
    fn page(&mut self, ui: &mut egui::Ui) {
        match self.tab {
            Tab::Status => self.status_tab(ui),
            Tab::Activity => {
                let log_on = self.config.as_ref().is_none_or(|c| c.events);
                match self
                    .activity
                    .show(ui, log_on, &self.snapshot.devices, now_unix())
                {
                    Some(crate::activity_view::Action::ShowFile(path)) => {
                        if let Err(e) = crate::reveal::show_in_folder(&path) {
                            self.notice = crate::notice::Notice::error(format!(
                                "Could not show the file: {e}"
                            ));
                        }
                    }
                    Some(crate::activity_view::Action::OpenSettings) => {
                        self.switch_to(Tab::Settings)
                    }
                    None => {}
                }
            }
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
            Tab::Notifications => {
                self.notes
                    .show(ui, &self.state_dir, &self.snapshot.devices, now_unix())
            }
            Tab::Phone => {
                self.phone_files
                    .show(ui, &self.state_dir, &self.snapshot.devices, now_unix());
                // C7: the chosen phone's recent photos below its folders.
                if let Some(phone) = self.phone_files.phone().map(str::to_string) {
                    self.photos.show(ui, &self.state_dir, &phone, now_unix());
                }
            }
            Tab::Clipboard => {
                let history = self.config.as_ref().map_or(0, |c| c.clipboard_history);
                match self.clipboard.show(
                    ui,
                    &self.state_dir,
                    &self.snapshot.devices,
                    history,
                    now_unix(),
                ) {
                    Some(crate::clipboard_view::Action::Copy(text)) => {
                        ui.ctx().copy_text(text);
                        self.notice = crate::notice::Notice::done("Copied.");
                    }
                    Some(crate::clipboard_view::Action::Done(said)) => {
                        self.notice = crate::notice::Notice::done(said);
                    }
                    Some(crate::clipboard_view::Action::Failed(said)) => {
                        self.notice = crate::notice::Notice::error(said);
                    }
                    Some(crate::clipboard_view::Action::OpenSettings) => {
                        self.switch_to(Tab::Settings)
                    }
                    Some(crate::clipboard_view::Action::Pair) => self.switch_to(Tab::Pair),
                    None => {}
                }
            }
            Tab::Settings => self.settings_tab(ui),
            Tab::Diagnostics => self
                .diagnostics
                .show(ui, &self.state_dir, self.config.as_ref()),
            Tab::Logs => self.logs_tab(ui),
        }
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if self.snapshot.running {
            status_line(ui, crate::theme::GOOD, "The server is running");
            self.restart_button(ui);
        } else {
            status_line(ui, crate::theme::MUTED, "The server is not running");
            ui.label(
                "Phones cannot connect until it starts. Once installed, it starts when you sign in.",
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
                // Phones, as listed below; other computers are in Settings.
                ui.label("Paired phones");
                ui.label(
                    self.snapshot
                        .devices
                        .iter()
                        .filter(|d| !d.computer)
                        .count()
                        .to_string(),
                );
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
        ui.add_space(16.0);
        self.phones_panel(ui);
    }

    /// Ask the phone `id` to ring (N1), and say how that went.
    fn ring(&mut self, id: &str) {
        let who = self.label(id);
        self.notice =
            crate::notice::Notice::outcome(
                match pocketdesk_core::local::rings::request(&self.state_dir, id, now_unix()) {
                    Ok(_) => Ok(format!("Asked {who} to ring.")),
                    Err(e) => Err(format!("Could not ring {who}: {e}")),
                },
            );
    }

    /// Ask the phone `id` to be quiet for `minutes`, or to end the mute with
    /// 0 (D2), and say how that went.
    fn quiet(&mut self, id: &str, minutes: u32) {
        let who = self.label(id);
        self.notice = crate::notice::Notice::outcome(match pocketdesk_core::local::quiet::request(
            &self.state_dir,
            id,
            minutes,
            now_unix(),
        ) {
            Ok(_) if minutes == 0 => Ok(format!("Asked {who} to end its mute.")),
            Ok(_) => Ok(format!("Asked {who} to be quiet for an hour.")),
            Err(e) => Err(format!("Could not mute {who}: {e}")),
        });
    }

    /// Ask the server to put this computer's clipboard on the phone `id`
    /// (D5, the tray), and say how that went.
    fn send_clipboard(&mut self, id: &str) {
        let who = self.label(id);
        self.notice = crate::notice::Notice::outcome(
            match pocketdesk_core::local::clip_history::request_current(
                &self.state_dir,
                id,
                now_unix(),
            ) {
                // Asked, not sent: the server reads the clipboard then, and
                // shows a notification if it cannot send it (D5 review).
                Ok(_) => Ok(format!(
                    "Sending the clipboard to {who}; a notification says if it cannot."
                )),
                Err(e) => Err(format!("Could not send the clipboard to {who}: {e}")),
            },
        );
    }

    /// The Overview's phones (N6): where each is, its battery and last
    /// session, and Ring, Send file… and Send link… for it.
    fn phones_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Your phones").strong());
        let now = now_unix();
        let rows = model::phone_rows(&self.snapshot, &self.last_sessions);
        if rows.is_empty() {
            ui.label("No phone is paired yet. The phone scans a code from the Pair page.");
            if ui.button("Pair a phone").clicked() {
                self.switch_to(Tab::Pair);
            }
            self.overview_link = None;
            return;
        }
        let mut ring = None;
        let mut mute = None;
        let mut send_file = None;
        let mut open_link = None;
        let mut send_clip = None;
        // One short block per phone, so it fits a narrow window (N6).
        for row in &rows {
            let device = row.device;
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(model::device_label(&self.snapshot.devices, &device.id)).strong())
                        .on_hover_text(&device.id);
                    let (place, why) = match row.place {
                        model::Place::Session => {
                            ("In a session", "It is in a session with this computer now.")
                        }
                        model::Place::Linked => (
                            "Linked",
                            "Not in a session, but a feature on the phone keeps a link to this computer.",
                        ),
                        model::Place::Away => (
                            "Away",
                            "Nothing reaches it now: not in a session, and no feature on the phone keeps a link.",
                        ),
                    };
                    let color = if row.place == model::Place::Away {
                        crate::theme::muted(ui)
                    } else {
                        crate::theme::good(ui)
                    };
                    ui.label(RichText::new(place).color(color)).on_hover_text(why);
                });
                // Its battery, and its last session unless one runs now
                // (a session is recorded when it ends).
                let mut facts = vec![format!("Battery {}", battery_text(device.battery.as_ref(), now))];
                if let Some(quiet) = model::quiet_text(device.quiet.as_ref(), now) {
                    facts.push(quiet);
                }

                if row.place != model::Place::Session {
                    facts.push(match row.last_session {
                        Some(at) => format!("last session {}", crate::words::ago(now, at)),
                        // The log's tail holds a few hundred sessions.
                        None => "no recent session".to_string(),
                    });
                }
                ui.label(RichText::new(facts.join(" · ")).color(crate::theme::muted(ui)));
                // A refused mute on its own line, as a warning (E5): in the
                // facts it read as one more fact.
                if model::mute_refused(device.quiet.as_ref(), now) {
                    ui.label(
                        RichText::new(
                            "It said no to the mute: turn on \"Let the computer mute this phone\" in Pocketdesk's Settings on the phone.",
                        )
                        .color(crate::theme::warn(ui)),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    let ring_why = row.ring.err().unwrap_or(
                        "Ring it for 30 seconds, through Do Not Disturb when the phone lets alarms through. At most once a minute.",
                    );
                    if ui
                        .add_enabled(row.ring.is_ok(), egui::Button::new("Ring"))
                        .on_hover_text(ring_why)
                        .on_disabled_hover_text(ring_why)
                        .clicked()
                    {
                        ring = Some(device.id.clone());
                    }
                    // D2: Mute for an hour, or Unmute a mute from here.
                    let muted = model::muted_from_here(device.quiet.as_ref(), now);
                    let mute_why = row.mute.err().unwrap_or(if muted {
                        "End the mute asked for from here. The phone's own Do Not Disturb settings stay as they are."
                    } else {
                        "Turn on the phone's Do Not Disturb for an hour. Calls and messages its own settings let through still come."
                    });
                    let owners = !muted && model::quiet_by_owner(device.quiet.as_ref());
                    let mute_why = if owners && row.mute.is_ok() {
                        "The phone is already in its own Do Not Disturb; the computer leaves it as it is."
                    } else {
                        mute_why
                    };
                    if ui
                        .add_enabled(
                            row.mute.is_ok() && !owners,
                            egui::Button::new(if muted { "Unmute" } else { "Mute for an hour" }),
                        )
                        .on_hover_text(mute_why)
                        .on_disabled_hover_text(mute_why)
                        .clicked()
                    {
                        mute = Some((device.id.clone(), if muted { 0 } else { 60 }));
                    }
                    let file_why = row.file.err().unwrap_or("Open Files with this phone chosen.");
                    if ui
                        .add_enabled(row.file.is_ok(), egui::Button::new("Send file…"))
                        .on_hover_text(file_why)
                        .on_disabled_hover_text(file_why)
                        .clicked()
                    {
                        send_file = Some(device.id.clone());
                    }
                    let link_why = row.link.err().unwrap_or("A web link it opens from a notification.");
                    if ui
                        .add_enabled(row.link.is_ok(), egui::Button::new("Send link…"))
                        .on_hover_text(link_why)
                        .on_disabled_hover_text(link_why)
                        .clicked()
                    {
                        open_link = Some(device.id.clone());
                    }
                    // As the tray offers it (E5): this computer's clipboard
                    // as it is when pressed.
                    let clip_why = row.clipboard.err().unwrap_or(
                        "This computer's clipboard as it is now, never a copy a password manager marks private.",
                    );
                    if ui
                        .add_enabled(row.clipboard.is_ok(), egui::Button::new("Send the clipboard"))
                        .on_hover_text(clip_why)
                        .on_disabled_hover_text(clip_why)
                        .clicked()
                    {
                        send_clip = Some(device.id.clone());
                    }
                });
            });
        }
        // Whether the link box's phone can take a link now (N6 review): a
        // phone gone from the list closes the box.
        let link_state = self
            .overview_link
            .as_ref()
            .map(|(id, _)| rows.iter().find(|r| &r.device.id == id).map(|r| r.link));
        drop(rows);
        if let Some(id) = ring {
            self.ring(&id);
        }
        if let Some((id, minutes)) = mute {
            self.quiet(&id, minutes);
        }
        if let Some(id) = send_file {
            self.send_to = Some(id);
            self.switch_to(Tab::Files);
        }
        match link_state {
            Some(None) => self.overview_link = None,
            Some(Some(reach)) => self.overview_link_box(ui, reach),
            None => {}
        }
        if let Some(id) = open_link {
            self.overview_link = Some((id, String::new()));
            self.overview_link_focus = true;
        }
        if let Some(id) = send_clip {
            self.send_clipboard(&id);
        }
    }

    /// The Overview's link box, for the phone its Send link… was pressed on;
    /// `reach` says whether a link reaches that phone now, or why not.
    fn overview_link_box(&mut self, ui: &mut egui::Ui, reach: Result<(), &'static str>) {
        let Some((id, text)) = self.overview_link.as_mut() else {
            return;
        };
        let who = model::device_label(&self.snapshot.devices, id);
        let mut send = false;
        let mut cancel = false;
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("A link for {who}:"));
            let field = ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text("A web link (http or https)")
                    .desired_width(320.0),
            );
            if std::mem::take(&mut self.overview_link_focus) {
                field.request_focus();
            }
            let can_send = !text.trim().is_empty() && reach.is_ok();
            if can_send && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                send = true;
            }
            send |= ui
                .add_enabled(can_send, egui::Button::new("Send"))
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if let Err(why) = reach {
            ui.label(RichText::new(why).color(crate::theme::muted(ui)));
        }
        if cancel {
            self.overview_link = None;
        } else if send && let Some((id, url)) = self.overview_link.take() {
            match model::queue_link(&self.state_dir, &id, &url, now_unix()) {
                Ok(said) => self.notice = crate::notice::Notice::outcome(Ok(said)),
                Err(e) => {
                    // Keep the box, so the link can be fixed.
                    self.notice = crate::notice::Notice::outcome(Err(e));
                    self.overview_link = Some((id, url));
                }
            }
        }
    }

    fn devices_tab(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.devices.is_empty() {
            // U5: an empty page says what to do next, with the way there.
            ui.label("No phone is paired yet. The phone scans a code from the Pair page.");
            if ui.button("Pair a phone").clicked() {
                self.switch_to(Tab::Pair);
            }
            return;
        }
        let mut revoke = None;
        let mut confirmed = None;
        let mut ring = None;
        let now = now_unix();
        // E5: a block per device, its parts wrapping, as the Overview's
        // phones are: a grid of six columns ran off a small window.
        for device in &self.snapshot.devices {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if let Some(name) = &device.name {
                        ui.label(RichText::new(name).strong());
                    }
                    ui.monospace(short(&device.id)).on_hover_text(&device.id);
                    ui.label(
                        RichText::new(match device.grant_remaining {
                            None if device.computer => "a computer".to_string(),
                            None => "permanent".to_string(),
                            Some(secs) => format!("{} left", human_duration(secs)),
                        })
                        .color(crate::theme::muted(ui)),
                    );
                    // Where it is, in the Overview's words (E5).
                    if device.connected {
                        ui.label(RichText::new("In a session").color(crate::theme::good(ui)));
                    } else if device.linked {
                        ui.label(RichText::new("Linked").color(crate::theme::good(ui)));
                    } else {
                        ui.label(RichText::new("Away").color(crate::theme::muted(ui)));
                    }
                    ui.label(battery_text(device.battery.as_ref(), now))
                        .on_hover_text("The phone's battery, as it last reported it in a session");
                    // N1: ring a phone that keeps a link, with Ring on.
                    let can_ring = device.linked
                        && !device.computer
                        && device.permissions.allows(Capability::Ring);
                    let why = if device.computer {
                        "A computer is not rung."
                    } else if !device.permissions.allows(Capability::Ring) {
                        "Ring is off for this phone, in its section below."
                    } else if !device.linked {
                        "Not linked now: it rings in a session, or with \"Ring from the computer\" on in Pocketdesk's Settings on the phone."
                    } else {
                        "Ring it for 30 seconds, through Do Not Disturb when the phone lets alarms through: at full volume before Android 17, at the phone's alarm volume on 17 and newer. At most once a minute."
                    };
                    if ui
                        .add_enabled(can_ring, egui::Button::new("Ring"))
                        .on_hover_text(why)
                        .on_disabled_hover_text(why)
                        .clicked()
                    {
                        ring = Some(device.id.clone());
                    }
                    if self.confirm_revoke.as_deref() != Some(&device.id)
                        && ui.button("Unpair…").clicked()
                    {
                        revoke = Some(Some(device.id.clone()));
                    }
                });
                // U5: "Unpair" for a pairing everywhere, asked by name.
                if self.confirm_revoke.as_deref() == Some(&device.id) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "Unpair {}? It must pair again to connect.",
                            model::device_label(&self.snapshot.devices, &device.id)
                        ));
                        if ui.button("Unpair").clicked() {
                            confirmed = Some(device.id.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            revoke = Some(None);
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "A change applies at once. Turning off control, sound or the microphone ends that phone's running session, so it reconnects with the change.",
            )
            .color(crate::theme::muted(ui)),
        );
        let mut change = None;
        let mut ask = None;
        let mut rename = None;
        for device in &self.snapshot.devices {
            let label = model::device_label(&self.snapshot.devices, &device.id);
            egui::CollapsingHeader::new(format!("What {label} may do"))
                .id_salt(("permissions", &device.id))
                .show(ui, |ui| {
                    // U3: the owner's name for it, which the phone's own
                    // never replaces; blank goes back to the phone's.
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Name");
                        let text = self.renaming.entry(device.id.clone()).or_insert_with(|| {
                            if device.renamed {
                                device.name.clone().unwrap_or_default()
                            } else {
                                String::new()
                            }
                        });
                        ui.add(
                            egui::TextEdit::singleline(text)
                                .hint_text(device.name.as_deref().unwrap_or("its own name"))
                                .desired_width(200.0),
                        );
                        if ui
                            .button("Rename")
                            .on_hover_text("Blank: the name the phone gives itself.")
                            .clicked()
                        {
                            rename = Some((device.id.clone(), text.clone()));
                        }
                    });
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
                        if !shown_for(capability, device.computer) {
                            continue;
                        }
                        let mut allowed = device.permissions.allows(capability);
                        let unavailable = (capability == Capability::AwayLock)
                            .then(away_lock_unavailable)
                            .flatten();
                        if ui
                            .add_enabled(unavailable.is_none(), egui::Checkbox::new(&mut allowed, label))
                            .on_hover_text(hint)
                            .on_disabled_hover_text(unavailable.unwrap_or(hint))
                            .changed()
                        {
                            change = Some((device.id.clone(), capability, allowed));
                        }
                        // While it is on, what it waits for, in plain sight (E5).
                        if capability == Capability::AwayLock && allowed && unavailable.is_none() {
                            let minutes = self.config.as_ref().map_or(5, |c| c.away_lock_minutes);
                            ui.label(
                                RichText::new(format!(
                                    "Locks after {minutes} min away with nobody at the keyboard (Settings, Phones)."
                                ))
                                .color(crate::theme::muted(ui)),
                            );
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
        if let Some((id, name)) = rename {
            self.renaming.remove(&id);
            self.rename_device(&id, &name);
        }
        if let Some(choice) = revoke {
            self.confirm_revoke = choice;
        }
        if let Some(id) = ring {
            self.ring(&id);
        }
        if let Some(id) = confirmed {
            self.confirm_revoke = None;
            self.revoke(&id);
        }
    }

    fn sessions_tab(&mut self, ui: &mut egui::Ui) {
        if !self.snapshot.running {
            ui.label(
                "The server is not running, so no phone is connected. Start it from Overview.",
            );
            if ui.button("Overview").clicked() {
                self.switch_to(Tab::Status);
            }
            return;
        }
        if self.snapshot.sessions.is_empty() {
            ui.label(
                "No phone is viewing this computer right now. On the phone, tap Connect on this \
                 computer's card.",
            );
            if self.snapshot.devices.is_empty() && ui.button("Pair a phone").clicked() {
                self.switch_to(Tab::Pair);
            }
            return;
        }
        let now = now_unix();
        let mut end = None;
        let mut give = None;
        let mut show_cast = false;
        // A cast whose window the owner closed (T10k).
        let cast_closed = self
            .cast
            .as_ref()
            .and_then(|c| c.showing())
            .is_some_and(|(_, closed)| closed);
        let count = self.snapshot.sessions.len();
        egui::Grid::new("sessions")
            .num_columns(5)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for session in &self.snapshot.sessions {
                    match session.device_id.as_deref() {
                        Some(id) => {
                            ui.label(model::device_label(&self.snapshot.devices, id))
                                .on_hover_text(id);
                        }
                        None => {
                            ui.monospace("unannounced");
                        }
                    }
                    ui.label(format!(
                        "{} · {}{}",
                        session.transport.to_uppercase(),
                        session.peer,
                        if session.cast {
                            " · showing its screen"
                        } else if session.control_only {
                            " · control only"
                        } else if session.sound_only {
                            " · sound only"
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
                            ui.label(RichText::new("Driving").color(crate::theme::good(ui)));
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
                    ui.horizontal(|ui| {
                        if ui.button("End").clicked() {
                            end = Some(session.clone());
                        }
                        if session.cast && cast_closed && ui.button("Show screen").clicked() {
                            show_cast = true;
                        }
                    });
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
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Playing on {}:", self.label(id)));
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
            self.notice = crate::notice::Notice::outcome(
                match model::press_phone_media(&self.state_dir, &id, command, now_unix()) {
                    // Asked, not done: the phone acts while Pocketdesk is
                    // in front there, with its media setting on.
                    Ok(()) => Ok(format!(
                        "Asked the phone to {}.",
                        match command {
                            "next" => "skip ahead",
                            "previous" => "go back",
                            _ => "play or pause",
                        }
                    )),
                    Err(e) => Err(e),
                },
            );
        }
        if show_cast && let Some(cast) = &self.cast {
            cast.reopen();
        }
        if let Some(session) = end {
            self.end_session(&session);
        }
        if let Some(id) = give {
            self.notice = crate::notice::Notice::outcome(
                match sessions::request_control(&self.state_dir, id, now_unix()) {
                    Ok(()) => Ok("Asked to hand control over.".to_string()),
                    Err(e) => Err(format!("Could not hand control over: {e}")),
                },
            );
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        // Taken out while the page draws, so the sections can also call on
        // the app's own parts (the file manager's menu, the computers).
        let mut form = match self.settings.take() {
            Some(Ok(form)) => form,
            other => {
                if let Some(Err(error)) = &other {
                    ui.label(
                        RichText::new(format!("config.toml could not be read: {error}"))
                            .color(crate::theme::bad(ui)),
                    );
                    ui.label(
                        "Fix or remove the file, then reopen this tab. Nothing is saved over it.",
                    );
                }
                self.settings = other;
                return;
            }
        };
        if form.restart_needed() {
            ui.label(
                RichText::new(format!(
                    "Restart the server to apply: {}.",
                    form.restart_names().join(", ")
                ))
                .strong(),
            );
            if self.snapshot.running {
                self.restart_button(ui);
            }
            ui.add_space(8.0);
        }
        // As the running server read it, not the box as edited (3c
        // review: requests to a server with it off lapsed unread).
        let compat_on = form.running().kdeconnect_compat;
        let kde_waiting = self.kdeconnect.waiting(&self.state_dir, now_unix());
        for section in crate::settings_view::Section::ALL {
            // A section with a problem, or a pairing request waiting, stays
            // open until it is dealt with, and its heading says why.
            let held = if form.section_has_problem(section) {
                Some("has a problem".to_string())
            } else if section == crate::settings_view::Section::KdeConnect && kde_waiting > 0 {
                Some(match kde_waiting {
                    1 => "a pairing request waits".to_string(),
                    n => format!("{n} pairing requests wait"),
                })
            } else {
                None
            };
            let mut header =
                egui::CollapsingHeader::new(section_heading(ui, section, held.as_deref()))
                    .id_salt(section.title())
                    .default_open(section.opens());
            if held.is_some() {
                header = header.open(Some(true));
            }
            header.show(ui, |ui| {
                form.show_section(ui, section);
                match section {
                    crate::settings_view::Section::Phones => {
                        self.send_menu_setting(ui);
                        self.browser_setting(ui);
                    }
                    crate::settings_view::Section::Computers => {
                        self.computers
                            .show(ui, &self.state_dir, self.snapshot.running, now_unix());
                    }
                    crate::settings_view::Section::KdeConnect => {
                        self.kdeconnect.show(
                            ui,
                            &self.state_dir,
                            self.snapshot.running && compat_on,
                            now_unix(),
                        );
                    }
                    _ => {}
                }
                ui.add_space(8.0);
            });
        }
        self.settings = Some(Ok(form));
    }

    /// Settings' Save and Undo, in a bar at the bottom while the form has
    /// changes.
    fn settings_save_bar(&mut self, ui: &mut egui::Ui) {
        let Some(Ok(form)) = &mut self.settings else {
            return;
        };
        if !form.shows_save_bar() {
            return;
        }
        let mut save = false;
        egui::Panel::bottom("save").show(ui, |ui| {
            ui.add_space(6.0);
            save = form.save_bar(ui);
            ui.add_space(6.0);
        });
        if save {
            // D3: the history turned off is emptied at once.
            let history_off = form.turns_history_off();
            match Config::path() {
                Ok(path) => match form.save_to(&path) {
                    Ok(()) => {
                        if history_off {
                            let _ = pocketdesk_core::local::clip_history::clear(&self.state_dir);
                        }
                        self.notice = crate::notice::Notice::done("Saved.");
                    }
                    Err(e) => self.notice = crate::notice::Notice::error(format!("Not saved: {e}")),
                },
                Err(e) => self.notice = crate::notice::Notice::error(format!("Not saved: {e}")),
            }
        }
    }

    /// "Send from the browser" (ecosystem 1d): the extension's host,
    /// registered by the server program, which says where in
    /// `<config>/browser/registered.json`.
    fn browser_setting(&mut self, ui: &mut egui::Ui) {
        let Ok(config) = pocketdesk_core::local::paths::config_dir() else {
            return;
        };
        // Read once, not every frame; again after a change.
        let registered = self
            .browser_registered
            .get_or_insert_with(|| {
                std::fs::read_to_string(config.join("browser").join("registered.json"))
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|v| {
                        v["browsers"].as_array().map(|list| {
                            list.iter()
                                .filter_map(|b| b.as_str().map(str::to_string))
                                .collect()
                        })
                    })
            })
            .clone();
        let installed = registered.is_some();
        let mut on = installed;
        ui.add_enabled(
            self.setting_job.is_none(),
            egui::Checkbox::new(
                &mut on,
                "Send from the browser (the Pocketdesk extension, in browser/: page, link or image to the phone)",
            ),
        );
        if let Some(browsers) = &registered {
            ui.label(
                RichText::new(if browsers.is_empty() {
                    "No browser was found to register with.".to_string()
                } else {
                    format!("Registered with {}.", browsers.join(", "))
                })
                .small()
                .color(crate::theme::muted(ui)),
            );
        }
        if on == installed {
            return;
        }
        let Some(exe) = crate::restart::server_exe_beside_app() else {
            self.notice = crate::notice::Notice::error(
                "Not changed: the server program is not beside this app",
            );
            return;
        };
        let flag = if on { "--install" } else { "--remove" };
        let doing = if on {
            "Registering with the browsers"
        } else {
            "Removing from the browsers"
        };
        self.run_setting(doing, move || {
            let out = crate::restart::quiet(exe)
                .args(["browser-host", flag])
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
                Ok(if said.is_empty() {
                    "Done.".to_string()
                } else {
                    said
                })
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        });
    }

    /// "Send to phone" in the file manager (T10e): Explorer's Send To, or the
    /// Nautilus, Nemo and Dolphin menus.
    fn send_menu_setting(&mut self, ui: &mut egui::Ui) {
        use pocketdesk_core::local::send_menu;
        ui.add_space(8.0);
        let Ok(data) = send_menu::data_dir() else {
            return;
        };
        // Read once, not every frame; again after a change.
        let installed = *self
            .send_menu_installed
            .get_or_insert_with(|| send_menu::installed_in(&data));
        let mut on = installed;
        ui.add_enabled(
            self.setting_job.is_none(),
            egui::Checkbox::new(
                &mut on,
                "\"Send to phone\" in the file manager (sends to the connected phone)",
            ),
        );
        if on == installed {
            return;
        }
        if on {
            let Some(exe) = crate::restart::server_exe_beside_app() else {
                self.notice = crate::notice::Notice::error(
                    "Not changed: the server program is not beside this app",
                );
                return;
            };
            self.run_setting("Adding to the file manager", move || {
                send_menu::install_in(&data, &exe)
                    .map(|()| "Added to the file manager.".to_string())
                    .map_err(|e| e.to_string())
            });
        } else {
            self.run_setting("Removing from the file manager", move || {
                send_menu::remove_in(&data)
                    .map(|()| "Removed from the file manager.".to_string())
                    .map_err(|e| e.to_string())
            });
        }
    }

    fn files_tab(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.send_box(ui);
            ui.add_space(12.0);
            self.under_way_list(ui);
            self.sent_list(ui);
            self.received_list(ui);
        });
    }

    /// The phone the send box shows (F7b): the one chosen while it is in a
    /// session, else the first phone that is.
    fn send_target(&self) -> Option<String> {
        let targets = model::send_target_ids(&self.snapshot);
        self.send_to
            .as_ref()
            .filter(|id| targets.contains(id))
            .cloned()
            .or_else(|| targets.first().cloned())
    }

    /// Queue `path` for the chosen phone and say what happened. A chosen
    /// phone that has left is reported, never swapped for another.
    fn send_file(&mut self, path: &std::path::Path) {
        let device = match model::target_for_send(&self.snapshot, self.send_to.as_deref()) {
            Ok(device) => device,
            Err(e) => {
                self.send_to = None;
                self.notice = crate::notice::Notice::error(e);
                return;
            }
        };
        self.notice = crate::notice::Notice::outcome(
            match model::queue_send(&self.state_dir, &device, path, now_unix()) {
                Ok(name) => Ok(format!(
                    "Queued {name} for {}: the Sent list says when it has gone.",
                    model::target_label(&self.snapshot, &device)
                )),
                Err(e) => Err(e),
            },
        );
        self.refreshed = None;
    }

    fn send_box(&mut self, ui: &mut egui::Ui) {
        ui.strong("Send a file");
        let Some(device) = self.send_target() else {
            ui.label(
                RichText::new(
                    "No phone or computer can take a file right now. A phone can in a session, or \
                     with \"Files from the computer in the background\" on in Pocketdesk's Settings on the phone; \
                     another of your computers, while its link is up. Files land in the \
                     phone's folder for files from computers (Downloads/Pocketdesk unless chosen).",
                )
                .color(crate::theme::muted(ui)),
            );
            // Not only a session: a phone linked at home or by the internet
            // road, or another computer, takes files too (U5).
            if self.snapshot.devices.is_empty() && ui.button("Pair a phone").clicked() {
                self.switch_to(Tab::Pair);
            }
            return;
        };
        let targets = model::send_target_ids(&self.snapshot);
        let mut chosen = device.clone();
        let label = |id: &str| model::target_label(&self.snapshot, id);
        ui.horizontal(|ui| {
            ui.label("To");
            egui::ComboBox::from_id_salt("send-to")
                .selected_text(label(&chosen))
                .show_ui(ui, |ui| {
                    for id in &targets {
                        ui.selectable_value(&mut chosen, id.clone(), label(id));
                    }
                });
        });
        self.send_to = Some(chosen);
        let mut send = None;
        // Wrapped: the box and three buttons are wider than a narrow page.
        ui.horizontal_wrapped(|ui| {
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
        ui.horizontal_wrapped(|ui| {
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
            // A link opens on a phone; another computer takes files (2h).
            if self
                .snapshot
                .linked_computers
                .iter()
                .any(|(id, _)| *id == device)
            {
                self.notice = crate::notice::Notice::error(
                    "Links go to phones: choose a phone to send it to.",
                );
                return;
            }
            self.notice = crate::notice::Notice::outcome(
                match model::queue_link(&self.state_dir, &device, &url, now_unix()) {
                    Ok(said) => {
                        self.send_link.clear();
                        Ok(said)
                    }
                    Err(e) => Err(e),
                },
            );
        }
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        let hint = if hovering {
            RichText::new("Drop to send.").strong()
        } else {
            RichText::new("Or drop files or folders on this window.").color(crate::theme::muted(ui))
        };
        ui.label(hint);
    }

    /// Files moving now, each with its bar and Cancel (C1).
    fn under_way_list(&mut self, ui: &mut egui::Ui) {
        if self.snapshot.under_way.is_empty() {
            return;
        }
        ui.strong("Under way");
        let mut cancel = None;
        egui::Grid::new("under_way")
            .num_columns(4)
            .striped(true)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                for transfer in &self.snapshot.under_way {
                    ui.label(model::transfer_title(transfer));
                    let other = model::target_label(&self.snapshot, &transfer.device_id);
                    ui.label(match transfer.direction {
                        pocketdesk_core::local::transfers::Direction::ToPhone => {
                            format!("to {other}")
                        }
                        pocketdesk_core::local::transfers::Direction::FromPhone => {
                            format!("from {other}")
                        }
                    });
                    ui.add(
                        egui::ProgressBar::new(model::transfer_fraction(transfer))
                            .desired_width(220.0)
                            .text(model::transfer_words(transfer)),
                    );
                    if ui
                        .button("Cancel")
                        .on_hover_text("Stop it on both ends; sending it again continues")
                        .clicked()
                    {
                        cancel = Some((transfer.id.clone(), transfer.name.clone()));
                    }
                    ui.end_row();
                }
            });
        if let Some((id, name)) = cancel {
            self.notice = match pocketdesk_core::local::transfers::request_cancel(
                &self.state_dir,
                &id,
                now_unix(),
            ) {
                // Short: the row goes once it stops (C1 review).
                Ok(_) => crate::notice::Notice::done(format!("Cancel asked for {name}.")),
                Err(e) => crate::notice::Notice::error(format!("Could not cancel {name}: {e}")),
            };
        }
        ui.add_space(12.0);
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
                    ui.label(model::target_label(&self.snapshot, &file.device_id));
                    ui.label(crate::words::ago(now, file.sent_unix));
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
                                .color(crate::theme::bad(ui)),
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
                    "Share a file to Pocketdesk on your phone; it lands where Settings says (Downloads unless chosen) and is \
                     listed here.",
                )
                .color(crate::theme::muted(ui)),
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
                    ui.label(crate::words::ago(now, file.received_unix));
                    if row.exists {
                        if ui.button("Show in folder").clicked() {
                            show = Some(file.path.clone());
                        }
                    } else {
                        ui.label(RichText::new("moved or deleted").color(crate::theme::muted(ui)));
                    }
                    ui.end_row();
                }
            });
        if let Some(path) = show
            && let Err(e) = crate::reveal::show_in_folder(std::path::Path::new(&path))
        {
            self.notice = crate::notice::Notice::error(format!("Could not open the folder: {e}"));
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
    /// The page shown, kept for the next run (eframe keeps the window's
    /// size and place itself).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(PAGE_KEY, kept_page(self.chosen).key().to_string());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.pair.reset(&self.state_dir);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_tray(ctx);
        // Started again: bring this window forward, from the tray too.
        if crate::single_instance::take_raise() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.finish_job();
        self.finish_setting();
        // Closing hides to the tray; Quit (or no tray to come back from) exits.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && closes_to_tray(self.tray.is_some(), on_wayland())
        {
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
            // E2: a page asked for by the tray program or a second launch.
            if let Some(open) = pocketdesk_core::local::app_open::take(&self.state_dir, now_unix())
            {
                self.open_asked(ctx, open);
            }
            self.raise_for_approvals(ctx);
            self.raise_for_tasks(ctx);
            self.update_tray();
        }
        self.cast
            .get_or_insert_with(|| crate::cast_view::CastView::new(ctx, self.state_dir.clone()))
            .show(
                ctx,
                &self.snapshot.devices,
                self.config.as_ref().is_none_or(|c| c.cast_control),
            );
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.take_files_to_send(ui.ctx());
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(170.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("Pocketdesk");
                // Scrolls when the window is short (U1 review).
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        for group in crate::nav::GROUPS {
                            ui.add_space(10.0);
                            if let Some(title) = group.title {
                                ui.label(
                                    RichText::new(title).small().color(crate::theme::muted(ui)),
                                );
                            }
                            for tab in group.pages {
                                if ui.selectable_label(self.tab == *tab, tab.name()).clicked() {
                                    self.switch_to(*tab);
                                    self.chosen = *tab;
                                }
                            }
                        }
                    });
                });
            });
        if self.tab == Tab::Settings {
            self.settings_save_bar(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            crate::notice::show(ui, &mut self.notice);
            ui.heading(self.tab.name());
            ui.label(RichText::new(self.tab.purpose()).color(crate::theme::muted(ui)));
            ui.add_space(6.0);
            if self.tab.scrolls_itself() {
                self.page(ui);
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.page(ui));
            }
        });
        // What a page said happened becomes the one notice (E5).
        let said = [
            self.notes.take_notice(),
            self.phone_files.take_notice(),
            self.photos.take_notice(),
            self.kdeconnect.take_notice(),
            self.computers.take_notice(),
            self.messages.take_notice(),
        ];
        if let Some(notice) = said.into_iter().flatten().last() {
            self.notice = Some(notice);
            // Shown now, not at the next refresh (E5 review).
            ui.ctx().request_repaint();
        }
        // An empty page's button asked for another page (E5).
        if let Some(tab) = self.notes.take_go_to().or(self.phone_files.take_go_to()) {
            self.switch_to(tab);
        }
        self.approval_prompt(ui);
        self.task_prompt(ui);
    }
}

/// A coloured dot and a bold line. The dot is painted: the bundled fonts
/// have no "●" glyph.
fn status_line(ui: &mut egui::Ui, token: crate::theme::Token, text: &str) {
    let color = crate::theme::pick(ui, token);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
        ui.label(RichText::new(text).color(color).strong());
    });
}

/// The page kept for the next run when `chosen` was the last chosen: never
/// Pair, whose page shows a fresh code (a PIN) nobody asked for at launch
/// (U1 review).
pub fn kept_page(chosen: Tab) -> Tab {
    if chosen == Tab::Pair {
        Tab::Status
    } else {
        chosen
    }
}

/// "Phone", the device's name (said as its own claim unless the owner gave
/// it) and its id: where a decision rests on who asks (U3).
fn phone_row(ui: &mut egui::Ui, devices: &[model::Device], id: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Phone");
        if let Some(device) = devices.iter().find(|d| d.id == id)
            && let Some(name) = &device.name
        {
            ui.label(RichText::new(name).strong());
            if !device.renamed {
                ui.label(RichText::new("(calls itself)").color(crate::theme::muted(ui)));
            }
        }
        ui.monospace(short(id)).on_hover_text(id);
    });
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
    if now_unix.saturating_sub(battery.updated_unix) >= 60 {
        text.push_str(&format!(
            " · {}",
            crate::words::ago(now_unix, battery.updated_unix)
        ));
    }
    text
}

/// A Settings section's heading: its title, and what is in it, quieter.
fn section_heading(
    ui: &egui::Ui,
    section: crate::settings_view::Section,
    held: Option<&str>,
) -> egui::WidgetText {
    let mut job = egui::text::LayoutJob::default();
    RichText::new(section.title()).strong().append_to(
        &mut job,
        ui.style(),
        egui::FontSelection::Default,
        egui::Align::Center,
    );
    RichText::new(format!("   {}", section.summary()))
        .small()
        .weak()
        .append_to(
            &mut job,
            ui.style(),
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    // Why it will not close: a problem, or a request waiting.
    if let Some(held) = held {
        RichText::new(format!("   · {held}"))
            .small()
            .color(crate::theme::warn(ui))
            .append_to(
                &mut job,
                ui.style(),
                egui::FontSelection::Default,
                egui::Align::Center,
            );
    }
    job.into()
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

/// Whether closing the window hides it to the tray: only with a tray, and
/// not on Wayland, which lets no app hide its own window (closing there
/// would do nothing at all, N6 review), so it quits as without a tray.
fn closes_to_tray(has_tray: bool, wayland: bool) -> bool {
    has_tray && !wayland
}

/// Whether the window is a Wayland one: winit takes Wayland when the
/// session offers it.
fn on_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_paired_computer_is_not_shown_a_phone_s_switches() {
        for capability in [
            Capability::Ring,
            Capability::Mute,
            Capability::AwayLock,
            Capability::Cast,
        ] {
            assert!(!shown_for(capability, true), "{capability:?}");
            assert!(shown_for(capability, false), "{capability:?}");
        }
        assert!(shown_for(Capability::Clipboard, true));
        assert!(shown_for(Capability::Files, true));
    }

    #[test]
    fn the_away_lock_says_why_it_is_off_where_it_never_locks() {
        assert_eq!(away_lock_unavailable().is_none(), cfg!(windows));
    }

    /// N6 review: Wayland lets no app hide its own window.
    #[test]
    fn closing_hides_to_the_tray_except_on_wayland() {
        use super::closes_to_tray;
        assert!(closes_to_tray(true, false));
        assert!(!closes_to_tray(true, true), "Wayland: closing quits");
        assert!(!closes_to_tray(false, false), "no tray: closing quits");
    }

    #[test]
    fn the_page_kept_is_the_one_chosen_and_never_pair() {
        use super::{Tab, kept_page};
        assert_eq!(kept_page(Tab::Settings), Tab::Settings);
        assert_eq!(
            kept_page(Tab::Pair),
            Tab::Status,
            "no code minted at launch"
        );
    }

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
            "54 % · 3 min ago"
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
