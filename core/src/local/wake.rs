//! How a phone wakes this computer (N6): the network adapter a phone
//! reaches it on, that adapter's MAC and its subnet's broadcast address.
//! The server tells paired phones, so they can send the magic packet
//! themselves instead of asking the sleeping computer to.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// One network adapter, as far as waking goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub mac: Option<[u8; 6]>,
    /// Its IPv4 addresses with their prefix lengths.
    pub ipv4: Vec<(Ipv4Addr, u8)>,
    /// Ethernet (as opposed to Wi-Fi or anything else).
    pub wired: bool,
    /// A real adapter, not a tunnel, loopback or virtual switch.
    pub physical: bool,
    /// The adapter the default route leaves by.
    pub default: bool,
}

/// What a phone needs to wake this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeFacts {
    /// The adapter's name, for the desktop's Settings.
    pub adapter: String,
    /// `AA:BB:CC:DD:EE:FF`.
    pub mac: String,
    /// The subnet's directed broadcast address.
    pub broadcast: Ipv4Addr,
    /// Ethernet. Most Wi-Fi adapters wake a computer only from sleep.
    pub wired: bool,
}

/// The facts for the adapter holding `local` (the address a phone's
/// connection arrived on). When that is no adapter with a MAC (a tunnel,
/// loopback), the default route's adapter is used. `None` when neither has
/// a usable MAC and an IPv4 address.
pub fn facts_for(adapters: &[Adapter], local: Option<Ipv4Addr>) -> Option<WakeFacts> {
    let holding = local.and_then(|ip| {
        adapters
            .iter()
            .find(|a| a.ipv4.iter().any(|(addr, _)| *addr == ip))
            .and_then(|a| facts_of(a, Some(ip)))
    });
    holding.or_else(|| {
        adapters
            .iter()
            .filter(|a| a.default && a.physical)
            .find_map(|a| facts_of(a, None))
    })
}

fn facts_of(adapter: &Adapter, local: Option<Ipv4Addr>) -> Option<WakeFacts> {
    let mac = adapter.mac.filter(usable_mac)?;
    let (addr, prefix) = match local {
        Some(ip) => *adapter.ipv4.iter().find(|(addr, _)| *addr == ip)?,
        None => *adapter
            .ipv4
            .iter()
            .find(|(addr, _)| !addr.is_link_local() && !addr.is_loopback())?,
    };
    Some(WakeFacts {
        adapter: adapter.name.clone(),
        mac: format_mac(&mac),
        broadcast: broadcast_of(addr, prefix),
        wired: adapter.wired,
    })
}

/// A MAC a magic packet can carry: not all zeros (tunnels) or all ones.
fn usable_mac(mac: &[u8; 6]) -> bool {
    *mac != [0; 6] && *mac != [0xFF; 6]
}

fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The directed broadcast of `addr`'s subnet (`/32` and `/31` have none, so
/// the limited broadcast stands in).
fn broadcast_of(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    if prefix >= 31 {
        return Ipv4Addr::BROADCAST;
    }
    let host_bits = u32::MAX >> prefix;
    Ipv4Addr::from(u32::from(addr) | host_bits)
}

/// This computer's adapters.
pub fn adapters() -> Vec<Adapter> {
    use netdev::interface::types::InterfaceType;
    netdev::get_interfaces()
        .into_iter()
        .map(|i| Adapter {
            name: i.friendly_name.clone().unwrap_or_else(|| i.name.clone()),
            mac: i.mac_addr.map(|m| m.octets()),
            ipv4: i.ipv4.iter().map(|n| (n.addr(), n.prefix_len())).collect(),
            wired: matches!(
                i.if_type,
                InterfaceType::Ethernet
                    | InterfaceType::Ethernet3Megabit
                    | InterfaceType::FastEthernetT
                    | InterfaceType::FastEthernetFx
                    | InterfaceType::GigabitEthernet
            ),
            physical: i.is_physical() && !i.is_loopback() && !i.is_tun(),
            default: i.default,
        })
        .collect()
}

/// The facts for the adapter holding `local`, else the default one.
pub fn wake_facts(local: Option<Ipv4Addr>) -> Option<WakeFacts> {
    facts_for(&adapters(), local)
}

