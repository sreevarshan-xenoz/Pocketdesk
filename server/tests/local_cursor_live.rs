//! The other computer's pointer, sent apart from its picture (local
//! cursor), end to end: two real servers side by side (each with its own
//! folders and ports, on loopback). B captures a headless sway the test is
//! started in, by wlroots screencopy; A opens a window onto B asking for
//! the pointer, and a stand-in for that window reads A's view relay. The
//! test moves that sway's pointer through a virtual pointer of its own
//! (never B's input: B keeps A's Input off, so the session is view-only and
//! nothing is injected by B at all) and reads what reaches the window:
//!
//! 1. the host said its picture has no pointer in it (`Mode`, separate), its
//!    pointer's shape (an image with something in it, a hotspot inside it),
//!    and that it shows;
//! 2. asked for places (as a window does while its pointer is locked), the
//!    pointer's places follow the pointer, and how long a move of that
//!    pointer takes to reach the window is measured: this is the path the
//!    pointer drawn here travels while locked; with the pointer free it is
//!    drawn by this computer's own system pointer and waits for nothing;
//! 3. a second window that asks for no pointer hears none of it (and the
//!    pictures keep coming).
//!
//! The test refuses to run outside a headless sway's own environment
//! (`WAYLAND_DISPLAY` and `SWAYSOCK` set, the socket under `/tmp`), and
//! where `/dev/uinput` is open to this user.
//!
//! Ignored: it starts two real servers, a real encoder and a compositor
//! client. Run it from a headless sway of its own (`WLR_BACKENDS=headless`,
//! its own `XDG_RUNTIME_DIR`) with a cursor theme:
//! `cargo test -p pocketdesk-server --test local_cursor_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::input::wlr_input::WlrInput;
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::view::{self as wire, Message};
use pocketdesk_core::local::{identity, pin};
use pocketdesk_core::protocol::cursor::{CursorMessage, CursorState};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::streaming::InputPacket;

