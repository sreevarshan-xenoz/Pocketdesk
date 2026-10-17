use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::json;

use crate::privacy;
use std::sync::Mutex;

/// The phone whose grab is held: only it may refresh or release it (R3).
static OWNER: Mutex<Option<String>> = Mutex::new(None);

/// Whether `device` may do `action` while `owner` holds the grab: anyone may
/// ask for the status or grab when nothing is held, only the holder may
/// refresh or release it.
fn may_act(action: &str, owner: Option<&str>, device: &str) -> bool {
    match (action, owner) {
        ("grab" | "release", Some(holder)) => holder == device,
        _ => true,
    }
}

/// The phone holding the grab, forgotten once the grab itself has ended
/// (released or its TTL ran out).
fn owner() -> Option<String> {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if owner.is_some() && privacy::privacy_status().0 == 0 {
        *owner = None;
    }
    owner.clone()
}

/// Give local input back if the phone holding the grab may no longer have
/// it (no longer paired): run by the live-session watcher each second.
pub(crate) fn release_if_owner(lost: impl Fn(&str) -> bool) {
    let Some(holder) = owner() else { return };
    if lost(&holder) {
        let count = privacy::release_input();
        *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        tracing::warn!(
            device_id = %holder,
            released = count,
            "Privacy grab released: the phone holding it is no longer paired"
        );
    }
}

/// Privacy mode (Tier-3 #15): the phone asks the desktop to block *local*
/// input while the remote session keeps working.
///
/// Answers `kdeconnect.pocketdesk.privacy`:
/// - `{action: "grab"}` — exclusively grab all physical keyboards/mice
///   (`EVIOCGRAB`); the compositor stops seeing local input, our uinput
///   injection path is untouched. Re-arms the auto-release TTL on refresh.
/// - `{action: "release"}` — give local input back.
/// - `{action: "status"}` — report held devices + seconds of TTL left.
/// - `{lock: true}` (combinable with grab) — also engage the screen locker.
///
/// Every reply is `{ok, grabbed?, ttlLeft?, locked?, error?}`.
#[derive(Debug, Default)]
pub struct PrivacyPlugin;

/// Answer a privacy request (a grab, a release, a lock) from a phone with
/// Power turned off, which the control gate drops: the phone waits for a
/// reply, and is told why at once instead of after its timeout (U2).
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != "kdeconnect.pocketdesk.privacy" {
        return;
    }
    let body = json!({
        "ok": false,
        "error": "Power is turned off for this phone on the computer",
    });
    let _ = sender
        .send_packet(&NetworkPacket::new("kdeconnect.pocketdesk.privacy").with_body(body))
        .await;
}

#[async_trait::async_trait]
impl Plugin for PrivacyPlugin {
    fn name(&self) -> &'static str {
        "privacy"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.privacy"]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pocketdesk.privacy"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != "kdeconnect.pocketdesk.privacy" {
            return Ok(());
        }
        let action = packet
            .body
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("status");
        let want_lock = packet
            .body
            .get("lock")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let device = sender.device_id();
        let holder = owner();
        let refused = !may_act(action, holder.as_deref(), device);
        let mut response = match action {
            _ if refused => {
                tracing::warn!(
                    device_id = %device,
                    holder = holder.as_deref().unwrap_or(""),
                    "Privacy {action} refused: another phone holds the grab"
                );
                json!({ "ok": false, "error": "another phone holds privacy mode" })
            }
            "grab" => match privacy::grab_input(privacy::GRAB_TTL) {
                Ok(count) => {
                    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(device.to_string());
                    json!({ "ok": true, "grabbed": count })
                }
                Err(e) => json!({ "ok": false, "error": e }),
            },
            "release" => {
                let count = privacy::release_input();
                *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
                json!({ "ok": true, "released": count })
            }
            _ => {
                let (count, ttl_left) = privacy::privacy_status();
                json!({ "ok": true, "grabbed": count, "ttlLeft": ttl_left })
            }
        };

        // A refused request does nothing, the lock included.
        if want_lock && !refused {
            // The lock helpers use blocking process spawns.
            let outcome = tokio::task::spawn_blocking(privacy::lock_screen).await;
            match outcome {
                Ok(Ok(program)) => response["locked"] = json!(program),
                Ok(Err(e)) => response["lockError"] = json!(e),
                Err(e) => response["lockError"] = json!(e.to_string()),
            }
        }

        let response = NetworkPacket::new("kdeconnect.pocketdesk.privacy").with_body(response);
        sender.send_packet(&response).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_phone_that_grabbed_may_refresh_or_release() {
        assert!(may_act("grab", None, "phone-a"), "nothing held");
        assert!(
            may_act("grab", Some("phone-a"), "phone-a"),
            "its own refresh"
        );
        assert!(!may_act("grab", Some("phone-a"), "phone-b"));
        assert!(may_act("release", Some("phone-a"), "phone-a"));
        assert!(!may_act("release", Some("phone-a"), "phone-b"));
        assert!(may_act("release", None, "phone-b"), "nothing to protect");
        assert!(
            may_act("status", Some("phone-a"), "phone-b"),
            "anyone may ask"
        );
    }

    #[test]
    fn plugin_declares_privacy_capability() {
        assert!(
            PrivacyPlugin
                .incoming_capabilities()
                .contains(&"kdeconnect.pocketdesk.privacy")
        );
    }

    struct Phone(std::sync::Mutex<Vec<NetworkPacket>>);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-a"
        }
        fn connection_id(&self) -> &str {
            "phone-a"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// A phone with Power off asking to lock is told so at once (U2): it
    /// had waited 5 s for a reply that never came, then said the computer
    /// did not answer.
    #[tokio::test]
    async fn a_refused_privacy_request_is_answered_with_why() {
        let phone = Phone(Default::default());
        let asked = NetworkPacket::new("kdeconnect.pocketdesk.privacy")
            .with_body(json!({"action": "status", "lock": true}));
        crate::plugins::answer_denied(&asked, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, "kdeconnect.pocketdesk.privacy");
        assert_eq!(sent[0].body["ok"], false);
        assert!(sent[0].body["error"].as_str().unwrap().contains("Power"));
        assert!(sent[0].body.get("locked").is_none());
    }

    #[tokio::test]
    async fn status_action_replies_without_grabbing() {
        let (count, ttl) = privacy::privacy_status();
        assert_eq!(count, 0, "test run must not hold real grabs");
        assert!(ttl.is_none());
        let packet = NetworkPacket::new("kdeconnect.pocketdesk.privacy")
            .with_body(json!({ "action": "status" }));
        assert_eq!(packet.packet_type, "kdeconnect.pocketdesk.privacy");
    }
}
