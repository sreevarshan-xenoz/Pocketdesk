use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::{Value, json};

use crate::privacy;
use std::sync::Mutex;

/// The privacy packet: a request, or its answer (which says `ok`).
pub const PRIVACY: &str = "kdeconnect.pocketdesk.privacy";

/// Who holds the grab: the device, and for another of the owner's
/// computers (PC8, the window that shows this screen there) the connection
/// it asked on, so the grab ends as that connection does: never left on
/// with nobody there.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holder {
    device: String,
    connection: Option<String>,
}

/// The device whose grab is held: only it may refresh or release it (R3).
static OWNER: Mutex<Option<Holder>> = Mutex::new(None);

/// Whether `device` may do `action` while `owner` holds the grab: anyone may
/// ask for the status or grab when nothing is held, only the holder may
/// refresh or release it.
fn may_act(action: &str, owner: Option<&str>, device: &str) -> bool {
    match (action, owner) {
        ("grab" | "release", Some(holder)) => holder == device,
        _ => true,
    }
}

/// Whether a renewal (PC8: `{action: "grab", renew: true}`, a computer's
/// window keeping its grab alive) is refused: only a grab still held by
/// `device` is renewed, so a grab that ended here (its connection closed,
/// its TTL ran out, released here) is not started again by a late renewal.
fn renewal_refused(renew: bool, owner: Option<&str>, device: &str) -> bool {
    renew && owner != Some(device)
}

/// Whether a packet's body is an answer (it says `ok`), never a request:
/// one computer's answer to another's window is never answered again.
pub(crate) fn is_answer(body: &Value) -> bool {
    body.get("ok").is_some()
}

/// The holder, forgotten once the grab itself has ended (released or its
/// TTL ran out).
fn holder() -> Option<Holder> {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if owner.is_some() && privacy::privacy_status().0 == 0 {
        *owner = None;
    }
    owner.clone()
}

/// The device holding the grab.
fn owner() -> Option<String> {
    holder().map(|h| h.device)
}

/// Whether the grab held by `holder` ends as the connection
/// `connection_id` closes: only a computer's, on the connection it asked on.
fn ends_with(holder: &Holder, connection_id: &str) -> bool {
    holder.connection.as_deref() == Some(connection_id)
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

/// The connection `connection_id` closed (PC8): a computer's grab asked on
/// it ends with it. A phone's is kept (it may come back on a new
/// connection), to its TTL as before.
pub(crate) fn connection_closed(connection_id: &str) {
    let Some(holder) = holder() else { return };
    if !ends_with(&holder, connection_id) {
        return;
    }
    let count = privacy::release_input();
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tracing::info!(
        device_id = %holder.device,
        released = count,
        "Privacy grab released: the computer holding it is no longer connected"
    );
}

/// The connection id that the link this computer keeps to `device_id`
/// stands for (PC8), for a grab asked on that link.
pub(crate) fn link_connection(device_id: &str) -> String {
    format!("link:{device_id}")
}

/// Ends a grab asked on the link this computer keeps to a computer as the
/// link ends (PC8).
pub(crate) struct LinkGrab(pub String);

impl Drop for LinkGrab {
    fn drop(&mut self) {
        connection_closed(&link_connection(&self.0));
    }
}

/// Privacy mode (Tier-3 #15): the phone asks the desktop to block *local*
/// input while the remote session keeps working.
///
/// Answers `kdeconnect.pocketdesk.privacy`:
/// - `{action: "grab"}` — exclusively grab all physical keyboards/mice
///   (`EVIOCGRAB`); the compositor stops seeing local input, our uinput
///   injection path is untouched. Re-arms the auto-release TTL on refresh;
///   with `renew: true` (PC8) only a grab the asker still holds.
/// - `{action: "release"}` — give local input back.
/// - `{action: "status"}` — report held devices + seconds of TTL left.
/// - `{lock: true}` (combinable with grab) — also engage the screen locker.
///
/// Every reply is `{ok, grabbed?, ttlLeft?, locked?, error?}`. Another of
/// the owner's computers asks the same (PC8, its window's "Hide what I do
/// there"), held to the same Power switch; its grab also ends as the
/// connection it asked on closes.
#[derive(Debug, Default)]
pub struct PrivacyPlugin;

/// Answer a privacy request (a grab, a release, a lock) from a phone with
/// Power turned off, which the control gate drops: the phone waits for a
/// reply, and is told why at once instead of after its timeout (U2). An
/// answer is never answered (PC8).
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != PRIVACY || is_answer(&packet.body) {
        return;
    }
    let computer = computer(sender.device_id()).await;
    let body = json!({ "ok": false, "error": denied_words(computer) });
    let _ = sender
        .send_packet(&NetworkPacket::new(PRIVACY).with_body(body))
        .await;
}

/// Why a request from a device without Power is refused: a computer is
/// told the switch by its name on this computer's Devices page (PC8).
pub(crate) fn denied_words(computer: bool) -> &'static str {
    if computer {
        "\u{201c}Lock, sleep, privacy and wake others\u{201d} is off for this computer there"
    } else {
        "Power is turned off for this phone on the computer"
    }
}

/// Whether `device_id` is another of the owner's computers (the trust
/// store, read off the runtime).
async fn computer(device_id: &str) -> bool {
    let id = device_id.to_string();
    tokio::task::spawn_blocking(move || crate::plugins::pair::is_computer_device(&id))
        .await
        .unwrap_or(false)
}

