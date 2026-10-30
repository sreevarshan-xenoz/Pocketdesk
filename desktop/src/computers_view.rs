//! The owner's other computers (2f). Pair → Computer pastes another
//! computer's pairing link (its own Pair → Computer's "Copy link") and
//! shows how pairing goes ([`ComputersView::show_pairing`]); Settings →
//! Other computers lists the computers this one paired with, how each
//! one's link is doing, Show its screen and Unpair
//! ([`ComputersView::show_list`]). The server does the pairing and keeps
//! the links; this asks it through `computers_out/`.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile, PeerComputer};

/// How often the file is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);
/// How long Paste waits for the clipboard's text (PR #553 review M2).
const PASTE_WAIT: Duration = Duration::from_secs(2);
/// How often while a pairing waits for its outcome (UX 1).
const PAIRING_REFRESH: Duration = Duration::from_millis(500);
/// How long a pairing's progress waits for its outcome: a request the
/// server has not taken within its life is dropped, and pairing itself
/// tries each of the link's addresses for some seconds.
const PAIRING_WAIT: Duration = Duration::from_secs(computers::REQUEST_TTL_SECS + 60);

/// A pairing asked for, waiting for the server's outcome (UX 1).
struct Pairing {
    /// The request's id, which its outcome names.
    request: String,
    /// The link's computer, as shown.
    name: String,
    since: Instant,
    /// Cancel came after the server took it (PR #553 review I1).
    too_late: bool,
}

#[derive(Default)]
pub struct ComputersView {
    file: ComputersFile,
    read_at: Option<Instant>,
    /// The pairing link being pasted.
    link: String,
    /// The computer whose Unpair waits for its answer.
    forgetting: Option<String>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
    /// The computer whose screen was asked for, until the app takes it (BK7).
    view: Option<(String, String)>,
    /// PC1: the owner ticked that the pasted link's computer, which asks to
    /// pair both ways, is their own; and the link it was ticked for.
    consent: Option<String>,
    /// Paste was pressed: the clipboard's text, which comes as a paste
    /// event, goes in the box (UX 1).
    pasting: Option<Instant>,
    /// The frame this page was last drawn in: a gap is the page left, and
    /// the link, which holds the other computer's PIN, goes (review M1).
    drawn_pass: Option<u64>,
    /// This computer's own id, read with the file, to tell its own link.
    own_id: Option<String>,
    /// The pairing under way, until its outcome comes (UX 1).
    pairing: Option<Pairing>,
    /// Whether Windows treats this computer's network as Public, where
    /// the other computer cannot reach it.
    public: crate::network_profile::PublicNetwork,
    /// PC6: the computer whose Show its screen, with its link down,
    /// offers to wake it first.
    offer: Option<String>,
    /// PC6: a computer woken to show its screen, until its link comes.
    waking: Option<Waking>,
}

/// How long "Wake it and show" waits for the computer's link (PC6).
pub const WAKE_WAIT: Duration = Duration::from_secs(60);

/// What is said wherever Wake is offered (PC6).
pub const WAKE_NOTE: &str = "Wake-on-LAN works best wired: Wi-Fi rarely wakes a computer.";

/// The button that wakes a computer (PC6).
pub const WAKE: &str = "Wake";

/// The offer of Show its screen with the link down (PC6).
pub const WAKE_AND_SHOW: &str = "Wake it and show";

/// A computer woken to show its screen, waiting for its link (PC6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waking {
    pub device_id: String,
    pub name: String,
    pub since: Instant,
}

/// Where a wait for a woken computer stands (PC6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeStep {
    /// Still waiting, this many seconds left.
    Waiting(u64),
    /// Its link came: show its screen.
    Linked,
    /// [`WAKE_WAIT`] passed without its link.
    TimedOut,
}

impl Waking {
    /// Where the wait stands at `now`, its link up or not.
    pub fn step(&self, linked: bool, now: Instant) -> WakeStep {
        let waited = now.saturating_duration_since(self.since);
        if linked {
            WakeStep::Linked
        } else if waited >= WAKE_WAIT {
            WakeStep::TimedOut
        } else {
            WakeStep::Waiting((WAKE_WAIT - waited).as_secs_f32().ceil() as u64)
        }
    }
}

pub use crate::model::{can_wake, view_reach};

/// A link's state in words.
pub fn link_words(computer: &PeerComputer) -> String {
    match computer.link.as_str() {
        "linked" => "linked".to_string(),
        "connecting" | "" => "connecting…".to_string(),
        "refused" => {
            "refused: it did not admit this computer (removed there, or older); pair again"
                .to_string()
        }
        other => other
            .strip_prefix("away: ")
            .map_or_else(|| other.to_string(), |why| format!("not linked ({why})")),
    }
}

/// When `computer`'s screen was last shown here, as "viewed 2 h ago"
/// (PC6); none when never.
pub fn viewed_words(computer: &PeerComputer, now_unix: u64) -> Option<String> {
    (computer.viewed_unix > 0).then(|| {
        format!(
            "viewed {}",
            crate::words::ago(now_unix, computer.viewed_unix)
        )
    })
}

/// Whether `text` looks like a pairing link (the server checks it fully),
/// once cleaned of what a paste carries unseen around it
/// ([`pair_link::clean_link`]: a byte-order mark kept Pair greyed out).
///
/// [`pair_link::clean_link`]: pocketdesk_core::local::pair_link::clean_link
pub fn looks_like_link(text: &str) -> bool {
    pocketdesk_core::local::pair_link::clean_link(text).starts_with("pocketdesk://pair?")
}

/// What a pasted text is, as the line under the box says it (UX 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkCheck {
    /// Nothing pasted.
    Empty,
    /// A pairing link from another computer.
    Valid(Box<pocketdesk_core::local::pair_link::PairLink>),
    /// This computer's own pairing link.
    Own,
    /// A pairing link's start, but not a whole one (PR #553 review M4).
    CutOff,
    /// Not a pairing link.
    NotALink,
}

/// Check `text`, pasted on the computer whose id is `own_id`.
pub fn check_link(text: &str, own_id: Option<&str>) -> LinkCheck {
    let text = pocketdesk_core::local::pair_link::clean_link(text);
    if text.is_empty() {
        return LinkCheck::Empty;
    }
    if !looks_like_link(text) {
        return LinkCheck::NotALink;
    }
    match pocketdesk_core::local::pair_link::PairLink::from_uri(text) {
        // Cut inside its certificate's fingerprint, it still reads.
        Ok(link) if !whole_fingerprint(link.fingerprint.as_deref()) => LinkCheck::CutOff,
        Ok(link) if own_id == Some(link.device_id.as_str()) => LinkCheck::Own,
        Ok(link) => LinkCheck::Valid(Box::new(link)),
        Err(_) => LinkCheck::CutOff,
    }
}

/// What the line under the box says of a text that is not a link.
pub const NOT_A_LINK: &str = "Not a Pocketdesk link. Copy it on the other computer's Pair page.";
/// Whether a link's certificate fingerprint is whole: 32 bytes in hex, as
/// the server reads it (its `parse_fingerprint`).
fn whole_fingerprint(fingerprint: Option<&str>) -> bool {
    fingerprint.is_some_and(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit()))
}

/// What it says of a link's start that is not a whole link (PR #553
/// review M4).
pub const CUT_OFF: &str = "This link looks cut off: copy it again on the other computer, whole.";
/// What it says of this computer's own link.
pub const OWN_LINK: &str =
    "That's this computer's own link: paste the one copied on the other computer.";

