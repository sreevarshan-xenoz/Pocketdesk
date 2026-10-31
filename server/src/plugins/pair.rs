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
    // P9: a profile's permissions in place of someone else's; never this
    // computer's keyboard and mouse onto the phone (BK5), which only its
    // own page sets.
    let mut preset = profile
        .cloned()
        .unwrap_or_else(DevicePermissions::someone_else);
    preset.set(
        pocketdesk_core::protocol::permissions::Capability::SharedInput,
        false,
    );
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

/// Keep the Pocketdesk release a paired device's identity says it runs
/// (item 4): `claimed` is its `pocketdeskVersion`, `None` when it sent
/// none, which clears the one kept (a downgrade shows no old release).
/// Written under the store's lock only when it changed. Blocking file work.
pub fn note_claimed_version(device_id: &str, claimed: Option<&str>) {
    let Ok(path) = state::trust_store_path() else {
        return;
    };
    let version = claimed.and_then(pocketdesk_core::version::clean_version);
    let unchanged = TrustStore::read(&path)
        .map(|store| !store.is_trusted(device_id) || store.version(device_id) == version.as_deref())
        .unwrap_or(true);
    if unchanged {
        return;
    }
    if let Err(e) = TrustStore::update(&path, |store| {
        store.note_claimed_version(device_id, version.as_deref())
    }) {
        tracing::debug!("Could not keep {device_id}'s version: {e}");
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

/// May the computer `device_id` set this computer's clipboard on the link
/// this computer keeps to it ("Clipboard with my computers")? One paired
/// into this computer too, by its Clipboard permission here (the switch on
/// its Devices entry); one this computer paired into alone, yes, as the
/// owner's setting decides. A store that cannot be read is no.
pub fn kept_link_clipboard(device_id: &str) -> bool {
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::read(path).ok());
    kept_link_clipboard_in(store.as_ref(), device_id)
}

/// [`kept_link_clipboard`] against a given store.
fn kept_link_clipboard_in(store: Option<&TrustStore>, device_id: &str) -> bool {
    store.is_some_and(|store| {
        !store.is_trusted(device_id)
            || store
                .permissions(device_id)
                .allows(pocketdesk_core::protocol::permissions::Capability::Clipboard)
    })
}

/// [`permits`] for a request that comes over the link this computer keeps
/// to `device_id`: only a computer this one trusts may ask for anything that
/// needs a permission. A computer this one paired into alone keeps no trust
/// entry here, and the defaults [`permits`] falls back to (Power among them)
/// are a paired device's, not a stranger's (range review C1).
pub fn link_permits(device_id: &str, packet_type: &str) -> bool {
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::read(path).ok());
    link_permits_in(store.as_ref(), device_id, packet_type)
}

