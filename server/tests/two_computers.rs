//! Two computers pair and keep a link (2f), as two real servers side by
//! side (R6a's seams: each with its own config and state folders and
//! ports). B mints a code as its Pair tab does; A is asked to pair with
//! B's pairing link as the desktop app asks, through `computers_out/`.
//!
//! Ignored: it starts two real servers, which reach iroh's relays. Run it
//! with `cargo test -p pocketdesk-server --test two_computers -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::{device_keys, identity, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};

const EXE: &str = env!("CARGO_BIN_EXE_pocketdesk");

fn folders(name: &str, control: u16, streaming: u16) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("pd-computers-{name}-{}", std::process::id()));
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

/// Stops the servers and removes their folders however the test ends.
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

/// Wait up to `secs` for `ready`, saying `what` if it never is.
fn wait_for<T>(secs: u64, what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "{what} within {secs} s"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// B's pairing link, as its Pair tab would show it, for `pin`.
fn pairing_link(state: &Path, control: u16, streaming: u16, pin: &str) -> PairLink {
    PairLink {
        name: "Computer B".to_string(),
        device_id: identity::device_id(state).unwrap(),
        addresses: vec!["127.0.0.1".to_string()],
        control_port: control,
        stream_port: streaming,
        pin: pin.to_string(),
        fingerprint: Some(identity::fingerprint_hex(
            identity::cert_fingerprint(state).unwrap(),
        )),
        endpoint: identity::read_endpoint(state).map(|e| e.to_string()),
    }
}

/// Ask A to pair with `link` and wait for that request's outcome.
fn pair(a_state: &Path, link: &PairLink) -> String {
    let asked = computers::request(
        a_state,
        ComputerAsk::Pair {
            link: link.to_uri(),
        },
        pin::now_unix(),
    )
    .unwrap();
    wait_for(60, "a pairing outcome", || {
        computers::read(a_state)
            .last_pairing
            .filter(|p| p.request == asked.id)
            .map(|p| p.outcome)
    })
}

#[test]
#[ignore = "starts two real servers, which reach iroh's relays"]
fn two_computers_pair_once_and_keep_a_link_over_the_internet_road() {
    let (a_control, b_control) = (18916, 19016);
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("a", a_control, a_control + 1);
    let (b_config, b_state) = folders("b", b_control, b_control + 1);
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
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
                && identity::read_endpoint(state).is_some())
            .then_some(())
        });
    }
    let a_id = identity::device_id(&a_state).unwrap();
    let a_endpoint = identity::read_endpoint(&a_state).unwrap()["endpointId"]
        .as_str()
        .unwrap()
        .to_string();
    let b_trust = || TrustStore::read(b_state.join("trusted_devices.json")).unwrap();

    // B's code, as its Pair tab mints one.
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = pairing_link(&b_state, b_control, b_control + 1, &code);

    // Another certificate: the PIN never reaches B.
    let mut forged = link.clone();
    forged.fingerprint = Some("ab".repeat(32));
    let outcome = pair(&a_state, &forged);
    assert!(outcome.contains("is not the one in the link"), "{outcome}");
    assert!(!b_trust().is_trusted(&a_id));
    assert!(computers::read(&a_state).computers.is_empty());

    // A wrong PIN.
    let mut wrong = link.clone();
    wrong.pin = if code == "000000" { "111111" } else { "000000" }.to_string();
    let outcome = pair(&a_state, &wrong);
    assert!(outcome.contains("refused the code"), "{outcome}");
    assert!(!b_trust().is_trusted(&a_id));

    // The code itself.
    let outcome = pair(&a_state, &link);
    assert_eq!(outcome, "paired");
    let trust = b_trust();
    assert!(trust.is_trusted(&a_id) && trust.is_computer(&a_id));
    assert_eq!(trust.permissions(&a_id), DevicePermissions::computer());
    assert!(trust.permissions(&a_id).allows(Capability::Clipboard));
    assert!(!trust.permissions(&a_id).allows(Capability::Input));
    assert!(
        matches!(
            device_keys::check(&b_state, &a_id, device_keys::Slot::Iroh, Some(&a_endpoint)),
            device_keys::Verdict::Matches
        ),
        "B holds A's iroh key"
    );
    let ComputersFile { computers, .. } = computers::read(&a_state);
    assert_eq!(computers.len(), 1);
    assert_eq!(computers[0].device_id, link.device_id);
    assert_eq!(computers[0].fingerprint, link.fingerprint.clone().unwrap());

    // The link comes up over the internet road.
    wait_for(60, "the link", || {
        let file = computers::read(&a_state);
        (file.computers.first()?.link == "linked").then_some(())
    });

    // A forgets B: its entry goes, and the link with it.
    computers::request(
        &a_state,
        ComputerAsk::Forget {
            device_id: link.device_id.clone(),
        },
        pin::now_unix(),
    )
    .unwrap();
    wait_for(30, "B forgotten", || {
        computers::read(&a_state).computers.is_empty().then_some(())
    });

    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let _ = pocketdesk(config, state).arg("stop").status();
    }
    for server in &mut run.servers {
        let _ = server.wait();
    }
}
