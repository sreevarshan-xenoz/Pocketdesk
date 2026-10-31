//! PC11 on Linux: each screen window's clicks land on its own screen of a
//! computer with several. Two real servers side by side (each with its own
//! folders and ports, on loopback): A pairs with B and opens two windows
//! onto B, screen 1 and screen 2, with control; a stand-in for each window
//! moves to the corners and the middle of its picture and clicks there.
//! Where the clicks land is read inside the compositor B injects into: a
//! `wev` filling each output logs to the files `PD_WEV_LOGS` names (screen
//! 1's, then screen 2's, `:` between), and the test reads them back: each
//! window's clicks all on its own screen, its corners on that screen's
//! corners. With `PD_RELAYOUT_CMD` (a shell command that moves or rescales
//! an output there), the screens change mid-session and every click is
//! made again.
//!
//! B injects into a nested or headless compositor only, never the desktop
//! this computer shows: the test refuses to run without
//! `PD_NESTED_WAYLAND` (that compositor's socket), gives the servers only
//! that compositor (its `DISPLAY` from `PD_NESTED_DISPLAY`, its Hyprland
//! instance from `PD_NESTED_HYPRLAND`, else none), and refuses where
//! `/dev/uinput` is open to this user, as a uinput device would reach every
//! session here, or where that socket is the desktop's own (this process's
//! `WAYLAND_DISPLAY`, or one in the login's runtime folder) or the Hyprland
//! instance is this desktop's. B's picture is the test pattern, so no
//! screen is read.
//!
//! Ignored: it starts two real servers and injects input. Run it with
//! `PD_NESTED_WAYLAND=<socket> cargo test -p pocketdesk-server --test
//! per_screen_pointer_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::view::{self as wire, Message, Phase};
use pocketdesk_core::local::{identity, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::streaming::InputPacket;

/// The picture's corners and middle, in the order they are clicked.
const POINTS: [(&str, u16, u16); 5] = [
    ("top left", 0, 0),
    ("top right", 65535, 0),
    ("bottom left", 0, 65535),
    ("bottom right", 65535, 65535),
    ("middle", 32768, 32768),
];

/// Window `window`'s relay, once its view shows with control; what its
/// status said of where clicks land.
fn join(a_state: &Path, window: u8, b_id: &str) -> (TcpStream, Option<bool>) {
    let file = wait_for(60, "the window's view file", || {
        wire::read_window(a_state, window).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let start = Instant::now();
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "window {window} never showed with control"
        );
        match Message::read(&mut stream) {
            Ok(Message::Status(status))
                if status.phase == Phase::Showing && status.control == Some(true) =>
            {
                // The ready stream's word comes with the control; give it a
                // moment to be read if not yet.
                if status.per_screen.is_some() || start.elapsed() > Duration::from_secs(20) {
                    return (stream, status.per_screen);
                }
            }
            Ok(Message::End { why }) => panic!("window {window} ended: {why}"),
            Ok(_) => {}
            Err(e) => panic!("window {window}'s relay went: {e}"),
        }
    }
}

/// Where `socket` (a `WAYLAND_DISPLAY`) is, as clients find it.
fn socket_path(socket: &str, runtime: Option<&std::ffi::OsStr>) -> PathBuf {
    let path = PathBuf::from(socket);
    if path.is_absolute() {
        return path;
    }
    runtime.map_or(path.clone(), |dir| Path::new(dir).join(&path))
}

/// Refuse a socket or Hyprland instance that is this desktop's (review N2).
fn refuse_the_desktop(nested: &str) {
    let ours = std::env::var("WAYLAND_DISPLAY").ok();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let nested_path = socket_path(nested, runtime.as_deref());
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some(ours) = &ours {
        assert!(
            canon(&nested_path) != canon(&socket_path(ours, runtime.as_deref())),
            "PD_NESTED_WAYLAND is this desktop's own WAYLAND_DISPLAY"
        );
    }
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let login = PathBuf::from(format!("/run/user/{uid}"));
    assert!(
        canon(&nested_path).parent() != Some(login.as_path()),
        "PD_NESTED_WAYLAND is in the login's runtime folder: the desktop's, not a nested one"
    );
    if let (Ok(ours), Ok(nested)) = (
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE"),
        std::env::var("PD_NESTED_HYPRLAND"),
    ) {
        assert_ne!(
            ours, nested,
            "PD_NESTED_HYPRLAND is this desktop's instance"
        );
    }
}

/// Where each left press landed in one `wev` log: the pointer's last
/// place on that surface (its output's own logical pixels).
fn presses(log: &Path) -> Vec<(f64, f64)> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut object: Option<String> = None;
    let mut at: Option<(f64, f64)> = None;
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix('[') else {
            continue;
        };
        let Some((head, event)) = rest.split_once("] ") else {
            continue;
        };
        let Some((id, interface)) = head.split_once(':') else {
            continue;
        };
        if interface.trim() != "wl_pointer" {
            continue;
        }
        // wev binds the pointer twice; one object's events are enough.
        let id = id.trim().to_string();
        if object.get_or_insert_with(|| id.clone()) != &id {
            continue;
        }
        if event.starts_with("enter:") || event.starts_with("motion:") {
            at = event.split_once("x, y: ").and_then(|(_, xy)| {
                let (x, y) = xy.split_once(", ")?;
                Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
            });
        } else if event.starts_with("leave:") {
            at = None;
        } else if event.starts_with("button:") && event.contains("(pressed)") {
            out.push(at.expect("a press with the pointer on no surface"));
        }
    }
    out
}

