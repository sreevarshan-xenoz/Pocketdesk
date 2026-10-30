//! One pairing for all the owner's devices (ecosystem phase 4): this phone
//! asks its computer which of the owner's other computers it could pair
//! with too, and, once its owner said yes here, to be introduced to one.
//! The computer answers only a phone paired with its own code, and hands
//! back a pairing link the app pairs with as from a scanned code.

use std::time::Duration;

use pocketdesk_core::protocol::introductions::{
    INTRODUCE, INTRODUCE_ANSWER, PhoneAsk, PhoneRequest,
};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

/// One ask at a time: the shared connection hands a waiter the first
/// answer of its type, so two at once could each take the other's.
static ASKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The packet asking `ask` under the app's `request` id (the answer must
/// name it), or a fresh one when the app gave none that reads.
fn ask_packet(ask: PhoneAsk, request: String) -> NetworkPacket {
    let mut asked = PhoneRequest { request, ask };
    if PhoneRequest::parse(&asked.body()).is_none() {
        asked.request = uuid::Uuid::new_v4().simple().to_string();
    }
    NetworkPacket::new(INTRODUCE).with_body(asked.body())
}

/// Ask, and take only the answer naming this ask's request id.
async fn ask(
    address: String,
    port: u16,
    ask: PhoneAsk,
    request: String,
    wait: Duration,
) -> Result<String, String> {
    let _alone = ASKING.lock().await;
    let request = ask_packet(ask, request);
    crate::api::query_peer(&address, port, &request, INTRODUCE_ANSWER, wait)
        .await
        .map(|body| body.to_string())
}

/// The computers this phone could be introduced to: `{request, ask:
/// "list", computers: [{id, name}]}`, or `{request, ask, refused}`.
pub async fn offers(address: String, port: u16, request: String) -> Result<String, String> {
    ask(
        address,
        port,
        PhoneAsk::List,
        request,
        Duration::from_secs(6),
    )
    .await
}

/// Introduce this phone to the computer `device_id`: `{request, ask:
/// "pair", deviceId, link}`, or `{request, ask, deviceId, refused}`. The
/// computer waits up to 10 s for the other's answer, so this waits longer.
pub async fn introduce(
    address: String,
    port: u16,
    device_id: String,
    request: String,
) -> Result<String, String> {
    ask(
        address,
        port,
        PhoneAsk::Pair { device_id },
        request,
        Duration::from_secs(20),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_ask_names_itself_and_a_fresh_request() {
        let a = ask_packet(PhoneAsk::List, String::new());
        let b = ask_packet(
            PhoneAsk::Pair {
                device_id: "computer-b".to_string(),
            },
            String::new(),
        );
        assert_eq!(a.packet_type, INTRODUCE);
        assert_eq!(a.body["ask"], "list");
        assert_eq!(b.body["ask"], "pair");
        assert_eq!(b.body["deviceId"], "computer-b");
        assert_ne!(a.body["request"], b.body["request"]);
        assert!(
            PhoneRequest::parse(&b.body).is_some(),
            "the computer reads it"
        );
        let c = ask_packet(PhoneAsk::List, "app-7".to_string());
        assert_eq!(c.body["request"], "app-7");
    }
}
