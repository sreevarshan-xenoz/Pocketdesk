//! The owner's phone as another screen for this computer's keyboard and
//! mouse (BK5).
//!
//! A phone whose owner lets it says its screen on its control link
//! ([`SCREEN`](pocketdesk_core::protocol::phone_screen::SCREEN), kept by
//! [`heard`] and, as a block, in `shared_screens.json`); the driver places
//! it and crosses onto it as onto a computer. Whether it may be driven now
//! is [`standing`]: linked, its own switch and Accessibility service on, and
//! this computer's Keyboard and mouse permission for it on. While the
//! cursor is on it, what the driver would send a computer ([`Msg`]) is put
//! in the phone's words ([`ToPhone`]) and sent in batches at most one every
//! [`PACE`] ([`PhoneChannel`]); the phone answers the driver's pings
//! ([`ponged`]), so a phone gone quiet brings the mouse home as a computer
//! does.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pocketdesk_core::local::shared_input::{
    PHONE_AWAY, PHONE_NEEDS_SERVICE, PHONE_NOT_ALLOWED, PHONE_OFF, PHONE_READY,
};
use pocketdesk_core::protocol::phone_screen::{
    PhoneButton, PhoneEvent, PhoneKey, Press, ScreenReport,
};

use super::wire::Msg;

/// The least time between two batches to a phone: the moves between are
/// kept to the last.
pub const PACE: Duration = Duration::from_millis(8);

/// How long a phone's report is current: it says it again every 15 s while
/// its switch is on.
pub const REPORT_FRESH: Duration = Duration::from_secs(45);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// What each phone last said of itself, and when.
static REPORTS: LazyLock<Mutex<HashMap<String, (ScreenReport, Instant)>>> =
    LazyLock::new(Default::default);

/// When each phone last answered a ping, and how many answers it gave.
static PONGS: LazyLock<Mutex<HashMap<String, (Instant, u64)>>> = LazyLock::new(Default::default);

/// Keep what the phone `device_id` said of itself.
pub fn heard(device_id: &str, report: ScreenReport) {
    lock(&REPORTS).insert(device_id.to_string(), (report, Instant::now()));
}

/// What the phone `device_id` last said of itself, if it is current.
pub fn report(device_id: &str) -> Option<ScreenReport> {
    lock(&REPORTS)
        .get(device_id)
        .filter(|(_, at)| at.elapsed() < REPORT_FRESH)
        .map(|(report, _)| *report)
}

/// The phone `device_id` answered a ping.
pub fn ponged(device_id: &str) {
    let mut pongs = lock(&PONGS);
    let entry = pongs
        .entry(device_id.to_string())
        .or_insert((Instant::now(), 0));
    *entry = (Instant::now(), entry.1 + 1);
}

/// When the phone `device_id` last answered a ping, and how many answers
/// it gave this run.
pub fn pongs(device_id: &str) -> Option<(Instant, u64)> {
    lock(&PONGS).get(device_id).copied()
}

/// The devices with a link to this server now, as last looked at
/// ([`keep_linked`]).
static LINKED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// Whether `device_id` has a link to this server now.
pub fn is_linked(device_id: &str) -> bool {
    lock(&LINKED).contains(device_id)
}

/// Look at which devices have a link twice a second, for the server's
/// life, so the driver's thread and the status know without waiting.
pub async fn keep_linked() {
    let mut every = tokio::time::interval(Duration::from_millis(500));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        every.tick().await;
        let now: HashSet<String> = crate::state::clone_clients()
            .await
            .iter()
            .map(|c| c.device_id().to_string())
            .collect();
        *lock(&LINKED) = now;
    }
}

/// The phone `device_id`'s standing now: what it said, its link, and this
/// computer's Keyboard and mouse permission for it.
pub fn standing_now(device_id: &str) -> &'static str {
    let report = report(device_id);
    let allowed = report.is_some()
        && crate::plugins::pair::allows(
            device_id,
            pocketdesk_core::protocol::permissions::Capability::SharedInput,
        );
    standing(report.as_ref(), is_linked(device_id), allowed)
}

