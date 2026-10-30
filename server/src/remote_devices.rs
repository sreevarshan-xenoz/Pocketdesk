//! Removing a lost device from another of the owner's devices (remote
//! revoke). An owner device asks this computer which devices are paired
//! here ([`REMOTE_DEVICES`] `list`) and asks it to remove one (`remove`),
//! as the Devices page's Unpair would: its trust and keys go, its sessions
//! and connections end, a computer is forgotten as Unpair forgets it, an
//! event is written and this computer says who removed what.
//!
//! **Who may ask** ([`is_owner_device`]): a phone paired with the owner's
//! own code (never someone else's, a profile's or a guest's), or another of
//! the owner's computers paired both ways whose grant has not lapsed. The
//! same gate holds on every road an ask comes by: a phone's connection, a
//! computer's connection here, and the link this computer keeps to a
//! computer ([`crate::peer_computers`]'s `take`).
//!
//! **Its limits:** the computer's owner can turn it off ("Let my devices
//! remove others from afar", `remote_revoke`, on by default); at most
//! [`REMOVALS_PER_MINUTE`] removals and [`LISTS_PER_MINUTE`] lists a
//! minute per asker (kept in memory: a restart starts the count again);
//! never the asker itself. The asker must still be an owner device as the
//! store is written, under its lock, so it is always left, and two owner
//! devices removing each other at once leave one.
//!
//! This computer asks another the same way ([`ask`]), for the desktop
//! app's "Devices there…", and keeps the answer in `computers.json`
//! ([`computers::DevicesThere`]) for the app.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputersFile, DevicesThere};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::remote_devices::{
    self as wire, Answer, Ask, REMOTE_DEVICES, REMOTE_DEVICES_ANSWER, REMOVALS_PER_MINUTE, Refusal,
    Request, ThereDevice,
};
use serde_json::Value;

/// Whether `device_id` is an owner device here: a phone paired for good
/// with the owner's own code, or a computer paired both ways (`mine`, its
/// grant neither waiting for the link nor lapsed).
pub(crate) fn is_owner_device(store: &TrustStore, file: &ComputersFile, device_id: &str) -> bool {
    if !store.is_trusted(device_id) {
        return false;
    }
    if store.is_computer(device_id) {
        file.both_ways(device_id) && !file.pending.iter().any(|id| id == device_id)
    } else {
        store.is_owner_phone(device_id) && store.grant_remaining(device_id).is_none()
    }
}

/// Who asked, as this computer knows it, never the name the device gives
/// itself (a stolen phone could call itself anything): the owner's rename
/// here, else a listed computer's name, with its kind and the start of
/// its id, as the Activity page names it.
fn asker_name(store: &TrustStore, file: &ComputersFile, device_id: &str) -> String {
    let kind = if store.is_computer(device_id) {
        "computer"
    } else {
        "phone"
    };
    let short: String = device_id.chars().take(8).collect();
    let known = store
        .chosen_name(device_id)
        .map(str::to_string)
        .or_else(|| {
            file.computers
                .iter()
                .find(|c| c.device_id == device_id)
                .and_then(|c| pocketdesk_core::protocol::kdeconnect::clean_device_name(&c.name))
        });
    match known {
        Some(name) => format!("{name} ({kind} {short})"),
        None => format!("A {kind} ({short})"),
    }
}

/// A paired device's name as the Devices page shows it: the owner's
/// rename or its own, a listed computer's name, else the start of its id.
fn shown_name(store: &TrustStore, file: &ComputersFile, device_id: &str) -> String {
    store
        .name(device_id)
        .map(str::to_string)
        .or_else(|| {
            file.computers
                .iter()
                .find(|c| c.device_id == device_id)
                .and_then(|c| pocketdesk_core::protocol::kdeconnect::clean_device_name(&c.name))
        })
        .unwrap_or_else(|| device_id.chars().take(8).collect())
}

/// The devices paired here, for the owner device `asker`: each one's id,
/// name, kind, standing and whether it is `connected` now. No key, address
/// or permission.
pub(crate) fn listing(
    store: &TrustStore,
    file: &ComputersFile,
    asker: &str,
    connected: impl Fn(&str) -> bool,
) -> Vec<ThereDevice> {
    store
        .trusted_devices()
        .into_iter()
        .map(|id| ThereDevice {
            name: store.name(&id).map(str::to_string).unwrap_or_else(|| {
                file.computers
                    .iter()
                    .find(|c| c.device_id == id)
                    .and_then(|c| pocketdesk_core::protocol::kdeconnect::clean_device_name(&c.name))
                    .unwrap_or_default()
            }),
            computer: store.is_computer(&id),
            owner: is_owner_device(store, file, &id),
            guest: store.grant_remaining(&id).is_some(),
            connected: connected(&id),
            you: id == asker,
            id,
        })
        .collect()
}

