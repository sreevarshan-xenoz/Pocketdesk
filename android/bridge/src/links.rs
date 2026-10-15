//! Links between the phone and its computer (roadmap T8a): the phone hands
//! one over to open on the computer, and the computer hands one over that
//! the app shows as a notification. Both are `kdeconnect.pocketdesk.link
//! {url}`, and only a link `link_problem` passes goes either way.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::protocol::link::{LINK, link_problem};

/// How many received links wait for the app at most.
const MAX_WAITING: usize = 16;

/// Links the computer handed over, oldest first, for the app to show.
static WAITING: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(Default::default);

/// Keep a link the computer handed over, if it may be opened.
pub(crate) fn receive(body: &serde_json::Value) {
    let Some(url) = body.get("url").and_then(|v| v.as_str()) else {
        return;
    };
    if let Some(problem) = link_problem(url) {
        tracing::warn!("A link from the computer was dropped: {problem}");
        return;
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(url.to_string());
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// The links waiting for the app, as a JSON array, oldest first; taken.
pub(crate) fn take_links_json() -> String {
    let taken: Vec<String> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect();
    serde_json::to_string(&taken).unwrap_or_else(|_| "[]".to_string())
}

fn link_packet(url: &str) -> Result<NetworkPacket, String> {
    if let Some(problem) = link_problem(url) {
        return Err(format!("Not sent: {problem}"));
    }
    Ok(NetworkPacket::new(LINK).with_body(serde_json::json!({ "url": url })))
}

/// Hand `url` to the computer at `address` to open in its browser.
pub async fn send_link(address: String, port: u16, url: String) -> Result<(), String> {
    let packet = link_packet(url.trim())?;
    crate::api::push_packet(&address, port, &packet).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is process-wide: these tests take it one at a time.
    static QUEUE: Mutex<()> = Mutex::new(());

    #[test]
    fn a_link_from_the_computer_waits_once_and_only_if_it_may_open() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_links_json();
        receive(&serde_json::json!({ "url": "https://example.org/a" }));
        receive(&serde_json::json!({ "url": "javascript:alert(1)" }));
        receive(&serde_json::json!({ "nothing": true }));
        let taken: Vec<String> = serde_json::from_str(&take_links_json()).unwrap();
        assert_eq!(taken, ["https://example.org/a"]);
        assert_eq!(take_links_json(), "[]");
    }

    #[test]
    fn a_flood_of_links_keeps_only_the_newest() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_links_json();
        for n in 0..MAX_WAITING + 4 {
            receive(&serde_json::json!({ "url": format!("https://example.org/{n}") }));
        }
        let taken: Vec<String> = serde_json::from_str(&take_links_json()).unwrap();
        assert_eq!(taken.len(), MAX_WAITING);
        assert_eq!(taken[0], "https://example.org/4");
    }

    #[test]
    fn a_link_handed_over_is_checked_before_it_goes() {
        assert!(link_packet("https://example.org/").is_ok());
        assert!(link_packet("file:///sdcard/secret").is_err());
        let packet = link_packet("https://example.org/").unwrap();
        assert_eq!(packet.packet_type, LINK);
        assert_eq!(packet.body["url"], "https://example.org/");
    }
}
