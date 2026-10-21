//! Typing on the phone from the computer's keyboard (M4):
//! `kdeconnect.pocketdesk.type {events: [...]}`, sent to a phone on its
//! control link, each event one of `{"text": "..."}`, `{"key": "backspace"
//! | "enter"}` and `{"global": "back" | "home" | "recents"}`. A batch holds at
//! most [`MAX_EVENTS`] events and [`MAX_TEXT`] bytes of text, as C8's.

use serde_json::{Value, json};

/// The packet the computer types with.
pub const TYPE: &str = "kdeconnect.pocketdesk.type";

/// The most events in one batch.
pub const MAX_EVENTS: usize = 64;

/// The most text, in bytes, in one batch.
pub const MAX_TEXT: usize = 1024;

/// One thing typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeEvent {
    Text(String),
    Backspace,
    Enter,
    Back,
    Home,
    Recents,
}

impl TypeEvent {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Text(text) => json!({ "text": text }),
            Self::Backspace => json!({ "key": "backspace" }),
            Self::Enter => json!({ "key": "enter" }),
            Self::Back => json!({ "global": "back" }),
            Self::Home => json!({ "global": "home" }),
            Self::Recents => json!({ "global": "recents" }),
        }
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        if let Some(text) = value.get("text").and_then(Value::as_str) {
            return (!text.is_empty()).then(|| Self::Text(text.to_string()));
        }
        match (
            value.get("key").and_then(Value::as_str),
            value.get("global").and_then(Value::as_str),
        ) {
            (Some("backspace"), None) => Some(Self::Backspace),
            (Some("enter"), None) => Some(Self::Enter),
            (None, Some("back")) => Some(Self::Back),
            (None, Some("home")) => Some(Self::Home),
            (None, Some("recents")) => Some(Self::Recents),
            _ => None,
        }
    }
}

/// Whether `events` fit one batch: not empty, at most [`MAX_EVENTS`] and
/// [`MAX_TEXT`] bytes of text.
pub fn fits(events: &[TypeEvent]) -> bool {
    let text: usize = events
        .iter()
        .map(|e| match e {
            TypeEvent::Text(t) => t.len(),
            _ => 0,
        })
        .sum();
    !events.is_empty() && events.len() <= MAX_EVENTS && text <= MAX_TEXT
}

/// The packet's body for `events`.
pub fn body(events: &[TypeEvent]) -> Value {
    json!({ "events": events.iter().map(TypeEvent::to_json).collect::<Vec<_>>() })
}

/// The events of a packet's `body`, if it is a batch that fits; one event
/// not known spoils the batch, as a half-typed word would.
pub fn events_of(body: &Value) -> Option<Vec<TypeEvent>> {
    let events = body
        .get("events")?
        .as_array()?
        .iter()
        .map(TypeEvent::from_json)
        .collect::<Option<Vec<_>>>()?;
    fits(&events).then_some(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_goes_and_comes_back_whole() {
        let events = vec![
            TypeEvent::Text("héllo தமிழ்".to_string()),
            TypeEvent::Backspace,
            TypeEvent::Enter,
            TypeEvent::Back,
            TypeEvent::Home,
            TypeEvent::Recents,
        ];
        assert_eq!(events_of(&body(&events)), Some(events));
    }

    #[test]
    fn a_batch_that_does_not_fit_or_names_an_unknown_event_is_refused() {
        assert_eq!(events_of(&json!({ "events": [] })), None, "empty");
        let many = vec![TypeEvent::Enter; MAX_EVENTS + 1];
        assert_eq!(events_of(&body(&many)), None, "too many");
        let long = vec![TypeEvent::Text("x".repeat(MAX_TEXT + 1))];
        assert_eq!(events_of(&body(&long)), None, "too much text");
        assert_eq!(
            events_of(&json!({ "events": [{ "key": "tab" }] })),
            None,
            "a key not known"
        );
        assert_eq!(
            events_of(&json!({ "events": [{ "key": "enter", "global": "home" }] })),
            None,
            "two at once"
        );
        assert_eq!(events_of(&json!({})), None);
    }
}
