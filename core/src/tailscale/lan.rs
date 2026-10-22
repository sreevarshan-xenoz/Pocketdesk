//! LAN peer discovery via mDNS (multicast DNS)
//!
//! Discovers Pocketdesk servers on the local network without Tailscale.
//! Uses `mdns-sd` to discover `_pocketdesk._tcp.local` services and resolves
//! their IP addresses and ports.
//!
//! This is a fallback for environments where Tailscale isn't running or
//! peers are on the same LAN and don't need Tailscale routing.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use super::PeerInfo;

/// mDNS service type used by Pocketdesk servers.
const SERVICE_TYPE: &str = "_pocketdesk._tcp.local.";

/// Scan timeout: how long to wait for mDNS responses during a single scan.
const SCAN_TIMEOUT: Duration = Duration::from_secs(3);

/// Events emitted by [`LanDiscoveryService`].
#[derive(Debug, Clone)]
pub enum LanEvent {
    /// A new LAN peer was discovered.
    PeerDiscovered(PeerInfo),
    /// A previously discovered peer is no longer responding.
    PeerOffline(String),
    /// The service is ready to emit events.
    ServiceReady,
    /// Discovery encountered a fatal error (e.g. daemon died).
    DiscoveryError {
        method: &'static str,
        reason: String,
    },
}

/// This server's live mDNS announcement. Withdraw it on the way out: the
/// daemon sends no goodbye when it just stops, and phones then keep listing
/// a computer that is gone until their cached record expires (over an hour
/// for the PTR record).
pub struct Announcement {
    daemon: ServiceDaemon,
    fullname: String,
    /// What it was made with, to make it again at new addresses (P1).
    name: String,
    device_id: String,
    port: u16,
    stream_port: u16,
    /// The addresses it announces, sorted.
    addresses: Vec<String>,
}

impl Announcement {
    /// The addresses it announces, as [`announced_addresses`] has them.
    pub fn addresses(&self) -> &[String] {
        &self.addresses
    }

    /// P1: announce again at `ips` (the old record withdrawn with a goodbye
    /// first, so phones drop the old address), if they differ from those
    /// announced. Whether it changed. Blocks up to a second for the goodbye.
    pub fn renew(&mut self, ips: Vec<String>) -> Result<bool> {
        let addresses = announced_addresses(ips);
        if !addresses_changed(&self.addresses, &addresses) {
            return Ok(false);
        }
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(Duration::from_secs(1));
        }
        let info = service_info(
            &self.name,
            &self.device_id,
            self.port,
            self.stream_port,
            &addresses,
        )?;
        self.fullname = info.get_fullname().to_string();
        self.daemon
            .register(info)
            .map_err(|e| anyhow::anyhow!("mDNS registration failed: {e}"))?;
        self.addresses = addresses;
        Ok(true)
    }

    /// Send the goodbye and wait (briefly) for it to go out.
    pub fn withdraw(self) {
        match self.daemon.unregister(&self.fullname) {
            Ok(done) => {
                if done.recv_timeout(Duration::from_secs(1)).is_err() {
                    warn!("mDNS goodbye for {} not confirmed", self.fullname);
                } else {
                    info!("Withdrew mDNS announcement {}", self.fullname);
                }
            }
            Err(e) => warn!("mDNS goodbye for {} failed: {e}", self.fullname),
        }
    }
}

/// LAN peer discovery service using mDNS.
pub struct LanDiscoveryService {
    tx: broadcast::Sender<LanEvent>,
    daemon: ServiceDaemon,
}

impl LanDiscoveryService {
    /// Create a new LAN discovery service.
    pub fn new() -> Result<Self> {
        let (tx, _) = broadcast::channel(100);
        let daemon = ServiceDaemon::new().context("Failed to create mDNS daemon")?;
        Ok(Self { tx, daemon })
    }

