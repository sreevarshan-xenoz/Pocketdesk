//! Type on the phone (M4): a small window that takes this computer's
//! keyboard while it has focus and sends what is typed to a phone's focused
//! field, batch by batch through `typing_out/`, about every 100 ms. Text,
//! Backspace, Enter and Ctrl+V (the clipboard's text, cut to 1024 bytes);
//! its buttons Back, Home and Recents; Esc closes it.

use std::path::Path;
use std::sync::{Arc, Mutex};
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
            // The system's input method's text (M4 review).
            egui::Event::Ime(egui::ImeEvent::Commit(text)) if !text.is_empty() => {
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

/// The typing window's state, shared with its viewport, which draws on its
/// own (as the cast window's does, M4 review: an immediate viewport drew
/// only with the main window and stopped while it was minimized).
#[derive(Default)]
struct State {
    /// The phone typed to, and its name; `None` while closed.
    phone: Option<(String, String)>,
    /// Why typing cannot reach it now, as the main window last judged.
    blocked: Option<&'static str>,
    pending: Vec<TypeEvent>,
    since: Option<Instant>,
    seq: u64,
    error: Option<String>,
}

/// The typing window.
#[derive(Default)]
pub struct TypeView {
    shared: Arc<Mutex<State>>,
}

fn lock(shared: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pocketdesk-type")
}

impl TypeView {
    /// Open the window for `device_id`, called `name`. What was typed to
    /// the phone before goes first (M4 review: it was dropped).
    pub fn open(&mut self, state_dir: &Path, device_id: &str, name: &str) {
        let mut state = lock(&self.shared);
        flush(&mut state, state_dir);
        state.phone = Some((device_id.to_string(), name.to_string()));
        state.error = None;
    }

    /// The phone the window types to, while it is open.
    pub fn phone(&self) -> Option<String> {
        lock(&self.shared).phone.as_ref().map(|(id, _)| id.clone())
    }

    /// Keep the window open while it is, with `reach` judged afresh each
    /// frame for its phone (M4 review: a phone that went away, or a
    /// permission turned off, still took typing).
    pub fn show(&mut self, ctx: &egui::Context, state_dir: &Path, reach: Result<(), &'static str>) {
        let title = {
            let mut state = lock(&self.shared);
            let Some((_, name)) = state.phone.clone() else {
                return;
            };
            let blocked = reach.err();
            if state.blocked != blocked {
                state.blocked = blocked;
                ctx.request_repaint_of(viewport_id());
            }
            format!("Typing to {name}")
        };
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([440.0, 170.0]);
        let shared = self.shared.clone();
        let state_dir = state_dir.to_path_buf();
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, _class| {
            frame(&shared, ui, &state_dir);
        });
    }
}

/// One frame of the typing window.
fn frame(shared: &Mutex<State>, ui: &mut egui::Ui, state_dir: &Path) {
    let ctx = ui.ctx().clone();
    // Tab would move focus to a button, which Enter or Space then presses.
    ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    let (events, asked_close) = ctx.input(|i| (i.events.clone(), i.viewport().close_requested()));
    let typed = events_from(&events);
    let mut state = lock(shared);
    let Some((_, name)) = state.phone.clone() else {
        return;
    };
    let close = asked_close || typed.close;
    let push = |state: &mut State, events: Vec<TypeEvent>| {
        if events.is_empty() {
            return;
        }
        match state.blocked {
            Some(why) => state.error = Some(format!("Not sent: {why}")),
            None => {
                state.since.get_or_insert_with(Instant::now);
                state.pending.extend(events);
            }
        }
    };
    push(&mut state, typed.events);
    egui::CentralPanel::default().show(ui, |ui| {
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
                    push(&mut state, vec![event]);
                }
            }
        });
        if let Some(why) = state.blocked {
            ui.colored_label(ui.visuals().warn_fg_color, why);
        } else if let Some(error) = &state.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.label(
            egui::RichText::new(
                "Needs \"Typing from the computer's keyboard\" on in Pocketdesk's Settings on the phone, with its Accessibility service and notification access. Never into a password field, nor while the phone is locked; the phone shows that it is being typed on.",
            )
            .small()
            .weak(),
        );
        // Ask for the system's input method here (M4 review: Chinese,
        // Japanese and Korean typing never came without it).
        if state.blocked.is_none() {
            let rect = ui.max_rect();
            let cursor = egui::Rect::from_min_size(rect.left_top(), egui::vec2(1.0, 16.0));
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    purpose: egui::IMEPurpose::Normal,
                    rect,
                    cursor_rect: cursor,
                    should_interrupt_composition: false,
                });
            });
        }
    });
    if state.since.is_some_and(|since| since.elapsed() >= FLUSH) || close {
        flush(&mut state, state_dir);
    }
    if state.since.is_some() {
        ctx.request_repaint_after(FLUSH);
    }
    if close {
        state.phone = None;
        state.blocked = None;
    }
}

/// Send what is pending, as one flush.
fn flush(state: &mut State, state_dir: &Path) {
    state.since = None;
    let events = std::mem::take(&mut state.pending);
    let Some((device_id, _)) = state.phone.clone() else {
        return;
    };
    let batches = batches(events);
    if batches.is_empty() {
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // One file a flush (M4 review), so its batches stay in order.
    state.seq += 1;
    match pocketdesk_core::local::typing::request(
        state_dir, &device_id, state.seq, &batches, now_ms,
    ) {
        Ok(()) => state.error = None,
        Err(e) => state.error = Some(format!("Not sent: {e}")),
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
            egui::Event::Ime(egui::ImeEvent::Commit("日本".into())),
            key(egui::Key::Backspace),
            key(egui::Key::Enter),
            egui::Event::Paste(long),
            key(egui::Key::A),
            key(egui::Key::Escape),
        ]);
        assert!(typed.close);
        assert_eq!(typed.events[0], TypeEvent::Text("hi".into()));
        assert_eq!(
            typed.events[1],
            TypeEvent::Text("日本".into()),
            "an input method's text"
        );
        assert_eq!(typed.events[2], TypeEvent::Backspace);
        assert_eq!(typed.events[3], TypeEvent::Enter);
        let TypeEvent::Text(pasted) = &typed.events[4] else {
            panic!("a paste is text");
        };
        assert!(
            pasted.len() <= MAX_TEXT && pasted.chars().all(|c| c == 'é'),
            "cut at a character"
        );
        assert_eq!(typed.events.len(), 5, "other keys are not sent");
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