impl ComputersView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }

    /// The computer whose screen "Show its screen" asked for, by id and
    /// name, once (BK7).
    pub fn take_view(&mut self) -> Option<(String, String)> {
        self.view.take()
    }
    /// Show `computer`'s screen (BK7), or with its link down and a way
    /// to wake it known, offer to wake it first (PC6).
    pub fn show_or_offer(&mut self, computer: &PeerComputer, server_running: bool) {
        if can_wake(computer, server_running) {
            self.offer = Some(computer.device_id.clone());
        } else if view_reach(computer, server_running).is_ok() {
            self.view = Some((computer.device_id.clone(), computer.name.clone()));
        }
    }

    /// Show the computer `device_id`'s screen, as the tray program's
    /// "Show <name>'s screen" asks (PC6): offered a wake first as Show its
    /// screen does, else said why not.
    pub fn show_asked(&mut self, state_dir: &Path, device_id: &str, server_running: bool) {
        self.read_at = None;
        self.refresh(state_dir);
        let Some(computer) = self
            .file
            .computers
            .iter()
            .find(|c| c.device_id == device_id)
            .cloned()
        else {
            return;
        };
        if let Err(why) = view_reach(&computer, server_running)
            && !can_wake(&computer, server_running)
        {
            self.notice = crate::notice::Notice::error(format!("{why}."));
            return;
        }
        self.show_or_offer(&computer, server_running);
    }

    /// Send the magic packet for the computer `device_id` (PC6).
    pub fn wake(&mut self, state_dir: &Path, device_id: &str, name: &str, now_unix: u64) {
        self.ask(
            state_dir,
            ComputerAsk::Wake {
                device_id: device_id.to_string(),
            },
            now_unix,
            &format!("Sent the wake to {name}. It shows as linked once it is up. {WAKE_NOTE}"),
        );
    }

    /// Wake the computer and show its screen once its link comes, within
    /// [`WAKE_WAIT`] (PC6).
    pub fn wake_and_show(&mut self, state_dir: &Path, device_id: &str, name: &str, now_unix: u64) {
        self.offer = None;
        let ask = ComputerAsk::Wake {
            device_id: device_id.to_string(),
        };
        match computers::request(state_dir, ask, now_unix) {
            Ok(_) => {
                self.waking = Some(Waking {
                    device_id: device_id.to_string(),
                    name: name.to_string(),
                    since: Instant::now(),
                });
                self.read_at = None;
            }
            Err(e) => self.notice = crate::notice::Notice::error(format!("Not asked: {e}")),
        }
    }

    /// The computer being woken to show, if any (PC6).
    pub fn waking(&self) -> Option<&Waking> {
        self.waking.as_ref()
    }

    /// Follow a wake that waits to show (PC6): its window is asked for as
    /// its link comes, or it is said that it did not come in time.
    pub fn tick(&mut self, state_dir: &Path) {
        if self.waking.is_none() {
            return;
        }
        self.refresh(state_dir);
        let Some(waking) = &self.waking else {
            return;
        };
        let linked = self
            .file
            .computers
            .iter()
            .any(|c| c.device_id == waking.device_id && c.link == "linked");
        match waking.step(linked, Instant::now()) {
            WakeStep::Waiting(_) => {}
            WakeStep::Linked => {
                self.view = Some((waking.device_id.clone(), waking.name.clone()));
                self.waking = None;
            }
            WakeStep::TimedOut => {
                self.notice = crate::notice::Notice::error(timed_out(&waking.name));
                self.waking = None;
            }
        }
    }

    /// `computer`'s wake controls (PC6), under its Show its screen: Wake
    /// with its link down and a way known, the offer Show its screen made,
    /// and the countdown of a wake waiting to show, with Cancel.
    pub fn wake_row(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        computer: &PeerComputer,
        server_running: bool,
        now_unix: u64,
    ) {
        let id = computer.device_id.as_str();
        if let Some(waking) = self.waking.as_ref().filter(|w| w.device_id == id) {
            let left = match waking.step(false, Instant::now()) {
                WakeStep::Waiting(left) => left,
                _ => 0,
            };
            let mut cancel = false;
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(waiting_words(&computer.name, left));
                cancel = ui.button("Cancel").clicked();
            });
            if cancel {
                self.waking = None;
            }
            ui.ctx().request_repaint_after(Duration::from_millis(500));
            return;
        }
        if !can_wake(computer, server_running) {
            if self.offer.as_deref() == Some(id) {
                self.offer = None;
            }
            return;
        }
        let (mut wake, mut wake_show, mut show_anyway, mut cancel) = (false, false, false, false);
        if self.offer.as_deref() == Some(id) {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{}'s link is down: it may be asleep.",
                    computer.name
                ));
                wake_show = ui.button(WAKE_AND_SHOW).clicked();
                if view_reach(computer, server_running).is_ok() {
                    show_anyway = ui
                        .button("Show without waking")
                        .on_hover_text("Try its address without the wake.")
                        .clicked();
                }
                cancel = ui.button("Cancel").clicked();
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                wake = ui
                    .button(WAKE)
                    .on_hover_text("Send the Wake-on-LAN packet it told this computer how to send.")
                    .clicked();
            });
        }
        let wifi = computer.wake.as_ref().is_some_and(|w| !w.wired);
        ui.label(RichText::new(wake_note(wifi)).color(crate::theme::muted(ui)));
        let name = computer.name.clone();
        if wake {
            self.wake(state_dir, id, &name, now_unix);
        }
        if wake_show {
            self.wake_and_show(state_dir, id, &name, now_unix);
        }
        if show_anyway {
            self.offer = None;
            self.view = Some((id.to_string(), name));
        } else if cancel {
            self.offer = None;
        }
    }

    /// The computers this one paired with, read again as the list is
    /// (UX 3: the Overview lists them).
    pub fn paired(&mut self, state_dir: &Path) -> &[PeerComputer] {
        self.refresh(state_dir);
        &self.file.computers
    }

    fn refresh(&mut self, state_dir: &Path) {
        let every = if self.pairing.is_some() || self.waking.is_some() {
            PAIRING_REFRESH
        } else {
            REFRESH
        };
        if self.read_at.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        self.file = computers::read(state_dir);
        self.own_id = pocketdesk_core::local::identity::device_id(state_dir);
        self.read_at = Some(Instant::now());
    }

    fn ask(&mut self, state_dir: &Path, ask: ComputerAsk, now_unix: u64, done: &str) {
        match computers::request(state_dir, ask, now_unix) {
            Ok(_) => {
                self.notice = crate::notice::Notice::done(done);
                // Read again soon, for the outcome.
                self.read_at = None;
            }
            Err(e) => self.notice = crate::notice::Notice::error(format!("Not asked: {e}")),
        }
    }

    /// The pairing under way, with Cancel, until the server says how it
    /// went (UX 1).
    fn pairing_progress(&mut self, ui: &mut egui::Ui, state_dir: &Path) {
        let Some(pairing) = &self.pairing else {
            return;
        };
        let answered = self
            .file
            .last_pairing
            .as_ref()
            .is_some_and(|last| last.request == pairing.request);
        if answered {
            self.pairing = None;
            return;
        }
        if pairing.since.elapsed() > PAIRING_WAIT {
            self.notice = crate::notice::Notice::error(format!(
                "The server didn't say how pairing with {} went. Check it's running, then pair again.",
                pairing.name
            ));
            self.pairing = None;
            return;
        }
        let mut cancel = false;
        ui.horizontal_wrapped(|ui| {
            // Clear of the frame's edge, so it is drawn whole.
            ui.add_space(4.0);
            ui.spinner();
            ui.label(format!("Pairing with {}…", pairing.name));
            if pairing.too_late {
                ui.label(RichText::new(TOO_LATE).color(crate::theme::muted(ui)));
            } else {
                cancel = ui.button("Cancel").clicked();
            }
        });
        ui.ctx().request_repaint_after(PAIRING_REFRESH);
        if cancel {
            // The server claims a request before it reads it, so this
            // either takes it back whole or finds it taken (review I1).
            match computers::withdraw(state_dir, &pairing.request) {
                Ok(true) => {
                    self.notice = crate::notice::Notice::done(format!(
                        "Pairing with {} cancelled.",
                        pairing.name
                    ));
                    self.pairing = None;
                }
                // Taken already: it goes on, and its outcome shows here.
                Ok(false) => {
                    if let Some(pairing) = self.pairing.as_mut() {
                        pairing.too_late = true;
                    }
                }
                Err(e) => self.notice = crate::notice::Notice::error(format!("Not cancelled: {e}")),
            }
        }
    }

    /// A Paste whose text never came stops waiting (PR #553 review M2).
    fn expire_paste(&mut self) {
        if self.pasting.is_some_and(|at| at.elapsed() > PASTE_WAIT) {
            self.pasting = None;
        }
    }

    /// Both halves, as one page (the tests' view of them).
    #[cfg(test)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        server_running: bool,
        now_unix: u64,
    ) {
        self.show_pairing(ui, state_dir, server_running, now_unix);
        self.show_list(ui, state_dir, server_running, now_unix);
    }

    /// Pair → Computer's paste half (UX 2): the other computer's link,
    /// Pair, its progress and its outcome.
    pub fn show_pairing(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        server_running: bool,
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        // Left and come back to: the pasted link is not kept (review M1).
        let pass = ui.ctx().cumulative_pass_nr();
        if self.drawn_pass.is_some_and(|at| at + 1 < pass) {
            self.link.clear();
            self.consent = None;
            self.pasting = None;
        }
        self.drawn_pass = Some(pass);
        ui.add_space(12.0);
        ui.label(RichText::new("From the other computer").heading());
        ui.label("Paste the link copied there.");
        // A pairing from the other side times out with no word why here.
        self.public.show(ui);
        // UX 1: shown as pasted, not masked (PC1 re-review m-1 had masked
        // it, as it holds the PIN): a link that cannot be read back cannot
        // be checked. The PIN is short-lived, and the other computer shows
        // it anyway; the box is cleared once Pair is pressed.
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.link)
                    .hint_text("pocketdesk://pair?...")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(420.0),
            );
            if ui
                .button("Paste")
                .on_hover_text("Put the copied link in the box")
                .clicked()
            {
                self.pasting = Some(Instant::now());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            if !self.link.is_empty() && ui.button("Clear").clicked() {
                self.link.clear();
            }
        });
        // The clipboard's text comes as a paste event a frame later, and
        // takes the box's place.
        self.expire_paste();
        if self.pasting.is_some() {
            let pasted = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
            });
            if let Some(text) = pasted {
                self.link = text;
                self.pasting = None;
            }
        }
        // What is sent, as what is checked: without what the paste
        // carried unseen around it.
        let pasted = pocketdesk_core::local::pair_link::clean_link(&self.link).to_string();
        let check = check_link(&pasted, self.own_id.as_deref());
        let parsed = match &check {
            LinkCheck::Valid(link) => Some(link.as_ref().clone()),
            _ => None,
        };
        let empty = check == LinkCheck::Empty;
        match check {
            LinkCheck::Empty | LinkCheck::Valid(_) => {}
            LinkCheck::Own => {
                ui.label(RichText::new(OWN_LINK).color(crate::theme::warn(ui)));
            }
            LinkCheck::CutOff => {
                ui.label(RichText::new(CUT_OFF).color(crate::theme::bad(ui)));
            }
            LinkCheck::NotALink => {
                ui.label(RichText::new(NOT_A_LINK).color(crate::theme::bad(ui)));
            }
        }
        // A yes is for the link it was given for, never a later one.
        if self.consent.as_deref().is_some_and(|l| l != pasted) {
            self.consent = None;
        }
        let asks = parsed.as_ref().is_some_and(|l| l.mine);
        // Both ways needs the other's internet endpoint (PC1 review M4).
        let asks_both = asks && parsed.as_ref().is_some_and(names_endpoint);
        if let Some(link) = &parsed {
            let name = link_name(&link.name);
            // Its name is the link's own word; its addresses show whose it
            // is (PC1 re-review m-1, m-2).
            ui.label(
                RichText::new(format!(
                    "A pairing link from {name}, at {}.",
                    link.addresses.join(", ")
                ))
                .color(crate::theme::good(ui)),
            );
            if asks && !asks_both {
                ui.label(
                    RichText::new(format!(
                        "{name} can pair one way only: its link names no internet endpoint."
                    ))
                    .weak(),
                );
            }
            if asks_both {
                ui.label(
                    RichText::new(format!(
                        "{name} would see and control this computer at any time, without asking. Tick only if it's yours and you copied its link."
                    ))
                    .color(crate::theme::warn(ui)),
                );
                let mut yes = self.consent.is_some();
                if ui.checkbox(&mut yes, CONSENT_BOX).changed() {
                    self.consent = yes.then(|| pasted.clone());
                }
                if !yes {
                    ui.label(
                        RichText::new(format!(
                            "Unticked, it pairs one way: {name} gets no control here."
                        ))
                        .weak(),
                    );
                }
            }
        }
        let both = asks_both && self.consent.is_some();
        let ready = parsed.is_some() && server_running && self.pairing.is_none();
        let pair = match (asks, both) {
            (_, true) => "Pair both ways",
            (true, false) => "Pair one way",
            _ => "Pair",
        };
        if ui
            .add_enabled(ready, egui::Button::new(pair))
            .on_disabled_hover_text(if server_running {
                "Paste the other computer's pairing link"
            } else {
                "Start the server first: it does the pairing"
            })
            .clicked()
        {
            self.link.clear();
            self.consent = None;
            let name = parsed
                .as_ref()
                .map_or_else(String::new, |l| link_name(&l.name));
            let asked = computers::request(
                state_dir,
                ComputerAsk::Pair {
                    link: pasted,
                    mine: both,
                },
                now_unix,
            );
            match asked {
                // UX 1: its progress shows until the outcome comes.
                Ok(request) => {
                    self.pairing = Some(Pairing {
                        request: request.id,
                        name,
                        since: Instant::now(),
                        too_late: false,
                    });
                    self.read_at = None;
                }
                Err(e) => self.notice = crate::notice::Notice::error(format!("Not asked: {e}")),
            }
        }
        self.pairing_progress(ui, state_dir);
        // UX 1: why Pair is off, beside it (a link that is not one, or is
        // this computer's own, is said under the box).
        if self.pairing.is_some() {
            // Said by its progress.
        } else if !server_running {
            crate::widgets::why_off_ask(
                ui,
                PAIR_NEEDS_SERVER,
                crate::widgets::START,
                crate::widgets::Ask::StartServer,
            );
        } else if empty {
            crate::widgets::why_off(ui, PASTE_FIRST, None);
        }
        // Not under a link for another computer (PR #553 review M5).
        if self.pairing.is_none()
            && let Some(last) = &self.file.last_pairing
            && parsed.as_ref().is_none_or(|link| is_of_link(last, link))
        {
            show_outcome(ui, last);
        }
    }

    /// Settings → Other computers' list (UX 2): each computer this one
    /// paired with, its link, Show its screen and Unpair.
    pub fn show_list(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        server_running: bool,
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        if self.file.computers.is_empty() {
            ui.label(RichText::new("No other computer paired yet.").weak());
        }
        let mut forget = None;
        for computer in self.file.computers.clone() {
            let computer = &computer;
            // UX 3: a card per computer, as Devices draws each device.
            crate::theme::card(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&computer.name).strong());
                    ui.label(RichText::new(link_words(computer)).weak());
                    // PC1: paired both ways, with consent given at pairing; the
                    // grant here waits for the link (review M1).
                    if computer.mine && self.file.lapsed.contains(&computer.device_id) {
                        ui.label(RichText::new(MINE_LAPSED).color(crate::theme::warn(ui)))
                            .on_hover_text(MINE_HOVER);
                    } else if computer.mine && self.file.pending.contains(&computer.device_id) {
                        ui.label(RichText::new(MINE_PENDING).color(crate::theme::warn(ui)))
                            .on_hover_text(MINE_HOVER);
                    } else if computer.mine {
                        ui.label(RichText::new(MINE).color(crate::theme::good(ui)))
                            .on_hover_text(MINE_HOVER);
                    }
                    // BK7: its screen in a window here; PC6: or a wake first.
                    let reach = view_reach(computer, server_running);
                    let wakes = can_wake(computer, server_running);
                    if ui
                        .add_enabled(reach.is_ok() || wakes, egui::Button::new("Show its screen"))
                        .on_hover_text(
                            "Its screen in a window here, controlled from it if it lets this computer (its Devices page's Control switch for this one). It may ask its owner first.",
                        )
                        .on_disabled_hover_text(reach.err().unwrap_or_default())
                        .clicked()
                    {
                        self.show_or_offer(computer, server_running);
                    }
                    // UX 1: why, beside it; a stopped server is said once,
                    // under the list.
                    if server_running && reach.is_err() && !wakes {
                        ui.label(
                            RichText::new(unreachable(&computer.name))
                                .color(crate::theme::muted(ui)),
                        );
                    }
                    // U5: "Unpair" for a pairing everywhere, asked by name.
                    if self.forgetting.as_deref() == Some(computer.device_id.as_str()) {
                        ui.label(format!(
                            "Unpair {}? It must pair again to share with this one.",
                            computer.name
                        ));
                        if ui
                            .add_enabled(server_running, egui::Button::new("Unpair"))
                            .on_disabled_hover_text(STOPPED)
                            .clicked()
                        {
                            forget = Some((computer.device_id.clone(), computer.name.clone()));
                        }
                        if ui.button("Cancel").clicked() {
                            self.forgetting = None;
                        }
                    } else if ui
                        // The server does the unpairing (BA3): asked with it
                        // stopped, the request expired unread and the link came
                        // back at its next start, while this said it was done.
                        .add_enabled(server_running, egui::Button::new("Unpair…"))
                        .on_disabled_hover_text(STOPPED)
                        .clicked()
                    {
                        self.forgetting = Some(computer.device_id.clone());
                    }
                });
                self.wake_row(ui, state_dir, computer, server_running, now_unix);
            });
        }
        if !server_running && !self.file.computers.is_empty() {
            crate::widgets::why_off_ask(
                ui,
                LIST_NEEDS_SERVER,
                crate::widgets::START,
                crate::widgets::Ask::StartServer,
            );
        }
        if let Some((device_id, name)) = forget {
            self.forgetting = None;
            // Named, as the app's other notices are (BA3 review).
            self.ask(
                state_dir,
                ComputerAsk::Forget { device_id },
                now_unix,
                &format!(
                    "Asked the server to unpair {name} here: it leaves the list once done. Unpair this computer on {name}'s Devices page too."
                ),
            );
        }
    }
}

