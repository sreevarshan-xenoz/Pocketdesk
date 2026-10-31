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
pub fn render_json(
    live: &[LiveSession],
    driver: Option<u64>,
    linked: &[shared::LinkedPhone],
    now_unix: u64,
) -> String {
    let sessions: Vec<MirrorSession> = live
        .iter()
        .map(|s| MirrorSession {
            id: s.id,
            driving: driver == Some(s.id),
            device_id: s.device_id.clone(),
            peer: s.peer.to_string(),
            transport: if s.lan { "lan" } else { "wan" }.to_string(),
            started_unix: s.started_unix,
            // A remote gives input; a sound session (N5) none.
            control_only: !s.streamed.video && !s.streamed.cast && s.streamed.input,
            cast: s.streamed.cast,
            // The phone's sound alone (M7) gives no input either.
            sound_only: !s.streamed.video
                && !s.streamed.cast
                && !s.streamed.input
                && !s.streamed.phone_sound,
            phone_sound_only: !s.streamed.video && !s.streamed.cast && s.streamed.phone_sound,
        })
        .collect();
    shared::render_mirror(&sessions, crate::driving::is_shared(), linked, now_unix)
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

/// The live sessions given more at their start than their phone is now
/// allowed (F5: Input, Audio or Mic turned off while they run), by session
/// id. A session keeps what it started with, so it ends instead.
pub fn narrowed_sessions(
    live: &[LiveSession],
    permissions: impl Fn(&str) -> pocketdesk_core::protocol::permissions::DevicePermissions,
) -> Vec<u64> {
    use pocketdesk_core::protocol::permissions::Capability;
    live.iter()
        .filter(|s| {
            let Some(id) = s.device_id.as_deref() else {
                return false;
            };
            let now = permissions(id);
            (s.streamed.input && !now.allows(Capability::Input))
                || (s.streamed.audio && !now.allows(Capability::Audio))
                || (s.streamed.mic && !now.allows(Capability::Mic))
                // A cast's sound (K5) ends with its permission, as the mic.
                || (s.streamed.phone_sound && !now.allows(Capability::PhoneSound))
        })
        .map(|s| s.id)
        .collect()
}

/// The live sessions whose phone may no longer stream, by session id:
/// with pairing required, a device that is not (or no longer) paired, which
/// covers an unpair from the CLI or the app, an expired guest grant and a
/// hand-edited trust store; and a session that never said which device it
/// is. Nothing with pairing off. Session ids, not device ids: a device id
/// such as `all`, or one another phone's id starts with, would end other
/// phones' sessions as a kick target.
pub fn untrusted_sessions(
    live: &[LiveSession],
    pairing_required: bool,
    is_paired: impl Fn(&str) -> bool,
) -> Vec<u64> {
    if !pairing_required {
        return Vec::new();
    }
    live.iter()
        .filter(|s| !s.device_id.as_deref().is_some_and(&is_paired))
        .map(|s| s.id)
        .collect()
}

/// Phones are told again this often even when nothing changed: a phone
/// whose control connection reconnected mid-session, or whose last push was
/// lost, learns who drives within this long.
const RETELL: Duration = Duration::from_secs(5);

/// Whether to tell the phones who drives: on any change, and every
/// [`RETELL`] while phones are in sessions.
fn retell(told: Option<&Told>, current: &Told, since: Duration) -> bool {
    told != Some(current) || (!current.0.is_empty() && since >= RETELL)
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

/// What a new session is, for its notification.
fn session_summary(streamed: &pocketdesk_core::streaming::sessions::Streamed) -> &'static str {
    if streamed.cast {
        "Pocketdesk: a phone is showing its screen here"
    } else if streamed.phone_sound {
        // The phone's sound alone (M7).
        "Pocketdesk: a phone is playing its sound here"
    } else if streamed.video {
        "Pocketdesk: a device is streaming this desktop"
    } else if streamed.input {
        // The phone as a remote (T10i) sees nothing; it only gives input.
        "Pocketdesk: a device is controlling this desktop (no screen)"
    } else {
        // A sound session (N5): neither picture nor input.
        "Pocketdesk: a phone is this computer's speaker and mic"
    }
}

/// "phone is watching this desktop" — desktop notification, log fallback
/// (same delivery discipline as the pairing PIN notification).
fn notify_session_started(session: &LiveSession) {
    let who = match &session.device_id {
        Some(id) => id.clone(),
        None => session.peer.ip().to_string(),
    };
    let summary = session_summary(&session.streamed);
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

/// Ticks in a row the trust store may be unreadable before every session is
/// judged unpaired. One failed read (a writer replacing the file) must not
/// end every phone's session; a store that stays unreadable fails closed.
const UNREADABLE_TICKS: u32 = 3;

/// Whether this tick may judge sessions by pairing, given whether the store
/// was read, counting the unread ticks in a row.
fn may_judge_pairing(read: bool, unread_ticks: &mut u32) -> bool {
    *unread_ticks = if read {
        0
    } else {
        unread_ticks.saturating_add(1)
    };
    read || *unread_ticks >= UNREADABLE_TICKS
}

/// The mirror is written at least this often: its readers take one older
/// than 5 s for a server that has stopped, and its stamp is a whole second
/// taken at the top of a tick that may run long, so 2 s leaves them 3 s.
const MIRROR_HEARTBEAT: Duration = Duration::from_secs(2);
/// Daemon task: mirror the registry, ring the consent notification, and
/// consume kick requests. Runs for the process lifetime.
pub async fn run_watcher() {
    let mut unread_ticks = 0;
    let mut prev_ids: Vec<u64> = Vec::new();
    // What the last look saw, for the event log (ecosystem 1a).
    let mut seen_sessions: Vec<(u64, Option<String>)> = Vec::new();
    let mut seen_linked: Vec<String> = Vec::new();
    // The paired phones as last read; unknown until the first read, which
    // is no change (a restart pairs no one).
    let mut seen_paired: Option<Vec<String>> = None;
    // When the trust store was last changed, as last read: it is read again
    // only once that moves.
    let mut seen_trust_change: Option<std::time::SystemTime> = None;
    // When the soonest guest grant then read ends: it is read again then
    // too, as the grant's end changes no file (AY4).
    let mut seen_grant_end: Option<u64> = None;
    // Whether the paired list last read still has unpaired phones' notes,
    // cards, texts or contacts to forget (AY5): tried each tick until done.
    let mut forget_due = false;
    // What the phones were last told about who drives (roadmap N7).
    let mut told: Option<Told> = None;
    let mut told_at = std::time::Instant::now();
    // The mirror is written when it changes, and at least every
    // MIRROR_HEARTBEAT so its readers (5 s freshness) see the server alive;
    // the kick and control folders are read when the watcher saw them
    // change, and every inbox SWEEP anyway (R6c).
    let mut prev_content = String::new();
    let mut last_write = std::time::Instant::now() - MIRROR_HEARTBEAT;
    let mut last_sweep = std::time::Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let sweep = last_sweep.elapsed() >= crate::inbox::SWEEP;
        if sweep {
            last_sweep = std::time::Instant::now();
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // 1. Kick requests (consume-then-act: a crash can't replay a kick),
        //    every one since the last tick.
        if let Ok(dir) = crate::state::state_dir()
            && (crate::inbox::changed(pocketdesk_core::local::paths::KICKS_DIR) || sweep)
        {
            // A device's own kick (AY3): its sessions alone.
            for device in shared::take_device_kicks(&dir, now) {
                for s in pocketdesk_core::streaming::sessions::kick_device(&device) {
                    tracing::info!(
                        device_id = s.device_id.as_deref().unwrap_or("<unannounced>"),
                        peer = %s.peer,
                        "Ended the device's streaming session (the desktop app)"
                    );
                }
            }
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
        // The privacy grab of a phone no longer paired is given back too,
        // with or without a stream (the store is read only while one is held).
        if crate::state::pairing_required() {
            crate::plugins::privacy::release_if_owner(|id| {
                !crate::plugins::pair::is_paired_device(id)
            });
        }
        // A computer's grab ends as the session here is unlocked (PR #572
        // review I1): Ctrl+Alt+Del and unlock always give the keys back.
        // Releasing blocks (Windows joins its hook thread): off the
        // runtime's workers (PR #572 re-review N8).
        let _ = tokio::task::spawn_blocking(crate::plugins::privacy::unlock_tick).await;
        if !live.is_empty() {
            let store = crate::state::trust_store_path().ok().and_then(|path| {
                pocketdesk_core::protocol::kdeconnect::TrustStore::load_or_create(path).ok()
            });
            let paired = |id: &str| store.as_ref().is_some_and(|s| s.is_trusted(id));
            if let Some(store) = &store {
                for id in narrowed_sessions(&live, |device| store.permissions(device)) {
                    if let Some(s) = pocketdesk_core::streaming::sessions::end_session(
                        id,
                        b"permissions changed",
                    ) {
                        tracing::warn!(
                            device_id = s.device_id.as_deref().unwrap_or(""),
                            "Ended a streaming session: its permissions were narrowed"
                        );
                    }
                }
            }
            let unpaired = if may_judge_pairing(store.is_some(), &mut unread_ticks) {
                untrusted_sessions(&live, crate::state::pairing_required(), paired)
            } else {
                tracing::debug!("The trust store could not be read; sessions judged next tick");
                Vec::new()
            };
            for id in unpaired {
                if let Some(s) =
                    pocketdesk_core::streaming::sessions::end_session(id, b"not paired")
                {
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
        if let Ok(dir) = crate::state::state_dir()
            && (crate::inbox::changed(pocketdesk_core::local::paths::CONTROL_DIR) || sweep)
        {
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
        let current = (in_sessions.clone(), driver);
        if retell(told.as_ref(), &current, told_at.elapsed()) {
            told_at = std::time::Instant::now();
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

        // 3. Mirror for `pocketdesk status`, rewritten only on change. It
        //    also names the phones with a control link (T10d).
        let linked = linked_phones(&crate::state::clone_clients().await);
        for (kind, device, meta) in changes(&seen_sessions, &live, &seen_linked, &linked) {
            crate::events::emit(kind, device.as_deref(), meta, serde_json::json!({}));
        }
        seen_sessions = live.iter().map(|s| (s.id, s.device_id.clone())).collect();
        seen_linked = linked.iter().map(|l| l.device_id.clone()).collect();
        // Paired or unpaired, however it was done: the PIN, the desktop app,
        // `pocketdesk unpair` (ecosystem 1b). An unreadable store is no change.
        let trust_path = crate::state::trust_store_path().ok();
        let trust_change = trust_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok());
        let changed = paired_list_due(
            seen_paired.is_none(),
            trust_change != seen_trust_change,
            seen_grant_end,
            now,
        );
        if changed
            && let Some((paired, grant_end)) = trust_path.and_then(|path| {
                pocketdesk_core::protocol::kdeconnect::TrustStore::read(path)
                    .ok()
                    .map(|store| (store.trusted_devices(), store.next_grant_end()))
            })
        {
            seen_trust_change = trust_change;
            seen_grant_end = grant_end;
            for (kind, device) in pairing_changes(seen_paired.as_deref(), &paired) {
                crate::events::emit(
                    kind,
                    Some(&device),
                    serde_json::json!({}),
                    serde_json::json!({}),
                );
            }
            // Ecosystem phase 4: a phone introduced between this computer
            // and another, removed here however it went, is owed there.
            if let (Ok(dir), Ok(trust)) =
                (crate::state::state_dir(), crate::state::trust_store_path())
            {
                let _ = tokio::task::spawn_blocking(move || {
                    crate::introductions::owe_removals(&dir, &trust)
                })
                .await;
            }
            forget_due = true;
            seen_paired = Some(paired);
        }
        // The removals owed, sent to the computers linked now.
        if sweep && let Ok(dir) = crate::state::state_dir() {
            crate::introductions::send_owed(&dir).await;
        }
        // An unpaired phone's notifications, live cards, texts and contacts
        // go with it (AY5); with pairing off it is still let in.
        if forget_due && let Some(paired) = &seen_paired {
            forget_due = crate::state::pairing_required()
                && match crate::state::state_dir() {
                    Ok(dir) => !forget_unpaired(&dir, paired).await,
                    Err(_) => false,
                };
        }
        // What it says, without the time it was written.
        let content = render_json(&live, driver, &linked, 0);
        if content != prev_content || last_write.elapsed() >= MIRROR_HEARTBEAT {
            let json = render_json(&live, driver, &linked, now);
            if let Err(e) = write_live_json(&json) {
                tracing::warn!(error = %e, "Failed to mirror live sessions file");
            } else {
                prev_content = content;
                last_write = std::time::Instant::now();
            }
        }
    }
}

/// The events between the last look (`seen_*`) and this one: sessions that
/// started or ended, phones whose control link came up or went.
fn changes(
    seen_sessions: &[(u64, Option<String>)],
    live: &[LiveSession],
    seen_linked: &[String],
    linked: &[shared::LinkedPhone],
) -> Vec<(&'static str, Option<String>, serde_json::Value)> {
    let mut out = Vec::new();
    for s in live {
        if !seen_sessions.iter().any(|(id, _)| *id == s.id) {
            let mode = if s.streamed.cast {
                "cast"
            } else if s.streamed.phone_sound {
                // The phone's sound alone (M7).
                "phone_sound"
            } else if s.streamed.video {
                "screen"
            } else if s.streamed.input {
                "remote"
            } else {
                // The phone as this computer's speaker and mic (N5).
                "sound"
            };
            out.push((
                "session_started",
                s.device_id.clone(),
                serde_json::json!({
                    "session_id": s.id,
                    "transport": if s.lan { "lan" } else { "wan" },
                    "mode": mode,
                }),
            ));
        }
    }
    for (id, device) in seen_sessions {
        if !live.iter().any(|s| s.id == *id) {
            out.push((
                "session_ended",
                device.clone(),
                serde_json::json!({ "session_id": id }),
            ));
        }
    }
    for phone in linked {
        if !seen_linked.contains(&phone.device_id) {
            out.push((
                "phone_linked",
                Some(phone.device_id.clone()),
                serde_json::json!({}),
            ));
        }
    }
    for device in seen_linked {
        if !linked.iter().any(|l| &l.device_id == device) {
            out.push((
                "phone_unlinked",
                Some(device.clone()),
                serde_json::json!({}),
            ));
        }
    }
    out
}

/// Keep only the `paired` phones' notifications the computer can answer,
/// live cards, texts, replies waiting and contacts (AY5: an unpaired phone's
/// stayed on the Notifications and Messages pages until the server
/// restarted, its texts for good); whether all of it was done. A file that
/// cannot be read is left as it is.
async fn forget_unpaired(dir: &std::path::Path, paired: &[String]) -> bool {
    use pocketdesk_core::local::{phone_contacts, phone_live, phone_notes};
    let mut done = true;
    if let Err(e) = phone_notes::keep_only(dir, paired) {
        tracing::warn!("Could not forget the notifications of phones no longer paired: {e}");
        done = false;
    }
    if let Err(e) = phone_live::keep_only(dir, paired) {
        tracing::warn!("Could not forget the live cards of phones no longer paired: {e}");
        done = false;
    }
    done &= crate::plugins::sms::keep_only(dir, paired).await;
    // From this list, not the start's prune, which forgets every phone's
    // when its own read of the store fails (review).
    for id in phone_contacts::read(dir).into_keys() {
        if !paired.contains(&id)
            && let Err(e) = phone_contacts::forget(dir, &id)
        {
            tracing::warn!("Could not forget a phone's contacts: {e}");
            done = false;
        }
    }
    done
}

/// Whether the paired list is read again: the first look, the trust store
/// changed, or the soonest guest grant then read has ended by `now` (AY4: it
/// ends without a write, so its phone_unpaired waited for some other one).
fn paired_list_due(first: bool, file_changed: bool, grant_end: Option<u64>, now: u64) -> bool {
    first || file_changed || grant_end.is_some_and(|end| now >= end)
}

/// The phones paired or unpaired since the last read (`None`: the first,
/// which changes nothing).
fn pairing_changes(seen: Option<&[String]>, paired: &[String]) -> Vec<(&'static str, String)> {
    let Some(seen) = seen else {
        return Vec::new();
    };
    let added = paired
        .iter()
        .filter(|id| !seen.contains(id))
        .map(|id| ("phone_paired", id.clone()));
    let removed = seen
        .iter()
        .filter(|id| !paired.contains(id))
        .map(|id| ("phone_unpaired", id.clone()));
    added.chain(removed).collect()
}

/// Each phone with a control link, and whether files can reach it: its link
/// has an address to dial back (T10d), or came by the internet road with
/// `internet_files` on (2c).
pub fn linked_phones(
    clients: &[std::sync::Arc<dyn pocketdesk_core::protocol::kdeconnect::DeviceSender>],
) -> Vec<shared::LinkedPhone> {
    // One connection per device: the one pushes go to.
    clients
        .iter()
        .map(|client| shared::LinkedPhone {
            device_id: client.device_id().to_string(),
            files: crate::plugins::share::files_reach(client.as_ref()),
        })
        .collect()
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
                let mode = if s.cast {
                    "  showing its screen"
                } else if s.phone_sound_only {
                    "  playing its sound"
                } else if s.control_only {
                    "  control only"
                } else if s.sound_only {
                    "  sound only"
                } else {
                    ""
                };
                format!("{short}  {}  {}  up {age}{mode}", s.transport, s.peer)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    /// AY5: an unpaired phone's notifications, live cards, texts and
    /// replies waiting leave the computer's pages; a paired one's stay.
    #[tokio::test]
    async fn an_unpaired_phone_s_notes_cards_and_texts_go() {
        use pocketdesk_core::local::{phone_live, phone_notes, sms};
        let dir = std::env::temp_dir().join(format!("pd-ay5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for phone in ["a", "b"] {
            let note = serde_json::json!({"device_id": phone, "key": "k", "app": "Chat",
                "title": "Ann", "text": "Lunch?", "at_unix": 1});
            phone_notes::keep(&dir, serde_json::from_value(note).unwrap()).unwrap();
            let card = serde_json::json!({"device_id": phone, "tag": "t", "app": "Files",
                "title": "Downloading", "text": "movie.mp4", "updated_unix": 1,
                "live": {"progress": 1, "max": 2, "indeterminate": false}});
            phone_live::put(&dir, serde_json::from_value(card).unwrap()).unwrap();
            let text = serde_json::json!({"device_id": phone, "address": "+1", "body": "hi",
                "at_unix": 1, "direction": "received"});
            sms::record(&dir, serde_json::from_value(text).unwrap(), sms::KEEP).unwrap();
            sms::queue_reply(&dir, phone, "+1", "ok", 1).unwrap();
        }
        assert!(super::forget_unpaired(&dir, &["a".to_string()]).await);
        let only_a = |ids: Vec<String>| ids == vec!["a".to_string()];
        assert!(only_a(
            phone_notes::read(&dir)
                .into_iter()
                .map(|n| n.device_id)
                .collect()
        ));
        assert!(only_a(
            phone_live::read(&dir)
                .into_iter()
                .map(|c| c.device_id)
                .collect()
        ));
        assert!(only_a(
            sms::read(&dir).into_iter().map(|m| m.device_id).collect()
        ));
        assert!(only_a(
            sms::pending(&dir)
                .into_iter()
                .map(|r| r.device_id)
                .collect()
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_paired_list_is_read_again_when_a_grant_ends() {
        // AY4: an ended guest grant fired no phone_unpaired until something
        // else wrote the trust store.
        use super::paired_list_due;
        assert!(paired_list_due(true, false, None, 100), "the first look");
        assert!(paired_list_due(false, true, None, 100), "the file changed");
        assert!(!paired_list_due(false, false, None, 100));
        assert!(!paired_list_due(false, false, Some(150), 100), "not yet");
        assert!(
            paired_list_due(false, false, Some(100), 100),
            "a grant ended"
        );
    }

    use super::*;
    use pocketdesk_core::streaming::sessions::LiveSession;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn each_kind_of_session_is_announced_as_what_it_is() {
        use pocketdesk_core::streaming::sessions::Streamed;
        let screen = Streamed {
            input: true,
            audio: true,
            mic: true,
            video: true,
            cast: false,
            phone_sound: false,
        };
        assert!(session_summary(&screen).contains("streaming this desktop"));
        let remote = Streamed {
            video: false,
            audio: false,
            mic: false,
            ..screen
        };
        assert!(session_summary(&remote).contains("controlling this desktop"));
        let cast = Streamed {
            cast: true,
            input: false,
            ..remote
        };
        assert!(session_summary(&cast).contains("showing its screen"));
        // A sound session (N5): no picture and no input.
        let sound = Streamed {
            input: false,
            video: false,
            ..screen
        };
        assert!(session_summary(&sound).contains("speaker and mic"));
    }

    #[test]
    fn phones_are_told_on_a_change_and_again_every_few_seconds() {
        let phones: Told = (vec![(1, "phone-a".to_string())], Some(1));
        let none: Told = (Vec::new(), None);
        assert!(retell(None, &phones, Duration::ZERO), "first time");
        assert!(!retell(Some(&phones), &phones, Duration::from_secs(1)));
        assert!(
            retell(Some(&phones), &phones, RETELL),
            "again after a while"
        );
        let moved: Told = (vec![(1, "phone-a".to_string())], None);
        assert!(retell(Some(&phones), &moved, Duration::ZERO), "on a change");
        assert!(!retell(Some(&none), &none, RETELL), "no one to tell");
    }

    #[test]
    fn a_session_whose_phone_is_no_longer_paired_is_ended() {
        let mut live = vec![
            session(Some("paired-phone"), true),
            session(Some("unpaired-phone"), true),
            session(None, false),
            // An id that is a kick keyword, or a prefix of another phone's,
            // still names only its own session.
            session(Some("all"), true),
            session(Some("paired"), true),
        ];
        for (i, s) in live.iter_mut().enumerate() {
            s.id = i as u64 + 1;
        }
        let paired = |id: &str| id == "paired-phone";
        let mut ended = untrusted_sessions(&live, true, paired);
        ended.sort();
        assert_eq!(ended, vec![2, 3, 4, 5]);
    }

    #[test]
    fn one_unreadable_store_does_not_end_every_session_but_a_lasting_one_does() {
        let mut unread = 0;
        assert!(may_judge_pairing(true, &mut unread));
        assert!(
            !may_judge_pairing(false, &mut unread),
            "a writer mid-rename"
        );
        assert!(!may_judge_pairing(false, &mut unread));
        assert!(
            may_judge_pairing(false, &mut unread),
            "still unreadable: fail closed"
        );
        assert!(may_judge_pairing(true, &mut unread));
        assert!(
            !may_judge_pairing(false, &mut unread),
            "the count starts over"
        );
    }

    #[test]
    fn with_pairing_off_no_session_is_ended_for_it() {
        let live = vec![session(Some("unpaired-phone"), true), session(None, false)];
        assert!(untrusted_sessions(&live, false, |_| false).is_empty());
    }

    #[test]
    fn a_session_whose_permissions_were_narrowed_is_ended() {
        use pocketdesk_core::protocol::permissions::DevicePermissions;
        use pocketdesk_core::streaming::sessions::Streamed;
        let all = Streamed {
            input: true,
            audio: true,
            mic: true,
            video: true,
            cast: false,
            phone_sound: false,
        };
        let view_only = Streamed {
            input: false,
            ..all
        };
        let mut live = vec![
            session(Some("input-off"), true),
            session(Some("untouched"), true),
            session(Some("was-view-only"), true),
            session(None, false),
        ];
        for (i, s) in live.iter_mut().enumerate() {
            s.id = i as u64 + 1;
            s.streamed = all;
        }
        live[2].streamed = view_only;
        let permissions = |id: &str| DevicePermissions {
            input: Some(id == "untouched"),
            ..DevicePermissions::default()
        };
        assert_eq!(narrowed_sessions(&live, permissions), vec![1]);
    }

    /// K5: a cast whose sound plays here ends when its permission goes.
    #[test]
    fn a_cast_s_sound_ends_with_its_permission() {
        use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
        use pocketdesk_core::streaming::sessions::Streamed;
        let mut cast = session(Some("phone"), true);
        cast.streamed = Streamed {
            cast: true,
            phone_sound: true,
            ..Streamed::default()
        };
        let mut on = DevicePermissions::default();
        on.set(Capability::PhoneSound, true);
        assert!(narrowed_sessions(std::slice::from_ref(&cast), |_| on.clone()).is_empty());
        assert_eq!(
            narrowed_sessions(&[cast], |_| DevicePermissions::default()),
            vec![7],
            "off by default"
        );
    }

    /// M7: the phone's sound alone, told as such, and ended when its
    /// permission goes.
    #[test]
    fn the_phone_s_sound_alone_is_told_and_ends_with_its_permission() {
        use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
        use pocketdesk_core::streaming::sessions::Streamed;
        let alone = Streamed {
            phone_sound: true,
            ..Streamed::default()
        };
        assert_eq!(
            session_summary(&alone),
            "Pocketdesk: a phone is playing its sound here"
        );
        let mut session = session(Some("phone"), true);
        session.streamed = alone;
        let mut on = DevicePermissions::default();
        on.set(Capability::PhoneSound, true);
        assert!(narrowed_sessions(std::slice::from_ref(&session), |_| on.clone()).is_empty());
        assert_eq!(
            narrowed_sessions(&[session], |_| DevicePermissions::default()),
            vec![7]
        );
    }

    /// M7: the phone's sound alone is its own kind, not sound only.
    #[test]
    fn the_phone_s_sound_alone_is_its_own_kind() {
        use pocketdesk_core::streaming::sessions::Streamed;
        let mut alone = session(Some("phone"), true);
        alone.streamed = Streamed {
            phone_sound: true,
            ..Streamed::default()
        };
        let mirror: serde_json::Value =
            serde_json::from_str(&render_json(std::slice::from_ref(&alone), None, &[], 100))
                .unwrap();
        let row = &mirror["sessions"][0];
        assert_eq!(row["phone_sound_only"], true);
        assert_eq!(row["sound_only"], false);
        assert_eq!(row["cast"], false);
        let got = changes(&[], &[alone], &[], &[]);
        assert_eq!(got[0].2["mode"], "phone_sound");
    }

    fn session(device: Option<&str>, lan: bool) -> LiveSession {
        LiveSession {
            streamed: Default::default(),
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
        let mut live = vec![
            session(Some("abcdef0123456789"), true),
            session(None, false),
            session(Some("casting"), true),
            session(Some("speaker"), true),
        ];
        live[0].streamed.video = true;
        live[1].streamed.input = true;
        live[2].streamed.cast = true;
        live[3].streamed.audio = true;
        let json = render_json(&live, None, &[], 1_030);
        let lines = format_status_lines(&json, 1_030).expect("parseable");
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("abcdef012345…"));
        assert!(lines[0].contains("lan") && lines[0].contains("up 30s"));
        assert!(!lines[0].contains("control only"));
        assert!(lines[1].starts_with("<unannounced>"));
        assert!(lines[1].contains("wan"));
        // The phone as a remote (T10i): no screen sent.
        assert!(lines[1].ends_with("control only"));
        // A cast (T10k): the phone's screen shown here, not a remote.
        assert!(lines[2].ends_with("showing its screen"));
        assert!(!lines[2].contains("control only"));
        // The phone as this computer's speaker and mic (N5): no input.
        assert!(lines[3].ends_with("sound only"));
        // The mirror is machine-readable: same-file garbage must not panic.
        assert!(format_status_lines("not json", 1).is_none());
        assert!(format_status_lines(r#"{"sessions":{}}"#, 1).is_none());
    }

    #[test]
    fn sessions_and_links_that_came_or_went_are_events() {
        let mut cast = session(Some("phone-a"), true);
        cast.id = 1;
        cast.streamed.cast = true;
        let mut screen = session(Some("phone-b"), false);
        screen.id = 2;
        screen.streamed.video = true;
        let link = |id: &str| shared::LinkedPhone {
            device_id: id.into(),
            files: true,
        };
        let seen = vec![
            (2, Some("phone-b".to_string())),
            (9, Some("phone-c".to_string())),
        ];
        let got = changes(
            &seen,
            &[cast, screen],
            &["phone-c".into()],
            &[link("phone-a")],
        );
        let kinds: Vec<_> = got
            .iter()
            .map(|(k, d, _)| (*k, d.clone().unwrap_or_default()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("session_started", "phone-a".to_string()),
                ("session_ended", "phone-c".to_string()),
                ("phone_linked", "phone-a".to_string()),
                ("phone_unlinked", "phone-c".to_string()),
            ]
        );
        assert_eq!(got[0].2["mode"], "cast");
        assert_eq!(got[0].2["transport"], "lan");
        assert_eq!(got[1].2["session_id"], 9);
        assert!(
            changes(&seen[..1], &[], &[], &[])
                .iter()
                .all(|(k, _, _)| *k == "session_ended")
        );
    }

    #[test]
    fn pairings_that_came_or_went_are_events_but_not_at_start() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(
            pairing_changes(None, &ids(&["a", "b"])).is_empty(),
            "a restart pairs no one"
        );
        assert_eq!(
            pairing_changes(Some(&ids(&["a", "b"])), &ids(&["b", "c"])),
            vec![
                ("phone_paired", "c".to_string()),
                ("phone_unpaired", "a".to_string())
            ]
        );
    }
}
