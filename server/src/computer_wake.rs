//! The owner's computers wake each other (PC6). Each computer tells the
//! computers it is linked with how to wake it, as [`crate::wake_info`]
//! tells phones: the MAC and broadcast address of the adapter its link
//! leaves by. The other keeps them on its entry in `computers.json`
//! (owner-only), and its Wake sends the magic packet from there while the
//! link is down. `computer_wake_facts` off, a computer tells them to forget.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use pocketdesk_core::local::computers::{self, ComputerWake};
use pocketdesk_core::local::wake::WakeFacts;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

/// A computer's word on how to wake it, on the link between two of the
/// owner's computers: `{mac, broadcast, wired}`, or `{}` to forget.
pub const COMPUTER_WAKE: &str = "kdeconnect.pocketdesk.computer_wake";

/// The port a magic packet goes to.
pub const WAKE_PORT: u16 = 9;

static TELL: AtomicBool = AtomicBool::new(true);

/// The user's setting, read at start (`computer_wake_facts`).
pub fn configure(tell: bool) {
    TELL.store(tell, Ordering::Relaxed);
}

/// The packet for `facts`: empty with telling off or nothing to tell, so
/// the other computer forgets what an earlier link told it.
pub fn packet_for(tell: bool, facts: Option<&WakeFacts>) -> NetworkPacket {
    let body = match facts {
        Some(facts) if tell => serde_json::json!({
            "mac": facts.mac,
            "broadcast": facts.broadcast.to_string(),
            "wired": facts.wired,
        }),
        _ => serde_json::json!({}),
    };
    NetworkPacket::new(COMPUTER_WAKE).with_body(body)
}

/// The packet for a link to a computer known at `addresses` (best first),
/// or that came in on this computer's `local` address: the facts of the
/// adapter the link leaves by, with the setting. Blocking: it lists the
/// adapters.
pub fn packet_toward(addresses: &[String], local: Option<IpAddr>) -> NetworkPacket {
    packet_toward_as(TELL.load(Ordering::Relaxed), addresses, local)
}

fn packet_toward_as(tell: bool, addresses: &[String], local: Option<IpAddr>) -> NetworkPacket {
    if !tell {
        return packet_for(false, None);
    }
    let local = match local {
        Some(IpAddr::V4(v4)) => Some(v4),
        Some(IpAddr::V6(v6)) => v6.to_ipv4_mapped(),
        None => None,
    }
    .or_else(|| local_toward(addresses));
    packet_for(
        true,
        pocketdesk_core::local::wake::wake_facts(local).as_ref(),
    )
}

