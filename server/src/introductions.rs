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
//! ([`take_pin`]). As the phone pairs, everything is decided again under
//! the files' locks ([`admit_in`]): the introducer is still paired both
//! ways, the switch still on, the id still free. The phone is then an
//! owner phone here, with this computer's defaults for a new phone
//! narrowed to what the introducing computer allows it (never more than
//! either), and its keys as the introducing computer bound them. This
//! computer says so ("<A> added <phone> to this computer") and writes
//! `device_introduced`.
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
    self, ComputersFile, Introduced, OWED_FOR, OWED_RESEND, OwedRemoval, PeerComputer,
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
    dir: &Path,
    store: &TrustStore,
    file: &ComputersFile,
    phone: &str,
    computer: &str,
) -> Result<Introduction, Refusal> {
    if !is_mine_computer(store, file, computer) {
        return Err(Refusal::NotMine);
    }
    let keys = device_keys::read(dir)
        .map_err(|_| Refusal::Failed)?
        .remove(phone)
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
/// the other computer's `pin`, its addresses (its own word first, then
/// those listed here), its ports, and the certificate and iroh endpoint
/// this computer pinned for it (its own word for the endpoint only when
/// none is listed).
pub(crate) fn link_to(computer: &PeerComputer, name: &str, pin: &str, facts: &Value) -> PairLink {
    // Each fact read by itself: one that does not read is left to what is
    // listed here.
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
    let mut addresses = order_addresses(said);
    for address in &computer.addresses {
        if !addresses.contains(address) {
            addresses.push(address.clone());
        }
    }
    addresses.truncate(MOST_ADDRESSES);
    let port = |key: &str, listed: u16| {
        facts
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p != 0)
            .unwrap_or(listed)
    };
    let endpoint = computer
        .endpoint
        .clone()
        .or_else(|| facts.get("endpoint").filter(|e| e.is_object()).cloned())
        .map(|e| e.to_string());
    PairLink {
        name: name.to_string(),
        device_id: computer.device_id.clone(),
        addresses,
        control_port: port("controlPort", computer.control_port),
        stream_port: port("streamPort", computer.stream_port),
        pin: pin.to_string(),
        fingerprint: Some(computer.fingerprint.to_ascii_lowercase()),
        endpoint,
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
                .map(|store| (store, computers::read(&dir)))
        })
        .await
        .ok()
        .flatten()
    };
    let Some((store, file)) = read else {
        return refused(Refusal::Failed);
    };
    if !is_owner_phone(&store, &file, asker) {
        tracing::warn!(
            asker,
            "Refused an introduction ask from a phone not the owner's"
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
            let introduction = match prepare(dir, &store, &file, asker, device_id) {
                Ok(introduction) => introduction,
                Err(refusal) => return refused(refusal),
            };
            if !reachable(device_id) {
                return refused(Refusal::NotLinked);
            }
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
            record_introduced(dir, asker, device_id);
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

/// Keep that `phone` was introduced between this computer and `computer`.
fn record_introduced(dir: &Path, phone: &str, computer: &str) {
    let now = pocketdesk_core::local::pin::now_unix();
    let saved = computers::update(dir, |file| note_introduced(file, phone, computer, now));
    if let Err(e) = saved {
        tracing::warn!("Could not keep an introduction: {e}");
    }
}

/// [`record_introduced`] on `file`: one entry per phone and computer.
fn note_introduced(file: &mut ComputersFile, phone: &str, computer: &str, now: u64) {
    file.introduced
        .retain(|i| !(i.phone == phone && i.computer == computer));
    file.introduced.push(Introduced {
        phone: phone.to_string(),
        computer: computer.to_string(),
        unix: now,
    });
}

/// The introductions waiting for the other computer's answer: by request
/// id, the computer asked and where its answer goes.
type Waiting = HashMap<String, (String, tokio::sync::oneshot::Sender<ComputerAnswer>)>;
static WAITING: LazyLock<Mutex<Waiting>> = LazyLock::new(Default::default);

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
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request);
    match answer {
        Some(ComputerAnswer::Accepted { pin, facts }) => Ok((pin, facts)),
        Some(ComputerAnswer::Refused(refusal)) => Err(refusal),
        None if sent => Err(Refusal::NoAnswer),
        None => Err(Refusal::NotLinked),
    }
}

