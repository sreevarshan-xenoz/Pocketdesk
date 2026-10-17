//! The Notifications tab (T10c): the phones' notifications that can be
//! answered, newest first, each with its buttons and, when it takes one, a
//! reply box. The server keeps them; an answer goes in its outbox.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::phone_notes::{self, Answer, PhoneNote};

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct NotesView {
    notes: Vec<PhoneNote>,
    /// Each notification's reply being typed, by phone and key.
    drafts: HashMap<(String, String), String>,
    read_at: Option<Instant>,
    notice: Option<String>,
}

/// The notifications to list, newest first.
pub fn newest_first(mut notes: Vec<PhoneNote>) -> Vec<PhoneNote> {
    notes.reverse();
    notes
}

impl NotesView {
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.notes = newest_first(phone_notes::read(state_dir));
        // Drafts for notifications that went are dropped.
        let live: Vec<(String, String)> = self
            .notes
            .iter()
            .map(|n| (n.device_id.clone(), n.key.clone()))
            .collect();
        self.drafts.retain(|k, _| live.contains(k));
        self.read_at = Some(Instant::now());
    }

    fn answer(&mut self, state_dir: &Path, note: &PhoneNote, answer: Answer, now_unix: u64) {
        let what = match &answer {
            Answer::Action(title) => format!("\"{title}\""),
            Answer::Reply(_) => "your reply".to_string(),
        };
        match phone_notes::request(state_dir, &note.device_id, &note.key, answer, now_unix) {
            Ok(_) => {
                // Queued: the server sends it within a minute, once the
                // phone is connected.
                self.notice = Some(format!("Queued {what} for {}", note.app));
                self.drafts
                    .remove(&(note.device_id.clone(), note.key.clone()));
            }
            Err(e) => self.notice = Some(e.to_string()),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, now_unix: u64) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(REFRESH);
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).italics());
        }
        if self.notes.is_empty() {
            ui.label(
                RichText::new(
                    "Notifications from your phone that you can answer appear here: \
                     their buttons, and a reply box for chats. The phone sends them once \
                     \"This phone's notifications\" is on, and lets you answer once \
                     \"Let the computer answer notifications\" is on, in the phone's Settings.",
                )
                .color(crate::theme::muted(ui)),
            );
            return;
        }
        let notes = self.notes.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for note in &notes {
                ui.group(|ui| {
                    ui.strong(format!("{}: {}", note.app, note.title));
                    if !note.text.is_empty() {
                        ui.label(&note.text);
                    }
                    ui.label(
                        RichText::new(format!("from {}", crate::app::short(&note.device_id)))
                            .small()
                            .color(crate::theme::muted(ui)),
                    );
                    ui.horizontal(|ui| {
                        for action in &note.actions {
                            if ui.button(action).clicked() {
                                self.answer(
                                    state_dir,
                                    note,
                                    Answer::Action(action.clone()),
                                    now_unix,
                                );
                            }
                        }
                    });
                    if note.reply {
                        let key = (note.device_id.clone(), note.key.clone());
                        let draft = self.drafts.entry(key.clone()).or_default();
                        ui.add(egui::TextEdit::singleline(draft).hint_text("Reply"));
                        let ready = !draft.trim().is_empty();
                        if ui
                            .add_enabled(ready, egui::Button::new("Send reply"))
                            .clicked()
                        {
                            let message = self.drafts.get(&key).cloned().unwrap_or_default();
                            self.answer(state_dir, note, Answer::Reply(message), now_unix);
                        }
                    }
                });
                ui.add_space(4.0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_notification_is_listed_first() {
        let note = |key: &str| PhoneNote {
            device_id: "phone".into(),
            key: key.into(),
            app: "Chat".into(),
            title: "Ann".into(),
            text: String::new(),
            at_unix: 0,
            actions: vec![],
            reply: true,
        };
        let listed = newest_first(vec![note("old"), note("new")]);
        assert_eq!(listed[0].key, "new");
    }
}
