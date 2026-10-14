//! iroh WAN endpoint lifecycle (R1 stage 3, server slice).
//!
//! The quinn listener stays the LAN transport; this module owns one always-on
//! iroh endpoint with a persisted identity, feeding accepted v1 streaming
//! connections into the same [`StreamingServer`] pipeline. The endpoint's
//! identity (`EndpointId` plus current relay URLs and direct addresses) is
//! pushed to registered control-channel clients as a
//! `kdeconnect.pocketdesk.endpoint` packet — the phone caches it and dials the
//! full `EndpointAddr` later. Unless `dht_publish = "off"`, the endpoint also
//! publishes a record signed with its key on the Mainline DHT (roadmap N3),
//! so a phone whose cached address went stale can look it up by id.

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::address_lookup::{AddrFilter, AddressLookup as _};
use iroh::endpoint::QuicTransportConfig;
use iroh::endpoint::presets::Minimal;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr};
use iroh_mainline_address_lookup::DhtAddressLookup;
use pocketdesk_core::local::config::DhtPublish;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};
use pocketdesk_core::streaming::input_packet::InputPacket;
use pocketdesk_core::streaming::transport::{CertManager, StreamTransportConfig};
use pocketdesk_core::streaming::{IrohConnection, StreamingConfig, StreamingServer};
use tokio::sync::broadcast;

/// Packet type carrying the server's WAN identity to registered clients.
pub const ENDPOINT_PACKET: &str = "kdeconnect.pocketdesk.endpoint";

/// Persisted iroh identity key, next to `config.toml`.
const KEY_FILE: &str = "iroh_secret.key";

pub fn key_path() -> Result<PathBuf> {
    Ok(pocketdesk_core::local::paths::config_dir()
        .context("unable to determine config directory")?
        .join(KEY_FILE))
}

/// Load the endpoint identity from `path`, creating one (mode 0600) if absent.
///
/// The key file is the server's iroh identity — losing it changes the
/// `EndpointId` the phone has cached, so a corrupt or truncated file is a hard
/// error rather than a silent regenerate.
pub fn load_or_create_secret_key(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let bytes =
            std::fs::read(path).with_context(|| format!("failed reading {}", path.display()))?;
        let raw: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "{} is not a 32-byte iroh secret key — restore it or delete it \
                     and re-pair (the phone caches the EndpointId)",
                path.display()
            )
        })?;
        return Ok(SecretKey::from_bytes(&raw));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let key = SecretKey::generate();
    // create_new: if a second server instance won the race, adopt its key
    // rather than silently diverging identities.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path);
    match file {
        Ok(mut file) => {
            file.write_all(&key.to_bytes())
                .with_context(|| format!("failed writing {}", path.display()))?;
            // Unix got 0600 at creation; Windows gets the owner-only DACL.
            #[cfg(windows)]
            {
                drop(file);
                pocketdesk_core::secure_fs::restrict_to_owner(path)
                    .with_context(|| format!("failed restricting {}", path.display()))?;
            }
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("failed reading {}", path.display()))?;
            let raw: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("{} has unexpected length", path.display()))?;
            Ok(SecretKey::from_bytes(&raw))
        }
        Err(e) => Err(e).with_context(|| format!("failed creating {}", path.display())),
    }
}

/// WAN identity announcement built from an [`EndpointAddr`]. `dht` tells
/// the phone it may look this desktop up on the Mainline DHT (N3); a phone
/// never looks up a desktop that publishes nothing.
pub fn endpoint_packet(addr: &EndpointAddr, id: EndpointId, dht: bool) -> NetworkPacket {
    NetworkPacket::new(ENDPOINT_PACKET).with_body(serde_json::json!({
        "endpointId": id.to_string(),
        "relayUrls": addr.relay_urls().map(|u| u.to_string()).collect::<Vec<_>>(),
        "directAddrs": addr.ip_addrs().map(|a| a.to_string()).collect::<Vec<_>>(),
        "dht": dht,
    }))
}

/// The endpoint's current announcement.
pub fn current_packet(endpoint: &Endpoint) -> NetworkPacket {
    endpoint_packet(&endpoint.addr(), endpoint.id(), on_dht(endpoint))
}

/// This process's `dht_publish`, set when its one WAN endpoint binds.
static DHT_MODE: std::sync::OnceLock<DhtPublish> = std::sync::OnceLock::new();

