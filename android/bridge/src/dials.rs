//! Numbers the computer asks this phone to dial (L4):
//! `kdeconnect.pocketdesk.dial {number}`, kept with the computer it came
//! from for the app, which opens its dialer with it (never calls).

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::dial::clean_number;

/// How many numbers wait for the app at most.
const MAX_WAITING: usize = 8;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a number the computer at `host:port` asked to dial, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(number) = body
        .get("number")
        .and_then(|v| v.as_str())
        .and_then(clean_number)
    else {
        tracing::warn!("A number from the computer was dropped: not a phone number");
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(serde_json::json!({ "host": host, "port": port, "number": number }));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The numbers waiting, oldest first, as a JSON array; taken.
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

    /// L4: a number waits with its computer, cleaned; anything else is dropped.
    #[tokio::test]
    async fn a_number_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "number": "+1 555 010 0100" }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "number": "javascript:1" }),
            "192.0.2.7",
            1716,
        );
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([{ "host": "192.0.2.7", "port": 1716, "number": "+15550100100" }])
        );
        assert_eq!(take_json(), "[]");
    }
}
