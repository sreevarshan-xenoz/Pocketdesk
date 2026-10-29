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

/// How long after a goodbye mdns-sd may still send it again (120 ms), with
/// room to spare.
const GOODBYE_RESENT_WITHIN: Duration = Duration::from_millis(300);

impl Announcement {
    /// The addresses it announces, as [`announced_addresses`] has them.
    pub fn addresses(&self) -> &[String] {
        &self.addresses
    }

    /// P1: announce again at `addresses` (as [`announced_addresses`] has
    /// them). The old record goes first with a goodbye: Android's resolver
    /// keeps an old address until the service is lost (seen on the OPPO,
    /// where cache-flushed records alone left it). mdns-sd sends the goodbye
    /// again 120 ms later, so the new record waits past that (P1 review: it
    /// had removed the new one too). Blocks about 1.3 s at most.
    pub fn announce_at(&mut self, addresses: Vec<String>) -> Result<()> {
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(Duration::from_secs(1));
        }
        std::thread::sleep(GOODBYE_RESENT_WITHIN);
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
        Ok(())
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
        let deadline = tokio::time::Instant::now() + SCAN_TIMEOUT;

        // Collect mDNS responses within the timeout, awaited: a blocking wait
        // held a runtime thread for the whole scan, and a server asked to
        // stop then lingered until the scan was over.
        while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, receiver.recv_async()).await {
            if let ServiceEvent::ServiceResolved(info) = event
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

/// Whether `address` is the tailnet's (100.64.0.0/10, or Tailscale's IPv6).
fn is_tailnet(address: &str) -> bool {
    match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0xc0) == 64
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments();
            s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
        }
        Err(_) => false,
    }
}

/// What a change is judged by: the IPv4 addresses where there are any,
/// so IPv6's rotating temporary addresses do not move the announcement
/// each day (P1 review); else all.
fn judged(addresses: &[String]) -> Vec<&String> {
    let v4: Vec<&String> = addresses.iter().filter(|a| !a.contains(':')).collect();
    if v4.is_empty() {
        addresses.iter().collect()
    } else {
        v4
    }
}

/// P1: whether `now` (as [`announced_addresses`] has them) differs from
/// `before`. Not a change: an empty reading (an adapter between networks,
/// or a failed read must not withdraw the computer); one left with the
/// tailnet's only while the network's were announced (mDNS goes nowhere
/// then, P1 review); IPv6 alone moving beside an unchanged IPv4.
pub fn addresses_changed(before: &[String], now: &[String]) -> bool {
    if now.is_empty() {
        return false;
    }
    if now.iter().all(|a| is_tailnet(a)) && before.iter().any(|a| !is_tailnet(a)) {
        return false;
    }
    judged(before) != judged(now)
}

/// P1 review: a new reading is announced once two in a row agree, so
/// mdns-sd has opened its sockets on a new interface (its own check runs
/// every 5 s, and a record announced before then goes nowhere), and an
/// address that comes and goes does not flap phones.
#[derive(Debug, Default)]
pub struct Settle {
    pending: Option<Vec<String>>,
}

