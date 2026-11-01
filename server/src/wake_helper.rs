//! A spare phone at home as this computer's wake helper (`core::protocol::
//! wake_helper` has the wire and the rules).
//!
//! **Who may be the helper:** an owner phone, one paired with this
//! computer's own code (never a guest's, someone else's, a time-limited one
//! or a computer), that offers itself ([`OFFER`]). The key kept for it is the
//! helper key the phone named; one helper at most, kept in
//! `<state>/wake_helper.json`, where the desktop app shows it and lets the
//! owner remove it. A phone the owner removed is refused until its owner
//! turns the switch off and on again on the phone.
//!
//! **What the computer tells, while awake** (checked every 15 seconds and at
//! once when an offer changes something, on each phone's newest connection,
//! only when it changed since that connection was last told):
//!
//! - the helper phone, its [`DUTY`]: this computer's own id, its MAC and the
//!   broadcast address of the network the helper is on with it (found from
//!   the helper's own address, never from anything it said), and the iroh
//!   keys of the owner phones that may ask (every owner phone that has
//!   registered one, but the helper itself). A phone that is no longer an
//!   owner phone drops out of that list at the next tick.
//! - each owner phone, an [`INFO`]: this computer's id and where the helper
//!   is (its key, relays and name), or that it has none.
//!
//! A helper that stops being an owner phone (unpaired, demoted) stops being
//! one here at the next tick.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::local::computers::{self, ComputersFile};
use pocketdesk_core::local::device_keys;
use pocketdesk_core::local::wake::{Adapter, facts_for_peer};
use pocketdesk_core::local::wake_helper::{self as store, Taken};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
pub use pocketdesk_core::protocol::wake_helper::{DUTY, INFO, OFFER};
use pocketdesk_core::protocol::wake_helper::{
    Duty, DutyMessage, HelperInfo, Info, Offer, parse_mac,
};
use serde_json::Value;

/// How often what each phone was told is checked against what is true now.
const TICK: Duration = Duration::from_secs(15);

/// What each phone was told is told again this often even when it did not
/// change: the helper drops a duty it has not heard for a week, and a
/// connection can outlive that.
const REFRESH: Duration = Duration::from_secs(60 * 60);

/// At most this many offers a minute are taken from one phone: it offers on
/// start, on a change and every few minutes, so more is not an offer.
const OFFERS_PER_MINUTE: usize = 6;

/// Woken when an offer changed something, so the phones are told at once.
static NUDGE: tokio::sync::Notify = tokio::sync::Notify::const_new();

static PACE: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(Default::default);

/// Whether `device` may make one more offer at `now`; counted if so.
fn pace_allows(device: &str, now: Instant) -> bool {
    let mut pace = PACE.lock().unwrap_or_else(|e| e.into_inner());
    pace.retain(|_, at| {
        at.last()
            .is_some_and(|t| now.duration_since(*t) < Duration::from_secs(60))
    });
    let at = pace.entry(device.to_string()).or_default();
    at.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    if at.len() >= OFFERS_PER_MINUTE {
        return false;
    }
    at.push(now);
    true
}

/// What the rules need of the stores, read once.
#[derive(Debug, Default, Clone)]
pub(crate) struct Snapshot {
    /// This computer's own device id.
    pub me: String,
    /// The phone held as helper, still an owner phone.
    pub helper: Option<store::Helper>,
    /// The name this computer shows the helper by.
    pub helper_name: String,
    /// The owner phones paired here.
    pub owners: HashSet<String>,
    /// Their iroh keys, for those that registered one.
    pub keys: HashMap<String, String>,
    /// Phones the owner removed as helper.
    pub refused: HashSet<String>,
}

