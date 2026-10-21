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
    /// The phones' ongoing updates (N4), newest first.
    cards: Vec<pocketdesk_core::local::phone_live::LiveCard>,
    /// Where each answer stands (E1).
    answers: Vec<Outcome>,
    /// Each notification's reply being typed, by phone and key.
    drafts: HashMap<(String, String), String>,
    read_at: Option<Instant>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
    /// The page an empty state's button asked for (E5).
    go_to: Option<crate::app::Tab>,
    /// Each app's icon as kept for its phone (K3), by phone and app, with
    /// its file's time; looked for again at the next read when none was
    /// kept or the file changed (K3 review: an updated icon showed only
    /// after a restart).
    icons: HashMap<(String, String), (Option<std::time::SystemTime>, Option<egui::TextureHandle>)>,
}

/// When `phone`'s icon for `app` was kept, if it is.
fn icon_stamp(state_dir: &Path, phone: &str, app: &str) -> Option<std::time::SystemTime> {
    let path = pocketdesk_core::local::notification_icons::icon_path(state_dir, phone, app);
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// An app's kept icon as a texture, if there is one that reads as a PNG
/// within an icon's size (K3: held to it before decoding).
fn icon_texture(
    ctx: &egui::Context,
    state_dir: &Path,
    phone: &str,
    app: &str,
) -> Option<egui::TextureHandle> {
    use pocketdesk_core::local::notification_icons as icons;
    let path = icons::kept_icon(state_dir, phone, app)?;
    let bytes = std::fs::read(path).ok()?;
    if !icons::icon_fits(&bytes) {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(icons::MAX_ICON_SIDE);
    limits.max_image_height = Some(icons::MAX_ICON_SIDE);
    limits.max_alloc = Some(8 << 20);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(
        format!("icon-{phone}-{}", app.to_lowercase()),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

/// The notifications to list, newest first.
pub fn newest_first(mut notes: Vec<PhoneNote>) -> Vec<PhoneNote> {
    notes.reverse();
    notes
}

impl NotesView {
    /// The page its empty state's button asked for, since the last call (E5).
    pub fn take_go_to(&mut self) -> Option<crate::app::Tab> {
        self.go_to.take()
    }
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
        let mut cards = pocketdesk_core::local::phone_live::read(state_dir);
        cards.reverse();
        self.cards = cards;
        self.answers = answers::read(state_dir);
        // Drafts for notifications that went are dropped.
        let live: Vec<(String, String)> = self
            .notes
            .iter()
            .map(|n| (n.device_id.clone(), n.key.clone()))
            .collect();
        self.drafts.retain(|k, _| live.contains(k));
        // An icon not kept before may be now, or kept anew.
        self.icons.retain(|(phone, app), (stamp, icon)| {
            icon.is_some() && *stamp == icon_stamp(state_dir, phone, app)
        });
        self.read_at = Some(Instant::now());
    }

    fn answer(&mut self, state_dir: &Path, note: &PhoneNote, answer: Answer, now_unix: u64) {
        let asked = match &answer {
            Answer::Action(title) => {
                format!("Asked the phone to send \"{title}\" to {}.", note.app)
            }
            Answer::Reply(_) => format!("Asked the phone to send your reply to {}.", note.app),
            // N3: it opens on the phone's screen, unlocked.
            Answer::Open => format!("Asked the phone to open it in {}.", note.app),
        };
        let opening = answer == Answer::Open;
        match phone_notes::request(state_dir, &note.device_id, &note.key, answer, now_unix) {
            Ok(_) => {
                // Queued: the server sends it within a minute, once the
                // phone is connected.
                self.notice = crate::notice::Notice::done(asked);
                // An open leaves a reply being typed as it is (N3 review).
                if !opening {
                    self.drafts
                        .remove(&(note.device_id.clone(), note.key.clone()));
                }
            }
            Err(e) => self.notice = crate::notice::Notice::error(e.to_string()),
        }
    }

    /// M8: ask the phone the call rang on to open its dialer with `number`.
    fn call_back(&mut self, state_dir: &Path, note: &PhoneNote, number: &str, now_unix: u64) {
        let allowed = pocketdesk_core::protocol::kdeconnect::TrustStore::read(
            state_dir.join(pocketdesk_core::local::paths::TRUST_FILE),
        )
        .map(|store| {
            store
                .permissions(&note.device_id)
                .allows(pocketdesk_core::protocol::permissions::Capability::Dial)
        })
        .unwrap_or(true);
        self.notice = crate::notice::Notice::outcome(if !allowed {
            Err("Not asked: Call numbers from here is off for this phone (Devices).".to_string())
        } else {
            pocketdesk_core::local::dials::request(state_dir, &note.device_id, number, now_unix)
                .map(|_| {
                    format!(
                        "Asked the phone to open its dialer with {number}; it does with \u{201c}Calls from the computer\u{201d} on."
                    )
                })
                .map_err(|e| format!("Not asked: {e}"))
        });
    }

    /// Ask the phone to clear `note` (L1).
    fn clear(&mut self, state_dir: &Path, note: &PhoneNote, now_unix: u64) {
        self.notice = match pocketdesk_core::local::note_clears::request(
            state_dir,
            &note.device_id,
            &note.key,
            now_unix,
        ) {
            Ok(_) => crate::notice::Notice::done(format!(
                "Asked the phone to clear the {} notification; it does with \u{201c}Let the computer clear my notifications\u{201d} on.",
                note.app
            )),
            Err(e) => crate::notice::Notice::error(format!("Not cleared: {e}")),
        };
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
        // N4: what is going on now, above what can be answered.
        let going = going_on(&self.cards, devices);
        if !going.is_empty() {
            ui.label(RichText::new("Going on now").strong());
            for card in going {
                ui.group(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{}: {}", card.app, card.title)).strong());
                    if !card.text.is_empty() {
                        ui.label(&card.text);
                    }
                    let now_ms = now_unix.saturating_mul(1000);
                    let status = card.live.status(now_ms);
                    match card.live.percent() {
                        Some(percent) => {
                            ui.add(
                                egui::ProgressBar::new(f32::from(percent) / 100.0)
                                    .desired_width(260.0)
                                    .text(status),
                            );
                        }
                        None => {
                            ui.label(RichText::new(status).color(crate::theme::muted(ui)));
                        }
                    }
                });
            }
            ui.add_space(8.0);
        }
        if self.notes.is_empty() {
            ui.label(
                RichText::new(
                    "Notifications from your phone that you can answer appear here: \
                     their buttons, and a reply box for chats. The phone sends them once \
                     \"This phone's notifications\" is on, and lets you answer once \
                     \"Let the computer answer notifications\" is on, in Pocketdesk's Settings on the phone.",
                )
                .color(crate::theme::muted(ui)),
            );
            // With no phone paired, the step before (E5).
            if devices.is_empty() && ui.button("Pair a phone").clicked() {
                self.go_to = Some(crate::app::Tab::Pair);
            }
            return;
        }
        let notes = self.notes.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for note in &notes {
                let icon = self
                    .icons
                    .entry((note.device_id.clone(), note.app.to_lowercase()))
                    .or_insert_with(|| {
                        (
                            icon_stamp(state_dir, &note.device_id, &note.app),
                            icon_texture(ui.ctx(), state_dir, &note.device_id, &note.app),
                        )
                    })
                    .1
                    .clone();
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        // K3: the app's icon, as the phone sent it.
                        if let Some(icon) = &icon {
                            ui.add(
                                egui::Image::new(icon)
                                    .fit_to_exact_size(egui::vec2(20.0, 20.0))
                                    .alt_text(note.app.clone()),
                            );
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}: {}", note.app, note.title)).strong(),
                            )
                            .wrap(),
                        );
                    });
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
                            "From {} · {}",
                            crate::model::device_label(devices, &note.device_id),
                            crate::words::ago(now_unix, note.at_unix)
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
                            Tone::Unsure => crate::theme::warn(ui),
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
                        // N3: where the phone lets the computer open it.
                        if note.open
                            && ui
                                .button("Open on the phone")
                                .on_hover_text("It opens on the phone's screen, as a tap on it would; the phone must be unlocked")
                                .clicked()
                        {
                            self.answer(state_dir, note, Answer::Open, now_unix);
                        }
                        // L1: cleared here, cleared on the phone; an ended
                        // call is gone there, so only here (M8 review).
                        if ended_call(note) {
                            if ui.button("Remove").on_hover_text("Remove it from this list").clicked() {
                                self.notice = match pocketdesk_core::local::phone_notes::forget(
                                    state_dir,
                                    &note.device_id,
                                    &note.key,
                                ) {
                                    Ok(()) => crate::notice::Notice::done("Removed."),
                                    Err(e) => crate::notice::Notice::error(format!("Not removed: {e}")),
                                };
                            }
                        } else if ui
                            .button("Clear")
                            .on_hover_text("Clear it on the phone too")
                            .clicked()
                        {
                            self.clear(state_dir, note, now_unix);
                        }
                        // M8: a call's number, dialled back through the phone.
                        if let Some(number) = call_back_number(note)
                            && ui
                                .button("Call back")
                                .on_hover_text("The phone's dialer opens with the number; you press Call there")
                                .clicked()
                        {
                            self.call_back(state_dir, note, &number, now_unix);
                        }
                    });
                    if note.reply {
                        let key = (note.device_id.clone(), note.key.clone());
                        let draft = self.drafts.entry(key.clone()).or_default();
                        let reply_box =
                            ui.add(egui::TextEdit::singleline(draft).hint_text("Reply"));
                        let ready = !draft.trim().is_empty();
                        // Enter in the box sends, as the button does (I3).
                        let entered = crate::keys::entered(ui, &reply_box);
                        if (ui
                            .add_enabled(ready, egui::Button::new("Send reply"))
                            .clicked()
                            || entered)
                            && ready
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

/// A call's note whose call ended (M8 review): kept for Call back, with
/// nothing left to answer.
fn ended_call(note: &PhoneNote) -> bool {
    note.key
        .starts_with(pocketdesk_core::local::phone_notes::CALL_PREFIX)
        && note.actions.is_empty()
        && !note.reply
}

/// The number a ringing call's note (F5) names, for Call back (M8): its
/// title is "Call from Ann (+15550100)" or "Call from +15550100"; `None`
/// for another note, or a caller with no number.
pub fn call_back_number(note: &PhoneNote) -> Option<String> {
    if !note
        .key
        .starts_with(pocketdesk_core::local::phone_notes::CALL_PREFIX)
    {
        return None;
    }
    let who = note.title.strip_prefix("Call from ")?;
    let number = match who.strip_suffix(')').and_then(|w| w.rsplit_once(" (")) {
        Some((_, number)) => number,
        None => who,
    };
    pocketdesk_core::protocol::dial::clean_number(number)
}

/// How an answer's line reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Waiting,
    Done,
    Failed,
    /// Sent, and the phone never said how it went (E5: said in the warning
    /// colour, not as waiting).
    Unsure,
}

