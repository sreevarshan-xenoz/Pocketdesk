use crate::config::Config;
use crate::hypr_events;
use crate::hyprland::HyprlandIpc;
use crate::input_injector::InputInjector;
use crate::iroh_endpoint;
use crate::kde;
use crate::notification_monitor::start_notification_monitor;
use crate::state;
use crate::v2_multiplexer::handle_v2_session;
use anyhow::{Context, Result, bail};
use pocketdesk_core::local::pin as local_pin;
use pocketdesk_core::protocol::ALPN_V1_STREAM;
use pocketdesk_core::protocol::connection::ConnectionManager;
use pocketdesk_core::protocol::kdeconnect::{
    DeviceIdentity, DeviceSender, NetworkPacket, PluginRegistry, TcpDeviceSender, TrustStore,
};
use pocketdesk_core::protocol::v2::{ALPN_V2, IdentityPacketV2, V2_MAX_VERSION, V2_MIN_VERSION};
use pocketdesk_core::protocol::{HANDSHAKE_HELLO, HANDSHAKE_OK};
use pocketdesk_core::streaming::QuinnConnection;
use pocketdesk_core::streaming::StreamingServer;
use pocketdesk_core::streaming::input_packet::InputPacket;
use pocketdesk_core::streaming::transport::{CertManager, StreamTransportConfig};
use pocketdesk_core::tailscale::{DiscoveryEvent, DiscoveryService, TailscaleClient};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid;

