//! The Phone apps page (Apps part 1, over P2's list): a linked phone's apps
//! as icons in a grid, found by a search, the ones opened here last first.
//! Clicking one asks the phone to open it and to show its screen in the
//! window here (the phone asks its owner once for Android's screen-capture
//! consent, and a screen already shown is reused). The list is asked of the
//! phone as the page opens; its answer, its icons and how an ask went are
//! read from the state folder once a second while the page shows.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::phone_apps::{self as store, Outcome};
use pocketdesk_core::local::phone_pins;
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_action::{AppResult, PhoneAction, PhoneApp, WindowsMode};

use crate::model::Device;

/// How often the page reads what the phone said while it shows.
const READ_EVERY: Duration = Duration::from_secs(1);

/// How long the page waits for the phone's answer before saying it has none.
const ANSWER_WITHIN_SECS: u64 = 15;

/// The most apps drawn at once: a search narrows the rest.
const MAX_SHOWN: usize = 200;

/// The icons decoded in one frame, so a long list does not stall it.
const DECODE_PER_FRAME: usize = 16;

/// An icon's side on screen, and a cell's.
const ICON: f32 = 40.0;
const CELL: egui::Vec2 = egui::vec2(88.0, 78.0);

/// An open asked of the phone, until its answer or the wait runs out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opening {
    pub app: String,
    pub name: String,
    pub asked_at: u64,
    /// The app window asked for (Phone apps tier 2), if it was for one.
    pub window: Option<u8>,
}

/// What the page says of where things stand, and how to go on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No phone is paired.
    NoPhone,
    /// The phone's Phone actions permission is off here.
    PermissionOff,
    /// The phone keeps no link to this computer now.
    NotLinked,
    /// Waiting for the phone's list.
    Asking,
    /// Nothing came back from the phone for its list.
    NoAnswer,
    /// The phone refused to list its apps, and why.
    ListRefused(AppResult),
    /// The list is here.
    Ready,
    /// An open is waiting for the phone's answer.
    Opening(String),
    /// Nothing came back for the open.
    OpenNoAnswer(String),
    /// The app opened.
    Opened(String),
    /// The phone's owner is asked once to share its screen, and the app
    /// opens after.
    NeedsScreen(String),
    /// The phone would not open the app, and why.
    OpenRefused(AppResult, String),
    /// Hands-free could not accept the phone's screen-sharing question: one
    /// tap on the phone's notification is all it needs, and the app opens
    /// after.
    NeedsTap(String),
}

/// How a locked phone can be reached without anyone giving a PIN away: the
/// phone never bypasses a secure lock, and Android's own Extend Unlock
/// keeps it unlocked near a trusted device. Generic, as phone makers name
/// it differently.
pub const EXTEND_UNLOCK_HINT: &str = "Unlock the phone, or let Android keep it unlocked near this computer: Settings \u{2192} Security \u{2192} Extend Unlock / Smart Lock \u{2192} Trusted devices.";

/// What the page knows, for [`status`].
pub struct Known<'a> {
    pub has_phone: bool,
    pub allowed: bool,
    pub linked: bool,
    /// When the list was asked (Unix seconds), if it was.
    pub list_asked_at: Option<u64>,
    /// When the list on hand was made, if there is one.
    pub listed_at: Option<u64>,
    pub outcome: Option<&'a Outcome>,
    pub opening: Option<&'a Opening>,
    pub now: u64,
}

/// Where things stand: the permission and the link first, then an open in
/// flight, then the list.
pub fn status(k: &Known) -> Status {
    if !k.has_phone {
        return Status::NoPhone;
    }
    if !k.allowed {
        return Status::PermissionOff;
    }
    if !k.linked {
        return Status::NotLinked;
    }
    if let Some(open) = k.opening {
        let answer = k.outcome.filter(|o| {
            o.answers("open", open.asked_at) && o.report.app.as_deref() == Some(&open.app)
        });
        return match answer.map(|o| o.report.result) {
            Some(AppResult::Opened) => Status::Opened(open.name.clone()),
            Some(AppResult::NeedsScreen) => Status::NeedsScreen(open.name.clone()),
            Some(AppResult::NeedsTap) => Status::NeedsTap(open.name.clone()),
            Some(other) => Status::OpenRefused(other, open.name.clone()),
            None if k.now.saturating_sub(open.asked_at) < ANSWER_WITHIN_SECS => {
                Status::Opening(open.name.clone())
            }
            None => Status::OpenNoAnswer(open.name.clone()),
        };
    }
    let asked = k.list_asked_at.unwrap_or(0);
    if list_fresh(k.listed_at, asked) {
        return Status::Ready;
    }
    if let Some(refused) = k
        .outcome
        .filter(|o| o.answers("apps", asked) && !o.opened())
        .map(|o| o.report.result)
    {
        return Status::ListRefused(refused);
    }
    if k.now.saturating_sub(asked) < ANSWER_WITHIN_SECS {
        Status::Asking
    } else {
        Status::NoAnswer
    }
}

/// What to offer beside a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    None,
    /// Ask the phone for its list again.
    Retry,
    /// The Devices page, where the phone's permissions are.
    Devices,
}

