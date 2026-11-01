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
//! - `5` backspace; `6` enter;
//! - `7` window: `window u8` (Phone apps tier 2): the events after it, up to
//!   the next window event, are for that app window, not the phone's screen;
//! - `8` close: the app window named before it is closed and its display
//!   given back (a batch of a window event and this, nothing else);
//! - `9` pointer (keys to taps): `pointer u8` (0 to 9) `+ action u8` (as a
//!   touch) `+ x u16 LE + y u16 LE`: one finger of several at once, for the
//!   app window named before it. A touch is the finger `0`. A peer that
//!   does not know this event drops the whole batch, so a sender keeps
//!   these to batches of their own.
//!
//! Points are fractions of the picture, 0 to 65535 on each axis, so the
//! phone maps them to its own screen whatever size it encodes at.
//! `sent_ms` is the sender's clock, for measuring the delay.

/// Most events in one batch.
pub const MAX_EVENTS: usize = 64;
/// Most text in one event, in bytes.
pub const MAX_TEXT: usize = 1024;
/// Most fingers at once: a [`ControlEvent::Pointer`]'s id is below this.
pub const MAX_POINTERS: u8 = 10;

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
    Touch {
        action: TouchAction,
        x: u16,
        y: u16,
    },
    Scroll {
        x: u16,
        y: u16,
        dx: i8,
        dy: i8,
    },
    Global(Global),
    Text(String),
    Backspace,
    Enter,
    /// The events after this one are for app window `n` (tier 2).
    Window(u8),
    /// Close the app window named before it (tier 2).
    Close,
    /// One finger of several at once in an app window (keys to taps): the
    /// finger `pointer` (below [`MAX_POINTERS`]), what it does and where.
    Pointer {
        pointer: u8,
        action: TouchAction,
        x: u16,
        y: u16,
    },
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
                ControlEvent::Window(window) => {
                    out.push(7);
                    out.push(*window);
                }
                ControlEvent::Close => out.push(8),
                ControlEvent::Pointer {
                    pointer,
                    action,
                    x,
                    y,
                } => {
                    if *pointer >= MAX_POINTERS {
                        return None;
                    }
                    out.push(9);
                    out.push(*pointer);
                    out.push(match action {
                        TouchAction::Down => 0,
                        TouchAction::Move => 1,
                        TouchAction::Up => 2,
                    });
                    out.extend_from_slice(&x.to_le_bytes());
                    out.extend_from_slice(&y.to_le_bytes());
                }
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
                7 => {
                    let window = r.u8()?;
                    if !super::cast::valid_window(window) {
                        return None;
                    }
                    ControlEvent::Window(window)
                }
                8 => ControlEvent::Close,
                9 => {
                    let pointer = r.u8()?;
                    if pointer >= MAX_POINTERS {
                        return None;
                    }
                    let action = match r.u8()? {
                        0 => TouchAction::Down,
                        1 => TouchAction::Move,
                        2 => TouchAction::Up,
                        _ => return None,
                    };
                    ControlEvent::Pointer {
                        pointer,
                        action,
                        x: r.u16()?,
                        y: r.u16()?,
                    }
                }
                _ => return None,
            };
            events.push(event);
        }
        r.0.is_empty().then_some(ControlBatch { sent_ms, events })
    }

    /// The app window the batch's first event names, if it is for one.
    pub fn window(&self) -> Option<u8> {
        match self.events.first() {
            Some(ControlEvent::Window(window)) => Some(*window),
            _ => None,
        }
    }

    /// Whether this batch only closes the app window it names.
    pub fn is_close(&self) -> bool {
        matches!(
            self.events.as_slice(),
            [ControlEvent::Window(_), ControlEvent::Close]
        )
    }

    /// The batch that closes app window `window`; `None` for one that is not.
    pub fn close_window(sent_ms: u64, window: u8) -> Option<ControlBatch> {
        super::cast::valid_window(window).then_some(ControlBatch {
            sent_ms,
            events: vec![ControlEvent::Window(window), ControlEvent::Close],
        })
    }

    /// This batch made for app window `window`: any window events it had
    /// are dropped and one naming `window` leads it, so a viewer of one
    /// window cannot touch another. `None` when `window` is not one or the
    /// batch would then be too large.
    pub fn for_window(self, window: u8) -> Option<ControlBatch> {
        if !super::cast::valid_window(window) {
            return None;
        }
        let mut events = vec![ControlEvent::Window(window)];
        events.extend(
            self.events
                .into_iter()
                .filter(|e| !matches!(e, ControlEvent::Window(_))),
        );
        (events.len() <= MAX_EVENTS).then_some(ControlBatch {
            sent_ms: self.sent_ms,
            events,
        })
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
                ControlEvent::Window(window) => {
                    serde_json::json!({"type": "window", "window": window})
                }
                ControlEvent::Close => serde_json::json!({"type": "close"}),
                ControlEvent::Pointer {
                    pointer,
                    action,
                    x,
                    y,
                } => serde_json::json!({
                    "type": "pointer",
                    "pointer": pointer,
                    "action": match action {
                        TouchAction::Down => "down",
                        TouchAction::Move => "move",
                        TouchAction::Up => "up",
                    },
                    "x": x,
                    "y": y,
                }),
            })
            .collect();
        serde_json::json!({"sent": self.sent_ms, "events": events})
    }
}