/// The phone's standing, in the status file's words: [`PHONE_READY`] when
/// the keyboard and mouse may go onto it, else why not. It must be
/// `linked` and have said a current `report`; then its own switch, its
/// Accessibility service, and this computer's Keyboard and mouse
/// permission for it (`allowed_here`), in that order.
pub fn standing(report: Option<&ScreenReport>, linked: bool, allowed_here: bool) -> &'static str {
    match report {
        Some(_) if !linked => PHONE_AWAY,
        None => PHONE_AWAY,
        Some(r) if !r.allows => PHONE_OFF,
        Some(r) if !r.service => PHONE_NEEDS_SERVICE,
        Some(_) if !allowed_here => PHONE_NOT_ALLOWED,
        Some(_) => PHONE_READY,
    }
}

/// The keys a US keyboard types, as evdev codes: unshifted and shifted.
const KEYMAP: [(u16, char, char); 52] = [
    (2, '1', '!'),
    (3, '2', '@'),
    (4, '3', '#'),
    (5, '4', '$'),
    (6, '5', '%'),
    (7, '6', '^'),
    (8, '7', '&'),
    (9, '8', '*'),
    (10, '9', '('),
    (11, '0', ')'),
    (12, '-', '_'),
    (13, '=', '+'),
    (16, 'q', 'Q'),
    (17, 'w', 'W'),
    (18, 'e', 'E'),
    (19, 'r', 'R'),
    (20, 't', 'T'),
    (21, 'y', 'Y'),
    (22, 'u', 'U'),
    (23, 'i', 'I'),
    (24, 'o', 'O'),
    (25, 'p', 'P'),
    (26, '[', '{'),
    (27, ']', '}'),
    (30, 'a', 'A'),
    (31, 's', 'S'),
    (32, 'd', 'D'),
    (33, 'f', 'F'),
    (34, 'g', 'G'),
    (35, 'h', 'H'),
    (36, 'j', 'J'),
    (37, 'k', 'K'),
    (38, 'l', 'L'),
    (39, ';', ':'),
    (40, '\'', '"'),
    (41, '`', '~'),
    (43, '\\', '|'),
    (44, 'z', 'Z'),
    (45, 'x', 'X'),
    (46, 'c', 'C'),
    (47, 'v', 'V'),
    (48, 'b', 'B'),
    (49, 'n', 'N'),
    (50, 'm', 'M'),
    (51, ',', '<'),
    (52, '.', '>'),
    (53, '/', '?'),
    (57, ' ', ' '),
    (55, '*', '*'),
    (74, '-', '-'),
    (78, '+', '+'),
    (98, '/', '/'),
];

/// The number pad's digits and point, as evdev codes (Num Lock taken as on).
const KEYPAD: [(u16, char); 11] = [
    (71, '7'),
    (72, '8'),
    (73, '9'),
    (75, '4'),
    (76, '5'),
    (77, '6'),
    (79, '1'),
    (80, '2'),
    (81, '3'),
    (82, '0'),
    (83, '.'),
];

/// A key held on the phone repeats after this (the review's M1): the
/// driver sends a key down once, as a computer repeats by itself.
pub const REPEAT_DELAY: Duration = Duration::from_millis(500);

/// And then once every this long, about 30 a second.
pub const REPEAT_EVERY: Duration = Duration::from_millis(33);

/// The most repeats one look sends, so a late look does not type a burst.
const REPEATS_AT_ONCE: u32 = 5;

/// A key held down and what it does there, since when, and how many times
/// it has repeated.
#[derive(Debug)]
struct Held {
    code: u16,
    event: PhoneEvent,
    since: Instant,
    repeated: u32,
}

