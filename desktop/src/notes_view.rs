//! The Notifications tab (T10c): the phones' notifications that can be
//! answered, newest first, each with its buttons and, when it takes one, a
//! reply box. The server keeps them; an answer goes in its outbox.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::answers::{self, Outcome, State};
use pocketdesk_core::local::phone_notes::{self, Answer, PhoneNote};

/// How often the list is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct NotesView {
    notes: Vec<PhoneNote>,
    /// Where each answer stands (E1).
    answers: Vec<Outcome>,
    /// Each notification's reply being typed, by phone and key.
    drafts: HashMap<(String, String), String>,
    read_at: Option<Instant>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
}

/// The notifications to list, newest first.
pub fn newest_first(mut notes: Vec<PhoneNote>) -> Vec<PhoneNote> {
    notes.reverse();
    notes
}

impl NotesView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.notes = newest_first(phone_notes::read(state_dir));
        self.answers = answers::read(state_dir);
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
                self.notice =
                    crate::notice::Notice::done(format!("Queued {what} for {}", note.app));
                self.drafts
                    .remove(&(note.device_id.clone(), note.key.clone()));
            }
            Err(e) => self.notice = crate::notice::Notice::error(e.to_string()),
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        devices: &[crate::model::Device],
        now_unix: u64,
    ) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(REFRESH);
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
                    if note.messages.is_empty() {
                        if !note.text.is_empty() {
                            ui.label(&note.text);
                        }
                    } else {
                        // D4: a chat's recent messages, as a thread.
                        for (line, mine) in thread_lines(&note.messages, note.at_unix, now_unix) {
                            if mine {
                                ui.label(
                                    RichText::new(line).italics().color(crate::theme::muted(ui)),
                                );
                            } else {
                                ui.label(line);
                            }
                        }
                    }
                    ui.label(
                        RichText::new(format!(
                            "from {}",
                            crate::model::device_label(devices, &note.device_id)
                        ))
                        .small()
                        .color(crate::theme::muted(ui)),
                    );
                    // E1: how the last answer to it went.
                    if let Some(outcome) =
                        answers::latest(&self.answers, &note.device_id, &note.key)
                    {
                        let (line, tone) = answer_line(outcome, now_unix);
                        let colour = match tone {
                            Tone::Done => crate::theme::good(ui),
                            Tone::Failed => crate::theme::bad(ui),
                            Tone::Waiting => crate::theme::muted(ui),
                        };
                        ui.label(RichText::new(line).small().color(colour));
                    }
                    ui.horizontal_wrapped(|ui| {
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

/// How an answer's line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Done,
    Failed,
}

/// How long an answer sent to the phone may go unanswered before the page
/// says so: a phone from before E1 never answers.
pub const NO_WORD_SECS: u64 = 60;

/// Where an answer stands, as the page says it (E1).
pub fn answer_line(outcome: &Outcome, now_unix: u64) -> (String, Tone) {
    match &outcome.state {
        State::Waiting => (
            "Your answer: waiting for the phone…".to_string(),
            Tone::Waiting,
        ),
        State::Sent if now_unix.saturating_sub(outcome.at_unix) >= NO_WORD_SECS => (
            "Your answer: sent, but no word back from the phone (an older app on it, or the link dropped)"
                .to_string(),
            Tone::Waiting,
        ),
        State::Sent => ("Your answer: sent to the phone…".to_string(), Tone::Waiting),
        State::Handed => ("Your answer: handed to the app".to_string(), Tone::Done),
        State::NotSent { reason } => (format!("Your answer was not sent: {reason}"), Tone::Failed),
    }
}

/// A chat's messages as the page shows them (D4): "Ann · 5 min ago: text",
/// oldest first, each with whether it is the owner's own ("You"), which the
/// page sets apart by style, not by name alone (a contact may be called
/// "You"). The phone's clock need not agree with this one (C3): a message's
/// age is the note's own (`note_at_unix`, when it came on the phone) plus
/// how much older it is than the newest message.
pub fn thread_lines(
    messages: &[pocketdesk_core::protocol::conversation::Message],
    note_at_unix: u64,
    now_unix: u64,
) -> Vec<(String, bool)> {
    let newest = messages.iter().map(|m| m.at_ms).max().unwrap_or(0);
    messages
        .iter()
        .map(|m| {
            let who = if m.mine {
                "You"
            } else if m.from.is_empty() {
                "Someone"
            } else {
                m.from.as_str()
            };
            let line = if m.at_ms == 0 {
                format!("{who}: {}", m.text)
            } else {
                let at = note_at_unix.saturating_sub((newest - m.at_ms) / 1000);
                format!("{who} · {}: {}", crate::words::ago(now_unix, at), m.text)
            };
            (line, m.mine)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E1: each state in words, and a silent phone said after a minute.
    #[test]
    fn an_answer_s_line_says_where_it_stands() {
        let at = |state: State| Outcome {
            id: "a".into(),
            device_id: "p".into(),
            key: "k".into(),
            state,
            at_unix: 1_000,
        };
        assert_eq!(answer_line(&at(State::Waiting), 1_000).1, Tone::Waiting);
        assert_eq!(
            answer_line(&at(State::Sent), 1_010).0,
            "Your answer: sent to the phone…"
        );
        assert!(
            answer_line(&at(State::Sent), 1_000 + NO_WORD_SECS)
                .0
                .contains("no word")
        );
        assert_eq!(answer_line(&at(State::Handed), 1_000).1, Tone::Done);
        let refused = answer_line(
            &at(State::NotSent {
                reason: "the app refused it".into(),
            }),
            1_000,
        );
        assert_eq!(
            refused,
            (
                "Your answer was not sent: the app refused it".to_string(),
                Tone::Failed
            )
        );
    }

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
            messages: vec![],
        };
        let listed = newest_first(vec![note("old"), note("new")]);
        assert_eq!(listed[0].key, "new");
    }

    /// D4: a chat's thread, the owner's own as "You", aged from the note.
    #[test]
    fn a_chat_reads_as_a_thread() {
        use pocketdesk_core::protocol::conversation::Message;
        let lines = thread_lines(
            &[
                Message {
                    from: "Ann".into(),
                    text: "Lunch?".into(),
                    at_ms: 0,
                    mine: false,
                },
                Message {
                    from: String::new(),
                    text: "Yes".into(),
                    at_ms: 0,
                    mine: true,
                },
            ],
            100,
            100,
        );
        assert_eq!(
            lines,
            [
                ("Ann: Lunch?".to_string(), false),
                ("You: Yes".to_string(), true)
            ]
        );
        // The phone's clock an hour off: the newest is as old as the note,
        // the older one two minutes before it.
        let base = 1_800_000_000_000;
        let timed = thread_lines(
            &[
                Message {
                    from: "Ann".into(),
                    text: "Hi".into(),
                    at_ms: base,
                    mine: false,
                },
                Message {
                    from: "Ann".into(),
                    text: "There".into(),
                    at_ms: base + 120_000,
                    mine: false,
                },
            ],
            10_000,
            10_000,
        );
        assert_eq!(
            timed[1].0,
            format!("Ann · {}: There", crate::words::ago(10_000, 10_000))
        );
        assert_eq!(
            timed[0].0,
            format!("Ann · {}: Hi", crate::words::ago(10_000, 10_000 - 120))
        );
    }
}