/// Whether a phone should look this endpoint up on the DHT: it has joined
/// the DHT (its only address lookup is the DHT's), and its record holds
/// something. A desktop whose only relay has a private address publishes
/// nothing, and a phone looking it up would only tell the DHT it looks.
fn on_dht(endpoint: &Endpoint) -> bool {
    let with_ips = DHT_MODE.get() == Some(&DhtPublish::Addresses);
    let now: Vec<TransportAddr> = endpoint.addr().addrs.into_iter().collect();
    endpoint
        .address_lookup()
        .is_ok_and(|lookup| !lookup.is_empty())
        && !publishable(&now, with_ips).is_empty()
}

/// An IP a phone anywhere could reach: not private, loopback, link-local,
/// carrier NAT (100.64/10, which is also Tailscale's), unique-local IPv6
/// (fc00::/7, Tailscale's too), multicast, reserved or documentation.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (b & 0xc0) == 64)
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (b & 0xfe) == 18)
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            // Global unicast (2000::/3), less the documentation prefixes
            // (2001:db8::/32 and 3fff::/20).
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                && !(s[0] == 0x3fff && (s[1] & 0xf000) == 0)
        }
    }
}

/// Name suffixes that only mean something on a home or office network.
const LAN_SUFFIXES: [&str; 9] = [
    "localhost",
    "local",
    "lan",
    "home",
    "home.arpa",
    "internal",
    "localdomain",
    "intranet",
    "corp",
];

/// A relay a phone anywhere could reach: one with a public name, or a public
/// IP. A single-label name (`nas`) or a LAN suffix (`pi.lan`) names a
/// machine on the home network, and publishing it would name the network.
fn is_public_relay(url: &iroh::RelayUrl) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name.contains('.')
                && !LAN_SUFFIXES
                    .iter()
                    .any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}")))
        }
        Some(url::Host::Ipv4(v4)) => is_public_ip(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => is_public_ip(IpAddr::V6(v6)),
        None => false,
    }
}

/// What the DHT record may hold (N3): relays a phone anywhere could reach,
/// and, with `with_ips` (`dht_publish = "addresses"`), public IP addresses.
/// LAN, tailnet and documentation addresses are never published.
pub fn publishable(addrs: &[TransportAddr], with_ips: bool) -> Vec<TransportAddr> {
    addrs
        .iter()
        .filter(|addr| match addr {
            TransportAddr::Relay(url) => is_public_relay(url),
            TransportAddr::Ip(socket) => with_ips && is_public_ip(socket.ip()),
            _ => false,
        })
        .cloned()
        .collect()
}

/// The server's own check of its DHT record, for Diagnostics (N3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhtCheck {
    /// The DHT node has not started: its bootstrap nodes do not resolve.
    NotJoined,
    /// The record read back holds an address published now; `ips` when one
    /// of them is an IP address, not only a relay.
    Found { ips: bool },
    /// Nothing, or only a stale record, came back.
    NotFound,
    /// No address may be published (only private relays, and IPs off).
    NothingToPublish,
}

type DhtCheckState = Arc<std::sync::Mutex<Option<DhtCheck>>>;

/// The DHT lookup per `dht_publish`, publishing with `key` through the
/// [`publishable`] filter, on a DHT node bootstrapped by `dht`.
fn dht_lookup(
    mode: DhtPublish,
    key: &SecretKey,
    dht: pocketdesk_core::streaming::dht::DhtBuilder,
) -> Result<DhtAddressLookup> {
    let with_ips = mode == DhtPublish::Addresses;
    let filter = AddrFilter::new(move |addrs| Cow::Owned(publishable(addrs, with_ips)));
    DhtAddressLookup::builder()
        .dht_builder(dht)
        .secret_key(key.clone())
        .addr_filter(filter)
        .build()
        .context("the Mainline DHT could not start")
}

/// One attempt at a DHT node for this endpoint: `None` while the bootstrap
/// nodes do not resolve (no DNS yet) or the node cannot start.
async fn join_dht(endpoint: &Endpoint, mode: DhtPublish) -> Option<DhtAddressLookup> {
    let nodes =
        pocketdesk_core::streaming::dht::bootstrap_nodes(std::time::Duration::from_secs(10)).await;
    if nodes.is_empty() {
        tracing::debug!("WAN: the DHT's bootstrap nodes do not resolve yet");
        return None;
    }
    let dht = pocketdesk_core::streaming::dht::dht_builder(&nodes);
    dht_lookup(mode, endpoint.secret_key(), dht)
        .map_err(|e| tracing::warn!("WAN: {e:#}"))
        .ok()
}

