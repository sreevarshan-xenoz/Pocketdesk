//! A KDE Connect device's link after the handshake (3b): pairing both ways,
//! held for the owner with the code both screens show; unpairing; ping.
//! Once paired (3c), what its permissions allow: its battery, the
//! clipboard both ways, ringing it from here, and files it sends. Anything else it sends, Pocketdesk's own packets
//! among them, is not taken.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::kdeconnect::{self, SeenDevice};
use pocketdesk_core::local::kdeconnect_devices::{self as devices, KdeDevice, Pending};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::handshake::{DeviceLink, MAX_LINE};
use super::pairing::{self, PairSays};

/// The packet type of a ping, both ways.
pub const PING: &str = "kdeconnect.ping";

/// A device's battery report (3c).
pub const BATTERY: &str = "kdeconnect.battery";

/// Clipboard text, both ways (3c).
pub const CLIPBOARD: &str = "kdeconnect.clipboard";

/// Ring a device, to find it (3c): KDE Connect's find-my-phone request
/// (3c review: `kdeconnect.findmydevice` is Pocketdesk's own siren).
pub const RING: &str = "kdeconnect.findmyphone.request";

/// The longest line a paired device's link reads, as the Pocketdesk control
/// channel's (a long copied text is one line); an unpaired one's is
/// [`MAX_LINE`].
const PAIRED_LINE: usize = 4 * 1024 * 1024;

/// An unpaired link silent this long is closed (3a review): it holds one
/// of the few unpaired places, and the device dials again at the next Find.
const UNPAIRED_IDLE: Duration = Duration::from_secs(300);

/// The longest a write to a device, or to the clipboard, may take.
const WRITE_WAIT: Duration = Duration::from_secs(10);

/// A device's next pairing request on a link is taken this long after its
/// last (3b review: each one is a file written and a notification).
const REQUEST_GAP_SECS: u64 = 10;

/// Notifications about KDE Connect devices, at most so many a minute in
/// all (3b review: requests and pings from many links).
const TOASTS_PER_MINUTE: usize = 5;

/// What the desktop app (through the server) asks of a device's link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Ask the device to pair.
    PairFromHere,
    /// The owner's answer to the device's request, with the code the owner
    /// saw: taken only for the request that showed it.
    Answer { yes: bool, code: String },
    /// Unpair the device.
    Unpair,
    /// Ping it.
    Ping,
    /// The computer's clipboard changed to this text: sent to a paired
    /// device allowed the clipboard.
    Clipboard(String),
    /// Ring a paired device allowed it.
    Ring,
}

/// A live link: its token, the certificate it came with, the way to tell
/// it of an event, and what closes it when a newer link takes its place (a
/// token, not an event: an event is dropped when the link's queue is full,
/// 3c review).
type LiveLink = (
    u64,
    [u8; 32],
    mpsc::Sender<Event>,
    tokio_util::sync::CancellationToken,
);

/// The links up now, by device id, each with a token of its own.
static LIVE: LazyLock<Mutex<HashMap<String, LiveLink>>> = LazyLock::new(Default::default);

/// The next link's token.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The devices with a link up now.
pub fn live() -> Vec<String> {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect()
}

/// The paired devices with a link up now: the clipboard watcher's (3c
/// review: an unpaired link had kept it awake, with no one to tell).
pub fn live_paired() -> Vec<String> {
    let live = live();
    if live.is_empty() {
        return live;
    }
    let Ok(dir) = crate::state::state_dir() else {
        return Vec::new();
    };
    paired_among(live, &devices::devices(&dir))
}

/// Those of `live` that are paired.
fn paired_among(live: Vec<String>, paired: &[KdeDevice]) -> Vec<String> {
    live.into_iter()
        .filter(|id| paired.iter().any(|d| &d.device_id == id))
        .collect()
}

/// Tell `device_id`'s link of `event`; false when it has none.
pub fn tell(device_id: &str, event: Event) -> bool {
    let link = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .map(|(_, _, tx, _)| tx.clone());
    link.is_some_and(|tx| tx.try_send(event).is_ok())
}

/// A link counted live while held. A newer one from the same device, with
/// the same certificate, takes its place and closes it; one with another
/// certificate is refused while it lives (3b review: a device claiming the
/// id would otherwise close the real one's link, and its pairing request
/// with it). A link going removes only itself (the A71's new link had been
/// dropped from the list by its old one going).
struct Live(String, u64, tokio_util::sync::CancellationToken);

impl Live {
    fn new(device_id: &str, cert_hash: [u8; 32], tx: mpsc::Sender<Event>) -> Option<Self> {
        let token = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let replaced = tokio_util::sync::CancellationToken::new();
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if live
            .get(device_id)
            .is_some_and(|(_, theirs, _, _)| *theirs != cert_hash)
        {
            return None;
        }
        let entry = (token, cert_hash, tx, replaced.clone());
        if let Some((_, _, _, older)) = live.insert(device_id.to_string(), entry) {
            older.cancel();
        }
        Some(Self(device_id.to_string(), token, replaced))
    }

    /// Cancelled when a newer link takes this one's place.
    fn replaced(&self) -> tokio_util::sync::CancellationToken {
        self.2.clone()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if live
            .get(&self.0)
            .is_some_and(|(token, _, _, _)| *token == self.1)
        {
            live.remove(&self.0);
        }
    }
}

/// A pairing in progress on this link.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Waiting {
    since: u64,
    from_device: bool,
    /// The code shown for it.
    code: String,
    /// Ours, and the device accepted: the owner confirms here.
    accepted: bool,
}

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// At most `per_minute` in any minute.
struct Allowance {
    per_minute: usize,
    given: std::collections::VecDeque<std::time::Instant>,
}

impl Allowance {
    const fn new(per_minute: usize) -> Self {
        Self {
            per_minute,
            given: std::collections::VecDeque::new(),
        }
    }

