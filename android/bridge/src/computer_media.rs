//! The computer's music as it last pushed it (N5), kept for the app until
//! taken: only the newest matters, so a new push replaces one not taken.

use std::sync::Mutex;

/// The newest push not yet taken, with the computer it came from.
static WAITING: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Keep the push `body` from the computer at `host:port`.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(fields) = body.as_object() else {
        return;
    };
    let mut kept = serde_json::Value::Object(fields.clone());
    kept["host"] = serde_json::Value::from(host);
    kept["port"] = serde_json::Value::from(port);
    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = Some(kept);
}

/// Drop what waits: its link ended.
pub(crate) fn forget() {
    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The newest push as JSON, taken; empty when none came since.
pub(crate) fn take_json() -> String {
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map(|v| v.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_newest_push_waits() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        assert_eq!(take_json(), "");
        receive(
            &serde_json::json!({"playing": {"title": "One"}}),
            "192.0.2.7",
            1716,
        );
        receive(&serde_json::json!({"playing": null}), "192.0.2.7", 1716);
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert!(taken["playing"].is_null(), "the newest");
        assert_eq!(taken["host"], "192.0.2.7");
        assert_eq!(take_json(), "", "taken");
        receive(&serde_json::json!("not an object"), "192.0.2.7", 1716);
        assert_eq!(take_json(), "");
    }
}
