//! iroh WAN endpoint lifecycle (R1 stage 3, server slice).
//!
//! The quinn listener stays the LAN transport; this module owns one always-on
//! iroh endpoint with a persisted identity, feeding accepted v1 streaming
//! connections into the same [`StreamingServer`] pipeline. The endpoint's
//! identity (`EndpointId` plus current relay URLs and direct addresses) is
//! pushed to registered control-channel clients as a
//! `kdeconnect.linuxlink.endpoint` packet — the phone caches it and dials the
//! full `EndpointAddr` later, so neither side needs pkarr or any address
//! discovery infrastructure (relays are used only for hole punching).

use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::endpoint::QuicTransportConfig;
use iroh::endpoint::presets::Minimal;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey};
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket};
use linux_link_core::streaming::input_packet::InputPacket;
use linux_link_core::streaming::transport::{CertManager, StreamTransportConfig};
use linux_link_core::streaming::{IrohConnection, StreamingConfig, StreamingServer};
use tokio::sync::broadcast;

/// Packet type carrying the server's WAN identity to registered clients.
pub const ENDPOINT_PACKET: &str = "kdeconnect.linuxlink.endpoint";

/// Persisted iroh identity key, next to `config.toml`.
const KEY_FILE: &str = "iroh_secret.key";

fn key_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("unable to determine config directory")?
        .join("linux-link")
        .join(KEY_FILE))
}

/// Load the endpoint identity from `path`, creating one (mode 0600) if absent.
///
/// The key file is the server's iroh identity — losing it changes the
/// `EndpointId` the phone has cached, so a corrupt or truncated file is a hard
/// error rather than a silent regenerate.
pub fn load_or_create_secret_key(path: &Path) -> Result<SecretKey> {
    if path.exists() {
        let bytes =
            std::fs::read(path).with_context(|| format!("failed reading {}", path.display()))?;
        let raw: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
            anyhow::anyhow!(
                "{} is not a 32-byte iroh secret key — restore it or delete it \
                     and re-pair (the phone caches the EndpointId)",
                path.display()
            )
        })?;
        return Ok(SecretKey::from_bytes(&raw));
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let key = SecretKey::generate();
    // create_new: if a second server instance won the race, adopt its key
    // rather than silently diverging identities.
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path);
    match file {
        Ok(mut file) => {
            file.write_all(&key.to_bytes())
                .with_context(|| format!("failed writing {}", path.display()))?;
            // Unix got 0600 at creation; Windows gets the owner-only DACL.
            #[cfg(windows)]
            {
                drop(file);
                linux_link_core::secure_fs::restrict_to_owner(path)
                    .with_context(|| format!("failed restricting {}", path.display()))?;
            }
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("failed reading {}", path.display()))?;
            let raw: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("{} has unexpected length", path.display()))?;
            Ok(SecretKey::from_bytes(&raw))
        }
        Err(e) => Err(e).with_context(|| format!("failed creating {}", path.display())),
    }
}

/// WAN identity announcement built from an [`EndpointAddr`].
pub fn endpoint_packet(addr: &EndpointAddr, id: EndpointId) -> NetworkPacket {
    NetworkPacket::new(ENDPOINT_PACKET).with_body(serde_json::json!({
        "endpointId": id.to_string(),
        "relayUrls": addr.relay_urls().map(|u| u.to_string()).collect::<Vec<_>>(),
        "directAddrs": addr.ip_addrs().map(|a| a.to_string()).collect::<Vec<_>>(),
    }))
}

/// Write the endpoint body to `<state_dir>/endpoint.json` if it differs from
/// `last`; returns whether it wrote. The desktop app's pairing code reads it.
pub fn record_endpoint_in(
    state_dir: &Path,
    addr: &EndpointAddr,
    id: EndpointId,
    last: &mut Option<serde_json::Value>,
) -> bool {
    let body = endpoint_packet(addr, id).body;
    if last.as_ref() == Some(&body) {
        return false;
    }
    match linux_link_core::local::identity::write_endpoint(state_dir, &body) {
        Ok(()) => {
            *last = Some(body);
            true
        }
        Err(e) => {
            tracing::debug!("could not record the WAN endpoint: {e}");
            false
        }
    }
}