/// Join the DHT in the background, so the WAN endpoint never waits on it:
/// each minute until the bootstrap nodes resolve (as after a boot before the
/// network is up), then add the lookup to the endpoint (iroh publishes the
/// current addresses to it at once) and keep checking the record.
fn spawn_dht(endpoint: Endpoint, mode: DhtPublish, state: DhtCheckState) {
    if mode == DhtPublish::Off {
        return;
    }
    tokio::spawn(async move {
        let lookup = loop {
            if let Some(lookup) = join_dht(&endpoint, mode).await {
                break lookup;
            }
            *state.lock().unwrap_or_else(|e| e.into_inner()) = Some(DhtCheck::NotJoined);
            tokio::time::sleep(DHT_RETRY).await;
            if endpoint.is_closed() {
                return;
            }
        };
        let Ok(services) = endpoint.address_lookup() else {
            return;
        };
        services.add(lookup.clone());
        tracing::info!("WAN: publishing on the Mainline DHT ({})", mode.as_str());
        check_dht_record(lookup, endpoint, mode, state).await;
    });
}

/// How soon to check again while the record is not confirmed.
const DHT_RETRY: std::time::Duration = std::time::Duration::from_secs(60);
/// How soon to check again once it is.
const DHT_RECHECK: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Misses in a row after which the DHT node is rebuilt: a node whose
/// bootstrap went stale never recovers by itself, and a publish that failed
/// waits an hour before it tries again.
const DHT_REBUILD_AFTER: u32 = 3;

/// What the record should hold now, or the verdict when there is nothing to
/// look for: `None` (checking) while the endpoint has no relay yet, so a
/// slow start is not reported as nothing to publish.
fn expected_record(
    now: &[TransportAddr],
    with_ips: bool,
) -> std::result::Result<Vec<TransportAddr>, Option<DhtCheck>> {
    let expected = publishable(now, with_ips);
    if !expected.is_empty() {
        Ok(expected)
    } else if now.iter().any(TransportAddr::is_relay) {
        Err(Some(DhtCheck::NothingToPublish))
    } else {
        Err(None)
    }
}

/// Read this endpoint's record back from the DHT: publishing is
/// fire-and-forget, so this is what says it worked. A minute after joining,
/// then each minute until it is found and every 15 minutes after; the node
/// is rebuilt after a few misses in a row. Ends with the endpoint.
async fn check_dht_record(
    mut lookup: DhtAddressLookup,
    endpoint: Endpoint,
    mode: DhtPublish,
    state: DhtCheckState,
) {
    use futures_lite::StreamExt as _;
    let with_ips = mode == DhtPublish::Addresses;
    let mut wait = DHT_RETRY;
    let mut misses = 0;
    loop {
        tokio::time::sleep(wait).await;
        if endpoint.is_closed() {
            return;
        }
        let now: Vec<TransportAddr> = endpoint.addr().addrs.into_iter().collect();
        let check = match expected_record(&now, with_ips) {
            Err(verdict) => verdict,
            Ok(expected) => {
                let read = async {
                    match lookup.resolve(endpoint.id()) {
                        Some(stream) => stream.collect::<Vec<_>>().await,
                        None => Vec::new(),
                    }
                };
                let items = tokio::time::timeout(std::time::Duration::from_secs(30), read)
                    .await
                    .unwrap_or_default();
                let read_back: Vec<TransportAddr> = items
                    .into_iter()
                    .flatten()
                    .flat_map(|item| item.into_endpoint_addr().addrs)
                    .filter(|addr| expected.contains(addr))
                    .collect();
                Some(if read_back.is_empty() {
                    DhtCheck::NotFound
                } else {
                    DhtCheck::Found {
                        ips: read_back.iter().any(TransportAddr::is_ip),
                    }
                })
            }
        };
        tracing::debug!(?check, "WAN: DHT record checked");
        *state.lock().unwrap_or_else(|e| e.into_inner()) = check;
        if matches!(check, Some(DhtCheck::Found { .. })) {
            misses = 0;
            wait = DHT_RECHECK;
            continue;
        }
        wait = DHT_RETRY;
        if check != Some(DhtCheck::NotFound) {
            continue;
        }
        misses += 1;
        if misses >= DHT_REBUILD_AFTER {
            misses = 0;
            if let (Some(fresh), Ok(services)) =
                (join_dht(&endpoint, mode).await, endpoint.address_lookup())
            {
                tracing::info!("WAN: the DHT record was not read back; rejoining the DHT");
                services.clear();
                services.add(fresh.clone());
                lookup = fresh;
            }
        }
    }
}

