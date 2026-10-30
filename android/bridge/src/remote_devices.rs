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

/// The packet asking `ask`, under a fresh request id.
fn ask_packet(ask: Ask) -> NetworkPacket {
    let request = Request {
        request: uuid::Uuid::new_v4().simple().to_string(),
        ask,
    };
    NetworkPacket::new(REMOTE_DEVICES).with_body(request.body())
}

async fn ask(address: String, port: u16, ask: Ask, wait: Duration) -> Result<String, String> {
    let _alone = ASKING.lock().await;
    let request = ask_packet(ask);
    crate::api::query_peer(&address, port, &request, REMOTE_DEVICES_ANSWER, wait)
        .await
        .map(|body| body.to_string())
}

/// The devices paired with the computer: `{request, ask: "list",
/// devices: [{id, name, computer, owner, guest, connected, you}]}`, or
/// `{request, ask, refused}` for a phone not the owner's.
pub async fn list(address: String, port: u16) -> Result<String, String> {
    ask(address, port, Ask::List, Duration::from_secs(6)).await
}

/// Remove `device_id` from the computer: `{request, ask: "remove",
/// deviceId, removed, reason}`, or `{request, ask, refused}`.
pub async fn remove(address: String, port: u16, device_id: String) -> Result<String, String> {
    ask(
        address,
        port,
        Ask::Remove { device_id },
        Duration::from_secs(10),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_ask_names_itself_and_a_fresh_request() {
        let a = ask_packet(Ask::List);
        let b = ask_packet(Ask::Remove {
            device_id: "lost".to_string(),
        });
        assert_eq!(a.packet_type, REMOTE_DEVICES);
        assert_eq!(a.body["ask"], "list");
        assert_eq!(b.body["ask"], "remove");
        assert_eq!(b.body["deviceId"], "lost");
        assert_ne!(a.body["request"], b.body["request"]);
        assert!(Request::parse(&b.body).is_some(), "the computer reads it");
    }
}