/// A pairing's outcome in words (UX 1): what happened, its cause and fix,
/// and whether Run diagnostics may find more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ok: bool,
    pub said: String,
    pub diagnostics: bool,
}

/// What a Cancel after the server took the request says (review I1).
pub const TOO_LATE: &str = "Too late to cancel: the server is already pairing.";

/// What the timeout of a pairing says: the commonest failure.
pub const NO_ANSWER: &str = "The other computer didn't answer. Check both are on the same network and Pocketdesk is running there.";

/// The server's `outcome` of a pairing with `name` (`paired`, or `failed:
/// <why>` in the server's words) as the owner reads it.
pub fn outcome_words(outcome: &str, name: &str) -> Outcome {
    let failed = |said: String, diagnostics: bool| Outcome {
        ok: false,
        said,
        diagnostics,
    };
    if outcome == "paired" {
        return Outcome {
            ok: true,
            said: format!("Paired with {name}."),
            diagnostics: false,
        };
    }
    let why = outcome
        .strip_prefix("failed: ")
        .unwrap_or(outcome)
        .to_lowercase();
    let has = |words: &str| why.contains(words);
    if has("own pairing link") {
        failed(
            "That was this computer's own link: paste the one copied on the other computer."
                .to_string(),
            false,
        )
    } else if has("listed here has that id with another certificate") {
        failed(
            format!(
                "{name} is listed here with another certificate: unpair it first, then pair again."
            ),
            false,
        )
    } else if has("is not the one in the link") {
        failed(
            format!(
                "A different computer answered at {name}'s address: its certificate isn't the one in the link. Copy a fresh link on {name}."
            ),
            true,
        )
    } else if has("refused the code") {
        failed(
            format!(
                "{name} didn't take the code: it was wrong, used already or expired. Copy a fresh link on {name} and paste it again."
            ),
            false,
        )
    } else if has("did not answer the code") {
        failed(
            format!(
                "{name} answered, but not the code in time. Pair again; if it happens again, run diagnostics."
            ),
            true,
        )
    } else if has("refused") {
        failed(
            format!(
                "{name} turned the connection away: Pocketdesk may not be running there, or a firewall blocks it."
            ),
            true,
        )
    } else if has("timeout") || has("timed out") || has("did not answer") {
        failed(NO_ANSWER.to_string(), true)
    } else if has("not a pairing link") || has("names no address") {
        failed(
            format!("The link wasn't a whole one. Copy it again on {name}."),
            false,
        )
    } else {
        failed(format!("Pairing with {name} didn't work."), true)
    }
}

