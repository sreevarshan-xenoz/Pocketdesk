//! One pairing for all the owner's devices (ecosystem phase 4): this
//! computer introduces the owner's phone to the owner's other computers,
//! and takes the phones they introduce.
//!
//! **As the introducing computer (A).** An owner phone (paired with this
//! computer's own code: remote revoke's `owners` mark, never someone
//! else's, a profile's or a guest's) asks which of this computer's
//! computers it could pair with too ([`INTRODUCE`] `list`): those paired
//! both ways (PC1) whose grant neither waits nor lapsed, with a link up
//! now. Its owner confirms one on the phone, and the phone asks for it
//! (`pair`). This computer asks that computer on the link between the two
//! ([`INTRODUCE_PHONE`]), naming the phone, the TLS (and iroh) key bound
//! to it here and its permissions here, and waits [`ANSWER_WAIT`] for the
//! answer. With a PIN in it, the phone is handed a pairing link to that
//! computer: the PIN, that computer's addresses and ports, and the
//! certificate and iroh endpoint this computer pinned for it.
//!
//! **As the computer the phone is introduced to (B).** An introduction is
//! taken only from a computer paired both ways, only with "Let my
//! computers add my phones here" (`computer_introductions`, on by default)
//! on, only for an id free here (not paired either way, not a listed
//! computer, not bound to another key), at most
//! [`INTRODUCTIONS_PER_WINDOW`] per introducing computer in ten minutes
//! and [`PINS_WAITING`] at once. Its answer is a PIN of its own, kept in
//! memory for [`PIN_FOR_SECS`], which pairs only that phone id on a
//! connection proving the TLS key the introducing computer named, once
//! ([`take_pin`]), discarded after [`PIN_MISSES`] wrong PINs from that
//! phone or on the introducing computer's cancel. As the phone pairs,
//! everything is decided again under the files' locks ([`admit_in`]): the
//! introducer is still paired both ways, the switch still on, the id still
//! free. The phone is then an owner phone here that the introducer
//! introduced (never introduced onward, and demoted when the introducer is
//! no longer paired both ways), with this computer's defaults for a new
//! phone narrowed to what the introducing computer allows it and to what
//! this computer allows the introducer ([`permissions_for`]), and its keys
//! as the introducing computer bound them. This computer says so ("<A>
//! added <phone> to this computer"), writes `device_introduced` and
//! confirms the pairing to the introducer, which keeps the introduction
//! only then.
//!
//! **Removal follows the introduction.** Each computer keeps which phones
//! were introduced between it and which computer (`computers.json`'s
//! `introduced`). A phone removed here however it went (Devices → Unpair,
//! `pocketdesk unpair`, a removal from afar) is owed to each such computer
//! as remote revoke's `remove` ([`owe_removals`], [`send_owed`]), sent on
//! the link when it is up and dropped once answered: that computer
//! removes it under its own "Let my devices remove others from afar".

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{
    self, ComputersFile, Introduced, OWED_FOR, OWED_RESEND, OwedRemoval, PENDING_INTRODUCTION_FOR,
    PeerComputer, PendingIntroduction,
};
use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
use pocketdesk_core::local::pair_link::{PairLink, order_addresses};
use pocketdesk_core::protocol::introductions::{
    self as wire, ComputerAnswer, INTRODUCE, INTRODUCE_ANSWER, INTRODUCE_PHONE,
    INTRODUCE_PHONE_ANSWER, INTRODUCTIONS_PER_WINDOW, IntroducedPhone, Introduction,
    LISTS_PER_MINUTE, Offer, PACE_WINDOW_SECS, PIN_FOR_SECS, PINS_WAITING, PhoneAsk, PhoneRequest,
    Refusal,
};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
use pocketdesk_core::protocol::remote_devices::{self as revoke, Ask};
use pocketdesk_core::streaming::PeerKey;
use serde_json::Value;

/// How long this computer waits for the other's answer to an
/// introduction.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_secs(10);

/// The most addresses a pairing link to the other computer names.
const MOST_ADDRESSES: usize = 16;

// ---- Who is who ----

/// Whether `device_id` is an owner phone here: paired for good with this
/// computer's own code, never a guest's grant nor a computer.
fn is_owner_phone(store: &TrustStore, file: &ComputersFile, device_id: &str) -> bool {
    !store.is_computer(device_id) && crate::remote_devices::is_owner_device(store, file, device_id)
}

/// Whether `device_id` is another of the owner's computers here: paired
/// both ways (PC1), its grant neither waiting for the link nor lapsed.
fn is_mine_computer(store: &TrustStore, file: &ComputersFile, device_id: &str) -> bool {
    store.is_computer(device_id) && crate::remote_devices::is_owner_device(store, file, device_id)
}

/// The name this computer shows `device_id` by: the owner's rename, else
/// a listed computer's name, else its own word, else the start of its id.
fn shown(store: &TrustStore, file: &ComputersFile, device_id: &str) -> String {
    store
        .chosen_name(device_id)
        .map(str::to_string)
        .or_else(|| {
            file.computers
                .iter()
                .find(|c| c.device_id == device_id)
                .and_then(|c| pocketdesk_core::protocol::kdeconnect::clean_device_name(&c.name))
        })
        .or_else(|| store.name(device_id).map(str::to_string))
        .unwrap_or_else(|| device_id.chars().take(8).collect())
}

/// Whether `computer` can be sent to now: the link this computer keeps to
/// it is up, or it is connected here (it keeps the link).
async fn reachable_now() -> impl Fn(&str) -> bool {
    let clients: Vec<String> = crate::state::clone_clients()
        .await
        .iter()
        .map(|c| c.device_id().to_string())
        .collect();
    move |id: &str| crate::peer_computers::is_linked(id) || clients.iter().any(|c| c == id)
}

// ---- Pacing ----

/// How many asks each asker made in the last window.
#[derive(Default)]
struct Pace {
    asks: HashMap<String, VecDeque<Instant>>,
}

impl Pace {
    /// Whether `asker` may ask one more of `most` per `window` at `now`;
    /// counted if so.
    fn take(&mut self, asker: &str, now: Instant, most: usize, window: Duration) -> bool {
        self.asks.retain(|_, at| {
            at.back()
                .is_some_and(|t| now.saturating_duration_since(*t) < window)
        });
        let at = self.asks.entry(asker.to_string()).or_default();
        while at
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= window)
        {
            at.pop_front();
        }
        if at.len() >= most {
            return false;
        }
        at.push_back(now);
        true
    }
}

