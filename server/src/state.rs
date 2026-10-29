//! Shared server state: state directory paths and active client registry.
//!
//! Centralizes path resolution and client lifecycle management so that
//! `service.rs`, `v2_multiplexer.rs`, and `kde.rs` don't duplicate logic.

use anyhow::{Context, Result};
use pocketdesk_core::protocol::kdeconnect::DeviceSender;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// State directory paths
// ---------------------------------------------------------------------------

/// Base state directory for Pocketdesk server files.
///
/// Resolves to `$XDG_STATE_HOME/pocketdesk` (or equivalent on the current OS).
pub fn state_dir() -> Result<PathBuf> {
    // The shared resolver, which also moves a folder from before the rename.
    pocketdesk_core::local::paths::state_dir().context("unable to determine local state directory")
}

/// For tests: this process's throwaway state and config folders
/// ([`pocketdesk_core::local::paths::use_test_dirs`]); returns the state one.
#[cfg(test)]
pub(crate) fn use_test_dirs() -> &'static std::path::Path {
    pocketdesk_core::local::paths::use_test_dirs("server-tests")
}

/// Where this session keeps the screen-sharing portal's grant (N8), read
/// from the config as it is now: the state folder when the owner chose to
/// keep it, else `None`, and a kept one forgotten, so turning the setting
/// off brings the dialog back.
pub fn screen_grant() -> Option<PathBuf> {
    let dir = state_dir().ok()?;
    if crate::events::current_config().remember_screen_grant {
        Some(dir)
    } else {
        let _ = pocketdesk_core::portal_grant::forget(&dir);
        None
    }
}

/// Path to the server PID file.
pub fn pid_file_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("server.pid"))
}

/// Path to the pairing PIN file.
pub fn pair_pin_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("pairing.pin"))
}

/// Path to the trust store file.
pub fn trust_store_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("trusted_devices.json"))
}

/// Write the current process ID to the given file, creating parent dirs as needed.
pub fn write_pid_file(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, format!("{}\n", std::process::id()))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Active client registry
// ---------------------------------------------------------------------------

/// Active KDE Connect client connections for broadcasting notifications.
pub static ACTIVE_CLIENTS: LazyLock<Mutex<Vec<Arc<dyn DeviceSender>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Register a client for pushed desktop state.
///
/// Every live connection stays registered; pushes go only to each device's
/// newest one (see [`newest_per_device`]), so a device is never pushed to
/// twice. Registering used to evict the device's older connections instead,
/// and the phone opens short query connections alongside its long-lived
/// control one: the first query evicted the control connection, closed a
/// moment later, and left the device with nothing registered — no window
/// titles, no notifications, for the rest of the session. Returns how many
/// other connections the device already had.
pub async fn register_client(sender: Arc<dyn DeviceSender>) -> usize {
    let mut clients = ACTIVE_CLIENTS.lock().await;
    let device_id = sender.device_id().to_string();
    let others = clients
        .iter()
        .filter(|c| c.device_id() == device_id)
        .count();
    if others > 0 {
        tracing::debug!(
            device_id = %device_id,
            others,
            "Device has other live connections; pushes go to the newest"
        );
    }

    clients.push(sender);
    tracing::debug!(
        active_clients = clients.len(),
        "Client registered for broadcasts"
    );

    others
}

/// One push target per device: its most recently registered live connection
/// (a reconnecting phone's new connection wins over a half-open old one; when
/// a short query connection closes, the long-lived one is the target again).
/// `clients` is in registration order.
fn newest_per_device(clients: Vec<Arc<dyn DeviceSender>>) -> Vec<Arc<dyn DeviceSender>> {
    let mut newest: Vec<Arc<dyn DeviceSender>> = Vec::new();
    for client in clients.into_iter().rev() {
        if !newest.iter().any(|c| c.device_id() == client.device_id()) {
            newest.push(client);
        }
    }
    newest.reverse();
    newest
}

/// The connections a phone opened only to carry copies of its notifications
/// (L8 part 2): they take that computer's answers and clears, and whether
/// it is in use (N7), and nothing else, so they are no push target and no
/// sign the phone is linked.
static COPY_LINKS: LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(Default::default);

/// Mark `connection_id` as a phone's copies-only link.
pub fn mark_copy_link(connection_id: &str) {
    COPY_LINKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(connection_id.to_string());
}

fn is_copy_link(sender: &dyn DeviceSender) -> bool {
    COPY_LINKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(sender.connection_id())
}

/// Remove a client by its connection ID.
pub async fn unregister_client(connection_id: &str) {
    COPY_LINKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(connection_id);
    // A nearby computer's ask on it is over (it frees the prompt's slot).
    crate::nearby::connection_closed(connection_id);
    let mut clients = ACTIVE_CLIENTS.lock().await;
    clients.retain(|c| c.connection_id() != connection_id);
    tracing::info!(
        active_clients = clients.len(),
        "Client disconnected, removed from registry"
    );
}

/// Whether pushes are restricted to paired clients (`pairing_required`).
/// Defaults to the safe side until the service sets it from config.
static PAIRING_REQUIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Record `pairing_required` from config, once, at service start.
pub fn set_pairing_required(required: bool) {
    PAIRING_REQUIRED.store(required, std::sync::atomic::Ordering::Relaxed);
}

