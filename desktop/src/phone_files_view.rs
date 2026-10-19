//! The Phone tab (T10g): the folders a phone's owner shared, browsed from
//! here. A list or a download is a request the server hands to the phone;
//! the phone's answer is the last listing the server keeps, read again
//! every second while the tab is open.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::drive::DriveInfo;
use pocketdesk_core::local::phone_files::{self as store, Kept, Kind};

use crate::model::Device;

const REFRESH: Duration = Duration::from_secs(1);

/// How long a list request waits for the phone's answer before the tab says so.
const ANSWER_SECS: u64 = 15;

/// A listing older than this says how old it is.
const STALE_SECS: u64 = 30;

#[derive(Default)]
pub struct PhoneFilesView {
    /// The phone browsed.
    phone: Option<String>,
    listings: BTreeMap<String, Kept>,
    read_at: Option<Instant>,
    notice: Option<String>,
    /// The phone and the time of the list request still waiting for an answer.
    waiting: Option<(String, u64)>,
    /// Where the server serves the phones' folders as a drive (D8), while
    /// it does.
    drive: Option<DriveInfo>,
    /// When the drive's address was copied, for "Copied.".
    drive_copied: Option<Instant>,
}

/// The drive's address for this computer's file manager (D8), and how to
/// open it there.
pub fn drive_address(info: &DriveInfo) -> (String, &'static str) {
    if cfg!(windows) {
        (
            info.windows_path(),
            "Paste it in File Explorer's address bar, or map it as a network drive. Windows' WebDAV client opens files of up to 50 MB unless its limit is raised.",
        )
    } else {
        (
            info.dav_url(),
            "Open it in Files, Dolphin or another file manager that speaks WebDAV.",
        )
    }
}

/// What a list request waiting since `asked_unix` has come to, given the
/// phone's kept listing time: `Some(true)` answered, `Some(false)` given
/// up on, `None` still waiting.
fn answered(asked_unix: u64, kept_unix: Option<u64>, now_unix: u64) -> Option<bool> {
    if kept_unix.is_some_and(|at| at >= asked_unix) {
        Some(true)
    } else if now_unix.saturating_sub(asked_unix) > ANSWER_SECS {
        Some(false)
    } else {
        None
    }
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
        self.drive = pocketdesk_core::local::drive::read(state_dir);
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
                self.waiting = (kind == Kind::List).then(|| (phone.to_string(), now_unix));
                self.notice = Some(match kind {
                    Kind::List => "Asked the phone…".to_string(),
                    Kind::Fetch => format!(
                        "Asked the phone to send {}; it arrives where received files land.",
                        path.last().map(String::as_str).unwrap_or("it")
                    ),
                });
                self.read_at = None;
            }
            Err(e) => self.notice = Some(format!("Not asked: {e}")),
        }
    }

    /// The phone the page shows (C7: its recent photos follow it).
    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, devices: &[Device], now_unix: u64) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(REFRESH);
        if let Some(info) = &self.drive {
            let (address, how) = drive_address(info);
            ui.label(RichText::new("As a drive, read-only").strong());
            ui.horizontal_wrapped(|ui| {
                ui.monospace(&address);
                if ui.button("Copy").clicked() {
                    ui.ctx().copy_text(address.clone());
                    self.drive_copied = Some(Instant::now());
                }
                crate::notice::copied(ui, &mut self.drive_copied);
            });
            ui.label(RichText::new(how).small().color(crate::theme::muted(ui)));
            ui.add_space(8.0);
        }
        let phones: Vec<&Device> = devices.iter().filter(|d| d.takes_files).collect();
        if phones.is_empty() {
            ui.label(
                RichText::new(
                    "No phone is connected. A phone with folders shared (its Settings, \"Folders the \
                     computer may open\") stays connected to answer, through notification access.",
                )
                .color(crate::theme::muted(ui)),
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
                    .selectable_label(d.id == phone, crate::model::device_label(devices, &d.id))
                    .clicked()
                {
                    self.phone = Some(d.id.clone());
                }
            }
        });
        if let Some((asking, asked_unix)) = self.waiting.clone() {
            let kept_unix = self.listings.get(&asking).map(|k| k.at_unix);
            match answered(asked_unix, kept_unix, now_unix) {
                Some(true) => {
                    self.waiting = None;
                    self.notice = None;
                }
                Some(false) => {
                    self.waiting = None;
                    self.notice = Some(
                        "The phone did not answer. Is it connected, with Files on in Devices, \
                         and Pocketdesk allowed to run in the background on it?"
                            .to_string(),
                    );
                }
                None => {}
            }
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).italics());
        }
        let kept = self.listings.get(&phone).cloned();
        let Some(Kept { listing, at_unix }) = kept else {
            if ui.button("Show the phone's shared folders").clicked() {
                self.ask(state_dir, &phone, Kind::List, 0, Vec::new(), now_unix);
            }
            return;
        };
        let age = now_unix.saturating_sub(at_unix);
        if age > STALE_SECS {
            ui.label(
                RichText::new(format!(
                    "Listed {}: Refresh to see the phone as it is now.",
                    crate::words::ago(now_unix, at_unix)
                ))
                .color(crate::theme::muted(ui)),
            );
        }
        if let Some(error) = &listing.error {
            ui.label(RichText::new(error).color(crate::theme::bad(ui)));
        }
        if listing.roots.is_empty() {
            ui.label(
                RichText::new(
                    "The phone shares no folder yet: add one in its Settings, \"Folders the computer may open\".",
                )
                .color(crate::theme::muted(ui)),
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
        if listing.truncated {
            ui.label(
                RichText::new(format!(
                    "Showing the first {} entries: the folder holds more.",
                    listing.entries.len()
                ))
                .color(crate::theme::muted(ui)),
            );
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            if listing.entries.is_empty() {
                ui.label(RichText::new("Empty").color(crate::theme::muted(ui)));
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
                        ui.label(
                            RichText::new(size_text(entry.size)).color(crate::theme::muted(ui)),
                        );
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
    use super::{ANSWER_SECS, answered, size_text};

    #[test]
    fn a_list_request_is_answered_by_a_newer_listing_or_given_up() {
        assert_eq!(answered(100, Some(100), 101), Some(true));
        assert_eq!(
            answered(100, Some(99), 101),
            None,
            "an older listing is no answer"
        );
        assert_eq!(answered(100, None, 100 + ANSWER_SECS), None);
        assert_eq!(answered(100, Some(50), 101 + ANSWER_SECS), Some(false));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MB");
    }
}
