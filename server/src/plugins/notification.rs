//! Notifications from a phone shown on this desktop (roadmap N5a), and the
//! answer to a phone asking whether this desktop takes them.
//!
//! A phone that shares its notifications may send many at once; at most
//! `notifications_per_minute` (config, default [`PER_MINUTE`]) a minute from
//! one phone are shown, the rest dropped. `phone_notifications = false` shows
//! none (a phone's cancels still close what is shown).

use pocketdesk_core::local::phone_notes::{self, Answer, AnswerRequest, PhoneNote};
use pocketdesk_core::protocol::notification_action;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// The most notifications shown from one phone in a minute, unless config
/// says otherwise.
const PER_MINUTE: usize = 20;

static SHOW_PHONES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(PER_MINUTE);

/// Apply config.toml's `phone_notifications` and `notifications_per_minute`
/// (read once, at server start).
pub fn configure(show_phones: bool, per_minute: u32) {
    use std::sync::atomic::Ordering;
    SHOW_PHONES.store(show_phones, Ordering::Relaxed);
    LIMIT.store((per_minute as usize).max(1), Ordering::Relaxed);
}

/// How many notifications each phone had shown in the last minute.
#[derive(Default)]
struct FloodGuard {
    shown: HashMap<String, Vec<Instant>>,
    /// When a drop was last logged, per phone, so a flood logs once a minute.
    logged: HashMap<String, Instant>,
}

impl FloodGuard {
    /// Whether one more notification from `phone` may be shown at `now`.
    fn admit(&mut self, phone: &str, now: Instant, limit: usize) -> bool {
        let recent = self.shown.entry(phone.to_string()).or_default();
        recent.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
        if recent.len() < limit {
            recent.push(now);
            return true;
        }
        let quiet = self
            .logged
            .get(phone)
            .is_none_or(|at| now.duration_since(*at) >= Duration::from_secs(60));
        if quiet {
            self.logged.insert(phone.to_string(), now);
            tracing::info!("Notifications from {phone} dropped: more than {limit} a minute");
        }
        false
    }
}

/// Whether one more notification from `phone` may be shown now, under the
/// same per-phone limit as its notifications (texts use it too, N5b).
pub(crate) fn admit(phone: &str) -> bool {
    let limit = LIMIT.load(std::sync::atomic::Ordering::Relaxed);
    flood_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .admit(phone, Instant::now(), limit)
}

fn flood_guard() -> &'static Mutex<FloodGuard> {
    static GUARD: std::sync::OnceLock<Mutex<FloodGuard>> = std::sync::OnceLock::new();
    GUARD.get_or_init(Default::default)
}

/// How a phone's notification is known on this desktop (N5c): 16 hex digits
/// of the SHA-256 of the phone's id and the notification's, so one phone's
/// cancel can never close another's.
fn tag_for(phone: &str, id: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, format!("{phone}\n{id}").as_bytes());
    digest.as_ref()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Linux: the D-Bus ids of the phone notifications shown, by tag, the newest
/// [`MAX_SHOWN`], so an update replaces its notification and a cancel can
/// close it. Built on every OS so it is tested everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_SHOWN: usize = 500;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn shown_ids() -> &'static Mutex<std::collections::VecDeque<(String, u32)>> {
    static SHOWN: std::sync::OnceLock<Mutex<std::collections::VecDeque<(String, u32)>>> =
        std::sync::OnceLock::new();
    SHOWN.get_or_init(Default::default)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn remember_shown(tag: &str, dbus_id: u32) {
    let mut shown = shown_ids().lock().unwrap_or_else(|e| e.into_inner());
    shown.retain(|(t, _)| t != tag);
    shown.push_back((tag.to_string(), dbus_id));
    while shown.len() > MAX_SHOWN {
        shown.pop_front();
    }
}

/// The D-Bus id `tag` was shown with, if it is still remembered.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn shown_id(tag: &str) -> Option<u32> {
    let shown = shown_ids().lock().unwrap_or_else(|e| e.into_inner());
    shown.iter().find(|(t, _)| t == tag).map(|(_, id)| *id)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn take_shown(tag: &str) -> Option<u32> {
    let mut shown = shown_ids().lock().unwrap_or_else(|e| e.into_inner());
    let at = shown.iter().position(|(t, _)| t == tag)?;
    shown.remove(at).map(|(_, id)| id)
}

