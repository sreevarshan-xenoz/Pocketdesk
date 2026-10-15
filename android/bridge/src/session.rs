//! Crate-root session state for the Android bridge.
//! Ported from the Flutter-era bridge (git 4bf3c73) `lib.rs`; the JNI entry
//! points live in `lib.rs`, the API functions in `api.rs`.

use crate::api;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, LazyLock};
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::broadcast;

/// Initialize the Pocketdesk backend (internal, called from api::init_app).
///
/// Logging goes to logcat. This used to install a stdout subscriber, which on
/// Android writes nowhere; being first, it also kept the logcat one out.
pub(crate) fn init_app_impl() {
    crate::logcat::init();
}

/// Maximum number of H.264 frames to drain per `receive_frames` call.
pub(crate) const MAX_FRAMES_PER_RECEIVE: usize = 16;

/// Maximum number of audio packets to drain per `receive_audio` call.
pub(crate) const MAX_AUDIO_PACKETS_PER_RECEIVE: usize = 32;

/// The write half of a control connection (TLS over TCP, phase B).
pub(crate) type ControlWriter =
    tokio::io::WriteHalf<pocketdesk_core::protocol::connection::ControlStream>;

/// Global handle for the active control connection writer.
pub(crate) static CONTROL_WRITER: LazyLock<TokioMutex<Option<Arc<TokioMutex<ControlWriter>>>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Held across `connect_to_peer`'s check, dial and install, so two callers
/// that both find no connection (the session's keeper and the clipboard
/// loop, at session start) open one, not two: the loser's reader would keep
/// its socket open and the three shared slots could end up from different
/// connections (R2 review).
pub(crate) static CONNECT_LOCK: LazyLock<TokioMutex<()>> = LazyLock::new(|| TokioMutex::new(()));

/// `{address, port}` the writer above belongs to, so `connect_to_peer` can
/// tell a live session to the requested desktop from one to another host.
pub(crate) static CONTROL_PEER: LazyLock<TokioMutex<Option<(String, u16)>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Global handle for the active control connection state.
pub(crate) static CONNECTION_STATE: LazyLock<TokioMutex<api::ConnectionState>> =
    LazyLock::new(|| TokioMutex::new(api::ConnectionState::Disconnected));

/// Persistent device identity for the Android client.
pub(crate) static DEVICE_IDENTITY: LazyLock<
    std::sync::Mutex<Option<pocketdesk_core::protocol::kdeconnect::DeviceIdentity>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));

/// mDNS discovery service status flag.
pub(crate) static MDNS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Exponential backoff state for reconnection.
pub(crate) static RECONNECT_BACKOFF: LazyLock<
    std::sync::Mutex<pocketdesk_core::protocol::backoff::ExponentialBackoff>,
> = LazyLock::new(|| {
    std::sync::Mutex::new(pocketdesk_core::protocol::backoff::ExponentialBackoff::default())
});

/// High-level session status for reconnection state machine.
pub(crate) static SESSION_STATUS: LazyLock<std::sync::Mutex<api::SessionStatus>> =
    LazyLock::new(|| std::sync::Mutex::new(api::SessionStatus::Disconnected));

/// The transport's RTT reports from this session, newest last, bounded to
/// [`crate::rates::RTT_SAMPLES`]. The HUD quotes the median of these rather
/// than whichever one-second poll happened to be current — quinn and iroh
/// expose no variance, so successive samples are the only confidence a link
/// figure can carry (roadmap 2056).
pub(crate) static RTT_HISTORY: LazyLock<std::sync::Mutex<Vec<u64>>> =
    LazyLock::new(std::sync::Mutex::default);

/// Cumulative counter reads the HUD's rates are diffed across. Written when the
/// HUD asks for stats, so the window advances on the display's own cadence and
/// a poller that stops polling cannot make the numbers go stale.
pub(crate) static RATE_SNAPSHOTS: LazyLock<std::sync::Mutex<crate::rates::History>> =
    LazyLock::new(std::sync::Mutex::default);