/// The keys that are no text, as evdev codes.
const KEYS: [(u16, PhoneKey); 9] = [
    (28, PhoneKey::Enter),
    (96, PhoneKey::Enter),
    (14, PhoneKey::Backspace),
    (111, PhoneKey::Delete),
    (1, PhoneKey::Escape),
    (105, PhoneKey::Left),
    (106, PhoneKey::Right),
    (103, PhoneKey::Up),
    (108, PhoneKey::Down),
];

const LEFT_SHIFT: u16 = 42;
const RIGHT_SHIFT: u16 = 54;
const CAPS_LOCK: u16 = 58;
/// Ctrl, Alt (AltGr too) and the Windows keys: held, a key is a shortcut
/// on the computer, not text on the phone.
const SHORTCUT_KEYS: [u16; 6] = [29, 97, 56, 100, 125, 126];

/// The key that types `c` on a US keyboard, as an evdev code, and whether
/// Shift is held for it (the test sender's way back).
pub fn code_of(c: char) -> Option<(u16, bool)> {
    KEYMAP.iter().find_map(|(code, plain, shifted)| {
        if *plain == c {
            Some((*code, false))
        } else if *shifted == c {
            Some((*code, true))
        } else {
            None
        }
    })
}

/// What the driver sends a computer, put in the phone's words: positions
/// as they are (the same fractions out of 65535), the left, middle and
/// right buttons, the wheel, keys as text on a US keyboard (Shift and Caps
/// Lock followed) or as the keys that are no text, and pings. Keys held
/// with Ctrl, Alt or a Windows key, the side buttons, and what only
/// computers say (screens, activity, Identify) send nothing.
#[derive(Debug, Default)]
pub struct ToPhone {
    shift: HashSet<u16>,
    shortcut: HashSet<u16>,
    caps: bool,
    /// The key held now, repeated by [`ToPhone::repeats`].
    held: Option<Held>,
}

impl ToPhone {
    /// The key held down, repeated: after [`REPEAT_DELAY`], once every
    /// [`REPEAT_EVERY`] until it is let go of, another key goes down or the
    /// cursor leaves (Enter and Esc never repeat, nor a shortcut).
    pub fn repeats(&mut self, now: Instant) -> Vec<PhoneEvent> {
        let Some(held) = self.held.as_mut() else {
            return Vec::new();
        };
        let since = now.saturating_duration_since(held.since);
        let Some(past) = since.checked_sub(REPEAT_DELAY) else {
            return Vec::new();
        };
        let due =
            1 + u32::try_from(past.as_millis() / REPEAT_EVERY.as_millis()).unwrap_or(u32::MAX);
        let n = due.saturating_sub(held.repeated).min(REPEATS_AT_ONCE);
        // A late look skips what it did not send, as a slow key repeat does.
        held.repeated = due;
        vec![held.event.clone(); n as usize]
    }

    /// The key `code` went down (`pressed`) or up, doing `event` there:
    /// the one held now, if it repeats.
    fn note_held(&mut self, code: u16, pressed: bool, event: Option<&PhoneEvent>, now: Instant) {
        if !pressed {
            if self.held.as_ref().is_some_and(|h| h.code == code) {
                self.held = None;
            }
            return;
        }
        if [LEFT_SHIFT, RIGHT_SHIFT, CAPS_LOCK].contains(&code) || SHORTCUT_KEYS.contains(&code) {
            return;
        }
        let repeats = |e: &&PhoneEvent| {
            !matches!(
                e,
                PhoneEvent::Key {
                    key: PhoneKey::Enter | PhoneKey::Escape
                }
            )
        };
        self.held = event.filter(repeats).map(|event| Held {
            code,
            event: event.clone(),
            since: now,
            repeated: 0,
        });
    }

    /// [`ToPhone::events_at`] now.
    pub fn events(&mut self, msg: &Msg) -> Vec<PhoneEvent> {
        self.events_at(msg, Instant::now())
    }

