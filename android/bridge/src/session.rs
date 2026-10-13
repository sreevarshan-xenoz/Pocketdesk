//! Crate-root session state for the Android bridge.
//! Ported from the Flutter-era bridge (git 4bf3c73) `lib.rs`; the JNI entry
//! points live in `lib.rs`, the API functions in `api.rs`.

use crate::api;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, LazyLock};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::broadcast;

/// Initialize the Linux Link backend (internal, called from api::init_app).
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

/// Global handle for the active control connection writer.
pub(crate) static CONTROL_WRITER: LazyLock<TokioMutex<Option<Arc<TokioMutex<OwnedWriteHalf>>>>> =
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
    std::sync::Mutex<Option<linux_link_core::protocol::kdeconnect::DeviceIdentity>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));

/// mDNS discovery service status flag.
pub(crate) static MDNS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Exponential backoff state for reconnection.
pub(crate) static RECONNECT_BACKOFF: LazyLock<
    std::sync::Mutex<linux_link_core::protocol::backoff::ExponentialBackoff>,
> = LazyLock::new(|| {
    std::sync::Mutex::new(linux_link_core::protocol::backoff::ExponentialBackoff::default())
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
pub(crate) static DECODE_SAMPLES: linux_link_core::streaming::SampleBatch =
    linux_link_core::streaming::SampleBatch::new();
pub(crate) static RENDER_SAMPLES: linux_link_core::streaming::SampleBatch =
    linux_link_core::streaming::SampleBatch::new();

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
    TokioMutex<Option<tokio::sync::mpsc::Receiver<linux_link_core::streaming::EncodedPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));

/// The live session's audio receiver, apart from [`STREAMING_HANDLE`] and
/// [`VIDEO_RX`] for the same reason: `receive_audio` waits on it.
pub(crate) static AUDIO_RX: LazyLock<
    TokioMutex<Option<tokio::sync::mpsc::Receiver<linux_link_core::streaming::AudioPacket>>>,
> = LazyLock::new(|| TokioMutex::new(None));

/// Global handle for the active v2 unified connection.
pub(crate) static V2_HANDLE: LazyLock<TokioMutex<Option<V2Handle>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// Persistent cert directory set from Kotlin (std Mutex for sync access).
pub(crate) static CERT_DIR: LazyLock<std::sync::Mutex<Option<PathBuf>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

/// Currently active CertManager for trust management UI.
pub(crate) static CERT_MANAGER: LazyLock<
    std::sync::Mutex<Option<Arc<linux_link_core::streaming::transport::CertManager>>>,
> = LazyLock::new(|| std::sync::Mutex::new(None));

/// Broadcast channel for incoming KDE Connect packets from the control connection.
/// Kotlin polls these via `poll_incoming_packets`.
pub(crate) static INCOMING_PACKETS: LazyLock<TokioMutex<Option<broadcast::Sender<String>>>> =
    LazyLock::new(|| TokioMutex::new(None));

/// The server's iroh WAN identity (`kdeconnect.linuxlink.endpoint` body JSON),
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

/// Holds the live streaming client and its packet receiver.
pub(crate) struct StreamingHandle {
    pub(crate) address: String,
    pub(crate) port: u16,
    /// Token that can be used to cancel the receive loop.
    pub(crate) cancel: tokio_util::sync::CancellationToken,
    /// JoinHandle of the background `client.start()` task.
    pub(crate) task: tokio::task::JoinHandle<()>,
    /// JoinHandle of the background RTT polling task.
    pub(crate) rtt_task: tokio::task::JoinHandle<()>,
    /// The streaming connection (transport-agnostic handle), kept alive for
    /// sending input events.
    pub(crate) connection: linux_link_core::streaming::SharedConnection,
    /// Live iroh dial for WAN sessions — owns the endpoint and must be
    /// closed (not dropped) when the session ends.
    pub(crate) wan_dial: Option<linux_link_core::streaming::IrohDial>,
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