/// Atomic flag indicating whether streaming is active (avoids try_lock race).
pub(crate) static STREAMING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Durations the decoder measures, waiting to be reported to the desktop, which
/// is where a session's percentile tails are computed. `DECODE_SAMPLES` is
/// feed→drained-frame time, `RENDER_SAMPLES` the gap between frames reaching the
/// panel. Both are bounded, so a session that reports on its 1 s poller cannot
/// grow them; a new session clears them (see `install_streaming`).
pub(crate) static DECODE_SAMPLES: pocketdesk_core::streaming::SampleBatch =
    pocketdesk_core::streaming::SampleBatch::new();
pub(crate) static RENDER_SAMPLES: pocketdesk_core::streaming::SampleBatch =
    pocketdesk_core::streaming::SampleBatch::new();

/// True when the live streaming session rode a relay path (iroh WAN without a
/// punched direct path). Updated by the RTT poller; LAN sessions are always
/// direct, so the flag also doubles as the relay-transition latch for the
/// server log.
pub(crate) static SESSION_RELAYED: AtomicBool = AtomicBool::new(false);

/// Live link path for the stats DTO / session UI chip:
/// 0 = unknown (no session, or a WAN connection with no path snapshot yet),
/// 1 = LAN direct (quinn), 2 = WAN direct (punched), 3 = WAN via relay.
pub(crate) static LINK_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Whether the live streaming session was dialed over iroh (WAN) rather than
/// quinn (LAN). Written by `install_streaming` before `STREAMING_ACTIVE`.
pub(crate) static SESSION_IS_WAN: AtomicBool = AtomicBool::new(false);

/// Video received so far this session, as cumulative counters. The HUD's rates
/// are differences across reads of these (see [`crate::rates`]), never a total
/// divided by the session's lifetime.
pub(crate) static STREAMING_FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static STREAMING_BYTE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Why the live session's connection ended, once it has (the desktop's own
/// reason when it refused or ended the session). The decode loop polls it so
/// a closed stream reaches the UI at once instead of after a frame-gap
/// watchdog. Cleared when a session starts or stops.
pub(crate) static STREAM_CLOSED_REASON: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

/// Global handle for the active streaming client session.
pub(crate) static STREAMING_HANDLE: LazyLock<TokioMutex<Option<StreamingHandle>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// The live session's video receiver, kept apart from [`STREAMING_HANDLE`]:
/// `receive_frames` waits on it for up to its timeout, and holding the
/// session handle that long held up every input send behind it.
pub(crate) static VIDEO_RX: LazyLock<
    TokioMutex<Option<tokio::sync::mpsc::Receiver<pocketdesk_core::streaming::EncodedPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));

/// The live session's video byte count, as its frames arrive
/// (`StreamingClient::video_bytes`): the phone's watchdog reads it to tell
/// a slow link from a dead one.
pub(crate) static VIDEO_BYTES: std::sync::Mutex<Option<Arc<AtomicU64>>> =
    std::sync::Mutex::new(None);

/// Video bytes the live session has received so far; 0 with none.
pub(crate) fn video_bytes() -> u64 {
    VIDEO_BYTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or(0, |b| b.load(std::sync::atomic::Ordering::Relaxed))
}

/// The live session's audio receiver, apart from [`STREAMING_HANDLE`] and
/// [`VIDEO_RX`] for the same reason: `receive_audio` waits on it.
pub(crate) static AUDIO_RX: LazyLock<
    TokioMutex<Option<tokio::sync::mpsc::Receiver<pocketdesk_core::streaming::AudioPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));

/// Global handle for the active v2 unified connection.
pub(crate) static V2_HANDLE: LazyLock<TokioMutex<Option<V2Handle>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Persistent cert directory set from Kotlin (std Mutex for sync access).
pub(crate) static CERT_DIR: LazyLock<std::sync::Mutex<Option<PathBuf>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

/// Currently active CertManager for trust management UI.
pub(crate) static CERT_MANAGER: LazyLock<
    std::sync::Mutex<Option<Arc<pocketdesk_core::streaming::transport::CertManager>>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));

