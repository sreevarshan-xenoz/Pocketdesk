//! Settings → KDE Connect devices (3a, 3b): look for devices that run KDE
//! Connect (the server tells the network this computer is here for a
//! minute), the devices that have connected, and pairing with them: a
//! device's request waits here with the code both screens show, for Allow
//! or Deny; Pair asks a device from here, and its acceptance waits for
//! Confirm with the same code. Once paired (3c): its battery, what it may
//! do (the clipboard, its battery, ringing it, sending files here), Ping,
//! Ring and Unpair.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{
    self as devices, KdeAsk, KdeDevice, KdePermission, KdePermissions, Pending,
};
use pocketdesk_core::local::phone_status::{self, PhoneBattery};
use std::collections::BTreeMap;

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

/// How long one "Find" asks the server to look.
const FIND_SECS: u64 = 60;

#[derive(Default)]
pub struct KdeConnectView {
    rows: Vec<Row>,
    batteries: BTreeMap<String, PhoneBattery>,
    pending: Vec<Pending>,
    finding: bool,
    read_at: Option<Instant>,
    notice: Option<String>,
    /// Switches clicked, shown as clicked until the server has written
    /// them (or a while has passed): a read in between would flip them
    /// back (3c review).
    held: Vec<Held>,
}

/// A switch clicked and not yet written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub device_id: String,
    pub permission: KdePermission,
    pub on: bool,
    pub since: Instant,
}

/// How long a clicked switch is held when the server does not write it.
const HOLD: Duration = Duration::from_secs(10);

/// Show `held` over `rows`, and keep those still to be written.
pub fn hold(rows: &mut [Row], held: &mut Vec<Held>, now: Instant) {
    held.retain(|h| {
        if now.duration_since(h.since) >= HOLD {
            return false;
        }
        let Some(permissions) = rows
            .iter_mut()
            .find(|r| r.device_id == h.device_id)
            .and_then(|r| r.permissions.as_mut())
        else {
            return false;
        };
        if permissions.allows(h.permission) == h.on {
            // Written.
            return false;
        }
        *permissions = permissions.with(h.permission, h.on);
        true
    });
}

/// A device in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub device_id: String,
    pub name: String,
    pub paired: bool,
    /// When it last connected, if it is in the seen list.
    pub seen_unix: Option<u64>,
    /// What it may do, when paired.
    pub permissions: Option<KdePermissions>,
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
            permissions: Some(d.permissions),
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
            permissions: None,
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
        hold(&mut self.rows, &mut self.held, Instant::now());
        self.batteries = phone_status::read(state_dir);
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
        for device in &mut self.rows {
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
                    if let Some(battery) = self.batteries.get(&device_id) {
                        ui.label(crate::app::battery_text(Some(battery), now_unix))
                            .on_hover_text("Its battery, as it last reported it");
                    }
                    if ui.add_enabled(running, egui::Button::new("Ping")).clicked() {
                        ask = Some(KdeAsk::Ping {
                            device_id: device_id.clone(),
                        });
                    }
                    let may_ring = device.permissions.is_some_and(|p| p.find);
                    if ui
                        .add_enabled(running && may_ring, egui::Button::new("Ring"))
                        .on_hover_text("Rings it, to find it")
                        .on_disabled_hover_text(if running {
                            "Turn on \"Ring it from here\" for it"
                        } else {
                            "Start the server with KDE Connect devices on"
                        })
                        .clicked()
                    {
                        ask = Some(KdeAsk::Ring {
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
            if let Some(permissions) = device.permissions.as_mut() {
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    if let Some((permission, on)) = permission_boxes(ui, running, permissions) {
                        ask = Some(KdeAsk::Permission {
                            device_id: device.device_id.clone(),
                            permission,
                            on,
                        });
                    }
                });
            }
        }
        if let Some(ask) = ask {
            let said = asked(&ask);
            let switch = match &ask {
                KdeAsk::Permission {
                    device_id,
                    permission,
                    on,
                } => Some(Held {
                    device_id: device_id.clone(),
                    permission: *permission,
                    on: *on,
                    since: Instant::now(),
                }),
                _ => None,
            };
            self.notice = Some(match devices::request(state_dir, ask, now_unix) {
                Ok(_) => {
                    self.held.extend(switch);
                    said.to_string()
                }
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

/// A paired device's switches; what they are now, when one was changed.
fn permission_boxes(
    ui: &mut egui::Ui,
    running: bool,
    permissions: &mut KdePermissions,
) -> Option<(KdePermission, bool)> {
    let mut changed = None;
    for (which, on, label, hover) in [
        (
            KdePermission::Clipboard,
            &mut permissions.clipboard,
            "Clipboard",
            "Its copies here, and this computer's to it (with \"Clipboard to phones\" on)",
        ),
        (
            KdePermission::Battery,
            &mut permissions.battery,
            "Battery",
            "Its battery, and a warning when it runs low",
        ),
        (
            KdePermission::Find,
            &mut permissions.find,
            "Ring it from here",
            "Ring rings it, to find it",
        ),
        (
            KdePermission::Files,
            &mut permissions.files,
            "Send files here",
            "Files it sends go to Downloads",
        ),
    ] {
        if ui
            .add_enabled(running, egui::Checkbox::new(on, label))
            .on_hover_text(hover)
            .on_disabled_hover_text("Start the server with KDE Connect devices on")
            .changed()
        {
            changed = Some((which, *on));
        }
    }
    changed
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
            permissions: KdePermissions {
                files: true,
                ..Default::default()
            },
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
            list[0].permissions.is_some_and(|p| p.files),
            "its own switches"
        );
        assert_eq!(list[1].permissions, None, "none for a device not paired");
        assert!(
            !list[1].paired,
            "unpaired while away, though the seen list says paired"
        );
        let list = rows(&[paired("p", "Phone")], &[]);
        assert_eq!(list[0].seen_unix, None, "kept though no longer seen");
    }

    #[test]
    fn a_clicked_switch_is_shown_until_it_is_written() {
        let start = Instant::now();
        let read = || rows(&[paired("p", "Phone")], &[]);
        let mut held = vec![Held {
            device_id: "p".to_string(),
            permission: KdePermission::Files,
            on: false,
            since: start,
        }];
        // Read before the server wrote it: shown as clicked.
        let mut shown = read();
        hold(&mut shown, &mut held, start);
        assert!(!shown[0].permissions.unwrap().files);
        assert_eq!(held.len(), 1);
        // Written: let go.
        let mut written = read();
        written[0].permissions = written[0]
            .permissions
            .map(|p| p.with(KdePermission::Files, false));
        hold(&mut written, &mut held, start);
        assert!(held.is_empty());
        // Never written: let go after a while, and the file shown.
        let mut held = vec![Held {
            device_id: "p".to_string(),
            permission: KdePermission::Files,
            on: false,
            since: start,
        }];
        let mut late = read();
        hold(&mut late, &mut held, start + HOLD);
        assert!(held.is_empty() && late[0].permissions.unwrap().files);
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
