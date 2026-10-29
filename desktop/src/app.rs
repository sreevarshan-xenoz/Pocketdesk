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

/// The features both sides must have on, for one phone (L7): a block per
/// feature (it wraps at the minimum width, E5), with what holds it back.
fn switches_table(
    ui: &mut egui::Ui,
    device: &model::Device,
    settings_off: &[(&'static str, &'static str)],
) {
    let yes_no = |on: Option<bool>| match on {
        Some(true) => "on",
        Some(false) => "off",
        None => "not said",
    };
    match device.switches.as_ref() {
        None => {
            ui.label(
                RichText::new(
                    "The phone has not said which of its features are on. It needs this \
                     version of Pocketdesk and \"Share the phone's state with the computer\" on.",
                )
                .color(crate::theme::muted(ui)),
            );
        }
        Some(said) => {
            let now = pocketdesk_core::local::pin::now_unix();
            ui.label(
                RichText::new(format!(
                    "As the phone said {}.",
                    crate::words::ago(now, said.updated_unix)
                ))
                .color(crate::theme::muted(ui)),
            );
        }
    }
    for row in model::switch_rows(&device.permissions, device.switches.as_ref(), settings_off) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let works = row.advice.is_none() && row.phone == Some(true) && row.here;
            let mark = if row.advice.is_some() {
                RichText::new("✕").color(crate::theme::warn(ui))
            } else if works {
                RichText::new("✓").color(crate::theme::good(ui))
            } else {
                RichText::new("·").color(crate::theme::muted(ui))
            };
            ui.label(mark);
            ui.label(RichText::new(row.what).strong());
        });
        let mut sides = format!(
            "This computer allows: {} · The phone sends: {}",
            if row.here { "yes" } else { "no" },
            yes_no(row.phone)
        );
        if let Some(android) = row.android {
            sides.push_str(&format!(
                " · Android allows: {}",
                if android { "yes" } else { "no" }
            ));
        }
        ui.label(RichText::new(sides).color(crate::theme::muted(ui)));
        if let Some(advice) = row.advice {
            ui.label(RichText::new(advice).color(crate::theme::warn(ui)));
        }
    }
}

/// The Devices tab's switches, in the order a person thinks of them.
pub(crate) const PERMISSIONS: [(Capability, &str, &str); 25] = [
    (
        Capability::Input,
        "Control the mouse and keyboard",
        "Off: the phone can watch the screen but not touch it.",
    ),
    (
        Capability::SharedInput,
        "Keyboard and mouse",
        "This computer may move its mouse and keyboard onto this one",
    ),
    (
        Capability::Clipboard,
        "Share the clipboard",
        "Copy on one, paste on the other.",
    ),
    (
        Capability::Files,
        "Send files",
        "Send files to this computer, and share the phone's own folders and photos here.",
    ),
    (
        Capability::Browse,
        "Browse this computer's files",
        "List this computer's folders from the phone and download from them.",
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
        "Texts the phone receives appear in Messages here, and replies go out through the phone. Messages can also ask for the phone's contacts; turning this off forgets the contacts kept here.",
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
        "Show its screen or camera here",
        "The phone can show its own screen, or its camera, in a window here, when its owner starts it on the phone.",
    ),
    (
        Capability::Dial,
        "Call numbers from here",
        "Call on the Messages page (and pocketdesk call) opens the phone's dialer with the number; you press Call on the phone. The phone takes it with \u{201c}Calls from the computer\u{201d} on.",
    ),
    (
        Capability::Command,
        "Send commands to its automation apps",
        "A script here can hand a name and a text to Tasker, Automate or MacroDroid on the phone, with \u{201c}Commands from the computer\u{201d} on there.",
    ),
    (
        Capability::PhoneActions,
        "Take screenshots of it, lock it and open its apps from here",
        "Screenshot of the phone and Lock the phone, on the Overview and in the tray, and Open an app on the phone\u{2026} on the Overview. The phone takes a screenshot or locks with \u{201c}Let the computer take screenshots and lock this phone\u{201d} on and its Pocketdesk control service, and opens apps with \u{201c}Let the computer open my apps\u{201d} on; a screenshot arrives as its new screenshots do.",
    ),
    (
        Capability::Notes,
        "Add to my notes",
        "Text or a link shared to Note to computer on the phone is added, with the time, to the notes file picked in Settings (Phones). Off unless you turn it on.",
    ),
    (
        Capability::Ask,
        "Answer my questions",
        "A script here can ask a Yes or No with pocketdesk ask, and wait for the answer from the phone: No is one tap there, Yes takes its fingerprint or screen lock. Off unless you turn it on: a Yes can let a deploy through. The phone takes questions with \u{201c}Questions from the computer\u{201d} on.",
    ),
    (
        Capability::Peek,
        "Peek at this screen",
        "The phone can see this screen as one still, from its Home, with no session; each look is a new still, never a stream. Off unless you turn it on; held while this computer is paused, and by \u{201c}Phones may peek at this screen\u{201d} in Settings.",
    ),
    (
        Capability::PhoneSound,
        "Play its sound here",
        "With its screen shown here, what the phone's apps play comes out of this computer's speakers (when the phone's own setting sends it).",
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
        "When the phone's link on this computer's network has been gone for the time in Settings (Phones) and nobody has used the keyboard or mouse meanwhile, this computer locks, once per departure. A phone linked from away (the internet road or a tailnet) counts as gone. A phone asleep may lose its link, so reading or watching without touching anything can lock too. On Linux it reads the keyboard's idle time from GNOME, KDE, the compositor (Sway, Hyprland and others with ext-idle-notify) or X11 (xprintidle), and never locks on a desktop that says none of these.",
    ),
];

/// The switches a profile sets on a device: those its page shows, and
/// AwayLock only where it works (Q2 review: the question named one Apply
/// then left alone).
fn profile_touches(capability: Capability, computer: bool) -> bool {
    profile_carries(capability, computer)
        && (capability != Capability::AwayLock || away_lock_unavailable().is_none())
}

/// Whether a profile carries `capability` from or to a device's page: what
/// the page shows, but a phone's Keyboard and mouse, which says the other
/// way round from a computer's (BK5: this computer onto the phone), is
/// only ever set on its own.
fn profile_carries(capability: Capability, computer: bool) -> bool {
    shown_for(capability, computer) && (computer || capability != Capability::SharedInput)
}