const MINUTE: Duration = Duration::from_secs(60);
const PACE_WINDOW: Duration = Duration::from_secs(PACE_WINDOW_SECS);

/// A phone's lists here.
static LISTS: LazyLock<Mutex<Pace>> = LazyLock::new(Default::default);
/// A phone's introductions asked here.
static PAIRS: LazyLock<Mutex<Pace>> = LazyLock::new(Default::default);
/// A computer's introductions taken here.
static TAKEN: LazyLock<Mutex<Pace>> = LazyLock::new(Default::default);

fn paced(pace: &Mutex<Pace>, asker: &str, most: usize, window: Duration) -> bool {
    pace.lock()
        .unwrap_or_else(|e| e.into_inner())
        .take(asker, Instant::now(), most, window)
}

// ---- As the introducing computer (A) ----

/// The computers the owner phone could be introduced to: those paired both
/// ways here and `reachable` now, by the name this computer shows them.
pub(crate) fn offers(
    store: &TrustStore,
    file: &ComputersFile,
    reachable: impl Fn(&str) -> bool,
) -> Vec<Offer> {
    file.computers
        .iter()
        .filter(|c| is_mine_computer(store, file, &c.device_id) && reachable(&c.device_id))
        .map(|c| Offer {
            id: c.device_id.clone(),
            name: shown(store, file, &c.device_id),
        })
        .collect()
}

/// The introduction of `phone` to `computer`, or why there is none: the
/// computer is not paired both ways here, or no TLS key is bound to the
/// phone here to vouch for it with.
pub(crate) fn prepare(
    keys: Option<&HashMap<String, device_keys::DeviceKeys>>,
    store: &TrustStore,
    file: &ComputersFile,
    phone: &str,
    computer: &str,
) -> Result<Introduction, Refusal> {
    if !is_mine_computer(store, file, computer) {
        return Err(Refusal::NotMine);
    }
    let keys = keys
        .ok_or(Refusal::Failed)?
        .get(phone)
        .cloned()
        .unwrap_or_default();
    let Some(tls) = keys.tls else {
        return Err(Refusal::NoKey);
    };
    Ok(Introduction {
        request: uuid::Uuid::new_v4().simple().to_string(),
        phone: IntroducedPhone {
            device_id: phone.to_string(),
            name: store.name(phone).unwrap_or_default().to_string(),
            tls,
            iroh: keys.iroh,
        },
        permissions: store.permissions(phone),
    })
}

/// The pairing link to `computer` (as listed here) the phone is handed:
/// the other computer's `pin`, the addresses listed here (then any more it
/// names itself), the ports listed here, and the certificate and iroh
/// endpoint this computer pinned for it.
pub(crate) fn link_to(computer: &PeerComputer, name: &str, pin: &str, facts: &Value) -> PairLink {
    // What this computer lists and pinned comes first (PR review 5): the
    // other's word only adds addresses to try after them, each pinned to
    // the certificate this computer pinned.
    let said: Vec<String> = facts
        .get("addresses")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .take(MOST_ADDRESSES)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut addresses = computer.addresses.clone();
    for address in order_addresses(said) {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses.truncate(MOST_ADDRESSES);
    PairLink {
        name: name.to_string(),
        device_id: computer.device_id.clone(),
        addresses,
        control_port: computer.control_port,
        stream_port: computer.stream_port,
        pin: pin.to_string(),
        fingerprint: Some(computer.fingerprint.to_ascii_lowercase()),
        endpoint: computer.endpoint.as_ref().map(|e| e.to_string()),
        mine: false,
    }
}

/// Answer the owner phone `asker`'s ask in `body`; `None` for a body that
/// does not read.
pub(crate) async fn answer_phone(asker: &str, body: &Value) -> Option<Value> {
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        let request = PhoneRequest::parse(body)?;
        return Some(wire::refused_answer(
            &request.request,
            &request.ask,
            Refusal::Failed,
        ));
    };
    let reachable = reachable_now().await;
    answer_phone_in(&dir, &trust, asker, body, reachable, ask_computer).await
}

/// [`answer_phone`] on the given folder and store, with `reachable` saying
/// which computers can be sent to and `ask` asking one of them.
pub(crate) async fn answer_phone_in<A, F>(
    dir: &Path,
    trust: &Path,
    asker: &str,
    body: &Value,
    reachable: impl Fn(&str) -> bool,
    ask: A,
) -> Option<Value>
where
    A: FnOnce(String, Introduction) -> F,
    F: Future<Output = Result<(String, Value), Refusal>>,
{
    let request = PhoneRequest::parse(body)?;
    let refused = |refusal: Refusal| {
        Some(wire::refused_answer(
            &request.request,
            &request.ask,
            refusal,
        ))
    };
    let read = {
        let (dir, trust) = (dir.to_path_buf(), trust.to_path_buf());
        tokio::task::spawn_blocking(move || {
            TrustStore::read(trust)
                .ok()
                .map(|store| (store, computers::read(&dir), device_keys::read(&dir).ok()))
        })
        .await
        .ok()
        .flatten()
    };
    let Some((store, file, keys)) = read else {
        return refused(Refusal::Failed);
    };
    // Only a phone paired with this computer's own code: one another
    // computer introduced here is never introduced onward (PR #581 I2).
    if !introduces(&store, &file, asker) {
        tracing::warn!(
            asker,
            "Refused an introduction ask from a phone not paired with this computer's own code"
        );
        return refused(Refusal::NotOwner);
    }
    match &request.ask {
        PhoneAsk::List => {
            if !paced(&LISTS, asker, LISTS_PER_MINUTE, MINUTE) {
                return refused(Refusal::Busy);
            }
            let offers = offers(&store, &file, reachable);
            Some(wire::list_answer(&request.request, &offers))
        }
        PhoneAsk::Pair { device_id } => {
            if !paced(&PAIRS, asker, INTRODUCTIONS_PER_WINDOW, PACE_WINDOW) {
                return refused(Refusal::Busy);
            }
            let introduction = match prepare(keys.as_ref(), &store, &file, asker, device_id) {
                Ok(introduction) => introduction,
                Err(refusal) => return refused(refusal),
            };
            if !reachable(device_id) {
                return refused(Refusal::NotLinked);
            }
            let asked = introduction.request.clone();
            let (pin, facts) = match ask(device_id.clone(), introduction).await {
                Ok(answer) => answer,
                Err(refusal) => {
                    tracing::info!(
                        asker,
                        computer = %device_id,
                        reason = refusal.code(),
                        "An introduction was not made"
                    );
                    return refused(refusal);
                }
            };
            let Some(computer) = file.computers.iter().find(|c| &c.device_id == device_id) else {
                return refused(Refusal::NotMine);
            };
            let link = link_to(computer, &shown(&store, &file, device_id), &pin, &facts);
            // Still a phone of this computer's own code after the wait (PR
            // review 1): one removed meanwhile is handed nothing, and the
            // other computer is told to drop its PIN (PR #581 I1).
            let kept = {
                let (dir, trust) = (dir.to_path_buf(), trust.to_path_buf());
                let (asker, computer, asked) =
                    (asker.to_string(), device_id.clone(), asked.clone());
                tokio::task::spawn_blocking(move || {
                    record_pending_in(&dir, &trust, &asker, &computer, &asked)
                })
                .await
                .unwrap_or(Err(Refusal::Failed))
            };
            if let Err(refusal) = kept {
                let cancel =
                    NetworkPacket::new(INTRODUCE_PHONE).with_body(wire::cancel_body(&asked));
                crate::remote_devices::send_to_computer(device_id, cancel).await;
                return refused(refusal);
            }
            tracing::info!(
                asker,
                computer = %device_id,
                "Introduced an owner phone to another of the owner's computers"
            );
            Some(wire::pair_answer(
                &request.request,
                device_id,
                &link.to_uri(),
            ))
        }
    }
}