impl Status {
    /// What the page says of it, for the phone called `who`.
    pub fn message(&self, who: &str) -> String {
        match self {
            Status::NoPhone => "No phone is paired yet: pair one to open its apps here.".into(),
            Status::PermissionOff => format!(
                "{who} may not be asked for its apps: \"Screenshot, lock and open its apps\" is off for it in Devices."
            ),
            Status::NotLinked => format!(
                "{who} is not linked now. Open Pocketdesk on it, or turn on one of its background features, to link it."
            ),
            Status::Asking => format!("Asking {who} for its apps…"),
            Status::NoAnswer => format!(
                "{who} did not answer. It answers while unlocked, with \"Let the computer open my apps\" on in Pocketdesk's Settings there."
            ),
            Status::ListRefused(AppResult::Off) => format!(
                "{who} keeps its apps to itself: turn on \"Let the computer open my apps\" in Pocketdesk's Settings there (off by default)."
            ),
            Status::ListRefused(AppResult::Locked) => {
                format!("{who} is locked. Unlock the phone once, then ask again.")
            }
            Status::ListRefused(AppResult::TooMany) => {
                format!("{who} was asked too often. Wait a minute, then ask again.")
            }
            Status::ListRefused(_) => format!("{who} would not list its apps. Try again."),
            Status::Ready => String::new(),
            Status::Opening(app) => format!("Opening {app} on {who}…"),
            Status::OpenNoAnswer(app) => format!(
                "{who} did not answer for {app}. It answers while unlocked, with \"Let the computer open my apps\" on there."
            ),
            Status::Opened(app) => {
                format!("Opened {app} on {who}. Its screen is in the window here.")
            }
            Status::NeedsScreen(app) => format!(
                "{app} opens after you tap the notification on {who} to show its screen here (Android asks once for the connection)."
            ),
            Status::NeedsTap(app) => format!(
                "{app} needs one tap on {who}: tap its notification to share the screen here (Pocketdesk could not press Android's own question for you), and the app opens after."
            ),
            Status::OpenRefused(result, app) => match result {
                AppResult::ControlOff => format!(
                    "Pocketdesk's control service is off on {who}. To open {app} from here, turn on Pocketdesk control in Android's Accessibility settings; it is used only to start apps and to control the screen shown here."
                ),
                AppResult::Locked => {
                    format!("{who} is locked: unlock the phone once, then open {app} again.")
                }
                AppResult::LockedSecure => format!(
                    "{who} has a lock Pocketdesk never bypasses. {EXTEND_UNLOCK_HINT} Then open {app} again."
                ),
                AppResult::Asleep => {
                    format!("{who}'s screen is off: wake it, then open {app} again.")
                }
                AppResult::Off => format!(
                    "{who} keeps its apps to itself: turn on \"Let the computer open my apps\" in Pocketdesk's Settings there."
                ),
                AppResult::NotListed => {
                    format!("{who} does not know {app} yet: the list is asked again.")
                }
                AppResult::NotFound => format!("{app} is no longer on {who}."),
                AppResult::TooMany => {
                    format!("{who} was asked too often. Wait a minute, then open {app} again.")
                }
                AppResult::WindowLimit => format!(
                    "{who} already shows as many app windows as it takes: close one, then open {app} again."
                ),
                AppResult::WindowsUnavailable => format!(
                    "{who} could not make a window for {app} (its \u{201c}Separate windows (Shizuku)\u{201d} is off, or Shizuku is not running there)."
                ),
                _ => format!("Android would not open {app} on {who}."),
            },
        }
    }

    /// What to offer with it.
    pub fn fix(&self) -> Fix {
        match self {
            Status::PermissionOff => Fix::Devices,
            Status::NoAnswer
            | Status::ListRefused(_)
            | Status::OpenNoAnswer(_)
            | Status::OpenRefused(AppResult::NotListed, _) => Fix::Retry,
            _ => Fix::None,
        }
    }

    /// Whether the owner has one tap to make at the phone, drawn as a
    /// notice of its own (not a failure, not a wait).
    pub fn needs_tap(&self) -> bool {
        matches!(self, Status::NeedsTap(_))
    }

    /// Whether this is a problem, drawn as one.
    pub fn is_problem(&self) -> bool {
        !matches!(
            self,
            Status::Ready
                | Status::Asking
                | Status::Opening(_)
                | Status::Opened(_)
                | Status::NeedsScreen(_)
                | Status::NeedsTap(_)
        )
    }
}

/// Whether a list made at `listed_at` answers the ask made at `asked_at`
/// (both this computer's clock).
pub fn list_fresh(listed_at: Option<u64>, asked_at: u64) -> bool {
    listed_at.is_some_and(|at| at >= asked_at)
}

/// The apps `query` finds (every word in a name or an id, any case), the
/// ones whose name starts with the query's first word first, then the ones
/// opened here last first (`recents`: app to Unix seconds), then by name.
pub fn order<'a>(
    apps: &'a [PhoneApp],
    recents: &BTreeMap<String, u64>,
    query: &str,
) -> Vec<&'a PhoneApp> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<&PhoneApp> = apps
        .iter()
        .filter(|app| {
            let text = format!("{} {}", app.name, app.id).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .collect();
    let first = words.first().cloned().unwrap_or_default();
    found.sort_by_cached_key(|app| {
        let name = app.name.to_lowercase();
        (
            !(first.is_empty() || name.starts_with(&first)),
            std::cmp::Reverse(recents.get(&app.id).copied().unwrap_or(0)),
            name,
        )
    });
    found
}

/// The apps opened here recently, in the order [`order`] puts them: for
/// the row above the grid.
pub fn recent<'a>(apps: &'a [PhoneApp], recents: &BTreeMap<String, u64>) -> Vec<&'a PhoneApp> {
    let mut found: Vec<&PhoneApp> = apps
        .iter()
        .filter(|a| recents.contains_key(&a.id))
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(recents.get(&a.id).copied().unwrap_or(0)));
    found
}