fn stop(a_state: &Path) {
    let session_id = wire::read(a_state).map(|v| v.session_id);
    computers::request(
        a_state,
        ComputerAsk::StopView {
            session_id,
            lock: false,
            window: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
}

/// A stand-in for the window: joined to A's view relay for B.
fn join(a_state: &Path, b_id: &str) -> TcpStream {
    let file = wait_for(60, "A's view.json for B", || {
        wire::read(a_state).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    stream
}

/// The next message, or `None` after the read timeout.
fn next(window: &mut TcpStream) -> Option<Message> {
    match Message::read(window) {
        Ok(message) => Some(message),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            None
        }
        Err(e) => panic!("the relay went: {e}"),
    }
}

fn ask_view(a_state: &Path, b_id: &str, cursor: bool) {
    computers::request(
        a_state,
        ComputerAsk::View {
            device_id: b_id.to_string(),
            monitor: 0,
            hevc: false,
            cursor,
            window: 0,
            fit: None,
            borrow: None,
        },
        pin::now_unix(),
    )
    .unwrap();
}

#[test]
#[ignore = "starts two real servers and an encoder, in a headless sway of its own"]
fn a_computers_pointer_reaches_the_window_apart_from_its_picture() {
    let sway = std::env::var("SWAYSOCK").unwrap_or_default();
    assert!(
        std::env::var_os("WAYLAND_DISPLAY").is_some() && sway.starts_with("/tmp/"),
        "run this in a headless sway of its own (XDG_RUNTIME_DIR under /tmp, SWAYSOCK set)"
    );
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .is_err(),
        "/dev/uinput is open to this user: a device made there would reach every session"
    );
    let (a_control, b_control) = (19316, 19416);
    for port in [a_control, b_control] {
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("pointer", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("pointer", "b", b_control, b_control + 1);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        run.servers.push(
            pocketdesk(config, state)
                .env(
                    pocketdesk_server::computer_view::SOUND_SEAM,
                    state.parent().unwrap().join("view-sound.wav"),
                )
                .arg("start")
                .spawn()
                .unwrap(),
        );
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
    // B lets A in without asking, with Input off: view-only.
    let mut trust = TrustStore::read(b_state.join(TRUST_FILE)).unwrap();
    let mut permissions = trust.permissions(&a_id);
    permissions.ask_first = false;
    trust.set_permissions(&a_id, permissions).unwrap();

    // The sway's pointer, through a virtual one of the test's own.
    let mut mouse = WlrInput::connect().expect("a virtual pointer in the headless sway");
    let (y_norm, base) = (24_000u16, 12_000u16);
    mouse.motion_absolute(base, y_norm).unwrap();

    // 1. A window that asks for the pointer.
    ask_view(&a_state, &b_id, true);
    let mut window = join(&a_state, &b_id);
    let mut state = CursorState::default();
    let mut heard: Vec<CursorMessage> = Vec::new();
    let mut frames = 0u32;
    let mut places_asked = false;
    let (mut sent_at, mut latencies) = (None::<Instant>, Vec::<Duration>::new());
    let mut step = 0u16;
    let started = Instant::now();
    let mut last_move = Instant::now();
    let mut refreshing = false;
    let mut told_places = false;
    while latencies.len() < 20 {
        assert!(
            started.elapsed() < Duration::from_secs(90),
            "pointer and places within 90 s: {heard:?}, {frames} frames"
        );
        // Jiggle the pointer; once places are asked for, time each move.
        if last_move.elapsed() >= Duration::from_millis(120) {
            last_move = Instant::now();
            step += 1;
            let x = base + (step % 9) * 400;
            if places_asked && sent_at.is_none() {
                sent_at = Some(Instant::now());
            }
            mouse.motion_absolute(x, y_norm).unwrap();
        }
        match next(&mut window) {
            Some(Message::Cursor(message)) => {
                // The host says the whole pointer again every 1.5 s; the
                // place in that is not a move to time.
                if matches!(message, CursorMessage::Mode { .. }) {
                    refreshing = places_asked;
                }
                let refreshed = matches!(message, CursorMessage::Move { .. })
                    && std::mem::take(&mut refreshing);
                if matches!(message, CursorMessage::Move { .. })
                    && !refreshed
                    && let Some(at) = sent_at.take()
                {
                    // The first place after asking is the host's answer to
                    // the ask (where the pointer already was), not a move
                    // to time.
                    if std::mem::replace(&mut told_places, true) {
                        latencies.push(at.elapsed());
                    }
                }
                state.apply(message.clone());
                heard.push(message);
            }
            Some(Message::Frame { .. }) => frames += 1,
            Some(Message::End { why }) => panic!("the view ended: {why}"),
            _ => {}
        }
        // The window's pointer locks: it asks for the places.
        if state.separate && state.shape.is_some() && !places_asked {
            window
                .write_all(&Message::Input(InputPacket::CursorTrack { on: true }.encode()).encode())
                .unwrap();
            places_asked = true;
        }
    }
    assert!(
        matches!(heard.first(), Some(CursorMessage::Mode { separate: true })),
        "the first word of the host is that the picture has no pointer: {:?}",
        &heard[..heard.len().min(3)]
    );
    let shape = state.shape.as_ref().expect("the pointer's shape");
    assert!(shape.width > 0 && shape.height > 0);
    assert!(shape.hot_x < shape.width && shape.hot_y < shape.height);
    assert!(
        shape.rgba.iter().skip(3).step_by(4).any(|a| *a > 0),
        "an image with something in it"
    );
    assert!(state.visible, "it shows");
    // One shape for as many moves: sent again only by the host's refresh
    // (every 1.5 s), whole, never as another image.
    let shape_ids: std::collections::HashSet<u32> = heard
        .iter()
        .filter_map(|m| match m {
            CursorMessage::Shape(s) => Some(s.id),
            _ => None,
        })
        .collect();
    assert_eq!(shape_ids.len(), 1, "one image: {shape_ids:?}");
    let shapes = heard
        .iter()
        .filter(|m| matches!(m, CursorMessage::Shape(_)))
        .count();
    let refreshes = 2 + started.elapsed().as_millis() / 1500;
    assert!(
        shapes as u128 <= refreshes,
        "the shape is not sent again but by a refresh: {shapes} in {:?}",
        started.elapsed()
    );
    assert!(frames > 0, "the picture keeps coming");
    latencies.sort();
    println!(
        "pointer moves ({} of them): shape {}x{} hotspot ({}, {}), one image; \
         a move of the host's pointer reaches the window in median {:?}, worst {:?} (loopback)",
        latencies.len(),
        shape.width,
        shape.height,
        shape.hot_x,
        shape.hot_y,
        latencies[latencies.len() / 2],
        latencies[latencies.len() - 1],
    );
    stop(&a_state);
    wait_for(10, "A's view.json gone", || {
        wire::read(&a_state).is_none().then_some(())
    });

    // 3. A window that did not ask hears none of it.
    ask_view(&a_state, &b_id, false);
    let mut window = join(&a_state, &b_id);
    let (mut frames, started) = (0u32, Instant::now());
    while frames < 20 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "pictures within 60 s"
        );
        mouse
            .motion_absolute(base + (frames as u16 % 9) * 400, y_norm)
            .unwrap();
        match next(&mut window) {
            Some(Message::Cursor(message)) => panic!("a pointer nobody asked for: {message:?}"),
            Some(Message::Frame { .. }) => frames += 1,
            Some(Message::End { why }) => panic!("the view ended: {why}"),
            _ => {}
        }
    }
    stop(&a_state);
    drop(run);
}
