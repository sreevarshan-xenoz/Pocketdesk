use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use linux_link_core::error::Result;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
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
