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
    let state_dir = state::state_dir()?;
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("failed to create {}", state_dir.display()))?;
    match pocketdesk_core::local::paths::protect_state_dir(&state_dir) {
        Ok(true) => tracing::info!("Made {} owner-only (0700)", state_dir.display()),
        Ok(false) => {}
        // A folder the user chose and does not own, or a filesystem without
        // modes: the server still runs, and says the files are not private.
        Err(e) => tracing::warn!(
            "Could not make {} owner-only, so other local users may read it: {e}",
            state_dir.display()
        ),
    }
    // T1 review: a second server once stopped at the control port's bind;
    // now that the bind moves past a held port, it is stopped here, before
    // it takes the pid file, `listening.json` or the announcement.
    if let Some(pid) = pocketdesk_core::local::server::server_pid(&state_dir)
        .filter(|&pid| pid != std::process::id())
        .filter(|_| pocketdesk_core::local::server::is_running(&state_dir))
    {
        anyhow::bail!("a Pocketdesk server already runs for this state folder (pid {pid})");
    }
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

    // For the LAN announcement only, which leaves the tailnet's addresses out.
    let local_ips = pocketdesk_core::tailscale::lan::get_local_ips().await;
    if tailscale_ready && let Ok(ip) = tailscale.get_self_ip().await {
        tracing::info!("Tailscale online at {}", ip);
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

    // The inbox folders the app's requests land in, watched (R6c).
    crate::inbox::start(&state_dir);
    // Q4/R2: Linux's Do Not Disturb, read before a notification needs it.
    crate::quiet_hours::keep_warm();
    // Test seams (2g, 2h, D9): said, so one left set by mistake is noticed.
    for seam in [
        "POCKETDESK_BROWSE_HOME",
        "POCKETDESK_CLIPBOARD_FILE",
        "POCKETDESK_CLIPBOARD_IMAGE_FILE",
        "POCKETDESK_DOWNLOAD_DIR",
        "POCKETDESK_LOCK_FILE",
        "POCKETDESK_MEDIA_FILE",
    ] {
        if let Some(value) = std::env::var_os(seam) {
            let path = std::path::Path::new(&value);
            if path.is_absolute() {
                tracing::warn!("{seam} is set: {} stands in (a test seam)", path.display());
            } else if seam == "POCKETDESK_BROWSE_HOME" {
                // Fails closed (AD2 review): never the owner's home instead.
                tracing::warn!("{seam} is not an absolute path: the file browser shows no folder");
            } else {
                tracing::warn!("{seam} is not an absolute path; ignored");
            }
        }
    }
    if let Some(list) = std::env::var_os("POCKETDESK_FAIL_ENCODERS") {
        tracing::warn!(
            "POCKETDESK_FAIL_ENCODERS is set: {} fail as they open (a test seam)",
            list.to_string_lossy()
        );
    }
    // R4 D2: mirror live streaming sessions to the state dir, notify the
    // desktop when one starts, and consume `pocketdesk kick` requests.
    tokio::spawn(crate::live_sessions::run_watcher());
    // F7b: files the desktop app sends to a phone in a session.
    tokio::spawn(crate::offers::run_watcher());
    tokio::spawn(crate::plugins::links::run_watcher());
    // N1: rings the desktop app or `pocketdesk ring` queues for a phone.
    tokio::spawn(crate::ring_phone::run_watcher());
    // D2: mutes for phones, and what the phones said before this run.
    crate::quiet_phone::load();
    tokio::spawn(crate::quiet_phone::run_watcher());
    tokio::spawn(crate::transfers::run());
    tokio::spawn(crate::plugins::phone_battery::run_writer());
    // Home Assistant over MQTT (ecosystem 1e), while config.toml turns it on.
    tokio::spawn(crate::home_assistant::run());
    tokio::spawn(crate::plugins::phone_media::run_watcher());
    // N5: the computer's music for phones that watch it.
    tokio::spawn(crate::plugins::computer_media::run_watcher());
    // The desktop app's requests to browse the phones' storage (T10g).
    tokio::spawn(crate::plugins::phone_files::run_watcher());
    tokio::spawn(crate::drive::live::run());
    tokio::spawn(crate::away_lock::run());
    tokio::spawn(crate::plugins::phone_photos::run_watcher());
    // A phone's own screen (T10j): those a stopped server left go, and
    // each goes once its phone has no session.
    tokio::task::spawn_blocking(crate::virtual_screen::clear_left_over);
    // Task starts held by an earlier run wait for no one now (ecosystem 1c).
    tokio::task::spawn_blocking(crate::plugins::tasks::clear_left_over);
    tokio::spawn(crate::virtual_screen::run_sweeper());
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

    let (listener, control_port) = bind_control("0.0.0.0", config.control_port)
        .await
        .with_context(|| format!("failed to bind 0.0.0.0:{}", config.control_port))?;
    let bind_addr = format!("0.0.0.0:{control_port}");
    tracing::info!("Control listener ready on {}", bind_addr);
    // T1: what phones reach, for the desktop app's pairing code and pages.
    // KDE Connect's own program (or GSConnect) on the configured port is
    // said where the owner sees it, and the bridge stays off: the KDE
    // devices talk to that program.
    let moved = control_port != config.control_port;
    if moved {
        tracing::warn!(
            "TCP {} is held by another program (KDE Connect or GSConnect?); phones reach this computer on {control_port}, and KDE Connect devices are left to that program",
            config.control_port
        );
    }
    if let Ok(dir) = crate::state::state_dir() {
        let listening = pocketdesk_core::local::listening::Listening {
            pid: std::process::id(),
            control_port,
            configured_port: config.control_port,
        };
        if let Err(e) = pocketdesk_core::local::listening::write(&dir, &listening) {
            tracing::warn!("Could not record the control port: {e}");
        }
    }

    // Announced only now that the control port answers: a phone checks it
    // before listing this computer.
    crate::plugins::notification::configure(
        config.phone_notifications,
        config.notifications_per_minute,
    );
    crate::plugins::sms::configure(config.sms_notify, config.sms_keep, config.sms_names);
    // Names turned off: forget the ones already kept, not only new ones.
    if !config.sms_names
        && let Ok(dir) = crate::state::state_dir()
        && let Err(e) = pocketdesk_core::local::sms::forget_names(&dir, config.sms_keep as usize)
    {
        tracing::warn!("Could not forget the contact names kept with texts: {e}");
    }
    crate::plugins::links::configure(config.open_links);
    // P10: contacts of phones gone or with Texts off are not kept.
    crate::plugins::phone_contacts::prune();
    crate::virtual_screen::configure(config.virtual_screens);
    // The deck's injector now, not at its first press (F1 review).
    crate::plugins::deck::warm_up(config.deck_enabled && !config.deck.is_empty());
    // The desktop app's answers to the phones' notifications (T10c).
    tokio::spawn(crate::plugins::notification::run_answers());
    // L1: notifications closed here, cleared on their phones.
    tokio::spawn(crate::plugins::notification::run_clears());
    // L6: commands for phones' automation apps.
    tokio::spawn(crate::plugins::links::run_commands());
    // L4: numbers for phones to dial.
    tokio::spawn(crate::plugins::links::run_dials());
    // O2: screenshots and locks asked of phones.
    tokio::spawn(crate::plugins::links::run_phone_actions());
    // M4: what the typing window types on a phone.
    tokio::spawn(crate::plugins::typing::run());
    // The clipboard to connected phones as it changes (T10b), and shared
    // with the owner's other computers (2g).
    if crate::clipboard_push::configure(config.clipboard_push, config.computer_clipboard) {
        tokio::spawn(crate::clipboard_push::run());
    }
    // D3: a text from the clipboard's history put on a phone's clipboard.
    tokio::spawn(crate::clip_history::run_sends());
    crate::plugins::phone_battery::configure(config.phone_battery_alerts);
    // N8 review: a kept screen-sharing grant goes as the server starts when
    // the setting is off, not only when a session next arrives.
    let _ = state::screen_grant();
    // KDE Connect devices (3a): told where this computer is while the app
    // looks for them.
    let kdeconnect = config.kdeconnect_compat && !moved;
    crate::kdeconnect_compat::configure(kdeconnect);
    if kdeconnect {
        tokio::spawn(crate::kdeconnect_compat::discovery::run(
            control_port,
            config.lan_discovery,
        ));
        tokio::spawn(crate::kdeconnect_compat::run_requests());
    }
    crate::wake_info::configure(config.wake_announce, config.wake_port);
    crate::driving::configure(config.input_control, config.input_idle_secs);
    crate::plugins::share::configure(config.transfer_keep_hours);
    crate::plugins::share::spawn_partials_sweep();
    let announcement = if config.lan_discovery {
        Some(lan_discovery.register_service(
            &host_name,
            &host_id,
            control_port,
            config.streaming_port,
            local_ips,
        )?)
    } else {
        tracing::info!("Not announced on the local network (lan_discovery = false)");
        None
    };
    // P1: announced again when this computer's addresses change (a new
    // network, a wake, a VPN), so phones find it at the new one.
    let announcement = std::sync::Arc::new(std::sync::Mutex::new(announcement));
    tokio::spawn(follow_addresses(announcement.clone()));

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
    let (input_tx, mut input_rx) =
        tokio::sync::broadcast::channel::<pocketdesk_core::streaming::SessionInput>(16);
    let streaming_config = config.video_quality.to_streaming_config();
    // T10k: phones' casts are handed to the desktop app through one relay.
    let cast_tx = match crate::state::state_dir() {
        Ok(dir) => Some(crate::cast_relay::spawn(dir)),
        Err(e) => {
            tracing::warn!("No cast relay, phones' screens cannot be shown: {e:#}");
            None
        }
    };

    // R1 stage 3: WAN (iroh) endpoint alongside the quinn LAN listener.
    // A bind failure (no network, relays unreachable) must not take the LAN
    // experience down, so the endpoint is optional at runtime.
    // Control connections over the internet road (T10a) come from the WAN
    // listener to the loop below, which owns the plugins.
    let (internet_control_tx, mut internet_control) = tokio::sync::mpsc::channel(16);
    let wan_endpoint = match iroh_endpoint::spawn_wan_endpoint(
        streaming_config.clone(),
        input_tx.clone(),
        Arc::clone(&cert_manager),
        iroh_endpoint::WanSettings {
            pairing_required: config.pairing_required,
            allow_hevc: config.allow_hevc,
            slow_link_frame_rate: config.slow_link_frame_rate,
            laser: config.laser_pointer && crate::laser::available(),
            capture_backend: config.capture_backend,
            mic_relay_device: config.mic_relay_device.clone(),
            cast: cast_tx.clone(),
            relay_urls: config.relay_urls.clone(),
            dht_publish: config.dht_publish,
            control: Some(internet_control_tx),
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
    // The owner's other computers (2f): pairing with one, and a link to
    // each over this endpoint.
    tokio::spawn(crate::peer_computers::run(
        Arc::clone(&cert_manager),
        wan_endpoint.clone(),
    ));
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
        // Made at once (a device made late misses the first event), and if
        // that fails (no access to /dev/uinput yet, no display) tried again
        // while input arrives, instead of no input until a restart.
        let mut last_try = Some(std::time::Instant::now());
        let mut injector = match InputInjector::new() {
            Ok(inj) => Some(inj),
            Err(e) => {
                tracing::error!(
                    "Failed to create InputInjector: {e}; trying again when input arrives"
                );
                None
            }
        };

        // What each phone holds down (gamepad buttons, mouse buttons, keys),
        // so one phone's snapshot never releases another's, and what a phone
        // held is let go when control moves away or its session ends.
        let mut gamepads: std::collections::HashMap<u64, u16> = std::collections::HashMap::new();
        let mut held: std::collections::HashMap<u64, crate::driving::Held> =
            std::collections::HashMap::new();
        let mut last_session: Option<u64> = None;
        // Ended sessions are swept each second too, not only when input
        // arrives: a phone that stopped mid-drag must not leave a button down
        // until some phone next sends input (R2 review).
        let mut sweep = tokio::time::interval(Duration::from_secs(1));
        loop {
            let received = tokio::select! {
                received = input_rx.recv() => received,
                _ = sweep.tick() => {
                    if let Some(injector) = injector.as_mut() {
                        release_ended(injector, &mut gamepads, &mut held);
                    }
                    continue;
                }
            };
            match received {
                Ok((session, packet)) => {
                    if injector.is_none() && retry_injector_now(last_try, std::time::Instant::now())
                    {
                        last_try = Some(std::time::Instant::now());
                        match InputInjector::new() {
                            Ok(inj) => {
                                tracing::info!("Input injector ready");
                                injector = Some(inj);
                            }
                            Err(e) => tracing::warn!("Input injector still unavailable: {e}"),
                        }
                    }
                    let Some(injector) = injector.as_mut() else {
                        continue;
                    };
                    release_ended(injector, &mut gamepads, &mut held);
                    let live = crate::driving::live_refs();
                    if crate::driving::admit(session, &live) == crate::driving::Drive::Drop {
                        tracing::trace!(session, "Input dropped: another phone drives");
                        continue;
                    }
                    // Control moved to this session: the last driver's held
                    // buttons and keys are let go (its releases would be
                    // dropped). With every phone driving, nothing moves.
                    if let Some(previous) = last_session.filter(|p| *p != session)
                        && !crate::driving::is_shared()
                    {
                        release_session(injector, &mut gamepads, &mut held, previous);
                    }
                    last_session = Some(session);
                    held.entry(session).or_default().note(&packet);
                    let state = gamepads.entry(session).or_insert(0);
                    let saved = injector.swap_gamepad_buttons(*state);
                    if let Err(e) = handle_input_packet(injector, packet) {
                        tracing::warn!("Input injection error: {e}");
                    }
                    *state = injector.swap_gamepad_buttons(saved);
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

    // At most a few connections per address and so many in all (R3): every
    // accepted connection costs a task and buffers before it proves anything.
    let budget = crate::conn_budget::ConnBudget::new(
        crate::conn_budget::PER_ADDRESS,
        crate::conn_budget::TOTAL,
    );
    let mut refused_said = std::time::Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(std::time::Instant::now);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer_addr)) => {
                        let Some(slot) = budget.admit(peer_addr.ip()) else {
                            if refused_said.elapsed() >= Duration::from_secs(10) {
                                refused_said = std::time::Instant::now();
                                tracing::warn!(
                                    "Refused a control connection from {peer_addr}: too many open"
                                );
                            }
                            continue;
                        };
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
                            let _slot = slot;
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
            Some((conn, slot)) = internet_control.recv() => {
                let identity_packet = kde_service.identity_packet().clone();
                let registry_clone = Arc::clone(&registry);
                let hypr = hypr_ipc.clone();
                let wan = wan_endpoint.clone();
                let pairing_required = config.pairing_required;
                tokio::spawn(async move {
                    let _slot: tokio::sync::OwnedSemaphorePermit = slot;
                    if let Err(error) = handle_internet_control(
                        conn,
                        identity_packet,
                        &registry_clone,
                        hypr.as_deref(),
                        wan.as_ref(),
                        pairing_required,
                    )
                    .await
                    {
                        tracing::warn!("internet control connection failed: {error:#}");
                    }
                });
            }
            quic_incoming = quic_endpoint.accept() => {
                if let Some(incoming) = quic_incoming {
                    let Some(incoming) = crate::conn_budget::proven(incoming) else {
                        continue;
                    };
                    let peer = incoming.remote_address();
                    let Some(slot) = budget.admit(peer.ip()) else {
                        if refused_said.elapsed() >= Duration::from_secs(10) {
                            refused_said = std::time::Instant::now();
                            tracing::warn!("Refused a QUIC connection from {peer}: too many open");
                        }
                        incoming.refuse();
                        continue;
                    };
                    let registry_clone = Arc::clone(&registry);
                    let cert_manager_clone = Arc::clone(&cert_manager);
                    let local_v2_identity = local_v2_identity.clone();
                    let streaming_config = streaming_config.clone();
                    let input_tx = input_tx.clone();
                    let pairing_required = config.pairing_required;
                    let allow_hevc = config.allow_hevc;
                    let slow_link_frame_rate = config.slow_link_frame_rate;
                    let laser = config.laser_pointer && crate::laser::available();
                    let capture_backend = config.capture_backend;
                    let mic_relay_device = config.mic_relay_device.clone();
                    let cast_tx = cast_tx.clone();

                    tokio::spawn(async move {
                        let _slot = slot;
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
                                // K5: a cast's sound plays here.
                                let (sound_tx, sound_rx) = tokio::sync::mpsc::channel(64);
                                tokio::spawn(crate::phone_sound::run(sound_rx));
                                streaming_server.set_phone_sound_channel(sound_tx);
                                if let Some(cast_tx) = cast_tx {
                                    streaming_server.set_cast_channel(cast_tx);
                                }
                                streaming_server.set_session_telemetry(true);
                                streaming_server.set_hevc_allowed(allow_hevc);
                                streaming_server.set_slow_link_frame_rate(slow_link_frame_rate);
                                streaming_server.set_laser(laser);
                                streaming_server.set_capture_backend(capture_backend);
                                streaming_server.set_screen_grant(state::screen_grant());
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
                // Taken off the loop: an announcement under way holds the
                // lock (P1 review).
                let held = announcement.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let current = held.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some(announcement) = current {
                        announcement.withdraw();
                    }
                })
                .await;
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

/// The first line of `pocketdesk status`: Tailscale's own when it runs,
/// else a note. Tailscale is optional, so its absence must not hide the
/// sessions below it.
fn tailscale_status_line(status: Result<String>) -> String {
    status.unwrap_or_else(|_| "Tailscale: not running (it is optional)".to_string())
}

pub async fn print_status() -> Result<()> {
    // Bounded: a `tailscale` that hangs must not hang `status` with it.
    let status = match TailscaleClient::new() {
        Ok(tailscale) => {
            match tokio::time::timeout(Duration::from_secs(3), tailscale.status_text()).await {
                Ok(status) => status,
                Err(_) => Err(anyhow::anyhow!("tailscale did not answer in 3 s")),
            }
        }
        Err(e) => Err(e),
    };
    println!("{}", tailscale_status_line(status));

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

pub async fn list_peers(json: bool) -> Result<()> {
    let tailscale = TailscaleClient::new().context("failed to initialize Tailscale client")?;
    let peers = tailscale.get_peers().await?;

    if json {
        let peers: Vec<_> = peers
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "dns_name": p.dns_name,
                    "online": p.online,
                    "ips": p.ips,
                })
            })
            .collect();
        crate::local_api::print(&crate::local_api::versioned(
            serde_json::json!({ "peers": peers }),
        ));
        return Ok(());
    }

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

/// Keep what a paired phone calls itself (U3), off the runtime: the trust
/// store is a file, written only when the name changed.
fn note_claimed_name(device_id: &str, claimed: Option<&str>) {
    let Some(name) = claimed.map(str::to_string) else {
        return;
    };
    let id = device_id.to_string();
    tokio::task::spawn_blocking(move || crate::plugins::pair::note_claimed_name(&id, &name));
}

/// Bind the control port, or the next free one when another program holds
/// it (T1: KDE Connect and GSConnect take 1716). Only a port in use (or, on
/// Windows, one it keeps for Hyper-V or WinNAT, which refuses with access
/// denied) moves it on; any other error is the server's to report.
async fn bind_control(
    host: &str,
    configured: u16,
) -> std::io::Result<(tokio::net::TcpListener, u16)> {
    let mut last = None;
    for port in pocketdesk_core::local::listening::ports_to_try(configured) {
        match tokio::net::TcpListener::bind((host, port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => last = Some(e),
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied => {
                last = Some(e)
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no port to try")))
}

/// Send a peer the desktop state it should not have to wait for: the Workspace
/// HUD snapshot and the WAN (iroh) identity it dials when the LAN path fails.
async fn seed_desktop_state(
    sender: &Arc<dyn DeviceSender>,
    hypr_ipc: Option<&HyprlandIpc>,
    wan_endpoint: Option<&iroh::Endpoint>,
    road: Road,
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
    // Files offered while its app was gone, offered again (S2).
    crate::offers::offer_again(&**sender).await;
    // How to wake this computer, for the adapter the phone reached (N6). A
    // connection over the internet reached none: the phone keeps the one it
    // learned at home.
    let Road::Local { local, .. } = road else {
        return;
    };
    let wake = tokio::task::spawn_blocking(move || crate::wake_info::packet_for_connection(local))
        .await
        .ok();
    if let Some(packet) = wake {
        let _ = sender.send_packet(&packet).await;
    }
}

/// The longest handshake line (HELLO, the identity) a peer may send before
/// it has proved anything; an identity packet is a few hundred bytes.
const MAX_HANDSHAKE_LINE: u64 = 64 * 1024;

/// The longest control packet a connection may send, as one line. The
/// largest real ones (a clipboard text, a batch of texts) are far smaller;
/// without a cap, one endless line from any TLS client, paired or not,
/// would grow the server's memory until it ran out.
const MAX_PACKET_LINE: u64 = 4 * 1024 * 1024;

/// Read one handshake line of at most [`MAX_HANDSHAKE_LINE`] bytes.
async fn read_handshake_line<R>(reader: &mut R, line: &mut String) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    read_line_capped(reader, line, MAX_HANDSHAKE_LINE).await
}

/// Read one line of at most `limit` bytes; a longer one is an error, and
/// the connection is dropped.
async fn read_line_capped<R>(
    reader: &mut R,
    line: &mut String,
    limit: u64,
) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let read = (&mut *reader).take(limit).read_line(line).await?;
    if read as u64 == limit && !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("line longer than {limit} bytes"),
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
    use tracing::Instrument;

    let peer_addr = stream.peer_addr().ok();
    let local_ip = stream.local_addr().ok().map(|a| a.ip());
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
        // A KDE Connect device's connection packet (3a).
        if first[0] == b'{' && crate::kdeconnect_compat::enabled() {
            crate::kdeconnect_compat::serve(stream).await;
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
        serve_control(
            stream,
            Road::Local {
                peer: peer_addr,
                local: local_ip,
            },
            None,
            peer_key,
            identity_packet,
            registry,
            hypr_ipc,
            wan_endpoint,
            pairing_required,
        )
        .await
    }
    .instrument(conn_span)
    .await
}

/// Which road a control connection came by.
#[derive(Debug, Clone, Copy)]
enum Road {
    /// TLS over TCP, from `peer`, reaching this computer's `local` address.
    Local {
        peer: Option<std::net::SocketAddr>,
        local: Option<std::net::IpAddr>,
    },
    /// iroh, from anywhere (T10a): only a paired phone with its registered
    /// iroh key, no pairing, and no address to dial back.
    Internet,
}

/// Streams for transfers on an internet control connection (2c): each a
/// bidirectional stream the computer opens on the phone's own connection,
/// sent behind the control stream so packets keep flowing during a file.
struct IrohLink(iroh::endpoint::Connection);

#[async_trait::async_trait]
impl pocketdesk_core::protocol::kdeconnect::OpenLinkStream for IrohLink {
    async fn open(
        &self,
    ) -> std::io::Result<Box<dyn pocketdesk_core::protocol::kdeconnect::TransferIo>> {
        let (send, recv) = self.0.open_bi().await.map_err(std::io::Error::other)?;
        // The control stream keeps the default priority, 0.
        let _ = send.set_priority(-1);
        Ok(Box::new(tokio::io::join(recv, send)))
    }
}

/// A control connection over the internet road (T10a): the phone opens one
/// bidirectional stream on an iroh connection with [`ALPN_CONTROL`] and
/// runs the same handshake and packets on it as on the TCP channel.
///
/// [`ALPN_CONTROL`]: pocketdesk_core::protocol::ALPN_CONTROL
pub(crate) async fn handle_internet_control(
    conn: iroh::endpoint::Connection,
    identity_packet: Option<NetworkPacket>,
    registry: &Arc<PluginRegistry>,
    hypr_ipc: Option<&HyprlandIpc>,
    wan_endpoint: Option<&iroh::Endpoint>,
    pairing_required: bool,
) -> Result<()> {
    use tracing::Instrument;
    let conn_span = tracing::info_span!(
        "conn",
        id = %uuid::Uuid::new_v4(),
        peer = tracing::field::Empty,
        transport = "iroh",
        proto = %pocketdesk_core::protocol::PROTOCOL_VERSION
    );
    async move {
        tracing::info!("Handling a control connection over the internet");
        let peer_key = Some(pocketdesk_core::streaming::PeerKey::Iroh(
            conn.remote_id().to_string(),
        ));
        let (send, recv) = timeout(Duration::from_secs(8), conn.accept_bi())
            .await
            .context("control stream timeout")?
            .context("no control stream")?;
        let link: Arc<dyn pocketdesk_core::protocol::kdeconnect::OpenLinkStream> =
            Arc::new(IrohLink(conn.clone()));
        let served = serve_control(
            tokio::io::join(recv, send),
            Road::Internet,
            Some(link),
            peer_key,
            identity_packet,
            registry,
            hypr_ipc,
            wan_endpoint,
            pairing_required,
        )
        .await;
        // The phone hangs up first when it can: closing at once could
        // overtake the last line written (a refusal), which QUIC would then
        // never deliver.
        let _ = timeout(Duration::from_secs(2), conn.closed()).await;
        conn.close(0u32.into(), b"control closed");
        served
    }
    .instrument(conn_span)
    .await
}

/// The control channel past its transport: the handshake, the phone's
/// identity held to the key its road proved, and the packet loop.
#[allow(clippy::too_many_arguments)]
async fn serve_control<S>(
    stream: S,
    road: Road,
    link: Option<Arc<dyn pocketdesk_core::protocol::kdeconnect::OpenLinkStream>>,
    peer_key: Option<pocketdesk_core::streaming::PeerKey>,
    identity_packet: Option<NetworkPacket>,
    registry: &Arc<PluginRegistry>,
    hypr_ipc: Option<&HyprlandIpc>,
    wan_endpoint: Option<&iroh::Endpoint>,
    pairing_required: bool,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    use tokio::io::AsyncWriteExt;
    use tracing::Instrument;
    {
        let (reader_half, writer_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader_half);

        // Wrap writer in Arc<Mutex> so both handshake and TcpDeviceSender can use it
        let writer = Arc::new(Mutex::new(writer_half));

        // Step 1: POCKETDESK_HELLO handshake
        let mut line = String::new();
        let bytes = timeout(
            Duration::from_secs(8),
            read_handshake_line(&mut reader, &mut line),
        )
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

        // Step 2: Send identity packet. Over the internet road the desktop
        // names itself only once it has admitted the phone (below): anyone
        // can dial its iroh id, which the DHT publishes.
        if let (Some(identity), Road::Local { .. }) = (&identity_packet, road) {
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
        let bytes = timeout(
            Duration::from_secs(5),
            read_handshake_line(&mut reader, &mut line),
        )
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
        // U3: what the phone calls itself, kept once it is paired (at the
        // start, and when it pairs on this connection).
        let claimed_name = identity
            .body
            .get("deviceName")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let peer = match road {
            Road::Local { peer: None, .. } => return Ok(()),
            Road::Local {
                peer: Some(peer_addr),
                ..
            } => {
                // A connection that claims a paired device without its key
                // is kept for pairing only (the owner's PIN recovers a phone
                // whose key changed); the per-packet gate below holds it
                // there.
                if let crate::plugins::pair::Standing::WrongKey(refusal) =
                    crate::plugins::pair::admit_control(&device_id, peer_addr, peer_key.as_ref())
                {
                    tracing::warn!(
                        "Control connection for {device_id} held to pairing only: {}",
                        refusal.reason()
                    );
                }
                Some(peer_addr)
            }
            Road::Internet => {
                // Nothing is kept for pairing on this road: a connection
                // that is not the paired phone with its iroh key ends here.
                if let Err(refusal) =
                    crate::plugins::pair::internet_standing(&device_id, peer_key.as_ref())
                {
                    tracing::warn!(
                        "Refused {device_id}'s control connection over the internet: {}",
                        refusal.reason()
                    );
                    // Said, so the phone counts this as a failed link and
                    // backs off, instead of redialing a link that just ends.
                    let mut w = writer.lock().await;
                    let refused = format!("{}\n", pocketdesk_core::protocol::HANDSHAKE_REFUSED);
                    let _ = w.write_all(refused.as_bytes()).await;
                    let _ = w.flush().await;
                    let _ = w.shutdown().await;
                    return Ok(());
                }
                // Admitted: the desktop's identity is the phone's answer.
                if let Some(identity) = &identity_packet {
                    let mut w = writer.lock().await;
                    w.write_all(&identity.to_wire()?).await?;
                    w.flush().await?;
                }
                None
            }
        };

        let sender: Arc<dyn DeviceSender> = Arc::new(
            TcpDeviceSender::from_arc(writer, device_id.clone())
                .with_peer(peer)
                .with_peer_key(peer_key.clone())
                .with_link_streams(link),
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
        // The first refused packet on a connection is said where the owner
        // sees it; the rest (a phone polling every few seconds) at debug.
        let mut refusal_said = false;
        // Packet types already reported as denied on this connection: a
        // phone polling with a permission off would otherwise log each poll.
        let mut denied_said = std::collections::HashSet::new();
        if state::may_receive_pushes(&*sender) {
            seed_desktop_state(&sender, hypr_ipc, wan_endpoint, road).await;
            seeded = true;
            note_claimed_name(&device_id, claimed_name.as_deref());
        }

        tracing::info!("Entering main packet loop");
        loop {
            let mut line = String::new();
            let read_result = timeout(
                Duration::from_secs(60),
                read_line_capped(&mut reader, &mut line, MAX_PACKET_LINE),
            )
            .await;

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
                                tracing::warn!(
                                    "Ignored a second identity on {device_id}'s connection"
                                );
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
                            let may_act = match road {
                                Road::Local { .. } => {
                                    crate::plugins::pair::standing(&device_id, peer_key.as_ref())
                                        .may_act(pairing_required)
                                }
                                Road::Internet => crate::plugins::pair::internet_standing(
                                    &device_id,
                                    peer_key.as_ref(),
                                )
                                .is_ok(),
                            };
                            // Pairing stays at home: on the internet road
                            // nothing pairs, or pairs again (T10a).
                            if pairing_packet && matches!(road, Road::Internet) {
                                tracing::info!(
                                    "Ignored {packet_type} from {device_id}: no pairing over \
                                     the internet"
                                );
                                continue;
                            }
                            if !pairing_packet && !may_act {
                                // Let in before, and no longer (BA2): the
                                // connection closes.
                                if closes(seeded, state::may_receive_pushes(&*sender)) {
                                    tracing::info!(
                                        "Closing {device_id}'s connection: no longer paired"
                                    );
                                    break;
                                }
                                if refusal_said {
                                    tracing::debug!(
                                        "Blocking {packet_type} from {device_id}: not paired, \
                                         or not with this key"
                                    );
                                } else {
                                    refusal_said = true;
                                    tracing::info!(
                                        "Blocking {packet_type} from {device_id}: not paired, \
                                         or not with this key (later packets at debug)"
                                    );
                                }
                                continue;
                            }
                            // F5: the owner's per-device permissions (spec P3).
                            if !pairing_packet
                                && !crate::plugins::pair::permits(&device_id, &packet_type)
                            {
                                if denied_said.insert(packet_type.clone()) {
                                    tracing::info!(
                                        "Denied {packet_type} from {device_id}: not permitted \
                                         for this device (later ones at debug)"
                                    );
                                } else {
                                    tracing::debug!(
                                        "Denied {packet_type} from {device_id}: not permitted"
                                    );
                                }
                                crate::plugins::answer_denied(&packet, sender_ref).await;
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
                                seed_desktop_state(&sender, hypr_ipc, wan_endpoint, road).await;
                                seeded = true;
                                note_claimed_name(&device_id, claimed_name.as_deref());
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
                    // Idle a minute: closed too once no longer let in (BA2).
                    if closes(seeded, state::may_receive_pushes(&*sender)) {
                        tracing::info!("Closing {device_id}'s connection: no longer paired");
                        break;
                    }
                    tracing::debug!("Peer idle - connection still alive");
                }
            }
        }

        // Cleanup
        state::unregister_client(sender.connection_id()).await;

        Ok(())
    }
}

/// Whether a connection closes: its device was let in on it (`was_let_in`:
/// paired, or pairing off) and is not now (BA2: unpaired, its connection
/// stayed open for good, iroh's keepalive holding an internet-road one, so
/// what it had going on it went on and it never saw the unpair). A device
/// never let in keeps its connection, to pair on it.
fn closes(was_let_in: bool, let_in_now: bool) -> bool {
    was_let_in && !let_in_now
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

pub async fn pair(
    pin: Option<String>,
    grant: Option<u64>,
    someone_else: bool,
    json: bool,
) -> Result<()> {
    // A guest's phone is someone else's too (M2b): the server narrows it either way.
    let someone_else = someone_else || grant.is_some();
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
    local_pin::write_preset(
        &dir,
        &pin_value,
        local_pin::now_unix(),
        grant,
        someone_else,
        None,
    )
    .with_context(|| format!("failed to write the PIN file in {}", dir.display()))?;
    let path = state::pair_pin_path()?;

    if json {
        crate::local_api::print(&crate::local_api::versioned(serde_json::json!({
            "pin": pin_value,
            "valid_secs": local_pin::PIN_TTL.as_secs(),
            "grant_secs": grant,
            "someone_else": someone_else,
        })));
        return Ok(());
    }
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
    if someone_else {
        println!(
            "The phone may send files and links only, and its sessions ask you first; change it on the desktop app's Devices page."
        );
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

pub async fn unpair(device_id: Option<String>, json: bool) -> Result<()> {
    // Checked first, so a mistyped id is said plainly; the update below
    // re-reads the store under its lock.
    let trusted = TrustStore::load_or_create(state::trust_store_path()?)?.trusted_devices();
    unpair_targets(device_id.as_deref(), &trusted).map_err(anyhow::Error::msg)?;
    let mut removed = Vec::new();
    TrustStore::update(state::trust_store_path()?, |store| {
        let targets = unpair_targets(device_id.as_deref(), &store.trusted_devices())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        for id in &targets {
            store.untrust_device(id)?;
            // Its keys go too, so a phone that pairs again can bind new ones.
            if let Ok(dir) = state::state_dir() {
                let _ = pocketdesk_core::local::device_keys::clear(&dir, id);
            }
        }
        removed = targets;
        Ok(())
    })?;
    if json {
        crate::local_api::print(&crate::local_api::versioned(
            serde_json::json!({ "unpaired": removed }),
        ));
    } else if removed.is_empty() {
        println!("No trusted devices.");
    } else {
        for id in &removed {
            println!("Untrusted {id}");
        }
    }
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

/// Let go of everything `session` holds: its gamepad buttons (through the
/// injector's per-session state), mouse buttons and keys.
/// Let go of whatever sessions that have ended still hold down.
fn release_ended(
    injector: &mut InputInjector,
    gamepads: &mut std::collections::HashMap<u64, u16>,
    held: &mut std::collections::HashMap<u64, crate::driving::Held>,
) {
    let live = crate::driving::live_refs();
    let gone: Vec<u64> = gamepads
        .keys()
        .chain(held.keys())
        .copied()
        .filter(|id| !live.iter().any(|(live_id, _)| live_id == id))
        .collect();
    for id in gone {
        release_session(injector, gamepads, held, id);
    }
}

fn release_session(
    injector: &mut InputInjector,
    gamepads: &mut std::collections::HashMap<u64, u16>,
    held: &mut std::collections::HashMap<u64, crate::driving::Held>,
    session: u64,
) {
    // T7: the game controller at rest, sticks too (its packets keep coming
    // while a stick is held, so one that stopped held it).
    injector.release_pad();
    if let Some(buttons) = gamepads.remove(&session).filter(|b| *b != 0) {
        let saved = injector.swap_gamepad_buttons(buttons);
        let _ = handle_input_packet(
            injector,
            InputPacket::Gamepad {
                axes: [0; 6],
                buttons: 0,
            },
        );
        injector.swap_gamepad_buttons(saved);
    }
    if let Some(mut keys) = held.remove(&session) {
        for release in keys.take_releases() {
            let _ = handle_input_packet(injector, release);
        }
    }
}

/// How often a missing input injector is tried again while input arrives.
const INJECTOR_RETRY: Duration = Duration::from_secs(10);

/// Whether to try making the input injector again at `now`.
fn retry_injector_now(last_try: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    last_try.is_none_or(|t| now.saturating_duration_since(t) >= INJECTOR_RETRY)
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

    /// T1: another program on the control port moves the server to the
    /// next free one; a free port is taken as configured.
    #[tokio::test]
    async fn the_control_port_moves_on_when_another_program_holds_it() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let (_listener, bound) = bind_control("127.0.0.1", port).await.unwrap();
        assert_ne!(bound, port, "held: moved on");
        let last = port.saturating_add(pocketdesk_core::local::listening::TRIES - 1);
        assert!(bound > port && bound <= last, "{bound} past {port}");
        drop(held);
        let (_again, bound) = bind_control("127.0.0.1", port).await.unwrap();
        assert_eq!(bound, port, "free again: taken as configured");
    }

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
    async fn a_kde_connect_devices_packet_goes_to_the_bridge_not_the_refusal() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The bridge makes its identity in a throwaway state folder.
        pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        let addr = control_port().await;
        let mut tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let hello = serde_json::json!({
            "id": 1,
            "type": "kdeconnect.identity",
            "body": {"deviceId": "a".repeat(32), "deviceName": "Phone", "protocolVersion": 8},
        });
        tcp.write_all(
            format!(
                "{hello}
"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let mut first = [0u8; 1];
        tokio::time::timeout(Duration::from_secs(5), tcp.read_exact(&mut first))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first[0],
            pocketdesk_core::protocol::TLS_HANDSHAKE_RECORD,
            "this computer opens TLS as the client (KDE's roles), no plain refusal"
        );
    }

    /// Counts the packets the connection handler dispatches to it.
    pub(super) struct Counting(pub(super) Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl pocketdesk_core::protocol::kdeconnect::Plugin for Counting {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn incoming_capabilities(&self) -> &'static [&'static str] {
            &["kdeconnect.ping"]
        }
        fn outgoing_capabilities(&self) -> &'static [&'static str] {
            &[]
        }
        async fn handle_packet(
            &self,
            _packet: &NetworkPacket,
            _sender: &dyn pocketdesk_core::protocol::kdeconnect::DeviceSender,
        ) -> pocketdesk_core::error::Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// A control port served by the real handler with one counting plugin.
    async fn counting_port(
        pairing_required: bool,
    ) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        // The gates read a throwaway state folder, never the user's.
        pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let desktop = CertManager::new().unwrap();
        let tls = tokio_rustls::TlsAcceptor::from(desktop.control_server_config().unwrap());
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = pocketdesk_core::protocol::kdeconnect::PluginRegistry::default();
        registry.register(Counting(count.clone()));
        let registry = Arc::new(registry);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let tls = tls.clone();
                let registry = registry.clone();
                tokio::spawn(async move {
                    let _ = handle_connection_with_kde(
                        stream,
                        tls,
                        None,
                        &registry,
                        None,
                        None,
                        pairing_required,
                    )
                    .await;
                });
            }
        });
        (addr, count)
    }

    /// Connect as a new phone and send one ping; the dispatch count after.
    async fn ping(addr: std::net::SocketAddr, count: &std::sync::atomic::AtomicUsize) -> usize {
        let phone = CertManager::new().unwrap();
        let identity = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
            format!("gate-test-{}", uuid::Uuid::new_v4()),
            "Gate Test Phone",
        );
        let mut stream =
            ConnectionManager::new(Duration::from_secs(5), phone.control_client_config())
                .connect(&addr.ip().to_string(), addr.port(), &identity)
                .await
                .expect("the control channel over TLS");
        let mut line = NetworkPacket::new("kdeconnect.ping").to_wire().unwrap();
        if !line.ends_with(b"\n") {
            line.push(b'\n');
        }
        stream.write_all(&line).await.unwrap();
        stream.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        count.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[tokio::test]
    async fn an_unpaired_phones_packets_are_never_dispatched() {
        let (addr, count) = counting_port(true).await;
        assert_eq!(ping(addr, &count).await, 0, "refused before any plugin");
        // The same packet with pairing off reaches the plugin: the refusal
        // above is the gate, not a test that cannot see dispatches.
        let (open_addr, open_count) = counting_port(false).await;
        assert_eq!(ping(open_addr, &open_count).await, 1);
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

    #[tokio::test]
    async fn a_packet_line_has_a_limit_too() {
        let endless = vec![b'x'; (MAX_PACKET_LINE + 1024) as usize];
        let mut reader = BufReader::new(&endless[..]);
        let mut line = String::new();
        assert!(
            read_line_capped(&mut reader, &mut line, MAX_PACKET_LINE)
                .await
                .is_err(),
            "one endless line cannot fill the desktop's memory"
        );
        let packet = format!(
            "{{\"type\":\"kdeconnect.ping\",\"body\":\"{}\"}}
",
            "y".repeat(100_000)
        );
        let mut reader = BufReader::new(packet.as_bytes());
        let mut line = String::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut line, MAX_PACKET_LINE)
                .await
                .unwrap(),
            packet.len()
        );
    }
}

#[cfg(test)]
mod unpair_tests {
    use super::unpair_targets;

    /// BA2: a connection closes once its device, let in on it, no longer
    /// is; one never let in stays, to pair on.
    #[test]
    fn a_connection_closes_once_its_device_is_no_longer_let_in() {
        assert!(super::closes(true, false));
        assert!(!super::closes(true, true));
        assert!(
            !super::closes(false, false),
            "an unpaired phone pairs on it"
        );
        assert!(!super::closes(false, true));
    }

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

#[cfg(test)]
mod injector_retry_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_missing_injector_is_tried_again_but_not_on_every_packet() {
        let t = Instant::now();
        assert!(retry_injector_now(None, t), "never tried yet");
        assert!(!retry_injector_now(Some(t), t + Duration::from_secs(3)));
        assert!(retry_injector_now(Some(t), t + INJECTOR_RETRY));
    }
}

#[cfg(test)]
mod internet_control_tests {
    use super::*;
    use pocketdesk_core::protocol::ALPN_CONTROL;
    use tokio::io::AsyncWriteExt;

    async fn endpoint(key: iroh::SecretKey) -> iroh::Endpoint {
        iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .secret_key(key)
            .alpns(vec![ALPN_CONTROL.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .unwrap()
            .bind()
            .await
            .unwrap()
    }

    /// 2c: a transfer on a stream the computer opens on the phone's own
    /// iroh connection, behind the control stream, which keeps answering.
    /// On threads, as the server's and the bridge's runtimes are: on one,
    /// the echo waited for the receiving end's hashing, which is the test's
    /// scheduling, not the link's (a CI runner's ping took most of the file).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_transfer_rides_the_link_while_the_control_stream_keeps_answering() {
        use pocketdesk_core::protocol::kdeconnect::OpenLinkStream;
        use pocketdesk_core::protocol::transfer::{self, Pace};
        use tokio::io::AsyncReadExt;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let desktop = endpoint(iroh::SecretKey::generate()).await;
        let phone = endpoint(iroh::SecretKey::generate()).await;
        let to_desktop =
            iroh::EndpointAddr::new(desktop.id()).with_ip_addr(desktop.bound_sockets()[0]);
        let accepting = {
            let desktop = desktop.clone();
            tokio::spawn(async move { desktop.accept().await.unwrap().await.unwrap() })
        };
        let phone_conn = phone.connect(to_desktop, ALPN_CONTROL).await.unwrap();
        let desktop_conn = accepting.await.unwrap();

        // The control stream: the phone pings, the desktop echoes.
        let (mut ping_send, mut ping_recv) = phone_conn.open_bi().await.unwrap();
        ping_send.write_all(b"p").await.unwrap();
        let (mut echo_send, mut echo_recv) = desktop_conn.accept_bi().await.unwrap();
        let echo = tokio::spawn(async move {
            let mut byte = [0u8; 1];
            while echo_recv.read_exact(&mut byte).await.is_ok() {
                if echo_send.write_all(&byte).await.is_err() {
                    break;
                }
            }
        });
        let mut byte = [0u8; 1];
        ping_recv.read_exact(&mut byte).await.unwrap();

        // The phone's side of the transfer: the key line, then the bytes.
        let data: Vec<u8> = (0..20_000_000u32).map(|i| (i % 251) as u8).collect();
        let size = data.len() as u64;
        let receiving = {
            let phone_conn = phone_conn.clone();
            tokio::spawn(async move {
                let (mut send, recv) = phone_conn.accept_bi().await.unwrap();
                let mut recv = tokio::io::BufReader::new(recv);
                let mut key = [0u8; 33];
                recv.read_exact(&mut key).await.unwrap();
                let mut stream = tokio::io::join(recv, &mut send);
                let mut memory = transfer::Memory::holding(&[]);
                let got = transfer::receive(
                    &mut stream,
                    &mut memory,
                    None,
                    Some(size),
                    size,
                    Pace::DEFAULT,
                    |_, _| {},
                )
                .await;
                transfer::finish(&mut stream, got.is_ok(), Pace::DEFAULT).await;
                (key, memory.into_bytes())
            })
        };
        let sending = tokio::spawn(async move {
            let link = IrohLink(desktop_conn);
            let mut stream = link.open().await.unwrap();
            stream
                .write_all(b"0123456789abcdef0123456789abcdef\n")
                .await
                .unwrap();
            let mut reader: &[u8] = &data;
            transfer::send(&mut reader, size, &mut stream, Pace::DEFAULT, |_| {}).await
        });

        // Pings while the file moves.
        let moving = std::time::Instant::now();
        let mut slowest = std::time::Duration::ZERO;
        while !sending.is_finished() {
            let started = std::time::Instant::now();
            ping_send.write_all(b"p").await.unwrap();
            ping_recv.read_exact(&mut byte).await.unwrap();
            slowest = slowest.max(started.elapsed());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(sending.await.unwrap().unwrap(), 0, "sent from the start");
        let (key, got) = receiving.await.unwrap();
        assert_eq!(&key, b"0123456789abcdef0123456789abcdef\n");
        assert_eq!(got.len() as u64, size);
        // A control stream starved behind the transfer would wait out all of
        // it; a busy runner slows every ping a little.
        let took = moving.elapsed();
        assert!(
            slowest < (took / 3).max(std::time::Duration::from_millis(500)),
            "the control stream answered within {slowest:?}, while the file took {took:?}"
        );
        echo.abort();
        phone.close().await;
        desktop.close().await;
    }

    /// Counts the pairing packets dispatched to it.
    struct CountingPairs(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl pocketdesk_core::protocol::kdeconnect::Plugin for CountingPairs {
        fn name(&self) -> &'static str {
            "counting-pairs"
        }
        fn incoming_capabilities(&self) -> &'static [&'static str] {
            &["kdeconnect.pair", "kdeconnect.pocketdesk.pair"]
        }
        fn outgoing_capabilities(&self) -> &'static [&'static str] {
            &[]
        }
        async fn handle_packet(
            &self,
            _packet: &NetworkPacket,
            _sender: &dyn pocketdesk_core::protocol::kdeconnect::DeviceSender,
        ) -> pocketdesk_core::error::Result<()> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// A phone dials the internet road as `phone_key`, says it is
    /// `device_id`, and sends one `packet_type`: how many packets the
    /// desktop's plugins saw, or the handshake's failure.
    async fn send_over_iroh(
        device_id: &str,
        phone_key: iroh::SecretKey,
        packet_type: &str,
    ) -> std::result::Result<usize, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let desktop = endpoint(iroh::SecretKey::generate()).await;
        let phone = endpoint(phone_key).await;
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = PluginRegistry::default();
        registry.register(super::control_tls_tests::Counting(count.clone()));
        registry.register(CountingPairs(count.clone()));
        let registry = Arc::new(registry);
        let serve = {
            let desktop = desktop.clone();
            tokio::spawn(async move {
                let conn = desktop
                    .accept()
                    .await
                    .unwrap()
                    .accept()
                    .unwrap()
                    .await
                    .unwrap();
                let desktop_identity = Some(
                    pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
                        "road-desktop",
                        "Road Desktop",
                    )
                    .as_identity_packet(),
                );
                handle_internet_control(conn, desktop_identity, &registry, None, None, true).await
            })
        };
        let conn = phone.connect(desktop.addr(), ALPN_CONTROL).await.unwrap();
        let (send, recv) = conn.open_bi().await.unwrap();
        let identity =
            pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(device_id, "Internet Phone");
        let shook = pocketdesk_core::protocol::connection::internet_control_handshake(
            tokio::io::join(recv, send),
            "desktop",
            &identity,
        )
        .await
        .map_err(|e| e.to_string());
        let mut shook = shook;
        if let Ok(stream) = shook.as_mut() {
            let mut line = NetworkPacket::new(packet_type)
                .with_body(serde_json::json!({ "pair": true }))
                .to_wire()
                .unwrap();
            if !line.ends_with(b"\n") {
                line.push(b'\n');
            }
            let _ = stream.write_all(&line).await;
            let _ = stream.flush().await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            let _ = stream.shutdown().await;
        }
        let _ = timeout(Duration::from_secs(5), serve).await;
        conn.close(0u32.into(), b"test");
        phone.close().await;
        desktop.close().await;
        shook.map(|_| count.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn paired_with_iroh_key(device_id: &str, key: Option<&iroh::SecretKey>) {
        pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        let dir = state::state_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        TrustStore::update(state::trust_store_path().unwrap(), |store| {
            store.trust_device(device_id)
        })
        .unwrap();
        if let Some(key) = key {
            use pocketdesk_core::local::device_keys::{self, Slot};
            device_keys::register(&dir, device_id, Slot::Tls, &"1".repeat(64)).unwrap();
            device_keys::register(&dir, device_id, Slot::Iroh, &key.public().to_string()).unwrap();
        }
    }

    #[tokio::test]
    async fn a_paired_phone_with_its_iroh_key_reaches_the_plugins() {
        let id = format!("internet-{}", uuid::Uuid::new_v4().simple());
        let key = iroh::SecretKey::generate();
        paired_with_iroh_key(&id, Some(&key));
        assert_eq!(
            send_over_iroh(&id, key.clone(), "kdeconnect.ping").await,
            Ok(1)
        );
        // Nothing pairs, or pairs again, over the internet.
        for pairing in ["kdeconnect.pair", "kdeconnect.pocketdesk.pair"] {
            assert_eq!(send_over_iroh(&id, key.clone(), pairing).await, Ok(0));
        }
    }

    #[test]
    fn a_phone_on_the_internet_road_receives_pushes_only_with_its_key() {
        let id = format!("internet-{}", uuid::Uuid::new_v4().simple());
        let key = iroh::SecretKey::generate();
        paired_with_iroh_key(&id, Some(&key));
        let on_road = |key: &iroh::SecretKey| {
            TcpDeviceSender::from_arc(Arc::new(Mutex::new(tokio::io::sink())), id.clone())
                .with_peer_key(Some(pocketdesk_core::streaming::PeerKey::Iroh(
                    key.public().to_string(),
                )))
        };
        assert!(state::may_receive_pushes(&on_road(&key)));
        assert!(!state::may_receive_pushes(&on_road(
            &iroh::SecretKey::generate()
        )));
    }

    #[tokio::test]
    async fn another_key_or_no_registered_key_is_refused_before_the_desktop_names_itself() {
        let refused = |outcome: &std::result::Result<usize, String>| matches!(outcome, Err(e) if e.contains(pocketdesk_core::protocol::HANDSHAKE_REFUSED));
        let id = format!("internet-{}", uuid::Uuid::new_v4().simple());
        paired_with_iroh_key(&id, Some(&iroh::SecretKey::generate()));
        let outcome = send_over_iroh(&id, iroh::SecretKey::generate(), "kdeconnect.ping").await;
        assert!(refused(&outcome), "someone else's key: {outcome:?}");
        let unregistered = format!("internet-{}", uuid::Uuid::new_v4().simple());
        paired_with_iroh_key(&unregistered, None);
        let outcome = send_over_iroh(
            &unregistered,
            iroh::SecretKey::generate(),
            "kdeconnect.ping",
        )
        .await;
        assert!(refused(&outcome), "no iroh key proved at home: {outcome:?}");
        let stranger = format!("internet-{}", uuid::Uuid::new_v4().simple());
        let outcome =
            send_over_iroh(&stranger, iroh::SecretKey::generate(), "kdeconnect.ping").await;
        assert!(refused(&outcome), "a stranger: {outcome:?}");
    }
}

/// The protocol's conformance vectors (ecosystem 1g,
/// `docs/protocol-vectors/control-v1.json`), replayed against the real
/// control handler with every plugin the server runs.
#[cfg(test)]
mod vector_tests {
    use super::*;
    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    const VECTORS: &str = include_str!("../../docs/protocol-vectors/control-v1.json");

    /// A control port served by the real handler, the real plugins and the
    /// computer's real identity packet, over a throwaway state folder.
    async fn real_port() -> std::net::SocketAddr {
        pocketdesk_core::local::paths::use_state_dir(
            std::env::temp_dir().join(format!("pd-server-tests-{}", std::process::id())),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let desktop = CertManager::new().unwrap();
        let tls = tokio_rustls::TlsAcceptor::from(desktop.control_server_config().unwrap());
        let registry = Arc::new(crate::plugins::register_all());
        let (incoming, outgoing) = crate::plugins::announced(&registry);
        let mut identity = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(
            "vector-test-computer",
            "Vector Test Computer",
        );
        identity.incoming_capabilities = incoming;
        identity.outgoing_capabilities = outgoing;
        let identity_packet = identity.as_identity_packet();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (tls, registry, identity_packet) =
                    (tls.clone(), registry.clone(), identity_packet.clone());
                tokio::spawn(async move {
                    let _ = handle_connection_with_kde(
                        stream,
                        tls,
                        Some(identity_packet),
                        &registry,
                        None,
                        None,
                        true,
                    )
                    .await;
                });
            }
        });
        addr
    }

    type Stream = Box<dyn ReadWrite>;
    trait ReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
    impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> ReadWrite for T {}

    /// A connection as a new phone: TLS or not, and the handshake done
    /// unless the case does it itself.
    async fn open(addr: std::net::SocketAddr, tls: bool, handshake: bool) -> BufReader<Stream> {
        open_as(
            addr,
            tls,
            handshake,
            &format!("vector-phone-{}", uuid::Uuid::new_v4()),
        )
        .await
    }

    async fn open_as(
        addr: std::net::SocketAddr,
        tls: bool,
        handshake: bool,
        id: &str,
    ) -> BufReader<Stream> {
        let tcp = TcpStream::connect(addr).await.unwrap();
        if !tls {
            return BufReader::new(Box::new(tcp));
        }
        let phone = CertManager::new().unwrap();
        let stream = tokio_rustls::TlsConnector::from(phone.control_client_config())
            .connect(
                rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap(),
                tcp,
            )
            .await
            .expect("TLS");
        let mut reader = BufReader::new(Box::new(stream) as Stream);
        if handshake {
            let me = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(id, "Vector Phone");
            write_line(&mut reader, HANDSHAKE_HELLO).await;
            assert_eq!(
                read_line(&mut reader, 5000).await.as_deref(),
                Some(HANDSHAKE_OK)
            );
            read_line(&mut reader, 5000)
                .await
                .expect("the computer's identity");
            let wire = String::from_utf8(me.as_identity_packet().to_wire().unwrap()).unwrap();
            write_line(&mut reader, wire.trim_end()).await;
        }
        reader
    }

    async fn write_line(stream: &mut BufReader<Stream>, line: &str) {
        let inner = stream.get_mut();
        inner.write_all(line.as_bytes()).await.unwrap();
        inner.write_all(b"\n").await.unwrap();
        inner.flush().await.unwrap();
    }

    /// The next line within `ms`, or `None` for silence; `Some("")` never
    /// (a closed connection reads as `CLOSED`).
    async fn read_line(stream: &mut BufReader<Stream>, ms: u64) -> Option<String> {
        let mut line = String::new();
        match timeout(Duration::from_millis(ms), stream.read_line(&mut line)).await {
            Err(_) => None,
            Ok(Ok(0)) | Ok(Err(_)) => Some(CLOSED.to_string()),
            Ok(Ok(_)) => Some(line.trim_end().to_string()),
        }
    }

    const CLOSED: &str = "<closed>";

    /// Whether `body` holds every field of `want` with the same value.
    fn holds(body: &Value, want: &Value) -> bool {
        want.as_object()
            .is_none_or(|fields| fields.iter().all(|(k, v)| body.get(k) == Some(v)))
    }

    async fn replay(addr: std::net::SocketAddr, case: &Value) {
        let name = case["name"].as_str().unwrap();
        let tls = case["tls"].as_bool().unwrap_or(true);
        let handshake = case["handshake"].as_bool().unwrap_or(true);
        let mut stream = open(addr, tls, tls && handshake).await;
        for step in case["steps"].as_array().unwrap() {
            let within = step["within_ms"].as_u64().unwrap_or(5000);
            if let Some(raw) = step["send_raw"].as_str() {
                write_line(&mut stream, raw).await;
            } else if let Some(repeat) = step.get("send_raw_repeat") {
                let text = repeat["text"]
                    .as_str()
                    .unwrap()
                    .repeat(repeat["times"].as_u64().unwrap() as usize);
                // The computer may close mid-line; what it then reads is the point.
                let inner = stream.get_mut();
                let _ = inner.write_all(text.as_bytes()).await;
                let _ = inner.write_all(b"\n").await;
                let _ = inner.flush().await;
            } else if let Some(packet) = step.get("send") {
                write_line(&mut stream, &packet.to_string()).await;
            } else if let Some(raw) = step["expect_raw"].as_str() {
                assert_eq!(
                    read_line(&mut stream, within).await.as_deref(),
                    Some(raw),
                    "{name}"
                );
            } else if step["expect_closed"].as_bool() == Some(true) {
                // Whatever was buffered, then the end.
                let mut rest = Vec::new();
                let closed =
                    timeout(Duration::from_millis(within), stream.read_to_end(&mut rest)).await;
                assert!(
                    matches!(closed, Ok(Ok(_)) | Ok(Err(_))),
                    "{name}: the connection was not closed"
                );
            } else if let Some(want) = step.get("expect") {
                let line = read_line(&mut stream, within).await;
                if want.is_null() {
                    assert_eq!(line, None, "{name}: expected silence");
                    continue;
                }
                let line = line.unwrap_or_else(|| panic!("{name}: nothing came"));
                let packet: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("{name}: not a packet ({e}): {line}"));
                assert_eq!(packet["type"], want["type"], "{name}: {line}");
                assert!(holds(&packet["body"], &want["body"]), "{name}: {line}");
                for field in want["body_has"].as_array().into_iter().flatten() {
                    assert!(
                        packet["body"].get(field.as_str().unwrap()).is_some(),
                        "{name}: no {field}"
                    );
                }
                if let Some(include) = want.get("capabilities_include") {
                    for (side, key) in [
                        ("incoming", "incomingCapabilities"),
                        ("outgoing", "outgoingCapabilities"),
                    ] {
                        let listed = packet["body"][key].as_array().unwrap();
                        for wanted in include[side].as_array().into_iter().flatten() {
                            assert!(listed.contains(wanted), "{name}: {key} lacks {wanted}");
                        }
                    }
                }
            } else {
                panic!("{name}: a step this replay does not know: {step}");
            }
        }
    }

    #[tokio::test]
    async fn a_second_identity_naming_a_paired_phone_borrows_nothing() {
        let addr = real_port().await;
        // A phone paired in the throwaway state folder, whose id a stranger
        // names after its own handshake.
        let pair = || {
            pocketdesk_core::protocol::kdeconnect::TrustStore::load_or_create(
                crate::state::trust_store_path().unwrap(),
            )
            .unwrap()
            .trust_device("vector-paired-phone")
            .unwrap();
        };
        let request = serde_json::json!({
            "type": "kdeconnect.pocketdesk.monitors",
            "body": { "request": true }
        });
        // The control: the paired phone itself is answered. The lib tests
        // share one throwaway state folder, so another may rewrite the trust
        // store meanwhile: pair again and retry before calling it a failure.
        let mut answered = false;
        for _ in 0..3 {
            pair();
            let mut paired = open_as(addr, true, true, "vector-paired-phone").await;
            write_line(&mut paired, &request.to_string()).await;
            // Its seed pushes come first; then the answer.
            for _ in 0..10 {
                match read_line(&mut paired, 5000).await {
                    Some(line) if line.contains("\"kdeconnect.pocketdesk.monitors\"") => {
                        answered = true;
                        break;
                    }
                    Some(line) if line != CLOSED => continue,
                    _ => break,
                }
            }
            if answered {
                break;
            }
        }
        assert!(answered, "the paired phone is answered");
        let identity = serde_json::json!({
            "type": "kdeconnect.identity",
            "body": { "deviceId": "vector-paired-phone", "deviceName": "Borrowed" }
        });
        // The stranger's silence counts only while the phone it names stayed
        // paired throughout: otherwise it would be unanswered for the wrong
        // reason, so try again.
        let still_paired = || {
            pocketdesk_core::protocol::kdeconnect::TrustStore::load_or_create(
                crate::state::trust_store_path().unwrap(),
            )
            .unwrap()
            .is_trusted("vector-paired-phone")
        };
        let mut judged = false;
        for _ in 0..3 {
            pair();
            let mut stream = open(addr, true, true).await;
            write_line(&mut stream, &identity.to_string()).await;
            write_line(&mut stream, &request.to_string()).await;
            let reply = read_line(&mut stream, 1000).await;
            if !still_paired() {
                continue;
            }
            assert_eq!(
                reply, None,
                "still the stranger it opened as: a paired phone's request would be answered"
            );
            judged = true;
            break;
        }
        assert!(judged, "the phone stayed paired through none of the tries");
    }

    #[tokio::test]
    async fn every_control_vector_holds_against_the_real_handler() {
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(vectors["version"], 1);
        let addr = real_port().await;
        let cases = vectors["cases"].as_array().unwrap();
        assert!(cases.len() >= 7);
        for case in cases {
            replay(addr, case).await;
        }
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;

    #[test]
    fn status_without_tailscale_says_so_and_goes_on() {
        let line = tailscale_status_line(Err(anyhow::anyhow!("tailscale status failed")));
        assert_eq!(line, "Tailscale: not running (it is optional)");
        assert_eq!(
            tailscale_status_line(Ok("100.64.0.1 desk".to_string())),
            "100.64.0.1 desk"
        );
    }
}

/// How often the computer's addresses are read for its announcement (P1).
const ADDRESSES_EVERY: Duration = Duration::from_secs(30);

/// P1: read this computer's addresses every [`ADDRESSES_EVERY`] and
/// announce again on a change. A test seam: with `POCKETDESK_LOCAL_IPS_FILE`
/// (absolute) set, the addresses are that file's lines.
async fn follow_addresses(
    announcement: std::sync::Arc<
        std::sync::Mutex<Option<pocketdesk_core::tailscale::lan::Announcement>>,
    >,
) {
    let seam = std::env::var_os("POCKETDESK_LOCAL_IPS_FILE").map(std::path::PathBuf::from);
    if seam.as_ref().is_some_and(|p| !p.is_absolute()) {
        tracing::warn!("POCKETDESK_LOCAL_IPS_FILE is not absolute: not used");
    }
    let seam = seam.filter(|p| p.is_absolute());
    let mut settle = pocketdesk_core::tailscale::lan::Settle::default();
    if let Some(file) = &seam {
        tracing::warn!(
            "POCKETDESK_LOCAL_IPS_FILE is set: {} stands in (a test seam)",
            file.display()
        );
    }
    loop {
        tokio::time::sleep(ADDRESSES_EVERY).await;
        let ips = match &seam {
            Some(file) => std::fs::read_to_string(file)
                .map(|s| {
                    s.lines()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            None => pocketdesk_core::tailscale::lan::get_local_ips().await,
        };
        let reading = pocketdesk_core::tailscale::lan::announced_addresses(ips);
        let current = announcement
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|a| a.addresses().to_vec());
        let Some(current) = current else {
            continue;
        };
        let Some(addresses) = settle.next(&current, reading) else {
            continue;
        };
        let count = addresses.len();
        let held = announcement.clone();
        let announced = tokio::task::spawn_blocking(move || {
            let mut slot = held.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_mut() {
                Some(current) => current
                    .announce_at(addresses.clone())
                    .map(|()| Some(addresses)),
                None => Ok(None),
            }
        })
        .await;
        match announced {
            // The addresses themselves at debug only: the Logs page's Copy
            // goes into reports (P1 review).
            Ok(Ok(Some(addresses))) => {
                tracing::info!(
                    "This computer's addresses changed: announced again at {count} address(es)"
                );
                tracing::debug!("Announced at {}", addresses.join(", "));
            }
            Ok(Ok(None)) => {}
            Ok(Err(e)) => tracing::warn!("Could not announce again at the new addresses: {e}"),
            Err(_) => {}
        }
    }
}
