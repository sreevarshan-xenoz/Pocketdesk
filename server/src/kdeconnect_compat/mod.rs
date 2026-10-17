//! Working with KDE Connect devices (3a–3c), a bridge from the protocol's
//! behaviour, never KDE's code. Design:
//! `docs/superpowers/specs/2026-10-17-kdeconnect-bridge-design.md`.
//!
//! 3a: a separate identity, this computer's identity broadcast while the
//! desktop app looks for devices (and once a network change), and the v8
//! handshake on a connection a device opens to the control port (its first
//! byte `{`). Unpaired, a device may only pair (3b); the rest is ignored.

pub mod discovery;
pub mod handshake;
pub mod identity;
pub mod link;
pub mod pairing;

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{self, KdeAsk};
use pocketdesk_core::local::paths::KDECONNECT_OUT_DIR;
use tokio::net::TcpStream;

/// `kdeconnect_compat`, on by default (the owner's decision).
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Set from the config at start.
pub fn configure(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether KDE Connect devices are worked with.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// KDE Connect's own cap on links from unpaired devices.
pub const MAX_UNPAIRED: usize = 42;

/// Unpaired links from one address at most: a few hosts must not take
/// every place (3a review).
pub const MAX_UNPAIRED_PER_ADDRESS: usize = 4;

/// Places for links: so many in all, so many per address.
pub struct Slots {
    total: usize,
    per_address: usize,
    taken: std::sync::Mutex<(usize, BTreeMap<IpAddr, usize>)>,
}

impl Slots {
    pub const fn new(total: usize, per_address: usize) -> Self {
        Self {
            total,
            per_address,
            taken: std::sync::Mutex::new((0, BTreeMap::new())),
        }
    }

    /// A place for a link from `address`, if one is free.
    pub fn take(&'static self, address: IpAddr) -> Option<Slot> {
        let mut taken = self.taken.lock().unwrap_or_else(|e| e.into_inner());
        let (all, by_address) = &mut *taken;
        let here = by_address.get(&address).copied().unwrap_or(0);
        if *all >= self.total || here >= self.per_address {
            return None;
        }
        *all += 1;
        by_address.insert(address, here + 1);
        Some(Slot {
            slots: self,
            address,
        })
    }
}

/// One place, given back when dropped.
pub struct Slot {
    slots: &'static Slots,
    address: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut taken = self.slots.taken.lock().unwrap_or_else(|e| e.into_inner());
        let (all, by_address) = &mut *taken;
        *all = all.saturating_sub(1);
        if let Some(here) = by_address.get_mut(&self.address) {
            *here -= 1;
            if *here == 0 {
                by_address.remove(&self.address);
            }
        }
    }
}

/// The server's places for unpaired links, taken once the handshake has
/// shown the device is not paired (3b review: a paired device is never
/// kept out by them).
static UNPAIRED: Slots = Slots::new(MAX_UNPAIRED, MAX_UNPAIRED_PER_ADDRESS);

/// Connections in the handshake at once, more than the unpaired places:
/// each is over within the handshake's deadlines.
static HANDSHAKING: Slots = Slots::new(64, 8);

/// The compat identity in the state folder, read once.
pub fn load_identity() -> anyhow::Result<identity::CompatIdentity> {
    static LOADED: std::sync::OnceLock<identity::CompatIdentity> = std::sync::OnceLock::new();
    if let Some(identity) = LOADED.get() {
        return Ok(identity.clone());
    }
    let dir = crate::state::state_dir()?.join("certs").join(identity::DIR);
    let identity = identity::CompatIdentity::load_or_create(&dir)?;
    Ok(LOADED.get_or_init(|| identity).clone())
}

