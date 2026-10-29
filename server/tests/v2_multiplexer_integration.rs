use std::sync::Arc;
use std::time::Duration;

use pocketdesk_core::protocol::kdeconnect::PluginRegistry;
use pocketdesk_core::protocol::v2::{ALPN_V2, IdentityPacketV2, perform_v2_handshake};
use pocketdesk_core::streaming::transport::{
    CertManager, StreamClient, StreamServer, StreamTransportConfig,
};

// We need to access handle_v2_session and the push registry
use pocketdesk_server::state::{self, ACTIVE_CLIENTS};
use pocketdesk_server::v2_multiplexer::handle_v2_session;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_reconnect_storm_pushes_to_one_connection() -> anyhow::Result<()> {
    pocketdesk_core::local::paths::use_test_dirs("server-it");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert_manager = Arc::new(CertManager::new()?);
    let registry = Arc::new(PluginRegistry::new());

    // Server
    let server_config = StreamTransportConfig {
        address: "127.0.0.1:0".parse()?,
        alpn: ALPN_V2.to_vec(),
        ..Default::default()
    };
    let server = StreamServer::new(server_config, &cert_manager).await?;
    let server_addr = server.local_addr()?;

    // Server loop calling handle_v2_session directly
    let registry_clone = registry.clone();
    tokio::spawn(async move {
        while let Some(incoming) = server.accept_connection().await {
            let conn = incoming.await.unwrap();
            let registry_c = registry_clone.clone();
            tokio::spawn(async move {
                let local_identity = IdentityPacketV2 {
                    device_id: "server-test-id".to_string(),
                    device_name: "Mock Server".to_string(),
                    min_version: 1,
                    max_version: 1,
                    capabilities: vec![],
                };
                // pairing_required=false: these mock clients never paired.
                let _ = handle_v2_session(conn, local_identity, registry_c, false).await;
            });
        }
    });

    // The storm clients never pair; with pairing off they may receive
    // pushes, which is what this test counts.
    state::set_pairing_required(false);

    // Clear active clients list to ensure clean state
    {
        let mut clients = ACTIVE_CLIENTS.lock().await;
        clients.clear();
    }

    // Client Storm!
    let mut handles = Vec::new();
    for _ in 0..10 {
        let cert_mgr = cert_manager.clone();

        let handle = tokio::spawn(async move {
            let client_config = StreamTransportConfig {
                alpn: ALPN_V2.to_vec(),
                ..Default::default()
            };
            let client = StreamClient::new(client_config, &cert_mgr).unwrap();

            if let Ok(conn) = client.connect(server_addr, "127.0.0.1").await
                && let Ok((mut send0, mut recv0)) = conn.open_bi().await
            {
                let local_id = IdentityPacketV2 {
                    device_id: "storm-client-id".to_string(), // SAME ID for all
                    device_name: "Storm Client".to_string(),
                    min_version: 1,
                    max_version: 1,
                    capabilities: vec![],
                };

                if perform_v2_handshake(&mut send0, &mut recv0, &local_id)
                    .await
                    .is_ok()
                {
                    // Keep connection alive for a while so they overlap
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                }
            }
        });
        handles.push(handle);
    }

    // Wait for all handshakes to theoretically complete and overlap
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Ten overlapping connections from one device: every live one stays
    // registered (closing one must not orphan the others), but pushes go to
    // exactly one of them — the newest — so the device is never pushed to
    // twice.
    {
        let registered = ACTIVE_CLIENTS.lock().await.len();
        assert!(registered > 1, "live connections must stay registered");
        let newest = ACTIVE_CLIENTS
            .lock()
            .await
            .last()
            .map(|c| c.connection_id().to_string());
        let targets = state::clone_clients().await;
        assert_eq!(
            targets.len(),
            1,
            "Reconnect storm failed! {} parallel brains",
            targets.len()
        );
        assert_eq!(
            Some(targets[0].connection_id().to_string()),
            newest,
            "pushes must go to the newest connection"
        );
    }

    // Wait for all to finish
    for handle in handles {
        let _ = handle.await;
    }

    Ok(())
}