/// Broadcast channel for incoming KDE Connect packets from the control connection.
/// Kotlin polls these via `poll_incoming_packets`.
pub(crate) static INCOMING_PACKETS: LazyLock<TokioMutex<Option<broadcast::Sender<String>>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// The server's iroh WAN identity (`kdeconnect.pocketdesk.endpoint` body JSON),
/// cached by the control reader so WAN dialing survives the poll loop's absence.
pub(crate) static WAN_IDENTITY: LazyLock<TokioMutex<Option<String>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// How the connected desktop can be woken (`kdeconnect.pocketdesk.wake_info`
/// body JSON, N6), cached by the control reader with the computer (`host`,
/// `port`) whose connection carried it: it is handed out only for that one.
pub(crate) static WAKE_INFO: LazyLock<TokioMutex<Option<(String, u16, String)>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Who drives, as the desktop last said (`kdeconnect.pocketdesk.control`
/// body JSON, N7), with the computer whose connection carried it.
pub(crate) static CONTROL_STATE: LazyLock<TokioMutex<Option<(String, u16, String)>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Latch set when the desktop pushes `kdeconnect.findmydevice` `{ring:true}`
/// (R3 Tier-2 #11 siren). Kotlin consumes-and-clears it via `checkSiren`.
pub(crate) static SIREN_RINGING: AtomicBool = AtomicBool::new(false);

/// Last `kdeconnect.pair` decision body pushed by the desktop on the
/// persistent control connection (R3 Tier-2 #11b). Consumed-and-cleared by
/// `check_pair_result`.
pub(crate) static PAIR_RESULT: LazyLock<TokioMutex<Option<serde_json::Value>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Desktop-originated notifications (Tier-2 #11c) waiting to be posted as
/// Android notifications. Bounded; drained by `take_pending_notifications`.
pub(crate) static PENDING_NOTIFICATIONS: LazyLock<std::sync::Mutex<Vec<serde_json::Value>>> =
    LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// Queue one captured `kdeconnect.notification` body for the UI.
pub(crate) fn queue_notification(body: &serde_json::Value, source: &str) {
    let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
        return;
    };
    let entry = serde_json::json!({
        "id": id,
        "app": body.get("app").and_then(|v| v.as_str()).unwrap_or(source),
        "title": body.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "text": body.get("text").and_then(|v| v.as_str()).unwrap_or(""),
        "source": source,
    });
    let mut q = PENDING_NOTIFICATIONS.lock().expect("notification queue");
    if q.iter().any(|n| n.get("id") == entry.get("id")) {
        return; // desktop re-sends on reconnect; keep one per id
    }
    q.push(entry);
    const MAX_QUEUED: usize = 32;
    if q.len() > MAX_QUEUED {
        q.remove(0);
    }
}

/// Replies the desktop asked this phone to send as texts (N5b
/// `kdeconnect.sms.request`), waiting for the app. Bounded; drained by
/// `take_sms_requests`.
pub(crate) static PENDING_SMS_REQUESTS: LazyLock<std::sync::Mutex<Vec<serde_json::Value>>> =
    LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

/// Queue one `kdeconnect.sms.request` body: `{id, address, body}`, all
/// strings, or it is dropped; with the computer (`host`, `port`) whose
/// connection carried it. One per id.
pub(crate) fn queue_sms_request(body: &serde_json::Value, host: &str, port: u16) {
    let field = |key: &str| body.get(key).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(id), Some(address), Some(text)) = (field("id"), field("address"), field("body"))
    else {
        return;
    };
    let mut q = PENDING_SMS_REQUESTS.lock().expect("sms queue");
    if q.iter()
        .any(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        return;
    }
    q.push(serde_json::json!({
        "id": id,
        "address": address,
        "body": text,
        "fromHost": host,
        "fromPort": port,
    }));
    const MAX_QUEUED: usize = 50;
    if q.len() > MAX_QUEUED {
        q.remove(0);
    }
}

/// The most offers waiting: a whole folder send (T6b, up to 10 000 files,
/// offered at once) and a few single files beside it.
pub(crate) const MAX_QUEUED_OFFERS: usize = 10_000 + 64;

/// Offers waiting, oldest first, with the ids they hold.
#[derive(Default)]
pub(crate) struct OfferQueue {
    offers: std::collections::VecDeque<serde_json::Value>,
    ids: std::collections::HashSet<String>,
}

