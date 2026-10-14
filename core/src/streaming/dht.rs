//! The Mainline DHT that finds the desktop by its WAN id (roadmap N3), for
//! both ends: the desktop publishes there, and the phone reads.
//!
//! n0-mainline resolves its bootstrap host names with blocking DNS inside its
//! constructor, and a name that does not resolve then is never retried, so a
//! node built before the network is up never joins. Both ends therefore
//! resolve the names here, off the async runtime and within a deadline, and
//! hand the DHT addresses.

use std::net::ToSocketAddrs;
use std::time::Duration;

use anyhow::{Context, Result, bail};
pub use iroh_mainline_address_lookup::DhtAddressLookup;
pub use n0_mainline::DhtBuilder;

/// n0-mainline's default bootstrap nodes (the crate does not export them).
const BOOTSTRAP_NODES: [&str; 4] = [
    "router.bittorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "dht.libtorrent.org:25401",
    "relay.pkarr.org:6881",
];

/// The bootstrap nodes' IPv4 addresses (the DHT speaks IPv4 only), resolved
/// on a blocking thread within `limit`. Empty when DNS does not answer.
pub async fn bootstrap_nodes(limit: Duration) -> Vec<String> {
    let resolve = tokio::task::spawn_blocking(|| {
        BOOTSTRAP_NODES
            .iter()
            .filter_map(|host| host.to_socket_addrs().ok())
            .flatten()
            .filter(|addr| addr.is_ipv4())
            .map(|addr| addr.to_string())
            .collect::<Vec<_>>()
    });
    tokio::time::timeout(limit, resolve)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
}

/// A DHT node bootstrapped from `nodes`, on a port the OS picks: never 6881,
/// which a torrent client on the same machine may want.
pub fn dht_builder(nodes: &[String]) -> DhtBuilder {
    let mut builder = DhtBuilder::default();
    builder.bootstrap(nodes).port(0);
    builder
}

/// A lookup on the public Mainline DHT that only reads: the phone finds the
/// desktop's signed record by its id, and never publishes its own.
pub async fn dht_resolver() -> Result<DhtAddressLookup> {
    let nodes = bootstrap_nodes(Duration::from_secs(5)).await;
    if nodes.is_empty() {
        bail!("the DHT's bootstrap nodes did not resolve (no DNS yet?)");
    }
    DhtAddressLookup::builder()
        .dht_builder(dht_builder(&nodes))
        .no_publish()
        .build()
        .context("the Mainline DHT could not start")
}
