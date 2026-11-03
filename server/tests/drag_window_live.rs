//! Dragging a window past the edge (window handoff 4), the host's half, on
//! Sway. In this process, as the server's own code runs it: the window in
//! front is read from the compositor's tree and picked for a crossing at the
//! screen's edge (a floating one is; a tiled one and a full-screen one are
//! not), an offer to a computer that cannot be reached moves nothing here,
//! and the window lent to the computer and dragged back is put where the
//! pointer came in.
//!
//! What this cannot do is the drag itself: the input-capture portal that
//! tells shared input where the pointer crossed (GNOME, KDE, Hyprland's) is
//! not Sway's, so the crossing is the test's own, and the buttons let go of
//! are the driver's unit tests'. Hyprland and two real computers are owed.
//!
//! The compositor is a throwaway headless Sway, never the desktop this
//! computer shows: the test refuses to run without `PD_NESTED_WAYLAND` (that
//! compositor's socket) and `PD_NESTED_SWAYSOCK`, gives this process only
//! that compositor, and refuses where that socket is the desktop's own. The
//! test program (`PD_LEND_CLIENT`, default `wev`) is started there; nothing
//! is sent to any input device, and commands go to the throwaway Sway only.
//!
//! Ignored: it makes outputs and moves a window in a compositor. Run it with
//! `PD_NESTED_WAYLAND=<socket> PD_NESTED_SWAYSOCK=<socket> cargo test -p
//! pocketdesk-server --test drag_window_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_server::hyprland::HyprWindow;
use pocketdesk_server::shared_input::drag_window::{self, Handover, Return, Why};
use pocketdesk_server::shared_input::wire::Screen;
use pocketdesk_server::{lend_window, sway_tree, virtual_screen};
use serde_json::{Value, json};

/// A computer paired both ways, and one paired one way.
const BORROWER: &str = "drag-test-borrower";
const ONE_WAY: &str = "drag-test-one-way";