/// Take another computer's answer to an introduction: only from the
/// computer asked, and only for the introduction waiting (its random
/// request id).
pub(crate) fn heard(from: &str, body: &Value) {
    let Some((request, answer)) = wire::read_computer_answer(body) else {
        return;
    };
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
/// key, once, within [`PIN_FOR_SECS`].
#[derive(Debug, Clone)]
pub(crate) struct IntroPin {
    pub pin: String,
    pub phone: IntroducedPhone,
    pub permissions: DevicePermissions,
    /// The computer that introduced it.
    pub introducer: String,
    pub minted: Instant,
}

static PINS: LazyLock<Mutex<Vec<IntroPin>>> = LazyLock::new(Default::default);

const PIN_FOR: Duration = Duration::from_secs(PIN_FOR_SECS);

/// Why the id `phone` cannot be introduced here, or `None` when it is
/// free: paired here either way, a computer listed here, this computer's
/// own or the introducer's id, or bound to another TLS key than `tls`.
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
}

/// Decide whether the computer `from` may introduce the phone in
/// `introduction` here: paired both ways, the owner's switch (`enabled`)
/// on, the id free, and not past the pace.
pub(crate) fn decide_in(
    dir: &Path,
    trust: &Path,
    from: &str,
    introduction: &Introduction,
    enabled: bool,
    own_id: &str,
) -> Result<(), Refusal> {
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
    if !paced(&TAKEN, from, INTRODUCTIONS_PER_WINDOW, PACE_WINDOW) {
        return Err(Refusal::Busy);
    }
    Ok(())
}

/// Mint the PIN for `introduction` from `from`, unless [`PINS_WAITING`]
/// already wait: a PIN no other waiting one has.
fn mint(from: &str, introduction: Introduction) -> Result<String, Refusal> {
    let mut pins = PINS.lock().unwrap_or_else(|e| e.into_inner());
    pins.retain(|p| p.minted.elapsed() < PIN_FOR);
    if pins.len() >= PINS_WAITING {
        return Err(Refusal::Busy);
    }
    // A phone introduced again replaces its waiting PIN.
    pins.retain(|p| p.phone.device_id != introduction.phone.device_id);
    let pin = loop {
        let pin = pocketdesk_core::local::pin::mint_value();
        if !pins.iter().any(|p| p.pin == pin) {
            break pin;
        }
    };
    pins.push(IntroPin {
        pin: pin.clone(),
        phone: introduction.phone,
        permissions: introduction.permissions,
        introducer: from.to_string(),
        minted: Instant::now(),
    });
    Ok(pin)
}

