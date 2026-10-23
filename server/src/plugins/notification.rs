//! Notifications from a phone shown on this desktop (roadmap N5a), and the
//! answer to a phone asking whether this desktop takes them.
//!
//! A phone that shares its notifications may send many at once; at most
//! `notifications_per_minute` (config, default [`PER_MINUTE`]) a minute from
//! one phone are shown, the rest dropped. `phone_notifications = false` shows
//! none (a phone's cancels still close what is shown).

use pocketdesk_core::local::answers;
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
    // N4: no card from an earlier run is going on now.
    if !cfg!(test)
        && let Ok(dir) = crate::state::state_dir()
    {
        pocketdesk_core::local::phone_live::clear(&dir);
    }
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

/// The tag the D-Bus id `dbus_id` was shown with, if it is still remembered.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn tag_of(dbus_id: u32) -> Option<String> {
    let shown = shown_ids().lock().unwrap_or_else(|e| e.into_inner());
    shown
        .iter()
        .find(|(_, id)| *id == dbus_id)
        .map(|(t, _)| t.clone())
}

/// Each phone notification shown here, by tag: its phone and the phone's id
/// for it, so one closed here can be cleared there (L1). The newest
/// [`MAX_SHOWN`].
fn shown_notes() -> &'static Mutex<std::collections::VecDeque<(String, String, String)>> {
    static NOTES: std::sync::OnceLock<Mutex<std::collections::VecDeque<(String, String, String)>>> =
        std::sync::OnceLock::new();
    NOTES.get_or_init(Default::default)
}

fn remember_note(tag: &str, phone: &str, id: &str) {
    let mut notes = shown_notes().lock().unwrap_or_else(|e| e.into_inner());
    notes.retain(|(t, _, _)| t != tag);
    notes.push_back((tag.to_string(), phone.to_string(), id.to_string()));
    while notes.len() > MAX_SHOWN {
        notes.pop_front();
    }
}

/// The phone and its id for the notification shown with `tag`.
fn note_of(tag: &str) -> Option<(String, String)> {
    let notes = shown_notes().lock().unwrap_or_else(|e| e.into_inner());
    notes
        .iter()
        .find(|(t, _, _)| t == tag)
        .map(|(_, phone, id)| (phone.clone(), id.clone()))
}

/// What this run knows of each live card (N4), by its phone-scoped tag:
/// whether its toast was shown, whether the person closed it, when it was
/// last heard and when it ended. Kept apart from the store so a card held
/// at first (a pause, the phone in hand) still shows once it may (N4
/// review), and counted once whether or not phones' notifications show.
#[derive(Default)]
struct LiveCards {
    cards: HashMap<String, LiveState>,
}

struct LiveState {
    shown: bool,
    closed: bool,
    heard: Instant,
    ended: Option<Instant>,
}

/// How a live update is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveHeard {
    /// Dropped: just ended (a late update after its cancel), or too soon
    /// after the last.
    Drop,
    /// Its first this run: counted and logged.
    First,
    /// A later one.
    Again,
}

/// The least time between two updates of one card taken here, whatever a
/// phone sends; a finished bar is always taken.
const LIVE_PACE: Duration = Duration::from_secs(1);

/// How long a card's end is remembered, so an update that crossed it is
/// not taken for a new card (N4 review).
const LIVE_ENDED: Duration = Duration::from_secs(10);

impl LiveCards {
    fn heard(&mut self, tag: &str, now: Instant, finished: bool) -> LiveHeard {
        self.cards
            .retain(|_, s| s.ended.is_none_or(|at| now.duration_since(at) < LIVE_ENDED));
        if self.cards.len() > 200 {
            // Forged tags cannot grow it without end.
            let oldest = self
                .cards
                .iter()
                .min_by_key(|(_, s)| s.heard)
                .map(|(t, _)| t.clone());
            if let Some(oldest) = oldest {
                self.cards.remove(&oldest);
            }
        }
        match self.cards.get_mut(tag) {
            None => {
                self.cards.insert(
                    tag.to_string(),
                    LiveState {
                        shown: false,
                        closed: false,
                        heard: now,
                        ended: None,
                    },
                );
                LiveHeard::First
            }
            Some(state) if state.ended.is_some() => LiveHeard::Drop,
            Some(state) if !finished && now.duration_since(state.heard) < LIVE_PACE => {
                LiveHeard::Drop
            }
            Some(state) => {
                state.heard = now;
                LiveHeard::Again
            }
        }
    }

    /// Whether its toast is to be shown in full (none shown yet) rather
    /// than updated, or not at all (closed here).
    fn show(&self, tag: &str) -> Option<bool> {
        let state = self.cards.get(tag)?;
        (!state.closed).then_some(!state.shown)
    }

    fn shown(&mut self, tag: &str) {
        if let Some(state) = self.cards.get_mut(tag) {
            state.shown = true;
        }
    }

    /// The person closed it here: no toast for it again until it ends.
    fn closed(&mut self, tag: &str) -> bool {
        match self.cards.get_mut(tag) {
            Some(state) if state.ended.is_none() => {
                state.closed = true;
                true
            }
            _ => false,
        }
    }

    fn ended(&mut self, tag: &str, now: Instant) {
        if let Some(state) = self.cards.get_mut(tag) {
            state.ended = Some(now);
        }
    }

    fn is_live(&self, tag: &str) -> bool {
        self.cards.get(tag).is_some_and(|s| s.ended.is_none())
    }
}

fn live_cards() -> &'static Mutex<LiveCards> {
    static CARDS: std::sync::OnceLock<Mutex<LiveCards>> = std::sync::OnceLock::new();
    CARDS.get_or_init(Default::default)
}

/// A live card's toast was not found to update (Windows): closed here.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn live_gone(tag: &str) {
    live_cards()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .closed(tag);
}

