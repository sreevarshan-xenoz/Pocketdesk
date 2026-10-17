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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

/// Links from devices that have not paired, at most [`MAX_UNPAIRED`].
static UNPAIRED: AtomicUsize = AtomicUsize::new(0);

/// KDE Connect's own cap on links from unpaired devices.
pub const MAX_UNPAIRED: usize = 42;

/// One of the [`MAX_UNPAIRED`] places, given back when dropped.
struct UnpairedSlot;

impl UnpairedSlot {
    fn take() -> Option<Self> {
        let mut now = UNPAIRED.load(Ordering::Relaxed);
        loop {
            if now >= MAX_UNPAIRED {
                return None;
            }
            match UNPAIRED.compare_exchange(now, now + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(Self),
                Err(seen) => now = seen,
            }
        }
    }
}

impl Drop for UnpairedSlot {
    fn drop(&mut self) {
        UNPAIRED.fetch_sub(1, Ordering::AcqRel);
    }
}

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
    let peer = stream.peer_addr().ok();
    let Some(slot) = UnpairedSlot::take() else {
        tracing::warn!(
            ?peer,
            "A KDE Connect connection refused: {MAX_UNPAIRED} unpaired already"
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
            tracing::warn!(?peer, "A KDE Connect connection refused: {why}");
            return;
        }
    };
    // Held to its pin in the handshake: a paired device's link is not one
    // of the unpaired ones.
    let paired = link::pin_of(&dir, &link.hello.device_id) == Some(link.cert_hash);
    let slot = (!paired).then_some(slot);
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
    link::run(link, &identity.cert_der, paired).await;
    drop(slot);
}

/// The desktop app's requests about KDE Connect devices (3b), for the
/// server's life: each goes to the device's link; an unpairing of a device
/// with none is done here.
pub async fn run_requests() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        crate::inbox::wait(KDECONNECT_OUT_DIR, std::time::Duration::from_secs(1)).await;
        for request in kdeconnect_devices::take_all(&dir, pocketdesk_core::local::pin::now_unix()) {
            let (device_id, event) = match request.ask {
                KdeAsk::Pair { device_id } => (device_id, link::Event::PairFromHere),
                KdeAsk::Allow { device_id } => (device_id, link::Event::Answer(true)),
                KdeAsk::Deny { device_id } => (device_id, link::Event::Answer(false)),
                KdeAsk::Unpair { device_id } => (device_id, link::Event::Unpair),
                KdeAsk::Ping { device_id } => (device_id, link::Event::Ping),
            };
            if link::tell(&device_id, event) {
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
    fn unpaired_links_stop_at_kdes_cap_and_free_their_place() {
        let held: Vec<_> = std::iter::from_fn(UnpairedSlot::take).take(100).collect();
        assert_eq!(held.len(), MAX_UNPAIRED);
        assert!(UnpairedSlot::take().is_none());
        drop(held);
        assert!(UnpairedSlot::take().is_some());
    }
}
