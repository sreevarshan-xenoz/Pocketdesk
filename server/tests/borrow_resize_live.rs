//! The window handoff's "resize follows the viewer", host side, on Sway. In
//! this process, as the server's own code runs it: a stand-in for the owner's
//! other computer (paired both ways, in a throwaway state folder) lends a test
//! program's window, then asks again with `width` and `height` (the picture
//! area of the window that shows it) as that window is resized. The screen
//! made for the window takes the new mode in place, the window takes the new
//! size alone on it at its corner, and one frame of the screen (the picture a
//! session streams) is that size, so the borrowing computer's picture is one
//! pixel to a pixel. A size asked again that is the screen's own changes
//! nothing; odd, huge and tiny sizes are the screens' own clamp; a stranger's
//! ask resizes nothing; and `return` puts the window back at the size it had.
//!
//! The compositor is a throwaway headless Sway, never the desktop this
//! computer shows: the test refuses to run without `PD_NESTED_WAYLAND` (that
//! compositor's socket) and `PD_NESTED_SWAYSOCK`, gives this process only
//! that compositor, and refuses where that socket is the desktop's own. The
//! test program (`PD_LEND_CLIENT`, default `wev`) is started there, and the
//! frame is taken with `grim`. Hyprland's resize is owed (its live check
//! needs a Hyprland, which is never started beside the owner's session).
//!
//! Ignored: it makes outputs and moves a window in a compositor. Run it with
//! `PD_NESTED_WAYLAND=<socket> PD_NESTED_SWAYSOCK=<socket> cargo test -p
//! pocketdesk-server --test borrow_resize_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_server::hyprland::{HyprMonitor, HyprWindow};
use pocketdesk_server::{lend_window, sway_tree, virtual_screen};
use serde_json::{Value, json};

/// A computer paired both ways.
const BORROWER: &str = "resize-test-borrower";

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

/// What the compositor shows: the windows and the screens.
fn snapshot() -> (Vec<HyprWindow>, Vec<HyprMonitor>) {
    let (windows, _, monitors) = sway_tree::list().expect("Sway's tree");
    (windows, monitors)
}

