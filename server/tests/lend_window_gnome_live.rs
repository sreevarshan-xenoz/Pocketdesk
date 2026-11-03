//! Phase 6 of the window handoff on GNOME: lending one of a GNOME computer's
//! windows, live, against a throwaway headless GNOME Shell. In this process,
//! as the server's own code runs it: a stand-in for the owner's other
//! computer (paired both ways, in a throwaway state folder) asks
//! `kdeconnect.pocketdesk.virtual_screen`'s `list`, `lend` and `return`.
//!
//! Two kinds of Shell are tried, by `PD_GNOME_RIG_SHELL`:
//!
//! - `stock`: an ordinary GNOME Shell. It refuses a program its window list
//!   (`org.gnome.Shell.Introspect.GetWindows`: access denied), so `list` is
//!   refused in the owner's words and no window can be lent; so is a `lend`
//!   of any id. That is what a GNOME owner meets, and nothing is got round.
//! - `open`: a Shell that gives the list (started with `--unsafe-mode`, a
//!   stand-in for a Shell extension that answers the same call). `list` names
//!   the test program's window; `lend` registers it under an index past every
//!   monitor's; mutter's `RecordWindow` stream of it arrives as frames of the
//!   window alone (not the screen); the lease says it takes no input; a
//!   `return` ends the capture; a window closed ends the lend by the server's
//!   own watcher rule.
//!
//! The Shell is never this computer's own desktop: the test refuses to run
//! without `PD_GNOME_RIG_BUS` (the throwaway session bus's address), refuses
//! where that is this process's own session bus, and gives this process only
//! that bus (and `PD_GNOME_RIG_RUNTIME`, the folder of its PipeWire socket).
//!
//! Ignored: it records a window of a compositor. Run it with
//! `PD_GNOME_RIG_BUS=<address> PD_GNOME_RIG_RUNTIME=<folder>
//! PD_GNOME_RIG_SHELL=stock|open cargo test -p pocketdesk-server --test
//! lend_window_gnome_live -- --ignored --nocapture`; for `open`, also
//! `PD_GNOME_RIG_CLOSE_CMD` (a shell command that closes the test program's
//! window) to try the closed window.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::streaming::{
    StreamingConfig, VideoFrame, capture_mutter, gnome_unattended, latest_frame, window_target,
};
use pocketdesk_server::{lend_window, virtual_screen};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// A computer paired both ways.
const BORROWER: &str = "lend-test-borrower";

/// Removes the throwaway folders.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn ask(device: &str, body: Value) -> Value {
    virtual_screen::respond(device, &body).await
}

