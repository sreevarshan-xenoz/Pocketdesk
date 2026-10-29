//! Pair a computer nearby (`pocketdesk_core::local::nearby`): this
//! computer's ask of another found on the network ([`ask`]), and the other
//! side, a nearby computer's ask of this one ([`handle`]), answered by the
//! owner in the app ([`answer`]). Accepted, the two pair as from a pasted
//! link: the asking computer pairs with the PIN this one hands it, which
//! pairs only the certificate whose code the owner matched
//! ([`take_pin`]).
//!
//! An ask grants nothing by itself. The asking computer is told its nonce
//! and, once accepted, what pairing needs; refused, "busy" or "unavailable"
//! (asks off here and too many from its address read alike).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers;
use pocketdesk_core::local::nearby::{
    self, ANSWER_WITHIN, Answered, AskState, HostDesk, NearbyAsk, NearbyRequest, STEP, step,
};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::pin as local_pin;
use pocketdesk_core::protocol::connection::ConnectionManager;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};
use pocketdesk_core::streaming::PeerKey;
use pocketdesk_core::streaming::transport::{CertManager, peer_cert_fingerprint};
use serde_json::json;
use tokio::io::BufReader;

/// Set, a nearby computer's ask shows no desktop notification (the live
/// tests' seam: the app's prompt is still written).
const QUIET: &str = "POCKETDESK_NEARBY_QUIET";

/// How long a PIN handed to an accepted computer pairs it.
const PIN_FOR: Duration = Duration::from_secs(120);
/// How long the asking side waits for each of the other's first answers.
const FIRST_ANSWER: Duration = Duration::from_secs(10);
/// How long it waits for the owner there, with room for the trip.
const OWNER_ANSWER: Duration = Duration::from_secs(ANSWER_WITHIN.as_secs() + 15);

/// Whether `packet` is a step of a nearby pairing.
pub(crate) fn is_step(packet: &NetworkPacket) -> bool {
    packet.packet_type == nearby::PACKET && packet.body.get(STEP).is_some()
}

fn step_packet(step: &str) -> NetworkPacket {
    NetworkPacket::new(nearby::PACKET).with_body(json!({ STEP: step }))
}

// ---- This computer, asked by a nearby one ----

/// One ask on this computer's screen, or the owner's turn on it.
static DESK: LazyLock<Mutex<HostDesk>> = LazyLock::new(Default::default);

/// An ask whose nonce was sent, waiting for the asking computer's reveal,
/// by its connection.
struct Progress {
    id: String,
    commit: [u8; 32],
    nonce: [u8; 32],
    name: String,
    key: [u8; 32],
    since: Instant,
}

static PROGRESS: LazyLock<Mutex<HashMap<String, Progress>>> = LazyLock::new(Default::default);

/// The owner's answer, by ask: accepted, and both are theirs (PC1).
type Answer = (bool, bool);

static ANSWERS: LazyLock<Mutex<HashMap<String, tokio::sync::oneshot::Sender<Answer>>>> =
    LazyLock::new(Default::default);

/// A PIN handed to an accepted computer: for its id and the certificate
/// whose code was matched, once, for [`PIN_FOR`].
struct NearbyPin {
    pin: String,
    mine: bool,
    device_id: String,
    key: [u8; 32],
    minted: Instant,
    minted_unix: u64,
}