/// Read the stores. A helper that is no longer an owner phone is forgotten
/// here (and so is the removal of a phone no longer paired); `None` when the
/// trust store does not read (one caught mid-replace: try again).
pub(crate) fn snapshot(dir: &std::path::Path, trust: &std::path::Path) -> Option<Snapshot> {
    let store_file = TrustStore::read(trust).ok()?;
    let file = computers::read(dir);
    let me = pocketdesk_core::local::identity::device_id(dir)?;
    let mut held = store::read(dir);
    let owner = |id: &str| crate::introductions::is_owner_phone(&store_file, &file, id);
    if let Some(helper) = &held.helper
        && !owner(&helper.device_id)
    {
        let _ = store::forget_device(dir, &helper.device_id.clone());
        held.helper = None;
    }
    // A removal of a phone that is not paired any more is forgotten.
    for id in held.refused.clone() {
        if !store_file.is_trusted(&id) {
            let _ = store::forget_device(dir, &id);
            held.refused.retain(|d| *d != id);
        }
    }
    let owners: HashSet<String> = store_file
        .paired_devices()
        .into_iter()
        .filter(|id| owner(id))
        .collect();
    let keys = device_keys::read(dir)
        .map(|all| {
            all.into_iter()
                .filter(|(id, _)| owners.contains(id))
                .filter_map(|(id, k)| k.iroh.map(|iroh| (id, iroh)))
                .collect()
        })
        .unwrap_or_default();
    let helper_name = held
        .helper
        .as_ref()
        .map(|h| crate::introductions::shown(&store_file, &file, &h.device_id))
        .unwrap_or_default();
    Some(Snapshot {
        me,
        helper: held.helper,
        helper_name,
        owners,
        keys,
        refused: held.refused.into_iter().collect(),
    })
}

/// The iroh keys that may ask the helper `helper`: each owner phone that
/// registered one, except the helper itself, sorted.
pub(crate) fn requesters(snap: &Snapshot, helper: &str) -> Vec<String> {
    let mut keys: Vec<String> = snap
        .keys
        .iter()
        .filter(|(id, _)| id.as_str() != helper && snap.owners.contains(*id))
        .map(|(_, key)| pocketdesk_core::protocol::wake_helper::key_form(key))
        .filter(|key| pocketdesk_core::protocol::wake_helper::valid_key(key))
        .take(pocketdesk_core::protocol::wake_helper::MAX_REQUESTERS)
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// What `device` on a connection from `peer` is to be told now, if
/// anything. `adapters` is read lazily: only a helper's duty needs it.
pub(crate) fn packet_for(
    snap: &Snapshot,
    device: &str,
    peer: Option<IpAddr>,
    adapters: &dyn Fn() -> Vec<Adapter>,
) -> Option<NetworkPacket> {
    if snap.refused.contains(device) && snap.owners.contains(device) {
        let refused = DutyMessage::Refused {
            computer: snap.me.clone(),
        };
        return Some(NetworkPacket::new(DUTY).with_body(refused.body()));
    }
    let helper = snap.helper.as_ref();
    if helper.is_some_and(|h| h.device_id == device) {
        let helper = helper?;
        // No duty can be worked out (the helper is on none of this
        // computer's networks, or only a tunnel): it is told so, so what it
        // held, requesters included, is dropped rather than left to serve
        // a phone that was revoked meanwhile.
        let withdrawn = || {
            let message = DutyMessage::Withdrawn {
                computer: snap.me.clone(),
            };
            Some(NetworkPacket::new(DUTY).with_body(message.body()))
        };
        let peer = match peer {
            Some(IpAddr::V4(v4)) => Some(v4),
            Some(IpAddr::V6(v6)) => v6.to_ipv4_mapped(),
            None => None,
        };
        let Some(peer) = peer else {
            return withdrawn();
        };
        let Some(facts) = facts_for_peer(&adapters(), peer) else {
            return withdrawn();
        };
        let Some(mac) = parse_mac(&facts.mac) else {
            return withdrawn();
        };
        let duty = Duty {
            computer: snap.me.clone(),
            mac,
            broadcast: facts.broadcast,
            requesters: requesters(snap, &helper.device_id),
        };
        return Some(NetworkPacket::new(DUTY).with_body(DutyMessage::Duty(duty).body()));
    }
    if snap.owners.contains(device) {
        let info = Info {
            computer: snap.me.clone(),
            helper: helper.map(|h| HelperInfo {
                iroh: h.iroh.clone(),
                relays: h.relays.clone(),
                name: snap.helper_name.clone(),
            }),
        };
        return Some(NetworkPacket::new(INFO).with_body(info.body()));
    }
    None
}

fn hash_of(body: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.to_string().hash(&mut hasher);
    hasher.finish()
}

fn peer_ip(sender: &dyn DeviceSender) -> Option<IpAddr> {
    sender.peer_addr().map(|a| a.ip())
}

/// Daemon task: keep each owner phone, and the helper, told what is true.
pub async fn run_watcher() {
    let mut told: HashMap<String, (u64, Instant)> = HashMap::new();
    loop {
        tokio::select! {
            () = NUDGE.notified() => {}
            () = tokio::time::sleep(TICK) => {}
        }
        tick(&mut told).await;
    }
}

/// One pass: tell each connection what changed since it was last told.
pub(crate) async fn tick(told: &mut HashMap<String, (u64, Instant)>) {
    let clients = crate::state::clone_clients().await;
    let live: HashSet<String> = clients
        .iter()
        .map(|c| c.connection_id().to_string())
        .collect();
    told.retain(|id, _| live.contains(id));
    if clients.is_empty() {
        return;
    }
    let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path()) else {
        return;
    };
    let Some(snap) = tokio::task::spawn_blocking(move || snapshot(&dir, &trust))
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    // Adapters are listed once per pass, and only if the helper is here.
    let mut adapters: Option<Vec<Adapter>> = None;
    for client in &clients {
        let helper_here = snap
            .helper
            .as_ref()
            .is_some_and(|h| h.device_id == client.device_id());
        if helper_here && adapters.is_none() {
            adapters = tokio::task::spawn_blocking(pocketdesk_core::local::wake::adapters)
                .await
                .ok();
        }
        let listed = adapters.clone().unwrap_or_default();
        let Some(packet) = packet_for(&snap, client.device_id(), peer_ip(&**client), &|| {
            listed.clone()
        }) else {
            continue;
        };
        let hash = hash_of(&packet.body);
        let now = Instant::now();
        if told
            .get(client.connection_id())
            .is_some_and(|(told_hash, at)| *told_hash == hash && now.duration_since(*at) < REFRESH)
        {
            continue;
        }
        if client.send_packet(&packet).await.is_ok() {
            told.insert(client.connection_id().to_string(), (hash, now));
        }
    }
}

