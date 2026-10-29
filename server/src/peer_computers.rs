//! The owner's other computers (2f): pairing with one from its pairing
//! link, and the link this computer keeps to each over the internet road.
//! Phase 2f–2h in `docs/status-history.md`.
//!
//! Pairing runs over TLS on the other computer's control port, its
//! certificate pinned from the link before the first byte; this computer
//! then names its iroh key on that connection (`wan_key`). The link always
//! rides iroh, dialed from this server's own WAN endpoint (a second
//! endpoint with the same key could not use a relay, 2b): direct at home,
//! direct or relayed from anywhere. On it this computer is the other's
//! client, as a phone is: it shares files as a phone shares them and
//! fetches what the other offers it (2h), on streams the other opens on the
//! link.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::computers::{self, ComputerAsk, PairingOutcome, PeerComputer};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::paths::COMPUTERS_DIR;
use pocketdesk_core::protocol::connection::ConnectionManager;
use pocketdesk_core::protocol::kdeconnect::{DeviceIdentity, NetworkPacket, TransferIo};
use pocketdesk_core::streaming::transport::CertManager;
use pocketdesk_core::streaming::transport::peer_label_for_address;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// How long a pairing waits for the other computer's answer.
const PAIR_ANSWER: Duration = Duration::from_secs(15);

/// A dial or handshake on the link.
const LINK_DIAL: Duration = Duration::from_secs(20);

/// The link's backoff after a failure, from the first to the most.
const BACKOFF_FIRST: Duration = Duration::from_secs(2);
const BACKOFF_MOST: Duration = Duration::from_secs(60);

/// A link the other computer refused (this one is no longer paired there)
/// is tried again this rarely, in case it was a moment's race.
const REFUSED_RETRY: Duration = Duration::from_secs(300);

/// Without a watcher, how often the requests folder is looked at.
const POLL: Duration = Duration::from_secs(1);

/// The longest packet line taken from the other computer.
const MAX_LINE: u64 = 4 * 1024 * 1024;

/// A stream's key line, 32 hex digits and a newline, must come this soon.
const KEY_WAIT: Duration = Duration::from_secs(5);

/// How long a transfer waits for the other computer to open its stream.
const STREAM_WAIT: Duration = Duration::from_secs(30);

/// Seconds before each retry of a file that broke off: the other computer
/// keeps what arrived, so a retry continues it (as a phone's, T6a).
const RETRIES: [u64; 3] = [5, 15, 45];

/// The server's side: take the app's requests and keep a link to each
/// paired computer, for the server's life.
pub async fn run(cert_manager: Arc<CertManager>, wan: Option<iroh::Endpoint>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut links: HashMap<String, Link> = HashMap::new();
    // Another computer's screen in a window here (BK7), from the first ask.
    let mut views: Option<mpsc::Sender<crate::computer_view::Command>> = None;
    // What an earlier run said of its links is not this run's.
    let restarted = computers::update(&dir, |file| {
        for computer in &mut file.computers {
            computer.link = first_link(wan.is_some()).to_string();
        }
    });
    if let Err(e) = restarted {
        tracing::debug!("Could not reset the links' state: {e}");
    }
    loop {
        for request in computers::take_all(&dir, pocketdesk_core::local::pin::now_unix()) {
            match request.ask {
                ComputerAsk::Pair { link } => {
                    // Paired again: the link starts over now, not after the
                    // back-off a refused or broken one was sleeping out.
                    if let Some(paired) =
                        pair(&dir, &cert_manager, wan.as_ref(), &link, &request.id).await
                    {
                        if let Some(stale) = links.remove(&paired) {
                            stale.task.abort();
                        }
                        // No link is kept without the road (BD5).
                        if wan.is_none() {
                            set_link(&dir, &paired, first_link(false));
                        }
                    }
                }
                ComputerAsk::Forget { device_id } => {
                    if let Some(link) = links.remove(&device_id) {
                        link.task.abort();
                    }
                    forget(&dir, &cert_manager, &device_id);
                }
                ComputerAsk::LetDrive { device_id, allow } => {
                    let_drive(&dir, &device_id, allow);
                }
                ComputerAsk::View { device_id, monitor } => {
                    let views = views.get_or_insert_with(|| {
                        crate::computer_view::spawn(dir.clone(), cert_manager.clone(), wan.clone())
                    });
                    let start = crate::computer_view::Command::Start { device_id, monitor };
                    if views.send(start).await.is_err() {
                        tracing::warn!(
                            "Another computer's screen cannot be shown: the view relay stopped"
                        );
                    }
                }
                ComputerAsk::StopView { session_id } => {
                    if let Some(views) = &views {
                        let stop = crate::computer_view::Command::Stop { session_id };
                        let _ = views.send(stop).await;
                    }
                }
            }
        }
        if let Some(wan) = &wan {
            keep_links(&dir, wan, &mut links);
        }
        crate::inbox::wait(COMPUTERS_DIR, POLL).await;
    }
}

/// What a computer's link says before it is tried: "connecting", or, with
/// the internet road off here (iroh did not start), why none is kept (BD5:
/// it said "connecting" for good).
fn first_link(road: bool) -> &'static str {
    if road {
        "connecting"
    } else {
        "away: the internet road is off on this computer"
    }
}

/// One kept link's task.
struct Link {
    task: tokio::task::JoinHandle<()>,
}

/// The links that are up, by the other computer's id: the way to send on
/// each.
static LIVE: LazyLock<Mutex<HashMap<String, mpsc::Sender<NetworkPacket>>>> =
    LazyLock::new(Default::default);

/// A computer this one keeps a link to, while the link is up.
pub(crate) struct LinkedComputer {
    pub device_id: String,
    outgoing: mpsc::Sender<NetworkPacket>,
}

impl LinkedComputer {
    /// Send `packet` on the link, or drop it when the link is busy or gone.
    pub(crate) fn send(&self, packet: NetworkPacket) {
        if self.outgoing.try_send(packet).is_err() {
            tracing::debug!(device_id = %self.device_id, "Not sent: the link is busy or gone");
        }
    }
}

/// The way to send on the link to `device_id`, while it is up.
fn live(device_id: &str) -> Option<mpsc::Sender<NetworkPacket>> {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .cloned()
}

/// The link to the computer `device_id`, while it is up, whether or not
/// the clipboard is shared with computers (shared input's clipboard
/// follows the mouse with that off).
pub(crate) fn live_link(device_id: &str) -> Option<LinkedComputer> {
    live(device_id).map(|outgoing| LinkedComputer {
        device_id: device_id.to_string(),
        outgoing,
    })
}

/// Whether the link to `device_id` is up now.
pub(crate) fn is_linked(device_id: &str) -> bool {
    live(device_id).is_some()
}

/// The monitors packet, asked of a computer on its link and answered there.
pub(crate) const MONITORS: &str = "kdeconnect.pocketdesk.monitors";

/// Each linked computer's screens, by name and size, as it last answered
/// (BK7: the window that shows its screen offers the choice).
static SCREENS: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);

/// Ask the computer `device_id` for its screens on its link, when it is up;
/// its answer is kept for [`screens`].
pub(crate) fn ask_screens(device_id: &str) {
    if let Some(link) = live_link(device_id) {
        link.send(NetworkPacket::new(MONITORS));
    }
}

/// The computer `device_id`'s screens as it last said, in its own order;
/// empty when not known.
pub(crate) fn screens(device_id: &str) -> Vec<String> {
    SCREENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .cloned()
        .unwrap_or_default()
}