impl OfferQueue {
    /// Queue `offer` under `id`, once per id; past the bound the oldest goes.
    fn push(&mut self, id: String, offer: serde_json::Value) {
        if !self.ids.insert(id) {
            return;
        }
        self.offers.push_back(offer);
        if self.offers.len() > MAX_QUEUED_OFFERS
            && let Some(old) = self.offers.pop_front()
            && let Some(old_id) = old.get("id").and_then(|v| v.as_str())
        {
            self.ids.remove(old_id);
        }
    }

    /// Every offer waiting, oldest first; the queue is left empty.
    fn take(&mut self) -> Vec<serde_json::Value> {
        self.ids.clear();
        self.offers.drain(..).collect()
    }

    pub(crate) fn clear(&mut self) {
        self.take();
    }
}

/// Files the desktop offered (F7b `file_offer`), waiting for the session
/// to start their download. Bounded; drained by [`take_file_offers_json`].
pub(crate) static PENDING_OFFERS: LazyLock<std::sync::Mutex<OfferQueue>> =
    LazyLock::new(Default::default);

/// Tests that touch the offer queue run one at a time: it is process-wide,
/// and one test's drain or overflow would take another's offer.
#[cfg(test)]
pub(crate) static OFFERS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Queue one offer `{id, name, size}`, once per id.
pub(crate) fn queue_file_offer(body: &serde_json::Value) {
    let (Some(id), Some(name), Some(size)) = (
        body.get("id").and_then(|v| v.as_str()),
        body.get("name").and_then(|v| v.as_str()),
        body.get("size").and_then(|v| v.as_u64()),
    ) else {
        return;
    };
    let mut entry = serde_json::json!({ "id": id, "name": name, "size": size });
    // The file's id, so a retry continues a broken download of it (T6a).
    if let Some(source) = body
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| pocketdesk_core::protocol::partials::valid_source(s))
    {
        entry["source"] = serde_json::json!(source);
    }
    // A file of a folder send (T6b): its place, when every name in it is one
    // the phone may make. One that is not is refused, never saved flat or
    // renamed: the app declines it with the reason.
    if body.get("folderId").is_some() {
        match folder_part(body) {
            Some(folder) => {
                entry["folderId"] = serde_json::json!(folder.0);
                entry["folder"] = serde_json::json!(folder.1);
                entry["path"] = serde_json::json!(folder.2);
            }
            None => {
                entry["folderRefused"] = serde_json::json!("an unsafe place in a folder");
            }
        }
    }
    PENDING_OFFERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(id.to_string(), entry);
}

/// An offer's folder fields (T6b), if all are there and safe: the send's
/// id (32 hex digits), the folder's name and the file's path under it.
fn folder_part(body: &serde_json::Value) -> Option<(String, String, Vec<String>)> {
    use pocketdesk_core::protocol::relpath;
    let id = body.get("folderId")?.as_str()?;
    let folder = body.get("folder")?.as_str()?;
    let path: Vec<String> = body
        .get("path")?
        .as_array()?
        .iter()
        .map(|p| p.as_str().map(str::to_string))
        .collect::<Option<_>>()?;
    let id_ok = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    (id_ok
        && relpath::component_problem(folder).is_none()
        && relpath::path_problem(&path).is_none())
    .then(|| (id.to_string(), folder.to_string(), path))
}

/// Answers to the phone's folder requests (`folder_fetch_reply`), by the
/// path asked: kept here as the reader sees them, so neither a burst of the
/// folder's offers nor a second request can take one from its waiter.
static FOLDER_REPLIES: LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
> = LazyLock::new(Default::default);
/// Woken as each folder answer arrives.
pub(crate) static FOLDER_REPLY: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Keep a folder answer for its waiter.
pub(crate) fn latch_folder_reply(body: &serde_json::Value) {
    let Some(path) = body.get("path").and_then(|v| v.as_str()) else {
        return;
    };
    let mut replies = FOLDER_REPLIES.lock().unwrap_or_else(|e| e.into_inner());
    // Answers nobody waited for are not kept forever.
    if replies.len() >= 16 {
        replies.clear();
    }
    replies.insert(path.to_string(), body.clone());
    drop(replies);
    FOLDER_REPLY.notify_waiters();
}

