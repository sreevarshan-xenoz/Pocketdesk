//! Settings → KDE Connect devices (3a, 3b): look for devices that run KDE
//! Connect (the server tells the network this computer is here for a
//! minute), the devices that have connected, and pairing with them: a
//! device's request waits here with the code both screens show, for Allow
//! or Deny; Pair asks a device from here, and its acceptance waits for
//! Confirm with the same code. Ping and Unpair once paired.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{self as devices, KdeAsk, KdeDevice, Pending};

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

/// How long one "Find" asks the server to look.
const FIND_SECS: u64 = 60;

#[derive(Default)]
pub struct KdeConnectView {
    rows: Vec<Row>,
    pending: Vec<Pending>,
    finding: bool,
    read_at: Option<Instant>,
    notice: Option<String>,
}

/// A device in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub device_id: String,
    pub name: String,
    pub paired: bool,
    /// When it last connected, if it is in the seen list.
    pub seen_unix: Option<u64>,
}

/// The list: every paired device, from the pins (3b review: the seen list
/// keeps only the latest few, so a paired device could drop out of it and
/// take its Unpair with it, and it said "paired" after an unpairing made
/// with the device away), then the others seen, newest first.
pub fn rows(paired: &[KdeDevice], seen: &[SeenDevice]) -> Vec<Row> {
    let seen_at = |id: &str| seen.iter().find(|d| d.device_id == id).map(|d| d.seen_unix);
    let mut rows: Vec<Row> = paired
        .iter()
        .map(|d| Row {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            paired: true,
            seen_unix: seen_at(&d.device_id),
        })
        .collect();
    let mut others: Vec<Row> = seen
        .iter()
        .filter(|d| !paired.iter().any(|p| p.device_id == d.device_id))
        .map(|d| Row {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            paired: false,
            seen_unix: Some(d.seen_unix),
        })
        .collect();
    others.sort_by_key(|r| std::cmp::Reverse(r.seen_unix));
    rows.extend(others);
    rows
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
        self.rows = rows(&devices::devices(state_dir), &kdeconnect::read(state_dir));
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
                    if answer(ui, running, "Allow") {
                        ask = Some(KdeAsk::Allow {
                            device_id: pending.device_id.clone(),
                            code: pending.code.clone(),
                        });
                    }
                    if answer(ui, running, "Deny") {
                        ask = Some(KdeAsk::Deny {
                            device_id: pending.device_id.clone(),
                            code: pending.code.clone(),
                        });
                    }
                } else if pending.accepted {
                    ui.label(
                        RichText::new(format!(
                            "{} accepted. Confirm only if it showed the code {}.",
                            pending.name,
                            spaced(&pending.code)
                        ))
                        .strong(),
                    );
                    if answer(ui, running, "Confirm") {
                        ask = Some(KdeAsk::Allow {
                            device_id: pending.device_id.clone(),
                            code: pending.code.clone(),
                        });
                    }
                    if answer(ui, running, "Cancel") {
                        ask = Some(KdeAsk::Deny {
                            device_id: pending.device_id.clone(),
                            code: pending.code.clone(),
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
        if self.rows.is_empty() {
            ui.label(RichText::new("None has connected yet.").weak());
        }
        for device in &self.rows {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&device.name).strong());
                let paired = if device.paired {
                    "paired"
                } else {
                    "not paired"
                };
                ui.label(
                    RichText::new(match device.seen_unix {
                        Some(at) => format!("{paired}, seen {}", ago(now_unix, at)),
                        None => paired.to_string(),
                    })
                    .weak(),
                );
                let device_id = device.device_id.clone();
                if device.paired {
                    if ui.add_enabled(running, egui::Button::new("Ping")).clicked() {
                        ask = Some(KdeAsk::Ping {
                            device_id: device_id.clone(),
                        });
                    }
                    if answer(ui, running, "Unpair") {
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
            let said = asked(&ask);
            self.notice = Some(match devices::request(state_dir, ask, now_unix) {
                Ok(_) => said.to_string(),
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

/// A button that leaves the server a request: off while it is not running
/// (the request would lapse unread); whether it was pressed.
fn answer(ui: &mut egui::Ui, running: bool, label: &str) -> bool {
    ui.add_enabled(running, egui::Button::new(label))
        .on_disabled_hover_text("Start the server with KDE Connect devices on")
        .clicked()
}

/// What the app says once it has asked: a pairing or a ping goes only to
/// a device connected now.
fn asked(ask: &KdeAsk) -> &'static str {
    match ask {
        KdeAsk::Pair { .. } | KdeAsk::Ping { .. } => {
            "Asked: the device must be connected now, with KDE Connect open."
        }
        _ => "Asked.",
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

    fn paired(id: &str, name: &str) -> KdeDevice {
        KdeDevice {
            device_id: id.to_string(),
            name: name.to_string(),
            cert_hash: String::new(),
            paired_unix: 1,
            permissions: Default::default(),
        }
    }

    fn seen(id: &str, at: u64, said_paired: bool) -> SeenDevice {
        SeenDevice {
            device_id: id.to_string(),
            name: format!("seen {id}"),
            seen_unix: at,
            paired: said_paired,
        }
    }

    #[test]
    fn the_list_is_the_paired_devices_then_the_others_seen() {
        let list = rows(
            &[paired("p", "Phone")],
            &[
                seen("a", 5, false),
                seen("gone", 9, true),
                seen("p", 7, true),
            ],
        );
        let ids: Vec<_> = list.iter().map(|r| r.device_id.as_str()).collect();
        assert_eq!(ids, ["p", "gone", "a"]);
        assert!(list[0].paired && list[0].seen_unix == Some(7));
        assert!(
            !list[1].paired,
            "unpaired while away, though the seen list says paired"
        );
        let list = rows(&[paired("p", "Phone")], &[]);
        assert_eq!(list[0].seen_unix, None, "kept though no longer seen");
    }

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