/// The screens a monitors answer names, by their index: "name (WxH)", at
/// most 16, each name cut as a computer's is.
fn screens_in(body: &serde_json::Value) -> Vec<String> {
    let Some(monitors) = body.get("monitors").and_then(|m| m.as_array()) else {
        return Vec::new();
    };
    let mut listed: Vec<(u64, String)> = monitors
        .iter()
        .take(16)
        .enumerate()
        .map(|(at, m)| {
            let index = m.get("index").and_then(|v| v.as_u64()).unwrap_or(at as u64);
            let name = match m.get("name").and_then(|v| v.as_str()) {
                Some(name) if !name.trim().is_empty() => shown_name(name),
                _ => format!("Screen {}", index + 1),
            };
            let size = match (
                m.get("width").and_then(|v| v.as_u64()),
                m.get("height").and_then(|v| v.as_u64()),
            ) {
                (Some(w), Some(h)) => format!(" ({w}x{h})"),
                _ => String::new(),
            };
            (index, format!("{name}{size}"))
        })
        .collect();
    listed.sort_by_key(|(index, _)| *index);
    listed.into_iter().map(|(_, name)| name).collect()
}

/// Whether `device_id` is a computer this one paired with.
pub(crate) fn is_paired(device_id: &str) -> bool {
    crate::state::state_dir().is_ok_and(|dir| {
        computers::read(&dir)
            .computers
            .iter()
            .any(|c| c.device_id == device_id)
    })
}

/// The computers this one keeps a link to that are up now, for the
/// clipboard (2g) while it is shared with them.
pub(crate) fn linked() -> Vec<LinkedComputer> {
    if !crate::clipboard_push::with_computers() {
        return Vec::new();
    }
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(id, outgoing)| LinkedComputer {
            device_id: id.clone(),
            outgoing: outgoing.clone(),
        })
        .collect()
}

/// A link counted live while it is held; gone however the link ends.
struct LiveLink(String);

impl LiveLink {
    fn new(device_id: &str, outgoing: mpsc::Sender<NetworkPacket>) -> Self {
        LIVE.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(device_id.to_string(), outgoing);
        Self(device_id.to_string())
    }
}

impl Drop for LiveLink {
    fn drop(&mut self) {
        LIVE.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// A link for every paired computer, and none for one forgotten. A file
/// that does not read leaves the links as they are.
fn keep_links(dir: &Path, wan: &iroh::Endpoint, links: &mut HashMap<String, Link>) {
    let Ok(file) = computers::try_read(dir) else {
        return;
    };
    let paired: Vec<String> = file.computers.into_iter().map(|c| c.device_id).collect();
    links.retain(|id, link| {
        let keep = paired.contains(id) && !link.task.is_finished();
        if !keep {
            link.task.abort();
        }
        keep
    });
    for id in paired {
        if let std::collections::hash_map::Entry::Vacant(slot) = links.entry(id) {
            let id = slot.key().clone();
            let task = tokio::spawn(keep_link(dir.to_path_buf(), wan.clone(), id));
            slot.insert(Link { task });
        }
    }
}

/// Pair with the computer whose pairing link `uri` is, and say how it went;
/// its id once paired.
async fn pair(
    dir: &Path,
    certs: &CertManager,
    wan: Option<&iroh::Endpoint>,
    uri: &str,
    request: &str,
) -> Option<String> {
    let (name, device_id) = PairLink::from_uri(uri)
        .map(|link| (link.name, link.device_id))
        .unwrap_or_default();
    let (outcome, paired) = match pair_with(dir, certs, wan, uri).await {
        Ok(computer) => {
            tracing::info!(device_id = %computer.device_id, "Paired with another computer");
            ("paired".to_string(), Some(computer.device_id))
        }
        Err(why) => {
            tracing::warn!("Pairing with another computer failed: {why}");
            (format!("failed: {why}"), None)
        }
    };
    let saved = computers::update(dir, |file| {
        file.last_pairing = Some(PairingOutcome {
            name,
            outcome,
            unix: pocketdesk_core::local::pin::now_unix(),
            request: request.to_string(),
            device_id,
        })
    });
    if let Err(e) = saved {
        tracing::warn!("Could not record the pairing's outcome: {e}");
    }
    paired
}

/// Pair as [`pair`] says, and record the computer.
async fn pair_with(
    dir: &Path,
    certs: &CertManager,
    wan: Option<&iroh::Endpoint>,
    uri: &str,
) -> Result<PeerComputer, String> {
    let link = PairLink::from_uri(uri).map_err(|e| format!("not a pairing link: {e}"))?;
    let fingerprint = link
        .fingerprint
        .as_deref()
        .ok_or("the link has no certificate fingerprint")?;
    let hash = parse_fingerprint(fingerprint).ok_or("the link's fingerprint does not read")?;
    let me = crate::kde::host_identity();
    if link.device_id == me.device_id {
        return Err("that is this computer's own pairing link".to_string());
    }
    if !pocketdesk_core::protocol::kdeconnect::valid_device_id(&link.device_id) {
        return Err("the link's computer id is not a valid one".to_string());
    }
    // One id, one device: a computer by a paired phone's id would take that
    // phone's sends.
    if crate::plugins::pair::is_paired_device(&link.device_id) {
        return Err("the link's id is one of this computer's paired devices".to_string());
    }
    let name = shown_name(&link.name);
    let labels: Vec<String> = link
        .addresses
        .iter()
        .filter_map(|a| peer_label_for_address(a))
        .collect();
    if labels.is_empty() {
        return Err("the link names no address".to_string());
    }
    // Pinned before the first byte, so the PIN goes only to that
    // certificate; what was pinned before is put back if pairing fails.
    let before: HashMap<String, [u8; 32]> = certs.known_peers().into_iter().collect();
    for label in &labels {
        certs
            .pin_peer(label, hash)
            .map_err(|e| format!("could not pin its certificate: {e:#}"))?;
    }
    let paired = dial_and_pair(certs, &link, &me, wan)
        .await
        .and_then(|pushed| {
            // Paired here meanwhile, the other way (BD1 review: both at
            // once), it is one of this computer's devices, not one it
            // paired with.
            if crate::plugins::pair::is_paired_device(&link.device_id) {
                Err("that computer paired with this one meanwhile".to_string())
            } else {
                Ok(pushed)
            }
        });
    if paired.is_err() {
        for label in &labels {
            let _ = match before.get(label) {
                Some(old) => certs.pin_peer(label, *old),
                None => certs.remove_peer(label).map(|_| ()),
            };
        }
    }
    // Its endpoint as it said once paired, else as the link said.
    let pushed = paired?;
    let now = pocketdesk_core::local::pin::now_unix();
    let computer = PeerComputer {
        device_id: link.device_id.clone(),
        name,
        addresses: link.addresses.clone(),
        control_port: link.control_port,
        stream_port: link.stream_port,
        fingerprint: fingerprint.to_ascii_lowercase(),
        endpoint: pushed.or_else(|| {
            link.endpoint
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .filter(|body| WanTarget::parse(body).is_some())
        }),
        paired_unix: now,
        link: "connecting".to_string(),
        link_unix: now,
        lets_drive: false,
        mine: false,
    };
    let mut was = None;
    computers::update(dir, |file| {
        if let Some(at) = file
            .computers
            .iter()
            .position(|c| c.device_id == computer.device_id)
        {
            was = Some(file.computers.remove(at));
        }
        file.computers.push(computer.clone());
    })
    .map_err(|e| format!("paired, but could not record it: {e}"))?;
    // Paired again at new addresses, the old ones' pins go (BD6: they
    // stayed for good, and read as a changed certificate to a dial there).
    if let Some(was) = was {
        drop_pins(certs, &was, &labels);
    }
    Ok(computer)
}

/// Dial the link's addresses in order over TLS and pair on the first that
/// answers.
async fn dial_and_pair(
    certs: &CertManager,
    link: &PairLink,
    me: &DeviceIdentity,
    wan: Option<&iroh::Endpoint>,
) -> Result<Option<serde_json::Value>, String> {
    let manager = ConnectionManager::new(Duration::from_secs(8), certs.control_client_config());
    let mut last = String::new();
    for address in &link.addresses {
        match manager.connect(address, link.control_port, me).await {
            Ok(stream) => {
                return pair_on(stream, &link.pin, wan.map(|e| e.id().to_string())).await;
            }
            // Someone answers there, but not the computer in the link: no
            // other address is tried with its PIN either.
            Err(e) if is_pin_mismatch(&e.to_string()) => {
                return Err(format!(
                    "the computer at {address} is not the one in the link (another certificate)"
                ));
            }
            Err(e) => last = format!("{address}: {e}"),
        }
    }
    Err(format!("it did not answer ({last})"))
}

/// A TLS refusal because the certificate is not the pinned one.
fn is_pin_mismatch(error: &str) -> bool {
    error.contains("has changed since the last connection")
        || error.contains("does not match its pin")
}

/// Pair on a control connection: the PIN, as a computer, then this
/// computer's iroh key once paired. The other's endpoint, when it pushed
/// one meanwhile.
async fn pair_on<S>(
    stream: S,
    pin: &str,
    wan_key: Option<String>,
) -> Result<Option<serde_json::Value>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut endpoint = None;
    let mut keep_endpoint = |packet: &NetworkPacket| {
        if packet.packet_type == crate::iroh_endpoint::ENDPOINT_PACKET
            && WanTarget::parse(&packet.body).is_some()
        {
            endpoint = Some(packet.body.clone());
        }
    };
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let ask = NetworkPacket::new("kdeconnect.pair")
        .with_body(serde_json::json!({"pin": pin, "computer": true}));
    send(&mut writer, &ask).await?;
    let answer = tokio::time::timeout(PAIR_ANSWER, async {
        loop {
            match read_packet(&mut reader).await? {
                Some(packet) if packet.packet_type == "kdeconnect.pair" => {
                    if let Some(paired) = packet.body.get("pair").and_then(|v| v.as_bool()) {
                        return Ok(paired);
                    }
                }
                // What it pushes to any client.
                Some(packet) => keep_endpoint(&packet),
                None => return Err("it closed the connection".to_string()),
            }
        }
    })
    .await
    .map_err(|_| "it did not answer the code".to_string())??;
    if !answer {
        return Err("it refused the code (a wrong or used PIN)".to_string());
    }
    if let Some(endpoint_id) = wan_key {
        let named = NetworkPacket::new(crate::plugins::pair::WAN_KEY)
            .with_body(serde_json::json!({"endpointId": endpoint_id}));
        send(&mut writer, &named).await?;
    }
    // Closed from this side and read to its end: the other computer takes
    // the packets in order, so once it closes, the key is registered and
    // the link may be dialed.
    let _ = writer.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(Some(packet)) = read_packet(&mut reader).await {
            keep_endpoint(&packet);
        }
    })
    .await;
    Ok(endpoint)
}

