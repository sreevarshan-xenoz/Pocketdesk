//! Another computer's screen in a window (BK7), between two real servers
//! side by side (R6a's seams: each with its own config and state folders
//! and ports): A pairs with B as the desktop app asks, then asks to show
//! B's screen; a stand-in for A's window reads A's view relay and counts
//! the frames and their size. B's screen is the test pattern
//! (`POCKETDESK_TEST_PATTERN`), so no one's desktop is read, and B's trust
//! entry for A keeps Input off, so the session is view-only and nothing is
//! injected anywhere; nothing is sent as input at all. No picture is kept:
//! frame counts and sizes are the evidence. The home network first, then,
//! with A's addresses for B taken away, the internet road.
//!
//! Ignored: it starts two real servers with a real encoder, which reach
//! iroh's relays. Run it with
//! `cargo test -p pocketdesk-server --test computer_view_live -- --ignored --nocapture`.

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::paths::{LIVE_FILE, TRUST_FILE};
use pocketdesk_core::local::view::{self as wire, Message, Phase};
use pocketdesk_core::local::{identity, pin, sessions};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

/// What the stand-in window saw of one view.
#[derive(Debug, Default)]
struct Seen {
    frames: u64,
    keyframes: u64,
    bytes: u64,
    size: Option<(u16, u16)>,
    route: String,
    control: Option<bool>,
    ended: Option<String>,
}

/// Read A's view relay as the window does for `secs`, after the first
/// frame, keeping only counts and the size.
fn watch(a_state: &Path, b_id: &str, secs: u64) -> Seen {
    let file = wait_for(30, "A's view.json for B", || {
        wire::read(a_state).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut seen = Seen::default();
    let start = Instant::now();
    let mut first: Option<Instant> = None;
    loop {
        if first.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "no picture within 90 s: {seen:?}"
        );
        match Message::read(&mut stream) {
            Ok(Message::Status(status)) => {
                if status.phase == Phase::Showing {
                    seen.control = status.control;
                }
                seen.route = status.route;
            }
            Ok(Message::Config { width, height }) => seen.size = Some((width, height)),
            Ok(Message::Frame { keyframe, data, .. }) => {
                first.get_or_insert_with(Instant::now);
                seen.frames += 1;
                seen.keyframes += u64::from(keyframe);
                seen.bytes += data.len() as u64;
            }
            Ok(Message::End { why }) => {
                seen.ended = Some(why);
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("the relay went: {e} ({seen:?})"),
        }
    }
    seen
}

/// B's live sessions from `device_id`.
fn sessions_from(b_state: &Path, device_id: &str) -> usize {
    std::fs::read_to_string(b_state.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .map_or(0, |mirror| {
            mirror
                .sessions
                .iter()
                .filter(|s| s.device_id.as_deref() == Some(device_id))
                .count()
        })
}

fn stop(a_state: &Path) {
    computers::request(a_state, ComputerAsk::StopView, pin::now_unix()).unwrap();
}

#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn a_computers_screen_is_shown_on_another_by_both_roads() {
    let (a_control, b_control) = (19116, 19216);
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("view", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("view", "b", b_control, b_control + 1);
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
                .env(pocketdesk_core::streaming::test_pattern::VAR, "1")
                .arg("start")
                .spawn()
                .unwrap(),
        );
    }
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
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
    assert_eq!(
        computers::read(&a_state).computers[0].stream_port,
        b_control + 1,
        "B's streaming port, from its link"
    );
    // B lets A in without asking (the prompt would wait for B's owner),
    // and keeps A's Input off: view-only.
    let mut trust = TrustStore::read(b_state.join(TRUST_FILE)).unwrap();
    let mut permissions = trust.permissions(&a_id);
    assert!(!permissions.allows(Capability::Input));
    assert!(!permissions.allows(Capability::Audio), "no sound is sent");
    permissions.ask_first = false;
    trust.set_permissions(&a_id, permissions).unwrap();

    // The home network.
    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    let home = watch(&a_state, &b_id, 10);
    println!(
        "home network: {} frames ({} keyframes, {} KiB) at {:?}, route {:?}, control {:?}",
        home.frames,
        home.keyframes,
        home.bytes / 1024,
        home.size,
        home.route,
        home.control
    );
    assert_eq!(home.ended, None);
    assert!(home.frames >= 20, "{home:?}");
    assert!(home.keyframes >= 1);
    assert!(home.size.is_some_and(|(w, h)| w > 0 && h > 0));
    assert_eq!(home.route, "home network");
    assert_eq!(home.control, Some(false), "B takes none of A's input");
    assert_eq!(sessions_from(&b_state, &a_id), 1, "B lists A's session");
    // Closing the window ends it on B.
    stop(&a_state);
    wait_for(30, "A's session gone from B", || {
        (sessions_from(&b_state, &a_id) == 0).then_some(())
    });
    wait_for(10, "A's view.json gone", || {
        wire::read(&a_state).is_none().then_some(())
    });

    // The internet road: no address of B is known to A now.
    computers::update(&a_state, |file| file.computers[0].addresses.clear()).unwrap();
    wait_for(60, "the link", || {
        (computers::read(&a_state).computers.first()?.link == "linked").then_some(())
    });
    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    let internet = watch(&a_state, &b_id, 10);
    println!(
        "internet road: {} frames ({} keyframes, {} KiB) at {:?}, route {:?}, control {:?}",
        internet.frames,
        internet.keyframes,
        internet.bytes / 1024,
        internet.size,
        internet.route,
        internet.control
    );
    assert_eq!(internet.ended, None);
    assert!(internet.frames >= 10, "{internet:?}");
    assert_eq!(internet.route, "internet");
    assert_eq!(internet.size, home.size);
    stop(&a_state);
    wait_for(30, "A's session gone from B", || {
        (sessions_from(&b_state, &a_id) == 0).then_some(())
    });
    drop(run);
}