/// [`link_permits`] against a given store.
fn link_permits_in(store: Option<&TrustStore>, device_id: &str, packet_type: &str) -> bool {
    store.is_some_and(|s| s.is_trusted(device_id)) && permits_in(store, device_id, packet_type)
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
        Some(id) => {
            let mut permissions = store.permissions(id);
            // A computer's mic plays only while it controls (PC10 review).
            permissions.mic_needs_control = store.is_computer(id);
            Some(permissions)
        }
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
            crate::peer_computers::BOTH_WAYS,
            crate::computer_wake::COMPUTER_WAKE,
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
            // Pair a computer nearby: its steps ride this packet.
            "kdeconnect.pocketdesk.pair" if crate::nearby::is_step(packet) => {
                crate::nearby::handle(packet, sender).await;
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
            // Only from one of the owner's computers (third review).
            crate::peer_computers::BOTH_WAYS if is_computer_device(&conn) => {
                let (key, connection) = (sender.peer_key(), sender.connection_id().to_string());
                let heard = tokio::task::spawn_blocking(move || {
                    crate::peer_computers::both_ways_heard(&conn, key.as_ref(), &connection)
                })
                .await;
                if let Err(e) = heard {
                    tracing::debug!("A computer's word that it paired both ways was lost: {e}");
                }
            }
            // How to wake one of the owner's computers (PC6), kept only
            // from a computer, and only for one this computer lists.
            crate::computer_wake::COMPUTER_WAKE => {
                crate::computer_wake::heard(&conn, &packet.body, false).await;
            }
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

    // Ecosystem phase 4: a PIN this computer handed another of the owner's
    // computers for a phone it introduced, for this id and the key that
    // computer named only, pairs that phone as an owner phone once.
    if let Some(intro) = crate::introductions::take_pin(&phone_id, sender.peer_key().as_ref(), pin)
    {
        crate::introductions::pair_introduced(&phone_id, packet, sender, intro).await;
        return;
    }

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
    let from_file = live
        .clone()
        .filter(|cli| cli.pin == pin && spend_pin(pin, cli.minted_unix));
    // Pair a computer nearby: the PIN this computer handed one whose ask
    // its owner accepted, for that certificate only, reads as the owner's
    // own code (both ways when the owner said both are theirs). It was
    // handed to a computer, and pairs only as one (PR #556 review C3).
    let nearby_pin = from_file
        .is_none()
        .then(|| crate::nearby::take_pin(conn, sender.peer_key().as_ref(), pin))
        .flatten();
    let nearby = nearby_pin.is_some();
    let cli = from_file.clone().or(nearby_pin);
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
        if from_file.is_some() {
            // Spent for this run (above) even if its file cannot be deleted.
            if let Ok(dir) = state::state_dir()
                && let Err(e) = local_pin::remove_if(&dir, pin)
            {
                tracing::error!("The PIN that paired could not be deleted ({e}); it stays refused");
            }
        }
        let grant_secs = cli_grant.flatten();
        let packet_computer = packet.body.get("computer").and_then(|v| v.as_bool()) == Some(true);
        if let Some(why) = nearby_as_phone(nearby, packet_computer) {
            tracing::warn!("Pairing refused: {why}");
            respond_pair(sender, false, "").await;
            return;
        }
        // A nearby PIN pairs a computer, with a computer's narrow
        // permissions, whatever the packet says.
        let asked_computer = packet_computer || nearby;
        // A nearby PIN claims no id already paired here, either way, nor one
        // bound to another key, checked now as it pairs (PR #556 review
        // I2b): the owner accepted a new computer, never a takeover.
        if nearby {
            let taken = match state::state_dir() {
                Ok(dir) => nearby_id_taken_in(&dir, &phone_id, sender.peer_key().as_ref()),
                Err(_) => Some("the state folder is unknown".to_string()),
            };
            if let Some(why) = taken {
                tracing::warn!("Pairing refused: {why}");
                respond_pair(sender, false, "").await;
                return;
            }
        }
        // Before any key is bound (PC1 re-review I-1): an id that is a
        // computer listed here, with another certificate, is refused, as a
        // computer or a phone, and that computer keeps its keys.
        if let Ok(dir) = state::state_dir()
            && let Some(why) =
                reused_id_in(&dir, &phone_id, sender.peer_key().as_ref(), asked_computer)
        {
            tracing::warn!("Pairing refused: {why}");
            respond_pair(sender, false, "").await;
            return;
        }
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
                .and_then(|dir| {
                    pairing_keys_in(&dir, &phone_id, sender.peer_key().as_ref(), asked_computer)
                        .map_err(std::io::Error::other)
                });
            if let Err(e) = bound {
                tracing::error!("Pairing refused: could not bind the keys of {phone_id}: {e}");
                respond_pair(sender, false, "").await;
                return;
            }
        }
        // Another of the owner's computers (2f) asks to be one: the PIN
        // allowed the pairing, this chooses its narrow permissions. Never a
        // guest's.
        let computer = grant_secs.is_none() && asked_computer;
        // PC1: one of the owner's own, paired both ways, when it asked and
        // this computer's own code was minted for that; listed here in turn
        // from what it said of itself.
        let both_ways = pairs_both_ways(
            computer,
            packet.body.get("mine").and_then(|v| v.as_bool()) == Some(true),
            cli.as_ref(),
            someone_else,
            sender.peer_key().as_ref(),
        )
        .then(|| listed_back(&phone_id, &packet.body, sender.peer_key().as_ref()))
        .flatten()
        // Only while the server can list it: else a one-way pairing.
        .filter(|_| crate::peer_computers::takes_paired_back());
        // Paired again one way: no grant both ways waits for it any more,
        // before it is trusted narrowly again (PC1 third review m-A).
        if computer
            && both_ways.is_none()
            && let Ok(dir) = state::state_dir()
        {
            crate::peer_computers::stop_waiting(&dir, &phone_id);
        }
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
                // PC1: both ways, but as one way until the link between
                // the two shows the other computer recorded it too (review
                // M1): `peer_computers::grant_on_link` gives the rest then.
                None if computer => store.trust_computer(phone_id.clone()),
                // Remote revoke: only the owner's own code makes an owner
                // phone, which may remove this computer's devices from afar.
                None => store.trust_phone(
                    phone_id.clone(),
                    is_owner_code(someone_else, profile.is_some()),
                ),
            }
        });
        match outcome {
            Ok(()) => {
                if let Some(listed) = both_ways {
                    // The endpoint it is listed at is the key it is admitted
                    // by from now (PC1 review M3); its WAN key names the same.
                    if let (Some(target), Ok(dir)) = (
                        listed
                            .endpoint
                            .as_ref()
                            .and_then(crate::peer_computers::WanTarget::parse),
                        state::state_dir(),
                    ) && let Err(e) = pocketdesk_core::local::device_keys::register(
                        &dir,
                        &phone_id,
                        pocketdesk_core::local::device_keys::Slot::Iroh,
                        &target.endpoint_id,
                    ) {
                        tracing::warn!("Could not bind {phone_id}'s internet key: {e}");
                    }
                    tracing::info!(
                        "Paired computer {phone_id} both ways on connection {conn}: it sees and controls this one without asking"
                    );
                    // Listed here in turn (PC1), not forgotten as one paired
                    // the other way (BD1): the two keep one link between them.
                    // Its connection up now is not the link that shows it
                    // recorded the pairing: the newest per device (third
                    // review m-B), in practice this pairing connection. An
                    // older link is no proof either way: the grant needs the
                    // computer's both-ways word, sent only once recorded.
                    let before: Vec<String> = state::clone_answer_targets()
                        .await
                        .iter()
                        .filter(|c| c.device_id() == phone_id)
                        .map(|c| c.connection_id().to_string())
                        .collect();
                    if !crate::peer_computers::paired_back(listed, before) {
                        tracing::warn!(
                            "Paired computer {phone_id} both ways, but the server could not list it here"
                        );
                    }
                    respond_pair_as(sender, true, &desktop_id_field(), true).await;
                    return;
                }
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
    } else if crate::introductions::miss(&phone_id, sender.peer_key().as_ref()) {
        // Ecosystem phase 4 (PR #581 I1): a wrong PIN from a phone an
        // introduction PIN waits for is that PIN's miss alone (it goes after
        // three), never the owner's own PIN's.
        tracing::warn!("Pairing attempt from {conn} with a wrong introduction PIN");
        respond_pair(sender, false, "").await;
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

/// Whether a phone's permanent pairing is the owner's own (remote revoke):
/// not someone else's code, and not one that names a permission profile
/// (which pairs as someone else's, P9). A guest's grant never reaches here.
fn is_owner_code(someone_else: bool, profile: bool) -> bool {
    !someone_else && !profile
}

/// Why a pairing with a PIN handed to a nearby computer (`nearby`) is
/// refused: it did not ask as a computer (PR #556 review C3). The owner
/// accepted a computer's ask, with a computer's narrow permissions; the
/// same PIN must never trust a phone with a phone's full ones.
fn nearby_as_phone(nearby: bool, asked_computer: bool) -> Option<&'static str> {
    (nearby && !asked_computer).then_some(
        "a PIN handed to a nearby computer pairs only as a computer, and this pairing asked as a phone",
    )
}

/// Why a nearby computer's pairing (or ask) under `device_id`, on a
/// connection that proved `key`, is refused (PR #556 review I2b): the id
/// is already paired here, as a phone or a computer, or listed as a
/// computer this one paired with, or bound to another key. A nearby ask is
/// for a computer new to this one; an id in use is someone else's, and the
/// owner's Accept must never hand it over. None when the id is free.
pub(crate) fn nearby_id_taken_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> Option<String> {
    use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
    let trusted = TrustStore::read(dir.join(pocketdesk_core::local::paths::TRUST_FILE))
        .map(|store| store.is_trusted(device_id))
        // Unreadable: refused, as nothing can be said of the id.
        .unwrap_or(true);
    if trusted {
        return Some(format!(
            "{device_id} is already paired here (a nearby ask is for a new computer)"
        ));
    }
    let listed = pocketdesk_core::local::computers::read(dir)
        .computers
        .iter()
        .any(|c| c.device_id == device_id);
    if listed {
        return Some(format!(
            "{device_id} is a computer this one paired with (a nearby ask is for a new computer)"
        ));
    }
    let presented = match key {
        Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => Some(hex(hash)),
        _ => None,
    };
    match device_keys::check(dir, device_id, Slot::Tls, presented.as_deref()) {
        Verdict::Refused(why) => Some(format!(
            "{device_id} is bound to another key here ({why:?})"
        )),
        Verdict::Matches | Verdict::FirstUse | Verdict::Unbound => None,
    }
}

/// Bind the keys of the owner's pairing of `phone_id` on a connection that
/// proved `key`, as a computer or a phone (`computer`).
fn pairing_keys_in(
    dir: &std::path::Path,
    phone_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    computer: bool,
) -> std::result::Result<(), String> {
    if let Some(why) = reused_id_in(dir, phone_id, key, computer) {
        return Err(why);
    }
    owner_paired_in(dir, phone_id, key).map_err(|e| e.to_string())
}

/// Why a pairing under `device_id` on a connection that proved `key` is
/// refused before anything is bound (PC1 re-review I-1): the id is a
/// computer listed here with another certificate, or none proved, which
/// would take that computer's place and lock it out. Asked as a computer
/// or as a phone (`computer`) alike: no phone takes a listed computer's id.
fn reused_id_in(
    dir: &std::path::Path,
    device_id: &str,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
    computer: bool,
) -> Option<String> {
    let file = pocketdesk_core::local::computers::read(dir);
    if !file.computers.iter().any(|c| c.device_id == device_id) {
        return None;
    }
    let other = match key {
        Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) => {
            file.listed_with_other_certificate(device_id, &hex(hash))
        }
        _ => true,
    };
    other.then(|| {
        format!(
            "{device_id} is a computer listed here with another certificate (asked as a {})",
            if computer { "computer" } else { "phone" }
        )
    })
}