/// A computer's name as the link gives it, fit to show: no control or
/// invisible characters, at most 64 of them, else "Computer".
fn shown_name(name: &str) -> String {
    let shown: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !pocketdesk_core::protocol::kdeconnect::is_invisible_format(*c)
        })
        .take(64)
        .collect();
    let shown = shown.trim();
    if shown.is_empty() {
        "Computer".to_string()
    } else {
        shown.to_string()
    }
}

/// A computer that just paired here (one of this one's devices now) and
/// that this one still lists as one it paired with, the other way (BD1):
/// unpaired there and paired back this way, its stale entry took every
/// send to it by the computer road, ending "not linked now" though it was
/// connected, and it was redialed every five minutes. Pairing is one way
/// (a link from a computer already trusted here is refused), so it is
/// forgotten as the app's Unpair does, by the run loop. Whether it was
/// listed.
pub(crate) fn paired_here(dir: &Path, device_id: &str, now_unix: u64) -> bool {
    let listed = computers::read(dir)
        .computers
        .iter()
        .any(|c| c.device_id == device_id);
    if listed {
        let ask = ComputerAsk::Forget {
            device_id: device_id.to_string(),
        };
        match computers::request(dir, ask, now_unix) {
            Ok(_) => tracing::info!(
                device_id,
                "A computer paired here is no longer one this computer paired with"
            ),
            Err(e) => tracing::warn!("Could not forget computer {device_id}: {e}"),
        }
    }
    listed
}

/// Let the computer `device_id`, one this one paired with, drive this one's
/// keyboard and mouse (`allow`), or not. Shared input reads it each second.
fn let_drive(dir: &Path, device_id: &str, allow: bool) {
    let mut found = false;
    let saved = computers::update(dir, |file| {
        if let Some(computer) = file.computers.iter_mut().find(|c| c.device_id == device_id) {
            computer.lets_drive = allow;
            found = true;
        }
    });
    match saved {
        Ok(()) if found => tracing::info!(
            device_id,
            allow,
            "Shared input: whether another computer may drive this one"
        ),
        Ok(()) => tracing::debug!(device_id, "Shared input: no such computer to let drive"),
        Err(e) => {
            tracing::warn!("Could not change whether {device_id} may drive this computer: {e}")
        }
    }
}

/// Forget a computer: its entry and the pins for its addresses. It keeps
/// this one until its own owner removes it (its Devices tab).
fn forget(dir: &Path, certs: &CertManager, device_id: &str) {
    let mut gone = None;
    // Its pairing's line goes with it (BD3).
    let saved = computers::update(dir, |file| gone = file.remove(device_id));
    if let Err(e) = saved {
        tracing::warn!("Could not forget computer {device_id}: {e}");
        return;
    }
    let Some(computer) = gone else {
        return;
    };
    drop_pins(certs, &computer, &[]);
    tracing::info!(device_id, "Forgot another computer");
}

/// Remove the pins `computer` set for its addresses, but those under
/// `keep`: only the pin it set, as another computer may have that address
/// now.
fn drop_pins(certs: &CertManager, computer: &PeerComputer, keep: &[String]) {
    let pinned = parse_fingerprint(&computer.fingerprint);
    let pins: HashMap<String, [u8; 32]> = certs.known_peers().into_iter().collect();
    for label in computer
        .addresses
        .iter()
        .filter_map(|a| peer_label_for_address(a))
        .filter(|label| !keep.contains(label))
    {
        if pinned.is_some() && pins.get(&label) == pinned.as_ref() {
            let _ = certs.remove_peer(&label);
        }
    }
}

/// The other computer's iroh endpoint, as its endpoint packet names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WanTarget {
    pub(crate) endpoint_id: String,
    pub(crate) relay_urls: Vec<String>,
    pub(crate) direct_addrs: Vec<String>,
}

impl WanTarget {
    pub(crate) fn parse(body: &serde_json::Value) -> Option<Self> {
        let endpoint_id = body.get("endpointId")?.as_str()?;
        endpoint_id.parse::<iroh::EndpointId>().ok()?;
        let strings = |key: &str| -> Vec<String> {
            body.get(key)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Self {
            endpoint_id: endpoint_id.to_string(),
            relay_urls: strings("relayUrls"),
            direct_addrs: strings("directAddrs"),
        })
    }
}

/// A link that lasts at least this long before it closes is redialed at
/// once; one that closes sooner (admitted, then dropped) backs off.
const STEADY: Duration = Duration::from_secs(30);

/// How a link ended.
#[derive(Debug, PartialEq, Eq)]
enum LinkEnd {
    /// It was up, for how long, and closed.
    Closed(Duration),
    /// The other computer would not admit this one.
    Refused,
    /// It never came up.
    Failed(String),
}