/// `wan_status.json`'s DHT fields: what the record holds as read back
/// ("relay", "addresses"), or the setting while it is not ("off", or the
/// `dht_publish` value while checking); "empty" when nothing may be
/// published and "unjoined" when the DHT has not started. Then whether it was
/// read back (absent until known).
fn dht_fields(mode: DhtPublish, check: Option<DhtCheck>) -> (Option<String>, Option<bool>) {
    let text = |s: &str| Some(s.to_string());
    if mode == DhtPublish::Off {
        return (text("off"), None);
    }
    match check {
        Some(DhtCheck::NotJoined) => (text("unjoined"), None),
        Some(DhtCheck::NothingToPublish) => (text("empty"), None),
        Some(DhtCheck::Found { ips: true }) => (text("addresses"), Some(true)),
        Some(DhtCheck::Found { ips: false }) => (text("relay"), Some(true)),
        Some(DhtCheck::NotFound) => (text(mode.as_str()), Some(false)),
        None => (text(mode.as_str()), None),
    }
}

/// Write the endpoint body to `<state_dir>/endpoint.json` if it differs from
/// `last`; returns whether it wrote. The desktop app's pairing code reads it.
pub fn record_endpoint_in(
    state_dir: &Path,
    addr: &EndpointAddr,
    id: EndpointId,
    dht: bool,
    last: &mut Option<serde_json::Value>,
) -> bool {
    let body = endpoint_packet(addr, id, dht).body;
    if last.as_ref() == Some(&body) {
        return false;
    }
    match pocketdesk_core::local::identity::write_endpoint(state_dir, &body) {
        Ok(()) => {
            *last = Some(body);
            true
        }
        Err(e) => {
            tracing::debug!("could not record the WAN endpoint: {e}");
            false
        }
    }
}

/// n0's public relays, or your own (`relay_urls`, N2).
pub fn relay_mode(relay_urls: &[String]) -> Result<RelayMode> {
    if relay_urls.is_empty() {
        return Ok(RelayMode::Default);
    }
    let urls = relay_urls
        .iter()
        .map(|url| {
            if let Some(problem) = pocketdesk_core::local::config::relay_url_problem(url) {
                anyhow::bail!("relay_urls: {problem}");
            }
            url.trim()
                .parse::<iroh::RelayUrl>()
                .with_context(|| format!("relay_urls: not a URL: {url}"))
        })
        .collect::<Result<Vec<_>>>()?;
    tracing::info!("WAN relays: your own ({})", relay_urls.join(", "));
    Ok(RelayMode::Custom(urls.into_iter().collect()))
}

/// What the Diagnostics tab shows about reaching this computer from outside
/// (N1), from iroh's own network report: yes/no facts, no address.
pub fn wan_status_from(
    report: &iroh::unstable_net_report::NetReport,
    unix_secs: u64,
) -> pocketdesk_core::local::diagnostics::WanStatus {
    pocketdesk_core::local::diagnostics::WanStatus {
        unix_secs,
        udp: report.has_udp(),
        // The NAT verdict is IPv4's: an IPv6 address that rotates is not a
        // NAT, and IPv6 has its own line.
        nat_varies: report.mapping_varies_by_dest_ipv4,
        public_v6: report.global_v6.is_some(),
        captive_portal: report.captive_portal,
        dht: None,
        dht_found: None,
    }
}

