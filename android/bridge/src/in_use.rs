//! Whether each computer is in use, as it last answered (N7): kept with
//! the computer it came from, on the own link or another computer's (L8),
//! with when, for the app to tell which are current.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// Each computer's last answer, by address and control port: in use or
/// not (none when it does not say), and when it came.
static ANSWERS: LazyLock<Mutex<HashMap<(String, u16), (Option<bool>, Instant)>>> =
    LazyLock::new(Default::default);

/// Keep the answer `body` from the computer at `host:port`.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let said = pocketdesk_core::protocol::in_use::in_use(body);
    ANSWERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((host.to_string(), port), (said, Instant::now()));
}

/// Every computer's last answer, as `{"host:port": {inUse, ageMs}}`
/// (`inUse` null when it did not say).
pub(crate) fn json() -> String {
    let answers = ANSWERS.lock().unwrap_or_else(|e| e.into_inner());
    let map: serde_json::Map<String, serde_json::Value> = answers
        .iter()
        .map(|((host, port), (said, at))| {
            (
                format!("{host}:{port}"),
                serde_json::json!({ "inUse": said, "ageMs": at.elapsed().as_millis() as u64 }),
            )
        })
        .collect();
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_computer_s_last_answer_is_kept_with_it() {
        receive(&serde_json::json!({ "inUse": true }), "192.0.2.61", 1716);
        receive(&serde_json::json!({ "inUse": false }), "192.0.2.62", 1716);
        receive(&serde_json::json!({}), "192.0.2.62", 1716);
        let kept: serde_json::Value = serde_json::from_str(&json()).unwrap();
        assert_eq!(kept["192.0.2.61:1716"]["inUse"], true);
        assert!(
            kept["192.0.2.62:1716"]["inUse"].is_null(),
            "the newest: it does not say"
        );
        assert!(kept["192.0.2.61:1716"]["ageMs"].as_u64().unwrap() < 10_000);
    }
}