impl Settle {
    /// The addresses to announce now, given those announced (`current`)
    /// and a new `reading` (as [`announced_addresses`] has them).
    pub fn next(&mut self, current: &[String], reading: Vec<String>) -> Option<Vec<String>> {
        if !addresses_changed(current, &reading) {
            self.pending = None;
            return None;
        }
        if self.pending.as_ref() == Some(&reading) {
            self.pending = None;
            return Some(reading);
        }
        self.pending = Some(reading);
        None
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

/// The Pocketdesk computer an announcement describes, as the Nearby list
/// shows it ([`crate::local::nearby::Found`]): its id, name, addresses
/// (IPv4 first), control port and stream port, from the record this
/// server's [`announcement_properties`] makes. None without an id or an
/// address on this network ([`on_this_network`]). Nothing is read that the
/// announcement did not say before.
#[cfg(feature = "local")]
pub fn found_from(info: &ServiceInfo) -> Option<crate::local::nearby::Found> {
    let text = |key: &str| {
        info.get_property(key)
            .and_then(|prop| prop.val())
            .and_then(|v| String::from_utf8(v.to_vec()).ok())
    };
    let device_id = text("id").filter(|id| !id.is_empty())?;
    let name = text("name").unwrap_or_else(|| {
        info.get_hostname()
            .trim_end_matches('.')
            .split('.')
            .next()
            .unwrap_or("A computer")
            .to_string()
    });
    // Only addresses on this network: an announcement is anyone's word,
    // and a Pair here dials its first address.
    let mut addresses: Vec<std::net::IpAddr> = info
        .get_addresses()
        .iter()
        .copied()
        .filter(|a| on_this_network(*a))
        .collect();
    // IPv4 first: link-local IPv6 needs a scope a dial cannot name here.
    addresses.sort_by_key(|a| (a.is_ipv6(), *a));
    let addresses: Vec<String> = addresses.iter().map(|a| a.to_string()).collect();
    if addresses.is_empty() {
        return None;
    }
    Some(crate::local::nearby::Found {
        device_id,
        name,
        addresses,
        control_port: info.get_port(),
        stream_port: text("stream")
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::streaming::client::DEFAULT_STREAMING_PORT),
    })
}

/// Whether `ip` is on this computer's own network, as the LAN's other
/// rules judge it: a private or link-local IPv4 address, or a unique-local
/// or link-local IPv6 one. A tailnet's (100.64/10, `fd7a:115c:a1e0::/48`)
/// reaches from anywhere, and loopback is this computer: neither counts,
/// nor does any public address.
#[cfg(feature = "local")]
pub fn on_this_network(ip: std::net::IpAddr) -> bool {
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            let tailnet = v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64;
            !tailnet && (v4.is_private() || v4.is_link_local())
        }
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            let tailnet = s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0;
            let unique_local = (s[0] & 0xfe00) == 0xfc00;
            let link_local = (s[0] & 0xffc0) == 0xfe80;
            !tailnet && (unique_local || link_local)
        }
    }
}

/// A live look for Pocketdesk computers on this network (the Pair page's
/// Nearby list): what was found, kept up to date as announcements come and
/// go, until dropped. Only listens: nothing is announced by it.
#[cfg(feature = "local")]
pub struct Browse {
    daemon: ServiceDaemon,
    found: std::sync::Arc<std::sync::Mutex<HashMap<String, crate::local::nearby::Found>>>,
    started: std::time::Instant,
}

/// The most computers a look keeps: a flood of announcements grows
/// neither its memory nor the Nearby list past it.
#[cfg(feature = "local")]
pub const BROWSE_MOST: usize = 64;

/// Keep `computer`, found under the record `fullname`: in place of what
/// that record said before, or as a new one while fewer than
/// [`BROWSE_MOST`] are kept.
#[cfg(feature = "local")]
fn keep_found(
    found: &mut HashMap<String, crate::local::nearby::Found>,
    fullname: &str,
    computer: crate::local::nearby::Found,
) {
    if found.contains_key(fullname) || found.len() < BROWSE_MOST {
        found.insert(fullname.to_string(), computer);
    }
}

