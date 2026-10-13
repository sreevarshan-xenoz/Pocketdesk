use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use linux_link_core::protocol::permissions::DevicePermissions;
use serde_json::json;

use crate::state;
use linux_link_core::local::approval::Verdict;
use linux_link_core::local::pin as local_pin;

// PIN pairing (KDE Connect parity, R3 Tier-2 #11b).
//
// Two flows, converging on the same `kdeconnect.pair` `{pin}` packet:
// - **push**: the phone requests a PIN (`kdeconnect.linuxlink.pair`
//   `{requestPin:true}`); the desktop generates one, shows it via a desktop
//   notification (and the log), and the phone echoes it back.
// - **pull**: `linux-link pair` on the desktop writes a PIN file; the phone
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

/// connection key (peer IP) -> pending/used PIN
static PINS: OnceLock<Mutex<HashMap<String, PinEntry>>> = OnceLock::new();
/// connection key -> the phone's deviceId as learned from its identity packet
static PHONE_IDS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
/// connection key -> deviceIds that completed pairing on this connection
/// (survives re-registration from a second socket of the same phone)
static TRUSTED_CONNECTIONS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

fn pins() -> &'static Mutex<HashMap<String, PinEntry>> {
    PINS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn phone_ids() -> &'static Mutex<HashMap<String, String>> {
    PHONE_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn trusted_connections() -> &'static Mutex<HashMap<String, Vec<String>>> {
    TRUSTED_CONNECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Is this control connection allowed to use plugins (pairing enforcement)?
/// Always true for connections whose peer device is in the persisted
/// [`TrustStore`], or once pairing succeeded on this connection.
pub fn is_trusted(conn_key: &str) -> bool {
    let phone_id = phone_ids()
        .lock()
        .expect("pair state")
        .get(conn_key)
        .cloned();
    if let Some(id) = &phone_id
        && let Ok(store) = TrustStore::load_or_create(state::trust_store_path().expect("state dir"))
        && store.is_trusted(id)
    {
        return true;
    }
    trusted_connections()
        .lock()
        .expect("pair state")
        .contains_key(conn_key)
}

/// The deviceId the phone at `conn_key` (a TCP peer address) announced in
/// its identity, if it did (F7b: the desktop app names phones by that id).
pub fn phone_id_of(conn_key: &str) -> Option<String> {
    phone_ids()
        .lock()
        .expect("pair state")
        .get(conn_key)
        .cloned()
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

/// F5: may this device send `packet_type`? Packets that use no capability
/// always pass. [conn_key] is a TCP connection's peer IP (resolved to the
/// phone's deviceId learned from its identity) or, on QUIC, the deviceId.
/// The store is re-read per check, so a change in the desktop app applies
/// to the next packet.
pub fn permits(conn_key: &str, packet_type: &str) -> bool {
    let device_id = phone_ids()
        .lock()
        .expect("pair state")
        .get(conn_key)
        .cloned()
        .unwrap_or_else(|| conn_key.to_string());
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::load_or_create(path).ok());
    permits_in(store.as_ref(), &device_id, packet_type)
}

