//! Fine scrolling on Linux, end to end: a computer's screen window sends
//! the wheel in 120ths of a notch, and the computer it shows scrolls by
//! them. Two real servers side by side (each with its own folders and
//! ports, on loopback): A pairs with B and opens a window onto B with
//! control; a stand-in for the window puts the pointer in the middle and
//! turns the wheel, first a whole notch as a sender before fine scrolling
//! does, then a touchpad's quarter notches, then half notches sideways.
//! What arrives is read inside the compositor B injects into: a `wev`
//! filling its output logs to the file `PD_WEV_LOG` names, and the test
//! reads back each `axis` and `axis_value120` (or `axis_discrete`).
//!
//! B injects into a nested or headless compositor only, never the desktop
//! this computer shows: the test refuses to run without `PD_NESTED_WAYLAND`
//! (that compositor's socket), gives B only that compositor, and refuses
//! where `/dev/uinput` is open to this user (a uinput device would reach
//! every session here) or where that socket is the desktop's own. B's
//! picture is the test pattern, so no screen is read.
//!
//! Ignored: it starts two real servers and injects input. Run it with
//! `PD_NESTED_WAYLAND=<socket> PD_WEV_LOG=<file> cargo test -p
//! pocketdesk-server --test fine_scroll_live -- --ignored --nocapture`.

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

/// The window's relay, once its view shows with control.
fn join(a_state: &Path, b_id: &str) -> TcpStream {
    let file = wait_for(60, "the window's view file", || {
        wire::read_window(a_state, 0).filter(|f| f.device_id == b_id)
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
            "the window never showed with control"
        );
        match Message::read(&mut stream) {
            Ok(Message::Status(status))
                if status.phase == Phase::Showing && status.control == Some(true) =>
            {
                return stream;
            }
            Ok(Message::End { why }) => panic!("the window ended: {why}"),
            Ok(_) => {}
            Err(e) => panic!("the window's relay went: {e}"),
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

/// Refuse a socket that is this desktop's.
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
}

/// One wheel event `wev` logged on the pointer: its kind (`axis`,
/// `value120` or `discrete`), axis (0 vertical, 1 horizontal) and value.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wheel {
    kind: &'static str,
    axis: u32,
    value: f64,
}

/// The wheel events in one `wev` log, in order, from one pointer object.
fn wheel_events(log: &Path) -> Vec<Wheel> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let mut object: Option<String> = None;
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
        let kind = if event.starts_with("axis_value120:") {
            "value120"
        } else if event.starts_with("axis_discrete:") {
            "discrete"
        } else if event.starts_with("axis:") {
            "axis"
        } else {
            continue;
        };
        // "... axis: 0 (vertical), value: -3.750000" or "value120: 120"
        // or "discrete: 1".
        let axis = event
            .split("axis: ")
            .last()
            .and_then(|a| a.split_whitespace().next())
            .and_then(|a| a.parse().ok())
            .unwrap_or(u32::MAX);
        let value = event
            .rsplit(": ")
            .next()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(f64::NAN);
        out.push(Wheel { kind, axis, value });
    }
    out
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

/// Turn the wheel with `packet`, then wait for it to land.
fn turn(stream: &mut TcpStream, packet: InputPacket) {
    send(stream, packet);
    drain(stream, Duration::from_millis(250));
}

#[test]
#[ignore = "starts two real servers and injects input into a nested compositor"]
fn a_touchpads_fine_scroll_lands_in_120ths() {
    let nested = std::env::var("PD_NESTED_WAYLAND")
        .expect("PD_NESTED_WAYLAND: the nested compositor's socket (never this desktop's)");
    refuse_the_desktop(&nested);
    let log = PathBuf::from(std::env::var("PD_WEV_LOG").expect("PD_WEV_LOG: the wev log"));
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .is_err(),
        "/dev/uinput is open to this user: a uinput device would reach every session here"
    );
    let (a_control, b_control) = (19536, 19636);
    for port in [a_control, b_control] {
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("fine-scroll", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("fine-scroll", "b", b_control, b_control + 1);
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
            .env_remove("SWAYSOCK")
            .env_remove("DISPLAY")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE");
        if state == &b_state
            && let Some(file) = std::env::var_os("PD_SERVER_LOG")
        {
            let file = std::fs::File::create(file).unwrap();
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

    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
            hevc: false,
            window: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    let mut stream = join(&a_state, &b_id);
    // The pointer onto wev's surface, so the wheel goes to it.
    turn(
        &mut stream,
        InputPacket::MouseMoveAbs {
            x_norm: 32768,
            y_norm: 32768,
        },
    );
    let before = wheel_events(&log).len();

    // A notch up, as every build before fine scrolling sends it.
    turn(
        &mut stream,
        InputPacket::MouseScroll {
            dx: 0,
            dy: 1,
            fine: None,
        },
    );
    // A touchpad's quarter notches down: the fourth completes one.
    for notches in [0, 0, 0, -1] {
        turn(
            &mut stream,
            InputPacket::MouseScroll {
                dx: 0,
                dy: notches,
                fine: Some((0, -30)),
            },
        );
    }
    // Half notches right: the second completes one.
    for notches in [0, 1] {
        turn(
            &mut stream,
            InputPacket::MouseScroll {
                dx: notches,
                dy: 0,
                fine: Some((60, 0)),
            },
        );
    }
    std::thread::sleep(Duration::from_millis(500));

    let landed = wheel_events(&log)[before..].to_vec();
    for w in &landed {
        println!("{} axis {} value {}", w.kind, w.axis, w.value);
    }
    let axis: Vec<(u32, f64)> = landed
        .iter()
        .filter(|w| w.kind == "axis")
        .map(|w| (w.axis, w.value))
        .collect();
    // Wayland's down is positive: a notch up is -15, a quarter down 3.75.
    assert_eq!(
        axis,
        [
            (0, -15.0),
            (0, 3.75),
            (0, 3.75),
            (0, 3.75),
            (0, 3.75),
            (1, 7.5),
            (1, 7.5)
        ],
        "every turn as pixels, the fine ones as finely as they came"
    );
    // A client of wl_pointer 8 or newer gets value120 for the whole notches
    // only (the virtual pointer's steps are whole); an older one discrete.
    let steps: Vec<(u32, f64)> = landed
        .iter()
        .filter(|w| w.kind != "axis")
        .map(|w| {
            let per = if w.kind == "value120" { 120.0 } else { 1.0 };
            (w.axis, w.value / per)
        })
        .collect();
    assert_eq!(
        steps,
        [(0, -1.0), (0, 1.0), (1, 1.0)],
        "a step for each whole notch, on the turn that completed it"
    );

    let session_id = wire::read_window(&a_state, 0).map(|v| v.session_id);
    computers::request(
        &a_state,
        ComputerAsk::StopView {
            session_id,
            lock: false,
            window: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    drop(stream);
    std::thread::sleep(Duration::from_secs(1));
    drop(run);
}