/// Whether `last` is the pairing of `link`'s computer: by its id, or by
/// name for an outcome written before the id was kept.
fn is_of_link(
    last: &computers::PairingOutcome,
    link: &pocketdesk_core::local::pair_link::PairLink,
) -> bool {
    if last.device_id.is_empty() {
        last.name == link.name
    } else {
        last.device_id == link.device_id
    }
}

/// The last pairing's outcome in words (UX 1), with its next step: for a
/// failure the server's own words as details, and Run diagnostics where
/// it may find more.
fn show_outcome(ui: &mut egui::Ui, last: &computers::PairingOutcome) {
    let words = outcome_words(&last.outcome, &link_name(&last.name));
    ui.horizontal_wrapped(|ui| {
        if words.ok {
            ui.label(RichText::new(&words.said).color(crate::theme::good(ui)));
            if ui.button("Set up keyboard and mouse").clicked() {
                crate::widgets::ask(
                    ui.ctx(),
                    crate::widgets::Ask::Go(crate::app::Tab::KeyboardMouse),
                );
            }
        } else {
            ui.label(RichText::new(&words.said).color(crate::theme::bad(ui)));
            if words.diagnostics && ui.button("Run diagnostics").clicked() {
                crate::widgets::ask(ui.ctx(), crate::widgets::Ask::RunDiagnostics);
            }
        }
    });
    if !words.ok {
        ui.label(
            RichText::new(format!("Details: {}", last.outcome))
                .small()
                .color(crate::theme::muted(ui)),
        );
    }
}