/// Why `asker` may not remove `target` here, or `None` when it may. The
/// last-owner check cannot refuse today: past the first two checks the
/// asker is an owner device that stays. It is kept as a guard.
pub(crate) fn refusal(
    store: &TrustStore,
    file: &ComputersFile,
    asker: &str,
    target: &str,
) -> Option<Refusal> {
    if !is_owner_device(store, file, asker) {
        return Some(Refusal::NotOwner);
    }
    if target == asker {
        return Some(Refusal::Yourself);
    }
    if !store.is_trusted(target) {
        return Some(Refusal::NotPaired);
    }
    if leaves_no_owner(store, file, target) {
        return Some(Refusal::LastOwner);
    }
    None
}

/// Whether removing `target` would leave no owner device here.
fn leaves_no_owner(store: &TrustStore, file: &ComputersFile, target: &str) -> bool {
    !store
        .trusted_devices()
        .iter()
        .any(|id| id != target && is_owner_device(store, file, id))
}

/// How many removals each asker made in the last minute.
#[derive(Default)]
struct Pace {
    asks: HashMap<String, VecDeque<Instant>>,
}

impl Pace {
    /// Whether `asker` may ask one more removal at `now`; counted if so.
    fn take(&mut self, asker: &str, now: Instant) -> bool {
        self.take_up_to(asker, now, REMOVALS_PER_MINUTE)
    }

