//! The phone as another screen for the shared keyboard and mouse (BK5).
//!
//! A phone whose owner lets it says what it is ([`SCREEN`], a
//! [`ScreenReport`]) on its control link as the link starts and as it
//! changes: whether it lets the computer's keyboard and mouse onto it, and
//! its screen. While the computer's cursor is on the phone, the computer
//! sends what its keyboard and mouse do there ([`INPUT`], a batch of
//! [`PhoneEvent`]s, positions as fractions out of [`FRACTION_MAX`] as C8's
//! touches are), and the phone answers each ping in it ([`PONG`]) so the
//! computer brings its mouse home when the phone goes quiet.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the phone says of itself.
pub const SCREEN: &str = "kdeconnect.pocketdesk.shared_input.screen";

/// What the computer's keyboard and mouse do on the phone.
pub const INPUT: &str = "kdeconnect.pocketdesk.shared_input.input";

/// The phone's answer to a ping in an [`INPUT`] batch.
pub const PONG: &str = "kdeconnect.pocketdesk.shared_input.pong";

/// The most events in one batch.
pub const MAX_EVENTS: usize = 64;

/// The most text, in bytes, in one batch.
pub const MAX_TEXT: usize = 1024;

/// The top of a position's fraction: the screen's last pixel.
pub const FRACTION_MAX: u16 = 65535;

/// The longest side, in pixels or dp, a phone's screen may have.
pub const MAX_PHONE_SIDE: u32 = 16_384;

/// A phone's screen, as it last said it: its size in pixels and in
/// density-independent pixels (dp) as it is drawn now, and its rotation
/// (0 to 3, a quarter turn each).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneScreen {
    pub width_px: u32,
    pub height_px: u32,
    pub width_dp: u32,
    pub height_dp: u32,
    #[serde(default)]
    pub rotation: u8,
}

impl PhoneScreen {
    /// Whether every side has a size a phone's can, and the rotation is one.
    pub fn is_sound(&self) -> bool {
        [self.width_px, self.height_px, self.width_dp, self.height_dp]
            .iter()
            .all(|side| (1..=MAX_PHONE_SIDE).contains(side))
            && self.rotation <= 3
    }
}

/// What a phone says of itself: whether it lets the computer's keyboard
/// and mouse onto it ("Let my computer's keyboard and mouse use this
/// phone"), whether the Accessibility service that touches and types
/// there is on, and its screen as it is drawn now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenReport {
    pub allows: bool,
    pub service: bool,
    #[serde(flatten)]
    pub screen: PhoneScreen,
}

impl ScreenReport {
    pub fn to_body(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    /// The report in `body`, if it is one with a sound screen.
    pub fn from_body(body: &Value) -> Option<ScreenReport> {
        serde_json::from_value::<ScreenReport>(body.clone())
            .ok()
            .filter(|r| r.screen.is_sound())
    }
}

/// A mouse button, as the phone takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneButton {
    Left,
    Middle,
    Right,
}

/// A button going down or coming up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Press {
    Down,
    Up,
}

/// A key that is no text: what it does in the focused field, or Back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhoneKey {
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Escape,
}

/// One thing the computer's keyboard or mouse did on the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PhoneEvent {
    /// The cursor came onto the phone here.
    Enter {
        x: u16,
        y: u16,
    },
    /// The cursor went back to the computer: every finger down lifts.
    Leave,
    /// The pointer moved here (no finger down unless a button is).
    Pointer {
        x: u16,
        y: u16,
    },
    Button {
        button: PhoneButton,
        action: Press,
    },
    /// Notches, positive down and to the right.
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// Text typed into the focused field.
    Text {
        text: String,
    },
    Key {
        key: PhoneKey,
    },
    /// Answered at once with a [`PONG`] of the same `t`.
    Ping {
        t: u64,
    },
}

/// Whether `events` fit one batch: not empty, at most [`MAX_EVENTS`] and
/// [`MAX_TEXT`] bytes of text.
pub fn fits(events: &[PhoneEvent]) -> bool {
    !events.is_empty() && events.len() <= MAX_EVENTS && text_of(events) <= MAX_TEXT
}