/// What the page asks of the app this frame.
pub enum Action {
    /// Another page.
    Go(crate::app::Tab),
    /// Show (and raise) this phone's screen window: an app opened and the
    /// screen is wanted in view.
    Raise(String),
}

/// An icon the page decoded, or its absence, with the file it came from.
type Icon = (PathBuf, Option<egui::TextureHandle>);

/// The page.
#[derive(Default)]
pub struct AppsPage {
    /// The phone shown.
    phone: Option<String>,
    query: String,
    apps: Vec<PhoneApp>,
    /// When the list on hand was made by the phone (Unix seconds).
    listed_at: Option<u64>,
    /// When the page asked the phone for its list, and which phone.
    asked: Option<(String, u64)>,
    outcome: Option<Outcome>,
    opening: Option<Opening>,
    /// The word of the phone last acted on (when, and what), so each is acted on once.
    acted: Option<(u64, AppResult)>,
    recents: BTreeMap<String, u64>,
    /// The apps of this phone pinned to Start or the applications menu.
    pinned: BTreeSet<String>,
    /// Settings: no pin menu on the apps (Apps part 2).
    pub pins_off: bool,
    read_at: Option<Instant>,
    icons: HashMap<String, Icon>,
    focus: bool,
    go_to: Option<crate::app::Tab>,
    raise: Option<String>,
    /// What the page said happened, until the app takes it.
    notice: Option<crate::notice::Notice>,
    /// Settings: open apps in windows of their own when the phone offers
    /// it (Phone apps tier 2). Set by the app before each pass.
    pub windows_setting: bool,
    /// The app windows in use (shown, or asked for), set by the app before
    /// each pass.
    pub used_windows: Vec<u8>,
    /// What the phone shown said of windows with its list.
    windows_mode: WindowsMode,
    /// A window asked for by an open this pass: its id and the app's name,
    /// until the app takes it.
    asked_window: Option<(u8, String)>,
    /// A window the phone did not make, until the app takes it.
    released_window: Option<u8>,
}

impl AppsPage {
    /// Show `phone`'s apps (from the Overview's button).
    pub fn choose(&mut self, phone: &str) {
        if self.phone.as_deref() != Some(phone) {
            self.reset_for_phone();
        }
        self.phone = Some(phone.to_string());
    }

    fn reset_for_phone(&mut self) {
        self.apps.clear();
        self.listed_at = None;
        self.asked = None;
        self.outcome = None;
        self.opening = None;
        self.acted = None;
        self.windows_mode = WindowsMode::default();
        self.icons.clear();
        self.pinned.clear();
        self.read_at = None;
    }

