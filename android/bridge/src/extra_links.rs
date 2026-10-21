//! Links to the phone's other computers (L8 part 2): besides the control
//! link to its own computer, one link to each other computer its owner
//! picked, on the home road. A copy of a notification goes on it, and the
//! answers and clears that computer sends back come in on it, kept with the
//! computer they came from, as the own link's are. Nothing else on such a
//! link is taken: the phone works with its own computer only.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, TcpDeviceSender};
use tokio::io::AsyncBufReadExt;
use tokio::sync::Mutex;

use crate::session::ControlWriter;

/// The most other computers kept linked (L8: three besides the own one).
pub(crate) const MAX_LINKS: usize = 3;

/// What is sent on such a link: a notification or its cancel, and how an
/// answer from that computer went (E1).
const SENT: [&str; 2] = [
    "kdeconnect.notification",
    pocketdesk_core::protocol::notification_action::NOTIFICATION_ANSWERED,
];

type Writer = Arc<Mutex<ControlWriter>>;

/// The links up, by the computer's address and control port.
static LINKS: LazyLock<Mutex<HashMap<(String, u16), Writer>>> = LazyLock::new(Default::default);

/// The links up, as `[{host, port}]`.
pub(crate) async fn live_json() -> String {
    let links = LINKS.lock().await;
    let list: Vec<serde_json::Value> = links
        .keys()
        .map(|(host, port)| serde_json::json!({ "host": host, "port": port }))
        .collect();
    serde_json::Value::Array(list).to_string()
}

/// Open a link to `address:port`, unless one is up or [`MAX_LINKS`] are.
pub(crate) async fn connect(address: String, port: u16) -> Result<(), String> {
    {
        let links = LINKS.lock().await;
        if links.contains_key(&(address.clone(), port)) {
            return Ok(());
        }
        if links.len() >= MAX_LINKS {
            return Err("as many other computers are linked as may be".to_string());
        }
    }
    let manager = crate::api::control_manager(Duration::from_secs(5))?;
    let identity = crate::api::client_identity();
    let (stream, claims) = manager
        .connect_named(&address, port, &identity)
        .await
        .map_err(|e| format!("Connection failed: {e}"))?;
    crate::peer_names::note(&address, claims);
    let stream: pocketdesk_core::protocol::connection::ControlDuplex = Box::new(stream);
    let (reader, writer) = tokio::io::split(stream);
    let writer: Writer = Arc::new(Mutex::new(writer));
    {
        let mut links = LINKS.lock().await;
        // Another caller linked it meanwhile: keep theirs.
        if links.contains_key(&(address.clone(), port)) {
            use tokio::io::AsyncWriteExt;
            let _ = writer.lock().await.shutdown().await;
            return Ok(());
        }
        links.insert((address.clone(), port), writer.clone());
    }
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            // The computer's answers and clears (T10c, L1), kept with it.
            if trimmed.contains("kdeconnect.notification.")
                && let Ok(packet) = NetworkPacket::from_wire(trimmed)
            {
                crate::notification_answers::receive(
                    &packet.packet_type,
                    &packet.body,
                    &address,
                    port,
                );
            }
        }
        // Gone: forgotten, unless a newer link took its place.
        let mut links = LINKS.lock().await;
        if links
            .get(&(address.clone(), port))
            .is_some_and(|live| Arc::ptr_eq(live, &writer))
        {
            links.remove(&(address, port));
        }
    });
    Ok(())
}

/// Send one of [`SENT`] (`body` a JSON object) on the link to
/// `address:port`; an error when none is up.
pub(crate) async fn send(
    address: &str,
    port: u16,
    packet_type: &str,
    body: &str,
) -> Result<(), String> {
    if !SENT.contains(&packet_type) {
        return Err(format!("{packet_type} is not sent this way"));
    }
    let body: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid notification: {e}"))?;
    if !body.is_object() {
        return Err("Invalid notification: not an object".to_string());
    }
    let writer = LINKS
        .lock()
        .await
        .get(&(address.to_string(), port))
        .cloned()
        .ok_or_else(|| "no link to that computer".to_string())?;
    let packet = NetworkPacket::new(packet_type).with_body(body);
    let sender = TcpDeviceSender::from_arc(writer, address.to_string());
    tokio::time::timeout(Duration::from_secs(5), sender.send_packet(&packet))
        .await
        .map_err(|_| "the computer did not take it in time".to_string())?
        .map_err(|e| format!("Failed to send: {e}"))
}

/// Close the links not to one of `keep` (`[(host, port)]`): a computer no
/// longer picked, or the one that became the phone's own.
pub(crate) async fn keep_only(keep: &[(String, u16)]) {
    let gone: Vec<Writer> = {
        let mut links = LINKS.lock().await;
        let gone_keys: Vec<(String, u16)> = links
            .keys()
            .filter(|k| !keep.contains(k))
            .cloned()
            .collect();
        gone_keys.iter().filter_map(|k| links.remove(k)).collect()
    };
    for writer in gone {
        use tokio::io::AsyncWriteExt;
        let _ = writer.lock().await.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_send_without_a_link_says_so_and_a_bad_body_is_refused() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let note = "kdeconnect.notification";
        let none = runtime.block_on(super::send("192.0.2.9", 1716, note, "{\"id\":\"k\"}"));
        assert!(none.unwrap_err().contains("no link"));
        let other = runtime.block_on(super::send("192.0.2.9", 1716, "kdeconnect.battery", "{}"));
        assert!(other.unwrap_err().contains("not sent this way"));
        let bad = runtime.block_on(super::send("192.0.2.9", 1716, note, "[1]"));
        assert!(bad.unwrap_err().contains("not an object"));
        assert_eq!(runtime.block_on(super::live_json()), "[]");
    }
}
