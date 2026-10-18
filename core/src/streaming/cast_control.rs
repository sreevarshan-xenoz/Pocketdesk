//! Control of the phone from its screen window (C8): what the desktop app
//! sends down a cast session, batched. The phone performs it through its
//! Accessibility service, only while it has said control is on
//! ([`super::cast::CAST_CONTROL_STATE`]).
//!
//! A batch is `sent_ms u64 LE + count u8 + events`, each event a tag and
//! fixed fields:
//!
//! - `1` touch: `action u8` (0 down, 1 move, 2 up) `+ x u16 LE + y u16 LE`;
//! - `2` scroll: `x u16 LE + y u16 LE + dx i8 + dy i8` (notches);
//! - `3` global action: `u8` ([`Global`]);
//! - `4` text: `len u16 LE + UTF-8`;
//! - `5` backspace; `6` enter.
//!
//! Points are fractions of the picture, 0 to 65535 on each axis, so the
//! phone maps them to its own screen whatever size it encodes at.
//! `sent_ms` is the sender's clock, for measuring the delay.

/// Most events in one batch.
pub const MAX_EVENTS: usize = 64;
/// Most text in one event, in bytes.
pub const MAX_TEXT: usize = 1024;

/// What a touch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    Down,
    Move,
    Up,
}

/// A global action the phone performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Global {
    Back,
    Home,
    Recents,
    Notifications,
}

impl Global {
    fn code(self) -> u8 {
        match self {
            Global::Back => 1,
            Global::Home => 2,
            Global::Recents => 3,
            Global::Notifications => 4,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Global::Back,
            2 => Global::Home,
            3 => Global::Recents,
            4 => Global::Notifications,
            _ => return None,
        })
    }

    /// Its name, as the phone's app reads it.
    pub fn name(self) -> &'static str {
        match self {
            Global::Back => "back",
            Global::Home => "home",
            Global::Recents => "recents",
            Global::Notifications => "notifications",
        }
    }
}

/// One thing to do on the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    Touch { action: TouchAction, x: u16, y: u16 },
    Scroll { x: u16, y: u16, dx: i8, dy: i8 },
    Global(Global),
    Text(String),
    Backspace,
    Enter,
}

/// A batch of events, and when its sender sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlBatch {
    pub sent_ms: u64,
    pub events: Vec<ControlEvent>,
}

impl ControlBatch {
    /// The bytes on the wire; `None` for a batch too large to send (too many
    /// events, or text past [`MAX_TEXT`]).
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.events.is_empty() || self.events.len() > MAX_EVENTS {
            return None;
        }
        let mut out = Vec::with_capacity(9 + self.events.len() * 6);
        out.extend_from_slice(&self.sent_ms.to_le_bytes());
        out.push(self.events.len() as u8);
        for event in &self.events {
            match event {
                ControlEvent::Touch { action, x, y } => {
                    out.push(1);
                    out.push(match action {
                        TouchAction::Down => 0,
                        TouchAction::Move => 1,
                        TouchAction::Up => 2,
                    });
                    out.extend_from_slice(&x.to_le_bytes());
                    out.extend_from_slice(&y.to_le_bytes());
                }
                ControlEvent::Scroll { x, y, dx, dy } => {
                    out.push(2);
                    out.extend_from_slice(&x.to_le_bytes());
                    out.extend_from_slice(&y.to_le_bytes());
                    out.push(*dx as u8);
                    out.push(*dy as u8);
                }
                ControlEvent::Global(global) => {
                    out.push(3);
                    out.push(global.code());
                }
                ControlEvent::Text(text) => {
                    if text.is_empty() || text.len() > MAX_TEXT {
                        return None;
                    }
                    out.push(4);
                    out.extend_from_slice(&(text.len() as u16).to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
                ControlEvent::Backspace => out.push(5),
                ControlEvent::Enter => out.push(6),
            }
        }
        Some(out)
    }

    /// A batch from the wire; `None` for anything malformed, with nothing
    /// left over.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = Reader(bytes);
        let sent_ms = u64::from_le_bytes(r.take(8)?.try_into().ok()?);
        let count = r.u8()? as usize;
        if count == 0 || count > MAX_EVENTS {
            return None;
        }
        let mut events = Vec::with_capacity(count);
        for _ in 0..count {
            let event = match r.u8()? {
                1 => {
                    let action = match r.u8()? {
                        0 => TouchAction::Down,
                        1 => TouchAction::Move,
                        2 => TouchAction::Up,
                        _ => return None,
                    };
                    ControlEvent::Touch {
                        action,
                        x: r.u16()?,
                        y: r.u16()?,
                    }
                }
                2 => ControlEvent::Scroll {
                    x: r.u16()?,
                    y: r.u16()?,
                    dx: r.u8()? as i8,
                    dy: r.u8()? as i8,
                },
                3 => ControlEvent::Global(Global::from_code(r.u8()?)?),
                4 => {
                    let len = r.u16()? as usize;
                    if len == 0 || len > MAX_TEXT {
                        return None;
                    }
                    ControlEvent::Text(String::from_utf8(r.take(len)?.to_vec()).ok()?)
                }
                5 => ControlEvent::Backspace,
                6 => ControlEvent::Enter,
                _ => return None,
            };
            events.push(event);
        }
        r.0.is_empty().then_some(ControlBatch { sent_ms, events })
    }

