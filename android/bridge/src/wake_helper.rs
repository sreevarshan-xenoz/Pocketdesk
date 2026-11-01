//! A spare phone as the wake helper, this phone's side (`core::protocol::
//! wake_helper` has the wire and the rules).
//!
//! **As the helper** (its switch on, on a charger, on Wi-Fi: the app decides
//! and calls [`start`] and [`stop`]): an iroh endpoint on its own key
//! listens for the one request, and serves it from the duties the app set
//! ([`set_duties`], what the computer told the phone, kept by the app) and
//! the broadcast addresses of the networks the phone is on now
//! ([`set_networks`]). The listener, the rate limit and the check that only
//! a key the computer introduced is served are core's.
//!
//! **As an owner phone** ([`ask`]): dial the helper the computer named, as
//! this phone's own device key, and ask it to wake that computer.
//!
//! What the computer says (`wake_helper_duty`, `wake_helper_info`) is kept
//! here for the app to take ([`take_json`]), read again by the same rules
//! the computer's packets are written by, so a body that does not read never
//! reaches the app. Nothing here logs a key, a MAC or an address.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::wake_helper::{DUTY, Duties, DutyMessage, INFO, Info};
use pocketdesk_core::streaming::wake_helper_net::{
    Helper, HelperState, HelperTarget, NetworkSender, ask as ask_helper,
};
use serde_json::Value;

/// The helper endpoint while it runs.
static HELPER: tokio::sync::Mutex<Option<Helper>> = tokio::sync::Mutex::const_new(None);

/// What the helper holds, whether it runs or not: the app sets it before
/// it starts and as the computer's word changes.
static STATE: LazyLock<Arc<HelperState>> =
    LazyLock::new(|| HelperState::new(Box::new(NetworkSender)));

/// What the computer said, waiting for the app: the packet type, where it
/// came from and its body as read.
static WAITING: Mutex<Vec<(String, String, u16, Value)>> = Mutex::new(Vec::new());

/// At most this many are kept: one of each kind from each computer.
const MAX_WAITING: usize = 8;

