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

/// How long Clear's question waits for its answer.
const CONFIRM: Duration = Duration::from_secs(30);

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
    /// Open Pair: nothing is paired to copy with (E5).
    Pair,
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

/// The texts shown, newest first as kept: those holding every word of
/// `search` in the text or where it came from (J4).
pub fn shown<'a>(entries: &'a [ClipEntry], search: &str, devices: &[Device]) -> Vec<&'a ClipEntry> {
    entries
        .iter()
        .filter(|e| {
            search.trim().is_empty()
                || crate::words::holds_all(search, &[&e.text, &origin(e, devices)])
        })
        .collect()
}

#[derive(Default)]
pub struct ClipboardView {
    entries: Vec<ClipEntry>,
    read_at: Option<Instant>,
    /// Clear's question asked, when: it goes after [`CONFIRM`] unanswered
    /// (D3 review: an armed Clear had stayed armed for good).
    confirm_clear: Option<Instant>,
    /// What the Search box holds (J4).
    search: String,
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
                // Asked as the other forgettings are, with what it forgets
                // (E5): the button had turned into "Clear: sure?" for 5 s.
                if self.confirm_clear.is_some() {
                    ui.label(format!(
                        "Forget all {} texts kept here?",
                        self.entries.len()
                    ));
                    if ui.button("Clear").clicked() {
                        action = Some(match clip_history::clear(state_dir) {
                            Ok(()) => {
                                self.entries.clear();
                                Action::Done("Cleared.".to_string())
                            }
                            // Kept on screen: they are still kept (D3 review).
                            Err(e) => Action::Failed(format!("Not cleared: {e}")),
                        });
                        self.confirm_clear = None;
                    } else if ui.button("Cancel").clicked() || crate::keys::escaped(ui) {
                        self.confirm_clear = None;
                    }
                } else if ui.button("Clear history…").clicked() {
                    self.confirm_clear = Some(Instant::now());
                }
            }
        });
        if self.entries.is_empty() {
            ui.label(
                RichText::new("Nothing yet: copy something on a connected phone, or here with \"Clipboard to phones\" on.")
                    .color(crate::theme::muted(ui)),
            );
            // The next step, with its button (E5).
            if devices.is_empty() {
                if ui.button("Pair a phone").clicked() {
                    action = Some(Action::Pair);
                }
            } else if ui.button("Clipboard settings").clicked() {
                action = Some(Action::OpenSettings);
            }
            return action;
        }
        crate::search_box::show(ui, &mut self.search);
        ui.add_space(4.0);
        let shown = shown(&self.entries, &self.search, devices);
        let query = self.search.trim().to_string();
        if shown.is_empty() && !query.is_empty() {
            // J4: a search that finds nothing says so, with the way back.
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("No kept text holds \"{query}\"."))
                        .color(crate::theme::muted(ui)),
                );
                if ui.button("Clear search").clicked() {
                    self.search.clear();
                }
            });
            return action;
        }
        let targets = targets(devices);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in shown {
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

    /// J4: Search finds a text by its words or where it came from.
    #[test]
    fn a_search_finds_a_text_or_its_origin() {
        let devices = [Device {
            id: "phone-a".to_string(),
            name: Some("Pixel 8".to_string()),
            ..Default::default()
        }];
        let entries = [
            ClipEntry::new("12 Main Street", Some("phone-a"), 2),
            ClipEntry::new("the meeting link", None, 1),
        ];
        let texts =
            |found: Vec<&ClipEntry>| found.iter().map(|e| e.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(shown(&entries, "", &devices)).len(), 2);
        assert_eq!(
            texts(shown(&entries, "main STREET", &devices)),
            ["12 Main Street"]
        );
        assert_eq!(
            texts(shown(&entries, "pixel", &devices)),
            ["12 Main Street"]
        );
        assert_eq!(
            texts(shown(&entries, "this computer link", &devices)),
            ["the meeting link"]
        );
        assert!(shown(&entries, "nowhere", &devices).is_empty());
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
