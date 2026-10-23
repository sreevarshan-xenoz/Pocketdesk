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
    /// P10: the contacts' search, and the lists as last read.
    contacts_query: String,
    contacts:
        std::collections::BTreeMap<String, pocketdesk_core::local::phone_contacts::ContactList>,
    /// When the phone was asked for its contacts, to say when no list came.
    contacts_asked: Option<(String, u64)>,
    /// The search's last answer: phone, list's time, words, and the found
    /// contacts' places (P10 review: not filtered again each frame).
    contacts_found: Option<(String, u64, String, Vec<usize>)>,
}

/// Whether `phone` may do `capability` by the trust store, yes when it
/// cannot be read (the server holds it to it anyway).
fn phone_allows(
    state_dir: &Path,
    phone: &str,
    capability: pocketdesk_core::protocol::permissions::Capability,
) -> bool {
    pocketdesk_core::protocol::kdeconnect::TrustStore::read(
        state_dir.join(pocketdesk_core::local::paths::TRUST_FILE),
    )
    .map(|store| store.permissions(phone).allows(capability))
    .unwrap_or(true)
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
        self.contacts = pocketdesk_core::local::phone_contacts::read(state_dir);
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
            if list
                .button("New text")
                .on_hover_text("Write a new text, or find a number in the phone's contacts")
                .clicked()
            {
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
                            // Said at once with Dial off (L4 review).
                            let allowed = pocketdesk_core::protocol::kdeconnect::TrustStore::read(
                                state_dir.join(pocketdesk_core::local::paths::TRUST_FILE),
                            )
                            .map(|store| {
                                store.permissions(&device).allows(
                                    pocketdesk_core::protocol::permissions::Capability::Dial,
                                )
                            })
                            .unwrap_or(true);
                            self.notice = crate::notice::Notice::outcome(if !allowed {
                                Err("Not asked: Call numbers from here is off for this phone (Devices).".to_string())
                            } else {
                                pocketdesk_core::local::dials::request(
                                    state_dir, &device, &address, now_unix,
                                )
                                .map(|_| {
                                    "Asked the phone to open its dialer with the number; it does with \u{201c}Calls from the computer\u{201d} on.".to_string()
                                })
                                .map_err(|e| format!("Not asked: {e}"))
                            });
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
                                self.selected = Some((chosen.clone(), number));
                                self.new_number.clear();
                            }
                        }
                        pane.add_space(10.0);
                        self.contacts_pane(pane, state_dir, &chosen, now_unix);
                    }
                }
            }
        });
    }
}

/// How long the phone has to answer an ask for its contacts: the ask waits
/// a minute for it to connect (P10 review), and a few seconds more.
const CONTACTS_ANSWER_SECS: u64 = pocketdesk_core::local::phone_actions::ACTION_TTL_SECS + 10;

/// The most contacts shown at once: the search finds the rest.
const CONTACTS_SHOWN: usize = 50;

/// The contacts `query` finds: each word in the name or a number, case
/// aside; all with no words.
pub fn contacts_shown<'a>(
    contacts: &'a [pocketdesk_core::protocol::phone_action::PhoneContact],
    query: &str,
) -> Vec<&'a pocketdesk_core::protocol::phone_action::PhoneContact> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    contacts
        .iter()
        .filter(|c| {
            let text = format!("{} {}", c.name, c.numbers.join(" ")).to_lowercase();
            let digits: String = c
                .numbers
                .join(" ")
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect();
            words.iter().all(|w| {
                text.contains(w.as_str())
                    || (w.chars().all(|ch| ch.is_ascii_digit()) && digits.contains(w.as_str()))
            })
        })
        .collect()
}