/// Keep the link to `device_id` up while it is paired, backing off.
async fn keep_link(dir: PathBuf, wan: iroh::Endpoint, device_id: String) {
    let me = crate::kde::host_identity();
    let mut backoff = BACKOFF_FIRST;
    loop {
        let Some(computer) = computers::read(&dir)
            .computers
            .into_iter()
            .find(|c| c.device_id == device_id)
        else {
            return;
        };
        let Some(target) = computer.endpoint.as_ref().and_then(WanTarget::parse) else {
            set_link(&dir, &device_id, "away: no internet endpoint");
            tokio::time::sleep(BACKOFF_MOST).await;
            continue;
        };
        let end = run_link(&dir, &wan, &computer, &target, &me).await;
        let wait = match &end {
            LinkEnd::Closed(lasted) => {
                set_link(&dir, &device_id, "away: the link closed");
                if *lasted >= STEADY {
                    backoff = BACKOFF_FIRST;
                    backoff
                } else {
                    let wait = backoff;
                    backoff = (backoff * 2).min(BACKOFF_MOST);
                    wait
                }
            }
            LinkEnd::Refused => {
                tracing::warn!(
                    device_id,
                    "Another computer refused the link: paired there no more?"
                );
                set_link(&dir, &device_id, "refused");
                REFUSED_RETRY
            }
            LinkEnd::Failed(why) => {
                tracing::debug!(device_id, "No link to another computer: {why}");
                set_link(&dir, &device_id, &format!("away: {why}"));
                let wait = backoff;
                backoff = (backoff * 2).min(BACKOFF_MOST);
                wait
            }
        };
        tokio::time::sleep(wait).await;
    }
}

/// What a transfer waiting for its stream gets: the stream, or why none
/// will come.
type Arrival = Result<Box<dyn TransferIo>, String>;

/// The transfers waiting for the other computer's stream, by key.
static WAITERS: LazyLock<Mutex<HashMap<String, tokio::sync::oneshot::Sender<Arrival>>>> =
    LazyLock::new(Default::default);

/// A transfer waiting for its stream on a link, under a key of its own.
/// Dropped, it stops waiting.
struct Waiter {
    key: String,
    arrival: tokio::sync::oneshot::Receiver<Arrival>,
}