/// The bytes of text in `events`.
fn text_of(events: &[PhoneEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            PhoneEvent::Text { text } => text.len(),
            _ => 0,
        })
        .sum()
}

/// The packet's body for `events`.
pub fn body(events: &[PhoneEvent]) -> Value {
    json!({ "events": events })
}

/// The events of a packet's `body`, if it is a batch that fits. An event
/// not known (from a newer computer) is passed over, not the batch: a
/// lost release is lifted on the leave.
pub fn events_of(body: &Value) -> Option<Vec<PhoneEvent>> {
    let listed = body.get("events")?.as_array()?;
    if listed.len() > MAX_EVENTS {
        return None;
    }
    let events: Vec<PhoneEvent> = listed
        .iter()
        .filter_map(|e| serde_json::from_value(e.clone()).ok())
        .collect();
    fits(&events).then_some(events)
}

/// `events` as batches that each fit, in order: moves next to each other
/// kept to the last of them, text next to each other joined, and a batch
/// cut where it would hold too many events or too much text.
pub fn batches(events: Vec<PhoneEvent>) -> Vec<Vec<PhoneEvent>> {
    // Joined first: a move followed by a move is only the second, text
    // followed by text one text.
    let mut joined: Vec<PhoneEvent> = Vec::with_capacity(events.len());
    for event in events {
        match (joined.last_mut(), event) {
            (Some(last @ PhoneEvent::Pointer { .. }), next @ PhoneEvent::Pointer { .. }) => {
                *last = next
            }
            (Some(PhoneEvent::Text { text }), PhoneEvent::Text { text: more }) => {
                text.push_str(&more)
            }
            (_, event) => joined.push(event),
        }
    }
    // Then cut: a text too long for what room is left is split at a
    // character.
    let mut out: Vec<Vec<PhoneEvent>> = Vec::new();
    let mut batch: Vec<PhoneEvent> = Vec::new();
    for event in joined {
        let mut event = Some(event);
        while let Some(next) = event.take() {
            if batch.len() >= MAX_EVENTS {
                out.push(std::mem::take(&mut batch));
            }
            let room = MAX_TEXT - text_of(&batch);
            match next {
                PhoneEvent::Text { text } if text.len() > room => {
                    let mut cut = room;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    if cut > 0 {
                        batch.push(PhoneEvent::Text {
                            text: text[..cut].to_string(),
                        });
                    }
                    out.push(std::mem::take(&mut batch));
                    event = Some(PhoneEvent::Text {
                        text: text[cut..].to_string(),
                    });
                }
                next => batch.push(next),
            }
        }
    }
    if !batch.is_empty() {
        out.push(batch);
    }
    out
}

/// The body of a [`PONG`] for the ping `t`.
pub fn pong_body(t: u64) -> Value {
    json!({ "t": t })
}

