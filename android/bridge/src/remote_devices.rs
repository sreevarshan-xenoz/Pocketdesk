//! Removing a lost device from this phone (remote revoke): the devices
//! paired with a computer, and one of them removed there. The computer
//! answers only a phone paired with its owner's own code.

use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::protocol::remote_devices::{
    Ask, REMOTE_DEVICES, REMOTE_DEVICES_ANSWER, Request,
};

/// One ask at a time: the shared connection hands a waiter the first
/// answer of its type, so two at once could each take the other's.
static ASKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The packet asking `ask` under the app's `request` id (the answer must
/// name it), or a fresh one when the app gave none that reads.
fn ask_packet(ask: Ask, request: String) -> NetworkPacket {
    let mut asked = Request { request, ask };
    if Request::parse(&asked.body()).is_none() {
        asked.request = uuid::Uuid::new_v4().simple().to_string();
    }
    NetworkPacket::new(REMOTE_DEVICES).with_body(asked.body())
}

/// Ask, and take only the answer naming this ask's request id: a late
/// answer to an earlier ask is skipped (PR #574 review I2).
async fn ask(
    address: String,
    port: u16,
    ask: Ask,
    request: String,
    wait: Duration,
) -> Result<String, String> {
    let _alone = ASKING.lock().await;
    let request = ask_packet(ask, request);
    crate::api::query_peer(&address, port, &request, REMOTE_DEVICES_ANSWER, wait)
        .await
        .map(|body| body.to_string())
}

/// The devices paired with the computer: `{request, ask: "list",
/// devices: [{id, name, computer, owner, guest, connected, you}]}`, or
/// `{request, ask, refused}`.
pub async fn list(address: String, port: u16, request: String) -> Result<String, String> {
    ask(address, port, Ask::List, request, Duration::from_secs(6)).await
}

/// Remove `device_id` from the computer: `{request, ask: "remove",
/// deviceId, removed, reason}`, or `{request, ask, refused}`.
pub async fn remove(
    address: String,
    port: u16,
    device_id: String,
    request: String,
) -> Result<String, String> {
    ask(
        address,
        port,
        Ask::Remove { device_id },
        request,
        Duration::from_secs(15),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_ask_names_itself_and_a_fresh_request() {
        let a = ask_packet(Ask::List, String::new());
        let b = ask_packet(
            Ask::Remove {
                device_id: "lost".to_string(),
            },
            String::new(),
        );
        assert_eq!(a.packet_type, REMOTE_DEVICES);
        assert_eq!(a.body["ask"], "list");
        assert_eq!(b.body["ask"], "remove");
        assert_eq!(b.body["deviceId"], "lost");
        assert_ne!(a.body["request"], b.body["request"]);
        assert!(Request::parse(&b.body).is_some(), "the computer reads it");
        // The app's own id is used, so it can match the answer to it.
        let c = ask_packet(Ask::List, "app-7".to_string());
        assert_eq!(c.body["request"], "app-7");
    }
}
