//! Links to the phone's other computers (L8 part 2): besides the control
//! link to its own computer, one link to each other computer its owner
//! picked, on the home road. A copy of a notification goes on it, and the
//! answers and clears that computer sends back come in on it (and whether
//! it is in use, N7), kept with the
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

/// The packet that names the link's role to the computer (L8 part 2).
const LINK_ROLE: &str = "kdeconnect.pocketdesk.link_role";

/// What is sent on such a link: a notification or its cancel, how an
/// answer from that computer went (E1), and whether it is in use (N7).
const SENT: [&str; 3] = [
    "kdeconnect.notification",
    pocketdesk_core::protocol::notification_action::NOTIFICATION_ANSWERED,
    pocketdesk_core::protocol::in_use::ASK,
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
    // Say what the link is for, so the computer neither pushes to it nor
    // shows the phone as linked by it (L8 part 2 review).
    let role = NetworkPacket::new(LINK_ROLE).with_body(serde_json::json!({ "role": "copies" }));
    tokio::time::timeout(
        Duration::from_secs(5),
        TcpDeviceSender::from_arc(writer.clone(), address.clone()).send_packet(&role),
    )
    .await
    .map_err(|_| "the computer did not take the link's role in time".to_string())?
    .map_err(|e| format!("Failed to send the link's role: {e}"))?;
    let raced = {
        let mut links = LINKS.lock().await;
        // Another caller linked it meanwhile: keep theirs.
        if links.contains_key(&(address.clone(), port)) || links.len() >= MAX_LINKS {
            true
        } else {
            links.insert((address.clone(), port), writer.clone());
            false
        }
    };
    if raced {
        // Closed outside the lock (L8 part 2 review).
        use tokio::io::AsyncWriteExt;
        let _ = writer.lock().await.shutdown().await;
        return Ok(());
    }
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            // What it says of this phone's standing there (BB1 review: its
            // copies went on to a computer that unpaired the phone).
            if let Ok(packet) = NetworkPacket::from_wire(trimmed) {
                crate::unpaired::heard(&address, &packet.packet_type, &packet.body);
            }
            // Whether that computer is in use (N7), kept with it.
            if trimmed.contains("in_use_answer")
                && let Ok(packet) = NetworkPacket::from_wire(trimmed)
                && packet.packet_type == pocketdesk_core::protocol::in_use::ANSWER
            {
                crate::in_use::receive(&packet.body, &address, port);
            }
            // The computer's answers and clears (T10c, L1), kept with it.
            if (trimmed.contains("kdeconnect.notification.")
                || trimmed.contains("notification_open"))
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
            links.remove(&(address.clone(), port));
            drop(links);
            // N7 review: it can show nothing now.
            crate::in_use::forget(&address, port);
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
    // Its copies stay on the phone once it said the phone is not paired (BB2).
    if crate::unpaired::refuses(address, packet_type) {
        return Err(crate::unpaired::REFUSED.to_string());
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
    let sender = TcpDeviceSender::from_arc(writer.clone(), address.to_string());
    match tokio::time::timeout(Duration::from_secs(5), sender.send_packet(&packet)).await {
        Ok(sent) => sent.map_err(|e| format!("Failed to send: {e}")),
        Err(_) => {
            // A write cut off mid-line would garble the next one: the link
            // goes (L8 part 2 review), and the next tick links again.
            {
                let mut links = LINKS.lock().await;
                if links
                    .get(&(address.to_string(), port))
                    .is_some_and(|live| Arc::ptr_eq(live, &writer))
                {
                    links.remove(&(address.to_string(), port));
                }
            }
            use tokio::io::AsyncWriteExt;
            let _ = writer.lock().await.shutdown().await;
            Err("the computer did not take it in time".to_string())
        }
    }
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
        for (host, port) in &gone_keys {
            crate::in_use::forget(host, *port);
        }
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
