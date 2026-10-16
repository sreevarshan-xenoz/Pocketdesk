//! 2b's question: the phone opens its session and its internet control link
//! as two iroh endpoints with one secret key (its device key). Over a relay,
//! which knows an endpoint by its key, can both stay connected? Checklist §64
//! owed this. Relay only here (no direct paths), on a relay run in-process.
//!
//! Answered 2026-10-17: no. Two dialers with their own keys both connect and
//! answer every ping; with one key the first never connects (all 30 pings
//! lost) while the second works, as a relay knows one client per key. The
//! one-key test stays ignored until 2b gives the phone one endpoint per
//! computer, carrying both connections.

use iroh::endpoint::presets::Minimal;
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey};
use std::net::SocketAddr;
use std::time::Duration;

const ALPN: &[u8] = b"pocketdesk-one-key-test";

async fn relay_only(relay: &RelayUrl, key: Option<SecretKey>) -> Endpoint {
    let mut builder = Endpoint::builder(Minimal)
        .relay_mode(RelayMode::Custom(relay.clone().into()))
        .clear_ip_transports()
        .alpns(vec![ALPN.to_vec()]);
    if let Some(key) = key {
        builder = builder.secret_key(key);
    }
    builder.bind().await.unwrap()
}

async fn relay() -> (iroh_relay::server::Server, RelayUrl) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let relay = iroh_relay::server::RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap())
        .parse()
        .unwrap();
    (server, url)
}

/// One ping on `conn`: a bidirectional stream the desktop echoes.
async fn ping(conn: &iroh::endpoint::Connection) -> bool {
    let round = async {
        let (mut send, mut recv) = conn.open_bi().await.ok()?;
        send.write_all(b"ping").await.ok()?;
        send.finish().ok()?;
        recv.read_to_end(16).await.ok()
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(3), round).await,
        Ok(Some(reply)) if reply == b"ping"
    )
}

/// What two endpoints dialing one desktop over the relay saw in `secs`
/// seconds of a ping each second: (connected, pings lost) for each.
async fn two_dialers(same_key: bool, secs: usize) -> [(bool, usize); 2] {
    let (_relay, url) = relay().await;
    let desktop = relay_only(&url, None).await;
    // The desktop echoes every stream on every connection.
    let echoing = {
        let desktop = desktop.clone();
        tokio::spawn(async move {
            while let Some(incoming) = desktop.accept().await {
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                        if let Ok(data) = recv.read_to_end(16).await {
                            let _ = send.write_all(&data).await;
                            let _ = send.finish();
                        }
                    }
                });
            }
        })
    };
    let key = SecretKey::generate();
    let other = if same_key {
        key.clone()
    } else {
        SecretKey::generate()
    };
    let session = relay_only(&url, Some(key)).await;
    let control = relay_only(&url, Some(other)).await;
    for endpoint in [&desktop, &session, &control] {
        tokio::time::timeout(Duration::from_secs(10), endpoint.online())
            .await
            .expect("on the relay");
    }
    let to_desktop = EndpointAddr::new(desktop.id()).with_relay_url(url.clone());
    let dial = |endpoint: &Endpoint| {
        let (endpoint, to) = (endpoint.clone(), to_desktop.clone());
        async move {
            tokio::time::timeout(Duration::from_secs(15), endpoint.connect(to, ALPN))
                .await
                .ok()
                .and_then(Result::ok)
        }
    };
    let (session_conn, control_conn) = tokio::join!(dial(&session), dial(&control));
    let mut lost = [0usize; 2];
    for _ in 0..secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
        for (i, conn) in [&session_conn, &control_conn].into_iter().enumerate() {
            let answered = match conn {
                Some(conn) => ping(conn).await,
                None => false,
            };
            lost[i] += usize::from(!answered);
        }
    }
    echoing.abort();
    [
        (session_conn.is_some(), lost[0]),
        (control_conn.is_some(), lost[1]),
    ]
}

#[tokio::test]
async fn two_endpoints_with_their_own_keys_both_stay_connected() {
    // The harness's control: the relay carries two dialers when they differ.
    let seen = two_dialers(false, 10).await;
    println!("own keys: {seen:?}");
    assert_eq!(seen, [(true, 0), (true, 0)]);
}

#[tokio::test]
#[ignore = "documents the relay rule 2b works around: two endpoints with one key cannot share a relay"]
async fn two_endpoints_with_one_key_on_one_relay_both_stay_connected() {
    let seen = two_dialers(true, 30).await;
    println!("one key: (connected, pings lost in 30 s) {seen:?}");
    assert_eq!(
        seen,
        [(true, 0), (true, 0)],
        "both connect and answer every ping"
    );
}