/// The longest app name, title and text shown, in characters.
const MAX_APP: usize = 64;
const MAX_TITLE: usize = 200;
const MAX_TEXT: usize = 1000;

/// `raw` as a phone may have it shown: control characters (other than line
/// breaks) dropped, trimmed, and cut to `max` characters with an ellipsis.
/// The text comes from any app on the phone, and so from anyone who can
/// send that app a message.
fn clean(raw: &str, max: usize) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .collect();
    let kept = kept.trim();
    if kept.chars().count() <= max {
        return kept.to_string();
    }
    let mut cut: String = kept.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// A notification packet to show: its app, title and text, or `None` for
/// one that only cancels (`isCancel`, N5c) or says nothing.
fn to_show(body: &serde_json::Value) -> Option<(String, String, String)> {
    if body.get("isCancel").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    let field = |key: &str| {
        let max = match key {
            "app" => MAX_APP,
            "title" => MAX_TITLE,
            _ => MAX_TEXT,
        };
        body.get(key)
            .and_then(|v| v.as_str())
            .map(|s| clean(s, max))
            .filter(|s| !s.is_empty())
    };
    let (title, text) = (field("title"), field("text"));
    if title.is_none() && text.is_none() {
        return None;
    }
    Some((
        field("app").unwrap_or_else(|| "Phone".to_string()),
        title.unwrap_or_else(|| "Notification".to_string()),
        text.unwrap_or_default(),
    ))
}

#[derive(Debug)]
pub struct NotificationPlugin;

impl Default for NotificationPlugin {
    fn default() -> Self {
        Self
    }
}