static PINS: LazyLock<Mutex<Vec<NearbyPin>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A step of a nearby computer's ask, on its connection. Only on the home
/// network's TLS road, from a connection that proved a certificate.
pub(crate) async fn handle(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(PeerKey::Tls(key)) = sender.peer_key() else {
        return;
    };
    let Some(from) = sender.peer_addr() else {
        return;
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let connection = sender.connection_id().to_string();
    match packet.body.get(STEP).and_then(|v| v.as_str()) {
        Some(step::ASK) => {
            let Some(commit) = packet
                .body
                .get("commit")
                .and_then(|v| v.as_str())
                .and_then(nearby::from_hex)
            else {
                return;
            };
            let name = crate::peer_computers::shown_name(
                packet
                    .body
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(""),
            );
            let enabled = crate::events::current_config().nearby_pairing;
            let now = Instant::now();
            let taken = lock(&DESK).ask(from.ip(), enabled, now);
            let id = match taken {
                Ok(id) => id,
                Err(refusal) => {
                    tracing::info!("A nearby computer's ask to pair was turned away: {refusal:?}");
                    let _ = sender.send_packet(&step_packet(refusal.step())).await;
                    return;
                }
            };
            let nonce = nearby::nonce();
            {
                let mut progress = lock(&PROGRESS);
                progress.retain(|_, p| now.saturating_duration_since(p.since) < ANSWER_WITHIN);
                progress.insert(
                    connection,
                    Progress {
                        id,
                        commit,
                        nonce,
                        name,
                        key,
                        since: now,
                    },
                );
            }
            let mut answer = step_packet(step::NONCE);
            answer.body["nonce"] = nearby::to_hex(&nonce).into();
            let _ = sender.send_packet(&answer).await;
        }
        Some(step::REVEAL) => {
            let Some(progress) = lock(&PROGRESS).remove(&connection) else {
                return;
            };
            let revealed = packet
                .body
                .get("nonce")
                .and_then(|v| v.as_str())
                .and_then(nearby::from_hex)
                .filter(|n| nearby::commitment(n) == progress.commit && key == progress.key);
            let (Some(theirs), Ok(own)) = (
                revealed,
                pocketdesk_core::local::identity::cert_fingerprint(&dir),
            ) else {
                lock(&DESK).drop_ask(&progress.id);
                tracing::warn!("A nearby computer's ask broke off: its nonce did not match");
                return;
            };
            let code = nearby::matching_code(&key, &own, &theirs, &progress.nonce);
            let accepted = shown_and_answered(&dir, &progress, sender, from.ip(), &code).await;
            match accepted {
                Some(mine) => {
                    let pin = local_pin::mint_value();
                    lock(&PINS).push(NearbyPin {
                        pin: pin.clone(),
                        mine,
                        device_id: sender.device_id().to_string(),
                        key,
                        minted: Instant::now(),
                        minted_unix: local_pin::now_unix(),
                    });
                    let facts = crate::peer_computers::own_facts(&dir).await;
                    let me = crate::kde::host_identity();
                    let mut answer = step_packet(step::ACCEPTED);
                    answer.body["pin"] = pin.into();
                    answer.body["deviceId"] = me.device_id.into();
                    answer.body["name"] = me.device_name.into();
                    answer.body["mine"] = mine.into();
                    answer.body["addresses"] = json!(facts.addresses);
                    answer.body["controlPort"] = facts.control_port.into();
                    answer.body["streamPort"] = facts.stream_port.into();
                    if let Some(endpoint) = facts.endpoint {
                        answer.body["endpoint"] = endpoint;
                    }
                    tracing::info!("Accepted a nearby computer's ask to pair");
                    let _ = sender.send_packet(&answer).await;
                }
                None => {
                    let _ = sender.send_packet(&step_packet(step::DECLINED)).await;
                }
            }
        }
        _ => {}
    }
}

/// Show the ask (the app's prompt, and a notification since its window
/// may be hidden) and wait for the owner: `Some(mine)` when accepted.
async fn shown_and_answered(
    dir: &Path,
    progress: &Progress,
    sender: &dyn DeviceSender,
    from: std::net::IpAddr,
    code: &str,
) -> Option<bool> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    lock(&ANSWERS).insert(progress.id.clone(), tx);
    let request = NearbyRequest {
        id: progress.id.clone(),
        device_id: sender.device_id().to_string(),
        name: progress.name.clone(),
        address: from.to_string(),
        code: code.to_string(),
        unix: local_pin::now_unix(),
    };
    let shown = computers::update(dir, |file| file.nearby_in = Some(request));
    if let Err(e) = shown {
        tracing::warn!("Could not show a nearby computer's ask: {e}");
    }
    let summary = format!("{} wants to pair", progress.name);
    let body = format!(
        "Code {}. Accept in Pocketdesk if the other screen shows the same.",
        nearby::spaced(code)
    );
    // Tests with real servers on the owner's computer show none (a toast
    // may sound there).
    if std::env::var_os(QUIET).is_none() {
        std::thread::spawn(move || {
            crate::platform::notify(&summary, &body, ANSWER_WITHIN);
        });
    }
    let answer = tokio::time::timeout(ANSWER_WITHIN, rx).await;
    lock(&ANSWERS).remove(&progress.id);
    lock(&DESK).drop_ask(&progress.id);
    let id = progress.id.clone();
    let cleared = computers::update(dir, |file| {
        if file.nearby_in.as_ref().is_some_and(|r| r.id == id) {
            file.nearby_in = None;
        }
    });
    if let Err(e) = cleared {
        tracing::debug!("Could not clear a nearby computer's ask: {e}");
    }
    match answer {
        Ok(Ok((true, mine))) => Some(mine),
        _ => None,
    }
}

