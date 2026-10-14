//! Texts (SMS) from a phone, and replies to them from this desktop (roadmap
//! N5b; spec `docs/superpowers/specs/2026-10-14-sms-design.md`).
//!
//! A phone that shares its texts sends `kdeconnect.sms.messages`; each is
//! kept in the conversation (`core::local::sms`) and, with `sms_notify`,
//! shown as a notification. The desktop app's replies wait in the outbox;
//! [`run_watcher`] hands each to its phone (`kdeconnect.sms.request`) once
//! that phone is connected, and the phone answers `sms_sent`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::sms::{self, Direction, Message, Reply, ReplyRequest, ReplyState};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::permissions::Capability;

/// Texts a phone forwards.
pub const MESSAGES: &str = "kdeconnect.sms.messages";
/// A reply for the phone to send: `{id, address, body}`.
pub const REQUEST: &str = "kdeconnect.sms.request";
/// The phone's answer: `{id, ok, error?}`.
pub const SENT: &str = "kdeconnect.pocketdesk.sms_sent";

/// The longest number kept.
const MAX_ADDRESS: usize = 64;
/// The most texts taken from one packet.
const MAX_PER_PACKET: usize = 50;

static NOTIFY: AtomicBool = AtomicBool::new(true);
static KEEP: AtomicUsize = AtomicUsize::new(sms::KEEP);

/// Apply config.toml's `sms_notify` and `sms_keep` (read once, at start).
pub fn configure(notify: bool, keep: u32) {
    NOTIFY.store(notify, Ordering::Relaxed);
    KEEP.store((keep as usize).max(1), Ordering::Relaxed);
}

fn keep() -> usize {
    KEEP.load(Ordering::Relaxed)
}

