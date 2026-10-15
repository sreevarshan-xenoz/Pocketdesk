//! What the app shows, read from the files the server keeps — plain data,
//! no window, so it is tested directly.

use std::path::Path;

use pocketdesk_core::local::approval::{self, ApprovalRequest, Verdict};
use pocketdesk_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
use pocketdesk_core::local::received::{self, ReceivedFile};
use pocketdesk_core::local::sent::{self, SentFile};
use pocketdesk_core::local::{server, sessions};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};

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
    /// Every phone drives, as the running server says (N7).
    pub shared_control: bool,
    /// Sessions the server is holding for the owner's answer (F5).
    pub approvals: Vec<ApprovalRequest>,
    /// Files received from phones, newest first (F7).
    pub received: Vec<ReceivedRow>,
    /// Files sent to phones and how each went, newest first (F7b).
    pub sent: Vec<SentFile>,
}

/// A received file, and whether it is still where it was saved (checked
/// once per refresh, not on every repaint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRow {
    pub file: ReceivedFile,
    pub exists: bool,
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
    let mirror = std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .filter(|m| running && m.is_fresh(now_unix));
    let shared_control = mirror.as_ref().is_some_and(|m| m.shared_control);
    let sessions = mirror.map(|m| m.sessions).unwrap_or_default();
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
    let received = received::read(state_dir)
        .into_iter()
        .rev()
        .map(|file| ReceivedRow {
            exists: Path::new(&file.path).exists(),
            file,
        })
        .collect();
    let mut sent = sent::read(state_dir);
    sent.reverse();
    Snapshot {
        running,
        devices,
        sessions,
        shared_control,
        approvals,
        received,
        sent,
    }
}

/// The phones a file can be sent to: those in a session now, since only
/// then does a phone hold the connection an offer reaches (F7b).
/// What the Sessions tab shows about who drives (roadmap N7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCell {
    /// This session's phone drives.
    Driving,
    /// Another phone drives or no one does: a button hands control here.
    Give(u64),
    /// One phone, or a server too old to say.
    Nothing,
}

pub fn control_cell(
    session: &sessions::MirrorSession,
    sessions_live: usize,
    shared_control: bool,
) -> ControlCell {
    if sessions_live < 2 || session.id == 0 || shared_control {
        ControlCell::Nothing
    } else if session.driving {
        ControlCell::Driving
    } else {
        ControlCell::Give(session.id)
    }
}

pub fn send_targets(snapshot: &Snapshot) -> Vec<&Device> {
    snapshot.devices.iter().filter(|d| d.connected).collect()
}

/// The phone a send goes to: the one the owner chose, while it is still in
/// a session; with none chosen, the first phone that is. A chosen phone
/// that left is an error, never a quiet switch to another phone.
pub fn target_for_send(snapshot: &Snapshot, chosen: Option<&str>) -> Result<String, String> {
    let targets = send_targets(snapshot);
    match chosen {
        Some(id) if targets.iter().any(|d| d.id == id) => Ok(id.to_string()),
        Some(_) => Err(
            "The phone you chose has left its session. Choose a phone, then send again."
                .to_string(),
        ),
        None => targets
            .first()
            .map(|d| d.id.clone())
            .ok_or_else(|| "No phone is in a session to send to.".to_string()),
    }
}

/// Ask the server to send the file at `path` to `device_id`; the file's
/// name, or why not.
/// Queue the link `url` for the phone `device_id` (T8a), which shows it as
/// a notification that opens it; what to tell the owner.
pub fn queue_link(
    state_dir: &Path,
    device_id: &str,
    url: &str,
    now_unix: u64,
) -> Result<String, String> {
    let url = url.trim();
    pocketdesk_core::local::links::request(state_dir, device_id, url, now_unix)
        .map(|_| "Link sent: it waits on the phone as a notification.".to_string())
        .map_err(|e| format!("Not sent: {e}."))
}

