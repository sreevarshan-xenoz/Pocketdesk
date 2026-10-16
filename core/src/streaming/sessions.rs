//! R4 D2 — live streaming-session registry + kick.
//!
//! Every accepted video pipeline registers itself here for the life of the
//! session ([`register`], called from `run_pipeline`; the returned handle
//! deregisters on drop). This is the desktop's answer to RustDesk's session
//! tray: `pocketdesk status` lists what is watching right now, and
//! `pocketdesk kick <device>` tears a session down by closing its QUIC
//! connection — the pipeline's own teardown path does the rest, so capture,
//! encoder children and mic relays all die through the normal routes rather
//! than a special kill switch.
//!
//! The registry is process-global because sessions are spawned from two
//! independent accept loops (LAN quinn in `service.rs`, WAN iroh in
//! `iroh_endpoint.rs`) that share only this table.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::connection::{SharedConnection, TransportFamily};

/// What a streaming session was given when it started (F5). It keeps
/// these for its life, so the server ends it when the owner takes one away.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Streamed {
    /// Its input is injected (not view-only by permission).
    pub input: bool,
    /// Desktop sound is sent.
    pub audio: bool,
    /// The phone's mic is played.
    pub mic: bool,
    /// The screen is sent; false for the phone as a remote (T10i) and for a
    /// cast.
    pub video: bool,
    /// The phone shows its own screen here (T10k).
    pub cast: bool,
}

/// One currently-live streaming session, safe to render or serialize.
#[derive(Debug, Clone)]
pub struct LiveSession {
    /// Registry id — stable for the process lifetime, used for diffing.
    pub id: u64,
    /// What the session was given when it started.
    pub streamed: Streamed,
    /// The device id the client announced on its pre-pipeline stream
    /// (`None` for a client too old to announce; pairing gates reject
    /// those when enabled).
    pub device_id: Option<String>,
    pub peer: SocketAddr,
    /// true = quinn (LAN / Tailscale-routable), false = iroh WAN.
    pub lan: bool,
    pub started_unix: u64,
}

struct Entry {
    info: LiveSession,
    connection: SharedConnection,
}

#[derive(Default)]
struct Registry {
    sessions: HashMap<u64, Entry>,
}

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Register a freshly accepted session. The returned handle MUST be held
/// for the duration of the pipeline run; dropping it deregisters.
/// Only reachable from the capture-gated pipeline (`run_pipeline`), so it
/// is legitimately dead in client-only builds.
#[cfg_attr(not(feature = "capture"), allow(dead_code))]
pub(crate) fn register(
    device_id: Option<String>,
    connection: &SharedConnection,
    streamed: Streamed,
) -> SessionHandle {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let info = LiveSession {
        id,
        streamed,
        device_id,
        peer: connection.remote_address(),
        lan: matches!(connection.transport_family(), TransportFamily::Quinn),
        started_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    if let Ok(mut reg) = registry().lock() {
        reg.sessions.insert(
            id,
            Entry {
                info,
                connection: connection.clone(),
            },
        );
    }
    SessionHandle { id }
}

/// RAII deregistration — the session is live exactly as long as the
/// pipeline task holds this.
#[cfg_attr(not(feature = "capture"), allow(dead_code))]
pub(crate) struct SessionHandle {
    id: u64,
}

#[cfg_attr(not(feature = "capture"), allow(dead_code))]
impl SessionHandle {
    /// The session's registry id.
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        if let Ok(mut reg) = registry().lock() {
            reg.sessions.remove(&self.id);
        }
    }
}

/// All live sessions, oldest first.
pub fn list() -> Vec<LiveSession> {
    let mut out: Vec<LiveSession> = registry()
        .lock()
        .map(|reg| reg.sessions.values().map(|e| e.info.clone()).collect())
        .unwrap_or_default();
    out.sort_by_key(|s| (s.started_unix, s.id));
    out
}

/// Does `target` name this session? Accepted forms:
/// - `all` / `*` — every session;
/// - exact device id, or a unique-style prefix of at least 6 chars;
/// - peer IP (no port) — for clients that never announced an id.
pub fn matches(info: &LiveSession, target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() {
        return false;
    }
    if target.eq_ignore_ascii_case("all") || target == "*" {
        return true;
    }
    match &info.device_id {
        Some(id) => id == target || (target.len() >= 6 && id.starts_with(target)),
        None => info.peer.ip().to_string() == target,
    }
}

/// End the one session with registry id `id`, closing its connection with
/// `reason` (the phone shows it). Unlike [`kick`], which takes a target
/// string where `all` or an id prefix can name other devices' sessions,
/// this names exactly one.
pub fn end_session(id: u64, reason: &[u8]) -> Option<LiveSession> {
    let entry = registry().lock().ok()?.sessions.remove(&id)?;
    entry.connection.close(0u32, reason);
    Some(entry.info)
}

