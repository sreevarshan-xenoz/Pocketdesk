use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::permissions::DevicePermissions;
use serde_json::json;

use crate::state;
use pocketdesk_core::local::approval::Verdict;
use pocketdesk_core::local::pin as local_pin;

// PIN pairing (KDE Connect parity, R3 Tier-2 #11b).
//
// Two flows, converging on the same `kdeconnect.pair` `{pin}` packet:
// - **push**: the phone requests a PIN (`kdeconnect.pocketdesk.pair`
//   `{requestPin:true}`); the desktop generates one, shows it via a desktop
//   notification (and the log), and the phone echoes it back.
// - **pull**: `pocketdesk pair` on the desktop writes a PIN file; the phone
//   enters that PIN and the request validates against the file.
//
// On success the phone's deviceId (learned from its `kdeconnect.identity`)
// is persisted to the TrustStore and the response carries the desktop's own
// deviceId so the phone can trust *us* back. Enforcement lives in the
// service dispatch loop, which gates plugin packets on [`is_trusted`], and
// on the QUIC paths, which gate on [`is_paired_device`] (the v2 handshake /
// in-band identity carry the real deviceId directly).

/// How long a generated/CLI PIN stays valid.
const PIN_TTL: Duration = local_pin::PIN_TTL;

#[derive(Clone)]
struct PinEntry {
    pin: String,
    created: Instant,
}

/// connection key (the deviceId a connection goes by) -> pending/used PIN
static PINS: OnceLock<Mutex<HashMap<String, PinEntry>>> = OnceLock::new();

fn pins() -> &'static Mutex<HashMap<String, PinEntry>> {
    PINS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// PINs (from the CLI or the app) that have paired a phone this run, each
/// with its minting time: the same six digits minted again are a new PIN.
fn spent_pins() -> &'static Mutex<std::collections::HashSet<(String, u64)>> {
    static SPENT: OnceLock<Mutex<std::collections::HashSet<(String, u64)>>> = OnceLock::new();
    SPENT.get_or_init(Mutex::default)
}

/// Spend the PIN minted at `minted_unix`: true the first time only, so of two
/// phones sending it at once just one pairs. One PIN, one phone, even when
/// its file could not be deleted.
fn spend_pin(pin: &str, minted_unix: u64) -> bool {
    spent_pins()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((pin.to_string(), minted_unix))
}

/// Is the device a connection goes by paired? On both transports the
/// connection key is the deviceId itself (phase B), so trust is never
/// borrowed from another connection's announcement. The TCP control channel
/// also holds the connection to its key on every packet ([`standing`]).
pub fn is_trusted(conn_key: &str) -> bool {
    is_paired_device(conn_key)
}

/// Is this deviceId in the persisted [`TrustStore`]?
///
/// Used by the QUIC paths (v2 multiplexer + video stream pipeline), which
/// cannot key on a TCP connection: they take the deviceId presented in-band
/// (the v2 handshake packet / the identity config stream), the same id
/// pairing stored here, and hold it to the key the connection proved
/// (device keys).
pub fn is_paired_device(device_id: &str) -> bool {
    let Ok(path) = state::trust_store_path() else {
        return false;
    };
    TrustStore::load_or_create(path)
        .map(|store| store.is_trusted(device_id))
        .unwrap_or(false)
}

/// Whether a pairing is someone else's phone (M2b): its code said so, or
/// a PIN this computer showed for the phone's own ask ("Show PIN on
/// desktop") while the Pair page's live code says someone else's or a
/// guest's (M2 review: the owner's choice holds whichever way the phone
/// pairs).
fn someone_else_for(
    matched: Option<&local_pin::FilePin>,
    matches_pending: bool,
    live: Option<&local_pin::FilePin>,
) -> bool {
    match matched {
        Some(file) => file.someone_else,
        None => {
            matches_pending
                && live.is_some_and(|file| file.someone_else || file.grant_secs.is_some())
        }
    }
}

/// The permissions of the profile the code that paired names (P9), as the
/// Pair page saved them: read now, so a profile changed since the code was
/// shown pairs as it is now; one removed since is none.
fn profile_for(
    cli: Option<&local_pin::FilePin>,
    matches_pending: bool,
    live: Option<&local_pin::FilePin>,
) -> Option<DevicePermissions> {
    let file = cli.or(live.filter(|_| matches_pending))?;
    let name = file.profile.as_deref()?;
    let dir = state::state_dir().ok()?;
    pocketdesk_core::local::permission_profiles::get(&dir, name).map(|p| p.permissions)
}

/// The permissions a pairing sets (M2b), or `None` to leave them: someone
/// else's phone, and every new guest, sends files and links only; the
/// owner's own phone keeps what it has (the defaults when new), a computer
/// its own narrow set (2f), and a guest grant on a phone already paired
/// changes nothing (that phone stays as it was). A paired phone (`current`)
/// paired again as someone else's keeps only what both allow, so nothing
/// the owner turned off comes back on (M2 review).
fn preset_permissions(
    guest: bool,
    someone_else: bool,
    computer: bool,
    current: Option<&DevicePermissions>,
    profile: Option<&DevicePermissions>,
) -> Option<DevicePermissions> {
    let narrow = if computer {
        false
    } else if guest {
        current.is_none()
    } else {
        someone_else
    };
    if !narrow {
        return None;
    }
    // P9: a profile's permissions in place of someone else's.
    let preset = profile
        .cloned()
        .unwrap_or_else(DevicePermissions::someone_else);
    Some(match current {
        Some(current) => current.narrowed_to(&preset),
        None => preset,
    })
}

/// Is `device_id` one of the owner's other computers (2f)?
pub fn is_computer_device(device_id: &str) -> bool {
    let Ok(path) = state::trust_store_path() else {
        return false;
    };
    TrustStore::load_or_create(path)
        .map(|store| store.is_computer(device_id))
        .unwrap_or(false)
}

/// Keep what a paired device calls itself in its identity (U3). Read
/// first, and written under the store's lock only when the name changed,
/// so a connection that brings the same name writes nothing; a device not
/// paired leaves nothing. Blocking file work: call it off the runtime.
pub fn note_claimed_name(device_id: &str, claimed: &str) {
    let Ok(path) = state::trust_store_path() else {
        return;
    };
    let Some(name) = pocketdesk_core::protocol::kdeconnect::clean_device_name(claimed) else {
        return;
    };
    let unchanged = TrustStore::read(&path)
        .map(|store| {
            !store.is_trusted(device_id) || store.claimed_name(device_id) == Some(name.as_str())
        })
        .unwrap_or(true);
    if unchanged {
        return;
    }
    if let Err(e) = TrustStore::update(&path, |store| store.note_claimed_name(device_id, &name)) {
        tracing::debug!("Could not keep {device_id}'s name: {e}");
    }
}

/// F5: may this device send `packet_type`? Packets that use no capability
/// always pass. [conn_key] is the deviceId a connection goes by: on TCP the
/// one its identity named in the handshake and its TLS key backed (phase B),
/// on QUIC the one it presented in-band.
/// The store is re-read per check, so a change in the desktop app applies
/// to the next packet.
pub fn permits(conn_key: &str, packet_type: &str) -> bool {
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::load_or_create(path).ok());
    permits_in(store.as_ref(), conn_key, packet_type)
}

/// May the paired device `device_id` have `capability` (N1: Ring, which
/// gates what the computer sends, not a packet)? Not paired, or a store
/// that cannot be read, is no.
pub fn allows(
    device_id: &str,
    capability: pocketdesk_core::protocol::permissions::Capability,
) -> bool {
    let Ok(path) = state::trust_store_path() else {
        return false;
    };
    TrustStore::read(path)
        .map(|store| store.is_trusted(device_id) && store.permissions(device_id).allows(capability))
        .unwrap_or(false)
}

/// [`permits`] against a given store; without one, nothing that needs a
/// permission is allowed.
fn permits_in(store: Option<&TrustStore>, device_id: &str, packet_type: &str) -> bool {
    let Some(capability) = pocketdesk_core::protocol::permissions::capability_for(packet_type)
    else {
        return true;
    };
    match store {
        Some(store) => store.permissions(device_id).allows(capability),
        // An unreadable store could be hiding a denial: refuse rather than
        // fall back to the defaults (the stream gate refuses too).
        None => false,
    }
}

