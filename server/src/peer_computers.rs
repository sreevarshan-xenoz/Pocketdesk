//! The owner's other computers (2f): pairing with one from its pairing
//! link, and the link this computer keeps to each over the internet road.
//! Design: `docs/superpowers/specs/2026-10-17-computers-design.md`.
//!
//! Pairing runs over TLS on the other computer's control port, its
//! certificate pinned from the link before the first byte; this computer
//! then names its iroh key on that connection (`wan_key`). The link always
//! rides iroh, dialed from this server's own WAN endpoint (a second
//! endpoint with the same key could not use a relay, 2b): direct at home,
//! direct or relayed from anywhere. On it this computer is the other's
//! client, as a phone is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::computers::{self, ComputerAsk, PairingOutcome, PeerComputer};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::paths::COMPUTERS_DIR;
use pocketdesk_core::protocol::connection::ConnectionManager;
use pocketdesk_core::protocol::kdeconnect::{DeviceIdentity, NetworkPacket};
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

/// The server's side: take the app's requests and keep a link to each
/// paired computer, for the server's life.
pub async fn run(cert_manager: Arc<CertManager>, wan: Option<iroh::Endpoint>) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let mut links: HashMap<String, Link> = HashMap::new();
    loop {
        for request in computers::take_all(&dir, pocketdesk_core::local::pin::now_unix()) {
            match request.ask {
                ComputerAsk::Pair { link } => {
                    pair(&dir, &cert_manager, wan.as_ref(), &link, &request.id).await
                }
                ComputerAsk::Forget { device_id } => {
                    if let Some(link) = links.remove(&device_id) {
                        link.task.abort();
                    }
                    forget(&dir, &cert_manager, &device_id);
                }
            }
        }
        if let Some(wan) = &wan {
            keep_links(&dir, wan, &mut links);
        }
        crate::inbox::wait(COMPUTERS_DIR, POLL).await;
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

/// A link for every paired computer, and none for one forgotten.
fn keep_links(dir: &Path, wan: &iroh::Endpoint, links: &mut HashMap<String, Link>) {
    let paired: Vec<String> = computers::read(dir)
        .computers
        .into_iter()
        .map(|c| c.device_id)
        .collect();
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

/// Pair with the computer whose pairing link `uri` is, and say how it went.
async fn pair(
    dir: &Path,
    certs: &CertManager,
    wan: Option<&iroh::Endpoint>,
    uri: &str,
    request: &str,
) {
    let name = PairLink::from_uri(uri)
        .map(|link| link.name)
        .unwrap_or_default();
    let outcome = match pair_with(dir, certs, wan, uri).await {
        Ok(computer) => {
            tracing::info!(device_id = %computer.device_id, "Paired with another computer");
            "paired".to_string()
        }
        Err(why) => {
            tracing::warn!("Pairing with another computer failed: {why}");
            format!("failed: {why}")
        }
    };
    let saved = computers::update(dir, |file| {
        file.last_pairing = Some(PairingOutcome {
            name,
            outcome,
            unix: pocketdesk_core::local::pin::now_unix(),
            request: request.to_string(),
        })
    });
    if let Err(e) = saved {
        tracing::warn!("Could not record the pairing's outcome: {e}");
    }
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
    let paired = dial_and_pair(certs, &link, &me, wan).await;
    if paired.is_err() {
        for label in &labels {
            let _ = match before.get(label) {
                Some(old) => certs.pin_peer(label, *old),
                None => certs.remove_peer(label).map(|_| ()),
            };
        }
    }
    paired?;
    let now = pocketdesk_core::local::pin::now_unix();
    let computer = PeerComputer {
        device_id: link.device_id.clone(),
        name: link.name.clone(),
        addresses: link.addresses.clone(),
        control_port: link.control_port,
        fingerprint: fingerprint.to_ascii_lowercase(),
        endpoint: link
            .endpoint
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .filter(|body| WanTarget::parse(body).is_some()),
        paired_unix: now,
        link: "connecting".to_string(),
        link_unix: now,
    };
    computers::update(dir, |file| {
        file.computers.retain(|c| c.device_id != computer.device_id);
        file.computers.push(computer.clone());
    })
    .map_err(|e| format!("paired, but could not record it: {e}"))?;
    Ok(computer)
}

/// Dial the link's addresses in order over TLS and pair on the first that
/// answers.
async fn dial_and_pair(
    certs: &CertManager,
    link: &PairLink,
    me: &DeviceIdentity,
    wan: Option<&iroh::Endpoint>,
) -> Result<(), String> {
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
/// computer's iroh key once paired.
async fn pair_on<S>(stream: S, pin: &str, wan_key: Option<String>) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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
                Some(_) => {} // what it pushes to any client
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
        while let Ok(Some(_)) = read_packet(&mut reader).await {}
    })
    .await;
    Ok(())
}