/// Whether `phone` may be introduced from here: an owner phone paired with
/// this computer's own code, never one another computer introduced.
fn introduces(store: &TrustStore, file: &ComputersFile, phone: &str) -> bool {
    is_owner_phone(store, file, phone) && store.is_own_code_owner(phone)
}

/// Keep that `phone` was introduced to `computer` under `request`, as
/// waiting for that computer to confirm it paired (PR #581 N3), deciding
/// again under `computers.json`'s lock that it may still be introduced (it
/// may have been removed while the other computer was asked): else nothing
/// is kept, and it is refused.
pub(crate) fn record_pending_in(
    dir: &Path,
    trust: &Path,
    phone: &str,
    computer: &str,
    request: &str,
) -> Result<(), Refusal> {
    let now = pocketdesk_core::local::pin::now_unix();
    let mut outcome = Err(Refusal::Failed);
    let saved = computers::update(dir, |file| {
        outcome = match TrustStore::read(trust) {
            Ok(store) if introduces(&store, file, phone) => {
                file.pending_introductions
                    .retain(|p| !(p.phone == phone && p.computer == computer));
                file.pending_introductions.push(PendingIntroduction {
                    phone: phone.to_string(),
                    computer: computer.to_string(),
                    request: request.to_string(),
                    unix: now,
                });
                Ok(())
            }
            Ok(_) => Err(Refusal::NotOwner),
            Err(_) => Err(Refusal::Failed),
        };
    });
    if let Err(e) = saved {
        tracing::warn!("Could not keep an introduction: {e}");
        return Err(Refusal::Failed);
    }
    outcome
}

/// One entry per phone and computer in `file`'s introductions.
fn note_introduced(file: &mut ComputersFile, phone: &str, computer: &str, now: u64) {
    file.introduced
        .retain(|i| !(i.phone == phone && i.computer == computer));
    file.introduced.push(Introduced {
        phone: phone.to_string(),
        computer: computer.to_string(),
        unix: now,
    });
}

/// The other computer `from` confirmed the phone `phone` introduced under
/// `request` paired there: the introduction is kept from now (PR #581 N3).
/// Whether one waited.
pub(crate) fn confirm_in(
    file: &mut ComputersFile,
    from: &str,
    request: &str,
    phone: &str,
    now: u64,
) -> bool {
    let Some(at) = file
        .pending_introductions
        .iter()
        .position(|p| p.computer == from && p.request == request && p.phone == phone)
    else {
        return false;
    };
    file.pending_introductions.remove(at);
    note_introduced(file, phone, from, now);
    true
}

/// The introductions waiting for the other computer's answer: by request
/// id, the computer asked and where its answer goes.
type Waiting = HashMap<String, (String, tokio::sync::oneshot::Sender<ComputerAnswer>)>;
static WAITING: LazyLock<Mutex<Waiting>> = LazyLock::new(Default::default);

/// A waiting introduction's entry, taken out however the wait ends, a
/// dropped one included (PR #581 N5).
struct WaitingEntry(String);