pub fn queue_send(
    state_dir: &Path,
    device_id: &str,
    path: &Path,
    now_unix: u64,
) -> Result<String, String> {
    // A file, or a folder the server sends file by file (T6b).
    if !path.is_file() && !path.is_dir() {
        return Err(format!("{} is not a file or a folder.", path.display()));
    }
    // The server resolves the path, from its own working folder: name the
    // file as this app sees it.
    let path = &std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    pocketdesk_core::local::outbox::request(
        state_dir,
        device_id,
        &path.display().to_string(),
        now_unix,
    )
    .map_err(|e| format!("Could not queue {name}: {e}"))?;
    Ok(name)
}

/// Allow or deny one capability for `id`, in the store the server reads.
pub fn set_permission(
    state_dir: &Path,
    id: &str,
    capability: Capability,
    allowed: bool,
) -> pocketdesk_core::error::Result<()> {
    update_permissions(state_dir, id, |p| p.set(capability, allowed))
}

/// Whether a running session keeps its own copy of this permission, taken at
/// its start; the control channel re-reads the others for every packet.
pub fn fixed_at_session_start(capability: Capability) -> bool {
    matches!(
        capability,
        Capability::Input | Capability::Audio | Capability::Mic
    )
}

/// Hold each of `id`'s sessions for approval at this desktop, or not.
pub fn set_ask_first(
    state_dir: &Path,
    id: &str,
    ask_first: bool,
) -> pocketdesk_core::error::Result<()> {
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
) -> pocketdesk_core::error::Result<()> {
    TrustStore::update(state_dir.join(TRUST_FILE), |store| {
        let mut permissions = store.permissions(id);
        change(&mut permissions);
        store.set_permissions(id, permissions)
    })
}