/// One writer of the conversation file at a time: the plugin and the
/// watcher both write it.
fn store_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The texts in a `kdeconnect.sms.messages` body: `(address, body, unix
/// seconds)`, cleaned and bounded. `date` is milliseconds, as the phone sends
/// it; a missing or absurd date is now.
fn received_texts(body: &serde_json::Value, now: u64) -> Vec<(String, String, u64)> {
    let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    messages
        .iter()
        .take(MAX_PER_PACKET)
        .filter_map(|m| {
            let clean = |s: &str, max: usize| -> String {
                s.chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .take(max)
                    .collect::<String>()
                    .trim()
                    .to_string()
            };
            let address = clean(m.get("address")?.as_str()?, MAX_ADDRESS);
            let text = clean(m.get("body")?.as_str()?, sms::MAX_BODY);
            if address.is_empty() || text.is_empty() {
                return None;
            }
            let at = m
                .get("date")
                .and_then(|v| v.as_u64())
                .map(|ms| ms / 1000)
                .filter(|s| *s > 0 && *s <= now + 86_400)
                .unwrap_or(now);
            Some((address, text, at))
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct SmsPlugin;

#[async_trait::async_trait]
impl Plugin for SmsPlugin {
    fn name(&self) -> &'static str {
        "sms"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[MESSAGES, SENT]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[REQUEST]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        match packet.packet_type.as_str() {
            MESSAGES => {
                let texts = received_texts(&packet.body, now_unix());
                let phone = sender.device_id().to_string();
                {
                    let _held = store_lock().lock().await;
                    for (address, body, at) in &texts {
                        let message = Message {
                            device_id: phone.clone(),
                            address: address.clone(),
                            body: body.clone(),
                            at_unix: *at,
                            direction: Direction::Received,
                            reply: None,
                        };
                        if let Err(e) = sms::record(&dir, message, keep()) {
                            tracing::warn!("Could not keep a text: {e}");
                        }
                    }
                }
                if NOTIFY.load(Ordering::Relaxed) {
                    for (address, body, _) in texts {
                        let summary = format!("Text from {address}");
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::platform::notify(&summary, &body, Duration::from_secs(8))
                        })
                        .await;
                    }
                }
            }
            SENT => {
                let Some(id) = packet.body.get("id").and_then(|v| v.as_str()) else {
                    return Ok(());
                };
                let state = if packet.body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    ReplyState::Sent
                } else {
                    let reason = packet
                        .body
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("the phone could not send it");
                    ReplyState::Failed {
                        reason: reason.chars().take(200).collect(),
                    }
                };
                let _held = store_lock().lock().await;
                // Only a reply this phone was handed: another phone cannot
                // mark it.
                let ours = sms::read(&dir).iter().any(|m| {
                    m.device_id == sender.device_id()
                        && m.reply.as_ref().is_some_and(|r| r.id == id)
                });
                if ours && let Err(e) = sms::set_reply_state(&dir, id, state, keep()) {
                    tracing::warn!("Could not record a reply's result: {e}");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// What to do with a waiting reply.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Its phone is connected and may send texts: hand it over.
    Deliver,
    /// It waited too long, or its phone may not send texts.
    Fail(&'static str),
    /// Its phone is not connected yet.
    Wait,
}

fn plan(request: &ReplyRequest, connected: bool, permitted: bool, now: u64) -> Plan {
    if !permitted {
        Plan::Fail("this phone may not send texts from here (Devices)")
    } else if now.saturating_sub(request.created_unix) >= sms::OUTBOX_TTL_SECS {
        Plan::Fail("the phone did not connect within a day")
    } else if connected {
        Plan::Deliver
    } else {
        Plan::Wait
    }
}

fn reply_message(request: &ReplyRequest, state: ReplyState, now: u64) -> Message {
    Message {
        device_id: request.device_id.clone(),
        address: request.address.clone(),
        body: request.body.clone(),
        at_unix: now,
        direction: Direction::Reply,
        reply: Some(Reply {
            id: request.id.clone(),
            state,
        }),
    }
}

async fn handle_request(
    dir: &Path,
    request: ReplyRequest,
    clients: &[Arc<dyn DeviceSender>],
    store: Option<&TrustStore>,
) {
    let target = clients
        .iter()
        .find(|c| c.device_id() == request.device_id)
        .cloned();
    let permitted = store.is_some_and(|s| {
        s.is_trusted(&request.device_id)
            && s.permissions(&request.device_id).allows(Capability::Texts)
    });
    let now = now_unix();
    let _held = store_lock().lock().await;
    match plan(&request, target.is_some(), permitted, now) {
        Plan::Wait => {}
        Plan::Fail(reason) => {
            let failed = ReplyState::Failed {
                reason: reason.to_string(),
            };
            let _ = sms::record(dir, reply_message(&request, failed, now), keep());
            let _ = sms::remove_pending(dir, &request.id);
            tracing::info!("Reply {} not sent: {reason}", request.id);
        }
        Plan::Deliver => {
            let Some(target) = target else { return };
            let packet = NetworkPacket::new(REQUEST).with_body(serde_json::json!({
                "id": request.id,
                "address": request.address,
                "body": request.body,
            }));
            match target.send_packet(&packet).await {
                Ok(()) => {
                    let sending = ReplyState::Sending { since_unix: now };
                    let _ = sms::record(dir, reply_message(&request, sending, now), keep());
                    let _ = sms::remove_pending(dir, &request.id);
                    tracing::info!("Reply {} handed to the phone", request.id);
                }
                Err(e) => tracing::debug!("Reply {} not handed over yet: {e}", request.id),
            }
        }
    }
}

/// Daemon task: hand the desktop app's replies to their phones, and give up
/// on those never confirmed.
pub async fn run_watcher() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let requests = sms::pending(&dir);
        if !requests.is_empty() {
            let clients = crate::state::clone_clients().await;
            let store = crate::state::trust_store_path()
                .ok()
                .and_then(|path| TrustStore::load_or_create(path).ok());
            for request in requests {
                handle_request(&dir, request, &clients, store.as_ref()).await;
            }
        }
        let _held = store_lock().lock().await;
        for id in sms::unconfirmed(&sms::read(&dir), now_unix()) {
            let failed = ReplyState::Failed {
                reason: "the phone did not confirm it".to_string(),
            };
            let _ = sms::set_reply_state(&dir, &id, failed, keep());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn texts_are_read_cleaned_and_bounded() {
        let now = 1_800_000_000;
        let body = json!({"messages": [
            {"address": " +1 555 0100 ", "body": "on my way\u{0007}", "date": 1_799_999_000_000u64},
            {"address": "+1 555", "body": "  "},
            {"body": "no number"},
            {"address": "+44", "body": "later", "date": 99_999_999_999_999u64},
        ]});
        let texts = received_texts(&body, now);
        assert_eq!(
            texts,
            vec![
                ("+1 555 0100".into(), "on my way".into(), 1_799_999_000),
                ("+44".into(), "later".into(), now),
            ]
        );
        let many = json!({"messages": (0..80).map(|i| json!({"address": "+1", "body": format!("m{i}")})).collect::<Vec<_>>()});
        assert_eq!(received_texts(&many, now).len(), MAX_PER_PACKET);
        assert!(received_texts(&json!({}), now).is_empty());
    }

    #[test]
    fn a_reply_goes_once_its_phone_is_connected_and_permitted() {
        let request = ReplyRequest {
            id: "r".into(),
            device_id: "p".into(),
            address: "+1".into(),
            body: "hi".into(),
            created_unix: 100,
        };
        assert_eq!(plan(&request, false, true, 200), Plan::Wait);
        assert_eq!(plan(&request, true, true, 200), Plan::Deliver);
        assert!(matches!(plan(&request, true, false, 200), Plan::Fail(_)));
        assert!(matches!(
            plan(&request, false, true, 100 + sms::OUTBOX_TTL_SECS),
            Plan::Fail(_)
        ));
    }
}