impl Drop for WaitingEntry {
    fn drop(&mut self) {
        WAITING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Ask `computer` to take `introduction`, on the link between the two, and
/// wait [`ANSWER_WAIT`] for its PIN and facts.
async fn ask_computer(
    computer: String,
    introduction: Introduction,
) -> Result<(String, Value), Refusal> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let request = introduction.request.clone();
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(request.clone(), (computer.clone(), tx));
    let _entry = WaitingEntry(request);
    let packet = NetworkPacket::new(INTRODUCE_PHONE).with_body(introduction.body());
    let sent = crate::remote_devices::send_to_computer(&computer, packet).await;
    let answer = if sent {
        tokio::time::timeout(ANSWER_WAIT, rx)
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    match answer {
        Some(ComputerAnswer::Accepted { pin, facts }) => Ok((pin, facts)),
        Some(ComputerAnswer::Refused(refusal)) => Err(refusal),
        Some(ComputerAnswer::Paired { .. }) => Err(Refusal::Failed),
        None if sent => Err(Refusal::NoAnswer),
        None => Err(Refusal::NotLinked),
    }
}

/// Take another computer's answer to an introduction: only from the
/// computer asked, and only for the introduction waiting (its random
/// request id). Its word that an introduced phone paired keeps the
/// introduction it confirms.
pub(crate) fn heard(from: &str, body: &Value) {
    let Some((request, answer)) = wire::read_computer_answer(body) else {
        return;
    };
    if let ComputerAnswer::Paired { device_id } = answer {
        if let Ok(dir) = crate::state::state_dir() {
            let from = from.to_string();
            let now = pocketdesk_core::local::pin::now_unix();
            let confirm = move || {
                let saved = computers::update(&dir, |file| {
                    if !confirm_in(file, &from, &request, &device_id, now) {
                        tracing::debug!("A confirmation of no introduction waiting; dropped");
                    }
                });
                if let Err(e) = saved {
                    tracing::debug!("Could not keep a confirmed introduction: {e}");
                }
            };
            match tokio::runtime::Handle::try_current() {
                Ok(_) => drop(tokio::task::spawn_blocking(confirm)),
                Err(_) => confirm(),
            }
        }
        return;
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let asked = waiting.get(&request).is_some_and(|(to, _)| to == from);
    if !asked {
        tracing::debug!("An introduction answer that no ask here waits for; dropped");
        return;
    }
    if let Some((_, tx)) = waiting.remove(&request) {
        let _ = tx.send(answer);
    }
}

// ---- As the computer the phone is introduced to (B) ----

/// A PIN this computer handed another of the owner's computers for a
/// phone it introduced: it pairs that phone, on a connection proving that
/// key, once, within [`PIN_FOR_SECS`], and is discarded after
/// [`PIN_MISSES`] wrong PINs from that phone.
#[derive(Debug, Clone)]
pub(crate) struct IntroPin {
    pub pin: String,
    pub phone: IntroducedPhone,
    pub permissions: DevicePermissions,
    /// The computer that introduced it.
    pub introducer: String,
    /// The introduction's request id, which the confirmation names.
    pub request: String,
    pub minted: Instant,
    /// Wrong PINs the phone sent while it waited.
    pub misses: u32,
}

/// Wrong PINs after which a phone's introduction PIN is discarded.
pub(crate) const PIN_MISSES: u32 = 3;

pub(crate) static PINS: LazyLock<Mutex<Vec<IntroPin>>> = LazyLock::new(Default::default);

/// The tests that mint or take introduction PINs take turns: the list is
/// one for the process.
#[cfg(test)]
pub(crate) static PIN_TURN: Mutex<()> = Mutex::new(());

const PIN_FOR: Duration = Duration::from_secs(PIN_FOR_SECS);

/// Why the id `phone` cannot be introduced here, or `None` when it is
/// free: paired here either way, a computer listed here, this computer's
/// own or the introducer's id, or bound to another TLS key than `tls`, or
/// another iroh key than the one named.
fn id_taken(
    dir: &Path,
    store: &TrustStore,
    file: &ComputersFile,
    phone: &IntroducedPhone,
    own_id: &str,
    introducer: &str,
) -> bool {
    let id = phone.device_id.as_str();
    id == own_id
        || id == introducer
        || store.is_trusted(id)
        || file.computers.iter().any(|c| c.device_id == id)
        || matches!(
            device_keys::check(dir, id, Slot::Tls, Some(&phone.tls)),
            Verdict::Refused(_)
        )
        || phone.iroh.as_deref().is_some_and(|iroh| {
            matches!(
                device_keys::check(dir, id, Slot::Iroh, Some(iroh)),
                Verdict::Refused(device_keys::Refusal::WrongKey)
            )
        })
}

/// Decide whether the computer `from` may introduce the phone in
/// `introduction` here: within the pace (refusals counted too, PR #581
/// N4), paired both ways, the owner's switch (`enabled`) on, the id free.
/// What this computer allows `from`, for the phone's permissions.
pub(crate) fn decide_in(
    dir: &Path,
    trust: &Path,
    from: &str,
    introduction: &Introduction,
    enabled: bool,
    own_id: &str,
) -> Result<DevicePermissions, Refusal> {
    if !paced(&TAKEN, from, INTRODUCTIONS_PER_WINDOW, PACE_WINDOW) {
        return Err(Refusal::Busy);
    }
    let store = TrustStore::read(trust).map_err(|_| Refusal::Failed)?;
    let file = computers::read(dir);
    if !is_mine_computer(&store, &file, from) {
        return Err(Refusal::NotMine);
    }
    if !enabled {
        return Err(Refusal::Off);
    }
    if id_taken(dir, &store, &file, &introduction.phone, own_id, from) {
        return Err(Refusal::Taken);
    }
    Ok(store.permissions(from))
}

/// Mint the PIN for `introduction` from `from`, unless [`PINS_WAITING`]
/// already wait or another computer's PIN waits for that phone (PR #581
/// N4): a PIN no other waiting one has. The same computer introducing the
/// phone again replaces its own waiting PIN, before the cap.
pub(crate) fn mint(from: &str, introduction: Introduction) -> Result<String, Refusal> {
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    pins.retain(|p| p.minted.elapsed() < PIN_FOR);
    let phone = introduction.phone.device_id.as_str();
    if pins
        .iter()
        .any(|p| p.phone.device_id == phone && p.introducer != from)
    {
        return Err(Refusal::Busy);
    }
    pins.retain(|p| p.phone.device_id != phone);
    if pins.len() >= PINS_WAITING {
        return Err(Refusal::Busy);
    }
    let pin = loop {
        let pin = pocketdesk_core::local::pin::mint_value();
        if !pins.iter().any(|p| p.pin == pin) {
            break pin;
        }
    };
    pins.push(IntroPin {
        pin: pin.clone(),
        request: introduction.request,
        phone: introduction.phone,
        permissions: introduction.permissions,
        introducer: from.to_string(),
        minted: Instant::now(),
        misses: 0,
    });
    Ok(pin)
}

/// Drop the PIN waiting for the introduction `request` that `from` made
/// (PR #581 I1: the phone was removed there). Whether one waited.
pub(crate) fn cancel_pin(from: &str, request: &str) -> bool {
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    let before = pins.len();
    pins.retain(|p| !(p.introducer == from && p.request == request));
    pins.len() != before
}

/// Drop the PIN waiting for `phone` from the introducer `from` (PR #581 I1:
/// a removal it owed names the phone before it paired). Whether one waited.
pub(crate) fn cancel_phone(from: &str, phone: &str) -> bool {
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    let before = pins.len();
    pins.retain(|p| !(p.introducer == from && p.phone.device_id == phone));
    pins.len() != before
}

/// A wrong PIN from `device_id`, while an introduction PIN waits for it:
/// counted against that PIN alone, which goes after [`PIN_MISSES`], and
/// never against the owner's own PIN (PR #581 I1). Whether one waited.
pub(crate) fn miss(device_id: &str) -> bool {
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    pins.retain(|p| p.minted.elapsed() < PIN_FOR);
    let Some(pin) = pins.iter_mut().find(|p| p.phone.device_id == device_id) else {
        return false;
    };
    pin.misses += 1;
    if pin.misses >= PIN_MISSES {
        tracing::warn!("Discarded an introduction PIN after {PIN_MISSES} wrong PINs");
        pins.retain(|p| p.phone.device_id != device_id);
    }
    true
}

/// Answer the computer `from`'s introduction in `body`, or take its
/// cancel; `None` for a body that does not read or a cancel.
pub(crate) async fn answer_computer(from: &str, body: &Value) -> Option<Value> {
    if let Some(request) = wire::read_cancel(body) {
        if cancel_pin(from, &request) {
            tracing::info!(
                from,
                "Dropped an introduction PIN: the phone was removed there"
            );
        }
        return None;
    }
    let introduction = Introduction::parse(body)?;
    let request = introduction.request.clone();
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        return Some(wire::declined_answer(&request, Refusal::Failed));
    };
    let enabled = crate::events::current_config().computer_introductions;
    let own_id = crate::plugins::pair::host_device_id();
    let decided = {
        let (dir, trust, from, introduction) = (
            dir.clone(),
            trust.clone(),
            from.to_string(),
            introduction.clone(),
        );
        tokio::task::spawn_blocking(move || {
            decide_in(&dir, &trust, &from, &introduction, enabled, &own_id)
        })
        .await
        .unwrap_or(Err(Refusal::Failed))
    };
    if let Err(refusal) = decided {
        tracing::info!(
            from,
            reason = refusal.code(),
            "Refused another computer's introduction of a phone"
        );
        return Some(wire::declined_answer(&request, refusal));
    }
    match mint(from, introduction) {
        Ok(pin) => {
            let facts = crate::peer_computers::own_facts(&dir).await;
            let facts = serde_json::to_value(facts).unwrap_or(Value::Null);
            tracing::info!(from, "Took another computer's introduction of a phone");
            Some(wire::accepted_answer(&request, &pin, facts))
        }
        Err(refusal) => Some(wire::declined_answer(&request, refusal)),
    }
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The introduction PIN `pin` for the phone `device_id` on a connection
/// that proved `key`, taken (it pairs once); `None` when no such PIN waits:
/// another phone's, another key's, or one past [`PIN_FOR_SECS`].
pub(crate) fn take_pin(device_id: &str, key: Option<&PeerKey>, pin: &str) -> Option<IntroPin> {
    let Some(PeerKey::Tls(hash)) = key else {
        return None;
    };
    let presented = hex(hash);
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    pins.retain(|p| p.minted.elapsed() < PIN_FOR);
    let at = pins
        .iter()
        .position(|p| p.pin == pin && p.phone.device_id == device_id && p.phone.tls == presented)?;
    Some(pins.remove(at))
}

/// What an admitted phone is called, for this computer's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Admitted {
    /// The introducing computer, by this computer's name for it.
    pub introducer: String,
    /// The phone, by the name the introducing computer knew it by.
    pub phone: String,
}

/// What this computer says when a phone was added through an introduction.
pub(crate) fn added_words(admitted: &Admitted) -> String {
    format!(
        "{} added {} to this computer",
        admitted.introducer, admitted.phone
    )
}

/// The permissions an introduced phone gets here (PR #581 I2): this
/// computer's defaults for a new owner phone, narrowed to what the
/// introducing computer allows it (`introduced`), and with each feature
/// this computer turned off for the introducing computer itself
/// (`for_introducer`: one a computer paired both ways has, now off) off
/// too, and held for the owner when the introducer's sessions are. Never
/// this computer's keyboard and mouse onto the phone (BK5), which only its
/// own page sets.
pub(crate) fn permissions_for(
    introduced: &DevicePermissions,
    for_introducer: &DevicePermissions,
) -> DevicePermissions {
    let mut permissions = DevicePermissions::default().narrowed_to(introduced);
    let granted = DevicePermissions::computer_mine();
    for capability in Capability::ALL {
        if granted.allows(capability) && !for_introducer.allows(capability) {
            permissions.set(capability, false);
        }
    }
    permissions.ask_first |= for_introducer.ask_first;
    permissions.set(Capability::SharedInput, false);
    permissions
}

/// Admit the phone `intro` names as it pairs, deciding again with
/// `computers.json` held and then the trust store (the order a removal
/// takes them): the introducer still paired both ways, the owner's switch
/// (`enabled`) still on, the id still free. Its keys are bound as the
/// introducer bound them, then its permissions set, then it is trusted as
/// an owner phone that introducer introduced, the introduction kept and a
/// confirmation owed to the introducer.
pub(crate) fn admit_in(
    dir: &Path,
    trust: &Path,
    intro: &IntroPin,
    enabled: bool,
    own_id: &str,
    now: u64,
) -> Result<Admitted, Refusal> {
    // With no computers file no computer is paired both ways here.
    if !dir
        .join(pocketdesk_core::local::paths::COMPUTERS_FILE)
        .exists()
    {
        return Err(Refusal::NotMine);
    }
    let phone = &intro.phone;
    let mut outcome = Err(Refusal::Failed);
    let held = computers::update(dir, |file| {
        let written = TrustStore::update(trust, |store| {
            if !is_mine_computer(store, file, &intro.introducer) {
                return Ok(Err(Refusal::NotMine));
            }
            if !enabled {
                return Ok(Err(Refusal::Off));
            }
            if id_taken(dir, store, file, phone, own_id, &intro.introducer) {
                return Ok(Err(Refusal::Taken));
            }
            // The keys first: an id trusted with no key bound would bind
            // the first that came.
            device_keys::replace(dir, &phone.device_id, &phone.tls)?;
            if let Some(iroh) = &phone.iroh {
                device_keys::register(dir, &phone.device_id, Slot::Iroh, iroh)?;
            }
            // The permissions before the trust (M2 review), held to what
            // this computer allows the introducer now.
            let for_introducer = store.permissions(&intro.introducer);
            store.set_permissions(
                &phone.device_id,
                permissions_for(&intro.permissions, &for_introducer),
            )?;
            store.trust_introduced_phone(phone.device_id.clone(), intro.introducer.clone())?;
            if !phone.name.is_empty() {
                store.note_claimed_name(&phone.device_id, &phone.name)?;
            }
            Ok(Ok(Admitted {
                introducer: crate::remote_devices::asker_name(store, file, &intro.introducer),
                phone: if phone.name.is_empty() {
                    phone.device_id.chars().take(8).collect()
                } else {
                    phone.name.clone()
                },
            }))
        });
        outcome = match written {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!("An introduced phone could not be written to the trust store: {e}");
                Err(Refusal::Failed)
            }
        };
        if outcome.is_ok() {
            note_introduced(file, &phone.device_id, &intro.introducer, now);
            // The introducer keeps the introduction only on this word.
            file.owed_confirmations.push(PendingIntroduction {
                phone: phone.device_id.clone(),
                computer: intro.introducer.clone(),
                request: intro.request.clone(),
                unix: now,
            });
        }
    });
    if held.is_err() {
        return Err(Refusal::Failed);
    }
    outcome
}

/// Pair the phone on `conn` with the introduction PIN `intro` took: as a
/// phone only, admitted ([`admit_in`]), then said and logged.
pub(crate) async fn pair_introduced(
    conn: &str,
    packet: &NetworkPacket,
    sender: &dyn DeviceSender,
    intro: IntroPin,
) {
    let asked_computer = packet.body.get("computer").and_then(Value::as_bool) == Some(true);
    if asked_computer {
        tracing::warn!("Pairing refused: an introduction PIN pairs only a phone");
        crate::plugins::pair::respond_pair(sender, false, "").await;
        return;
    }
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        crate::plugins::pair::respond_pair(sender, false, "").await;
        return;
    };
    let enabled = crate::events::current_config().computer_introductions;
    let own_id = crate::plugins::pair::host_device_id();
    let introducer = intro.introducer.clone();
    let now = pocketdesk_core::local::pin::now_unix();
    let admitted =
        tokio::task::spawn_blocking(move || admit_in(&dir, &trust, &intro, enabled, &own_id, now))
            .await
            .unwrap_or(Err(Refusal::Failed));
    match admitted {
        Ok(admitted) => {
            tracing::info!(
                introducer = %introducer,
                "Paired phone {conn}, introduced by another of the owner's computers"
            );
            crate::events::emit(
                "device_introduced",
                Some(conn),
                serde_json::json!({ "by": introducer }),
                serde_json::json!({}),
            );
            say("A phone was added", added_words(&admitted));
            crate::plugins::pair::respond_pair(
                sender,
                true,
                &crate::plugins::pair::host_device_id(),
            )
            .await;
        }
        Err(refusal) => {
            tracing::warn!(
                reason = refusal.code(),
                "Pairing refused: the introduction no longer holds"
            );
            crate::plugins::pair::respond_pair(sender, false, "").await;
        }
    }
}

