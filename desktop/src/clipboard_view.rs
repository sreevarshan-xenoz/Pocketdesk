//! The Clipboard page (D3): the texts that crossed between this computer
//! and its phones, either way, newest first, each with Copy and Send to a
//! phone. The server keeps the history (`core::local::clip_history`) while
//! `clipboard_history` asks for any; this page reads it, empties it with
//! Clear, and leaves sends in `clipboard_out/` for the server.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::clip_history::{self, ClipEntry};

use crate::model::Device;

/// How often the history is read again while the page is open.
const REFRESH: Duration = Duration::from_secs(2);

/// How long a first press of Clear waits for the second.
const CONFIRM: Duration = Duration::from_secs(5);

/// The most characters of an entry shown.
const PREVIEW_CHARS: usize = 280;

/// What the page asks the window to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Put this text on this computer's clipboard.
    Copy(String),
    /// Say this (done, or an error).
    Done(String),
    Failed(String),
    /// Open Settings: the history is off.
    OpenSettings,
}

/// An entry's text as the page shows it: its first lines, at most
/// [`PREVIEW_CHARS`] characters, and an ellipsis when there is more.
pub fn preview(entry: &ClipEntry) -> String {
    let lines: Vec<&str> = entry.text.lines().collect();
    let mut shown = lines.iter().take(4).copied().collect::<Vec<_>>().join("\n");
    let mut more = entry.cut || lines.len() > 4;
    if shown.chars().count() > PREVIEW_CHARS {
        shown = shown.chars().take(PREVIEW_CHARS).collect();
        more = true;
    }
    if more {
        shown.push('…');
    }
    shown
}

/// Where an entry came from, in words.
pub fn origin(entry: &ClipEntry, devices: &[Device]) -> String {
    match &entry.from {
        Some(id) => format!("From {}", crate::model::device_label(devices, id)),
        None => "From this computer".to_string(),
    }
}

/// The phones and computers a text can go to now: linked, with "Share the clipboard" on.
pub fn targets(devices: &[Device]) -> Vec<&Device> {
    devices
        .iter()
        .filter(|d| crate::model::clipboard_reach(d).is_ok())
        .collect()
}

#[derive(Default)]
pub struct ClipboardView {
    entries: Vec<ClipEntry>,
    read_at: Option<Instant>,
    /// Clear pressed once, when: a press within [`CONFIRM`] empties the
    /// history (D3 review: an armed Clear had stayed armed for good).
    confirm_clear: Option<Instant>,
}

impl ClipboardView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        devices: &[Device],
        history: u32,
        now_unix: u64,
    ) -> Option<Action> {
        if history == 0 {
            ui.label(
                "The history is off. With it on, the texts that cross between this computer and its phones, either way, are kept here, to copy or send again.",
            );
            return ui
                .button("Turn it on in Settings")
                .clicked()
                .then_some(Action::OpenSettings);
        }
        if self.read_at.is_none_or(|at| at.elapsed() >= REFRESH) {
            self.entries = clip_history::read_limited(state_dir, history);
            self.read_at = Some(Instant::now());
        }
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "The last {history} texts that crossed, newest first."
            ));
            if self.confirm_clear.is_some_and(|at| at.elapsed() >= CONFIRM) {
                self.confirm_clear = None;
            }
            if !self.entries.is_empty() {
                let label = if self.confirm_clear.is_some() {
                    "Clear: sure?"
                } else {
                    "Clear history"
                };
                if ui.button(label).clicked() {
                    if self.confirm_clear.is_some() {
                        action = Some(match clip_history::clear(state_dir) {
                            Ok(()) => {
                                self.entries.clear();
                                Action::Done("Cleared.".to_string())
                            }
                            // Kept on screen: they are still kept (D3 review).
                            Err(e) => Action::Failed(format!("Not cleared: {e}")),
                        });
                        self.confirm_clear = None;
                    } else {
                        self.confirm_clear = Some(Instant::now());
                    }
                }
            }
        });
        if self.entries.is_empty() {
            ui.label(
                RichText::new("Nothing yet: copy something on a connected phone, or here with \"Clipboard to phones\" on.")
                    .color(crate::theme::muted(ui)),
            );
            return action;
        }
        let targets = targets(devices);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &self.entries {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut head = format!(
                        "{} · {}",
                        origin(entry, devices),
                        crate::words::ago(now_unix, entry.at_unix)
                    );
                    if entry.cut {
                        head.push_str(" · shortened, it was too long to keep");
                    }
                    ui.label(RichText::new(head).color(crate::theme::muted(ui)));
                    ui.label(preview(entry));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Copy").clicked() {
                            action = Some(Action::Copy(entry.text.clone()));
                        }
                        // A cut entry is not what was copied (D3 review).
                        let why = if entry.cut {
                            "Kept shortened, so it would not arrive as copied. Use Copy instead."
                        } else {
                            "No phone with \"Share the clipboard\" on (Devices) is linked now."
                        };
                        ui.add_enabled_ui(!targets.is_empty() && !entry.cut, |ui| {
                            ui.menu_button("Send to…", |ui| {
                                for device in &targets {
                                    let name = crate::model::device_label(devices, &device.id);
                                    if ui.button(&name).clicked() {
                                        action = Some(
                                            match clip_history::request_send(
                                                state_dir,
                                                &device.id,
                                                &entry.text,
                                                now_unix,
                                            ) {
                                                // Queued for the server (D3 review).
                                                Ok(_) => Action::Done(format!(
                                                    "Asked {name} to take it."
                                                )),
                                                Err(e) => Action::Failed(format!("Not sent: {e}")),
                                            },
                                        );
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_disabled_hover_text(why);
                        });
                    });
                });
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::permissions::Capability;

    fn entry(text: &str) -> ClipEntry {
        ClipEntry::new(text, None, 1)
    }

    #[test]
    fn a_long_entry_is_shown_short_with_an_ellipsis() {
        assert_eq!(preview(&entry("short")), "short");
        let many = "a\nb\nc\nd\ne\nf";
        assert_eq!(preview(&entry(many)), "a\nb\nc\nd…");
        let long = "x".repeat(PREVIEW_CHARS + 10);
        let shown = preview(&entry(&long));
        assert!(shown.ends_with('…'));
        assert_eq!(shown.chars().count(), PREVIEW_CHARS + 1);
    }

    #[test]
    fn an_entry_says_where_it_came_from() {
        let mut phone = Device {
            id: "phone-a".to_string(),
            name: Some("Ann's phone".to_string()),
            ..Default::default()
        };
        phone.linked = true;
        let devices = vec![phone];
        assert_eq!(
            origin(&ClipEntry::new("t", Some("phone-a"), 1), &devices),
            "From Ann's phone"
        );
        assert_eq!(origin(&entry("t"), &devices), "From this computer");
    }

    #[test]
    fn a_text_goes_only_to_a_linked_device_with_clipboard_on() {
        let mut on = Device {
            id: "on".to_string(),
            linked: true,
            ..Default::default()
        };
        on.permissions.set(Capability::Clipboard, true);
        let mut off = on.clone();
        off.id = "off".to_string();
        off.permissions.set(Capability::Clipboard, false);
        let mut away = on.clone();
        away.id = "away".to_string();
        away.linked = false;
        let devices = vec![on, off, away];
        assert_eq!(
            targets(&devices)
                .iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>(),
            ["on"]
        );
    }
}
