//! The live-session mirror and kick requests.
//!
//! The server mirrors its live streaming sessions to
//! `<state>/live_sessions.json` every second (atomically), and ends sessions
//! named in kick requests: one file each in `<state>/kicks/`
//! (`<target>\n<unix-secs>\n`), so two quick requests are both seen, or the
//! single `<state>/kick` an older CLI writes. The CLI's `status` and
//! `kick` and the desktop app go through these files. The desktop hands input
//! control to a session through `<state>/control/` (roadmap N7), one file per
//! request.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::{CONTROL_DIR, KICK_FILE, KICKS_DIR};

/// A kick request older than this is dropped unexecuted — the operator
/// meant it for a session that is no longer (or not yet) live.
pub const KICK_TTL_SECS: u64 = 60;

/// The mirror is rewritten every second; one older than this was left by a
/// server that is no longer running.
const FRESH_SECS: u64 = 5;

/// One live streaming session as mirrored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorSession {
    /// The server's session id, for handing it control (0 from a server
    /// older than N7).
    #[serde(default)]
    pub id: u64,
    pub device_id: Option<String>,
    pub peer: String,
    /// `"lan"` or `"wan"`.
    pub transport: String,
    pub started_unix: u64,
    /// This session's phone drives (roadmap N7).
    #[serde(default)]
    pub driving: bool,
}

/// A phone with a control link to the server, in a session or not (T10d).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedPhone {
    pub device_id: String,
    /// Files can reach it: its link has an address the server can dial
    /// back (not one over the internet).
    pub files: bool,
}

/// `live_sessions.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveMirror {
    pub updated_unix: u64,
    /// Every phone drives (`input_control = "shared"`), so there is no
    /// control to hand over (roadmap N7).
    #[serde(default)]
    pub shared_control: bool,
    pub sessions: Vec<MirrorSession>,
    /// The phones with a control link, in a session or not (T10d).
    #[serde(default)]
    pub linked: Vec<LinkedPhone>,
}

impl LiveMirror {
    /// Whether a file can be offered to `device_id` now: it is in a
    /// session, or linked with an address the server can dial back.
    pub fn files_reach(&self, device_id: &str) -> bool {
        self.sessions
            .iter()
            .any(|s| s.device_id.as_deref() == Some(device_id))
            || self
                .linked
                .iter()
                .any(|p| p.device_id == device_id && p.files)
    }
}

impl LiveMirror {
    /// Whether the mirror was written recently enough to describe a running
    /// server (a crash leaves the last one behind).
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.updated_unix) <= FRESH_SECS
    }
}

/// The mirror's JSON, as the server writes it.
pub fn render_mirror(
    sessions: &[MirrorSession],
    shared_control: bool,
    linked: &[LinkedPhone],
    now_unix: u64,
) -> String {
    serde_json::to_string(&LiveMirror {
        updated_unix: now_unix,
        shared_control,
        sessions: sessions.to_vec(),
        linked: linked.to_vec(),
    })
    .unwrap_or_default()
}

pub fn parse_mirror(raw: &str) -> Option<LiveMirror> {
    serde_json::from_str(raw).ok()
}

/// Ask the running server to end the sessions matching `target` (a device
/// id, an id prefix of 6+ characters, a peer IP, or `all`).
pub fn request_kick(state_dir: &Path, target: &str, now_unix: u64) -> io::Result<()> {
    let target = target.trim();
    if target.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "kick target is empty",
        ));
    }
    let dir = state_dir.join(KICKS_DIR);
    std::fs::create_dir_all(&dir)?;
    let name = uuid::Uuid::new_v4().to_string();
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, format!("{target}\n{now_unix}\n"))?;
    std::fs::rename(tmp, dir.join(format!("{name}.kick")))
}

/// The targets of every fresh kick request, each taken once: the files in
/// `kicks/` and an older CLI's single `kick` file. Every request read is
/// removed (a stale or malformed one too).
pub fn take_kicks(state_dir: &Path, now_unix: u64) -> Vec<String> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(state_dir.join(KICKS_DIR))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("kick"))
                .collect()
        })
        .unwrap_or_default();
    paths.push(state_dir.join(KICK_FILE));
    let mut targets = Vec::new();
    for path in paths {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let _ = std::fs::remove_file(&path);
        if let Some(target) = parse_kick(&raw, now_unix) {
            targets.push(target);
        }
    }
    targets
}

/// A control request older than this is dropped unapplied.
pub const CONTROL_TTL_SECS: u64 = 60;

#[derive(Serialize, Deserialize)]
struct ControlRequest {
    session: u64,
    at_unix: u64,
}

/// Ask the running server to hand input control to `session` (roadmap N7).
/// One file per request, so two quick requests are both seen.
pub fn request_control(state_dir: &Path, session: u64, now_unix: u64) -> io::Result<()> {
    let dir = state_dir.join(CONTROL_DIR);
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::to_string(&ControlRequest {
        session,
        at_unix: now_unix,
    })
    .map_err(io::Error::other)?;
    let name = format!("{}.json", uuid::Uuid::new_v4());
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, dir.join(name))
}