/// F5: what a stream from [device_id] may do, or `None` to reject it. Read
/// when the session starts; a live session keeps what it started with.
///
/// With `pairing_required`, an unpaired device (or one that announced no
/// id) is rejected. Without it, such a device gets the defaults, while a
/// paired one still gets its own permissions. An unreadable store rejects
/// either way, since it could be hiding a denial.
pub fn session_permissions(
    device_id: Option<&str>,
    pairing_required: bool,
) -> Option<DevicePermissions> {
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::load_or_create(path).ok());
    session_permissions_in(store.as_ref(), device_id, pairing_required)
}

/// The stream gate (F5): the device keys check, [`session_permissions`],
/// then, for a device whose owner asked to approve each session, the
/// owner's answer at the desk. A first-use key is bound, and a WAN key the
/// phone names is registered, only once all of that has allowed the
/// session: an impostor the owner denies must not leave its key bound.
pub async fn session_gate(
    claim: pocketdesk_core::streaming::SessionClaim,
    pairing_required: bool,
) -> Option<DevicePermissions> {
    let dir = state::state_dir().ok()?;
    let paired = claim.device_id.as_deref().is_some_and(is_paired_device);
    let keys = decide_keys_in(&dir, &claim, paired);
    tracing::debug!(
        device_id = claim.device_id.as_deref().unwrap_or("<none announced>"),
        paired,
        presented = claim.peer_key.as_ref().map(|k| k.to_string()),
        iroh_id = claim.iroh_id.as_deref(),
        bind = keys.bind_tls.is_some(),
        register_wan = keys.register_iroh.is_some(),
        refused = keys.refused.as_ref().map(|r| r.reason()),
        "Device keys decided"
    );
    if let Some(refusal) = keys.refused {
        report_refusal(
            &dir,
            claim.device_id.as_deref().unwrap_or("<none>"),
            claim.peer,
            refusal,
            keys.presented.as_deref(),
        );
        return None;
    }
    let permissions =
        permissions_gate(claim.device_id.clone(), claim.peer, pairing_required).await?;
    if let Some(id) = claim.device_id.as_deref()
        && !commit_keys_in(&dir, id, &keys)
    {
        tracing::warn!(
            device_id = id,
            "Session refused: another key was bound to this device first"
        );
        return None;
    }
    Some(permissions)
}

/// [`session_permissions`], and the owner's answer for a device that asks
/// first.
async fn permissions_gate(
    device_id: Option<String>,
    peer: std::net::SocketAddr,
    pairing_required: bool,
) -> Option<DevicePermissions> {
    let permissions = session_permissions(device_id.as_deref(), pairing_required)?;
    if !permissions.ask_first {
        return Some(permissions);
    }
    let id = device_id?;
    let verdict = crate::approvals::ask(&id, peer).await;
    if verdict == Verdict::AlwaysAllow {
        stop_asking(&id);
    }
    match verdict {
        Verdict::Deny => None,
        // Read again: the owner may have changed this phone's switches, or
        // removed it, while the session waited.
        Verdict::AllowOnce | Verdict::AlwaysAllow => {
            session_permissions(Some(&id), pairing_required).map(|mut p| {
                p.ask_first = false;
                p
            })
        }
    }
}

/// Device keys for a v2 session: refused when its connection did not prove
/// the key bound to the device it claims. Reads only: v2 never binds.
pub(crate) fn device_keys_refused(claim: &pocketdesk_core::streaming::SessionClaim) -> bool {
    let Ok(dir) = state::state_dir() else {
        return true;
    };
    let paired = claim.device_id.as_deref().is_some_and(is_paired_device);
    let keys = decide_keys_in(&dir, claim, paired);
    match keys.refused {
        Some(refusal) => {
            report_refusal(
                &dir,
                claim.device_id.as_deref().unwrap_or("<none>"),
                claim.peer,
                refusal,
                keys.presented.as_deref(),
            );
            true
        }
        None => false,
    }
}

/// What the device keys say about a session, before the rest of the gate.
#[derive(Debug, Default)]
struct KeyDecision {
    /// Refused, and why.
    refused: Option<pocketdesk_core::local::device_keys::Refusal>,
    /// The key the connection proved, as stored (for the log).
    presented: Option<String>,
    /// A LAN key to bind if the session is allowed (first use).
    bind_tls: Option<String>,
    /// The WAN key the phone named on a session its LAN key proves.
    register_iroh: Option<String>,
}

/// Read-only: decide the session's claim against the device key store.
fn decide_keys_in(
    dir: &std::path::Path,
    claim: &pocketdesk_core::streaming::SessionClaim,
    paired: bool,
) -> KeyDecision {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    use pocketdesk_core::streaming::PeerKey;
    // An unpaired device is the pairing gate's to judge; it has no keys.
    let Some(id) = claim.device_id.as_deref().filter(|_| paired) else {
        return KeyDecision::default();
    };
    let (slot, presented) = match &claim.peer_key {
        Some(PeerKey::Tls(hash)) => (Slot::Tls, Some(hex(hash))),
        Some(PeerKey::Iroh(endpoint)) => (Slot::Iroh, Some(endpoint.clone())),
        None => (Slot::Tls, None),
    };
    let verdict = device_keys::check(dir, id, slot, presented.as_deref());
    let mut decision = KeyDecision {
        presented: presented.clone(),
        ..KeyDecision::default()
    };
    match verdict {
        Verdict::Refused(refusal) => decision.refused = Some(refusal),
        Verdict::FirstUse => decision.bind_tls = presented,
        Verdict::Matches | Verdict::Unbound => {}
    }
    // Only a session the LAN key proves (or is about to) may name the
    // device's WAN key.
    if slot == Slot::Tls && matches!(verdict, Verdict::Matches | Verdict::FirstUse) {
        decision.register_iroh = claim.iroh_id.clone();
    }
    decision
}

/// A deviceId a connection may go by: short printable text. It keys the
/// connection's state and appears in logs and notifications.
pub(crate) use pocketdesk_core::protocol::kdeconnect::valid_device_id;

/// Where a control connection stands (phase B), re-read for every packet so
/// a pairing, an unpairing or a key change applies at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// The device is paired and the connection proved its key.
    Paired,
    /// The device is not paired; it may pair (or, with pairing off, act).
    Unpaired,
    /// The device is paired, but the connection did not prove its key: it
    /// may only pair again, with the owner's PIN.
    WrongKey(pocketdesk_core::local::device_keys::Refusal),
}

impl Standing {
    /// Whether a connection of this standing may act on this computer: a
    /// paired device with its key, or an unpaired one while pairing is not
    /// required; one with the wrong key never (it may only pair again).
    pub(crate) fn may_act(&self, pairing_required: bool) -> bool {
        match self {
            Standing::Paired => true,
            Standing::Unpaired => !pairing_required,
            Standing::WrongKey(_) => false,
        }
    }
}

/// Whether a packet from a connection that goes by `device_id` and proved
/// `key` may act on this computer now, re-read for each packet so pairing,
/// unpairing or a key change applies to a connection already open (AY1:
/// the v2 control stream judged the key only as it opened, and a session
/// opened while the device was unpaired, with another key, acted once the
/// real one paired again). A connection that proved an iroh key is held to
/// the internet road's rule, never to the LAN's TLS key.
pub(crate) fn may_act(
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    pairing_required: bool,
) -> bool {
    if let Some(key @ pocketdesk_core::streaming::PeerKey::Iroh(_)) = key {
        return internet_standing(device_id, Some(key)).is_ok();
    }
    standing(device_id, key).may_act(pairing_required)
}

/// The standing of a connection that goes by `device_id` and proved `key`.
pub(crate) fn standing(
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> Standing {
    match state::state_dir() {
        Ok(dir) => standing_in(&dir, device_id, key, is_paired_device(device_id)),
        Err(_) => Standing::WrongKey(pocketdesk_core::local::device_keys::Refusal::StoreUnreadable),
    }
}

fn standing_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    paired: bool,
) -> Standing {
    match decide_control_identity_in(dir, device_id, key, paired) {
        Ok(()) if paired => Standing::Paired,
        Ok(()) => Standing::Unpaired,
        Err(refusal) => Standing::WrongKey(refusal),
    }
}

