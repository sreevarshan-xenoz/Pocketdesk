//! The computer's answers to this phone's notifications (T10c):
//! `kdeconnect.notification.action {key, action}` and
//! `kdeconnect.notification.reply {requestReplyId, message}`, kept with the
//! computer they came from for the app, which acts only on its own
//! computer's.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::notification_action::{
    NOTIFICATION_ACTION, NOTIFICATION_REPLY, valid_action, valid_key, valid_reply,
};

/// Answers waiting for the app, at most; the oldest go first.
const MAX_WAITING: usize = 20;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep an answer of `packet_type` from the computer at `host:port`, if
/// its fields are within bounds.
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, host: &str, port: u16) {
    let text = |name: &str| body.get(name).and_then(|v| v.as_str());
    let answer = match packet_type {
        NOTIFICATION_ACTION => match (text("key"), text("action")) {
            (Some(key), Some(action)) if valid_key(key) && valid_action(action) => {
                serde_json::json!({ "host": host, "port": port, "key": key, "action": action })
            }
            _ => return,
        },
        NOTIFICATION_REPLY => match (text("requestReplyId"), text("message")) {
            (Some(key), Some(message)) if valid_key(key) && valid_reply(message) => {
                serde_json::json!({ "host": host, "port": port, "key": key, "message": message })
            }
            _ => return,
        },
        _ => return,
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(answer);
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The answers waiting, oldest first, as a JSON array; taken.
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

    #[tokio::test]
    async fn answers_within_bounds_wait_with_their_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let host = "192.0.2.7";
        receive(
            NOTIFICATION_ACTION,
            &serde_json::json!({"key": "k", "action": "Mark as read"}),
            host,
            1716,
        );
        receive(
            NOTIFICATION_REPLY,
            &serde_json::json!({"requestReplyId": "k", "message": "On my way"}),
            host,
            1716,
        );
        receive(
            NOTIFICATION_REPLY,
            &serde_json::json!({"requestReplyId": "k", "message": "  "}),
            host,
            1716,
        );
        receive(
            NOTIFICATION_ACTION,
            &serde_json::json!({"key": "k"}),
            host,
            1716,
        );
        receive(
            "kdeconnect.ping",
            &serde_json::json!({"key": "k", "action": "x"}),
            host,
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        let taken = taken.as_array().unwrap();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0]["action"], "Mark as read");
        assert_eq!(taken[1]["message"], "On my way");
        assert_eq!(taken[1]["host"], host);
        assert_eq!(take_json(), "[]");
        for i in 0..(MAX_WAITING + 5) {
            receive(
                NOTIFICATION_ACTION,
                &serde_json::json!({"key": format!("k{i}"), "action": "Ok"}),
                host,
                1716,
            );
        }
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken.as_array().unwrap().len(), MAX_WAITING);
        assert_eq!(taken[0]["key"], "k5", "the oldest went");
    }
}