#[cfg(feature = "local")]
impl Browse {
    /// Start looking. `changed` is called after each change (to redraw).
    pub fn start(changed: impl Fn() + Send + 'static) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("Failed to create mDNS daemon")?;
        let events = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("mDNS browse failed: {e}"))?;
        let found: std::sync::Arc<std::sync::Mutex<HashMap<String, crate::local::nearby::Found>>> =
            Default::default();
        let kept = found.clone();
        std::thread::Builder::new()
            .name("nearby-browse".into())
            .spawn(move || {
                // Ends as the daemon shuts down and the channel closes.
                while let Ok(event) = events.recv() {
                    let mut found = kept.lock().unwrap_or_else(|e| e.into_inner());
                    match event {
                        ServiceEvent::ServiceResolved(info) => match found_from(&info) {
                            Some(computer) => {
                                keep_found(&mut found, info.get_fullname(), computer);
                            }
                            None => {
                                found.remove(info.get_fullname());
                            }
                        },
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            found.remove(&fullname);
                        }
                        _ => continue,
                    }
                    drop(found);
                    changed();
                }
            })
            .context("Failed to start the mDNS browse")?;
        Ok(Self {
            daemon,
            found,
            started: std::time::Instant::now(),
        })
    }

    /// What was found so far.
    pub fn found(&self) -> Vec<crate::local::nearby::Found> {
        self.found
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// How long it has looked.
    pub fn looked_for(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

#[cfg(feature = "local")]
impl Drop for Browse {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
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

/// Get all local IP addresses of the current machine: [`interface_ips`].
pub async fn get_local_ips() -> Vec<String> {
    interface_ips()
}

/// This machine's adapter addresses, read from its network interfaces, less
/// the ones no other device can reach ([`shareable_ips`]). Read directly on
/// every system: `hostname -I` is not on a stock Arch install (it comes from
/// `inetutils`), and without it the pairing link had no address.
pub fn interface_ips() -> Vec<String> {
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => shareable_ips(interfaces.iter().map(if_addrs::Interface::ip)),
        Err(e) => {
            debug!("Enumerating network adapters failed: {e}");
            Vec::new()
        }
    }
}

/// The addresses of `ips` worth offering another device: no loopback and no
/// link-local (`169.254.0.0/16`, `fe80::/10`), each once, in the order
/// given. A tailnet's addresses stay; the LAN announcement leaves them out
/// itself ([`announced_addresses`]).
pub fn shareable_ips(ips: impl IntoIterator<Item = std::net::IpAddr>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ip in ips {
        let reachable = match ip {
            std::net::IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
            std::net::IpAddr::V6(v6) => !v6.is_loopback() && v6.segments()[0] & 0xffc0 != 0xfe80,
        };
        let ip = ip.to_string();
        if reachable && !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_another_device_can_reach_are_shared() {
        let ips = [
            "127.0.0.1",
            "192.0.2.10",
            "169.254.7.1",
            "::1",
            "fe80::1",
            "febf::1",
            "2001:db8::10",
            "100.64.0.9",
            "fd7a:115c:a1e0::9",
            "192.0.2.10",
        ]
        .map(|ip| ip.parse::<std::net::IpAddr>().unwrap());
        assert_eq!(
            shareable_ips(ips),
            [
                "192.0.2.10",
                "2001:db8::10",
                "100.64.0.9",
                "fd7a:115c:a1e0::9"
            ]
        );
        assert!(shareable_ips([]).is_empty());
    }

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
        // P1 review: the network gone, the tailnet left: not announced there.
        let tailnet_only = announced_addresses(vec!["100.64.1.2".into()]);
        assert!(!addresses_changed(&a, &tailnet_only));
        // IPv6 moving beside the same IPv4 is no change; alone it is.
        let v6 = |s: &str| announced_addresses(vec!["192.0.2.7".into(), s.into()]);
        assert!(!addresses_changed(&v6("2001:db8::1"), &v6("2001:db8::2")));
        assert!(addresses_changed(
            &announced_addresses(vec!["2001:db8::1".into()]),
            &announced_addresses(vec!["2001:db8::2".into()])
        ));
    }

    /// P1 review: a reading is announced once seen twice in a row.
    #[test]
    fn a_new_reading_settles_before_it_is_announced() {
        let current = announced_addresses(vec!["192.0.2.7".into()]);
        let moved = announced_addresses(vec!["203.0.113.9".into()]);
        let other = announced_addresses(vec!["198.51.100.3".into()]);
        let mut settle = Settle::default();
        assert_eq!(settle.next(&current, moved.clone()), None, "first seen");
        assert_eq!(
            settle.next(&current, moved.clone()),
            Some(moved.clone()),
            "seen again"
        );
        assert_eq!(settle.next(&current, moved.clone()), None, "starts over");
        assert_eq!(settle.next(&current, other.clone()), None);
        assert_eq!(
            settle.next(&current, current.clone()),
            None,
            "back before it settled"
        );
        assert_eq!(
            settle.next(&current, other.clone()),
            None,
            "pending forgotten"
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

    /// A computer's announcement reads back as the Nearby list's row: its
    /// id, name, ports and its addresses on this network, IPv4 first; one
    /// with no id, or no address on this network, is not a computer to
    /// pair with.
    #[cfg(feature = "local")]
    #[test]
    fn an_announcement_reads_as_a_nearby_computer() {
        let info = service_info(
            "Test Tower",
            "00000000-0000-4000-8000-0000000000e1",
            1716,
            4716,
            &[
                "fd00:db8::5".to_string(),
                "2001:db8::5".to_string(),
                "192.0.2.5".to_string(),
                "169.254.10.5".to_string(),
            ],
        )
        .expect("a record");
        let found = found_from(&info).expect("a computer");
        assert_eq!(found.device_id, "00000000-0000-4000-8000-0000000000e1");
        assert_eq!(found.name, "Test Tower");
        assert_eq!(found.control_port, 1716);
        assert_eq!(found.stream_port, 4716);
        assert_eq!(found.addresses, ["169.254.10.5", "fd00:db8::5"]);
        let elsewhere = service_info(
            "Far Tower",
            "00000000-0000-4000-8000-0000000000e2",
            1716,
            4716,
            &["192.0.2.6".to_string(), "2001:db8::6".to_string()],
        )
        .expect("a record");
        assert_eq!(found_from(&elsewhere), None, "no address on this network");
        let no_id = ServiceInfo::new(
            SERVICE_TYPE,
            "phone",
            "phone.local.",
            "192.0.2.6",
            1716,
            None,
        )
        .expect("a record");
        assert_eq!(found_from(&no_id), None);
    }

    /// A look keeps at most [`BROWSE_MOST`] computers; one already kept is
    /// still updated.
    #[cfg(feature = "local")]
    #[test]
    fn a_look_keeps_at_most_64_computers() {
        let computer = |n: usize| crate::local::nearby::Found {
            device_id: format!("d-{n}"),
            name: format!("Desk {n}"),
            addresses: vec!["169.254.10.5".to_string()],
            control_port: 1716,
            stream_port: 4716,
        };
        let mut found = HashMap::new();
        for n in 0..BROWSE_MOST + 10 {
            keep_found(&mut found, &format!("desk-{n}"), computer(n));
        }
        assert_eq!(found.len(), BROWSE_MOST);
        keep_found(&mut found, "desk-0", computer(999));
        assert_eq!(found["desk-0"].device_id, "d-999", "updated");
        assert_eq!(found.len(), BROWSE_MOST);
    }

    /// Only this network's addresses count: private and link-local IPv4,
    /// unique-local and link-local IPv6; never loopback, a tailnet's or a
    /// public one.
    #[cfg(feature = "local")]
    #[test]
    fn only_this_networks_addresses_are_nearby() {
        let on = |a: &str| on_this_network(a.parse().unwrap());
        for here in ["169.254.10.5", "fe80::5", "fd00:db8::5"] {
            assert!(on(here), "{here}");
        }
        for away in [
            "127.0.0.1",
            "::1",
            "192.0.2.5",
            "2001:db8::5",
            "100.64.0.5",
            "fd7a:115c:a1e0::5",
            "::ffff:192.0.2.5",
        ] {
            assert!(!on(away), "{away}");
        }
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