    /// What the page said happened, for the app's one notice.
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }

    /// A window an open asked for: its id and the app's name (Phone apps
    /// tier 2), for the app to hold the id.
    pub fn take_asked_window(&mut self) -> Option<(u8, String)> {
        self.asked_window.take()
    }

    /// A window the phone did not make, which is free again.
    pub fn take_released_window(&mut self) -> Option<u8> {
        self.released_window.take()
    }

    /// How an open goes now, from what the phone said and this computer's
    /// switch.
    fn route(&self) -> crate::phone_windows::Route {
        crate::phone_windows::Route {
            mode: self.windows_mode,
            setting: self.windows_setting,
            used: self.used_windows.clone(),
        }
    }

    /// Read what the phone said, once a second.
    fn refresh(&mut self, state_dir: &Path, phone: &str) {
        if self.read_at.is_some_and(|at| at.elapsed() < READ_EVERY) {
            return;
        }
        self.read_at = Some(Instant::now());
        if let Some(list) = store::read(state_dir).remove(phone) {
            self.apps = list.apps;
            self.listed_at = Some(list.at);
            self.windows_mode = list.windows;
        }
        self.outcome = store::read_results(state_dir).remove(phone);
        self.recents = store::read_recent(state_dir)
            .remove(phone)
            .unwrap_or_default();
        self.pinned = phone_pins::pinned(state_dir, phone);
        // An icon that was not there is looked for again: it may have come.
        self.icons.retain(|_, (_, texture)| texture.is_some());
    }

    /// Ask the phone for its list.
    fn ask(&mut self, state_dir: &Path, phone: &str, now: u64) {
        match pocketdesk_core::local::phone_actions::request(
            state_dir,
            phone,
            PhoneAction::Apps,
            now,
        ) {
            Ok(_) => self.asked = Some((phone.to_string(), now)),
            Err(e) => {
                self.notice = crate::notice::Notice::error(format!("Could not ask the phone: {e}"));
            }
        }
    }

    /// Ask the phone to open `app`: in a window of its own when the phone
    /// offers it and this computer's switch is on (Phone apps tier 2), else
    /// on its screen, shown here.
    fn open(&mut self, state_dir: &Path, phone: &str, app: &PhoneApp, now: u64) {
        self.open_by(state_dir, phone, app, now, &self.route());
    }

    fn open_by(
        &mut self,
        state_dir: &Path,
        phone: &str,
        app: &PhoneApp,
        now: u64,
        route: &crate::phone_windows::Route,
    ) {
        match crate::phone_windows::ask_open(state_dir, phone, &app.id, route, now) {
            Ok(asked) => {
                let window = match asked {
                    crate::phone_windows::Asked::Window(id) => Some(id),
                    crate::phone_windows::Asked::Screen => None,
                };
                if let Some(id) = window {
                    self.asked_window = Some((id, app.name.clone()));
                    // Held at once, so a second open this pass asks another.
                    self.used_windows.push(id);
                }
                self.opening = Some(Opening {
                    app: app.id.clone(),
                    name: app.name.clone(),
                    asked_at: now,
                    window,
                });
            }
            Err(e) => {
                self.notice = crate::notice::Notice::error(e);
            }
        }
    }

    /// Pin `app` to Start (Windows) or the applications menu (Linux): a
    /// shortcut that opens it on `phone` from this computer.
    fn pin(&mut self, state_dir: &Path, phone: &str, app: &PhoneApp) {
        self.notice = match self.try_pin(state_dir, phone, app) {
            Ok(()) => {
                self.pinned.insert(app.id.clone());
                crate::notice::Notice::done(pinned_words(&app.name))
            }
            Err(e) => crate::notice::Notice::error(format!("Could not pin {}: {e}", app.name)),
        };
    }

    fn try_pin(&self, state_dir: &Path, phone: &str, app: &PhoneApp) -> std::io::Result<()> {
        let places = phone_pins::places(state_dir)
            .ok_or_else(|| std::io::Error::other("this system's folders are not known"))?;
        let exe = std::env::current_exe()?;
        // The app's own icon as the phone sent it, else Pocketdesk's.
        let png =
            pocketdesk_core::local::phone_app_icons::kept(state_dir, phone, &app.id, app.version)
                .and_then(|path| std::fs::read(path).ok())
                // Whatever the phone sent is decoded and written again, so only a
                // picture this program made lands in a shortcut's icon file.
                .and_then(|bytes| reencode_png(&bytes))
                .or_else(own_icon_png)
                .ok_or_else(|| std::io::Error::other("no icon"))?;
        phone_pins::pin(state_dir, &places, &exe, phone, &app.id, &app.name, &png).map(|_| ())
    }

    /// Take `app`'s pin away.
    fn unpin(&mut self, state_dir: &Path, phone: &str, app: &PhoneApp) {
        let removed = phone_pins::places(state_dir)
            .ok_or_else(|| std::io::Error::other("this system's folders are not known"))
            .and_then(|places| phone_pins::unpin(state_dir, &places, phone, &app.id));
        self.notice = match removed {
            Ok(_) => {
                self.pinned.remove(&app.id);
                crate::notice::Notice::done(format!("Unpinned {}.", app.name))
            }
            Err(e) => crate::notice::Notice::error(format!("Could not unpin {}: {e}", app.name)),
        };
    }

    /// Act once on the result of an open: remember the app as recent and
    /// have the phone's screen window shown, or ask for the list again when
    /// the phone did not know the app.
    fn act_on_result(&mut self, state_dir: &Path, phone: &str, now: u64) {
        let Some(open) = self.opening.clone() else {
            return;
        };
        let Some(outcome) = self.outcome.clone() else {
            return;
        };
        if !outcome.answers("open", open.asked_at)
            || outcome.report.app.as_deref() != Some(&open.app)
            || self.acted == Some((outcome.at, outcome.report.result))
        {
            return;
        }
        self.acted = Some((outcome.at, outcome.report.result));
        match outcome.report.result {
            AppResult::Opened => {
                let _ = store::note_open(state_dir, phone, &open.app, now);
                self.recents.insert(open.app.clone(), now);
                // An app window comes up by itself; the phone's screen window
                // is brought up only for an app on the phone's screen.
                if open.window.is_none() {
                    self.raise = Some(phone.to_string());
                }
            }
            AppResult::NotListed => self.ask(state_dir, phone, now),
            // The phone could not make a window (its switch went off, Shizuku
            // stopped, Android refused a display): the app opens the ordinary
            // way, and the phone's word is not asked about again.
            AppResult::WindowsUnavailable if open.window.is_some() => {
                self.windows_mode = WindowsMode::default();
                let route = self.route();
                if let Some(app) = self.apps.iter().find(|a| a.id == open.app).cloned() {
                    self.open_by(state_dir, phone, &app, now, &route);
                }
            }
            _ => {}
        }
        // A window asked for that did not open is free again.
        if outcome.report.result != AppResult::Opened
            && let Some(id) = open.window
        {
            self.released_window = Some(id);
        }
    }

    /// The icon of `app` as a texture, decoded from the kept file once.
    fn icon(
        &mut self,
        ctx: &egui::Context,
        state_dir: &Path,
        phone: &str,
        app: &PhoneApp,
        budget: &mut usize,
    ) -> Option<egui::TextureHandle> {
        let path =
            pocketdesk_core::local::phone_app_icons::kept(state_dir, phone, &app.id, app.version)?;
        if let Some((from, texture)) = self.icons.get(&app.id)
            && *from == path
        {
            return texture.clone();
        }
        if *budget == 0 {
            ctx.request_repaint();
            return None;
        }
        *budget -= 1;
        let texture = decode(ctx, &path, &format!("app-icon-{phone}-{}", app.id));
        self.icons.insert(app.id.clone(), (path, texture.clone()));
        texture
    }

    /// Draw the page.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        devices: &[Device],
        now: u64,
    ) -> Option<Action> {
        let phones: Vec<&Device> = devices.iter().filter(|d| !d.computer).collect();
        if phones.is_empty() {
            let status = Status::NoPhone;
            if crate::widgets::empty(ui, "No phone is paired yet", &status.message(""), |ui| {
                ui.button("Pair a phone").clicked()
            }) {
                return Some(Action::Go(crate::app::Tab::Pair));
            }
            return None;
        }
        // The phone shown: the one chosen, else the first linked, else the first.
        if self
            .phone
            .as_ref()
            .is_none_or(|p| !phones.iter().any(|d| &d.id == p))
        {
            let first = phones.iter().find(|d| d.linked).unwrap_or(&phones[0]);
            let id = first.id.clone();
            self.choose(&id);
        }
        let phone = self.phone.clone().unwrap_or_default();
        let device = phones.iter().find(|d| d.id == phone).copied();
        let who = crate::model::device_label(devices, &phone);
        let mut action = None;
        if phones.len() > 1 {
            ui.horizontal_wrapped(|ui| {
                for d in &phones {
                    if ui
                        .selectable_label(d.id == phone, crate::model::device_label(devices, &d.id))
                        .clicked()
                    {
                        self.choose(&d.id);
                    }
                }
            });
        }
        let allowed = device.is_some_and(|d| d.permissions.allows(Capability::PhoneActions));
        let linked = device.is_some_and(|d| d.linked);
        ui.ctx().request_repaint_after(READ_EVERY);
        // Phone apps tier 2: how an app opens, in a window or on the phone.
        ui.label(
            RichText::new(crate::phone_windows::mode_words(
                self.windows_mode,
                self.windows_setting,
                self.used_windows.len(),
            ))
            .small()
            .color(crate::theme::muted(ui)),
        );
        self.refresh(state_dir, &phone);
        self.act_on_result(state_dir, &phone, now);
        // Asked as the page first shows for a phone that can be asked.
        if allowed && linked && self.asked.as_ref().is_none_or(|(p, _)| p != &phone) {
            self.ask(state_dir, &phone, now);
        }
        let known = Known {
            has_phone: true,
            allowed,
            linked,
            list_asked_at: self
                .asked
                .as_ref()
                .filter(|(p, _)| p == &phone)
                .map(|(_, at)| *at),
            listed_at: self.listed_at,
            outcome: self.outcome.as_ref(),
            opening: self.opening.as_ref(),
            now,
        };
        let status = status(&known);
        let message = status.message(&who);
        if !message.is_empty() {
            crate::theme::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if matches!(status, Status::Asking | Status::Opening(_)) {
                        ui.spinner();
                    }
                    let text = RichText::new(&message);
                    if status.needs_tap() {
                        ui.label(RichText::new("Tap needed on the phone").strong());
                    }
                    ui.label(if status.is_problem() {
                        text.color(crate::theme::warn(ui))
                    } else {
                        text
                    });
                    match status.fix() {
                        Fix::Retry if ui.button("Ask again").clicked() => {
                            self.opening = None;
                            self.ask(state_dir, &phone, now);
                        }
                        Fix::Devices if ui.button("Open Devices").clicked() => {
                            self.go_to = Some(crate::app::Tab::Devices);
                        }
                        _ => {}
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(tab) = self.go_to.take() {
            action = Some(Action::Go(tab));
        }
        if self.apps.is_empty() {
            if matches!(status, Status::Ready) {
                ui.label(RichText::new("The phone sent no apps.").color(crate::theme::muted(ui)));
            }
            return action.or_else(|| self.raise.take().map(Action::Raise));
        }
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Find an app")
                .desired_width(260.0),
        );
        if !std::mem::replace(&mut self.focus, true) {
            search.request_focus();
        }
        // An open needs the phone's link and its permission, not a fresh list.
        let can_open = allowed && linked && !matches!(status, Status::Opening(_));
        let recents = self.recents.clone();
        let pinned = self.pinned.clone();
        let pin_menu = !self.pins_off;
        let apps = self.apps.clone();
        let found = order(&apps, &recents, &self.query);
        if found.is_empty() {
            ui.label(RichText::new("No app matches.").color(crate::theme::muted(ui)));
        }
        let mut open = None;
        let mut pin_ask: Option<(PhoneApp, bool)> = None;
        let mut budget = DECODE_PER_FRAME;
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.query.trim().is_empty() {
                    let latest = recent(&apps, &recents);
                    if !latest.is_empty() {
                        ui.label(RichText::new("Opened from here").strong());
                        ui.horizontal_wrapped(|ui| {
                            for app in latest.iter().take(8) {
                                let icon = self.icon(&ctx, state_dir, &phone, app, &mut budget);
                                let pin = pin_menu.then(|| pinned.contains(&app.id));
                                match cell(ui, app, icon.as_ref(), can_open, pin) {
                                    Some(CellAction::Open) => open = Some((*app).clone()),
                                    Some(CellAction::Pin(on)) => {
                                        pin_ask = Some(((*app).clone(), on))
                                    }
                                    None => {}
                                }
                            }
                        });
                        ui.add_space(8.0);
                        ui.label(RichText::new("All apps").strong());
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for app in found.iter().take(MAX_SHOWN) {
                        let icon = self.icon(&ctx, state_dir, &phone, app, &mut budget);
                        let pin = pin_menu.then(|| pinned.contains(&app.id));
                        match cell(ui, app, icon.as_ref(), can_open, pin) {
                            Some(CellAction::Open) => open = Some((*app).clone()),
                            Some(CellAction::Pin(on)) => pin_ask = Some(((*app).clone(), on)),
                            None => {}
                        }
                    }
                });
                if found.len() > MAX_SHOWN {
                    ui.label(format!(
                        "{} more: type to narrow them.",
                        found.len() - MAX_SHOWN
                    ));
                }
            });
        if let Some(app) = open {
            self.open(state_dir, &phone, &app, now);
        }
        if let Some((app, on)) = pin_ask {
            if on {
                self.pin(state_dir, &phone, &app);
            } else {
                self.unpin(state_dir, &phone, &app);
            }
        }
        action.or_else(|| self.raise.take().map(Action::Raise))
    }
}

