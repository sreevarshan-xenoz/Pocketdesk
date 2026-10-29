//! Pair a computer nearby (`pocketdesk_core::local::nearby`): this
//! computer's ask of another found on the network ([`ask`]), and the other
//! side, a nearby computer's ask of this one ([`handle`]), answered by the
//! owner in the app ([`answer`]). Both owners say yes before anything
//! pairs: the asking computer's owner confirms the code on its screen (its
//! `confirmed`), this one's accepts on its own, and only then does this one
//! hand the asking computer the PIN, which pairs only the certificate whose
//! code was matched ([`take_pin`]). The asking computer pairs with it as
//! from a pasted link, and only once its own owner confirmed: an
//! `accepted` alone, which a computer in the middle could send, pairs
//! nothing there.
//!
//! An ask grants nothing by itself. The asking computer is told its nonce
//! and, once both said yes, what pairing needs; refused, "busy" or
//! "unavailable" (asks off here and too many from its address read alike).
//! This computer's side never holds the connection while its owner thinks:
//! each step is handled as it comes, so the connection's other packets (its
//! confirmation) and its close are seen meanwhile.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers;
use pocketdesk_core::local::nearby::{
    self, ANSWER_WITHIN, Answered, AskState, HostDesk, NearbyAsk, NearbyRequest, REVEAL_WITHIN,
    STEP, step,
};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::pin as local_pin;
use pocketdesk_core::protocol::connection::ConnectionManager;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};
use pocketdesk_core::streaming::PeerKey;
use pocketdesk_core::streaming::transport::{CertManager, peer_cert_fingerprint};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

/// Set, a nearby computer's ask shows no desktop notification (the live
/// tests' seam: the app's prompt is still written).
const QUIET: &str = "POCKETDESK_NEARBY_QUIET";

/// How long a PIN handed to an accepted computer pairs it.
const PIN_FOR: Duration = Duration::from_secs(120);
/// How long the asking side waits for each of the other's first answers.
const FIRST_ANSWER: Duration = Duration::from_secs(10);
/// How long it waits for both owners, with room for the trip.
const OWNER_ANSWER: Duration = Duration::from_secs(ANSWER_WITHIN.as_secs() + 15);

/// Whether `packet` is a step of a nearby pairing.
pub(crate) fn is_step(packet: &NetworkPacket) -> bool {
    packet.packet_type == nearby::PACKET && packet.body.get(STEP).is_some()
}

fn step_packet(step: &str) -> NetworkPacket {
    NetworkPacket::new(nearby::PACKET).with_body(json!({ STEP: step }))
}

fn step_of(packet: &NetworkPacket) -> Option<&str> {
    packet.body.get(STEP).and_then(|v| v.as_str())
}

// ---- This computer, asked by a nearby one ----

/// One ask on this computer's screen, and both owners' word on it.
static DESK: LazyLock<Mutex<HostDesk>> = LazyLock::new(Default::default);

/// A nearby computer's ask taken here: what its connection proved and
/// said, until both owners said yes or it ended.
#[derive(Clone)]
struct Taken {
    id: String,
    device_id: String,
    key: [u8; 32],
    name: String,
    commit: [u8; 32],
    nonce: [u8; 32],
    /// Its nonce was revealed and matched: the owner here sees it.
    revealed: bool,
}

/// The asks taken, by the connection each came on.
static TAKEN: LazyLock<Mutex<HashMap<String, Taken>>> = LazyLock::new(Default::default);

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