/// How long an answer sent to the phone may go unanswered before the page
/// says so: a phone from before E1 never answers.
pub const NO_WORD_SECS: u64 = 60;

/// The ongoing updates to show (N4): those of a phone linked or in a
/// session now, since one gone away cannot say they ended.
pub fn going_on<'a>(
    cards: &'a [pocketdesk_core::local::phone_live::LiveCard],
    devices: &[crate::model::Device],
) -> Vec<&'a pocketdesk_core::local::phone_live::LiveCard> {
    cards
        .iter()
        .filter(|c| {
            devices
                .iter()
                .any(|d| d.id == c.device_id && (d.linked || d.connected))
        })
        .collect()
}

/// Where an answer stands, as the page says it (E1).
pub fn answer_line(outcome: &Outcome, now_unix: u64) -> (String, Tone) {
    match &outcome.state {
        State::Waiting => (
            "Your answer: waiting for the phone…".to_string(),
            Tone::Waiting,
        ),
        State::Sent if now_unix.saturating_sub(outcome.at_unix) >= NO_WORD_SECS => (
            "Your answer was sent, but the phone has not said whether it arrived (an older app on it, or the link dropped)"
                .to_string(),
            Tone::Unsure,
        ),
        State::Sent => ("Your answer: sent to the phone…".to_string(), Tone::Waiting),
        // N3: an open says whether it opened or waits as an offer there.
        State::Handed => match outcome.said.as_deref() {
            Some(said) => (format!("Your answer: {said}"), Tone::Done),
            None => ("Your answer: handed to the app".to_string(), Tone::Done),
        },
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

    /// N4: only a reached phone's cards show.
    #[test]
    fn only_a_reached_phone_s_updates_show() {
        use pocketdesk_core::local::phone_live::LiveCard;
        use pocketdesk_core::protocol::live::Live;
        let card = |device: &str| LiveCard {
            device_id: device.into(),
            tag: "t".into(),
            app: "Files".into(),
            title: "Downloading".into(),
            text: String::new(),
            live: Live {
                progress: 1,
                max: 2,
                indeterminate: false,
                chronometer: None,
                countdown: false,
            },
            updated_unix: 1,
        };
        let device = |id: &str, linked: bool| crate::model::Device {
            id: id.into(),
            linked,
            ..Default::default()
        };
        let cards = [card("near"), card("away"), card("unknown")];
        let shown: Vec<&str> = going_on(&cards, &[device("near", true), device("away", false)])
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(shown, ["near"]);
    }
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
            said: None,
        };
        assert_eq!(answer_line(&at(State::Waiting), 1_000).1, Tone::Waiting);
        assert_eq!(
            answer_line(&at(State::Sent), 1_010).0,
            "Your answer: sent to the phone…"
        );
        assert!(
            answer_line(&at(State::Sent), 1_000 + NO_WORD_SECS)
                .0
                .contains("has not said")
        );
        assert_eq!(
            answer_line(&at(State::Sent), 1_000 + NO_WORD_SECS).1,
            Tone::Unsure,
            "a warning, not waiting (E5)"
        );
        assert_eq!(answer_line(&at(State::Handed), 1_000).1, Tone::Done);
        let opened = Outcome {
            said: Some("opened on the phone".to_string()),
            ..at(State::Handed)
        };
        assert_eq!(
            answer_line(&opened, 1_000).0,
            "Your answer: opened on the phone"
        );
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
            open: false,
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

    /// M8: Call back takes a call note's number, and nothing else's.
    #[test]
    fn a_call_note_s_number_for_call_back() {
        let note = |key: &str, title: &str| PhoneNote {
            device_id: "phone".into(),
            key: key.into(),
            app: "Pocketdesk".into(),
            title: title.into(),
            text: "Ringing on the phone".into(),
            at_unix: 0,
            actions: vec!["Silence".into()],
            reply: false,
            open: false,
            messages: Vec::new(),
        };
        assert_eq!(
            call_back_number(&note("pocketdesk:call:3", "Call from Ann (+1 555-0100)")).as_deref(),
            Some("+15550100")
        );
        assert_eq!(
            call_back_number(&note("pocketdesk:call:3", "Call from +15550100")).as_deref(),
            Some("+15550100")
        );
        assert_eq!(
            call_back_number(&note("pocketdesk:call:3", "Call from Ann (Work)")),
            None,
            "a name in brackets is no number"
        );
        assert_eq!(
            call_back_number(&note("pocketdesk:call:3", "Call from an unknown number")),
            None
        );
        assert_eq!(
            call_back_number(&note("chat:1", "Call from +15550100")),
            None,
            "not a call's note"
        );
        assert_eq!(
            call_back_number(&note("pocketdesk:call:3", "Call from *#06#")),
            None
        );
    }
}