    /// `msg` in the phone's words, come at `now`.
    pub fn events_at(&mut self, msg: &Msg, now: Instant) -> Vec<PhoneEvent> {
        let event = match *msg {
            Msg::Enter { x, y } => {
                // Nothing held from a visit before.
                *self = ToPhone::default();
                PhoneEvent::Enter { x, y }
            }
            Msg::Leave => {
                *self = ToPhone::default();
                PhoneEvent::Leave
            }
            Msg::Move { x, y, .. } => PhoneEvent::Pointer { x, y },
            Msg::Button { button, pressed } => {
                let button = match button {
                    0 => PhoneButton::Left,
                    1 => PhoneButton::Middle,
                    2 => PhoneButton::Right,
                    _ => return Vec::new(),
                };
                let action = if pressed { Press::Down } else { Press::Up };
                PhoneEvent::Button { button, action }
            }
            Msg::Wheel { dx, dy } => PhoneEvent::Wheel { dx, dy },
            Msg::Ping { t_us } => PhoneEvent::Ping { t: t_us },
            Msg::Key { code, pressed } => {
                let event = self.key(code, pressed);
                self.note_held(code, pressed, event.as_ref(), now);
                match event {
                    Some(event) => event,
                    None => return Vec::new(),
                }
            }
            _ => return Vec::new(),
        };
        vec![event]
    }

    /// A key going down or up: what it types or does there, if anything.
    fn key(&mut self, code: u16, pressed: bool) -> Option<PhoneEvent> {
        if code == LEFT_SHIFT || code == RIGHT_SHIFT {
            hold(&mut self.shift, code, pressed);
            return None;
        }
        if SHORTCUT_KEYS.contains(&code) {
            hold(&mut self.shortcut, code, pressed);
            return None;
        }
        if !pressed {
            return None;
        }
        if code == CAPS_LOCK {
            self.caps = !self.caps;
            return None;
        }
        if !self.shortcut.is_empty() {
            return None;
        }
        if let Some((_, key)) = KEYS.iter().find(|(c, _)| *c == code) {
            return Some(PhoneEvent::Key { key: *key });
        }
        let shift = !self.shift.is_empty();
        let typed = KEYPAD
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, digit)| *digit)
            .or_else(|| {
                let (_, plain, shifted) = KEYMAP.iter().find(|(c, _, _)| *c == code)?;
                // Caps Lock turns letters only, and Shift turns them back.
                let upper = if plain.is_ascii_alphabetic() {
                    shift != self.caps
                } else {
                    shift
                };
                Some(if upper { *shifted } else { *plain })
            })?;
        Some(PhoneEvent::Text {
            text: typed.to_string(),
        })
    }
}

/// `code` held (`down`) or let go of, in `held`.
fn hold(held: &mut HashSet<u16>, code: u16, down: bool) {
    if down {
        held.insert(code);
    } else {
        held.remove(&code);
    }
}

/// The channel to a phone driven: what the driver sends it, in the
/// phone's words, sent on its control link by a task of its own in
/// batches at most one every [`PACE`]. The task ends when the phone's link
/// is gone or a send fails; the next [`PhoneChannel::send`] then fails, and
/// the driver brings the keyboard and mouse home as for a computer.
pub struct PhoneChannel {
    events: tokio::sync::mpsc::UnboundedSender<PhoneEvent>,
    to_phone: ToPhone,
}

impl PhoneChannel {
    /// Open one to the phone `device_id` (inside the runtime).
    pub fn open(device_id: &str) -> PhoneChannel {
        let (events, taken) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(pump(device_id.to_string(), taken));
        PhoneChannel {
            events,
            to_phone: ToPhone::default(),
        }
    }

    /// Hand `msg` on, in the phone's words; an error once the phone's link
    /// is gone.
    pub fn send(&mut self, msg: &Msg) -> Result<(), String> {
        if self.events.is_closed() {
            return Err("the phone's link is gone".to_string());
        }
        for event in self.to_phone.events(msg) {
            self.events
                .send(event)
                .map_err(|_| "the phone's link is gone".to_string())?;
        }
        Ok(())
    }

