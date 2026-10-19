//! The computer's requests for this phone's recent photos (C7):
//! `phone_photos.list {}` and `phone_photos.fetch {id}`, and for a new one
//! taken with the camera (F3, `phone_photos.take {}`), kept with the
//! computer they came from for the app, which answers only its own
//! computer, with its switch on, and sends only a photo it listed.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::phone_photos::{FETCH, LIST, TAKE};

/// Requests waiting for the app, at most; the oldest go first.
const MAX_WAITING: usize = 20;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a request of `packet_type` from the computer at `host:port`, if it
/// is one: a list, or a fetch naming a photo by its id.
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, host: &str, port: u16) {
    let request = match packet_type {
        LIST => serde_json::json!({ "host": host, "port": port, "fetch": false }),
        // F3: the camera, offered to the phone's owner.
        TAKE => serde_json::json!({ "host": host, "port": port, "fetch": false, "take": true }),
        FETCH => match body.get("id").and_then(|v| v.as_u64()) {
            Some(id) => serde_json::json!({ "host": host, "port": port, "fetch": true, "id": id }),
            None => return,
        },
        _ => return,
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(request);
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The requests waiting, oldest first, as a JSON array; taken.
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
    async fn a_list_or_a_fetch_by_id_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let host = "192.0.2.7";
        receive(LIST, &serde_json::json!({}), host, 1716);
        receive(FETCH, &serde_json::json!({ "id": 42 }), host, 1716);
        receive(FETCH, &serde_json::json!({ "id": "42" }), host, 1716);
        receive(FETCH, &serde_json::json!({}), host, 1716);
        receive("kdeconnect.ping", &serde_json::json!({}), host, 1716);
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        let taken = taken.as_array().unwrap();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0]["fetch"], false);
        assert_eq!(taken[1]["id"], 42);
        assert_eq!(taken[1]["host"], host);
        assert_eq!(take_json(), "[]");
    }

    #[tokio::test]
    async fn a_take_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(TAKE, &serde_json::json!({}), "192.0.2.7", 1716);
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken[0]["take"], true);
        assert_eq!(taken[0]["host"], "192.0.2.7");
    }
}