/// The person closed the phone notification shown with `tag` here (its
/// toast's ✕, a Linux close): ask its phone to clear it too (L1), with
/// "Clearing here clears it on the phone" on.
pub(crate) fn cleared_here(tag: &str) {
    // N4: a live card closed here stays closed, and its D-Bus id is let go
    // so a later update cannot open a new one (N4 review).
    if live_cards()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .closed(tag)
    {
        let _ = take_shown(tag);
    }
    if !crate::events::current_config().clear_on_phone {
        return;
    }
    let Some((phone, id)) = note_of(tag) else {
        return;
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    if let Err(e) = pocketdesk_core::local::note_clears::request(&dir, &phone, &id, unix_now()) {
        tracing::warn!("A notification closed here was not cleared on the phone: {e}");
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

/// How long after an action or reply a close is the server's, not the
/// person's (L1 review).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const ANSWERED_CLOSE: Duration = Duration::from_secs(3);

/// What the notification server shows (N3), asked once it answers.
#[cfg(target_os = "linux")]
async fn server_caps(proxy: &zbus::Proxy<'static>) -> crate::toast_answers::DbusCaps {
    static CAPS: tokio::sync::OnceCell<crate::toast_answers::DbusCaps> =
        tokio::sync::OnceCell::const_new();
    CAPS.get_or_try_init(|| async {
        proxy
            .call::<_, _, Vec<String>>("GetCapabilities", &())
            .await
            .map(|caps| crate::toast_answers::DbusCaps::from_list(&caps))
    })
    .await
    .copied()
    .unwrap_or_default()
}

/// What a phone notification shown over D-Bus with buttons offers: N3's
/// answers, and its app for "Not from this app" (K2b).
#[cfg(target_os = "linux")]
#[derive(Clone)]
struct Pressable {
    answers: Option<crate::toast_answers::Answerable>,
    app: String,
}

/// The phone notifications shown over D-Bus with buttons, by their D-Bus
/// id, newest last: a press names only the id.
#[cfg(target_os = "linux")]
fn answerable_ids() -> &'static Mutex<std::collections::VecDeque<(u32, Pressable)>> {
    static IDS: std::sync::OnceLock<Mutex<std::collections::VecDeque<(u32, Pressable)>>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(Default::default)
}

/// As many as the notes kept to answer (`phone_notes`).
#[cfg(target_os = "linux")]
const MAX_ANSWERABLE: usize = 30;

#[cfg(target_os = "linux")]
fn remember_answerable(id: u32, pressable: Pressable) {
    let mut ids = answerable_ids().lock().unwrap_or_else(|e| e.into_inner());
    ids.retain(|(i, _)| *i != id);
    ids.push_back((id, pressable));
    // Each kind its own places (K2b review).
    crate::toast_answers::keep_within(&mut ids, |(_, p)| p.answers.is_some(), MAX_ANSWERABLE);
}

#[cfg(target_os = "linux")]
fn forget_answerable(id: u32) {
    let mut ids = answerable_ids().lock().unwrap_or_else(|e| e.into_inner());
    ids.retain(|(i, _)| *i != id);
}

#[cfg(target_os = "linux")]
fn answerable_for(id: u32) -> Option<Pressable> {
    let ids = answerable_ids().lock().unwrap_or_else(|e| e.into_inner());
    ids.iter().find(|(i, _)| *i == id).map(|(_, a)| a.clone())
}

/// Listen, for the server's life, for presses on the answerable
/// notifications (N3): started at the first, again after a failure.
#[cfg(target_os = "linux")]
fn listen_for_answers() {
    static LISTENING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if LISTENING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        if let Err(e) = answer_signals().await {
            tracing::warn!("Answers from notifications stopped: {e}");
        }
        // A notification server that restarted counts its ids from 1
        // again: the ids kept no longer name ours (N3 review).
        answerable_ids()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        LISTENING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// `ActionInvoked` (a button) and `NotificationReplied` (KDE's inline
/// reply) on this computer's notifications, answered for the ones shown
/// as answerable; any other notification's are not ours.
#[cfg(target_os = "linux")]
async fn answer_signals() -> Result<()> {
    use futures_lite::StreamExt;
    let failed = |e: zbus::Error| pocketdesk_core::error::PocketdeskError::Other {
        detail: format!("D-Bus signals failed: {e}"),
    };
    let proxy = notifications_proxy().await?;
    let mut invoked = proxy
        .receive_signal("ActionInvoked")
        .await
        .map_err(failed)?;
    let mut replied = proxy
        .receive_signal("NotificationReplied")
        .await
        .map_err(failed)?;
    let mut closed = proxy
        .receive_signal("NotificationClosed")
        .await
        .map_err(failed)?;
    // Notifications just answered or clicked: GNOME closes them after an
    // action with the person's reason, which is no clear (L1 review).
    let mut answered: std::collections::HashMap<u32, Instant> = std::collections::HashMap::new();
    loop {
        answered.retain(|_, at| at.elapsed() < ANSWERED_CLOSE);
        tokio::select! {
            Some(message) = invoked.next() => {
                if let Ok((id, _)) = message.body().deserialize::<(u32, String)>() {
                    answered.insert(id, Instant::now());
                }
                if let Ok((id, key)) = message.body().deserialize::<(u32, String)>()
                    && let Some(pressable) = answerable_for(id)
                {
                    if key == crate::toast_answers::MUTE {
                        crate::toast_answers::mute(&pressable.app);
                    } else if let Some(answers) = &pressable.answers
                        && let Some(answer) = crate::toast_answers::dbus_action_answer(&key, answers)
                    {
                        crate::toast_answers::send(answers, answer);
                    }
                }
            }
            // Closed by the person (reason 2), to clear on the phone (L1).
            Some(message) = closed.next() => {
                if let Ok((id, reason)) = message.body().deserialize::<(u32, u32)>()
                    && reason == 2
                    && !answered.contains_key(&id)
                    && let Some(tag) = tag_of(id)
                {
                    cleared_here(&tag);
                }
            }
            Some(message) = replied.next() => {
                if let Ok((id, _)) = message.body().deserialize::<(u32, String)>() {
                    answered.insert(id, Instant::now());
                }
                if let Ok((id, text)) = message.body().deserialize::<(u32, String)>()
                    && let Some(answers) = answerable_for(id).and_then(|p| p.answers)
                    && let Some(answer) = crate::toast_answers::dbus_reply_answer(&text, &answers)
                {
                    crate::toast_answers::send(&answers, answer);
                }
            }
            else => return Ok(()),
        }
    }
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

/// The computer's answer to a phone notification that asked for one (C3):
/// `{id}`, or `{id, refused: true}` when its Notifications permission is
/// off. A phone that hears nothing back keeps the notification to send
/// again, so one written into a link that had died is not lost.
pub const ACK: &str = "kdeconnect.notification.ack";

/// The answer to `body`, if it asked (`ack: true`) and names itself. Any
/// id it gave is echoed, even one this computer would not keep to answer
/// (C3 review): unanswered, the phone would send it again for an hour. A
/// cancel needs no answer.
fn ack_for(body: &serde_json::Value, refused: bool) -> Option<NetworkPacket> {
    if body.get("ack").and_then(|v| v.as_bool()) != Some(true)
        || body.get("isCancel").and_then(|v| v.as_bool()) == Some(true)
    {
        return None;
    }
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())?;
    let mut answer = serde_json::json!({ "id": id });
    if refused {
        answer["refused"] = true.into();
    }
    Some(NetworkPacket::new(ACK).with_body(answer))
}

/// A phone notification the phone may not send here: refused, if it asked.
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    if packet.packet_type != "kdeconnect.notification" {
        return;
    }
    if let Some(ack) = ack_for(&packet.body, true) {
        let _ = sender.send_packet(&ack).await;
    }
}

/// The longest wait a phone's notification may claim, in seconds (a day).
const MAX_WAITED_SECS: u64 = 86_400;

/// How long a phone's notification waited on the phone before it could be
/// sent, in seconds (C3): `waited`, measured on the phone's own clock; 0 when
/// it went at once, or when the value makes no sense.
fn waited_secs(body: &serde_json::Value) -> u64 {
    body.get("waited")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(MAX_WAITED_SECS)
}

/// The line that marks a notification late (C3): one that waited a minute
/// or more, with how long; a few seconds of retries are not late.
fn late_line(waited: u64) -> Option<String> {
    if waited < 60 {
        return None;
    }
    let (hours, minutes) = (waited / 3600, waited % 3600 / 60);
    Some(match (hours, minutes) {
        (0, m) => format!("Sent {m} min late"),
        (h, 0) => format!("Sent {h} h late"),
        (h, m) => format!("Sent {h} h {m} min late"),
    })
}

/// `text` as shown, with the late line last when it waited.
fn with_late(text: &str, waited: u64) -> String {
    match late_line(waited) {
        Some(line) if text.is_empty() => line,
        Some(line) => format!("{text}\n{line}"),
        None => text.to_string(),
    }
}

/// A KDE Connect device's notification to show (N9): its app is
/// `appName`, a ticker stands in for missing text, and one marked `silent`
/// (already there when the device connected) is not shown.
fn kde_to_show(body: &serde_json::Value) -> Option<(String, String, String)> {
    if body.get("silent").and_then(|v| v.as_bool()) == Some(true) {
        return None;
    }
    let pick = |key: &str| {
        body.get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
    };
    let mut ours = serde_json::json!({
        "app": pick("appName").unwrap_or("KDE Connect"),
        "isCancel": body.get("isCancel").cloned().unwrap_or(serde_json::Value::Null),
    });
    if let Some(title) = pick("title") {
        ours["title"] = title.into();
    }
    if let Some(text) = pick("text").or_else(|| pick("ticker")) {
        ours["text"] = text.into();
    }
    to_show(&ours)
}

/// Show a KDE Connect device's notification (N9), as a phone's: under the
/// same switch and flood limit, with an event, closed here when it closes
/// there. Not answerable from here.
pub(crate) async fn show_from_kde(device_id: &str, body: &serde_json::Value) {
    // A KDE device picks its own id, which may look like a phone's: its
    // tags and its share of the flood limit are its own (N9 review).
    let device_id = format!("kde:{device_id}");
    let device_id = device_id.as_str();
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let tag = (!id.is_empty()).then(|| tag_for(device_id, id));
    if body.get("isCancel").and_then(|v| v.as_bool()) == Some(true) {
        if let Some(tag) = &tag {
            close_desktop_notification(tag).await;
        }
        return;
    }
    let Some((app, title, text)) = kde_to_show(body) else {
        return;
    };
    // A muted app's never counts against the limit (K2b review); a pause
    // holds them all alike, still counted, so a phone's burst stays bounded
    // (K2a review).
    let muted = muted_here(&app);
    // P4: one held by its words is held as a pause holds it, still counted
    // (P4 review: the words are the sender's to choose).
    let words = held_by_words(&app, body);
    // P3: quiet hours, or presenting, hold them as a pause does.
    let quiet = crate::quiet_hours::now();
    let held = held_here(muted, quiet.is_some() || words.is_some());
    let limit = LIMIT.load(std::sync::atomic::Ordering::Relaxed);
    if !muted
        && !flood_guard()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .admit(device_id, Instant::now(), limit)
    {
        return;
    }
    let mut meta = serde_json::json!({ "app": app });
    if let Some(quiet) = quiet {
        meta[quiet.field()] = serde_json::json!(true);
    }
    let mut content = serde_json::json!({ "title": title, "text": text });
    held_by_fields(&words, &mut meta, &mut content);
    crate::events::emit("notification", Some(device_id), meta, content);
    // An update held by its words closes the toast an earlier one showed.
    if words.is_some()
        && let Some(tag) = &tag
    {
        close_desktop_notification(tag).await;
    }
    if !SHOW_PHONES.load(std::sync::atomic::Ordering::Relaxed) || held {
        return;
    }
    // Muteable only by a name the device gave (K2b review: the fallback
    // name stands for every nameless one).
    let named = given_name(body, "appName");
    if let Err(e) = show_desktop_notification(
        tag.as_deref(),
        &app,
        &title,
        &text,
        None,
        named,
        crate::toast_answers::Pictures::default(),
    )
    .await
    {
        tracing::warn!("Failed to show a KDE Connect device's notification: {}", e);
    }
}

/// The most base64 a picture or icon may come as: [`MAX_BYTES`]'s worth.
///
/// [`MAX_BYTES`]: pocketdesk_core::local::notification_icons::MAX_BYTES
const MAX_BASE64: usize = pocketdesk_core::local::notification_icons::MAX_BYTES / 3 * 4 + 4;

/// `body`'s `field` decoded from base64, if it is there and not too long.
fn decoded(body: &serde_json::Value, field: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let text = body.get(field)?.as_str()?;
    if text.len() > MAX_BASE64 {
        tracing::info!("A phone notification's {field} was dropped: too large");
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// Keep the icon `body` brings for `phone`'s `app`, if any and it fits (K3).
fn keep_icon(phone: &str, app: &str, body: &serde_json::Value) {
    let Some(bytes) = decoded(body, "appIcon") else {
        return;
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    if let Err(e) = pocketdesk_core::local::notification_icons::keep_icon(&dir, phone, app, &bytes)
    {
        tracing::info!("A phone notification's icon was not kept: {e}");
    }
}

/// What a notification to be shown shows with it (K3): `phone`'s kept icon
/// for `app` (one it `named`), and the picture `body` brings for its `id`.
fn pictures_for(
    phone: &str,
    app: &str,
    named: bool,
    id: &str,
    body: &serde_json::Value,
) -> crate::toast_answers::Pictures {
    use pocketdesk_core::local::notification_icons as icons;
    let Ok(dir) = crate::state::state_dir() else {
        return Default::default();
    };
    let icon = named.then(|| icons::kept_icon(&dir, phone, app)).flatten();
    if id.is_empty() {
        return crate::toast_answers::Pictures {
            icon,
            ..Default::default()
        };
    }
    // An update without a picture (redacted now, or the phone's switch
    // off) leaves none behind (K3 review).
    let picture = match decoded(body, "picture") {
        Some(bytes) => icons::keep_picture(&dir, phone, id, &bytes)
            .inspect_err(|e| tracing::info!("A phone notification's picture was not kept: {e}"))
            .ok(),
        None => None,
    };
    if picture.is_none() {
        icons::forget_picture(&dir, phone, id);
    }
    crate::toast_answers::Pictures {
        icon,
        picture,
        ..Default::default()
    }
}

/// Whether `body` names its app in `field`: a notification without one is
/// shown under a fallback name, which "Not from this app" must not mute
/// (K2b review).
fn given_name(body: &serde_json::Value, field: &str) -> bool {
    body.get(field)
        .and_then(|v| v.as_str())
        .is_some_and(|app| !app.trim().is_empty())
}

/// Whether the phone says it was in use when the notification came (M6,
/// `phoneInUse`): then no toast shows it here.
/// Whether the phone marked it as come while another computer was in use
/// (N7).
fn marked_elsewhere(body: &serde_json::Value) -> bool {
    body.get(pocketdesk_core::protocol::in_use::ELSEWHERE)
        .and_then(|v| v.as_bool())
        == Some(true)
}

/// Whether phones' notifications are shown here at all (N7: a computer
/// that would not show one never says it is in use).
pub(crate) fn shows_phones() -> bool {
    SHOW_PHONES.load(std::sync::atomic::Ordering::Relaxed)
}

fn phone_in_use(body: &serde_json::Value) -> bool {
    body.get("phoneInUse").and_then(|v| v.as_bool()) == Some(true)
}

/// Whether a notification is held here: its app `muted` (K2b), this
/// computer paused (K2a), or the phone `in_hand` when it came (M6).
fn held_here(muted: bool, in_hand: bool) -> bool {
    muted || in_hand || crate::paused::now()
}

/// The rule of desktop Settings "Hold notifications with these words" that
/// holds a notification (P4), as its line; read at each one, so a change in
/// Settings applies to the next. Held to the title and text as sent, not
/// the "Notification" this computer shows for none (P4 review).
fn held_by_words(app: &str, body: &serde_json::Value) -> Option<String> {
    let lines = crate::events::current_config().hold_words;
    if lines.is_empty() {
        return None;
    }
    let field = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let rules = pocketdesk_core::local::hold_words::rules(&lines);
    pocketdesk_core::local::hold_words::held_by(&rules, app, field("title"), field("text"))
        .map(pocketdesk_core::local::hold_words::Rule::line)
}

/// A notification held by its words says so in its event, and names the
/// line only in its content, which is logged with "details in events" on
/// (P4 review: the owner's words are not metadata).
fn held_by_fields(
    words: &Option<String>,
    meta: &mut serde_json::Value,
    content: &mut serde_json::Value,
) {
    if let Some(rule) = words {
        meta["held_by_words"] = serde_json::json!(true);
        content["held_by"] = serde_json::json!(rule);
    }
}

/// The phone-in-hand (M6) and elsewhere (N7) marks as they apply: never to
/// a ringing alarm, which, as a call, is not held by them (P5).
fn alarm_marks(alarm: bool, in_hand: bool, elsewhere: bool) -> (bool, bool) {
    (in_hand && !alarm, elsewhere && !alarm)
}

/// How long an alarm may have waited on the phone and still ring here (P5
/// review: one sent an hour late must not start ringing now).
const ALARM_LATE_SECS: u64 = 120;

/// Whether a notification rings here as an alarm (P5): the phone's alarm,
/// with desktop Settings "Phones' alarms ring here" on, come in time.
fn rings(alarm: bool, setting: bool, waited_secs: u64) -> bool {
    alarm && setting && waited_secs < ALARM_LATE_SECS
}

/// Whether the phone sent it as a ringing alarm or finished timer (P5).
fn is_alarm(body: &serde_json::Value) -> bool {
    body.get("alarm").and_then(|v| v.as_bool()) == Some(true)
}

/// Whether it is the phone's note of a ringing call (F5).
fn is_call(body: &serde_json::Value) -> bool {
    body.get("id")
        .and_then(|v| v.as_str())
        .is_some_and(|id| id.starts_with("pocketdesk:call:"))
}

/// Whether `app` is muted on this computer (K2b): its notifications are
/// logged and kept to answer, never shown. Read at each notification, so a
/// mute from the desktop app or a toast applies to the next one.
fn muted_here(app: &str) -> bool {
    crate::state::state_dir()
        .map(|dir| {
            pocketdesk_core::local::muted_apps::is_muted(
                &pocketdesk_core::local::muted_apps::read(&dir),
                app,
            )
        })
        .unwrap_or(false)
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
        &[
            "kdeconnect.notification",
            "kdeconnect.notification.request",
            notification_action::NOTIFICATION_ANSWERED,
        ]
    }

    // The phone's notifications answered from here (T10c) go out too.
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[
            "kdeconnect.notification",
            ACK,
            pocketdesk_core::protocol::notification_action::NOTIFICATION_REPLY,
            pocketdesk_core::protocol::notification_action::NOTIFICATION_ACTION,
            // A clear (L1).
            pocketdesk_core::protocol::notification_action::NOTIFICATION_REQUEST,
            // An open on the phone (N3).
            pocketdesk_core::protocol::notification_action::NOTIFICATION_OPEN,
        ]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        match packet.packet_type.as_str() {
            "kdeconnect.notification" => {
                // Here, whatever is done with it: said at once (C3).
                if let Some(ack) = ack_for(&packet.body, false) {
                    let _ = sender.send_packet(&ack).await;
                }
                let id = packet.body.get("id").and_then(|v| v.as_str()).unwrap_or("");
                // A notification without an id cannot be closed or updated
                // later, and is shown untagged so it never replaces another.
                let tag = (!id.is_empty()).then(|| tag_for(sender.device_id(), id));
                // Known by its tag, to clear on the phone when closed here (L1);
                // not as it is closed on the phone.
                if let Some(tag) = &tag
                    && packet.body.get("isCancel").and_then(|v| v.as_bool()) != Some(true)
                {
                    remember_note(tag, sender.device_id(), id);
                }
                if packet.body.get("isCancel").and_then(|v| v.as_bool()) == Some(true) {
                    // Closed on the phone (N5c): close it here too.
                    if let Some(tag) = &tag {
                        close_desktop_notification(tag).await;
                        live_cards()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .ended(tag, Instant::now());
                        // N4: its card ends with it.
                        if let Ok(dir) = crate::state::state_dir() {
                            let (phone, tag) = (sender.device_id().to_string(), tag.clone());
                            let _ = tokio::task::spawn_blocking(move || {
                                pocketdesk_core::local::phone_live::remove(&dir, &phone, &tag)
                            })
                            .await;
                        }
                    }
                    // And it can no longer be answered (T10c), nor is its
                    // picture kept (K3 review).
                    if !id.is_empty()
                        && let Ok(dir) = crate::state::state_dir()
                    {
                        // A call's stays, for Call back (M8 review).
                        let _ = phone_notes::end_call(&dir, sender.device_id(), id);
                        pocketdesk_core::local::notification_icons::forget_picture(
                            &dir,
                            sender.device_id(),
                            id,
                        );
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
                let Some((app, title, text)) = to_show(&packet.body) else {
                    return Ok(());
                };
                let named = given_name(&packet.body, "app");
                // One that waited on the phone says so (C3).
                let waited = waited_secs(&packet.body);
                // Muted here (K2b): logged and kept, never shown, and never
                // counted against the phone's limit, which is for what shows
                // (K2b review: a muted app's burst dropped the others).
                // Paused (K2a): held as a muted app's are, but still counted
                // (K2a review: a week's pause must not unbound a phone).
                let muted = muted_here(&app);
                // P4: held by its words, as a pause holds it, still counted
                // (P4 review); never a call's note, which M6 and N7 never hold.
                // P5: a ringing alarm, never held but by a pause, quiet
                // hours or its app muted, as a call is.
                let alarm = is_alarm(&packet.body);
                let words = if is_call(&packet.body) || alarm {
                    None
                } else {
                    held_by_words(&app, &packet.body)
                };
                // M6: one that came while the phone was in hand is held as a
                // pause holds it: kept, logged, counted, not shown.
                let (in_hand, elsewhere) = alarm_marks(
                    alarm,
                    phone_in_use(&packet.body),
                    marked_elsewhere(&packet.body),
                );
                // N7: another computer of the owner's is in use, not this one.
                // P3: quiet hours, or presenting, hold it as a pause does.
                let quiet = crate::quiet_hours::now();
                let held = held_here(
                    muted,
                    in_hand || elsewhere || quiet.is_some() || words.is_some(),
                );
                // N4: an ongoing update is one card, updated in place; only
                // its first counts against the limit and is an event.
                let live = pocketdesk_core::protocol::live::Live::from_body(&packet.body);
                // A card's notification that comes without live now (a
                // download done, the switch off) is a card no more (N4
                // review); with the setting off, none is kept.
                let was_live = tag.as_ref().is_some_and(|t| {
                    live_cards()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .is_live(t)
                });
                let live_off = !crate::events::current_config().phone_live_updates;
                if let Some(tag) = &tag
                    && (live_off || live.is_none())
                    && was_live
                {
                    live_cards()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .ended(tag, Instant::now());
                    if let Ok(dir) = crate::state::state_dir() {
                        let (phone, tag) = (sender.device_id().to_string(), tag.clone());
                        let _ = tokio::task::spawn_blocking(move || {
                            pocketdesk_core::local::phone_live::remove(&dir, &phone, &tag)
                        })
                        .await;
                    }
                }
                if live.is_some() && live_off {
                    return Ok(());
                }
                let first = match (&live, &tag) {
                    (Some(live), Some(tag)) => {
                        match live_cards()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .heard(tag, Instant::now(), live.percent() == Some(100))
                        {
                            LiveHeard::Drop => return Ok(()),
                            LiveHeard::First => true,
                            LiveHeard::Again => false,
                        }
                    }
                    _ => true,
                };
                let limit = LIMIT.load(std::sync::atomic::Ordering::Relaxed);
                if !muted
                    && first
                    && !flood_guard()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .admit(sender.device_id(), Instant::now(), limit)
                {
                    return Ok(());
                }
                // The app's icon, kept for this and later ones (K3), once
                // the phone's limit let the notification in (K3 review).
                if named
                    && !muted
                    && packet.body.get("appIcon").is_some()
                    && crate::events::current_config().notification_pictures
                {
                    let (phone, app, body) = (
                        sender.device_id().to_string(),
                        app.clone(),
                        packet.body.clone(),
                    );
                    let _ =
                        tokio::task::spawn_blocking(move || keep_icon(&phone, &app, &body)).await;
                }
                // An event whether or not it is shown here (a script may want
                // the phone's notifications without the toasts), as it would
                // be shown: one the phone hides stays hidden in the log too.
                // Late as the toast says it (a minute or more, C3 review).
                let mut meta = if late_line(waited).is_some() {
                    serde_json::json!({ "app": app, "late_secs": waited })
                } else {
                    serde_json::json!({ "app": app })
                };
                if alarm {
                    meta["alarm"] = serde_json::json!(true);
                }
                if in_hand {
                    meta["phone_in_use"] = serde_json::json!(true);
                }
                if elsewhere {
                    meta["in_use_elsewhere"] = serde_json::json!(true);
                }
                if let Some(quiet) = quiet {
                    meta[quiet.field()] = serde_json::json!(true);
                }
                if live.is_some() {
                    meta["live"] = serde_json::json!(true);
                }
                // A muted app's card is not kept, so its updates are not
                // told apart from its first: none is logged.
                if first && !(muted && live.is_some()) {
                    let mut content = serde_json::json!({ "title": title, "text": text });
                    held_by_fields(&words, &mut meta, &mut content);
                    crate::events::emit("notification", Some(sender.device_id()), meta, content);
                }
                if !SHOW_PHONES.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }
                // One that offers buttons or a reply is kept for the desktop
                // app to answer (T10c), and answered in its notification here
                // too (N3).
                let answers = note
                    .as_ref()
                    .filter(|_| crate::events::current_config().toast_answers)
                    .map(crate::toast_answers::Answerable::of);
                if let Some(note) = note
                    && let Ok(dir) = crate::state::state_dir()
                    && let Err(e) = phone_notes::keep(&dir, note)
                {
                    tracing::warn!("A phone notification was not kept to answer: {e}");
                }
                // N4: its card, for the Notifications page, kept while held too (a
                // pause, the phone in hand), so it counts once; not a muted app's.
                let live_show = match (&live, &tag) {
                    (Some(live), Some(tag)) => {
                        let now_ms = unix_now().saturating_mul(1000);
                        if !muted && let Ok(dir) = crate::state::state_dir() {
                            let card = pocketdesk_core::local::phone_live::LiveCard {
                                device_id: sender.device_id().to_string(),
                                tag: tag.clone(),
                                app: app.clone(),
                                title: title.clone(),
                                text: text.clone(),
                                live: live.clone(),
                                updated_unix: unix_now(),
                            };
                            let _ = tokio::task::spawn_blocking(move || {
                                pocketdesk_core::local::phone_live::put(&dir, card)
                            })
                            .await;
                        }
                        // Closed here by the person: kept for the page, no toast.
                        let show = live_cards()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .show(tag);
                        let Some(full) = show else {
                            return Ok(());
                        };
                        Some(crate::toast_answers::LiveShow {
                            percent: live.percent(),
                            status: live.status(now_ms),
                            first: full,
                        })
                    }
                    _ => None,
                };
                if held {
                    // An update held by its words closes the toast an
                    // earlier one showed (P4 review: it would stay up with
                    // the words it no longer has).
                    if words.is_some()
                        && let Some(tag) = &tag
                    {
                        close_desktop_notification(tag).await;
                    }
                    // Not shown, so no picture is kept for it (K3 review).
                    if !id.is_empty()
                        && let Ok(dir) = crate::state::state_dir()
                    {
                        pocketdesk_core::local::notification_icons::forget_picture(
                            &dir,
                            sender.device_id(),
                            id,
                        );
                    }
                    return Ok(());
                }
                let text = with_late(&text, waited);
                // The owner's switch (K3 review): off, nothing is kept.
                let mut pictures = if !crate::events::current_config().notification_pictures {
                    crate::toast_answers::Pictures::default()
                } else {
                    let (phone, app, id, body) = (
                        sender.device_id().to_string(),
                        app.clone(),
                        id.to_string(),
                        packet.body.clone(),
                    );
                    tokio::task::spawn_blocking(move || {
                        pictures_for(&phone, &app, named, &id, &body)
                    })
                    .await
                    .unwrap_or_default()
                };
                let is_live = live_show.is_some();
                pictures.live = live_show;
                // Rung as an alarm unless Settings say otherwise, or it
                // came late (P5).
                pictures.alarm = rings(alarm, crate::events::current_config().phone_alarms, waited);
                match show_desktop_notification(
                    tag.as_deref(),
                    &app,
                    &title,
                    &text,
                    answers,
                    // P5 review: an alarm offers no "Not from this app".
                    named && !pictures.alarm,
                    pictures,
                )
                .await
                {
                    // N4: its toast stands now; later updates change it.
                    Ok(()) => {
                        if is_live && let Some(tag) = &tag {
                            live_cards()
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .shown(tag);
                        }
                    }
                    Err(e) => tracing::warn!("Failed to show notification: {}", e),
                }
            }
            // How an answer from here went (E1).
            notification_action::NOTIFICATION_ANSWERED => {
                let Some(answered) = notification_action::Answered::from_body(&packet.body) else {
                    tracing::info!("A phone's word on an answer was dropped: out of bounds");
                    return Ok(());
                };
                let Ok(dir) = crate::state::state_dir() else {
                    return Ok(());
                };
                let device = sender.device_id().to_string();
                let said = answered.clone();
                let kept = tokio::task::spawn_blocking(move || {
                    answers::phone_said(&dir, &device, &said, unix_now())
                })
                .await
                .ok()
                .and_then(|r| r.ok())
                .unwrap_or(false);
                if !kept {
                    // Seen, so a word that came too early or for the wrong
                    // phone is not lost without a trace (E1 review).
                    tracing::info!(
                        "A phone's word on an answer was not kept: no answer sent to it waits"
                    );
                } else if !answered.ok {
                    let reason = answered.reason.unwrap_or_default();
                    tracing::info!("An answer to a phone notification was not sent: {reason}");
                    tell_not_sent(&reason);
                }
                return Ok(());
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
/// one that offers buttons, a reply for itself (`requestReplyId` is its
/// own id), or to be opened on the phone (N3). The fields are those shown,
/// held to the same limits.
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
    let open = body
        .get(notification_action::CAN_OPEN)
        .and_then(|v| v.as_bool())
        == Some(true);
    if actions.is_empty() && !reply && !open {
        return None;
    }
    let (app, title, text) = to_show(body)?;
    Some(PhoneNote {
        device_id: device_id.to_string(),
        key: key.to_string(),
        app,
        title,
        text,
        // When it came on the phone, not when it got here (C3).
        at_unix: now_unix.saturating_sub(waited_secs(body)),
        actions,
        reply,
        open,
        // A chat's recent messages (D4), held to their limits.
        messages: pocketdesk_core::protocol::conversation::messages(body),
    })
}

/// The packet that sends `request` to its phone, if the notification is
/// still kept for that phone and offered that answer; it carries the
/// request's id, for the phone to say how it went (E1).
pub(crate) fn answer_packet(notes: &[PhoneNote], request: &AnswerRequest) -> Option<NetworkPacket> {
    let note = notes
        .iter()
        .find(|n| n.device_id == request.device_id && n.key == request.key)?;
    let packet = match &request.answer {
        Answer::Action(title) if note.actions.contains(title) => {
            notification_action::action_packet(&note.key, title)
        }
        Answer::Reply(message) if note.reply => {
            notification_action::reply_packet(&note.key, message)
        }
        Answer::Open if note.open => notification_action::open_packet(&note.key),
        _ => None,
    }?;
    Some(notification_action::with_answer_id(packet, &request.id))
}

/// Keep where an answer stands (E1), off the runtime but in order (E1
/// review): each write is waited for, so states land as they happened.
async fn note_answer(request: &AnswerRequest, state: answers::State) {
    if cfg!(test) {
        return;
    }
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let outcome = answers::Outcome {
        id: request.id.clone(),
        device_id: request.device_id.clone(),
        key: request.key.clone(),
        state,
        at_unix: unix_now(),
        said: None,
    };
    let kept = tokio::task::spawn_blocking(move || answers::record(&dir, outcome)).await;
    if let Ok(Err(e)) = kept {
        tracing::warn!("Where an answer stands was not kept: {e}");
    }
}

/// An answer this computer drops, kept with why (E1), not only logged.
async fn dropped(request: &AnswerRequest, reason: &str) {
    tracing::info!("An answer to a phone notification was dropped: {reason}");
    note_answer(
        request,
        answers::State::NotSent {
            reason: reason.to_string(),
        },
    )
    .await;
    tell_not_sent(reason);
}

/// An answer that will not reach the app, said on the desktop too (E1): one
/// sent from a toast has no window to show it.
fn tell_not_sent(reason: &str) {
    if cfg!(test) {
        return;
    }
    // A burst (many presses on a stale answer) shows one, not one each
    // (E1 review); the log has them all.
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(5)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let body = format!("Not sent: {reason}");
    std::thread::spawn(move || {
        crate::platform::notify(
            "The answer was not sent",
            &body,
            std::time::Duration::from_secs(10),
        );
    });
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
    let _ = answers::clear(&dir);
    // Answers for a phone not connected yet wait here, within their time.
    let mut waiting: Vec<AnswerRequest> = Vec::new();
    loop {
        // Answers still waiting for their phone are tried again each half
        // second; otherwise only a new answer wakes this.
        if waiting.is_empty() {
            crate::inbox::wait(
                pocketdesk_core::local::paths::NOTIFICATION_OUT_DIR,
                std::time::Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let now = unix_now();
        let new = phone_notes::take_all(&dir, now);
        for request in &new {
            note_answer(request, answers::State::Waiting).await;
        }
        waiting.extend(new);
        if waiting.is_empty() {
            continue;
        }
        let notes = phone_notes::read(&dir);
        let clients = crate::state::clone_answer_targets().await;
        let mut still = Vec::new();
        for request in waiting.drain(..) {
            let Some(packet) = answer_packet(&notes, &request) else {
                dropped(&request, "the notification no longer offers that answer").await;
                continue;
            };
            if !crate::plugins::pair::permits(&request.device_id, &packet.packet_type) {
                dropped(
                    &request,
                    "Notifications is off for this phone, on the Devices page",
                )
                .await;
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == request.device_id) else {
                if now.saturating_sub(request.created_unix) < phone_notes::ANSWER_TTL_SECS {
                    still.push(request);
                } else {
                    dropped(&request, "the phone did not connect within a minute").await;
                }
                continue;
            };
            // Kept as sent before it goes (E1 review): the phone's word can
            // come back before a write after the send would land.
            note_answer(&request, answers::State::Sent).await;
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("An answer to a phone notification was not sent: {e}");
                dropped(&request, "the link to the phone failed").await;
            }
        }
        waiting = still;
    }
}

/// Send the clears of phones' notifications (L1), each to its own phone
/// if it is connected within a minute and its Notifications permission is
/// on, as KDE Connect's own `kdeconnect.notification.request {cancel}`.
/// Runs for the server's life.
pub async fn run_clears() {
    use pocketdesk_core::local::note_clears;
    use pocketdesk_core::protocol::notification_action::{NOTIFICATION_REQUEST, cancel_body};
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut waiting: Vec<note_clears::ClearRequest> = Vec::new();
    loop {
        if waiting.is_empty() {
            crate::inbox::wait(
                pocketdesk_core::local::paths::NOTIFICATION_CLEARS_DIR,
                std::time::Duration::from_millis(500),
            )
            .await;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let now = unix_now();
        waiting.extend(note_clears::take_all(&dir, now));
        if waiting.is_empty() {
            continue;
        }
        let clients = crate::state::clone_answer_targets().await;
        let mut still = Vec::new();
        // The owner's switch holds for every clear, the page's too, and one
        // queued before it was turned off (L1 review).
        if !crate::events::current_config().clear_on_phone {
            waiting.clear();
            continue;
        }
        for clear in waiting.drain(..) {
            // The Notifications permission covers the phone's notifications
            // here, both ways.
            // Paired now, not only allowed by default (as Ring is).
            if !crate::plugins::pair::allows(
                &clear.device_id,
                pocketdesk_core::protocol::permissions::Capability::Notifications,
            ) {
                tracing::info!("A clear was not sent: Notifications is off for the phone");
                continue;
            }
            let Some(phone) = clients.iter().find(|c| c.device_id() == clear.device_id) else {
                if now.saturating_sub(clear.created_unix) < note_clears::CLEAR_TTL_SECS {
                    still.push(clear);
                }
                continue;
            };
            let packet =
                NetworkPacket::new(NOTIFICATION_REQUEST).with_body(cancel_body(&clear.key));
            if let Err(e) = phone.send_packet(&packet).await {
                tracing::warn!("A clear was not sent to the phone: {e}");
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
/// notification server gave it and whether it shows any button.
#[cfg(target_os = "linux")]
async fn send_notification_dbus(
    summary: &str,
    body: &str,
    replaces: u32,
    answers: Option<&crate::toast_answers::Answerable>,
    mute: bool,
    pictures: &crate::toast_answers::Pictures,
) -> Result<(u32, bool)> {
    let proxy = notifications_proxy().await?;
    // Markup is escaped: the text may be a stranger's (N5a).
    let summary = crate::platform::markup_escaped(summary);
    let mut body = crate::platform::markup_escaped(body);
    // N3: an answerable one carries its buttons, and KDE's inline reply
    // when it takes one, as far as the server shows them; it stays longer,
    // to be answered.
    let caps = if answers.is_some() || mute || pictures.picture.is_some() {
        server_caps(&proxy).await
    } else {
        Default::default()
    };
    let actions = crate::toast_answers::dbus_actions(answers, caps, mute);
    let mut hints = std::collections::HashMap::<String, zbus::zvariant::Value>::new();
    // K3: the app's icon as the notification's image, and its picture in
    // the body where the server shows one.
    if let Some(icon) = &pictures.icon {
        hints.insert(
            "image-path".to_string(),
            zbus::zvariant::Value::from(crate::toast_answers::file_uri(icon)),
        );
    }
    if let Some(picture) = pictures.picture.as_ref().filter(|_| caps.body_images) {
        body.push_str(&format!(
            "\n<img src=\"{}\" alt=\"\"/>",
            crate::platform::markup_escaped(&crate::toast_answers::file_uri(picture))
        ));
    }
    if caps.inline_reply && answers.is_some_and(|a| a.reply) {
        hints.insert(
            "x-kde-reply-placeholder-text".to_string(),
            zbus::zvariant::Value::from("Reply"),
        );
    }
    // N4: a live card's bar, where the server draws one, and its words.
    if let Some(live) = &pictures.live {
        if let Some(percent) = live.percent {
            hints.insert(
                "value".to_string(),
                zbus::zvariant::Value::from(i32::from(percent)),
            );
        }
        body.push_str(&format!(
            "\n{}",
            crate::platform::markup_escaped(&live.status)
        ));
    }
    // One to answer stays longer; "Not from this app" alone does not; a
    // live card as the server keeps any (N4).
    // P5: an alarm is urgent and stays until closed or stopped on the phone.
    if pictures.alarm {
        hints.insert("urgency".to_string(), zbus::zvariant::Value::from(2u8));
    }
    let timeout: i32 = if pictures.alarm {
        0
    } else if pictures.live.is_some() {
        // The server's own: never-expiring sits on screen on Plasma (N4 review).
        -1
    } else if answers.is_some() && !actions.is_empty() {
        20_000
    } else {
        5000
    };
    let pressable = !actions.is_empty();
    proxy
        .call::<_, _, u32>(
            "Notify",
            &(
                "Pocketdesk", // app_name
                replaces,     // replaces_id
                "",           // app_icon
                summary.as_str(),
                body.as_str(),
                actions,
                hints,
                timeout,
            ),
        )
        .await
        .map(|id| (id, pressable))
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
    answers: Option<crate::toast_answers::Answerable>,
    muteable: bool,
    pictures: crate::toast_answers::Pictures,
) -> Result<()> {
    let summary = format!("{}: {}", app, title);

    #[cfg(target_os = "linux")]
    {
        let replaces = tag.and_then(shown_id).unwrap_or(0);
        match send_notification_dbus(
            &summary,
            text,
            replaces,
            answers.as_ref(),
            tag.is_some() && muteable,
            &pictures,
        )
        .await
        {
            Ok((id, pressable)) => {
                if let Some(tag) = tag {
                    remember_shown(tag, id);
                }
                // A plain update of an answerable one is answerable no
                // more (N3 review); a phone's can still be muted (K2b), where
                // the server shows its button at all (K2b review).
                if tag.is_some() && pressable {
                    remember_answerable(
                        id,
                        Pressable {
                            answers: answers.clone(),
                            app: app.to_string(),
                        },
                    );
                    listen_for_answers();
                } else {
                    forget_answerable(id);
                }
                // Its close is heard too, to clear it on the phone (L1).
                if tag.is_some() {
                    listen_for_answers();
                }
                tracing::info!("Desktop notification shown: {}", title);
                return Ok(());
            }
            Err(e) => tracing::debug!("D-Bus notification failed, trying notify-send: {e}"),
        }
    }

    let (toast_summary, toast_text, toast_tag, toast_app) = (
        summary,
        text.to_string(),
        tag.map(str::to_string),
        app.to_string(),
    );
    let shown = tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            match &toast_tag {
                Some(tag) => crate::platform::notify_tagged(
                    &toast_summary,
                    &toast_text,
                    tag,
                    answers.as_ref(),
                    muteable.then_some(toast_app.as_str()),
                    &pictures,
                ),
                None => crate::platform::notify(
                    &toast_summary,
                    &toast_text,
                    std::time::Duration::from_secs(5),
                ),
            }
        }
        #[cfg(not(windows))]
        {
            // The D-Bus path above answers, mutes and shows pictures;
            // notify-send cannot.
            let _ = (toast_tag, answers, toast_app, pictures);
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
        forget_answerable(id);
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

    /// N4: a card counts once, shows in full until it is shown, stays
    /// closed once closed, and an update after its end is dropped.
    #[test]
    fn a_live_card_counts_once_and_shows_until_shown() {
        let mut cards = LiveCards::default();
        let t0 = Instant::now();
        assert_eq!(cards.heard("t", t0, false), LiveHeard::First);
        assert_eq!(
            cards.show("t"),
            Some(true),
            "held at first: still to be shown"
        );
        assert_eq!(
            cards.heard("t", t0 + Duration::from_millis(200), false),
            LiveHeard::Drop,
            "paced"
        );
        assert_eq!(
            cards.heard("t", t0 + Duration::from_millis(300), true),
            LiveHeard::Again,
            "finished is taken"
        );
        assert_eq!(cards.show("t"), Some(true), "never shown yet: in full");
        cards.shown("t");
        assert_eq!(
            cards.heard("t", t0 + Duration::from_secs(5), false),
            LiveHeard::Again
        );
        assert_eq!(cards.show("t"), Some(false), "an update in place");
        assert!(cards.closed("t"));
        assert_eq!(cards.show("t"), None, "closed here");
        cards.ended("t", t0 + Duration::from_secs(6));
        assert!(!cards.is_live("t"));
        assert_eq!(
            cards.heard("t", t0 + Duration::from_secs(7), false),
            LiveHeard::Drop,
            "after its end"
        );
        assert_eq!(
            cards.heard("t", t0 + Duration::from_secs(20), false),
            LiveHeard::First,
            "a new card later"
        );
        assert!(!cards.closed("unknown"));
    }

    /// N3: a notification that only offers to be opened is kept, and Open
    /// goes only to one that offers it.
    #[test]
    fn an_open_goes_only_where_offered() {
        use pocketdesk_core::local::phone_notes::{Answer, AnswerRequest};
        let body = serde_json::json!({
            "id": "0|com.chat|1|null|10", "app": "Chat", "title": "Ann",
            "text": "Hi", "canOpen": true,
        });
        let note = answerable("phone", &body, 100).expect("kept for its open");
        assert!(note.open && note.actions.is_empty() && !note.reply);
        let plain = serde_json::json!({"id": "0|com.chat|2|null|10", "app": "Chat", "title": "Ann", "text": "Hi"});
        assert!(answerable("phone", &plain, 100).is_none());
        let request = |key: &str| AnswerRequest {
            id: "6c1f2a0e-0000-4000-8000-000000000001".to_string(),
            device_id: "phone".to_string(),
            key: key.to_string(),
            answer: Answer::Open,
            created_unix: 100,
        };
        let packet = answer_packet(std::slice::from_ref(&note), &request(&note.key)).expect("sent");
        assert_eq!(packet.packet_type, notification_action::NOTIFICATION_OPEN);
        assert_eq!(packet.body["key"], note.key.as_str());
        assert!(packet.body.get("answerId").is_some());
        let mut closed = note.clone();
        closed.open = false;
        assert!(
            answer_packet(&[closed], &request(&note.key)).is_none(),
            "not offered"
        );
    }
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

    /// D4: a chat's messages are kept on its note, held to their limits.
    #[test]
    fn a_chat_keeps_its_messages() {
        let chat = json!({
            "id": "0|com.chat|1", "app": "Chat", "title": "Ann", "text": "Lunch?",
            "requestReplyId": "0|com.chat|1",
            "messages": [
                {"from": "Ann", "text": "Are you free?", "time": 1000},
                {"from": "", "text": "Yes", "time": 2000, "mine": true},
                {"from": "Ann", "text": "Lunch?", "time": 3000}
            ]
        });
        let note = answerable("phone", &chat, 100).unwrap();
        assert_eq!(note.messages.len(), 3);
        assert!(note.messages[1].mine);
        assert_eq!(note.messages[2].text, "Lunch?");
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
            open: false,
            messages: vec![],
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
        // E1: it carries the request's id, for the phone to say how it went.
        assert_eq!(
            press.body["answerId"],
            "00000000-0000-0000-0000-000000000000"
        );
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

    /// N9: KDE Connect names the app `appName`, and re-sends the phone's
    /// notifications at connect marked `silent`, which must not each pop up.
    #[test]
    fn a_kde_connect_notification_is_read_in_its_own_words() {
        assert_eq!(
            kde_to_show(&json!({"id": "1", "appName": "Chat", "title": "Ana", "text": "hi"})),
            Some(("Chat".into(), "Ana".into(), "hi".into()))
        );
        assert_eq!(
            kde_to_show(&json!({"id": "2", "appName": "Chat", "ticker": "Ana: hi"})),
            Some(("Chat".into(), "Notification".into(), "Ana: hi".into())),
            "only a ticker: its text"
        );
        assert_eq!(
            kde_to_show(&json!({"id": "3", "appName": "Chat", "title": "Ana", "silent": true})),
            None,
            "one already there at connect"
        );
        assert_eq!(kde_to_show(&json!({"id": "4", "isCancel": true})), None);
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

    #[test]
    fn how_long_a_notification_waited_is_read_capped_and_ignored_when_bad() {
        assert_eq!(waited_secs(&json!({"waited": 125})), 125);
        assert_eq!(waited_secs(&json!({})), 0, "sent at once");
        assert_eq!(waited_secs(&json!({"waited": "125"})), 0, "not a number");
        assert_eq!(waited_secs(&json!({"waited": -5})), 0);
        assert_eq!(waited_secs(&json!({"waited": 1.5})), 0);
        assert_eq!(
            waited_secs(&json!({"waited": 10_000_000u64})),
            86_400,
            "a day at most"
        );
    }

    #[test]
    fn a_notification_that_waited_a_minute_or_more_says_it_is_late() {
        assert_eq!(late_line(0), None);
        assert_eq!(late_line(59), None, "a few retries are not late");
        assert_eq!(late_line(60).as_deref(), Some("Sent 1 min late"));
        assert_eq!(late_line(43 * 60 + 20).as_deref(), Some("Sent 43 min late"));
        assert_eq!(late_line(3600).as_deref(), Some("Sent 1 h late"));
        assert_eq!(
            late_line(2 * 3600 + 5 * 60).as_deref(),
            Some("Sent 2 h 5 min late")
        );
        assert_eq!(with_late("Lunch?", 120), "Lunch?\nSent 2 min late");
        assert_eq!(with_late("", 120), "Sent 2 min late");
        assert_eq!(with_late("Lunch?", 10), "Lunch?");
    }

    #[test]
    fn a_notification_that_asks_is_acknowledged_by_its_id() {
        let ack = ack_for(&json!({"id": "0|com.chat|1", "ack": true}), false).unwrap();
        assert_eq!(ack.packet_type, ACK);
        assert_eq!(ack.body, json!({"id": "0|com.chat|1"}));
        let refused = ack_for(&json!({"id": "k", "ack": true}), true).unwrap();
        assert_eq!(refused.body, json!({"id": "k", "refused": true}));
        assert!(ack_for(&json!({"id": "k"}), false).is_none(), "not asked");
        assert!(ack_for(&json!({"id": "k", "ack": "yes"}), false).is_none());
        assert!(ack_for(&json!({"ack": true}), false).is_none(), "no id");
        assert!(ack_for(&json!({"id": "", "ack": true}), false).is_none());
        assert!(
            ack_for(&json!({"id": "k", "ack": true, "isCancel": true}), false).is_none(),
            "a cancel needs no answer"
        );
        // An id this computer would not keep to answer is still answered.
        let odd = "0|com.chat|1|tag\u{1}".to_string() + &"x".repeat(600);
        let ack = ack_for(&json!({"id": odd, "ack": true}), false).unwrap();
        assert_eq!(ack.body["id"], odd.as_str());
    }

    struct Phone(std::sync::Mutex<Vec<NetworkPacket>>);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-a"
        }
        fn connection_id(&self) -> &str {
            "phone-a"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// L1: a notification closed here, known by its tag, becomes a clear
    /// for its phone; an unknown tag, none.
    #[test]
    fn a_notification_closed_here_is_cleared_on_its_phone() {
        let dir = pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        std::fs::create_dir_all(dir).unwrap();
        let tag = tag_for("phone-l1", "0|com.chat|7|null|10");
        remember_note(&tag, "phone-l1", "0|com.chat|7|null|10");
        cleared_here(&tag);
        cleared_here(&tag_for("phone-l1", "never shown"));
        let taken: Vec<_> = pocketdesk_core::local::note_clears::take_all(dir, unix_now())
            .into_iter()
            .filter(|c| c.device_id == "phone-l1")
            .map(|c| c.key)
            .collect();
        assert_eq!(taken, vec!["0|com.chat|7|null|10".to_string()]);
    }

    /// K2b review: only a name the phone gave can be muted.
    #[test]
    fn only_a_given_name_is_muteable() {
        assert!(given_name(&json!({"app": "Gmail"}), "app"));
        assert!(!given_name(&json!({"app": "  "}), "app"));
        assert!(!given_name(&json!({"title": "x"}), "app"));
        assert!(given_name(&json!({"appName": "Chat"}), "appName"));
    }

    #[test]
    fn one_marked_as_come_while_another_computer_was_in_use_is_held() {
        assert!(marked_elsewhere(&serde_json::json!({ "elsewhere": true })));
        assert!(!marked_elsewhere(
            &serde_json::json!({ "elsewhere": "yes" })
        ));
        assert!(!marked_elsewhere(&serde_json::json!({})));
        assert!(held_here(
            false,
            marked_elsewhere(&serde_json::json!({ "elsewhere": true }))
        ));
    }

    #[test]
    fn a_notification_from_a_phone_in_hand_is_held() {
        assert!(phone_in_use(&serde_json::json!({ "phoneInUse": true })));
        assert!(!phone_in_use(&serde_json::json!({ "phoneInUse": false })));
        assert!(!phone_in_use(&serde_json::json!({ "phoneInUse": "yes" })));
        assert!(!phone_in_use(&serde_json::json!({})));
        // Held as a mute or a pause holds it.
        assert!(held_here(false, true));
        assert!(held_here(true, false));
    }

    /// K2b: an app muted here is muted for the next notification, whatever
    /// the case of its name.
    #[test]
    fn a_muted_app_is_muted_here() {
        let dir = pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        std::fs::create_dir_all(dir).unwrap();
        assert!(!muted_here("Mail (K2b test)"));
        pocketdesk_core::local::muted_apps::mute(dir, "Mail (K2b test)", 1).unwrap();
        assert!(muted_here("mail (k2b test)"));
        pocketdesk_core::local::muted_apps::unmute(dir, "Mail (K2b test)").unwrap();
        assert!(!muted_here("Mail (K2b test)"));
    }

    /// K2a: a pause holds every app's notifications, phones' and KDE
    /// devices' alike (both ask `held_here`).
    #[test]
    fn a_pause_holds_every_app_s_notifications() {
        assert!(!held_here(false, false));
        assert!(held_here(true, false), "a muted app's");
        let _paused = crate::paused::paused_for_test();
        assert!(held_here(false, false));
    }

    /// E1 review: a phone's word is kept only for an answer sent to that
    /// phone and waiting, and a word out of bounds is dropped.
    #[tokio::test]
    async fn a_phone_s_word_is_kept_only_for_its_own_answer() {
        let dir = pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        std::fs::create_dir_all(dir).unwrap();
        let sent = |id: &str, device: &str| answers::Outcome {
            id: id.to_string(),
            device_id: device.to_string(),
            key: "k".to_string(),
            state: answers::State::Sent,
            at_unix: 1,
            said: None,
        };
        let theirs = "e1e1e1e1-0000-4000-8000-00000000a001";
        let mine = "e1e1e1e1-0000-4000-8000-00000000a002";
        let refused = "e1e1e1e1-0000-4000-8000-00000000a003";
        for (id, device) in [(theirs, "phone-b"), (mine, "phone-a"), (refused, "phone-a")] {
            answers::record(dir, sent(id, device)).unwrap();
        }
        let plugin = NotificationPlugin;
        let phone = Phone(Default::default());
        let word = |body: serde_json::Value| {
            NetworkPacket::new(notification_action::NOTIFICATION_ANSWERED).with_body(body)
        };
        for body in [
            json!({"answerId": theirs, "ok": true}),
            json!({"answerId": mine, "ok": true}),
            json!({"answerId": refused, "ok": false, "reason": "the app refused it"}),
            json!({"answerId": "../x", "ok": false}),
        ] {
            plugin.handle_packet(&word(body), &phone).await.unwrap();
        }
        let kept = answers::read(dir);
        let state = |id: &str| kept.iter().find(|a| a.id == id).unwrap().state.clone();
        assert_eq!(
            state(theirs),
            answers::State::Sent,
            "another phone's answer"
        );
        assert_eq!(state(mine), answers::State::Handed);
        assert_eq!(
            state(refused),
            answers::State::NotSent {
                reason: "the app refused it".to_string()
            }
        );
        assert!(phone.0.lock().unwrap().is_empty(), "nothing answered");
    }

    /// A phone with Notifications off is told its notification was refused
    /// (C3), so it does not keep it waiting to send again.
    #[tokio::test]
    async fn a_refused_notification_that_asked_is_answered_refused() {
        let phone = Phone(Default::default());
        let asked = NetworkPacket::new("kdeconnect.notification")
            .with_body(json!({"id": "k", "app": "Chat", "title": "Ann", "ack": true}));
        crate::plugins::answer_denied(&asked, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, ACK);
        assert_eq!(sent[0].body, json!({"id": "k", "refused": true}));
        // One that did not ask gets nothing, as before.
        let plain = NetworkPacket::new("kdeconnect.notification").with_body(json!({"id": "k"}));
        crate::plugins::answer_denied(&plain, &phone).await;
        assert_eq!(phone.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_late_notification_is_kept_from_when_it_came_on_the_phone() {
        let body = json!({
            "id": "0|com.chat|1", "app": "Chat", "title": "Ann", "text": "Lunch?",
            "actions": ["Mark read"], "waited": 600,
        });
        let note = answerable("phone", &body, 10_000).unwrap();
        assert_eq!(note.at_unix, 9_400);
        assert_eq!(
            note.text, "Lunch?",
            "the text kept to answer is the phone's own"
        );
    }

    /// P4 review: the owner's words go in the content, a flag in the meta.
    #[test]
    fn held_by_words_names_its_line_only_in_the_content() {
        let (mut meta, mut content) = (json!({"app": "Shop"}), json!({"title": "t"}));
        held_by_fields(&None, &mut meta, &mut content);
        assert_eq!(meta, json!({"app": "Shop"}));
        held_by_fields(&Some("sale".into()), &mut meta, &mut content);
        assert_eq!(meta["held_by_words"], json!(true));
        assert_eq!(meta.get("held_by"), None);
        assert_eq!(content["held_by"], json!("sale"));
    }

    /// P4 review: a call's note is never held by its words.
    #[test]
    fn a_call_note_is_known_by_its_id() {
        assert!(is_call(&json!({"id": "pocketdesk:call:3"})));
        assert!(!is_call(&json!({"id": "0|app|1|null|10"})));
        assert!(!is_call(&json!({})));
    }

    /// P5: an alarm is never held by the phone in hand or another computer
    /// in use; it rings only in time and with its setting on.
    #[test]
    fn an_alarm_ignores_the_marks_and_rings_only_in_time() {
        assert_eq!(alarm_marks(false, true, true), (true, true));
        assert_eq!(alarm_marks(true, true, true), (false, false));
        assert!(rings(true, true, 0));
        assert!(!rings(true, false, 0), "setting off: a plain notification");
        assert!(!rings(true, true, ALARM_LATE_SECS), "late: not rung");
        assert!(!rings(false, true, 0));
        assert!(is_alarm(&json!({"alarm": true})) && !is_alarm(&json!({"alarm": "yes"})));
    }
}