/// What came of an owner phone's offer, for what is done about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    NotOwner,
    Taken {
        taken: Taken,
        /// The held helper is a different phone or key than before.
        changed: bool,
        name: String,
    },
    Failed,
}

/// Take the offer in `body` from `device`, under the rules: an owner phone
/// only, at most [`OFFERS_PER_MINUTE`] a minute. `None` for a body that does
/// not read, or a phone past its pace: no answer is made to either.
pub(crate) fn take_in(
    dir: &std::path::Path,
    trust: &std::path::Path,
    device: &str,
    body: &Value,
    now: u64,
) -> Option<Outcome> {
    let offer = Offer::parse(body)?;
    let Ok(store_file) = TrustStore::read(trust) else {
        return Some(Outcome::Failed);
    };
    let file: ComputersFile = computers::read(dir);
    if !crate::introductions::is_owner_phone(&store_file, &file, device) {
        return Some(Outcome::NotOwner);
    }
    let key = |h: Option<store::Helper>| h.map(|h| (h.device_id, h.iroh, h.relays));
    let before = key(store::read(dir).helper);
    let Ok(taken) = store::take_offer(dir, device, &offer, now) else {
        return Some(Outcome::Failed);
    };
    let after = key(store::read(dir).helper);
    Some(Outcome::Taken {
        taken,
        changed: before != after && after.is_some(),
        name: crate::introductions::shown(&store_file, &file, device),
    })
}

/// The plugin taking an owner phone's offer.
#[derive(Debug, Default)]
pub struct WakeHelperPlugin;

