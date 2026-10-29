//! This computer's identity, broadcast on UDP 1716 for KDE Connect devices
//! to find it (3a): when the desktop app asks (`kdeconnect_find`), and again
//! every 10 s while it looks, until a device connects; and once after the
//! network changes, when this computer is visible on the network
//! (`lan_discovery`). A device that hears it dials the control port. It
//! dials again at every broadcast, replacing its link, so the broadcast is
//! not repeated once one has connected.

use std::time::Duration;

use pocketdesk_core::local::kdeconnect;

/// KDE Connect's discovery port.
pub const UDP_PORT: u16 = 1716;

/// How often the loop looks for a request.
const TICK: Duration = Duration::from_secs(2);

/// How often, in ticks, the network is looked at for a change: reading
/// the interfaces costs more than reading the request (R6's idle cost).
const NETWORK_EVERY: u32 = 5;

/// When a KDE Connect device last completed the handshake, unix seconds:
/// a Find stops broadcasting once one has.
static LAST_CONNECTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A device completed the handshake now.
pub fn connected() {
    LAST_CONNECTED.store(
        pocketdesk_core::local::pin::now_unix(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// While asked to look, how often (in ticks) the broadcast goes again.
const FIND_EVERY: u32 = 5;

/// Broadcast as the module says, for the server's life. `announce_changes`
/// is `lan_discovery`: a network change is told only on a network this
/// computer is visible on.
pub async fn run(control_port: u16, announce_changes: bool) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut addresses = local_addresses().await;
    let mut said = false;
    let mut ticks = 0u32;
    let mut last_ask = None;
    // Since when the current Find looks, and whether a change is still to
    // be told (a failed broadcast is tried again).
    let mut looking_since: Option<u64> = None;
    let mut to_tell = false;
    loop {
        tokio::time::sleep(TICK).await;
        if !super::enabled() {
            continue;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        let asked = kdeconnect::find_until(&dir, now);
        let new_ask = asked.is_some() && asked != last_ask;
        last_ask = asked;
        if new_ask {
            looking_since = Some(now);
        }
        if asked.is_none() {
            looking_since = None;
        }
        ticks = ticks.wrapping_add(1);
        if announce_changes && ticks.is_multiple_of(NETWORK_EVERY) {
            let current = local_addresses().await;
            if current != addresses {
                to_tell = true;
            }
            addresses = current;
        }
        let looking = looking_since
            .is_some_and(|since| LAST_CONNECTED.load(std::sync::atomic::Ordering::Relaxed) < since);
        let again = looking && ticks.is_multiple_of(FIND_EVERY);
        if !(new_ask || again || to_tell) {
            continue;
        }
        match broadcast(control_port).await {
            Ok(()) => to_tell = false,
            Err(e) => {
                if !std::mem::replace(&mut said, true) {
                    tracing::info!("KDE Connect devices cannot be told this computer is here: {e}");
                }
            }
        }
    }
}

/// This computer's IPv4 addresses, to notice a network change: the
/// broadcast is IPv4, and IPv6 privacy addresses change by themselves. Read
/// off the runtime, as it asks the system for its interfaces.
async fn local_addresses() -> Vec<std::net::Ipv4Addr> {
    tokio::task::spawn_blocking(|| {
        let mut found: Vec<std::net::Ipv4Addr> = pocketdesk_core::local::network::local_ips()
            .iter()
            .filter_map(|ip| ip.parse().ok())
            .collect();
        found.sort();
        found
    })
    .await
    .unwrap_or_default()
}

/// One broadcast of this computer's identity.
async fn broadcast(control_port: u16) -> anyhow::Result<()> {
    let identity = super::load_identity()?;
    let packet = super::handshake::identity_packet(
        &identity.id,
        &crate::kde::host_identity().device_name,
        control_port,
    );
    let wire = packet.to_wire()?;
    let socket = tokio::net::UdpSocket::bind(("0.0.0.0", 0)).await?;
    socket.set_broadcast(true)?;
    socket.send_to(&wire, ("255.255.255.255", UDP_PORT)).await?;
    Ok(())
}
