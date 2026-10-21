//! The computer's clipboard as it last arrived (T10b): a push, or the answer
//! to the phone's own request, both `kdeconnect.clipboard {content}`, or
//! `{image: {id, size}}` while it holds an image (M3). Only the newest is
//! kept, with the computer it came from, for the app to apply when it holds
//! a link in the background.

use std::sync::Mutex;

use pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE;

/// What the computer's clipboard held.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Clip {
    Text(String),
    /// An image, by its PNG's SHA-256 (hex) and size, for N4's fetch.
    Image {
        id: String,
        size: u64,
    },
}

/// The newest: `(host, port, clip)`.
static LATEST: Mutex<Option<(String, u16, Clip)>> = Mutex::new(None);

/// An image id as the computer makes one: 64 lowercase hex digits.
fn is_image_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What `body` says the clipboard holds: text, or an image within N4's limit.
fn clip_of(body: &serde_json::Value) -> Option<Clip> {
    if let Some(content) = body.get("content").and_then(|v| v.as_str()) {
        return (!content.is_empty()).then(|| Clip::Text(content.to_string()));
    }
    let image = body.get("image")?;
    let id = image
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| is_image_id(id))?;
    let size = image
        .get("size")
        .and_then(|v| v.as_u64())
        .filter(|size| (1..=MAX_CLIPBOARD_IMAGE).contains(size))?;
    Some(Clip::Image {
        id: id.to_string(),
        size,
    })
}

/// Keep what `body` says from the computer at `host:port`, if it says
/// anything usable.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(clip) = clip_of(body) else {
        return;
    };
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some((host.to_string(), port, clip));
}

/// Drop what is kept: it came from a computer the phone left.
pub(crate) fn forget() {
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The newest as `{"host", "port", "content"}` or `{"host", "port",
/// "image": {"id", "size"}}`, or `""`; taken.
pub(crate) fn take_json() -> String {
    LATEST
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .map(|(host, port, clip)| match clip {
            Clip::Text(content) => {
                serde_json::json!({ "host": host, "port": port, "content": content })
            }
            Clip::Image { id, size } => serde_json::json!({
                "host": host,
                "port": port,
                "image": { "id": id, "size": size },
            }),
        })
        .map(|json| json.to_string())
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
        // An image with a bad id is no clipboard (M3).
        receive(
            &serde_json::json!({ "image": { "id": "x", "size": 10 } }),
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

    #[tokio::test]
    async fn an_image_is_kept_by_id_and_size_and_a_bad_one_dropped() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let id = "ab".repeat(32);
        receive(
            &serde_json::json!({ "image": { "id": id, "size": 2048 } }),
            "192.0.2.7",
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken["image"]["id"], id);
        assert_eq!(taken["image"]["size"], 2048);
        assert!(taken.get("content").is_none());
        for bad in [
            serde_json::json!({ "image": { "id": "AB".repeat(32), "size": 2048 } }),
            serde_json::json!({ "image": { "id": id, "size": 0 } }),
            serde_json::json!({ "image": { "id": id, "size": MAX_CLIPBOARD_IMAGE + 1 } }),
            serde_json::json!({ "image": { "id": id } }),
        ] {
            receive(&bad, "192.0.2.7", 1716);
            assert_eq!(take_json(), "", "{bad}");
        }
        // The newest wins, text or image.
        receive(&serde_json::json!({ "content": "text" }), "192.0.2.7", 1716);
        receive(
            &serde_json::json!({ "image": { "id": id, "size": 9 } }),
            "192.0.2.7",
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken["image"]["size"], 9);
    }
}
