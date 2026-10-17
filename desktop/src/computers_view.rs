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
    /// The computer whose Forget waits for a second click.
    forgetting: Option<String>,
    notice: Option<String>,
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

/// Whether `text` looks like a pairing link (the server checks it fully).
pub fn looks_like_link(text: &str) -> bool {
    text.trim().starts_with("pocketdesk://pair?")
}

impl ComputersView {
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
                self.notice = Some(done.to_string());
                // Read again soon, for the outcome.
                self.read_at = None;
            }
            Err(e) => self.notice = Some(format!("Not asked: {e}")),
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
                self.ask(state_dir, ComputerAsk::Pair { link }, now_unix, "Pairing…");
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
            ui.horizontal(|ui| {
                ui.label(RichText::new(&computer.name).strong());
                ui.label(RichText::new(link_words(computer)).weak());
                let asked = self.forgetting.as_deref() == Some(computer.device_id.as_str());
                let label = if asked {
                    "Forget it? Click again"
                } else {
                    "Forget"
                };
                if ui.button(label).clicked() {
                    if asked {
                        forget = Some(computer.device_id.clone());
                    } else {
                        self.forgetting = Some(computer.device_id.clone());
                    }
                }
            });
        }
        if let Some(device_id) = forget {
            self.forgetting = None;
            self.ask(
                state_dir,
                ComputerAsk::Forget { device_id },
                now_unix,
                "Forgotten here. Remove this computer in the other one's Devices tab too.",
            );
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).weak());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn computer(link: &str) -> PeerComputer {
        PeerComputer {
            device_id: "b".to_string(),
            name: "Desk".to_string(),
            addresses: Vec::new(),
            control_port: 1716,
            fingerprint: String::new(),
            endpoint: None,
            paired_unix: 0,
            link: link.to_string(),
            link_unix: 0,
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

    #[test]
    fn only_a_pairing_link_can_be_sent() {
        assert!(looks_like_link("  pocketdesk://pair?v=1&id=b "));
        assert!(!looks_like_link("https://example.org/"));
        assert!(!looks_like_link(""));
    }
}