/// The box the owner ticks for a link that asks to pair both ways (PC1).
pub const CONSENT_BOX: &str = "That one is mine: no prompts";

/// Whether `link` names an internet endpoint (an `endpointId` in its `ep`),
/// which pairing both ways needs; the server checks it is a real key.
fn names_endpoint(link: &pocketdesk_core::local::pair_link::PairLink) -> bool {
    link.endpoint
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|ep| ep.get("endpointId").and_then(|id| id.as_str()).is_some())
}

/// A computer's name as another computer gave it, fit to show (a link's,
/// or a nearby one's).
pub fn shown(name: &str) -> String {
    link_name(name)
}

/// A link's computer name fit to show: one printable line, else
/// "a computer".
fn link_name(name: &str) -> String {
    pocketdesk_core::protocol::kdeconnect::clean_device_name(name)
        .unwrap_or_else(|| "A computer".to_string())
}

/// Why Pair is off with the server stopped (UX 1).
pub const PAIR_NEEDS_SERVER: &str = "Pair needs the server running: it does the pairing.";
/// Why Pair is off with nothing pasted (UX 1).
pub const PASTE_FIRST: &str = "Paste the link copied on the other computer first.";
/// Why each computer's Show its screen and Unpair are off with the server
/// stopped (UX 1).
pub const LIST_NEEDS_SERVER: &str = "Show its screen and Unpair need the server running.";

/// What the wait for a woken computer says, `left` seconds to go (PC6).
pub fn waiting_words(name: &str, left: u64) -> String {
    format!("Waking {name}: its screen shows once it is up ({left} s)")
}

/// What a wake that waited in vain says (PC6).
pub fn timed_out(name: &str) -> String {
    format!(
        "{name} did not come up within {} s. Is Wake-on-LAN on in its firmware and adapter settings? {WAKE_NOTE}",
        WAKE_WAIT.as_secs()
    )
}

/// The note beside Wake (PC6), naming a Wi-Fi adapter.
pub fn wake_note(wifi: bool) -> String {
    if wifi {
        format!("{WAKE_NOTE} Its adapter is Wi-Fi.")
    } else {
        WAKE_NOTE.to_string()
    }
}

/// Why `name`'s Show its screen is off with the server running (UX 1).
fn unreachable(name: &str) -> String {
    format!("{name} can't be reached now: its link is down and no address of it is known.")
}

/// Why Unpair waits (BA3).
const STOPPED: &str = "Start the server first: it does the unpairing";

/// What a computer paired both ways is (PC1), and how to have it ask again.
pub const MINE: &str = "Mine: sees and controls without asking";
/// Paired both ways, but no link came within ten minutes: one way here
/// until it pairs again (PC1 re-review).
pub const MINE_LAPSED: &str =
    "Paired one way here: no link came from it within ten minutes; pair again for both ways";
/// Paired both ways, but its grant here waits for the link (PC1 review M1).
pub const MINE_PENDING: &str =
    "Mine once linked: until the link comes up, it asks first and does not control this computer";