/// Keep `endpoint.json` current: the relay URL only appears once iroh has
/// reached its home relay, and direct addresses change with the network.
/// `wan_status.json` follows iroh's network report the same way, with the
/// DHT check's answer.
fn spawn_endpoint_recorder(endpoint: Endpoint, dht_mode: DhtPublish, dht_check: DhtCheckState) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    tokio::spawn(async move {
        use iroh::Watcher as _;
        let mut last = None;
        let mut last_status = None;
        loop {
            let dht = on_dht(&endpoint);
            record_endpoint_in(&dir, &endpoint.addr(), endpoint.id(), dht, &mut last);
            if let Some(report) = endpoint.net_report().get() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let mut status = wan_status_from(&report, now);
                let check = *dht_check.lock().unwrap_or_else(|e| e.into_inner());
                (status.dht, status.dht_found) = dht_fields(dht_mode, check);
                // Rewritten when a fact changes, not every tick.
                let facts = (
                    status.udp,
                    status.nat_varies,
                    status.public_v6,
                    status.captive_portal,
                    status.dht.clone(),
                    status.dht_found,
                );
                if last_status.as_ref() != Some(&facts) {
                    match pocketdesk_core::local::diagnostics::write_wan_status(&dir, &status) {
                        Ok(()) => last_status = Some(facts),
                        Err(e) => tracing::debug!("could not record the WAN status: {e}"),
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

/// Push the current WAN identity to one freshly-registered client.
pub async fn push_endpoint_to(sender: &Arc<dyn DeviceSender>, endpoint: &Endpoint) {
    let _ = sender.send_packet(&current_packet(endpoint)).await;
}

/// The settings the WAN endpoint takes from config.toml.
#[derive(Debug, Clone)]
pub struct WanSettings {
    pub pairing_required: bool,
    pub allow_hevc: bool,
    pub capture_backend: pocketdesk_core::streaming::CaptureBackend,
    pub mic_relay_device: Option<String>,
    /// Your own relays (N2); empty uses n0's.
    pub relay_urls: Vec<String>,
    /// What the Mainline DHT record holds (N3).
    pub dht_publish: DhtPublish,
}

/// Bind the WAN endpoint and spawn its accept loop.
///
/// Each accepted connection runs one v1 streaming session — identical to the
/// quinn LAN arm in `service.rs`, just over iroh/noq and reachable from
/// outside the LAN. With `pairing_required` the session is gated on the
/// deviceId the client announces in-band, exactly like the LAN arm.
/// Errors are logged per-connection; the loop lives until the endpoint is
/// closed.
pub async fn spawn_wan_endpoint(
    config: StreamingConfig,
    input_tx: broadcast::Sender<InputPacket>,
    cert_manager: Arc<CertManager>,
    settings: WanSettings,
) -> Result<Endpoint> {
    let WanSettings {
        pairing_required,
        allow_hevc,
        capture_backend,
        mic_relay_device,
        relay_urls,
        dht_publish,
    } = settings;
    // What an earlier run recorded about its internet path is not this
    // run's: dropped before anything can fail, so a run whose endpoint never
    // binds shows nothing measured rather than the last network's facts.
    if let Ok(dir) = crate::state::state_dir() {
        for stale in [
            pocketdesk_core::local::paths::WAN_STATUS_FILE,
            pocketdesk_core::local::paths::ENDPOINT_FILE,
        ] {
            let _ = std::fs::remove_file(dir.join(stale));
        }
    }
    let secret_key =
        load_or_create_secret_key(&key_path()?).context("failed to load iroh identity key")?;

    // Mirror the LAN stream budget: the pipeline opens one uni stream per
    // video frame. Relays (n0 defaults) are for hole punching only — dialing
    // rides the EndpointAddr announced over the control channel.
    let transport = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(1024u32.into())
        .max_concurrent_bidi_streams(128u32.into())
        .build();
    let endpoint = Endpoint::builder(Minimal)
        .alpns(vec![StreamTransportConfig::default().alpn])
        .relay_mode(relay_mode(&relay_urls)?)
        .secret_key(secret_key)
        .transport_config(transport)
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], 0)))
        .context("iroh bind_addr rejected wildcard socket addr")?
        .bind()
        .await
        .context("iroh endpoint bind failed")?;

    tracing::info!(id = %endpoint.id(), "WAN (iroh) endpoint ready");
    let _ = DHT_MODE.set(dht_publish);
    let dht_check = DhtCheckState::default();
    spawn_dht(endpoint.clone(), dht_publish, Arc::clone(&dht_check));
    spawn_endpoint_recorder(endpoint.clone(), dht_publish, dht_check);

    let accept = endpoint.clone();
    tokio::spawn(async move {
        loop {
            let Some(incoming) = accept.accept().await else {
                tracing::info!("WAN endpoint closed — accept loop exiting");
                break;
            };
            let config = config.clone();
            let input_tx = input_tx.clone();
            let cert_manager = Arc::clone(&cert_manager);
            let mic_relay_device = mic_relay_device.clone();
            tokio::spawn(async move {
                let conn = match incoming.accept() {
                    Ok(accepting) => match accepting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            tracing::warn!("WAN handshake failed: {e}");
                            return;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("WAN connection rejected: {e}");
                        return;
                    }
                };
                let mut streaming_server =
                    StreamingServer::new(config, StreamTransportConfig::default(), cert_manager);
                streaming_server.set_input_channel(input_tx);
                // R4 E2: WAN sessions get the same phone-mic relay as LAN.
                let (mic_tx, mic_rx) = tokio::sync::mpsc::channel(64);
                tokio::spawn(crate::mic_relay::run_mic_relay(mic_rx, mic_relay_device));
                streaming_server.set_mic_channel(mic_tx);
                streaming_server.set_session_telemetry(true);
                streaming_server.set_hevc_allowed(allow_hevc);
                streaming_server.set_capture_backend(capture_backend);
                // The gate judges the deviceId the client announces in-band
                // against the iroh key the connection proved (device keys). It
                // runs with pairing off too, so a paired device's own
                // permissions still apply.
                streaming_server.set_session_gate(move |claim| {
                    Box::pin(crate::plugins::pair::session_gate(claim, pairing_required))
                });
                if let Err(e) = streaming_server
                    .run_on_connection(IrohConnection::shared(conn))
                    .await
                {
                    tracing::error!("WAN streaming session error: {e}");
                }
            });
        }
    });

    Ok(endpoint)
}

