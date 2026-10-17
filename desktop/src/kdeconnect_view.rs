//! Settings → KDE Connect devices (3a, 3b): look for devices that run KDE
//! Connect (the server tells the network this computer is here for a
//! minute), the devices that have connected, and pairing with them: a
//! device's request waits here with the code both screens show, for Allow
//! or Deny; Pair asks a device from here. Ping and Unpair once paired.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{self as devices, KdeAsk, Pending};

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

/// How long one "Find" asks the server to look.
const FIND_SECS: u64 = 60;

#[derive(Default)]
pub struct KdeConnectView {
    seen: Vec<SeenDevice>,
    pending: Vec<Pending>,
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
        self.pending = devices::pending(state_dir, now_unix);
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
                            "Looking for a minute: open KDE Connect on the device.".to_string(),
                        );
                    }
                    Err(e) => self.notice = Some(format!("Not asked: {e}")),
                }
            }
        });
        let mut ask = None;
        for pending in &self.pending {
            ui.horizontal(|ui| {
                if pending.from_device {
                    ui.label(
                        RichText::new(format!(
                            "{} asks to pair. It shows the code {}: allow it only if it does.",
                            pending.name,
                            spaced(&pending.code)
                        ))
                        .strong(),
                    );
                    if ui.button("Allow").clicked() {
                        ask = Some(KdeAsk::Allow {
                            device_id: pending.device_id.clone(),
                        });
                    }
                    if ui.button("Deny").clicked() {
                        ask = Some(KdeAsk::Deny {
                            device_id: pending.device_id.clone(),
                        });
                    }
                } else {
                    ui.label(format!(
                        "Waiting for {} to accept: it shows the code {}.",
                        pending.name,
                        spaced(&pending.code)
                    ));
                }
            });
        }
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
                let device_id = device.device_id.clone();
                if device.paired {
                    if ui.add_enabled(running, egui::Button::new("Ping")).clicked() {
                        ask = Some(KdeAsk::Ping {
                            device_id: device_id.clone(),
                        });
                    }
                    if ui.button("Unpair").clicked() {
                        ask = Some(KdeAsk::Unpair { device_id });
                    }
                } else if ui
                    .add_enabled(running, egui::Button::new("Pair"))
                    .on_hover_text("Asks the device; accept it there if it shows the same code")
                    .clicked()
                {
                    ask = Some(KdeAsk::Pair { device_id });
                }
            });
        }
        if let Some(ask) = ask {
            self.notice = Some(match devices::request(state_dir, ask, now_unix) {
                Ok(_) => "Asked.".to_string(),
                Err(e) => format!("Not asked: {e}"),
            });
            // Read again soon, for the answer.
            self.read_at = None;
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).weak());
        }
    }
}

/// A code as two groups of four, easier to compare by eye.
pub fn spaced(code: &str) -> String {
    if code.len() == 8 && code.is_ascii() {
        format!("{} {}", &code[..4], &code[4..])
    } else {
        code.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_reads_in_two_groups() {
        assert_eq!(spaced("1ACBAC75"), "1ACB AC75");
        assert_eq!(spaced("odd"), "odd");
    }

    #[test]
    fn a_time_reads_in_words() {
        assert_eq!(ago(100, 90), "just now");
        assert_eq!(ago(1000, 100), "15 min ago");
        assert_eq!(ago(10_000, 100), "2 h ago");
        assert_eq!(ago(300_000, 100), "3 days ago");
        assert_eq!(ago(100, 200), "just now", "a clock behind is not negative");
    }
}