/// Answer the computer `from`'s introduction in `body`; `None` for a body
/// that does not read.
pub(crate) async fn answer_computer(from: &str, body: &Value) -> Option<Value> {
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

/// The permissions an introduced phone gets here: this computer's defaults
/// for a new owner phone, narrowed to what the introducing computer allows
/// it, and never this computer's keyboard and mouse onto the phone (BK5),
/// which only its own page sets.
pub(crate) fn permissions_for(introduced: &DevicePermissions) -> DevicePermissions {
    let mut permissions = DevicePermissions::default().narrowed_to(introduced);
    permissions.set(Capability::SharedInput, false);
    permissions
}

/// Admit the phone `intro` names as it pairs, deciding again with
/// `computers.json` held and then the trust store (the order a removal
/// takes them): the introducer still paired both ways, the owner's switch
/// (`enabled`) still on, the id still free. Its keys are bound as the
/// introducer bound them, then its permissions set, then it is trusted as
/// an owner phone, and the introduction is kept.
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
            // The permissions before the trust (M2 review).
            store.set_permissions(&phone.device_id, permissions_for(&intro.permissions))?;
            store.trust_phone(phone.device_id.clone(), true)?;
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
            if !cfg!(test) {
                let words = added_words(&admitted);
                tokio::task::spawn_blocking(move || {
                    crate::platform::notify("A phone was added", &words, Duration::from_secs(30))
                });
            }
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

// ---- Removal follows the introduction ----

/// Owe a removal for each phone introduced between this computer and
/// another that is no longer among the `paired` here, at `now`; whether
/// any was owed.
pub(crate) fn owe_in(file: &mut ComputersFile, paired: &[String], now: u64) -> bool {
    let (gone, kept): (Vec<Introduced>, Vec<Introduced>) = std::mem::take(&mut file.introduced)
        .into_iter()
        .partition(|i| !paired.contains(&i.phone));
    file.introduced = kept;
    for i in &gone {
        let owed = file
            .owed_removals
            .iter()
            .any(|o| o.phone == i.phone && o.computer == i.computer);
        if !owed {
            file.owed_removals.push(OwedRemoval {
                phone: i.phone.clone(),
                computer: i.computer.clone(),
                unix: now,
                request: uuid::Uuid::new_v4().simple().to_string(),
                sent_unix: 0,
            });
        }
    }
    !gone.is_empty()
}

/// [`owe_in`] on this computer's file, read first so a file with nothing
/// introduced is not written (nor made).
pub(crate) fn owe_removals(dir: &Path, paired: &[String]) {
    let file = computers::read(dir);
    if !file.introduced.iter().any(|i| !paired.contains(&i.phone)) {
        return;
    }
    let now = pocketdesk_core::local::pin::now_unix();
    if let Err(e) = computers::update(dir, |file| {
        owe_in(file, paired, now);
    }) {
        tracing::warn!("Could not owe a removed phone's removal to another computer: {e}");
    }
}

/// The owed removals to send at `now` to the computers `reachable` now,
/// each marked sent; those owed past [`OWED_FOR`] dropped.
pub(crate) fn due_in(
    file: &mut ComputersFile,
    now: u64,
    reachable: impl Fn(&str) -> bool,
) -> Vec<OwedRemoval> {
    file.owed_removals
        .retain(|o| now.saturating_sub(o.unix) < OWED_FOR);
    let mut due = Vec::new();
    for owed in &mut file.owed_removals {
        let waited = owed.sent_unix == 0 || now.saturating_sub(owed.sent_unix) >= OWED_RESEND;
        if waited && reachable(&owed.computer) {
            owed.sent_unix = now;
            due.push(owed.clone());
        }
    }
    due
}

/// Send the removals this computer owes ([`owe_removals`]) to the
/// computers it can reach now, as remote revoke's `remove`.
pub(crate) async fn send_owed(dir: &Path) {
    let owes = !computers::read(dir).owed_removals.is_empty();
    if !owes {
        return;
    }
    let reachable = reachable_now().await;
    let now = pocketdesk_core::local::pin::now_unix();
    let mut due = Vec::new();
    let changed = computers::update(dir, |file| due = due_in(file, now, &reachable));
    if let Err(e) = changed {
        tracing::debug!("Could not read the removals owed to other computers: {e}");
        return;
    }
    for owed in due {
        let request = revoke::Request {
            request: owed.request.clone(),
            ask: Ask::Remove {
                device_id: owed.phone.clone(),
            },
        };
        let packet = NetworkPacket::new(revoke::REMOTE_DEVICES).with_body(request.body());
        if crate::remote_devices::send_to_computer(&owed.computer, packet).await {
            tracing::info!(
                computer = %owed.computer,
                "Asked another computer to remove a phone removed here, introduced between the two"
            );
        }
    }
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
        if !cfg!(test) {
            let short: String = phone.chars().take(8).collect();
            let words = format!(
                "A phone removed here ({short}) stays paired with another of your computers: remove it there (Devices → Unpair)."
            );
            tokio::task::spawn_blocking(move || {
                crate::platform::notify(
                    "A phone stays paired elsewhere",
                    &words,
                    Duration::from_secs(30),
                )
            });
        }
    }
    true
}

/// Forget that `phone` was introduced between this computer and
/// `computer`: that computer removed it here itself, so no removal is owed
/// back to it.
pub(crate) fn forget_in(file: &mut ComputersFile, phone: &str, computer: &str) {
    file.introduced
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
            if let Some(body) = answer_phone(sender.device_id(), &packet.body).await {
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
