//! Item 8: "Fit the screen to this window" on a Linux computer. Two real
//! servers side by side (each with its own folders and ports, on loopback):
//! A pairs with B and opens a window onto B with a fit, as the desktop app's
//! window asks it; B makes a headless screen of the window's size in its
//! compositor, A shows it, and as the window is "resized" (a new ask at
//! another size) B resizes the same screen. A second window gets a screen
//! of its own; stopping the windows removes both.
//!
//! What B's compositor has is read with the shell command `PD_OUTPUTS_CMD`,
//! which prints one `name width height` line per output there. With
//! `PD_FIT_REAL=1` B captures its real (nested) outputs instead of the test
//! pattern, and the picture's size, as the stream says it, is checked to be
//! the fitted screen's.
//!
//! B makes screens in a nested or headless compositor only, never the
//! desktop this computer shows: the test refuses to run without
//! `PD_NESTED_WAYLAND` (that compositor's socket) and one of
//! `PD_NESTED_HYPRLAND` (its instance signature) or `PD_NESTED_SWAYSOCK`,
//! gives the servers only that compositor, and refuses where that socket is
//! the desktop's own or the Hyprland instance is this desktop's.
//!
//! Ignored: it starts two real servers and makes outputs. Run it with
//! `PD_NESTED_WAYLAND=<socket> PD_NESTED_HYPRLAND=<signature> PD_OUTPUTS_CMD=...
//! cargo test -p pocketdesk-server --test fit_screen_live -- --ignored
//! --nocapture`.

#![cfg(target_os = "linux")]

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk, FitSize};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::view::{self as wire, Message, Phase, ViewFit};
use pocketdesk_core::local::{identity, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

/// B's outputs, as `name width height`.
fn outputs(command: &str) -> Vec<(String, u32, u32)> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .unwrap();
    assert!(out.status.success(), "PD_OUTPUTS_CMD failed");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((
                parts.next()?.to_string(),
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        })
        .collect()
}

/// The outputs `after` has that `before` had not.
fn new_ones(
    before: &[(String, u32, u32)],
    after: &[(String, u32, u32)],
) -> Vec<(String, u32, u32)> {
    after
        .iter()
        .filter(|o| !before.iter().any(|b| b.0 == o.0))
        .cloned()
        .collect()
}

fn ask(a_state: &Path, b_id: &str, window: u8, fit: Option<FitSize>) {
    computers::request(
        a_state,
        ComputerAsk::View {
            device_id: b_id.to_string(),
            monitor: 0,
            hevc: false,
            cursor: false,
            window,
            fit,
            borrow: None,
        },
        pin::now_unix(),
    )
    .unwrap();
}

fn size(width: u32, height: u32) -> Option<FitSize> {
    Some(FitSize { width, height })
}