/// What a cell was used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CellAction {
    /// Clicked: open the app.
    Open,
    /// Its pin menu entry: pin it (`true`) or unpin it.
    Pin(bool),
}

/// The words for a pin that was made, for this system.
pub fn pinned_words(app: &str) -> String {
    if cfg!(windows) {
        format!(
            "Pinned {app} to Start. For the taskbar too, right-click it in Start and choose Pin to taskbar."
        )
    } else {
        format!(
            "Pinned {app} to {}. Some desktops take a moment to list it.",
            phone_pins::MENU_NAME
        )
    }
}

/// `bytes` decoded as an icon (held to an icon's size by its header and the
/// decoder's limits, as the page's textures are) and encoded again as a PNG:
/// nothing but the picture survives. `None` when it is not a small PNG.
fn reencode_png(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    use pocketdesk_core::local::notification_icons as icons;
    if !icons::icon_fits(bytes) {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(icons::MAX_ICON_SIDE);
    limits.max_image_height = Some(icons::MAX_ICON_SIDE);
    limits.max_alloc = Some(8 << 20);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(png)
}

/// Pocketdesk's own icon as a PNG, for an app whose icon the phone has not
/// sent.
fn own_icon_png() -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            &pocketdesk_desktop::icon::icon_rgba(64),
            64,
            64,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(png)
}