#[cfg(test)]
mod relay_mode_tests {
    use super::relay_mode;
    use iroh::RelayMode;

    #[test]
    fn the_relays_are_n0s_unless_you_name_your_own() {
        assert!(matches!(relay_mode(&[]).unwrap(), RelayMode::Default));
        assert!(matches!(
            relay_mode(&["http://192.0.2.10:3340".to_string()]).unwrap(),
            RelayMode::Custom(_)
        ));
        assert!(relay_mode(&["not a url".to_string()]).is_err());
        // These parse as URLs, with the host taken for a scheme.
        assert!(relay_mode(&["relay.example.org:3340".to_string()]).is_err());
        assert!(relay_mode(&["localhost:3340".to_string()]).is_err());
    }
}

#[cfg(test)]
mod dht_tests {
    use super::{endpoint_packet, publishable};
    use iroh::{EndpointAddr, SecretKey, TransportAddr};

    fn ip(a: &str) -> TransportAddr {
        TransportAddr::Ip(a.parse().unwrap())
    }

    fn relay(url: &str) -> TransportAddr {
        TransportAddr::Relay(url.parse().unwrap())
    }

    #[test]
    fn only_public_addresses_and_reachable_relays_are_published() {
        let public = [
            ip("1.1.1.1:4501"),
            ip("[2606:4700::1111]:4501"),
            relay("https://use1-1.relay.n0.iroh-canary.iroh.link/"),
            relay("https://relay.example.org/"),
            relay("http://8.8.8.8:3340/"),
        ];
        let private = [
            ip("192.168.0.10:4501"),
            ip("10.0.0.2:4501"),
            ip("172.16.0.9:4501"),
            ip("127.0.0.1:4501"),
            ip("169.254.3.3:4501"),
            ip("100.101.102.103:4501"), // carrier NAT, and Tailscale's IPv4
            ip("0.0.0.0:4501"),
            ip("203.0.113.7:4501"), // documentation
            ip("[::1]:4501"),
            ip("[fe80::1]:4501"),
            ip("[fd7a:115c:a1e0::1]:4501"), // unique-local, and Tailscale's IPv6
            ip("[2001:db8::1]:4501"),       // documentation
            ip("[ff02::1]:4501"),
            relay("http://192.168.0.10:3340/"),
            relay("http://[fd7a:115c:a1e0::1]:3340/"),
            relay("http://localhost:3340/"),
            relay("http://nas:3340/"),
            relay("http://pi.lan:3340/"),
            relay("http://relay.home.arpa:3340/"),
            relay("http://box.internal:3340/"),
            relay("http://box.localdomain:3340/"),
            relay("http://printer.home:3340/"),
            relay("http://desk.local:3340/"),
            ip("[3fff::1]:4501"), // documentation (RFC 9637)
        ];
        let all: Vec<_> = public.iter().chain(private.iter()).cloned().collect();
        assert_eq!(publishable(&all, true), public.to_vec());
        assert_eq!(
            publishable(&all, false),
            public
                .iter()
                .filter(|a| a.is_relay())
                .cloned()
                .collect::<Vec<_>>(),
            "relay mode publishes no IP"
        );
    }

