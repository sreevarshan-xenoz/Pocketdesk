//! The Phone tab (T10g): the folders a phone's owner shared, browsed from
//! here. A list or a download is a request the server hands to the phone;
//! the phone's answer is the last listing the server keeps, read again
//! every second while the tab is open.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use pocketdesk_core::local::phone_files::{self as store, Kept, Kind};

use crate::model::Device;

const REFRESH: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct PhoneFilesView {
    /// The phone browsed.
    phone: Option<String>,
    listings: BTreeMap<String, Kept>,
    read_at: Option<Instant>,
    notice: Option<String>,
}

/// A size in words.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

impl PhoneFilesView {
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.listings = store::read(state_dir);
        self.read_at = Some(Instant::now());
    }

    fn ask(
        &mut self,
        state_dir: &Path,
        phone: &str,
        kind: Kind,
        root: u32,
        path: Vec<String>,
        now_unix: u64,
    ) {
        match store::request(state_dir, phone, kind, root, &path, now_unix) {
            Ok(_) => {
                self.notice = Some(match kind {
                    Kind::List => "Asked the phone…".to_string(),
                    Kind::Fetch => format!(
                        "Asked the phone to send {}; it arrives in Downloads.",
                        path.last().map(String::as_str).unwrap_or("it")
                    ),
                });
                self.read_at = None;
            }
            Err(e) => self.notice = Some(format!("Not asked: {e}")),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, devices: &[Device], now_unix: u64) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(REFRESH);
        let phones: Vec<&Device> = devices.iter().filter(|d| d.takes_files).collect();
        if phones.is_empty() {
            ui.label(
                RichText::new(
                    "No phone is connected on this network. Open Pocketdesk on the phone; it \
                     shares the folders its owner adds under Settings, \"Folders the computer may open\".",
                )
                .color(Color32::GRAY),
            );
            return;
        }
        if self
            .phone
            .as_ref()
            .is_none_or(|p| !phones.iter().any(|d| &d.id == p))
        {
            self.phone = Some(phones[0].id.clone());
        }
        let phone = self.phone.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            for d in &phones {
                if ui
                    .selectable_label(d.id == phone, crate::app::short(&d.id))
                    .clicked()
                {
                    self.phone = Some(d.id.clone());
                }
            }
        });
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).italics());
        }
        let kept = self.listings.get(&phone).cloned();
        let Some(Kept { listing, .. }) = kept else {
            if ui.button("Show the phone's shared folders").clicked() {
                self.ask(state_dir, &phone, Kind::List, 0, Vec::new(), now_unix);
            }
            return;
        };
        if let Some(error) = &listing.error {
            ui.label(RichText::new(error).color(Color32::from_rgb(0xd9, 0x5c, 0x4a)));
        }
        if listing.roots.is_empty() {
            ui.label(
                RichText::new(
                    "The phone shares no folder yet: add one in its Settings, \"Folders the computer may open\".",
                )
                .color(Color32::GRAY),
            );
        }
        ui.horizontal_wrapped(|ui| {
            for root in &listing.roots {
                let here = listing.root == Some(root.id) && listing.path.is_empty();
                if ui.selectable_label(here, &root.name).clicked() {
                    self.ask(state_dir, &phone, Kind::List, root.id, Vec::new(), now_unix);
                }
            }
            if ui.button("Refresh").clicked()
                && let Some(root) = listing.root
            {
                self.ask(
                    state_dir,
                    &phone,
                    Kind::List,
                    root,
                    listing.path.clone(),
                    now_unix,
                );
            }
        });
        let Some(root) = listing.root else {
            return;
        };
        let root_name = listing
            .roots
            .iter()
            .find(|r| r.id == root)
            .map(|r| r.name.clone())
            .unwrap_or_default();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(
                std::iter::once(root_name.as_str())
                    .chain(listing.path.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" / "),
            );
            if !listing.path.is_empty() && ui.button("Up").clicked() {
                let mut up = listing.path.clone();
                up.pop();
                self.ask(state_dir, &phone, Kind::List, root, up, now_unix);
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            if listing.entries.is_empty() {
                ui.label(RichText::new("Empty").color(Color32::GRAY));
            }
            for entry in &listing.entries {
                ui.horizontal(|ui| {
                    let mut path = listing.path.clone();
                    path.push(entry.name.clone());
                    if entry.dir {
                        if ui.link(format!("{}/", entry.name)).clicked() {
                            self.ask(state_dir, &phone, Kind::List, root, path, now_unix);
                        }
                    } else {
                        ui.label(&entry.name);
                        ui.label(RichText::new(size_text(entry.size)).color(Color32::GRAY));
                        if ui.small_button("Download").clicked() {
                            self.ask(state_dir, &phone, Kind::Fetch, root, path, now_unix);
                        }
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::size_text;

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MB");
    }
}