/// Notify the owner (never in a test).
fn say(summary: &'static str, words: String) {
    if cfg!(test) {
        return;
    }
    let notify = move || {
        crate::platform::notify(summary, &words, Duration::from_secs(30));
    };
    match tokio::runtime::Handle::try_current() {
        Ok(_) => drop(tokio::task::spawn_blocking(notify)),
        Err(_) => notify(),
    }
}

/// The introduced owner phones whose introducer is no longer paired both
/// ways here (unpaired, or its grant lapsed) lose the owner mark: they stay
/// paired, with their permissions as they are (PR #581 I2). The phones
/// demoted, each with this computer's name for its introducer.
pub(crate) fn demote_orphans_in(dir: &Path, trust: &Path) -> Vec<(String, String)> {
    let Ok(store) = TrustStore::read(trust) else {
        return Vec::new();
    };
    let file = computers::read(dir);
    let orphans: Vec<(String, String)> = store
        .introduced_owners()
        .into_iter()
        .filter(|(_, by)| !is_mine_computer(&store, &file, by))
        .collect();
    if orphans.is_empty() {
        return orphans;
    }
    let mut demoted = Vec::new();
    let saved = TrustStore::update(trust, |store| {
        for (phone, by) in &orphans {
            if store.introducer(phone) == Some(by.as_str()) && !is_mine_computer(store, &file, by) {
                store.demote_owner(phone)?;
                let name = store
                    .name(phone)
                    .map(str::to_string)
                    .unwrap_or_else(|| phone.chars().take(8).collect());
                demoted.push((name, crate::remote_devices::asker_name(store, &file, by)));
            }
        }
        Ok(())
    });
    if let Err(e) = saved {
        tracing::warn!("Could not take the owner mark from introduced phones: {e}");
    }
    demoted
}