/// One round's five presses on one screen, in [`POINTS`]' order: the
/// corners on its corner pixels, the middle in its middle.
fn assert_round(screen: usize, round: &[(f64, f64)]) {
    let [top_left, top_right, bottom_left, bottom_right, middle] = round else {
        panic!("screen {screen}: {} presses in a round", round.len());
    };
    let near = |a: f64, b: f64, by: f64| (a - b).abs() <= by;
    assert!(
        near(top_left.0, 0.0, 0.5) && near(top_left.1, 0.0, 0.5),
        "screen {screen} top left at {top_left:?}"
    );
    let (right, bottom) = (bottom_right.0, bottom_right.1);
    assert!(
        right > 100.0 && bottom > 100.0,
        "screen {screen}: {bottom_right:?}"
    );
    assert!(
        near(top_right.0, right, 0.5) && near(top_right.1, 0.0, 0.5),
        "screen {screen} top right at {top_right:?}"
    );
    assert!(
        near(bottom_left.0, 0.0, 0.5) && near(bottom_left.1, bottom, 0.5),
        "screen {screen} bottom left at {bottom_left:?}"
    );
    // The last pixel: at most a pixel short of the size, never past it.
    assert!(
        right.fract() > 0.9 && bottom.fract() > 0.9,
        "screen {screen}: the bottom right {bottom_right:?} is not on its last pixel"
    );
    assert!(
        near(middle.0, (right + 1.0) / 2.0, 1.5) && near(middle.1, (bottom + 1.0) / 2.0, 1.5),
        "screen {screen} middle at {middle:?}"
    );
}

/// Each window clicks at each of [`POINTS`] of its picture.
fn click_round(windows: &mut [TcpStream], round: usize) {
    for (window, stream) in windows.iter_mut().enumerate() {
        for (name, x_norm, y_norm) in POINTS {
            println!(
                "round {round}, window {window} (screen {}): {name}",
                window + 1
            );
            send(stream, InputPacket::MouseMoveAbs { x_norm, y_norm });
            drain(stream, Duration::from_millis(300));
            for pressed in [true, false] {
                send(stream, InputPacket::MouseClick { button: 0, pressed });
                drain(stream, Duration::from_millis(100));
            }
            drain(stream, Duration::from_millis(400));
        }
    }
}

fn send(stream: &mut TcpStream, packet: InputPacket) {
    Message::Input(packet.encode()).write(stream).unwrap();
}

/// Read and drop what the relay sends meanwhile, for `wait`.
fn drain(stream: &mut TcpStream, wait: Duration) {
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let start = Instant::now();
    while start.elapsed() < wait {
        let _ = Message::read(stream);
    }
}