    fn allow(&mut self, now: std::time::Instant) -> bool {
        while self
            .given
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            self.given.pop_front();
        }
        if self.given.len() >= self.per_minute {
            return false;
        }
        self.given.push_back(now);
        true
    }
}

static TOASTS: Mutex<Allowance> = Mutex::new(Allowance::new(TOASTS_PER_MINUTE));

/// Pairing prompts have an allowance of their own (3c review: a paired
/// device's pings had spent the shared one, hiding another's request).
static PROMPTS: Mutex<Allowance> = Mutex::new(Allowance::new(TOASTS_PER_MINUTE));

/// Show `summary` and `body` on the desktop, off the link's task, within
/// [`TOASTS_PER_MINUTE`] (past it, the log has it). Not in tests: a toast's
/// thread outliving the test process crashes it.
pub(super) fn notify(summary: &'static str, body: String) {
    show(&TOASTS, summary, body);
}

/// [`notify`] for a pairing prompt, within its own allowance.
fn prompt(summary: &'static str, body: String) {
    show(&PROMPTS, summary, body);
}

fn show(allowance: &Mutex<Allowance>, summary: &'static str, body: String) {
    let allowed = allowance
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allow(std::time::Instant::now());
    if !allowed {
        tracing::info!("{summary}: {body}");
        return;
    }
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        if !crate::platform::notify(summary, &body, Duration::from_secs(10)) {
            tracing::info!("{summary}: {body}");
        }
    });
}

/// The link, until it closes. `unpaired`: the place an unpaired device's
/// link holds, given back once it pairs; none when the device is paired
/// with the certificate it brought (the handshake held it to the pin).
pub async fn run<S: AsyncRead + AsyncWrite + Unpin>(
    link: DeviceLink<S>,
    our_cert_der: &[u8],
    unpaired: Option<super::Slot>,
    address: Option<std::net::IpAddr>,
) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    run_in(dir, link, our_cert_der, unpaired, address).await;
}

/// [`run`], with the state folder `dir`.
async fn run_in<S: AsyncRead + AsyncWrite + Unpin>(
    dir: PathBuf,
    link: DeviceLink<S>,
    our_cert_der: &[u8],
    unpaired: Option<super::Slot>,
    address: Option<std::net::IpAddr>,
) {
    let device_id = link.hello.device_id.clone();
    let name = link.hello.device_name.clone();
    let (tx, mut events) = mpsc::channel(8);
    let Some(_live) = Live::new(&device_id, link.cert_hash, tx) else {
        tracing::warn!(
            device_id,
            "A KDE Connect link refused: this device is connected with another certificate"
        );
        return;
    };
    let keys = (
        pairing::public_key_der(our_cert_der),
        pairing::public_key_der(&link.cert_der),
    );
    let mut state = State {
        dir,
        device_id: device_id.clone(),
        name,
        cert_hash: link.cert_hash,
        keys,
        paired: unpaired.is_none(),
        waiting: None,
        last_asked: 0,
        unpaired,
        clipboard_in: None,
        file_in: None,
        told_unpaired: false,
    };
    let replaced = _live.replaced();
    let (reader, mut writer) = tokio::io::split(link.stream);
    let mut reader = PacketReader::new(reader);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut heard = tokio::time::Instant::now();
    'link: loop {
        reader.limit = if state.paired { PAIRED_LINE } else { MAX_LINE };
        if !state.paired && state.waiting.is_none() && heard.elapsed() >= UNPAIRED_IDLE {
            tracing::info!(device_id, "An unpaired KDE Connect link went quiet; closed");
            break;
        }
        let out = tokio::select! {
            line = reader.next() => match line {
                Some(packet) => {
                    heard = tokio::time::Instant::now();
                    state.packet(&packet)
                }
                None => break,
            },
            Some(event) = events.recv() => state.event(event),
            _ = replaced.cancelled() => break,
            _ = tick.tick() => state.expire(),
        };
        for packet in out {
            // A device that stops reading must not hold the link's loop
            // (its ticks, its time-outs, its replacing) for good.
            match tokio::time::timeout(WRITE_WAIT, send(&mut writer, &packet)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break 'link,
                Err(_) => {
                    tracing::info!(device_id, "A KDE Connect device stopped reading; closed");
                    break 'link;
                }
            }
        }
        if let Some(offer) = state.file_in.take() {
            match super::share::admit(&device_id) {
                // Fetched beside the link, which keeps answering.
                Some(ticket) => {
                    tokio::spawn(super::share::fetch(
                        state.dir.clone(),
                        device_id.clone(),
                        state.name.clone(),
                        address,
                        offer,
                        ticket,
                    ));
                }
                None => tracing::info!(
                    device_id,
                    "A file from a KDE Connect device refused: too many of its files on the way"
                ),
            }
        }
        if let Some(text) = state.clipboard_in.take() {
            // Not sent back to the device it came from.
            crate::clipboard_push::note_from_phone(&device_id, &text);
            match tokio::time::timeout(WRITE_WAIT, crate::plugins::clipboard::set_clipboard(&text))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("The clipboard from a KDE Connect device was not set: {e}")
                }
                Err(_) => tracing::warn!(
                    "The clipboard from a KDE Connect device was not set: the clipboard did not answer"
                ),
            }
        }
    }
    state.end_waiting(now_unix());
    tracing::info!(device_id, "A KDE Connect device went");
}

/// A link's pairing state, and what it sends in answer.
struct State {
    dir: PathBuf,
    device_id: String,
    name: String,
    cert_hash: [u8; 32],
    /// Our public key and the device's, for the code.
    keys: (Option<Vec<u8>>, Option<Vec<u8>>),
    paired: bool,
    waiting: Option<Waiting>,
    /// When the device last asked to pair on this link, unix seconds.
    last_asked: u64,
    /// The unpaired place this link holds, until it pairs.
    unpaired: Option<super::Slot>,
    /// Clipboard text the device sent, for the loop to set.
    clipboard_in: Option<String>,
    /// A file the device offers, for the loop to fetch.
    file_in: Option<super::share::Offer>,
    /// The device was told it is not paired here.
    told_unpaired: bool,
}