/// Keep what the computer at `host:port` said, if it is a duty or an info
/// that reads. A newer one from the same computer replaces the older.
pub(crate) fn receive(packet_type: &str, body: &Value, host: &str, port: u16) {
    let body = match packet_type {
        DUTY => DutyMessage::parse(body).map(|m| m.body()),
        INFO => Info::parse(body).map(|i| i.body()),
        _ => return,
    };
    let Some(body) = body else {
        tracing::warn!("Wake helper: a {packet_type} that does not read was dropped");
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.retain(|(t, h, p, _)| !(t == packet_type && h == host && *p == port));
    waiting.push((packet_type.to_string(), host.to_string(), port, body));
    while waiting.len() > MAX_WAITING {
        waiting.remove(0);
    }
}

/// What waits, as a JSON array `[{type, host, port, body}]`; taken.
pub(crate) fn take_json() -> String {
    let taken: Vec<Value> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .map(|(t, host, port, body)| {
            serde_json::json!({ "type": t, "host": host, "port": port, "body": body })
        })
        .collect();
    Value::Array(taken).to_string()
}

/// Set the duties the helper serves: the computers' bodies as the app kept
/// them, replacing all it held. A body that does not read, or is a refusal,
/// is not a duty. Returns how many are held.
pub(crate) fn set_duties(json: &str) -> Result<usize, String> {
    let bodies: Vec<Value> = serde_json::from_str(json).map_err(|e| format!("bad duties: {e}"))?;
    let mut duties = Duties::default();
    for body in &bodies {
        if let Some(DutyMessage::Duty(duty)) = DutyMessage::parse(body) {
            duties.set(duty);
        }
    }
    let held = duties.len();
    *STATE.duties.lock().unwrap_or_else(|e| e.into_inner()) = duties;
    Ok(held)
}

/// Set the broadcast addresses of the networks the phone is on: a magic
/// packet is only ever sent to one of these or the limited broadcast.
pub(crate) fn set_networks(json: &str) -> Result<usize, String> {
    let list: Vec<String> = serde_json::from_str(json).map_err(|e| format!("bad networks: {e}"))?;
    let addrs: Vec<std::net::Ipv4Addr> = list.iter().filter_map(|a| a.parse().ok()).collect();
    let held = addrs.len();
    *STATE.local.lock().unwrap_or_else(|e| e.into_inner()) = addrs;
    Ok(held)
}

/// Where the helper key is kept.
fn key_path() -> Result<std::path::PathBuf, String> {
    let dir = crate::CERT_DIR
        .lock()
        .map_err(|_| "the data folder is not available".to_string())?
        .clone()
        .ok_or_else(|| "the data folder is not set".to_string())?;
    Ok(dir.join("wake_helper.key"))
}

/// Start listening, or say what is already running. Returns the status.
pub(crate) async fn start() -> Result<String, String> {
    let mut slot = HELPER.lock().await;
    if slot.is_none() {
        let key = pocketdesk_core::streaming::load_or_create_device_key(&key_path()?)
            .map_err(|e| format!("{e:#}"))?;
        let helper = Helper::start_default(key, Arc::clone(&STATE))
            .await
            .map_err(|e| format!("{e:#}"))?;
        tracing::info!("Wake helper: listening");
        *slot = Some(helper);
    }
    Ok(status_of(slot.as_ref()))
}

/// Stop listening and close the endpoint.
pub(crate) async fn stop() {
    if let Some(helper) = HELPER.lock().await.take() {
        helper.stop().await;
        tracing::info!("Wake helper: stopped");
    }
}

/// The phone's network changed: the listener, if it runs, looks again.
pub(crate) async fn network_changed() {
    if let Some(helper) = HELPER.lock().await.as_ref() {
        helper.network_changed().await;
    }
}

/// `{running, id, relays, sent, refused, dropped}`: counts, never a key
/// beyond the helper's own, which the computer is told.
pub(crate) async fn status() -> String {
    status_of(HELPER.lock().await.as_ref())
}

fn status_of(helper: Option<&Helper>) -> String {
    let served = STATE.served();
    match helper {
        Some(h) => serde_json::json!({
            "running": true,
            "id": h.id(),
            "relays": h.relay_urls(),
            "sent": served.sent,
            "refused": served.refused,
            "dropped": served.dropped,
        }),
        None => serde_json::json!({
            "running": false,
            "sent": served.sent,
            "refused": served.refused,
            "dropped": served.dropped,
        }),
    }
    .to_string()
}

/// Ask the helper the computer named in `info_json` (a `wake_helper_info`
/// body as the app kept it) to wake that computer, dialing as this phone's
/// own device key. Returns the helper's answer word: `sent`, `slow_down`,
/// `unknown` or `failed`; an error when no helper is named or none is
/// reached.
pub(crate) async fn ask(info_json: &str) -> Result<String, String> {
    let body: Value = serde_json::from_str(info_json).map_err(|_| "no helper known".to_string())?;
    let info = Info::parse(&body).ok_or_else(|| "no helper known".to_string())?;
    let helper = info
        .helper
        .ok_or_else(|| "this computer has no wake helper".to_string())?;
    let target = HelperTarget {
        iroh: helper.iroh,
        relays: helper.relays,
    };
    let answer = ask_helper(
        &target,
        &info.computer,
        crate::api::device_wan_key(),
        &[],
        true,
        Duration::from_secs(20),
    )
    .await
    .map_err(|_| "the helper could not be reached".to_string())?;
    Ok(answer.code().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn duty_body(computer: &str) -> Value {
        json!({
            "computer": computer,
            "mac": "02:00:5E:00:53:01",
            "broadcast": "192.0.2.255",
            "requesters": [KEY],
        })
    }

    #[test]
    fn what_the_computer_says_waits_once_and_is_taken_once() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.blocking_lock();
        take_json();
        receive(DUTY, &duty_body("computer-1"), "192.0.2.7", 1716);
        receive(DUTY, &duty_body("computer-1"), "192.0.2.7", 1716);
        receive(
            INFO,
            &json!({ "computer": "computer-1", "helper": null }),
            "192.0.2.7",
            1716,
        );
        let taken: Vec<Value> = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken.len(), 2, "a newer duty replaces the older");
        assert_eq!(taken[0]["type"], DUTY);
        assert_eq!(taken[0]["host"], "192.0.2.7");
        assert_eq!(taken[1]["type"], INFO);
        assert_eq!(take_json(), "[]");
    }

    #[test]
    fn a_body_that_does_not_read_never_reaches_the_app() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.blocking_lock();
        take_json();
        let mut bad = duty_body("computer-1");
        bad["mac"] = "not a mac".into();
        receive(DUTY, &bad, "192.0.2.7", 1716);
        receive(INFO, &json!({ "helper": null }), "192.0.2.7", 1716);
        receive(
            "kdeconnect.pocketdesk.other",
            &duty_body("c"),
            "192.0.2.7",
            1716,
        );
        assert_eq!(take_json(), "[]");
    }

    #[test]
    fn duties_are_replaced_whole_and_a_refusal_is_no_duty() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.blocking_lock();
        let list = json!([
            duty_body("computer-1"),
            duty_body("computer-2"),
            { "computer": "computer-3", "refused": true },
            { "nonsense": true },
        ]);
        assert_eq!(set_duties(&list.to_string()), Ok(2));
        assert!(STATE.duties.lock().unwrap().knows(KEY));
        assert_eq!(set_duties("[]"), Ok(0));
        assert!(!STATE.duties.lock().unwrap().knows(KEY));
        assert!(set_duties("not json").is_err());
    }

    #[test]
    fn networks_are_addresses_and_the_rest_is_dropped() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.blocking_lock();
        assert_eq!(
            set_networks(r#"["192.0.2.255", "garbage", "198.51.100.255"]"#),
            Ok(2)
        );
        assert_eq!(set_networks("[]"), Ok(0));
    }

    #[tokio::test]
    async fn without_a_helper_named_there_is_nothing_to_ask() {
        assert!(ask("not json").await.is_err());
        let none = json!({ "computer": "computer-1", "helper": null }).to_string();
        assert_eq!(
            ask(&none).await,
            Err("this computer has no wake helper".to_string())
        );
    }
}
