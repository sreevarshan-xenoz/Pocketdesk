//! PC11 on Linux: each screen window's clicks land on its own screen of a
//! computer with several. Two real servers side by side (each with its own
//! folders and ports, on loopback): A pairs with B and opens two windows
//! onto B, screen 1 and screen 2, with control; a stand-in for each window
//! moves to the corners and the middle of its picture and clicks there.
//! Where the clicks land is read inside the compositor B injects into
//! (`wev` on each output), outside this test.
//!
//! B injects into a nested or headless compositor only, never the desktop
//! this computer shows: the test refuses to run without
//! `PD_NESTED_WAYLAND` (that compositor's socket), gives the servers only
//! that compositor (its `DISPLAY` from `PD_NESTED_DISPLAY`, its Hyprland
//! instance from `PD_NESTED_HYPRLAND`, else none), and refuses where
//! `/dev/uinput` is open to this user, as a uinput device would reach every
//! session here. B's picture is the test pattern, so no screen is read.
//!
//! Ignored: it starts two real servers and injects input. Run it with
//! `PD_NESTED_WAYLAND=<socket> cargo test -p pocketdesk-server --test
//! per_screen_pointer_live -- --ignored --nocapture`.

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
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
        windows.push(stream);
    }
    for (window, stream) in windows.iter_mut().enumerate() {
        for (name, x_norm, y_norm) in POINTS {
            println!("window {window} (screen {}): {name}", window + 1);
            send(stream, InputPacket::MouseMoveAbs { x_norm, y_norm });
            drain(stream, Duration::from_millis(300));
            for pressed in [true, false] {
                send(stream, InputPacket::MouseClick { button: 0, pressed });
                drain(stream, Duration::from_millis(100));
            }
            drain(stream, Duration::from_millis(400));
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
