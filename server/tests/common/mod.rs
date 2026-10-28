//! What the tests with two real servers side by side share (R6a's seams:
//! each with its own config and state folders and ports): their folders,
//! starting them, waiting, cleaning up, and pairing A with B as the
//! desktop app asks, through `computers_out/`. Each has a file for a
//! clipboard (`POCKETDESK_CLIPBOARD_FILE`) and a folder for Downloads
//! (`POCKETDESK_DOWNLOAD_DIR`): two servers on one machine would share its
//! real ones.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::{identity, pin};

const EXE: &str = env!("CARGO_BIN_EXE_pocketdesk");

/// A server's config and state folders, under `pd-<test>-<name>-<pid>` in
/// the temporary folder, with its ports and nothing reaching out but the
/// internet road.
pub fn folders(test: &str, name: &str, control: u16, streaming: u16) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("pd-{test}-{name}-{}", std::process::id()));
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

/// The server's command for those folders, its output dropped.
pub fn pocketdesk(config: &Path, state: &Path) -> Command {
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
pub fn clipboard(state: &Path) -> PathBuf {
    state.parent().unwrap().join("clipboard.txt")
}

/// The folder that stands in for a server's Downloads.
pub fn downloads(state: &Path) -> PathBuf {
    state.parent().unwrap().join("downloads")
}

/// Stops the servers and removes their folders however the test ends.
pub struct Cleanup {
    pub servers: Vec<std::process::Child>,
    pub folders: Vec<PathBuf>,
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
pub fn wait_for<T>(secs: u64, what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
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
pub fn pairing_link(state: &Path, control: u16, streaming: u16, pin: &str) -> PairLink {
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
pub fn pair(a_state: &Path, link: &PairLink) -> String {
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