/// A connection to the control port that opened with `{`: a KDE Connect
/// device's connection packet. The handshake (a paired device held to its
/// pin), then the link: pairing and ping (3b), until it closes.
pub async fn serve(stream: TcpStream) {
    let Ok(peer) = stream.peer_addr() else {
        return;
    };
    let Some(handshaking) = HANDSHAKING.take(peer.ip()) else {
        tracing::warn!(
            %peer,
            "A KDE Connect connection refused: too many in the handshake already"
        );
        return;
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let port = stream.local_addr().map(|a| a.port()).unwrap_or(0);
    let identity = match load_identity() {
        Ok(identity) => identity,
        Err(e) => {
            tracing::warn!("No KDE Connect identity: {e:#}");
            return;
        }
    };
    let ours =
        handshake::identity_packet(&identity.id, &crate::kde::host_identity().device_name, port);
    let link = match handshake::accept(stream, &identity, &ours, |id| link::pin_of(&dir, id)).await
    {
        Ok(link) => link,
        Err(why) => {
            tracing::warn!(%peer, "A KDE Connect connection refused: {why}");
            return;
        }
    };
    drop(handshaking);
    discovery::connected();
    // Held to its pin in the handshake: a paired device's link is not one
    // of the unpaired ones.
    let paired = link::pin_of(&dir, &link.hello.device_id) == Some(link.cert_hash);
    let slot = if paired {
        None
    } else {
        let Some(slot) = UNPAIRED.take(peer.ip()) else {
            tracing::warn!(
                %peer,
                "A KDE Connect connection refused: too many unpaired already"
            );
            return;
        };
        Some(slot)
    };
    tracing::info!(
        device_id = %link.hello.device_id,
        name = %link.hello.device_name,
        paired,
        "A KDE Connect device connected"
    );
    let noted = kdeconnect::record(
        &dir,
        SeenDevice {
            device_id: link.hello.device_id.clone(),
            name: link.hello.device_name.clone(),
            seen_unix: pocketdesk_core::local::pin::now_unix(),
            paired,
        },
    );
    if let Err(e) = noted {
        tracing::debug!("Could not note the KDE Connect device: {e}");
    }
    link::run(link, &identity.cert_der, slot).await;
}

/// The desktop app's requests about KDE Connect devices (3b), for the
/// server's life: each goes to the device's link; an unpairing of a device
/// with none, and a change of what one may do (3c), are done here.
pub async fn run_requests() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        crate::inbox::wait(KDECONNECT_OUT_DIR, std::time::Duration::from_secs(1)).await;
        for request in kdeconnect_devices::take_all(&dir, pocketdesk_core::local::pin::now_unix()) {
            let (device_id, event) = match request.ask {
                // The owner's change of what a paired device may do (3c):
                // the links read it as each packet comes.
                KdeAsk::Permissions {
                    device_id,
                    permissions,
                } => {
                    match kdeconnect_devices::set_permissions(&dir, &device_id, permissions) {
                        Ok(true) => tracing::info!(
                            device_id,
                            ?permissions,
                            "A KDE Connect device's permissions changed"
                        ),
                        Ok(false) => tracing::info!(
                            device_id,
                            "Permissions for a KDE Connect device that is not paired"
                        ),
                        Err(e) => tracing::warn!(
                            "Could not change a KDE Connect device's permissions: {e}"
                        ),
                    }
                    continue;
                }
                KdeAsk::Pair { device_id } => (device_id, link::Event::PairFromHere),
                KdeAsk::Allow { device_id, code } => {
                    (device_id, link::Event::Answer { yes: true, code })
                }
                KdeAsk::Deny { device_id, code } => {
                    (device_id, link::Event::Answer { yes: false, code })
                }
                KdeAsk::Unpair { device_id } => (device_id, link::Event::Unpair),
                KdeAsk::Ping { device_id } => (device_id, link::Event::Ping),
            };
            if link::tell(&device_id, event.clone()) {
                continue;
            }
            if event == link::Event::Unpair {
                match kdeconnect_devices::remove(&dir, &device_id) {
                    Ok(_) => {
                        tracing::info!(device_id, "Unpaired a KDE Connect device (not connected)")
                    }
                    Err(e) => tracing::warn!("Could not unpair a KDE Connect device: {e}"),
                }
            } else {
                tracing::info!(
                    device_id,
                    ?event,
                    "A KDE Connect device is not connected now"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_stop_at_the_cap_and_at_so_many_from_one_address() {
        static SLOTS: Slots = Slots::new(6, 4);
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "192.0.2.2".parse().unwrap();
        let from_a: Vec<_> = std::iter::from_fn(|| SLOTS.take(a)).take(10).collect();
        assert_eq!(from_a.len(), 4, "four from one address");
        let from_b: Vec<_> = std::iter::from_fn(|| SLOTS.take(b)).take(10).collect();
        assert_eq!(from_b.len(), 2, "six in all");
        drop(from_a);
        assert!(SLOTS.take(a).is_some(), "given back");
    }
}