/// Phase B: the standing of a control connection that proved `key` and
/// goes by `device_id`, as it opens. A connection that claims a paired
/// device without its key is logged and notified as the stream's are, and
/// kept only so that the owner's PIN can pair it again (a phone whose key
/// changed recovers that way).
pub(crate) fn admit_control(
    device_id: &str,
    peer: std::net::SocketAddr,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> Standing {
    let standing = standing(device_id, key);
    if let Standing::WrongKey(refusal) = standing
        && let Ok(dir) = state::state_dir()
    {
        let presented = match key {
            Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => Some(hex(hash)),
            _ => None,
        };
        report_refusal(&dir, device_id, peer, refusal, presented.as_deref());
    }
    standing
}

/// The control channel's half of device keys: a paired device must prove
/// the TLS key bound to it. Nothing is bound here, since no approval step
/// runs on the control channel; a paired device with no key yet passes by
/// its id until an allowed LAN stream binds one. An unpaired device passes,
/// so that it can pair.
fn decide_control_identity_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    paired: bool,
) -> std::result::Result<(), pocketdesk_core::local::device_keys::Refusal> {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    if !paired {
        return Ok(());
    }
    let presented = match key {
        Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => Some(hex(hash)),
        _ => None,
    };
    match device_keys::check(dir, device_id, Slot::Tls, presented.as_deref()) {
        Verdict::Matches | Verdict::FirstUse | Verdict::Unbound => Ok(()),
        Verdict::Refused(refusal) => Err(refusal),
    }
}

/// May a control connection over the internet road (iroh, T10a) go by
/// `device_id`? Only a paired device whose iroh key, registered from a LAN
/// session its TLS key proved, is the key this connection proved. Nothing
/// binds by first use here and no pairing runs on this road, whether or not
/// pairing is required: anyone on the internet can dial it.
pub(crate) fn internet_standing(
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> std::result::Result<(), pocketdesk_core::local::device_keys::Refusal> {
    match state::state_dir() {
        Ok(dir) => internet_standing_in(&dir, device_id, key, is_paired_device(device_id)),
        Err(_) => Err(pocketdesk_core::local::device_keys::Refusal::StoreUnreadable),
    }
}

fn internet_standing_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    paired: bool,
) -> std::result::Result<(), pocketdesk_core::local::device_keys::Refusal> {
    use pocketdesk_core::local::device_keys::{self, Refusal, Slot, Verdict};
    if !paired {
        return Err(Refusal::NotRegistered);
    }
    let presented = match key {
        Some(pocketdesk_core::streaming::PeerKey::Iroh(id)) => Some(id.as_str()),
        _ => None,
    };
    match device_keys::check(dir, device_id, Slot::Iroh, presented) {
        Verdict::Matches => Ok(()),
        Verdict::Refused(refusal) => Err(refusal),
        // Nothing to hold the key to: not this road.
        Verdict::FirstUse | Verdict::Unbound if presented.is_some() => Err(Refusal::NotRegistered),
        Verdict::FirstUse | Verdict::Unbound => Err(Refusal::NoKey),
    }
}

/// May files move between this desktop and the device a connection goes by,
/// over a transfer pinned to `key` (phase C)? A paired device must have
/// proved that very key and had it bound: one with no key bound yet may be
/// someone else claiming its id (pairing it again binds its key). With
/// pairing off, an unpaired device's own key is the one used.
pub(crate) fn may_transfer(
    device_id: &str,
    key: &pocketdesk_core::streaming::PeerKey,
) -> std::result::Result<(), String> {
    let dir = state::state_dir().map_err(|e| e.to_string())?;
    may_transfer_in(&dir, device_id, key, is_paired_device(device_id))
}

fn may_transfer_in(
    dir: &std::path::Path,
    device_id: &str,
    key: &pocketdesk_core::streaming::PeerKey,
    paired: bool,
) -> std::result::Result<(), String> {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    let hash = match key {
        pocketdesk_core::streaming::PeerKey::Tls(hash) => hash,
        // The internet road (2c): the phone's registered iroh key, which its
        // control connection there proved; it admits paired phones only.
        pocketdesk_core::streaming::PeerKey::Iroh(_) => {
            return internet_standing_in(dir, device_id, Some(key), paired)
                .map_err(|refusal| refusal.reason().to_string());
        }
    };
    if !paired {
        return Ok(());
    }
    match device_keys::check(dir, device_id, Slot::Tls, Some(&hex(hash))) {
        Verdict::Matches => Ok(()),
        Verdict::FirstUse | Verdict::Unbound => {
            Err("this phone has no key bound here yet; pair it again to move files".to_string())
        }
        Verdict::Refused(refusal) => Err(refusal.reason().to_string()),
    }
}

/// Apply what an allowed session may record. `false` when another session
/// bound a different key to the device first.
fn commit_keys_in(dir: &std::path::Path, device_id: &str, keys: &KeyDecision) -> bool {
    use pocketdesk_core::local::device_keys::{self, Slot};
    if let Some(key) = &keys.bind_tls {
        match device_keys::bind_first(dir, device_id, key) {
            Ok(true) => tracing::info!(device_id, "Bound the device's key on its first session"),
            Ok(false) => return false,
            Err(e) => {
                tracing::warn!(device_id, "Could not bind the device's key: {e}");
                return false;
            }
        }
    }
    if let Some(iroh) = &keys.register_iroh
        && let Err(e) = device_keys::register(dir, device_id, Slot::Iroh, iroh)
    {
        tracing::warn!(device_id, "Could not record the device's WAN key: {e}");
    }
    true
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Log a refusal with both keys, and tell the desk (at most once a minute
/// per device, so a retrying impostor cannot flood it).
fn report_refusal(
    dir: &std::path::Path,
    id: &str,
    peer: std::net::SocketAddr,
    refusal: pocketdesk_core::local::device_keys::Refusal,
    presented: Option<&str>,
) {
    use pocketdesk_core::local::device_keys::Refusal;
    let bound = pocketdesk_core::local::device_keys::read(dir)
        .ok()
        .and_then(|keys| keys.get(id).cloned());
    tracing::warn!(
        device_id = id,
        presented = presented.unwrap_or("<none>"),
        bound = ?bound,
        "Session refused: {}",
        refusal.reason()
    );
    static LAST: std::sync::OnceLock<Mutex<HashMap<String, Instant>>> = std::sync::OnceLock::new();
    let now = Instant::now();
    {
        let mut last = LAST
            .get_or_init(Default::default)
            .lock()
            .expect("refusal log");
        if last
            .get(id)
            .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(60))
        {
            return;
        }
        last.insert(id.to_string(), now);
    }
    let short: String = id.chars().take(8).collect();
    let body = match refusal {
        Refusal::StoreUnreadable => format!(
            "Pocketdesk could not read its device key store, so it refused your phone {short}… \
             Unpair the phone in the Pocketdesk app's Devices page and pair it again."
        ),
        _ => format!(
            "A connection from {} claimed to be your paired phone {short}… without its key, and was refused.",
            peer.ip()
        ),
    };
    std::thread::spawn(move || {
        let summary = "Pocketdesk: a session was refused";
        if !crate::platform::notify(summary, &body, Duration::from_secs(10)) {
            tracing::warn!("{summary} — {body}");
        }
    });
}

/// "Always allow": turn the device's "Ask before each session" off.
fn stop_asking(device_id: &str) {
    let result = state::trust_store_path()
        .map_err(|e| e.to_string())
        .and_then(|path| {
            TrustStore::update(path, |store| {
                let mut permissions = store.permissions(device_id);
                permissions.ask_first = false;
                store.set_permissions(device_id, permissions)
            })
            .map_err(|e| e.to_string())
        });
    if let Err(e) = result {
        tracing::warn!("Could not stop asking for {device_id}: {e}");
    }
}

fn session_permissions_in(
    store: Option<&TrustStore>,
    device_id: Option<&str>,
    pairing_required: bool,
) -> Option<DevicePermissions> {
    let store = store?;
    match device_id.filter(|id| store.is_trusted(id)) {
        Some(id) => Some(store.permissions(id)),
        None if pairing_required => None,
        None => Some(DevicePermissions::default()),
    }
}

/// The desktop's own device id, for phone-side trust persistence.
pub fn host_device_id() -> String {
    crate::kde::host_identity().device_id
}

fn pin_expired(entry: &PinEntry) -> bool {
    entry.created.elapsed() > PIN_TTL
}

/// The live PIN file written by `pocketdesk pair` or the desktop app, as
/// `(pin, grant_secs)` (format and expiry: `core::local::pin`).
fn cli_pin() -> Option<local_pin::FilePin> {
    let dir = state::state_dir().ok()?;
    local_pin::read(&dir, local_pin::now_unix())
}

fn desktop_id_field() -> String {
    host_device_id()
}

#[derive(Debug, Default)]
pub struct PairPlugin;

#[async_trait::async_trait]
impl Plugin for PairPlugin {
    fn name(&self) -> &'static str {
        "pair"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        // "kdeconnect.identity" must be listed here or the router never
        // dispatches it to handle_packet, phone_ids stays empty, and PIN
        // pairing cannot learn the phone's deviceId to persist.
        &[
            "kdeconnect.pair",
            "kdeconnect.pocketdesk.pair",
            "kdeconnect.identity",
            WAN_KEY,
        ]
    }

    // What it answers with: the identity announces it (ecosystem 1g).
    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.pair", "kdeconnect.pocketdesk.pair"]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let conn = sender.device_id().to_string();
        match packet.packet_type.as_str() {
            "kdeconnect.identity" => {
                // A connection is known by the id it opened with (the TCP
                // handshake, the v2 handshake); an identity naming another
                // device changes nothing.
                let named = packet.body.get("deviceId").and_then(|v| v.as_str());
                if named.is_some_and(|id| id != conn) {
                    tracing::warn!(
                        "Ignored an identity naming another device on {conn}'s connection"
                    );
                }
            }
            "kdeconnect.pocketdesk.pair" => {
                if packet
                    .body
                    .get("requestPin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    request_pin(&conn, sender).await;
                }
            }
            "kdeconnect.pair" => {
                handle_pair(&conn, packet, sender).await;
            }
            WAN_KEY => register_wan_key(&conn, packet, sender),
            _ => {}
        }
        Ok(())
    }
}

