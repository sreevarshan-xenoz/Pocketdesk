//! A second server runs beside the installed one (reliability pass, R6a):
//! `POCKETDESK_CONFIG_DIR` and `POCKETDESK_STATE_DIR` give it its own
//! config (ports), iroh identity, device id and pairings, and `pocketdesk
//! stop` with the same variables stops only it.
//!
//! Ignored: it starts two real servers, which reach iroh's relays. Run it
//! with `cargo test -p pocketdesk-server --test second_server -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_pocketdesk");

fn folders(name: &str, control: u16, streaming: u16) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("pd-second-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (config, state) = (base.join("config"), base.join("state"));
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        config.join("config.toml"),
        format!(
            "control_port = {control}\nstreaming_port = {streaming}\nlan_discovery = false\ndht_publish = \"off\"\n"
        ),
    )
    .unwrap();
    (config, state)
}

fn pocketdesk(config: &Path, state: &Path) -> Command {
    let mut command = Command::new(EXE);
    command
        .env("POCKETDESK_CONFIG_DIR", config)
        .env("POCKETDESK_STATE_DIR", state)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Stops the servers and removes their folders however the test ends, so a
/// failed run leaves no server on the ports for the next one to find.
struct Cleanup {
    servers: Vec<std::process::Child>,
    folders: Vec<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for server in &mut self.servers {
            let _ = server.kill();
            let _ = server.wait();
        }
        for folder in &self.folders {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

fn wait_for_port(port: u16) {
    let start = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "port {port} never answered"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "starts two real servers, which reach iroh's relays"]
fn two_servers_run_side_by_side_and_stop_apart() {
    for port in [18716, 18816] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("a", 18716, 18717);
    let (b_config, b_state) = folders("b", 18816, 18817);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        run.servers
            .push(pocketdesk(config, state).arg("start").spawn().unwrap());
    }
    wait_for_port(18716);
    wait_for_port(18816);

    let id = |state: &Path| std::fs::read_to_string(state.join("device_id")).unwrap();
    assert_ne!(id(&a_state), id(&b_state), "each its own device id");
    let key = |config: &Path| std::fs::read(config.join("iroh_secret.key")).unwrap();
    assert_ne!(key(&a_config), key(&b_config), "each its own iroh identity");

    let stopped = pocketdesk(&b_config, &b_state)
        .arg("stop")
        .status()
        .unwrap();
    assert!(stopped.success());
    let start = Instant::now();
    while run.servers[1].try_wait().unwrap().is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the second server did not stop"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        run.servers[0].try_wait().unwrap().is_none(),
        "the first one still runs"
    );
    assert!(std::net::TcpStream::connect(("127.0.0.1", 18716)).is_ok());

    let _ = pocketdesk(&a_config, &a_state).arg("stop").status();
    let _ = run.servers[0].wait();
}
