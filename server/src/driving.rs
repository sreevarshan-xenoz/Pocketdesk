//! Who drives when several phones are connected (roadmap N7). The input
//! task asks [`admit`] before injecting a packet; the live-sessions watcher
//! applies the desktop's requests ([`give`]), mirrors the driver, and tells
//! the phones in a session when it changes ([`control_bodies`]).

use pocketdesk_core::input::control::{Admit, InputControl, Policy};
use pocketdesk_core::local::config::InputControlMode;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::streaming::InputPacket;
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const CONTROL: &str = "kdeconnect.pocketdesk.control";

fn control() -> &'static Mutex<InputControl> {
    static CONTROL: OnceLock<Mutex<InputControl>> = OnceLock::new();
    CONTROL.get_or_init(|| {
        Mutex::new(InputControl::new(Policy::One {
            idle: Duration::from_secs(10),
        }))
    })
}

/// The user's settings, read at start.
pub fn configure(mode: InputControlMode, idle_secs: u32) {
    let policy = match mode {
        InputControlMode::One => Policy::One {
            idle: Duration::from_secs(u64::from(idle_secs.clamp(3, 120))),
        },
        InputControlMode::Shared => Policy::Shared,
    };
    if let Ok(mut control) = control().lock() {
        *control = InputControl::new(policy);
    }
}

/// A live session: its id, and the device it belongs to.
pub type LiveRef = (u64, Option<String>);

/// What becomes of one input packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drive {
    Inject,
    Drop,
}

fn device_of(live: &[LiveRef], session: u64) -> Option<&str> {
    live.iter()
        .find(|(id, _)| *id == session)
        .and_then(|(_, device)| device.as_deref())
}

/// The rule for one packet from `session`, given the sessions now `live`:
/// - a session that has ended injects nothing;
/// - a driver whose session ended lets go;
/// - the driving phone's own newer session (a reconnect, a Retry) takes
///   over from its stale one instead of waiting for it to go quiet;
/// - otherwise [`InputControl::admit`] decides.
pub fn decide(control: &mut InputControl, session: u64, live: &[LiveRef], now: Instant) -> Drive {
    if !live.iter().any(|(id, _)| *id == session) {
        return Drive::Drop;
    }
    if let Some(driver) = control.driver(now) {
        if !live.iter().any(|(id, _)| *id == driver) {
            control.release(driver);
        } else if driver != session
            && device_of(live, driver).is_some()
            && device_of(live, driver) == device_of(live, session)
        {
            control.give(session, now);
        }
    }
    match control.admit(session, now) {
        Admit::Yes { .. } => Drive::Inject,
        Admit::No { .. } => Drive::Drop,
    }
}

/// Whether `device` may drive through its control channel (mouse and
/// keyboard packets on TCP): when no one drives, or a session of its own
/// does.
pub fn device_may_drive(
    control: &InputControl,
    device: &str,
    live: &[LiveRef],
    now: Instant,
) -> bool {
    match control
        .driver(now)
        .filter(|d| live.iter().any(|(id, _)| id == d))
    {
        None => true,
        Some(driver) => device_of(live, driver) == Some(device),
    }
}

/// The live sessions, as [`decide`] takes them.
pub fn live_refs() -> Vec<LiveRef> {
    pocketdesk_core::streaming::sessions::list()
        .into_iter()
        .map(|s| (s.id, s.device_id))
        .collect()
}

/// [`decide`] on the server's control.
pub fn admit(session: u64, live: &[LiveRef]) -> Drive {
    match control().lock() {
        Ok(mut control) => decide(&mut control, session, live, Instant::now()),
        Err(_) => Drive::Inject,
    }
}

/// [`device_may_drive`] on the server's control.
pub fn may_drive(device: &str) -> bool {
    let live = live_refs();
    control()
        .lock()
        .map(|c| device_may_drive(&c, device, &live, Instant::now()))
        .unwrap_or(true)
}

/// Every phone drives (`input_control = "shared"`).
pub fn is_shared() -> bool {
    control()
        .lock()
        .map(|c| c.policy() == Policy::Shared)
        .unwrap_or(false)
}

/// The mouse buttons and keys one session holds down, so they can be let
/// go when control moves away from it or its session ends: its own
/// releases would be dropped, or never come.
#[derive(Debug, Default)]
pub struct Held {
    buttons: BTreeSet<u8>,
    keys: BTreeSet<u16>,
}