/// [`demote_orphans_in`] on this computer's files, each said.
fn demote_orphans(dir: &Path, trust: &Path) {
    for (phone, by) in demote_orphans_in(dir, trust) {
        tracing::warn!(
            "An introduced phone is no longer an owner phone here: its introducer is not paired both ways"
        );
        say(
            "A phone is no longer treated as yours",
            format!(
                "{phone}, added by {by}, stays paired here but may no longer manage this computer's devices: {by} is no longer paired both ways with it. Pair the phone with this computer's own code to make it yours again."
            ),
        );
    }
}

// ---- Removal follows the introduction ----

/// Owe a removal for each phone introduced between this computer and
/// another, or waiting for that computer's confirmation, that is no longer
/// among the `paired` here, at `now`; whether any was owed. One not yet
/// confirmed carries a cancel of its PIN there (PR #581 I1).
pub(crate) fn owe_in(file: &mut ComputersFile, paired: &[String], now: u64) -> bool {
    let (gone, kept): (Vec<Introduced>, Vec<Introduced>) = std::mem::take(&mut file.introduced)
        .into_iter()
        .partition(|i| !paired.contains(&i.phone));
    file.introduced = kept;
    let (waiting, still): (Vec<PendingIntroduction>, Vec<PendingIntroduction>) =
        std::mem::take(&mut file.pending_introductions)
            .into_iter()
            .partition(|p| !paired.contains(&p.phone));
    file.pending_introductions = still;
    let owed = gone
        .iter()
        .map(|i| (i.phone.clone(), i.computer.clone(), None))
        .chain(
            waiting
                .iter()
                .map(|p| (p.phone.clone(), p.computer.clone(), Some(p.request.clone()))),
        );
    let mut any = false;
    for (phone, computer, cancel) in owed {
        any = true;
        if let Some(o) = file
            .owed_removals
            .iter_mut()
            .find(|o| o.phone == phone && o.computer == computer)
        {
            o.cancel = o.cancel.take().or(cancel);
            continue;
        }
        file.owed_removals.push(OwedRemoval {
            phone,
            computer,
            unix: now,
            request: uuid::Uuid::new_v4().simple().to_string(),
            sent_unix: 0,
            cancel,
        });
    }
    any
}

/// [`owe_in`] against the trust store at `trust` as it is now. A store
/// that is missing or does not read owes nothing (PR #581 N2): no phone is
/// taken for removed because its record could not be read. A file with
/// nothing introduced is not written (nor made).
pub(crate) fn owe_removals(dir: &Path, trust: &Path) {
    let file = computers::read(dir);
    if file.introduced.is_empty() && file.pending_introductions.is_empty() {
        return;
    }
    if !trust.exists() {
        return;
    }
    let Ok(store) = TrustStore::read(trust) else {
        return;
    };
    let paired = store.trusted_devices();
    let due = file
        .introduced
        .iter()
        .map(|i| &i.phone)
        .chain(file.pending_introductions.iter().map(|p| &p.phone))
        .any(|phone| !paired.contains(phone));
    if !due {
        return;
    }
    let now = pocketdesk_core::local::pin::now_unix();
    if let Err(e) = computers::update(dir, |file| {
        owe_in(file, &paired, now);
    }) {
        tracing::warn!("Could not owe a removed phone's removal to another computer: {e}");
    }
}