impl State {
    fn code(&self, timestamp: u64) -> Option<String> {
        match &self.keys {
            (Some(ours), Some(theirs)) => Some(pairing::code(ours, theirs, timestamp)),
            _ => None,
        }
    }

    /// A packet from the device; what to send back.
    fn packet(&mut self, packet: &NetworkPacket) -> Vec<NetworkPacket> {
        if let Some(says) = pairing::read(packet) {
            return self.pair_packet(says);
        }
        if !self.paired {
            // Unpaired, nothing else is taken. A device that sends more
            // than pairing thinks itself paired (this computer forgot it
            // while it was away): it is told it is not, once, as KDE
            // Connect's own desktop does. Not while a pairing waits: a
            // device that accepted ours sends its plugins' packets at once.
            if self.waiting.is_none() && !std::mem::replace(&mut self.told_unpaired, true) {
                tracing::info!(device_id = %self.device_id, "A KDE Connect device that is not paired here was told so");
                return vec![pairing::answer(false)];
            }
            return Vec::new();
        }
        let may = self.permissions();
        match packet.packet_type.as_str() {
            PING => {
                // Quiet in the log: a device may ping as often as it likes.
                tracing::debug!(device_id = %self.device_id, "A ping from a KDE Connect device");
                notify("Ping from a KDE Connect device", self.name.clone());
            }
            BATTERY if may.battery => {
                crate::plugins::phone_battery::take_from(&self.device_id, &packet.body);
            }
            // `kdeconnect.clipboard.connect`, the device's clipboard as it
            // connects, is not taken: each connection would replace the
            // computer's clipboard.
            super::share::SHARE if !may.files => {
                tracing::info!(device_id = %self.device_id, "A file from a KDE Connect device refused: Files is off for it");
                notify(
                    "A KDE Connect device tried to send a file",
                    format!(
                        "{}: allow Files for it in Settings, KDE Connect devices, then send it again",
                        self.name
                    ),
                );
            }
            super::share::SHARE => match super::share::offer_of(packet) {
                Ok(offer) => self.file_in = Some(offer),
                Err(why) => {
                    tracing::info!(device_id = %self.device_id, "A share from a KDE Connect device not taken: {why}");
                    notify(
                        "A share from a KDE Connect device was not taken",
                        format!("{}: {why}", self.name),
                    );
                }
            },
            CLIPBOARD if may.clipboard => {
                if let Some(text) = packet.body.get("content").and_then(|v| v.as_str()) {
                    self.clipboard_in = Some(text.to_string());
                }
            }
            other => {
                // The device's own text, kept short for the log.
                let said: String = other.chars().take(60).collect();
                tracing::debug!(device_id = %self.device_id, "Not taken from a KDE Connect device: {said:?}");
            }
        }
        Vec::new()
    }

    /// What the device may do now: read as each packet comes, so the
    /// owner's change holds at once; nothing once it is not paired.
    fn permissions(&self) -> devices::KdePermissions {
        devices::permissions(&self.dir, &self.device_id).unwrap_or(devices::KdePermissions {
            clipboard: false,
            battery: false,
            find: false,
            files: false,
        })
    }

    fn pair_packet(&mut self, says: PairSays) -> Vec<NetworkPacket> {
        let now = now_unix();
        match says {
            PairSays::Yes(_) if self.paired => vec![pairing::answer(true)],
            // Its yes to ours, or its request while ours waits: accepted
            // there, and the owner confirms the code here (3b review).
            PairSays::Yes(_) if self.waiting.as_ref().is_some_and(|w| !w.from_device) => {
                self.accepted(now);
                Vec::new()
            }
            // One request at a time, and the next some seconds after.
            PairSays::Yes(Some(_))
                if self.waiting.is_some()
                    || now.saturating_sub(self.last_asked) < REQUEST_GAP_SECS =>
            {
                Vec::new()
            }
            PairSays::Yes(Some(timestamp)) => {
                self.last_asked = now;
                if !pairing::timestamp_ok(timestamp, now) {
                    tracing::warn!(device_id = %self.device_id, "A KDE pairing refused: its clock is off");
                    return vec![pairing::answer(false)];
                }
                let Some(code) = self.code(timestamp) else {
                    return vec![pairing::answer(false)];
                };
                let noted = devices::set_pending(
                    &self.dir,
                    Pending {
                        device_id: self.device_id.clone(),
                        name: self.name.clone(),
                        code: code.clone(),
                        asked_unix: now,
                        from_device: true,
                        accepted: false,
                    },
                    now,
                );
                if let Err(e) = noted {
                    tracing::warn!("A KDE pairing could not be held for the owner: {e}");
                    return vec![pairing::answer(false)];
                }
                self.waiting = Some(Waiting {
                    since: now,
                    from_device: true,
                    code: code.clone(),
                    accepted: false,
                });
                prompt(
                    "A KDE Connect device asks to pair",
                    format!(
                        "{}: check the code {code} on it, then allow it in Settings",
                        self.name
                    ),
                );
                Vec::new()
            }
            // A yes with no time, to nothing of ours: ignored.
            PairSays::Yes(None) => Vec::new(),
            PairSays::No => {
                if self.paired {
                    self.unpin();
                    self.paired = false;
                    self.note_seen();
                    notify("A KDE Connect device unpaired", self.name.clone());
                }
                self.end_waiting(now);
                Vec::new()
            }
        }
    }

