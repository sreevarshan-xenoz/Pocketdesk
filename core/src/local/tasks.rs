//! Named tasks (ecosystem phase 1c): commands the owner names on the
//! computer, which a phone with the Tasks permission may list and start.
//! A task that asks first is held until the owner answers at the desk: the
//! server lists the held starts in `<state>/task_requests.json` (owner-only,
//! the only writer, rewritten whole), and the desktop app answers one by
//! writing `<state>/task_answers/<id>.json`. No answer within
//! [`ASK_TIMEOUT_SECS`] is a deny.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::paths::{TASK_ANSWERS_DIR, TASK_REQUESTS_FILE};

/// How long a start that asks first waits for the owner.
pub const ASK_TIMEOUT_SECS: u64 = 60;
/// The most of a task's output (stdout and stderr together) sent back: its end.
pub const MAX_OUTPUT: usize = 64 * 1024;
/// A task's deadline when it names none, and the longest one may have.
pub const DEFAULT_DEADLINE_SECS: u32 = 60;
pub const MAX_DEADLINE_SECS: u32 = 3600;

fn yes() -> bool {
    true
}

fn default_deadline() -> u32 {
    DEFAULT_DEADLINE_SECS
}

/// A task as config.toml names it (`[[tasks]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSpec {
    /// What the phone shows and starts it by.
    pub name: String,
    /// The command line, run through the system shell as the owner.
    pub command: String,
    /// Hold each start until the owner allows it at the desk.
    #[serde(default = "yes")]
    pub ask: bool,
    /// Seconds before a run still going is stopped.
    #[serde(default = "default_deadline")]
    pub deadline_secs: u32,
}

impl TaskSpec {
    /// Its deadline, held to 1 s .. [`MAX_DEADLINE_SECS`].
    pub fn deadline(&self) -> Duration {
        Duration::from_secs(u64::from(self.deadline_secs.clamp(1, MAX_DEADLINE_SECS)))
    }
}

/// Why a task's name cannot be one, or `None` when it can: 1 to 40
/// characters, no control characters, not only spaces.
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("a task needs a name")
    } else if name.chars().count() > 40 {
        Some("a task's name is at most 40 characters")
    } else if name.chars().any(char::is_control) {
        Some("a task's name cannot hold control characters")
    } else {
        None
    }
}

/// A start held for the owner's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    /// The server's name for this start; also the answer's file name.
    pub id: String,
    pub device_id: String,
    pub task: String,
    pub asked_unix: u64,
}

impl TaskRequest {
    pub fn is_open(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.asked_unix) < ASK_TIMEOUT_SECS
    }

    pub fn remaining(&self, now_unix: u64) -> u64 {
        (self.asked_unix + ASK_TIMEOUT_SECS).saturating_sub(now_unix)
    }
}

/// The owner's answer to a held start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskVerdict {
    Allow,
    Deny,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    requests: Vec<TaskRequest>,
}

#[derive(Serialize, Deserialize)]
struct Answer {
    id: String,
    verdict: TaskVerdict,
    answered_unix: u64,
}

/// A request id is a file name: letters, digits and `-` only.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Replace the held starts (the server's side).
pub fn write_requests(state_dir: &Path, requests: &[TaskRequest]) -> io::Result<()> {
    let json = serde_json::to_string(&Pending {
        requests: requests.to_vec(),
    })
    .map_err(io::Error::other)?;
    write_atomic(&state_dir.join(TASK_REQUESTS_FILE), json.as_bytes())
}

/// The held starts still open at `now_unix`.
pub fn read_requests(state_dir: &Path, now_unix: u64) -> Vec<TaskRequest> {
    std::fs::read_to_string(state_dir.join(TASK_REQUESTS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<Pending>(&raw).ok())
        .map(|p| p.requests)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.is_open(now_unix) && is_valid_id(&r.id))
        .collect()
}

