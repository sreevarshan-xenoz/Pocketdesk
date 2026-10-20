//! The Messages tab (roadmap N5b): texts phones forwarded, and replies sent
//! back through them. The server keeps the conversation; this view reads it
//! and leaves replies in the outbox for the server to hand over.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::sms::{self, Direction, Message, ReplyRequest, ReplyState};
use pocketdesk_core::protocol::permissions::Capability;

use crate::model::Device;

/// How often the conversation is read again while the tab is open.
const REFRESH: Duration = Duration::from_secs(2);

/// One line in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub body: String,
    pub at_unix: u64,
    pub outgoing: bool,
    /// For a reply: where it stands, in words.
    pub status: Option<String>,
}

/// The messages of one conversation, oldest first, with the replies still
/// waiting in the outbox at the end.
pub fn thread(
    messages: &[Message],
    pending: &[ReplyRequest],
    device: &str,
    address: &str,
) -> Vec<Row> {
    let mut rows: Vec<Row> = messages
        .iter()
        .filter(|m| m.device_id == device && m.address == address)
        .map(|m| Row {
            body: m.body.clone(),
            at_unix: m.at_unix,
            outgoing: m.direction == Direction::Reply,
            status: m.reply.as_ref().map(|r| status_text(&r.state)),
        })
        .collect();
    rows.extend(
        pending
            .iter()
            .filter(|r| r.device_id == device && r.address == address)
            .map(|r| Row {
                body: r.body.clone(),
                at_unix: r.created_unix,
                outgoing: true,
                status: Some("waiting for the phone to connect".to_string()),
            }),
    );
    rows
}

/// A conversation's title: the sender's contact name and number when the
/// phone shared the name (T8e), else the number.
pub fn title(address: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{name} ({address})"),
        None => address.to_string(),
    }
}

/// A reply's state, as the tab says it.
pub fn status_text(state: &ReplyState) -> String {
    match state {
        ReplyState::Sending { .. } => "sending…".to_string(),
        ReplyState::Sent => "sent".to_string(),
        ReplyState::Failed { reason } => format!("not sent: {reason}"),
    }
}

/// The phones that may send texts from here: paired, with Texts allowed.
pub fn texting_phones(devices: &[Device]) -> Vec<&Device> {
    devices
        .iter()
        .filter(|d| d.permissions.allows(Capability::Texts))
        .collect()
}

/// A conversation's key: its phone and number; None for a new text.
type Key = Option<(String, String)>;

/// The draft of each conversation and of a new text (I3): a reply typed for
/// one conversation never goes to another when the selection moves.
#[derive(Default)]
struct Drafts(HashMap<Key, String>);

impl Drafts {
    fn of(&mut self, key: &Key) -> &mut String {
        self.0.entry(key.clone()).or_default()
    }

    fn clear(&mut self, key: &Key) {
        self.0.remove(key);
    }
}

#[derive(Default)]
pub struct MessagesView {
    /// The open conversation: phone and number.
    selected: Key,
    drafts: Drafts,
    /// A new conversation's number and phone.
    new_number: String,
    new_phone: Option<String>,
    messages: Vec<Message>,
    pending: Vec<ReplyRequest>,
    read_at: Option<Instant>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
}

