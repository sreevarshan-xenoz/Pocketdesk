//! What the computer asks this phone to do to itself (O2):
//! `kdeconnect.pocketdesk.phone_action {action}`, a screenshot or a lock,
//! kept with the computer it came from for the app, which does it with its
//! own switch on.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::phone_action::PhoneAction;

/// How many asks wait for the app at most.
const MAX_WAITING: usize = 8;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep an ask the computer at `host:port` sent, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(action) = body
        .get("action")
        .and_then(|v| v.as_str())
        .and_then(PhoneAction::parse)
    else {
        tracing::warn!("An ask from the computer was dropped: not an action");
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(serde_json::json!({ "host": host, "port": port, "action": action.as_str() }));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The asks waiting, oldest first, as a JSON array; taken.
pub(crate) fn take_json() -> String {
    let taken: Vec<serde_json::Value> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    serde_json::Value::Array(taken).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O2: an ask waits with its computer; anything else is dropped.
    #[tokio::test]
    async fn an_ask_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(&serde_json::json!({ "action": "lock" }), "192.0.2.7", 1716);
        receive(
            &serde_json::json!({ "action": "unlock" }),
            "192.0.2.7",
            1716,
        );
        receive(&serde_json::json!({}), "192.0.2.7", 1716);
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([{ "host": "192.0.2.7", "port": 1716, "action": "lock" }])
        );
        assert_eq!(take_json(), "[]");
    }
}