#[async_trait::async_trait]
impl Plugin for WakeHelperPlugin {
    fn name(&self) -> &'static str {
        "wake_helper"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[OFFER]
    }

    // What it tells is pushed by the watcher: the identity announces it.
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != OFFER {
            return Ok(());
        }
        let device = sender.device_id().to_string();
        if !pace_allows(&device, Instant::now()) {
            return Ok(());
        }
        let (Ok(dir), Ok(trust)) = (crate::state::state_dir(), crate::state::trust_store_path())
        else {
            return Ok(());
        };
        let body = packet.body.clone();
        let who = device.clone();
        let now = pocketdesk_core::local::pin::now_unix();
        let outcome = tokio::task::spawn_blocking(move || take_in(&dir, &trust, &who, &body, now))
            .await
            .ok()
            .flatten();
        match outcome {
            Some(Outcome::Taken {
                taken,
                changed,
                name,
            }) => {
                if changed {
                    tracing::info!("A phone is now this computer's wake helper");
                    if !cfg!(test) {
                        let words = format!("{name} is now this computer's wake helper");
                        tokio::task::spawn_blocking(move || {
                            crate::platform::notify("Wake helper", &words, Duration::from_secs(30))
                        });
                    }
                }
                if taken == Taken::Refused {
                    // Told at once, so its switch goes off on the phone.
                    let me = crate::state::state_dir()
                        .ok()
                        .and_then(|d| pocketdesk_core::local::identity::device_id(&d));
                    if let Some(computer) = me {
                        let refused = DutyMessage::Refused { computer };
                        sender
                            .send_packet(&NetworkPacket::new(DUTY).with_body(refused.body()))
                            .await?;
                    }
                }
                NUDGE.notify_one();
            }
            Some(Outcome::NotOwner) => {
                tracing::debug!("A phone that is not an owner phone offered to be the wake helper");
            }
            Some(Outcome::Failed) | None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const HELPER_KEY: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const OWNER_KEY: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    fn adapters() -> Vec<Adapter> {
        vec![Adapter {
            name: "Ethernet".into(),
            mac: Some([0x02, 0x00, 0x5e, 0x00, 0x53, 0x01]),
            ipv4: vec![(Ipv4Addr::new(192, 0, 2, 20), 24)],
            wired: true,
            physical: true,
            default: true,
        }]
    }

    fn snap() -> Snapshot {
        Snapshot {
            me: "computer-1".to_string(),
            helper: Some(store::Helper {
                device_id: "helper-phone".to_string(),
                iroh: HELPER_KEY.to_string(),
                relays: vec!["https://relay.example.com/".to_string()],
                seen: 1,
            }),
            helper_name: "Spare phone".to_string(),
            owners: ["helper-phone", "owner-phone", "keyless-phone"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            keys: [
                ("helper-phone".to_string(), "b".repeat(64)),
                ("owner-phone".to_string(), OWNER_KEY.to_string()),
            ]
            .into_iter()
            .collect(),
            refused: HashSet::new(),
        }
    }

    fn offer(fresh: bool) -> Value {
        Offer {
            on: true,
            fresh,
            iroh: HELPER_KEY.to_string(),
            relays: vec![],
        }
        .body()
    }

    #[test]
    fn the_helper_is_told_its_duty_from_its_own_network_and_the_owner_phones_keys() {
        let packet = packet_for(
            &snap(),
            "helper-phone",
            Some("192.0.2.77".parse().unwrap()),
            &adapters,
        )
        .unwrap();
        assert_eq!(packet.packet_type, DUTY);
        let DutyMessage::Duty(duty) = DutyMessage::parse(&packet.body).unwrap() else {
            panic!("a duty");
        };
        assert_eq!(duty.computer, "computer-1");
        assert_eq!(duty.mac, [0x02, 0x00, 0x5e, 0x00, 0x53, 0x01]);
        assert_eq!(duty.broadcast, Ipv4Addr::new(192, 0, 2, 255));
        // Only the owner phone with a key; not the helper itself, not the
        // owner phone that never registered one.
        assert_eq!(duty.requesters, vec![OWNER_KEY.to_string()]);
    }

    /// A helper that is on none of this computer's networks (the internet,
    /// a tunnel) has no broadcast to be given: it is told its duty is
    /// withdrawn, so the requesters it held are dropped, and never a duty.
    #[test]
    fn a_helper_off_the_computers_networks_has_its_duty_withdrawn() {
        let withdrawn = |peer: Option<&str>| {
            let packet = packet_for(
                &snap(),
                "helper-phone",
                peer.map(|p| p.parse().unwrap()),
                &adapters,
            )
            .expect("it is told something");
            assert_eq!(packet.packet_type, DUTY);
            matches!(
                DutyMessage::parse(&packet.body),
                Some(DutyMessage::Withdrawn { computer }) if computer == "computer-1"
            )
        };
        for peer in ["203.0.113.9", "198.51.100.9", "127.0.0.1"] {
            assert!(withdrawn(Some(peer)), "{peer}");
        }
        assert!(withdrawn(None));
        // Nothing of the withdrawal carries a MAC, an address or a key.
        let packet = packet_for(&snap(), "helper-phone", None, &adapters).unwrap();
        assert_eq!(
            packet.body,
            serde_json::json!({ "computer": "computer-1", "withdrawn": true })
        );
    }

    #[test]
    fn a_duty_is_told_again_in_the_hour_so_the_helper_does_not_drop_it() {
        assert!(REFRESH <= Duration::from_secs(60 * 60));
    }

    #[test]
    fn an_owner_phone_is_told_where_the_helper_is_and_others_nothing() {
        let packet = packet_for(&snap(), "owner-phone", None, &adapters).unwrap();
        assert_eq!(packet.packet_type, INFO);
        let info = Info::parse(&packet.body).unwrap();
        assert_eq!(info.computer, "computer-1");
        let helper = info.helper.unwrap();
        assert_eq!(helper.iroh, HELPER_KEY);
        assert_eq!(helper.name, "Spare phone");
        // A phone that is no owner phone (a guest's, someone else's) is
        // told nothing: it learns neither that a helper exists nor its key.
        assert!(packet_for(&snap(), "stranger-phone", None, &adapters).is_none());
        let mut without = snap();
        without.helper = None;
        let none = packet_for(&without, "owner-phone", None, &adapters).unwrap();
        assert!(Info::parse(&none.body).unwrap().helper.is_none());
    }

    #[test]
    fn a_removed_phone_is_told_it_is_refused() {
        let mut snap = snap();
        snap.helper = None;
        snap.refused.insert("helper-phone".to_string());
        let packet = packet_for(&snap, "helper-phone", None, &adapters).unwrap();
        assert!(matches!(
            DutyMessage::parse(&packet.body),
            Some(DutyMessage::Refused { .. })
        ));
    }

    #[test]
    fn a_revoked_phones_key_leaves_the_duty() {
        let mut snap = snap();
        snap.owners.remove("owner-phone");
        let peer = Some("192.0.2.77".parse().unwrap());
        let packet = packet_for(&snap, "helper-phone", peer, &adapters).unwrap();
        let DutyMessage::Duty(duty) = DutyMessage::parse(&packet.body).unwrap() else {
            panic!("a duty");
        };
        assert!(duty.requesters.is_empty());
    }

    #[test]
    fn an_offer_is_taken_only_from_an_owner_phone() {
        let state = crate::state::use_test_dirs();
        let dir: &std::path::Path = &state;
        let trust = dir.join("trusted_devices.json");
        TrustStore::update(&trust, |s| {
            s.trust_phone("wh-owner", true)?;
            s.trust_phone("wh-guest", false)?;
            s.trust_computer("wh-computer")
        })
        .unwrap();
        for who in ["wh-guest", "wh-computer", "wh-unknown"] {
            assert_eq!(
                take_in(dir, &trust, who, &offer(true), 10),
                Some(Outcome::NotOwner),
                "{who}"
            );
        }
        assert!(store::read(dir).helper.is_none());
        let outcome = take_in(dir, &trust, "wh-owner", &offer(true), 10).unwrap();
        assert!(matches!(
            outcome,
            Outcome::Taken {
                taken: Taken::Helper,
                changed: true,
                ..
            }
        ));
        assert_eq!(store::read(dir).helper.unwrap().device_id, "wh-owner");
        // The same offer again changes nothing.
        let again = take_in(dir, &trust, "wh-owner", &offer(false), 11).unwrap();
        assert!(matches!(again, Outcome::Taken { changed: false, .. }));
        // A body that does not read is no offer.
        assert_eq!(
            take_in(
                dir,
                &trust,
                "wh-owner",
                &serde_json::json!({ "on": "yes" }),
                12
            ),
            None
        );
        TrustStore::update(&trust, |s| {
            s.untrust_device("wh-owner")?;
            s.untrust_device("wh-guest")?;
            s.untrust_device("wh-computer")
        })
        .unwrap();
    }

    #[test]
    fn an_unpaired_helper_stops_being_one_at_the_next_look() {
        let state = crate::state::use_test_dirs();
        let dir: &std::path::Path = &state;
        std::fs::write(dir.join("device_id"), "computer-1").unwrap();
        let trust = dir.join("trusted_devices.json");
        TrustStore::update(&trust, |s| s.trust_phone("wh-gone", true)).unwrap();
        take_in(dir, &trust, "wh-gone", &offer(true), 10).unwrap();
        assert!(snapshot(dir, &trust).unwrap().helper.is_some());
        TrustStore::update(&trust, |s| s.untrust_device("wh-gone")).unwrap();
        assert!(snapshot(dir, &trust).unwrap().helper.is_none());
        assert!(store::read(dir).helper.is_none(), "forgotten on disk too");
    }

    #[test]
    fn a_phone_may_offer_a_few_times_a_minute() {
        let now = Instant::now();
        let phone = format!("pace-{}", std::process::id());
        for _ in 0..OFFERS_PER_MINUTE {
            assert!(pace_allows(&phone, now));
        }
        assert!(!pace_allows(&phone, now));
        assert!(pace_allows(&phone, now + Duration::from_secs(61)));
    }

    #[test]
    fn the_plugin_takes_the_offer_and_nothing_else() {
        assert_eq!(WakeHelperPlugin.incoming_capabilities(), &[OFFER]);
    }
}