impl MessagesView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }
    fn refresh(&mut self, state_dir: &Path) {
        if self.read_at.is_some_and(|at| at.elapsed() < REFRESH) {
            return;
        }
        self.messages = sms::read(state_dir);
        self.pending = sms::pending(state_dir);
        self.read_at = Some(Instant::now());
    }

    /// Queue the draft of [key]; whether it was queued.
    fn send(
        &mut self,
        state_dir: &Path,
        key: &Key,
        device: &str,
        address: &str,
        now_unix: u64,
    ) -> bool {
        let body = self.drafts.of(key).clone();
        match sms::queue_reply(state_dir, device, address, &body, now_unix) {
            Ok(_) => {
                self.drafts.clear(key);
                self.read_at = None;
                // In place of an earlier failure's (E5 review).
                self.notice = crate::notice::Notice::done("Queued: the phone sends it.");
                true
            }
            Err(e) => {
                self.notice = crate::notice::Notice::error(e.to_string());
                false
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path, devices: &[Device], now_unix: u64) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(REFRESH);
        let conversations = sms::conversations(&self.messages);
        let muted = crate::theme::muted(ui);
        ui.columns(2, |columns| {
            let list = &mut columns[0];
            if list.button("New text").clicked() {
                self.selected = None;
            }
            list.add_space(6.0);
            if conversations.is_empty() {
                list.label(
                    RichText::new(
                        "No texts yet. A phone shares its texts once its owner turns on \
                         \"Show texts on my computer\" in Pocketdesk's Settings on the phone.",
                    )
                    .color(muted),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("conversations")
                .show(list, |ui| {
                    for c in &conversations {
                        let key = (c.device_id.clone(), c.address.clone());
                        let preview: String = c.last_body.chars().take(40).collect();
                        let label = format!("{}\n{preview}", title(&c.address, c.name.as_deref()));
                        if ui
                            .selectable_label(self.selected.as_ref() == Some(&key), label)
                            .clicked()
                        {
                            self.selected = Some(key);
                        }
                    }
                });

            let pane = &mut columns[1];
            match self.selected.clone() {
                Some((device, address)) => {
                    let key = Some((device.clone(), address.clone()));
                    let name = conversations
                        .iter()
                        .find(|c| c.device_id == device && c.address == address)
                        .and_then(|c| c.name.clone());
                    pane.horizontal(|ui| {
                        ui.strong(title(&address, name.as_deref()));
                        // L4: the phone's dialer opens with the number.
                        if pocketdesk_core::protocol::dial::clean_number(&address).is_some()
                            && ui
                                .button("Call")
                                .on_hover_text(
                                    "Open the phone's dialer with this number; you press Call there",
                                )
                                .clicked()
                        {
                            self.notice = crate::notice::Notice::outcome(
                                pocketdesk_core::local::dials::request(
                                    state_dir, &device, &address, now_unix,
                                )
                                .map(|_| {
                                    "Asked the phone to open its dialer with the number; it does with \u{201c}Calls from the computer\u{201d} on.".to_string()
                                })
                                .map_err(|e| format!("Not asked: {e}")),
                            );
                        }
                    });
                    pane.label(
                        RichText::new(format!(
                            "through {}",
                            crate::model::device_label(devices, &device)
                        ))
                            .color(muted),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("thread")
                        .max_height(320.0)
                        .stick_to_bottom(true)
                        .show(pane, |ui| {
                            for row in thread(&self.messages, &self.pending, &device, &address) {
                                let text = if row.outgoing {
                                    RichText::new(format!("You: {}", row.body)).strong()
                                } else {
                                    RichText::new(&row.body)
                                };
                                ui.label(text);
                                if let Some(status) = row.status {
                                    ui.label(RichText::new(status).small().color(muted));
                                }
                                ui.add_space(4.0);
                            }
                        });
                    pane.add_space(6.0);
                    let reply_box = pane.add(
                        egui::TextEdit::multiline(self.drafts.of(&key))
                            .desired_rows(2)
                            .return_key(Some(crate::keys::NEW_LINE))
                            .hint_text("Reply (Enter sends, Shift+Enter starts a new line)"),
                    );
                    // Enter sends, as in a chat; Shift+Enter is a new line (I3).
                    let sent_by_key = crate::keys::enter_in(pane, &reply_box)
                        && !self.drafts.of(&key).trim().is_empty();
                    if sent_by_key
                        || pane
                        .add_enabled(!self.drafts.of(&key).trim().is_empty(), egui::Button::new("Send reply"))
                        .clicked()
                    {
                        self.send(state_dir, &key, &device, &address, now_unix);
                    }
                }
                None => {
                    pane.strong("New text");
                    let phones = texting_phones(devices);
                    if phones.is_empty() {
                        pane.label("No paired phone may send texts (Devices: Show its texts and send replies).");
                    } else {
                        let chosen = self
                            .new_phone
                            .clone()
                            .filter(|id| phones.iter().any(|d| &d.id == id))
                            .unwrap_or_else(|| phones[0].id.clone());
                        egui::ComboBox::from_id_salt("sms-phone")
                            .selected_text(crate::model::device_label(devices, &chosen))
                            .show_ui(pane, |ui| {
                                for phone in &phones {
                                    let label = crate::model::device_label(devices, &phone.id);
                                    if ui.selectable_label(phone.id == chosen, label).clicked() {
                                        self.new_phone = Some(phone.id.clone());
                                    }
                                }
                            });
                        pane.add(egui::TextEdit::singleline(&mut self.new_number).hint_text("Number"));
                        pane.add(
                            egui::TextEdit::multiline(self.drafts.of(&None))
                                .desired_rows(2)
                                .hint_text("Message"),
                        );
                        let ready = !self.new_number.trim().is_empty() && !self.drafts.of(&None).trim().is_empty();
                        if pane.add_enabled(ready, egui::Button::new("Send")).clicked() {
                            let number = self.new_number.trim().to_string();
                            if self.send(state_dir, &None, &chosen, &number, now_unix) {
                                self.selected = Some((chosen, number));
                                self.new_number.clear();
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I3: a reply typed for one conversation never goes to another.
    #[test]
    fn a_draft_stays_with_its_conversation() {
        let ann = Some(("p".to_string(), "+1".to_string()));
        let bob = Some(("p".to_string(), "+2".to_string()));
        let mut drafts = Drafts::default();
        drafts.of(&ann).push_str("to Ann");
        assert_eq!(drafts.of(&bob), "", "another conversation starts empty");
        assert_eq!(drafts.of(&None), "", "so does a new text");
        drafts.of(&None).push_str("new");
        assert_eq!(drafts.of(&ann), "to Ann", "and Ann's is kept");
        drafts.clear(&ann);
        assert_eq!(drafts.of(&ann), "");
        assert_eq!(drafts.of(&None), "new", "sending one keeps the others");
    }

    #[test]
    fn a_conversation_is_titled_by_its_contact_name_when_it_has_one() {
        assert_eq!(title("+1 555 0100", None), "+1 555 0100");
        assert_eq!(title("+1 555 0100", Some("Ann")), "Ann (+1 555 0100)");
    }
    use pocketdesk_core::local::sms::Reply;

    fn message(
        device: &str,
        address: &str,
        body: &str,
        at: u64,
        reply: Option<ReplyState>,
    ) -> Message {
        Message {
            device_id: device.into(),
            address: address.into(),
            body: body.into(),
            at_unix: at,
            direction: if reply.is_some() {
                Direction::Reply
            } else {
                Direction::Received
            },
            reply: reply.map(|state| Reply {
                id: format!("r{at}"),
                state,
            }),
            name: None,
        }
    }

    #[test]
    fn a_thread_is_one_phone_and_number_with_waiting_replies_last() {
        let messages = vec![
            message("p", "+1", "hello", 1, None),
            message("p", "+2", "other", 2, None),
            message("p", "+1", "on my way", 3, Some(ReplyState::Sent)),
            message("q", "+1", "other phone", 4, None),
        ];
        let pending = vec![ReplyRequest {
            id: "w".into(),
            device_id: "p".into(),
            address: "+1".into(),
            body: "see you".into(),
            created_unix: 5,
        }];
        let rows = thread(&messages, &pending, "p", "+1");
        let bodies: Vec<_> = rows.iter().map(|r| r.body.as_str()).collect();
        assert_eq!(bodies, ["hello", "on my way", "see you"]);
        assert!(!rows[0].outgoing && rows[1].outgoing && rows[2].outgoing);
        assert_eq!(rows[1].status.as_deref(), Some("sent"));
        assert_eq!(
            rows[2].status.as_deref(),
            Some("waiting for the phone to connect")
        );
    }

    #[test]
    fn a_failed_reply_says_why() {
        assert_eq!(
            status_text(&ReplyState::Failed {
                reason: "no signal".into()
            }),
            "not sent: no signal"
        );
        assert_eq!(
            status_text(&ReplyState::Sending {
                since_unix: 1,
                attempts: 1
            }),
            "sending…"
        );
    }
}