/// The ping a [`PONG`]'s body answers.
pub fn pong_of(body: &Value) -> Option<u64> {
    body.get("t")?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> PhoneScreen {
        PhoneScreen {
            width_px: 1080,
            height_px: 2400,
            width_dp: 411,
            height_dp: 914,
            rotation: 0,
        }
    }

    #[test]
    fn a_report_goes_and_comes_back_whole() {
        let report = ScreenReport {
            allows: true,
            service: false,
            screen: screen(),
        };
        let body = report.to_body();
        assert_eq!(body["allows"], true);
        assert_eq!(body["width_dp"], 411);
        assert_eq!(ScreenReport::from_body(&body), Some(report));
        // A screen with no size, or one too large to be a phone's, is none.
        for (key, value) in [("width_px", 0), ("height_dp", 0), ("width_dp", 99_999)] {
            let mut odd = body.clone();
            odd[key] = json!(value);
            assert_eq!(ScreenReport::from_body(&odd), None, "{key}");
        }
        let mut turned = body.clone();
        turned["rotation"] = json!(7);
        assert_eq!(ScreenReport::from_body(&turned), None, "no such rotation");
        assert_eq!(ScreenReport::from_body(&json!({ "allows": true })), None);
    }

    #[test]
    fn a_batch_goes_and_comes_back_whole() {
        let events = vec![
            PhoneEvent::Enter { x: 0, y: 32768 },
            PhoneEvent::Pointer { x: 10, y: 20 },
            PhoneEvent::Button {
                button: PhoneButton::Left,
                action: Press::Down,
            },
            PhoneEvent::Button {
                button: PhoneButton::Left,
                action: Press::Up,
            },
            PhoneEvent::Wheel { dx: 0, dy: -2 },
            PhoneEvent::Text {
                text: "héllo தமிழ்".to_string(),
            },
            PhoneEvent::Key {
                key: PhoneKey::Backspace,
            },
            PhoneEvent::Ping { t: 7 },
            PhoneEvent::Leave,
        ];
        let body = body(&events);
        assert_eq!(body["events"][0]["type"], "enter");
        assert_eq!(body["events"][2]["button"], "left");
        assert_eq!(body["events"][2]["action"], "down");
        assert_eq!(body["events"][6]["key"], "backspace");
        assert_eq!(events_of(&body), Some(events));
    }

    #[test]
    fn a_batch_that_does_not_fit_is_refused_and_an_unknown_event_passed_over() {
        assert_eq!(events_of(&json!({ "events": [] })), None, "empty");
        let many = vec![PhoneEvent::Leave; MAX_EVENTS + 1];
        assert_eq!(events_of(&body(&many)), None, "too many");
        let long = vec![PhoneEvent::Text {
            text: "x".repeat(MAX_TEXT + 1),
        }];
        assert_eq!(events_of(&body(&long)), None, "too much text");
        assert_eq!(events_of(&json!({})), None);
        // From a newer computer: the rest still go.
        let newer = json!({ "events": [{ "type": "laser" }, { "type": "leave" }] });
        assert_eq!(events_of(&newer), Some(vec![PhoneEvent::Leave]));
        // Only unknown ones: nothing to do.
        assert_eq!(events_of(&json!({ "events": [{ "type": "laser" }] })), None);
    }

    #[test]
    fn moves_are_kept_to_the_last_and_batches_cut_where_they_would_not_fit() {
        let p = |x| PhoneEvent::Pointer { x, y: 0 };
        let down = PhoneEvent::Button {
            button: PhoneButton::Left,
            action: Press::Down,
        };
        let text = |t: &str| PhoneEvent::Text {
            text: t.to_string(),
        };
        // Moves either side of a press are kept apart, so the finger goes
        // down where the pointer was.
        assert_eq!(
            batches(vec![p(1), p(2), p(3), down.clone(), p(4), p(5)]),
            vec![vec![p(3), down.clone(), p(5)]]
        );
        assert_eq!(
            batches(vec![text("a"), text("b"), p(1), text("c")]),
            vec![vec![text("ab"), p(1), text("c")]]
        );
        assert!(batches(Vec::new()).is_empty());
        // Too many events: cut, nothing lost, each batch fits.
        let keys = vec![
            PhoneEvent::Key {
                key: PhoneKey::Enter
            };
            MAX_EVENTS * 2 + 3
        ];
        let cut = batches(keys);
        assert_eq!(cut.len(), 3);
        assert!(cut.iter().all(|b| fits(b)));
        assert_eq!(cut.iter().map(Vec::len).sum::<usize>(), MAX_EVENTS * 2 + 3);
        // Too much text: cut at a character.
        let long: Vec<PhoneEvent> = (0..MAX_TEXT).map(|_| text("é")).collect();
        let cut = batches(long);
        assert!(cut.len() >= 2 && cut.iter().all(|b| fits(b)));
        let joined: String = cut
            .iter()
            .flatten()
            .map(|e| match e {
                PhoneEvent::Text { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(joined, "é".repeat(MAX_TEXT));
    }

    #[test]
    fn a_pong_carries_its_ping() {
        assert_eq!(pong_of(&pong_body(42)), Some(42));
        assert_eq!(pong_of(&json!({})), None);
    }
}
