//! What the app shows, read from the files the server keeps — plain data,
//! no window, so it is tested directly.

use std::path::Path;

use linux_link_core::local::approval::{self, ApprovalRequest, Verdict};
use linux_link_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
use linux_link_core::local::{server, sessions};
use linux_link_core::protocol::kdeconnect::TrustStore;
use linux_link_core::protocol::permissions::{Capability, DevicePermissions};

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    /// Seconds left on a time-boxed grant; `None` is permanent.
    pub grant_remaining: Option<u64>,
    /// Whether it has a live streaming session right now.
    pub connected: bool,
    /// What the owner lets it do (roadmap F5).
    pub permissions: DevicePermissions,
}

/// Everything the window shows, read once per refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub running: bool,
    pub devices: Vec<Device>,
    pub sessions: Vec<sessions::MirrorSession>,
    /// Sessions the server is holding for the owner's answer (F5).
    pub approvals: Vec<ApprovalRequest>,
}

/// The state under `state_dir` at `now_unix`.
pub fn snapshot(state_dir: &Path, now_unix: u64) -> Snapshot {
    snapshot_with(state_dir, now_unix, server::is_running(state_dir))
}

/// [`snapshot`] with the liveness answer supplied, so it is testable without
/// a server process.
pub fn snapshot_with(state_dir: &Path, now_unix: u64, running: bool) -> Snapshot {
    // A mirror is believed only while the server that writes it is alive
    // and has written it recently; a crash leaves the last one behind.
    let sessions = std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .filter(|m| running && m.is_fresh(now_unix))
        .map(|m| m.sessions)
        .unwrap_or_default();
    let devices = match TrustStore::load_or_create(state_dir.join(TRUST_FILE)) {
        Ok(store) => store
            .trusted_devices()
            .into_iter()
            .map(|id| Device {
                grant_remaining: store.grant_remaining(&id),
                connected: sessions.iter().any(|s| s.device_id.as_deref() == Some(&id)),
                permissions: store.permissions(&id),
                id,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    // Like the mirror: only a running server is still waiting for an answer.
    let approvals = if running {
        approval::read_pending(state_dir, now_unix)
    } else {
        Vec::new()
    };
    Snapshot {
        running,
        devices,
        sessions,
        approvals,
    }
}

/// Allow or deny one capability for `id`, in the store the server reads.
pub fn set_permission(
    state_dir: &Path,
    id: &str,
    capability: Capability,
    allowed: bool,
) -> linux_link_core::error::Result<()> {
    update_permissions(state_dir, id, |p| p.set(capability, allowed))
}

/// Hold each of `id`'s sessions for approval at this desktop, or not.
pub fn set_ask_first(
    state_dir: &Path,
    id: &str,
    ask_first: bool,
) -> linux_link_core::error::Result<()> {
    update_permissions(state_dir, id, |p| p.ask_first = ask_first)
}

/// The owner's answer to a held session.
pub fn answer_approval(
    state_dir: &Path,
    id: &str,
    verdict: Verdict,
    now_unix: u64,
) -> std::io::Result<()> {
    approval::answer(state_dir, id, verdict, now_unix)
}

// Load, change, save under the store's lock: the server writes the same file
// (pairing, grants, "Always allow"), so the copy read at the last refresh may
// be stale, and two writers must not lose each other's change.
fn update_permissions(
    state_dir: &Path,
    id: &str,
    change: impl FnOnce(&mut DevicePermissions),
) -> linux_link_core::error::Result<()> {
    TrustStore::update(state_dir.join(TRUST_FILE), |store| {
        let mut permissions = store.permissions(id);
        change(&mut permissions);
        store.set_permissions(id, permissions)
    })
}

/// The last `lines` lines of the newest daily server log
/// (`<state>/logs/linux-link.<date>.log`, written by `start --background`).
pub fn log_tail(state_dir: &Path, lines: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(LOG_DIR)) else {
        return Vec::new();
    };
    let newest = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("linux-link.") && n.ends_with(".log"))
        })
        .max();
    let Some(raw) = newest.and_then(|p| std::fs::read(p).ok()) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&raw);
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
    use linux_link_core::protocol::kdeconnect::TrustStore;
    use std::time::Duration;

    pub(crate) fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const ONE_SESSION: &str = r#"{"updated_unix":100,"sessions":[{"device_id":"a","peer":"1.2.3.4:5","transport":"lan","started_unix":90}]}"#;

    #[test]
    fn a_dead_server_shows_no_sessions_even_with_a_mirror_left_behind() {
        let dir = temp_dir("dead");
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, false);
        assert!(!snap.running && snap.sessions.is_empty());
    }

    #[test]
    fn a_stale_mirror_shows_no_sessions_even_while_running() {
        let dir = temp_dir("stale");
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        assert!(snapshot_with(&dir, 200, true).sessions.is_empty());
        assert_eq!(snapshot_with(&dir, 101, true).sessions.len(), 1);
    }

    #[test]
    fn devices_list_permanent_and_granted_and_mark_the_connected_one() {
        let dir = temp_dir("devices");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store
            .trust_device_with_ttl("b", Duration::from_secs(60))
            .unwrap();
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        let a = snap.devices.iter().find(|d| d.id == "a").unwrap();
        let b = snap.devices.iter().find(|d| d.id == "b").unwrap();
        assert!(a.connected && a.grant_remaining.is_none());
        assert!(!b.connected && b.grant_remaining.is_some_and(|s| s <= 60));
    }

    #[test]
    fn a_permission_set_here_shows_in_the_next_snapshot_and_reaches_the_store() {
        use linux_link_core::protocol::permissions::Capability;
        let dir = temp_dir("perms");
        TrustStore::load_or_create(dir.join(TRUST_FILE))
            .unwrap()
            .trust_device("a")
            .unwrap();
        let before = snapshot_with(&dir, 101, false);
        assert!(
            !before.devices[0].permissions.allows(Capability::Exec),
            "exec starts off"
        );

        set_permission(&dir, "a", Capability::Exec, true).unwrap();
        set_permission(&dir, "a", Capability::Clipboard, false).unwrap();
        let after = snapshot_with(&dir, 102, false);
        let a = &after.devices[0].permissions;
        assert!(a.allows(Capability::Exec) && !a.allows(Capability::Clipboard));
        assert!(
            TrustStore::load_or_create(dir.join(TRUST_FILE))
                .unwrap()
                .permissions("a")
                .allows(Capability::Exec),
            "the server reads the same file"
        );
    }

    #[test]
    fn held_sessions_show_while_the_server_runs_and_an_answer_reaches_it() {
        let dir = temp_dir("approvals");
        let request = ApprovalRequest {
            id: "r1".to_string(),
            device_id: "a".to_string(),
            peer: "192.0.2.7:51000".to_string(),
            asked_unix: 100,
        };
        approval::write_pending(&dir, std::slice::from_ref(&request)).unwrap();
        assert_eq!(snapshot_with(&dir, 101, true).approvals, [request]);
        assert!(
            snapshot_with(&dir, 101, false).approvals.is_empty(),
            "server gone"
        );
        assert!(
            snapshot_with(&dir, 131, true).approvals.is_empty(),
            "timed out"
        );

        answer_approval(&dir, "r1", Verdict::AllowOnce, 102).unwrap();
        assert_eq!(
            approval::take_answer(&dir, "r1", 102),
            Some(Verdict::AllowOnce)
        );
    }

    #[test]
    fn ask_first_is_set_per_device() {
        let dir = temp_dir("askfirst");
        TrustStore::load_or_create(dir.join(TRUST_FILE))
            .unwrap()
            .trust_device("a")
            .unwrap();
        set_ask_first(&dir, "a", true).unwrap();
        assert!(
            snapshot_with(&dir, 101, false).devices[0]
                .permissions
                .ask_first
        );
        set_ask_first(&dir, "a", false).unwrap();
        assert!(
            !snapshot_with(&dir, 101, false).devices[0]
                .permissions
                .ask_first
        );
    }

    #[test]
    fn the_log_tail_reads_the_newest_daily_file_last_lines() {
        let dir = temp_dir("logs");
        let logs = dir.join(LOG_DIR);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("linux-link.2026-10-11.log"), "old\n").unwrap();
        let lines: String = (0..250).map(|i| format!("line {i}\n")).collect();
        std::fs::write(logs.join("linux-link.2026-10-12.log"), lines).unwrap();
        let tail = log_tail(&dir, 200);
        assert_eq!(tail.len(), 200);
        assert_eq!(tail.first().map(String::as_str), Some("line 50"));
        assert_eq!(tail.last().map(String::as_str), Some("line 249"));
        assert!(log_tail(&temp_dir("nologs"), 10).is_empty());
    }
}