/// Window `window`'s relay, joined at the view that came after `after` (a
/// session number), read until its status says `want`; the picture's size
/// when it has come, if `look` asks for it.
fn read_until(
    a_state: &Path,
    window: u8,
    b_id: &str,
    after: u64,
    want: ViewFit,
    look: bool,
) -> (TcpStream, u64, Option<(u16, u16)>) {
    let file = wait_for(60, "the window's view file", || {
        wire::read_window(a_state, window).filter(|f| f.device_id == b_id && f.session_id > after)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let start = Instant::now();
    let (mut fitted, mut picture) = (false, None);
    loop {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "window {window}: no {want:?} (fitted {fitted}, picture {picture:?})"
        );
        match Message::read(&mut stream) {
            Ok(Message::Status(status)) => {
                if let ViewFit::Failed(why) = &*status.fit {
                    panic!("window {window}: the fit failed: {why}");
                }
                if *status.fit == want && status.phase == Phase::Showing {
                    fitted = true;
                }
            }
            Ok(Message::Config { width, height }) => picture = Some((width, height)),
            Ok(Message::End { why }) => panic!("window {window} ended: {why}"),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("window {window}'s relay went: {e}"),
        }
        if fitted && (!look || picture.is_some()) {
            return (stream, file.session_id, picture);
        }
    }
}

#[test]
#[ignore = "starts two real servers and makes outputs in a nested compositor"]
fn a_window_gets_a_screen_of_its_size_that_follows_it() {
    let nested = std::env::var("PD_NESTED_WAYLAND")
        .expect("PD_NESTED_WAYLAND: the nested compositor's socket (never this desktop's)");
    let hypr = std::env::var_os("PD_NESTED_HYPRLAND");
    let sway = std::env::var_os("PD_NESTED_SWAYSOCK");
    assert!(
        hypr.is_some() || sway.is_some(),
        "PD_NESTED_HYPRLAND or PD_NESTED_SWAYSOCK"
    );
    assert_ne!(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        Some(nested.as_str()),
        "PD_NESTED_WAYLAND is this desktop's own WAYLAND_DISPLAY"
    );
    if let (Ok(ours), Some(nested)) = (std::env::var("HYPRLAND_INSTANCE_SIGNATURE"), &hypr) {
        assert_ne!(
            ours.as_str(),
            nested.to_string_lossy(),
            "PD_NESTED_HYPRLAND is this desktop's instance"
        );
    }
    if let (Ok(ours), Some(nested)) = (std::env::var("SWAYSOCK"), &sway) {
        assert_ne!(
            ours.as_str(),
            nested.to_string_lossy(),
            "PD_NESTED_SWAYSOCK is this desktop's"
        );
    }
    let command = std::env::var("PD_OUTPUTS_CMD").expect("PD_OUTPUTS_CMD");
    let real = std::env::var_os("PD_FIT_REAL").is_some();
    let (a_control, b_control) = (19526, 19626);
    for port in [a_control, b_control] {
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("fit-screen", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("fit-screen", "b", b_control, b_control + 1);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let mut server = pocketdesk(config, state);
        if !real {
            server.env(pocketdesk_core::streaming::test_pattern::VAR, "1");
        }
        server
            .env(
                pocketdesk_server::computer_view::SOUND_SEAM,
                state.parent().unwrap().join("view-sound.wav"),
            )
            .env("WAYLAND_DISPLAY", &nested)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("XDG_CURRENT_DESKTOP")
            .env_remove("DISPLAY");
        match &sway {
            Some(socket) => server.env("SWAYSOCK", socket),
            None => server.env_remove("SWAYSOCK"),
        };
        match &hypr {
            Some(instance) => server.env("HYPRLAND_INSTANCE_SIGNATURE", instance),
            None => server.env_remove("HYPRLAND_INSTANCE_SIGNATURE"),
        };
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
    std::thread::sleep(Duration::from_secs(3));
    let a_id = identity::device_id(&a_state).unwrap();
    let b_id = identity::device_id(&b_state).unwrap();
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = pairing_link(&b_state, b_control, b_control + 1, &code);
    assert_eq!(pair(&a_state, &link), "paired");
    // B lets A in without asking, with Input (which a screen needs), no sound.
    let mut trust = TrustStore::read(b_state.join(TRUST_FILE)).unwrap();
    let mut permissions = trust.permissions(&a_id);
    permissions.set(Capability::Input, true);
    permissions.ask_first = false;
    assert!(!permissions.allows(Capability::Audio), "no sound is sent");
    trust.set_permissions(&a_id, permissions).unwrap();

    let before = outputs(&command);
    println!("before: {before:?}");

    // The window's first ask, at its size.
    ask(&a_state, &b_id, 0, size(1280, 720));
    let (mut window0, session0, picture) = read_until(
        &a_state,
        0,
        &b_id,
        0,
        ViewFit::Fitted {
            width: 1280,
            height: 720,
        },
        real,
    );
    let made = new_ones(&before, &outputs(&command));
    println!("after the first ask: {made:?}, picture {picture:?}");
    assert_eq!(made.len(), 1, "one screen made: {made:?}");
    assert_eq!((made[0].1, made[0].2), (1280, 720));
    if real {
        assert_eq!(
            picture,
            Some((1280, 720)),
            "the picture is the fitted screen"
        );
    }
    let name = made[0].0.clone();

    // The window is resized: the same screen takes the new size.
    drop(window0);
    ask(&a_state, &b_id, 0, size(1000, 600));
    let (window0_again, session0b, picture) = read_until(
        &a_state,
        0,
        &b_id,
        session0,
        ViewFit::Fitted {
            width: 1000,
            height: 600,
        },
        real,
    );
    window0 = window0_again;
    let made = new_ones(&before, &outputs(&command));
    println!("after the resize: {made:?}, picture {picture:?}");
    assert_eq!(made.len(), 1, "still one screen: {made:?}");
    assert_eq!(
        made[0],
        (name.clone(), 1000, 600),
        "the same screen, resized"
    );
    if real {
        assert_eq!(picture, Some((1000, 600)));
    }

    // A second window has a screen of its own.
    ask(&a_state, &b_id, 1, size(800, 800));
    let (window1, session1, _) = read_until(
        &a_state,
        1,
        &b_id,
        0,
        ViewFit::Fitted {
            width: 800,
            height: 800,
        },
        false,
    );
    let made = new_ones(&before, &outputs(&command));
    println!("with a second window: {made:?}");
    assert_eq!(made.len(), 2, "a screen each: {made:?}");
    assert!(made.iter().any(|o| (o.1, o.2) == (1000, 600)));
    assert!(made.iter().any(|o| (o.1, o.2) == (800, 800)));

    // A window turned to a plain screen gives its screen up.
    ask(&a_state, &b_id, 1, None);
    wait_for(30, "the second window's screen removed", || {
        (new_ones(&before, &outputs(&command)).len() == 1).then_some(())
    });
    drop(window1);
    let _ = session1;

    // Closing the other window takes its screen away too.
    let session_id = wire::read_window(&a_state, 0).map(|v| v.session_id);
    assert_eq!(session_id, Some(session0b));
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
    drop(window0);
    wait_for(30, "every screen removed", || {
        new_ones(&before, &outputs(&command))
            .is_empty()
            .then_some(())
    });
    let after = outputs(&command);
    println!("after: {after:?}");
    assert_eq!(
        after.iter().map(|o| &o.0).collect::<Vec<_>>(),
        before.iter().map(|o| &o.0).collect::<Vec<_>>(),
        "B's outputs are as they were"
    );
    drop(run);
}
