//! Questions the computer puts to this phone (BF3):
//! `kdeconnect.pocketdesk.ask {id, question, expires_unix}`, kept with the
//! computer it came from for the app, which asks its owner. The answer goes
//! back as `kdeconnect.pocketdesk.ask_answer` (`send_phone_packet`).

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::ask::question_of;

/// How many questions wait for the app at most.
const MAX_WAITING: usize = 8;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a question the computer at `host:port` asked, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some((id, question, expires_unix)) = question_of(body) else {
        tracing::warn!("A question from the computer was dropped: not a question");
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    // The computer puts a question again to a phone that links again.
    if waiting.iter().any(|w| w["id"] == id.as_str()) {
        return;
    }
    waiting.push_back(serde_json::json!({
        "host": host,
        "port": port,
        "id": id,
        "question": question,
        "expires_unix": expires_unix,
    }));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
    tracing::info!("A question from the computer, kept for the phone to ask");
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The questions waiting, oldest first, as a JSON array; taken.
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

    /// BF3: a question waits once with its computer, cleaned; anything else
    /// is dropped.
    #[tokio::test]
    async fn a_question_waits_once_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let asked =
            serde_json::json!({ "id": "q-1", "question": "Deploy\nnow?", "expires_unix": 1_120 });
        receive(&asked, "192.0.2.7", 1716);
        receive(&asked, "192.0.2.7", 1716);
        receive(
            &serde_json::json!({ "id": "../q", "question": "Go?", "expires_unix": 1 }),
            "192.0.2.7",
            1716,
        );
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([{
                "host": "192.0.2.7", "port": 1716, "id": "q-1",
                "question": "Deploy now?", "expires_unix": 1_120,
            }])
        );
        assert_eq!(take_json(), "[]");
    }
}