    /// Send what the key held repeats by `now` ([`ToPhone::repeats`]);
    /// looked at on each turn of the driver's loop.
    pub fn repeat(&mut self, now: Instant) {
        for event in self.to_phone.repeats(now) {
            if self.events.send(event).is_err() {
                return;
            }
        }
    }

    /// Whether its task has ended (the phone's link is gone).
    pub fn is_closed(&self) -> bool {
        self.events.is_closed()
    }
}

/// Send what comes in `taken` to the phone `device_id`: the first event at
/// once, then what came meanwhile after [`PACE`], as batches
/// ([`batches`](pocketdesk_core::protocol::phone_screen::batches)). Ends with the phone's link, or when the
/// channel is dropped and what it held is sent.
async fn pump(device_id: String, mut taken: tokio::sync::mpsc::UnboundedReceiver<PhoneEvent>) {
    use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
    use pocketdesk_core::protocol::phone_screen::{INPUT, batches, body};
    let mut sent_at: Option<tokio::time::Instant> = None;
    while let Some(first) = taken.recv().await {
        if let Some(at) = sent_at {
            tokio::time::sleep_until(at + PACE).await;
        }
        let mut events = vec![first];
        while let Ok(more) = taken.try_recv() {
            events.push(more);
        }
        let clients = crate::state::clone_clients().await;
        let Some(phone) = clients.iter().find(|c| c.device_id() == device_id) else {
            tracing::debug!("Shared input: the phone is not linked");
            return;
        };
        for batch in batches(events) {
            let packet = NetworkPacket::new(INPUT).with_body(body(&batch));
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::debug!("Shared input: not sent to the phone: {e}");
                return;
            }
        }
        sent_at = Some(tokio::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_screen::PhoneScreen;

    fn report(allows: bool, service: bool) -> ScreenReport {
        ScreenReport {
            allows,
            service,
            screen: PhoneScreen {
                width_px: 1080,
                height_px: 2400,
                width_dp: 411,
                height_dp: 914,
                rotation: 0,
            },
        }
    }

    /// Input goes to a phone only when it allows it and so does this
    /// computer; the first reason not is the one said.
    #[test]
    fn a_phone_is_driven_only_when_both_sides_allow_it() {
        let ready = report(true, true);
        assert_eq!(standing(Some(&ready), true, true), PHONE_READY);
        assert_eq!(standing(Some(&ready), false, true), PHONE_AWAY);
        assert_eq!(standing(None, true, true), PHONE_AWAY, "said nothing");
        assert_eq!(standing(Some(&report(false, true)), true, true), PHONE_OFF);
        assert_eq!(
            standing(Some(&report(true, false)), true, true),
            PHONE_NEEDS_SERVICE
        );
        assert_eq!(standing(Some(&ready), true, false), PHONE_NOT_ALLOWED);
        // Off there and not allowed here: the phone's is said first.
        assert_eq!(
            standing(Some(&report(false, false)), true, false),
            PHONE_OFF
        );
    }

    #[test]
    fn a_report_is_current_for_a_while_and_pongs_are_counted() {
        heard("phone-report", report(true, true));
        assert_eq!(super::report("phone-report"), Some(report(true, true)));
        assert_eq!(super::report("phone-never"), None);
        assert_eq!(pongs("phone-pong"), None);
        ponged("phone-pong");
        ponged("phone-pong");
        assert_eq!(pongs("phone-pong").map(|(_, n)| n), Some(2));
    }

    fn key(code: u16, pressed: bool) -> Msg {
        Msg::Key { code, pressed }
    }

    fn text(t: &str) -> Vec<PhoneEvent> {
        vec![PhoneEvent::Text {
            text: t.to_string(),
        }]
    }

    #[test]
    fn the_pointer_buttons_and_wheel_go_as_they_are() {
        let mut to = ToPhone::default();
        assert_eq!(
            to.events(&Msg::Enter { x: 0, y: 65535 }),
            vec![PhoneEvent::Enter { x: 0, y: 65535 }]
        );
        assert_eq!(
            to.events(&Msg::Move {
                seq: 4,
                x: 100,
                y: 200
            }),
            vec![PhoneEvent::Pointer { x: 100, y: 200 }]
        );
        let button = |button, pressed| Msg::Button { button, pressed };
        let on_phone = |button, action| vec![PhoneEvent::Button { button, action }];
        assert_eq!(
            to.events(&button(0, true)),
            on_phone(PhoneButton::Left, Press::Down)
        );
        assert_eq!(
            to.events(&button(0, false)),
            on_phone(PhoneButton::Left, Press::Up)
        );
        assert_eq!(
            to.events(&button(1, true)),
            on_phone(PhoneButton::Middle, Press::Down)
        );
        assert_eq!(
            to.events(&button(2, false)),
            on_phone(PhoneButton::Right, Press::Up)
        );
        // The side buttons do nothing there.
        assert!(to.events(&button(3, true)).is_empty());
        assert_eq!(
            to.events(&Msg::Wheel { dx: 0, dy: 3 }),
            vec![PhoneEvent::Wheel { dx: 0, dy: 3 }]
        );
        assert_eq!(
            to.events(&Msg::Ping { t_us: 9 }),
            vec![PhoneEvent::Ping { t: 9 }]
        );
        assert_eq!(to.events(&Msg::Leave), vec![PhoneEvent::Leave]);
        // What only computers say goes nowhere.
        for msg in [
            Msg::Active { idle_ms: 1 },
            Msg::Screens(Vec::new()),
            Msg::Identify {
                name: "Desk".to_string(),
            },
            Msg::ReleaseAll,
            Msg::Pong { t_us: 1, at_us: 2 },
        ] {
            assert!(to.events(&msg).is_empty(), "{msg:?}");
        }
    }

    #[test]
    fn keys_type_text_with_shift_and_caps_lock() {
        let mut to = ToPhone::default();
        assert_eq!(to.events(&key(35, true)), text("h"));
        assert!(
            to.events(&key(35, false)).is_empty(),
            "a release types nothing"
        );
        assert_eq!(to.events(&key(57, true)), text(" "));
        // Shift held: shifted, either Shift.
        assert!(to.events(&key(LEFT_SHIFT, true)).is_empty());
        assert_eq!(to.events(&key(2, true)), text("!"));
        assert_eq!(to.events(&key(30, true)), text("A"));
        assert!(to.events(&key(LEFT_SHIFT, false)).is_empty());
        assert_eq!(to.events(&key(30, true)), text("a"));
        to.events(&key(RIGHT_SHIFT, true));
        assert_eq!(to.events(&key(53, true)), text("?"));
        to.events(&key(RIGHT_SHIFT, false));
        // Caps Lock turns letters, not digits; Shift turns them back.
        to.events(&key(CAPS_LOCK, true));
        to.events(&key(CAPS_LOCK, false));
        assert_eq!(to.events(&key(30, true)), text("A"));
        assert_eq!(to.events(&key(2, true)), text("1"));
        to.events(&key(LEFT_SHIFT, true));
        assert_eq!(to.events(&key(30, true)), text("a"));
        to.events(&key(LEFT_SHIFT, false));
        to.events(&key(CAPS_LOCK, true));
        assert_eq!(to.events(&key(30, true)), text("a"));
        // The number pad types its digits.
        assert_eq!(to.events(&key(79, true)), text("1"));
    }

    #[test]
    fn keys_that_are_no_text_and_shortcuts() {
        let mut to = ToPhone::default();
        let k = |key| vec![PhoneEvent::Key { key }];
        assert_eq!(to.events(&key(28, true)), k(PhoneKey::Enter));
        assert_eq!(to.events(&key(14, true)), k(PhoneKey::Backspace));
        assert_eq!(to.events(&key(1, true)), k(PhoneKey::Escape));
        assert_eq!(to.events(&key(105, true)), k(PhoneKey::Left));
        assert_eq!(to.events(&key(108, true)), k(PhoneKey::Down));
        assert_eq!(to.events(&key(111, true)), k(PhoneKey::Delete));
        // Held with Ctrl, Alt or a Windows key: a shortcut, nothing typed.
        for held in SHORTCUT_KEYS {
            to.events(&key(held, true));
            assert!(to.events(&key(46, true)).is_empty(), "{held}");
            assert!(to.events(&key(28, true)).is_empty(), "{held}");
            to.events(&key(held, false));
        }
        assert_eq!(to.events(&key(46, true)), text("c"));
        // A key with no meaning there (F1) types nothing.
        assert!(to.events(&key(59, true)).is_empty());
        // Coming onto the phone again, nothing is held from before.
        to.events(&key(LEFT_SHIFT, true));
        to.events(&key(29, true));
        to.events(&Msg::Enter { x: 1, y: 1 });
        assert_eq!(to.events(&key(30, true)), text("a"));
    }

    /// The review's M1: a key held on the phone repeats after half a
    /// second, about 30 a second, until it is let go of; Enter and Esc never
    /// do, and leaving or another key stops it.
    #[test]
    fn a_key_held_repeats_until_it_is_let_go_of() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut to = ToPhone::default();
        assert_eq!(
            to.events_at(&key(14, true), t0),
            vec![PhoneEvent::Key {
                key: PhoneKey::Backspace
            }]
        );
        assert!(to.repeats(at(499)).is_empty(), "not yet");
        assert_eq!(
            to.repeats(at(500)),
            vec![PhoneEvent::Key {
                key: PhoneKey::Backspace
            }]
        );
        // 533, 566 and 599 since.
        assert_eq!(to.repeats(at(600)).len(), 3);
        assert!(to.repeats(at(600)).is_empty(), "each once");
        // A late look sends a few, not a burst.
        assert_eq!(to.repeats(at(5_000)).len(), REPEATS_AT_ONCE as usize);
        to.events_at(&key(14, false), at(5_010));
        assert!(to.repeats(at(6_000)).is_empty(), "let go of");
        // Text repeats as text.
        to.events_at(&key(30, true), at(7_000));
        assert_eq!(to.repeats(at(7_500)), text("a"));
        // Another key down takes over; leaving stops it.
        to.events_at(&key(31, true), at(7_510));
        assert!(to.repeats(at(7_600)).is_empty(), "its own half second");
        assert_eq!(to.repeats(at(8_010)), text("s"));
        to.events_at(&Msg::Leave, at(8_020));
        assert!(to.repeats(at(9_000)).is_empty());
        // Enter and Esc, and a shortcut, never repeat.
        for code in [28, 1] {
            to.events_at(&key(code, true), at(10_000));
            assert!(to.repeats(at(11_000)).is_empty(), "{code}");
            to.events_at(&key(code, false), at(11_000));
        }
        to.events_at(&key(29, true), at(12_000));
        to.events_at(&key(46, true), at(12_000));
        assert!(to.repeats(at(13_000)).is_empty(), "Ctrl+C");
    }

    #[test]
    fn each_character_typed_has_its_key_back() {
        for (code, plain, shifted) in KEYMAP {
            if [55, 74, 78, 98].contains(&code) {
                continue; // the number pad's: the main keys type these
            }
            assert_eq!(code_of(plain), Some((code, false)), "{plain}");
            if shifted != plain {
                assert_eq!(code_of(shifted), Some((code, true)), "{shifted}");
            }
        }
        assert_eq!(code_of('é'), None);
        // Round trip through the translator.
        let mut to = ToPhone::default();
        let mut typed = String::new();
        for c in "Hello, World! 42?".chars() {
            let (code, shift) = code_of(c).unwrap();
            if shift {
                to.events(&key(LEFT_SHIFT, true));
            }
            for e in to.events(&key(code, true)) {
                if let PhoneEvent::Text { text } = e {
                    typed.push_str(&text);
                }
            }
            if shift {
                to.events(&key(LEFT_SHIFT, false));
            }
        }
        assert_eq!(typed, "Hello, World! 42?");
    }
}
