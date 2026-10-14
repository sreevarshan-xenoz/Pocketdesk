//! R4 D2 — desktop-side session visibility and kick.
//!
//! The truth about live sessions lives in the core registry
//! (`pocketdesk_core::streaming::sessions`); the CLI is a separate process,
//! so this module mirrors it into the state dir and consumes kick requests,
//! both through files — the same mechanism R3 #11b uses for pairing PINs
//! (no admin socket to attack or keep alive).
//!
//! - `live_sessions.json` — rewritten whenever the registry changes; read
//!   by `pocketdesk status` to show what is watching right now.
//! - `kick` — written by `pocketdesk kick <device>` (single line
//!   `<target>\n<unix-secs>`); consumed and deleted by the watcher within
//!   ~1 s, ignored if stale (> 60 s) so a leftover file can't kill a future
//!   session.
//!
//! Consent: each *new* session also raises a `notify-send` ("phone is
//! watching") — the GUI-daemon-free parity with RustDesk's confirm dialog.
//! Notifications are best-effort: headless sessions just log.

use std::path::PathBuf;
use std::time::Duration;

use pocketdesk_core::local::sessions::{self as shared, MirrorSession};
use pocketdesk_core::streaming::sessions::LiveSession;

/// A kick request older than this is dropped unexecuted — the operator
/// meant it for a session that is no longer (or not yet) live.
pub const KICK_TTL_SECS: u64 = shared::KICK_TTL_SECS;

fn state_file(name: &str) -> anyhow::Result<PathBuf> {
    Ok(crate::state::state_dir()?.join(name))
}

pub fn live_sessions_path() -> anyhow::Result<PathBuf> {
    state_file("live_sessions.json")
}

/// Parse the CLI-written kick file: `<target>\n<unix-secs>`. Returns the
/// target only if the request is well-formed and fresh (shared with the
/// desktop app: `core::local::sessions`).
pub fn parse_kick_file(raw: &str, now_unix: u64) -> Option<String> {
    shared::parse_kick(raw, now_unix)
}

/// Serialize the registry snapshot in the mirror format the CLI's `status`
/// and the desktop app read.
pub fn render_json(live: &[LiveSession], driver: Option<u64>, now_unix: u64) -> String {
    let sessions: Vec<MirrorSession> = live
        .iter()
        .map(|s| MirrorSession {
            id: s.id,
            driving: driver == Some(s.id),
            device_id: s.device_id.clone(),
            peer: s.peer.to_string(),
            transport: if s.lan { "lan" } else { "wan" }.to_string(),
            started_unix: s.started_unix,
        })
        .collect();
    shared::render_mirror(&sessions, now_unix)
}