/// This computer's address on the adapter it reaches the first IPv4 of
/// `addresses` by, from the routing table: a UDP socket connected there
/// sends nothing.
fn local_toward(addresses: &[String]) -> Option<Ipv4Addr> {
    let to = addresses
        .iter()
        .find_map(|a| a.parse::<Ipv4Addr>().ok())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())?;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((to, WAKE_PORT)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// What a computer's packet says, `None` when it says to forget (or says
/// nothing usable): a MAC a magic packet can carry, written as
/// `AA:BB:CC:DD:EE:FF`, and an IPv4 broadcast address.
pub fn told(body: &serde_json::Value, now_unix: u64) -> Option<ComputerWake> {
    let mac = normal_mac(body.get("mac")?.as_str()?)?;
    let broadcast: Ipv4Addr = body.get("broadcast")?.as_str()?.trim().parse().ok()?;
    if broadcast.is_unspecified() || broadcast.is_loopback() || broadcast.is_multicast() {
        return None;
    }
    Some(ComputerWake {
        mac,
        broadcast,
        wired: body.get("wired").and_then(|v| v.as_bool()).unwrap_or(false),
        told_unix: now_unix,
    })
}

/// `mac` as six hex bytes split by `:` or `-`, upper case with colons;
/// none for all zeros or all ones, which no adapter has.
fn normal_mac(mac: &str) -> Option<String> {
    let bytes: Vec<u8> = mac
        .trim()
        .split([':', '-'])
        .map(|b| {
            (b.len() == 2)
                .then(|| u8::from_str_radix(b, 16).ok())
                .flatten()
        })
        .collect::<Option<_>>()?;
    if bytes.len() != 6 || bytes.iter().all(|b| *b == 0) || bytes.iter().all(|b| *b == 0xFF) {
        return None;
    }
    Some(
        bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// Keep what the computer `device_id` said of how to wake it, on its entry
/// here: a computer this one does not list is not kept. Blocking.
pub fn keep(dir: &Path, device_id: &str, body: &serde_json::Value, now_unix: u64) {
    let wake = told(body, now_unix);
    let saved = computers::update(dir, |file| {
        if let Some(c) = file.computers.iter_mut().find(|c| c.device_id == device_id) {
            c.wake = wake;
        }
    });
    if let Err(e) = saved {
        tracing::debug!("How to wake another computer was not kept: {e}");
    }
}

/// Take a computer's word on how to wake it, off the runtime.
pub async fn heard(device_id: &str, body: &serde_json::Value) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let (device_id, body) = (device_id.to_string(), body.clone());
    let now = pocketdesk_core::local::pin::now_unix();
    let _ = tokio::task::spawn_blocking(move || keep(&dir, &device_id, &body, now)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::computers::PeerComputer;

    fn facts() -> WakeFacts {
        WakeFacts {
            adapter: "Ethernet".into(),
            mac: "00:00:5E:00:53:01".into(),
            broadcast: Ipv4Addr::new(192, 0, 2, 255),
            wired: true,
        }
    }

    /// PC6: a computer's facts cross the link and read back as they were
    /// sent; with telling off, or nothing known, the packet is empty and
    /// reads as "forget".
    #[test]
    fn the_facts_cross_the_link_and_read_back() {
        let packet = packet_for(true, Some(&facts()));
        assert_eq!(packet.packet_type, COMPUTER_WAKE);
        assert_eq!(
            packet.body,
            serde_json::json!({"mac": "00:00:5E:00:53:01", "broadcast": "192.0.2.255", "wired": true})
        );
        let wire = serde_json::to_string(&packet).unwrap();
        let back: NetworkPacket = serde_json::from_str(&wire).unwrap();
        assert_eq!(
            told(&back.body, 50),
            Some(ComputerWake {
                mac: "00:00:5E:00:53:01".into(),
                broadcast: Ipv4Addr::new(192, 0, 2, 255),
                wired: true,
                told_unix: 50,
            })
        );
        let off = packet_for(false, Some(&facts()));
        assert_eq!(off.body, serde_json::json!({}));
        assert_eq!(told(&off.body, 50), None, "forget");
        assert_eq!(packet_for(true, None).body, serde_json::json!({}));
    }

    /// PC6: only a MAC a magic packet can carry and an IPv4 broadcast are
    /// kept; a MAC in dashes or lower case is written the usual way.
    #[test]
    fn only_usable_facts_are_taken() {
        let said = |mac: &str, broadcast: &str| {
            told(&serde_json::json!({"mac": mac, "broadcast": broadcast}), 1)
        };
        assert_eq!(
            said("00-00-5e-00-53-02", "192.0.2.255").map(|w| (w.mac, w.wired)),
            Some(("00:00:5E:00:53:02".to_string(), false))
        );
        for (mac, broadcast) in [
            ("00:00:00:00:00:00", "192.0.2.255"),
            ("FF:FF:FF:FF:FF:FF", "192.0.2.255"),
            ("00:00:5E:00:53", "192.0.2.255"),
            ("00:00:5E:00:53:0G", "192.0.2.255"),
            ("00:00:5E:00:53:001", "192.0.2.255"),
            ("00:00:5E:00:53:02", "2001:db8::ff"),
            ("00:00:5E:00:53:02", "127.0.0.1"),
            ("00:00:5E:00:53:02", "0.0.0.0"),
            ("00:00:5E:00:53:02", "224.0.0.1"),
        ] {
            assert_eq!(said(mac, broadcast), None, "{mac} {broadcast}");
        }
        assert_eq!(told(&serde_json::json!({"mac": 5}), 1), None);
    }

    fn computer(id: &str) -> PeerComputer {
        PeerComputer {
            device_id: id.to_string(),
            name: "Desk".to_string(),
            addresses: vec!["192.0.2.10".to_string()],
            control_port: 1716,
            stream_port: 4716,
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 1,
            link: "linked".to_string(),
            link_unix: 1,
            lets_drive: false,
            mine: false,
            wake: None,
        }
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-computer-wake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// PC6: the facts are kept on the entry of the computer that said
    /// them, an empty word forgets them, and a computer not listed here
    /// leaves nothing.
    #[test]
    fn the_facts_are_kept_on_the_computers_entry_and_forgotten() {
        let dir = temp("keep");
        computers::update(&dir, |file| file.computers.push(computer("desk"))).unwrap();
        keep(&dir, "desk", &packet_for(true, Some(&facts())).body, 7);
        let kept = computers::read(&dir).computers[0].wake.clone();
        assert_eq!(
            kept.map(|w| (w.mac, w.told_unix)),
            Some(("00:00:5E:00:53:01".into(), 7))
        );
        keep(&dir, "stranger", &packet_for(true, Some(&facts())).body, 8);
        assert_eq!(
            computers::read(&dir).computers.len(),
            1,
            "not listed: nothing"
        );
        keep(&dir, "desk", &packet_for(false, None).body, 9);
        assert_eq!(computers::read(&dir).computers[0].wake, None, "forgotten");
        // A file from before PC6 reads with nothing known.
        let raw = std::fs::read_to_string(dir.join(pocketdesk_core::local::paths::COMPUTERS_FILE))
            .unwrap();
        assert!(!raw.contains("\"wake\""), "none is not written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PC6: with telling off, the packet on a link says to forget.
    #[test]
    fn telling_off_says_to_forget_on_each_link() {
        let packet = packet_toward_as(false, &["192.0.2.10".to_string()], None);
        assert_eq!(packet.packet_type, COMPUTER_WAKE);
        assert_eq!(packet.body, serde_json::json!({}));
    }
}
