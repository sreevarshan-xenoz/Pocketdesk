//! Who drives when several phones are connected (roadmap N7). The input
//! task asks [`admit`] before injecting a packet; the live-sessions watcher
//! applies the desktop's requests ([`give`]), mirrors the driver, and tells
//! the phones in a session when it changes ([`control_bodies`]).

use pocketdesk_core::input::control::{Admit, InputControl, Policy};
use pocketdesk_core::local::config::InputControlMode;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
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

/// Whether input from `session` is injected. A driver whose session has
/// ended (`live` no longer holds it) lets go first.
pub fn admit(session: u64, live: &[u64]) -> Admit {
    let now = Instant::now();
    let Ok(mut control) = control().lock() else {
        return Admit::Yes { took: false };
    };
    if let Some(driver) = control.driver(now)
        && !live.contains(&driver)
    {
        control.release(driver);
    }
    control.admit(session, now)
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