/// The sessions named in fresh control requests, oldest request first;
/// every request file read is removed (a stale or broken one too).
pub fn take_control_requests(state_dir: &Path, now_unix: u64) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(CONTROL_DIR)) else {
        return Vec::new();
    };
    let mut requests = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let request = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<ControlRequest>(&raw).ok());
        let _ = std::fs::remove_file(&path);
        if let Some(request) = request
            && now_unix.saturating_sub(request.at_unix) <= CONTROL_TTL_SECS
        {
            requests.push(request);
        }
    }
    requests.sort_by_key(|r| r.at_unix);
    requests.into_iter().map(|r| r.session).collect()
}

/// The kick file's target, if the request is well-formed and fresh. Future
/// stamps count as fresh (wrap-safe).
pub fn parse_kick(raw: &str, now_unix: u64) -> Option<String> {
    let mut lines = raw.lines().map(str::trim);
    let target = lines.next()?.to_string();
    if target.is_empty() {
        return None;
    }
    let stamp = lines.next()?.parse::<u64>().ok()?;
    if now_unix.saturating_sub(stamp) > KICK_TTL_SECS {
        return None;
    }
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::paths::KICK_FILE;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-sess-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_mirror_older_than_five_seconds_is_not_live() {
        let raw = r#"{"updated_unix":100,"sessions":[{"device_id":"abc","peer":"10.0.0.2:5","transport":"lan","started_unix":90}]}"#;
        let mirror = parse_mirror(raw).unwrap();
        assert!(mirror.is_fresh(104) && !mirror.is_fresh(106));
        assert_eq!(mirror.sessions[0].device_id.as_deref(), Some("abc"));
        assert_eq!(mirror.sessions[0].transport, "lan");
    }

    #[test]
    fn a_linked_phone_takes_files_only_with_an_address_to_dial() {
        let mirror = parse_mirror(&render_mirror(
            &[MirrorSession {
                id: 1,
                device_id: Some("in-session".into()),
                peer: "192.0.2.7:9".into(),
                transport: "lan".into(),
                started_unix: 1,
                driving: false,
            }],
            false,
            &[
                LinkedPhone {
                    device_id: "at-home".into(),
                    files: true,
                },
                LinkedPhone {
                    device_id: "away".into(),
                    files: false,
                },
            ],
            2,
        ))
        .unwrap();
        assert!(mirror.files_reach("in-session") && mirror.files_reach("at-home"));
        assert!(!mirror.files_reach("away"));
        assert!(!mirror.files_reach("stranger"));
        // A mirror from before T10d has no `linked`.
        let old = parse_mirror(r#"{"updated_unix":1,"sessions":[]}"#).unwrap();
        assert!(old.linked.is_empty());
    }

    #[test]
    fn the_server_render_parses_back() {
        let rendered = render_mirror(
            &[MirrorSession {
                id: 3,
                device_id: None,
                peer: "1.2.3.4:9".into(),
                transport: "wan".into(),
                started_unix: 7,
                driving: true,
            }],
            true,
            &[],
            8,
        );
        let mirror = parse_mirror(&rendered).unwrap();
        assert_eq!(mirror.updated_unix, 8);
        assert!(mirror.shared_control);
        assert_eq!(mirror.sessions[0].device_id, None);
        assert_eq!(
            (mirror.sessions[0].id, mirror.sessions[0].driving),
            (3, true)
        );
    }

    #[test]
    fn a_mirror_from_before_n7_still_reads() {
        let raw = r#"{"updated_unix":1,"sessions":[{"device_id":null,"peer":"p","transport":"lan","started_unix":1}]}"#;
        let mirror = parse_mirror(raw).unwrap();
        assert!(!mirror.shared_control);
        let session = &mirror.sessions[0];
        assert_eq!((session.id, session.driving), (0, false));
    }

    #[test]
    fn control_requests_are_taken_once_each_and_expire() {
        let dir = temp_dir("control");
        request_control(&dir, 4, 1_000).unwrap();
        request_control(&dir, 7, 1_000).unwrap();
        request_control(&dir, 9, 900).unwrap();
        let mut taken = take_control_requests(&dir, 1_010);
        taken.sort_unstable();
        assert_eq!(taken, vec![4, 7], "the stale one is dropped");
        assert!(take_control_requests(&dir, 1_010).is_empty(), "taken once");
    }

    #[test]
    fn a_kick_request_parses_and_expires() {
        assert_eq!(parse_kick("all\n1000\n", 1_030).as_deref(), Some("all"));
        assert_eq!(parse_kick("all\n1000\n", 1_061), None);
        let dir = temp_dir("kick-empty");
        assert!(
            request_kick(&dir, "  ", 1).is_err(),
            "an empty target is refused"
        );
    }

    #[test]
    fn two_quick_kicks_are_both_taken_once() {
        let dir = temp_dir("kicks");
        request_kick(&dir, "phone-a", 1_000).unwrap();
        request_kick(&dir, "phone-b", 1_000).unwrap();
        request_kick(&dir, "stale", 900).unwrap();
        let mut taken = take_kicks(&dir, 1_010);
        taken.sort();
        assert_eq!(taken, vec!["phone-a", "phone-b"]);
        assert!(take_kicks(&dir, 1_010).is_empty(), "taken once");
    }

    #[test]
    fn a_kick_file_from_an_older_cli_is_still_taken() {
        let dir = temp_dir("kick-legacy");
        std::fs::write(dir.join(KICK_FILE), "all\n1000\n").unwrap();
        assert_eq!(take_kicks(&dir, 1_010), vec!["all"]);
        assert!(!dir.join(KICK_FILE).exists());
    }
}
