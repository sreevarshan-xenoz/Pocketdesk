//! `pocketdesk stop` returns with the server gone, and promptly: asked while
//! the server still starts (where Tailscale is missing its probes alone took
//! many seconds) and asked once it runs. The installer's uninstaller deletes
//! the executable right after `stop`, so a server left running fails it.
//!
//! Ignored: it starts a real server. Run it with
//! `cargo test -p pocketdesk-server --test server_stops_promptly -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const EXE: &str = env!("CARGO_BIN_EXE_pocketdesk");
const CONTROL: u16 = 18926;

/// Well under the 10 s `stop` waits, which the server once outlasted.
const PROMPT: Duration = Duration::from_secs(5);

/// Stops the server and removes its folders however the test ends.
struct Cleanup {
    server: Option<Child>,
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

fn start(config: &Path, state: &Path) -> Child {
    Command::new(EXE)
        .arg("start")
        .env("POCKETDESK_CONFIG_DIR", config)
        .env("POCKETDESK_STATE_DIR", state)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Run `pocketdesk stop` and check it said the server stopped, the server
/// is gone, and both happened within [`PROMPT`].
fn stop_promptly(config: &Path, state: &Path, server: &mut Child, when: &str) {
    let asked = Instant::now();
    let out = Command::new(EXE)
        .arg("stop")
        .env("POCKETDESK_CONFIG_DIR", config)
        .env("POCKETDESK_STATE_DIR", state)
        .stderr(Stdio::null())
        .output()
        .unwrap();
    let took = asked.elapsed();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{when}: stop failed: {said}");
    assert!(said.contains(") stopped"), "{when}: stop said {said:?}");
    assert!(took < PROMPT, "{when}: stop took {took:?}");
    assert!(
        server.try_wait().unwrap().is_some(),
        "{when}: stop returned with the server still running"
    );
    assert!(!state.join("server.pid").exists(), "{when}: PID file left");
}

#[test]
#[ignore = "starts a real server"]
fn stop_returns_promptly_with_the_server_gone() {
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", CONTROL)).is_err(),
        "something already listens on {CONTROL}"
    );
    let base = std::env::temp_dir().join(format!("pd-stop-{}", std::process::id()));
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
            "control_port = {CONTROL}\nstreaming_port = {}\nlan_discovery = false\n",
            CONTROL + 1
        ),
    )
    .unwrap();

    // Asked while it starts: as soon as it has a PID file (and a moment for
    // the stop event it makes next).
    let server = run.server.insert(start(&config, &state));
    let began = Instant::now();
    while !state.join("server.pid").exists() {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "no PID file after 30 s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500));
    stop_promptly(&config, &state, server, "while starting");

    // Asked once it runs: its control port answers, and its first LAN scan
    // (which once held a runtime thread for seconds) is under way.
    let server = run.server.insert(start(&config, &state));
    let began = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", CONTROL)).is_err() {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "port {CONTROL} never answered"
        );
        assert!(
            server.try_wait().unwrap().is_none(),
            "the server exited while starting"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    std::thread::sleep(Duration::from_millis(500));
    stop_promptly(&config, &state, server, "running");
}