impl NotificationPlugin {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Plugin for NotificationPlugin {
    fn name(&self) -> &'static str {
        "notification"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.notification", "kdeconnect.notification.request"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.notification"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        match packet.packet_type.as_str() {
            "kdeconnect.notification" => {
                let id = packet.body.get("id").and_then(|v| v.as_str()).unwrap_or("");
                // A notification without an id cannot be closed or updated
                // later, and is shown untagged so it never replaces another.
                let tag = (!id.is_empty()).then(|| tag_for(sender.device_id(), id));
                if packet.body.get("isCancel").and_then(|v| v.as_bool()) == Some(true) {
                    // Closed on the phone (N5c): close it here too.
                    if let Some(tag) = &tag {
                        close_desktop_notification(tag).await;
                    }
                    // And it can no longer be answered (T10c).
                    if !id.is_empty()
                        && let Ok(dir) = crate::state::state_dir()
                    {
                        let _ = phone_notes::forget(&dir, sender.device_id(), id);
                    }
                    return Ok(());
                }
                // An update that no longer offers buttons or a reply (or
                // is now sent redacted) can no longer be answered (T10c).
                let note = answerable(sender.device_id(), &packet.body, unix_now());
                if note.is_none()
                    && notification_action::valid_key(id)
                    && let Ok(dir) = crate::state::state_dir()
                {
                    let _ = phone_notes::forget(&dir, sender.device_id(), id);
                }
                if !SHOW_PHONES.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }
                let Some((app, title, text)) = to_show(&packet.body) else {
                    return Ok(());
                };
                let limit = LIMIT.load(std::sync::atomic::Ordering::Relaxed);
                let admitted = flood_guard()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .admit(sender.device_id(), Instant::now(), limit);
                if !admitted {
                    return Ok(());
                }
                // What is shown, as shown: a notification the phone hides
                // stays hidden in the log too.
                crate::events::emit(
                    "notification",
                    Some(sender.device_id()),
                    serde_json::json!({ "app": app }),
                    serde_json::json!({ "title": title, "text": text }),
                );
                // One that offers buttons or a reply is kept for the desktop
                // app to answer (T10c).
                if let Some(note) = note
                    && let Ok(dir) = crate::state::state_dir()
                    && let Err(e) = phone_notes::keep(&dir, note)
                {
                    tracing::warn!("A phone notification was not kept to answer: {e}");
                }
                if let Err(e) = show_desktop_notification(tag.as_deref(), &app, &title, &text).await
                {
                    tracing::warn!("Failed to show notification: {}", e);
                }
            }
            "kdeconnect.notification.request" => {
                // Acknowledge the request - we support notifications
                let response = NetworkPacket::new("kdeconnect.notification");
                sender.send_packet(&response).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The note to keep for a phone notification `body` that can be answered:
/// one that offers buttons, or a reply for itself (`requestReplyId` is its
/// own id). The fields are those shown, held to the same limits.
pub(crate) fn answerable(
    device_id: &str,
    body: &serde_json::Value,
    now_unix: u64,
) -> Option<PhoneNote> {
    let key = body.get("id").and_then(|v| v.as_str())?;
    if !notification_action::valid_key(key) {
        return None;
    }
    let actions = notification_action::offered_actions(body);
    let reply = body.get("requestReplyId").and_then(|v| v.as_str()) == Some(key);
    if actions.is_empty() && !reply {
        return None;
    }
    let (app, title, text) = to_show(body)?;
    Some(PhoneNote {
        device_id: device_id.to_string(),
        key: key.to_string(),
        app,
        title,
        text,
        at_unix: now_unix,
        actions,
        reply,
    })
}

/// The packet that sends `request` to its phone, if the notification is
/// still kept for that phone and offered that answer.
pub(crate) fn answer_packet(notes: &[PhoneNote], request: &AnswerRequest) -> Option<NetworkPacket> {
    let note = notes
        .iter()
        .find(|n| n.device_id == request.device_id && n.key == request.key)?;
    match &request.answer {
        Answer::Action(title) if note.actions.contains(title) => {
            notification_action::action_packet(&note.key, title)
        }
        Answer::Reply(message) if note.reply => {
            notification_action::reply_packet(&note.key, message)
        }
        _ => None,
    }
}

/// Send the desktop app's answers to the phones' notifications (T10c),
/// each to its own phone if it is connected and may receive them. Runs for
/// the server's life; the kept notifications of an earlier run are dropped
/// first.
pub async fn run_answers() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let _ = phone_notes::clear(&dir);
    // Answers for a phone not connected yet wait here, within their time.
    let mut waiting: Vec<AnswerRequest> = Vec::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let now = unix_now();
        waiting.extend(phone_notes::take_all(&dir, now));
        if waiting.is_empty() {
            continue;
        }
        let notes = phone_notes::read(&dir);
        let clients = crate::state::clone_clients().await;
        let mut still = Vec::new();
        for request in waiting.drain(..) {
            let Some(packet) = answer_packet(&notes, &request) else {
                tracing::info!("An answer to a phone notification was dropped: not offered");
                continue;
            };
            if !crate::plugins::pair::permits(&request.device_id, &packet.packet_type) {
                tracing::info!(
                    "An answer to a phone notification was dropped: Notifications is off"
                );
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == request.device_id) else {
                if now.saturating_sub(request.created_unix) < phone_notes::ANSWER_TTL_SECS {
                    still.push(request);
                } else {
                    tracing::info!(
                        "An answer to a phone notification was dropped: the phone did not connect in time"
                    );
                }
                continue;
            };
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("An answer to a phone notification was not sent: {e}");
            }
        }
        waiting = still;
    }
}

/// The session bus's notification server.
#[cfg(target_os = "linux")]
async fn notifications_proxy() -> Result<zbus::Proxy<'static>> {
    let conn = zbus::Connection::session().await.map_err(|e| {
        pocketdesk_core::error::PocketdeskError::Other {
            detail: format!("D-Bus session failed: {e}"),
        }
    })?;
    zbus::Proxy::new(
        &conn,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
        detail: format!("D-Bus proxy failed: {e}"),
    })
}

/// Send a desktop notification via D-Bus (native, no external binary),
/// replacing the one with id `replaces` (0: none), and return the id the
/// notification server gave it.
#[cfg(target_os = "linux")]
async fn send_notification_dbus(summary: &str, body: &str, replaces: u32) -> Result<u32> {
    let proxy = notifications_proxy().await?;
    // Markup is escaped: the text may be a stranger's (N5a).
    let summary = crate::platform::markup_escaped(summary);
    let body = crate::platform::markup_escaped(body);
    proxy
        .call::<_, _, u32>(
            "Notify",
            &(
                "Pocketdesk",                                                      // app_name
                replaces,                                                          // replaces_id
                "",                                                                // app_icon
                summary.as_str(),                                                  // summary
                body.as_str(),                                                     // body
                Vec::<String>::new(),                                              // actions
                std::collections::HashMap::<String, zbus::zvariant::Value>::new(), // hints
                5000i32, // timeout (5 seconds)
            ),
        )
        .await
        .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
            detail: format!("D-Bus call failed: {e}"),
        })
}