/// The owner's answer to the ask `id` (the app's Accept or Decline).
pub(crate) fn answer(dir: &Path, id: &str, accept: bool, mine: bool) {
    let answered = lock(&DESK).answer(id, accept, Instant::now());
    if answered != Answered::Stale
        && let Some(tx) = lock(&ANSWERS).remove(id)
    {
        let _ = tx.send((accept, mine));
    }
    let _ = computers::update(dir, |file| {
        if file.nearby_in.as_ref().is_some_and(|r| r.id == id) {
            file.nearby_in = None;
        }
    });
}

/// The PIN `pin`, if this computer handed it to `device_id` on accepting
/// its nearby ask, on a connection that proved the certificate whose code
/// was matched (`key`), within [`PIN_FOR`]: spent, as a PIN file the owner
/// minted would be, and read as one (both ways when the owner said both are
/// theirs, PC1).
pub(crate) fn take_pin(
    device_id: &str,
    key: Option<&PeerKey>,
    pin: &str,
) -> Option<local_pin::FilePin> {
    let Some(PeerKey::Tls(key)) = key else {
        return None;
    };
    let mut pins = lock(&PINS);
    pins.retain(|p| p.minted.elapsed() < PIN_FOR);
    let at = pins
        .iter()
        .position(|p| p.pin == pin && p.device_id == device_id && p.key == *key)?;
    let taken = pins.remove(at);
    Some(local_pin::FilePin {
        pin: taken.pin,
        minted_unix: taken.minted_unix,
        grant_secs: None,
        someone_else: false,
        profile: None,
        mine: taken.mine,
    })
}

// ---- This computer, asking a nearby one ----

/// What the app asked: the computer as found, and whether the owner here
/// said both are theirs.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub request: String,
    pub device_id: String,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub mine: bool,
}

/// Say how this computer's ask stands, for the app.
fn set_out(dir: &Path, asked: &Asked, state: AskState, code: Option<&str>) {
    let ask = NearbyAsk {
        request: asked.request.clone(),
        device_id: asked.device_id.clone(),
        name: crate::peer_computers::shown_name(&asked.name),
        code: code.map(str::to_string),
        state,
        unix: local_pin::now_unix(),
    };
    if let Err(e) = computers::update(dir, |file| file.nearby_out = Some(ask)) {
        tracing::debug!("Could not record a nearby ask: {e}");
    }
}