/// The last `lines` lines of the newest daily server log
/// (`<state>/logs/pocketdesk.<date>.log`, written by `start --background`).
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
                .is_some_and(|n| n.starts_with("pocketdesk.") && n.ends_with(".log"))
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
    use pocketdesk_core::local::paths::{LIVE_FILE, LOG_DIR, TRUST_FILE};
    use pocketdesk_core::protocol::kdeconnect::TrustStore;
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
        use pocketdesk_core::protocol::permissions::Capability;
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
    fn only_streamed_permissions_are_fixed_for_a_running_session() {
        let fixed: Vec<_> = Capability::ALL
            .into_iter()
            .filter(|c| fixed_at_session_start(*c))
            .collect();
        assert_eq!(
            fixed,
            [Capability::Input, Capability::Audio, Capability::Mic]
        );
    }

    #[test]
    fn received_files_are_listed_newest_first() {
        let dir = temp_dir("received");
        for n in 1..=3u64 {
            received::record(
                &dir,
                ReceivedFile {
                    name: format!("f{n}"),
                    path: format!("/d/f{n}"),
                    size: n,
                    received_unix: n,
                    from: "phone".into(),
                },
            )
            .unwrap();
        }
        let names: Vec<_> = snapshot_with(&dir, 10, false)
            .received
            .into_iter()
            .map(|row| row.file.name)
            .collect();
        assert_eq!(names, ["f3", "f2", "f1"]);
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
        std::fs::write(logs.join("pocketdesk.2026-10-11.log"), "old\n").unwrap();
        let lines: String = (0..250).map(|i| format!("line {i}\n")).collect();
        std::fs::write(logs.join("pocketdesk.2026-10-12.log"), lines).unwrap();
        let tail = log_tail(&dir, 200);
        assert_eq!(tail.len(), 200);
        assert_eq!(tail.first().map(String::as_str), Some("line 50"));
        assert_eq!(tail.last().map(String::as_str), Some("line 249"));
        assert!(log_tail(&temp_dir("nologs"), 10).is_empty());
    }

    #[test]
    fn only_a_phone_in_a_session_can_be_sent_a_file() {
        let dir = temp_dir("targets");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store.trust_device("b").unwrap();
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        let ids: Vec<_> = send_targets(&snap).iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, ["a"]);
    }

    #[test]
    fn a_link_is_queued_for_the_server_and_one_that_may_not_open_is_refused() {
        let dir = temp_dir("link");
        assert!(queue_link(&dir, "a", " https://example.org/page ", 100).is_ok());
        let queued = pocketdesk_core::local::links::take_all(&dir, 101);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].url, "https://example.org/page", "trimmed");
        assert!(queue_link(&dir, "a", "javascript:alert(1)", 100).is_err());
        assert!(queue_link(&dir, "a", "", 100).is_err());
    }

    #[test]
    fn a_send_queues_a_request_for_the_server_and_refuses_what_is_not_there() {
        let dir = temp_dir("send");
        let file = dir.join("report.pdf");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            queue_send(&dir, "a", &file, 100),
            Ok("report.pdf".to_string())
        );
        let queued = pocketdesk_core::local::outbox::take_all(&dir, 101);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].device_id, "a");
        assert_eq!(queued[0].path, file.display().to_string());
        assert!(queue_send(&dir, "a", &dir.join("gone"), 100).is_err());
        assert!(pocketdesk_core::local::outbox::take_all(&dir, 101).is_empty());
        // A folder goes too (T6b): the server offers it file by file.
        let folder = dir.join("Trip");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(queue_send(&dir, "a", &folder, 100), Ok("Trip".to_string()));
        assert_eq!(pocketdesk_core::local::outbox::take_all(&dir, 101).len(), 1);
    }

    #[test]
    fn a_relative_path_is_queued_as_the_app_sees_it() {
        let dir = temp_dir("relative");
        // Tests run in the package folder, which holds Cargo.toml.
        assert!(queue_send(&dir, "a", std::path::Path::new("Cargo.toml"), 100).is_ok());
        let queued = pocketdesk_core::local::outbox::take_all(&dir, 101);
        let path = std::path::Path::new(&queued[0].path);
        assert!(path.is_absolute(), "{}", path.display());
        assert!(path.ends_with("Cargo.toml"));
    }

    #[test]
    fn a_send_goes_only_to_the_phone_chosen_while_it_is_in_a_session() {
        let dir = temp_dir("chosen");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("a").unwrap();
        store.trust_device("b").unwrap();
        std::fs::write(dir.join(LIVE_FILE), ONE_SESSION).unwrap();
        let snap = snapshot_with(&dir, 101, true);
        assert_eq!(target_for_send(&snap, Some("a")), Ok("a".to_string()));
        assert_eq!(
            target_for_send(&snap, None),
            Ok("a".to_string()),
            "the one in a session"
        );
        assert!(
            target_for_send(&snap, Some("b")).is_err(),
            "b left its session: no silent switch to a"
        );
        let empty = snapshot_with(&temp_dir("chosen-none"), 101, true);
        assert!(target_for_send(&empty, None).is_err());
    }

    #[test]
    fn sent_files_are_listed_newest_first() {
        use pocketdesk_core::local::sent::{self, SentFile, SentState};
        let dir = temp_dir("sent");
        for n in 0..2u64 {
            sent::record(
                &dir,
                SentFile {
                    id: format!("s{n}"),
                    name: format!("f{n}"),
                    path: format!("/f{n}"),
                    size: n,
                    device_id: "a".to_string(),
                    sent_unix: n,
                    state: SentState::Sent,
                    note: None,
                },
            )
            .unwrap();
        }
        let names: Vec<_> = snapshot_with(&dir, 101, false)
            .sent
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["f1", "f0"]);
    }

    fn session(id: u64, driving: bool) -> sessions::MirrorSession {
        sessions::MirrorSession {
            id,
            device_id: Some(format!("phone-{id}")),
            peer: "192.0.2.1:5".into(),
            transport: "lan".into(),
            started_unix: 1,
            driving,
        }
    }

    #[test]
    fn with_several_phones_the_driver_is_marked_and_the_others_can_be_given_control() {
        assert_eq!(
            control_cell(&session(1, true), 2, false),
            ControlCell::Driving
        );
        assert_eq!(
            control_cell(&session(2, false), 2, false),
            ControlCell::Give(2)
        );
    }

    #[test]
    fn one_phone_or_an_old_server_shows_no_control() {
        assert_eq!(
            control_cell(&session(1, true), 1, false),
            ControlCell::Nothing
        );
        assert_eq!(
            control_cell(&session(0, false), 2, false),
            ControlCell::Nothing
        );
        assert_eq!(
            control_cell(&session(2, false), 2, true),
            ControlCell::Nothing,
            "nothing to hand over when every phone drives"
        );
    }
}