/// The answer to the folder request for `path`, once.
pub(crate) fn take_folder_reply(path: &str) -> Option<serde_json::Value> {
    FOLDER_REPLIES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(path)
}

/// Packets the control reader keeps itself rather than broadcasting to the
/// pollers: offers (thousands at once for a folder) and folder answers.
pub(crate) fn kept_from_pollers(packet_type: &str) -> bool {
    packet_type == crate::download::FILE_OFFER || packet_type == crate::download::FOLDER_FETCH_REPLY
}

/// Drain the queued offers as a JSON array.
pub(crate) fn take_file_offers_json() -> String {
    let drained = PENDING_OFFERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    serde_json::to_string(&drained).unwrap_or_else(|_| "[]".to_string())
}

/// Holds the live streaming client and its packet receiver.
pub(crate) struct StreamingHandle {
    pub(crate) address: String,
    pub(crate) port: u16,
    /// The monitor this session captures (`None`: the desktop's choice). A
    /// connect for another monitor is a new session, not this one.
    pub(crate) monitor_index: Option<u32>,
    /// Token that can be used to cancel the receive loop.
    pub(crate) cancel: tokio_util::sync::CancellationToken,
    /// JoinHandle of the background `client.start()` task.
    pub(crate) task: tokio::task::JoinHandle<()>,
    /// JoinHandle of the background RTT polling task.
    pub(crate) rtt_task: tokio::task::JoinHandle<()>,
    /// The streaming connection (transport-agnostic handle), kept alive for
    /// sending input events.
    pub(crate) connection: pocketdesk_core::streaming::SharedConnection,
    /// Live iroh dial for WAN sessions — owns the endpoint and must be
    /// closed (not dropped) when the session ends.
    pub(crate) wan_dial: Option<pocketdesk_core::streaming::IrohDial>,
}

/// Holds the unified v2 connection and its persistent control streams.
pub(crate) struct V2Handle {
    pub(crate) connection: quinn::Connection,
    /// Reserved control stream; the keepalive task holds its own clone, this
    /// keeps the stream alive with the handle.
    #[allow(dead_code)]
    pub(crate) control_send: Arc<TokioMutex<quinn::SendStream>>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

/// Record one RTT poll from the stats task.
pub(crate) fn update_streaming_rtt(rtt_us: u64) {
    let mut history = RTT_HISTORY.lock().unwrap();
    history.push(rtt_us);
    crate::rates::trim(&mut history, crate::rates::RTT_SAMPLES);
}

/// Which streaming connect is the live one. A connect can take seconds (a
/// LAN attempt, then the WAN fallback); a stop or a newer connect that comes
/// meanwhile retires it, and a retired connect must not install its session
/// when it finally lands — it would replace the session the phone moved on
/// to, and its decoder would come back to feed a view that is gone.
#[derive(Default)]
pub(crate) struct ConnectGeneration(std::sync::atomic::AtomicU64);

impl ConnectGeneration {
    /// Start a connect; it is current until retired.
    pub(crate) fn begin(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    /// Retire every connect in flight (a stop).
    pub(crate) fn retire(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst) == generation
    }
}

/// The bridge's one [`ConnectGeneration`].
pub(crate) static CONNECT_GENERATION: ConnectGeneration =
    ConnectGeneration(std::sync::atomic::AtomicU64::new(0));

#[cfg(test)]
mod connect_generation_tests {
    use super::ConnectGeneration;

    #[test]
    fn a_stop_retires_a_connect_still_in_flight() {
        let gens = ConnectGeneration::default();
        let mine = gens.begin();
        assert!(gens.is_current(mine));
        gens.retire();
        assert!(!gens.is_current(mine), "stopped while it was connecting");
    }

    #[test]
    fn a_newer_connect_retires_an_older_one() {
        let gens = ConnectGeneration::default();
        let old = gens.begin();
        let new = gens.begin();
        assert!(!gens.is_current(old));
        assert!(gens.is_current(new));
    }
}