    /// The device accepted this computer's request: held for the owner to
    /// confirm the code here, for another [`devices::PAIRING_TTL_SECS`].
    fn accepted(&mut self, now: u64) {
        let Some(waiting) = self.waiting.as_mut().filter(|w| !w.accepted) else {
            return;
        };
        waiting.accepted = true;
        waiting.since = now;
        let code = waiting.code.clone();
        let noted = devices::set_pending(
            &self.dir,
            Pending {
                device_id: self.device_id.clone(),
                name: self.name.clone(),
                code: code.clone(),
                asked_unix: now,
                from_device: false,
                accepted: true,
            },
            now,
        );
        if let Err(e) = noted {
            tracing::warn!("A KDE pairing could not be held for the owner: {e}");
        }
        prompt(
            "A KDE Connect device accepted pairing",
            format!(
                "{}: confirm it in Settings if it showed the code {code}",
                self.name
            ),
        );
    }

    /// The pairing in progress on this link ends: its request leaves the
    /// pending list, and a newer link's for the same device stays.
    fn end_waiting(&mut self, now: u64) {
        if let Some(waiting) = self.waiting.take() {
            let _ = devices::clear_pending(&self.dir, &self.device_id, &waiting.code, now);
        }
    }

    /// Forget the device's pin; a failure is told (its pin would let it
    /// back in as paired).
    fn unpin(&self) {
        if let Err(e) = devices::remove(&self.dir, &self.device_id) {
            tracing::warn!(device_id = %self.device_id, "A KDE Connect device's pairing could not be forgotten: {e}");
            notify(
                "A KDE Connect device is still paired",
                format!("{}: its pairing could not be forgotten ({e})", self.name),
            );
        }
    }

    /// Pair the device with the certificate it brought; whether it is.
    fn pair_now(&mut self) -> bool {
        let now = now_unix();
        let added = devices::add(
            &self.dir,
            KdeDevice {
                device_id: self.device_id.clone(),
                name: self.name.clone(),
                cert_hash: devices::hash_hex(self.cert_hash),
                paired_unix: now,
                permissions: devices::KdePermissions::default(),
            },
        );
        if let Err(e) = added {
            tracing::warn!("A KDE pairing could not be kept: {e}");
            return false;
        }
        self.paired = true;
        self.end_waiting(now);
        // Paired, it is not one of the unpaired links any more.
        self.unpaired = None;
        self.note_seen();
        tracing::info!(device_id = %self.device_id, "Paired with a KDE Connect device");
        true
    }

    fn note_seen(&self) {
        let _ = kdeconnect::record(
            &self.dir,
            SeenDevice {
                device_id: self.device_id.clone(),
                name: self.name.clone(),
                seen_unix: now_unix(),
                paired: self.paired,
            },
        );
    }

    /// What the app asks; what to send.
    fn event(&mut self, event: Event) -> Vec<NetworkPacket> {
        let now = now_unix();
        match event {
            Event::PairFromHere if !self.paired && self.waiting.is_none() => {
                let Some(code) = self.code(now) else {
                    return Vec::new();
                };
                let noted = devices::set_pending(
                    &self.dir,
                    Pending {
                        device_id: self.device_id.clone(),
                        name: self.name.clone(),
                        code: code.clone(),
                        asked_unix: now,
                        from_device: false,
                        accepted: false,
                    },
                    now,
                );
                if noted.is_err() {
                    return Vec::new();
                }
                self.waiting = Some(Waiting {
                    since: now,
                    from_device: false,
                    code,
                    accepted: false,
                });
                vec![pairing::ask(now)]
            }
            Event::PairFromHere => Vec::new(),
            Event::Answer { yes, code }
                if self
                    .waiting
                    .as_ref()
                    .is_some_and(|w| (w.from_device || w.accepted) && w.code == code) =>
            {
                let from_device = self.waiting.as_ref().is_some_and(|w| w.from_device);
                // A pairing that could not be kept is answered no (3b
                // review): the device would think itself paired.
                let yes = yes && self.pair_now();
                if !yes {
                    self.end_waiting(now);
                }
                match (from_device, yes) {
                    // The device accepted ours already: a yes needs no word.
                    (false, true) => Vec::new(),
                    // A no to ours after it accepted: it unpairs there.
                    _ => vec![pairing::answer(yes)],
                }
            }
            Event::Answer { .. } => {
                tracing::info!(device_id = %self.device_id, "A KDE pairing answer for another request; not taken");
                Vec::new()
            }
            Event::Unpair => {
                self.unpin();
                self.paired = false;
                self.note_seen();
                vec![pairing::answer(false)]
            }
            Event::Ping if self.paired => vec![NetworkPacket::new(PING)],
            Event::Clipboard(text) if self.paired && self.permissions().clipboard => {
                let mut packet = NetworkPacket::new(CLIPBOARD);
                packet.body = serde_json::json!({ "content": text });
                vec![packet]
            }
            Event::Ring if self.paired && self.permissions().find => {
                vec![NetworkPacket::new(RING)]
            }
            Event::Ping | Event::Clipboard(_) | Event::Ring => Vec::new(),
        }
    }

    /// A pairing past its time ends with a no, as KDE Connect's own does
    /// (a device that accepted ours unpairs there).
    fn expire(&mut self) -> Vec<NetworkPacket> {
        let Some(waiting) = self.waiting.clone() else {
            return Vec::new();
        };
        let now = now_unix();
        if now.saturating_sub(waiting.since) <= devices::PAIRING_TTL_SECS {
            return Vec::new();
        }
        self.end_waiting(now);
        vec![pairing::answer(false)]
    }
}

async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet: &NetworkPacket,
) -> std::io::Result<()> {
    let mut packet = packet.clone();
    // A KDE device reads the body as an object: a null one (a ping's) made
    // the A71 drop the link.
    if packet.body.is_null() {
        packet.body = serde_json::json!({});
    }
    packet.id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let wire = packet.to_wire().map_err(std::io::Error::other)?;
    writer.write_all(&wire).await?;
    writer.flush().await
}