/// Atomic mirror write: tmp + rename, so `status` never sees a torn file.
fn write_live_json(json: &str) -> anyhow::Result<()> {
    let path = live_sessions_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The phones in a session (session id, device id) and the driver, as
/// last told to them.
type Told = (Vec<(u64, String)>, Option<u64>);

/// Kick targets for the live sessions whose phone may no longer stream:
/// with pairing required, a device that is not (or no longer) paired, which
/// covers an unpair from the CLI or the app, an expired guest grant and a
/// hand-edited trust store; and a session that never said which device it
/// is, by its address. Nothing with pairing off.
pub fn untrusted_sessions(
    live: &[LiveSession],
    pairing_required: bool,
    is_paired: impl Fn(&str) -> bool,
) -> Vec<String> {
    if !pairing_required {
        return Vec::new();
    }
    live.iter()
        .filter_map(|s| match &s.device_id {
            Some(id) if is_paired(id) => None,
            Some(id) => Some(id.clone()),
            None => Some(s.peer.ip().to_string()),
        })
        .collect()
}

/// Tell each phone in a session who drives now (N7), on its control
/// connection.
async fn tell_phones(sessions: &[(u64, String)], driver: Option<u64>) {
    let clients = crate::state::clone_clients().await;
    for (device, packet) in crate::driving::control_bodies(sessions, driver) {
        if let Some(client) = clients.iter().find(|c| c.device_id() == device) {
            let _ = client.send_packet(&packet).await;
        }
    }
}

/// "phone is watching this desktop" — desktop notification, log fallback
/// (same delivery discipline as the pairing PIN notification).
fn notify_session_started(session: &LiveSession) {
    let who = match &session.device_id {
        Some(id) => id.clone(),
        None => session.peer.ip().to_string(),
    };
    let summary = "Pocketdesk: a device is streaming this desktop";
    let body = format!(
        "device {who} · {} · {peer}",
        if session.lan { "LAN" } else { "WAN" },
        peer = session.peer
    );
    tokio::task::spawn_blocking(move || {
        let ok = crate::platform::notify(summary, &body, std::time::Duration::from_secs(10));
        if !ok {
            tracing::info!("{summary} — {body}");
        }
    });
}

/// Daemon task: mirror the registry, ring the consent notification, and
/// consume kick requests. Runs for the process lifetime.
pub async fn run_watcher() {
    let mut prev_ids: Vec<u64> = Vec::new();
    let mut prev_json = String::new();
    // What the phones were last told about who drives (roadmap N7).
    let mut told: Option<Told> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 1. Kick requests (consume-then-act: a crash can't replay a kick),
        //    every one since the last tick.
        if let Ok(dir) = crate::state::state_dir() {
            for target in shared::take_kicks(&dir, now) {
                let kicked = pocketdesk_core::streaming::sessions::kick(&target);
                if kicked.is_empty() {
                    tracing::warn!(target = %target, "kick: no live session matched");
                } else {
                    for s in &kicked {
                        tracing::info!(
                            device_id = s.device_id.as_deref().unwrap_or("<unannounced>"),
                            peer = %s.peer,
                            "Kicked streaming session (pocketdesk kick)"
                        );
                    }
                }
            }
        }

        // 2. Consent notification for sessions that appeared since last tick.
        let live = pocketdesk_core::streaming::sessions::list();
        // A phone that stopped being paired loses its session at once,
        // however that happened.
        if !live.is_empty() {
            let store = crate::state::trust_store_path().ok().and_then(|path| {
                pocketdesk_core::protocol::kdeconnect::TrustStore::load_or_create(path).ok()
            });
            let paired = |id: &str| store.as_ref().is_some_and(|s| s.is_trusted(id));
            for target in untrusted_sessions(&live, crate::state::pairing_required(), paired) {
                for s in pocketdesk_core::streaming::sessions::kick(&target) {
                    tracing::warn!(
                        device_id = s.device_id.as_deref().unwrap_or("<unannounced>"),
                        peer = %s.peer,
                        "Ended a streaming session: its phone is no longer paired"
                    );
                }
            }
        }
        let live = pocketdesk_core::streaming::sessions::list();
        let live_ids: Vec<u64> = live.iter().map(|s| s.id).collect();

        // N7: the desktop app hands control to a session.
        if let Ok(dir) = crate::state::state_dir() {
            for session in shared::take_control_requests(&dir, now) {
                if live_ids.contains(&session) && crate::driving::give(session) {
                    tracing::info!(session, "Input control handed over by the desktop");
                }
            }
        }
        let driver = crate::driving::driver(&live_ids);
        let in_sessions: Vec<(u64, String)> = live
            .iter()
            .filter_map(|s| s.device_id.clone().map(|d| (s.id, d)))
            .collect();
        if told.as_ref() != Some(&(in_sessions.clone(), driver)) {
            tell_phones(&in_sessions, driver).await;
            told = Some((in_sessions, driver));
        }
        let ids: Vec<u64> = live.iter().map(|s| s.id).collect();
        for s in &live {
            if !prev_ids.contains(&s.id) {
                notify_session_started(s);
            }
        }
        prev_ids = ids;

        // 3. Mirror for `pocketdesk status`, rewritten only on change.
        let json = render_json(&live, driver, now);
        if json != prev_json {
            if let Err(e) = write_live_json(&json) {
                tracing::warn!(error = %e, "Failed to mirror live sessions file");
            } else {
                prev_json = json;
            }
        }
    }
}

/// CLI-side rendering of the mirrored file: one line per live session.
/// Returns `None` when the file is absent or unreadable (daemon not
/// running, or predates D2).
pub fn format_status_lines(raw: &str, now_unix: u64) -> Option<Vec<String>> {
    let mirror = shared::parse_mirror(raw)?;
    Some(
        mirror
            .sessions
            .iter()
            .map(|s| {
                let short = match &s.device_id {
                    Some(device) if device.len() > 12 => format!("{}…", &device[..12]),
                    Some(device) => device.clone(),
                    None => "<unannounced>".to_string(),
                };
                let age = now_unix
                    .checked_sub(s.started_unix)
                    .map(|secs| format!("{secs}s"))
                    .unwrap_or_else(|| "?".into());
                format!("{short}  {}  {}  up {age}", s.transport, s.peer)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::sessions::LiveSession;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn a_session_whose_phone_is_no_longer_paired_is_ended() {
        let live = vec![
            session(Some("paired-phone"), true),
            session(Some("unpaired-phone"), true),
            session(None, false),
        ];
        let paired = |id: &str| id == "paired-phone";
        let mut ended = untrusted_sessions(&live, true, paired);
        ended.sort();
        assert_eq!(
            ended,
            vec!["10.0.0.5".to_string(), "unpaired-phone".to_string()]
        );
    }

    #[test]
    fn with_pairing_off_no_session_is_ended_for_it() {
        let live = vec![session(Some("unpaired-phone"), true), session(None, false)];
        assert!(untrusted_sessions(&live, false, |_| false).is_empty());
    }

    fn session(device: Option<&str>, lan: bool) -> LiveSession {
        LiveSession {
            id: 7,
            device_id: device.map(str::to_string),
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 4433),
            lan,
            started_unix: 1_000,
        }
    }

    #[test]
    fn kick_file_fresh_stale_and_malformed() {
        assert_eq!(
            parse_kick_file("devabc\n5000\n", 5_010).as_deref(),
            Some("devabc")
        );
        // A future stamp is never stale (clock skew tolerance).
        assert_eq!(
            parse_kick_file("devabc\n5050\n", 5_000).as_deref(),
            Some("devabc")
        );
        assert_eq!(parse_kick_file("devabc\n4000\n", 5_000), None);
        assert_eq!(parse_kick_file("\n5000\n", 5_000), None);
        assert_eq!(parse_kick_file("devabc", 5_000), None);
        assert_eq!(parse_kick_file("devabc\nnope\n", 5_000), None);
    }

    #[test]
    fn json_roundtrips_through_status_formatter() {
        let live = vec![
            session(Some("abcdef0123456789"), true),
            session(None, false),
        ];
        let json = render_json(&live, None, 1_030);
        let lines = format_status_lines(&json, 1_030).expect("parseable");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("abcdef012345…"));
        assert!(lines[0].contains("lan") && lines[0].contains("up 30s"));
        assert!(lines[1].starts_with("<unannounced>"));
        assert!(lines[1].contains("wan"));
        // The mirror is machine-readable: same-file garbage must not panic.
        assert!(format_status_lines("not json", 1).is_none());
        assert!(format_status_lines(r#"{"sessions":{}}"#, 1).is_none());
    }
}