/// Ask the nearby computer to pair, with the code on both screens, until
/// its owner answers or `cancel` turns true; on Accept, the pairing link
/// its answer makes (its certificate the one this connection proved), for
/// the server to pair with as a pasted one.
pub(crate) async fn ask(
    dir: PathBuf,
    certs: Arc<CertManager>,
    asked: Asked,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Option<String> {
    set_out(&dir, &asked, AskState::Asking, None);
    let outcome = tokio::select! {
        outcome = ask_on(&dir, &certs, &asked) => outcome,
        _ = cancel.wait_for(|c| *c) => Err(AskState::Cancelled),
    };
    match outcome {
        Ok((link, code)) => {
            set_out(&dir, &asked, AskState::Accepted, Some(&code));
            Some(link)
        }
        Err(state) => {
            tracing::info!("Asking a nearby computer to pair ended: {state:?}");
            // The code stays shown with how it ended.
            let code = computers::read(&dir)
                .nearby_out
                .filter(|o| o.request == asked.request)
                .and_then(|o| o.code);
            set_out(&dir, &asked, state, code.as_deref());
            None
        }
    }
}

async fn ask_on(
    dir: &Path,
    certs: &CertManager,
    asked: &Asked,
) -> Result<(String, String), AskState> {
    let failed = |why: String| AskState::Failed(why);
    let me = crate::kde::host_identity();
    let manager = ConnectionManager::new(
        Duration::from_secs(8),
        certs.control_client_config_unpinned(),
    );
    let stream = manager
        .connect(&asked.address, asked.port, &me)
        .await
        .map_err(|e| failed(format!("it did not answer ({}: {e})", asked.address)))?;
    let theirs = peer_cert_fingerprint(stream.get_ref().1.peer_certificates())
        .ok_or_else(|| failed("it proved no certificate".to_string()))?;
    let ours = certs.cert_fingerprint();
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    let own = nearby::nonce();
    let mut ask = step_packet(step::ASK);
    ask.body["name"] = me.device_name.clone().into();
    ask.body["commit"] = nearby::to_hex(&nearby::commitment(&own)).into();
    crate::peer_computers::send(&mut writer, &ask)
        .await
        .map_err(failed)?;
    let first = next_step(&mut reader, FIRST_ANSWER).await?;
    let host_nonce = match first.body.get(STEP).and_then(|v| v.as_str()) {
        Some(step::NONCE) => first
            .body
            .get("nonce")
            .and_then(|v| v.as_str())
            .and_then(nearby::from_hex)
            .ok_or_else(|| failed("its answer did not read".to_string()))?,
        Some(step::BUSY) => return Err(AskState::Busy),
        _ => return Err(AskState::Unavailable),
    };
    let mut reveal = step_packet(step::REVEAL);
    reveal.body["nonce"] = nearby::to_hex(&own).into();
    crate::peer_computers::send(&mut writer, &reveal)
        .await
        .map_err(failed)?;
    let code = nearby::matching_code(&ours, &theirs, &own, &host_nonce);
    set_out(dir, asked, AskState::Waiting, Some(&code));
    let answer = next_step(&mut reader, OWNER_ANSWER).await?;
    if answer.body.get(STEP).and_then(|v| v.as_str()) != Some(step::ACCEPTED) {
        return Err(AskState::Declined);
    }
    let link = link_from(&answer.body, asked, &theirs).map_err(failed)?;
    Ok((link.to_uri(), code))
}

/// The other computer's next step, within `within`.
async fn next_step<R>(
    reader: &mut BufReader<R>,
    within: Duration,
) -> Result<NetworkPacket, AskState>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let read = tokio::time::timeout(within, async {
        loop {
            match crate::peer_computers::read_packet(reader).await? {
                Some(packet) if is_step(&packet) => return Ok(packet),
                // What it pushes to any client meanwhile.
                Some(_) => continue,
                None => return Err("it closed the connection".to_string()),
            }
        }
    })
    .await;
    match read {
        Ok(Ok(packet)) => Ok(packet),
        Ok(Err(why)) => Err(AskState::Failed(why)),
        Err(_) => Err(AskState::Failed("it did not answer in time".to_string())),
    }
}

