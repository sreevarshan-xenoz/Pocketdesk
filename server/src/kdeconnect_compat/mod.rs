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

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
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

/// The compat identity in the state folder.
pub fn load_identity() -> anyhow::Result<identity::CompatIdentity> {
    let dir = crate::state::state_dir()?.join("certs").join(identity::DIR);
    identity::CompatIdentity::load_or_create(&dir)
}

/// A connection to the control port that opened with `{`: a KDE Connect
/// device's connection packet. The handshake, then (3a) the link held,
/// its packets ignored, until it closes.
pub async fn serve(stream: TcpStream) {
    let peer = stream.peer_addr().ok();
    let Some(_slot) = UnpairedSlot::take() else {
        tracing::warn!(
            ?peer,
            "A KDE Connect connection refused: {MAX_UNPAIRED} unpaired already"
        );
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
    let link = match handshake::accept(stream, &identity, &ours, |_| None).await {
        Ok(link) => link,
        Err(why) => {
            tracing::warn!(?peer, "A KDE Connect connection refused: {why}");
            return;
        }
    };
    tracing::info!(
        device_id = %link.hello.device_id,
        name = %link.hello.device_name,
        "A KDE Connect device connected (not paired)"
    );
    if let Ok(dir) = crate::state::state_dir() {
        let noted = kdeconnect::record(
            &dir,
            SeenDevice {
                device_id: link.hello.device_id.clone(),
                name: link.hello.device_name.clone(),
                seen_unix: pocketdesk_core::local::pin::now_unix(),
                paired: false,
            },
        );
        if let Err(e) = noted {
            tracing::debug!("Could not note the KDE Connect device: {e}");
        }
    }
    hold(link).await;
}

/// Keep an unpaired device's link until it closes, taking nothing from it
/// (pairing is 3b).
async fn hold<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut link: handshake::DeviceLink<S>,
) {
    use tokio::io::AsyncReadExt;
    let mut sink = [0u8; 4096];
    loop {
        match link.stream.read(&mut sink).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    tracing::info!(device_id = %link.hello.device_id, "A KDE Connect device went");
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