/// Keep `endpoint.json` current: the relay URL only appears once iroh has
/// reached its home relay, and direct addresses change with the network.
fn spawn_endpoint_recorder(endpoint: Endpoint) {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    tokio::spawn(async move {
        let mut last = None;
        loop {
            record_endpoint_in(&dir, &endpoint.addr(), endpoint.id(), &mut last);
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

/// Push the current WAN identity to one freshly-registered client.
pub async fn push_endpoint_to(sender: &Arc<dyn DeviceSender>, endpoint: &Endpoint) {
    let _ = sender
        .send_packet(&endpoint_packet(&endpoint.addr(), endpoint.id()))
        .await;
}

/// Bind the WAN endpoint and spawn its accept loop.
///
/// Each accepted connection runs one v1 streaming session — identical to the
/// quinn LAN arm in `service.rs`, just over iroh/noq and reachable from
/// outside the LAN. With `pairing_required` the session is gated on the
/// deviceId the client announces in-band, exactly like the LAN arm.
/// Errors are logged per-connection; the loop lives until the endpoint is
/// closed.
pub async fn spawn_wan_endpoint(
    config: StreamingConfig,
    input_tx: broadcast::Sender<InputPacket>,
    cert_manager: Arc<CertManager>,
    pairing_required: bool,
    allow_hevc: bool,
    capture_backend: linux_link_core::streaming::CaptureBackend,
    mic_relay_device: Option<String>,
) -> Result<Endpoint> {
    let secret_key =
        load_or_create_secret_key(&key_path()?).context("failed to load iroh identity key")?;

    // Mirror the LAN stream budget: the pipeline opens one uni stream per
    // video frame. Relays (n0 defaults) are for hole punching only — dialing
    // rides the EndpointAddr announced over the control channel.
    let transport = QuicTransportConfig::builder()
        .max_concurrent_uni_streams(1024u32.into())
        .max_concurrent_bidi_streams(128u32.into())
        .build();
    let endpoint = Endpoint::builder(Minimal)
        .alpns(vec![StreamTransportConfig::default().alpn])
        .relay_mode(RelayMode::Default)
        .secret_key(secret_key)
        .transport_config(transport)
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], 0)))
        .context("iroh bind_addr rejected wildcard socket addr")?
        .bind()
        .await
        .context("iroh endpoint bind failed")?;

    tracing::info!(id = %endpoint.id(), "WAN (iroh) endpoint ready");
    spawn_endpoint_recorder(endpoint.clone());

    let accept = endpoint.clone();
    tokio::spawn(async move {
        loop {
            let Some(incoming) = accept.accept().await else {
                tracing::info!("WAN endpoint closed — accept loop exiting");
                break;
            };
            let config = config.clone();
            let input_tx = input_tx.clone();
            let cert_manager = Arc::clone(&cert_manager);
            let mic_relay_device = mic_relay_device.clone();
            tokio::spawn(async move {
                let conn = match incoming.accept() {
                    Ok(accepting) => match accepting.await {
                        Ok(conn) => conn,
                        Err(e) => {
                            tracing::warn!("WAN handshake failed: {e}");
                            return;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("WAN connection rejected: {e}");
                        return;
                    }
                };
                let mut streaming_server =
                    StreamingServer::new(config, StreamTransportConfig::default(), cert_manager);
                streaming_server.set_input_channel(input_tx);
                // R4 E2: WAN sessions get the same phone-mic relay as LAN.
                let (mic_tx, mic_rx) = tokio::sync::mpsc::channel(64);
                tokio::spawn(crate::mic_relay::run_mic_relay(mic_rx, mic_relay_device));
                streaming_server.set_mic_channel(mic_tx);
                streaming_server.set_session_telemetry(true);
                streaming_server.set_hevc_allowed(allow_hevc);
                streaming_server.set_capture_backend(capture_backend);
                if pairing_required {
                    streaming_server.set_pairing_gate(|device_id| {
                        device_id
                            .as_deref()
                            .is_some_and(crate::plugins::pair::is_paired_device)
                    });
                }
                if let Err(e) = streaming_server
                    .run_on_connection(IrohConnection::shared(conn))
                    .await
                {
                    tracing::error!("WAN streaming session error: {e}");
                }
            });
        }
    });

    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::TransportAddr;

    fn temp_key_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "linux-link-iroh-test-{}-{label}.key",
            std::process::id()
        ))
    }

    #[test]
    fn secret_key_persists_and_is_private() {
        let path = temp_key_path("persist");
        let _ = std::fs::remove_file(&path);

        let first = load_or_create_secret_key(&path).expect("create");
        let second = load_or_create_secret_key(&path).expect("load");
        assert_eq!(
            first.to_bytes(),
            second.to_bytes(),
            "identity must be stable across restarts"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "secret key must be owner-only");
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupt_key_file_is_a_hard_error() {
        let path = temp_key_path("corrupt");
        std::fs::write(&path, b"not-a-key").unwrap();
        let err = load_or_create_secret_key(&path).unwrap_err();
        assert!(err.to_string().contains("32-byte"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn endpoint_packet_lists_identity_and_addresses() {
        let id: EndpointId = SecretKey::from_bytes(&[7u8; 32]).public();
        let addr = EndpointAddr::from_parts(
            id,
            [TransportAddr::Ip(
                "203.0.113.7:4501".parse::<SocketAddr>().unwrap(),
            )],
        );
        let packet = endpoint_packet(&addr, id);
        assert_eq!(packet.packet_type, ENDPOINT_PACKET);
        let body: serde_json::Value =
            serde_json::from_str(&packet.body.to_string()).expect("body json");
        assert_eq!(body["endpointId"], id.to_string());
        assert_eq!(body["directAddrs"][0], "203.0.113.7:4501");
        assert!(body["relayUrls"].as_array().unwrap().is_empty());
    }

    /// The desktop app's pairing code carries `endpoint.json`; it must be
    /// exactly the body phones already receive, and rewritten only on change.
    #[test]
    fn the_recorded_endpoint_is_the_packet_body() {
        let dir = std::env::temp_dir().join(format!("ll-ep-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id: EndpointId = SecretKey::from_bytes(&[9u8; 32]).public();
        let addr = EndpointAddr::from_parts(
            id,
            [TransportAddr::Ip(
                "198.51.100.4:7".parse::<SocketAddr>().unwrap(),
            )],
        );
        let mut last = None;
        assert!(record_endpoint_in(&dir, &addr, id, &mut last));
        assert_eq!(
            linux_link_core::local::identity::read_endpoint(&dir),
            Some(endpoint_packet(&addr, id).body)
        );
        assert!(
            !record_endpoint_in(&dir, &addr, id, &mut last),
            "an unchanged address is not rewritten"
        );
    }
}