pub async fn run(config: Config) -> Result<()> {
    let pid_file = state::pid_file_path()?;
    state::write_pid_file(&pid_file)?;
    let _pid_guard = PidFileGuard { path: pid_file };
    let mut shutdown = crate::platform::shutdown_requests();
    state::set_pairing_required(config.pairing_required);

    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    let tailscale_ready = match tailscale.wait_for_ready(Duration::from_secs(5)).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("Tailscale not ready: {e}");
            false
        }
    };

    let mut local_ips = pocketdesk_core::tailscale::lan::get_local_ips().await;
    if tailscale_ready && let Ok(ip) = tailscale.get_self_ip().await {
        tracing::info!("Tailscale online at {}", ip);
        if !local_ips.contains(&ip) {
            local_ips.push(ip);
        }
    }

    let kde_service = kde::build_default_service().context("failed to initialize KDE service")?;
    let plugin_count = kde_service.registry.plugin_names().len();
    let trusted_count = kde_service
        .trust_store
        .as_ref()
        .map(|s| s.trusted_devices().len())
        .unwrap_or(0);
    tracing::info!(
        "KDE service initialized (plugins={}, trusted_devices={})",
        plugin_count,
        trusted_count
    );

    // This computer's mDNS announcement (roadmap F6), made once the control
    // listener is up.
    let lan_discovery = pocketdesk_core::tailscale::lan::LanDiscoveryService::new()?;
    let (host_name, host_id) = kde_service
        .identity
        .as_ref()
        .map(|i| (i.device_name.clone(), i.device_id.clone()))
        .unwrap_or_else(|| ("pocketdesk-host".to_string(), String::new()));

    let discovery = DiscoveryService::new(tailscale.clone());
    let mut discovery_rx = discovery.subscribe();
    tokio::spawn(async move {
        discovery.run(Duration::from_secs(10)).await;
    });

    // R4 D2: mirror live streaming sessions to the state dir, notify the
    // desktop when one starts, and consume `pocketdesk kick` requests.
    tokio::spawn(crate::live_sessions::run_watcher());
    // F7b: files the desktop app sends to a phone in a session.
    tokio::spawn(crate::offers::run_watcher());
    tokio::spawn(crate::plugins::sms::run_watcher());

    // Prepare shared state for v2 multiplexer and v1 streaming.
    // The QUIC identity must be STABLE across restarts: clients pin it with
    // TOFU, so a regenerated cert makes every phone reject LAN streaming
    // until its pin is cleared.
    let identity_dir = crate::state::state_dir()?.join("certs");
    let cert_manager = Arc::new(
        CertManager::load_or_create(&identity_dir).context("Failed to create CertManager")?,
    );
    // The control channel runs inside TLS with the same identity (phase B).
    let control_tls = tokio_rustls::TlsAcceptor::from(
        cert_manager
            .control_server_config()
            .context("Failed to configure the control channel's TLS")?,
    );
    let registry = Arc::new(kde_service.registry.clone_for_dispatch());
    let local_v2_identity = IdentityPacketV2 {
        device_id: kde_service
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .unwrap_or_default(),
        device_name: host_name.clone(),
        min_version: V2_MIN_VERSION,
        max_version: V2_MAX_VERSION,
        capabilities: registry.plugin_names(),
    };

    let bind_addr = format!("0.0.0.0:{}", config.control_port);
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind {bind_addr}"))?;

    tracing::info!("Control listener ready on {}", bind_addr);

    // Announced only now that the control port answers: a phone checks it
    // before listing this computer.
    crate::plugins::notification::configure(
        config.phone_notifications,
        config.notifications_per_minute,
    );
    crate::plugins::sms::configure(config.sms_notify, config.sms_keep);
    let announcement = if config.lan_discovery {
        Some(lan_discovery.register_service(
            &host_name,
            &host_id,
            config.control_port,
            config.streaming_port,
            local_ips,
        )?)
    } else {
        tracing::info!("Not announced on the local network (lan_discovery = false)");
        None
    };

    // Also run LAN discovery browser to detect other Pocketdesk servers
    let mut lan_rx = lan_discovery.subscribe();
    tokio::spawn(async move {
        lan_discovery.run(Duration::from_secs(30)).await;
    });

    // QUIC Multiplexer (v2) and Streaming (v1) listener
    let streaming_port = config.streaming_port;
    let quic_addr = format!("0.0.0.0:{}", streaming_port)
        .parse::<std::net::SocketAddr>()
        .with_context(|| format!("invalid QUIC listen address for port {streaming_port}"))?;
    let alpns = vec![ALPN_V2.to_vec(), ALPN_V1_STREAM.to_vec()];
    let server_config = cert_manager
        .server_config(alpns)
        .context("Failed to create QUIC server config")?;
    let quic_endpoint = quinn::Endpoint::server(server_config, quic_addr)
        .context("Failed to bind QUIC endpoint")?;
    tracing::info!("Unified QUIC listener ready on {}", quic_addr);

    // F19: Start notification monitor for forwarding PC notifications to Android clients
    let notification_tx = start_notification_monitor();
    // Spawn a task that broadcasts captured notifications to all connected clients
    {
        let mut notify_rx = notification_tx.subscribe();
        tokio::spawn(async move {
            tracing::info!("Notification broadcast task started");
            loop {
                match notify_rx.recv().await {
                    Ok(notification) if notification.is_own() => {}
                    Ok(notification) => {
                        let packet_json = notification.to_kdeconnect_payload();
                        match NetworkPacket::from_wire(&packet_json) {
                            Ok(packet) => {
                                // Optimized: Clone the list of clients and release the lock immediately
                                // to prevent slow clients from blocking the entire registration system.
                                let clients = state::clone_clients().await;

                                let mut dead_clients = Vec::new();
                                for sender in clients.iter() {
                                    if let Err(e) = sender.send_packet(&packet).await {
                                        tracing::debug!(
                                            "Detected dead client during broadcast: {e}"
                                        );
                                        dead_clients.push(sender.connection_id().to_string());
                                    }
                                }

                                // Prune dead clients if any were detected
                                if !dead_clients.is_empty() {
                                    state::prune_dead_clients(&dead_clients).await;
                                }

                                if !clients.is_empty() {
                                    tracing::debug!(
                                        "Forwarded notification to {} client(s)",
                                        clients.len()
                                    );
                                }
                            }
                            Err(e) => tracing::warn!("Failed to parse notification packet: {e}"),
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("Notification broadcast channel closed");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Notification broadcast lagged by {n} messages");
                    }
                }
            }
        });
    }

    // R3#8: Workspace HUD — forward Hyprland socket2 events to connected
    // clients. Absent compositor (non-Hyprland session) simply skips this.
    let hypr_ipc: Option<Arc<HyprlandIpc>> = if HyprlandIpc::is_available() {
        HyprlandIpc::from_env().ok().map(Arc::new)
    } else {
        None
    };
    if let Some(ipc) = hypr_ipc.as_ref() {
        // The client's packet poller re-subscribes per drain, so a register-time
        // push can fall into a subscription gap; periodic snapshots let the HUD
        // self-heal. Events stay for instant response between snapshots.
        let ipc_snap = ipc.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(4));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                hypr_events::push_state_to_clients(&ipc_snap).await;
            }
        });

        let ipc_events = ipc.clone();
        let mut ev_rx = hypr_events::start_hypr_event_monitor((**ipc).clone()).subscribe();
        tokio::spawn(async move {
            tracing::info!("Hyprland event broadcast task started");
            loop {
                match ev_rx.recv().await {
                    Ok(ev) => {
                        if hypr_events::is_monitor_event(&ev.name) {
                            // The output layout changed, so every geometry the
                            // HUD has is now wrong: re-snapshot now instead of
                            // waiting for the next tick (roadmap 2066).
                            tracing::info!(
                                event = %ev.name,
                                data = %ev.data,
                                "Hyprland output changed, refreshing compositor state"
                            );
                            hypr_events::push_state_to_clients(&ipc_events).await;
                        }
                        let Some(packet) = hypr_events::event_packet(&ev) else {
                            continue;
                        };
                        let clients = state::clone_clients().await;
                        let mut dead_clients = Vec::new();
                        for sender in clients.iter() {
                            if let Err(e) = sender.send_packet(&packet).await {
                                tracing::debug!("Detected dead client during Hyprland push: {e}");
                                dead_clients.push(sender.connection_id().to_string());
                            }
                        }
                        if !dead_clients.is_empty() {
                            state::prune_dead_clients(&dead_clients).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("Hyprland event broadcast channel closed");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Hyprland event broadcast lagged by {n} messages");
                    }
                }
            }
        });
    }

    // The Windows desktop has no compositor IPC; its HUD is fed by snapshots.
    #[cfg(windows)]
    hypr_events::start_windows_hud();

    // Input injection channel shared between v1 and v2
    let (input_tx, mut input_rx) = tokio::sync::broadcast::channel::<InputPacket>(16);
    let streaming_config = config.video_quality.to_streaming_config();

    // R1 stage 3: WAN (iroh) endpoint alongside the quinn LAN listener.
    // A bind failure (no network, relays unreachable) must not take the LAN
    // experience down, so the endpoint is optional at runtime.
    let wan_endpoint = match iroh_endpoint::spawn_wan_endpoint(
        streaming_config.clone(),
        input_tx.clone(),
        Arc::clone(&cert_manager),
        iroh_endpoint::WanSettings {
            pairing_required: config.pairing_required,
            allow_hevc: config.allow_hevc,
            capture_backend: config.capture_backend,
            mic_relay_device: config.mic_relay_device.clone(),
            relay_urls: config.relay_urls.clone(),
            dht_publish: config.dht_publish,
        },
    )
    .await
    {
        Ok(endpoint) => Some(endpoint),
        Err(e) if !config.relay_urls.is_empty() => {
            // Most likely the user's own relay_urls; say so loudly.
            tracing::error!("WAN (iroh) endpoint unavailable, away from home is off: {e:#}");
            None
        }
        Err(e) => {
            tracing::warn!("WAN (iroh) endpoint unavailable: {e:#}");
            None
        }
    };
    if let Some(endpoint) = wan_endpoint.as_ref() {
        // Relay URLs and direct addresses drift (relay reconnects, DHCP), and
        // the phone only ever dials its cached EndpointAddr — periodic
        // snapshots let that cache self-heal.
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let packet = iroh_endpoint::current_packet(&endpoint);
                let clients = state::clone_clients().await;
                for sender in clients.iter() {
                    let _ = sender.send_packet(&packet).await;
                }
            }
        });
    }

    // Spawn input injection task — receives InputPacket from the QUIC
    // streaming channel and injects them into the host system.
    tokio::spawn(async move {
        tracing::info!("Input injection task started");
        let mut injector = match InputInjector::new() {
            Ok(inj) => inj,
            Err(e) => {
                tracing::error!("Failed to create InputInjector: {e}");
                return;
            }
        };

        loop {
            match input_rx.recv().await {
                Ok(packet) => {
                    if let Err(e) = handle_input_packet(&mut injector, packet) {
                        tracing::warn!("Input injection error: {e}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("Input injection lagged by {n} messages, dropped oldest");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }

        tracing::info!("Input injection task ended");
    });

    tracing::info!("Press Ctrl+C to stop");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer_addr)) => {
                        tracing::info!("Incoming v1 TCP connection from {}", peer_addr);
                        if let Err(e) = pocketdesk_core::protocol::connection::keep_alive(&stream) {
                            tracing::debug!("No keepalive on {peer_addr}: {e}");
                        }
                        let identity_packet = kde_service.identity_packet().clone();
                        let registry_clone = Arc::clone(&registry);
                        let hypr = hypr_ipc.clone();
                        let wan = wan_endpoint.clone();
                        let pairing_required = config.pairing_required;
                        let tls = control_tls.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_connection_with_kde(
                                stream,
                                tls,
                                identity_packet,
                                &registry_clone,
                                hypr.as_deref(),
                                wan.as_ref(),
                                pairing_required,
                            )
                            .await
                            {
                                tracing::warn!("connection handler failed: {}", error);
                            }
                        });
                    }
                    Err(error) => tracing::warn!("Accept error: {}", error),
                }
            }
            quic_incoming = quic_endpoint.accept() => {
                if let Some(incoming) = quic_incoming {
                    let registry_clone = Arc::clone(&registry);
                    let cert_manager_clone = Arc::clone(&cert_manager);
                    let local_v2_identity = local_v2_identity.clone();
                    let streaming_config = streaming_config.clone();
                    let input_tx = input_tx.clone();
                    let pairing_required = config.pairing_required;
                    let allow_hevc = config.allow_hevc;
                    let capture_backend = config.capture_backend;
                    let mic_relay_device = config.mic_relay_device.clone();

                    tokio::spawn(async move {
                        let conn = match incoming.await {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!("QUIC connection failed: {}", e);
                                return;
                            }
                        };

                        let alpn = {
                            let handshake_data = conn.handshake_data();
                            handshake_data.as_ref()
                                .and_then(|any| any.downcast_ref::<quinn::crypto::rustls::HandshakeData>())
                                .and_then(|h| h.protocol.clone())
                        };

                        match alpn.as_deref() {
                            Some(ALPN_V2) => {
                                if let Err(e) = handle_v2_session(conn, local_v2_identity, registry_clone, pairing_required).await {
                                    tracing::error!("v2 session error: {}", e);
                                }
                            }
                            Some(ALPN_V1_STREAM) => {
                                let mut streaming_server = StreamingServer::new(
                                    streaming_config,
                                    StreamTransportConfig::default(),
                                    cert_manager_clone
                                );
                                streaming_server.set_input_channel(input_tx);
                                // R4 E2: phone-mic frames feed a pw-loopback
                                // virtual source for the life of this session.
                                let (mic_tx, mic_rx) = tokio::sync::mpsc::channel(64);
                                tokio::spawn(crate::mic_relay::run_mic_relay(
                                    mic_rx,
                                    mic_relay_device,
                                ));
                                streaming_server.set_mic_channel(mic_tx);
                                streaming_server.set_session_telemetry(true);
                                streaming_server.set_hevc_allowed(allow_hevc);
                                streaming_server.set_capture_backend(capture_backend);
                                // The gate judges the deviceId the client announces in-band
                                // against the key its TLS certificate proved (device keys). It
                                // runs with pairing off too, so a paired device's own
                                // permissions still apply.
                                streaming_server.set_session_gate(move |claim| {
                                    Box::pin(crate::plugins::pair::session_gate(
                                        claim,
                                        pairing_required,
                                    ))
                                });
                                if let Err(e) = streaming_server
                                    .run_on_connection(QuinnConnection::shared(conn))
                                    .await
                                {
                                    tracing::error!("v1 streaming session error: {}", e);
                                }
                            }
                            _ => {
                                tracing::warn!("Unknown ALPN on QUIC port: {:?}", alpn.map(|a| String::from_utf8_lossy(&a).into_owned()));
                            }
                        }
                    });
                }
            }
            Some(reason) = shutdown.recv() => {
                tracing::info!("Shutdown requested ({reason})");
                // Tell phones on the network this computer is going away.
                if let Some(announcement) = announcement {
                    let _ = tokio::task::spawn_blocking(move || announcement.withdraw()).await;
                }
                break;
            }
            event = discovery_rx.recv() => {
                match event {
                    Ok(event) => handle_discovery_event(event),
                    Err(error) => tracing::warn!("Discovery channel error: {}", error),
                }
            }
            lan_event = lan_rx.recv() => {
                match lan_event {
                    Ok(pocketdesk_core::tailscale::lan::LanEvent::PeerDiscovered(peer)) => {
                        handle_discovery_event(DiscoveryEvent::PeerDiscovered(peer));
                    }
                    Ok(pocketdesk_core::tailscale::lan::LanEvent::PeerOffline(name)) => {
                        handle_discovery_event(DiscoveryEvent::PeerOffline(name));
                    }
                    Ok(pocketdesk_core::tailscale::lan::LanEvent::DiscoveryError { method, reason }) => {
                        handle_discovery_event(DiscoveryEvent::DiscoveryError { method, reason });
                    }
                    Ok(pocketdesk_core::tailscale::lan::LanEvent::ServiceReady) => {
                        handle_discovery_event(DiscoveryEvent::ServiceReady);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

pub async fn stop() -> Result<()> {
    let pid_file = state::pid_file_path()?;
    if !pid_file.exists() {
        println!("No running Pocketdesk server found");
        return Ok(());
    }

    let raw_pid = std::fs::read_to_string(&pid_file)
        .with_context(|| format!("failed reading {}", pid_file.display()))?;
    let pid: u32 = raw_pid
        .trim()
        .parse()
        .context("invalid PID file contents")?;

    let asked = crate::platform::request_stop(pid)
        .await
        .context("failed to ask the server to stop")?;
    if asked {
        // A graceful exit removes the PID file itself.
        for _ in 0..100 {
            if !pid_file.exists() {
                println!("Pocketdesk server (pid {pid}) stopped");
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        println!("Stop requested from pid {pid}; it has not exited yet");
        return Ok(());
    }

    #[cfg(windows)]
    {
        // No stop event: an older or wedged server. Terminate it in-process
        // (no `taskkill` to resolve off PATH), and only if the pid is still
        // a Pocketdesk server rather than whatever reused it.
        if crate::platform::force_stop(pid).context("failed to terminate the server")? {
            let _ = std::fs::remove_file(&pid_file);
            println!("Pocketdesk server (pid {pid}) force-stopped");
            return Ok(());
        }
    }
    bail!("failed to stop process {}", pid)
}

pub async fn print_status() -> Result<()> {
    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    let status = tailscale.status_text().await?;
    println!("{}", status);

    // R4 D2: what is watching this desktop right now (mirrored by the
    // daemon's live-session watcher; absent when no daemon is running).
    if let Ok(path) = crate::live_sessions::live_sessions_path()
        && let Ok(raw) = std::fs::read_to_string(&path)
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(lines) = crate::live_sessions::format_status_lines(&raw, now) {
            if lines.is_empty() {
                println!("\nStreaming sessions: none live");
            } else {
                println!("\nStreaming sessions (live):");
                for line in lines {
                    println!("  {line}");
                }
                println!("  (end one with: pocketdesk kick <device-id>)");
            }
        }
    }
    Ok(())
}

/// R4 D2: request that the running daemon drop streaming session(s)
/// matching `device` (id, id prefix, peer IP, or "all"). File-based, like
/// the pairing PIN handoff — the daemon consumes it within ~1 s.
pub async fn kick(device: String) -> Result<()> {
    let pid_file = state::pid_file_path()?;
    if !pid_file.exists() {
        bail!("no running Pocketdesk server to kick through");
    }
    let target = device.trim().to_string();
    if target.is_empty() {
        bail!("kick target is empty");
    }
    pocketdesk_core::local::sessions::request_kick(
        &state::state_dir()?,
        &target,
        local_pin::now_unix(),
    )
    .context("failed to write the kick request")?;
    println!(
        "Kick request sent for '{target}' — matching sessions end within ~1 s \
         (verify with: pocketdesk status)"
    );
    Ok(())
}

pub async fn list_peers() -> Result<()> {
    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    let peers = tailscale.get_peers().await?;

    if peers.is_empty() {
        println!("No peers found");
        return Ok(());
    }

    for peer in peers {
        let status = if peer.online { "online" } else { "offline" };
        let ip = peer
            .ips
            .first()
            .cloned()
            .unwrap_or_else(|| "n/a".to_string());
        println!("{} [{}] {}", peer.name, status, ip);
    }

    Ok(())
}

pub async fn watch_peers(interval_secs: u64) -> Result<()> {
    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    tailscale
        .wait_for_ready(Duration::from_secs(30))
        .await
        .context("tailscale is not ready")?;

    let discovery = DiscoveryService::new(tailscale);
    let mut rx = discovery.subscribe();
    let poll = Duration::from_secs(interval_secs.max(1));

    tokio::spawn(async move {
        discovery.run(poll).await;
    });

    println!(
        "Watching peers (interval={}s). Press Ctrl+C to stop.",
        poll.as_secs()
    );

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(DiscoveryEvent::PeerDiscovered(peer)) => {
                        let ip = peer.ips.first().map_or("n/a", |v| v.as_str());
                        println!("ONLINE  {}  {}", peer.name, ip);
                    }
                    Ok(DiscoveryEvent::PeerOffline(name)) => {
                        println!("OFFLINE {}", name);
                    }
                    Ok(DiscoveryEvent::DiscoveryError { method, reason }) => {
                        println!("ERROR   {} - {}", method, reason);
                    }
                    Ok(DiscoveryEvent::ServiceReady) => {
                        println!("READY");
                    }
                    Err(error) => {
                        tracing::warn!("Discovery channel error: {}", error);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("Stopped watching peers");
                break;
            }
        }
    }

    Ok(())
}

pub async fn connect_peer(peer: String, port: u16, identity: &DeviceIdentity) -> Result<()> {
    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    let address = resolve_peer_address(&tailscale, &peer).await?;

    // This desktop's own certificate is the client's (phase B: the control
    // channel runs inside TLS, and a client must present one).
    let certs = CertManager::load_or_create(&crate::state::state_dir()?.join("certs"))
        .context("Failed to load this desktop's certificate")?;
    let manager = ConnectionManager::new(Duration::from_secs(10), certs.control_client_config());
    let _stream = manager
        .connect(&address, port, identity)
        .await
        .with_context(|| format!("failed to connect to {}:{}", address, port))?;

    println!("Connected to {}:{} ({})", address, port, HANDSHAKE_OK);
    Ok(())
}

/// Send a peer the desktop state it should not have to wait for: the Workspace
/// HUD snapshot and the WAN (iroh) identity it dials when the LAN path fails.
async fn seed_desktop_state(
    sender: &Arc<dyn DeviceSender>,
    hypr_ipc: Option<&HyprlandIpc>,
    wan_endpoint: Option<&iroh::Endpoint>,
) {
    // Workspace HUD with the current compositor state (R3#8)
    if let Some(ipc) = hypr_ipc {
        hypr_events::push_state_to(sender, ipc).await;
    }
    #[cfg(windows)]
    let _ = sender
        .send_packet(&hypr_events::windows_state_packet().await)
        .await;
    // WAN identity so the phone can cache it for iroh dialing (R1 stage 3)
    if let Some(endpoint) = wan_endpoint {
        iroh_endpoint::push_endpoint_to(sender, endpoint).await;
    }
}

/// The longest handshake line (HELLO, the identity) a peer may send before
/// it has proved anything; an identity packet is a few hundred bytes.
const MAX_HANDSHAKE_LINE: u64 = 64 * 1024;

/// Read one handshake line of at most [`MAX_HANDSHAKE_LINE`] bytes.
async fn read_handshake_line<R>(reader: &mut R, line: &mut String) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let read = (&mut *reader)
        .take(MAX_HANDSHAKE_LINE)
        .read_line(line)
        .await?;
    if read as u64 == MAX_HANDSHAKE_LINE && !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "handshake line too long",
        ));
    }
    Ok(read)
}

