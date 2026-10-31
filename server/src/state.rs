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
    #[cfg(test)]
    test_dirs_first();
    // The shared resolver, which also moves a folder from before the rename.
    pocketdesk_core::local::paths::state_dir().context("unable to determine local state directory")
}

/// For tests: this process's throwaway state and config folders
/// ([`pocketdesk_core::local::paths::use_test_dirs`]), held by the calling
/// test alone until the guard drops. The folders are one for the process
/// (the trust store, the pairing PIN, device keys, the peer list), and
/// tests that changed them side by side lost each other's changes; each
/// test that reaches them takes the guard in its own body and keeps it to
/// the end (a helper that makes something for the test returns the guard
/// with it, or asserts the caller's with [`assert_test_turn`]). The turn is
/// the test thread's: taken again on that thread it nests, but work the
/// test moves to another thread (the blocking pool, a `std::thread`, a
/// multi-thread runtime's workers) is covered only while the test still
/// holds it, and must never take it itself. A test that waits more than
/// [`TEST_TURN_WAIT`] for it fails, naming the test that holds it. Derefs
/// to the state folder.
#[cfg(test)]
pub(crate) fn use_test_dirs() -> TestState {
    let dir = pocketdesk_core::local::paths::use_test_dirs("server-tests");
    let me = std::thread::current();
    let deadline = std::time::Instant::now() + TEST_TURN_WAIT;
    let mut turn = TEST_TURN.lock().unwrap_or_else(|e| e.into_inner());
    while let Some((holder, name)) = turn.holder.as_ref() {
        if *holder == me.id() {
            break;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            panic!(
                "waited {}s for the state folder: test {name} still holds it",
                TEST_TURN_WAIT.as_secs()
            );
        }
        turn = TEST_TURN_FREED
            .wait_timeout(turn, left)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    turn.holder = Some((me.id(), me.name().unwrap_or("?").to_string()));
    turn.depth += 1;
    TestState {
        dir,
        _not_send: std::marker::PhantomData,
    }
}

/// For a test helper that reaches the state folder: the test calling it
/// must hold its turn ([`use_test_dirs`]).
#[cfg(test)]
pub(crate) fn assert_test_turn() {
    let me = std::thread::current().id();
    let turn = TEST_TURN.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        turn.holder
            .as_ref()
            .is_some_and(|(holder, _)| *holder == me),
        "take crate::state::use_test_dirs() in the test before this helper"
    );
}

/// How long a test waits for another to let the state folder go: far past
/// any test's run, so only a test that never lets go reaches it.
#[cfg(test)]
const TEST_TURN_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Which test thread holds the shared state folder (its id and name), and
/// how many times.
#[cfg(test)]
struct TestTurn {
    holder: Option<(std::thread::ThreadId, String)>,
    depth: usize,
}

#[cfg(test)]
static TEST_TURN: std::sync::Mutex<TestTurn> = std::sync::Mutex::new(TestTurn {
    holder: None,
    depth: 0,
});

#[cfg(test)]
static TEST_TURN_FREED: std::sync::Condvar = std::sync::Condvar::new();

/// A test's hold on the shared state folder ([`use_test_dirs`]). It stays
/// on the thread that took it, as the turn is that thread's.
#[cfg(test)]
#[must_use = "the state folder is this test's only while the guard lives"]
pub(crate) struct TestState {
    dir: &'static std::path::Path,
    _not_send: std::marker::PhantomData<*const ()>,
}

#[cfg(test)]
impl std::ops::Deref for TestState {
    type Target = std::path::Path;

    fn deref(&self) -> &std::path::Path {
        self.dir
    }
}

#[cfg(test)]
impl Drop for TestState {
    fn drop(&mut self) {
        let mut turn = TEST_TURN.lock().unwrap_or_else(|e| e.into_inner());
        turn.depth -= 1;
        if turn.depth == 0 {
            turn.holder = None;
            TEST_TURN_FREED.notify_all();
        }
    }
}

/// In tests, every path below resolves inside the throwaway folders, never
/// the owner's, even for a test that reaches one before taking its turn.
#[cfg(test)]
fn test_dirs_first() {
    pocketdesk_core::local::paths::use_test_dirs("server-tests");
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
/// Run blocking file work (a state file's save, which on Windows can wait
/// up to about half a second for another program's handle) from async code
/// whose borrows make `spawn_blocking` awkward. On the server's
/// multi-thread runtime the worker hands its other tasks off first
/// (`block_in_place`); on a current-thread runtime (the tests) or outside
/// a runtime it runs inline, where `block_in_place` would panic.
pub fn off_the_workers<R>(work: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

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
    // A computer's privacy grab asked on it ends with it (PC8).
    crate::plugins::privacy::connection_closed(connection_id);
    let mut clients = ACTIVE_CLIENTS.lock().await;
    clients.retain(|c| c.connection_id() != connection_id);
    tracing::info!(
        active_clients = clients.len(),
        "Client disconnected, removed from registry"
    );
}

/// The devices whose connections are to close now (remote revoke): each
/// control connection's loop listens, and ends its own when its device is
/// named, rather than at its next packet or a minute idle.
static CLOSING: LazyLock<tokio::sync::broadcast::Sender<String>> =
    LazyLock::new(|| tokio::sync::broadcast::channel(16).0);

/// Listen for [`close_device`].
pub fn closing() -> tokio::sync::broadcast::Receiver<String> {
    CLOSING.subscribe()
}

/// Close every control connection of `device_id` (remote revoke: it was
/// removed). A connection whose loop missed it (too many at once) still
/// closes at its next packet, as one no longer paired does.
pub fn close_device(device_id: &str) {
    let _ = CLOSING.send(device_id.to_string());
}

/// Wait until [`close_device`] names `device_id` on `closing`; never, once
/// nothing can name it. A receiver that fell behind (too many at once)
/// may have missed its name: it ends then if `gone` says the device is
/// no longer let in (PR #574 review N9).
pub async fn closed(
    device_id: &str,
    closing: &mut tokio::sync::broadcast::Receiver<String>,
    gone: impl Fn() -> bool,
) {
    loop {
        match closing.recv().await {
            Ok(id) if id == device_id => return,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) if gone() => return,
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                std::future::pending::<()>().await
            }
        }
    }
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

#[cfg(test)]
mod turn_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Another thread's take, sent once it holds the turn (and let go).
    fn take_elsewhere() -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _state = use_test_dirs();
            let _ = tx.send(());
        });
        rx
    }

    /// The turn nests on its own thread, holds another thread off until
    /// the outer guard goes, and a helper sees it held.
    #[test]
    fn the_turn_nests_and_holds_other_threads_off() {
        let outer = use_test_dirs();
        let inner = use_test_dirs();
        assert_test_turn();
        drop(inner);
        let other = take_elsewhere();
        assert!(
            other.recv_timeout(Duration::from_millis(200)).is_err(),
            "still held by the outer guard"
        );
        drop(outer);
        other
            .recv_timeout(Duration::from_secs(130))
            .expect("free once the outer guard went");
    }

    /// A test that panics while holding the turn lets it go.
    #[test]
    fn a_panicking_holder_lets_the_turn_go() {
        let panicked = std::panic::catch_unwind(|| {
            let _state = use_test_dirs();
            panic!("the test failed");
        });
        assert!(panicked.is_err());
        take_elsewhere()
            .recv_timeout(Duration::from_secs(130))
            .expect("free after the panic");
    }

    /// A helper called with no turn held says so.
    #[test]
    fn a_helper_without_the_turn_fails() {
        let asked = std::thread::spawn(assert_test_turn).join();
        assert!(asked.is_err());
    }
}
