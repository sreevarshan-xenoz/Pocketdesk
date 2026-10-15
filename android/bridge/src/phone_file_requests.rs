//! The computer's requests to browse this phone's shared folders (T10g):
//! `phone_files.list {root, path}` and `phone_files.fetch {root, path}`,
//! kept with the computer they came from for the app, which runs them only
//! for its own computer and only in folders its owner shared.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::phone_files::{FETCH, LIST, MAX_DEPTH, valid_path};

/// Requests waiting for the app, at most; the oldest go first.
const MAX_WAITING: usize = 20;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a request of `packet_type` from the computer at `host:port`, if it
/// is one and its path is within bounds.
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, host: &str, port: u16) {
    let fetch = match packet_type {
        LIST => false,
        FETCH => true,
        _ => return,
    };
    let Some(root) = body
        .get("root")
        .and_then(|v| v.as_u64())
        .and_then(|r| u32::try_from(r).ok())
    else {
        return;
    };
    let Some(path) = body.get("path").and_then(|v| v.as_array()) else {
        return;
    };
    if path.len() > MAX_DEPTH {
        return;
    }
    let path: Option<Vec<String>> = path
        .iter()
        .map(|n| n.as_str().map(str::to_string))
        .collect();
    let Some(path) = path.filter(|p| valid_path(p)) else {
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(serde_json::json!({
        "host": host, "port": port, "fetch": fetch, "root": root, "path": path,
    }));
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
    async fn only_requests_within_bounds_wait_with_their_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let host = "192.0.2.7";
        receive(
            LIST,
            &serde_json::json!({ "root": 0, "path": [] }),
            host,
            1716,
        );
        receive(
            FETCH,
            &serde_json::json!({ "root": 1, "path": ["Docs", "a.pdf"] }),
            host,
            1716,
        );
        receive(
            FETCH,
            &serde_json::json!({ "root": 1, "path": [".."] }),
            host,
            1716,
        );
        receive(LIST, &serde_json::json!({ "path": [] }), host, 1716);
        receive(
            "kdeconnect.ping",
            &serde_json::json!({ "root": 0, "path": [] }),
            host,
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        let taken = taken.as_array().unwrap();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0]["fetch"], false);
        assert_eq!(taken[1]["path"][1], "a.pdf");
        assert_eq!(taken[1]["host"], host);
        assert_eq!(take_json(), "[]");
    }
}