    /// Whether `asker` may ask one more of `most` a minute at `now`.
    fn take_up_to(&mut self, asker: &str, now: Instant, most: usize) -> bool {
        self.asks
            .retain(|_, at| at.back().is_some_and(|t| now.duration_since(*t) < MINUTE));
        let at = self.asks.entry(asker.to_string()).or_default();
        while at.front().is_some_and(|t| now.duration_since(*t) >= MINUTE) {
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

/// At most this many lists a minute are taken from one asker.
pub(crate) const LISTS_PER_MINUTE: usize = 30;

static PACE: LazyLock<Mutex<Pace>> = LazyLock::new(Default::default);

/// The lists each asker made in the last minute.
static LIST_PACE: LazyLock<Mutex<Pace>> = LazyLock::new(Default::default);

/// What a removal took away, for its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Removed {
    /// Who asked, by name.
    pub asker: String,
    /// What went, by name.
    pub device: String,
}

/// What this computer says when `asker` removed `device` here.
pub(crate) fn removal_words(removed: &Removed) -> String {
    format!(
        "{} removed {} from this computer",
        removed.asker, removed.device
    )
}

/// Remove `target` for `asker`, deciding again with `computers.json` held
/// under its own lock and then the trust store's (the order the link's
/// grant takes them), so neither a lapse nor a grant slips between: its
/// trust and name go, and its keys and contacts with them, as the Devices
/// page's Unpair takes them, before a pairing again could bind new keys.
pub(crate) fn remove_in(
    dir: &Path,
    trust: &Path,
    asker: &str,
    target: &str,
) -> Result<Removed, Refusal> {
    let decide = |file: &ComputersFile| {
        TrustStore::update(trust, |store| {
            if let Some(refusal) = refusal(store, file, asker, target) {
                return Ok(Err(refusal));
            }
            let removed = Removed {
                asker: asker_name(store, file, asker),
                device: shown_name(store, file, target),
            };
            store.untrust_device(target)?;
            // Its keys go too, so it pairs again only with a new code.
            if let Err(e) = pocketdesk_core::local::device_keys::clear(dir, target) {
                tracing::warn!("Could not clear a removed device's keys: {e}");
            }
            // A phone's contacts go with it (P10); its notifications, cards
            // and texts go as the watcher sees it unpaired (AY5).
            let _ = pocketdesk_core::local::phone_contacts::forget(dir, target);
            Ok(Ok(removed))
        })
    };
    // With no computers file there is nothing to hold, and none is made.
    let written = if dir
        .join(pocketdesk_core::local::paths::COMPUTERS_FILE)
        .exists()
    {
        let mut written = None;
        let held = computers::update(dir, |file| written = Some(decide(file)));
        match (held, written) {
            (Ok(()), Some(written)) => written,
            _ => {
                tracing::warn!("A removal from afar could not read computers.json");
                return Err(Refusal::Failed);
            }
        }
    } else {
        decide(&ComputersFile::default())
    };
    match written {
        Ok(outcome) => outcome,
        Err(e) => {
            tracing::warn!("A removal from afar could not write the trust store: {e}");
            Err(Refusal::Failed)
        }
    }
}

/// End what the removed `target` has going here now: its sessions, and its
/// control connections (told it is not paired as they close). A computer
/// paired both ways is forgotten by the links' loop, which sees it is no
/// longer trusted: told to forget, its nudge dropped, its view ended.
async fn end_what_it_has(target: &str) {
    end_what_it_has_with(target, pocketdesk_core::streaming::sessions::kick_device);
}

/// [`end_what_it_has`] with the sessions ended by `kick`; how many were.
fn end_what_it_has_with(
    target: &str,
    kick: impl Fn(&str) -> Vec<pocketdesk_core::streaming::sessions::LiveSession>,
) -> usize {
    let ended = kick(target);
    for session in &ended {
        tracing::info!(session = session.id, "Ended a removed device's session");
    }
    crate::state::close_device(target);
    ended.len()
}

/// Answer the ask in `body` from `asker`, the device on the connection or
/// link it came by; `None` for a body that does not read.
pub(crate) async fn answer(asker: &str, body: &Value) -> Option<Value> {
    answer_with(asker, body, crate::events::current_config().remote_revoke).await
}

/// [`answer`], with the owner's switch (`remote_revoke`) as `enabled`.
async fn answer_with(asker: &str, body: &Value, enabled: bool) -> Option<Value> {
    let request = Request::parse(body)?;
    let refused = |refusal| {
        Some(wire::refused_answer(
            &request.request,
            &request.ask,
            refusal,
        ))
    };
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        return refused(Refusal::Failed);
    };
    let read = {
        let (dir, trust) = (dir.clone(), trust.clone());
        tokio::task::spawn_blocking(move || {
            TrustStore::read(trust)
                .ok()
                .map(|store| (store, computers::read(&dir)))
        })
        .await
        .ok()
        .flatten()
    };
    // A store that does not read (one caught mid-replace) is a failure to
    // try again, never "not yours" (PR #574 review N10).
    let Some((store, file)) = read else {
        return refused(Refusal::Failed);
    };
    if !is_owner_device(&store, &file, asker) {
        tracing::warn!(
            asker,
            "Refused a device list or removal from a device not the owner's"
        );
        return refused(Refusal::NotOwner);
    }
    // The owner's switch, read at each ask (PR #574 review I1).
    if !enabled {
        tracing::info!(asker, "Refused a device list or removal: turned off here");
        return refused(Refusal::Off);
    }
    match request.ask {
        Ask::List => {
            let paced = LIST_PACE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take_up_to(asker, Instant::now(), LISTS_PER_MINUTE);
            if !paced {
                return refused(Refusal::Busy);
            }
            let connected = connected_now().await;
            let devices = listing(&store, &file, asker, |id| {
                connected.iter().any(|c| c == id) || crate::peer_computers::is_linked(id)
            });
            Some(wire::list_answer(&request.request, &devices))
        }
        Ask::Remove { device_id } => {
            let paced = PACE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(asker, Instant::now());
            let outcome = if paced {
                remove(&dir, &trust, asker, &device_id).await
            } else {
                Err(Refusal::TooMany)
            };
            Some(wire::remove_answer(&request.request, &device_id, outcome))
        }
    }
}

/// Remove `target` for `asker` ([`remove_in`]), then end what it has here,
/// say so and write the event.
async fn remove(dir: &Path, trust: &Path, asker: &str, target: &str) -> Result<(), Refusal> {
    let outcome = {
        let (dir, trust) = (dir.to_path_buf(), trust.to_path_buf());
        let (asker, target) = (asker.to_string(), target.to_string());
        tokio::task::spawn_blocking(move || remove_in(&dir, &trust, &asker, &target))
            .await
            .unwrap_or(Err(Refusal::Failed))
    };
    let removed = match outcome {
        Ok(removed) => removed,
        Err(refusal) => {
            tracing::info!(
                asker,
                target,
                reason = refusal.code(),
                "A removal from afar was refused"
            );
            return Err(refusal);
        }
    };
    tracing::warn!(asker, target, "{}", removal_words(&removed));
    end_what_it_has(target).await;
    crate::events::emit(
        "device_removed",
        Some(target),
        serde_json::json!({ "by": asker }),
        serde_json::json!({}),
    );
    if !cfg!(test) {
        let words = removal_words(&removed);
        tokio::task::spawn_blocking(move || {
            crate::platform::notify("A device was removed", &words, Duration::from_secs(30))
        });
    }
    Ok(())
}

/// The devices connected here now by a control connection or a session
/// (a link this computer keeps is asked of the links, in [`answer`]).
async fn connected_now() -> Vec<String> {
    let mut ids: Vec<String> = crate::state::clone_clients()
        .await
        .iter()
        .map(|c| c.device_id().to_string())
        .collect();
    ids.extend(
        pocketdesk_core::streaming::sessions::list()
            .into_iter()
            .filter_map(|s| s.device_id),
    );
    ids
}

/// The plugin taking an owner device's ask on a connection here, and
/// another computer's answer to this one's ask.
pub struct RemoteDevicesPlugin;

#[async_trait::async_trait]
impl Plugin for RemoteDevicesPlugin {
    fn name(&self) -> &'static str {
        "remote_devices"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[REMOTE_DEVICES, REMOTE_DEVICES_ANSWER]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[REMOTE_DEVICES, REMOTE_DEVICES_ANSWER]
    }

    async fn handle_packet(
        &self,
        packet: &NetworkPacket,
        sender: &dyn DeviceSender,
    ) -> pocketdesk_core::error::Result<()> {
        if packet.packet_type == REMOTE_DEVICES {
            if let Some(body) = answer(sender.device_id(), &packet.body).await {
                sender
                    .send_packet(&NetworkPacket::new(REMOTE_DEVICES_ANSWER).with_body(body))
                    .await?;
            }
        } else if packet.packet_type == REMOTE_DEVICES_ANSWER {
            heard(sender.device_id(), &packet.body).await;
        }
        Ok(())
    }
}

