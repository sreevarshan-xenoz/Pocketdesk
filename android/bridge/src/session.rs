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
/// strings, or it is dropped. One per id.
pub(crate) fn queue_sms_request(body: &serde_json::Value) {
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
    q.push(serde_json::json!({ "id": id, "address": address, "body": text }));
    const MAX_QUEUED: usize = 50;
    if q.len() > MAX_QUEUED {
        q.remove(0);
    }
}

/// Files the desktop offered (F7b `file_offer`), waiting for the session
/// to start their download. Bounded; drained by [`take_file_offers_json`].
pub(crate) static PENDING_OFFERS: LazyLock<std::sync::Mutex<Vec<serde_json::Value>>> =
    LazyLock::new(|| std::sync::Mutex::new(Vec::new()));

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
    let entry = serde_json::json!({ "id": id, "name": name, "size": size });
    let mut q = PENDING_OFFERS.lock().unwrap_or_else(|e| e.into_inner());
    if q.iter().any(|o| o.get("id") == entry.get("id")) {
        return;
    }
    q.push(entry);
    const MAX_QUEUED: usize = 16;
    if q.len() > MAX_QUEUED {
        q.remove(0);
    }
}

/// Drain the queued offers as a JSON array.
pub(crate) fn take_file_offers_json() -> String {
    let drained = std::mem::take(&mut *PENDING_OFFERS.lock().unwrap_or_else(|e| e.into_inner()));
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
