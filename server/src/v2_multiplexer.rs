use anyhow::Context;
use quinn::{Connection, ConnectionError, RecvStream, SendStream};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{Instrument, debug, info, info_span, trace, warn};

use linux_link_core::error::LinuxLinkError;
use linux_link_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, PluginRegistry};
use linux_link_core::protocol::v2::{
    ChannelKind, IdentityPacketV2, perform_v2_handshake, read_framed_json, write_framed_json,
};

use crate::state;

/// Handles a v2 multiplexed QUIC session.
///
/// With `pairing_required`, the control channel mirrors the TCP gate in
/// `handle_connection_with_kde`: until the peer's deviceId is in the
/// persisted TrustStore, only the pairing handshake packets are dispatched
/// and everything else from this device is dropped.
pub async fn handle_v2_session(
    conn: Connection,
    local_identity: IdentityPacketV2,
    registry: Arc<PluginRegistry>,
    pairing_required: bool,
) -> anyhow::Result<()> {
    let peer_addr = conn.remote_address();
    let session_id = uuid::Uuid::new_v4().to_string();

    let span = info_span!(
        "v2_session",
        session = %session_id,
        peer = ?peer_addr,
        proto = "v2"
    );

    async move {
        info!("New v2 session established");

        // Step 1: Accept the first bidirectional stream for handshake
        let (mut send0, mut recv0) = conn
            .accept_bi()
            .await
            .context("failed to accept handshake stream")?;

        let peer_identity = perform_v2_handshake(&mut send0, &mut recv0, &local_identity).await?;

        tracing::Span::current().record("peer_id", &peer_identity.device_id);
        info!(
            "Handshake successful with {} ({})",
            peer_identity.device_name, peer_identity.device_id
        );

        // Wrap Stream 0 for use with existing v1 plugins (control channel)
        let sender: Arc<dyn DeviceSender> = Arc::new(QuicDeviceSender::new(
            send0,
            peer_identity.device_id.clone(),
            session_id.clone(),
        ));

        // Register v2 client for pushes (one target per device: its newest connection)
        state::register_client(Arc::clone(&sender)).await;

        // Spawn a dedicated task for the control stream (Stream 0) to ensure cancellation safety
        let control_registry = Arc::clone(&registry);
        let control_sender = Arc::clone(&sender);
        let control_pairing_required = pairing_required;
        let control_device_id = peer_identity.device_id.clone();
        let control_task = tokio::spawn(async move {
            loop {
                match read_framed_json::<NetworkPacket>(&mut recv0).await {
                    Ok(p) => {
                         let packet_type = p.packet_type.clone();

                         // PIN pairing enforcement (mirrors the TCP dispatch gate
                         // in handle_connection_with_kde): until this device is in
                         // the TrustStore, only the pairing handshake itself is
                         // dispatched — every other plugin packet is dropped. The
                         // v2 handshake already exchanged real device ids, so the
                         // persisted TrustStore is the source of truth here.
                         let pairing_packet = matches!(
                             packet_type.as_str(),
                             "kdeconnect.identity"
                                 | "kdeconnect.pair"
                                 | "kdeconnect.linuxlink.pair"
                         );
                         if control_pairing_required
                             && !pairing_packet
                             && !crate::plugins::pair::is_paired_device(&control_device_id)
                         {
                             debug!(
                                 "Blocking {packet_type} from unpaired {} \
                                  (set pairing_required = false to disable)",
                                 control_device_id
                             );
                             continue;
                         }
                         // F5: the owner's per-device permissions (spec P3).
                         if !pairing_packet
                             && !crate::plugins::pair::permits(&control_device_id, &packet_type)
                         {
                             tracing::info!(
                                 "Denied {packet_type} from {control_device_id}: not permitted for this device"
                             );
                             continue;
                         }

                         let packet_span = tracing::debug_span!("packet", type = %packet_type);
                         let registry = Arc::clone(&control_registry);
                         let sender = Arc::clone(&control_sender);

                         tokio::spawn(async move {
                             debug!("Processing v1-over-v2 control packet");
                             if let Err(e) = registry.dispatch_packet(&p, &*sender).await {
                                 warn!("Packet dispatch failed: {}", e);
                             }
                         }.instrument(packet_span));
                    }
                    Err(e) => {
                        warn!("Control stream ended: {}", e);
                        break;
                    }
                }
            }
        }.instrument(info_span!("control_stream")));

        // Main loop to accept new streams
        let mut control_task = control_task;
        loop {
            tokio::select! {
                uni = conn.accept_uni() => {
                    match uni {
                        Ok(recv) => {
                            tokio::spawn(handle_unidirectional_stream(recv).instrument(info_span!("uni_stream")));
                        }
                        Err(e) => {
                            handle_conn_error(e);
                            break;
                        }
                    }
                }
                bi = conn.accept_bi() => {
                    match bi {
                        Ok((send, recv)) => {
                            tokio::spawn(handle_bidirectional_stream(send, recv).instrument(info_span!("bi_stream")));
                        }
                        Err(e) => {
                            handle_conn_error(e);
                            break;
                        }
                    }
                }
                _ = &mut control_task => {
                    info!("Control task finished, closing session");
                    break;
                }
            }
        }

        // Cleanup
        control_task.abort(); // Ensure the control task is terminated immediately
        state::unregister_client(&session_id).await;

        info!("v2 session ended");
        Ok(())
    }
    .instrument(span)
    .await
}