/// Ask the computer `computer` (one of the owner's, paired both ways) for
/// its devices, or to remove one, for the desktop app: recorded in
/// `computers.json` as waiting, then sent on the link between the two,
/// whichever of them keeps it.
pub(crate) async fn ask(dir: &Path, computer: &str, ask: Ask) {
    let request = Request {
        request: uuid::Uuid::new_v4().simple().to_string(),
        ask,
    };
    let now = pocketdesk_core::local::pin::now_unix();
    let recorded = {
        let (dir, computer, request) = (dir.to_path_buf(), computer.to_string(), request.clone());
        tokio::task::spawn_blocking(move || {
            computers::update(&dir, |file| record_ask(file, &computer, &request, now))
        })
        .await
    };
    if !matches!(recorded, Ok(Ok(()))) {
        tracing::warn!("Could not record an ask of another computer's devices");
        return;
    }
    let packet = NetworkPacket::new(REMOTE_DEVICES).with_body(request.body());
    if !send_to_computer(computer, packet).await {
        let (dir, computer, id) = (dir.to_path_buf(), computer.to_string(), request.request);
        let _ = tokio::task::spawn_blocking(move || {
            computers::update(&dir, |file| not_sent(file, &computer, &id))
        })
        .await;
    }
}

/// Record in `file` that `request` was asked of `computer` at `now`: a
/// list keeps what that computer listed before, until the answer.
fn record_ask(file: &mut ComputersFile, computer: &str, request: &Request, now: u64) {
    let mut there = file
        .there
        .take()
        .filter(|t| t.device_id == computer)
        .unwrap_or_else(|| DevicesThere {
            device_id: computer.to_string(),
            ..Default::default()
        });
    there.request = request.request.clone();
    there.asked_unix = now;
    there.refused = None;
    match &request.ask {
        Ask::List => there.removing = None,
        Ask::Remove { device_id } => {
            there.removing = Some(device_id.clone());
            there.outcome = None;
        }
    }
    file.there = Some(there);
}

/// Forget what `computer` listed, its "Devices there…" closed (PR #574
/// review N12): the names of its devices are not kept past that.
pub(crate) fn close_there(dir: &Path, computer: &str) {
    let saved = computers::update(dir, |file| {
        if file.there.as_ref().is_some_and(|t| t.device_id == computer) {
            file.there = None;
        }
    });
    if let Err(e) = saved {
        tracing::debug!("Could not forget another computer's devices: {e}");
    }
}

/// The ask `request` of `computer` could not be sent: no link is up.
fn not_sent(file: &mut ComputersFile, computer: &str, request: &str) {
    if let Some(there) = file
        .there
        .as_mut()
        .filter(|t| t.device_id == computer && t.request == request)
    {
        there.request.clear();
        there.removing = None;
        there.refused = Some("not_linked".to_string());
    }
}

/// Send `packet` to the computer `computer`: on the link this computer
/// keeps to it, else on its connection here (the link it keeps). Whether
/// it went.
async fn send_to_computer(computer: &str, packet: NetworkPacket) -> bool {
    if let Some(link) = crate::peer_computers::live_link(computer) {
        link.send(packet);
        return true;
    }
    let clients = crate::state::clone_clients().await;
    match clients.iter().find(|c| c.device_id() == computer) {
        Some(client) => client.send_packet(&packet).await.is_ok(),
        None => false,
    }
}

/// Take another computer's answer: only from the computer asked, and only
/// to the ask waiting there now (its random request id).
pub(crate) async fn heard(from: &str, body: &Value) {
    let Some((request, answer)) = wire::read_answer(body) else {
        return;
    };
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    let from = from.to_string();
    let now = pocketdesk_core::local::pin::now_unix();
    let _ = tokio::task::spawn_blocking(move || {
        computers::update(&dir, |file| take_answer(file, &from, &request, answer, now))
    })
    .await;
}