    /// Register this server on the LAN via mDNS. The service's port is the
    /// control port; the TXT record also carries the device id and the
    /// streaming port, so a phone can fill both ports in and recognise a
    /// computer it has paired with whatever its address.
    pub fn register_service(
        &self,
        name: &str,
        device_id: &str,
        port: u16,
        stream_port: u16,
        ips: Vec<String>,
    ) -> Result<Announcement> {
        let span = tracing::info_span!("mdns_register", service = %name, %port);
        let _enter = span.enter();

        let instance_name = name.to_string();
        let addresses = announced_addresses(ips);
        let service_info = service_info(name, device_id, port, stream_port, &addresses)?;
        let fullname = service_info.get_fullname().to_string();

        self.daemon.register(service_info).map_err(|e| {
            let err = format!("{e}");
            error!(error = %err, "mDNS registration failed");
            let _ = self.tx.send(LanEvent::DiscoveryError {
                method: "mdns_register",
                reason: err.clone(),
            });
            anyhow::anyhow!("mDNS registration failed: {err}")
        })?;

        info!(
            "Registered mDNS service: {} on port {}",
            instance_name, port
        );
        Ok(Announcement {
            daemon: self.daemon.clone(),
            fullname,
            name: name.to_string(),
            device_id: device_id.to_string(),
            port,
            stream_port,
            addresses,
        })
    }

    /// Subscribe to LAN discovery events.
    pub fn subscribe(&self) -> broadcast::Receiver<LanEvent> {
        self.tx.subscribe()
    }

    /// Run the discovery loop, polling mDNS at the given interval.
    pub async fn run(&self, scan_interval: Duration) {
        let span = tracing::info_span!("lan_discovery_loop");
        use tracing::Instrument;

        async move {
            let mut ticker = tokio::time::interval(scan_interval);
            let mut known_peers: HashMap<String, bool> = HashMap::new();

            // Initial scan
            match self.scan_lan_peers().await {
                Ok(peers) => {
                    for peer in peers {
                        known_peers.insert(peer.name.clone(), true);
                        let _ = self.tx.send(LanEvent::PeerDiscovered(peer));
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Initial LAN scan failed");
                    let _ = self.tx.send(LanEvent::DiscoveryError {
                        method: "initial_scan",
                        reason: e.to_string(),
                    });
                }
            }

            let _ = self.tx.send(LanEvent::ServiceReady);

            // Periodic re-scan
            loop {
                ticker.tick().await;
                match self.scan_lan_peers().await {
                    Ok(peers) => {
                        let mut current_names: HashMap<String, bool> = HashMap::new();
                        for peer in &peers {
                            let name = peer.name.clone();
                            current_names.insert(name.clone(), true);
                            if !known_peers.contains_key(&name) {
                                info!(peer = %peer.name, "Discovered new LAN peer");
                                let _ = self.tx.send(LanEvent::PeerDiscovered(peer.clone()));
                            }
                        }
                        // Detect peers that went offline
                        for name in known_peers.keys() {
                            if !current_names.contains_key(name) {
                                info!(peer = %name, "LAN peer went offline");
                                let _ = self.tx.send(LanEvent::PeerOffline(name.clone()));
                            }
                        }
                        known_peers = current_names;
                    }
                    Err(e) => {
                        warn!(error = %e, "Periodic LAN scan failed");
                    }
                }
            }
        }
        .instrument(span)
        .await
    }

    /// Perform a single scan of the LAN for Pocketdesk servers.
    pub async fn scan_lan_peers(&self) -> Result<Vec<PeerInfo>> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .context("Failed to browse mDNS")?;

        let mut peers = Vec::new();
        let deadline = std::time::Instant::now() + SCAN_TIMEOUT;

        // Collect mDNS responses within the timeout
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            if let Ok(ServiceEvent::ServiceResolved(info)) =
                receiver.recv_timeout(remaining.min(Duration::from_millis(500)))
                && let Some(peer) = service_info_to_peer(&info)
            {
                peers.push(peer);
            }
        }