/// Forget a computer: its entry and the pins for its addresses. It keeps
/// this one until its own owner removes it (its Devices tab).
fn forget(dir: &Path, certs: &CertManager, device_id: &str) {
    let mut gone = None;
    let saved = computers::update(dir, |file| {
        if let Some(at) = file.computers.iter().position(|c| c.device_id == device_id) {
            gone = Some(file.computers.remove(at));
        }
    });
    if let Err(e) = saved {
        tracing::warn!("Could not forget computer {device_id}: {e}");
        return;
    }
    let Some(computer) = gone else {
        return;
    };
    let pinned = parse_fingerprint(&computer.fingerprint);
    let pins: HashMap<String, [u8; 32]> = certs.known_peers().into_iter().collect();
    for label in computer
        .addresses
        .iter()
        .filter_map(|a| peer_label_for_address(a))
    {
        // Only the pin it set: another computer may have that address now.
        if pinned.is_some() && pins.get(&label) == pinned.as_ref() {
            let _ = certs.remove_peer(&label);
        }
    }
    tracing::info!(device_id, "Forgot another computer");
}

/// The other computer's iroh endpoint, as its endpoint packet names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WanTarget {
    endpoint_id: String,
    relay_urls: Vec<String>,
    direct_addrs: Vec<String>,
}

impl WanTarget {
    fn parse(body: &serde_json::Value) -> Option<Self> {
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

/// How a link ended.
#[derive(Debug, PartialEq, Eq)]
enum LinkEnd {
    /// It was up and closed.
    Closed,
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
            LinkEnd::Closed => {
                set_link(&dir, &device_id, "away: the link closed");
                backoff = BACKOFF_FIRST;
                backoff
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
    set_link(dir, &computer.device_id, "linked");
    let (outgoing_tx, mut outgoing) = mpsc::channel(32);
    let _live = LiveLink::new(&computer.device_id, outgoing_tx);
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    loop {
        tokio::select! {
            packet = read_packet(&mut reader) => match packet {
                Ok(Some(packet)) => take(dir, &computer.device_id, &packet).await,
                Ok(None) | Err(_) => return LinkEnd::Closed,
            },
            Some(packet) = outgoing.recv() => {
                if send(&mut writer, &packet).await.is_err() {
                    return LinkEnd::Closed;
                }
            }
        }
    }
}

/// What this computer takes from another on the link: its endpoint, kept
/// current, and its clipboard while it is shared (2g). The rest it pushes
/// to any client (the HUD, wake info, control) is for phones.
async fn take(dir: &Path, device_id: &str, packet: &NetworkPacket) {
    if packet.packet_type == "kdeconnect.clipboard" {
        let Some(content) = packet.body.get("content").and_then(|v| v.as_str()) else {
            return;
        };
        if !crate::clipboard_push::with_computers() {
            return;
        }
        // Not sent back to the computer it came from.
        crate::clipboard_push::note_from_phone(device_id, content);
        if let Err(e) = crate::plugins::clipboard::set_clipboard(content).await {
            tracing::warn!("Another computer's clipboard was not set: {e}");
        }
        return;
    }
    if packet.packet_type == crate::iroh_endpoint::ENDPOINT_PACKET
        && WanTarget::parse(&packet.body).is_some()
    {
        let body = packet.body.clone();
        let _ = computers::update(dir, |file| {
            if let Some(c) = file.computers.iter_mut().find(|c| c.device_id == device_id) {
                c.endpoint = Some(body);
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
fn parse_fingerprint(hex: &str) -> Option<[u8; 32]> {
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
        pair_on(ours, "123456", Some("e".repeat(64))).await.unwrap();
        other.await.unwrap();
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