/// Show a phone's notification, known by `tag` so an update replaces it and
/// a cancel can close it: a tagged toast on Windows (the same tag replaces);
/// over D-Bus on Linux, replacing the id shown before for that tag and
/// keeping the new one, with notify-send as the fallback (which can neither).
async fn show_desktop_notification(
    tag: Option<&str>,
    app: &str,
    title: &str,
    text: &str,
) -> Result<()> {
    let summary = format!("{}: {}", app, title);

    #[cfg(target_os = "linux")]
    {
        let replaces = tag.and_then(shown_id).unwrap_or(0);
        match send_notification_dbus(&summary, text, replaces).await {
            Ok(id) => {
                if let Some(tag) = tag {
                    remember_shown(tag, id);
                }
                tracing::info!("Desktop notification shown: {}", title);
                return Ok(());
            }
            Err(e) => tracing::debug!("D-Bus notification failed, trying notify-send: {e}"),
        }
    }

    let (toast_summary, toast_text, toast_tag) =
        (summary, text.to_string(), tag.map(str::to_string));
    let shown = tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            match &toast_tag {
                Some(tag) => crate::platform::notify_tagged(&toast_summary, &toast_text, tag),
                None => crate::platform::notify(
                    &toast_summary,
                    &toast_text,
                    std::time::Duration::from_secs(5),
                ),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = toast_tag;
            crate::platform::notify(
                &toast_summary,
                &toast_text,
                std::time::Duration::from_secs(5),
            )
        }
    })
    .await
    .unwrap_or(false);
    if shown {
        tracing::info!("Desktop notification shown: {}", title);
    } else {
        tracing::warn!("Desktop notification could not be shown; dropped: {title}");
    }
    Ok(())
}

