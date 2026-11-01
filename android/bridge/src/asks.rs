//! Questions the computer puts to this phone (BF3):
//! `kdeconnect.pocketdesk.ask {id, question, expires_unix}`, kept with the
//! computer it came from for the app, which asks its owner. The answer goes
//! back as `kdeconnect.pocketdesk.ask_answer` (`send_phone_packet`).
//!
//! An approval request (approve with my phone) rides in the same packet
//! with `"approve": {…}`; it is kept only when it is well formed and names,
//! as its computer, the certificate this phone pinned for the address it
//! came from, so a request relayed by another computer is never signed.

use std::collections::VecDeque;
use std::sync::Mutex;

use pocketdesk_core::protocol::approve::approval_of;
use pocketdesk_core::protocol::ask::{question_of, wait_secs_of};

/// How many questions wait for the app at most.
const MAX_WAITING: usize = 8;

static WAITING: Mutex<VecDeque<serde_json::Value>> = Mutex::new(VecDeque::new());

/// Keep a question the computer at `host:port` asked, if it is one. Its
/// end is counted on this phone's clock from its wait, when it says one
/// (BF3 review: a phone's clock ahead of the computer's had dropped it on
/// arrival as expired).
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let computer = body.get("approve").and_then(|_| pinned_computer(host));
    receive_at(body, host, port, now, computer.as_deref());
}

/// The certificate hash (lowercase hex) this phone pinned for the computer
/// at `host`.
fn pinned_computer(host: &str) -> Option<String> {
    let ip: std::net::IpAddr = host.trim().parse().ok()?;
    let dir = crate::CERT_DIR.lock().ok()?.clone()?;
    let manager = pocketdesk_core::streaming::transport::CertManager::load_or_create(&dir).ok()?;
    let label = pocketdesk_core::streaming::transport::peer_label_for_ip(ip);
    manager
        .known_peers()
        .into_iter()
        .find(|(known, _)| *known == label)
        .map(|(_, hash)| hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// `computer`: the certificate hash pinned for `host`, when the body
/// carries an approval.
fn receive_at(
    body: &serde_json::Value,
    host: &str,
    port: u16,
    now_unix: u64,
    computer: Option<&str>,
) {
    let Some((id, question, expires_unix)) = question_of(body) else {
        tracing::warn!("A question from the computer was dropped: not a question");
        return;
    };
    let approve = match body.get("approve") {
        None => None,
        Some(_) => match approval_of(body) {
            Some(a) if a.id == id && computer == Some(a.computer.as_str()) => Some(a.to_json()),
            Some(_) => {
                tracing::warn!(
                    "An approval request was dropped: not for the computer it came from"
                );
                return;
            }
            None => {
                tracing::warn!("An approval request was dropped: malformed");
                return;
            }
        },
    };
    let expires_unix = wait_secs_of(body).map_or(expires_unix, |wait| now_unix + wait);
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    // The computer puts a question again to a phone that links again.
    if waiting.iter().any(|w| w["id"] == id.as_str()) {
        return;
    }
    let mut kept = serde_json::json!({
        "host": host,
        "port": port,
        "id": id,
        "question": question,
        "expires_unix": expires_unix,
    });
    if let Some(approve) = approve {
        kept["approve"] = approve;
    }
    waiting.push_back(kept);
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
        // Its wait counted from now on this phone's clock.
        receive_at(
            &serde_json::json!({ "id": "q-2", "question": "Go?", "expires_unix": 1_120, "wait_secs": 90 }),
            "192.0.2.7",
            1716,
            5_000,
            None,
        );
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(waiting[0]["expires_unix"], 5_090);
    }

    /// Approve with my phone: an approval is kept only from the computer it
    /// names, as this phone pinned it.
    #[tokio::test]
    async fn an_approval_is_kept_only_from_the_computer_it_names() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let pinned = "ab".repeat(32);
        let approval = |id: &str, computer: &str| {
            serde_json::json!({
                "id": id, "question": "sudo for alice?", "expires_unix": 1_020, "wait_secs": 20,
                "approve": {
                    "id": id, "kind": "admin", "service": "sudo", "user": "alice",
                    "command": "sudo true", "computer": computer, "nonce": "01".repeat(32),
                    "issued_unix": 1_000, "expires_unix": 1_020,
                },
            })
        };
        receive_at(
            &approval("p-1", &pinned),
            "192.0.2.7",
            1716,
            5_000,
            Some(&pinned),
        );
        // Another computer's request, relayed; one this phone pinned none
        // for; one whose id differs from the ask's.
        receive_at(
            &approval("p-2", &"cd".repeat(32)),
            "192.0.2.7",
            1716,
            5_000,
            Some(&pinned),
        );
        receive_at(&approval("p-3", &pinned), "192.0.2.8", 1716, 5_000, None);
        let mut odd = approval("p-4", &pinned);
        odd["approve"]["id"] = serde_json::json!("p-5");
        receive_at(&odd, "192.0.2.7", 1716, 5_000, Some(&pinned));
        let waiting: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(waiting.as_array().unwrap().len(), 1);
        assert_eq!(waiting[0]["id"], "p-1");
        assert_eq!(waiting[0]["approve"]["computer"], pinned.as_str());
        assert_eq!(waiting[0]["expires_unix"], 5_020);
    }
}