impl Waiter {
    /// Wait from now, so a fast answer is not missed: before the request
    /// goes out.
    fn new() -> Self {
        let key = uuid::Uuid::new_v4().simple().to_string();
        let (tx, arrival) = tokio::sync::oneshot::channel();
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.clone(), tx);
        Self { key, arrival }
    }

    /// The stream, its key line read; or why it will not come.
    async fn arrived(&mut self) -> Arrival {
        match tokio::time::timeout(STREAM_WAIT, &mut self.arrival).await {
            Ok(Ok(arrival)) => arrival,
            Ok(Err(_)) => Err("the link closed".to_string()),
            Err(_) => Err("the other computer did not answer in time".to_string()),
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// Read `stream`'s key line and hand it to the transfer it names; drop one
/// with no key in time, or a key nothing waits for.
async fn hand_over(mut stream: Box<dyn TransferIo>) {
    use tokio::io::AsyncReadExt;
    let mut line = [0u8; 33];
    match tokio::time::timeout(KEY_WAIT, stream.read_exact(&mut line)).await {
        Ok(Ok(_)) if line[32] == b'\n' => {}
        _ => {
            tracing::warn!("Another computer's stream came without a key line; dropped");
            return;
        }
    }
    let Ok(key) = std::str::from_utf8(&line[..32]) else {
        return;
    };
    let waiter = WAITERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);
    match waiter {
        Some(waiter) => {
            let _ = waiter.send(Ok(stream));
        }
        None => tracing::warn!("Another computer's stream for no transfer waiting; dropped"),
    }
}

/// The other computer refused the transfer `key` is for.
fn refused(key: &str, reason: &str) {
    let waiter = WAITERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(key);
    if let Some(waiter) = waiter {
        let _ = waiter.send(Err(format!("refused: {reason}")));
    }
}

/// Whether a transfer's failure is worth a retry: not a refusal, unless it
/// was only that the other computer was busy with another of this one's.
fn retried(reason: &str, attempt: usize) -> Option<Duration> {
    let busy = ["still running", "busy"].iter().any(|b| reason.contains(b));
    if reason.starts_with("refused") && !busy {
        return None;
    }
    RETRIES.get(attempt).map(|secs| Duration::from_secs(*secs))
}

/// One transfer at a time on each link: the other computer takes two from
/// one device at once and refuses the rest, so a folder's 50 offers would
/// otherwise mostly be refused.
fn turn(device_id: &str) -> Arc<tokio::sync::Semaphore> {
    static TURNS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> =
        LazyLock::new(Default::default);
    TURNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(device_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone()
}

/// A file's place in a folder send (T5), as a phone's share request says
/// it (T6b): the send's id, the top folder's name, and the file's names
/// under it, its own last.
pub(crate) struct FolderPart<'a> {
    pub id: &'a str,
    pub folder: &'a str,
    pub path: &'a [String],
}

/// Send the file at `path` to the computer `device_id` on the link (2h), as
/// a phone shares one: it opens a stream for it and keeps it in its
/// Downloads, or with `folder` (T5) in that send's new folder there. A
/// file that breaks off is sent again (continuing) up to 3 times. It shows
/// under way in the desktop app, whose Cancel stops it (T5 review: only the
/// receiving computer could).
pub(crate) async fn send_file(
    device_id: &str,
    path: &Path,
    folder: Option<&FolderPart<'_>>,
) -> Result<(), String> {
    // The link is the internet road, at home too: this computer's switch
    // applies, as the other's does.
    if !crate::events::current_config().internet_files {
        return Err("files over the internet are off on this computer (Settings)".to_string());
    }
    let size = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .ok_or("not a file")?;
    let name = match folder.and_then(|f| f.path.last()) {
        Some(name) => name.clone(),
        None => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("no file name")?,
    };
    let source = crate::offers::source_of(path, size);
    let key = format!("computer:{device_id}:{source}");
    let tracker = crate::transfers::track(crate::transfers::Shown {
        key: key.clone(),
        direction: pocketdesk_core::local::transfers::Direction::ToPhone,
        device_id: device_id.to_string(),
        name: name.clone(),
        folder: folder.map(|f| f.folder.to_string()),
        size,
    });
    let mut attempt = 0;
    loop {
        if crate::transfers::take_cancelled(&key) {
            return Err(crate::transfers::OFFER_CANCELLED.to_string());
        }
        let turn = turn(device_id);
        let _turn = turn.acquire().await.map_err(|e| e.to_string())?;
        let outgoing = live(device_id).ok_or("the computer is not linked now")?;
        let mut waiter = Waiter::new();
        let mut body = serde_json::json!({
            "filename": name,
            "sourceId": source,
            "payloadTransferInfo": {"key": waiter.key},
        });
        if let Some(part) = folder {
            body["folderId"] = part.id.into();
            body["folder"] = part.folder.into();
            body["path"] = part.path.into();
        }
        let ask = NetworkPacket::new("kdeconnect.share.request")
            .with_body(crate::offers::with_date(body, path))
            .with_payload_size(size);
        if outgoing.send(ask).await.is_err() {
            return Err("the link closed".to_string());
        }
        let outcome = match waiter.arrived().await {
            // Run where the app's Cancel can stop it (C1's registry).
            Ok(stream) => {
                let (done, result) = tokio::sync::oneshot::channel();
                let (key_sent, path_owned) = (waiter.key.clone(), path.to_path_buf());
                let progress = tracker.progress();
                crate::plugins::share::running()
                    .spawn(key.clone(), async move {
                        let sent = crate::plugins::share::send_file_counted(
                            crate::plugins::share::Route::arrived(stream),
                            &key_sent,
                            &path_owned,
                            progress,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                        let _ = done.send(sent);
                    })
                    .await;
                match result.await {
                    Ok(sent) => sent,
                    // Stopped by the app's Cancel.
                    Err(_) => {
                        crate::transfers::take_cancelled(&key);
                        return Err(crate::transfers::OFFER_CANCELLED.to_string());
                    }
                }
            }
            Err(reason) => Err(reason),
        };
        match outcome {
            Ok(()) => return Ok(()),
            Err(reason) => match retried(&reason, attempt) {
                Some(wait) => {
                    tracing::info!(
                        "Sending {} to another computer failed ({reason}); again in {wait:?}",
                        crate::offers::unnamed(path)
                    );
                    // Others go while this one waits.
                    drop(_turn);
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                None => return Err(reason),
            },
        }
    }
}

/// Fetch a file the computer `device_id` offered this one (2h), as a phone
/// fetches an offer, into Downloads. One that breaks off is fetched again
/// (continuing) up to 3 times.
async fn fetch_offer(device_id: String, offer: serde_json::Value) {
    let Some(id) = offer.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
        return;
    };
    let name = offer
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("a file")
        .to_string();
    if !crate::events::current_config().internet_files {
        // Said to the other computer, so its sent list ends now.
        if let Some(outgoing) = live(&device_id) {
            let declined =
                NetworkPacket::new(crate::offers::DECLINE).with_body(serde_json::json!({
                    "id": id,
                    "reason": "files over the internet are off on that computer",
                }));
            let _ = outgoing.send(declined).await;
        }
        tracing::info!("Declined {name} from another computer: files over the internet are off");
        return;
    }
    let mut attempt = 0;
    loop {
        let turn = turn(&device_id);
        let Ok(_turn) = turn.acquire().await else {
            return;
        };
        let Some(outgoing) = live(&device_id) else {
            tracing::warn!("Not fetched {name} from another computer: the link went");
            return;
        };
        let mut waiter = Waiter::new();
        let ask = NetworkPacket::new(crate::plugins::share::FETCH)
            .with_body(serde_json::json!({"offer": id, "key": waiter.key}));
        if outgoing.send(ask).await.is_err() {
            return;
        }
        let outcome = match waiter.arrived().await {
            Ok(stream) => crate::plugins::share::receive_offered(stream, &device_id, &offer)
                .await
                .map_err(|e| e.to_string()),
            Err(reason) => Err(reason),
        };
        match outcome {
            Ok(path) => {
                tracing::info!("Received {} from another computer", path.display());
                return;
            }
            Err(reason) => match retried(&reason, attempt) {
                Some(wait) => {
                    tracing::info!(
                        "Fetching {name} from another computer failed ({reason}); again in {wait:?}"
                    );
                    // Others go while this one waits.
                    drop(_turn);
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                None => {
                    tracing::warn!("Not fetched {name} from another computer: {reason}");
                    return;
                }
            },
        }
    }
}

/// Why an image on its way to or from another computer went no further: a
/// later switch of the shared keyboard and mouse came first (BK3).
const LATER_SWITCH: &str = "a later switch came first";

/// Send this computer's clipboard image to the computer `device_id` on the
/// link (BK3), as a phone sends its own (N4): told its size and a key,
/// that computer opens a stream for it and puts it on its clipboard.
/// Marked with `follows` when it follows the mouse; `still` says whether it
/// is still wanted, asked before it is offered and before its bytes go.
pub(crate) async fn send_clipboard_image(
    device_id: &str,
    image: Arc<crate::plugins::clipboard::Image>,
    follows: bool,
    still: impl Fn() -> bool,
) -> Result<(), String> {
    // The link is the internet road, at home too, as for files.
    if !crate::events::current_config().internet_files {
        return Err("files over the internet are off on this computer (Settings)".to_string());
    }
    let outgoing = live(device_id).ok_or("the computer is not linked now")?;
    image_on_link(&outgoing, &image.png, follows, still).await
}

/// [`send_clipboard_image`] on the link `outgoing` sends on.
async fn image_on_link(
    outgoing: &mpsc::Sender<NetworkPacket>,
    png: &[u8],
    follows: bool,
    still: impl Fn() -> bool,
) -> Result<(), String> {
    if !still() {
        return Err(LATER_SWITCH.to_string());
    }
    let mut waiter = Waiter::new();
    let size = png.len() as u64;
    let offer = crate::clipboard_push::image_offer_packet(size, &waiter.key, follows);
    if outgoing.send(offer).await.is_err() {
        return Err("the link closed".to_string());
    }
    let stream = waiter.arrived().await?;
    // Dropped unopened: the other computer's receive ends at once.
    if !still() {
        return Err(LATER_SWITCH.to_string());
    }
    let mut reader = png;
    crate::plugins::share::send_reader(
        crate::plugins::share::Route::arrived(stream),
        &waiter.key,
        &mut reader,
        size,
    )
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Fetch the clipboard image `id`, of `size` bytes, that the computer
/// `device_id` told this one of (BK3), and put it on this computer's
/// clipboard; with `follows`, as one following the mouse here, which a
/// later switch drops.
async fn fetch_clipboard_image(device_id: String, id: String, size: u64, follows: bool) {
    // The link is the internet road, at home too, as for files.
    if !crate::events::current_config().internet_files {
        tracing::debug!(
            "Another computer's clipboard image not fetched: files over the internet are off"
        );
        return;
    }
    // A transfer slot, as a phone's image takes one (BK3 review).
    let _slot = match crate::plugins::share::transfer_slots().take(&device_id) {
        Ok(slot) => slot,
        Err(reason) => {
            tracing::debug!("Another computer's clipboard image not fetched: {reason}");
            return;
        }
    };
    let landing = follows.then(|| crate::clipboard_push::Landing::begin(&device_id));
    let Some(outgoing) = live(&device_id) else {
        return;
    };
    let (from, named) = (device_id.as_str(), id.as_str());
    let fetched = fetch_image_on_link(&outgoing, &id, size, |png| async move {
        if landing.as_ref().is_some_and(|l| !l.current()) {
            return Err(crate::plugins::share::io_error("clipboard", LATER_SWITCH));
        }
        crate::plugins::clipboard::put_image(from, png, Some(named)).await
    })
    .await;
    // Who and how large, never the image.
    match fetched {
        Ok(()) => tracing::info!("Clipboard image received from {device_id} ({size} bytes)"),
        Err(e) => tracing::debug!("Clipboard image from {device_id} not taken: {e}"),
    }
}

/// [`fetch_clipboard_image`] on the link `outgoing` sends on: the bytes go
/// to `keep`, and the other computer hears they were saved only if it
/// succeeds.
async fn fetch_image_on_link<F, Fut>(
    outgoing: &mpsc::Sender<NetworkPacket>,
    id: &str,
    size: u64,
    keep: F,
) -> Result<(), String>
where
    F: FnOnce(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = pocketdesk_core::error::Result<()>>,
{
    let mut waiter = Waiter::new();
    let ask = NetworkPacket::new(pocketdesk_core::protocol::clipboard::CLIPBOARD_IMAGE_FETCH)
        .with_body(serde_json::json!({"id": id, "key": waiter.key}));
    if outgoing.send(ask).await.is_err() {
        return Err("the link closed".to_string());
    }
    let stream = waiter.arrived().await?;
    crate::plugins::share::receive_bytes(
        crate::plugins::share::Route::arrived(stream),
        &waiter.key,
        size,
        pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE,
        keep,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Stops a task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One link, until it ends.
async fn run_link(
    dir: &Path,
    wan: &iroh::Endpoint,
    computer: &PeerComputer,
    target: &WanTarget,
    me: &DeviceIdentity,
) -> LinkEnd {
    let stream = match pocketdesk_core::streaming::IrohControlStream::dial_from(
        wan,
        &target.endpoint_id,
        &target.relay_urls,
        &target.direct_addrs,
        LINK_DIAL,
    )
    .await
    {
        Ok(stream) => stream,
        Err(e) => return LinkEnd::Failed(format!("{e:#}")),
    };
    // The streams the other computer opens for files (2h).
    let acceptor = stream.link_acceptor();
    let stream = match tokio::time::timeout(
        LINK_DIAL,
        pocketdesk_core::protocol::connection::internet_control_handshake(
            stream,
            &computer.name,
            me,
        ),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) if is_refusal(&e.to_string()) => return LinkEnd::Refused,
        Ok(Err(e)) => return LinkEnd::Failed(e.to_string()),
        Err(_) => return LinkEnd::Failed("no handshake in time".to_string()),
    };
    tracing::info!(device_id = %computer.device_id, "Linked to another computer");
    // Shared input rides the link's connection too (its datagrams and uni
    // streams) while the link is up, however it ends.
    let _shared_input = crate::shared_input::link::hold(&computer.device_id, stream.connection());
    set_link(dir, &computer.device_id, "linked");
    let linked_at = std::time::Instant::now();
    let (outgoing_tx, mut outgoing) = mpsc::channel(32);
    let _live = LiveLink::new(&computer.device_id, outgoing_tx);
    let _accepting = acceptor.map(|acceptor| {
        AbortOnDrop(tokio::spawn(async move {
            while let Some(stream) = acceptor.accept().await {
                tokio::spawn(hand_over(stream));
            }
        }))
    });
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    loop {
        tokio::select! {
            packet = read_packet(&mut reader) => match packet {
                Ok(Some(packet)) => take(dir, &computer.device_id, &packet).await,
                Ok(None) | Err(_) => return LinkEnd::Closed(linked_at.elapsed()),
            },
            Some(packet) = outgoing.recv() => {
                if send(&mut writer, &packet).await.is_err() {
                    return LinkEnd::Closed(linked_at.elapsed());
                }
            }
        }
    }
}

/// What this computer takes from another on the link: its endpoint, kept
/// current, its clipboard while it is shared (2g) or as it follows the
/// mouse (text, or an image fetched, BK3), and the files it offers
/// and its refusals of a transfer (2h). The rest it pushes to any client
/// (the HUD, wake info, control) is for phones.
async fn take(dir: &Path, device_id: &str, packet: &NetworkPacket) {
    // Its screens, asked for a window that shows them (BK7).
    if packet.packet_type == MONITORS {
        SCREENS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(device_id.to_string(), screens_in(&packet.body));
        return;
    }
    if packet.packet_type == crate::offers::OFFER {
        tokio::spawn(fetch_offer(device_id.to_string(), packet.body.clone()));
        return;
    }
    if packet.packet_type == crate::plugins::share::FETCH_REFUSED {
        if let (Some(key), reason) = (
            packet.body.get("key").and_then(|v| v.as_str()),
            packet.body.get("reason").and_then(|v| v.as_str()),
        ) {
            refused(key, reason.unwrap_or("no reason given"));
        }
        return;
    }
    if packet.packet_type == "kdeconnect.clipboard" {
        if !crate::clipboard_push::with_computers()
            && !crate::clipboard_push::takes_follow(device_id, &packet.body)
        {
            return;
        }
        if let Some(content) = packet.body.get("content").and_then(|v| v.as_str()) {
            crate::clipboard_push::follow_arrived(&packet.body);
            // Not sent back to the computer it came from.
            crate::clipboard_push::note_from_phone(device_id, content);
            if let Err(e) = crate::plugins::clipboard::set_clipboard(content).await {
                tracing::warn!("Another computer's clipboard was not set: {e}");
            }
        } else if let Some((id, size)) = crate::clipboard_push::told_image(&packet.body) {
            // Its image (BK3): fetched on a stream it opens on the link.
            let follows = crate::clipboard_push::marked(&packet.body);
            tokio::spawn(fetch_clipboard_image(
                device_id.to_string(),
                id,
                size,
                follows,
            ));
        }
        return;
    }
    if packet.packet_type == crate::iroh_endpoint::ENDPOINT_PACKET
        && let Some(target) = WanTarget::parse(&packet.body)
    {
        // Its addresses and relays move; its key does not: one naming
        // another key is not taken.
        let body = packet.body.clone();
        let _ = computers::update(dir, |file| {
            if let Some(c) = file.computers.iter_mut().find(|c| c.device_id == device_id) {
                let same_key = c
                    .endpoint
                    .as_ref()
                    .and_then(WanTarget::parse)
                    .is_none_or(|known| known.endpoint_id == target.endpoint_id);
                if same_key {
                    c.endpoint = Some(body);
                } else {
                    tracing::warn!(
                        device_id,
                        "Another computer's endpoint named another key; not taken"
                    );
                }
            }
        });
    }
}

/// Record how the link to `device_id` is doing, when it changed.
fn set_link(dir: &Path, device_id: &str, link: &str) {
    let now = pocketdesk_core::local::pin::now_unix();
    let saved = computers::update(dir, |file| {
        if let Some(c) = file.computers.iter_mut().find(|c| c.device_id == device_id)
            && c.link != link
        {
            c.link = link.to_string();
            c.link_unix = now;
        }
    });
    if let Err(e) = saved {
        tracing::debug!("Could not record the link's state: {e}");
    }
}

/// The internet road's refusal (`HANDSHAKE_REFUSED`), as a handshake error
/// carries it.
fn is_refusal(error: &str) -> bool {
    error.contains(pocketdesk_core::protocol::HANDSHAKE_REFUSED)
}

/// Hex SHA-256, either case.
pub(crate) fn parse_fingerprint(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, packet: &NetworkPacket) -> Result<(), String> {
    let wire = packet.to_wire().map_err(|e| e.to_string())?;
    writer.write_all(&wire).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}

/// The next packet, `None` at the end; a line too long or a packet that
/// does not read is an error.
async fn read_packet<R>(reader: &mut BufReader<R>) -> Result<Option<NetworkPacket>, String>
where
    R: AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut line = String::new();
    let read = (&mut *reader)
        .take(MAX_LINE)
        .read_line(&mut line)
        .await
        .map_err(|e| e.to_string())?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') && read as u64 == MAX_LINE {
        return Err("a line too long".to_string());
    }
    NetworkPacket::from_wire(&line)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BK7: a computer's screens, as its monitors answer names them, in
    /// its own order, and none from an answer that names none.
    #[test]
    fn a_computers_screens_are_read_from_its_answer() {
        let body = serde_json::json!({"count": 2, "monitors": [
            {"index": 1, "name": "HDMI-A-1", "width": 2560, "height": 1440, "is_primary": false},
            {"index": 0, "name": "eDP-1", "width": 1920, "height": 1080, "is_primary": true},
        ]});
        assert_eq!(
            screens_in(&body),
            ["eDP-1 (1920x1080)", "HDMI-A-1 (2560x1440)"]
        );
        assert!(screens_in(&serde_json::json!({})).is_empty());
        assert_eq!(
            screens_in(&serde_json::json!({"monitors": [{}]})),
            ["Screen 1"],
            "a screen with no name is still one"
        );
    }

    /// BD1: a computer paired here that this one still lists as one it
    /// paired with (the other way) is asked forgotten, as Unpair does.
    #[test]
    fn a_computer_paired_here_is_no_longer_one_this_one_paired_with() {
        let dir = std::env::temp_dir().join(format!("pd-paired-here-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        computers::update(&dir, |file| {
            file.computers.push(PeerComputer {
                device_id: "pc-b".to_string(),
                name: "Desk".to_string(),
                addresses: vec!["192.0.2.20".to_string()],
                control_port: 1716,
                stream_port: 4716,
                fingerprint: "ab".repeat(32),
                endpoint: None,
                paired_unix: 1,
                link: "refused".to_string(),
                link_unix: 1,
                lets_drive: false,
                mine: false,
            })
        })
        .unwrap();
        let now = pocketdesk_core::local::pin::now_unix();
        assert!(!paired_here(&dir, "pc-c", now), "not listed");
        assert!(computers::take_all(&dir, now).is_empty());
        assert!(paired_here(&dir, "pc-b", now));
        let asked: Vec<_> = computers::take_all(&dir, now)
            .into_iter()
            .map(|r| r.ask)
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            asked,
            [ComputerAsk::Forget {
                device_id: "pc-b".to_string()
            }]
        );
    }

    /// BD5: with the internet road off, no link is kept, and its computers
    /// say so rather than "connecting" for good.
    #[test]
    fn with_the_road_off_a_computer_says_so() {
        assert_eq!(first_link(true), "connecting");
        assert!(
            first_link(false).starts_with("away: "),
            "{}",
            first_link(false)
        );
    }

    /// BD6: a computer paired again at new addresses drops the pins it set
    /// for the old ones; the new ones, and another computer's, stay.
    #[test]
    fn the_pins_a_computer_set_go_but_those_kept() {
        let dir = std::env::temp_dir().join(format!("pd-drop-pins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let certs = CertManager::load_or_create(&dir).unwrap();
        let label = |a: &str| peer_label_for_address(a).unwrap();
        for address in ["192.0.2.30", "192.0.2.31"] {
            certs.pin_peer(&label(address), [0xab; 32]).unwrap();
        }
        certs.pin_peer(&label("192.0.2.32"), [0xcd; 32]).unwrap();
        let was = PeerComputer {
            device_id: "pc-b".to_string(),
            name: "Desk".to_string(),
            addresses: ["192.0.2.30", "192.0.2.31", "192.0.2.32"]
                .map(str::to_string)
                .to_vec(),
            control_port: 1716,
            stream_port: 4716,
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 1,
            link: String::new(),
            link_unix: 1,
            lets_drive: false,
            mine: false,
        };
        drop_pins(&certs, &was, &[label("192.0.2.31")]);
        let left: HashMap<String, [u8; 32]> = certs.known_peers().into_iter().collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!left.contains_key(&label("192.0.2.30")), "an old address");
        assert_eq!(left.get(&label("192.0.2.31")), Some(&[0xab; 32]), "kept");
        assert_eq!(
            left.get(&label("192.0.2.32")),
            Some(&[0xcd; 32]),
            "another's"
        );
    }

    #[test]
    fn a_fingerprint_reads_in_either_case_and_nothing_else_does() {
        let lower = "ab".repeat(32);
        assert_eq!(parse_fingerprint(&lower), Some([0xab; 32]));
        assert_eq!(parse_fingerprint(&lower.to_uppercase()), Some([0xab; 32]));
        assert_eq!(parse_fingerprint(&"ab".repeat(31)), None, "too short");
        assert_eq!(parse_fingerprint(&"zz".repeat(32)), None, "not hex");
        assert_eq!(parse_fingerprint(&"é".repeat(32)), None, "not ascii");
    }

    #[test]
    fn an_endpoint_needs_a_real_iroh_id() {
        let id = iroh::SecretKey::generate().public().to_string();
        let body = serde_json::json!({
            "endpointId": id,
            "relayUrls": ["https://relay.example.org./"],
            "directAddrs": ["192.0.2.1:4433", 7],
        });
        assert_eq!(
            WanTarget::parse(&body),
            Some(WanTarget {
                endpoint_id: id,
                relay_urls: vec!["https://relay.example.org./".to_string()],
                direct_addrs: vec!["192.0.2.1:4433".to_string()],
            })
        );
        assert_eq!(
            WanTarget::parse(&serde_json::json!({"endpointId": "nope"})),
            None
        );
        assert_eq!(WanTarget::parse(&serde_json::json!({})), None);
    }

    #[test]
    fn a_transfer_is_tried_again_unless_it_was_refused() {
        assert_eq!(retried("timed out", 0), Some(Duration::from_secs(5)));
        assert_eq!(retried("reset", 2), Some(Duration::from_secs(45)));
        assert_eq!(retried("reset", 3), None, "three retries at most");
        assert_eq!(retried("refused: no room", 0), None);
        assert_eq!(
            retried("refused: this phone's last transfer is still running", 0),
            Some(Duration::from_secs(5)),
            "busy passes"
        );
    }

    #[tokio::test]
    async fn one_transfer_at_a_time_on_each_link() {
        let first = turn("desk-one-at-a-time");
        let held = first.clone().acquire_owned().await.unwrap();
        assert!(
            turn("desk-one-at-a-time").try_acquire().is_err(),
            "the second waits"
        );
        assert!(
            turn("another-desk").try_acquire().is_ok(),
            "another link does not"
        );
        drop(held);
        assert!(turn("desk-one-at-a-time").try_acquire().is_ok());
    }

    #[tokio::test]
    async fn a_stream_goes_to_the_transfer_its_key_names_and_a_refusal_ends_one() {
        let mut waiter = Waiter::new();
        let (mut theirs, ours) = tokio::io::duplex(1024);
        tokio::spawn(hand_over(Box::new(ours)));
        use tokio::io::AsyncWriteExt;
        theirs
            .write_all(format!("{}\nbytes", waiter.key).as_bytes())
            .await
            .unwrap();
        let mut stream = waiter.arrived().await.unwrap();
        let mut rest = [0u8; 5];
        use tokio::io::AsyncReadExt;
        stream.read_exact(&mut rest).await.unwrap();
        assert_eq!(
            &rest, b"bytes",
            "the key line is read, the rest is the transfer's"
        );

        let mut refused_one = Waiter::new();
        refused(&refused_one.key, "no room");
        assert_eq!(
            refused_one.arrived().await.err().unwrap(),
            "refused: no room"
        );
        assert!(
            !WAITERS.lock().unwrap().contains_key(&refused_one.key),
            "nothing left waiting"
        );
    }

    /// The other computer's side of the link's streams, in memory: each it
    /// opens goes to this one's [`hand_over`], as the link's acceptor does.
    struct ToHandOver;

    #[async_trait::async_trait]
    impl pocketdesk_core::protocol::kdeconnect::OpenLinkStream for ToHandOver {
        async fn open(&self) -> std::io::Result<Box<dyn TransferIo>> {
            let (theirs, ours) = tokio::io::duplex(64 * 1024);
            tokio::spawn(hand_over(Box::new(ours)));
            Ok(Box::new(theirs))
        }
    }

    fn other_computers_route() -> crate::plugins::share::Route {
        crate::plugins::share::Route::Link(Arc::new(ToHandOver))
    }

    /// A picture of `len` bytes, not a real PNG: the link carries bytes.
    fn picture(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// BK3: a clipboard image offered on the link (its size and key, no
    /// port) goes over the stream the other computer opens for it, as a
    /// phone's does, and the other computer receives it whole.
    #[tokio::test]
    async fn a_clipboard_image_goes_over_the_stream_the_other_computer_opens() {
        let png = picture(300_000);
        let (outgoing, mut packets) = mpsc::channel::<NetworkPacket>(4);
        let other = tokio::spawn(async move {
            let offer = packets.recv().await.unwrap();
            assert_eq!(
                offer.packet_type,
                pocketdesk_core::protocol::clipboard::CLIPBOARD_IMAGE
            );
            assert_eq!(offer.body["follows"], true);
            assert!(offer.body.get("port").is_none());
            let size = offer.body["size"].as_u64().unwrap();
            let key = offer.body["key"].as_str().unwrap().to_string();
            let (kept, got) = tokio::sync::oneshot::channel();
            crate::plugins::share::receive_bytes(
                other_computers_route(),
                &key,
                size,
                pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE,
                |bytes| async move {
                    let _ = kept.send(bytes);
                    Ok(())
                },
            )
            .await
            .unwrap();
            got.await.unwrap()
        });
        image_on_link(&outgoing, &png, true, || true).await.unwrap();
        assert_eq!(other.await.unwrap(), png, "received whole");
    }

    /// BK3: an image a later switch overtakes is not offered, and one
    /// overtaken while the other computer opens its stream sends nothing.
    #[tokio::test]
    async fn a_clipboard_image_a_later_switch_overtakes_goes_no_further() {
        let png = picture(1000);
        let (outgoing, mut packets) = mpsc::channel::<NetworkPacket>(4);
        let refused = image_on_link(&outgoing, &png, true, || false).await;
        assert_eq!(refused.unwrap_err(), LATER_SWITCH);
        assert!(packets.try_recv().is_err(), "not offered");

        let wanted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let switched = wanted.clone();
        let other = tokio::spawn(async move {
            let offer = packets.recv().await.unwrap();
            let key = offer.body["key"].as_str().unwrap().to_string();
            // The switch comes while the stream opens.
            switched.store(false, std::sync::atomic::Ordering::SeqCst);
            crate::plugins::share::receive_bytes(
                other_computers_route(),
                &key,
                1000,
                pocketdesk_core::protocol::clipboard::MAX_CLIPBOARD_IMAGE,
                |_| async { panic!("nothing should arrive") },
            )
            .await
        });
        let overtaken = image_on_link(&outgoing, &png, true, || {
            wanted.load(std::sync::atomic::Ordering::SeqCst)
        })
        .await;
        assert_eq!(overtaken.unwrap_err(), LATER_SWITCH);
        assert!(other.await.unwrap().is_err(), "the other end got nothing");
    }

    /// BK3: an image the other computer told this one of is fetched on the
    /// link (its id and a key), over the stream that computer opens, and
    /// handed whole to what keeps it.
    #[tokio::test]
    async fn a_told_clipboard_image_is_fetched_over_the_stream_the_other_opens() {
        let png = picture(200_000);
        let sent = png.clone();
        let (outgoing, mut packets) = mpsc::channel::<NetworkPacket>(4);
        let other = tokio::spawn(async move {
            let ask = packets.recv().await.unwrap();
            assert_eq!(
                ask.packet_type,
                pocketdesk_core::protocol::clipboard::CLIPBOARD_IMAGE_FETCH
            );
            assert_eq!(ask.body["id"], "ab12");
            assert!(ask.body.get("port").is_none());
            let key = ask.body["key"].as_str().unwrap().to_string();
            let mut reader = sent.as_slice();
            crate::plugins::share::send_reader(
                other_computers_route(),
                &key,
                &mut reader,
                sent.len() as u64,
            )
            .await
        });
        let (kept, got) = tokio::sync::oneshot::channel();
        fetch_image_on_link(&outgoing, "ab12", png.len() as u64, |bytes| async move {
            let _ = kept.send(bytes);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(got.await.unwrap(), png);
        assert!(other.await.unwrap().is_ok(), "the other end heard it saved");
    }

    #[test]
    fn a_computers_name_is_shown_without_what_hides_or_misleads() {
        assert_eq!(shown_name("Desk"), "Desk");
        assert_eq!(shown_name("in\u{202E}fdp.exe"), "infdp.exe");
        assert_eq!(shown_name("  \n "), "Computer");
        assert_eq!(shown_name(&"x".repeat(100)).len(), 64);
    }

    #[tokio::test]
    async fn the_endpoint_it_pushes_while_pairing_is_kept() {
        let id = iroh::SecretKey::generate().public().to_string();
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let pushed_id = id.clone();
        let other = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(theirs);
            let mut reader = BufReader::new(reader);
            read_packet(&mut reader).await.unwrap().unwrap();
            let answer = NetworkPacket::new("kdeconnect.pair")
                .with_body(serde_json::json!({"pair": true, "serverId": "b"}));
            send(&mut writer, &answer).await.unwrap();
            let endpoint = NetworkPacket::new(crate::iroh_endpoint::ENDPOINT_PACKET).with_body(
                serde_json::json!({"endpointId": pushed_id, "relayUrls": [], "directAddrs": []}),
            );
            send(&mut writer, &endpoint).await.unwrap();
            while read_packet(&mut reader).await.unwrap().is_some() {}
        });
        let pushed = pair_on(ours, "123456", None).await.unwrap();
        other.await.unwrap();
        assert_eq!(pushed.unwrap()["endpointId"], id);
    }

    #[test]
    fn the_internet_roads_refusal_is_told_from_a_failure() {
        let refused = format!(
            "handshake failed with Desk: {}",
            pocketdesk_core::protocol::HANDSHAKE_REFUSED
        );
        assert!(is_refusal(&refused));
        assert!(!is_refusal("handshake failed with Desk: timed out"));
        assert!(is_pin_mismatch(
            "TLS: unexpected error: Certificate for V4(..) has changed since the last connection."
        ));
        assert!(!is_pin_mismatch("connection refused (os error 10061)"));
    }

    #[tokio::test]
    async fn pairing_sends_the_pin_as_a_computer_then_the_wan_key() {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let other = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(theirs);
            let mut reader = BufReader::new(reader);
            let ask = read_packet(&mut reader).await.unwrap().unwrap();
            assert_eq!(ask.packet_type, "kdeconnect.pair");
            assert_eq!(ask.body["pin"], "123456");
            assert_eq!(ask.body["computer"], true);
            // What a computer pushes any client comes first sometimes.
            send(&mut writer, &NetworkPacket::new("kdeconnect.battery"))
                .await
                .unwrap();
            let answer = NetworkPacket::new("kdeconnect.pair")
                .with_body(serde_json::json!({"pair": true, "serverId": "b"}));
            send(&mut writer, &answer).await.unwrap();
            let named = read_packet(&mut reader).await.unwrap().unwrap();
            assert_eq!(named.packet_type, crate::plugins::pair::WAN_KEY);
            assert_eq!(named.body["endpointId"], "e".repeat(64));
            assert!(
                read_packet(&mut reader).await.unwrap().is_none(),
                "then closed"
            );
        });
        let pushed = pair_on(ours, "123456", Some("e".repeat(64))).await.unwrap();
        other.await.unwrap();
        assert_eq!(pushed, None, "no endpoint pushed in this one");
    }

    #[tokio::test]
    async fn a_refused_code_pairs_nothing_and_names_no_key() {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let other = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(theirs);
            let mut reader = BufReader::new(reader);
            read_packet(&mut reader).await.unwrap().unwrap();
            let answer = NetworkPacket::new("kdeconnect.pair")
                .with_body(serde_json::json!({"pair": false, "serverId": ""}));
            send(&mut writer, &answer).await.unwrap();
            // Nothing more comes: no key for a pairing that failed.
            let rest =
                tokio::time::timeout(Duration::from_millis(300), read_packet(&mut reader)).await;
            assert!(!matches!(rest, Ok(Ok(Some(_)))), "{rest:?}");
        });
        let refused = pair_on(ours, "000000", Some("e".repeat(64))).await;
        assert!(refused.unwrap_err().contains("refused"));
        other.await.unwrap();
    }
}