/// Whether only paired phones may connect (`pairing_required`).
pub fn pairing_required() -> bool {
    PAIRING_REQUIRED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether a connection may be sent desktop state: never one that claims a
/// paired device without its key (phase B); otherwise always with pairing
/// off, and only a paired device's with it on. Over the internet road, only
/// the paired phone with its registered iroh key.
pub fn may_receive_pushes(sender: &dyn DeviceSender) -> bool {
    // A connection over the internet road (T10a) is held to that road's
    // rule, never to the LAN's TLS key (in may_act).
    crate::plugins::pair::may_act(
        sender.device_id(),
        sender.peer_key().as_ref(),
        PAIRING_REQUIRED.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Each device's newest connection among those `eligible` for pushes. The
/// filter comes first: a newer connection that may not receive pushes must
/// not hide the device's real one.
fn push_targets(
    clients: Vec<Arc<dyn DeviceSender>>,
    eligible: impl Fn(&dyn DeviceSender) -> bool,
) -> Vec<Arc<dyn DeviceSender>> {
    newest_per_device(clients.into_iter().filter(|c| eligible(&**c)).collect())
}

/// The clients that may receive pushed desktop state (notifications, window
/// titles, the WAN endpoint), cloned so the lock is not held while sending.
///
/// A client registers the moment its handshake completes, before it pairs;
/// handing it the broadcast list unfiltered would show an unpaired device
/// everything the desktop pushes. Pairing is enforced on what a client
/// *sends* by the packet loop, and on what it *receives* here.
pub async fn clone_clients() -> Vec<Arc<dyn DeviceSender>> {
    let clients = ACTIVE_CLIENTS.lock().await.clone();
    // A copies-only link (L8 part 2) is no push target and no sign of a link.
    push_targets(clients, |c| may_receive_pushes(c) && !is_copy_link(c))
}

/// Where answers and clears to a phone's notifications go (T10c, L1): as
/// [`clone_clients`], copies-only links included, which exist to take them.
pub async fn clone_answer_targets() -> Vec<Arc<dyn DeviceSender>> {
    let clients = ACTIVE_CLIENTS.lock().await.clone();
    push_targets(clients, may_receive_pushes)
}

/// Remove clients whose `connection_id` appears in `dead_ids`.
pub async fn prune_dead_clients(dead_ids: &[String]) {
    let mut clients = ACTIVE_CLIENTS.lock().await;
    clients.retain(|c| !dead_ids.iter().any(|d| d == c.connection_id()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::error::Result;
    use pocketdesk_core::protocol::kdeconnect::NetworkPacket;

    struct Conn(&'static str, &'static str);

    #[async_trait::async_trait]
    impl DeviceSender for Conn {
        fn device_id(&self) -> &str {
            self.0
        }
        fn connection_id(&self) -> &str {
            self.1
        }
        async fn send_packet(&self, _packet: &NetworkPacket) -> Result<()> {
            Ok(())
        }
    }

    fn ids(clients: &[Arc<dyn DeviceSender>]) -> Vec<&str> {
        clients.iter().map(|c| c.connection_id()).collect()
    }

    #[test]
    fn each_device_is_pushed_to_once_on_its_newest_connection() {
        let control: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "control"));
        let query: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "query"));
        let laptop: Arc<dyn DeviceSender> = Arc::new(Conn("laptop", "l1"));
        let all = vec![control.clone(), laptop.clone(), query.clone()];
        assert_eq!(ids(&newest_per_device(all)), ["l1", "query"]);
        // The query connection closes: the control connection is the target
        // again, instead of the phone having none.
        assert_eq!(
            ids(&newest_per_device(vec![control, laptop])),
            ["control", "l1"]
        );
    }

    #[test]
    fn a_connection_that_may_not_receive_pushes_neither_gets_them_nor_hides_the_phone() {
        // A newer connection that claims the phone's id without its key must
        // not become the phone's push target: it would see the pushes, or,
        // filtered out after the choice, leave the real phone with none.
        let phone: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "real"));
        let impostor: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "impostor"));
        let targets = push_targets(vec![phone, impostor], |c| c.connection_id() != "impostor");
        assert_eq!(ids(&targets), ["real"]);
    }

    /// L8 part 2: a phone's copies-only link is no push target, so the
    /// phone's own control link stays the one pushes take, and it goes from
    /// the list when it closes.
    #[tokio::test]
    async fn a_copies_only_link_is_left_out_of_pushes() {
        let control: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "l8b-control"));
        let copies: Arc<dyn DeviceSender> = Arc::new(Conn("phone", "l8b-copies"));
        mark_copy_link("l8b-copies");
        let targets = push_targets(vec![control.clone(), copies.clone()], |c| !is_copy_link(c));
        assert_eq!(ids(&targets), ["l8b-control"]);
        // Answers may take it (the answer targets keep it).
        let answers = push_targets(vec![control, copies], |_| true);
        assert_eq!(ids(&answers), ["l8b-copies"]);
        unregister_client("l8b-copies").await;
        assert!(!is_copy_link(&Conn("phone", "l8b-copies")));
    }
}