        Ok(peers)
    }

    /// Get a reference to the event sender (for testing).
    #[cfg(test)]
    pub fn tx(&self) -> &broadcast::Sender<LanEvent> {
        &self.tx
    }
}

/// The addresses worth announcing on the local network: not the tailnet's
/// (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`). A phone on the LAN reaches the
/// computer by its LAN address, and the multicast answer would otherwise
/// tell every device on the network this computer's tailnet address.
/// Anything that does not parse is dropped too. A network that numbers its
/// LAN inside `100.64.0.0/10` (some hotspots and carrier routers do) would
/// leave nothing, so then every parsable address is announced.
fn announceable(ips: Vec<String>) -> Vec<String> {
    use std::net::IpAddr;
    let parsed: Vec<(String, IpAddr)> = ips
        .into_iter()
        .filter_map(|ip| ip.parse::<IpAddr>().ok().map(|addr| (ip, addr)))
        .collect();
    let off_tailnet: Vec<String> = parsed
        .iter()
        .filter(|(_, addr)| match addr {
            IpAddr::V4(v4) => {
                let [a, b, ..] = v4.octets();
                !(a == 100 && (b & 0xc0) == 64)
            }
            IpAddr::V6(v6) => {
                let s = v6.segments();
                !(s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0)
            }
        })
        .map(|(ip, _)| ip.clone())
        .collect();
    if off_tailnet.is_empty() {
        debug!("Only 100.64.0.0/10 addresses here, so they are announced");
        return parsed.into_iter().map(|(ip, _)| ip).collect();
    }
    off_tailnet
}

/// The TXT record of this server's announcement.
fn announcement_properties(
    name: &str,
    device_id: &str,
    stream_port: u16,
) -> HashMap<String, String> {
    HashMap::from([
        ("name".to_string(), name.to_string()),
        (
            "version".to_string(),
            crate::protocol::PROTOCOL_VERSION.to_string(),
        ),
        ("id".to_string(), device_id.to_string()),
        ("stream".to_string(), stream_port.to_string()),
    ])
}

/// The record announcing `name` at `addresses`.
fn service_info(
    name: &str,
    device_id: &str,
    port: u16,
    stream_port: u16,
    addresses: &[String],
) -> Result<ServiceInfo> {
    let properties = announcement_properties(name, device_id, stream_port);
    ServiceInfo::new(
        SERVICE_TYPE,
        name,
        &format!("{name}.local."),
        &addresses.iter().map(|s| s.as_str()).collect::<Vec<_>>()[..],
        port,
        Some(properties),
    )
    .context("Failed to create service info")
}

/// P1: the addresses an announcement holds for `ips`: the tailnet's left
/// out, each once, sorted, so two readings compare whatever their order.
pub fn announced_addresses(ips: Vec<String>) -> Vec<String> {
    let mut addresses = announceable(ips);
    addresses.sort();
    addresses.dedup();
    addresses
}

/// P1: whether `now` (as [`announced_addresses`] has them) differs from
/// `before`. An empty reading is no change: an adapter between networks,
/// or a failed read, must not withdraw the computer.
pub fn addresses_changed(before: &[String], now: &[String]) -> bool {
    !now.is_empty() && before != now
}

/// Convert an mDNS `ServiceInfo` to a `PeerInfo`.
fn service_info_to_peer(info: &ServiceInfo) -> Option<PeerInfo> {
    let hostname = info.get_hostname();
    let hostname = hostname.trim_end_matches('.'); // Remove trailing dot

    // Try to get a friendly name from TXT records, fall back to hostname
    let name = info
        .get_property("name")
        .and_then(|prop| prop.val())
        .and_then(|v| String::from_utf8(v.to_vec()).ok())
        .unwrap_or_else(|| hostname.split('.').next().unwrap_or("unknown").to_string());

    // Collect all IP addresses from the mDNS response
    let ips: Vec<String> = info
        .get_addresses()
        .iter()
        .map(|addr| addr.to_string())
        .collect();

    if ips.is_empty() {
        debug!("mDNS service {} has no IP addresses", name);
        return None;
    }

    let port = info.get_port();
    let dns_name = format!("{hostname}:{port}");

    Some(PeerInfo {
        name,
        dns_name,
        ips,
        online: true,
    })
}

