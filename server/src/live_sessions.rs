//! R4 D2 — desktop-side session visibility and kick.
//!
//! The truth about live sessions lives in the core registry
//! (`linux_link_core::streaming::sessions`); the CLI is a separate process,
//! so this module mirrors it into the state dir and consumes kick requests,
//! both through files — the same mechanism R3 #11b uses for pairing PINs
//! (no admin socket to attack or keep alive).
//!
//! - `live_sessions.json` — rewritten whenever the registry changes; read
//!   by `linux-link status` to show what is watching right now.
//! - `kick` — written by `linux-link kick <device>` (single line
//!   `<target>\n<unix-secs>`); consumed and deleted by the watcher within
//!   ~1 s, ignored if stale (> 60 s) so a leftover file can't kill a future
//!   session.
//!
//! Consent: each *new* session also raises a `notify-send` ("phone is
//! watching") — the GUI-daemon-free parity with RustDesk's confirm dialog.
//! Notifications are best-effort: headless sessions just log.

use std::path::PathBuf;
use std::time::Duration;

use linux_link_core::streaming::sessions::LiveSession;

/// A kick request older than this is dropped unexecuted — the operator
/// meant it for a session that is no longer (or not yet) live.
pub const KICK_TTL_SECS: u64 = 60;

fn state_file(name: &str) -> anyhow::Result<PathBuf> {
    Ok(crate::state::state_dir()?.join(name))
}

pub fn live_sessions_path() -> anyhow::Result<PathBuf> {
    state_file("live_sessions.json")
}

pub fn kick_file_path() -> anyhow::Result<PathBuf> {
    state_file("kick")
}

/// Parse the CLI-written kick file: `<target>\n<unix-secs>`. Returns the
/// target only if the request is well-formed and fresh.
pub fn parse_kick_file(raw: &str, now_unix: u64) -> Option<String> {
    let mut lines = raw.lines().map(str::trim);
    let target = lines.next()?.to_string();
    if target.is_empty() {
        return None;
    }
    let stamp = lines.next()?.parse::<u64>().ok()?;
    // Wrap-safe freshness check: future stamps are treated as fresh.
    let age = now_unix.saturating_sub(stamp);
    if age > KICK_TTL_SECS {
        return None;
    }
    Some(target)
}

/// Serialize the registry snapshot. Hand-rolled JSON via serde_json keeps
/// the schema exactly what `status` prints — array of session objects with
/// an `updated_unix` envelope.
pub fn render_json(live: &[LiveSession], now_unix: u64) -> String {
    let items: Vec<serde_json::Value> = live
        .iter()
        .map(|s| {
            serde_json::json!({
                "device_id": s.device_id,
                "peer": s.peer.to_string(),
                "transport": if s.lan { "lan" } else { "wan" },
                "started_unix": s.started_unix,
            })
        })
        .collect();
    serde_json::json!({ "updated_unix": now_unix, "sessions": items }).to_string()
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
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 1. Kick requests (consume-then-act: a crash can't replay a kick).
        if let Ok(path) = kick_file_path()
            && let Ok(raw) = std::fs::read_to_string(&path)
        {
            let _ = std::fs::remove_file(&path);
            match parse_kick_file(&raw, now) {
                None => tracing::warn!("Ignoring stale/malformed kick request file"),
                Some(target) => {
                    let kicked = linux_link_core::streaming::sessions::kick(&target);
                    if kicked.is_empty() {
                        tracing::warn!(target = %target, "kick: no live session matched");
                    } else {
                        for s in &kicked {
                            tracing::info!(
                                device_id = s.device_id.as_deref().unwrap_or("<unannounced>"),
                                peer = %s.peer,
                                "Kicked streaming session (linux-link kick)"
                            );
                        }
                    }
                }
            }
        }

        // 2. Consent notification for sessions that appeared since last tick.
        let live = linux_link_core::streaming::sessions::list();
        let ids: Vec<u64> = live.iter().map(|s| s.id).collect();
        for s in &live {
            if !prev_ids.contains(&s.id) {
                notify_session_started(s);
            }
        }
        prev_ids = ids;

        // 3. Mirror for `linux-link status`, rewritten only on change.
        let json = render_json(&live, now);
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
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let items = value.get("sessions")?.as_array()?;
    let mut out = Vec::new();
    for s in items {
        let short = match s.get("device_id").and_then(|v| v.as_str()) {
            Some(device) if device.len() > 12 => format!("{}…", &device[..12]),
            Some(device) => device.to_string(),
            None => "<unannounced>".to_string(),
        };
        let transport = s.get("transport").and_then(|v| v.as_str()).unwrap_or("?");
        let peer = s.get("peer").and_then(|v| v.as_str()).unwrap_or("?");
        let age = s
            .get("started_unix")
            .and_then(|v| v.as_u64())
            .and_then(|started| now_unix.checked_sub(started))
            .map(|secs| format!("{secs}s"))
            .unwrap_or_else(|| "?".into());
        out.push(format!("{short}  {transport}  {peer}  up {age}"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::streaming::sessions::LiveSession;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

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
        let json = render_json(&live, 1_030);
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
