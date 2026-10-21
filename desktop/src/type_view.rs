//! Type on the phone (M4): a small window that takes this computer's
//! keyboard while it has focus and sends what is typed to a phone's focused
//! field, batch by batch through `typing_out/`, about every 100 ms. Text,
//! Backspace, Enter and Ctrl+V (the clipboard's text, cut to 1024 bytes);
//! its buttons Back, Home and Recents; Esc closes it.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::protocol::typing::{MAX_EVENTS, MAX_TEXT, TypeEvent};

/// How long typing gathers before it goes.
const FLUSH: Duration = Duration::from_millis(100);

/// Whether this computer may type on a phone now, or why not (the
/// Overview's button and its hover).
pub fn type_reach(linked: bool, control_on: bool, allowed: bool) -> Result<(), &'static str> {
    if !control_on {
        Err("Settings \"Control phones from their screen window\" is off.")
    } else if !allowed {
        Err("This phone's \"Show its screen or camera here\" is off in Devices.")
    } else if !linked {
        Err("The phone is not linked now.")
    } else {
        Ok(())
    }
}

/// What was asked of the window: events to send, and whether to close it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub events: Vec<TypeEvent>,
    pub close: bool,
}

/// `text` cut to [`MAX_TEXT`] bytes at a character's edge.
fn cut(text: &str) -> &str {
    if text.len() <= MAX_TEXT {
        return text;
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What egui's `events` ask of the phone.
pub fn events_from(events: &[egui::Event]) -> Typed {
    let mut typed = Typed::default();
    for event in events {
        match event {
            egui::Event::Text(text) if !text.is_empty() => {
                typed.events.push(TypeEvent::Text(text.clone()));
            }
            egui::Event::Paste(text) if !text.is_empty() => {
                typed.events.push(TypeEvent::Text(cut(text).to_string()));
            }
            egui::Event::Key {
                key, pressed: true, ..
            } => match key {
                egui::Key::Backspace => typed.events.push(TypeEvent::Backspace),
                egui::Key::Enter => typed.events.push(TypeEvent::Enter),
                egui::Key::Escape => typed.close = true,
                _ => {}
            },
            _ => {}
        }
    }
    typed
}

/// `events` as batches that fit: neighbouring text joined, each batch at
/// most [`MAX_EVENTS`] events and [`MAX_TEXT`] bytes of text.
pub fn batches(events: Vec<TypeEvent>) -> Vec<Vec<TypeEvent>> {
    let mut out: Vec<Vec<TypeEvent>> = Vec::new();
    let mut batch: Vec<TypeEvent> = Vec::new();
    let mut text = 0usize;
    for event in events {
        let len = match &event {
            TypeEvent::Text(t) => cut(t).len(),
            _ => 0,
        };
        if !batch.is_empty() && (batch.len() >= MAX_EVENTS || text + len > MAX_TEXT) {
            out.push(std::mem::take(&mut batch));
            text = 0;
        }
        match (event, batch.last_mut()) {
            (TypeEvent::Text(t), Some(TypeEvent::Text(last))) => last.push_str(cut(&t)),
            (TypeEvent::Text(t), _) => batch.push(TypeEvent::Text(cut(&t).to_string())),
            (other, _) => batch.push(other),
        }
        text += len;
    }
    if !batch.is_empty() {
        out.push(batch);
    }
    out
}

/// The typing window's state between frames.
#[derive(Default)]
pub struct TypeView {
    /// The phone typed to, and its name; `None` while closed.
    phone: Option<(String, String)>,
    pending: Vec<TypeEvent>,
    since: Option<Instant>,
    seq: u64,
    error: Option<String>,
}

impl TypeView {
    /// Open the window for `device_id`, called `name`.
    pub fn open(&mut self, device_id: &str, name: &str) {
        self.phone = Some((device_id.to_string(), name.to_string()));
        self.pending.clear();
        self.since = None;
        self.error = None;
    }

    /// Draw the window while it is open, and send what was typed.
    pub fn show(&mut self, ctx: &egui::Context, state_dir: Option<&Path>) {
        let Some((device_id, name)) = self.phone.clone() else {
            return;
        };
        let id = egui::ViewportId::from_hash_of("pocketdesk-type");
        let builder = egui::ViewportBuilder::default()
            .with_title(format!("Typing to {name}"))
            .with_inner_size([440.0, 170.0]);
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            let (events, asked_close) = ctx.input(|i| (i.events.clone(), i.viewport().close_requested()));
            let typed = events_from(&events);
            close |= asked_close || typed.close;
            if !typed.events.is_empty() {
                self.since.get_or_insert_with(Instant::now);
                self.pending.extend(typed.events);
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label(format!(
                    "What you type here goes to the field in front on {name}. Esc closes this window."
                ));
                ui.horizontal(|ui| {
                    for (label, event) in [
                        ("Back", TypeEvent::Back),
                        ("Home", TypeEvent::Home),
                        ("Recents", TypeEvent::Recents),
                    ] {
                        if ui.button(label).clicked() {
                            self.since.get_or_insert_with(Instant::now);
                            self.pending.push(event);
                        }
                    }
                });
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.label(
                    egui::RichText::new(
                        "Not into a password field, nor while the phone is locked. The phone shows that it is being typed on.",
                    )
                    .small()
                    .weak(),
                );
            });
            if self.since.is_some() {
                ctx.request_repaint_after(FLUSH);
            }
        });
        if self.since.is_some_and(|since| since.elapsed() >= FLUSH)
            || (close && !self.pending.is_empty())
        {
            self.flush(state_dir, &device_id);
        }
        if close {
            self.phone = None;
        }
    }

    fn flush(&mut self, state_dir: Option<&Path>, device_id: &str) {
        self.since = None;
        let events = std::mem::take(&mut self.pending);
        let Some(dir) = state_dir else {
            self.error = Some("This computer's state folder is not known.".to_string());
            return;
        };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let batches = batches(events);
        if batches.is_empty() {
            return;
        }
        // One file a flush (M4 review), so its batches stay in order.
        self.seq += 1;
        match pocketdesk_core::local::typing::request(dir, device_id, self.seq, &batches, now_ms) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(format!("Not sent: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_backspace_enter_and_a_paste_become_events_and_esc_closes() {
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let long = "é".repeat(MAX_TEXT);
        let typed = events_from(&[
            egui::Event::Text("hi".into()),
            key(egui::Key::Backspace),
            key(egui::Key::Enter),
            egui::Event::Paste(long),
            key(egui::Key::A),
            key(egui::Key::Escape),
        ]);
        assert!(typed.close);
        assert_eq!(typed.events[0], TypeEvent::Text("hi".into()));
        assert_eq!(typed.events[1], TypeEvent::Backspace);
        assert_eq!(typed.events[2], TypeEvent::Enter);
        let TypeEvent::Text(pasted) = &typed.events[3] else {
            panic!("a paste is text");
        };
        assert!(
            pasted.len() <= MAX_TEXT && pasted.chars().all(|c| c == 'é'),
            "cut at a character"
        );
        assert_eq!(typed.events.len(), 4, "other keys are not sent");
    }

    #[test]
    fn neighbouring_text_is_joined_and_batches_fit() {
        let joined = batches(vec![
            TypeEvent::Text("a".into()),
            TypeEvent::Text("b".into()),
            TypeEvent::Enter,
            TypeEvent::Text("c".into()),
        ]);
        assert_eq!(
            joined,
            [vec![
                TypeEvent::Text("ab".into()),
                TypeEvent::Enter,
                TypeEvent::Text("c".into())
            ]]
        );
        let many = batches(vec![TypeEvent::Enter; MAX_EVENTS * 2 + 1]);
        assert_eq!(many.len(), 3);
        assert!(many.iter().all(|b| b.len() <= MAX_EVENTS));
        let long = batches(vec![
            TypeEvent::Text("x".repeat(MAX_TEXT)),
            TypeEvent::Text("y".into()),
        ]);
        assert_eq!(long.len(), 2, "text past the limit starts a new batch");
        assert!(
            long.iter()
                .all(|b| pocketdesk_core::protocol::typing::fits(b))
        );
    }

    #[test]
    fn typing_needs_control_on_the_permission_and_a_link() {
        assert!(type_reach(true, true, true).is_ok());
        assert!(
            type_reach(true, false, true)
                .unwrap_err()
                .contains("Control phones")
        );
        assert!(
            type_reach(true, true, false)
                .unwrap_err()
                .contains("Show its screen")
        );
        assert!(
            type_reach(false, true, true)
                .unwrap_err()
                .contains("not linked")
        );
    }
}