/// Frames for `for_`: how many, and the newest.
fn count(
    rx: &mut latest_frame::FrameReceiver<VideoFrame>,
    for_: Duration,
) -> (u32, Option<VideoFrame>) {
    let until = Instant::now() + for_;
    let (mut frames, mut newest) = (0u32, None);
    while Instant::now() < until {
        if let Some(frame) = rx.try_recv() {
            frames += 1;
            newest = Some(frame);
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    (frames, newest)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "records a window of a throwaway headless GNOME Shell"]
async fn a_gnome_window_is_lent_to_look_at_when_the_shell_gives_its_list() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let bus = std::env::var("PD_GNOME_RIG_BUS")
        .expect("PD_GNOME_RIG_BUS: the throwaway session bus's address (never this desktop's)");
    let runtime_dir = std::env::var("PD_GNOME_RIG_RUNTIME")
        .expect("PD_GNOME_RIG_RUNTIME: the folder of the throwaway session's PipeWire socket");
    let shell = std::env::var("PD_GNOME_RIG_SHELL").expect("PD_GNOME_RIG_SHELL: stock or open");
    assert!(matches!(shell.as_str(), "stock" | "open"), "stock or open");
    assert_ne!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
        Some(bus.as_str()),
        "PD_GNOME_RIG_BUS is this process's own session bus"
    );
    assert!(
        !bus.contains("/run/user/1000/bus") || bus.contains("/proc/"),
        "PD_GNOME_RIG_BUS looks like this computer's own session bus"
    );

    let base = std::env::var_os("PD_LEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("pd-lend-gnome-live-{}", std::process::id()));
    let (state, config) = (base.join("state"), base.join("config"));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let _cleanup = Cleanup(base.clone());

    // This process sees only the throwaway Shell's bus and the throwaway
    // folders. SAFETY: the only test of this binary, and nothing else runs
    // yet.
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus);
        std::env::set_var("PIPEWIRE_RUNTIME_DIR", &runtime_dir);
        std::env::set_var("XDG_CURRENT_DESKTOP", "GNOME");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
        std::env::remove_var("SWAYSOCK");
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::set_var("POCKETDESK_STATE_DIR", &state);
        std::env::set_var("POCKETDESK_CONFIG_DIR", &config);
    }
    // "Unattended on GNOME" is off until the owner turns it on.
    std::fs::write(
        config.join("config.toml"),
        "lend_windows = true\ngnome_unattended = false\n",
    )
    .unwrap();
    TrustStore::update(state.join(TRUST_FILE), |s| {
        s.trust_computer(BORROWER)?;
        Ok(())
    })
    .unwrap();
    computers::update(&state, |file| {
        file.computers.push(PeerComputer {
            device_id: BORROWER.to_string(),
            name: format!("Computer {BORROWER}"),
            addresses: vec!["192.0.2.10".to_string()],
            control_port: 1716,
            stream_port: 4716,
            fingerprint: "ab".repeat(32),
            endpoint: None,
            paired_unix: 1,
            link: String::new(),
            link_unix: 0,
            lets_drive: false,
            mine: true,
            wake: None,
            viewed_unix: 0,
            keys_by_position: None,
        });
    })
    .unwrap();

    // With the "Unattended on GNOME" switch off, nothing is lent or listed:
    // GNOME's own screen cast is what shows a window with no dialog, which
    // the owner has not allowed.
    let said = ask(BORROWER, json!({ "action": "list", "id": 1 })).await;
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("Unattended on GNOME"),
        "{said}"
    );
    // The config is read again when its mtime changes.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    std::fs::write(
        config.join("config.toml"),
        "lend_windows = true\ngnome_unattended = true\n",
    )
    .unwrap();
    gnome_unattended::set_enabled(true);

    if shell == "stock" {
        // An ordinary Shell refuses the list, and says so in the owner's words.
        let said = ask(BORROWER, json!({ "action": "list", "id": 2 })).await;
        println!("stock list: {said}");
        let words = said["error"].as_str().expect("an error, not a list");
        assert!(words.contains("Shell extension"), "{said}");
        assert!(said.get("windows").is_none());
        // A window lent by an id (a real one, were it known) is refused too:
        // the server lends only what it can see listed.
        let said = ask(
            BORROWER,
            json!({ "action": "lend", "id": 3, "window": "2609479999" }),
        )
        .await;
        println!("stock lend: {said}");
        assert!(said.get("error").is_some(), "{said}");
        assert!(said.get("index").is_none(), "{said}");
        return;
    }

    // ---- open: the Shell gives the list ----
    let listed = ask(BORROWER, json!({ "action": "list", "id": 10 })).await;
    println!("open list: {} window(s)", listed["windows"]);
    assert!(listed.get("error").is_none(), "{listed}");
    let mine = listed["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["class"] == "wev")
        .unwrap_or_else(|| panic!("the test program's window is not listed: {listed}"))
        .clone();
    let id = mine["address"].as_str().unwrap().to_string();
    assert_eq!(mine["lent"], false);
    let (listed_w, listed_h) = (
        mine["width"].as_u64().unwrap(),
        mine["height"].as_u64().unwrap(),
    );
    println!("the window: {id} {listed_w}x{listed_h}");

    // Lent: an index past every monitor's, for this device alone.
    let lent = ask(
        BORROWER,
        json!({ "action": "lend", "id": 11, "window": id }),
    )
    .await;
    println!("lend: {lent}");
    assert!(lent.get("error").is_none(), "{lent}");
    assert_eq!(lent["window"], id.as_str());
    let index = lent["index"].as_u64().unwrap() as u32;
    assert!(window_target::is_window_index(index));
    assert_eq!(lent["width"], listed_w);
    assert_eq!(lent["height"], listed_h);
    let lease = window_target::lease_for(index, Some(BORROWER)).expect("lent to the borrower");
    assert_eq!(lease.native, format!("gnome:{id}"));
    assert!(
        !window_target::takes_input(&lease),
        "a lent GNOME window is view-only"
    );
    assert!(
        window_target::lease_for(index, Some("lend-test-stranger")).is_err(),
        "no other device may dial it"
    );
    // The list now marks it lent.
    let after = ask(BORROWER, json!({ "action": "list", "id": 12 })).await;
    assert_eq!(
        after["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["address"] == id.as_str())
            .unwrap()["lent"],
        true
    );

    // The picture is the window alone: frames of its size, not the screen's.
    let (tx, mut rx) = latest_frame::channel();
    let cancel = CancellationToken::new();
    let started = Instant::now();
    let _session = capture_mutter::start_window_capture(
        StreamingConfig::default(),
        tx,
        cancel.clone(),
        lease.clone(),
    )
    .await
    .expect("the window's stream starts");
    println!("START_SECS {:.1}", started.elapsed().as_secs_f32());
    let (frames, newest) = count(&mut rx, Duration::from_secs(4));
    let frame = newest.expect("a frame of the window");
    println!("FRAMES {frames} SIZE {}x{}", frame.width, frame.height);
    assert!(frames > 0);
    // Where the picture differs from its own corner: a window drawn inside a
    // larger frame shows as a box.
    let at = |x: usize, y: usize| {
        let i = y * frame.stride as usize + x * 4;
        [
            frame.data[i],
            frame.data[i + 1],
            frame.data[i + 2],
            frame.data[i + 3],
        ]
    };
    let corner = at(0, 0);
    let (mut min, mut max) = ((usize::MAX, usize::MAX), (0usize, 0usize));
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            if at(x, y) != corner {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
        }
    }
    println!("CORNER {corner:?} CONTENT {min:?}..{max:?}");
    // The stream is as big as a screen with the window in a corner of it; the
    // frame is cut to the window (even sides, as the encoder wants).
    assert_eq!(
        (u64::from(frame.width), u64::from(frame.height)),
        (listed_w & !1, listed_h & !1),
        "the window alone, not the screen"
    );
    if let Ok(path) = std::env::var("PD_GNOME_RIG_SNAP") {
        let mut out = format!(
            "P6
{} {}
255
",
            frame.width, frame.height
        )
        .into_bytes();
        for y in 0..frame.height as usize {
            for x in 0..frame.width as usize {
                let p = at(x, y);
                out.extend_from_slice(&[p[2], p[1], p[0]]);
            }
        }
        std::fs::write(path, out).unwrap();
    }
    assert!(
        !cancel.is_cancelled(),
        "the capture runs while the window is lent"
    );

    // Given back: the capture ends at once (the lease does not stand).
    let back = ask(
        BORROWER,
        json!({ "action": "return", "id": 13, "window": id }),
    )
    .await;
    assert_eq!(back["returned"], 1, "{back}");
    assert!(window_target::lease_for(index, Some(BORROWER)).is_err());
    let ended = Instant::now();
    while !cancel.is_cancelled() && ended.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(cancel.is_cancelled(), "the capture ended with the lend");
    println!(
        "capture ended {:.1}s after the return",
        ended.elapsed().as_secs_f32()
    );

    // A window closed ends its lend by the server's own rule: two looks.
    if let Ok(close) = std::env::var("PD_GNOME_RIG_CLOSE_CMD") {
        let lent = ask(
            BORROWER,
            json!({ "action": "lend", "id": 14, "window": id }),
        )
        .await;
        assert!(lent.get("error").is_none(), "{lent}");
        let index = lent["index"].as_u64().unwrap() as u32;
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&close)
            .status()
            .expect("the close command runs");
        assert!(status.success(), "the close command failed");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let live = vec![BORROWER.to_string()];
        let mut looks = lend_window::Looks::default();
        let ended = tokio::task::spawn_blocking(move || {
            let first = lend_window::sweep_once(true, &live, &mut looks);
            let second = lend_window::sweep_once(true, &live, &mut looks);
            (first, second)
        })
        .await
        .unwrap();
        println!("closed window: sweeps ended {ended:?}");
        assert_eq!(ended, (0, 1), "one look is not enough, two end it");
        assert!(window_target::lease_for(index, Some(BORROWER)).is_err());
    }
}