/// Close the phone notification shown as `tag`, if it is still here.
async fn close_desktop_notification(tag: &str) {
    #[cfg(windows)]
    {
        let tag = tag.to_string();
        let _ = tokio::task::spawn_blocking(move || crate::platform::remove_tagged(&tag)).await;
    }
    #[cfg(target_os = "linux")]
    if let Some(id) = take_shown(tag) {
        let closed = async {
            notifications_proxy()
                .await?
                .call::<_, _, ()>("CloseNotification", &(id,))
                .await
                .map_err(|e| pocketdesk_core::error::PocketdeskError::Other {
                    detail: format!("D-Bus call failed: {e}"),
                })
        };
        if let Err(e) = closed.await {
            tracing::debug!("Could not close notification {id}: {e}");
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = tag;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_notification_with_buttons_or_its_own_reply_is_kept() {
        let full = json!({
            "id": "0|com.chat|1", "app": "Chat", "title": "Ann", "text": "Lunch?",
            "actions": ["Mark as read", "Mute"], "requestReplyId": "0|com.chat|1"
        });
        let note = answerable("phone", &full, 100).unwrap();
        assert_eq!(note.actions, ["Mark as read", "Mute"]);
        assert!(note.reply);
        assert_eq!((note.title.as_str(), note.at_unix), ("Ann", 100));
        let plain = json!({"id": "k", "app": "Chat", "title": "Ann", "text": "Hi"});
        assert!(answerable("phone", &plain, 100).is_none());
        let foreign_reply = json!({"id": "k", "app": "Chat", "title": "Ann", "text": "Hi", "requestReplyId": "other"});
        assert!(
            answerable("phone", &foreign_reply, 100).is_none(),
            "a reply for another notification"
        );
        let no_id = json!({"app": "Chat", "title": "Ann", "text": "Hi", "actions": ["Ok"]});
        assert!(answerable("phone", &no_id, 100).is_none());
    }

    #[test]
    fn an_answer_goes_only_as_the_notification_offered() {
        let notes = vec![PhoneNote {
            device_id: "phone".into(),
            key: "k".into(),
            app: "Chat".into(),
            title: "Ann".into(),
            text: "Lunch?".into(),
            at_unix: 1,
            actions: vec!["Mark as read".into()],
            reply: false,
        }];
        let ask = |device: &str, key: &str, answer: Answer| AnswerRequest {
            id: "00000000-0000-0000-0000-000000000000".into(),
            device_id: device.into(),
            key: key.into(),
            answer,
            created_unix: 1,
        };
        let press = answer_packet(
            &notes,
            &ask("phone", "k", Answer::Action("Mark as read".into())),
        )
        .unwrap();
        assert_eq!(press.packet_type, notification_action::NOTIFICATION_ACTION);
        assert!(
            answer_packet(&notes, &ask("phone", "k", Answer::Action("Delete".into()))).is_none(),
            "not offered"
        );
        assert!(
            answer_packet(&notes, &ask("phone", "k", Answer::Reply("hi".into()))).is_none(),
            "takes no reply"
        );
        assert!(
            answer_packet(
                &notes,
                &ask("other", "k", Answer::Action("Mark as read".into()))
            )
            .is_none(),
            "another phone's"
        );
        assert!(
            answer_packet(
                &notes,
                &ask("phone", "gone", Answer::Action("Mark as read".into()))
            )
            .is_none()
        );
    }

    #[test]
    fn at_most_twenty_a_minute_are_shown_from_one_phone() {
        let mut guard = FloodGuard::default();
        let start = Instant::now();
        let shown = (0..30)
            .filter(|_| guard.admit("phone", start, PER_MINUTE))
            .count();
        assert_eq!(shown, PER_MINUTE);
        assert!(
            guard.admit("other", start, PER_MINUTE),
            "another phone has its own count"
        );
        assert!(
            guard.admit("phone", start + Duration::from_secs(61), PER_MINUTE),
            "a minute later it may show again"
        );
        let mut strict = FloodGuard::default();
        assert_eq!(
            (0..10).filter(|_| strict.admit("p", start, 3)).count(),
            3,
            "the configured limit"
        );
    }

    #[test]
    fn a_phones_text_is_cut_to_length_and_rid_of_control_characters() {
        assert_eq!(clean("  a\u{0007}b\nc\u{001b}[31m ", 10), "ab\nc[31m");
        let long = "x".repeat(MAX_TEXT + 50);
        let shown = to_show(&json!({"title": "t", "text": long})).unwrap().2;
        assert_eq!(shown.chars().count(), MAX_TEXT);
        assert!(shown.ends_with('…'));
        let (app, title, _) =
            to_show(&json!({"app": "A".repeat(500), "title": "T".repeat(500)})).unwrap();
        assert_eq!(
            (app.chars().count(), title.chars().count()),
            (MAX_APP, MAX_TITLE)
        );
    }

    #[test]
    fn an_update_finds_the_notification_it_replaces_and_a_cancel_takes_it() {
        remember_shown("t-update", 7);
        assert_eq!(shown_id("t-update"), Some(7), "an update replaces id 7");
        remember_shown("t-update", 9);
        assert_eq!(shown_id("t-update"), Some(9), "one entry per tag");
        assert_eq!(take_shown("t-update"), Some(9));
        assert_eq!(take_shown("t-update"), None, "closed once");
        for i in 0..MAX_SHOWN + 5 {
            remember_shown(&format!("t-bound-{i}"), i as u32);
        }
        assert_eq!(shown_id("t-bound-0"), None, "the oldest is forgotten");
        assert!(shown_id(&format!("t-bound-{}", MAX_SHOWN + 4)).is_some());
        assert!(shown_ids().lock().unwrap().len() <= MAX_SHOWN);
    }

    #[test]
    fn a_notification_is_known_by_a_tag_of_its_phone_and_id() {
        let tag = tag_for("phone-a", "0|com.chat|7|null|10123");
        assert_eq!(tag.len(), 16);
        assert!(tag.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(tag, tag_for("phone-a", "0|com.chat|7|null|10123"), "stable");
        assert_ne!(
            tag,
            tag_for("phone-b", "0|com.chat|7|null|10123"),
            "another phone's cancel cannot close it"
        );
    }

    #[test]
    fn a_cancel_or_an_empty_notification_is_not_shown() {
        assert_eq!(
            to_show(&json!({"app": "Chat", "title": "Ana", "text": "hi"})),
            Some(("Chat".into(), "Ana".into(), "hi".into()))
        );
        assert_eq!(to_show(&json!({"id": "k", "isCancel": true})), None);
        assert_eq!(
            to_show(&json!({"app": "Chat", "title": " ", "text": ""})),
            None
        );
        assert_eq!(
            to_show(&json!({"text": "body only"})),
            Some(("Phone".into(), "Notification".into(), "body only".into()))
        );
    }
}