/// [`heard`] on `file`.
fn take_answer(file: &mut ComputersFile, from: &str, request: &str, answer: Answer, now: u64) {
    let Some(there) = file
        .there
        .as_mut()
        .filter(|t| t.device_id == from && !t.request.is_empty() && t.request == request)
    else {
        tracing::debug!("An answer about devices that no ask here waits for; dropped");
        return;
    };
    // The answer must be to the kind of ask waiting, and a removal's to
    // the device asked (PR #574 review N7).
    let fits = match (&answer, there.removing.as_deref()) {
        (Answer::Refused(_), _) | (Answer::Listed(_), None) => true,
        (Answer::Removed { device_id, .. }, Some(asked)) => device_id == asked,
        _ => false,
    };
    if !fits {
        tracing::debug!("An answer about devices that does not fit the ask waiting; dropped");
        return;
    }
    there.request.clear();
    match answer {
        Answer::Listed(devices) => {
            there.devices = devices;
            there.listed_unix = now;
        }
        Answer::Removed { device_id, outcome } => {
            there.removing = None;
            there.outcome = Some(match outcome {
                Ok(()) => {
                    let name = there
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .map(|d| d.name.clone())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| device_id.chars().take(8).collect());
                    there.devices.retain(|d| d.id != device_id);
                    format!("removed: {name}")
                }
                Err(refusal) => format!("refused: {}", refusal.code()),
            });
        }
        Answer::Refused(refusal) => {
            there.removing = None;
            there.refused = Some(refusal.code().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::computers::PeerComputer;

    struct Dirs {
        dir: std::path::PathBuf,
    }

    impl Dirs {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pd-revoke-{name}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn trust(&self) -> std::path::PathBuf {
            self.dir.join("trusted_devices.json")
        }

        fn store(&self) -> TrustStore {
            TrustStore::read(self.trust()).unwrap()
        }

        /// A computer listed here, paired both ways (`mine`).
        fn list_computer(&self, id: &str, mine: bool) {
            computers::update(&self.dir, |file| {
                file.computers.push(PeerComputer {
                    device_id: id.to_string(),
                    name: format!("Computer {id}"),
                    addresses: vec!["192.0.2.10".to_string()],
                    control_port: 1716,
                    stream_port: 4716,
                    fingerprint: "ab".repeat(32),
                    endpoint: None,
                    paired_unix: 1,
                    link: String::new(),
                    link_unix: 0,
                    lets_drive: false,
                    mine,
                    wake: None,
                    viewed_unix: 0,
                });
            })
            .unwrap();
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The owner's phone, someone else's, a guest, an owner computer paired
    /// both ways, one whose grant waits, one lapsed, and one paired one way.
    fn household(d: &Dirs) {
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("owner-phone", true)?;
            s.trust_phone("someone-else", false)?;
            s.trust_device_with_ttl("guest", Duration::from_secs(600))?;
            s.trust_computer("mine-pc")?;
            s.trust_computer("pending-pc")?;
            s.trust_computer("lapsed-pc")?;
            s.trust_computer("one-way-pc")?;
            Ok(())
        })
        .unwrap();
        d.list_computer("mine-pc", true);
        d.list_computer("pending-pc", true);
        d.list_computer("lapsed-pc", true);
        computers::update(&d.dir, |file| {
            file.pending.push("pending-pc".to_string());
            file.lapsed.push("lapsed-pc".to_string());
        })
        .unwrap();
    }

    #[test]
    fn only_the_owner_phone_and_a_computer_paired_both_ways_are_owner_devices() {
        let d = Dirs::new("gate");
        household(&d);
        let (store, file) = (d.store(), computers::read(&d.dir));
        for (id, owner) in [
            ("owner-phone", true),
            ("mine-pc", true),
            ("someone-else", false),
            ("guest", false),
            ("pending-pc", false),
            ("lapsed-pc", false),
            ("one-way-pc", false),
            ("stranger", false),
        ] {
            assert_eq!(is_owner_device(&store, &file, id), owner, "{id}");
        }
    }

    #[test]
    fn a_guest_grant_on_an_id_is_never_an_owner_phone() {
        let d = Dirs::new("guest-mark");
        // A mark left on an id that is only a guest's is not read.
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("p", true)?;
            s.untrust_device("p")?;
            s.trust_device_with_ttl("p", Duration::from_secs(60))
        })
        .unwrap();
        assert!(!d.store().is_owner_phone("p"));
        // Paired again as someone else's, an owner phone is one no more.
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("q", true)?;
            s.trust_phone("q", false)
        })
        .unwrap();
        assert!(!d.store().is_owner_phone("q"));
        // Unpaired, its mark goes with it.
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("r", true)?;
            s.untrust_device("r")?;
            s.trust_device("r")
        })
        .unwrap();
        assert!(!d.store().is_owner_phone("r"));
    }

    #[test]
    fn a_device_not_the_owners_cannot_remove_anything() {
        let d = Dirs::new("refused");
        household(&d);
        for asker in [
            "someone-else",
            "guest",
            "pending-pc",
            "lapsed-pc",
            "one-way-pc",
            "stranger",
        ] {
            assert_eq!(
                remove_in(&d.dir, &d.trust(), asker, "owner-phone"),
                Err(Refusal::NotOwner),
                "{asker}"
            );
        }
        assert!(d.store().is_trusted("owner-phone"), "nothing removed");
    }

    #[test]
    fn an_owner_device_removes_another_and_not_itself() {
        let d = Dirs::new("remove");
        household(&d);
        pocketdesk_core::local::device_keys::register(
            &d.dir,
            "someone-else",
            pocketdesk_core::local::device_keys::Slot::Tls,
            &"cd".repeat(32),
        )
        .unwrap();
        assert_eq!(
            remove_in(&d.dir, &d.trust(), "owner-phone", "owner-phone"),
            Err(Refusal::Yourself)
        );
        assert_eq!(
            remove_in(&d.dir, &d.trust(), "owner-phone", "nobody"),
            Err(Refusal::NotPaired)
        );
        let removed = remove_in(&d.dir, &d.trust(), "mine-pc", "someone-else").unwrap();
        assert_eq!(
            removal_words(&removed),
            "Computer mine-pc (computer mine-pc) removed someone- from this computer"
        );
        let store = d.store();
        assert!(!store.is_trusted("someone-else"));
        assert!(
            !pocketdesk_core::local::device_keys::read(&d.dir)
                .unwrap()
                .contains_key("someone-else"),
            "its keys go"
        );
        // The phone removes a computer paired both ways, and the lost
        // computer goes; the links' loop forgets its listing.
        remove_in(&d.dir, &d.trust(), "owner-phone", "mine-pc").unwrap();
        assert!(!d.store().is_trusted("mine-pc"));
    }

    /// What this test checks (PR #574 review N1): two owner devices removing
    /// each other leave one, because the second asker is no longer an owner
    /// device; and the last-owner count itself, called directly, says when
    /// none would be left. `refusal` never reaches it with a live asker.
    #[test]
    fn mutual_removal_leaves_one_owner_and_the_count_sees_the_last() {
        let d = Dirs::new("last");
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("phone", true)?;
            s.trust_computer("pc")
        })
        .unwrap();
        d.list_computer("pc", true);
        let (store, file) = (d.store(), computers::read(&d.dir));
        assert!(!leaves_no_owner(&store, &file, "phone"));
        assert!(!leaves_no_owner(&store, &file, "pc"));
        // Two owner devices removing each other at once: the first wins,
        // and the second, no longer an owner device, removes nothing.
        assert!(remove_in(&d.dir, &d.trust(), "phone", "pc").is_ok());
        assert_eq!(
            remove_in(&d.dir, &d.trust(), "pc", "phone"),
            Err(Refusal::NotOwner)
        );
        // The phone is now the only owner device: the count says removing
        // it would leave none.
        let (store, file) = (d.store(), computers::read(&d.dir));
        assert!(leaves_no_owner(&store, &file, "phone"));
        assert!(store.is_owner_phone("phone"), "one owner device is left");
        assert!(!store.is_trusted("pc"));
    }

    /// N3: the asker is named by what this computer knows of it, never the
    /// name it gives itself, with its kind and the start of its id.
    #[test]
    fn the_asker_is_named_by_this_computer_not_by_itself() {
        let d = Dirs::new("asker-name");
        TrustStore::update(d.trust(), |s| {
            s.trust_phone("a1b2c3d4e5", true)?;
            s.note_claimed_name("a1b2c3d4e5", "Desk laptop")?;
            Ok(())
        })
        .unwrap();
        let file = computers::read(&d.dir);
        assert_eq!(
            asker_name(&d.store(), &file, "a1b2c3d4e5"),
            "A phone (a1b2c3d4)",
            "its own name is not taken"
        );
        TrustStore::update(d.trust(), |s| s.rename("a1b2c3d4e5", Some("Pixel 8"))).unwrap();
        assert_eq!(
            asker_name(&d.store(), &file, "a1b2c3d4e5"),
            "Pixel 8 (phone a1b2c3d4)"
        );
    }

    /// I1: with "Let my devices remove others from afar" off, an owner
    /// device's list and removal are both refused as off, and nothing goes.
    #[tokio::test]
    async fn turned_off_an_owner_device_lists_and_removes_nothing() {
        let dir = crate::state::use_test_dirs();
        let trust = dir.join("trusted_devices.json");
        TrustStore::update(&trust, |s| {
            s.trust_phone("off-owner", true)?;
            s.trust_phone("off-other", true)
        })
        .unwrap();
        for ask in [
            Ask::List,
            Ask::Remove {
                device_id: "off-other".to_string(),
            },
        ] {
            let request = Request {
                request: "off".to_string(),
                ask,
            };
            let answered = answer_with("off-owner", &request.body(), false)
                .await
                .unwrap();
            assert_eq!(answered["refused"], "off", "{answered}");
            assert!(answered.get("devices").is_none());
        }
        assert!(TrustStore::read(&trust).unwrap().is_trusted("off-other"));
        TrustStore::update(&trust, |s| {
            s.untrust_device("off-owner")?;
            s.untrust_device("off-other")
        })
        .unwrap();
    }

    /// N6: lists are paced too, more loosely than removals.
    #[test]
    fn lists_are_paced_per_asker() {
        let mut pace = Pace::default();
        let start = Instant::now();
        for _ in 0..LISTS_PER_MINUTE {
            assert!(pace.take_up_to("a", start, LISTS_PER_MINUTE));
        }
        assert!(!pace.take_up_to("a", start, LISTS_PER_MINUTE));
        assert!(pace.take_up_to("b", start, LISTS_PER_MINUTE));
    }

    /// N7: an answer of another kind, or about another device, is not
    /// taken for the ask waiting.
    #[test]
    fn an_answer_that_does_not_fit_the_ask_waiting_is_dropped() {
        let mut file = ComputersFile::default();
        let remove = Request {
            request: "w".to_string(),
            ask: Ask::Remove {
                device_id: "p".to_string(),
            },
        };
        record_ask(&mut file, "pc", &remove, 1);
        take_answer(&mut file, "pc", "w", Answer::Listed(vec![]), 2);
        take_answer(
            &mut file,
            "pc",
            "w",
            Answer::Removed {
                device_id: "q".to_string(),
                outcome: Ok(()),
            },
            2,
        );
        let there = file.there.clone().unwrap();
        assert_eq!(there.request, "w", "still waiting");
        assert_eq!(there.removing.as_deref(), Some("p"));
        let list = Request {
            request: "l".to_string(),
            ask: Ask::List,
        };
        record_ask(&mut file, "pc", &list, 3);
        take_answer(
            &mut file,
            "pc",
            "l",
            Answer::Removed {
                device_id: "p".to_string(),
                outcome: Ok(()),
            },
            4,
        );
        assert_eq!(file.there.as_ref().unwrap().request, "l");
        // Closed, its list goes (N12).
        let d = Dirs::new("close");
        computers::update(&d.dir, |f| *f = file.clone()).unwrap();
        close_there(&d.dir, "other");
        assert!(computers::read(&d.dir).there.is_some(), "another's stays");
        close_there(&d.dir, "pc");
        assert!(computers::read(&d.dir).there.is_none());
    }

    #[test]
    fn at_most_five_removals_a_minute_per_asker() {
        let mut pace = Pace::default();
        let start = Instant::now();
        for _ in 0..REMOVALS_PER_MINUTE {
            assert!(pace.take("a", start));
        }
        assert!(
            !pace.take("a", start + Duration::from_secs(30)),
            "the sixth"
        );
        assert!(pace.take("b", start), "each asker its own");
        assert!(
            pace.take("a", start + Duration::from_secs(61)),
            "a minute later"
        );
    }

    #[test]
    fn the_list_says_no_secret() {
        let d = Dirs::new("list");
        household(&d);
        pocketdesk_core::local::device_keys::register(
            &d.dir,
            "owner-phone",
            pocketdesk_core::local::device_keys::Slot::Tls,
            &"ef".repeat(32),
        )
        .unwrap();
        TrustStore::update(d.trust(), |s| s.rename("owner-phone", Some("My phone"))).unwrap();
        let devices = listing(&d.store(), &computers::read(&d.dir), "owner-phone", |id| {
            id == "mine-pc"
        });
        let phone = devices.iter().find(|x| x.id == "owner-phone").unwrap();
        assert!(phone.you && phone.owner && !phone.computer && phone.name == "My phone");
        let pc = devices.iter().find(|x| x.id == "mine-pc").unwrap();
        assert!(pc.connected && pc.computer && pc.owner && pc.name == "Computer mine-pc");
        assert!(devices.iter().find(|x| x.id == "guest").unwrap().guest);
        let wire = serde_json::to_string(&wire::list_answer("r", &devices)).unwrap();
        for secret in [
            "efef",
            "abab",
            "192.0.2.10",
            "fingerprint",
            "permissions",
            "1716",
        ] {
            assert!(!wire.contains(secret), "{secret} in {wire}");
        }
    }

    /// A non-owner forging the packet: it is answered as refused, on a
    /// connection as on a link, and nothing changes.
    #[tokio::test]
    async fn a_forged_ask_from_a_device_not_the_owners_is_refused() {
        let dir = crate::state::use_test_dirs();
        let trust = dir.join("trusted_devices.json");
        TrustStore::update(&trust, |s| {
            s.trust_phone("forge-owner", true)?;
            s.trust_phone("forge-other", false)
        })
        .unwrap();
        let ask = Request {
            request: "r1".to_string(),
            ask: Ask::Remove {
                device_id: "forge-owner".to_string(),
            },
        };
        let answered = answer("forge-other", &ask.body()).await.unwrap();
        assert_eq!(
            wire::read_answer(&answered),
            Some(("r1".to_string(), Answer::Refused(Refusal::NotOwner)))
        );
        let list = Request {
            request: "r2".to_string(),
            ask: Ask::List,
        };
        let answered = answer("forge-other", &list.body()).await.unwrap();
        assert!(answered.get("devices").is_none(), "no list for it");
        assert!(TrustStore::read(&trust).unwrap().is_trusted("forge-owner"));
        // Through the plugin, as on a connection.
        let sender = Recorder::new("forge-other");
        let packet = NetworkPacket::new(REMOTE_DEVICES).with_body(ask.body());
        RemoteDevicesPlugin
            .handle_packet(&packet, &sender)
            .await
            .unwrap();
        let sent = sender.sent.lock().unwrap();
        assert_eq!(sent[0].packet_type, REMOTE_DEVICES_ANSWER);
        assert_eq!(sent[0].body["refused"], "not_owner");
        assert!(TrustStore::read(&trust).unwrap().is_trusted("forge-owner"));
        TrustStore::update(&trust, |s| {
            s.untrust_device("forge-owner")?;
            s.untrust_device("forge-other")
        })
        .unwrap();
    }

    /// An owner phone's removal ends the removed device's connections.
    #[tokio::test]
    async fn a_removal_closes_the_removed_device_s_connections() {
        let dir = crate::state::use_test_dirs();
        let trust = dir.join("trusted_devices.json");
        TrustStore::update(&trust, |s| {
            s.trust_phone("ends-owner", true)?;
            s.trust_phone("ends-lost", true)
        })
        .unwrap();
        let mut closing = crate::state::closing();
        let ask = Request {
            request: "r3".to_string(),
            ask: Ask::Remove {
                device_id: "ends-lost".to_string(),
            },
        };
        let answered = answer("ends-owner", &ask.body()).await.unwrap();
        assert_eq!(answered["removed"], true, "{answered}");
        assert!(!TrustStore::read(&trust).unwrap().is_trusted("ends-lost"));
        tokio::time::timeout(
            Duration::from_secs(2),
            crate::state::closed("ends-lost", &mut closing, || false),
        )
        .await
        .expect("its connections are told to close");
        // Its sessions: the removal kicks the removed device's own, by its
        // exact id (core's `kick_device` closes each one's connection).
        let session = pocketdesk_core::streaming::sessions::LiveSession {
            streamed: Default::default(),
            id: 9,
            device_id: Some("ends-lost".to_string()),
            peer: "192.0.2.7:4433".parse().unwrap(),
            lan: true,
            started_unix: 1,
        };
        let asked = std::sync::Mutex::new(Vec::new());
        let ended = end_what_it_has_with("ends-lost", |id| {
            asked.lock().unwrap().push(id.to_string());
            vec![session.clone()]
        });
        assert_eq!(ended, 1);
        assert_eq!(*asked.lock().unwrap(), vec!["ends-lost".to_string()]);
        TrustStore::update(&trust, |s| s.untrust_device("ends-owner")).unwrap();
    }

    #[test]
    fn an_answer_is_taken_only_from_the_computer_asked_for_the_ask_waiting() {
        let mut file = ComputersFile::default();
        let request = Request {
            request: "wait-1".to_string(),
            ask: Ask::List,
        };
        record_ask(&mut file, "pc-a", &request, 10);
        let device = ThereDevice {
            id: "p".to_string(),
            name: "Lost phone".to_string(),
            computer: false,
            owner: true,
            guest: false,
            connected: false,
            you: false,
        };
        // Another device, or another request id: dropped.
        take_answer(
            &mut file,
            "pc-b",
            "wait-1",
            Answer::Listed(vec![device.clone()]),
            11,
        );
        take_answer(
            &mut file,
            "pc-a",
            "other",
            Answer::Listed(vec![device.clone()]),
            11,
        );
        assert!(file.there.as_ref().unwrap().devices.is_empty());
        take_answer(
            &mut file,
            "pc-a",
            "wait-1",
            Answer::Listed(vec![device]),
            12,
        );
        let there = file.there.clone().unwrap();
        assert_eq!((there.devices.len(), there.listed_unix), (1, 12));
        assert!(there.request.is_empty(), "answered");
        // Asked again once answered: a second answer is not taken.
        take_answer(&mut file, "pc-a", "wait-1", Answer::Listed(vec![]), 13);
        assert_eq!(file.there.as_ref().unwrap().devices.len(), 1);
        // A removal, answered.
        let remove = Request {
            request: "wait-2".to_string(),
            ask: Ask::Remove {
                device_id: "p".to_string(),
            },
        };
        record_ask(&mut file, "pc-a", &remove, 20);
        assert_eq!(file.there.as_ref().unwrap().removing.as_deref(), Some("p"));
        take_answer(
            &mut file,
            "pc-a",
            "wait-2",
            Answer::Removed {
                device_id: "p".to_string(),
                outcome: Ok(()),
            },
            21,
        );
        let there = file.there.clone().unwrap();
        assert_eq!(there.outcome.as_deref(), Some("removed: Lost phone"));
        assert!(there.devices.is_empty() && there.removing.is_none());
        // Not sent: said so.
        let again = Request {
            request: "wait-3".to_string(),
            ask: Ask::List,
        };
        record_ask(&mut file, "pc-a", &again, 30);
        not_sent(&mut file, "pc-a", "wait-3");
        assert_eq!(
            file.there.as_ref().unwrap().refused.as_deref(),
            Some("not_linked")
        );
    }

    /// A sender that keeps what it is sent.
    struct Recorder {
        id: String,
        sent: std::sync::Mutex<Vec<NetworkPacket>>,
    }

    impl Recorder {
        fn new(id: &str) -> Self {
            Self {
                id: id.to_string(),
                sent: Default::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl DeviceSender for Recorder {
        fn device_id(&self) -> &str {
            &self.id
        }

        fn connection_id(&self) -> &str {
            "recorder"
        }

        async fn send_packet(&self, packet: &NetworkPacket) -> pocketdesk_core::error::Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }
}