    /// The real publish path on a local Mainline DHT: what `dht_lookup`
    /// lets through `publishable` is what a stranger reads back. A LAN or
    /// tailnet address handed to it never reaches the DHT.
    #[tokio::test]
    async fn only_publishable_addresses_reach_the_dht() {
        use super::dht_lookup;
        use futures_lite::StreamExt as _;
        use iroh::address_lookup::{AddressLookup as _, EndpointData};
        use pocketdesk_core::local::config::DhtPublish;
        use pocketdesk_core::streaming::dht::{DhtAddressLookup, dht_builder};
        use std::collections::BTreeSet;

        let testnet = n0_mainline::Testnet::new(5).await.expect("testnet");
        let reader = DhtAddressLookup::builder()
            .dht_builder(dht_builder(&testnet.bootstrap))
            .no_publish()
            .build()
            .expect("reader");
        let handed = [
            relay("https://relay.example.org/"),
            relay("http://nas:3340/"),
            ip("1.1.1.1:4501"),
            ip("192.168.0.10:4501"),
            ip("[fd7a:115c:a1e0::1]:4501"),
        ];
        for (mode, want) in [
            (DhtPublish::Relay, vec![handed[0].clone()]),
            (
                DhtPublish::Addresses,
                vec![handed[0].clone(), handed[2].clone()],
            ),
        ] {
            let key = SecretKey::generate();
            let lookup = dht_lookup(mode, &key, dht_builder(&testnet.bootstrap)).expect("lookup");
            lookup.publish(&EndpointData::from_iter(handed.iter().cloned()));
            let want: BTreeSet<_> = want.into_iter().collect();
            let read = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    let items: Vec<_> = reader
                        .resolve(key.public())
                        .expect("stream")
                        .collect()
                        .await;
                    if let Some(item) = items.into_iter().flatten().next() {
                        break item.into_endpoint_addr().addrs;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            })
            .await
            .expect("the record reached the DHT");
            assert_eq!(read, want, "{mode:?}");
        }
    }

    #[test]
    fn a_record_is_looked_for_only_once_there_is_something_to_publish() {
        use super::{DhtCheck, expected_record};
        let n0 = relay("https://aps1-1.relay.n0.iroh.link/");
        let lan_relay = relay("http://192.168.0.10:3340/");
        let lan = ip("192.168.0.10:4501");
        assert_eq!(
            expected_record(std::slice::from_ref(&lan), false),
            Err(None),
            "no relay yet: still checking, not nothing to publish"
        );
        assert_eq!(
            expected_record(&[lan.clone(), lan_relay], false),
            Err(Some(DhtCheck::NothingToPublish))
        );
        assert_eq!(expected_record(&[lan, n0.clone()], false), Ok(vec![n0]));
    }

    #[test]
    fn the_wan_status_says_what_the_record_holds_and_if_it_was_read_back() {
        use super::{DhtCheck, dht_fields};
        use pocketdesk_core::local::config::DhtPublish;
        let s = |v: &str| Some(v.to_string());
        assert_eq!(dht_fields(DhtPublish::Relay, None), (s("relay"), None));
        assert_eq!(
            dht_fields(DhtPublish::Addresses, Some(DhtCheck::Found { ips: true })),
            (s("addresses"), Some(true))
        );
        assert_eq!(
            dht_fields(DhtPublish::Addresses, Some(DhtCheck::Found { ips: false })),
            (s("relay"), Some(true)),
            "public addresses are claimed only when one was read back"
        );
        assert_eq!(
            dht_fields(DhtPublish::Relay, Some(DhtCheck::NotJoined)),
            (s("unjoined"), None)
        );
        assert_eq!(
            dht_fields(DhtPublish::Off, Some(DhtCheck::NotJoined)),
            (s("off"), None)
        );
        assert_eq!(
            dht_fields(DhtPublish::Relay, Some(DhtCheck::NotFound)),
            (s("relay"), Some(false))
        );
        assert_eq!(
            dht_fields(DhtPublish::Relay, Some(DhtCheck::NothingToPublish)),
            (s("empty"), None)
        );
    }

    #[test]
    fn the_endpoint_packet_says_whether_the_desktop_is_on_the_dht() {
        let id = SecretKey::from_bytes(&[3u8; 32]).public();
        let addr = EndpointAddr::new(id);
        assert_eq!(endpoint_packet(&addr, id, true).body["dht"], true);
        assert_eq!(endpoint_packet(&addr, id, false).body["dht"], false);
    }
}

