//! Two computers pair, keep a link (2f), share the clipboard (2g) and send
//! each other files (2h), as
//! two real servers side by side (R6a's seams: each with its own config and
//! state folders and ports). B mints a code as its Pair tab does; A is
//! asked to pair with B's pairing link as the desktop app asks, through
//! `computers_out/`. Each has a file for a clipboard
//! (`POCKETDESK_CLIPBOARD_FILE`) and a folder for Downloads
//! (`POCKETDESK_DOWNLOAD_DIR`): two servers on one machine would share its
//! real ones.
//!
//! Ignored: it starts two real servers, which reach iroh's relays. Run it
//! with `cargo test -p pocketdesk-server --test two_computers -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputerAsk, ComputersFile};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::sent::{self, SentState};
use pocketdesk_core::local::{device_keys, identity, outbox, pin};
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
            "control_port = {control}\nstreaming_port = {streaming}\nlan_discovery = false\ndht_publish = \"off\"\ncomputer_clipboard = true\n"
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
        .env("POCKETDESK_CLIPBOARD_FILE", clipboard(state))
        .env("POCKETDESK_DOWNLOAD_DIR", downloads(state))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The file that stands in for a server's clipboard.
fn clipboard(state: &Path) -> PathBuf {
    state.parent().unwrap().join("clipboard.txt")
}

/// The folder that stands in for a server's Downloads.
fn downloads(state: &Path) -> PathBuf {
    state.parent().unwrap().join("downloads")
}

/// Ask `from`'s server to send `file` to `to` as its desktop app asks, and
/// wait for it to arrive whole in `to_state`'s Downloads.
fn send_and_receive(from: &Path, to: &str, to_state: &Path, file: &Path) {
    let asked = outbox::request(from, to, file.to_str().unwrap(), pin::now_unix()).unwrap();
    let sent = wait_for(60, "the send to end", || {
        sent::read(from)
            .into_iter()
            .find(|f| f.id == asked.id && f.state != SentState::Offered)
            .map(|f| f.state)
    });
    assert_eq!(sent, SentState::Sent);
    let arrived = downloads(to_state).join(file.file_name().unwrap());
    assert_eq!(
        std::fs::read(&arrived).unwrap(),
        std::fs::read(file).unwrap(),
        "{} arrived whole",
        arrived.display()
    );
    // K1: with the date it had where it came from.
    assert_eq!(
        std::fs::metadata(&arrived).unwrap().modified().unwrap(),
        std::fs::metadata(file).unwrap().modified().unwrap(),
        "{} kept its date",
        arrived.display()
    );
}

/// Wait for `state`'s clipboard to hold `text`.
fn clipboard_holds(state: &Path, text: &str, what: &str) {
    wait_for(15, what, || {
        (std::fs::read_to_string(clipboard(state)).ok().as_deref() == Some(text)).then_some(())
    });
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

    // The clipboard, both ways (2g), once each watcher has had a look.
    std::thread::sleep(Duration::from_secs(3));
    std::fs::write(clipboard(&a_state), "copied on A").unwrap();
    clipboard_holds(&b_state, "copied on A", "A's copy on B");
    std::fs::write(clipboard(&b_state), "copied on B").unwrap();
    clipboard_holds(&a_state, "copied on B", "B's copy on A");
    // A pairing link holds a PIN: it stays where it was copied.
    std::fs::write(clipboard(&a_state), "pocketdesk://pair?v=1&pin=000000").unwrap();
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(
        std::fs::read_to_string(clipboard(&b_state)).unwrap(),
        "copied on B"
    );

    // Files both ways (2h): A shares on the link, B offers and A fetches.
    let file = |state: &Path, name: &str, seed: u8| {
        let path = state.parent().unwrap().join(name);
        let bytes: Vec<u8> = (0..3 * 1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8 ^ seed)
            .collect();
        std::fs::write(&path, bytes).unwrap();
        // An old date, to be kept where it lands (K1).
        let june = std::time::UNIX_EPOCH + Duration::from_secs(1_780_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(june))
            .unwrap();
        path
    };
    let b_id = link.device_id.clone();
    send_and_receive(&a_state, &b_id, &b_state, &file(&a_state, "from-a.bin", 1));
    send_and_receive(&b_state, &a_id, &a_state, &file(&b_state, "from-b.bin", 2));

    // A folder from B: five offers at once, which A fetches one at a time
    // (B takes two transfers from one device at once and refuses the rest).
    let folder = b_state.parent().unwrap().join("Trip");
    std::fs::create_dir_all(&folder).unwrap();
    for n in 0..5u8 {
        std::fs::write(folder.join(format!("photo-{n}.bin")), vec![n; 300_000]).unwrap();
    }
    let asked =
        outbox::request(&b_state, &a_id, folder.to_str().unwrap(), pin::now_unix()).unwrap();
    let sent = wait_for(90, "the folder to end", || {
        sent::read(&b_state)
            .into_iter()
            .find(|f| f.id == asked.id && f.state != SentState::Offered)
            .map(|f| f.state)
    });
    assert_eq!(sent, SentState::Sent);
    for n in 0..5u8 {
        let arrived = downloads(&a_state)
            .join("Trip")
            .join(format!("photo-{n}.bin"));
        assert_eq!(
            std::fs::read(&arrived).unwrap(),
            vec![n; 300_000],
            "{}",
            arrived.display()
        );
    }

    // T5: a folder from A, the other way: each file a share on the link,
    // into one new folder in B's Downloads, a subfolder kept.
    let album = a_state.parent().unwrap().join("Album");
    std::fs::create_dir_all(album.join("Day 2")).unwrap();
    for n in 0..3u8 {
        std::fs::write(album.join(format!("pic-{n}.bin")), vec![n + 10; 200_000]).unwrap();
    }
    std::fs::write(album.join("Day 2").join("pic-9.bin"), vec![9; 50_000]).unwrap();
    // An empty file is left out and said, not the end of the send.
    std::fs::write(album.join("empty.txt"), b"").unwrap();
    let asked = outbox::request(&a_state, &b_id, album.to_str().unwrap(), pin::now_unix()).unwrap();
    let sent = wait_for(90, "the folder from A to end", || {
        sent::read(&a_state)
            .into_iter()
            .find(|f| f.id == asked.id && f.state != SentState::Offered)
            .map(|f| f.state)
    });
    assert_eq!(sent, SentState::Sent);
    let row = sent::read(&a_state)
        .into_iter()
        .find(|f| f.id == asked.id)
        .unwrap();
    assert_eq!(row.name, "Album/");
    assert_eq!(row.note.as_deref(), Some("1 empty file left out"));
    for n in 0..3u8 {
        let arrived = downloads(&b_state)
            .join("Album")
            .join(format!("pic-{n}.bin"));
        assert_eq!(
            std::fs::read(&arrived).unwrap(),
            vec![n + 10; 200_000],
            "{}",
            arrived.display()
        );
    }
    assert_eq!(
        std::fs::read(
            downloads(&b_state)
                .join("Album")
                .join("Day 2")
                .join("pic-9.bin")
        )
        .unwrap(),
        vec![9; 50_000]
    );

    // Pairing again (a fresh code) starts the link over, and it says so.
    let again = pin::mint_value();
    pin::write(&b_state, &again, pin::now_unix(), None).unwrap();
    let outcome = pair(
        &a_state,
        &pairing_link(&b_state, b_control, b_control + 1, &again),
    );
    assert_eq!(outcome, "paired");
    wait_for(30, "the link after pairing again", || {
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