/// Whether `nonce` (hex, as revealed) is the one `commit` was made to, on
/// a connection that proved `key`, the certificate the ask came with
/// (`asked_with`): the nonce, when it is.
fn revealed_nonce(
    nonce: Option<&str>,
    commit: &[u8; 32],
    key: &[u8; 32],
    asked_with: &[u8; 32],
) -> Option<[u8; 32]> {
    nonce
        .and_then(nearby::from_hex)
        .filter(|n| nearby::commitment(n) == *commit && key == asked_with)
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
    match step_of(packet) {
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
            // Asks off: turned away before any other work.
            if !crate::events::current_config().nearby_pairing {
                let _ = sender
                    .send_packet(&step_packet(nearby::Refusal::Off.step()))
                    .await;
                return;
            }
            // The cheap refusals (busy, paused, too many) before any file
            // is read for an ask that would be turned away anyway.
            let (early, tried) = {
                let mut desk = lock(&DESK);
                let early = desk.refusal(from.ip(), true, Instant::now());
                (early, desk.take_tried())
            };
            if tried {
                note_tried(&dir);
            }
            if let Some(refusal) = early {
                tracing::info!("A nearby computer's ask to pair was turned away: {refusal:?}");
                let _ = sender.send_packet(&step_packet(refusal.step())).await;
                return;
            }
            // An id paired here either way (a phone or a computer), or
            // bound to another key, never asks: the owner's Accept would
            // hand that id to whoever asked. Checked again as it pairs.
            let taken_id = {
                let (dir, id) = (dir.clone(), sender.device_id().to_string());
                let key = PeerKey::Tls(key);
                tokio::task::spawn_blocking(move || {
                    crate::plugins::pair::nearby_id_taken_in(&dir, &id, Some(&key))
                })
                .await
                .unwrap_or_else(|_| Some("the check did not run".to_string()))
            };
            if let Some(why) = &taken_id {
                tracing::info!("A nearby computer's ask to pair was turned away: {why}");
            }
            let enabled = crate::events::current_config().nearby_pairing && taken_id.is_none();
            let now = Instant::now();
            let (taken, tried) = {
                let mut desk = lock(&DESK);
                let taken = desk.ask(from.ip(), enabled, now);
                (taken, desk.take_tried())
            };
            if tried {
                note_tried(&dir);
            }
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
                let mut taken = lock(&TAKEN);
                // One ask at a time: any other entry is over.
                taken.clear();
                taken.insert(
                    connection,
                    Taken {
                        id,
                        device_id: sender.device_id().to_string(),
                        key,
                        name,
                        commit,
                        nonce,
                        revealed: false,
                    },
                );
            }
            // Not revealed in time: a failed ask, the screen free again.
            sweep_after(dir, REVEAL_WITHIN);
            let mut answer = step_packet(step::NONCE);
            answer.body["nonce"] = nearby::to_hex(&nonce).into();
            let _ = sender.send_packet(&answer).await;
        }
        Some(step::REVEAL) => {
            let Some(taken) = lock(&TAKEN)
                .get(&connection)
                .filter(|t| !t.revealed)
                .cloned()
            else {
                return;
            };
            let revealed = revealed_nonce(
                packet.body.get("nonce").and_then(|v| v.as_str()),
                &taken.commit,
                &key,
                &taken.key,
            );
            let Some(theirs) = revealed else {
                // A wrong code tried: counted host-wide, and the owner told.
                lock(&TAKEN).remove(&connection);
                lock(&DESK).failed(&taken.id, Instant::now());
                tracing::warn!("A nearby computer's ask failed: its nonce did not match");
                note_tried(&dir);
                return;
            };
            let Ok(own) = pocketdesk_core::local::identity::cert_fingerprint(&dir) else {
                lock(&TAKEN).remove(&connection);
                lock(&DESK).drop_ask(&taken.id);
                tracing::warn!(
                    "A nearby computer's ask ended: this computer's certificate did not read"
                );
                return;
            };
            if !lock(&DESK).prompted(&taken.id, Instant::now()) {
                // Ran out meanwhile.
                lock(&TAKEN).remove(&connection);
                return;
            }
            if let Some(t) = lock(&TAKEN).get_mut(&connection) {
                t.revealed = true;
            }
            let code = nearby::matching_code(&key, &own, &theirs, &taken.nonce);
            show(&dir, &taken, from.ip(), &code);
            sweep_after(dir, ANSWER_WITHIN);
        }
        Some(step::CONFIRMED) => {
            let Some(id) = lock(&TAKEN)
                .get(&connection)
                .filter(|t| t.revealed)
                .map(|t| t.id.clone())
            else {
                return;
            };
            if lock(&DESK).confirmed(&id, Instant::now()) {
                settle(&dir, &id).await;
            }
        }
        _ => {}
    }
}

/// Show the ask: the app's prompt, and a notification since its window may
/// be hidden.
fn show(dir: &Path, taken: &Taken, from: std::net::IpAddr, code: &str) {
    let request = NearbyRequest {
        id: taken.id.clone(),
        device_id: taken.device_id.clone(),
        name: taken.name.clone(),
        address: from.to_string(),
        code: code.to_string(),
        unix: local_pin::now_unix(),
    };
    if let Err(e) = computers::update(dir, |file| file.nearby_in = Some(request)) {
        tracing::warn!("Could not show a nearby computer's ask: {e}");
    }
    let summary = format!("{} wants to pair", taken.name);
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
}

