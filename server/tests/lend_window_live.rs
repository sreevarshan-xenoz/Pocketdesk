//! Phase 3a of the window handoff on a Linux computer: lending one of its
//! windows. In this process, as the server's own code runs it: a stand-in
//! for the owner's other computer (paired both ways, in a throwaway state
//! folder) asks `kdeconnect.pocketdesk.virtual_screen`'s `lend` for a test
//! program's window; the compositor makes a screen of the window's size, the
//! window sits alone on it, one frame of that screen is the window's size,
//! and a `return` puts the window on the screen and workspace it came from
//! and takes the screen away. The window list (`list`) names the test
//! program's window with its size and workspace and marks it once lent, and
//! one absolute pointer move and click, sent to the compositor's virtual
//! pointer as the host's input backend sends them, aimed at the lent screen,
//! land on the window (the test program, `wev`, logs where). The switch turned
//! off, the borrowing computer gone (the server's own watcher, run here) and a
//! lent window that is closed each end the loan with no sweep's minute to
//! wait for. A second lend is ended by the stop path and
//! a third by the start-up sweep of a server that stopped, each putting the
//! window back.
//!
//! The compositor is a throwaway headless Sway or a nested Hyprland, never
//! the desktop this computer shows: the test refuses to run without
//! `PD_NESTED_WAYLAND` (that compositor's socket) and one of
//! `PD_NESTED_SWAYSOCK` or `PD_NESTED_HYPRLAND` (its instance signature),
//! gives this process only that compositor, and refuses where that socket
//! is the desktop's own or the instance is this desktop's. The test program
//! (`PD_LEND_CLIENT`, default `wev`) is started there, and the frame is
//! taken with `grim`.
//!
//! Ignored: it makes outputs and moves a window in a compositor. Run it
//! with `PD_NESTED_WAYLAND=<socket> PD_NESTED_SWAYSOCK=<socket> cargo test
//! -p pocketdesk-server --test lend_window_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pocketdesk_core::input::wlr_input::WlrInput;
use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_server::hyprland::{HyprMonitor, HyprWindow, HyprlandIpc};
use pocketdesk_server::{lend_window, sway_tree, virtual_screen};
use serde_json::{Value, json};

/// A computer paired both ways.
const BORROWER: &str = "lend-test-borrower";

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

/// Whether the compositor is Sway, for a timeout's report.
static SWAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the compositor shows: the windows and the screens.
fn snapshot(sway: bool) -> (Vec<HyprWindow>, Vec<HyprMonitor>) {
    if sway {
        let (windows, _, monitors) = sway_tree::list().expect("Sway's tree");
        (windows, monitors)
    } else {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let ipc = HyprlandIpc::from_env().expect("Hyprland's sockets");
            (
                ipc.visible_windows().await.expect("j/clients"),
                ipc.monitors().await.expect("j/monitors"),
            )
        })
    }
}

