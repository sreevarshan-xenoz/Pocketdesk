//! Session approval at the desk (roadmap F5, spec P5).
//!
//! A phone whose owner chose "Ask before each session" is held at the stream
//! gate. The server lists every held session in
//! `<state>/pending_approvals.json` (it is the only writer, and rewrites the
//! whole file atomically). The desktop app answers one by writing
//! `<state>/approvals/<id>.json`; one file per request, so two phones held
//! at once cannot overwrite each other's answer. The server takes (reads and
//! deletes) its own answer. A request nobody answers within
//! [`APPROVAL_TIMEOUT_SECS`] is denied.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::paths::{APPROVAL_DIR, PENDING_APPROVALS_FILE};

/// How long a held session waits for an answer before it is denied.
pub const APPROVAL_TIMEOUT_SECS: u64 = 30;

/// One session held for approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// The server's name for this request; also the answer's file name.
    pub id: String,
    pub device_id: String,
    /// The address the session came from.
    pub peer: String,
    pub asked_unix: u64,
}

impl ApprovalRequest {
    /// Still waiting at `now_unix`: after this, the server has denied it.
    pub fn is_open(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.asked_unix) < APPROVAL_TIMEOUT_SECS
    }

    /// Seconds left to answer.
    pub fn remaining(&self, now_unix: u64) -> u64 {
        (self.asked_unix + APPROVAL_TIMEOUT_SECS).saturating_sub(now_unix)
    }
}

/// The owner's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    AllowOnce,
    /// Allow, and stop asking for this device.
    AlwaysAllow,
    Deny,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    requests: Vec<ApprovalRequest>,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    id: String,
    verdict: Verdict,
    answered_unix: u64,
}

/// A request id is a file name: letters, digits and `-` only, so an id can
/// never name a path outside the answer directory.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Replace the pending list (the server's side).
pub fn write_pending(state_dir: &Path, requests: &[ApprovalRequest]) -> io::Result<()> {
    let json = serde_json::to_string(&Pending {
        requests: requests.to_vec(),
    })
    .map_err(io::Error::other)?;
    write_atomic(&state_dir.join(PENDING_APPROVALS_FILE), json.as_bytes())
}

/// The requests still open at `now_unix`. A list left behind by a server
/// that died mid-wait shows nothing once its requests time out.
pub fn read_pending(state_dir: &Path, now_unix: u64) -> Vec<ApprovalRequest> {
    std::fs::read_to_string(state_dir.join(PENDING_APPROVALS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<Pending>(&raw).ok())
        .map(|p| p.requests)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.is_open(now_unix) && is_valid_id(&r.id))
        .collect()
}

/// Answer request `id` (the app's side).
pub fn answer(state_dir: &Path, id: &str, verdict: Verdict, now_unix: u64) -> io::Result<()> {
    let path = answer_path(state_dir, id)?;
    std::fs::create_dir_all(state_dir.join(APPROVAL_DIR))?;
    let json = serde_json::to_string(&Answer {
        id: id.to_string(),
        verdict,
        answered_unix: now_unix,
    })
    .map_err(io::Error::other)?;
    write_atomic(&path, json.as_bytes())
}

/// Take the answer to request `id`, if there is one (the server's side).
/// The file is removed either way once read; an answer that names another
/// request or is older than the time limit counts as none.
pub fn take_answer(state_dir: &Path, id: &str, now_unix: u64) -> Option<Verdict> {
    let path = answer_path(state_dir, id).ok()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let answer: Answer = serde_json::from_str(&raw).ok()?;
    // Same machine, same clock: an answer from the future is not one.
    let fresh = answer.answered_unix <= now_unix
        && now_unix - answer.answered_unix <= APPROVAL_TIMEOUT_SECS;
    (answer.id == id && fresh).then_some(answer.verdict)
}

/// Drop an answer nobody will take (the request ended without reading it).
pub fn discard_answer(state_dir: &Path, id: &str) {
    if let Ok(path) = answer_path(state_dir, id) {
        let _ = std::fs::remove_file(path);
    }
}

fn answer_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir.join(APPROVAL_DIR).join(format!("{id}.json")))
}

// Write then rename, so a reader never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-approval-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(id: &str, asked_unix: u64) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            device_id: "phone".to_string(),
            peer: "192.0.2.7:51000".to_string(),
            asked_unix,
        }
    }

    #[test]
    fn pending_requests_read_back_until_they_time_out() {
        let dir = temp_dir("pending");
        assert!(read_pending(&dir, 100).is_empty(), "no file is no requests");
        write_pending(&dir, &[request("a", 100), request("b", 80)]).unwrap();
        let open: Vec<_> = read_pending(&dir, 110).into_iter().map(|r| r.id).collect();
        assert_eq!(open, ["a"], "b was asked 30 s ago and is already denied");
        assert!(read_pending(&dir, 130).is_empty());
        assert_eq!(request("a", 100).remaining(110), 20);
    }

    #[test]
    fn an_answer_is_taken_once() {
        let dir = temp_dir("answer");
        assert_eq!(take_answer(&dir, "a", 100), None);
        answer(&dir, "a", Verdict::AlwaysAllow, 100).unwrap();
        assert_eq!(
            take_answer(&dir, "b", 101),
            None,
            "another request's answer"
        );
        assert_eq!(take_answer(&dir, "a", 101), Some(Verdict::AlwaysAllow));
        assert_eq!(take_answer(&dir, "a", 101), None, "taken");
    }

    #[test]
    fn a_stale_or_mislabelled_answer_counts_as_none() {
        let dir = temp_dir("stale");
        answer(&dir, "a", Verdict::AllowOnce, 100).unwrap();
        assert_eq!(take_answer(&dir, "a", 131), None, "older than the limit");
        answer(&dir, "a", Verdict::AllowOnce, 500).unwrap();
        assert_eq!(take_answer(&dir, "a", 100), None, "stamped in the future");
        // A file under one id that says it answers another.
        answer(&dir, "b", Verdict::AllowOnce, 100).unwrap();
        std::fs::rename(
            dir.join(APPROVAL_DIR).join("b.json"),
            dir.join(APPROVAL_DIR).join("c.json"),
        )
        .unwrap();
        assert_eq!(take_answer(&dir, "c", 101), None);
    }

    #[test]
    fn an_id_can_never_name_a_path() {
        let dir = temp_dir("ids");
        for bad in ["", "../trusted_devices", "a/b", "a\\b", "a.json"] {
            assert!(!is_valid_id(bad), "{bad:?}");
            assert!(answer(&dir, bad, Verdict::AllowOnce, 1).is_err(), "{bad:?}");
        }
        assert!(is_valid_id("0f6e1c3a-9b7d-4c1e-8a55-3d2b1f0e9c88"));
    }
}