/// Whether this computer may send a magic packet to `target` for a phone
/// (the wake relay): the limited broadcast, or an address inside one of its
/// own IPv4 subnets, which is where a magic packet can wake anything.
/// Anything else would let a phone use the desktop to send packets
/// elsewhere.
pub fn may_relay_to(target: Ipv4Addr, adapters: &[Adapter]) -> bool {
    if target == Ipv4Addr::BROADCAST {
        return true;
    }
    adapters
        .iter()
        .flat_map(|a| a.ipv4.iter())
        .any(|&(addr, prefix)| {
            if prefix == 0 || prefix > 32 {
                return false;
            }
            let mask = u32::MAX << (32 - u32::from(prefix));
            u32::from(addr) & mask == u32::from(target) & mask
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_relay_sends_only_on_this_computers_own_networks() {
        let adapters = vec![Adapter {
            name: "lan".into(),
            mac: None,
            ipv4: vec![(Ipv4Addr::new(192, 168, 1, 20), 24)],
            wired: true,
            physical: true,
            default: true,
        }];
        assert!(may_relay_to(Ipv4Addr::BROADCAST, &adapters));
        assert!(may_relay_to(Ipv4Addr::new(192, 168, 1, 255), &adapters));
        assert!(may_relay_to(Ipv4Addr::new(192, 168, 1, 7), &adapters));
        assert!(!may_relay_to(Ipv4Addr::new(192, 168, 2, 255), &adapters));
        assert!(!may_relay_to(Ipv4Addr::new(203, 0, 113, 9), &adapters));
    }

    fn ethernet() -> Adapter {
        Adapter {
            name: "Ethernet".into(),
            mac: Some([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]),
            ipv4: vec![(Ipv4Addr::new(192, 168, 1, 20), 24)],
            wired: true,
            physical: true,
            default: false,
        }
    }

    fn wifi() -> Adapter {
        Adapter {
            name: "Wi-Fi".into(),
            mac: Some([0x02, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE]),
            ipv4: vec![(Ipv4Addr::new(10, 0, 0, 7), 16)],
            wired: false,
            physical: true,
            default: true,
        }
    }

    fn tunnel() -> Adapter {
        Adapter {
            name: "Tailscale".into(),
            mac: None,
            ipv4: vec![(Ipv4Addr::new(100, 64, 0, 9), 32)],
            wired: false,
            physical: false,
            default: false,
        }
    }

    #[test]
    fn the_adapter_a_phone_reached_is_the_one_to_wake() {
        let facts = facts_for(&[wifi(), ethernet()], Some(Ipv4Addr::new(192, 168, 1, 20))).unwrap();
        assert_eq!(facts.mac, "02:11:22:33:44:55");
        assert_eq!(facts.broadcast, Ipv4Addr::new(192, 168, 1, 255));
        assert!(facts.wired);
        assert_eq!(facts.adapter, "Ethernet");
    }

    #[test]
    fn a_connection_through_a_tunnel_falls_back_to_the_default_adapter() {
        let facts = facts_for(
            &[tunnel(), ethernet(), wifi()],
            Some(Ipv4Addr::new(100, 64, 0, 9)),
        )
        .unwrap();
        assert_eq!(facts.adapter, "Wi-Fi");
        assert_eq!(facts.broadcast, Ipv4Addr::new(10, 0, 255, 255));
        assert!(!facts.wired);
    }

    #[test]
    fn loopback_and_unknown_addresses_fall_back_too() {
        let facts = facts_for(&[ethernet(), wifi()], Some(Ipv4Addr::LOCALHOST)).unwrap();
        assert_eq!(facts.adapter, "Wi-Fi");
        assert_eq!(
            facts_for(&[ethernet(), wifi()], None).unwrap().adapter,
            "Wi-Fi"
        );
    }

    #[test]
    fn no_usable_mac_means_nothing_to_tell() {
        let mut zero = wifi();
        zero.mac = Some([0; 6]);
        assert_eq!(facts_for(&[zero, tunnel()], None), None);
        let mut virtual_default = wifi();
        virtual_default.physical = false;
        assert_eq!(facts_for(&[virtual_default], None), None);
    }

    #[test]
    fn broadcasts_follow_the_prefix() {
        let a = Ipv4Addr::new(172, 16, 5, 9);
        assert_eq!(broadcast_of(a, 24), Ipv4Addr::new(172, 16, 5, 255));
        assert_eq!(broadcast_of(a, 20), Ipv4Addr::new(172, 16, 15, 255));
        assert_eq!(broadcast_of(a, 32), Ipv4Addr::BROADCAST);
        assert_eq!(broadcast_of(a, 0), Ipv4Addr::BROADCAST);
    }

    #[test]
    fn this_computer_reads_without_panicking() {
        // Whatever the box has; on a machine with a network, some adapter.
        let _ = wake_facts(None);
    }
}