/// The window of process `pid`, with the name of the screen it is on.
fn window_of(sway: bool, pid: u32) -> Option<(HyprWindow, String)> {
    let (windows, monitors) = snapshot(sway);
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
            snapshot(SWAY.load(std::sync::atomic::Ordering::Relaxed))
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A PNG's width and height, from its header.
fn png_size(path: &Path) -> (u32, u32) {
    let head = std::fs::read(path).unwrap();
    assert!(head.starts_with(b"\x89PNG"), "not a PNG");
    let be = |at: usize| u32::from_be_bytes(head[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

/// Where each left press landed in the test program's log: the pointer's
/// last place on its surface.
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
        } else if event.starts_with("button:")
            && event.contains("(pressed)")
            && let Some(at) = at
        {
            out.push(at);
        }
    }
    out
}

fn ask(runtime: &tokio::runtime::Runtime, device: &str, body: Value) -> Value {
    runtime.block_on(virtual_screen::respond(device, &body))
}

#[test]
#[ignore = "makes outputs and moves a window in a nested compositor"]
fn a_window_is_lent_to_a_screen_of_its_size_and_returned() {
    let nested = std::env::var("PD_NESTED_WAYLAND")
        .expect("PD_NESTED_WAYLAND: the nested compositor's socket (never this desktop's)");
    let hypr = std::env::var("PD_NESTED_HYPRLAND").ok();
    let sway_socket = std::env::var("PD_NESTED_SWAYSOCK").ok();
    assert!(
        hypr.is_some() ^ sway_socket.is_some(),
        "exactly one of PD_NESTED_HYPRLAND, PD_NESTED_SWAYSOCK"
    );
    assert_ne!(
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        Some(nested.as_str()),
        "PD_NESTED_WAYLAND is this desktop's own WAYLAND_DISPLAY"
    );
    if let (Ok(ours), Some(nested)) = (std::env::var("HYPRLAND_INSTANCE_SIGNATURE"), &hypr) {
        assert_ne!(
            ours, *nested,
            "PD_NESTED_HYPRLAND is this desktop's instance"
        );
    }
    if let (Ok(ours), Some(nested)) = (std::env::var("SWAYSOCK"), &sway_socket) {
        assert_ne!(ours, *nested, "PD_NESTED_SWAYSOCK is this desktop's");
    }
    let sway = sway_socket.is_some();
    SWAY.store(sway, std::sync::atomic::Ordering::Relaxed);
    let client = std::env::var("PD_LEND_CLIENT").unwrap_or_else(|_| "wev".to_string());

    let base = std::env::var_os("PD_LEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("pd-lend-live-{}", std::process::id()));
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
        std::env::set_var("POCKETDESK_STATE_DIR", &state);
        std::env::set_var("POCKETDESK_CONFIG_DIR", &config);
        match &sway_socket {
            Some(socket) => std::env::set_var("SWAYSOCK", socket),
            None => std::env::remove_var("SWAYSOCK"),
        }
        match &hypr {
            Some(instance) => std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", instance),
            None => std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE"),
        }
    }
    std::fs::write(config.join("config.toml"), "lend_windows = true\n").unwrap();
    TrustStore::update(state.join(TRUST_FILE), |s| {
        s.trust_computer(BORROWER)?;
        s.trust_computer("lend-test-one-way")?;
        s.trust_phone("lend-test-phone", true)?;
        Ok(())
    })
    .unwrap();
    computers::update(&state, |file| {
        for (id, mine) in [(BORROWER, true), ("lend-test-one-way", false)] {
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

    // The test program's window, on the compositor's first screen. It logs
    // the pointer's events (`wev` does), read back for the click.
    let program_log = base.join("program.log");
    // (Line-buffered, as a log read while the program runs needs: stdbuf
    // execs the program, so the pid is the window's.)
    let child = Command::new("stdbuf")
        .arg("-oL")
        .arg(&client)
        .env("WAYLAND_DISPLAY", &nested)
        .env_remove("DISPLAY")
        .env_remove("SWAYSOCK")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::fs::File::create(&program_log).unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("could not start {client}: {e}"));
    let pid = child.id();
    run.child = Some(child);
    let (before, home_screen) = wait_for(30, "the test program's window", || window_of(sway, pid));
    let (_, screens_before) = snapshot(sway);
    println!(
        "before: window {} {}x{} on {home_screen}, workspace {:?}, screens {:?}",
        before.address,
        before.size[0],
        before.size[1],
        before.workspace.name,
        screens_before.iter().map(|m| &m.name).collect::<Vec<_>>()
    );

    // Who may not ask, is not let: a phone, a computer paired one way, a
    // computer paired both ways while the switch is off.
    for device in ["lend-test-phone", "lend-test-one-way", "lend-test-stranger"] {
        for action in ["lend", "list"] {
            let said = ask(
                &runtime,
                device,
                json!({ "action": action, "id": 1, "window": before.address }),
            );
            assert!(
                said["error"].as_str().unwrap().contains("paired both ways"),
                "{device} {action}: {said}"
            );
            assert!(said.get("windows").is_none(), "no list for {device}");
            assert_eq!(said["id"], 1, "the ask's id is said back");
        }
    }

    // The window list names the test program's window, as it is now.
    let listed = ask(&runtime, BORROWER, json!({ "action": "list", "id": 20 }));
    assert!(listed.get("error").is_none(), "{listed}");
    assert_eq!(listed["id"], 20);
    let mine = listed["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["address"] == before.address.as_str())
        .unwrap_or_else(|| panic!("the window is not listed: {} windows", listed["windows"]))
        .clone();
    assert_eq!(mine["width"], before.size[0]);
    assert_eq!(mine["height"], before.size[1]);
    assert_eq!(mine["workspace"], before.workspace.name.as_str());
    assert_eq!(mine["lent"], false);
    assert!(
        !mine["title"].as_str().unwrap().is_empty() || !mine["class"].as_str().unwrap().is_empty(),
        "it has a title or an app"
    );
    println!(
        "list: {} window(s); ours {}x{} on workspace {:?}",
        listed["windows"].as_array().unwrap().len(),
        mine["width"],
        mine["height"],
        mine["workspace"]
    );
    std::fs::write(config.join("config.toml"), "lend_windows = false\n").unwrap();
    // The config is read again when its mtime changes.
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(config.join("config.toml"), "lend_windows = false\n# off\n").unwrap();
    let said = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 2, "window": before.address }),
    );
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("Let my computers borrow my windows"),
        "{said}"
    );
    let said = ask(&runtime, BORROWER, json!({ "action": "list", "id": 21 }));
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("Let my computers borrow my windows"),
        "the switch off also refuses the list: {said}"
    );
    assert_eq!(
        window_of(sway, pid).unwrap().1,
        home_screen,
        "refused: the window stayed"
    );
    std::thread::sleep(Duration::from_millis(1100));
    std::fs::write(config.join("config.toml"), "lend_windows = true\n").unwrap();

    // Lent.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 3, "window": before.address }),
    );
    println!("lend: {lent}");
    assert!(lent.get("error").is_none(), "{lent}");
    assert_eq!(lent["id"], 3);
    assert_eq!(lent["window"], before.address.as_str());
    let name = lent["name"].as_str().unwrap().to_string();
    let (width, height) = (
        lent["width"].as_u64().unwrap() as i32,
        lent["height"].as_u64().unwrap() as i32,
    );
    assert_eq!(
        (width, height),
        (before.size[0] / 2 * 2, before.size[1] / 2 * 2),
        "a screen of the window's size"
    );
    let (windows, screens) = snapshot(sway);
    let screen = screens
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no screen {name}: {screens:?}"));
    assert_eq!((screen.width, screen.height), (width, height));
    let (on_it, on_name) = window_of(sway, pid).unwrap();
    assert_eq!(on_name, name, "the window is on the new screen");
    assert_eq!(
        (on_it.size[0], on_it.size[1]),
        (width, height),
        "at the screen's size"
    );
    assert_eq!(
        on_it.at,
        [screen.x, screen.y],
        "at the screen's corner, filling it"
    );
    let on_screen: Vec<_> = windows
        .iter()
        .filter(|w| {
            screens
                .get(w.monitor as usize)
                .is_some_and(|m| m.name == name)
        })
        .collect();
    assert_eq!(on_screen.len(), 1, "alone on the new screen");
    println!(
        "lent: {name} {width}x{height}, window {}x{} at {:?}, alone",
        on_it.size[0], on_it.size[1], on_it.at
    );
    // The index a session captures it by is the screen's place in the
    // monitor list the capture reads.
    // (As the capture picks one: the listed monitor at that origin, else,
    // where the list is shorter than the compositor's outputs, the output's
    // place among them.)
    let index = lent["index"].as_u64().unwrap() as usize;
    let origins = pocketdesk_server::plugins::monitors::origins();
    let expected = origins
        .iter()
        .position(|&o| o == (screen.x, screen.y))
        .or_else(|| {
            screens
                .iter()
                .position(|m| m.name == name)
                .filter(|place| *place >= origins.len())
        });
    assert_eq!(Some(index), expected, "{origins:?}");
    println!("index: {index} (monitor list {origins:?})");

    // One frame of that screen is the window's size.
    let frame = base.join("frame.png");
    let grim = Command::new("grim")
        .args(["-o", &name])
        .arg(&frame)
        .env("WAYLAND_DISPLAY", &nested)
        .status()
        .expect("grim");
    assert!(grim.success(), "grim could not take the screen");
    assert_eq!(
        png_size(&frame),
        (width as u32, height as u32),
        "the frame is the window's size"
    );
    println!("frame: {}x{}", width, height);

    // The window list now marks it lent.
    let listed = ask(&runtime, BORROWER, json!({ "action": "list", "id": 22 }));
    let marked = listed["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["address"] == before.address.as_str())
        .map(|w| w["lent"].clone());
    assert_eq!(marked, Some(json!(true)), "{listed}");

    // One absolute pointer move and click aimed at the lent screen land on
    // the window: the virtual pointer spans the layout (the box round every
    // screen), so the screen's centre and a point near its corner are those
    // places' share of the box.
    let (_, all_screens) = snapshot(sway);
    let (left, top) = (
        all_screens.iter().map(|m| m.x).min().unwrap(),
        all_screens.iter().map(|m| m.y).min().unwrap(),
    );
    let (right, bottom) = (
        all_screens.iter().map(|m| m.x + m.width).max().unwrap(),
        all_screens.iter().map(|m| m.y + m.height).max().unwrap(),
    );
    let norm = |at: i32, from: i32, span: i32| -> u16 {
        // The pointer's extent is 65536; the middle of the pixel.
        let v = (i64::from(at - from) * 2 + 1) * 65_536 / (i64::from(span) * 2);
        u16::try_from(v.clamp(0, 65_535)).unwrap()
    };
    let mut pointer = WlrInput::connect().expect("the compositor's virtual pointer");
    let aims = [
        ("middle", width / 2, height / 2),
        ("near the corner", 20, 15),
    ];
    for (n, (name, dx, dy)) in aims.iter().enumerate() {
        pointer
            .motion_absolute(
                norm(screen.x + dx, left, right - left),
                norm(screen.y + dy, top, bottom - top),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        pointer.button(0x110, true).unwrap();
        pointer.button(0x110, false).unwrap();
        let clicked = Instant::now();
        let landed = loop {
            let all = presses(&program_log);
            if all.len() > n {
                break all[n];
            }
            let log = std::fs::read_to_string(&program_log).unwrap_or_default();
            assert!(
                clicked.elapsed() < Duration::from_secs(10),
                "no click reached the window ({name}); its log ends: {:?}",
                log.lines().rev().take(12).collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        println!(
            "click {name}: aimed at ({dx}, {dy}) of the screen, landed at {landed:?} on the window"
        );
        assert!(
            (landed.0 - f64::from(*dx)).abs() <= 2.5 && (landed.1 - f64::from(*dy)).abs() <= 2.5,
            "{name}: aimed ({dx}, {dy}), landed {landed:?}"
        );
    }
    drop(pointer);

    // Asked again, the same screen, nothing moved twice.
    let again = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 4, "window": before.address }),
    );
    assert_eq!(again["name"], name.as_str(), "{again}");
    // Not an id of a window.
    let odd = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 5, "window": "1; exec true" }),
    );
    assert_eq!(odd["error"], "not a window");

    // Returned: back on its screen and workspace, the new screen gone. (Sway's
    // tree shows the last command a moment late, so the size is waited for.)
    let returned = ask(
        &runtime,
        BORROWER,
        json!({ "action": "return", "id": 6, "window": before.address }),
    );
    println!("return: {returned}");
    assert_eq!(returned["returned"], 1, "{returned}");
    assert_eq!(returned["id"], 6);
    let (back, back_screen) = wait_for(10, "the window back", || {
        window_of(sway, pid).filter(|(w, s)| *s == home_screen && w.size == before.size)
    });
    assert_eq!(back_screen, home_screen);
    assert_eq!(back.workspace.name, before.workspace.name, "its workspace");
    assert_eq!(back.size, before.size, "its size");
    let (_, screens_after) = snapshot(sway);
    assert_eq!(
        screens_after.iter().map(|m| &m.name).collect::<Vec<_>>(),
        screens_before.iter().map(|m| &m.name).collect::<Vec<_>>(),
        "the new screen is gone"
    );
    assert!(
        !state.join(lend_window::STATE_FILE).exists(),
        "nothing left to put back"
    );
    println!(
        "returned: window {}x{} on {back_screen}, workspace {:?}, screens as before",
        back.size[0], back.size[1], back.workspace.name
    );

    // The stop path puts a lent window back too.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 7, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    assert!(
        state.join(lend_window::STATE_FILE).exists(),
        "what is lent is written down"
    );
    runtime.block_on(lend_window::return_all_on_stop());
    let (back, back_screen) = wait_for(10, "the window back after a stop", || {
        window_of(sway, pid).filter(|(w, s)| *s == home_screen && w.size == before.size)
    });
    assert_eq!(back.size, before.size);
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    assert!(!state.join(lend_window::STATE_FILE).exists());
    println!("stop: window back on {back_screen}, screen gone");

    // The start-up sweep of a server that stopped puts a lent window back.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 8, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    virtual_screen::clear_left_over();
    let (back, back_screen) = wait_for(10, "the window back after the sweep", || {
        window_of(sway, pid).filter(|(w, s)| *s == home_screen && w.size == before.size)
    });
    assert_eq!(back.size, before.size);
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    println!("sweep: window back on {back_screen}, screen gone");

    // A floating window comes back floating, where it was and its size.
    arrange_floating(
        sway,
        sway_socket.as_deref(),
        &before.address,
        [100, 80, 700, 500],
    );
    // (A size set on Sway counts its title bar and border: the window's is
    // a little less.)
    let (floating, _) = wait_for(10, "the window floating, small", || {
        window_of(sway, pid).filter(|(w, _)| w.size[0] <= 700 && w.size[1] <= 500)
    });
    println!("floating before: {:?} at {:?}", floating.size, floating.at);
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 9, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let even = [floating.size[0] / 2 * 2, floating.size[1] / 2 * 2];
    assert_eq!(
        (lent["width"].as_u64(), lent["height"].as_u64()),
        (Some(even[0] as u64), Some(even[1] as u64))
    );
    let (on_it, on_name) = window_of(sway, pid).unwrap();
    assert_eq!(on_name, lent["name"].as_str().unwrap());
    assert_eq!(on_it.size, even);
    let returned = ask(
        &runtime,
        BORROWER,
        json!({ "action": "return", "id": 10, "window": before.address }),
    );
    assert_eq!(returned["returned"], 1, "{returned}");
    let (back, back_screen) = wait_for(10, "the floating window back as it was", || {
        window_of(sway, pid)
            .filter(|(w, s)| *s == home_screen && w.size == floating.size && w.at == floating.at)
    });
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    println!(
        "floating returned: window {:?} at {:?} on {back_screen}",
        back.size, back.at
    );

    // The switch turned off with a window lent: it goes back, its screen goes.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 11, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let there = [BORROWER.to_string()];
    let mut looks = lend_window::Looks::default();
    assert_eq!(
        lend_window::sweep_once(true, &there, &mut looks),
        0,
        "switch on, borrower there: nothing to do"
    );
    assert_eq!(
        lend_window::sweep_once(false, &there, &mut looks),
        1,
        "switch off: the window goes back"
    );
    let (_, off_screen) = wait_for(10, "the window back with the switch off", || {
        window_of(sway, pid).filter(|(w, s)| *s == home_screen && w.size == floating.size)
    });
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    assert!(!state.join(lend_window::STATE_FILE).exists());
    println!("switch off: window back on {off_screen}, screen gone");

    // The borrowing computer gone (no session, connection or link): the
    // watcher, as the server runs it, has the window back within seconds.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 12, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let started = Instant::now();
    let watcher_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let watcher = watcher_runtime.spawn(lend_window::run_watcher());
    let (_, away_screen) = wait_for(15, "the window back with its borrower away", || {
        window_of(sway, pid).filter(|(w, s)| *s == home_screen && w.size == floating.size)
    });
    let took = started.elapsed();
    watcher.abort();
    drop(watcher_runtime);
    assert!(
        took < Duration::from_secs(8),
        "back in {took:?}, not a sweep's minute"
    );
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    assert!(!state.join(lend_window::STATE_FILE).exists());
    println!("borrower away: window back on {away_screen} after {took:?}, screen gone");

    // A lent window that is closed: its empty screen goes (after two looks),
    // and nothing is put back.
    let lent = ask(
        &runtime,
        BORROWER,
        json!({ "action": "lend", "id": 13, "window": before.address }),
    );
    assert!(lent.get("error").is_none(), "{lent}");
    let lent_name = lent["name"].as_str().unwrap().to_string();
    if let Some(mut child) = run.child.take() {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    wait_for(15, "the lent window to close", || {
        window_of(sway, pid).is_none().then_some(())
    });
    assert_eq!(
        lend_window::sweep_once(true, &there, &mut looks),
        0,
        "one look only notes it"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        lend_window::sweep_once(true, &there, &mut looks),
        1,
        "the second look removes its screen"
    );
    wait_for(10, "the closed window's screen to go", || {
        (!snapshot(sway).1.iter().any(|m| m.name == lent_name)).then_some(())
    });
    assert_eq!(snapshot(sway).1.len(), screens_before.len(), "screen gone");
    assert!(!state.join(lend_window::STATE_FILE).exists());
    println!("closed: the empty screen {lent_name} is gone");

    drop(run);
}