/// A paired computer names its iroh key (2f), as a phone's first LAN
/// session does with its stream marker.
pub const WAN_KEY: &str = "kdeconnect.pocketdesk.wan_key";

fn register_wan_key(device_id: &str, packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(endpoint) = packet
        .body
        .get("endpointId")
        .and_then(|v| v.as_str())
        .filter(|id| id.parse::<iroh::EndpointId>().is_ok())
    else {
        tracing::warn!("A WAN key from {device_id} that names no iroh endpoint; ignored");
        return;
    };
    let Ok(dir) = state::state_dir() else {
        return;
    };
    match register_wan_key_in(
        &dir,
        device_id,
        sender.peer_key().as_ref(),
        is_paired_device(device_id),
        is_computer_device(device_id),
        endpoint,
    ) {
        Ok(()) => tracing::info!(device_id, "Registered the device's WAN key"),
        Err(reason) => tracing::warn!(device_id, "WAN key not registered: {reason}"),
    }
}

/// Register `endpoint` as `device_id`'s iroh key: only from a paired
/// computer on the home road, whose TLS key is the one bound to it (the
/// trust the stream marker `[0xFC,0x00]` has, `docs/protocol.md` §2.3). A
/// phone registers its key through a session the owner allowed, as before.
fn register_wan_key_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    paired: bool,
    computer: bool,
    endpoint: &str,
) -> std::result::Result<(), String> {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    if !paired {
        return Err("not paired".to_string());
    }
    if !computer {
        return Err("only another computer names its key this way".to_string());
    }
    let Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) = key else {
        return Err("only on the home network".to_string());
    };
    match device_keys::check(dir, device_id, Slot::Tls, Some(&hex(hash))) {
        Verdict::Matches => {}
        _ => return Err("its key is not the one bound to it".to_string()),
    }
    device_keys::register(dir, device_id, Slot::Iroh, endpoint).map_err(|e| e.to_string())
}

/// Generate + surface a PIN for this connection and tell the phone it is
/// coming. If a CLI PIN file is live, the phone is told to expect manual
/// entry instead.
async fn request_pin(conn: &str, sender: &dyn DeviceSender) {
    if cli_pin().is_some() {
        let response = NetworkPacket::new("kdeconnect.pocketdesk.pair").with_body(json!({
            "pairStatus": "pinReady",
        }));
        let _ = sender.send_packet(&response).await;
        return;
    }

    // Any device that has not paired may ask, under an id it chooses, so
    // minting is paced: a burst of requests is not a stream of desktop
    // notifications, and a pending PIN is announced again, not replaced.
    let now = Instant::now();
    let pending = pins()
        .lock()
        .expect("pair state")
        .get(conn)
        .is_some_and(|e| !pin_expired(e));
    if !mint_pace().lock().expect("pair state").allow(now) {
        tracing::info!("PIN request from {conn} within 10 s of the last; no new PIN");
        if pending {
            let response = NetworkPacket::new("kdeconnect.pocketdesk.pair").with_body(json!({
                "pairStatus": "pinSent",
            }));
            let _ = sender.send_packet(&response).await;
        }
        return;
    }
    let pin = local_pin::mint_value();
    {
        let mut pins = pins().lock().expect("pair state");
        pins.retain(|_, e| !pin_expired(e));
        pins.insert(
            conn.to_string(),
            PinEntry {
                pin: pin.clone(),
                created: now,
            },
        );
    }
    // The PIN itself goes to the log only when no notification can show it.
    tracing::info!(
        "Pairing PIN minted for {conn} (valid {}s)",
        PIN_TTL.as_secs()
    );
    show_pin_notification(&pin);

    let response = NetworkPacket::new("kdeconnect.pocketdesk.pair").with_body(json!({
        "pairStatus": "pinSent",
    }));
    let _ = sender.send_packet(&response).await;
}

/// When the desktop last minted a PIN for a phone that asked.
#[derive(Default)]
struct MintPace {
    last: Option<Instant>,
}

impl MintPace {
    /// The shortest wait between two minted PINs.
    const GAP: Duration = Duration::from_secs(10);

    /// Whether a PIN may be minted at `now`; if so, it counts as minted.
    fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < Self::GAP)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

fn mint_pace() -> &'static Mutex<MintPace> {
    static PACE: OnceLock<Mutex<MintPace>> = OnceLock::new();
    PACE.get_or_init(Default::default)
}

/// Wrong guesses allowed against one PIN before it is discarded.
const MAX_WRONG_GUESSES: u32 = 5;

/// Wrong guesses against the current PIN. A six-digit PIN is 10^6 guesses,
/// and nothing else stops one connection from trying them all inside the
/// PIN's 300 s window, so after [`MAX_WRONG_GUESSES`] the PIN is discarded
/// and the person pairing shows a new one.
#[derive(Debug, Default)]
struct Guesses {
    /// The CLI/app PIN the count is for; a different one restarts it.
    pin: Option<String>,
    wrong: u32,
}

impl Guesses {
    /// Count a wrong guess while `current` is the live CLI/app PIN. True
    /// when the PIN must now be discarded (the count then starts over).
    fn wrong(&mut self, current: Option<&str>) -> bool {
        if self.pin.as_deref() != current {
            self.pin = current.map(str::to_string);
            self.wrong = 0;
        }
        self.wrong += 1;
        if self.wrong >= MAX_WRONG_GUESSES {
            self.wrong = 0;
            true
        } else {
            false
        }
    }

    fn reset(&mut self) {
        self.wrong = 0;
    }
}

fn guesses() -> &'static Mutex<Guesses> {
    static GUESSES: OnceLock<Mutex<Guesses>> = OnceLock::new();
    GUESSES.get_or_init(|| Mutex::new(Guesses::default()))
}