/// Take the ask `id`'s prompt off the screen, if it is still shown.
fn clear_prompt(dir: &Path, id: &str) {
    let cleared = computers::update(dir, |file| {
        if file.nearby_in.as_ref().is_some_and(|r| r.id == id) {
            file.nearby_in = None;
        }
    });
    if let Err(e) = cleared {
        tracing::debug!("Could not clear a nearby computer's ask: {e}");
    }
}

/// The ask `id`'s entry, taken out: its connection and what it was.
fn take_out(id: &str) -> Option<(String, Taken)> {
    let mut taken = lock(&TAKEN);
    let connection = taken.iter().find(|(_, t)| t.id == id)?.0.clone();
    taken.remove(&connection).map(|t| (connection, t))
}

/// Send `packet` on the connection `connection_id`, if it is still open.
async fn send_to(connection_id: &str, packet: &NetworkPacket) {
    let target = crate::state::ACTIVE_CLIENTS
        .lock()
        .await
        .iter()
        .find(|c| c.connection_id() == connection_id)
        .cloned();
    if let Some(target) = target {
        let _ = target.send_packet(packet).await;
    }
}

/// Once both owners said yes to the ask `id`: the PIN, for its id and the
/// certificate whose code was matched, and what pairing needs, to its
/// connection. Nothing before.
async fn settle(dir: &Path, id: &str) {
    let Some(mine) = lock(&DESK).both(id) else {
        return;
    };
    clear_prompt(dir, id);
    let Some((connection, taken)) = take_out(id) else {
        return;
    };
    let pin = local_pin::mint_value();
    lock(&PINS).push(NearbyPin {
        pin: pin.clone(),
        mine,
        device_id: taken.device_id,
        key: taken.key,
        minted: Instant::now(),
        minted_unix: local_pin::now_unix(),
    });
    let facts = crate::peer_computers::own_facts(dir).await;
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
    tracing::info!("Both owners said yes to a nearby computer's ask to pair");
    send_to(&connection, &answer).await;
}

/// The owner's answer to the ask `id` (the app's Accept or Decline).
pub(crate) async fn answer(dir: &Path, id: &str, accept: bool, mine: bool) {
    let answered = lock(&DESK).answer(id, accept, mine, Instant::now());
    clear_prompt(dir, id);
    match answered {
        Answered::Accepted => settle(dir, id).await,
        Answered::Declined => {
            if let Some((connection, _)) = take_out(id) {
                send_to(&connection, &step_packet(step::DECLINED)).await;
            }
        }
        Answered::Stale => {}
    }
}

/// Look again in `after`: an ask that ran out by then is declined, its
/// prompt taken down.
fn sweep_after(dir: PathBuf, after: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(after + Duration::from_millis(250)).await;
        sweep(&dir).await;
    });
}

/// Drop what ran out: the ask on screen, if its time is up, and every
/// entry not on screen any more.
async fn sweep(dir: &Path) {
    let (shown, tried) = {
        let mut desk = lock(&DESK);
        desk.expire(Instant::now());
        (desk.shown().map(str::to_string), desk.take_tried())
    };
    if tried {
        note_tried(dir);
    }
    let gone: Vec<(String, Taken)> = {
        let mut taken = lock(&TAKEN);
        let gone: Vec<String> = taken
            .iter()
            .filter(|(_, t)| shown.as_deref() != Some(t.id.as_str()))
            .map(|(c, _)| c.clone())
            .collect();
        gone.into_iter()
            .filter_map(|c| taken.remove(&c).map(|t| (c, t)))
            .collect()
    };
    for (connection, taken) in gone {
        if taken.revealed {
            clear_prompt(dir, &taken.id);
            send_to(&connection, &step_packet(step::DECLINED)).await;
        }
    }
}