/// One app in the grid: its icon and name, a button, and (with `pin` set to
/// whether it is pinned) a right-click menu to pin it. What it was used for.
fn cell(
    ui: &mut egui::Ui,
    app: &PhoneApp,
    icon: Option<&egui::TextureHandle>,
    enabled: bool,
    pin: Option<bool>,
) -> Option<CellAction> {
    // Always a click target, for the pin menu; an open needs `enabled`.
    let (rect, response) = ui.allocate_exact_size(CELL, egui::Sense::click());
    let label = format!("Open {}", app.name);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        if enabled && response.hovered() {
            painter.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + 6.0 + ICON / 2.0),
            egui::vec2(ICON, ICON),
        );
        let tint = if enabled {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_white_alpha(120)
        };
        match icon {
            Some(texture) => {
                painter.image(
                    texture.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    tint,
                );
            }
            None => {
                painter.rect_filled(icon_rect, 8.0, ui.visuals().faint_bg_color);
                painter.text(
                    icon_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    app.name
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                    egui::FontId::proportional(18.0),
                    ui.visuals().text_color(),
                );
            }
        }
        let mut job = egui::text::LayoutJob::simple(
            app.name.clone(),
            egui::FontId::proportional(11.0),
            ui.visuals().text_color(),
            CELL.x - 8.0,
        );
        job.wrap.max_rows = 2;
        job.wrap.break_anywhere = true;
        job.halign = egui::Align::Center;
        let galley = ui.painter().layout_job(job);
        let at = egui::pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.top() + ICON + 12.0,
        );
        painter.galley(at, galley, ui.visuals().text_color());
    }
    let hint = if pin.is_some() {
        format!(
            "{} ({})\nRight-click to pin it to {}.",
            app.name,
            app.id,
            phone_pins::MENU_NAME
        )
    } else {
        format!("{} ({})", app.name, app.id)
    };
    let response = response.on_hover_text(hint);
    let mut used = (enabled && response.clicked()).then_some(CellAction::Open);
    if let Some(pinned) = pin {
        response.context_menu(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new(format!("Open {}", app.name)))
                .clicked()
            {
                used = Some(CellAction::Open);
                ui.close();
            }
            let text = if pinned {
                format!("Unpin from {}", phone_pins::MENU_NAME)
            } else {
                format!("Pin to {}", phone_pins::MENU_NAME)
            };
            if ui.button(text).clicked() {
                used = Some(CellAction::Pin(!pinned));
                ui.close();
            }
        });
    }
    used
}

