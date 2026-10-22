//! Texts (SMS) from a phone, and replies to them from this desktop (roadmap
//! N5b in `docs/status-history.md`).
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
/// `sms_names` from the config: keep the contact names phones share (T8e).
static NAMES: AtomicBool = AtomicBool::new(true);
static KEEP: AtomicUsize = AtomicUsize::new(sms::KEEP);

/// Apply config.toml's `sms_notify` and `sms_keep` (read once, at start).
pub fn configure(notify: bool, keep: u32, names: bool) {
    NOTIFY.store(notify, Ordering::Relaxed);
    NAMES.store(names, Ordering::Relaxed);
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

/// One received text: address, body, unix seconds, and the sender's
/// contact name when the phone shares one (T8e).
type Received = (String, String, u64, Option<String>);

/// The longest contact name kept, in characters.
const MAX_NAME: usize = 64;

/// The texts in a `kdeconnect.sms.messages` body, cleaned and bounded.
/// `date` is milliseconds, as the phone sends it; a missing or absurd date
/// is now. A contact name is kept on one line, and only with [`names`] on.
fn received_texts(body: &serde_json::Value, now: u64, names: bool) -> Vec<Received> {
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
            let name = m
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|_| names)
                .map(|n| {
                    pocketdesk_core::protocol::phone_media::clean_line(n)
                        .chars()
                        .take(MAX_NAME)
                        .collect::<String>()
                })
                .filter(|n| !n.is_empty());
            Some((address, text, at, name))
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
                let texts = received_texts(&packet.body, now_unix(), NAMES.load(Ordering::Relaxed));
                let phone = sender.device_id().to_string();
                {
                    let _held = store_lock().lock().await;
                    let messages = texts
                        .iter()
                        .map(|(address, body, at, name)| Message {
                            device_id: phone.clone(),
                            address: address.clone(),
                            body: body.clone(),
                            at_unix: *at,
                            direction: Direction::Received,
                            reply: None,
                            name: name.clone(),
                        })
                        .collect();
                    if let Err(e) = sms::record_all(&dir, messages, keep()) {
                        tracing::warn!("Could not keep texts: {e}");
                    }
                }
                for (address, body, _, name) in &texts {
                    let from = match name {
                        Some(name) => format!("{name} ({address})"),
                        None => address.clone(),
                    };
                    // P3: why it was not shown, as a notification's says.
                    let mut meta = serde_json::json!({});
                    if let Some(quiet) = crate::quiet_hours::now() {
                        meta[quiet.field()] = serde_json::json!(true);
                    }
                    crate::events::emit(
                        "text_received",
                        Some(&phone),
                        meta,
                        serde_json::json!({ "from": from, "body": body }),
                    );
                }
                // Paused (K2a): kept on the Messages page, not shown.
                if shows_texts() {
                    // The phone's notification limit holds for texts too.
                    for (address, body, _, name) in texts {
                        if !crate::plugins::notification::admit(&phone) {
                            continue;
                        }
                        let summary = match name {
                            Some(name) => format!("Text from {name} ({address})"),
                            None => format!("Text from {address}"),
                        };
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
    /// It was handed over already (its outbox file could not be removed
    /// then): only remove the file, never send the text twice.
    Forget,
    /// Its phone is connected and may send texts: hand it over.
    Deliver,
    /// It waited too long, or its phone may not send texts.
    Fail(&'static str),
    /// Its phone is not connected yet.
    Wait,
}

fn plan(request: &ReplyRequest, handed: bool, connected: bool, permitted: bool, now: u64) -> Plan {
    if handed {
        Plan::Forget
    } else if !permitted {
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
        name: None,
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
    // Read under the lock, but never hold it while a phone's socket is
    // written: a stalled phone must not stop texts being kept.
    let handed = {
        let _held = store_lock().lock().await;
        match sms::read_checked(dir) {
            Ok(messages) => messages
                .iter()
                .any(|m| m.reply.as_ref().is_some_and(|r| r.id == request.id)),
            Err(e) => {
                tracing::warn!("The texts history cannot be read, replies wait: {e}");
                return;
            }
        }
    };
    match plan(&request, handed, target.is_some(), permitted, now) {
        Plan::Wait => {}
        Plan::Forget => forget(dir, &request.id),
        Plan::Fail(reason) => {
            let failed = ReplyState::Failed {
                reason: reason.to_string(),
            };
            let _held = store_lock().lock().await;
            if sms::record(dir, reply_message(&request, failed, now), keep()).is_ok() {
                forget(dir, &request.id);
            }
            tracing::info!("Reply {} not sent: {reason}", request.id);
        }
        Plan::Deliver => {
            let Some(target) = target else { return };
            match target
                .send_packet(&request_packet(
                    &request.id,
                    &request.address,
                    &request.body,
                ))
                .await
            {
                Ok(()) => {
                    let sending = ReplyState::Sending {
                        since_unix: now,
                        attempts: 1,
                    };
                    let _held = store_lock().lock().await;
                    if sms::record(dir, reply_message(&request, sending, now), keep()).is_ok() {
                        forget(dir, &request.id);
                    }
                    tracing::info!("Reply {} handed to the phone", request.id);
                }
                Err(e) => tracing::debug!("Reply {} not handed over yet: {e}", request.id),
            }
        }
    }
}

fn request_packet(id: &str, address: &str, body: &str) -> NetworkPacket {
    NetworkPacket::new(REQUEST).with_body(serde_json::json!({
        "id": id,
        "address": address,
        "body": body,
    }))
}

/// A reply handed over and not confirmed is handed over again after this
/// long, while its phone is connected: the phone may have taken it on a
/// connection it was closing, or on one that had silently died.
const RETRY_SECS: u64 = 120;
/// The most hand-overs of one reply.
const MAX_ATTEMPTS: u32 = 3;

/// What becomes of a reply handed over and still unconfirmed.
#[derive(Debug, PartialEq, Eq)]
enum Retry {
    Keep,
    Rehand,
    Fail,
}

fn retry(since_unix: u64, attempts: u32, connected: bool, now: u64) -> Retry {
    let waited = now.saturating_sub(since_unix);
    if waited >= sms::CONFIRM_TTL_SECS {
        Retry::Fail
    } else if waited < RETRY_SECS {
        Retry::Keep
    } else if attempts >= MAX_ATTEMPTS {
        Retry::Fail
    } else if connected {
        Retry::Rehand
    } else {
        Retry::Keep
    }
}

/// Hand unconfirmed replies over again, or give up on them.
async fn retry_unconfirmed(dir: &Path, clients: &[Arc<dyn DeviceSender>]) {
    let now = now_unix();
    let waiting: Vec<Message> = {
        let _held = store_lock().lock().await;
        sms::read(dir)
            .into_iter()
            .filter(|m| {
                matches!(
                    m.reply,
                    Some(Reply {
                        state: ReplyState::Sending { .. },
                        ..
                    })
                )
            })
            .collect()
    };
    for message in waiting {
        let Some(Reply {
            id,
            state:
                ReplyState::Sending {
                    since_unix,
                    attempts,
                },
        }) = &message.reply
        else {
            continue;
        };
        let target = clients.iter().find(|c| c.device_id() == message.device_id);
        let next = match retry(*since_unix, *attempts, target.is_some(), now) {
            Retry::Keep => continue,
            Retry::Fail => ReplyState::Failed {
                reason: "the phone did not confirm it".to_string(),
            },
            Retry::Rehand => {
                let Some(target) = target else { continue };
                if target
                    .send_packet(&request_packet(id, &message.address, &message.body))
                    .await
                    .is_err()
                {
                    continue;
                }
                tracing::info!("Reply {id} handed to the phone again");
                ReplyState::Sending {
                    since_unix: now,
                    attempts: attempts + 1,
                }
            }
        };
        let _held = store_lock().lock().await;
        let _ = sms::set_reply_state(dir, id, next, keep());
    }
}

/// Take reply `id` out of the outbox; one that stays is never sent twice
/// (the watcher sees it in the conversation), but says so.
fn forget(dir: &Path, id: &str) {
    if let Err(e) = sms::remove_pending(dir, id) {
        tracing::warn!("Reply {id} stays in the outbox: {e}");
    }
}

/// Daemon task: hand the desktop app's replies to their phones, and give up
/// on those never confirmed.
/// Whether a text that came is shown here: text notifications on, and this
/// computer not paused (K2a) nor quiet (P3).
fn shows_texts() -> bool {
    NOTIFY.load(Ordering::Relaxed) && !crate::paused::now() && crate::quiet_hours::now().is_none()
}

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
        let clients = crate::state::clone_clients().await;
        retry_unconfirmed(&dir, &clients).await;
    }
}

#[cfg(test)]
mod tests {
    /// K2a: a pause keeps texts off the screen; they stay on Messages.
    #[test]
    fn a_pause_holds_texts() {
        let shown = super::NOTIFY.load(Ordering::Relaxed);
        assert_eq!(super::shows_texts(), shown);
        let _paused = crate::paused::paused_for_test();
        assert!(!super::shows_texts());
    }

    /// P3: quiet hours hold texts as a pause does.
    #[test]
    fn quiet_hours_hold_texts() {
        let _quiet = crate::quiet_hours::quiet_for_test(crate::quiet_hours::Quiet::Hours);
        assert!(!super::shows_texts());
    }

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
        let texts = received_texts(&body, now, true);
        assert_eq!(
            texts,
            vec![
                (
                    "+1 555 0100".into(),
                    "on my way".into(),
                    1_799_999_000,
                    None
                ),
                ("+44".into(), "later".into(), now, None),
            ]
        );
        let many = json!({"messages": (0..80).map(|i| json!({"address": "+1", "body": format!("m{i}")})).collect::<Vec<_>>()});
        assert_eq!(received_texts(&many, now, true).len(), MAX_PER_PACKET);
        assert!(received_texts(&json!({}), now, true).is_empty());
    }

    /// A text may carry the sender's contact name (T8e): kept on one line,
    /// at most 64 characters, and dropped when names are turned off.
    #[test]
    fn a_contact_name_is_kept_clean_or_not_at_all() {
        let now = 1_800_000_000;
        let body = json!({"messages": [
            {"address": "+1", "body": "hi", "name": " Ann\nLee\u{202e} "},
            {"address": "+2", "body": "yo", "name": "B".repeat(100)},
            {"address": "+3", "body": "hey", "name": "   "},
        ]});
        let names: Vec<Option<String>> = received_texts(&body, now, true)
            .into_iter()
            .map(|t| t.3)
            .collect();
        assert_eq!(
            names,
            vec![Some("Ann Lee".to_string()), Some("B".repeat(64)), None]
        );
        assert!(
            received_texts(&body, now, false)
                .iter()
                .all(|t| t.3.is_none())
        );
    }

    #[test]
    fn an_unconfirmed_reply_is_handed_over_again_then_given_up() {
        assert_eq!(retry(100, 1, true, 100 + RETRY_SECS - 1), Retry::Keep);
        assert_eq!(retry(100, 1, true, 100 + RETRY_SECS), Retry::Rehand);
        assert_eq!(
            retry(100, 1, false, 100 + RETRY_SECS),
            Retry::Keep,
            "phone away"
        );
        assert_eq!(
            retry(100, MAX_ATTEMPTS, true, 100 + RETRY_SECS),
            Retry::Fail
        );
        assert_eq!(
            retry(100, 1, false, 100 + sms::CONFIRM_TTL_SECS),
            Retry::Fail
        );
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
        assert_eq!(plan(&request, false, false, true, 200), Plan::Wait);
        assert_eq!(plan(&request, false, true, true, 200), Plan::Deliver);
        assert!(matches!(
            plan(&request, false, true, false, 200),
            Plan::Fail(_)
        ));
        assert!(matches!(
            plan(&request, false, false, true, 100 + sms::OUTBOX_TTL_SECS),
            Plan::Fail(_)
        ));
        assert_eq!(
            plan(&request, true, true, true, 200),
            Plan::Forget,
            "a reply handed over before is never sent twice"
        );
    }
}
