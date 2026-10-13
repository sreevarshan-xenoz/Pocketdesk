use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use linux_link_core::protocol::permissions::DevicePermissions;
use serde_json::json;

use crate::state;
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

/// Is this deviceId in the persisted [`TrustStore`]?
///
/// Used by the QUIC paths (v2 multiplexer + video stream pipeline), which
/// cannot key on a TCP connection: the stream transport's TLS is anonymous,
/// so those paths bind sessions to the deviceId presented in-band (the v2
/// handshake packet / the identity config stream) — the same id pairing
/// stored here.
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

/// [`permits`] against a given store; without one, the defaults apply.
fn permits_in(store: Option<&TrustStore>, device_id: &str, packet_type: &str) -> bool {
    let Some(capability) = linux_link_core::protocol::permissions::capability_for(packet_type)
    else {
        return true;
    };
    match store {
        Some(store) => store.permissions(device_id).allows(capability),
        None => capability.default_allowed(),
    }
}

/// F5: what a stream from [device_id] may do, or `None` to reject it (not
/// paired, or no id announced). Read when the session starts; a live session
/// keeps what it started with.
pub fn session_permissions(device_id: Option<&str>) -> Option<DevicePermissions> {
    let store = state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::load_or_create(path).ok());
    session_permissions_in(store.as_ref(), device_id)
}

fn session_permissions_in(
    store: Option<&TrustStore>,
    device_id: Option<&str>,
) -> Option<DevicePermissions> {
    let (store, id) = (store?, device_id?);
    store.is_trusted(id).then(|| store.permissions(id))
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
        let outcome = TrustStore::load_or_create(state::trust_store_path().expect("state dir"))
            .and_then(|mut store| match grant_secs {
                Some(secs) => {
                    store.trust_device_with_ttl(phone_id.clone(), Duration::from_secs(secs))
                }
                None => store.trust_device(phone_id.clone()),
            });
        match outcome {
            Ok(()) => {
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
            !permits_in(None, "phone-1", "kdeconnect.linuxlink.exec"),
            "no readable store means the defaults, not everything"
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

        assert_eq!(
            session_permissions_in(Some(&store), Some("phone-s")),
            Some(permissions)
        );
        assert_eq!(session_permissions_in(Some(&store), Some("stranger")), None);
        assert_eq!(
            session_permissions_in(Some(&store), None),
            None,
            "no id announced"
        );
        assert_eq!(
            session_permissions_in(None, Some("phone-s")),
            None,
            "no store: not paired"
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
            "kdeconnect.linuxlink.notification_reply",
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