/// A device's packets, one a line.
struct PacketReader<R> {
    reader: BufReader<R>,
    /// The longest line taken.
    limit: usize,
    /// The line read so far: kept across a wait cut short, so a tick or an
    /// event in the middle of a line loses none of it (3b review).
    line: Vec<u8>,
}

impl<R: AsyncRead + Unpin> PacketReader<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            limit: MAX_LINE,
            line: Vec::new(),
        }
    }

    /// The next packet, `None` at the end or on a line longer than the
    /// limit; a line that is not a packet is skipped. Safe to cut
    /// short in `select!`: `read_until` keeps what it read in `line`.
    async fn next(&mut self) -> Option<NetworkPacket> {
        use tokio::io::AsyncReadExt;
        loop {
            let room = (self.limit + 1).saturating_sub(self.line.len()) as u64;
            let read = (&mut self.reader)
                .take(room)
                .read_until(b'\n', &mut self.line)
                .await
                .ok()?;
            if read == 0 {
                return None;
            }
            if !self.line.ends_with(b"\n") {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let packet = std::str::from_utf8(&line)
                .ok()
                .and_then(|text| NetworkPacket::from_wire(text).ok());
            if packet.is_some() {
                return packet;
            }
        }
    }
}

/// Whether a device is paired with this computer, for the handshake's pin.
pub fn pin_of(dir: &Path, device_id: &str) -> Option<[u8; 32]> {
    devices::pinned(dir, device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str, paired: bool) -> State {
        let dir = std::env::temp_dir().join(format!(
            "pd-kde-link-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        State {
            dir,
            device_id: "d".repeat(32),
            name: "Galaxy A71".to_string(),
            cert_hash: [9; 32],
            keys: (Some(vec![1; 91]), Some(vec![2; 91])),
            paired,
            waiting: None,
            last_asked: 0,
            unpaired: None,
            clipboard_in: None,
            file_in: None,
            told_unpaired: false,
        }
    }

    fn says(packet: &NetworkPacket) -> Option<PairSays> {
        pairing::read(packet)
    }

    #[test]
    fn a_devices_request_waits_for_the_owner_and_allow_pairs_it() {
        let mut s = state("allow", false);
        let now = now_unix();
        assert!(
            s.pair_packet(PairSays::Yes(Some(now))).is_empty(),
            "held, not answered"
        );
        let held = devices::pending(&s.dir, now);
        assert_eq!(held.len(), 1);
        assert!(held[0].from_device);
        assert_eq!(held[0].code.len(), 8);
        let code = held[0].code.clone();
        let out = s.event(Event::Answer {
            yes: true,
            code: "00000000".to_string(),
        });
        assert!(out.is_empty(), "an answer naming another code is not taken");
        assert!(devices::pinned(&s.dir, &s.device_id).is_none());
        let out = s.event(Event::Answer { yes: true, code });
        assert_eq!(says(&out[0]), Some(PairSays::Yes(None)));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), Some([9; 32]));
        assert!(devices::pending(&s.dir, now).is_empty());
    }

    #[test]
    fn a_denied_or_stale_request_pairs_nothing() {
        let mut s = state("deny", false);
        let now = now_unix();
        s.pair_packet(PairSays::Yes(Some(now)));
        let code = devices::pending(&s.dir, now)[0].code.clone();
        let out = s.event(Event::Answer { yes: false, code });
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);

        let mut s = state("skew", false);
        let out = s.pair_packet(PairSays::Yes(Some(now - pairing::MAX_SKEW_SECS - 5)));
        assert_eq!(says(&out[0]), Some(PairSays::No), "its clock is off");
        assert!(devices::pending(&s.dir, now).is_empty());

        let mut s = state("expire", false);
        s.pair_packet(PairSays::Yes(Some(now)));
        s.waiting = Some(Waiting {
            since: now - devices::PAIRING_TTL_SECS - 1,
            from_device: true,
            code: String::new(),
            accepted: false,
        });
        let out = s.expire();
        assert_eq!(says(&out[0]), Some(PairSays::No), "no answer is a no");
    }

    #[test]
    fn a_device_asks_once_at_a_time_and_not_again_at_once() {
        let mut s = state("flood", false);
        let now = now_unix();
        s.pair_packet(PairSays::Yes(Some(now)));
        let first = devices::pending(&s.dir, now)[0].clone();
        assert!(s.pair_packet(PairSays::Yes(Some(now + 1))).is_empty());
        assert_eq!(
            devices::pending(&s.dir, now + 1)[0],
            first,
            "a repeat while one waits changes nothing"
        );
        s.event(Event::Answer {
            yes: false,
            code: first.code,
        });
        assert!(s.pair_packet(PairSays::Yes(Some(now))).is_empty());
        assert!(
            devices::pending(&s.dir, now).is_empty(),
            "not again at once"
        );
        s.last_asked = now - REQUEST_GAP_SECS;
        s.pair_packet(PairSays::Yes(Some(now)));
        assert_eq!(devices::pending(&s.dir, now).len(), 1, "a while later");
    }

    #[test]
    fn notifications_are_limited_to_a_few_a_minute() {
        let mut toasts = Allowance::new(2);
        let start = std::time::Instant::now();
        assert!(toasts.allow(start));
        assert!(toasts.allow(start));
        assert!(!toasts.allow(start + Duration::from_secs(59)));
        assert!(toasts.allow(start + Duration::from_secs(60)));
    }

    #[test]
    fn pairing_from_here_pairs_when_the_owner_confirms_the_devices_yes() {
        let mut s = state("here", false);
        let now = now_unix();
        let out = s.event(Event::PairFromHere);
        assert!(matches!(says(&out[0]), Some(PairSays::Yes(Some(_)))));
        let code = devices::pending(&s.dir, now)[0].code.clone();
        assert!(
            s.event(Event::Answer {
                yes: true,
                code: code.clone()
            })
            .is_empty(),
            "nothing to confirm before the device accepts"
        );
        assert!(!s.paired);
        assert!(s.pair_packet(PairSays::Yes(None)).is_empty());
        assert!(!s.paired, "its yes alone pairs nothing");
        let held = devices::pending(&s.dir, now);
        assert!(held[0].accepted && !held[0].from_device);
        assert!(
            s.event(Event::Answer {
                yes: true,
                code: code.clone()
            })
            .is_empty()
        );
        assert!(s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), Some([9; 32]));
        assert!(devices::pending(&s.dir, now).is_empty());
    }

    #[test]
    fn a_devices_yes_the_owner_does_not_confirm_unpairs_it_there() {
        let mut s = state("here-no", false);
        let now = now_unix();
        s.event(Event::PairFromHere);
        let code = devices::pending(&s.dir, now)[0].code.clone();
        s.pair_packet(PairSays::Yes(None));
        let out = s.event(Event::Answer { yes: false, code });
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert!(!s.paired);

        let mut s = state("here-late", false);
        s.event(Event::PairFromHere);
        s.pair_packet(PairSays::Yes(None));
        s.waiting.as_mut().unwrap().since = now - devices::PAIRING_TTL_SECS - 1;
        let out = s.expire();
        assert_eq!(says(&out[0]), Some(PairSays::No), "not confirmed in time");
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
    }

    #[test]
    fn an_answer_without_a_request_and_a_yes_to_nothing_do_nothing() {
        let mut s = state("nothing", false);
        assert!(
            s.event(Event::Answer {
                yes: true,
                code: String::new()
            })
            .is_empty()
        );
        assert!(s.pair_packet(PairSays::Yes(None)).is_empty());
        assert!(!s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
    }

    #[test]
    fn a_link_that_pairs_gives_its_unpaired_place_back() {
        static PLACES: super::super::Slots = super::super::Slots::new(1, 1);
        let address: std::net::IpAddr = "192.0.2.7".parse().unwrap();
        let mut s = state("place", false);
        s.unpaired = PLACES.take(address);
        assert!(s.unpaired.is_some());
        assert!(PLACES.take(address).is_none(), "held");
        s.pair_now();
        assert!(PLACES.take(address).is_some(), "given back");
    }

    #[test]
    fn an_older_links_end_leaves_the_newer_ones_request() {
        let mut old = state("ends", false);
        let mut new = state("ends", false);
        new.dir = old.dir.clone();
        let now = now_unix();
        old.pair_packet(PairSays::Yes(Some(now)));
        new.pair_packet(PairSays::Yes(Some(now + 1)));
        let newer = devices::pending(&new.dir, now + 1);
        assert_eq!(newer.len(), 1);
        old.end_waiting(now + 1);
        assert_eq!(devices::pending(&new.dir, now + 1), newer, "left");
        new.end_waiting(now + 1);
        assert!(devices::pending(&new.dir, now + 1).is_empty());
    }

    #[test]
    fn a_pairing_that_cannot_be_kept_is_answered_no() {
        let mut s = state("unkept", false);
        let now = now_unix();
        s.pair_packet(PairSays::Yes(Some(now)));
        let code = devices::pending(&s.dir, now)[0].code.clone();
        // The pins do not read: nothing can be added to them.
        std::fs::write(
            s.dir
                .join(pocketdesk_core::local::paths::KDECONNECT_DEVICES_FILE),
            "not json",
        )
        .unwrap();
        let out = s.event(Event::Answer { yes: true, code });
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert!(!s.paired);
    }

    #[test]
    fn unpairing_either_way_forgets_the_pin() {
        let mut s = state("unpair", false);
        s.pair_now();
        assert!(s.paired);
        assert!(s.pair_packet(PairSays::No).is_empty());
        assert!(!s.paired);
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
        s.pair_now();
        let out = s.event(Event::Unpair);
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert_eq!(devices::pinned(&s.dir, &s.device_id), None);
    }

    #[tokio::test]
    async fn a_packet_cut_by_another_wait_is_read_whole() {
        let (mut device, ours) = tokio::io::duplex(64 * 1024);
        let mut reader = PacketReader::new(ours);
        let wire = NetworkPacket::new(PING).to_wire().unwrap();
        let (first, rest) = wire.split_at(wire.len() / 2);
        device.write_all(first).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), reader.next())
                .await
                .is_err(),
            "half a line, cut short"
        );
        device.write_all(rest).await.unwrap();
        device.write_all(b"not a packet\n").await.unwrap();
        device.write_all(&wire).await.unwrap();
        assert_eq!(reader.next().await.unwrap().packet_type, PING);
        assert_eq!(
            reader.next().await.unwrap().packet_type,
            PING,
            "a line that is not a packet is skipped"
        );
        device.write_all(&vec![b'x'; MAX_LINE + 2]).await.unwrap();
        assert!(reader.next().await.is_none(), "a line too long ends it");
    }

    fn link_of(
        id: &str,
        cert_hash: [u8; 32],
    ) -> (tokio::io::DuplexStream, DeviceLink<tokio::io::DuplexStream>) {
        let (device, ours) = tokio::io::duplex(64 * 1024);
        let link = DeviceLink {
            stream: ours,
            hello: super::super::handshake::Hello {
                device_id: id.to_string(),
                device_name: "Galaxy A71".to_string(),
                protocol: 8,
            },
            cert_der: Vec::new(),
            cert_hash,
        };
        (device, link)
    }

    #[tokio::test(start_paused = true)]
    async fn an_unpaired_link_that_says_nothing_is_closed() {
        static PLACES: super::super::Slots = super::super::Slots::new(4, 4);
        let dir = state("idle", false).dir;
        let id = format!("idle{}", uuid::Uuid::new_v4().simple());
        let (_device, link) = link_of(&id, [3; 32]);
        let place = PLACES.take("192.0.2.9".parse().unwrap());
        let ran = tokio::time::timeout(
            UNPAIRED_IDLE + Duration::from_secs(5),
            run_in(dir, link, &[], place, None),
        )
        .await;
        assert!(ran.is_ok(), "closed after {UNPAIRED_IDLE:?} quiet");
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_that_stops_reading_is_closed() {
        let mut paired_state = state("stops", false);
        let id = format!("stops{}", uuid::Uuid::new_v4().simple());
        paired_state.device_id = id.clone();
        paired_state.pair_now();
        let dir = paired_state.dir.clone();
        // The device reads nothing: a big clipboard fills the pipe.
        let (_device, link) = link_of(&id, [9; 32]);
        let running = tokio::spawn(run_in(dir, link, &[], None, None));
        tokio::task::yield_now().await;
        assert!(tell(&id, Event::Clipboard("x".repeat(512 * 1024))));
        tokio::time::timeout(WRITE_WAIT * 2, running)
            .await
            .expect("closed once the write waited too long")
            .unwrap();
    }

    #[tokio::test]
    async fn a_newer_link_from_the_device_closes_the_older() {
        let dir = state("replace", true).dir;
        let id = format!("replace{}", uuid::Uuid::new_v4().simple());
        let (_old_device, old) = link_of(&id, [4; 32]);
        let older = tokio::spawn(run_in(dir.clone(), old, &[], None, None));
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_new_device, new) = link_of(&id, [4; 32]);
        let newer = tokio::spawn(run_in(dir, new, &[], None, None));
        tokio::time::timeout(Duration::from_secs(5), older)
            .await
            .expect("the older link closes")
            .unwrap();
        assert!(!newer.is_finished());
        assert!(tell(&id, Event::Ping), "the newer one is live");
        newer.abort();
    }

    #[tokio::test]
    async fn every_packet_goes_with_an_object_for_a_body() {
        let mut wire = Vec::new();
        send(&mut wire, &NetworkPacket::new(PING)).await.unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        assert!(sent["body"].is_object(), "{sent}");
        assert!(sent["id"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn a_newer_link_takes_the_older_ones_place_and_its_going_leaves_the_new_one() {
        let id = "r".repeat(32);
        let (old_tx, mut old_rx) = mpsc::channel(1);
        let old = Live::new(&id, [1; 32], old_tx).unwrap();
        let (other_tx, _other_rx) = mpsc::channel(4);
        assert!(
            Live::new(&id, [2; 32], other_tx).is_none(),
            "another certificate does not take a live link's place"
        );
        assert!(
            !old.replaced().is_cancelled(),
            "and the live one is left alone"
        );
        // The old link's queue is full: its closing must not need room.
        assert!(tell(&id, Event::Ping));
        let (new_tx, mut new_rx) = mpsc::channel(4);
        let new = Live::new(&id, [1; 32], new_tx).unwrap();
        assert!(old.replaced().is_cancelled(), "the old link is closed");
        assert_eq!(old_rx.try_recv().unwrap(), Event::Ping);
        drop(old);
        assert!(tell(&id, Event::Ping), "the new link is still there");
        assert_eq!(new_rx.try_recv().unwrap(), Event::Ping);
        drop(new);
        assert!(!tell(&id, Event::Ping));
        let (other_tx, _other_rx) = mpsc::channel(4);
        assert!(
            Live::new(&id, [2; 32], other_tx).is_some(),
            "once none is live, it may connect"
        );
    }

    fn battery(charge: u8) -> NetworkPacket {
        let mut packet = NetworkPacket::new(BATTERY);
        packet.body =
            serde_json::json!({ "currentCharge": charge, "isCharging": true, "thresholdEvent": 0 });
        packet
    }

    #[test]
    fn a_paired_devices_battery_is_kept_while_it_may_share_it() {
        let mut s = state("battery", false);
        s.device_id = format!("battery{}", uuid::Uuid::new_v4().simple());
        s.packet(&battery(50));
        assert_eq!(
            crate::plugins::phone_battery::charge_of(&s.device_id),
            None,
            "not paired"
        );
        s.pair_now();
        s.packet(&battery(60));
        assert_eq!(
            crate::plugins::phone_battery::charge_of(&s.device_id),
            Some(60)
        );
        let off = devices::KdePermissions {
            battery: false,
            ..devices::KdePermissions::default()
        };
        devices::set_permissions(&s.dir, &s.device_id, off).unwrap();
        s.packet(&battery(70));
        assert_eq!(
            crate::plugins::phone_battery::charge_of(&s.device_id),
            Some(60),
            "the owner's change holds at once"
        );
    }

    fn clipboard(text: &str) -> NetworkPacket {
        let mut packet = NetworkPacket::new(CLIPBOARD);
        packet.body = serde_json::json!({ "content": text });
        packet
    }

    #[test]
    fn the_clipboard_goes_both_ways_with_a_paired_device_allowed_it() {
        let mut s = state("clipboard", false);
        s.packet(&clipboard("from the phone"));
        assert_eq!(s.clipboard_in, None, "not paired");
        assert!(s.event(Event::Clipboard("hi".to_string())).is_empty());
        s.pair_now();
        s.packet(&clipboard("from the phone"));
        assert_eq!(s.clipboard_in.take().as_deref(), Some("from the phone"));
        let mut connect = clipboard("as it connects");
        connect.packet_type = "kdeconnect.clipboard.connect".to_string();
        s.packet(&connect);
        assert_eq!(s.clipboard_in, None, "not on connecting");
        let out = s.event(Event::Clipboard("from here".to_string()));
        assert_eq!(out[0].packet_type, CLIPBOARD);
        assert_eq!(out[0].body["content"], "from here");
        let off = devices::KdePermissions {
            clipboard: false,
            ..devices::KdePermissions::default()
        };
        devices::set_permissions(&s.dir, &s.device_id, off).unwrap();
        s.packet(&clipboard("again"));
        assert_eq!(s.clipboard_in, None);
        assert!(s.event(Event::Clipboard("again".to_string())).is_empty());
    }

    #[tokio::test]
    async fn a_paired_links_line_may_be_long_and_an_unpaired_ones_not() {
        let long = clipboard(&"x".repeat(MAX_LINE * 2));
        let wire = long.to_wire().unwrap();
        let (mut device, ours) = tokio::io::duplex(64 * 1024);
        let mut reader = PacketReader::new(ours);
        let writing = tokio::spawn(async move {
            device.write_all(&wire).await.unwrap();
            device.write_all(&wire).await.unwrap();
            device
        });
        reader.limit = PAIRED_LINE;
        assert_eq!(reader.next().await.unwrap().packet_type, CLIPBOARD);
        reader.limit = MAX_LINE;
        assert!(reader.next().await.is_none());
        drop(writing);
    }

    #[test]
    fn a_paired_device_allowed_it_is_rung() {
        let mut s = state("ring", false);
        assert!(s.event(Event::Ring).is_empty(), "not paired");
        s.pair_now();
        assert_eq!(s.event(Event::Ring)[0].packet_type, RING);
        let off = devices::KdePermissions {
            find: false,
            ..devices::KdePermissions::default()
        };
        devices::set_permissions(&s.dir, &s.device_id, off).unwrap();
        assert!(s.event(Event::Ring).is_empty());
    }

    #[test]
    fn a_file_is_fetched_only_from_a_paired_device_allowed_files() {
        let mut offer = NetworkPacket::new(super::super::share::SHARE)
            .with_body(serde_json::json!({ "filename": "photo.jpg" }));
        offer.payload_size = Some(5);
        offer.payload_transfer_info = Some(serde_json::json!({ "port": 1739 }));
        let mut s = state("files", false);
        s.packet(&offer);
        assert_eq!(s.file_in, None, "not paired");
        s.pair_now();
        s.packet(&offer);
        assert_eq!(s.file_in, None, "Files is off by default");
        let files = devices::KdePermissions {
            files: true,
            ..devices::KdePermissions::default()
        };
        devices::set_permissions(&s.dir, &s.device_id, files).unwrap();
        s.packet(&offer);
        assert_eq!(s.file_in.take().unwrap().name, "photo.jpg");
    }

    /// Every packet the Pocketdesk road knows (`docs/packets.md`) but the
    /// KDE link does not take is not taken from a paired device allowed
    /// everything: nothing answered, nothing set, nothing fetched.
    #[test]
    fn a_paired_device_is_given_nothing_beyond_its_plugins() {
        let packets = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/packets.md"),
        )
        .unwrap();
        let mut types: Vec<&str> = packets
            .split('`')
            .filter(|t| t.starts_with("kdeconnect.") && !t.contains(' '))
            .collect();
        types.sort_unstable();
        types.dedup();
        assert!(types.len() > 40, "{} packet types read", types.len());
        let taken = [
            PING,
            BATTERY,
            CLIPBOARD,
            super::super::share::SHARE,
            super::super::pairing::PAIR,
        ];
        let mut s = state("sweep", false);
        s.device_id = format!("sweep{}", uuid::Uuid::new_v4().simple());
        s.pair_now();
        let all = devices::KdePermissions {
            clipboard: true,
            battery: true,
            find: true,
            files: true,
        };
        devices::set_permissions(&s.dir, &s.device_id, all).unwrap();
        for packet_type in types.into_iter().filter(|t| !taken.contains(t)) {
            let mut packet = NetworkPacket::new(packet_type).with_body(serde_json::json!({
                "content": "x",
                "filename": "y",
                "currentCharge": 5,
                "url": "https://example.org",
                "command": "calc",
            }));
            packet.payload_size = Some(1);
            packet.payload_transfer_info = Some(serde_json::json!({ "port": 1739 }));
            assert!(s.packet(&packet).is_empty(), "{packet_type} answered");
            assert_eq!(s.clipboard_in, None, "{packet_type} set the clipboard");
            assert_eq!(s.file_in, None, "{packet_type} fetched a file");
            assert!(s.paired, "{packet_type} unpaired it");
        }
        assert_eq!(
            crate::plugins::phone_battery::charge_of(&s.device_id),
            None,
            "no battery taken from another packet"
        );
    }

    #[test]
    fn only_paired_links_count_for_the_clipboard() {
        let paired = KdeDevice {
            device_id: "p".to_string(),
            name: "Phone".to_string(),
            cert_hash: String::new(),
            paired_unix: 1,
            permissions: devices::KdePermissions::default(),
        };
        assert_eq!(
            paired_among(vec!["p".to_string(), "stranger".to_string()], &[paired]),
            ["p"]
        );
    }

    #[test]
    fn a_device_that_thinks_itself_paired_is_told_it_is_not_once() {
        let mut s = state("forgot", false);
        let out = s.packet(&battery(50));
        assert_eq!(says(&out[0]), Some(PairSays::No));
        assert!(s.packet(&battery(50)).is_empty(), "once");

        let mut s = state("accepted", false);
        s.event(Event::PairFromHere);
        s.pair_packet(PairSays::Yes(None));
        assert!(
            s.packet(&battery(50)).is_empty(),
            "not while the owner confirms the device's yes"
        );
    }

    #[test]
    fn only_a_paired_device_is_pinged() {
        let mut s = state("ping", false);
        assert!(s.event(Event::Ping).is_empty());
        s.pair_now();
        assert_eq!(s.event(Event::Ping)[0].packet_type, PING);
    }
}