/// Quick check whether mDNS is available on this system.
///
/// Returns `true` if an mDNS daemon can be created and a browse query
/// starts without error.
pub fn check_mdns_available() -> bool {
    match ServiceDaemon::new() {
        Ok(daemon) => {
            let available = daemon.browse(SERVICE_TYPE).is_ok();
            drop(daemon);
            available
        }
        Err(_) => false,
    }
}

/// Scan the LAN once for Pocketdesk servers.
///
/// Returns all discovered peers within the default timeout (3 seconds).
/// Useful for quick "list available peers" commands.
pub async fn scan_lan_once() -> Vec<PeerInfo> {
    let service = match LanDiscoveryService::new() {
        Ok(s) => s,
        Err(e) => {
            debug!("Failed to create LAN discovery service: {e}");
            return Vec::new();
        }
    };
    match service.scan_lan_peers().await {
        Ok(peers) => peers,
        Err(e) => {
            debug!("LAN scan failed: {e}");
            Vec::new()
        }
    }
}

/// Get all local IP addresses of the current machine.
///
/// Uses `hostname -I` on Linux to get all IPv4 and IPv6 addresses. Elsewhere
/// (Windows has no `hostname -I`) the adapters are enumerated directly, with
/// the same exclusions: no loopback, no link-local.
pub async fn get_local_ips() -> Vec<String> {
    #[cfg(not(target_os = "linux"))]
    {
        return match if_addrs::get_if_addrs() {
            Ok(interfaces) => interfaces
                .iter()
                .filter(|iface| !iface.is_loopback() && !iface.is_link_local())
                .map(|iface| iface.ip().to_string())
                .collect(),
            Err(e) => {
                debug!("Enumerating network adapters failed: {e}");
                Vec::new()
            }
        };
    }
    #[cfg_attr(not(target_os = "linux"), allow(unreachable_code))]
    let output = Command::new("hostname").arg("-I").output().await;
    if let Ok(output) = output {
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_ips_are_routable_addresses() {
        // Every listed address parses and is neither loopback nor link-local
        // (a CI box may have none at all, which is also fine).
        for ip in get_local_ips().await {
            let ip: std::net::IpAddr = ip.parse().expect("an IP address");
            assert!(!ip.is_loopback(), "{ip}");
            if let std::net::IpAddr::V6(v6) = ip {
                assert_ne!(v6.segments()[0] & 0xffc0, 0xfe80, "{ip} is link-local");
            }
        }
    }

    #[test]
    fn test_lan_discovery_service_new() {
        let service = LanDiscoveryService::new();
        assert!(service.is_ok());
    }

    #[test]
    fn test_check_mdns_available_runs() {
        // Should not panic. May return false in CI without mDNS.
        let _ = check_mdns_available();
    }

    /// Needs multicast on a real interface, so opt-in.
    #[tokio::test]
    #[ignore = "needs multicast on a real network interface"]
    async fn a_withdrawn_announcement_says_goodbye() {
        let name = format!("ll-test-{}", std::process::id());
        let ips = get_local_ips().await;
        assert!(!ips.is_empty(), "no local address to announce");
        let host = LanDiscoveryService::new().unwrap();
        let browser = ServiceDaemon::new().unwrap();
        let events = browser.browse(SERVICE_TYPE).unwrap();
        let announcement = host
            .register_service(&name, "id", 47160, 47161, ips)
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(ServiceEvent::ServiceResolved(info))
                    if info.get_fullname().starts_with(&name) =>
                {
                    announcement.withdraw();
                    break;
                }
                _ if std::time::Instant::now() > deadline => panic!("never seen"),
                _ => {}
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(ServiceEvent::ServiceRemoved(_, fullname)) if fullname.starts_with(&name) => {
                    break;
                }
                _ if std::time::Instant::now() > deadline => panic!("no goodbye seen"),
                _ => {}
            }
        }
        let _ = browser.shutdown();
    }

    /// P1: an announcement is made again only for other addresses, never
    /// for none.
    #[test]
    fn the_announcement_follows_its_addresses() {
        let a = announced_addresses(vec![
            "192.0.2.7".into(),
            "198.51.100.3".into(),
            "192.0.2.7".into(),
        ]);
        assert_eq!(a, vec!["192.0.2.7".to_string(), "198.51.100.3".to_string()]);
        let same = announced_addresses(vec!["198.51.100.3".into(), "192.0.2.7".into()]);
        assert!(!addresses_changed(&a, &same), "order aside");
        let moved = announced_addresses(vec!["203.0.113.9".into()]);
        assert!(addresses_changed(&a, &moved));
        assert!(!addresses_changed(&a, &[]), "no addresses read: kept");
        let with_tailnet = announced_addresses(vec![
            "192.0.2.7".into(),
            "100.64.1.2".into(),
            "198.51.100.3".into(),
        ]);
        assert!(
            !addresses_changed(&a, &with_tailnet),
            "a tailnet address beside the others is not announced"
        );
    }

    #[test]
    fn the_announcement_leaves_tailnet_addresses_out() {
        let ips = [
            "192.168.1.10",
            "100.64.0.9",
            "100.127.255.1",
            "100.128.0.1",
            "fd7a:115c:a1e0::9",
            "2001:db8::4",
            "not an address",
        ];
        assert_eq!(
            announceable(ips.iter().map(|s| s.to_string()).collect()),
            ["192.168.1.10", "100.128.0.1", "2001:db8::4"],
            "the tailnet's 100.64.0.0/10 and fd7a:115c:a1e0::/48 stay off the LAN"
        );
        assert_eq!(
            announceable(vec!["100.70.0.5".into(), "junk".into()]),
            ["100.70.0.5"],
            "a LAN numbered in 100.64.0.0/10 is still announced"
        );
    }

    #[test]
    fn the_announcement_names_the_device_and_its_stream_port() {
        let txt = announcement_properties("desk", "abc-123", 4716);
        assert_eq!(txt["name"], "desk");
        assert_eq!(txt["id"], "abc-123");
        assert_eq!(txt["stream"], "4716");
        assert!(txt.contains_key("version"));
    }

    #[test]
    fn test_service_info_to_peer_valid() {
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "test-peer",
            "test-peer.local.",
            "192.168.1.10",
            1716,
            None,
        )
        .expect("Failed to create test ServiceInfo");

        let peer = service_info_to_peer(&info);
        assert!(peer.is_some());
        let peer = peer.unwrap();
        assert_eq!(peer.name, "test-peer");
        assert!(peer.ips.contains(&"192.168.1.10".to_string()));
        assert_eq!(peer.dns_name, "test-peer.local:1716");
    }

    #[test]
    fn test_service_info_to_peer_no_name_property() {
        // Without "name" TXT property, should fall back to hostname
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            "my-machine",
            "my-machine.local.",
            "10.0.0.5",
            1716,
            None,
        )
        .expect("Failed to create test ServiceInfo");

        let peer = service_info_to_peer(&info);
        assert!(peer.is_some());
        let peer = peer.unwrap();
        // Falls back to hostname without domain suffix
        assert_eq!(peer.name, "my-machine");
        assert!(peer.ips.contains(&"10.0.0.5".to_string()));
    }

    #[test]
    fn test_event_channel() {
        let service = LanDiscoveryService::new().unwrap();
        let mut rx = service.subscribe();

        let _ = service.tx.send(LanEvent::ServiceReady);
        let event = rx.try_recv();
        assert!(matches!(event, Ok(LanEvent::ServiceReady)));
    }
}