/// Close every session matching `target` and return what was kicked.
/// `close()` on an already-dying connection is a harmless no-op; the RAII
/// handles deregister as each pipeline exits.
pub fn kick(target: &str) -> Vec<LiveSession> {
    let victims: Vec<Entry> = if let Ok(mut reg) = registry().lock() {
        let ids: Vec<u64> = reg
            .sessions
            .iter()
            .filter(|(_, e)| matches(&e.info, target))
            .map(|(id, _)| *id)
            .collect();
        ids.into_iter()
            .filter_map(|id| reg.sessions.remove(&id))
            .collect()
    } else {
        Vec::new()
    };
    let mut kicked = Vec::with_capacity(victims.len());
    for entry in victims {
        entry.connection.close(0u32, b"kicked by user");
        kicked.push(entry.info);
    }
    kicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::connection::{Connection, ConnectionStats, InStream, OutStream};
    use async_trait::async_trait;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// Connection double: only the D2 surface (address, family, close) is
    /// real; stream methods are never called by the registry.
    struct DummyConnection {
        peer: SocketAddr,
        lan: bool,
        closed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Connection for DummyConnection {
        async fn open_uni(
            &self,
        ) -> Result<Box<dyn OutStream>, crate::streaming::connection::ConnectionError> {
            unimplemented!()
        }
        async fn accept_uni(
            &self,
        ) -> Result<Box<dyn InStream>, crate::streaming::connection::ConnectionError> {
            unimplemented!()
        }
        fn remote_address(&self) -> SocketAddr {
            self.peer
        }
        fn stats(&self) -> ConnectionStats {
            ConnectionStats {
                rtt: std::time::Duration::ZERO,
                ..Default::default()
            }
        }
        fn transport_family(&self) -> TransportFamily {
            if self.lan {
                TransportFamily::Quinn
            } else {
                TransportFamily::Iroh
            }
        }
        fn close(&self, _code: u32, _reason: &[u8]) {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    fn dummy(lan: bool, ip_last: u8) -> (SharedConnection, Arc<AtomicBool>) {
        let closed = Arc::new(AtomicBool::new(false));
        let conn: SharedConnection = Arc::new(DummyConnection {
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, ip_last)), 4433),
            lan,
            closed: closed.clone(),
        });
        (conn, closed)
    }

    /// The registry is process-global; every test that mutates it holds this
    /// lock for its whole body.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static TEST_LOCK: Mutex<()> = Mutex::new(());
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn register_list_and_deregister() {
        let _guard = test_lock();
        let before = list().len();
        let (conn, _closed) = dummy(true, 201);
        let handle = register(Some("devregister1".into()), &conn, Streamed::default());
        let live = list();
        assert!(
            live.iter()
                .any(|s| s.device_id.as_deref() == Some("devregister1") && s.lan)
        );
        drop(handle);
        assert_eq!(list().len(), before, "handle drop must deregister");
    }

    #[test]
    fn ending_a_session_by_id_touches_no_other() {
        let _guard = test_lock();
        let (short, short_closed) = dummy(true, 203);
        let (long, long_closed) = dummy(true, 204);
        let short_handle = register(Some("abcdef".into()), &short, Streamed::default());
        let _long_handle = register(Some("abcdef123".into()), &long, Streamed::default());
        let ended = end_session(short_handle.id(), b"not paired");
        assert_eq!(ended.map(|s| s.device_id), Some(Some("abcdef".to_string())));
        assert!(short_closed.load(Ordering::Relaxed));
        assert!(
            !long_closed.load(Ordering::Relaxed),
            "a device id another one starts with ends only its own session"
        );
        assert!(
            end_session(short_handle.id(), b"again").is_none(),
            "ended once"
        );
    }

    #[test]
    fn kick_matches_id_prefix_and_closes_connection() {
        let _guard = test_lock();
        let (conn, closed) = dummy(false, 202);
        let _handle = register(Some("abcdef123456".into()), &conn, Streamed::default());
        // < 6-char prefixes must not match anything (this run registers no
        // other sessions, so a hit on "abc" would be a false positive).
        assert!(kick("abc").is_empty());
        let kicked = kick("abcdef");
        assert_eq!(kicked.len(), 1);
        assert_eq!(kicked[0].device_id.as_deref(), Some("abcdef123456"));
        assert!(closed.load(Ordering::Relaxed), "kick must close the conn");
        assert!(
            list()
                .iter()
                .all(|s| s.device_id.as_deref() != Some("abcdef123456"))
        );
    }

    #[test]
    fn matches_rules() {
        let info = LiveSession {
            streamed: Streamed::default(),
            id: 1,
            device_id: Some("deadbeefcafe".into()),
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1),
            lan: true,
            started_unix: 0,
        };
        assert!(matches(&info, "all") || matches(&info, "*"));
        assert!(matches(&info, "deadbeefcafe"));
        assert!(matches(&info, "deadbe"));
        assert!(!matches(&info, "deadb"));
        assert!(!matches(&info, "cafe"));
        assert!(!matches(&info, ""));
        let anonymous = LiveSession {
            streamed: Streamed::default(),
            id: 2,
            device_id: None,
            peer: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 7)), 9),
            lan: false,
            started_unix: 0,
        };
        assert!(matches(&anonymous, "192.168.1.7"));
        assert!(!matches(&anonymous, "deadbeef"));
    }
}