async fn handle_pair(conn: &str, packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let Some(pin) = packet.body.get("pin").and_then(|v| v.as_str()) else {
        return;
    };
    // The connection's own id (the TCP or v2 handshake's), never one named
    // later in a packet.
    let phone_id = conn.to_string();

    let matches_pending = pins()
        .lock()
        .expect("pair state")
        .get(conn)
        .filter(|e| !pin_expired(e))
        .is_some_and(|e| e.pin == pin);
    // R4 D3: only a CLI PIN can carry a scoped grant; the push flow (a PIN
    // this desktop generated itself) keeps trusting permanently.
    // Matching spends it, in one step.
    let live = cli_pin();
    let cli = live
        .clone()
        .filter(|cli| cli.pin == pin && spend_pin(pin, cli.minted_unix));
    let cli_grant: Option<Option<u64>> = cli.as_ref().map(|cli| cli.grant_secs);
    // M2b: the Pair page said the phone is someone else's.
    let someone_else = someone_else_for(cli.as_ref(), matches_pending, live.as_ref());
    // P9: with a profile's permissions, if it named one still saved; else
    // someone else's.
    let profile = profile_for(cli.as_ref(), matches_pending, live.as_ref());
    let matches_cli = cli_grant.is_some();

    if matches_pending || matches_cli {
        pins().lock().expect("pair state").remove(conn);
        guesses().lock().expect("pair state").reset();
        // One PIN, one phone: a CLI/app PIN that has paired is spent, so a
        // code still on screen (or photographed) cannot pair another.
        if matches_cli {
            // Spent for this run (above) even if its file cannot be deleted.
            if let Ok(dir) = state::state_dir()
                && let Err(e) = local_pin::remove_if(&dir, pin)
            {
                tracing::error!("The PIN that paired could not be deleted ({e}); it stays refused");
            }
        }
        let grant_secs = cli_grant.flatten();
        // A guest grant, and someone else's phone (M2 review), binds the key
        // that paired before anything is trusted, and never claims an id
        // another phone has proved: only the owner's own code may.
        if (grant_secs.is_some() || someone_else)
            && let Ok(dir) = state::state_dir()
            && !guest_may_pair_in(
                &dir,
                &phone_id,
                sender.peer_key().as_ref(),
                is_paired_device(&phone_id),
            )
        {
            tracing::warn!(
                "Pairing refused: {phone_id} is the id of a phone with another key, and only the owner's own code may take it"
            );
            respond_pair(sender, false, "").await;
            return;
        }
        // The owner's own PIN, inside TLS, binds the key that paired (phase
        // B), replacing any older one (perhaps a reinstalled phone's), before
        // anything is trusted: a pairing that cannot bind fails, rather than
        // trusting an id with stale keys or none.
        if grant_secs.is_none() && !someone_else {
            let bound = state::state_dir()
                .map_err(|e| std::io::Error::other(e.to_string()))
                .and_then(|dir| owner_paired_in(&dir, &phone_id, sender.peer_key().as_ref()));
            if let Err(e) = bound {
                tracing::error!("Pairing refused: could not bind the keys of {phone_id}: {e}");
                respond_pair(sender, false, "").await;
                return;
            }
        }
        // Another of the owner's computers (2f) asks to be one: the PIN
        // allowed the pairing, this chooses its narrow permissions. Never a
        // guest's.
        let computer = grant_secs.is_none()
            && packet.body.get("computer").and_then(|v| v.as_bool()) == Some(true);
        let outcome = TrustStore::update(state::trust_store_path().expect("state dir"), |store| {
            let was_trusted = store.is_trusted(&phone_id);
            // The permissions before the trust (M2 review): a write that fails
            // between them leaves an untrusted id's entry, which the store
            // drops as it loads, never a phone trusted with the defaults.
            if let Some(permissions) = preset_permissions(
                grant_secs.is_some(),
                someone_else,
                computer,
                was_trusted.then(|| store.permissions(&phone_id)).as_ref(),
                profile.as_ref(),
            ) {
                store.set_permissions(&phone_id, permissions)?;
            }
            match grant_secs {
                Some(secs) => {
                    store.trust_device_with_ttl(phone_id.clone(), Duration::from_secs(secs))
                }
                None if computer => store.trust_computer(phone_id.clone()),
                None => store.trust_device(phone_id.clone()),
            }
        });
        match outcome {
            Ok(()) => {
                match grant_secs {
                    Some(secs) => tracing::info!(
                        "Paired phone {phone_id} on connection {conn} (scoped grant: {secs}s)"
                    ),
                    None if computer => {
                        tracing::info!("Paired computer {phone_id} on connection {conn}");
                        // Not one this computer paired with, too (BD1).
                        if let Ok(dir) = state::state_dir() {
                            crate::peer_computers::paired_here(
                                &dir,
                                &phone_id,
                                pocketdesk_core::local::pin::now_unix(),
                            );
                        }
                    }
                    None => tracing::info!("Paired phone {phone_id} on connection {conn}"),
                }
                respond_pair(sender, true, &desktop_id_field()).await;
            }
            Err(e) => {
                tracing::error!("Pairing succeeded but trust store write failed: {e}");
                respond_pair(sender, false, "").await;
            }
        }
    } else {
        tracing::warn!("Pairing attempt from {conn} with a wrong/expired PIN");
        let current = cli_pin().map(|cli| cli.pin);
        let discard = guesses()
            .lock()
            .expect("pair state")
            .wrong(current.as_deref());
        if discard {
            tracing::warn!(
                "{MAX_WRONG_GUESSES} wrong PIN guesses: the current PIN is discarded; show a new one to pair"
            );
            pins().lock().expect("pair state").clear();
            if let (Some(cli), Ok(dir)) = (current, state::state_dir()) {
                let _ = local_pin::remove_if(&dir, &cli);
            }
        }
        respond_pair(sender, false, "").await;
    }
}