/// Kills the test program and removes the throwaway folders.
struct Cleanup {
    child: Option<Child>,
    dir: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn windows() -> Vec<HyprWindow> {
    sway_tree::list().expect("Sway's tree").0
}

fn window_of(pid: u32) -> Option<HyprWindow> {
    windows().into_iter().find(|w| w.pid == pid)
}

fn wait_for<T>(secs: u64, what: &str, mut look: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(found) = look() {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; the compositor shows {:?}",
            windows()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A command to the throwaway Sway, and only that one.
fn sway(socket: &str, command: &str) {
    let done = Command::new("swaymsg")
        .args(["-s", socket, command])
        .env_remove("WAYLAND_DISPLAY")
        .stdout(Stdio::null())
        .status()
        .expect("swaymsg");
    assert!(done.success(), "Sway refused `{command}`");
}

fn ask(runtime: &tokio::runtime::Runtime, device: &str, body: Value) -> Value {
    runtime.block_on(virtual_screen::respond(device, &body))
}

/// The container the compositor draws for `pid` (title bar included): its
/// top left and size, as the pick reads it.
fn container(pid: u32) -> Option<[i32; 4]> {
    let candidates = drag_window::candidates_now().ok()?;
    let id = window_of(pid)?.address;
    candidates.into_iter().find(|c| c.id == id).map(|c| c.rect)
}

#[test]
#[ignore = "makes outputs and moves a window in a nested compositor"]
fn a_floating_window_at_the_edge_is_picked_and_dragged_back_where_the_pointer_came_in() {
    let nested = std::env::var("PD_NESTED_WAYLAND")
        .expect("PD_NESTED_WAYLAND: the nested compositor's socket (never this desktop's)");
    let sway_socket = std::env::var("PD_NESTED_SWAYSOCK")
        .expect("PD_NESTED_SWAYSOCK: the nested Sway's socket (never this desktop's)");
    assert_ne!(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        Some(nested.as_str()),
        "PD_NESTED_WAYLAND is this desktop's own WAYLAND_DISPLAY"
    );
    if let Ok(ours) = std::env::var("SWAYSOCK") {
        assert_ne!(ours, sway_socket, "PD_NESTED_SWAYSOCK is this desktop's");
    }
    let client = std::env::var("PD_LEND_CLIENT").unwrap_or_else(|_| "wev".to_string());

    let base = std::env::var_os("PD_LEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("pd-drag-live-{}", std::process::id()));
    let (state, config) = (base.join("state"), base.join("config"));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let mut run = Cleanup {
        child: None,
        dir: base.clone(),
    };

    // This process sees only the nested compositor and the throwaway folders.
    // SAFETY: the only test of this binary, and nothing else runs yet.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &nested);
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_SOCKET");
        std::env::remove_var("XDG_CURRENT_DESKTOP");
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::set_var("POCKETDESK_STATE_DIR", &state);
        std::env::set_var("POCKETDESK_CONFIG_DIR", &config);
        std::env::set_var("SWAYSOCK", &sway_socket);
    }
    std::fs::write(config.join("config.toml"), "lend_windows = true\n").unwrap();
    TrustStore::update(state.join(TRUST_FILE), |s| {
        s.trust_computer(BORROWER)?;
        s.trust_computer(ONE_WAY)?;
        Ok(())
    })
    .unwrap();
    computers::update(&state, |file| {
        for (id, mine) in [(BORROWER, true), (ONE_WAY, false)] {
            file.computers.push(PeerComputer {
                device_id: id.to_string(),
                name: format!("Computer {id}"),
                addresses: vec!["192.0.2.10".to_string()],
                control_port: 1716,
                stream_port: 4716,
                fingerprint: "ab".repeat(32),
                endpoint: None,
                paired_unix: 1,
                link: String::new(),
                link_unix: 0,
                lets_drive: false,
                mine,
                wake: None,
                viewed_unix: 0,
                keys_by_position: None,
            });
        }
    })
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let child = Command::new("stdbuf")
        .arg("-oL")
        .arg(&client)
        .env("WAYLAND_DISPLAY", &nested)
        .env_remove("DISPLAY")
        .env_remove("SWAYSOCK")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("could not start {client}: {e}"));
    let pid = child.id();
    run.child = Some(child);
    let first = wait_for(30, "the test program's window", || window_of(pid));
    let (_, _, monitors) = sway_tree::list().expect("Sway's tree");
    let home = usize::try_from(first.monitor)
        .ok()
        .and_then(|i| monitors.get(i))
        .expect("the window's screen")
        .clone();
    println!(
        "window on {} ({}x{} at {},{})",
        home.name, home.width, home.height, home.x, home.y
    );
    let right = home.x + home.width - 1;
    let mine = format!("[pid={pid}]");

    // A floating window at the right edge, with the focus: sized, then moved
    // until its container's right side is the screen's.
    let edge = home.x + home.width;
    let at_edge = |why: &str| -> [i32; 4] {
        sway(&sway_socket, &format!("{mine} floating enable"));
        wait_for(10, "the window floating", || {
            let all = drag_window::candidates_now().ok()?;
            all.iter()
                .any(|c| c.id == first.address && c.floating)
                .then_some(())
        });
        sway(&sway_socket, &format!("{mine} focus"));
        wait_for(15, why, || {
            let r = container(pid)?;
            // A window coming out of full screen takes its old size back a
            // moment after it is told one: told again until it holds.
            if r[2] != 600 {
                sway(
                    &sway_socket,
                    &format!("{mine} resize set width 600 px height 400 px"),
                );
                return None;
            }
            if r[0] + r[2] == edge && r[1] >= home.y {
                return Some(r);
            }
            sway(
                &sway_socket,
                &format!(
                    "{mine} move absolute position {} {}",
                    edge - r[2],
                    home.y + 200
                ),
            );
            None
        })
    };
    let rect = at_edge("the floating window at the edge");
    println!("floating container {rect:?}");
    let crossed = (right, rect[1] + 60);

    // The pick reads the real tree: the window in front, floating, under
    // the pointer that crossed.
    let all = drag_window::candidates_now().expect("Sway's windows");
    let picked = drag_window::pick(&all, crossed).expect("the floating window is picked");
    assert_eq!(picked.id, first.address);
    assert_eq!(picked.rect, rect);
    assert_eq!(picked.grab, (crossed.0 - rect[0], crossed.1 - rect[1]));
    println!(
        "picked: container {:?}, held at {:?}",
        picked.rect, picked.grab
    );

    // Crossed where the window is not: nothing is picked.
    assert_eq!(
        drag_window::pick(&all, (home.x + 10, home.y + 10)).unwrap_err(),
        Why::NotUnderPointer
    );

    // A tiled window is left alone, and so is a full-screen one.
    sway(&sway_socket, &format!("{mine} floating disable"));
    wait_for(10, "the window tiled", || {
        let all = drag_window::candidates_now().ok()?;
        all.iter()
            .any(|c| c.id == first.address && !c.floating)
            .then_some(())
    });
    let tiled = drag_window::candidates_now().unwrap();
    assert_eq!(
        drag_window::pick(&tiled, crossed).unwrap_err(),
        Why::Tiled,
        "{tiled:?}"
    );
    sway(&sway_socket, &format!("{mine} fullscreen enable"));
    wait_for(10, "the window full screen", || {
        let all = drag_window::candidates_now().ok()?;
        all.iter()
            .any(|c| c.id == first.address && c.fullscreen)
            .then_some(())
    });
    let full = drag_window::candidates_now().unwrap();
    assert_eq!(
        drag_window::pick(&full, crossed).unwrap_err(),
        Why::Fullscreen,
        "{full:?}"
    );
    sway(&sway_socket, &format!("{mine} fullscreen disable"));
    wait_for(10, "the window out of full screen", || {
        let all = drag_window::candidates_now().ok()?;
        all.iter()
            .any(|c| c.id == first.address && !c.fullscreen)
            .then_some(())
    });
    let rect = at_edge("the floating window again");

    // The lend's gate: paired both ways with the switch on is let through;
    // a computer paired one way, and a stranger, are not.
    assert_eq!(lend_window::may_lend(BORROWER), Ok(()));
    assert!(lend_window::may_lend(ONE_WAY).is_err());
    assert!(lend_window::may_lend("drag-test-stranger").is_err());

    // An offer to a computer that cannot be reached (no link here) moves
    // nothing: the window stays where it is until the other computer borrows
    // it.
    assert!(
        drag_window::pick(&drag_window::candidates_now().unwrap(), crossed).is_ok(),
        "the window is there to be picked, so the offer is made and only its sending fails"
    );
    let screen = Screen {
        x: 0,
        y: 0,
        width: 2560,
        height: 1440,
    };
    // Meanwhile the pointer comes back over the edge with the window's button
    // held, before the other computer has borrowed the window (the offer is
    // still waiting for its answer): the offer is withdrawn, never a silent
    // success, and the lend its app would ask next is refused.
    runtime.block_on(async {
        let offer = tokio::spawn(drag_window::hand_over(Handover {
            to: BORROWER.to_string(),
            crossed_at: crossed,
            drop_at: (1000, 700),
            screen,
        }));
        tokio::time::sleep(Duration::from_millis(800)).await;
        drag_window::bring_back(Return {
            from: BORROWER.to_string(),
            at: (home.x + 5, rect[1] + 60),
            screen: Screen {
                x: home.x,
                y: home.y,
                width: u32::try_from(home.width).unwrap(),
                height: u32::try_from(home.height).unwrap(),
            },
        })
        .await;
        offer.await.unwrap();
    });
    assert_eq!(
        container(pid),
        Some(rect),
        "the window is where it was, on its screen"
    );
    assert!(
        !state.join(lend_window::STATE_FILE).exists(),
        "nothing lent"
    );
    let there = window_of(pid).unwrap();
    assert_eq!(there.workspace.name, first.workspace.name);
    let refused = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 1, "window": first.address }),
    );
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|e| e.contains("dragged back")),
        "{refused}"
    );
    assert!(
        !state.join(lend_window::STATE_FILE).exists(),
        "a refused lend lends nothing"
    );
    assert_eq!(container(pid), Some(rect));
    // A drag back of a window nobody handed over asks no one and moves
    // nothing, and the window is lent as any borrow lends it (once: the
    // refusal was one).
    runtime.block_on(drag_window::bring_back(Return {
        from: BORROWER.to_string(),
        at: (home.x + 5, rect[1] + 60),
        screen: Screen {
            x: home.x,
            y: home.y,
            width: u32::try_from(home.width).unwrap(),
            height: u32::try_from(home.height).unwrap(),
        },
    }));
    assert_eq!(container(pid), Some(rect));

    // The other computer borrows it (the ask its view makes), and the window
    // is dragged back over the edge: it goes home with its container's top
    // left where the pointer came in.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 1, "window": first.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let screens_lent = sway_tree::list().unwrap().2.len();
    let want = [home.x + 240, home.y + 130];
    let started = Instant::now();
    let back = lend_window::return_dragged(BORROWER, &first.address, want);
    assert_eq!(back["returned"], 1, "{back}");
    let origin = wait_for(10, "the window back on its screen", || {
        let id: i64 = first.address.parse().ok()?;
        sway_tree::seen(id)
            .ok()
            .flatten()
            .filter(|w| w.output == home.name)
            .map(|w| (w.origin, w.frame, w.floating))
    });
    println!(
        "dragged back in {} ms: container at {:?}, {:?}, floating {}",
        started.elapsed().as_millis(),
        origin.0,
        origin.1,
        origin.2
    );
    assert_eq!(origin.0, want, "put where the pointer came in");
    assert_eq!(origin.1, [rect[2], rect[3]], "at the size it had");
    assert!(origin.2, "floating, as it was");
    assert_eq!(
        sway_tree::list().unwrap().2.len(),
        screens_lent - 1,
        "its screen is gone"
    );
    assert!(!state.join(lend_window::STATE_FILE).exists());

    // A window of another computer's, or one not lent, is not moved by a drag
    // back.
    let nothing = lend_window::return_dragged("drag-test-stranger", &first.address, [5, 5]);
    assert_eq!(nothing["returned"], 0, "{nothing}");
    assert_eq!(container(pid).map(|r| [r[0], r[1]]), Some(want));
}