impl Held {
    /// Follow a packet that was injected.
    pub fn note(&mut self, packet: &InputPacket) {
        match packet {
            InputPacket::MouseClick { button, pressed } => {
                if *pressed {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
            }
            InputPacket::KeyEvent { key, pressed } => {
                if *pressed {
                    self.keys.insert(*key);
                } else {
                    self.keys.remove(key);
                }
            }
            _ => {}
        }
    }

    /// The releases for what is held, which then counts as let go.
    pub fn take_releases(&mut self) -> Vec<InputPacket> {
        let keys = std::mem::take(&mut self.keys)
            .into_iter()
            .map(|key| InputPacket::KeyEvent {
                key,
                pressed: false,
            });
        let buttons =
            std::mem::take(&mut self.buttons)
                .into_iter()
                .map(|button| InputPacket::MouseClick {
                    button,
                    pressed: false,
                });
        keys.chain(buttons).collect()
    }
}

/// The desktop hands control to `session`.
pub fn give(session: u64) -> bool {
    control()
        .lock()
        .map(|mut c| c.give(session, Instant::now()))
        .unwrap_or(false)
}

/// The session driving now, if it is still `live`.
pub fn driver(live: &[u64]) -> Option<u64> {
    control()
        .lock()
        .ok()
        .and_then(|c| c.driver(Instant::now()))
        .filter(|d| live.contains(d))
}

/// What each phone in a session is told: `driving` for the driver's phone,
/// `other` while another phone drives. `sessions` holds (session id,
/// device id); a phone in several sessions drives if any of them does.
pub fn control_bodies(
    sessions: &[(u64, String)],
    driver: Option<u64>,
) -> Vec<(String, NetworkPacket)> {
    let driving_device = driver.and_then(|d| {
        sessions
            .iter()
            .find(|(id, _)| *id == d)
            .map(|(_, device)| device.clone())
    });
    let mut devices: Vec<&String> = sessions.iter().map(|(_, device)| device).collect();
    devices.sort();
    devices.dedup();
    devices
        .into_iter()
        .map(|device| {
            let driving = driving_device.as_ref() == Some(device);
            let other = driving_device.is_some() && !driving;
            (
                device.clone(),
                NetworkPacket::new(CONTROL).with_body(serde_json::json!({
                    "driving": driving,
                    "other": other,
                })),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::InputPacket;

    fn one() -> InputControl {
        InputControl::new(Policy::One {
            idle: Duration::from_secs(10),
        })
    }

    fn live() -> Vec<LiveRef> {
        vec![
            (1, Some("phone-a".into())),
            (2, Some("phone-b".into())),
            (3, Some("phone-a".into())),
        ]
    }

    #[test]
    fn another_phone_waits_while_one_drives() {
        let (mut control, t) = (one(), Instant::now());
        assert_eq!(decide(&mut control, 1, &live(), t), Drive::Inject);
        assert_eq!(decide(&mut control, 2, &live(), t), Drive::Drop);
    }

    #[test]
    fn the_same_phones_newer_session_takes_over_from_its_stale_one() {
        let (mut control, t) = (one(), Instant::now());
        decide(&mut control, 1, &live(), t);
        assert_eq!(decide(&mut control, 3, &live(), t), Drive::Inject);
        assert_eq!(control.driver(t), Some(3));
    }

    #[test]
    fn input_from_a_session_that_has_ended_is_dropped() {
        let (mut control, t) = (one(), Instant::now());
        let gone = vec![(2, Some("phone-b".to_string()))];
        assert_eq!(decide(&mut control, 1, &gone, t), Drive::Drop);
        assert_eq!(control.driver(t), None, "a dead session takes nothing");
    }

    #[test]
    fn a_driver_whose_session_ended_lets_go() {
        let (mut control, t) = (one(), Instant::now());
        decide(&mut control, 1, &live(), t);
        let after = vec![(2, Some("phone-b".to_string()))];
        assert_eq!(decide(&mut control, 2, &after, t), Drive::Inject);
    }

    #[test]
    fn the_control_channel_drives_only_for_the_driving_phone() {
        let (mut control, t) = (one(), Instant::now());
        assert!(
            device_may_drive(&control, "phone-b", &live(), t),
            "no driver yet"
        );
        decide(&mut control, 1, &live(), t);
        assert!(device_may_drive(&control, "phone-a", &live(), t));
        assert!(!device_may_drive(&control, "phone-b", &live(), t));
        assert!(device_may_drive(
            &InputControl::new(Policy::Shared),
            "phone-b",
            &live(),
            t
        ));
    }

    #[test]
    fn held_buttons_and_keys_are_released_once() {
        let mut held = Held::default();
        held.note(&InputPacket::MouseClick {
            button: 0,
            pressed: true,
        });
        held.note(&InputPacket::KeyEvent {
            key: 42,
            pressed: true,
        });
        held.note(&InputPacket::KeyEvent {
            key: 30,
            pressed: true,
        });
        held.note(&InputPacket::KeyEvent {
            key: 30,
            pressed: false,
        });
        let releases = held.take_releases();
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().any(|p| matches!(
            p,
            InputPacket::MouseClick {
                button: 0,
                pressed: false
            }
        )));
        assert!(releases.iter().any(|p| matches!(
            p,
            InputPacket::KeyEvent {
                key: 42,
                pressed: false
            }
        )));
        assert!(held.take_releases().is_empty());
    }

    fn body(bodies: &[(String, NetworkPacket)], device: &str) -> serde_json::Value {
        bodies
            .iter()
            .find(|(d, _)| d == device)
            .map(|(_, p)| p.body.clone())
            .unwrap()
    }

    #[test]
    fn the_driver_is_told_it_drives_and_the_others_who_waits() {
        let sessions = vec![(1, "phone-a".to_string()), (2, "phone-b".to_string())];
        let bodies = control_bodies(&sessions, Some(2));
        assert_eq!(
            body(&bodies, "phone-b"),
            serde_json::json!({"driving": true, "other": false})
        );
        assert_eq!(
            body(&bodies, "phone-a"),
            serde_json::json!({"driving": false, "other": true})
        );
    }

    #[test]
    fn with_no_driver_no_one_waits() {
        let sessions = vec![(1, "phone-a".to_string()), (2, "phone-b".to_string())];
        for (_, packet) in control_bodies(&sessions, None) {
            assert_eq!(
                packet.body,
                serde_json::json!({"driving": false, "other": false})
            );
        }
    }

    #[test]
    fn a_phone_in_two_sessions_is_told_once() {
        let sessions = vec![(1, "phone-a".to_string()), (2, "phone-a".to_string())];
        let bodies = control_bodies(&sessions, Some(2));
        assert_eq!(bodies.len(), 1);
        assert_eq!(body(&bodies, "phone-a")["driving"], true);
    }
}