/// [`permits`] against a given store; without one, nothing that needs a
/// permission is allowed.
fn permits_in(store: Option<&TrustStore>, device_id: &str, packet_type: &str) -> bool {
    let Some(capability) = linux_link_core::protocol::permissions::capability_for(packet_type)
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
    claim: linux_link_core::streaming::SessionClaim,
    pairing_required: bool,
) -> Option<DevicePermissions> {
    let dir = state::state_dir().ok()?;
    let paired = claim.device_id.as_deref().is_some_and(is_paired_device);
    let keys = decide_keys_in(&dir, &claim, paired);
    if let Some(refusal) = keys.refused {
        report_refusal(&dir, &claim, refusal, keys.presented.as_deref());
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
pub(crate) fn device_keys_refused(claim: &linux_link_core::streaming::SessionClaim) -> bool {
    let Ok(dir) = state::state_dir() else {
        return true;
    };
    let paired = claim.device_id.as_deref().is_some_and(is_paired_device);
    let keys = decide_keys_in(&dir, claim, paired);
    match keys.refused {
        Some(refusal) => {
            report_refusal(&dir, claim, refusal, keys.presented.as_deref());
            true
        }
        None => false,
    }
}

/// What the device keys say about a session, before the rest of the gate.
#[derive(Debug, Default)]
struct KeyDecision {
    /// Refused, and why.
    refused: Option<linux_link_core::local::device_keys::Refusal>,
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
    claim: &linux_link_core::streaming::SessionClaim,
    paired: bool,
) -> KeyDecision {
    use linux_link_core::local::device_keys::{self, Slot, Verdict};
    use linux_link_core::streaming::PeerKey;
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

/// Apply what an allowed session may record. `false` when another session
/// bound a different key to the device first.
fn commit_keys_in(dir: &std::path::Path, device_id: &str, keys: &KeyDecision) -> bool {
    use linux_link_core::local::device_keys::{self, Slot};
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
    claim: &linux_link_core::streaming::SessionClaim,
    refusal: linux_link_core::local::device_keys::Refusal,
    presented: Option<&str>,
) {
    use linux_link_core::local::device_keys::Refusal;
    let id = claim.device_id.as_deref().unwrap_or("<none>");
    let bound = linux_link_core::local::device_keys::read(dir)
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
             Remove the phone in the Pocketdesk app and pair it again."
        ),
        _ => format!(
            "A connection from {} claimed to be your paired phone {short}… without its key, and was refused.",
            claim.peer.ip()
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

/// The live PIN file written by `linux-link pair` or the desktop app, as
/// `(pin, grant_secs)` (format and expiry: `core::local::pin`).
fn cli_pin() -> Option<(String, Option<u64>)> {
    let dir = state::state_dir().ok()?;
    let file = local_pin::read(&dir, local_pin::now_unix())?;
    Some((file.pin, file.grant_secs))
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
            "kdeconnect.linuxlink.pair",
            "kdeconnect.identity",
        ]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let conn = sender.device_id().to_string();
        match packet.packet_type.as_str() {
            "kdeconnect.identity" => {
                if let Some(id) = packet.body.get("deviceId").and_then(|v| v.as_str()) {
                    phone_ids()
                        .lock()
                        .expect("pair state")
                        .insert(conn.clone(), id.to_string());
                    // A fresh identity announcement resets per-connection trust.
                    trusted_connections()
                        .lock()
                        .expect("pair state")
                        .remove(&conn);
                }
            }
            "kdeconnect.linuxlink.pair" => {
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
            _ => {}
        }
        Ok(())
    }
}

/// Generate + surface a PIN for this connection and tell the phone it is
/// coming. If a CLI PIN file is live, the phone is told to expect manual
/// entry instead.
async fn request_pin(conn: &str, sender: &dyn DeviceSender) {
    if cli_pin().is_some() {
        let response = NetworkPacket::new("kdeconnect.linuxlink.pair").with_body(json!({
            "pairStatus": "pinReady",
        }));
        let _ = sender.send_packet(&response).await;
        return;
    }

    let pin = local_pin::mint_value();
    pins().lock().expect("pair state").insert(
        conn.to_string(),
        PinEntry {
            pin: pin.clone(),
            created: Instant::now(),
        },
    );
    tracing::info!(
        "Pairing PIN for {conn}: {pin} (valid {}s)",
        PIN_TTL.as_secs()
    );
    show_pin_notification(&pin);

    let response = NetworkPacket::new("kdeconnect.linuxlink.pair").with_body(json!({
        "pairStatus": "pinSent",
    }));
    let _ = sender.send_packet(&response).await;
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
    let Some(phone_id) = phone_ids().lock().expect("pair state").get(conn).cloned() else {
        respond_pair(sender, false, "").await;
        return;
    };

    let matches_pending = pins()
        .lock()
        .expect("pair state")
        .get(conn)
        .filter(|e| !pin_expired(e))
        .is_some_and(|e| e.pin == pin);
    // R4 D3: only a CLI PIN can carry a scoped grant; the push flow (a PIN
    // this desktop generated itself) keeps trusting permanently.
    let cli_grant: Option<Option<u64>> = cli_pin()
        .filter(|(cli, _)| cli == pin)
        .map(|(_, grant)| grant);
    let matches_cli = cli_grant.is_some();

    if matches_pending || matches_cli {
        pins().lock().expect("pair state").remove(conn);
        guesses().lock().expect("pair state").reset();
        // One PIN, one phone: a CLI/app PIN that has paired is spent, so a
        // code still on screen (or photographed) cannot pair another.
        if matches_cli && let Ok(dir) = state::state_dir() {
            let _ = local_pin::remove_if(&dir, pin);
        }
        let grant_secs = cli_grant.flatten();
        let outcome = TrustStore::update(state::trust_store_path().expect("state dir"), |store| {
            match grant_secs {
                Some(secs) => {
                    store.trust_device_with_ttl(phone_id.clone(), Duration::from_secs(secs))
                }
                None => store.trust_device(phone_id.clone()),
            }
        });
        match outcome {
            Ok(()) => {
                // Pairing again with the owner's own PIN (perhaps from a
                // reinstalled phone, with new keys) lets the device bind the
                // keys it proves next. A time-boxed guest PIN never does: the
                // id it pairs under is only claimed, and clearing it would
                // hand a guest someone else's phone.
                if grant_secs.is_none()
                    && let Ok(dir) = state::state_dir()
                    && let Err(e) = linux_link_core::local::device_keys::clear(&dir, &phone_id)
                {
                    tracing::warn!("Could not clear the keys of {phone_id}: {e}");
                }
                match grant_secs {
                    Some(secs) => tracing::info!(
                        "Paired phone {phone_id} on connection {conn} (scoped grant: {secs}s)"
                    ),
                    None => tracing::info!("Paired phone {phone_id} on connection {conn}"),
                }
                trusted_connections()
                    .lock()
                    .expect("pair state")
                    .insert(conn.to_string(), vec![phone_id.clone()]);
                respond_pair(sender, true, &desktop_id_field()).await;
            }
            Err(e) => {
                tracing::error!("Pairing succeeded but trust store write failed: {e}");
                respond_pair(sender, false, "").await;
            }
        }
    } else {
        tracing::warn!("Pairing attempt from {conn} with a wrong/expired PIN");
        let current = cli_pin().map(|(cli, _)| cli);
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

async fn respond_pair(sender: &dyn DeviceSender, paired: bool, server_id: &str) {
    let response = NetworkPacket::new("kdeconnect.pair").with_body(json!({
        "pair": paired,
        "serverId": server_id,
    }));
    if let Err(e) = sender.send_packet(&response).await {
        tracing::debug!("Failed to deliver pair response: {e}");
    }
}

/// Show the PIN on the desktop, log fallback — never fails the pairing flow.
fn show_pin_notification(pin: &str) {
    let summary = format!("Pocketdesk pairing PIN: {pin}");
    let body = "Enter it on your phone. It expires in 5 minutes.";
    let handle = std::thread::spawn(move || {
        crate::platform::notify(&summary, body, std::time::Duration::from_secs(300))
    });
    if !handle.join().unwrap_or(false) {
        tracing::info!("No desktop notification available; PIN is only in the log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::streaming::{PeerKey, SessionClaim};

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
            linux_link_core::local::device_keys::read(&dir)
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
        use linux_link_core::protocol::permissions::Capability;
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
                "kdeconnect.linuxlink.notification_reply"
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
            !permits_in(Some(&store), "phone-1", "kdeconnect.linuxlink.exec"),
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
        use linux_link_core::protocol::permissions::Capability;
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
        use linux_link_core::protocol::permissions::capability_for;
        const ALWAYS_ALLOWED: &[&str] = &[
            "kdeconnect.identity",
            "kdeconnect.pair",
            "kdeconnect.linuxlink.pair",
            "kdeconnect.battery",
            "kdeconnect.battery.request",
            "kdeconnect.notification",
            "kdeconnect.notification.request",
            "kdeconnect.linuxlink.monitors",
            "kdeconnect.linuxlink.windows",
            "kdeconnect.linuxlink.wol",
            "kdeconnect.findmydevice",
            "kdeconnect.mousepad.echo",
            "kdeconnect.filebrowse.response",
        ];
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
        assert!(caps.contains(&"kdeconnect.linuxlink.pair"));
    }

    #[test]
    fn unknown_device_is_not_paired() {
        // QUIC paths lean on this being a strict deny-by-default lookup.
        assert!(!is_paired_device("00000000-0000-0000-0000-00000000dead"));
    }
}
