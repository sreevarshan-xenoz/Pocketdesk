//! A chat's recent messages, carried on its phone notification (D4).
//!
//! A messaging app keeps one notification per conversation and updates it as
//! messages come (Android's `MessagingStyle`), so the notification itself is
//! the conversation. A phone that sends it in full adds `messages:
//! [{from, text, time, mine}]`, oldest first; the computer keeps them on the
//! notification it can answer and shows them as a thread. The texts come
//! from anyone who can message the phone, so they are held to limits and
//! cleaned here.

use serde::{Deserialize, Serialize};

/// The most messages kept from one notification: the newest.
pub const MAX_MESSAGES: usize = 10;

/// A sender's name, at most, in characters.
pub const MAX_FROM_CHARS: usize = 64;

/// A message's text, at most, in characters.
pub const MAX_TEXT_CHARS: usize = 1000;

/// The earliest a message's time may be, unix milliseconds (1973): an app
/// that says seconds, or nothing sane, is taken as saying no time.
pub const MIN_AT_MS: u64 = 100_000_000_000;

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who sent it, as the app names them; empty for the phone's owner.
    pub from: String,
    pub text: String,
    /// When it was sent, unix milliseconds, as the app says.
    pub at_ms: u64,
    /// Sent by the phone's owner.
    pub mine: bool,
}

/// The messages a `kdeconnect.notification` body carries, oldest first:
/// each with a text, held to the limits, the newest [`MAX_MESSAGES`].
/// A body without them, or with a malformed list, has none.
pub fn messages(body: &serde_json::Value) -> Vec<Message> {
    let Some(list) = body.get("messages").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<Message> = list
        .iter()
        .filter_map(|m| {
            let text = clean(m.get("text")?.as_str()?, MAX_TEXT_CHARS);
            if text.is_empty() {
                return None;
            }
            let mine = m.get("mine").and_then(|v| v.as_bool()) == Some(true);
            let from = if mine {
                String::new()
            } else {
                // One line: a name.
                clean(
                    m.get("from").and_then(|v| v.as_str()).unwrap_or(""),
                    MAX_FROM_CHARS,
                )
                .replace('\n', " ")
            };
            Some(Message {
                from,
                text,
                at_ms: m
                    .get("time")
                    .and_then(|v| v.as_u64())
                    .filter(|t| *t >= MIN_AT_MS)
                    .unwrap_or(0),
                mine,
            })
        })
        .collect();
    let excess = out.len().saturating_sub(MAX_MESSAGES);
    out.drain(..excess);
    out
}

/// `raw` as shown: control and invisible formatting characters dropped
/// (line breaks kept), trimmed, cut to `max` characters with an ellipsis.
fn clean(raw: &str, max: usize) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| *c == '\n' || !(c.is_control() || super::kdeconnect::is_invisible_format(*c)))
        .collect();
    let kept = kept.trim();
    if kept.chars().count() <= max {
        return kept.to_string();
    }
    let mut cut: String = kept.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_are_read_oldest_first_and_held_to_the_limits() {
        let body = json!({"messages": [
            {"from": "Ann", "text": "hi", "time": 1000, "mine": false},
            {"from": "ignored", "text": "hello back", "time": 2000, "mine": true},
            {"from": "Ann", "text": "   ", "time": 3000},
            {"from": "Ann", "time": 4000},
            {"from": "Bo\u{202e}b\nby", "text": "line one\nline two", "time": 5000},
        ]});
        let got = messages(&body);
        assert_eq!(got.len(), 3, "blank and missing texts left out");
        assert_eq!(
            got[0],
            Message {
                from: "Ann".into(),
                text: "hi".into(),
                at_ms: 0,
                mine: false
            },
            "1000 ms is no time"
        );
        assert_eq!(got[1].from, "", "the owner's own have no sender");
        assert!(got[1].mine);
        assert_eq!(got[2].from, "Bob by", "invisible characters out, one line");
        assert_eq!(
            got[2].text, "line one\nline two",
            "a message keeps its lines"
        );
    }

    #[test]
    fn the_newest_are_kept_and_long_texts_cut() {
        let list: Vec<_> = (0..15)
            .map(|i| json!({"from": "Ann", "text": format!("m{i}"), "time": i}))
            .collect();
        let got = messages(&json!({ "messages": list }));
        assert_eq!(got.len(), MAX_MESSAGES);
        assert_eq!(got[0].text, "m5");
        assert_eq!(got[MAX_MESSAGES - 1].text, "m14");
        let long = "x".repeat(MAX_TEXT_CHARS + 5);
        let got = messages(&json!({"messages": [{"from": "A", "text": long}]}));
        assert_eq!(got[0].text.chars().count(), MAX_TEXT_CHARS);
        assert!(got[0].text.ends_with('…'));
    }

    #[test]
    fn a_body_without_messages_has_none() {
        assert!(messages(&json!({})).is_empty());
        assert!(messages(&json!({"messages": "nope"})).is_empty());
        assert!(messages(&json!({"messages": [1, "x", null]})).is_empty());
    }
}
