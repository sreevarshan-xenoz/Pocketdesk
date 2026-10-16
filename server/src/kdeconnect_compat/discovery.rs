//! This computer's identity, broadcast on UDP 1716 for KDE Connect devices
//! to find it (3a): while the desktop app looks for them (it asks in
//! `kdeconnect_find`), every 2 s, and once after the network changes. A
//! device that hears it dials the control port.

use std::time::Duration;

use pocketdesk_core::local::kdeconnect;

/// KDE Connect's discovery port.
pub const UDP_PORT: u16 = 1716;

/// How often the loop looks, and broadcasts while asked to.
const TICK: Duration = Duration::from_secs(2);

/// How often, in ticks, the network is looked at for a change: reading
/// the interfaces costs more than reading the request (R6's idle cost).
const NETWORK_EVERY: u32 = 5;

/// Broadcast as the module says, for the server's life.
pub async fn run(control_port: u16) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut addresses = local_addresses();
    let mut said = false;
    let mut ticks = 0u32;
    loop {
        tokio::time::sleep(TICK).await;
        if !super::enabled() {
            continue;
        }
        let now = pocketdesk_core::local::pin::now_unix();
        ticks = ticks.wrapping_add(1);
        let moved = ticks.is_multiple_of(NETWORK_EVERY) && {
            let current = local_addresses();
            let moved = current != addresses;
            addresses = current;
            moved
        };
        if !(moved || kdeconnect::finding(&dir, now)) {
            continue;
        }
        if let Err(e) = broadcast(control_port).await
            && !std::mem::replace(&mut said, true)
        {
            tracing::info!("KDE Connect devices cannot be told this computer is here: {e}");
        }
    }
}

/// This computer's addresses, to notice a network change.
fn local_addresses() -> Vec<String> {
    let mut found = pocketdesk_core::local::network::local_ips();
    found.sort();
    found
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