const MINE_HOVER: &str = "Paired both ways from a link made for another of your computers: each lists the other, and either shows and controls the other with no prompt. To have it ask first again, turn on its Ask before each session (and off its Control switches) on Devices. Unpair takes both ways away.";

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(link: &str) -> PeerComputer {
        PeerComputer {
            device_id: "b".to_string(),
            name: "Desk".to_string(),
            addresses: Vec::new(),
            control_port: 1716,
            stream_port: 4716,
            fingerprint: String::new(),
            endpoint: None,
            paired_unix: 0,
            link: link.to_string(),
            link_unix: 0,
            lets_drive: false,
            mine: false,
            wake: None,
            viewed_unix: 0,
        }
    }

    fn sleeping(wired: bool) -> PeerComputer {
        PeerComputer {
            wake: Some(pocketdesk_core::local::computers::ComputerWake {
                mac: "00:00:5E:00:53:01".to_string(),
                broadcast: std::net::Ipv4Addr::new(192, 0, 2, 255),
                wired,
                told_unix: 1,
            }),
            ..computer("away: the link closed")
        }
    }

    /// PC6: Wake is offered while the link is down, for a computer that
    /// said how, and only with the server running.
    #[test]
    fn wake_shows_with_the_link_down_and_the_facts_known() {
        assert!(can_wake(&sleeping(true), true));
        assert!(!can_wake(&sleeping(true), false), "no server");
        let linked = PeerComputer {
            link: "linked".to_string(),
            ..sleeping(true)
        };
        assert!(!can_wake(&linked, true), "up already");
        assert!(!can_wake(&computer("away: x"), true), "never said");
        assert!(wake_note(true).contains("Wi-Fi rarely wakes"));
        assert!(wake_note(true).ends_with("Its adapter is Wi-Fi."));
        assert_eq!(wake_note(false), WAKE_NOTE);
    }

    /// PC6: a wake that waits to show counts down 60 s, shows as the link
    /// comes, and gives up after.
    #[test]
    fn a_wake_waits_a_minute_for_the_link() {
        let since = Instant::now();
        let waking = Waking {
            device_id: "b".to_string(),
            name: "Desk".to_string(),
            since,
        };
        assert_eq!(waking.step(false, since), WakeStep::Waiting(60));
        assert_eq!(
            waking.step(false, since + Duration::from_millis(17_500)),
            WakeStep::Waiting(43)
        );
        assert_eq!(
            waking.step(true, since + Duration::from_secs(20)),
            WakeStep::Linked
        );
        assert_eq!(waking.step(false, since + WAKE_WAIT), WakeStep::TimedOut);
        assert!(waiting_words("Desk", 43).ends_with("(43 s)"));
        assert!(timed_out("Desk").starts_with("Desk did not come up within 60 s"));
    }

    /// PC6: Show its screen on a sleeping computer offers to wake it
    /// first; on one up, it shows.
    #[test]
    fn show_its_screen_offers_a_wake_when_the_link_is_down() {
        let mut view = ComputersView::default();
        view.show_or_offer(&sleeping(true), true);
        assert_eq!(view.offer.as_deref(), Some("b"));
        assert_eq!(view.take_view(), None);
        let mut view = ComputersView::default();
        view.show_or_offer(&computer("linked"), true);
        assert_eq!(view.offer, None);
        assert_eq!(
            view.take_view(),
            Some(("b".to_string(), "Desk".to_string()))
        );
    }

    /// The list alone over `dir`, the server `running`.
    fn list(
        dir: &std::path::Path,
        view: ComputersView,
    ) -> egui_kittest::Harness<'static, ComputersView> {
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.to_path_buf();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show_list(ui, &state, true, now),
            view,
        );
        harness.run_steps(2);
        harness
    }

    /// PC6: Settings → Other computers offers Wake for a sleeping computer,
    /// with the note on Wi-Fi, and Wake asks the server.
    #[test]
    fn wake_asks_the_server_from_the_list() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("wake");
        computers::update(&dir, |file| file.computers.push(sleeping(false))).unwrap();
        let mut harness = list(&dir, ComputersView::default());
        let noted = harness
            .query_by_label_contains("Its adapter is Wi-Fi")
            .is_some();
        let unreachable = harness
            .query_by_label_contains("can't be reached now")
            .is_some();
        harness.get_by_label(WAKE).click();
        harness.run_steps(2);
        let asked: Vec<ComputerAsk> =
            computers::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .into_iter()
                .map(|r| r.ask)
                .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(noted);
        assert!(!unreachable, "it can be woken");
        assert_eq!(
            asked,
            [ComputerAsk::Wake {
                device_id: "b".to_string()
            }]
        );
    }

    /// PC6: Show its screen on a sleeping computer offers "Wake it and
    /// show", which asks the wake and counts down with Cancel.
    #[test]
    fn wake_it_and_show_counts_down_with_cancel() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("wake-show");
        computers::update(&dir, |file| file.computers.push(sleeping(true))).unwrap();
        let mut harness = list(&dir, ComputersView::default());
        harness.get_by_label("Show its screen").click();
        harness.run_steps(2);
        harness.get_by_label(WAKE_AND_SHOW).click();
        harness.run_steps(2);
        let counting = harness
            .query_by_label_contains("Waking Desk: its screen shows once it is up")
            .is_some();
        let waking = harness.state().waking().map(|w| w.device_id.clone());
        let asked = computers::take_all(&dir, pocketdesk_core::local::pin::now_unix()).len();
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        let cancelled = harness.state().waking().is_none();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(counting);
        assert_eq!(waking.as_deref(), Some("b"));
        assert_eq!(asked, 1);
        assert!(cancelled);
    }

    /// PC6: the tray's Show asks for a computer's window, offers a wake
    /// for one asleep, and says why for one that cannot be reached.
    #[test]
    fn the_trays_show_shows_offers_or_says_why() {
        let dir = temp("show-asked");
        computers::update(&dir, |file| {
            file.computers.push(sleeping(true));
            file.computers.push(PeerComputer {
                device_id: "up".to_string(),
                ..computer("linked")
            });
            file.computers.push(PeerComputer {
                device_id: "gone".to_string(),
                ..computer("away: the link closed")
            });
        })
        .unwrap();
        let mut view = ComputersView::default();
        view.show_asked(&dir, "b", true);
        let offered = view.offer.clone();
        view.show_asked(&dir, "up", true);
        let shown = view.take_view();
        view.show_asked(&dir, "gone", true);
        let said = view.take_notice().is_some();
        view.show_asked(&dir, "nobody", true);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(offered.as_deref(), Some("b"));
        assert_eq!(shown, Some(("up".to_string(), "Desk".to_string())));
        assert!(said);
        assert_eq!(view.take_view(), None);
    }

    /// PC6: a woken computer whose link comes up has its window asked for.
    #[test]
    fn a_woken_computer_is_shown_as_its_link_comes() {
        let dir = temp("woken");
        computers::update(&dir, |file| file.computers.push(sleeping(true))).unwrap();
        let mut view = ComputersView::default();
        view.wake_and_show(&dir, "b", "Desk", pocketdesk_core::local::pin::now_unix());
        view.tick(&dir);
        assert_eq!(view.take_view(), None, "still asleep");
        computers::update(&dir, |file| file.computers[0].link = "linked".to_string()).unwrap();
        view.read_at = None;
        view.tick(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            view.take_view(),
            Some(("b".to_string(), "Desk".to_string()))
        );
        assert!(view.waking().is_none());
    }

    /// PC6: a computer's last view reads as how long ago; never viewed,
    /// nothing.
    #[test]
    fn the_last_view_reads_as_how_long_ago() {
        let viewed = |at: u64| PeerComputer {
            viewed_unix: at,
            ..computer("linked")
        };
        assert_eq!(viewed_words(&viewed(0), 10_000), None);
        assert_eq!(
            viewed_words(&viewed(10_000 - 7_300), 10_000).as_deref(),
            Some("viewed 2 h ago")
        );
    }

    #[test]
    fn a_links_state_reads_as_words() {
        assert_eq!(link_words(&computer("linked")), "linked");
        assert_eq!(link_words(&computer("")), "connecting…");
        assert_eq!(
            link_words(&computer("away: iroh dial timed out")),
            "not linked (iroh dial timed out)"
        );
        assert!(link_words(&computer("refused")).starts_with("refused"));
    }

    /// BK7: a computer's screen is asked for through the running server,
    /// for a computer linked or with an address.
    #[test]
    fn its_screen_can_be_asked_for_when_it_can_be_reached() {
        assert!(view_reach(&computer("linked"), true).is_ok());
        assert!(view_reach(&computer("linked"), false).is_err(), "no server");
        assert!(
            view_reach(&computer("away: the link closed"), true).is_err(),
            "no link and no address"
        );
        let at_home = PeerComputer {
            addresses: vec!["192.0.2.10".into()],
            ..computer("away: the link closed")
        };
        assert!(
            view_reach(&at_home, true).is_ok(),
            "tried on the home network"
        );
    }

    /// A link for another of the owner's computers, as its Pair page copies
    /// it with the box ticked.
    fn mine_link() -> String {
        mine_link_with(Some(
            r#"{"endpointId":"made-up","relayUrls":[],"directAddrs":[]}"#.to_string(),
        ))
    }

    /// [`mine_link`] with `endpoint` as its internet endpoint.
    fn mine_link_with(endpoint: Option<String>) -> String {
        pocketdesk_core::local::pair_link::PairLink {
            name: "Test Tower".to_string(),
            device_id: "00000000-0000-4000-8000-0000000000f1".to_string(),
            addresses: vec!["192.0.2.70".to_string(), "192.0.2.71".to_string()],
            control_port: 1716,
            stream_port: 4716,
            pin: "123456".to_string(),
            fingerprint: Some("ab".repeat(32)),
            endpoint,
            mine: true,
        }
        .to_uri()
    }

    /// The Pair button of the page with `link` pasted, after ticking the
    /// consent first when `consent`; what it asked the server.
    fn paired(link: String, consent: bool) -> ComputerAsk {
        use egui_kittest::kittest::Queryable;
        let dir = std::env::temp_dir().join(format!(
            "pd-computers-view-{consent}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show(ui, &state, true, now),
            ComputersView {
                link,
                ..ComputersView::default()
            },
        );
        harness.run_steps(2);
        // What it asks for is said before anything is pressed, by name and
        // by the addresses the link names (PC1 re-review m-1, m-2): a link
        // wearing one of the owner's computers' names is told by them.
        assert!(
            harness
                .query_by_label_contains(
                    "Test Tower would see and control this computer at any time, without asking"
                )
                .is_some()
        );
        assert!(
            harness
                .query_by_label_contains("at 192.0.2.70, 192.0.2.71")
                .is_some()
        );
        let pair = if consent {
            harness.get_by_label(CONSENT_BOX).click();
            harness.run_steps(2);
            "Pair both ways"
        } else {
            "Pair one way"
        };
        harness.get_by_label(pair).click();
        harness.run_steps(2);
        let asked = computers::take_all(&dir, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(asked.len(), 1, "{asked:?}");
        asked[0].ask.clone()
    }

    /// PC1 review C1: a pasted link that asks to pair both ways says so by
    /// the computer's name and what it would be allowed, and pairs both
    /// ways only once the owner here ticks that it is their own; else it
    /// pairs one way.
    #[test]
    fn a_link_asking_for_both_ways_needs_this_owners_yes() {
        assert_eq!(
            paired(mine_link(), false),
            ComputerAsk::Pair {
                link: mine_link(),
                mine: false
            }
        );
        assert_eq!(
            paired(mine_link(), true),
            ComputerAsk::Pair {
                link: mine_link(),
                mine: true
            }
        );
    }

    /// PC1 re-review m-3: a link that asks for both ways but names no
    /// internet endpoint can pair only one way, and says so before Pair:
    /// no box to tick, and the button says one way.
    #[test]
    fn a_link_without_an_endpoint_says_it_pairs_one_way() {
        use egui_kittest::kittest::Queryable;
        let dir =
            std::env::temp_dir().join(format!("pd-computers-view-no-road-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.clone();
        let link = mine_link_with(None);
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show(ui, &state, true, now),
            ComputersView {
                link: link.clone(),
                ..ComputersView::default()
            },
        );
        harness.run_steps(2);
        let said = harness
            .query_by_label_contains("can pair one way only: its link names no internet endpoint")
            .is_some();
        let no_box = harness.query_by_label(CONSENT_BOX).is_none();
        harness.get_by_label("Pair one way").click();
        harness.run_steps(2);
        let asked = computers::take_all(&dir, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said && no_box);
        assert_eq!(
            asked.iter().map(|r| r.ask.clone()).collect::<Vec<_>>(),
            [ComputerAsk::Pair { link, mine: false }]
        );
    }

    /// UX 1: what is pasted is checked as it is pasted, by this
    /// computer's own id too.
    #[test]
    fn a_pasted_link_is_checked_as_it_is_pasted() {
        assert_eq!(check_link("  ", Some("me")), LinkCheck::Empty);
        assert_eq!(
            check_link("https://example.org/", Some("me")),
            LinkCheck::NotALink
        );
        assert_eq!(
            check_link("pocketdesk://pair?v=1", Some("me")),
            LinkCheck::CutOff,
            "begun, but not a whole link"
        );
        let link = mine_link();
        assert_eq!(
            check_link(&link[..link.len() / 2], Some("me")),
            LinkCheck::CutOff
        );
        let link = mine_link();
        assert!(matches!(
            check_link(&link, Some("me")),
            LinkCheck::Valid(l) if l.name == "Test Tower"
        ));
        assert_eq!(
            check_link(&link, Some("00000000-0000-4000-8000-0000000000f1")),
            LinkCheck::Own
        );
    }

    /// The page over `dir`, the server `running`, with `link` in the box.
    fn page(
        dir: &std::path::Path,
        running: bool,
        link: &str,
    ) -> egui_kittest::Harness<'static, ComputersView> {
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.to_path_buf();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, view: &mut ComputersView| view.show(ui, &state, running, now),
            ComputersView {
                link: link.to_string(),
                ..ComputersView::default()
            },
        );
        harness.run_steps(2);
        harness
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-computers-view-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// UX 1: the box shows what was pasted (it was a password box, all
    /// dots), and the line under it says what it is.
    #[test]
    fn the_pasted_link_shows_and_says_what_it_is() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let dir = temp("shows");
        let link = mine_link();
        let harness = page(&dir, true, &link);
        assert!(
            harness
                .query_all_by_role(egui::accesskit::Role::PasswordInput)
                .next()
                .is_none(),
            "not masked"
        );
        let shown = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("the link box");
        assert_eq!(
            shown.accesskit_node().value().as_deref(),
            Some(link.as_str())
        );
        assert!(
            harness
                .query_by_label_contains("A pairing link from Test Tower")
                .is_some()
        );
        let harness = page(&dir, true, "https://example.org/");
        assert!(harness.query_by_label(NOT_A_LINK).is_some());
        assert!(
            harness.get_by_label("Pair").accesskit_node().is_disabled(),
            "not a link, no Pair"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UX 1: this computer's own link is said, and cannot be paired with.
    #[test]
    fn this_computers_own_link_is_said() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let dir = temp("own");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(pocketdesk_core::local::paths::DEVICE_ID_FILE),
            "00000000-0000-4000-8000-0000000000f1",
        )
        .unwrap();
        let harness = page(&dir, true, &mine_link());
        let own = harness.query_by_label(OWN_LINK).is_some();
        let off = harness.get_by_label("Pair").accesskit_node().is_disabled();
        let consent = harness.query_by_label(CONSENT_BOX).is_some();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(own && off && !consent);
    }

    /// PR #553 review M1: the link, which holds the other computer's PIN,
    /// goes once the page is left; M2: a Paste that brought nothing stops
    /// waiting, so a later paste event is not taken for it.
    #[test]
    fn the_link_goes_when_the_page_is_left() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let dir = temp("left");
        let now = pocketdesk_core::local::pin::now_unix();
        let state = dir.clone();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, (shown, view): &mut (bool, ComputersView)| {
                if *shown {
                    view.show(ui, &state, true, now);
                }
            },
            (
                true,
                ComputersView {
                    link: mine_link(),
                    ..ComputersView::default()
                },
            ),
        );
        harness.run_steps(2);
        harness.state_mut().0 = false;
        harness.run_steps(2);
        harness.state_mut().0 = true;
        harness.run_steps(2);
        let shown = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("the link box")
            .accesskit_node()
            .value()
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(shown, "");
        let mut view = ComputersView {
            pasting: Some(Instant::now() - PASTE_WAIT - Duration::from_millis(1)),
            ..ComputersView::default()
        };
        view.expire_paste();
        assert!(view.pasting.is_none());
    }

    /// UX 1: Paste puts the clipboard's text in the box, in place of what
    /// was there.
    #[test]
    fn paste_fills_the_box() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let dir = temp("paste");
        let mut harness = page(&dir, true, "old");
        harness.get_by_label("Paste").click();
        harness.run_steps(2);
        harness.event(egui::Event::Paste(format!("{}  ", mine_link())));
        harness.run_steps(2);
        let shown = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("the link box");
        assert_eq!(
            shown.accesskit_node().value().as_deref(),
            Some(format!("{}  ", mine_link()).as_str())
        );
        assert!(
            harness
                .query_by_label_contains("A pairing link from Test Tower")
                .is_some()
        );
        harness.get_by_label("Clear").click();
        harness.run_steps(2);
        assert!(harness.query_by_label_contains("A pairing link").is_none());
    }

    /// UX 1: Pair, off, says why beside it, with Start when the server is
    /// the reason; the fix asks the app.
    #[test]
    fn pair_says_why_it_is_off() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("why");
        let harness = page(&dir, true, "");
        assert!(harness.query_by_label(PASTE_FIRST).is_some());
        let mut harness = page(&dir, false, &mine_link());
        assert!(harness.query_by_label(PAIR_NEEDS_SERVER).is_some());
        harness.get_by_label(crate::widgets::START).click();
        harness.run_steps(2);
        assert_eq!(
            crate::widgets::take_ask(&harness.ctx),
            Some(crate::widgets::Ask::StartServer)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// UX 1: a computer that cannot be reached says so beside its greyed
    /// Show its screen.
    #[test]
    fn show_its_screen_says_why_it_is_off() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("reach");
        computers::update(&dir, |file| {
            file.computers.push(computer("away: the link closed"));
        })
        .unwrap();
        let harness = page(&dir, true, "");
        let said = harness
            .query_by_label_contains("Desk can't be reached now")
            .is_some();
        let harness = page(&dir, false, "");
        let stopped = harness.query_by_label(LIST_NEEDS_SERVER).is_some();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said && stopped);
    }

    /// UX 1: each known failure reads as its cause and its fix, the
    /// network ones with Run diagnostics.
    #[test]
    fn a_pairings_outcome_reads_as_cause_and_fix() {
        let said = |raw: &str| outcome_words(raw, "Test Tower");
        let paired = said("paired");
        assert!(paired.ok && paired.said == "Paired with Test Tower.");
        let timeout =
            said("failed: it did not answer (192.0.2.70: timeout after 8000ms during connect)");
        assert_eq!(timeout.said, NO_ANSWER);
        assert!(!timeout.ok && timeout.diagnostics);
        let refused = said(
            "failed: it did not answer (192.0.2.70: connection to 192.0.2.70:1716 failed: No connection could be made because the target machine actively refused it. (os error 10061))",
        );
        assert!(refused.said.contains("turned the connection away") && refused.diagnostics);
        let code = said("failed: it refused the code (a wrong or used PIN)");
        assert!(code.said.contains("didn't take the code") && !code.diagnostics);
        assert!(
            code.said.contains("expired"),
            "the server says expired so too"
        );
        let cert = said(
            "failed: the computer at 192.0.2.70 is not the one in the link (another certificate)",
        );
        assert!(cert.said.contains("certificate isn't the one in the link") && cert.diagnostics);
        let own = said("failed: that is this computer's own pairing link");
        assert!(own.said.contains("this computer's own link"));
        let other = said("failed: something new");
        assert!(other.said.contains("didn't work") && other.diagnostics);
    }

    /// The page as [`page`] makes it, with what the app was asked.
    fn pressed(
        harness: &egui_kittest::Harness<'static, ComputersView>,
    ) -> Option<crate::widgets::Ask> {
        crate::widgets::take_ask(&harness.ctx)
    }

    /// UX 1: Pair shows its progress until the server says how it went,
    /// then the cause and fix of a failure, with the raw words as details.
    #[test]
    fn pairing_shows_its_progress_then_its_outcome_in_words() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("progress");
        let mut harness = page(&dir, true, &mine_link());
        harness.get_by_label("Pair one way").click();
        harness.run_steps(2);
        let waiting = harness.query_by_label("Pairing with Test Tower…").is_some()
            && harness.query_by_label("Cancel").is_some();
        // The server takes the request and answers it.
        let now = pocketdesk_core::local::pin::now_unix();
        let taken = computers::take_all(&dir, now);
        let raw = "failed: it did not answer (192.0.2.70: timeout after 8000ms during connect)";
        computers::update(&dir, |file| {
            file.last_pairing = Some(computers::PairingOutcome {
                name: "Test Tower".to_string(),
                outcome: raw.to_string(),
                unix: now,
                request: taken[0].id.clone(),
                device_id: String::new(),
            });
        })
        .unwrap();
        std::thread::sleep(PAIRING_REFRESH + Duration::from_millis(100));
        harness.run_steps(2);
        let done = harness.query_by_label("Pairing with Test Tower…").is_none();
        let words = harness.query_by_label(NO_ANSWER).is_some();
        let details = harness.query_by_label(&format!("Details: {raw}")).is_some();
        harness.get_by_label("Run diagnostics").click();
        harness.run_steps(2);
        let asked = pressed(&harness);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(waiting && done && words && details);
        assert_eq!(asked, Some(crate::widgets::Ask::RunDiagnostics));
    }

    /// UX 1: Cancel takes back a request the server has not taken.
    #[test]
    fn cancel_takes_the_pairing_back() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("cancel");
        let mut harness = page(&dir, true, &mine_link());
        harness.get_by_label("Pair one way").click();
        harness.run_steps(2);
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        let gone = harness.query_by_label("Pairing with Test Tower…").is_none();
        let now = pocketdesk_core::local::pin::now_unix();
        let left = computers::take_all(&dir, now);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(gone);
        assert!(left.is_empty(), "{left:?}");
    }

    /// PR #553 review I1: Cancel after the server took the request says
    /// it is too late, and the page keeps waiting for that outcome.
    #[test]
    fn cancel_too_late_keeps_waiting_for_the_outcome() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("too-late");
        let mut harness = page(&dir, true, &mine_link());
        harness.get_by_label("Pair one way").click();
        harness.run_steps(2);
        let now = pocketdesk_core::local::pin::now_unix();
        let taken = computers::take_all(&dir, now);
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        let late = harness.query_by_label_contains(TOO_LATE).is_some();
        let waiting = harness
            .query_by_label_contains("Pairing with Test Tower")
            .is_some();
        let no_cancel = harness.query_by_label("Cancel").is_none();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(taken.len(), 1);
        assert!(late && waiting && no_cancel);
    }

    /// PR #553 review M5: the last pairing's outcome is not shown under a
    /// link for another computer.
    #[test]
    fn the_last_outcome_is_not_shown_for_another_computer() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("stale");
        computers::update(&dir, |file| {
            file.last_pairing = Some(computers::PairingOutcome {
                name: "Test Tower".to_string(),
                outcome:
                    "failed: it did not answer (192.0.2.70: timeout after 8000ms during connect)"
                        .to_string(),
                unix: 0,
                request: String::new(),
                device_id: "00000000-0000-4000-8000-0000000000f1".to_string(),
            });
        })
        .unwrap();
        let other = pocketdesk_core::local::pair_link::PairLink {
            name: "Test Desk".to_string(),
            device_id: "00000000-0000-4000-8000-0000000000f2".to_string(),
            addresses: vec!["192.0.2.80".to_string()],
            control_port: 1716,
            stream_port: 4716,
            pin: "654321".to_string(),
            fingerprint: Some("cd".repeat(32)),
            endpoint: None,
            mine: false,
        }
        .to_uri();
        let empty = page(&dir, true, "").query_by_label(NO_ANSWER).is_some();
        let same = page(&dir, true, &mine_link())
            .query_by_label(NO_ANSWER)
            .is_some();
        let another = page(&dir, true, &other).query_by_label(NO_ANSWER).is_some();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(empty && same && !another);
    }

    /// UX 1: a pairing that worked says so, with the next step.
    #[test]
    fn a_pairing_that_worked_offers_the_next_step() {
        use egui_kittest::kittest::Queryable;
        let dir = temp("worked");
        computers::update(&dir, |file| {
            file.last_pairing = Some(computers::PairingOutcome {
                name: "Test Tower".to_string(),
                outcome: "paired".to_string(),
                unix: 0,
                request: String::new(),
                device_id: String::new(),
            });
        })
        .unwrap();
        let mut harness = page(&dir, true, "");
        let said = harness.query_by_label("Paired with Test Tower.").is_some();
        harness.get_by_label("Set up keyboard and mouse").click();
        harness.run_steps(2);
        let asked = pressed(&harness);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(said);
        assert_eq!(
            asked,
            Some(crate::widgets::Ask::Go(crate::app::Tab::KeyboardMouse))
        );
    }

    #[test]
    fn only_a_pairing_link_can_be_sent() {
        assert!(looks_like_link("  pocketdesk://pair?v=1&id=b "));
        // The owner's first live test: a paste that began with a
        // byte-order mark, and one with a zero-width space and a line break.
        let bom = char::from_u32(0xFEFF).unwrap();
        let zwsp = char::from_u32(0x200B).unwrap();
        assert!(looks_like_link(&format!("{bom}pocketdesk://pair?v=1&id=b")));
        assert!(looks_like_link(&format!(
            "{zwsp}pocketdesk://pair?v=1&id=b{zwsp}{}",
            char::from_u32(10).unwrap()
        )));
        assert!(!looks_like_link("https://example.org/"));
        assert!(!looks_like_link(""));
    }
}
