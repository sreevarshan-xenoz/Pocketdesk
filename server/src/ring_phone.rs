//! Ring a paired phone from this computer (N1): each ring the desktop app or
//! `pocketdesk ring` queued (`core::local::rings`) goes to its phone as
//! `kdeconnect.findmydevice {ring: true}` over the phone's live connection
//! (a session's or the background link), under the phone's Ring permission,
//! at most once a minute per phone.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::local::rings::RingRequest;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};

/// The packet that rings a phone: the one a phone sends to ring a computer.
pub const FIND: &str = "kdeconnect.findmydevice";

/// The least time between two rings of one phone.
pub const EVERY: Duration = Duration::from_secs(60);

/// When each phone was last rung.
#[derive(Debug, Default)]
pub struct Pace {
    last: HashMap<String, Instant>,
}

impl Pace {
    /// Whether `device` may be rung at `now`; if so, it counts as rung.
    pub fn due(&mut self, device: &str, now: Instant) -> bool {
        if self
            .last
            .get(device)
            .is_some_and(|at| now.saturating_duration_since(*at) < EVERY)
        {
            return false;
        }
        self.last.insert(device.to_string(), now);
        true
    }

    /// A ring counted by [`Pace::due`] that did not go: it uses up no minute.
    pub fn forget(&mut self, device: &str) {
        self.last.remove(device);
    }
}

fn pace() -> &'static Mutex<Pace> {
    static PACE: OnceLock<Mutex<Pace>> = OnceLock::new();
    PACE.get_or_init(Default::default)
}

/// Daemon task: send each queued ring to its phone.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: ringing phones is off");
        return;
    };
    loop {
        crate::inbox::wait(
            pocketdesk_core::local::paths::RINGS_DIR,
            Duration::from_millis(250),
        )
        .await;
        let now = pocketdesk_core::local::pin::now_unix();
        for request in pocketdesk_core::local::rings::take_all(&dir, now) {
            let clients = crate::state::clone_clients().await;
            // A computer is never rung, whatever its switches say (N1 review).
            let allowed = crate::plugins::pair::allows(
                &request.device_id,
                pocketdesk_core::protocol::permissions::Capability::Ring,
            ) && !crate::plugins::pair::is_computer_device(&request.device_id);
            match ring(&request, &clients, allowed, pace(), Instant::now()).await {
                Ok(()) => tracing::info!("Rang {}", request.device_id),
                Err(reason) => tracing::info!("{} not rung: {reason}", request.device_id),
            }
        }
    }
}

/// Ring one phone: allowed, connected, and not rung in the last minute.
async fn ring(
    request: &RingRequest,
    clients: &[Arc<dyn DeviceSender>],
    allowed: bool,
    pace: &Mutex<Pace>,
    now: Instant,
) -> Result<(), String> {
    if !allowed {
        return Err("Ring is off for this phone".to_string());
    }
    let phone = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .ok_or("the phone is not connected")?;
    if !pace
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .due(&request.device_id, now)
    {
        return Err("it was rung less than a minute ago".to_string());
    }
    let packet = NetworkPacket::new(FIND).with_body(serde_json::json!({ "ring": true }));
    let sent = phone.send_packet(&packet).await;
    if sent.is_err() {
        // Counted only once it went (N1 review): a failed send is tried again.
        pace.lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget(&request.device_id);
    }
    sent.map_err(|e| format!("could not send: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::error::Result;

    struct Phone {
        id: &'static str,
        sent: Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.id
        }
        fn connection_id(&self) -> &str {
            self.id
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    fn request(device: &str) -> RingRequest {
        RingRequest {
            id: "x".to_string(),
            device_id: device.to_string(),
            created_unix: 1,
        }
    }

    #[tokio::test]
    async fn a_ring_goes_to_its_phone_once_a_minute_and_only_with_ring_on() {
        let a = Arc::new(Phone {
            id: "phone-a",
            sent: Mutex::default(),
        });
        let b = Arc::new(Phone {
            id: "phone-b",
            sent: Mutex::default(),
        });
        let clients: Vec<Arc<dyn DeviceSender>> = vec![a.clone(), b.clone()];
        let pace = Mutex::new(Pace::default());
        let start = Instant::now();

        let off = ring(&request("phone-b"), &clients, false, &pace, start).await;
        assert!(off.unwrap_err().contains("off"));
        let away = ring(&request("phone-c"), &clients, true, &pace, start).await;
        assert!(away.unwrap_err().contains("not connected"));

        ring(&request("phone-b"), &clients, true, &pace, start)
            .await
            .unwrap();
        assert!(a.sent.lock().unwrap().is_empty(), "not another phone");
        let sent = b.sent.lock().unwrap().clone();
        assert_eq!(sent[0].packet_type, FIND);
        assert_eq!(sent[0].body["ring"], true);

        let again = ring(
            &request("phone-b"),
            &clients,
            true,
            &pace,
            start + Duration::from_secs(30),
        )
        .await;
        assert!(again.unwrap_err().contains("minute"));
        ring(&request("phone-b"), &clients, true, &pace, start + EVERY)
            .await
            .expect("a minute on");
        assert_eq!(b.sent.lock().unwrap().len(), 2);
    }

    /// N1 review: a send that fails uses up no minute.
    #[tokio::test]
    async fn a_ring_that_could_not_be_sent_is_tried_again() {
        struct Broken;
        #[async_trait::async_trait]
        impl DeviceSender for Broken {
            fn device_id(&self) -> &str {
                "phone-x"
            }
            fn connection_id(&self) -> &str {
                "phone-x"
            }
            async fn send_packet(&self, _packet: &NetworkPacket) -> Result<()> {
                Err(pocketdesk_core::error::PocketdeskError::Io {
                    operation: "send",
                    detail: "gone".to_string(),
                })
            }
        }
        let clients: Vec<Arc<dyn DeviceSender>> = vec![Arc::new(Broken)];
        let pace = Mutex::new(Pace::default());
        let now = Instant::now();
        let failed = ring(&request("phone-x"), &clients, true, &pace, now).await;
        assert!(failed.unwrap_err().contains("could not send"));
        assert!(
            pace.lock()
                .unwrap()
                .due("phone-x", now + Duration::from_secs(1)),
            "the failed ring used up no minute"
        );
    }

    #[test]
    fn each_phone_has_its_own_minute() {
        let mut pace = Pace::default();
        let now = Instant::now();
        assert!(pace.due("phone-a", now));
        assert!(!pace.due("phone-a", now + Duration::from_secs(59)));
        assert!(pace.due("phone-b", now), "each phone its own minute");
    }
}