/// The window of process `pid`, with the name of the screen it is on.
fn window_of(pid: u32) -> Option<(HyprWindow, String)> {
    let (windows, monitors) = snapshot();
    let w = windows.into_iter().find(|w| w.pid == pid)?;
    let name = usize::try_from(w.monitor)
        .ok()
        .and_then(|i| monitors.get(i))
        .map(|m| m.name.clone())?;
    Some((w, name))
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
            snapshot()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A PNG's width and height, from its header.
fn png_size(path: &Path) -> (u32, u32) {
    let head = std::fs::read(path).unwrap();
    assert!(head.starts_with(b"\x89PNG"), "not a PNG");
    let be = |at: usize| u32::from_be_bytes(head[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

fn ask(runtime: &tokio::runtime::Runtime, device: &str, body: Value) -> Value {
    runtime.block_on(virtual_screen::respond(device, &body))
}

/// One frame of the screen `name`, as a session streams it: its size.
fn frame_size(nested: &str, name: &str, to: &Path) -> (u32, u32) {
    let grim = Command::new("grim")
        .args(["-o", name])
        .arg(to)
        .env("WAYLAND_DISPLAY", nested)
        .status()
        .expect("grim");
    assert!(grim.success(), "grim could not take the screen");
    png_size(to)
}

/// Check that the window of `pid` is alone on the screen `name`, at its
/// corner, `width`×`height`, with the screen's mode that size, and one frame
/// of it is that size too. Waits for the compositor to show it.
fn check_follows(nested: &str, pid: u32, name: &str, width: i32, height: i32, frame: &Path) {
    let (on_it, on_name) = wait_for(10, "the window at the new size", || {
        window_of(pid).filter(|(w, s)| s == name && w.size == [width, height])
    });
    assert_eq!(on_name, name);
    let (windows, screens) = snapshot();
    let screen = screens
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no screen {name}: {screens:?}"));
    assert_eq!(
        (screen.width, screen.height),
        (width, height),
        "the screen's mode"
    );
    assert_eq!(on_it.at, [screen.x, screen.y], "at the screen's corner");
    let alone = windows
        .iter()
        .filter(|w| {
            screens
                .get(w.monitor as usize)
                .is_some_and(|m| m.name == name)
        })
        .count();
    assert_eq!(alone, 1, "alone on the screen");
    assert_eq!(
        frame_size(nested, name, frame),
        (width as u32, height as u32),
        "a frame of the screen is the size asked"
    );
}

#[test]
#[ignore = "makes outputs and moves a window in a nested compositor"]
fn a_lent_window_takes_the_size_its_viewers_window_has() {
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
        .join(format!("pd-resize-live-{}", std::process::id()));
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
        s.trust_computer("resize-test-one-way")?;
        Ok(())
    })
    .unwrap();
    computers::update(&state, |file| {
        for (id, mine) in [(BORROWER, true), ("resize-test-one-way", false)] {
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
    let (before, home_screen) = wait_for(30, "the test program's window", || window_of(pid));
    let (_, screens_before) = snapshot();
    println!(
        "before: window {}x{} on {home_screen}, workspace {:?}, {} screens",
        before.size[0],
        before.size[1],
        before.workspace.name,
        screens_before.len()
    );
    let frame = base.join("frame.png");

    // Lent at the window's own size (no size asked, as a borrower before
    // this asks).
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 1, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let name = lent["name"].as_str().unwrap().to_string();
    let (w0, h0) = (
        lent["width"].as_u64().unwrap() as i32,
        lent["height"].as_u64().unwrap() as i32,
    );
    assert_eq!((w0, h0), (before.size[0] / 2 * 2, before.size[1] / 2 * 2));
    check_follows(&nested, pid, &name, w0, h0, &frame);
    println!("lent: {name} {w0}x{h0}");
    let index = lent["index"].as_u64().unwrap();

    // The viewer's window is made smaller, larger, and a size of the screen
    // the other way round, each asked as the view would ask it.
    for (n, (ask_w, ask_h, want_w, want_h)) in [
        (1000u64, 600u64, 1000i32, 600i32),
        (1400, 900, 1400, 900),
        (640, 1000, 640, 1000),
        (1001, 601, 1000, 600),
        (100, 100, 320, 320),
        (4000, 1000, 2560, 640),
        (1280, 720, 1280, 720),
    ]
    .into_iter()
    .enumerate()
    {
        let started = Instant::now();
        let said = ask(
            &runtime,
            BORROWER,
            json!({ "action": "lend", "id": 10 + n, "window": before.address,
                    "width": ask_w, "height": ask_h }),
        );
        let took = started.elapsed();
        assert!(said.get("error").is_none(), "{ask_w}x{ask_h}: {said}");
        assert_eq!(said["name"], name.as_str(), "the same screen");
        assert_eq!(said["window"], before.address.as_str());
        assert_eq!(
            (said["width"].as_i64(), said["height"].as_i64()),
            (Some(i64::from(want_w)), Some(i64::from(want_h))),
            "{ask_w}x{ask_h}: {said}"
        );
        assert_eq!(said["index"].as_u64().unwrap(), index, "same capture index");
        check_follows(&nested, pid, &name, want_w, want_h, &frame);
        println!(
            "asked {ask_w}x{ask_h}: screen and window {want_w}x{want_h}, a frame {want_w}x{want_h}; answered in {} ms",
            took.as_millis()
        );
    }
    assert_eq!(snapshot().1.len(), screens_before.len() + 1, "one screen");

    // The same size again changes nothing (no mode set, no move).
    let started = Instant::now();
    let same = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 30, "window": before.address,
                "width": 1280, "height": 720 }),
    );
    assert_eq!(same["width"], 1280, "{same}");
    println!(
        "the same size again: answered in {} ms",
        started.elapsed().as_millis()
    );
    check_follows(&nested, pid, &name, 1280, 720, &frame);

    // A size that is no size is the screen as it is; a computer paired one
    // way, and a stranger, are refused and resize nothing.
    for odd in [
        json!({ "width": 0, "height": 600 }),
        json!({ "width": "wide", "height": 600 }),
        json!({ "width": 800 }),
    ] {
        let mut body = json!({ "action": "lend", "id": 31, "window": before.address });
        for (k, v) in odd.as_object().unwrap() {
            body[k] = v.clone();
        }
        let said = ask(&runtime, BORROWER, body);
        assert_eq!(said["width"], 1280, "{said}");
    }
    for device in ["resize-test-one-way", "resize-test-stranger"] {
        let said = ask(
            &runtime,
            device,
            json!({ "action": "lend", "id": 32, "window": before.address,
                    "width": 900, "height": 500 }),
        );
        assert!(
            said["error"].as_str().unwrap().contains("paired both ways"),
            "{device}: {said}"
        );
    }
    check_follows(&nested, pid, &name, 1280, 720, &frame);
    println!("odd sizes, a one-way computer and a stranger: nothing changed");

    // Returned: the window is back at the size it had, the screen is gone.
    let returned = ask(
        &runtime,
        BORROWER,
        json!({ "action": "return", "id": 40, "window": before.address }),
    );
    assert_eq!(returned["returned"], 1, "{returned}");
    let (back, back_screen) = wait_for(10, "the window back", || {
        window_of(pid).filter(|(w, s)| *s == home_screen && w.size == before.size)
    });
    assert_eq!(back.size, before.size, "its own size, not the last asked");
    assert_eq!(back.workspace.name, before.workspace.name);
    assert_eq!(
        snapshot().1.iter().map(|m| &m.name).collect::<Vec<_>>(),
        screens_before.iter().map(|m| &m.name).collect::<Vec<_>>(),
        "the screen is gone"
    );
    assert!(!state.join(lend_window::STATE_FILE).exists());
    println!(
        "returned: {}x{} on {back_screen}, screens as before",
        back.size[0], back.size[1]
    );
}
