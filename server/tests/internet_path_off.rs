//! With `internet_path = false` the server binds no WAN (iroh) endpoint:
//! it makes no iroh identity, records no endpoint for a pairing code to
//! carry, and drops the one an earlier run left, so nothing reaches a relay
//! or the DHT.
//!
//! Ignored: it starts a real server. Run it with
//! `cargo test -p pocketdesk-server --test internet_path_off -- --ignored`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_pocketdesk");
const CONTROL: u16 = 18916;

/// Stops the server and removes its folders however the test ends.
struct Cleanup {
    server: Option<std::process::Child>,
    base: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(server) = &mut self.server {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
#[ignore = "starts a real server"]
fn a_server_with_the_internet_path_off_binds_no_wan_endpoint() {
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", CONTROL)).is_err(),
        "something already listens on {CONTROL}"
    );
    let base = std::env::temp_dir().join(format!("pd-netoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let mut run = Cleanup {
        server: None,
        base: base.clone(),
    };
    let (config, state) = (base.join("config"), base.join("state"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "control_port = {CONTROL}\nstreaming_port = {}\nlan_discovery = false\ninternet_path = false\n",
            CONTROL + 1
        ),
    )
    .unwrap();
    // What an earlier run with the path on left behind.
    let stale = state.join("endpoint.json");
    std::fs::write(
        &stale,
        r#"{"endpointId":"x","relayUrls":[],"directAddrs":[]}"#,
    )
    .unwrap();
    std::fs::write(state.join("wan_status.json"), "{}").unwrap();

    run.server = Some(
        Command::new(EXE)
            .arg("start")
            .env("POCKETDESK_CONFIG_DIR", &config)
            .env("POCKETDESK_STATE_DIR", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", CONTROL)).is_err() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "port {CONTROL} never answered"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    // Time for an endpoint to bind, were one started.
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !config.join("iroh_secret.key").exists(),
        "no iroh identity is made"
    );
    assert!(!stale.exists(), "the last run's endpoint is dropped");
    assert!(
        !state.join("wan_status.json").exists(),
        "and its measurements"
    );
}
