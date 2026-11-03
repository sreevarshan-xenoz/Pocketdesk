//! A computer's window the compositor does not show must not stop the app:
//! the regression test for the stall found in phase 1d of the window handoff
//! work (`docs/status-history.md`). A window on a Wayland compositor gets its
//! frame callbacks only while it is shown; a swap that waits for the
//! display's refresh (vsync) then waits for ever, and as every window of the
//! app is drawn by the one event-loop thread, every window stopped with it.
//!
//! The real computer window (its deferred viewport, its decoder, its NV12
//! shader) shows a throwaway server's test pattern beside the app's own
//! window, in a headless sway the test is started in, and the test moves the
//! windows about with `swaymsg`. Four spells of a few seconds each:
//!
//! 1. `side by side`: both windows shown (the control).
//! 2. `covered`: the computer window over the app's window (the covered one
//!    gets no frame callbacks).
//! 3. `hidden`: the computer window on a workspace that is not shown.
//! 4. `shown again`: the computer window back beside the app's window.
//!
//! For each spell it prints the passes per second of each window and the
//! process's processor time (percent of one core). The app's window asks
//! for a pass every 100 ms, so it must keep making about ten a second in
//! every spell, and in the last spell the computer window must be drawing
//! again.
//!
//! Ignored: it starts two real servers (`POCKETDESK_SERVER_EXE`), opens real
//! windows and moves them in a compositor of its own, never the owner's
//! session. Run it from a headless sway (`WLR_BACKENDS=headless`, its own
//! `XDG_RUNTIME_DIR` under `/tmp`, `SWAYSOCK` set, every window floating):
//! it refuses to run anywhere else. `PD_LIVE_VSYNC=1` or `0` makes the swap
//! wait for the display or not (the app's own choice for this session when
//! unset: it waits except on Wayland), and `PD_LIVE_SPELL` (5) is each
//! spell's seconds. Before the fix, `PD_LIVE_VSYNC=1` is the stall: the
//! `covered` and `hidden` spells make no pass at all.
//!
//! `an_animating_page_is_paced` is the other half: with a swap that does not
//! wait, a spinner asked for 14,500 frames a second and a whole core, until
//! [`crate::frame_pacing`]. winit makes one event loop a process, so run
//! each test on its own (`cargo test ... view_stall_live::a_window` then
//! `... view_stall_live::an_animating`).

use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::computer_view::{ComputerView, viewport_id};
use crate::view_live::{StopViewOnDrop, start_pair};

const SPELLS: [&str; 4] = ["side by side", "covered", "hidden", "shown again"];

/// The window's passes in a spell, per second, for the app's window and the
/// computer window.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rates {
    app: f64,
    view: f64,
    /// Percent of one core.
    cpu: f64,
}

