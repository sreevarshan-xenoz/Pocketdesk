//! The computer's asks for this phone's location (GeoClue):
//! `phone_location.ask {precision}`, kept with the computer they came from for
//! the app, which answers only its own computer, with its switch on, and
//! never stores or logs what it answers. Only the ask is kept here: the
//! answer goes out as a packet from the app, and a place never passes
//! through this file.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::phone_location::{ASK, ask_precision};

/// Asks waiting for the app, at most; the oldest go first. The computer asks
/// at most once in ten minutes, so more than a few are not an ask.
const MAX_WAITING: usize = 4;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep an ask of `packet_type` from the computer at `host:port`, if it is
/// one.
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, host: &str, port: u16) {
    if packet_type != ASK {
        return;
    }
    let ask = serde_json::json!({
        "host": host,
        "port": port,
        "precision": ask_precision(body).name(),
    });
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(ask);
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

    #[tokio::test]
    async fn an_ask_waits_with_its_computer_and_its_precision() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let host = "192.0.2.7";
        receive(ASK, &serde_json::json!({"precision": "exact"}), host, 1716);
        receive(ASK, &serde_json::json!({}), host, 1716);
        receive(ASK, &serde_json::json!({"precision": "street"}), host, 1716);
        receive("kdeconnect.ping", &serde_json::json!({}), host, 1716);
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        let taken = taken.as_array().unwrap();
        assert_eq!(taken.len(), 3);
        assert_eq!(taken[0]["precision"], "exact");
        assert_eq!(taken[0]["host"], host);
        assert_eq!(taken[0]["port"], 1716);
        assert_eq!(taken[1]["precision"], "city", "none said: the safe one");
        assert_eq!(
            taken[2]["precision"], "city",
            "a word not known: the safe one"
        );
        assert_eq!(take_json(), "[]");
    }

    #[tokio::test]
    async fn only_a_few_asks_wait_and_a_departure_drops_them() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        for _ in 0..10 {
            receive(ASK, &serde_json::json!({}), "192.0.2.7", 1716);
        }
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken.as_array().unwrap().len(), MAX_WAITING);
        receive(ASK, &serde_json::json!({}), "192.0.2.7", 1716);
        forget();
        assert_eq!(take_json(), "[]");
    }
}