/// The pairing link an accepted ask's answer makes: the computer as found
/// (the id must be the one announced), its certificate the one this
/// connection proved (`theirs`), the address it was reached at first.
fn link_from(
    body: &serde_json::Value,
    asked: &Asked,
    theirs: &[u8; 32],
) -> Result<PairLink, String> {
    let text = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or("");
    if text("deviceId") != asked.device_id {
        return Err("another computer answered than the one found".to_string());
    }
    let pin = text("pin");
    if !local_pin::is_valid(pin) {
        return Err("its answer held no code".to_string());
    }
    let mut addresses = vec![asked.address.clone()];
    if let Some(more) = body.get("addresses").and_then(|v| v.as_array()) {
        for a in more.iter().filter_map(|v| v.as_str()).take(16) {
            if !addresses.iter().any(|have| have == a) {
                addresses.push(a.to_string());
            }
        }
    }
    let port = |key: &str, default: u16| {
        body.get(key)
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .unwrap_or(default)
    };
    Ok(PairLink {
        name: text("name").to_string(),
        device_id: asked.device_id.clone(),
        addresses,
        control_port: port("controlPort", asked.port),
        stream_port: port(
            "streamPort",
            pocketdesk_core::streaming::client::DEFAULT_STREAMING_PORT,
        ),
        pin: pin.to_string(),
        fingerprint: Some(nearby::to_hex(theirs)),
        mine: body.get("mine").and_then(|v| v.as_bool()) == Some(true),
        endpoint: body.get("endpoint").map(|e| e.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked() -> Asked {
        Asked {
            request: "r-1".to_string(),
            device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
            name: "Test Tower".to_string(),
            address: "192.0.2.30".to_string(),
            port: 1716,
            mine: false,
        }
    }

    /// An accepted answer makes the link the server pairs with: the
    /// certificate this connection proved, never one the answer names; the
    /// address reached first; its PIN; both ways only if it said so.
    #[test]
    fn an_accepted_answer_makes_the_link_for_the_proved_certificate() {
        let theirs = [0xab; 32];
        let body = json!({
            STEP: step::ACCEPTED,
            "pin": "012345",
            "deviceId": "00000000-0000-4000-8000-0000000000e1",
            "name": "Test Tower",
            "mine": true,
            "addresses": ["192.0.2.31", "192.0.2.30"],
            "controlPort": 1717,
            "streamPort": 4717,
            "fingerprint": "cd".repeat(32),
        });
        let link = link_from(&body, &asked(), &theirs).expect("a link");
        assert_eq!(link.fingerprint.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(link.addresses, ["192.0.2.30", "192.0.2.31"]);
        assert_eq!((link.control_port, link.stream_port), (1717, 4717));
        assert_eq!(link.pin, "012345");
        assert!(link.mine);
        let reread = PairLink::from_uri(&link.to_uri()).expect("it reads back");
        assert_eq!(reread.device_id, "00000000-0000-4000-8000-0000000000e1");
    }

    /// An answer from another id than the one found, or with no code, is
    /// no pairing.
    #[test]
    fn an_answer_from_another_computer_or_without_a_code_is_refused() {
        let mut body = json!({
            STEP: step::ACCEPTED,
            "pin": "012345",
            "deviceId": "00000000-0000-4000-8000-0000000000e2",
        });
        assert!(link_from(&body, &asked(), &[0; 32]).is_err());
        body["deviceId"] = "00000000-0000-4000-8000-0000000000e1".into();
        body["pin"] = "12".into();
        assert!(link_from(&body, &asked(), &[0; 32]).is_err());
    }

    /// A handed PIN pairs only the id and certificate it was handed to,
    /// once.
    #[test]
    fn a_handed_pin_pairs_only_its_computer_once() {
        let key = [0x5a; 32];
        lock(&PINS).push(NearbyPin {
            pin: "424242".to_string(),
            mine: true,
            device_id: "nearby-a".to_string(),
            key,
            minted: Instant::now(),
            minted_unix: 1,
        });
        assert!(take_pin("nearby-b", Some(&PeerKey::Tls(key)), "424242").is_none());
        assert!(take_pin("nearby-a", Some(&PeerKey::Tls([0; 32])), "424242").is_none());
        assert!(take_pin("nearby-a", None, "424242").is_none());
        let taken = take_pin("nearby-a", Some(&PeerKey::Tls(key)), "424242").expect("its PIN");
        assert!(taken.mine && taken.grant_secs.is_none() && !taken.someone_else);
        assert!(
            take_pin("nearby-a", Some(&PeerKey::Tls(key)), "424242").is_none(),
            "spent"
        );
    }
}