/// Do what the privacy request `body` from `device` asks and say how it
/// went (`{ok, grabbed?, ttlLeft?, locked?, error?}`). `connection` is the
/// connection a computer asked on, its grab ending with it (PC8); `None`
/// for a phone. Shared by the plugin and the link this computer keeps to a
/// computer.
pub(crate) async fn respond(body: &Value, device: &str, connection: Option<&str>) -> Value {
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let want_lock = body.get("lock").and_then(|v| v.as_bool()).unwrap_or(false);
    let renew = body.get("renew").and_then(|v| v.as_bool()).unwrap_or(false);

    let holder = owner();
    let refused = !may_act(action, holder.as_deref(), device);
    let mut response = match action {
        _ if refused => {
            tracing::warn!(
                device_id = %device,
                holder = holder.as_deref().unwrap_or(""),
                "Privacy {action} refused: another device holds the grab"
            );
            json!({ "ok": false, "error": "another phone holds privacy mode" })
        }
        "grab" if renewal_refused(renew, holder.as_deref(), device) => {
            json!({ "ok": false, "error": "privacy mode ended there" })
        }
        "grab" => match privacy::grab_input(privacy::GRAB_TTL) {
            Ok(count) => {
                *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Holder {
                    device: device.to_string(),
                    connection: connection.map(str::to_string),
                });
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
    response
}

#[async_trait::async_trait]
impl Plugin for PrivacyPlugin {
    fn name(&self) -> &'static str {
        "privacy"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[PRIVACY]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[PRIVACY]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        // An answer to this computer's window (PC8) is taken before the
        // gate (`take_answer`); one that comes here anyway is never
        // answered.
        if packet.packet_type != PRIVACY || is_answer(&packet.body) {
            return Ok(());
        }
        let device = sender.device_id();
        // A computer's grab ends with the connection it asked on (PC8).
        let connection = computer(device)
            .await
            .then(|| sender.connection_id().to_string());
        let response = respond(&packet.body, device, connection.as_deref()).await;
        sender
            .send_packet(&NetworkPacket::new(PRIVACY).with_body(response))
            .await
    }
}

/// Take the answer to a privacy ask this computer's window made of the
/// computer `device_id` (PC8), before the permission gate: an answer asks
/// nothing of this computer, so this computer's own switches for that one
/// do not decide whether it is heard. Whether it is used is for the view
/// that asked ([`crate::computer_view::privacy_answer`]). Returns whether
/// `packet` was such an answer.
pub(crate) fn take_answer(device_id: &str, packet: &NetworkPacket) -> bool {
    if packet.packet_type != PRIVACY || !is_answer(&packet.body) {
        return false;
    }
    crate::computer_view::privacy_answer(device_id, &packet.body);
    true
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

    /// PC8: a computer's window renews only a grab it still holds here.
    #[test]
    fn a_renewal_does_not_start_a_grab_that_ended() {
        assert!(!renewal_refused(true, Some("desk"), "desk"), "still held");
        assert!(renewal_refused(true, None, "desk"), "ended here");
        assert!(renewal_refused(true, Some("phone-a"), "desk"));
        assert!(!renewal_refused(false, None, "desk"), "a first grab");
    }

    /// PC8: a computer's grab ends with the connection it asked on; a
    /// phone's waits for its TTL as before.
    #[test]
    fn a_computers_grab_ends_with_its_connection() {
        let computer = Holder {
            device: "desk".into(),
            connection: Some("conn-7".into()),
        };
        assert!(ends_with(&computer, "conn-7"));
        assert!(!ends_with(&computer, "conn-8"), "another connection of it");
        let phone = Holder {
            device: "phone-a".into(),
            connection: None,
        };
        assert!(!ends_with(&phone, "conn-7"));
        assert_eq!(link_connection("desk"), "link:desk");
    }

    /// PC8: an answer is never taken for a request, so two computers never
    /// answer each other's answers.
    #[test]
    fn an_answer_is_told_from_a_request() {
        assert!(is_answer(&json!({"ok": false, "error": "no"})));
        assert!(is_answer(&json!({"ok": true, "grabbed": 2})));
        assert!(!is_answer(&json!({"action": "grab"})));
        assert!(!is_answer(&json!({"action": "release", "lock": true})));
        assert!(!is_answer(&json!({})));
        assert!(denied_words(true).contains("this computer"));
        assert!(
            denied_words(false).contains("Power"),
            "a phone's words kept"
        );
    }

    #[test]
    fn plugin_declares_privacy_capability() {
        assert!(PrivacyPlugin.incoming_capabilities().contains(&PRIVACY));
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
        // The answer asks the trust store whether it is a computer.
        crate::state::use_test_dirs();
        let phone = Phone(Default::default());
        let asked =
            NetworkPacket::new(PRIVACY).with_body(json!({"action": "status", "lock": true}));
        crate::plugins::answer_denied(&asked, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, PRIVACY);
        assert_eq!(sent[0].body["ok"], false);
        assert!(sent[0].body["error"].as_str().unwrap().contains("Power"));
        assert!(sent[0].body.get("locked").is_none());
    }

    /// An answer that reaches the gate is never answered (PC8).
    #[tokio::test]
    async fn an_answer_refused_by_the_gate_is_not_answered() {
        let phone = Phone(Default::default());
        let answer = NetworkPacket::new(PRIVACY).with_body(json!({"ok": true, "grabbed": 2}));
        crate::plugins::answer_denied(&answer, &phone).await;
        assert!(phone.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn status_action_replies_without_grabbing() {
        let (count, ttl) = privacy::privacy_status();
        assert_eq!(count, 0, "test run must not hold real grabs");
        assert!(ttl.is_none());
        let packet = NetworkPacket::new(PRIVACY).with_body(json!({ "action": "status" }));
        assert_eq!(packet.packet_type, PRIVACY);
    }
}