/// What `capability`'s switch says of itself on a device's page: a phone's
/// Keyboard and mouse is this computer's onto the phone (BK5).
fn hint_for(capability: Capability, computer: bool, hint: &'static str) -> &'static str {
    if capability == Capability::Input && computer {
        COMPUTER_CONTROL
    } else if capability == Capability::SharedInput && !computer {
        "This computer may move its keyboard and mouse onto this phone, used as another screen beside it (the phone must allow it too)"
    } else {
        hint
    }
}

/// What a paired computer's Control switch does (BK7): it can show this
/// screen in a window there, and with this on, control this computer from it.
pub(crate) const COMPUTER_CONTROL: &str = "That computer can show this screen in a window (Show its screen, in its Settings under Other computers). On, it can also control this one from that window; off, it can only watch.";

/// Whether a device is shown `capability`'s switch (E5): a paired computer
/// is not a phone to ring, mute, cast or lock this computer by.
fn shown_for(capability: Capability, computer: bool) -> bool {
    // A computer may drive this one; this one may drive a phone (BK5).
    if capability == Capability::SharedInput {
        return true;
    }
    !(computer
        && matches!(
            capability,
            Capability::Ring
                | Capability::Mute
                | Capability::AwayLock
                | Capability::Cast
                | Capability::PhoneSound
                | Capability::Dial
                | Capability::Command
                | Capability::PhoneActions
                | Capability::Peek
                | Capability::Ask
                | Capability::Notes
        ))
}

/// Why "Lock this computer when this phone leaves" cannot be turned on
/// here, if it cannot: this computer cannot tell yet whether someone is at
/// it (E5).
fn away_lock_unavailable() -> Option<&'static str> {
    // R3: Linux reads the keyboard's idle time from GNOME, KDE or X11.
    (!cfg!(any(windows, target_os = "linux")))
        .then_some("This system cannot tell whether someone is at the keyboard, so it never locks.")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    Activity,
    Pair,
    Devices,
    KeyboardMouse,
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
    /// The Logs page: it reads the log itself, when it changed (J1).
    logs: crate::logs_view::LogsView,
    refreshed: Option<Instant>,
    /// The pause as last read, and when (K2a review: once a second).
    pause_read: Option<(
        Instant,
        Result<Option<pocketdesk_core::local::pause::Pause>, String>,
    )>,
    /// A device the user asked to revoke, awaiting confirmation.
    confirm_revoke: Option<String>,
    /// A profile to apply to a phone, asked first (Q2 review).
    confirm_profile: Option<(String, pocketdesk_core::local::permission_profiles::Profile)>,
    /// A profile to remove, asked first under the phone it was chosen on.
    confirm_remove_profile: Option<(String, String)>,
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
    /// What "Find a setting" holds (J2).
    settings_query: String,
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
    /// The Devices page's profile names being typed, by phone (P9).
    profile_names: std::collections::HashMap<String, String>,
    /// The others a send goes to as well (P6).
    send_also: Vec<String>,
    /// Files being read from the clipboard (P7).
    pasting:
        Option<std::thread::JoinHandle<Option<pocketdesk_core::local::clipboard_files::Copied>>>,
    /// Files dropped while several phones or computers could take them,
    /// waiting for the owner to say which (J5).
    dropped: Vec<PathBuf>,
    /// The Files tab's path field.
    send_path: String,
    /// The link the send box hands to the phone (T8a).
    send_link: String,
    /// Each phone's last session, for the Overview (N6), read while it shows.
    last_sessions: std::collections::HashMap<String, u64>,
    /// The session log as last read for [`Self::last_sessions`].
    last_sessions_stamp: Option<(u64, std::time::SystemTime)>,
    /// The Sessions page's recent sessions, and the log as they were read
    /// from it (J3).
    recent_sessions: Vec<model::RecentSession>,
    recent_sessions_stamp: Option<(u64, std::time::SystemTime)>,
    /// The Overview's link box: the phone it is for, and the link typed.
    overview_link: Option<(String, String)>,
    /// The link box was just opened: its field takes the cursor once.
    overview_link_focus: bool,
    /// The Overview's "Open an app on the phone…" box (P2).
    apps_box: Option<crate::apps_view::AppsBox>,
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
    /// The Keyboard and mouse page (shared input).
    arrange: crate::arrange_view::ArrangeView,
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
    /// Whether phone numbers' links open on the phone, as last read (M8).
    tel_registered: Option<bool>,
    /// A link's number waiting for a phone (M8), and when it was read.
    tel_ask: Option<pocketdesk_core::local::tel_ask::TelAsk>,
    tel_ask_read: Option<std::time::Instant>,
    /// The window showing a phone's screen (T10k), started on the first pass.
    cast: Option<crate::cast_view::CastView>,
    /// The window showing another computer's screen (BK7), from the first
    /// "Show its screen".
    computer_view: Option<crate::computer_view::ComputerView>,
    /// M4: the typing window, while it types on a phone.
    type_view: crate::type_view::TypeView,
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
            logs: Default::default(),
            refreshed: None,
            pause_read: None,
            confirm_revoke: None,
            confirm_profile: None,
            confirm_remove_profile: None,
            renaming: std::collections::HashMap::new(),
            notice: None,
            quitting: false,
            started_unix: now_unix(),
            pair: PairView::default(),
            settings: None,
            settings_query: String::new(),
            job: None,
            setting_job: None,
            confirm_restart: false,
            raised_approvals: Vec::new(),
            send_to: None,
            profile_names: std::collections::HashMap::new(),
            send_also: Vec::new(),
            pasting: None,
            dropped: Vec::new(),
            send_path: String::new(),
            send_link: String::new(),
            last_sessions: Default::default(),
            last_sessions_stamp: None,
            recent_sessions: Vec::new(),
            recent_sessions_stamp: None,
            overview_link: None,
            overview_link_focus: false,
            apps_box: None,
            picking: None,
            diagnostics: Default::default(),
            messages: Default::default(),
            clipboard: Default::default(),
            notes: Default::default(),
            computers: Default::default(),
            arrange: Default::default(),
            kdeconnect: Default::default(),
            phone_files: Default::default(),
            photos: Default::default(),
            send_menu_installed: None,
            browser_registered: None,
            tel_registered: None,
            tel_ask: None,
            tel_ask_read: None,
            cast: None,
            computer_view: None,
            type_view: Default::default(),
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
        self.tel_registered = None;
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
                // Esc answers Cancel (I3).
                if ui.button("Cancel").clicked() || crate::keys::escaped(ui) {
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

    /// The Sessions page's recent sessions, read again only when the log
    /// changed (J3).
    fn read_recent_sessions(&mut self) {
        let stamp = model::session_log_stamp(&self.state_dir);
        if stamp != self.recent_sessions_stamp {
            self.recent_sessions =
                model::recent_sessions(&model::session_log_tail(&self.state_dir), 10);
            self.recent_sessions_stamp = stamp;
        }
    }

    fn switch_to(&mut self, tab: Tab) {
        // The Overview's link box is for the moment it was opened in.
        if tab != self.tab {
            self.overview_link = None;
        }
        // Dropped files waiting for "which?" go when the page does, and say
        // so, rather than waiting unseen for a click hours later (J5 review).
        if self.tab == Tab::Files && tab != Tab::Files && !self.dropped.is_empty() {
            let what = dropped_words(&std::mem::take(&mut self.dropped));
            self.notice = crate::notice::Notice::done(format!(
                "Not sent: {what}, since you left the question."
            ));
        }
        if self.tab == Tab::Pair && tab != Tab::Pair {
            self.pair.reset(&self.state_dir);
        }
        if tab == Tab::Activity && self.tab != Tab::Activity {
            self.activity.reopen();
        }
        // Read before the page's first frame, not after it (J3 review: the
        // list popped in a frame late, or showed the last visit's).
        if tab == Tab::Sessions {
            self.read_recent_sessions();
        }
        if tab == Tab::Settings {
            // Read again on each visit: it may have changed outside the app.
            self.send_menu_installed = None;
            self.browser_registered = None;
            self.tel_registered = None;
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
        if self.tab == Tab::Status {
            // Read again only when the log changed, not every second.
            let stamp = model::session_log_stamp(&self.state_dir);
            if stamp != self.last_sessions_stamp {
                self.last_sessions =
                    model::last_sessions(&model::session_log_tail(&self.state_dir));
                self.last_sessions_stamp = stamp;
            }
        }
        if self.tab == Tab::Sessions {
            self.read_recent_sessions();
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
                    self.send_also.clear();
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
                TrayAction::TakePhoto(id) => {
                    self.take_photo(&id);
                    false
                }
                TrayAction::Type(id) => {
                    self.type_on(&id);
                    true
                }
                TrayAction::PhoneAction(id, action) => {
                    self.phone_action(&id, action);
                    false
                }
                TrayAction::Pause(minutes) => {
                    self.pause(minutes);
                    false
                }
                TrayAction::PhoneMedia(id, command) => {
                    self.press_phone_media(&id, command);
                    false
                }
                TrayAction::Resume => {
                    self.resume();
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

    /// Open a window on the computer `device_id`'s screen (BK7), at the
    /// quality, view-only and stats the settings start it with.
    fn show_computer(&mut self, ctx: &egui::Context, device_id: &str, name: &str) {
        let (preset, view_only, stats) = self.config.as_ref().map_or(
            (
                pocketdesk_core::streaming::input_packet::PRESET_AUTO,
                false,
                false,
            ),
            |c| {
                (
                    c.computer_view_preset(),
                    c.computer_view_only,
                    c.computer_view_stats,
                )
            },
        );
        let view = self.computer_view.get_or_insert_with(|| {
            crate::computer_view::ComputerView::new(
                ctx,
                self.state_dir.clone(),
                Config::path().ok(),
            )
        });
        if let Err(e) = view.open(device_id, name, 0, preset, view_only, stats) {
            self.notice = crate::notice::Notice::error(e);
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
        // N9: the typing window, from the tray program or its key.
        if open.page == pocketdesk_core::local::app_open::TYPE {
            if let Some(id) = &open.phone {
                self.type_on(id);
            }
        } else if let Some(tab) = Tab::from_key(&open.page) {
            if tab == Tab::Files && open.phone.is_some() {
                self.send_to = open.phone;
                self.send_also.clear();
            }
            self.switch_to(tab);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Open the typing window on phone `id` (M4), by its name as shown.
    fn type_on(&mut self, id: &str) {
        let name = crate::model::device_label(&self.snapshot.devices, id);
        self.type_view.open(&self.state_dir, id, &name);
    }

    /// The tray's menu for the phones as last read (D5); rebuilt by the
    /// tray only when it changes.
    fn update_tray(&mut self) {
        let Some(tray) = self.tray.as_mut() else {
            return;
        };
        let rows = model::phone_rows(&self.snapshot, &self.last_sessions);
        let now = now_unix();
        tray.show(crate::tray_menu::with_pause(
            crate::tray_menu::menu(&rows, &self.snapshot.devices, now),
            pocketdesk_core::local::pause::current(&self.state_dir, now).as_ref(),
            now,
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
        // Its contacts go with it (P10).
        let _ = pocketdesk_core::local::phone_contacts::forget(&self.state_dir, id);
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
        // Its sessions alone (AY3): as a kick target, an id of "all" or a
        // prefix of another phone's would end theirs too.
        if let Err(e) = sessions::request_kick_device(&self.state_dir, id, now_unix()) {
            self.notice = crate::notice::Notice::error(format!(
                "Could not end {}'s session: {e}",
                self.label(id)
            ));
        }
    }

    /// Give `id` the profile's permissions (P9), in one write, its session
    /// ended once if a permission it started with was turned off.
    fn apply_profile(
        &mut self,
        id: &str,
        profile: &pocketdesk_core::local::permission_profiles::Profile,
    ) {
        let Some(device) = self.snapshot.devices.iter().find(|d| d.id == id).cloned() else {
            return;
        };
        let shown = |c: Capability| profile_touches(c, device.computer);
        let label = model::device_label(&self.snapshot.devices, id);
        match model::apply_profile(&self.state_dir, id, &profile.permissions, shown) {
            Ok(narrowed) => {
                if narrowed {
                    self.end_sessions_of(id);
                }
                self.notice = crate::notice::Notice::done(format!(
                    "{label} has the {} profile's permissions now{}.",
                    profile.name,
                    if narrowed {
                        "; its session was ended, to reconnect with them"
                    } else {
                        ""
                    }
                ));
            }
            Err(e) => {
                self.notice = crate::notice::Notice::error(format!(
                    "Could not give {label} the {} profile: {e}",
                    profile.name
                ))
            }
        }
        self.refresh();
    }

    fn set_permission(&mut self, id: &str, capability: Capability, allowed: bool) {
        // P10: a phone's contacts go with its texts.
        if capability == Capability::Texts && !allowed {
            let _ = pocketdesk_core::local::phone_contacts::forget(&self.state_dir, id);
        }
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
        // A device id names its device alone (AY3 review: as a kick target,
        // "all" or a prefix of another's id named others too); a session
        // that never announced one is ended by its address.
        let (target, asked) = match &session.device_id {
            Some(id) => (
                id.clone(),
                sessions::request_kick_device(&self.state_dir, id, now_unix()),
            ),
            None => {
                let ip = session
                    .peer
                    .rsplit_once(':')
                    .map_or(session.peer.clone(), |(ip, _)| ip.to_string());
                let asked = sessions::request_kick(&self.state_dir, &ip, now_unix());
                (ip, asked)
            }
        };
        self.notice = crate::notice::Notice::outcome(match asked {
            Ok(()) => Ok(format!(
                "Asked the session from {} to end.",
                self.label(&target)
            )),
            Err(e) => Err(format!("Could not end the session: {e}")),
        });
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
                    Some(crate::activity_view::Action::Mute(app)) => self.mute_app(&app),
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
            Tab::KeyboardMouse => self.keyboard_mouse_tab(ui),
            Tab::Sessions => self.sessions_tab(ui),
            Tab::Files => self.files_tab(ui),
            Tab::Messages => {
                self.messages
                    .show(ui, &self.state_dir, &self.snapshot.devices, now_unix())
            }
            Tab::Notifications => {
                // N4: no cards with the setting off (N4 review).
                self.notes.live_off = self.config.as_ref().is_some_and(|c| !c.phone_live_updates);
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
            Tab::Logs => self.logs.show(ui, &self.state_dir),
        }
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        self.pause_row(ui);
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
                    let port = self.snapshot.control_port(config.control_port);
                    if port == config.control_port {
                        ui.label(format!("TCP {port}"));
                    } else {
                        // T1: another program holds the configured one.
                        ui.label(format!(
                            "TCP {port} (TCP {} is held by another program)",
                            config.control_port
                        ));
                    }
                    ui.end_row();
                    ui.label("Streaming port");
                    ui.label(format!("UDP {}", config.streaming_port));
                    ui.end_row();
                    ui.label("Pairing required");
                    ui.label(if config.pairing_required { "yes" } else { "no" });
                    ui.end_row();
                }
            });
        // Not a fact among the others: a warning, with the way to change it (I3).
        if self.config.as_ref().is_some_and(|c| !c.pairing_required) {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(
                        "Pairing is not required: any device that reaches this computer can connect without pairing.",
                    )
                    .color(crate::theme::warn(ui)),
                );
                if ui.button("Open Settings").clicked() {
                    self.switch_to(Tab::Settings);
                    self.chosen = Tab::Settings;
                }
            });
        }
        ui.add_space(16.0);
        self.phones_panel(ui);
    }

    /// Send a press to `id`'s player, saying it was asked (the Sessions
    /// page, the Overview and the window's tray, N2).
    fn press_phone_media(&mut self, id: &str, command: &str) {
        self.notice = crate::notice::Notice::outcome(
            match model::press_phone_media(&self.state_dir, id, command, now_unix()) {
                // Asked, not done: the phone acts while it reports its
                // music, with its media setting on.
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

    /// Pause this computer for `minutes`, or until resumed (K2a).
    fn pause(&mut self, minutes: Option<u32>) {
        self.pause_read = None;
        self.notice = crate::notice::Notice::outcome(
            pocketdesk_core::local::pause::pause(&self.state_dir, minutes, now_unix())
                .map(|_| match minutes {
                    Some(m) => format!("Paused for {}.", crate::tray_menu::span(m)),
                    None => "Paused until you resume.".to_string(),
                })
                .map_err(|e| format!("Could not pause: {e}")),
        );
    }

    /// End the pause (K2a).
    fn resume(&mut self) {
        self.pause_read = None;
        self.notice = crate::notice::Notice::outcome(
            pocketdesk_core::local::pause::resume(&self.state_dir)
                .map(|()| "Resumed.".to_string())
                .map_err(|e| format!("Could not resume: {e}")),
        );
    }

    /// The pause in force at `now`, read at most once a second (K2a review:
    /// not on every frame the mouse moves), and at once after a change.
    fn pause_now(
        &mut self,
        now: u64,
    ) -> Result<Option<pocketdesk_core::local::pause::Pause>, String> {
        let stale = self
            .pause_read
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(1));
        if stale {
            let read =
                pocketdesk_core::local::pause::load(&self.state_dir).map_err(|e| e.to_string());
            self.pause_read = Some((Instant::now(), read));
        }
        match &self.pause_read {
            Some((_, Ok(pause))) => Ok(pause.filter(|p| p.active(now))),
            Some((_, Err(e))) => Err(e.clone()),
            None => Ok(None),
        }
    }

    /// The Overview's pause (K2a): while paused, what is held and Resume;
    /// else Pause notifications with its three.
    fn pause_row(&mut self, ui: &mut egui::Ui) {
        let now = now_unix();
        let mut picked = None;
        let mut resume = false;
        match self.pause_now(now) {
            Err(e) => {
                ui.label(
                    RichText::new(format!(
                        "{} could not be read ({e}): nothing is held. Remove the file to pause again.",
                        pocketdesk_core::local::paths::PAUSE_FILE
                    ))
                    .color(crate::theme::bad(ui)),
                );
            }
            Ok(Some(pause)) => {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(crate::tray_menu::pause_text(&pause, now)).strong());
                    ui.label(
                        "Phones' notifications are not shown here: they stay on the phone, and those you can answer wait on the Notifications page. Texts wait on Messages. Links are not opened, nor kept: send them again after. A phone ringing this computer stays silent, and its battery warnings and the files that arrive are not announced. Sessions, files and the clipboard go on.",
                    );
                    resume = ui.button("Resume").clicked();
                });
            }
            Ok(None) => {
                ui.menu_button(crate::tray_menu::PAUSE_LABEL, |ui| {
                    for (label, minutes) in crate::tray_menu::PAUSES {
                        if ui.button(label).clicked() {
                            picked = Some(minutes);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(
                    "Hold phones' notifications, texts, links, rings and warnings for a while",
                );
            }
        }
        // P3: quiet hours now, said beside the pause.
        if let Some(config) = &self.config
            && let (Some(schedule), Some(at)) = (
                pocketdesk_core::local::quiet_hours::Schedule::from_config(config),
                pocketdesk_core::local::quiet_hours::local_now(),
            )
            && schedule.holds(at)
        {
            // The same time twice is the whole day (P3 review).
            let until = if schedule.from == schedule.to {
                "the day ends".to_string()
            } else {
                config.quiet_to.trim().to_string()
            };
            // In a group with its way out, as the pause is (Q2 review).
            let mut open = false;
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!(
                        "Quiet hours until {until}: phones' notifications and texts are not shown here, as in a pause."
                    ));
                    open = ui.button("Change quiet hours").clicked();
                });
            });
            if open {
                self.switch_to(Tab::Settings);
                self.chosen = Tab::Settings;
            }
        } else if self.config.as_ref().is_some_and(|c| c.quiet_presenting)
            && pocketdesk_core::local::quiet_hours::presenting()
        {
            // R2: what the server holds them by, said here as quiet hours are.
            let mut open = false;
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(if cfg!(windows) {
                        "Presenting or in full screen: phones' notifications and texts are not shown here, as in a pause."
                    } else {
                        "Do Not Disturb is on: phones' notifications and texts are not shown here, as in a pause."
                    });
                    open = ui.button("Change it").clicked();
                });
            });
            if open {
                self.switch_to(Tab::Settings);
                self.chosen = Tab::Settings;
            }
        }
        if let Some(minutes) = picked {
            self.pause(minutes);
        }
        if resume {
            self.resume();
        }
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

    /// Ask the phone `id` for a photo (F3, from the tray and the Overview:
    /// G2), and say how that went.
    fn take_photo(&mut self, id: &str) {
        let who = self.label(id);
        self.notice = crate::notice::Notice::outcome(
            match pocketdesk_core::local::phone_photos::request(
                &self.state_dir,
                id,
                pocketdesk_core::local::phone_photos::Ask::Take,
                now_unix(),
            ) {
                Ok(_) => Ok(format!(
                    "Asked {who}: tap its notification to take the photo (\"Photos the computer asks for\" on in its Settings); it lands with the received files."
                )),
                Err(e) => Err(format!("Could not ask {who} for a photo: {e}")),
            },
        );
    }

    /// Ask the phone `id` for a screenshot or to lock (O2), and say how that went.
    fn phone_action(
        &mut self,
        id: &str,
        action: pocketdesk_core::protocol::phone_action::PhoneAction,
    ) {
        use pocketdesk_core::protocol::phone_action::PhoneAction;
        let who = self.label(id);
        self.notice = crate::notice::Notice::outcome(
            match pocketdesk_core::local::phone_actions::request(
                &self.state_dir,
                id,
                action,
                now_unix(),
            ) {
                Ok(_) => Ok(match action {
                    PhoneAction::Screenshot => format!(
                        "Asked {who} for a screenshot. It arrives with \"Send new screenshots\" on in its Settings and \"Send files\" allowed here."
                    ),
                    PhoneAction::Lock => format!("Asked {who} to lock its screen."),
                    PhoneAction::Apps => format!("Asked {who} for its apps."),
                    PhoneAction::Open => format!("Asked {who} to open an app."),
                    PhoneAction::Contacts => format!("Asked {who} for its contacts."),
                }),
                Err(e) => Err(format!("Could not ask {who}: {e}")),
            },
        );
    }

    /// P2: ask the phone `id` to open the app `app`, and say how that went.
    fn open_phone_app(&mut self, id: &str, app: &str, name: &str) {
        let who = self.label(id);
        self.notice = crate::notice::Notice::outcome(
            match pocketdesk_core::local::phone_actions::request_open(
                &self.state_dir,
                id,
                app,
                now_unix(),
            ) {
                Ok(_) => Ok(format!(
                    "Asked {who} to open {name}. With the phone unlocked it opens at once while Pocketdesk control is on, else the phone offers \"Tap to open\"."
                )),
                Err(e) => Err(format!("Could not ask {who}: {e}")),
            },
        );
    }

    /// The Overview's phones (N6): where each is, its battery and last
    /// session, and Ring, Send file… and Send link… for it.
    fn phones_panel(&mut self, ui: &mut egui::Ui) {
        self.tel_question(ui);
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
        let mut take_photo = None;
        let mut phone_action = None;
        let mut open_apps = None;
        // P2: the phones listed, for the apps box drawn after them.
        let listed: Vec<String> = rows.iter().map(|r| r.device.id.clone()).collect();
        let mut type_to = None;
        let mut media_press = None;
        let control_on = self.config.as_ref().is_none_or(|c| c.cast_control);
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
                // The battery as a small bar, in the warning colour when low
                // (I3); its words beside it.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Battery").color(crate::theme::muted(ui)));
                    if let Some(battery) = device.battery.as_ref() {
                        let low = battery_low(battery);
                        let mut bar = egui::ProgressBar::new(f32::from(battery.charge) / 100.0)
                            .desired_width(90.0)
                            .desired_height(8.0);
                        if low {
                            bar = bar.fill(crate::theme::warn(ui));
                        }
                        ui.add(bar);
                    }
                    let words = battery_text(device.battery.as_ref(), now);
                    let low = device.battery.as_ref().is_some_and(battery_low);
                    ui.label(if low {
                        RichText::new(format!("{words}, low")).color(crate::theme::warn(ui))
                    } else {
                        RichText::new(words).color(crate::theme::muted(ui))
                    });
                });
                // What else it said of itself (L9), on its own line.
                // Only while it is reached and the report is recent (L9
                // review): an away phone's Wi-Fi and ringer are not now's.
                if let Some(state) = device
                    .battery
                    .as_ref()
                    .filter(|b| model::state_is_now(b, row.place, now))
                    .and_then(|b| model::phone_state_text(&b.state))
                {
                    ui.label(RichText::new(state).color(crate::theme::muted(ui)))
                        .on_hover_text("As the phone last said, with its battery");
                }
                let mut facts = Vec::new();
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
                // N2: what it plays, in a session or linked with its
                // background reporter on, with the player's buttons.
                if let Some(playing) = row.playing {
                    let media = &playing.media;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(if media.is_playing { "Playing:" } else { "Paused:" });
                        ui.label(RichText::new(model::media_line(media)).strong());
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
                                media_press = Some((device.id.clone(), command));
                            }
                        }
                    });
                }
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
                    // G2: as the tray offers it, where files reach the phone.
                    let photo_why = row.file.err().unwrap_or(
                        "With \"Photos the computer asks for\" on in Pocketdesk's Settings on the phone, it shows a notification that opens its camera; the photo comes here as a received file.",
                    );
                    if ui
                        .add_enabled(row.file.is_ok(), egui::Button::new("Take a photo"))
                        .on_hover_text(photo_why)
                        .on_disabled_hover_text(photo_why)
                        .clicked()
                    {
                        take_photo = Some(device.id.clone());
                    }
                    // O2: a screenshot of the phone, or its screen locked.
                    let action_reach = model::phone_action_reach(
                        device.linked,
                        device.permissions.allows(Capability::PhoneActions),
                    );
                    for (label, action, why) in [
                        (
                            "Screenshot of the phone",
                            pocketdesk_core::protocol::phone_action::PhoneAction::Screenshot,
                            "The phone takes a screenshot, which comes here as its new screenshots do. On the phone it needs \"Let the computer take screenshots and lock this phone\" and \"Send new screenshots\" on, and the Pocketdesk control service.",
                        ),
                        (
                            "Lock the phone",
                            pocketdesk_core::protocol::phone_action::PhoneAction::Lock,
                            "The phone locks its screen. On the phone it needs \"Let the computer take screenshots and lock this phone\" on, and the Pocketdesk control service.",
                        ),
                    ] {
                        let why = action_reach.err().unwrap_or(why);
                        if ui
                            .add_enabled(action_reach.is_ok(), egui::Button::new(label))
                            .on_hover_text(why)
                            .on_disabled_hover_text(why)
                            .clicked()
                        {
                            phone_action = Some((device.id.clone(), action));
                        }
                    }
                    // P2: one of the phone's apps opened from here.
                    let apps_why = action_reach.err().unwrap_or(
                        "Lists the phone's apps, to open one there. On the phone it needs \"Let the computer open my apps\" on, and the Pocketdesk control service to open one.",
                    );
                    if ui
                        .add_enabled(action_reach.is_ok(), egui::Button::new("Open an app on the phone…"))
                        .on_hover_text(apps_why)
                        .on_disabled_hover_text(apps_why)
                        .clicked()
                    {
                        open_apps = Some(device.id.clone());
                    }
                    // M4: this computer's keyboard typing on the phone.
                    // Linked, not a link's reach (M4 review: that one asks
                        // for Links, which typing does not need).
                    let type_reach = crate::type_view::type_reach(
                        device.linked,
                        control_on,
                        device.permissions.allows(Capability::Cast),
                    );
                    let type_why = type_reach.err().unwrap_or(
                        "A window that types what you type at the phone's field. On the phone it needs \"Typing from the computer's keyboard\" on in Pocketdesk's Settings, its Accessibility service and notification access.",
                    );
                    if ui
                        .add_enabled(type_reach.is_ok(), egui::Button::new("Type on the phone"))
                        .on_hover_text(type_why)
                        .on_disabled_hover_text(type_why)
                        .clicked()
                    {
                        type_to = Some((
                            device.id.clone(),
                            model::device_label(&self.snapshot.devices, &device.id),
                        ));
                    }
                });
                // Q2 review: the phone's own switch, as it last said it.
                if model::phone_action_reach(
                    device.linked,
                    device.permissions.allows(Capability::PhoneActions),
                )
                .is_ok()
                    && let Some(refused) = model::phone_actions_refused(device.switches.as_ref())
                {
                    ui.colored_label(crate::theme::warn(ui), refused);
                }
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
        if let Some((id, command)) = media_press {
            self.press_phone_media(&id, command);
        }
        if let Some((id, minutes)) = mute {
            self.quiet(&id, minutes);
        }
        if let Some(id) = send_file {
            self.send_to = Some(id);
            self.send_also.clear();
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
        if let Some(id) = take_photo {
            self.take_photo(&id);
        }
        if let Some((id, action)) = phone_action {
            self.phone_action(&id, action);
        }
        if let Some(id) = open_apps {
            self.phone_action(
                &id,
                pocketdesk_core::protocol::phone_action::PhoneAction::Apps,
            );
            self.apps_box = Some(crate::apps_view::AppsBox::new(id, now));
        }
        // The box for the phone it was opened for, while that phone is listed.
        let box_for = self.apps_box.as_ref().map(|b| b.device_id.clone());
        if let Some(id) = box_for {
            if listed.contains(&id) {
                let who = self.label(&id);
                let state_dir = self.state_dir.clone();
                let action = self
                    .apps_box
                    .as_mut()
                    .and_then(|b| b.ui(ui, &state_dir, &who));
                match action {
                    Some(crate::apps_view::Action::Open(app, name)) => {
                        self.open_phone_app(&id, &app, &name)
                    }
                    Some(crate::apps_view::Action::Close) => self.apps_box = None,
                    Some(crate::apps_view::Action::AskAgain) => {
                        self.phone_action(
                            &id,
                            pocketdesk_core::protocol::phone_action::PhoneAction::Apps,
                        );
                        self.apps_box = Some(crate::apps_view::AppsBox::new(id.clone(), now));
                    }
                    None => {}
                }
            } else {
                self.apps_box = None;
            }
        }
        if let Some((id, name)) = type_to {
            self.type_view.open(&self.state_dir, &id, &name);
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
            let who = model::device_label(&self.snapshot.devices, &id);
            match model::queue_link(&self.state_dir, &id, &who, &url, now_unix()) {
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
                        if ui.button("Cancel").clicked() || crate::keys::escaped(ui) {
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
        let mut save_profile = None;
        let mut apply_profile = None;
        // P9: the saved profiles, to apply; a file that cannot be read is
        // said, not taken for none (Q2 review).
        let (profiles, profiles_error) =
            match pocketdesk_core::local::permission_profiles::load(&self.state_dir) {
                Ok(profiles) => (profiles, None),
                Err(e) => (Vec::new(), Some(e.to_string())),
            };
        if let Some(e) = &profiles_error {
            ui.colored_label(
                crate::theme::warn(ui),
                format!("The saved profiles cannot be read: {e}"),
            );
        }
        let mut remove_profile = None;
        let mut profile_answer = None;
        let mut remove_answer = None;
        // L7 review: Settings that turn a feature off for every phone.
        let settings_off = self
            .config
            .as_ref()
            .map(model::settings_off)
            .unwrap_or_default();
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
                    // P9: these permissions under a name, or a profile's.
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Profile");
                        let name = self.profile_names.entry(device.id.clone()).or_default();
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text("a name, such as Kids")
                                .desired_width(140.0),
                        );
                        let named = !name.trim().is_empty();
                        if ui
                            .add_enabled(named, egui::Button::new("Save as profile"))
                            .on_hover_text(
                                "Keep what it may do under this name, to apply to another phone or to pair one with.",
                            )
                            .on_disabled_hover_text("Type a name for it first.")
                            .clicked()
                        {
                            // What a computer's page hides stays the default,
                            // so its profile turns nothing off on a phone
                            // (P9 review).
                            let mut kept = device.permissions.clone();
                            for capability in Capability::ALL {
                                if !profile_carries(capability, device.computer) {
                                    kept.set(capability, capability.default_allowed());
                                }
                            }
                            save_profile = Some((kept, name.clone()));
                        }
                        if !profiles.is_empty() {
                            ui.menu_button("Apply a profile", |ui| {
                                for profile in &profiles {
                                    if ui
                                        .button(&profile.name)
                                        .on_hover_text(format!(
                                            "Allows: {}.",
                                            crate::pair_view::profile_allows(&profile.permissions)
                                        ))
                                        .clicked()
                                    {
                                        apply_profile = Some((device.id.clone(), profile.clone()));
                                        ui.close();
                                    }
                                }
                            });
                            ui.menu_button("Remove a profile\u{2026}", |ui| {
                                for profile in &profiles {
                                    if ui.button(&profile.name).clicked() {
                                        remove_profile =
                                            Some((device.id.clone(), profile.name.clone()));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    });
                    // Q2 review: Apply turns switches off and may end a
                    // session, so it asks, saying what goes; Remove asks too.
                    if let Some((id, profile)) = &self.confirm_profile
                        && *id == device.id
                    {
                        let off = model::profile_turns_off(
                            &device.permissions,
                            &profile.permissions,
                            |c| profile_touches(c, device.computer),
                        );
                        let ends = off.iter().any(|c| model::fixed_at_session_start(*c));
                        let turns_off = if off.is_empty() {
                            "It turns nothing off.".to_string()
                        } else {
                            format!(
                                "It turns off: {}.",
                                off.iter()
                                    .map(|c| PERMISSIONS
                                        .iter()
                                        .find(|(p, _, _)| p == c)
                                        .map_or_else(|| format!("{c:?}"), |(_, l, _)| l.to_string()))
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            )
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "Give {label} the {} profile? {turns_off}{}",
                                profile.name,
                                if ends {
                                    " Its running session ends, to reconnect with them."
                                } else {
                                    ""
                                }
                            ));
                            if ui.button("Apply").clicked() {
                                profile_answer = Some(true);
                            }
                            if ui.button("Cancel").clicked() || crate::keys::escaped(ui) {
                                profile_answer = Some(false);
                            }
                        });
                    }
                    if let Some((id, name)) = &self.confirm_remove_profile
                        && *id == device.id
                    {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "Remove the {name} profile? Phones given it keep their permissions."
                            ));
                            if ui.button("Remove").clicked() {
                                remove_answer = Some(true);
                            }
                            if ui.button("Cancel").clicked() || crate::keys::escaped(ui) {
                                remove_answer = Some(false);
                            }
                        });
                    }
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
                        let hint = hint_for(capability, device.computer, hint);
                        if ui
                            .add_enabled(unavailable.is_none(), egui::Checkbox::new(&mut allowed, label))
                            .on_hover_text(hint)
                            .on_disabled_hover_text(unavailable.unwrap_or(hint))
                            .changed()
                        {
                            change = Some((device.id.clone(), capability, allowed));
                        }
                        // BK7: a computer's Control is also its window's.
                        if capability == Capability::Input && device.computer {
                            ui.label(
                                RichText::new(
                                    "Also lets that computer control this one from a window showing this screen.",
                                )
                                .color(crate::theme::muted(ui)),
                            );
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
            // L7: each feature as this computer, the phone and Android stand.
            if !device.computer {
                egui::CollapsingHeader::new(format!("Features on both sides for {label}"))
                    .id_salt(("switches", &device.id))
                    .show(ui, |ui| switches_table(ui, device, &settings_off));
            }
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
        if let Some((permissions, name)) = save_profile {
            self.notice = crate::notice::Notice::outcome(
                pocketdesk_core::local::permission_profiles::save(
                    &self.state_dir,
                    &name,
                    &permissions,
                )
                .map(|saved| {
                    // A profile of that name is replaced, and says so (P9 review).
                    let verb = if saved.replaced { "Replaced" } else { "Saved" };
                    format!(
                        "{verb} the {} profile: apply it to a phone below, or pair one with it.",
                        saved.name
                    )
                })
                .map_err(|e| format!("Could not save the profile: {e}")),
            );
        }
        if let Some(chosen) = apply_profile {
            self.confirm_profile = Some(chosen);
        }
        if let Some(chosen) = remove_profile {
            self.confirm_remove_profile = Some(chosen);
        }
        if let Some(yes) = profile_answer
            && let Some((id, asked)) = self.confirm_profile.take()
            && yes
        {
            // Read again: one replaced or removed since it was asked is
            // not applied as it was (Q2 review).
            match pocketdesk_core::local::permission_profiles::get(&self.state_dir, &asked.name) {
                Some(profile) => self.apply_profile(&id, &profile),
                None => {
                    self.notice = crate::notice::Notice::error(format!(
                        "The {} profile is gone: nothing was changed.",
                        asked.name
                    ))
                }
            }
        }
        if let Some(yes) = remove_answer
            && let Some((_, name)) = self.confirm_remove_profile.take()
            && yes
        {
            self.notice = crate::notice::Notice::outcome(
                pocketdesk_core::local::permission_profiles::remove(&self.state_dir, &name)
                    .map(|()| format!("Removed the {name} profile."))
                    .map_err(|e| format!("Could not remove the {name} profile: {e}")),
            );
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

    /// The Keyboard and mouse page: its switches go where Settings' and
    /// Devices' do.
    fn keyboard_mouse_tab(&mut self, ui: &mut egui::Ui) {
        let sharing_on = self.config.as_ref().is_some_and(|c| c.shared_input);
        let action = self.arrange.show(
            ui,
            &self.state_dir,
            &self.snapshot.devices,
            sharing_on,
            self.snapshot.running,
            now_unix(),
        );
        match action {
            Some(crate::arrange_view::Action::Sharing(on)) => self.set_sharing(on),
            Some(crate::arrange_view::Action::Allow { device_id, allow }) => {
                self.set_permission(&device_id, Capability::SharedInput, allow)
            }
            None => {}
        }
    }

    /// Turn the shared keyboard and mouse on or off (`shared_input`),
    /// written as Settings writes it: that key alone, the rest of the file
    /// as it is. The server reads it within a second.
    fn set_sharing(&mut self, on: bool) {
        let saved = Config::path().map_err(|e| e.to_string()).and_then(|path| {
            let mut config = Config::load_from(&path).map_err(|e| e.to_string())?;
            config.shared_input = on;
            config
                .save_keys_to(&path, &["shared_input"])
                .map_err(|e| e.to_string())
        });
        match saved {
            Ok(()) => {
                if let Some(Ok(form)) = self.settings.as_mut() {
                    form.saved_elsewhere_shared_input(on);
                }
                self.notice = crate::notice::Notice::done(if on {
                    "Sharing the keyboard and mouse with your computers."
                } else {
                    "The keyboard and mouse stay on each computer."
                });
            }
            Err(e) => self.notice = crate::notice::Notice::error(format!("Not saved: {e}")),
        }
        self.refresh();
    }

    fn sessions_tab(&mut self, ui: &mut egui::Ui) {
        if !self.snapshot.running {
            ui.label(
                "The server is not running, so no phone is connected. Start it from Overview.",
            );
            if ui.button("Overview").clicked() {
                self.switch_to(Tab::Status);
            }
        } else if self.snapshot.sessions.is_empty() {
            ui.label(
                "No phone is viewing this computer right now. On the phone, tap Connect on this \
                 computer's card.",
            );
            if self.snapshot.devices.is_empty() && ui.button("Pair a phone").clicked() {
                self.switch_to(Tab::Pair);
            }
        } else {
            self.live_sessions(ui);
        }
        self.recent_sessions_list(ui);
    }

    /// A block per live session (J3): the phone, chips for how it is
    /// connected and what it is, its control, End, and its music.
    fn live_sessions(&mut self, ui: &mut egui::Ui) {
        let now = now_unix();
        let mut end = None;
        let mut give = None;
        let mut show_cast = false;
        let mut pressed = None;
        // A cast whose window the owner closed (T10k).
        let cast_closed = self
            .cast
            .as_ref()
            .and_then(|c| c.showing())
            .is_some_and(|(_, closed)| closed);
        let count = self.snapshot.sessions.len();
        for session in &self.snapshot.sessions {
            let control = model::control_cell(session, count, self.snapshot.shared_control);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    match session.device_id.as_deref() {
                        Some(id) => {
                            ui.label(
                                RichText::new(model::device_label(&self.snapshot.devices, id))
                                    .strong(),
                            )
                            .on_hover_text(id);
                        }
                        None => {
                            ui.label(RichText::new("A phone that has not said who it is").strong());
                        }
                    }
                    let neutral = ui.visuals().widgets.inactive.bg_fill;
                    let (road, why) = if session.transport == "lan" {
                        ("on this network", "Straight across your network")
                    } else {
                        ("over the internet", "Through the internet road")
                    };
                    chip(ui, road, neutral).on_hover_text(format!("{why}, from {}", session.peer));
                    chip(ui, model::session_kind(session), neutral);
                    if matches!(control, model::ControlCell::Driving) {
                        let good = crate::theme::good(ui);
                        chip(ui, "driving", good);
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "For {}",
                        human_duration(now.saturating_sub(session.started_unix))
                    ))
                    .color(crate::theme::muted(ui)),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.button("End").clicked() {
                        end = Some(session.clone());
                    }
                    if let model::ControlCell::Give(id) = control
                        && ui
                            .button("Give control")
                            .on_hover_text(
                                "This phone drives; the others only watch until it goes quiet",
                            )
                            .clicked()
                    {
                        give = Some(id);
                    }
                    if session.cast && cast_closed && ui.button("Show screen").clicked() {
                        show_cast = true;
                    }
                });
                // Its music, in its own block (J3: it had sat below them all).
                if let Some(id) = session.device_id.as_deref()
                    && let Some(playing) = self.snapshot.phone_media.get(id)
                {
                    let media = &playing.media;
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Playing:");
                        ui.label(RichText::new(model::media_line(media)).strong());
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
            });
            ui.add_space(4.0);
        }
        if let Some((id, command)) = pressed {
            self.press_phone_media(&id, command);
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

    /// The last sessions from the session log, newest first (J3).
    fn recent_sessions_list(&mut self, ui: &mut egui::Ui) {
        if self.recent_sessions.is_empty() {
            return;
        }
        ui.add_space(12.0);
        ui.heading("Recent sessions");
        let now = now_unix();
        for recent in &self.recent_sessions {
            let who = model::recent_who(recent, &self.snapshot.devices);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(who).strong());
                let mut line = vec![crate::words::ago(now, recent.ended_unix)];
                if !recent.refused
                    && let Some(secs) = recent.duration_secs.filter(|&secs| secs > 0)
                {
                    line.push(format!("for {}", human_duration(secs)));
                }
                line.push(match recent.times {
                    1 => recent.how.to_string(),
                    n => format!("{} {n} times", recent.how),
                });
                // Capture to arrival, not to the screen (J3 review: "latency"
                // read as the whole way to the phone's screen).
                if let Some(ms) = recent.e2e_ms {
                    line.push(format!("{ms} ms capture to arrival"));
                }
                let text = RichText::new(line.join(" · "));
                let label = ui.label(if recent.refused {
                    text.color(crate::theme::warn(ui))
                } else {
                    text.color(crate::theme::muted(ui))
                });
                if recent.e2e_ms.is_some() {
                    label.on_hover_text(
                        "The session's median, measured by the phone: from this computer                          capturing the screen to the picture arriving on the phone, before                          the phone decodes and shows it.",
                    );
                }
            });
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
        // J2: the rows below are held to the words in the box.
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.settings_query)
                .hint_text("Find a setting")
                .desired_width(260.0),
        );
        if ui.input_mut(|i| i.consume_shortcut(&crate::keys::FIND)) {
            search.request_focus();
            // The page may be scrolled down past the box (J2 review).
            search.scroll_to_me(Some(egui::Align::TOP));
        }
        // Esc empties a search, and is taken, so it does not also cancel a
        // question drawn later in the frame (J2 review).
        if search.lost_focus() && !self.settings_query.is_empty() && crate::keys::escaped(ui) {
            self.settings_query.clear();
        }
        let query = self.settings_query.trim().to_string();
        if !query.is_empty()
            && !crate::settings_view::Section::ALL
                .into_iter()
                .any(|s| crate::settings_view::section_shown(&query, s))
        {
            ui.label(
                RichText::new(format!("No setting matches \"{query}\"."))
                    .color(crate::theme::muted(ui)),
            );
        }
        ui.add_space(8.0);
        crate::settings_view::set_search(ui.ctx(), &query);
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
        // M9: the phones a command may go to, paired for good (M9 review: a
        // guest's commands are never offered).
        form.phones = self
            .snapshot
            .devices
            .iter()
            .filter(|d| !d.computer && d.grant_remaining.is_none())
            .map(|d| {
                (
                    d.id.clone(),
                    model::device_label(&self.snapshot.devices, &d.id),
                )
            })
            .collect();
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
            // A held section is drawn whole whatever is searched (J2
            // review: a problem's field, or KDE's Allow, had been hidden).
            let section_query = if held.is_some() { "" } else { query.as_str() };
            if !crate::settings_view::section_shown(section_query, section) {
                continue;
            }
            crate::settings_view::set_search(ui.ctx(), section_query);
            let mut header =
                egui::CollapsingHeader::new(section_heading(ui, section, held.as_deref()))
                    .id_salt(section.title())
                    .default_open(section.opens());
            // A search opens every section it finds something in.
            if held.is_some() || !query.is_empty() {
                header = header.open(Some(true));
            }
            let extra = |name| crate::settings_view::extra_shown(section_query, section, name);
            header.show(ui, |ui| {
                form.show_section(ui, section);
                match section {
                    crate::settings_view::Section::Phones => {
                        if extra(crate::settings_view::EXTRA_SEND_MENU) {
                            self.send_menu_setting(ui);
                        }
                        if extra(crate::settings_view::EXTRA_BROWSER) {
                            self.browser_setting(ui);
                        }
                        if extra(crate::settings_view::EXTRA_TEL) {
                            self.tel_setting(ui);
                        }
                    }
                    crate::settings_view::Section::Computers
                        if extra(crate::settings_view::EXTRA_COMPUTERS) =>
                    {
                        self.computers
                            .show(ui, &self.state_dir, self.snapshot.running, now_unix());
                        // BK7: "Show its screen" opens its window.
                        if let Some((device_id, name)) = self.computers.take_view() {
                            self.show_computer(ui.ctx(), &device_id, &name);
                        }
                    }
                    crate::settings_view::Section::Notifications
                        if extra(crate::settings_view::EXTRA_MUTED) =>
                    {
                        self.muted_apps_setting(ui);
                    }
                    crate::settings_view::Section::KdeConnect
                        if extra(crate::settings_view::EXTRA_KDE) =>
                    {
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
        crate::settings_view::set_search(ui.ctx(), "");
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
        // Ctrl+S saves too, while there is something to save (I3).
        save |= ui.input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            ))
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

    /// A `tel:` link with several phones to take it (M8): which calls it.
    fn tel_question(&mut self, ui: &mut egui::Ui) {
        use pocketdesk_core::local::tel_ask;
        let now = now_unix();
        // Read once a second, not every frame.
        if self
            .tel_ask_read
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(1))
        {
            self.tel_ask_read = Some(std::time::Instant::now());
            self.tel_ask = tel_ask::read(&self.state_dir, now);
        }
        let Some(ask) = self.tel_ask.clone() else {
            return;
        };
        let phones: Vec<(String, String)> = self
            .snapshot
            .devices
            .iter()
            .filter(|d| !d.computer && d.permissions.allows(Capability::Dial))
            .map(|d| {
                (
                    d.id.clone(),
                    model::device_label(&self.snapshot.devices, &d.id),
                )
            })
            .collect();
        let mut chosen = None;
        let mut cancelled = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(format!("Call {} on:", ask.number)).strong());
            ui.horizontal_wrapped(|ui| {
                for (id, name) in &phones {
                    if ui.button(name).clicked() {
                        chosen = Some(id.clone());
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
            if phones.is_empty() {
                ui.label("No paired phone allows \"Call numbers from here\" (Devices).");
            }
            ui.label(
                RichText::new("The phone's dialer opens with the number; you press Call there.")
                    .small()
                    .color(crate::theme::muted(ui)),
            );
        });
        ui.add_space(8.0);
        if let Some(id) = &chosen {
            self.notice = crate::notice::Notice::outcome(
                pocketdesk_core::local::dials::request(&self.state_dir, id, &ask.number, now)
                    .map(|_| {
                        "Asked the phone to open its dialer with the number; it does with \u{201c}Calls from the computer\u{201d} on.".to_string()
                    })
                    .map_err(|e| format!("Not asked: {e}")),
            );
        }
        if chosen.is_some() || cancelled {
            tel_ask::clear(&self.state_dir, &ask);
            self.tel_ask = None;
        }
    }

    /// Phone numbers' links open on the phone (M8): registered by the
    /// server program, which says so in `<config>/tel_handler.json`.
    fn tel_setting(&mut self, ui: &mut egui::Ui) {
        let Ok(config) = pocketdesk_core::local::paths::config_dir() else {
            return;
        };
        let installed = *self
            .tel_registered
            .get_or_insert_with(|| config.join("tel_handler.json").exists());
        let mut on = installed;
        ui.add_enabled(
            self.setting_job.is_none(),
            egui::Checkbox::new(
                &mut on,
                "Open phone numbers on the phone (tel: links open its dialer)",
            ),
        );
        if installed && cfg!(windows) {
            ui.label(
                RichText::new("Pick Pocketdesk when Windows asks which app opens a phone number, or in Settings, Apps, Default apps.")
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
        let doing = if on {
            "Taking phone numbers' links"
        } else {
            "Leaving phone numbers' links"
        };
        let args: &'static [&'static str] = if on {
            &["tel-handler"]
        } else {
            &["tel-handler", "--remove"]
        };
        self.run_setting(doing, move || {
            let out = crate::restart::quiet(exe)
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
            self.drop_question(ui);
            self.send_box(ui);
            ui.add_space(12.0);
            self.under_way_list(ui);
            self.sent_list(ui);
            self.received_list(ui);
        });
    }

    /// Files dropped with several phones or computers to take them: which
    /// (J5).
    fn drop_question(&mut self, ui: &mut egui::Ui) {
        if self.dropped.is_empty() {
            return;
        }
        let targets = match model::drop_to(&self.snapshot) {
            model::DropTo::Ask(targets) => targets,
            model::DropTo::Send(id) => vec![id],
            model::DropTo::Nobody => {
                let what = dropped_words(&std::mem::take(&mut self.dropped));
                self.notice = crate::notice::Notice::error(format!(
                    "Not sent: no phone or computer can take {what} now."
                ));
                // Said with no click to bring a frame (J5 review).
                ui.ctx().request_repaint();
                return;
            }
        };
        let mut chosen = None;
        let mut all = false;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.strong(format!("Send {} to:", dropped_words(&self.dropped)));
            ui.horizontal_wrapped(|ui| {
                for id in &targets {
                    if ui.button(model::target_label(&self.snapshot, id)).clicked() {
                        chosen = Some(id.clone());
                    }
                }
                // P6: every one of them at once.
                if targets.len() > 1 && ui.button("All of them").clicked() {
                    all = true;
                }
                // Esc in a text box only leaves the box (J5 review).
                let typing = ui.memory(|m| m.focused().is_some());
                cancel = ui.button("Cancel").clicked() || (!typing && crate::keys::escaped(ui));
            });
        });
        ui.add_space(12.0);
        if let Some(id) = chosen {
            self.send_to = Some(id);
            self.send_also.clear();
            let dropped = std::mem::take(&mut self.dropped);
            self.send_files(dropped);
        } else if all {
            self.send_to = targets.first().cloned();
            self.send_also = targets.iter().skip(1).cloned().collect();
            let dropped = std::mem::take(&mut self.dropped);
            self.send_files(dropped);
            // For this drop only (P6 review): the next send goes as chosen.
            self.send_also.clear();
        } else if cancel {
            let what = dropped_words(&std::mem::take(&mut self.dropped));
            self.notice = crate::notice::Notice::done(format!("Not sent: {what}."));
        }
    }

    /// Mute `app`'s phone notifications here (K2b), and say so.
    fn mute_app(&mut self, app: &str) {
        self.notice = crate::notice::Notice::outcome(
            pocketdesk_core::local::muted_apps::mute(&self.state_dir, app, now_unix())
                .map(|()| {
                    format!(
                        "{app}'s notifications from phones are not shown here now. Unmute in Settings."
                    )
                })
                .map_err(|e| format!("Could not mute {app}: {e}")),
        );
        self.activity.refresh(&self.state_dir);
    }

    /// Settings' "Muted apps" (K2b): each with Unmute.
    fn muted_apps_setting(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new(crate::settings_view::EXTRA_MUTED).strong());
        // A file that does not parse says so (K2b review: it read as none,
        // and each mute then failed with no word why).
        let muted = match pocketdesk_core::local::muted_apps::load(&self.state_dir) {
            Ok(muted) => muted,
            Err(e) => {
                ui.label(
                    RichText::new(format!(
                        "{} could not be read ({e}): every app shows. Remove the file to start the list again.",
                        pocketdesk_core::local::paths::MUTED_APPS_FILE
                    ))
                    .color(crate::theme::bad(ui)),
                );
                return;
            }
        };
        if muted.is_empty() {
            ui.label(
                RichText::new(
                    "None. \"Not from this app\" on a phone's notification, or its row on Activity, stops its app's notifications showing here.",
                )
                .small()
                .weak(),
            );
            return;
        }
        let mut unmute = None;
        for app in &muted {
            ui.horizontal_wrapped(|ui| {
                ui.label(&app.app);
                if ui.small_button("Unmute").clicked() {
                    unmute = Some(app.app.clone());
                }
            });
        }
        if let Some(app) = unmute {
            self.notice = crate::notice::Notice::outcome(
                pocketdesk_core::local::muted_apps::unmute(&self.state_dir, &app)
                    .map(|()| format!("{app}'s notifications show here again."))
                    .map_err(|e| format!("Could not unmute {app}: {e}")),
            );
        }
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

    /// The phones and computers a send goes to, as chosen (P6): the "To"
    /// box's, then the others ticked.
    fn send_chosen(&self) -> Vec<String> {
        let mut chosen: Vec<String> = self.send_to.iter().cloned().collect();
        for id in &self.send_also {
            if !chosen.contains(id) {
                chosen.push(id.clone());
            }
        }
        chosen
    }

    /// Queue `path` for the chosen phones and computers, and say what
    /// happened. A chosen one that has left is reported, never swapped for
    /// another.
    fn send_file(&mut self, path: &std::path::Path) {
        self.send_files(vec![path.to_path_buf()]);
    }

    /// Queue `paths` for each chosen phone or computer with one notice for
    /// them all (J5 review: each file's notice replaced the last, an early
    /// failure hidden behind a later success).
    fn send_files(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let (devices, gone) = match model::targets_for_send(&self.snapshot, &self.send_chosen()) {
            Ok(targets) => targets,
            Err(e) => {
                self.send_to = None;
                self.send_also.clear();
                self.notice = crate::notice::Notice::error(e);
                return;
            }
        };
        let to = model::targets_words(&self.snapshot, &devices);
        let left = if gone.is_empty() {
            String::new()
        } else {
            format!(
                " Not to {}: no longer connected.",
                model::targets_words(&self.snapshot, &gone)
            )
        };
        let mut what = dropped_words(&paths);
        let mut failed = Vec::new();
        for device in &devices {
            for path in &paths {
                match model::queue_send(&self.state_dir, device, path, now_unix()) {
                    Ok(name) if paths.len() == 1 => what = name,
                    Ok(_) => {}
                    Err(e) if devices.len() == 1 && paths.len() == 1 => {
                        self.notice = crate::notice::Notice::error(format!("{e}{left}"));
                        self.refreshed = None;
                        return;
                    }
                    Err(e) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        let whom = if devices.len() > 1 {
                            format!(" to {}", model::target_label(&self.snapshot, device))
                        } else {
                            String::new()
                        };
                        failed.push(format!("{name}{whom} ({e})"));
                    }
                }
            }
        }
        let total = devices.len() * paths.len();
        let them = if total == 1 { "it has" } else { "they have" };
        self.notice = if failed.is_empty() && gone.is_empty() {
            crate::notice::Notice::done(format!(
                "Queued {what} for {to}: the Sent list says when {them} gone."
            ))
        } else if failed.is_empty() {
            crate::notice::Notice::error(format!("Queued {what} for {to}.{left}"))
        } else if failed.len() == total {
            crate::notice::Notice::error(format!("Not queued: {}.{left}", failed.join(", ")))
        } else {
            crate::notice::Notice::error(format!(
                "Queued {} of {total} for {to}. Not queued: {}.{left}",
                total - failed.len(),
                failed.join(", ")
            ))
        };
        self.refreshed = None;
    }

    fn send_box(&mut self, ui: &mut egui::Ui) {
        ui.strong("Send a file");
        let Some(device) = self.send_target() else {
            ui.label(
                RichText::new(
                    "No phone or computer can take a file right now. A phone takes one in a screen or sound session, or \
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
        // P6: the others it goes to as well.
        let others: Vec<(String, String)> = targets
            .iter()
            .filter(|id| **id != chosen)
            .map(|id| (id.clone(), label(id)))
            .collect();
        // A tick outlives a link's moment away (P6 review): one gone at the
        // send is said then. Only the main one is never also.
        self.send_also.retain(|id| *id != chosen);
        self.send_to = Some(chosen);
        if !others.is_empty() {
            let also = &mut self.send_also;
            ui.horizontal(|ui| {
                let ticked = also
                    .iter()
                    .filter(|id| others.iter().any(|(o, _)| o == *id))
                    .count();
                let title = match ticked {
                    0 => "Also to…".to_string(),
                    n => format!("And {n} more…"),
                };
                let names: Vec<&str> = others
                    .iter()
                    .filter(|(id, _)| also.contains(id))
                    .map(|(_, name)| name.as_str())
                    .collect();
                let menu = ui.menu_button(title, |ui| {
                    let mut every = others.iter().all(|(id, _)| also.contains(id));
                    if ui.checkbox(&mut every, "All of them").changed() {
                        *also = if every {
                            others.iter().map(|(id, _)| id.clone()).collect()
                        } else {
                            Vec::new()
                        };
                    }
                    for (id, name) in &others {
                        let mut on = also.contains(id);
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                also.push(id.clone());
                            } else {
                                also.retain(|x| x != id);
                            }
                        }
                    }
                });
                // Q2 review: "And 2 more…" says who.
                if !names.is_empty() {
                    menu.response
                        .on_hover_text(format!("Also to {}", names.join(", ")));
                }
            });
        }
        let mut send = None;
        let mut paste = false;
        // Wrapped: the box and three buttons are wider than a narrow page.
        ui.horizontal_wrapped(|ui| {
            let path_box = ui.add(
                egui::TextEdit::singleline(&mut self.send_path)
                    .hint_text("Path to a file or folder")
                    .desired_width(360.0),
            );
            // Enter in the box sends, as the button does (I3).
            let entered = crate::keys::entered(ui, &path_box);
            if (ui.button("Send").clicked() || entered) && !self.send_path.trim().is_empty() {
                send = Some(PathBuf::from(self.send_path.trim()));
            }
            // P7: files copied in Explorer or a file manager. Not Ctrl+V:
            // egui takes that key for text only, and with files copied
            // there is none.
            if ui
                .add_enabled(
                    self.pasting.is_none(),
                    egui::Button::new("Paste copied files"),
                )
                .on_hover_text("Send the files copied in Explorer or a file manager")
                .on_disabled_hover_text("Reading the copied files\u{2026}")
                .clicked()
            {
                paste = true;
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
        // Read on a thread (P7 review: the clipboard's retries, or a dead
        // network drive's files, held the window).
        if paste && self.pasting.is_none() {
            self.notice = crate::notice::Notice::working("Reading the copied files\u{2026}");
            self.pasting = Some(std::thread::spawn(
                pocketdesk_core::local::clipboard_files::read,
            ));
        }
        let mut link = None;
        ui.horizontal_wrapped(|ui| {
            let link_box = ui.add(
                egui::TextEdit::singleline(&mut self.send_link)
                    .hint_text("A web link (http or https)")
                    .desired_width(360.0),
            );
            let entered = crate::keys::entered(ui, &link_box);
            if (ui.button("Send link").clicked() || entered) && !self.send_link.trim().is_empty() {
                link = Some(self.send_link.clone());
            }
        });
        if let Some(url) = link {
            // A link opens on a phone; another computer or a KDE Connect
            // device takes files (2h, T6).
            if self
                .snapshot
                .linked_computers
                .iter()
                .chain(&self.snapshot.kde_devices)
                .any(|(id, _)| *id == device)
            {
                self.notice = crate::notice::Notice::error(
                    "Links go to phones: choose a phone to send it to.",
                );
                return;
            }
            self.notice = crate::notice::Notice::outcome(
                match model::queue_link(
                    &self.state_dir,
                    &device,
                    &model::device_label(&self.snapshot.devices, &device),
                    &url,
                    now_unix(),
                ) {
                    Ok(said) => {
                        self.send_link.clear();
                        // Q2 review: the ticks under "Also to" are for files.
                        Ok(if self.send_also.is_empty() {
                            said
                        } else {
                            format!("{said} A link goes to the phone under To only.")
                        })
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
        let targets = model::send_target_ids(&self.snapshot);
        let mut again = None;
        // A block a row, wrapping at a narrow width (I3), as Devices' are.
        for file in &self.snapshot.sent {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&file.name).strong())
                    .on_hover_text(&file.path);
                ui.label(
                    RichText::new(format!(
                        "to {} · {}",
                        model::target_label(&self.snapshot, &file.device_id),
                        crate::words::ago(now, file.sent_unix)
                    ))
                    .color(crate::theme::muted(ui)),
                );
                // What a folder send left out goes beside its state.
                let note = file
                    .note
                    .as_deref()
                    .map(|n| format!(" · {n}"))
                    .unwrap_or_default();
                match &file.state {
                    SentState::Offered => {
                        // U3: the row says to whom; a computer or a KDE
                        // device takes files too.
                        ui.label(format!("waiting to be taken{note}"));
                    }
                    SentState::Sent => {
                        ui.label(format!("saved there{note}"));
                    }
                    SentState::Failed(reason) => {
                        ui.label(
                            RichText::new(format!("not sent: {reason}{note}"))
                                .color(crate::theme::bad(ui)),
                        );
                        // I3: tried again while its phone can take files.
                        let reachable = targets.contains(&file.device_id);
                        if ui
                            .add_enabled(reachable, egui::Button::new("Send again"))
                            .on_disabled_hover_text("Its phone cannot take files now.")
                            .clicked()
                        {
                            again = Some((file.device_id.clone(), file.path.clone()));
                        }
                    }
                }
            });
            ui.add_space(4.0);
        }
        if let Some((device, path)) = again {
            self.notice = crate::notice::Notice::outcome(
                match model::queue_send(
                    &self.state_dir,
                    &device,
                    std::path::Path::new(&path),
                    now_unix(),
                ) {
                    Ok(name) => Ok(format!(
                        "Queued {name} again for {}.",
                        model::target_label(&self.snapshot, &device)
                    )),
                    Err(e) => Err(e),
                },
            );
            self.refreshed = None;
        }
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
        let mut open = None;
        // A block a row, wrapping at a narrow width (I3).
        for row in &self.snapshot.received {
            let file = &row.file;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&file.name).strong())
                    .on_hover_text(&file.path);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        human_size(file.size),
                        crate::words::ago(now, file.received_unix)
                    ))
                    .color(crate::theme::muted(ui)),
                );
                if row.exists {
                    // I3: what opens in a viewer opens from here.
                    // Judged by the file opened as well as the name shown
                    // (I3 review): the two come from different places.
                    let opened_name = std::path::Path::new(&file.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if crate::reveal::openable(&file.name)
                        && crate::reveal::openable(&opened_name)
                        && ui.button("Open").clicked()
                    {
                        open = Some(file.path.clone());
                    }
                    if ui.button("Show in folder").clicked() {
                        show = Some(file.path.clone());
                    }
                } else {
                    ui.label(RichText::new("moved or deleted").color(crate::theme::muted(ui)));
                }
            });
            ui.add_space(4.0);
        }
        if let Some(path) = show
            && let Err(e) = crate::reveal::show_in_folder(std::path::Path::new(&path))
        {
            self.notice = crate::notice::Notice::error(format!("Could not open the folder: {e}"));
        }
        if let Some(path) = open
            && let Err(e) = crate::reveal::open_file(std::path::Path::new(&path))
        {
            self.notice = crate::notice::Notice::error(format!("Could not open it: {e}"));
        }
    }

    /// Files dropped on the window, or pasted from the clipboard (P7): to
    /// the only phone or computer that takes them, or asked which.
    fn take_dropped(&mut self, dropped: Vec<PathBuf>) {
        self.switch_to(Tab::Files);
        // Judged on what can take files now, not up to a second ago: a
        // phone that just linked must be asked about (J5 review).
        self.refresh();
        match model::drop_to(&self.snapshot) {
            // Several could take them, or a question is open already:
            // they wait with it, never going around it (J5 review).
            _ if !self.dropped.is_empty() => model::join_drops(&mut self.dropped, dropped),
            model::DropTo::Ask(_) => model::join_drops(&mut self.dropped, dropped),
            // The only one, or none (send_file says so).
            to => {
                if let model::DropTo::Send(id) = to {
                    self.send_to = Some(id);
                    self.send_also.clear();
                }
                self.send_files(dropped);
            }
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
            self.take_dropped(dropped);
        }
        // P7: files pasted from the clipboard, once read.
        if self.pasting.as_ref().is_some_and(|h| h.is_finished()) {
            // Its "Reading…" ends with the read (Q2 review: a question for
            // several phones left it spinning for five minutes).
            self.notice = None;
            match self.pasting.take().and_then(|h| h.join().ok()).flatten() {
                Some(copied) => {
                    let left_out = copied.left_out;
                    self.take_dropped(copied.files);
                    if left_out > 0 {
                        self.notice = crate::notice::Notice::error(format!(
                            "Only the first {} copied files were taken; {left_out} more were left out.",
                            pocketdesk_core::local::clipboard_files::MAX_FILES
                        ));
                    }
                }
                None => self.notice = crate::notice::Notice::error(
                    "The clipboard holds no files: copy them in Explorer or a file manager first."
                        .to_string(),
                ),
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
                model::send_target_ids(&self.snapshot),
            );
        // BK7: another computer's screen, while its window is open.
        if let Some(view) = &self.computer_view {
            view.show(ctx);
            // PC2: its Stats switch, saved there, is what Settings shows.
            if let Some(on) = view.take_stats_saved() {
                if let Some(config) = self.config.as_mut() {
                    config.computer_view_stats = on;
                }
                if let Some(Ok(form)) = self.settings.as_mut() {
                    form.saved_elsewhere_computer_view_stats(on);
                }
            }
        }
        // M4: the typing window, while open.
        if let Some(id) = self.type_view.phone() {
            let control_on = self.config.as_ref().is_none_or(|c| c.cast_control);
            let reach = self.snapshot.devices.iter().find(|d| d.id == id).map_or(
                Err("The phone is no longer paired."),
                |d| {
                    crate::type_view::type_reach(
                        d.linked,
                        control_on,
                        d.permissions.allows(Capability::Cast),
                    )
                },
            );
            self.type_view.show(ctx, &self.state_dir, reach);
        }
        ctx.request_repaint_after(REFRESH);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.take_files_to_send(ui.ctx());
        // Ctrl+Page Down and Up move through the pages as listed (I3).
        let (next, previous) = ui.input_mut(|i| {
            (
                i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::PageDown,
                )),
                i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::PageUp,
                )),
            )
        });
        if next || previous {
            let tab = crate::nav::step(self.tab, next);
            self.switch_to(tab);
            self.chosen = tab;
        }
        let waiting = crate::nav::Waiting {
            sessions: self.snapshot.sessions.len(),
            transfers: self.snapshot.under_way.len(),
            unsaved: matches!(&self.settings, Some(Ok(form)) if form.dirty()),
        };
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
                                // What waits there, after its name (I3).
                                let label = match crate::nav::badge(*tab, waiting) {
                                    Some(badge) => {
                                        let mut job = egui::text::LayoutJob::default();
                                        let font = egui::TextStyle::Button.resolve(ui.style());
                                        job.append(
                                            tab.name(),
                                            0.0,
                                            // The label's own colour, selected
                                            // or hovered (I3 review).
                                            egui::TextFormat::simple(
                                                font.clone(),
                                                egui::Color32::PLACEHOLDER,
                                            ),
                                        );
                                        job.append(
                                            &badge,
                                            8.0,
                                            // A count as the name; Settings'
                                            // unsaved dot as a warning, not "good".
                                            egui::TextFormat::simple(
                                                font,
                                                if *tab == Tab::Settings {
                                                    crate::theme::warn(ui)
                                                } else {
                                                    egui::Color32::PLACEHOLDER
                                                },
                                            ),
                                        );
                                        egui::WidgetText::from(job)
                                    }
                                    None => egui::WidgetText::from(tab.name()),
                                };
                                if ui.selectable_label(self.tab == *tab, label).clicked() {
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
        // The notice has a bar of its own at the foot of the window (I3): at
        // the top of the page it pushed the page down a line under the
        // pointer, and back as it went.
        if self
            .notice
            .as_ref()
            .is_some_and(|n| n.showing(std::time::Instant::now()))
        {
            egui::Panel::bottom("notice").show(ui, |ui| {
                ui.add_space(4.0);
                crate::notice::show(ui, &mut self.notice);
                ui.add_space(4.0);
            });
        } else {
            self.notice = None;
        }
        egui::CentralPanel::default().show(ui, |ui| {
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
            self.arrange.take_notice(),
            self.messages.take_notice(),
            self.logs.take_notice(),
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

/// Whether a battery is low as the server warns of it (T8b's 15 %, the
/// server's `phone_battery::LOW_AT`, kept in step by hand: the desktop
/// cannot take it from the server crate), not charging: its bar on the
/// Overview is drawn as a warning (I3).
pub fn battery_low(battery: &pocketdesk_core::local::phone_status::PhoneBattery) -> bool {
    battery.charge <= 15 && !battery.charging
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

/// A word on a filled chip (I3's Diagnostics, J3's Sessions).
fn chip(ui: &mut egui::Ui, text: &str, fill: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(crate::theme::on_chip(ui, fill)));
        })
        .response
}

/// Dropped paths in words, a folder as a folder (J5 review: "1 file" for
/// a dropped folder).
fn dropped_words(paths: &[PathBuf]) -> String {
    let folders = paths.iter().filter(|p| p.is_dir()).count();
    model::files_and_folders(paths.len() - folders, folders)
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
    /// L7: the switches table names each permission as its switch reads.
    #[test]
    fn the_switches_table_names_permissions_as_their_switches() {
        for feature in crate::model::FEATURES {
            let label = super::PERMISSIONS
                .iter()
                .find(|(c, _, _)| *c == feature.permission())
                .map(|(_, label, _)| *label);
            assert_eq!(label, Some(feature.permission_name), "{}", feature.key);
        }
    }

    #[test]
    fn a_paired_computer_is_not_shown_a_phone_s_switches() {
        for capability in [
            Capability::Ring,
            Capability::Mute,
            Capability::AwayLock,
            Capability::Cast,
            Capability::PhoneSound,
            Capability::Dial,
            Capability::Command,
            Capability::PhoneActions,
            Capability::Peek,
            Capability::Ask,
            Capability::Notes,
        ] {
            assert!(!shown_for(capability, true), "{capability:?}");
            assert!(shown_for(capability, false), "{capability:?}");
        }
        assert!(shown_for(Capability::Clipboard, true));
        assert!(shown_for(Capability::Files, true));
    }

    /// BK7: a computer's Control switch says it also controls this one from
    /// a window showing this screen; a phone's says the phone's way.
    #[test]
    fn a_computers_control_switch_says_its_window() {
        let (_, _, phone_hint) = PERMISSIONS
            .iter()
            .find(|(c, _, _)| *c == Capability::Input)
            .copied()
            .unwrap();
        assert!(shown_for(Capability::Input, true));
        assert_eq!(
            hint_for(Capability::Input, true, phone_hint),
            COMPUTER_CONTROL
        );
        assert!(COMPUTER_CONTROL.contains("window"));
        assert_eq!(hint_for(Capability::Input, false, phone_hint), phone_hint);
    }

    /// The shared keyboard and mouse: a paired computer's page has its
    /// switch (it may drive this one), and so does a phone's (BK5: this one
    /// may drive the phone), said the phone's way.
    #[test]
    fn the_keyboard_and_mouse_switch_is_on_a_computers_page_and_a_phones() {
        assert!(shown_for(Capability::SharedInput, true));
        assert!(shown_for(Capability::SharedInput, false));
        // A profile saved from a computer's page keeps it; one saved from
        // or applied to a phone never carries it, as it says the other way.
        assert!(profile_touches(Capability::SharedInput, true));
        assert!(!profile_touches(Capability::SharedInput, false));
        assert!(!profile_carries(Capability::SharedInput, false));
        assert!(profile_carries(Capability::Clipboard, false));
        assert!(hint_for(Capability::SharedInput, false, "x").contains("onto this phone"));
        assert_eq!(hint_for(Capability::SharedInput, true, "x"), "x");
        let row = PERMISSIONS
            .iter()
            .find(|(c, _, _)| *c == Capability::SharedInput)
            .expect("a switch for it");
        assert_eq!(row.1, "Keyboard and mouse");
        assert_eq!(
            row.2,
            "This computer may move its mouse and keyboard onto this one"
        );
    }

    #[test]
    fn the_away_lock_says_why_it_is_off_where_it_never_locks() {
        assert_eq!(
            away_lock_unavailable().is_none(),
            cfg!(any(windows, target_os = "linux"))
        );
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
            ..Default::default()
        };
        assert_eq!(model::media_line(&song), "Song · Artist (Spotify)");
        let video = PhoneMedia {
            artist: String::new(),
            ..song.clone()
        };
        assert_eq!(model::media_line(&video), "Song (Spotify)");
        let untitled = PhoneMedia {
            title: String::new(),
            artist: String::new(),
            ..song.clone()
        };
        assert_eq!(model::media_line(&untitled), "Spotify");
        let bare = PhoneMedia {
            title: String::new(),
            artist: String::new(),
            app: String::new(),
            ..song
        };
        assert_eq!(model::media_line(&bare), "something");
    }

    #[test]
    fn a_phones_battery_reads_with_its_age() {
        use pocketdesk_core::local::phone_status::PhoneBattery;
        let battery = |charge, charging| PhoneBattery {
            charge,
            charging,
            updated_unix: 1000,
            ..Default::default()
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

    /// I3: low as the server warns of it, 15 % or under and not charging.
    #[test]
    fn a_battery_is_low_at_the_servers_mark() {
        use pocketdesk_core::local::phone_status::PhoneBattery;
        let battery = |charge, charging| PhoneBattery {
            charge,
            charging,
            updated_unix: 0,
            ..Default::default()
        };
        assert!(battery_low(&battery(15, false)));
        assert!(battery_low(&battery(3, false)));
        assert!(!battery_low(&battery(16, false)));
        assert!(!battery_low(&battery(9, true)), "charging is not low");
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