/// PC1: whether a computer's pairing makes it one of the owner's own,
/// paired both ways, seeing and controlling this one without asking. It
/// asked (its link said so: a request only, as anyone holding a link can
/// change it), and this computer's own code it paired with (`cli`, the PIN
/// file's) was minted for one: the file decides, never the link. Only the
/// owner's code (no guest's, nor someone else's), and only on a connection
/// that proved a certificate, by which it is listed here in turn.
fn pairs_both_ways(
    computer: bool,
    asked: bool,
    cli: Option<&local_pin::FilePin>,
    someone_else: bool,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> bool {
    computer
        && asked
        && !someone_else
        && cli.is_some_and(|cli| cli.mine && cli.grant_secs.is_none() && !cli.someone_else)
        && matches!(key, Some(pocketdesk_core::streaming::PeerKey::Tls(_)))
}

/// The computer that paired both ways, as this one lists it in turn: what
/// it said of itself in its ask (`facts`), by the id and the certificate
/// its connection proved. None when it said nothing that reads.
fn listed_back(
    device_id: &str,
    body: &serde_json::Value,
    key: Option<&pocketdesk_core::streaming::PeerKey>,
) -> Option<pocketdesk_core::local::computers::PeerComputer> {
    let Some(pocketdesk_core::streaming::PeerKey::Tls(hash)) = key else {
        return None;
    };
    let facts = serde_json::from_value(body.get("facts")?.clone()).ok()?;
    Some(crate::peer_computers::listed_from(
        device_id,
        facts,
        &hex(hash),
        local_pin::now_unix(),
    ))
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

pub(crate) async fn respond_pair(sender: &dyn DeviceSender, paired: bool, server_id: &str) {
    respond_pair_as(sender, paired, server_id, false).await;
}

/// [`respond_pair`], saying `mine` when the computer that paired is now
/// paired both ways (PC1), so it trusts this one in turn.
async fn respond_pair_as(sender: &dyn DeviceSender, paired: bool, server_id: &str, mine: bool) {
    let mut body = json!({
        "pair": paired,
        "serverId": server_id,
    });
    if mine {
        body["mine"] = true.into();
    }
    let response = NetworkPacket::new("kdeconnect.pair").with_body(body);
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

    /// PR #556 review I2b: a nearby ask or pairing never takes an id in
    /// use: one paired here as a phone or as a computer, one listed as a
    /// computer this one paired with, or one bound to another key. A new id
    /// (or one bound to this very key) is free.
    #[test]
    fn a_nearby_pairing_takes_no_id_in_use() {
        use pocketdesk_core::local::computers::{self, ComputerFacts};
        use pocketdesk_core::local::device_keys;
        use pocketdesk_core::local::paths::TRUST_FILE;
        use pocketdesk_core::streaming::PeerKey;
        let dir = std::env::temp_dir().join(format!("pd-nearby-i2b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = PeerKey::Tls([0x5a; 32]);
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("phone").unwrap();
        store.trust_computer("paired-in").unwrap();
        computers::update(&dir, |file| {
            let facts = ComputerFacts {
                name: "Desk".to_string(),
                addresses: vec!["192.0.2.80".to_string()],
                control_port: 1716,
                stream_port: 4716,
                endpoint: None,
            };
            file.computers
                .push(facts.into_peer("paired-out", &"ab".repeat(32), 1));
        })
        .unwrap();
        device_keys::replace(&dir, "bound", &"cd".repeat(32)).unwrap();
        device_keys::replace(&dir, "mine-already", &"5a".repeat(32)).unwrap();
        let taken = |id: &str| super::nearby_id_taken_in(&dir, id, Some(&key)).is_some();
        let seen = [
            taken("phone"),
            taken("paired-in"),
            taken("paired-out"),
            taken("bound"),
            taken("new-desk"),
            taken("mine-already"),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen, [true, true, true, true, false, false]);
    }

    /// PR #556 review C3: a PIN handed to a nearby computer never pairs a
    /// phone. A pairing with it that does not ask as a computer is
    /// refused; one that does pairs as a computer, with a computer's narrow
    /// permissions and none narrowed further; any other PIN is unaffected.
    #[test]
    fn a_nearby_pin_pairs_only_as_a_computer() {
        use super::nearby_as_phone;
        assert!(nearby_as_phone(true, false).is_some(), "asked as a phone");
        assert_eq!(nearby_as_phone(true, true), None);
        assert_eq!(nearby_as_phone(false, false), None, "the owner's own PIN");
        // Forced to a computer: trusted with a computer's permissions,
        // never a phone's full ones (Input, Exec, Power and the rest).
        let computer = true;
        assert_eq!(
            super::preset_permissions(false, false, computer, None, None),
            None
        );
        let mut store = temp_store("nearby-c3");
        store.trust_computer("nearby-desk").unwrap();
        assert!(store.is_computer("nearby-desk"));
        assert_eq!(
            store.permissions("nearby-desk"),
            pocketdesk_core::protocol::permissions::DevicePermissions::computer()
        );
        assert_ne!(
            store.permissions("nearby-desk"),
            pocketdesk_core::protocol::permissions::DevicePermissions::default()
        );
    }

    /// PC1 re-review I-1: a pairing under the id of a computer listed here
    /// with another certificate is refused before any key is touched,
    /// whether it says it is a computer or a phone: the listed computer
    /// keeps its keys, its iroh key too, and nobody else takes them.
    #[test]
    fn a_listed_computers_id_with_another_key_touches_no_key() {
        use pocketdesk_core::local::computers::{self, ComputerFacts};
        use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
        use pocketdesk_core::streaming::PeerKey;
        let dir = std::env::temp_dir().join(format!("pd-pc1-reused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (own, iroh) = ("ab".repeat(32), "e".repeat(64));
        computers::update(&dir, |file| {
            let facts = ComputerFacts {
                name: "Desk".to_string(),
                addresses: vec!["192.0.2.80".to_string()],
                control_port: 1716,
                stream_port: 4716,
                endpoint: None,
            };
            file.computers.push(facts.into_peer("desk", &own, 1));
        })
        .unwrap();
        device_keys::replace(&dir, "desk", &own).unwrap();
        device_keys::register(&dir, "desk", Slot::Iroh, &iroh).unwrap();
        let impostor = PeerKey::Tls([0xcd; 32]);
        let mut refusals = Vec::new();
        for computer in [true, false] {
            refusals.push(pairing_keys_in(&dir, "desk", Some(&impostor), computer).is_err());
            refusals.push(pairing_keys_in(&dir, "desk", None, computer).is_err());
        }
        let tls = device_keys::check(&dir, "desk", Slot::Tls, Some(&own));
        let kept_iroh = device_keys::check(&dir, "desk", Slot::Iroh, Some(&iroh));
        // Its own certificate pairs again, as before.
        let again = pairing_keys_in(&dir, "desk", Some(&PeerKey::Tls([0xab; 32])), true);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(refusals, [true; 4], "refused, as a computer and as a phone");
        assert_eq!(tls, Verdict::Matches, "its certificate stands");
        assert_eq!(kept_iroh, Verdict::Matches, "its iroh key stands");
        assert!(again.is_ok());
    }

    #[test]
    fn a_pin_shown_for_the_phone_follows_the_pair_pages_choice() {
        let file = |someone_else, grant_secs| local_pin::FilePin {
            profile: None,
            pin: "123456".to_string(),
            minted_unix: 0,
            grant_secs,
            someone_else,
            mine: false,
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

    /// PC1: a computer pairs both ways only when it asked and this
    /// computer's own code was minted for one of the owner's computers, on
    /// a connection that proved a certificate. Adding `mine=1` to a link
    /// whose code was not minted so, or taking it off one that was, pairs
    /// one way.
    #[test]
    fn a_computer_pairs_both_ways_only_when_both_sides_said_so() {
        use pocketdesk_core::streaming::PeerKey;
        let code = |mine, grant_secs, someone_else| local_pin::FilePin {
            profile: None,
            pin: "123456".to_string(),
            minted_unix: 0,
            grant_secs,
            someone_else,
            mine,
        };
        let key = Some(PeerKey::Tls([7; 32]));
        let mine = code(true, None, false);
        assert!(pairs_both_ways(
            true,
            true,
            Some(&mine),
            false,
            key.as_ref()
        ));
        // The link's flag alone: this computer's code was not minted so.
        let plain = code(false, None, false);
        assert!(!pairs_both_ways(
            true,
            true,
            Some(&plain),
            false,
            key.as_ref()
        ));
        // The code alone: the link that was pasted did not ask (an old
        // link, or one with the flag taken off).
        assert!(!pairs_both_ways(
            true,
            false,
            Some(&mine),
            false,
            key.as_ref()
        ));
        // A phone, a code shown for a phone (not the file's), a guest's or
        // someone else's code, and a connection with no certificate.
        assert!(!pairs_both_ways(
            false,
            true,
            Some(&mine),
            false,
            key.as_ref()
        ));
        assert!(!pairs_both_ways(true, true, None, false, key.as_ref()));
        let guest = code(true, Some(900), false);
        assert!(!pairs_both_ways(
            true,
            true,
            Some(&guest),
            false,
            key.as_ref()
        ));
        let theirs = code(true, None, true);
        assert!(!pairs_both_ways(
            true,
            true,
            Some(&theirs),
            false,
            key.as_ref()
        ));
        assert!(!pairs_both_ways(
            true,
            true,
            Some(&mine),
            true,
            key.as_ref()
        ));
        assert!(!pairs_both_ways(true, true, Some(&mine), false, None));
    }

    /// PC1: the computer that paired both ways is listed by the id and the
    /// certificate its connection proved, with what it said of itself; an
    /// ask that says nothing that reads lists nothing.
    #[test]
    fn a_computer_paired_both_ways_is_listed_by_what_its_connection_proved() {
        use pocketdesk_core::streaming::PeerKey;
        let body = serde_json::json!({
            "pin": "123456",
            "computer": true,
            "mine": true,
            "facts": {
                "name": "Lap\u{202E}top",
                "addresses": ["192.0.2.41"],
                "controlPort": 1716,
                "streamPort": 4716,
                "endpoint": {"endpointId": "not an iroh key"},
            },
        });
        let key = PeerKey::Tls([0xab; 32]);
        let listed = listed_back("lap", &body, Some(&key)).unwrap();
        assert_eq!(listed.device_id, "lap");
        assert_eq!(listed.fingerprint, "ab".repeat(32));
        assert_eq!(listed.name, "Laptop", "cleaned as a link's name is");
        assert_eq!(listed.addresses, ["192.0.2.41"]);
        assert_eq!(listed.endpoint, None, "not a real iroh key");
        assert!(listed.mine);
        assert!(listed_back("lap", &body, None).is_none(), "no certificate");
        let said_nothing = serde_json::json!({"pin": "123456", "mine": true});
        assert!(listed_back("lap", &said_nothing, Some(&key)).is_none());
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
        let _state = crate::state::use_test_dirs();
        let path = state::trust_store_path().unwrap();
        TrustStore::update(&path, |store| store.trust_device("names-paired-phone")).unwrap();
        note_claimed_name("names-paired-phone", "Galaxy\u{202E} A71");
        note_claimed_name("names-stranger-phone", "Stranger");
        let store = TrustStore::read(&path).unwrap();
        assert_eq!(store.name("names-paired-phone"), Some("Galaxy A71"));
        assert_eq!(store.name("names-stranger-phone"), None);
        TrustStore::update(&path, |store| store.untrust_device("names-paired-phone")).unwrap();
    }

    /// Item 4: a paired device's release, from its identity, is kept
    /// cleaned; a device not paired leaves nothing, and an identity with
    /// none, or with a word that is no version, clears it.
    #[test]
    fn a_paired_devices_release_is_kept_and_an_unpaired_ones_is_not() {
        let _state = crate::state::use_test_dirs();
        let path = state::trust_store_path().unwrap();
        TrustStore::update(&path, |store| store.trust_device("versions-paired-phone")).unwrap();
        note_claimed_version("versions-paired-phone", Some("0.8.1 "));
        note_claimed_version("versions-stranger-phone", Some("0.8.1"));
        let store = TrustStore::read(&path).unwrap();
        assert_eq!(store.version("versions-paired-phone"), Some("0.8.1"));
        assert_eq!(store.version("versions-stranger-phone"), None);
        note_claimed_version("versions-paired-phone", None);
        let store = TrustStore::read(&path).unwrap();
        assert_eq!(store.version("versions-paired-phone"), None, "a downgrade");
        note_claimed_version("versions-paired-phone", Some("0.8.2"));
        note_claimed_version("versions-paired-phone", Some("newest"));
        let store = TrustStore::read(&path).unwrap();
        assert_eq!(store.version("versions-paired-phone"), None, "no version");
        TrustStore::update(&path, |store| store.untrust_device("versions-paired-phone")).unwrap();
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
    fn a_link_request_needs_a_computer_trusted_here() {
        use pocketdesk_core::protocol::permissions::Capability;
        let privacy = crate::plugins::privacy::PRIVACY;
        let mut store = temp_store("link-permits");
        assert!(
            !link_permits_in(Some(&store), "computer-1", privacy),
            "a computer this one paired into alone asks for nothing"
        );
        store.trust_device("computer-1").unwrap();
        let mut permissions = store.permissions("computer-1");
        permissions.set(Capability::Power, true);
        store.set_permissions("computer-1", permissions).unwrap();
        assert!(link_permits_in(Some(&store), "computer-1", privacy));
        let mut permissions = store.permissions("computer-1");
        permissions.set(Capability::Power, false);
        store.set_permissions("computer-1", permissions).unwrap();
        assert!(
            !link_permits_in(Some(&store), "computer-1", privacy),
            "its Power switch off here"
        );
        assert!(!link_permits_in(None, "computer-1", privacy));
    }

    #[test]
    fn a_kept_link_takes_the_clipboard_by_the_permission_here() {
        use pocketdesk_core::protocol::permissions::Capability;
        let mut store = temp_store("kept-link");
        store.trust_device("computer-1").unwrap();
        assert!(
            kept_link_clipboard_in(Some(&store), "computer-1"),
            "paired into this one, with Clipboard"
        );
        let mut permissions = store.permissions("computer-1");
        permissions.set(Capability::Clipboard, false);
        store.set_permissions("computer-1", permissions).unwrap();
        assert!(
            !kept_link_clipboard_in(Some(&store), "computer-1"),
            "its Clipboard switch off here is kept"
        );
        assert!(
            kept_link_clipboard_in(Some(&store), "computer-2"),
            "one this computer paired into alone"
        );
        assert!(
            !kept_link_clipboard_in(None, "computer-2"),
            "no readable store is no"
        );
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

    /// PC10 review: a computer's session marks its mic as needing control,
    /// so the host drops it in a view-only session; a phone's does not.
    #[test]
    fn a_computers_session_needs_control_for_its_mic() {
        let mut store = temp_store("session-mic");
        store.trust_device("phone-m").unwrap();
        store
            .trust_computer_as("desk-m", DevicePermissions::computer_mine())
            .unwrap();
        let desk = session_permissions_in(Some(&store), Some("desk-m"), true).unwrap();
        assert!(desk.mic_needs_control);
        let phone = session_permissions_in(Some(&store), Some("phone-m"), true).unwrap();
        assert!(!phone.mic_needs_control);
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
            // PC1: a computer paired both ways says it recorded the
            // pairing; it gives only what this computer's owner granted, on
            // its link, held to its iroh key.
            "kdeconnect.pocketdesk.both_ways",
            // PC6: how to wake one of the owner's computers, taken only
            // from a computer and kept only for one this computer lists.
            "kdeconnect.pocketdesk.computer_wake",
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
            // Remote revoke: answered only for an owner device (the plugin
            // holds the gate); an answer is taken only for the ask waiting.
            "kdeconnect.pocketdesk.remote_devices",
            "kdeconnect.pocketdesk.remote_devices_answer",
            // Ecosystem phase 4: an owner phone's ask (the plugin holds the
            // gate), a computer's introduction (a computer paired both ways
            // only) and its answer (only for the introduction waiting).
            "kdeconnect.pocketdesk.introduce",
            "kdeconnect.pocketdesk.introduce_phone",
            "kdeconnect.pocketdesk.introduce_phone_answer",
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
        let _state = crate::state::use_test_dirs();
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
        // BK5: a profile never lets this computer drive a phone; that is
        // set on the phone's own page alone.
        let mut drives = DevicePermissions::someone_else();
        drives.set(Capability::SharedInput, true);
        let preset = preset_permissions(false, true, false, None, Some(&drives)).unwrap();
        assert!(!preset.allows(Capability::SharedInput));
    }
}

#[cfg(test)]
mod owner_mark_tests {
    use super::*;

    /// A sender that keeps what it is sent, with the key its TLS proved.
    struct Phone {
        id: String,
        key: [u8; 32],
        sent: std::sync::Mutex<Vec<NetworkPacket>>,
    }

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            &self.id
        }
        fn connection_id(&self) -> &str {
            "owner-mark-test"
        }
        fn peer_key(&self) -> Option<pocketdesk_core::streaming::PeerKey> {
            Some(pocketdesk_core::streaming::PeerKey::Tls(self.key))
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.sent.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// Pair `id` with the PIN `pin`, as its pairing packet asks; whether it
    /// paired, and whether it is an owner phone now.
    async fn pair_with(id: &str, key: u8, pin: &str) -> (bool, bool) {
        let phone = Phone {
            id: id.to_string(),
            key: [key; 32],
            sent: Default::default(),
        };
        let packet = NetworkPacket::new("kdeconnect.pair").with_body(json!({ "pin": pin }));
        handle_pair(id, &packet, &phone).await;
        let paired = phone
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.body["pair"] == true);
        let store = TrustStore::read(state::trust_store_path().unwrap()).unwrap();
        (paired, store.is_owner_phone(id))
    }

    /// Remote revoke (PR #574 review I3): pairing marks a phone as the
    /// owner's only with this computer's own code; a guest grant, someone
    /// else's code, a profile's code (one whose profile is gone too) and a
    /// PIN shown for the phone while the Pair page's code is someone else's
    /// never do.
    #[tokio::test]
    async fn only_the_computers_own_code_makes_an_owner_phone() {
        let _state = state::use_test_dirs();
        let dir = _state.to_path_buf();
        let now = local_pin::now_unix();
        let id = |n: u8| format!("0d7c3a52-91e4-4b6f-8a2e-0000000000{n:02x}");

        local_pin::write(&dir, "411001", now, None).unwrap();
        assert_eq!(
            pair_with(&id(1), 1, "411001").await,
            (true, true),
            "own code"
        );

        local_pin::write(&dir, "411002", now, Some(900)).unwrap();
        assert_eq!(pair_with(&id(2), 2, "411002").await, (true, false), "guest");

        local_pin::write_preset(&dir, "411003", now, None, true, None).unwrap();
        assert_eq!(
            pair_with(&id(3), 3, "411003").await,
            (true, false),
            "someone else's"
        );

        local_pin::write_preset(&dir, "411004", now, None, false, Some("gone")).unwrap();
        assert_eq!(
            pair_with(&id(4), 4, "411004").await,
            (true, false),
            "a profile's, the profile gone"
        );

        // "Show PIN on desktop" while the page's live code is someone else's.
        local_pin::write_preset(&dir, "411005", now, None, true, None).unwrap();
        pins().lock().unwrap().insert(
            id(5),
            PinEntry {
                pin: "722005".to_string(),
                created: Instant::now(),
            },
        );
        assert_eq!(
            pair_with(&id(5), 5, "722005").await,
            (true, false),
            "a PIN shown while the code is someone else's"
        );

        // The owner phone paired again with someone else's code loses it.
        local_pin::write_preset(&dir, "411006", now, None, true, None).unwrap();
        assert_eq!(pair_with(&id(1), 1, "411006").await, (true, false));

        let _ = local_pin::remove_if(&dir, "411005");
        TrustStore::update(state::trust_store_path().unwrap(), |s| {
            for n in 1..=5 {
                s.untrust_device(&id(n))?;
            }
            Ok(())
        })
        .unwrap();
        for n in 1..=5 {
            let _ = pocketdesk_core::local::device_keys::clear(&dir, &id(n));
        }
    }

    /// Ecosystem phase 4 (PR #581 N6): an introduction PIN through
    /// `handle_pair` pairs only its phone on its key, as a phone only, and
    /// three wrong PINs from that phone discard it without touching the
    /// owner's own PIN.
    #[tokio::test]
    // The PIN turn is held through the test on purpose.
    #[allow(clippy::await_holding_lock)]
    async fn an_introduction_pin_pairs_through_handle_pair_and_its_misses_are_its_own() {
        let _turn = crate::introductions::PIN_TURN
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::introductions::PINS.lock().unwrap().clear();
        let _state = state::use_test_dirs();
        let dir = _state.to_path_buf();
        let introducer = "0d7c3a52-91e4-4b6f-8a2e-0000000a1a01";
        let id = |n: u8| format!("0d7c3a52-91e4-4b6f-8a2e-0000000a1b{n:02x}");
        let hex_of = |k: u8| format!("{k:02x}").repeat(32);
        // The introducer, paired both ways here.
        TrustStore::update(state::trust_store_path().unwrap(), |s| {
            s.trust_computer(introducer)
        })
        .unwrap();
        pocketdesk_core::local::computers::update(&dir, |file| {
            file.computers
                .push(pocketdesk_core::local::computers::PeerComputer {
                    device_id: introducer.to_string(),
                    name: "Introducer".to_string(),
                    addresses: vec!["192.0.2.40".to_string()],
                    control_port: 1716,
                    stream_port: 4716,
                    fingerprint: "ab".repeat(32),
                    endpoint: None,
                    paired_unix: 1,
                    link: String::new(),
                    link_unix: 0,
                    lets_drive: false,
                    mine: true,
                    wake: None,
                    viewed_unix: 0,
                    keys_by_position: None,
                });
        })
        .unwrap();
        let introduce = |phone: &str, key: u8| {
            crate::introductions::mint(
                introducer,
                pocketdesk_core::protocol::introductions::Introduction {
                    request: format!("req-{phone}"),
                    phone: pocketdesk_core::protocol::introductions::IntroducedPhone {
                        device_id: phone.to_string(),
                        name: "Pixel".to_string(),
                        tls: hex_of(key),
                        iroh: None,
                    },
                    permissions: DevicePermissions::default(),
                },
            )
            .unwrap()
        };

        // Its own phone, on its own key: paired, an owner phone introduced.
        let pin = introduce(&id(1), 0x61);
        assert_eq!(pair_with(&id(1), 0x61, &pin).await, (true, true));
        let store = TrustStore::read(state::trust_store_path().unwrap()).unwrap();
        assert_eq!(store.introducer(&id(1)), Some(introducer));

        // Another key: the PIN is not taken (and is that phone's miss).
        let pin = introduce(&id(2), 0x62);
        assert_eq!(pair_with(&id(2), 0x63, &pin).await, (false, false));

        // Asked as a computer: refused, nothing trusted.
        let pin = introduce(&id(3), 0x64);
        let phone = Phone {
            id: id(3),
            key: [0x64; 32],
            sent: Default::default(),
        };
        let packet = NetworkPacket::new("kdeconnect.pair")
            .with_body(json!({ "pin": pin, "computer": true }));
        handle_pair(&id(3), &packet, &phone).await;
        assert!(
            !TrustStore::read(state::trust_store_path().unwrap())
                .unwrap()
                .is_trusted(&id(3))
        );

        // Three wrong PINs discard it, never charged to the owner's own
        // PIN's count (which other tests may share: compared, not set).
        let pin = introduce(&id(4), 0x65);
        let wrong = if pin == "000000" { "000001" } else { "000000" };
        let before = guesses().lock().unwrap().wrong;
        for _ in 0..crate::introductions::PIN_MISSES {
            assert_eq!(pair_with(&id(4), 0x65, wrong).await, (false, false));
        }
        assert_eq!(
            guesses().lock().unwrap().wrong,
            before,
            "the owner's own PIN was never charged"
        );
        assert!(
            !crate::introductions::PINS
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.phone.device_id == id(4)),
            "the third discarded it"
        );

        crate::introductions::PINS.lock().unwrap().clear();
        TrustStore::update(state::trust_store_path().unwrap(), |s| {
            s.untrust_device(introducer)?;
            for n in 1..=4 {
                s.untrust_device(&id(n))?;
            }
            Ok(())
        })
        .unwrap();
        pocketdesk_core::local::computers::update(&dir, |file| {
            file.computers.retain(|c| c.device_id != introducer);
            file.introduced.retain(|i| i.computer != introducer);
            file.owed_confirmations.retain(|i| i.computer != introducer);
        })
        .unwrap();
        for n in 1..=4 {
            let _ = pocketdesk_core::local::device_keys::clear(&dir, &id(n));
        }
    }

    #[test]
    fn only_a_plain_code_is_the_owners() {
        assert!(is_owner_code(false, false));
        assert!(!is_owner_code(true, false));
        assert!(!is_owner_code(false, true));
        assert!(!is_owner_code(true, true));
    }
}
