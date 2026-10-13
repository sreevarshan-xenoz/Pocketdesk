//! Holding a session until the owner approves it at the desk (roadmap F5,
//! spec P5). The files are `core::local::approval`; the desktop app shows
//! the prompt and writes the answer.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use pocketdesk_core::local::approval::{self, APPROVAL_TIMEOUT_SECS, ApprovalRequest, Verdict};
use pocketdesk_core::local::paths::APPROVAL_DIR;
use pocketdesk_core::local::pin::now_unix;

/// How often a held session looks for its answer.
const POLL: Duration = Duration::from_millis(250);

/// Every session held right now, per state directory (one in production;
/// tests use their own).
fn held() -> &'static Mutex<HashMap<PathBuf, Vec<ApprovalRequest>>> {
    static HELD: OnceLock<Mutex<HashMap<PathBuf, Vec<ApprovalRequest>>>> = OnceLock::new();
    HELD.get_or_init(Default::default)
}

/// Ask the owner whether `device_id`, connecting from `peer`, may start a
/// session. No answer within the time limit is [`Verdict::Deny`].
pub async fn ask(device_id: &str, peer: SocketAddr) -> Verdict {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("Cannot hold {device_id} for approval: {e}");
            return Verdict::Deny;
        }
    };
    notify(device_id, peer);
    ask_in(
        &dir,
        device_id,
        peer,
        Duration::from_secs(APPROVAL_TIMEOUT_SECS),
    )
    .await
}

async fn ask_in(dir: &Path, device_id: &str, peer: SocketAddr, limit: Duration) -> Verdict {
    let request = ApprovalRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        peer: peer_label(peer),
        asked_unix: now_unix(),
    };
    tracing::info!("Holding a session from {device_id} ({peer}) for approval at the desk");
    // Listed until this returns or is dropped (the phone gave up).
    let _listed = Listed::new(dir, request.clone());
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if let Some(verdict) = approval::take_answer(dir, &request.id, now_unix()) {
            tracing::info!("Session from {device_id}: {verdict:?}");
            return verdict;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::info!("Session from {device_id} denied: no answer in {limit:?}");
            return Verdict::Deny;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Where the session comes from, for the prompt. An iroh (WAN) connection
/// reports no socket address, only a placeholder.
fn peer_label(peer: SocketAddr) -> String {
    if peer.ip().is_unspecified() {
        "the internet (WAN)".to_string()
    } else {
        peer.to_string()
    }
}

/// A request's place in the pending file, removed on drop.
struct Listed {
    dir: PathBuf,
    id: String,
}

impl Listed {
    fn new(dir: &Path, request: ApprovalRequest) -> Self {
        let id = request.id.clone();
        let mut all = held().lock().expect("approvals");
        let list = all.entry(dir.to_path_buf()).or_default();
        list.push(request);
        publish(dir, list);
        Self {
            dir: dir.to_path_buf(),
            id,
        }
    }
}

impl Drop for Listed {
    fn drop(&mut self) {
        let mut all = held().lock().expect("approvals");
        let list = all.entry(self.dir.clone()).or_default();
        list.retain(|r| r.id != self.id);
        publish(&self.dir, list);
        // Answers nobody will take: this one's, and any that arrived after
        // their session went away.
        approval::discard_answer(&self.dir, &self.id);
        sweep_answers(&self.dir, list);
    }
}

fn sweep_answers(dir: &Path, held: &[ApprovalRequest]) {
    let Ok(entries) = std::fs::read_dir(dir.join(APPROVAL_DIR)) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        // `<id>.json.tmp` is an answer the app is writing right now.
        if path.extension().is_some_and(|e| e == "tmp") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !held.iter().any(|r| r.id == stem) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn publish(dir: &Path, list: &[ApprovalRequest]) {
    if let Err(e) = approval::write_pending(dir, list) {
        tracing::warn!("Could not write the pending approvals: {e}");
    }
}

fn notify(device_id: &str, peer: SocketAddr) {
    let summary = "Pocketdesk: a phone wants to connect";
    let body = format!(
        "{device_id} from {}. Allow or deny it in the Pocketdesk app within {APPROVAL_TIMEOUT_SECS} s.",
        peer_label(peer)
    );
    tokio::task::spawn_blocking(move || {
        if !crate::platform::notify(summary, &body, Duration::from_secs(APPROVAL_TIMEOUT_SECS)) {
            tracing::info!("{summary} — {body}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-held-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PEER: &str = "192.0.2.7:51000";

    /// Answers the first request that shows up in `dir`'s pending file.
    async fn answer_when_listed(dir: PathBuf, verdict: Verdict) {
        loop {
            if let Some(request) = approval::read_pending(&dir, now_unix()).first() {
                approval::answer(&dir, &request.id, verdict, now_unix()).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_held_session_gets_the_owners_answer_and_leaves_the_list() {
        for verdict in [Verdict::AllowOnce, Verdict::AlwaysAllow, Verdict::Deny] {
            let dir = temp_dir(&format!("{verdict:?}"));
            let owner = tokio::spawn(answer_when_listed(dir.clone(), verdict));
            let got = ask_in(
                &dir,
                "phone",
                PEER.parse().unwrap(),
                Duration::from_secs(10),
            )
            .await;
            owner.await.unwrap();
            assert_eq!(got, verdict);
            assert!(approval::read_pending(&dir, now_unix()).is_empty());
        }
    }

    #[test]
    fn a_wan_session_is_named_as_such_not_as_a_placeholder_address() {
        assert_eq!(peer_label(PEER.parse().unwrap()), PEER);
        assert_eq!(
            peer_label("0.0.0.0:0".parse().unwrap()),
            "the internet (WAN)"
        );
    }

    #[tokio::test]
    async fn no_answer_in_time_is_a_deny() {
        let dir = temp_dir("timeout");
        let got = ask_in(
            &dir,
            "phone",
            PEER.parse().unwrap(),
            Duration::from_millis(300),
        )
        .await;
        assert_eq!(got, Verdict::Deny);
        assert!(approval::read_pending(&dir, now_unix()).is_empty());
    }

    #[tokio::test]
    async fn a_phone_that_gives_up_leaves_the_list_and_its_late_answer_is_dropped() {
        let dir = temp_dir("dropped");
        let wait = tokio::spawn({
            let dir = dir.clone();
            async move {
                ask_in(
                    &dir,
                    "phone",
                    PEER.parse().unwrap(),
                    Duration::from_secs(10),
                )
                .await
            }
        });
        let id = loop {
            if let Some(r) = approval::read_pending(&dir, now_unix()).first() {
                break r.id.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        wait.abort();
        let _ = wait.await;
        approval::answer(&dir, &id, Verdict::AllowOnce, now_unix()).unwrap();
        assert!(approval::read_pending(&dir, now_unix()).is_empty());
        // The late answer answers nothing, and the next session to leave the
        // list sweeps it away.
        assert_eq!(
            ask_in(
                &dir,
                "phone",
                PEER.parse().unwrap(),
                Duration::from_millis(300)
            )
            .await,
            Verdict::Deny
        );
        assert_eq!(
            std::fs::read_dir(dir.join(APPROVAL_DIR)).unwrap().count(),
            0,
            "no answer is left behind"
        );
    }
}
