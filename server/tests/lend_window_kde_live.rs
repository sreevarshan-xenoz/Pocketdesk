//! Phase 6 follow-up of the window handoff on KDE Plasma: lending one of a KDE
//! computer's windows, live, against a throwaway headless KWin
//! (`kwin_wayland --virtual`). In this process, as the server's own code runs
//! it: a stand-in for the owner's other computer (paired both ways, in a
//! throwaway state folder) asks `kdeconnect.pocketdesk.virtual_screen`'s
//! `list`, `lend` and `return`.
//!
//! **Written, not yet run.** The unit tests cover the table, the rules and the
//! flow over fakes; this is the first thing that reaches a real KWin, its
//! scripting interface, its global-shortcut service and the screen-sharing
//! portal, and nothing in it has been run (docs/development-setup.md section
//! 3a-2b says how; checklist section 284 lists what it should settle).
//!
//! Two modes, by `PD_KDE_RIG_MODE`:
//!
//! - `no-portal`: the rig's xdg-desktop-portal is stopped. `list` and `lend`
//!   are refused in the owner's words (the portal is not there), and nothing
//!   is registered.
//! - `full`: the portal and the rig's one test window (`PD_KDE_RIG_CLASS`,
//!   `wev` by default) are there. `list` loads the script and names the test
//!   window; `lend` registers `kde:<n>` under an index past every monitor's;
//!   the lease takes no input; a stranger device is refused; the table the
//!   script reports has the window with a size and a place, ordinary, shown and
//!   not Pocketdesk's own, and is kept current (a second report follows each
//!   look); `KdeOs`, the guard's Os over the table, answers a frame for the
//!   window; `return` ends the lease and unloads the script; a window closed
//!   ends its lend by the server's own rule (two looks), if
//!   `PD_KDE_RIG_CLOSE_CMD` closes it. With `PD_KDE_RIG_CAPTURE=1` the portal
//!   capture is tried too (`PD_KDE_RIG_CAPTURE_WAIT` seconds to answer the
//!   dialog by hand, 90 by default): it needs a person or a restore token
//!   already kept for the window in `PD_KDE_RIG_GRANTS`.
//!
//! The KWin is never this computer's own desktop: the test refuses to run
//! without `PD_KDE_RIG_BUS` (the throwaway session bus's address), refuses
//! where that is this process's own session bus, and gives this process only
//! that bus. `PD_KDE_RIG_RUN` is a folder that the container sees at the same
//! path as this process does (bound into it): it is this process's
//! `XDG_RUNTIME_DIR`, where the script is written for KWin to read.
//!
//! Ignored: it loads a script into a compositor and may record a window. Run
//! it with `PD_KDE_RIG_BUS=<address> PD_KDE_RIG_RUN=<folder>
//! PD_KDE_RIG_MODE=no-portal|full cargo test -p pocketdesk-server --test
//! lend_window_kde_live -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pocketdesk_core::local::computers::{self, PeerComputer};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::streaming::window_guard::Os;
use pocketdesk_core::streaming::{
    StreamingConfig, VideoFrame, capture_kde, latest_frame, window_kde, window_target,
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
#[ignore = "loads a script into a throwaway headless KWin and may record a window"]
async fn a_kde_window_is_lent_to_look_at_through_a_kwin_script_and_the_portal() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let bus = std::env::var("PD_KDE_RIG_BUS")
        .expect("PD_KDE_RIG_BUS: the throwaway session bus's address (never this desktop's)");
    let run = std::env::var("PD_KDE_RIG_RUN").expect(
        "PD_KDE_RIG_RUN: a folder the container sees at this same path (this process's XDG_RUNTIME_DIR)",
    );
    let mode = std::env::var("PD_KDE_RIG_MODE").expect("PD_KDE_RIG_MODE: no-portal or full");
    assert!(
        matches!(mode.as_str(), "no-portal" | "full"),
        "no-portal or full"
    );
    let class = std::env::var("PD_KDE_RIG_CLASS").unwrap_or_else(|_| "wev".to_string());
    assert_ne!(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
        Some(bus.as_str()),
        "PD_KDE_RIG_BUS is this process's own session bus"
    );
    assert!(
        !bus.contains("/run/user/1000/bus") || bus.contains("/proc/"),
        "PD_KDE_RIG_BUS looks like this computer's own session bus"
    );
    assert!(
        PathBuf::from(&run).is_absolute() && PathBuf::from(&run).is_dir(),
        "PD_KDE_RIG_RUN is not an existing absolute folder"
    );

    let base = std::env::var_os("PD_LEND_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("pd-lend-kde-live-{}", std::process::id()));
    let (state, config) = (base.join("state"), base.join("config"));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let _cleanup = Cleanup(base.clone());

    // This process sees only the throwaway KWin's bus and the throwaway
    // folders. SAFETY: the only test of this binary, and nothing else runs
    // yet.
    unsafe {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus);
        std::env::set_var("XDG_RUNTIME_DIR", &run);
        std::env::set_var("XDG_CURRENT_DESKTOP", "KDE");
        // Only said to be a Wayland session: nothing here draws on it.
        std::env::set_var("WAYLAND_DISPLAY", "pd-rig");
        std::env::remove_var("DISPLAY");
        std::env::remove_var("SWAYSOCK");
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::set_var("POCKETDESK_STATE_DIR", &state);
        std::env::set_var("POCKETDESK_CONFIG_DIR", &config);
    }
    std::fs::write(config.join("config.toml"), "lend_windows = true\n").unwrap();
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

    if mode == "no-portal" {
        // The portal is stopped: nothing is listed or lent, and it says why.
        let said = ask(BORROWER, json!({ "action": "list", "id": 1 })).await;
        println!("no-portal list: {said}");
        let words = said["error"].as_str().expect("an error, not a list");
        assert!(words.contains("screen-sharing portal"), "{said}");
        assert!(said.get("windows").is_none());
        let said = ask(
            BORROWER,
            json!({ "action": "lend", "id": 2, "window": "1" }),
        )
        .await;
        println!("no-portal lend: {said}");
        assert!(said.get("error").is_some(), "{said}");
        assert!(said.get("index").is_none(), "{said}");
        return;
    }

    // ---- full: the portal and the test window are there ----
    let started = Instant::now();
    let listed = ask(BORROWER, json!({ "action": "list", "id": 10 })).await;
    println!(
        "LIST_SECS {:.1}: {} window(s)",
        started.elapsed().as_secs_f32(),
        listed["windows"]
    );
    assert!(
        listed.get("error").is_none(),
        "the script did not load or the table did not come: {listed}"
    );
    let mine = listed["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["class"] == class.as_str())
        .unwrap_or_else(|| panic!("the test program's window is not listed: {listed}"))
        .clone();
    let id = mine["address"].as_str().unwrap().to_string();
    assert_eq!(mine["lent"], false);
    let (listed_w, listed_h) = (
        mine["width"].as_u64().unwrap(),
        mine["height"].as_u64().unwrap(),
    );
    println!("the window: {id} {listed_w}x{listed_h}");
    // Nothing that is not an ordinary window of a program is offered.
    for w in listed["windows"].as_array().unwrap() {
        let class = w["class"].as_str().unwrap_or("");
        assert!(
            !class.to_ascii_lowercase().contains("pocketdesk")
                && !class.eq_ignore_ascii_case("plasmashell"),
            "offered: {w}"
        );
    }

    // The table the script reports: the window, with its size and place.
    let table = window_kde::snapshot().expect("the script reported a table");
    assert!(table.is_fresh(Instant::now()), "the table is current");
    let seq: u64 = id.parse().expect("the window id is a number");
    let window = table.window(seq).expect("the window is in the table");
    println!(
        "TABLE {} window(s); the window frame {:?}, shown {}, normal {}, pid {}, active {:?}",
        table.windows.len(),
        window.frame,
        window.shown,
        window.normal,
        window.pid,
        table.active
    );
    assert!(window.normal && window.shown && !window.minimized);
    assert!(window.frame.2 > 0 && window.frame.3 > 0);
    assert_eq!(
        (u64::from(window.frame.2), u64::from(window.frame.3)),
        (listed_w, listed_h)
    );

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
    assert_eq!(lent["name"], format!("KDE-{index}").as_str());
    let lease = window_target::lease_for(index, Some(BORROWER)).expect("lent to the borrower");
    assert_eq!(lease.native, format!("kde:{id}"));
    assert!(
        !window_target::takes_input(&lease),
        "a lent KDE window is view-only"
    );
    assert!(
        window_target::lease_for(index, Some("lend-test-stranger")).is_err(),
        "no other device may dial it"
    );

    // The table is kept current while a window is lent: each look asks the
    // script to report (the global shortcut), so a second report follows.
    let mut updates = window_kde::updates();
    updates.mark_unchanged();
    let again = ask(BORROWER, json!({ "action": "list", "id": 12 })).await;
    assert!(again.get("error").is_none(), "{again}");
    let reported = tokio::time::timeout(Duration::from_secs(3), updates.changed())
        .await
        .is_ok();
    println!("POKE_REPORT {reported}");
    assert!(
        reported,
        "no report followed a look: the shortcut that makes the script report did nothing"
    );

    // The guard's Os over the table answers a frame for the window, and the
    // raise it asks for is taken by the script.
    let os = window_kde::KdeOs;
    let frame = os.frame(&lease).expect("a frame for the lent window");
    println!("KDEOS frame {frame:?} front {}", os.front_is_lent(&lease));
    assert_eq!(
        (u64::from(frame.2), u64::from(frame.3)),
        (listed_w, listed_h)
    );
    os.raise(&lease);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = window_kde::snapshot().expect("a table after the raise");
    println!(
        "RAISED active is the lent window: {}",
        after.active == Some(seq)
    );

    if std::env::var("PD_KDE_RIG_CAPTURE").as_deref() == Ok("1") {
        if let Ok(grants) = std::env::var("PD_KDE_RIG_GRANTS") {
            window_kde::set_grant_dir(Some(PathBuf::from(grants)));
        }
        let wait = std::env::var("PD_KDE_RIG_CAPTURE_WAIT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(90);
        println!("CAPTURE: answer the portal's dialog for the lent window within {wait} s");
        let (tx, mut rx) = latest_frame::channel();
        let cancel = CancellationToken::new();
        let started = Instant::now();
        let _session = tokio::time::timeout(
            Duration::from_secs(wait),
            capture_kde::start_window_capture(
                StreamingConfig::default(),
                tx,
                cancel.clone(),
                lease.clone(),
            ),
        )
        .await
        .expect("the dialog was not answered in time")
        .expect("the window's stream starts and matches the lent window");
        println!("START_SECS {:.1}", started.elapsed().as_secs_f32());
        let (frames, newest) = count(&mut rx, Duration::from_secs(4));
        let frame = newest.expect("a frame of the window");
        println!("FRAMES {frames} SIZE {}x{}", frame.width, frame.height);
        assert!(frames > 0);
        assert_eq!(
            (u64::from(frame.width), u64::from(frame.height)),
            (listed_w & !1, listed_h & !1),
            "the window alone, not the screen"
        );
        assert!(
            !cancel.is_cancelled(),
            "the capture runs while the window is lent"
        );
        cancel.cancel();
    }

    // Given back: the lease does not stand, and with nothing lent the script
    // is unloaded (its table is forgotten).
    let back = ask(
        BORROWER,
        json!({ "action": "return", "id": 13, "window": id }),
    )
    .await;
    assert_eq!(back["returned"], 1, "{back}");
    assert!(window_target::lease_for(index, Some(BORROWER)).is_err());
    let ended = Instant::now();
    while window_kde::snapshot().is_some() && ended.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        window_kde::snapshot().is_none(),
        "the script was not unloaded when nothing was lent"
    );
    println!(
        "script unloaded {:.1}s after the return",
        ended.elapsed().as_secs_f32()
    );

    // A window closed ends its lend by the server's own rule: two looks.
    if let Ok(close) = std::env::var("PD_KDE_RIG_CLOSE_CMD") {
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