/// The processor time this process has used, in seconds.
fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // After the command's closing bracket: state, then fields 4.. (utime is
    // the 14th field, stime the 15th).
    let after = stat.rsplit_once(')').map_or("", |(_, rest)| rest);
    let fields: Vec<&str> = after.split_whitespace().collect();
    let ticks = |i: usize| {
        fields
            .get(i)
            .and_then(|f| f.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (ticks(11) + ticks(12)) / 100.0
}

/// Whether `rates` show an app that kept drawing: the app's window made at
/// least a third of the ten passes a second it asks for.
fn kept_going(rates: &Rates) -> bool {
    rates.app >= 3.0
}

fn sway(args: &str) {
    let out = Command::new("swaymsg").arg(args).output();
    match out {
        Ok(out) if out.status.success() => {}
        Ok(out) => eprintln!(
            "swaymsg {args}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => eprintln!("swaymsg {args}: {e}"),
    }
}

static APP_PASSES: AtomicU64 = AtomicU64::new(0);
static VIEW_PASSES: AtomicU64 = AtomicU64::new(0);
static VIEW_FRAMES: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

struct Show {
    view: Option<ComputerView>,
    device: String,
    state: std::path::PathBuf,
    opened: bool,
}

impl eframe::App for Show {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        crate::frame_pacing::watch_refresh(frame);
        let ctx = ui.ctx().clone();
        let view = self
            .view
            .get_or_insert_with(|| ComputerView::new(&ctx, self.state.clone(), None));
        if !self.opened {
            crate::video::start_hevc_probe();
            view.set_hevc(false);
            view.set_hw_decode(true);
            view.open(
                &self.device,
                "Probe",
                0,
                pocketdesk_core::streaming::input_packet::PRESET_AUTO,
                true,
                false,
            )
            .unwrap();
            self.opened = true;
        }
        view.show(&ctx);
        APP_PASSES.store(
            ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
            Ordering::Relaxed,
        );
        VIEW_PASSES.store(
            ctx.cumulative_pass_nr_for(viewport_id(0)),
            Ordering::Relaxed,
        );
        if ctx.cumulative_pass_nr_for(viewport_id(0)) > 0 {
            VIEW_FRAMES.store(
                ctx.cumulative_frame_nr_for(viewport_id(0)),
                Ordering::Relaxed,
            );
        }
        if STOP.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Run the four spells against the windows, on its own thread beside the
/// event loop (which the stall stops), and give back each spell's rates.
fn spells(secs: u64) -> Vec<Rates> {
    // The app's window small and the computer window big, both centred;
    // then the computer window is moved about.
    let view = "[title=\"^Probe\"]";
    let app = "[title=\"^present probe\"]";
    let mut found = Vec::new();
    // Let the windows open and the stream start.
    std::thread::sleep(Duration::from_secs(8));
    for (n, name) in SPELLS.iter().enumerate() {
        match n {
            0 => {
                sway(&format!("{app} resize set 320 200"));
                sway(&format!("{app} move position 20 20"));
                sway(&format!("{view} resize set 800 600"));
                sway(&format!("{view} move position 400 20"));
            }
            1 => {
                sway(&format!("{app} move position 100 100"));
                sway(&format!("{view} move position 60 60"));
                sway(&format!("{view} resize set 800 600"));
            }
            2 => sway(&format!("{view} move container to workspace 2")),
            _ => {
                sway(&format!("{view} move container to workspace 1"));
                sway(&format!("{view} move position 400 20"));
            }
        }
        // Settle, then measure.
        std::thread::sleep(Duration::from_millis(1500));
        let (a0, v0, f0, c0, t0) = (
            APP_PASSES.load(Ordering::Relaxed),
            VIEW_PASSES.load(Ordering::Relaxed),
            VIEW_FRAMES.load(Ordering::Relaxed),
            cpu_seconds(),
            Instant::now(),
        );
        std::thread::sleep(Duration::from_secs(secs));
        let dt = t0.elapsed().as_secs_f64();
        let rates = Rates {
            app: (APP_PASSES.load(Ordering::Relaxed) - a0) as f64 / dt,
            view: (VIEW_PASSES.load(Ordering::Relaxed) - v0) as f64 / dt,
            cpu: (cpu_seconds() - c0) / dt * 100.0,
        };
        println!(
            "  {name:>13}: app {:5.1} passes/s, computer window {:5.1} passes/s ({:5.1} frames/s), cpu {:5.1}%",
            rates.app,
            rates.view,
            (VIEW_FRAMES.load(Ordering::Relaxed) - f0) as f64 / dt,
            rates.cpu
        );
        found.push(rates);
    }
    // Whatever happened, let the app end.
    sway(&format!("{view} move container to workspace 1"));
    STOP.store(true, Ordering::Relaxed);
    found
}

#[test]
#[ignore = "starts two real servers and opens windows: needs a headless sway of its own"]
fn a_window_the_compositor_does_not_show_does_not_stop_the_app() {
    use winit::platform::wayland::EventLoopBuilderExtWayland as _;
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    assert!(
        runtime.starts_with("/tmp/")
            && std::env::var_os("SWAYSOCK").is_some()
            && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none(),
        "run this in a headless sway of its own (XDG_RUNTIME_DIR under /tmp, SWAYSOCK set)"
    );
    let secs: u64 = std::env::var("PD_LIVE_SPELL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let vsync = match std::env::var("PD_LIVE_VSYNC").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => crate::frame_pacing::swap_waits_here(false),
    };
    let pair = start_pair(
        (19536, 19636),
        Some(pocketdesk_core::streaming::latency_probe::MODE),
        false,
        false,
    );
    let shown = Mutex::new(Some(Show {
        view: None,
        device: pair.b_id.clone(),
        state: pair.a_state.clone(),
        opened: false,
    }));
    // The spells run beside the event loop; if the stall never lets the app
    // end, the run is ended after its spells, and says so.
    let found: &'static Mutex<Vec<Rates>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    let ended: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    std::thread::spawn(move || {
        *found.lock().unwrap() = spells(secs);
        std::thread::sleep(Duration::from_secs(10));
        if !ended.load(Ordering::Relaxed) {
            eprintln!("the app did not end after its spells: it is still stalled");
            std::process::exit(2);
        }
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("present probe")
            .with_inner_size([320.0, 200.0]),
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync,
            ..Default::default()
        },
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    println!("vsync {}", if vsync { "on" } else { "off" });
    {
        let _stop = StopViewOnDrop(&pair.a_state);
        eframe::run_native(
            "present probe",
            options,
            Box::new(move |cc| {
                crate::nv12::set_glow(cc.gl.is_some());
                if !vsync {
                    crate::frame_pacing::install(&cc.egui_ctx);
                }
                Ok(Box::new(shown.lock().unwrap().take().unwrap()))
            }),
        )
        .unwrap();
    }
    ended.store(true, Ordering::Relaxed);
    drop(pair);
    let found = found.lock().unwrap().clone();
    assert_eq!(found.len(), SPELLS.len(), "every spell was measured");
    for (name, rates) in SPELLS.iter().zip(&found) {
        assert!(
            kept_going(rates),
            "{name}: the app kept drawing ({rates:?})"
        );
    }
    assert!(
        found[3].view >= 5.0,
        "the computer window draws again once shown ({:?})",
        found[3]
    );
}

/// An animating page: a spinner asks for a pass the moment the last is done.
struct Spin;

impl eframe::App for Spin {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        crate::frame_pacing::watch_refresh(frame);
        ui.spinner();
        APP_PASSES.store(
            ui.ctx().cumulative_pass_nr_for(egui::ViewportId::ROOT),
            Ordering::Relaxed,
        );
        if STOP.load(Ordering::Relaxed) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[test]
#[ignore = "opens a window: needs a headless sway of its own"]
fn an_animating_page_is_paced() {
    use winit::platform::wayland::EventLoopBuilderExtWayland as _;
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    assert!(
        runtime.starts_with("/tmp/") && std::env::var_os("SWAYSOCK").is_some(),
        "run this in a headless sway of its own"
    );
    let vsync = match std::env::var("PD_LIVE_VSYNC").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => crate::frame_pacing::swap_waits_here(false),
    };
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        let (a0, c0, t0) = (
            APP_PASSES.load(Ordering::Relaxed),
            cpu_seconds(),
            Instant::now(),
        );
        std::thread::sleep(Duration::from_secs(5));
        let dt = t0.elapsed().as_secs_f64();
        let rate = (APP_PASSES.load(Ordering::Relaxed) - a0) as f64 / dt;
        println!(
            "  spinner: {rate:.1} passes/s, cpu {:.1}%",
            (cpu_seconds() - c0) / dt * 100.0
        );
        *SPIN_RATE.lock().unwrap() = Some(rate);
        STOP.store(true, Ordering::Relaxed);
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("present probe")
            .with_inner_size([320.0, 200.0]),
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync,
            ..Default::default()
        },
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    println!("vsync {}", if vsync { "on" } else { "off" });
    eframe::run_native(
        "present probe",
        options,
        Box::new(move |cc| {
            if !vsync {
                crate::frame_pacing::install(&cc.egui_ctx);
            }
            Ok(Box::new(Spin))
        }),
    )
    .unwrap();
    // No more than twice a refresh, or a refresh when the swap waits for
    // it (with a margin for the compositor's rate), and not stopped.
    let rate = SPIN_RATE.lock().unwrap().expect("the spinner was measured");
    assert!(
        (20.0..=150.0).contains(&rate),
        "an animating page drew {rate:.0} passes a second"
    );
}

static SPIN_RATE: Mutex<Option<f64>> = Mutex::new(None);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_that_kept_drawing_made_most_of_its_passes() {
        let r = |app| Rates {
            app,
            view: 0.0,
            cpu: 0.0,
        };
        assert!(kept_going(&r(9.8)));
        assert!(kept_going(&r(3.0)));
        assert!(!kept_going(&r(0.0)), "a stall");
        assert!(!kept_going(&r(0.4)));
    }
}