impl MessagesView {
    /// P10: the phone's contacts, searched, each number with Text, Copy and
    /// Call; asked of the phone with a button.
    fn contacts_pane(&mut self, pane: &mut egui::Ui, state_dir: &Path, phone: &str, now_unix: u64) {
        let muted = crate::theme::muted(pane);
        pane.strong("Contacts");
        let list = self.contacts.get(phone);
        pane.horizontal_wrapped(|ui| {
            if ui
                .button(if list.is_some() {
                    "Ask again"
                } else {
                    "Ask the phone for its contacts"
                })
                .on_hover_text("Names and numbers only, kept on this computer")
                .clicked()
            {
                // Said at once with Texts off (P10 review).
                self.notice = crate::notice::Notice::outcome(
                    if !phone_allows(
                        state_dir,
                        phone,
                        pocketdesk_core::protocol::permissions::Capability::Texts,
                    ) {
                        Err("Not asked: \"Show its texts and send replies\" is off for this phone on Devices.".to_string())
                    } else {
                        pocketdesk_core::local::phone_actions::request(
                            state_dir,
                            phone,
                            pocketdesk_core::protocol::phone_action::PhoneAction::Contacts,
                            now_unix,
                        )
                        .map(|_| {
                            self.contacts_asked = Some((phone.to_string(), now_unix));
                            self.read_at = None;
                            "Asked the phone for its contacts.".to_string()
                        })
                        .map_err(|e| format!("Not asked: {e}"))
                    },
                );
            }
            if let Some(list) = list {
                ui.label(
                    RichText::new(format!(
                        "{}, {}",
                        crate::words::count(list.contacts.len(), "contact", "contacts"),
                        crate::words::ago(now_unix, list.at)
                    ))
                    .color(muted),
                );
            }
        });
        // An ask over ten minutes old is forgotten (Q2 review: its "No
        // answer" greeted the page hours later).
        if self
            .contacts_asked
            .as_ref()
            .is_some_and(|(_, at)| now_unix.saturating_sub(*at) > 600)
        {
            self.contacts_asked = None;
        }
        // Said while it waits, and when no list came (as P2's box does).
        if let Some((asked_phone, at)) = &self.contacts_asked
            && asked_phone == phone
            && list.is_none_or(|l| l.at < *at)
            && now_unix.saturating_sub(*at) < CONTACTS_ANSWER_SECS
        {
            pane.label(RichText::new("Asking the phone for its contacts\u{2026}").color(muted));
        }
        if let Some((asked_phone, at)) = &self.contacts_asked
            && asked_phone == phone
            && list.is_none_or(|l| l.at < *at)
            && now_unix.saturating_sub(*at) >= CONTACTS_ANSWER_SECS
        {
            pane.label(
                RichText::new(
                    "No answer from the phone. It answers with \"My contacts on the computer\" on in Pocketdesk's Settings on the phone, and \"Show its texts and send replies\" allowed on Devices here.",
                )
                .color(muted),
            );
        }
        let Some(list) = self.contacts.get(phone).cloned() else {
            return;
        };
        pane.add(
            egui::TextEdit::singleline(&mut self.contacts_query)
                .hint_text("Search contacts")
                .desired_width(240.0),
        );
        // Found again only when the words or the list changed.
        let fresh = self
            .contacts_found
            .as_ref()
            .is_some_and(|(p, at, q, _)| p == phone && *at == list.at && *q == self.contacts_query);
        if !fresh {
            let found = contacts_shown(&list.contacts, &self.contacts_query)
                .into_iter()
                .filter_map(|c| list.contacts.iter().position(|x| std::ptr::eq(x, c)))
                .collect();
            self.contacts_found = Some((
                phone.to_string(),
                list.at,
                self.contacts_query.clone(),
                found,
            ));
        }
        let places = self
            .contacts_found
            .as_ref()
            .map(|(_, _, _, found)| found.clone())
            .unwrap_or_default();
        let shown: Vec<&pocketdesk_core::protocol::phone_action::PhoneContact> = places
            .iter()
            .filter_map(|i| list.contacts.get(*i))
            .collect();
        if list.contacts.is_empty() {
            pane.label(RichText::new("The phone sent no contacts.").color(muted));
        } else if shown.is_empty() {
            pane.label(RichText::new("No contact matches.").color(muted));
        }
        let mut text_to = None;
        let mut call = None;
        let mut copied = false;
        egui::ScrollArea::vertical()
            .id_salt("contacts")
            .max_height(260.0)
            .show(pane, |ui| {
                for contact in shown.iter().take(CONTACTS_SHOWN) {
                    for number in &contact.numbers {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&contact.name).strong());
                            ui.label(number);
                            // Each says whose to a screen reader (Q2 review,
                            // as E5's photos do).
                            let named = |response: egui::Response, what: &str| {
                                let label = format!("{what} {} {number}", contact.name);
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        true,
                                        &label,
                                    )
                                });
                                response
                            };
                            if named(ui.small_button("Text"), "Text").clicked() {
                                text_to = Some(number.clone());
                            }
                            if named(ui.small_button("Copy"), "Copy").clicked() {
                                ui.ctx().copy_text(number.clone());
                                copied = true;
                            }
                            if named(ui.small_button("Call"), "Call").clicked() {
                                call = Some(number.clone());
                            }
                        });
                    }
                }
            });
        if shown.len() > CONTACTS_SHOWN {
            pane.label(
                RichText::new(format!(
                    "{} more: search to find them.",
                    shown.len() - CONTACTS_SHOWN
                ))
                .color(muted),
            );
        }
        if copied {
            self.notice = crate::notice::Notice::done("Copied.");
        }
        if let Some(number) = text_to {
            self.new_number = number;
        }
        if let Some(number) = call {
            // Held to Dial as the conversation's Call is (P10 review).
            if !phone_allows(
                state_dir,
                phone,
                pocketdesk_core::protocol::permissions::Capability::Dial,
            ) {
                self.notice = crate::notice::Notice::error(
                    "Not asked: Call numbers from here is off for this phone (Devices).",
                );
                return;
            }
            self.notice = crate::notice::Notice::outcome(
                pocketdesk_core::local::dials::request(state_dir, phone, &number, now_unix)
                    .map(|_| "Asked the phone to open its dialer with the number; it does with \u{201c}Calls from the computer\u{201d} on.".to_string())
                    .map_err(|e| format!("Not asked: {e}")),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P10: a contact is found by its name's words or its number's digits.
    #[test]
    fn contacts_are_found_by_name_or_number() {
        use pocketdesk_core::protocol::phone_action::PhoneContact;
        let contacts = vec![
            PhoneContact {
                name: "Ann Lee".into(),
                numbers: vec!["+1 555-0100".into()],
            },
            PhoneContact {
                name: "Bo".into(),
                numbers: vec!["5550199".into()],
            },
        ];
        assert_eq!(contacts_shown(&contacts, "").len(), 2);
        assert_eq!(contacts_shown(&contacts, "ann")[0].name, "Ann Lee");
        assert_eq!(contacts_shown(&contacts, "lee ann").len(), 1);
        assert_eq!(
            contacts_shown(&contacts, "5550100")[0].name,
            "Ann Lee",
            "digits across spaces and dashes"
        );
        assert!(contacts_shown(&contacts, "zed").is_empty());
    }

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