#[test]
#[ignore = "starts two real servers and injects input into a nested compositor"]
fn each_windows_clicks_land_on_its_own_screen() {
    let nested = std::env::var("PD_NESTED_WAYLAND")
        .expect("PD_NESTED_WAYLAND: the nested compositor's socket (never this desktop's)");
    refuse_the_desktop(&nested);
    let logs: Vec<PathBuf> = std::env::var("PD_WEV_LOGS")
        .expect("PD_WEV_LOGS: screen 1's and screen 2's wev logs, `:` between")
        .split(':')
        .map(PathBuf::from)
        .collect();
    assert_eq!(logs.len(), 2, "two wev logs");
    let relayout = std::env::var("PD_RELAYOUT_CMD").ok();
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .is_err(),
        "/dev/uinput is open to this user: a uinput device would reach every session here"
    );
    let (a_control, b_control) = (19516, 19616);
    for port in [a_control, b_control] {
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("per-screen", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("per-screen", "b", b_control, b_control + 1);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let mut server = pocketdesk(config, state);
        server
            .env(pocketdesk_core::streaming::test_pattern::VAR, "1")
            .env(
                pocketdesk_server::computer_view::SOUND_SEAM,
                state.parent().unwrap().join("view-sound.wav"),
            )
            .env("WAYLAND_DISPLAY", &nested)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XDG_CURRENT_DESKTOP")
            .env_remove("SWAYSOCK");
        match std::env::var_os("PD_NESTED_DISPLAY") {
            Some(display) => server.env("DISPLAY", display),
            None => server.env_remove("DISPLAY"),
        };
        match std::env::var_os("PD_NESTED_HYPRLAND") {
            Some(instance) => server.env("HYPRLAND_INSTANCE_SIGNATURE", instance),
            None => server.env_remove("HYPRLAND_INSTANCE_SIGNATURE"),
        };
        // B's log, where asked (`PD_SERVER_LOG`, with `RUST_LOG`).
        if state == &b_state
            && let Some(log) = std::env::var_os("PD_SERVER_LOG")
        {
            let file = std::fs::File::create(log).unwrap();
            server.stderr(file.try_clone().unwrap()).stdout(file);
        }
        run.servers.push(server.arg("start").spawn().unwrap());
    }
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (TcpStream::connect(("127.0.0.1", port)).is_ok()
                && identity::read_endpoint(state).is_some())
            .then_some(())
        });
    }
    // B reads its screens once it is up (xcap's names may wait for
    // Xwayland to start); a session before that says it does not know.
    std::thread::sleep(Duration::from_secs(3));
    let a_id = identity::device_id(&a_state).unwrap();
    let b_id = identity::device_id(&b_state).unwrap();
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = pairing_link(&b_state, b_control, b_control + 1, &code);
    assert_eq!(pair(&a_state, &link), "paired");
    // B lets A in without asking, and takes its input (no sound).
    let mut trust = TrustStore::read(b_state.join(TRUST_FILE)).unwrap();
    let mut permissions = trust.permissions(&a_id);
    permissions.set(Capability::Input, true);
    permissions.ask_first = false;
    assert!(!permissions.allows(Capability::Audio), "no sound is sent");
    trust.set_permissions(&a_id, permissions).unwrap();

    let mut windows = Vec::new();
    for window in [0u8, 1] {
        computers::request(
            &a_state,
            ComputerAsk::View {
                device_id: b_id.clone(),
                monitor: u32::from(window),
                hevc: false,
                window,
            },
            pin::now_unix(),
        )
        .unwrap();
        let (stream, per_screen) = join(&a_state, window, &b_id);
        println!(
            "window {window} (screen {}): per_screen {per_screen:?}",
            window + 1
        );
        assert_eq!(
            per_screen,
            Some(true),
            "window {window}: B says its clicks land on its screen"
        );
        windows.push(stream);
    }
    let before: Vec<usize> = logs.iter().map(|log| presses(log).len()).collect();
    click_round(&mut windows, 1);
    let mut rounds = 1;
    if let Some(command) = &relayout {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .status()
            .unwrap();
        assert!(status.success(), "the relayout command failed");
        println!("relaid out");
        // The compositor says the change, the server hears it at once.
        drain(&mut windows[0], Duration::from_secs(1));
        drain(&mut windows[1], Duration::from_secs(1));
        click_round(&mut windows, 2);
        rounds = 2;
    }
    std::thread::sleep(Duration::from_millis(500));
    for (screen, log) in logs.iter().enumerate() {
        let all = presses(log);
        let landed = &all[before[screen]..];
        println!(
            "screen {}: {} presses: {:?}",
            screen + 1,
            landed.len(),
            landed
        );
        assert_eq!(
            landed.len(),
            POINTS.len() * rounds,
            "screen {}: every press of its own window, and none of the other's",
            screen + 1
        );
        for round in landed.chunks(POINTS.len()) {
            assert_round(screen + 1, round);
        }
    }
    for window in [1u8, 0] {
        let session_id = wire::read_window(&a_state, window).map(|v| v.session_id);
        computers::request(
            &a_state,
            ComputerAsk::StopView {
                session_id,
                lock: false,
                window,
            },
            pin::now_unix(),
        )
        .unwrap();
    }
    drop(windows);
    std::thread::sleep(Duration::from_secs(1));
    drop(run);
}