/// `path` decoded to a texture, if it is a small PNG (held to an icon's
/// size by its header before anything decodes).
fn decode(ctx: &egui::Context, path: &Path, name: &str) -> Option<egui::TextureHandle> {
    use pocketdesk_core::local::notification_icons as icons;
    let bytes = std::fs::read(path).ok()?;
    if !icons::icon_fits(&bytes) {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(icons::MAX_ICON_SIDE);
    limits.max_image_height = Some(icons::MAX_ICON_SIDE);
    limits.max_alloc = Some(8 << 20);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(name, color, egui::TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_action::AppReport;

    fn app(id: &str, name: &str) -> PhoneApp {
        PhoneApp {
            id: id.into(),
            name: name.into(),
            version: 1,
        }
    }

    fn apps() -> Vec<PhoneApp> {
        vec![
            app("org.example.camera", "Camera"),
            app("org.example.music", "Music Player"),
            app("org.example.maps", "Maps"),
            app("org.example.clock", "Clock"),
        ]
    }

    fn names(found: Vec<&PhoneApp>) -> Vec<&str> {
        found.into_iter().map(|a| a.name.as_str()).collect()
    }

    /// Apps part 2 review: a phone's icon is decoded and written again before
    /// it reaches a shortcut's icon file, so anything riding in the file goes.
    #[test]
    fn a_phone_s_icon_is_written_again_before_it_is_kept() {
        let own = own_icon_png().unwrap();
        // A picture with something after it, or hidden in a chunk, comes out clean.
        let mut dirty = own.clone();
        dirty.extend_from_slice(b"MZ-not-a-picture-but-appended-bytes");
        let clean = reencode_png(&dirty).unwrap();
        assert!(clean.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(!clean.windows(5).any(|w| w == b"MZ-no"));
        assert_eq!(&clean[12..16], b"IHDR");
        assert_eq!(
            reencode_png(&own).unwrap(),
            clean,
            "the same picture, the same file"
        );
        for bad in [&b""[..], b"not a png at all, no, none", &own[..40]] {
            assert!(reencode_png(bad).is_none());
        }
        // Past an icon's size it is refused before anything decodes.
        let big = {
            use image::ImageEncoder;
            let side = pocketdesk_core::local::notification_icons::MAX_ICON_SIDE + 1;
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(
                    &vec![0u8; (side * side * 4) as usize],
                    side,
                    side,
                    image::ExtendedColorType::Rgba8,
                )
                .unwrap();
            png
        };
        assert!(reencode_png(&big).is_none());
    }

    /// P2: a search finds apps by name or id, every word, any case.
    #[test]
    fn a_search_finds_apps_by_name_or_id() {
        let apps = apps();
        let none = BTreeMap::new();
        assert_eq!(order(&apps, &none, "").len(), 4);
        assert_eq!(names(order(&apps, &none, "MUSIC")), vec!["Music Player"]);
        assert_eq!(names(order(&apps, &none, "example maps")), vec!["Maps"]);
        assert!(order(&apps, &none, "music maps").is_empty());
        assert!(order(&apps, &none, "zzz").is_empty());
    }

    /// Apps part 1: the apps opened here last come first, then by name.
    #[test]
    fn the_apps_opened_here_come_first() {
        let apps = apps();
        let none = BTreeMap::new();
        assert_eq!(
            names(order(&apps, &none, "")),
            vec!["Camera", "Clock", "Maps", "Music Player"],
            "by name"
        );
        let recents = BTreeMap::from([
            ("org.example.maps".to_string(), 50),
            ("org.example.music".to_string(), 90),
        ]);
        assert_eq!(
            names(order(&apps, &recents, "")),
            vec!["Music Player", "Maps", "Camera", "Clock"],
            "the latest first"
        );
        assert_eq!(
            names(recent(&apps, &recents)),
            vec!["Music Player", "Maps"],
            "the row of recents"
        );
    }

    /// A name that starts with what was typed comes before one that only holds it.
    #[test]
    fn a_name_starting_with_the_search_comes_first() {
        let apps = vec![
            app("org.example.photos", "Gallery Photos"),
            app("org.example.pic", "Pictures"),
            app("org.example.pho", "Phone"),
        ];
        let recents = BTreeMap::from([("org.example.photos".to_string(), 99)]);
        assert_eq!(
            names(order(&apps, &recents, "p")),
            vec!["Phone", "Pictures", "Gallery Photos"]
        );
    }

    fn outcome(about: &str, app: Option<&str>, result: AppResult, at: u64) -> Outcome {
        Outcome {
            report: AppReport {
                about: about.into(),
                app: app.map(str::to_string),
                name: None,
                window: None,
                result,
            },
            at,
        }
    }

    fn known<'a>(outcome: Option<&'a Outcome>, opening: Option<&'a Opening>) -> Known<'a> {
        Known {
            has_phone: true,
            allowed: true,
            linked: true,
            list_asked_at: Some(100),
            listed_at: None,
            outcome,
            opening,
            now: 105,
        }
    }

    fn opening() -> Opening {
        Opening {
            window: None,
            app: "org.example.app".into(),
            name: "Example".into(),
            asked_at: 100,
        }
    }

    /// The states the page tells apart before anything is asked.
    #[test]
    fn the_page_says_why_it_cannot_ask() {
        let mut k = known(None, None);
        k.has_phone = false;
        assert_eq!(status(&k), Status::NoPhone);
        let mut k = known(None, None);
        k.allowed = false;
        k.linked = false;
        assert_eq!(status(&k), Status::PermissionOff, "the permission first");
        let mut k = known(None, None);
        k.linked = false;
        assert_eq!(status(&k), Status::NotLinked);
        assert_eq!(Status::PermissionOff.fix(), Fix::Devices);
        assert!(Status::NotLinked.message("Phone").contains("not linked"));
    }

    /// The list: asked, answered, refused, or not answered.
    #[test]
    fn the_list_is_asked_then_ready_or_refused() {
        let k = known(None, None);
        assert_eq!(status(&k), Status::Asking);
        let mut late = known(None, None);
        late.now = 100 + ANSWER_WITHIN_SECS;
        assert_eq!(status(&late), Status::NoAnswer);
        assert_eq!(Status::NoAnswer.fix(), Fix::Retry);
        let mut ready = known(None, None);
        ready.listed_at = Some(100);
        assert_eq!(status(&ready), Status::Ready);
        ready.listed_at = Some(99);
        assert_eq!(
            status(&ready),
            Status::Asking,
            "an older list is not the answer"
        );
        let off = outcome("apps", None, AppResult::Off, 101);
        let refused = known(Some(&off), None);
        assert_eq!(status(&refused), Status::ListRefused(AppResult::Off));
        assert!(
            Status::ListRefused(AppResult::Off)
                .message("Phone")
                .contains("Let the computer open my apps")
        );
        let locked = outcome("apps", None, AppResult::Locked, 101);
        assert_eq!(
            status(&known(Some(&locked), None)),
            Status::ListRefused(AppResult::Locked)
        );
        assert!(
            Status::ListRefused(AppResult::Locked)
                .message("Phone")
                .contains("Unlock the phone once")
        );
        // A word from before the ask is not its answer.
        let old = outcome("apps", None, AppResult::Off, 99);
        assert_eq!(status(&known(Some(&old), None)), Status::Asking);
    }

    /// An open: waiting, opened, the screen first, refused, or not answered.
    #[test]
    fn an_open_says_how_it_went() {
        let open = opening();
        assert_eq!(
            status(&known(None, Some(&open))),
            Status::Opening("Example".into())
        );
        let mut late = known(None, Some(&open));
        late.now = 100 + ANSWER_WITHIN_SECS;
        assert_eq!(status(&late), Status::OpenNoAnswer("Example".into()));
        let said = |result| outcome("open", Some("org.example.app"), result, 101);
        let opened = said(AppResult::Opened);
        assert_eq!(
            status(&known(Some(&opened), Some(&open))),
            Status::Opened("Example".into())
        );
        let screen = said(AppResult::NeedsScreen);
        assert_eq!(
            status(&known(Some(&screen), Some(&open))),
            Status::NeedsScreen("Example".into())
        );
        for (result, words) in [
            (AppResult::ControlOff, "Accessibility"),
            (AppResult::Locked, "unlock the phone once"),
            (AppResult::LockedSecure, "Extend Unlock"),
            (AppResult::Asleep, "wake it"),
            (AppResult::Off, "Let the computer open my apps"),
            (AppResult::NotFound, "no longer on"),
        ] {
            let refused = said(result);
            let s = status(&known(Some(&refused), Some(&open)));
            assert_eq!(s, Status::OpenRefused(result, "Example".into()));
            assert!(s.message("Phone").contains(words), "{result:?}");
            assert!(s.is_problem());
        }
        // Another app's word, or one from before the ask, is not its answer.
        let other = outcome("open", Some("org.example.other"), AppResult::Opened, 101);
        assert_eq!(
            status(&known(Some(&other), Some(&open))),
            Status::Opening("Example".into())
        );
        let before = outcome("open", Some("org.example.app"), AppResult::Opened, 99);
        assert_eq!(
            status(&known(Some(&before), Some(&open))),
            Status::Opening("Example".into())
        );
        assert_eq!(
            Status::OpenRefused(AppResult::NotListed, "Example".into()).fix(),
            Fix::Retry
        );
        assert!(!Status::Opened("Example".into()).is_problem());
    }

    /// Hands-free: a phone that could not press its own screen-sharing
    /// question says it needs a tap, which the page draws as one to make,
    /// not as a failure; a secure lock names Android's own Extend Unlock.
    #[test]
    fn a_phone_that_needs_a_tap_says_so() {
        let open = opening();
        let tap = outcome("open", Some("org.example.app"), AppResult::NeedsTap, 101);
        let s = status(&known(Some(&tap), Some(&open)));
        assert_eq!(s, Status::NeedsTap("Example".into()));
        assert!(s.message("Phone").contains("one tap on Phone"));
        assert!(s.needs_tap());
        assert!(!s.is_problem());
        assert!(!Status::NeedsScreen("Example".into()).needs_tap());
        assert!(!Status::Opened("Example".into()).needs_tap());
        let secure = outcome(
            "open",
            Some("org.example.app"),
            AppResult::LockedSecure,
            101,
        );
        let s = status(&known(Some(&secure), Some(&open)));
        assert_eq!(
            s,
            Status::OpenRefused(AppResult::LockedSecure, "Example".into())
        );
        let words = s.message("Phone");
        assert!(words.contains("Trusted devices"));
        assert!(words.contains("never bypasses"));
        assert!(s.is_problem());
    }

    /// P2 review: only a list made since the ask answers it.
    #[test]
    fn a_list_answers_only_the_ask_before_it() {
        assert!(list_fresh(Some(100), 100));
        assert!(!list_fresh(Some(99), 100));
        assert!(!list_fresh(None, 100));
    }
}
