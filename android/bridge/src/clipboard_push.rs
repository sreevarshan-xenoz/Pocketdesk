//! The computer's clipboard as it last arrived (T10b): a push, or the answer
//! to the phone's own request, both `kdeconnect.clipboard {content}`. Only the
//! newest text is kept, with the computer it came from, for the app to apply
//! when it holds a link in the background.

use std::sync::Mutex;

/// The newest text: `(host, port, content)`.
static LATEST: Mutex<Option<(String, u16, String)>> = Mutex::new(None);

/// Keep `body`'s text from the computer at `host:port`, if it has any.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(content) = body.get("content").and_then(|v| v.as_str()) else {
        return;
    };
    if content.is_empty() {
        return;
    }
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((host.to_string(), port, content.to_string()));
}

/// Drop what is kept: it came from a computer the phone left.
pub(crate) fn forget() {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The newest text as `{"host", "port", "content"}`, or `""`; taken.
pub(crate) fn take_json() -> String {
    LATEST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map(|(host, port, content)| {
            serde_json::json!({ "host": host, "port": port, "content": content }).to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_newest_text_is_kept_and_taken_once() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "content": "first" }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "content": "second" }),
            "192.0.2.7",
            1716,
        );
        receive(&serde_json::json!({ "content": "" }), "192.0.2.7", 1716);
        receive(
            &serde_json::json!({ "image": { "id": "x" } }),
            "192.0.2.7",
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken["content"], "second");
        assert_eq!(taken["host"], "192.0.2.7");
        assert_eq!(take_json(), "", "taken once");
        receive(
            &serde_json::json!({ "content": "third" }),
            "192.0.2.7",
            1716,
        );
        forget();
        assert_eq!(take_json(), "");
    }
}
