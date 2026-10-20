//! Commands the computer sends this phone's automation apps (L6):
//! `kdeconnect.pocketdesk.command {name, text}`, kept with the computer it
//! came from for the app, which broadcasts it.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::command::{valid_name, valid_text};

/// How many commands wait for the app at most.
const MAX_WAITING: usize = 16;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a command from the computer at `host:port`, if it is within bounds.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let name = body.get("name").and_then(|v| v.as_str());
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let Some(name) = name.filter(|n| valid_name(n)) else {
        tracing::warn!("A command from the computer was dropped: no name it may have");
        return;
    };
    if !valid_text(text) {
        tracing::warn!("A command from the computer was dropped: its text is out of bounds");
        return;
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting
        .push_back(serde_json::json!({ "host": host, "port": port, "name": name, "text": text }));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The commands waiting, oldest first, as a JSON array; taken.
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

    /// L6: a command waits with its computer; one out of bounds is dropped.
    #[tokio::test]
    async fn a_command_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "name": "lights", "text": "off" }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "name": "", "text": "x" }),
            "192.0.2.7",
            1716,
        );
        receive(&serde_json::json!({ "name": "lights" }), "192.0.2.7", 1716);
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([
                { "host": "192.0.2.7", "port": 1716, "name": "lights", "text": "off" },
                { "host": "192.0.2.7", "port": 1716, "name": "lights", "text": "" }
            ])
        );
    }
}