/// The owed removals to send at `now` to the computers `reachable` now
/// (sent ones are marked only once sent, [`mark_sent`]), and those dropped:
/// owed past [`OWED_FOR`], or to a computer no longer listed here (PR #581
/// N1), which the caller says. Unconfirmed introductions past
/// [`PENDING_INTRODUCTION_FOR`] are dropped too.
pub(crate) fn due_in(
    file: &mut ComputersFile,
    now: u64,
    reachable: impl Fn(&str) -> bool,
) -> (Vec<OwedRemoval>, Vec<OwedRemoval>) {
    let listed: Vec<String> = file.computers.iter().map(|c| c.device_id.clone()).collect();
    let (dropped, kept): (Vec<OwedRemoval>, Vec<OwedRemoval>) =
        std::mem::take(&mut file.owed_removals)
            .into_iter()
            .partition(|o| now.saturating_sub(o.unix) >= OWED_FOR || !listed.contains(&o.computer));
    file.owed_removals = kept;
    file.pending_introductions
        .retain(|p| now.saturating_sub(p.unix) < PENDING_INTRODUCTION_FOR);
    file.owed_confirmations.retain(|p| {
        now.saturating_sub(p.unix) < PENDING_INTRODUCTION_FOR && listed.contains(&p.computer)
    });
    let due = file
        .owed_removals
        .iter()
        .filter(|o| o.sent_unix == 0 || now.saturating_sub(o.sent_unix) >= OWED_RESEND)
        .filter(|o| reachable(&o.computer))
        .cloned()
        .collect();
    (due, dropped)
}

/// Mark the owed removal `request` sent at `now` (PR #581 N1: only once it
/// went).
pub(crate) fn mark_sent(file: &mut ComputersFile, request: &str, now: u64) {
    if let Some(o) = file.owed_removals.iter_mut().find(|o| o.request == request) {
        o.sent_unix = now;
    }
}

/// The packets an owed removal goes as: the cancel of its PIN there first,
/// when the introduction was not yet confirmed, then the removal, marked
/// as one owed for an introduction (PR #581 N3).
pub(crate) fn owed_packets(owed: &OwedRemoval) -> Vec<NetworkPacket> {
    let mut packets = Vec::new();
    if let Some(cancel) = &owed.cancel {
        packets.push(NetworkPacket::new(INTRODUCE_PHONE).with_body(wire::cancel_body(cancel)));
    }
    let mut body = revoke::Request {
        request: owed.request.clone(),
        ask: Ask::Remove {
            device_id: owed.phone.clone(),
        },
    }
    .body();
    body["introduced"] = true.into();
    packets.push(NetworkPacket::new(revoke::REMOTE_DEVICES).with_body(body));
    packets
}

/// Send what this computer owes the computers it can reach now: the
/// removals of phones introduced between them ([`owe_removals`]), and the
/// confirmations of introduced phones that paired here. `send` sends one
/// packet to one computer, saying whether it went. The removals dropped
/// unsent, for the caller to say.
pub(crate) async fn send_owed_with<S, F>(
    dir: &Path,
    trust: &Path,
    reachable: impl Fn(&str) -> bool + Send + 'static,
    send: S,
) -> Vec<OwedRemoval>
where
    S: Fn(String, NetworkPacket) -> F,
    F: Future<Output = bool>,
{
    let owes = {
        let (dir, trust) = (dir.to_path_buf(), trust.to_path_buf());
        tokio::task::spawn_blocking(move || {
            owe_removals(&dir, &trust);
            let file = computers::read(&dir);
            !file.owed_removals.is_empty()
                || !file.owed_confirmations.is_empty()
                || !file.pending_introductions.is_empty()
        })
        .await
        .unwrap_or(false)
    };
    if !owes {
        return Vec::new();
    }
    let now = pocketdesk_core::local::pin::now_unix();
    let worked = {
        let dir = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut out = (Vec::new(), Vec::new(), Vec::new());
            computers::update(&dir, |file| {
                let (due, dropped) = due_in(file, now, &reachable);
                let confirmations = file
                    .owed_confirmations
                    .iter()
                    .filter(|c| reachable(&c.computer))
                    .cloned()
                    .collect();
                out = (due, dropped, confirmations);
            })
            .map(|()| out)
        })
        .await
    };
    let (due, dropped, confirmations) = match worked {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            tracing::debug!("Could not read what is owed to other computers: {e}");
            return Vec::new();
        }
        Err(_) => return Vec::new(),
    };
    let mut sent = Vec::new();
    for owed in &due {
        let mut went = true;
        for packet in owed_packets(owed) {
            went &= send(owed.computer.clone(), packet).await;
        }
        if went {
            tracing::info!(
                computer = %owed.computer,
                "Asked another computer to remove a phone removed here, introduced between the two"
            );
            sent.push(owed.request.clone());
        }
    }
    let mut confirmed = Vec::new();
    for c in &confirmations {
        let packet = NetworkPacket::new(INTRODUCE_PHONE_ANSWER)
            .with_body(wire::confirmed_answer(&c.request, &c.phone));
        if send(c.computer.clone(), packet).await {
            confirmed.push(c.request.clone());
        }
    }
    if !sent.is_empty() || !confirmed.is_empty() {
        let dir = dir.to_path_buf();
        let _ = tokio::task::spawn_blocking(move || {
            computers::update(&dir, |file| {
                for request in &sent {
                    mark_sent(file, request, now);
                }
                file.owed_confirmations
                    .retain(|c| !confirmed.contains(&c.request));
            })
        })
        .await;
    }
    dropped
}

/// Send what this computer owes the computers linked now, say any removal
/// dropped unsent, and take the owner mark from phones whose introducer is
/// no longer paired both ways. Every few seconds, from the watcher.
pub(crate) async fn send_owed(dir: &Path) {
    let Ok(trust) = crate::state::trust_store_path() else {
        return;
    };
    let reachable = reachable_now().await;
    let dropped = send_owed_with(dir, &trust, reachable, |computer, packet| async move {
        crate::remote_devices::send_to_computer(&computer, packet).await
    })
    .await;
    for owed in dropped {
        tracing::warn!(
            computer = %owed.computer,
            "Dropped a removal owed to another computer unsent"
        );
        let short: String = owed.phone.chars().take(8).collect();
        say(
            "A phone may stay paired elsewhere",
            format!(
                "A phone removed here ({short}) could not be removed from another of your computers it was added to (it was not reached in time, or was unpaired here): remove it there (Devices → Unpair)."
            ),
        );
    }
    let (dir, trust) = (dir.to_path_buf(), trust.clone());
    let _ = tokio::task::spawn_blocking(move || demote_orphans(&dir, &trust)).await;
}