/// Tell a client that did not start TLS why it is dropped. What it already
/// sent is read first and the socket half-closed, then drained: closing with
/// unread data sends a reset, which can discard the answer before the client
/// reads it.
async fn refuse_plain(mut stream: TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sink = [0u8; 1024];
    let _ = timeout(Duration::from_secs(1), stream.read(&mut sink)).await;
    let refusal = format!("{}\n", pocketdesk_core::protocol::HANDSHAKE_TLS_REQUIRED);
    if stream.write_all(refusal.as_bytes()).await.is_err() {
        return;
    }
    let _ = stream.shutdown().await;
    let _ = timeout(Duration::from_millis(500), async {
        while let Ok(n) = stream.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

/// Handle a TCP connection with KDE Connect protocol support.
///
/// The connection runs inside TLS (phase B): a client that does not start a
/// TLS handshake is told so and dropped. After the initial handshake, this
/// sends our identity packet and then enters a loop reading JSON packets and
/// dispatching them to plugins.
async fn handle_connection_with_kde(
    stream: TcpStream,
    tls: tokio_rustls::TlsAcceptor,
    identity_packet: Option<NetworkPacket>,
    registry: &Arc<PluginRegistry>,
    hypr_ipc: Option<&HyprlandIpc>,
    wan_endpoint: Option<&iroh::Endpoint>,
    pairing_required: bool,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    use tracing::Instrument;

    let peer_addr = stream.peer_addr().ok();
    let conn_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string(); // In v1, session == connection for now

    let conn_span = tracing::info_span!(
        "conn",
        id = %conn_id,
        session = %session_id,
        peer = ?peer_addr,
        transport = "tcp",
        proto = %pocketdesk_core::protocol::PROTOCOL_VERSION
    );

    async move {
        tracing::info!("Handling new connection");

        // A TLS client opens with a handshake record. Anything else is a
        // client from before phase B (or not ours): say why, and drop it.
        let mut first = [0u8; 1];
        let peeked = timeout(Duration::from_secs(8), stream.peek(&mut first))
            .await
            .context("connection handshake timeout")?
            .context("failed to read incoming handshake")?;
        if peeked == 0 {
            tracing::info!("Connection closed by peer before handshake");
            return Ok(());
        }
        if first[0] != pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD {
            tracing::warn!("Refused a control connection without TLS (an outdated app?)");
            refuse_plain(stream).await;
            return Ok(());
        }
        let stream = timeout(Duration::from_secs(8), tls.accept(stream))
            .await
            .context("TLS handshake timeout")?
            .context("TLS handshake failed")?;
        let peer_key = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(pocketdesk_core::streaming::PeerKey::from_tls_certs);

        let (reader_half, writer_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader_half);

        // Wrap writer in Arc<Mutex> so both handshake and TcpDeviceSender can use it
        let writer = Arc::new(Mutex::new(writer_half));

        // Step 1: POCKETDESK_HELLO handshake
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(8), read_handshake_line(&mut reader, &mut line))
            .await
            .context("connection handshake timeout")?
            .context("failed to read incoming handshake")?;

        if bytes == 0 {
            tracing::info!("Connection closed by peer before handshake");
            return Ok(());
        }

        tracing::debug!("Received handshake line: {:?}", line.trim());

        if line.trim() != HANDSHAKE_HELLO {
            tracing::warn!("Invalid handshake preface: {:?}", line.trim());
            let mut w = writer.lock().await;
            w.write_all(b"POCKETDESK_ERR 1\n")
                .await
                .context("failed writing error handshake")?;
            bail!("invalid handshake preface")
        }

        // Step 1.5: Send HANDSHAKE_OK back to client
        {
            tracing::info!("Handshake successful, sending HANDSHAKE_OK");
            let mut w = writer.lock().await;
            w.write_all(format!("{}\n", HANDSHAKE_OK).as_bytes())
                .await
                .context("failed writing handshake OK")?;
            w.flush().await?;
        }

        // Step 2: Send identity packet
        if let Some(ref identity) = identity_packet {
            let mut w = writer.lock().await;
            let wire_bytes = identity.to_wire()?;
            w.write_all(&wire_bytes).await?;
            w.flush().await?;
            tracing::debug!("Sent local identity packet");
        }

        // Step 3: the phone's identity, held to the key the TLS handshake
        // proved (phase B). The connection goes by that id from here on, so
        // nothing about it rests on its IP address.
        let mut line = String::new();
        let bytes = timeout(Duration::from_secs(5), read_handshake_line(&mut reader, &mut line))
            .await
            .context("identity timeout")?
            .context("failed to read the peer's identity")?;
        if bytes == 0 {
            tracing::info!("Connection closed by peer before its identity");
            return Ok(());
        }
        let Some(identity) = NetworkPacket::from_wire(line.trim())
            .ok()
            .filter(|p| p.packet_type == "kdeconnect.identity")
        else {
            tracing::warn!("Control connection without an identity; dropped");
            return Ok(());
        };
        let Some(device_id) = identity
            .body
            .get("deviceId")
            .and_then(|v| v.as_str())
            .filter(|id| crate::plugins::pair::valid_device_id(id))
            .map(str::to_string)
        else {
            tracing::warn!("Control connection with an unusable deviceId; dropped");
            return Ok(());
        };
        let Some(peer_addr) = peer_addr else {
            return Ok(());
        };
        // A connection that claims a paired device without its key is kept
        // for pairing only (the owner's PIN recovers a phone whose key
        // changed); the per-packet gate below holds it there.
        if let crate::plugins::pair::Standing::WrongKey(refusal) =
            crate::plugins::pair::admit_control(&device_id, peer_addr, peer_key.as_ref())
        {
            tracing::warn!(
                "Control connection for {device_id} held to pairing only: {}",
                refusal.reason()
            );
        }

        let sender: Arc<dyn DeviceSender> = Arc::new(
            TcpDeviceSender::from_arc(writer, device_id.clone())
                .with_peer(Some(peer_addr))
                .with_peer_key(peer_key.clone()),
        );
        let sender_ref = &*sender;

        tracing::Span::current().record("peer", &device_id);

        // The plugins learn the identity as they always have, before anything
        // asks whether this device is paired.
        if let Err(e) = registry.dispatch_packet(&identity, sender_ref).await {
            tracing::warn!("Identity dispatch failed: {}", e);
        }

        // Register this client for pushes (one target per device: its newest connection)
        state::register_client(sender.clone()).await;

        // Seed the Workspace HUD and the WAN identity — only once the peer
        // may receive pushes (paired, or pairing off): window titles and the
        // endpoint address are not for an unpaired device. A peer that pairs
        // on this connection is seeded the moment it does (below), rather
        // than waiting for the periodic pushes.
        let mut seeded = false;
        if state::may_receive_pushes(&*sender) {
            seed_desktop_state(&sender, hypr_ipc, wan_endpoint).await;
            seeded = true;
        }

        tracing::info!("Entering main packet loop");
        loop {
            let mut line = String::new();
            let read_result = timeout(Duration::from_secs(60), reader.read_line(&mut line)).await;

            match read_result {
                Ok(Ok(0)) => {
                    tracing::info!("Peer disconnected");
                    break;
                }
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    match NetworkPacket::from_wire(trimmed) {
                        Ok(packet) => {
                            let packet_type = packet.packet_type.clone();
                            // The connection's identity was settled in the
                            // handshake; another would re-point it at a
                            // different device.
                            if packet_type == "kdeconnect.identity" {
                                tracing::warn!("Ignored a second identity on {device_id}'s connection");
                                continue;
                            }
                            // PIN pairing enforcement (Tier-2 #11b): before a
                            // connection is paired, only the pairing handshake
                            // itself and the identity announcement run.
                            let pairing_packet = matches!(
                                packet_type.as_str(),
                                "kdeconnect.pair" | "kdeconnect.pocketdesk.pair"
                            );
                            // Re-read per packet: pairing, unpairing or a key
                            // change applies to a connection already open.
                            use crate::plugins::pair::Standing;
                            let may_act = match crate::plugins::pair::standing(
                                &device_id,
                                peer_key.as_ref(),
                            ) {
                                Standing::Paired => true,
                                Standing::Unpaired => !pairing_required,
                                Standing::WrongKey(_) => false,
                            };
                            if !pairing_packet && !may_act {
                                tracing::debug!(
                                    "Blocking {packet_type} from {device_id}: not paired, \
                                     or not with this key"
                                );
                                continue;
                            }
                            // F5: the owner's per-device permissions (spec P3).
                            if !pairing_packet
                                && !crate::plugins::pair::permits(&device_id, &packet_type)
                            {
                                tracing::info!(
                                    "Denied {packet_type} from {device_id}: not permitted for this device"
                                );
                                continue;
                            }
                            let packet_span = tracing::debug_span!("packet", type = %packet_type);
                            async {
                                tracing::debug!("Processing packet");
                                if let Err(e) = registry.dispatch_packet(&packet, sender_ref).await
                                {
                                    tracing::warn!("Packet dispatch failed: {}", e);
                                }
                            }
                            .instrument(packet_span)
                            .await;
                            if pairing_packet && !seeded && state::may_receive_pushes(&*sender) {
                                tracing::info!(
                                    "Peer paired on this connection; seeding desktop state"
                                );
                                seed_desktop_state(&sender, hypr_ipc, wan_endpoint).await;
                                seeded = true;
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Malformed packet received: {}", e);
                        }
                    }
                }
                // A phone that drops the socket without TLS's close_notify
                // (the app stopped, the network went) has simply left.
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    tracing::info!("Peer disconnected");
                    break;
                }
                Ok(Err(e)) => {
                    tracing::error!("Read error in control channel: {}", e);
                    break;
                }
                Err(_) => {
                    tracing::debug!("Peer idle - connection still alive");
                }
            }
        }

        // Cleanup
        state::unregister_client(sender.connection_id()).await;

        Ok(())
    }
    .instrument(conn_span)
    .await
}

pub async fn print_capabilities(json: bool, markdown: bool) -> Result<()> {
    if json && markdown {
        bail!("--json and --markdown are mutually exclusive");
    }
    let capabilities = pocketdesk_core::capabilities::Capabilities::collect();
    if json {
        println!("{}", capabilities.render_json());
        return Ok(());
    }
    if markdown {
        print!("{}", capabilities.render_markdown());
        return Ok(());
    }
    print!("{}", capabilities.render_text());

    let kde_service = kde::build_default_service().context("failed to initialize KDE service")?;
    let plugin_names = kde_service.registry.plugin_names();
    let (incoming, outgoing) = kde_service.registry.capability_sets();
    let trusted = kde_service
        .trust_store
        .as_ref()
        .map(|s| s.trusted_devices())
        .unwrap_or_default();

    println!(
        "Plugins ({}): {}",
        plugin_names.len(),
        plugin_names.join(", ")
    );
    println!("Incoming capabilities ({}):", incoming.len());
    for cap in incoming {
        println!("  - {}", cap);
    }

    println!("Outgoing capabilities ({}):", outgoing.len());
    for cap in outgoing {
        println!("  - {}", cap);
    }

    println!("Trusted devices ({}):", trusted.len());
    for device in trusted {
        println!("  - {}", device);
    }

    Ok(())
}

pub async fn pair(pin: Option<String>, grant: Option<u64>) -> Result<()> {
    let pin_value = match pin {
        Some(value) => {
            if !local_pin::is_valid(&value) {
                bail!("PIN must be exactly 6 numeric digits");
            }
            value
        }
        None => local_pin::mint_value(),
    };

    // R4 D3: an optional grant (secs) caps how long pairing with this PIN
    // trusts the phone — the PIN file itself still lives 5 minutes.
    let dir = state::state_dir()?;
    local_pin::write(&dir, &pin_value, local_pin::now_unix(), grant)
        .with_context(|| format!("failed to write the PIN file in {}", dir.display()))?;
    let path = state::pair_pin_path()?;

    println!("Pairing PIN: {}", pin_value);
    match grant {
        Some(secs) => println!(
            "Stored at {} (valid 5 minutes, for one phone; pairing grants {} of trust)",
            path.display(),
            humantime(secs)
        ),
        None => println!(
            "Stored at {} (valid 5 minutes, for one phone)",
            path.display()
        ),
    }
    Ok(())
}

/// Compact rendering of a grant duration for CLI output.
fn humantime(secs: u64) -> String {
    if secs.is_multiple_of(86400) {
        format!("{}d", secs / 86400)
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{}s", secs)
    }
}

/// Remove devices from the persistent trust store (`pocketdesk unpair`).
/// The devices `unpair` removes: the one named, which must be paired, or
/// every one when none is named. An empty name is refused, not read as
/// "all".
fn unpair_targets(
    device_id: Option<&str>,
    trusted: &[String],
) -> std::result::Result<Vec<String>, String> {
    match device_id {
        None => Ok(trusted.to_vec()),
        Some(id) if id.trim().is_empty() => Err(
            "no device id given; run `pocketdesk unpair` with no argument to remove every paired device"
                .to_string(),
        ),
        Some(id) if trusted.iter().any(|t| t == id) => Ok(vec![id.to_string()]),
        Some(id) => Err(format!("{id} is not a paired device")),
    }
}

pub async fn unpair(device_id: Option<String>) -> Result<()> {
    // Checked first, so a mistyped id is said plainly; the update below
    // re-reads the store under its lock.
    let trusted = TrustStore::load_or_create(state::trust_store_path()?)?.trusted_devices();
    unpair_targets(device_id.as_deref(), &trusted).map_err(anyhow::Error::msg)?;
    TrustStore::update(state::trust_store_path()?, |store| {
        let targets = unpair_targets(device_id.as_deref(), &store.trusted_devices())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        if targets.is_empty() {
            println!("No trusted devices.");
        }
        for id in &targets {
            store.untrust_device(id)?;
            // Its keys go too, so a phone that pairs again can bind new ones.
            if let Ok(dir) = state::state_dir() {
                let _ = pocketdesk_core::local::device_keys::clear(&dir, id);
            }
            println!("Untrusted {id}");
        }
        Ok(())
    })?;
    Ok(())
}

fn handle_discovery_event(event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::PeerDiscovered(peer) => {
            tracing::info!(peer = %peer.name, ips = ?peer.ips, "Peer came online");
        }
        DiscoveryEvent::PeerOffline(name) => {
            tracing::info!(peer = %name, "Peer went offline");
        }
        DiscoveryEvent::ServiceReady => {
            tracing::info!("Discovery service ready");
        }
        DiscoveryEvent::DiscoveryError { method, reason } => {
            tracing::error!(method, error = %reason, "Discovery service failure");
        }
    }
}

