//! What the computer asks this phone to do to itself (O2):
//! `kdeconnect.pocketdesk.phone_action {action}`, a screenshot or a lock,
//! kept with the computer it came from for the app, which does it with its
//! own switch on.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::phone_action::PhoneAction;

/// How many asks wait for the app at most.
const MAX_WAITING: usize = 8;

/// How long an ask waits for the app: a screenshot read later would be of
/// another moment, a lock a surprise (O2 review: a frozen app read them
/// minutes on).
const FRESH: Duration = Duration::from_secs(60);

static WAITING: Mutex<VecDeque<(Instant, serde_json::Value)>> = Mutex::new(VecDeque::new());

/// Keep an ask the computer at `host:port` sent, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(action) = body
        .get("action")
        .and_then(|v| v.as_str())
        .and_then(PhoneAction::parse)
    else {
        tracing::warn!("An ask from the computer was dropped: not an action");
        return;
    };
    // P2: an open names its app, a valid package name; nothing else names one.
    let app = body.get("app").and_then(|v| v.as_str());
    let mut ask = serde_json::json!({ "host": host, "port": port, "action": action.as_str() });
    match (action, app) {
        (PhoneAction::Open, Some(app))
            if pocketdesk_core::protocol::phone_action::valid_app_id(app) =>
        {
            ask["app"] = serde_json::Value::from(app);
            // Apps part 1: the computer wants the phone's screen shown too.
            if body.get("screen").and_then(|v| v.as_bool()) == Some(true) {
                ask["screen"] = serde_json::Value::from(true);
            }
        }
        (PhoneAction::Open, _) => {
            tracing::warn!("An ask to open an app was dropped: no app named");
            return;
        }
        // Apps part 2: a share names the file being sent, by name and size.
        (PhoneAction::Share, _) => {
            let name = body.get("name").and_then(|v| v.as_str());
            let size = body.get("size").and_then(|v| v.as_u64());
            match (name, size) {
                (Some(name), Some(size))
                    if pocketdesk_core::protocol::phone_action::valid_share_name(name) =>
                {
                    ask["name"] = serde_json::Value::from(name);
                    ask["size"] = serde_json::Value::from(size);
                }
                _ => {
                    tracing::warn!("An ask to share a file was dropped: no file named");
                    return;
                }
            }
        }
        // Apps part 1: the icons the computer keeps (id and version), so
        // only the rest are sent.
        (PhoneAction::Apps, _) => {
            let have = pocketdesk_core::protocol::phone_action::parse_have(body);
            if !have.is_empty() {
                ask["have"] = serde_json::json!(have);
            }
        }
        _ => {}
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back((Instant::now(), ask));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The asks waiting, oldest first, as a JSON array; taken. One older than
/// [`FRESH`] is dropped.
pub(crate) fn take_json() -> String {
    take_json_at(Instant::now())
}

fn take_json_at(now: Instant) -> String {
    let taken: Vec<serde_json::Value> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .filter(|(at, _)| now.saturating_duration_since(*at) <= FRESH)
        .map(|(_, ask)| ask)
        .collect();
    serde_json::Value::Array(taken).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O2: an ask waits with its computer; anything else is dropped.
    #[tokio::test]
    async fn an_ask_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(&serde_json::json!({ "action": "lock" }), "192.0.2.7", 1716);
        receive(
            &serde_json::json!({ "action": "unlock" }),
            "192.0.2.7",
            1716,
        );
        receive(&serde_json::json!({}), "192.0.2.7", 1716);
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([{ "host": "192.0.2.7", "port": 1716, "action": "lock" }])
        );
        assert_eq!(take_json(), "[]");
    }

    /// Apps part 2: a share names the file being sent, by name and size, or is dropped.
    #[tokio::test]
    async fn a_share_ask_names_its_file() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "action": "share", "name": "a.png", "size": 12 }),
            "192.0.2.7",
            1716,
        );
        for bad in [
            serde_json::json!({ "action": "share" }),
            serde_json::json!({ "action": "share", "name": "a.png" }),
            serde_json::json!({ "action": "share", "name": "../a.png", "size": 1 }),
            serde_json::json!({ "action": "share", "name": "a.png", "size": "1" }),
        ] {
            receive(&bad, "192.0.2.7", 1716);
        }
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([
                { "host": "192.0.2.7", "port": 1716, "action": "share", "name": "a.png", "size": 12 },
            ])
        );
    }

    /// Apps part 1: an open may want the screen, and a list ask carries the
    /// icons kept, only well-formed ones.
    #[tokio::test]
    async fn an_apps_ask_carries_what_the_computer_has() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "action": "apps", "have": {"org.example.a": 3, "bad id": 1, "org.example.b": "x"} }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "action": "open", "app": "org.example.a", "screen": true }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "action": "open", "app": "org.example.b", "screen": "yes" }),
            "192.0.2.7",
            1716,
        );
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(
            waiting,
            serde_json::json!([
                { "host": "192.0.2.7", "port": 1716, "action": "apps", "have": {"org.example.a": 3} },
                { "host": "192.0.2.7", "port": 1716, "action": "open", "app": "org.example.a", "screen": true },
                { "host": "192.0.2.7", "port": 1716, "action": "open", "app": "org.example.b" },
            ])
        );
    }

    /// O2 review: an ask read over a minute late is dropped.
    #[tokio::test]
    async fn an_ask_read_late_is_dropped() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "action": "screenshot" }),
            "192.0.2.7",
            1716,
        );
        let later = Instant::now() + FRESH + Duration::from_secs(1);
        assert_eq!(take_json_at(later), "[]");
    }
}
