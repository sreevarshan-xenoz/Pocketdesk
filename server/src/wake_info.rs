//! Telling a paired phone how to wake this computer (roadmap N6): the MAC
//! and broadcast address of the adapter its connection arrived on, and the
//! outside wake port the user set up, so the phone sends the magic packet
//! itself instead of asking the computer that is asleep.

use pocketdesk_core::local::wake::WakeFacts;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

pub const WAKE_INFO: &str = "kdeconnect.pocketdesk.wake_info";

static ANNOUNCE: AtomicBool = AtomicBool::new(true);
static OUTSIDE_PORT: AtomicU16 = AtomicU16::new(0);

/// The user's settings, read at start (`wake_announce`, `wake_port`).
pub fn configure(announce: bool, outside_port: u16) {
    ANNOUNCE.store(announce, Ordering::Relaxed);
    OUTSIDE_PORT.store(outside_port, Ordering::Relaxed);
}

/// Whether a connection is told this packet: a phone's, never a
/// computer's (PC6 review M-2). A computer learns how to wake this one only
/// from `computer_wake`, only when paired both ways and with
/// `computer_wake_facts` on.
pub fn told_to(is_computer: bool) -> bool {
    !is_computer
}

/// The packet for `facts`. With announcing off or nothing to tell, its body
/// is empty, so a phone forgets what an earlier connection told it.
pub fn packet_for(announce: bool, facts: Option<&WakeFacts>, outside_port: u16) -> NetworkPacket {
    let body = match facts {
        Some(facts) if announce => serde_json::json!({
            "mac": facts.mac,
            "broadcast": facts.broadcast.to_string(),
            "wired": facts.wired,
            "port": outside_port,
        }),
        _ => serde_json::json!({}),
    };
    NetworkPacket::new(WAKE_INFO).with_body(body)
}

/// The packet for a connection that arrived on `local`, with the settings.
pub fn packet_for_connection(local: Option<IpAddr>) -> NetworkPacket {
    let announce = ANNOUNCE.load(Ordering::Relaxed);
    if !announce {
        return packet_for(false, None, 0);
    }
    let local: Option<Ipv4Addr> = match local {
        Some(IpAddr::V4(v4)) => Some(v4),
        Some(IpAddr::V6(v6)) => v6.to_ipv4_mapped(),
        None => None,
    };
    let facts = pocketdesk_core::local::wake::wake_facts(local);
    packet_for(
        announce,
        facts.as_ref(),
        OUTSIDE_PORT.load(Ordering::Relaxed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> WakeFacts {
        WakeFacts {
            adapter: "Ethernet".into(),
            mac: "02:11:22:33:44:55".into(),
            broadcast: Ipv4Addr::new(192, 168, 1, 255),
            wired: true,
        }
    }

    #[test]
    fn the_packet_says_how_to_wake_and_nothing_else() {
        let packet = packet_for(true, Some(&facts()), 40009);
        assert_eq!(packet.packet_type, WAKE_INFO);
        assert_eq!(
            packet.body,
            serde_json::json!({
                "mac": "02:11:22:33:44:55",
                "broadcast": "192.168.1.255",
                "wired": true,
                "port": 40009,
            })
        );
    }

    /// PC6 review M-2: phones are told as before; a computer never gets
    /// this packet, whatever `wake_announce` says.
    #[test]
    fn only_phones_are_told_this_packet() {
        assert!(told_to(false));
        assert!(!told_to(true));
    }

    #[test]
    fn nothing_is_told_when_announcing_is_off_or_unknown_so_phones_forget() {
        let off = packet_for(false, Some(&facts()), 9);
        assert_eq!(off.packet_type, WAKE_INFO);
        assert_eq!(off.body, serde_json::json!({}));
        assert_eq!(packet_for(true, None, 9).body, serde_json::json!({}));
    }
}
