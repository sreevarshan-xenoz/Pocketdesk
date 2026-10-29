//! Settings → Computers (2f): the owner's other computers this one paired
//! with, how each one's link is doing, and pairing with another from its
//! pairing link (its Pair tab's "Copy the link for another computer"). The
//! server does the pairing and keeps the links; this asks it through
//! `computers_out/`.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile, PeerComputer};

/// How often the file is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

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
}

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

/// Whether "Show its screen" can ask for `computer` now (BK7): the server
/// dials it, so it must run, and the computer must be reachable some way,
/// its link up or an address known. Why not, when it cannot.
pub fn view_reach(computer: &PeerComputer, server_running: bool) -> Result<(), &'static str> {
    if !server_running {
        return Err("Start the server first: it reaches the other computer");
    }
    if computer.link != "linked" && computer.addresses.is_empty() {
        return Err("Not reachable now: its link is down and no address of it is known");
    }
    Ok(())
}

/// Whether `text` looks like a pairing link (the server checks it fully).
pub fn looks_like_link(text: &str) -> bool {
    text.trim().starts_with("pocketdesk://pair?")
}

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
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.file = computers::read(state_dir);
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

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        server_running: bool,
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        // Under Settings' "Other computers" heading (U5).
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "Your other computers. On the other one, open Pair and copy its link for \
                 another computer, then paste it here.",
            )
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.link)
                    .hint_text("pocketdesk://pair?...")
                    .password(true)
                    .desired_width(320.0),
            );
            let ready = looks_like_link(&self.link) && server_running;
            if ui
                .add_enabled(ready, egui::Button::new("Pair"))
                .on_disabled_hover_text(if server_running {
                    "Paste the other computer's pairing link"
                } else {
                    "Start the server first: it does the pairing"
                })
                .clicked()
            {
                let link = std::mem::take(&mut self.link).trim().to_string();
                self.ask(
                    state_dir,
                    ComputerAsk::Pair { link, mine: false },
                    now_unix,
                    "Asked the other computer to pair.",
                );
            }
        });
        if let Some(last) = &self.file.last_pairing {
            let failed = last.outcome.starts_with("failed");
            let text = format!("Last pairing, with {}: {}", last.name, last.outcome);
            ui.label(if failed {
                RichText::new(text).color(crate::theme::bad(ui))
            } else {
                RichText::new(text).weak()
            });
        }
        if self.file.computers.is_empty() {
            ui.label(RichText::new("No other computer paired yet.").weak());
        }
        let mut forget = None;
        for computer in &self.file.computers {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&computer.name).strong());
                ui.label(RichText::new(link_words(computer)).weak());
                // PC1: paired both ways, with consent given at pairing.
                if computer.mine {
                    ui.label(RichText::new(MINE).color(crate::theme::good(ui)))
                        .on_hover_text(MINE_HOVER);
                }
                // BK7: its screen in a window here.
                let reach = view_reach(computer, server_running);
                if ui
                    .add_enabled(reach.is_ok(), egui::Button::new("Show its screen"))
                    .on_hover_text(
                        "Its screen in a window here, controlled from it if it lets this computer (its Devices page's Control switch for this one). It may ask its owner first.",
                    )
                    .on_disabled_hover_text(reach.err().unwrap_or_default())
                    .clicked()
                {
                    self.view = Some((computer.device_id.clone(), computer.name.clone()));
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

/// Why Unpair waits (BA3).
const STOPPED: &str = "Start the server first: it does the unpairing";

/// What a computer paired both ways is (PC1), and how to have it ask again.
pub const MINE: &str = "Mine: sees and controls without asking";
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
        }
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

    #[test]
    fn only_a_pairing_link_can_be_sent() {
        assert!(looks_like_link("  pocketdesk://pair?v=1&id=b "));
        assert!(!looks_like_link("https://example.org/"));
        assert!(!looks_like_link(""));
    }
}