/// Make the window floating at (`x`, `y`) of size `w`×`h` (`[x, y, w, h]`),
/// as a person would with the compositor's own commands.
fn arrange_floating(sway: bool, socket: Option<&str>, id: &str, [x, y, w, h]: [i32; 4]) {
    if sway {
        let socket = socket.unwrap();
        for command in [
            format!("[con_id={id}] floating enable"),
            format!("[con_id={id}] resize set width {w} px height {h} px"),
            format!("[con_id={id}] move absolute position {x} {y}"),
        ] {
            let status = Command::new("swaymsg")
                .args(["-s", socket, &command])
                .stdout(Stdio::null())
                .status()
                .expect("swaymsg");
            assert!(status.success(), "{command}");
            // A size asked for in the same breath as the floating is lost.
            std::thread::sleep(Duration::from_millis(500));
        }
    } else {
        let ipc = HyprlandIpc::from_env().expect("Hyprland's sockets");
        for lua in [
            format!(
                "hl.dispatch(hl.dsp.window.float({{ action = \"enable\", window = \"address:{id}\" }}))"
            ),
            format!(
                "hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, exact = true, window = \"address:{id}\" }}))"
            ),
            format!(
                "hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, exact = true, window = \"address:{id}\" }}))"
            ),
        ] {
            let said = ipc.request_blocking(&format!("eval {lua}")).unwrap();
            assert_eq!(said.trim(), "ok", "{lua}: {said}");
        }
    }
}