#[cfg(test)]
mod wan_status_tests {
    #[test]
    fn the_wan_status_takes_ipv4_for_the_nat_and_no_address() {
        let mut report = iroh::unstable_net_report::NetReport::default();
        report.udp_v4 = true;
        report.mapping_varies_by_dest_ipv4 = Some(false);
        report.mapping_varies_by_dest_ipv6 = Some(true);
        report.global_v6 = Some("[2001:db8::5]:4000".parse().unwrap());
        let status = super::wan_status_from(&report, 7);
        assert!(status.udp);
        assert_eq!(
            status.nat_varies,
            Some(false),
            "a rotating IPv6 address is not a NAT"
        );
        assert!(status.public_v6);
        let json = serde_json::to_string(&status).unwrap();
        assert!(!json.contains("2001:db8"), "no address is written: {json}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::TransportAddr;

    fn temp_key_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pocketdesk-iroh-test-{}-{label}.key",
            std::process::id()
        ))
    }

    #[test]
    fn secret_key_persists_and_is_private() {
        let path = temp_key_path("persist");
        let _ = std::fs::remove_file(&path);

        let first = load_or_create_secret_key(&path).expect("create");
        let second = load_or_create_secret_key(&path).expect("load");
        assert_eq!(
            first.to_bytes(),
            second.to_bytes(),
            "identity must be stable across restarts"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secret key must be owner-only");
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_key_file_is_a_hard_error() {
        let path = temp_key_path("corrupt");
        std::fs::write(&path, b"not-a-key").unwrap();
        let err = load_or_create_secret_key(&path).unwrap_err();
        assert!(err.to_string().contains("32-byte"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn endpoint_packet_lists_identity_and_addresses() {
        let id: EndpointId = SecretKey::from_bytes(&[7u8; 32]).public();
        let addr = EndpointAddr::from_parts(
            id,
            [TransportAddr::Ip(
                "203.0.113.7:4501".parse::<SocketAddr>().unwrap(),
            )],
        );
        let packet = endpoint_packet(&addr, id, false);
        assert_eq!(packet.packet_type, ENDPOINT_PACKET);
        let body: serde_json::Value =
            serde_json::from_str(&packet.body.to_string()).expect("body json");
        assert_eq!(body["endpointId"], id.to_string());
        assert_eq!(body["directAddrs"][0], "203.0.113.7:4501");
        assert!(body["relayUrls"].as_array().unwrap().is_empty());
    }

    /// The desktop app's pairing code carries `endpoint.json`; it must be
    /// exactly the body phones already receive, and rewritten only on change.
    #[test]
    fn the_recorded_endpoint_is_the_packet_body() {
        let dir = std::env::temp_dir().join(format!("ll-ep-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id: EndpointId = SecretKey::from_bytes(&[9u8; 32]).public();
        let addr = EndpointAddr::from_parts(
            id,
            [TransportAddr::Ip(
                "198.51.100.4:7".parse::<SocketAddr>().unwrap(),
            )],
        );
        let mut last = None;
        assert!(record_endpoint_in(&dir, &addr, id, true, &mut last));
        assert_eq!(
            pocketdesk_core::local::identity::read_endpoint(&dir),
            Some(endpoint_packet(&addr, id, true).body)
        );
        assert!(
            !record_endpoint_in(&dir, &addr, id, true, &mut last),
            "an unchanged address is not rewritten"
        );
    }
}