    /// The batch as JSON for the phone's app: `{"sent": ms, "events": [...]}`,
    /// each event an object with its `type` and fields.
    pub fn to_json(&self) -> serde_json::Value {
        let events: Vec<serde_json::Value> = self
            .events
            .iter()
            .map(|event| match event {
                ControlEvent::Touch { action, x, y } => serde_json::json!({
                    "type": "touch",
                    "action": match action {
                        TouchAction::Down => "down",
                        TouchAction::Move => "move",
                        TouchAction::Up => "up",
                    },
                    "x": x,
                    "y": y,
                }),
                ControlEvent::Scroll { x, y, dx, dy } => {
                    serde_json::json!({"type": "scroll", "x": x, "y": y, "dx": dx, "dy": dy})
                }
                ControlEvent::Global(global) => {
                    serde_json::json!({"type": "global", "action": global.name()})
                }
                ControlEvent::Text(text) => serde_json::json!({"type": "text", "text": text}),
                ControlEvent::Backspace => serde_json::json!({"type": "backspace"}),
                ControlEvent::Enter => serde_json::json!({"type": "enter"}),
            })
            .collect();
        serde_json::json!({"sent": self.sent_ms, "events": events})
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> ControlBatch {
        ControlBatch {
            sent_ms: 1_792_296_000_123,
            events: vec![
                ControlEvent::Touch {
                    action: TouchAction::Down,
                    x: 0,
                    y: 65535,
                },
                ControlEvent::Touch {
                    action: TouchAction::Move,
                    x: 100,
                    y: 200,
                },
                ControlEvent::Touch {
                    action: TouchAction::Up,
                    x: 32768,
                    y: 1,
                },
                ControlEvent::Scroll {
                    x: 5,
                    y: 6,
                    dx: -1,
                    dy: 3,
                },
                ControlEvent::Global(Global::Back),
                ControlEvent::Global(Global::Notifications),
                ControlEvent::Text("héllo தமிழ் 😀".to_string()),
                ControlEvent::Backspace,
                ControlEvent::Enter,
            ],
        }
    }

    #[test]
    fn a_batch_of_every_kind_comes_back_as_it_went() {
        let batch = every_kind();
        let bytes = batch.encode().unwrap();
        assert_eq!(ControlBatch::decode(&bytes), Some(batch));
    }

    #[test]
    fn malformed_batches_are_refused() {
        let bytes = every_kind().encode().unwrap();
        for cut in 0..bytes.len() {
            assert_eq!(ControlBatch::decode(&bytes[..cut]), None, "cut at {cut}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(ControlBatch::decode(&extra), None, "trailing bytes");
        let mut unknown = 7u64.to_le_bytes().to_vec();
        unknown.extend_from_slice(&[1, 9]);
        assert_eq!(ControlBatch::decode(&unknown), None, "an unknown tag");
        let mut empty = 7u64.to_le_bytes().to_vec();
        empty.push(0);
        assert_eq!(ControlBatch::decode(&empty), None, "no events");
    }

    #[test]
    fn a_batch_too_large_is_not_sent() {
        let many = ControlBatch {
            sent_ms: 0,
            events: vec![ControlEvent::Enter; MAX_EVENTS + 1],
        };
        assert_eq!(many.encode(), None);
        let long = ControlBatch {
            sent_ms: 0,
            events: vec![ControlEvent::Text("a".repeat(MAX_TEXT + 1))],
        };
        assert_eq!(long.encode(), None);
        let none = ControlBatch {
            sent_ms: 0,
            events: vec![],
        };
        assert_eq!(none.encode(), None);
    }

    #[test]
    fn the_phone_reads_it_as_json() {
        let json = every_kind().to_json();
        assert_eq!(json["sent"], 1_792_296_000_123u64);
        assert_eq!(json["events"][0]["type"], "touch");
        assert_eq!(json["events"][0]["action"], "down");
        assert_eq!(json["events"][3]["dx"], -1);
        assert_eq!(json["events"][5]["action"], "notifications");
        assert_eq!(json["events"][6]["text"], "héllo தமிழ் 😀");
    }
}