/// Whether the computer's cast session may pass a control batch (as the
/// bytes on the wire) to the phone: the phone said it takes control of its
/// screen (`screen_control`), or the batch is for an app window whose
/// `window_input` bit (bit `n - 1` for window `n`) the phone set, or it only
/// closes an app window (the owner closing a window never waits on its input
/// switch). A batch for a window is never passed on the screen's say-so
/// alone, nor the other way round.
pub fn passes(batch: &[u8], screen_control: bool, window_input: u8) -> bool {
    let Some(batch) = ControlBatch::decode(batch) else {
        return false;
    };
    match batch.window() {
        None => screen_control,
        Some(window) => batch.is_close() || window_input & (1 << (window - 1)) != 0,
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
    fn a_window_event_names_the_window_the_events_after_it_are_for() {
        let batch = ControlBatch {
            sent_ms: 5,
            events: vec![
                ControlEvent::Window(3),
                ControlEvent::Touch {
                    action: TouchAction::Down,
                    x: 1,
                    y: 2,
                },
            ],
        };
        let bytes = batch.encode().unwrap();
        let back = ControlBatch::decode(&bytes).unwrap();
        assert_eq!(back, batch);
        assert_eq!(back.window(), Some(3));
        assert_eq!(back.to_json()["events"][0]["window"], 3);
        assert_eq!(every_kind().window(), None);
        // A window that is not one is refused when read.
        let mut bad = 7u64.to_le_bytes().to_vec();
        bad.extend_from_slice(&[1, 7, 0]);
        assert_eq!(ControlBatch::decode(&bad), None, "window 0");
        let mut past = 7u64.to_le_bytes().to_vec();
        past.extend_from_slice(&[1, 7, super::super::cast::MAX_WINDOWS + 1]);
        assert_eq!(ControlBatch::decode(&past), None, "past the cap");
    }

    #[test]
    fn several_fingers_at_once_come_back_and_stay_within_the_cap() {
        let batch = ControlBatch {
            sent_ms: 9,
            events: vec![
                ControlEvent::Window(2),
                ControlEvent::Pointer {
                    pointer: 1,
                    action: TouchAction::Down,
                    x: 100,
                    y: 200,
                },
                ControlEvent::Pointer {
                    pointer: MAX_POINTERS - 1,
                    action: TouchAction::Move,
                    x: 65535,
                    y: 0,
                },
                ControlEvent::Pointer {
                    pointer: 1,
                    action: TouchAction::Up,
                    x: 101,
                    y: 201,
                },
            ],
        };
        let bytes = batch.encode().unwrap();
        let back = ControlBatch::decode(&bytes).unwrap();
        assert_eq!(back, batch);
        let json = back.to_json();
        assert_eq!(json["events"][1]["type"], "pointer");
        assert_eq!(json["events"][1]["pointer"], 1);
        assert_eq!(json["events"][1]["action"], "down");
        assert_eq!(json["events"][2]["pointer"], MAX_POINTERS - 1);
        // A finger past the cap is neither sent nor read.
        let past = ControlBatch {
            sent_ms: 9,
            events: vec![ControlEvent::Pointer {
                pointer: MAX_POINTERS,
                action: TouchAction::Down,
                x: 0,
                y: 0,
            }],
        };
        assert_eq!(past.encode(), None);
        let mut wire = 7u64.to_le_bytes().to_vec();
        wire.extend_from_slice(&[1, 9, MAX_POINTERS, 0, 0, 0, 0, 0]);
        assert_eq!(ControlBatch::decode(&wire), None, "a finger past the cap");
        let mut action = 7u64.to_le_bytes().to_vec();
        action.extend_from_slice(&[1, 9, 1, 3, 0, 0, 0, 0]);
        assert_eq!(ControlBatch::decode(&action), None, "an action unknown");
        // A viewer of one window still makes the fingers its window's.
        let made = batch.for_window(3).unwrap();
        assert_eq!(made.window(), Some(3));
        assert!(made.encode().is_some());
    }

    #[test]
    fn a_window_viewer_s_batch_is_made_for_its_window_only() {
        let mut batch = every_kind();
        batch.events.insert(2, ControlEvent::Window(4));
        let made = batch.clone().for_window(2).unwrap();
        assert_eq!(made.window(), Some(2));
        assert_eq!(
            made.events
                .iter()
                .filter(|e| matches!(e, ControlEvent::Window(_)))
                .count(),
            1,
            "any window the viewer named is dropped"
        );
        assert_eq!(made.events.len(), every_kind().events.len() + 1);
        assert_eq!(batch.clone().for_window(0), None);
        assert_eq!(batch.for_window(super::super::cast::MAX_WINDOWS + 1), None);
        let full = ControlBatch {
            sent_ms: 0,
            events: vec![ControlEvent::Enter; MAX_EVENTS],
        };
        assert_eq!(full.for_window(1), None, "no room for the window event");
    }

    #[test]
    fn the_session_passes_a_batch_by_what_the_phone_said() {
        let screen = every_kind().encode().unwrap();
        let w2 = every_kind().for_window(2).unwrap().encode().unwrap();
        assert!(passes(&screen, true, 0));
        assert!(
            !passes(&screen, false, 0b1111),
            "windows' input is not the screen's"
        );
        assert!(passes(&w2, false, 0b10));
        assert!(
            !passes(&w2, true, 0b01),
            "the screen's control is not a window's"
        );
        assert!(!passes(&w2, true, 0b1101), "window 2's bit is off");
        assert!(!passes(&[1, 2, 3], true, 0b1111), "not a batch");
        // Closing a window needs no input switch, and closes only the window named.
        let close = ControlBatch::close_window(1, 3).unwrap();
        assert!(close.is_close());
        let close = close.encode().unwrap();
        assert!(passes(&close, false, 0), "a close passes with input off");
        assert_eq!(ControlBatch::close_window(1, 0), None);
        // A batch of a window event and more is not a close.
        assert!(
            !ControlBatch {
                sent_ms: 1,
                events: vec![
                    ControlEvent::Window(1),
                    ControlEvent::Close,
                    ControlEvent::Enter
                ],
            }
            .is_close()
        );
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
