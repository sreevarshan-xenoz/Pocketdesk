//! N2: endpoints connect through a relay we run ourselves, the dialer
//! knowing only the relay's address (no direct address), and the relay
//! admits only the endpoints on its list.

use iroh::endpoint::presets::Minimal;
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl};
use std::net::SocketAddr;

const ALPN: &[u8] = b"pocketdesk-relay-test";

async fn endpoint(relay: &RelayUrl) -> Endpoint {
    Endpoint::builder(Minimal)
        .relay_mode(RelayMode::Custom(relay.clone().into()))
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap()
}

/// A relay on loopback admitting what `allow` admits (`None`: everyone).
async fn relay(
    allow: Option<pocketdesk_server::relay::Allowlist>,
) -> (iroh_relay::server::Server, RelayUrl) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut relay = iroh_relay::server::RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    if let Some(allow) = allow {
        relay.access = std::sync::Arc::new(allow);
    }
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

#[tokio::test]
async fn two_endpoints_connect_through_a_relay_we_run() {
    let (_relay, url) = relay(None).await;
    let desktop = endpoint(&url).await;
    let phone = endpoint(&url).await;
    let accepting = {
        let desktop = desktop.clone();
        tokio::spawn(async move {
            let conn = desktop.accept().await.unwrap().await.unwrap();
            let mut recv = conn.accept_uni().await.unwrap();
            recv.read_to_end(64).await.unwrap()
        })
    };
    let to_desktop = EndpointAddr::new(desktop.id()).with_relay_url(url.clone());
    let conn = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        phone.connect(to_desktop, ALPN),
    )
    .await
    .expect("connected in time")
    .expect("connected through our relay");
    let mut send = conn.open_uni().await.unwrap();
    send.write_all(b"hello").await.unwrap();
    send.finish().unwrap();
    assert_eq!(accepting.await.unwrap(), b"hello");
}

#[tokio::test]
async fn an_endpoint_that_is_not_on_the_list_cannot_use_the_relay() {
    // The desktop is listed and binds its home relay; a stranger is not.
    let desktop_key = iroh::SecretKey::generate();
    let opts = pocketdesk_server::relay::RelayOptions {
        allow: vec![desktop_key.public().to_string()],
        ..Default::default()
    };
    let allow = pocketdesk_server::relay::Allowlist::from_options(&opts, None, None).unwrap();
    let (_relay, url) = relay(Some(allow)).await;
    let desktop = Endpoint::builder(Minimal)
        .relay_mode(RelayMode::Custom(url.clone().into()))
        .secret_key(desktop_key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let stranger = endpoint(&url).await;
    let to_desktop = EndpointAddr::new(desktop.id()).with_relay_url(url.clone());
    let attempt = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        stranger.connect(to_desktop, ALPN),
    )
    .await;
    assert!(
        !matches!(attempt, Ok(Ok(_))),
        "a stranger got through the relay"
    );
}