fn handle_conn_error(e: ConnectionError) {
    match e {
        ConnectionError::ApplicationClosed(close) => {
            info!("Session closed by peer: {:?}", close);
        }
        ConnectionError::LocallyClosed => {
            debug!("Session closed locally");
        }
        _ => {
            warn!("Connection error: {}", e);
        }
    }
}

async fn handle_unidirectional_stream(mut recv: RecvStream) -> anyhow::Result<()> {
    let mut header = [0u8; 1];
    recv.read_exact(&mut header).await?;
    let kind_raw = header[0];

    // Map raw byte to ChannelKind
    if kind_raw == ChannelKind::Video as u8 {
        debug!("Incoming Video stream");
        handle_persistent_stream(recv, "video").await
    } else if kind_raw == ChannelKind::Audio as u8 {
        debug!("Incoming Audio stream");
        handle_persistent_stream(recv, "audio").await
    } else if kind_raw == ChannelKind::Input as u8 {
        debug!("Incoming Input stream (Reliable Uni)");
        handle_persistent_stream(recv, "input").await
    } else if kind_raw == ChannelKind::File as u8 {
        debug!("Incoming File stream (Reliable Uni)");
        handle_persistent_stream(recv, "file").await
    } else {
        Err(LinuxLinkError::ProtocolError {
            detail: format!("Unknown unidirectional stream kind: {}", kind_raw),
        }
        .into())
    }
}

async fn handle_bidirectional_stream(
    _send: SendStream,
    mut recv: RecvStream,
) -> anyhow::Result<()> {
    let mut header = [0u8; 1];
    recv.read_exact(&mut header).await?;
    let kind_raw = header[0];

    Err(LinuxLinkError::ProtocolError {
        detail: format!("Unexpected bidirectional stream kind: {}", kind_raw),
    }
    .into())
}

async fn handle_persistent_stream(mut recv: RecvStream, name: &'static str) -> anyhow::Result<()> {
    debug!("Starting persistent handler for {} stream", name);
    let mut buf = [0u8; 8192];
    loop {
        match recv.read(&mut buf).await {
            Ok(Some(n)) => {
                // For now, just discard the data.
                // Future PRs will route this to actual consumers.
                trace!("Read {} bytes from {} stream", n, name);
            }
            Ok(None) => {
                debug!("{} stream closed by peer", name);
                break;
            }
            Err(e) => {
                warn!("Error reading from {} stream: {}", name, e);
                return Err(e.into());
            }
        }
    }
    Ok(())
}

/// DeviceSender implementation that uses a QUIC SendStream with v2 framing.
struct QuicDeviceSender {
    send: Arc<Mutex<SendStream>>,
    device_id: String,
    connection_id: String,
}

impl QuicDeviceSender {
    fn new(send: SendStream, device_id: String, connection_id: String) -> Self {
        Self {
            send: Arc::new(Mutex::new(send)),
            device_id,
            connection_id,
        }
    }
}

#[async_trait::async_trait]
impl DeviceSender for QuicDeviceSender {
    fn device_id(&self) -> &str {
        &self.device_id
    }

    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    async fn send_packet(&self, packet: &NetworkPacket) -> std::result::Result<(), LinuxLinkError> {
        let mut guard = self.send.lock().await;
        write_framed_json(&mut guard, packet).await?;
        Ok(())
    }
}