/// Take another computer's answer to an owed removal (`request`): dropped
/// once answered, unless the answer says to try again (too many, busy,
/// failed). What it said, when it was one; `None` for an answer to no
/// owed removal (the app's own ask).
pub(crate) fn answered_in(
    file: &mut ComputersFile,
    from: &str,
    request: &str,
    answer: &revoke::Answer,
) -> Option<Result<(), revoke::Refusal>> {
    let at = file
        .owed_removals
        .iter()
        .position(|o| o.computer == from && o.request == request)?;
    let outcome = match answer {
        revoke::Answer::Removed { outcome, .. } => *outcome,
        revoke::Answer::Refused(refusal) => Err(*refusal),
        revoke::Answer::Listed(_) => return None,
    };
    let again = matches!(
        outcome,
        Err(revoke::Refusal::TooMany | revoke::Refusal::Busy | revoke::Refusal::Failed)
    );
    if !again {
        file.owed_removals.remove(at);
    }
    Some(outcome)
}

/// [`answered_in`] on this computer's file; whether it was the answer to
/// an owed removal. One the other computer refused for good (its switch
/// off, or this computer no longer its own) is said here.
pub(crate) fn answered(dir: &Path, from: &str, request: &str, answer: &revoke::Answer) -> bool {
    let owed = computers::read(dir)
        .owed_removals
        .iter()
        .any(|o| o.computer == from && o.request == request);
    if !owed {
        return false;
    }
    let mut outcome = None;
    let mut phone = String::new();
    let saved = computers::update(dir, |file| {
        if let Some(o) = file
            .owed_removals
            .iter()
            .find(|o| o.computer == from && o.request == request)
        {
            phone = o.phone.clone();
        }
        outcome = answered_in(file, from, request, answer);
    });
    if let Err(e) = saved {
        tracing::debug!("Could not take an owed removal's answer: {e}");
    }
    if let Some(Err(refusal @ (revoke::Refusal::Off | revoke::Refusal::NotOwner))) = outcome {
        tracing::warn!(
            computer = from,
            reason = refusal.code(),
            "Another computer kept a phone removed here, introduced between the two"
        );
        let short: String = phone.chars().take(8).collect();
        say(
            "A phone stays paired elsewhere",
            format!(
                "A phone removed here ({short}) stays paired with another of your computers: remove it there (Devices → Unpair)."
            ),
        );
    }
    true
}

/// How a removal another computer owes this one for an introduction is
/// taken here (PR #581 N3, I1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwedGate {
    /// The phone had not paired yet: its waiting PIN from that computer is
    /// dropped, and that is the removal.
    Cancelled,
    /// The phone was introduced between the two: removed as asked.
    Remove,
    /// The phone was not introduced between the two: not removed this way
    /// (that computer can still remove it by asking, as remote revoke).
    NotIntroduced,
}

/// [`OwedGate`] for the removal of `target` that `asker` owed this computer.
pub(crate) fn owed_gate_in(dir: &Path, trust: &Path, asker: &str, target: &str) -> OwedGate {
    if cancel_phone(asker, target) {
        return OwedGate::Cancelled;
    }
    let file = computers::read(dir);
    let recorded = file
        .introduced
        .iter()
        .any(|i| i.phone == target && i.computer == asker)
        || file
            .pending_introductions
            .iter()
            .any(|p| p.phone == target && p.computer == asker);
    let introduced_by = TrustStore::read(trust)
        .ok()
        .is_some_and(|store| store.introducer(target) == Some(asker));
    if recorded || introduced_by {
        OwedGate::Remove
    } else {
        OwedGate::NotIntroduced
    }
}

/// Forget that `phone` was introduced between this computer and
/// `computer`: that computer removed it here itself, so no removal is owed
/// back to it.
pub(crate) fn forget_in(file: &mut ComputersFile, phone: &str, computer: &str) {
    file.introduced
        .retain(|i| !(i.phone == phone && i.computer == computer));
    file.pending_introductions
        .retain(|i| !(i.phone == phone && i.computer == computer));
}

// ---- The plugin ----

/// The plugin taking an owner phone's ask here, another computer's
/// introduction of a phone, and another computer's answer to this one's.
pub struct IntroductionsPlugin;

#[async_trait::async_trait]
impl Plugin for IntroductionsPlugin {
    fn name(&self) -> &'static str {
        "introductions"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[INTRODUCE, INTRODUCE_PHONE, INTRODUCE_PHONE_ANSWER]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[INTRODUCE_ANSWER, INTRODUCE_PHONE, INTRODUCE_PHONE_ANSWER]
    }

    async fn handle_packet(
        &self,
        packet: &NetworkPacket,
        sender: &dyn DeviceSender,
    ) -> pocketdesk_core::error::Result<()> {
        if packet.packet_type == INTRODUCE {
            // A pairing ask waits up to ANSWER_WAIT for the other computer:
            // off this connection's packet loop, answered on the same
            // connection (PR #581 N5).
            let pairing = PhoneRequest::parse(&packet.body)
                .is_some_and(|r| matches!(r.ask, PhoneAsk::Pair { .. }));
            let connection = sender.connection_id().to_string();
            let own = if pairing {
                crate::state::clone_clients()
                    .await
                    .into_iter()
                    .find(|c| c.connection_id() == connection)
            } else {
                None
            };
            if let Some(own) = own {
                let body = packet.body.clone();
                tokio::spawn(async move {
                    if let Some(answer) = answer_phone(own.device_id(), &body).await {
                        let packet = NetworkPacket::new(INTRODUCE_ANSWER).with_body(answer);
                        let _ = own.send_packet(&packet).await;
                    }
                });
            } else if let Some(body) = answer_phone(sender.device_id(), &packet.body).await {
                sender
                    .send_packet(&NetworkPacket::new(INTRODUCE_ANSWER).with_body(body))
                    .await?;
            }
        } else if packet.packet_type == INTRODUCE_PHONE {
            if let Some(body) = answer_computer(sender.device_id(), &packet.body).await {
                sender
                    .send_packet(&NetworkPacket::new(INTRODUCE_PHONE_ANSWER).with_body(body))
                    .await?;
            }
        } else if packet.packet_type == INTRODUCE_PHONE_ANSWER {
            heard(sender.device_id(), &packet.body);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "introductions_tests.rs"]
mod tests;