/// The connection `connection_id` closed (the server's cleanup of every
/// connection): its ask, if any, is over. Before its nonce was revealed,
/// that is a failed ask (a computer in the middle that found the code
/// would not match leaves this way); once shown, its owner may simply
/// have cancelled, and the prompt goes.
pub(crate) fn connection_closed(connection_id: &str) {
    let Some(taken) = lock(&TAKEN).remove(connection_id) else {
        return;
    };
    let failed = {
        let mut desk = lock(&DESK);
        let failed = !taken.revealed && desk.failed(&taken.id, Instant::now());
        desk.drop_ask(&taken.id);
        failed
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    if failed {
        tracing::warn!("A nearby computer's ask failed: it left before revealing its nonce");
        note_tried(&dir);
    } else {
        clear_prompt(&dir, &taken.id);
    }
}

/// Tell the owner someone nearby tried a wrong pairing code: noted in
/// the computers file for the app, and as a notification.
fn note_tried(dir: &Path) {
    let now = local_pin::now_unix();
    if let Err(e) = computers::update(dir, |file| file.nearby_tried = Some(now)) {
        tracing::debug!("Could not note a failed nearby ask: {e}");
    }
    let paused = lock(&DESK).paused(Instant::now());
    if std::env::var_os(QUIET).is_none() {
        let body = if paused {
            "Nearby asks to pair are paused for 10 minutes."
        } else {
            "Nothing was paired."
        };
        std::thread::spawn(move || {
            crate::platform::notify(TRIED, body, ANSWER_WITHIN);
        });
    }
}

/// What the owner is told of a failed ask.
const TRIED: &str = "Someone nearby tried a wrong pairing code";

/// The PIN `pin`, if this computer handed it to `device_id` once both
/// owners said yes to its nearby ask, on a connection that proved the
/// certificate whose code was matched (`key`), within [`PIN_FOR`]: spent,
/// as a PIN file the owner minted would be, and read as one (both ways
/// when the owner said both are theirs, PC1).
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

/// The owner here, on this computer's ask: not yet, confirmed the code,
/// or cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Undecided,
    Confirmed,
    Cancelled,
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
/// both owners said yes or the owner here cancels (`owner`); once both
/// did, the pairing link its answer makes (its certificate the one this
/// connection proved), for the server to pair with as a pasted one.
pub(crate) async fn ask(
    dir: PathBuf,
    certs: Arc<CertManager>,
    asked: Asked,
    owner: tokio::sync::watch::Receiver<Owner>,
) -> Option<String> {
    set_out(&dir, &asked, AskState::Asking, None);
    let mut cancel = owner.clone();
    // Cancelled, the connection drops, and the other computer takes its
    // prompt down.
    let outcome = tokio::select! {
        outcome = ask_on(&dir, &certs, &asked, owner) => outcome,
        _ = cancel.wait_for(|o| *o == Owner::Cancelled) => Err(AskState::Cancelled),
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
    owner: tokio::sync::watch::Receiver<Owner>,
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
    let sides = Sides {
        ours: certs.cert_fingerprint(),
        theirs,
        name: me.device_name.clone(),
    };
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    converse(
        &mut reader,
        &mut writer,
        &sides,
        asked,
        owner,
        |state, code| set_out(dir, asked, state, Some(code)),
    )
    .await
}

/// What this computer's side of the conversation knows: both certificates
/// (its own, and the one the connection proved) and its own name.
struct Sides {
    ours: [u8; 32],
    theirs: [u8; 32],
    name: String,
}

/// The asking side's steps, once connected: ask, read the other's nonce,
/// reveal, show the code (`report`), and wait for both owners. The owner
/// here must confirm the code (`owner`) before anything that completes the
/// pairing is sent or believed: the other's `accepted` before that is no
/// pairing (an honest computer sends it only after the confirmation; one
/// in the middle would send it at once).
async fn converse<R, W>(
    reader: &mut BufReader<R>,
    writer: &mut W,
    sides: &Sides,
    asked: &Asked,
    mut owner: tokio::sync::watch::Receiver<Owner>,
    mut report: impl FnMut(AskState, &str),
) -> Result<(String, String), AskState>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let failed = |why: String| AskState::Failed(why);
    let own = nearby::nonce();
    let mut ask = step_packet(step::ASK);
    ask.body["name"] = sides.name.clone().into();
    ask.body["commit"] = nearby::to_hex(&nearby::commitment(&own)).into();
    crate::peer_computers::send(writer, &ask)
        .await
        .map_err(failed)?;
    let first = next_step(reader, FIRST_ANSWER).await?;
    let host_nonce = match step_of(&first) {
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
    crate::peer_computers::send(writer, &reveal)
        .await
        .map_err(failed)?;
    let code = nearby::matching_code(&sides.ours, &sides.theirs, &own, &host_nonce);
    report(AskState::Confirm, &code);
    let answer = next_step(reader, OWNER_ANSWER);
    tokio::pin!(answer);
    tokio::select! {
        early = &mut answer => {
            // It answered before the owner here confirmed: a decline ends
            // the ask, and anything else is no pairing.
            return Err(match step_of(&early?) {
                Some(step::DECLINED) => AskState::Declined,
                _ => failed("it answered before the code was confirmed here".to_string()),
            });
        }
        confirmed = owner.wait_for(|o| *o == Owner::Confirmed) => {
            if confirmed.is_err() {
                return Err(AskState::Cancelled);
            }
        }
    }
    crate::peer_computers::send(writer, &step_packet(step::CONFIRMED))
        .await
        .map_err(failed)?;
    report(AskState::Waiting, &code);
    let answer = answer.await?;
    if step_of(&answer) != Some(step::ACCEPTED) {
        return Err(AskState::Declined);
    }
    let link = link_from(&answer.body, asked, &sides.theirs).map_err(failed)?;
    Ok((link.to_uri(), code))
}

/// The other computer's next step, within `within`.
async fn next_step<R>(
    reader: &mut BufReader<R>,
    within: Duration,
) -> Result<NetworkPacket, AskState>
where
    R: AsyncRead + Unpin,
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
/// connection proved (`theirs`), at the address it was reached at.
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
    // Only the address dialled, where the certificate whose code was matched
    // was seen: the answer's own list is the other side's word, and pinning
    // it would let it re-pin any address (PR #556 review I2).
    let addresses = vec![asked.address.clone()];
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
    /// certificate this connection proved, never one the answer names; only
    /// the address dialled, never those the answer lists; its PIN; both
    /// ways only if it said so.
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
        assert_eq!(link.addresses, ["192.0.2.30"]);
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

    /// A revealed nonce counts only when it is the one committed to, on
    /// the connection that asked: another nonce, one that does not read,
    /// none, or another certificate is a bad reveal.
    #[test]
    fn a_bad_reveal_is_refused() {
        let nonce = [0x42; 32];
        let commit = nearby::commitment(&nonce);
        let key = [0x5a; 32];
        let hex = nearby::to_hex(&nonce);
        assert_eq!(revealed_nonce(Some(&hex), &commit, &key, &key), Some(nonce));
        let other = nearby::to_hex(&[0x43; 32]);
        assert_eq!(revealed_nonce(Some(&other), &commit, &key, &key), None);
        assert_eq!(revealed_nonce(Some("zz"), &commit, &key, &key), None);
        assert_eq!(revealed_nonce(None, &commit, &key, &key), None);
        assert_eq!(
            revealed_nonce(Some(&hex), &commit, &key, &[0x5b; 32]),
            None,
            "another certificate"
        );
    }

    // ---- The asking side against a scripted other computer ----

    const THEM: &str = "00000000-0000-4000-8000-0000000000e1";

    fn sides() -> Sides {
        Sides {
            ours: [0xa1; 32],
            theirs: [0xb2; 32],
            name: "Asking Desk".to_string(),
        }
    }

    fn accepted() -> NetworkPacket {
        let mut packet = step_packet(step::ACCEPTED);
        packet.body["pin"] = "012345".into();
        packet.body["deviceId"] = THEM.into();
        packet.body["name"] = "Test Tower".into();
        packet
    }

    /// The other side as a script: it takes the ask, answers with its
    /// nonce, takes the reveal, then runs `then` on the connection; every
    /// step it read after the reveal is returned.
    async fn other_side<F, Fut>(stream: tokio::io::DuplexStream, then: F) -> Vec<String>
    where
        F: FnOnce(
            BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
            tokio::io::WriteHalf<tokio::io::DuplexStream>,
        ) -> Fut,
        Fut: std::future::Future<
                Output = (
                    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
                    Vec<String>,
                ),
            >,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let ask = crate::peer_computers::read_packet(&mut reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(step_of(&ask), Some(step::ASK));
        let mut nonce = step_packet(step::NONCE);
        nonce.body["nonce"] = nearby::to_hex(&[0x22; 32]).into();
        crate::peer_computers::send(&mut writer, &nonce)
            .await
            .unwrap();
        let reveal = crate::peer_computers::read_packet(&mut reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(step_of(&reveal), Some(step::REVEAL));
        let (mut reader, mut read) = then(reader, writer).await;
        // Whatever else the asking side sent, to the end.
        while let Ok(Some(packet)) = crate::peer_computers::read_packet(&mut reader).await {
            read.push(step_of(&packet).unwrap_or("").to_string());
        }
        read
    }

    /// A computer in the middle (or any other) answers `accepted` the
    /// moment the code shows, with a valid PIN and the announced id: the
    /// asking side, whose owner has not confirmed the code, does not pair,
    /// and sends nothing that would complete the pairing.
    #[tokio::test]
    async fn an_accepted_before_the_owner_here_confirmed_pairs_nothing() {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let script = tokio::spawn(other_side(theirs, |reader, mut writer| async move {
            crate::peer_computers::send(&mut writer, &accepted())
                .await
                .unwrap();
            drop(writer);
            (reader, Vec::new())
        }));
        let (owner_tx, owner) = tokio::sync::watch::channel(Owner::Undecided);
        let mut reported = Vec::new();
        let (reader, mut writer) = tokio::io::split(ours);
        let mut reader = BufReader::new(reader);
        let outcome = converse(
            &mut reader,
            &mut writer,
            &sides(),
            &asked(),
            owner,
            |state, _| reported.push(state),
        )
        .await;
        assert!(
            matches!(&outcome, Err(AskState::Failed(why)) if why.contains("before the code was confirmed")),
            "{outcome:?}"
        );
        assert_eq!(reported, [AskState::Confirm]);
        drop((reader, writer, owner_tx));
        let read = script.await.unwrap();
        assert!(
            !read.iter().any(|s| s == step::CONFIRMED),
            "nothing confirmed: {read:?}"
        );
    }

    /// The honest order: the code shows, the owner here confirms, the
    /// other side reads the confirmation and only then hands its PIN; the
    /// link is for the certificate this connection proved.
    #[tokio::test]
    async fn both_owners_confirm_then_it_pairs() {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let script = tokio::spawn(other_side(theirs, |mut reader, mut writer| async move {
            let confirmed = crate::peer_computers::read_packet(&mut reader)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(step_of(&confirmed), Some(step::CONFIRMED));
            crate::peer_computers::send(&mut writer, &accepted())
                .await
                .unwrap();
            (reader, vec![step::CONFIRMED.to_string()])
        }));
        let (owner_tx, owner) = tokio::sync::watch::channel(Owner::Undecided);
        let mut reported = Vec::new();
        let (reader, mut writer) = tokio::io::split(ours);
        let mut reader = BufReader::new(reader);
        let outcome = converse(
            &mut reader,
            &mut writer,
            &sides(),
            &asked(),
            owner,
            |state, _| {
                if state == AskState::Confirm {
                    // The owner looks, and confirms.
                    let _ = owner_tx.send(Owner::Confirmed);
                }
                reported.push(state);
            },
        )
        .await;
        let (link, code) = outcome.expect("paired once both said yes");
        assert_eq!(code.len(), 6);
        let link = PairLink::from_uri(&link).unwrap();
        assert_eq!(link.fingerprint.as_deref(), Some("b2".repeat(32).as_str()));
        assert_eq!(reported, [AskState::Confirm, AskState::Waiting]);
        drop((reader, writer));
        assert_eq!(script.await.unwrap(), [step::CONFIRMED]);
    }

    /// The other's owner declines while the code shows here: declined,
    /// and nothing confirmed.
    #[tokio::test]
    async fn a_decline_before_the_owner_here_confirmed_ends_the_ask() {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let script = tokio::spawn(other_side(theirs, |reader, mut writer| async move {
            crate::peer_computers::send(&mut writer, &step_packet(step::DECLINED))
                .await
                .unwrap();
            drop(writer);
            (reader, Vec::new())
        }));
        let (_owner_tx, owner) = tokio::sync::watch::channel(Owner::Undecided);
        let (reader, mut writer) = tokio::io::split(ours);
        let mut reader = BufReader::new(reader);
        let outcome = converse(
            &mut reader,
            &mut writer,
            &sides(),
            &asked(),
            owner,
            |_, _| {},
        )
        .await;
        assert_eq!(outcome, Err(AskState::Declined));
        drop((reader, writer));
        assert!(script.await.unwrap().is_empty());
    }
}
