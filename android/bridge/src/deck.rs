//! The deck (F1): the computer's buttons, listed and pressed; and the
//! computer's state (F2), asked the same way.

use std::sync::LazyLock;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

const DECK: &str = "kdeconnect.pocketdesk.deck";
const DECK_PRESS: &str = "kdeconnect.pocketdesk.deck_press";
const DECK_RESULT: &str = "kdeconnect.pocketdesk.deck_result";
const SYSTEM: &str = "kdeconnect.pocketdesk.system";

/// The computer's buttons: `{"buttons": [{"index", "label", "kind"}]}`.
pub async fn list_deck(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new(DECK).with_body(serde_json::json!({}));
    crate::api::query_peer(&address, port, &request, DECK, Duration::from_secs(5))
        .await
        .map(|body| body.to_string())
}

/// The computer's state (F2): CPU, memory, disks, battery and uptime, or
/// `{"off": true}`.
pub async fn system_state(address: String, port: u16) -> Result<String, String> {
    let request = NetworkPacket::new(SYSTEM).with_body(serde_json::json!({}));
    crate::api::query_peer(&address, port, &request, SYSTEM, Duration::from_secs(6))
        .await
        .map(|body| body.to_string())
}

/// One press in flight at a time: the computer's answers name no press,
/// and the shared connection hands a waiter the first of its type, so two
/// at once could each take the other's (F1 review).
static PRESSING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

/// Press the button listed at `index` as `label`: `{"index", "ok", "reason"}`.
/// A key combination or a link is done at once; a task that asks first is
/// answered as it starts waiting, so the answer comes within a few seconds.
pub async fn press_deck(
    address: String,
    port: u16,
    index: u32,
    label: String,
) -> Result<String, String> {
    let _alone = PRESSING.lock().await;
    let request = press_packet(index, &label);
    crate::api::query_peer(
        &address,
        port,
        &request,
        DECK_RESULT,
        Duration::from_secs(10),
    )
    .await
    .map(|body| body.to_string())
}

fn press_packet(index: u32, label: &str) -> NetworkPacket {
    NetworkPacket::new(DECK_PRESS).with_body(serde_json::json!({ "index": index, "label": label }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_names_the_index_and_the_label_it_was_listed_with() {
        let packet = press_packet(3, "Mute mic");
        assert_eq!(packet.packet_type, DECK_PRESS);
        assert_eq!(packet.body["index"], 3);
        assert_eq!(packet.body["label"], "Mute mic");
    }
}
