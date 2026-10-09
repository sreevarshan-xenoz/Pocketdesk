//! LAN peer discovery via mDNS (multicast DNS)
//!
//! Discovers Linux Link servers on the local network without Tailscale.
//! Uses `mdns-sd` to discover `_linux-link._tcp.local` services and resolves
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

/// mDNS service type used by Linux Link servers.
const SERVICE_TYPE: &str = "_linux-link._tcp.local.";

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

    /// Register this server on the LAN via mDNS.
    pub fn register_service(&self, name: &str, port: u16, ips: Vec<String>) -> Result<()> {
        let span = tracing::info_span!("mdns_register", service = %name, %port);
        let _enter = span.enter();

        let hostname = format!("{}.local.", name);
        let service_type = SERVICE_TYPE.to_string();
        let instance_name = name.to_string();

        let mut properties = HashMap::new();
        properties.insert("name".to_string(), name.to_string());
        properties.insert(
            "version".to_string(),
            crate::protocol::PROTOCOL_VERSION.to_string(),
        );

        let service_info = ServiceInfo::new(
            &service_type,
            &instance_name,
            &hostname,
            &ips.iter().map(|s| s.as_str()).collect::<Vec<_>>()[..],
            port,
            Some(properties),
        )
        .context("Failed to create service info")?;

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
        Ok(())
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

    /// Perform a single scan of the LAN for Linux Link servers.
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

/// Scan the LAN once for Linux Link servers.
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
