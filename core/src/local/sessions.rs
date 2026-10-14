//! The live-session mirror and kick requests.
//!
//! The server mirrors its live streaming sessions to
//! `<state>/live_sessions.json` every second (atomically), and ends sessions
//! named in `<state>/kick` (`<target>\n<unix-secs>\n`). The CLI's `status` and
//! `kick` and the desktop app go through these files. The desktop hands input
//! control to a session through `<state>/control/` (roadmap N7), one file per
//! request.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::{CONTROL_DIR, KICK_FILE};

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

/// `live_sessions.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveMirror {
    pub updated_unix: u64,
    pub sessions: Vec<MirrorSession>,
}

impl LiveMirror {
    /// Whether the mirror was written recently enough to describe a running
    /// server (a crash leaves the last one behind).
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        now_unix.saturating_sub(self.updated_unix) <= FRESH_SECS
    }
}

/// The mirror's JSON, as the server writes it.
pub fn render_mirror(sessions: &[MirrorSession], now_unix: u64) -> String {
    serde_json::to_string(&LiveMirror {
        updated_unix: now_unix,
        sessions: sessions.to_vec(),
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
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(state_dir.join(KICK_FILE), format!("{target}\n{now_unix}\n"))
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
            8,
        );
        let mirror = parse_mirror(&rendered).unwrap();
        assert_eq!(mirror.updated_unix, 8);
        assert_eq!(mirror.sessions[0].device_id, None);
        assert_eq!(
            (mirror.sessions[0].id, mirror.sessions[0].driving),
            (3, true)
        );
    }

    #[test]
    fn a_mirror_from_before_n7_still_reads() {
        let raw = r#"{"updated_unix":1,"sessions":[{"device_id":null,"peer":"p","transport":"lan","started_unix":1}]}"#;
        let session = &parse_mirror(raw).unwrap().sessions[0];
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
    fn a_kick_request_round_trips_and_expires() {
        let dir = temp_dir("kick");
        request_kick(&dir, "all", 1_000).unwrap();
        let raw = std::fs::read_to_string(dir.join(KICK_FILE)).unwrap();
        assert_eq!(parse_kick(&raw, 1_030).as_deref(), Some("all"));
        assert_eq!(parse_kick(&raw, 1_061), None);
        assert!(
            request_kick(&dir, "  ", 1).is_err(),
            "an empty target is refused"
        );
    }
}