/// Answer held start `id` (the app's side).
pub fn answer(state_dir: &Path, id: &str, verdict: TaskVerdict, now_unix: u64) -> io::Result<()> {
    let path = answer_path(state_dir, id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(TASK_ANSWERS_DIR))?;
    let json = serde_json::to_string(&Answer {
        id: id.to_string(),
        verdict,
        answered_unix: now_unix,
    })
    .map_err(io::Error::other)?;
    write_atomic(&path, json.as_bytes())
}

/// Take the answer to held start `id`, if there is one (the server's side):
/// removed once read; one naming another start, or too old, is none.
pub fn take_answer(state_dir: &Path, id: &str, now_unix: u64) -> Option<TaskVerdict> {
    let path = answer_path(state_dir, id).ok()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let answer: Answer = serde_json::from_str(&raw).ok()?;
    let fresh =
        answer.answered_unix <= now_unix && now_unix - answer.answered_unix <= ASK_TIMEOUT_SECS;
    (answer.id == id && fresh).then_some(answer.verdict)
}

fn answer_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir.join(TASK_ANSWERS_DIR).join(format!("{id}.json")))
}

// Write then rename, owner-only, so a reader never sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// The last [`MAX_OUTPUT`] bytes of `bytes` as text, and whether any were
/// left out.
pub fn output_tail(bytes: &[u8]) -> (String, bool) {
    if bytes.len() <= MAX_OUTPUT {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }
    (
        String::from_utf8_lossy(&bytes[bytes.len() - MAX_OUTPUT..]).into_owned(),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-tasks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(id: &str, at: u64) -> TaskRequest {
        TaskRequest {
            id: id.into(),
            device_id: "phone".into(),
            task: "Back up".into(),
            asked_unix: at,
        }
    }

    #[test]
    fn a_task_asks_first_and_has_a_minute_unless_it_says_otherwise() {
        let spec: TaskSpec = toml::from_str("name = \"Back up\"\ncommand = \"backup.sh\"").unwrap();
        assert!(spec.ask);
        assert_eq!(spec.deadline(), Duration::from_secs(60));
        let long = TaskSpec {
            deadline_secs: 99_999,
            ..spec.clone()
        };
        assert_eq!(long.deadline(), Duration::from_secs(3600));
        let none = TaskSpec {
            deadline_secs: 0,
            ..spec
        };
        assert_eq!(none.deadline(), Duration::from_secs(1));
    }

    #[test]
    fn a_name_is_short_printable_text() {
        assert_eq!(name_problem("Back up"), None);
        assert!(name_problem("  ").is_some());
        assert!(name_problem(&"x".repeat(41)).is_some());
        assert!(name_problem("a\nb").is_some());
    }

    #[test]
    fn a_held_start_is_answered_once_and_only_while_fresh() {
        let dir = temp("answer");
        write_requests(&dir, &[request("r1", 100), request("r2", 10)]).unwrap();
        let open: Vec<String> = read_requests(&dir, 100).into_iter().map(|r| r.id).collect();
        assert_eq!(open, vec!["r1"], "r2 is past its minute");
        answer(&dir, "r1", TaskVerdict::Allow, 101).unwrap();
        assert_eq!(take_answer(&dir, "r1", 102), Some(TaskVerdict::Allow));
        assert_eq!(take_answer(&dir, "r1", 102), None, "taken once");
        answer(&dir, "r1", TaskVerdict::Allow, 10).unwrap();
        assert_eq!(take_answer(&dir, "r1", 200), None, "too old");
        assert!(answer(&dir, "../x", TaskVerdict::Allow, 1).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_past_the_cap_keeps_its_end() {
        let big = vec![b'a'; MAX_OUTPUT + 10];
        let (text, cut) = output_tail(&big);
        assert!(cut);
        assert_eq!(text.len(), MAX_OUTPUT);
        assert_eq!(output_tail(b"ok"), ("ok".to_string(), false));
    }
}
