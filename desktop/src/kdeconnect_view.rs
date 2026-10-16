//! Settings → KDE Connect devices (3a): look for devices that run KDE
//! Connect (the server tells the network this computer is here for a
//! minute), and the devices that have connected. Pairing with one is 3b.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::kdeconnect::{self, SeenDevice};

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

/// How long one "Find" asks the server to look.
const FIND_SECS: u64 = 60;

#[derive(Default)]
pub struct KdeConnectView {
    seen: Vec<SeenDevice>,
    finding: bool,
    read_at: Option<Instant>,
    notice: Option<String>,
}

/// How long ago `then` was, in words.
pub fn ago(now_unix: u64, then: u64) -> String {
    let secs = now_unix.saturating_sub(then);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

impl KdeConnectView {
    fn refresh(&mut self, state_dir: &Path, now_unix: u64) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.seen = kdeconnect::read(state_dir);
        self.finding = kdeconnect::finding(state_dir, now_unix);
        self.read_at = Some(Instant::now());
    }

    /// `running` is whether the server runs with the switch on.
    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, running: bool, now_unix: u64) {
        self.refresh(state_dir, now_unix);
        ui.add_space(12.0);
        ui.label(RichText::new("KDE Connect devices").strong());
        ui.label(
            RichText::new(
                "Phones and computers that run KDE Connect. Open KDE Connect on the device, \
                 on this network, and look for this computer.",
            )
            .weak(),
        );
        ui.horizontal(|ui| {
            let label = if self.finding {
                "Looking for KDE Connect devices…"
            } else {
                "Find KDE Connect devices"
            };
            if ui
                .add_enabled(running && !self.finding, egui::Button::new(label))
                .on_disabled_hover_text(if running {
                    "Looking already"
                } else {
                    "Start the server with KDE Connect devices on"
                })
                .clicked()
            {
                match kdeconnect::ask_to_find(state_dir, now_unix, FIND_SECS) {
                    Ok(()) => {
                        self.finding = true;
                        self.notice = Some(
                            "Looking for a minute: refresh KDE Connect on the device.".to_string(),
                        );
                    }
                    Err(e) => self.notice = Some(format!("Not asked: {e}")),
                }
            }
        });
        if self.seen.is_empty() {
            ui.label(RichText::new("None has connected yet.").weak());
        }
        for device in &self.seen {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&device.name).strong());
                ui.label(
                    RichText::new(format!(
                        "{}, seen {}",
                        if device.paired {
                            "paired"
                        } else {
                            "not paired"
                        },
                        ago(now_unix, device.seen_unix)
                    ))
                    .weak(),
                );
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).weak());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_in_words() {
        assert_eq!(ago(100, 90), "just now");
        assert_eq!(ago(1000, 100), "15 min ago");
        assert_eq!(ago(10_000, 100), "2 h ago");
        assert_eq!(ago(300_000, 100), "3 days ago");
        assert_eq!(ago(100, 200), "just now", "a clock behind is not negative");
    }
}