async fn resolve_peer_address(client: &TailscaleClient, peer_hint: &str) -> Result<String> {
    if peer_hint.parse::<std::net::IpAddr>().is_ok() {
        return Ok(peer_hint.to_string());
    }

    let peers = client.get_peers().await?;

    for peer in peers {
        if (peer.name == peer_hint
            || peer.dns_name == peer_hint
            || peer
                .dns_name
                .trim_end_matches('.')
                .eq_ignore_ascii_case(peer_hint)
            || peer.ips.iter().any(|ip| ip == peer_hint))
            && let Some(ip) = peer.ips.first()
        {
            return Ok(ip.clone());
        }
    }

    bail!("peer not found on tailnet: {}", peer_hint)
}

/// Handle an `InputPacket` received over the QUIC streaming channel by injecting
/// it into the host system via the `InputInjector`.
///
/// Delegates all packet variants to `InputInjector::handle_input_packet()`
/// which handles mouse, keyboard, gamepad, and scroll events uniformly
/// across both enigo and uinput backends.
fn handle_input_packet(injector: &mut InputInjector, packet: InputPacket) -> Result<()> {
    injector.handle_input_packet(&packet)?;
    Ok(())
}

struct PidFileGuard {
    path: PathBuf,
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod control_tls_tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// A control port on loopback, served by the real connection handler.
    async fn control_port() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let desktop = CertManager::new().unwrap();
        let tls = tokio_rustls::TlsAcceptor::from(desktop.control_server_config().unwrap());
        tokio::spawn(async move {
            let registry =
                Arc::new(pocketdesk_core::protocol::kdeconnect::PluginRegistry::default());
            while let Ok((stream, _)) = listener.accept().await {
                let tls = tls.clone();
                let registry = registry.clone();
                tokio::spawn(async move {
                    let _ =
                        handle_connection_with_kde(stream, tls, None, &registry, None, None, true)
                            .await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn a_client_without_tls_is_told_and_dropped() {
        let addr = control_port().await;
        let mut plain = TcpStream::connect(addr).await.unwrap();
        plain
            .write_all(
                format!(
                    "{HANDSHAKE_HELLO}
"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut reply = String::new();
        let mut reader = BufReader::new(plain);
        timeout(Duration::from_secs(5), reader.read_line(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.trim(),
            pocketdesk_core::protocol::HANDSHAKE_TLS_REQUIRED
        );
        let mut rest = String::new();
        let closed = timeout(Duration::from_secs(5), reader.read_line(&mut rest))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(closed, 0, "the connection is closed after the refusal");
    }

    #[tokio::test]
    async fn a_client_inside_tls_completes_the_handshake() {
        let addr = control_port().await;
        let phone = CertManager::new().unwrap();
        let identity = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
            "control-tls-test-phone",
            "Test Phone",
        );
        ConnectionManager::new(Duration::from_secs(5), phone.control_client_config())
            .connect(&addr.ip().to_string(), addr.port(), &identity)
            .await
            .expect("the control channel over TLS");
    }
}

#[cfg(test)]
mod handshake_line_tests {
    use super::*;

    #[tokio::test]
    async fn a_handshake_line_has_a_limit() {
        let endless = vec![b'x'; 200 * 1024];
        let mut reader = BufReader::new(&endless[..]);
        let mut line = String::new();
        assert!(
            read_handshake_line(&mut reader, &mut line).await.is_err(),
            "a peer that has proved nothing cannot make the desktop buffer this"
        );
        let mut reader = BufReader::new(&b"POCKETDESK_HELLO 1\n"[..]);
        let mut line = String::new();
        assert_eq!(
            read_handshake_line(&mut reader, &mut line).await.unwrap(),
            19
        );
        assert_eq!(line.trim(), HANDSHAKE_HELLO);
    }
}

#[cfg(test)]
mod unpair_tests {
    use super::unpair_targets;

    #[test]
    fn unpair_names_a_paired_device_or_every_one() {
        let trusted = vec!["phone-a".to_string(), "phone-b".to_string()];
        assert_eq!(
            unpair_targets(Some("phone-a"), &trusted),
            Ok(vec!["phone-a".to_string()])
        );
        assert_eq!(
            unpair_targets(None, &trusted),
            Ok(trusted.clone()),
            "no id: all of them"
        );
        assert!(
            unpair_targets(Some(""), &trusted).is_err(),
            "an empty id is not 'all'"
        );
        assert!(unpair_targets(Some("  "), &trusted).is_err());
        assert!(
            unpair_targets(Some("phone-z"), &trusted).is_err(),
            "not a paired device"
        );
    }
}