/// The owner's PIN paired `phone_id` on a connection that proved `key`: that
/// key becomes the device's only one. A connection that proved none (v2
/// without a client certificate) clears the keys, so the next allowed LAN
/// stream binds, as before phase B.
fn owner_paired_in(
    dir: &std::path::Path,
    phone_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> std::io::Result<()> {
    use pocketdesk_core::local::device_keys;
    match key {
        Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => {
            let tls = hex(hash);
            // The same certificate paired again (perhaps over an address
            // the phone reaches only through the WAN): its keys stand, the
            // WAN key included, which nothing could register again there.
            let unchanged = device_keys::read(dir)
                .ok()
                .and_then(|keys| keys.get(phone_id).cloned())
                .is_some_and(|keys| keys.tls.as_deref() == Some(tls.as_str()));
            if unchanged {
                return Ok(());
            }
            device_keys::replace(dir, phone_id, &tls)
        }
        _ => device_keys::clear(dir, phone_id),
    }
}

/// May a guest grant pair `phone_id` on a connection that proved `key`? Its
/// key is bound if the id is free. An id bound to a different key, or one
/// already paired (even before it has proved a key), belongs to another
/// phone, and a guest never takes it over.
fn guest_may_pair_in(
    dir: &std::path::Path,
    phone_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    already_paired: bool,
) -> bool {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    if already_paired {
        let presented = match key {
            Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => Some(hex(hash)),
            _ => None,
        };
        return device_keys::check(dir, phone_id, Slot::Tls, presented.as_deref())
            == Verdict::Matches;
    }
    match key {
        Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => {
            device_keys::bind_first(dir, phone_id, &hex(hash)).unwrap_or(false)
        }
        // Nothing proved: allowed only while the id holds no key.
        _ => matches!(
            device_keys::check(dir, phone_id, Slot::Tls, None),
            Verdict::Unbound
        ),
    }
}

async fn respond_pair(sender: &dyn DeviceSender, paired: bool, server_id: &str) {
    let response = NetworkPacket::new("kdeconnect.pair").with_body(json!({
        "pair": paired,
        "serverId": server_id,
    }));
    if let Err(e) = sender.send_packet(&response).await {
        tracing::debug!("Failed to deliver pair response: {e}");
    }
}

/// Show the PIN on the desktop. Only when no notification can show it does
/// it go to the log, the one place left to read it (a headless desktop);
/// never fails the pairing flow. Off the caller's task: a hung notification
/// daemon would otherwise hold a runtime worker for as long as it hangs.
fn show_pin_notification(pin: &str) {
    let fallback = pin.to_string();
    let summary = format!("Pocketdesk pairing PIN: {pin}");
    let body = "Enter it on your phone. It expires in 5 minutes.";
    std::thread::spawn(move || {
        if !crate::platform::notify(&summary, body, std::time::Duration::from_secs(300)) {
            tracing::info!("No desktop notification available; the pairing PIN is {fallback}");
        }
    });
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_pin_shown_for_the_phone_follows_the_pair_pages_choice() {
        let file = |someone_else, grant_secs| local_pin::FilePin {
            profile: None,
            pin: "123456".to_string(),
            minted_unix: 0,
            grant_secs,
            someone_else,
        };
        // The code itself says it.
        assert!(someone_else_for(Some(&file(true, None)), false, None));
        assert!(!someone_else_for(Some(&file(false, None)), false, None));
        // A PIN shown for the phone, while the page's code is someone else's or a guest's.
        assert!(someone_else_for(None, true, Some(&file(true, None))));
        assert!(someone_else_for(None, true, Some(&file(false, Some(900)))));
        assert!(!someone_else_for(None, true, Some(&file(false, None))));
        assert!(!someone_else_for(None, true, None));
    }

    #[test]
    fn a_pairing_sets_narrow_permissions_for_someone_else_and_new_guests() {
        use pocketdesk_core::protocol::permissions::Capability;
        let narrow = Some(DevicePermissions::someone_else());
        let mine = DevicePermissions::default();
        // The owner's own phone: as it was.
        assert_eq!(preset_permissions(false, false, false, None, None), None);
        assert_eq!(
            preset_permissions(false, false, false, Some(&mine), None),
            None
        );
        // Someone else's: narrow when new.
        assert_eq!(preset_permissions(false, true, false, None, None), narrow);
        // Paired again as someone else's: narrowed, never widened.
        let mut no_links = DevicePermissions::default();
        no_links.set(Capability::Links, false);
        let again = preset_permissions(false, true, false, Some(&no_links), None).unwrap();
        assert!(again.allows(Capability::Files));
        assert!(
            !again.allows(Capability::Links),
            "the owner's off stays off"
        );
        assert!(!again.allows(Capability::Input));
        assert!(again.ask_first);
        // A guest: narrow when new; a phone already paired keeps its own.
        assert_eq!(preset_permissions(true, false, false, None, None), narrow);
        assert_eq!(
            preset_permissions(true, true, false, Some(&mine), None),
            None
        );
        // A computer keeps its own set.
        assert_eq!(preset_permissions(false, true, true, None, None), None);
    }
    use super::*;

    /// U3: a paired phone's name, from its identity, is kept cleaned; a
    /// phone not paired leaves nothing in the store.
    #[test]
    fn a_paired_phones_name_is_kept_and_an_unpaired_ones_is_not() {
        pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        let path = state::trust_store_path().unwrap();
        TrustStore::update(&path, |store| store.trust_device("names-paired-phone")).unwrap();
        note_claimed_name("names-paired-phone", "Galaxy\u{202E} A71");
        note_claimed_name("names-stranger-phone", "Stranger");
        let store = TrustStore::read(&path).unwrap();
        assert_eq!(store.name("names-paired-phone"), Some("Galaxy A71"));
        assert_eq!(store.name("names-stranger-phone"), None);
        TrustStore::update(&path, |store| store.untrust_device("names-paired-phone")).unwrap();
    }

    #[test]
    fn a_pin_that_paired_stays_spent() {
        assert!(spend_pin("482913", 1_000), "the first phone pairs");
        assert!(
            !spend_pin("482913", 1_000),
            "a second, even at once, does not"
        );
        assert!(spend_pin("482914", 1_000), "only that PIN");
        assert!(
            spend_pin("482913", 2_000),
            "the same digits minted again are new"
        );
    }
    use pocketdesk_core::streaming::{PeerKey, SessionClaim};

    fn key_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-gatekeys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn claim(peer_key: Option<PeerKey>, iroh_id: Option<&str>) -> SessionClaim {
        SessionClaim {
            device_id: Some("phone".to_string()),
            peer: "192.0.2.7:51000".parse().unwrap(),
            peer_key,
            iroh_id: iroh_id.map(str::to_string),
        }
    }

    /// The gate as an allowed session runs it: decide, then commit.
    fn allowed(dir: &std::path::Path, claim: &SessionClaim) -> bool {
        let keys = decide_keys_in(dir, claim, true);
        keys.refused.is_none() && commit_keys_in(dir, "phone", &keys)
    }

    #[test]
    fn the_control_channel_admits_a_paired_phone_only_with_its_key() {
        let dir = key_dir("control");
        let phone = PeerKey::Tls([1; 32]);
        assert!(
            allowed(&dir, &claim(Some(phone.clone()), None)),
            "bound on the stream"
        );
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&phone), true),
            Ok(())
        );
        assert!(
            decide_control_identity_in(&dir, "phone", Some(&PeerKey::Tls([2; 32])), true).is_err(),
            "someone else claiming the phone's id"
        );
        assert!(
            decide_control_identity_in(&dir, "phone", None, true).is_err(),
            "no key, now that the phone has one"
        );
    }

    #[test]
    fn the_internet_road_admits_only_a_paired_phone_with_its_registered_iroh_key() {
        use pocketdesk_core::local::device_keys::{self, Refusal, Slot};
        let dir = key_dir("internet");
        let iroh = PeerKey::Iroh("a".repeat(64));
        // Paired, with only a LAN key: it has never named an iroh key.
        assert!(allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)));
        assert_eq!(
            internet_standing_in(&dir, "phone", Some(&iroh), true),
            Err(Refusal::NotRegistered)
        );
        device_keys::register(&dir, "phone", Slot::Iroh, &"a".repeat(64)).unwrap();
        assert_eq!(
            internet_standing_in(&dir, "phone", Some(&iroh), true),
            Ok(())
        );
        assert_eq!(
            internet_standing_in(&dir, "phone", Some(&PeerKey::Iroh("b".repeat(64))), true),
            Err(Refusal::WrongKey),
            "another iroh key"
        );
        assert_eq!(
            internet_standing_in(&dir, "phone", Some(&PeerKey::Tls([1; 32])), true),
            Err(Refusal::NoKey),
            "a TLS key is not this road's"
        );
        assert_eq!(
            internet_standing_in(&dir, "phone", None, true),
            Err(Refusal::NoKey)
        );
        assert_eq!(
            internet_standing_in(&dir, "phone", Some(&iroh), false),
            Err(Refusal::NotRegistered),
            "unpaired, whatever its keys say"
        );
        // A device with no keys at all passes nothing here (no first use).
        assert_eq!(
            internet_standing_in(&dir, "stranger", Some(&iroh), true),
            Err(Refusal::NotRegistered)
        );
    }

    #[test]
    fn only_a_paired_device_with_its_key_acts_and_an_unpaired_one_with_pairing_off() {
        // AY1: the v2 control stream asked only "is this id paired", so a
        // session opened while it was not, with another key, acted once
        // the real phone paired again; the standing is what decides.
        let wrong =
            Standing::WrongKey(pocketdesk_core::local::device_keys::Refusal::StoreUnreadable);
        assert!(Standing::Paired.may_act(true));
        assert!(!Standing::Unpaired.may_act(true));
        assert!(Standing::Unpaired.may_act(false));
        assert!(!wrong.may_act(true));
        assert!(!wrong.may_act(false));
    }

    #[test]
    fn a_connection_with_another_key_may_only_pair_and_the_owners_pin_recovers_it() {
        let dir = key_dir("standing");
        assert!(allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)));
        let reinstalled = PeerKey::Tls([5; 32]);
        assert!(matches!(
            standing_in(&dir, "phone", Some(&reinstalled), true),
            Standing::WrongKey(_)
        ));
        assert_eq!(
            standing_in(&dir, "phone", Some(&PeerKey::Tls([1; 32])), true),
            Standing::Paired
        );
        assert_eq!(
            standing_in(&dir, "stranger", Some(&reinstalled), false),
            Standing::Unpaired
        );
        // Pairing again with the owner's PIN, on that same connection.
        owner_paired_in(&dir, "phone", Some(&reinstalled)).unwrap();
        assert_eq!(
            standing_in(&dir, "phone", Some(&reinstalled), true),
            Standing::Paired
        );
    }

    #[test]
    fn the_desktop_mints_a_pin_at_most_every_ten_seconds() {
        let start = Instant::now();
        let mut mints = MintPace::default();
        assert!(mints.allow(start));
        assert!(
            !mints.allow(start + Duration::from_secs(3)),
            "a burst of requests"
        );
        assert!(mints.allow(start + Duration::from_secs(10)));
    }

    #[test]
    fn files_move_only_with_the_key_bound_to_a_paired_phone() {
        let dir = key_dir("transfer");
        let phone = PeerKey::Tls([1; 32]);
        assert!(
            may_transfer_in(&dir, "phone", &phone, true).is_err(),
            "a paired phone with no key bound yet: someone may be claiming its id"
        );
        assert!(allowed(&dir, &claim(Some(phone.clone()), None)));
        assert!(may_transfer_in(&dir, "phone", &phone, true).is_ok());
        assert!(may_transfer_in(&dir, "phone", &PeerKey::Tls([2; 32]), true).is_err());
        assert!(
            may_transfer_in(&dir, "stranger", &PeerKey::Tls([3; 32]), false).is_ok(),
            "unpaired: only possible with pairing off"
        );
    }

    #[test]
    fn a_computer_names_its_wan_key_only_at_home_with_its_bound_key() {
        use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
        let dir = key_dir("wan-key");
        let tls = PeerKey::Tls([4; 32]);
        let endpoint = "e".repeat(64);
        assert!(
            register_wan_key_in(&dir, "phone", Some(&tls), false, true, &endpoint).is_err(),
            "unpaired"
        );
        assert!(
            register_wan_key_in(&dir, "phone", Some(&tls), true, true, &endpoint).is_err(),
            "no TLS key bound yet"
        );
        assert!(allowed(&dir, &claim(Some(tls.clone()), None)));
        assert!(
            register_wan_key_in(
                &dir,
                "phone",
                Some(&PeerKey::Iroh("f".repeat(64))),
                true,
                true,
                &endpoint
            )
            .is_err(),
            "not by the internet road"
        );
        assert!(
            register_wan_key_in(
                &dir,
                "phone",
                Some(&PeerKey::Tls([5; 32])),
                true,
                true,
                &endpoint
            )
            .is_err(),
            "another TLS key"
        );
        register_wan_key_in(&dir, "phone", Some(&tls), true, true, &endpoint).unwrap();
        assert!(matches!(
            device_keys::check(&dir, "phone", Slot::Iroh, Some(&endpoint)),
            Verdict::Matches
        ));
    }

    #[test]
    fn on_the_internet_road_files_move_with_the_registered_iroh_key() {
        use pocketdesk_core::local::device_keys::{self, Slot};
        let dir = key_dir("transfer-internet");
        let iroh = PeerKey::Iroh("c".repeat(64));
        assert!(
            may_transfer_in(&dir, "phone", &iroh, true).is_err(),
            "no iroh key registered at home yet"
        );
        device_keys::register(&dir, "phone", Slot::Iroh, &"c".repeat(64)).unwrap();
        assert!(may_transfer_in(&dir, "phone", &iroh, true).is_ok());
        assert!(
            may_transfer_in(&dir, "phone", &PeerKey::Iroh("d".repeat(64)), true).is_err(),
            "another iroh key"
        );
        assert!(
            may_transfer_in(&dir, "phone", &iroh, false).is_err(),
            "unpaired: never by this road"
        );
    }

    #[test]
    fn the_control_channel_never_binds_a_key() {
        // No approval step runs on the control channel, so a paired phone
        // without keys yet passes by its id and nothing is recorded.
        let dir = key_dir("control-unbound");
        let stranger = PeerKey::Tls([9; 32]);
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&stranger), true),
            Ok(())
        );
        assert!(
            pocketdesk_core::local::device_keys::read(&dir)
                .unwrap()
                .get("phone")
                .is_none_or(|k| k.tls.is_none() && k.iroh.is_none())
        );
    }

    #[test]
    fn pairing_again_with_the_same_key_keeps_the_wan_key() {
        // A phone paired again over an address it reaches only through the
        // WAN (no LAN stream to register the key again) must not lose it.
        let dir = key_dir("pair-same");
        let phone = PeerKey::Tls([1; 32]);
        assert!(allowed(
            &dir,
            &claim(Some(phone.clone()), Some("phone-wan"))
        ));
        owner_paired_in(&dir, "phone", Some(&phone)).unwrap();
        assert_eq!(
            pocketdesk_core::local::device_keys::read(&dir).unwrap()["phone"]
                .iroh
                .as_deref(),
            Some("phone-wan")
        );
        // A new certificate is a new phone: its old WAN key goes.
        owner_paired_in(&dir, "phone", Some(&PeerKey::Tls([2; 32]))).unwrap();
        assert_eq!(
            pocketdesk_core::local::device_keys::read(&dir).unwrap()["phone"].iroh,
            None
        );
    }

    #[test]
    fn pairing_with_the_owners_pin_binds_the_key_that_paired() {
        let dir = key_dir("pair-owner");
        assert!(allowed(
            &dir,
            &claim(Some(PeerKey::Tls([1; 32])), Some("old-wan"))
        ));
        let reinstalled = PeerKey::Tls([5; 32]);
        owner_paired_in(&dir, "phone", Some(&reinstalled)).unwrap();
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&reinstalled), true),
            Ok(())
        );
        assert!(
            decide_control_identity_in(&dir, "phone", Some(&PeerKey::Tls([1; 32])), true).is_err(),
            "the old key is gone"
        );
    }

    #[test]
    fn pairing_without_a_proved_key_leaves_the_device_to_bind_later() {
        let dir = key_dir("pair-owner-nokey");
        assert!(allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)));
        owner_paired_in(&dir, "phone", None).unwrap();
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&PeerKey::Tls([2; 32])), true),
            Ok(()),
            "cleared, as before phase B"
        );
    }

    #[test]
    fn a_guest_pin_binds_a_free_id_but_never_another_phones() {
        let dir = key_dir("pair-guest");
        let guest = PeerKey::Tls([3; 32]);
        assert!(
            guest_may_pair_in(&dir, "guest", Some(&guest), false),
            "a free id"
        );
        assert_eq!(
            decide_control_identity_in(&dir, "guest", Some(&guest), true),
            Ok(())
        );
        assert!(
            guest_may_pair_in(&dir, "guest", Some(&guest), false),
            "the same key again"
        );
        assert!(
            allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)),
            "the owner's phone"
        );
        assert!(
            !guest_may_pair_in(&dir, "phone", Some(&guest), true),
            "a guest claiming the owner's phone"
        );
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&PeerKey::Tls([1; 32])), true),
            Ok(()),
            "the owner's phone keeps its key"
        );
    }

    #[test]
    fn a_guest_pin_never_takes_a_paired_phones_id_even_before_it_has_a_key() {
        // A phone paired before device keys, that has not streamed on the
        // LAN since, holds no key yet; its id is still its own.
        let dir = key_dir("pair-guest-unbound");
        assert!(!guest_may_pair_in(
            &dir,
            "phone",
            Some(&PeerKey::Tls([3; 32])),
            true
        ));
        assert!(
            !pocketdesk_core::local::device_keys::read(&dir)
                .unwrap()
                .contains_key("phone"),
            "nothing bound"
        );
    }

    #[test]
    fn an_unpaired_device_may_use_the_control_channel_to_pair() {
        let dir = key_dir("control-unpaired");
        assert!(allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)));
        assert_eq!(
            decide_control_identity_in(&dir, "phone", Some(&PeerKey::Tls([2; 32])), false),
            Ok(()),
            "pairing decides; an unpaired id holds nothing yet"
        );
    }

    #[test]
    fn a_device_id_is_short_printable_text() {
        assert!(valid_device_id("0d7c3a52-91e4-4b6f-8a2e-5f1c9b3d6e70"));
        assert!(!valid_device_id(""));
        assert!(!valid_device_id(
            "phone
forged log line"
        ));
        assert!(!valid_device_id(&"x".repeat(300)));
    }

    #[test]
    fn a_paired_phone_is_held_to_the_key_it_first_proved() {
        let dir = key_dir("tls");
        let phone = Some(PeerKey::Tls([1; 32]));
        assert!(allowed(&dir, &claim(phone.clone(), None)));
        assert!(allowed(&dir, &claim(phone, None)));
        assert!(
            decide_keys_in(&dir, &claim(Some(PeerKey::Tls([2; 32])), None), true)
                .refused
                .is_some(),
            "someone else claiming the phone's id"
        );
        assert!(
            decide_keys_in(&dir, &claim(None, None), true)
                .refused
                .is_some(),
            "no key, now that the phone has one"
        );
    }

    #[test]
    fn deciding_binds_nothing_until_the_session_is_allowed() {
        // An impostor the owner denies at the desk must not leave its key
        // bound: the decision only reads.
        let dir = key_dir("deny");
        let keys = decide_keys_in(
            &dir,
            &claim(Some(PeerKey::Tls([7; 32])), Some("attacker")),
            true,
        );
        assert!(keys.refused.is_none() && keys.bind_tls.is_some());
        assert!(
            pocketdesk_core::local::device_keys::read(&dir)
                .unwrap()
                .is_empty()
        );
        // The real phone, later, still binds.
        assert!(allowed(&dir, &claim(Some(PeerKey::Tls([1; 32])), None)));
    }

    #[test]
    fn the_wan_key_is_taken_only_from_an_authenticated_session() {
        let dir = key_dir("iroh");
        // A WAN session before any LAN one binds nothing, and the LAN key
        // still binds afterwards.
        assert!(allowed(
            &dir,
            &claim(Some(PeerKey::Iroh("stranger".into())), None)
        ));
        let phone = Some(PeerKey::Tls([1; 32]));
        assert!(allowed(&dir, &claim(phone.clone(), Some("phone-endpoint"))));
        assert!(allowed(
            &dir,
            &claim(Some(PeerKey::Iroh("phone-endpoint".into())), None)
        ));
        assert!(
            decide_keys_in(
                &dir,
                &claim(Some(PeerKey::Iroh("stranger".into())), None),
                true
            )
            .refused
            .is_some()
        );
        // The wrong certificate names nothing.
        let wrong = decide_keys_in(
            &dir,
            &claim(Some(PeerKey::Tls([9; 32])), Some("stranger")),
            true,
        );
        assert!(wrong.refused.is_some() && wrong.register_iroh.is_none());
    }

    #[test]
    fn of_two_first_sessions_the_second_is_refused() {
        let dir = key_dir("race");
        let first = decide_keys_in(&dir, &claim(Some(PeerKey::Tls([1; 32])), None), true);
        let second = decide_keys_in(&dir, &claim(Some(PeerKey::Tls([2; 32])), None), true);
        assert!(commit_keys_in(&dir, "phone", &first));
        assert!(!commit_keys_in(&dir, "phone", &second));
    }

    #[test]
    fn an_unpaired_device_binds_nothing() {
        let dir = key_dir("unpaired");
        let keys = decide_keys_in(&dir, &claim(Some(PeerKey::Tls([1; 32])), None), false);
        assert!(keys.refused.is_none() && keys.bind_tls.is_none());
    }

    fn temp_store(name: &str) -> TrustStore {
        let path = std::env::temp_dir().join(format!("ll-perm-{name}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        TrustStore::load_or_create(path).unwrap()
    }

    #[test]
    fn a_denied_capability_blocks_its_packets_and_only_those() {
        use pocketdesk_core::protocol::permissions::Capability;
        let mut store = temp_store("deny");
        store.trust_device("phone-1").unwrap();
        let mut permissions = store.permissions("phone-1");
        permissions.set(Capability::Clipboard, false);
        store.set_permissions("phone-1", permissions).unwrap();

        assert!(!permits_in(Some(&store), "phone-1", "kdeconnect.clipboard"));
        assert!(
            !permits_in(
                Some(&store),
                "phone-1",
                "kdeconnect.pocketdesk.notification_reply"
            ),
            "a reply lands on the desktop clipboard"
        );
        assert!(permits_in(
            Some(&store),
            "phone-1",
            "kdeconnect.share.request"
        ));
        assert!(permits_in(
            Some(&store),
            "phone-1",
            "kdeconnect.battery.request"
        ));
        assert!(
            !permits_in(Some(&store), "phone-1", "kdeconnect.pocketdesk.exec"),
            "exec is off until granted"
        );
        assert!(
            permits_in(Some(&store), "phone-2", "kdeconnect.clipboard"),
            "another device keeps its defaults"
        );
        assert!(
            !permits_in(None, "phone-1", "kdeconnect.clipboard"),
            "no readable store denies what needs a permission"
        );
        assert!(
            permits_in(None, "phone-1", "kdeconnect.battery.request"),
            "and still answers what needs none"
        );
    }

    #[test]
    fn a_stream_gets_its_devices_permissions_and_an_unpaired_one_none() {
        use pocketdesk_core::protocol::permissions::Capability;
        let mut store = temp_store("session");
        store.trust_device("phone-s").unwrap();
        let mut permissions = store.permissions("phone-s");
        permissions.set(Capability::Audio, false);
        store
            .set_permissions("phone-s", permissions.clone())
            .unwrap();

        for pairing_required in [true, false] {
            assert_eq!(
                session_permissions_in(Some(&store), Some("phone-s"), pairing_required),
                Some(permissions.clone()),
                "a paired device gets its own permissions either way"
            );
            assert_eq!(
                session_permissions_in(None, Some("phone-s"), pairing_required),
                None,
                "no readable store rejects"
            );
        }
        assert_eq!(
            session_permissions_in(Some(&store), Some("stranger"), true),
            None
        );
        assert_eq!(
            session_permissions_in(Some(&store), None, true),
            None,
            "no id announced"
        );
        assert_eq!(
            session_permissions_in(Some(&store), Some("stranger"), false),
            Some(DevicePermissions::default()),
            "pairing off: an unpaired device gets the defaults"
        );
    }

    /// A plugin that adds a packet type must either map it to a capability
    /// or be listed here as always allowed, so nothing slips past the gate.
    #[test]
    fn every_incoming_packet_type_is_mapped_or_deliberately_always_allowed() {
        use pocketdesk_core::protocol::permissions::capability_for;
        const ALWAYS_ALLOWED: &[&str] = &[
            "kdeconnect.identity",
            "kdeconnect.pair",
            "kdeconnect.pocketdesk.pair",
            "kdeconnect.battery",
            "kdeconnect.battery.request",
            "kdeconnect.notification.request",
            "kdeconnect.pocketdesk.monitors",
            "kdeconnect.pocketdesk.windows",
            "kdeconnect.findmydevice",
            "kdeconnect.mousepad.echo",
            "kdeconnect.filebrowse.response",
            // A computer's iroh key (2f): it holds itself to a paired
            // device's bound TLS key, on the home road.
            "kdeconnect.pocketdesk.wan_key",
            // The deck (F1): the listing tells only labels and kinds, and a
            // press is held to the phone's permission for its action.
            "kdeconnect.pocketdesk.deck",
            "kdeconnect.pocketdesk.deck_press",
            // The computer's state (F2): what a paired phone already sees of
            // the battery, with the CPU, memory, disks and uptime.
            "kdeconnect.pocketdesk.system",
            // A link's role (L8 part 2): it only narrows what the link is for.
            "kdeconnect.pocketdesk.link_role",
            // M2a: a phone asks what it is allowed; it is told only about itself.
            "kdeconnect.pocketdesk.allowed",
            // Which of its features a phone has on (L7): status about the
            // phone itself, as its battery is, booleans only.
            "kdeconnect.pocketdesk.switches",
            // The phone as a screen (BK5): what the phone says of itself,
            // and its answers to pings; this computer's SharedInput gates
            // what it sends the phone.
            "kdeconnect.pocketdesk.shared_input.screen",
            "kdeconnect.pocketdesk.shared_input.pong",
        ];
        for packet_type in ALWAYS_ALLOWED {
            assert!(
                capability_for(packet_type).is_none(),
                "{packet_type} is gated, so it does not belong in the list"
            );
        }
        let (incoming, _) = crate::plugins::register_all().capability_sets();
        for packet_type in incoming {
            assert!(
                capability_for(&packet_type).is_some()
                    || ALWAYS_ALLOWED.contains(&packet_type.as_str()),
                "{packet_type} has no capability and is not listed as always allowed"
            );
        }
    }

    /// A six-digit PIN is 10^6 guesses; without a limit, one TCP connection
    /// can try them all inside the 300 s window.
    #[test]
    fn five_wrong_guesses_discard_the_pin_and_a_new_pin_starts_over() {
        let mut guesses = Guesses::default();
        for _ in 0..MAX_WRONG_GUESSES - 1 {
            assert!(!guesses.wrong(Some("123456")));
        }
        assert!(
            guesses.wrong(Some("123456")),
            "the fifth wrong guess discards it"
        );
        assert!(
            !guesses.wrong(Some("123456")),
            "the count starts over after a discard"
        );
        for _ in 0..MAX_WRONG_GUESSES - 2 {
            guesses.wrong(Some("123456"));
        }
        assert!(
            !guesses.wrong(Some("654321")),
            "a new PIN gets its own five"
        );
        guesses.reset();
        assert_eq!(guesses.wrong, 0);
    }

    #[test]
    fn expired_pin_is_rejected() {
        let entry = PinEntry {
            pin: "123456".into(),
            created: Instant::now() - PIN_TTL - Duration::from_secs(1),
        };
        assert!(pin_expired(&entry));
        let fresh = PinEntry {
            pin: "123456".into(),
            created: Instant::now(),
        };
        assert!(!pin_expired(&fresh));
    }

    #[test]
    fn plugin_declares_pair_capabilities() {
        let caps = PairPlugin.incoming_capabilities();
        assert!(caps.contains(&"kdeconnect.pair"));
        assert!(caps.contains(&"kdeconnect.pocketdesk.pair"));
    }

    #[test]
    fn unknown_device_is_not_paired() {
        // QUIC paths lean on this being a strict deny-by-default lookup.
        assert!(!is_paired_device("00000000-0000-0000-0000-00000000dead"));
    }

    /// P9: a profile's permissions for a new phone; never widening one paired.
    #[test]
    fn a_profile_sets_its_permissions_and_never_widens() {
        use pocketdesk_core::protocol::permissions::Capability;
        let mut kids = DevicePermissions::someone_else();
        kids.set(Capability::Audio, true);
        assert_eq!(
            preset_permissions(false, true, false, None, Some(&kids)),
            Some(kids.clone())
        );
        let mut no_audio = DevicePermissions::default();
        no_audio.set(Capability::Audio, false);
        let again = preset_permissions(false, true, false, Some(&no_audio), Some(&kids)).unwrap();
        assert!(
            !again.allows(Capability::Audio),
            "never on what the owner turned off"
        );
        // A computer's are its own, profile or not.
        assert_eq!(
            preset_permissions(false, true, true, None, Some(&kids)),
            None
        );
    }
}
