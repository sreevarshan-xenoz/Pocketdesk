//! Where the window's time goes after a picture is decoded, phase 1d of the
//! window handoff work: the real computer window ([`crate::computer_view`],
//! its deferred viewport, its decoder, its NV12 shader, vsync on) showing a
//! throwaway server's test pattern, with the window's stage trace
//! ([`crate::view_present_trace`]) joined to the host's
//! ([`pocketdesk_core::streaming::latency_trace`]) by the pattern's frame
//! number. For every picture the window painted the table gives each step
//! from the host's frame to the GPU finishing the picture, in milliseconds
//! (p50 and p95). What it leaves out is the display itself: the compositor's
//! pass and the scan-out after the swap.
//!
//! Ignored: it starts two real servers (`POCKETDESK_SERVER_EXE`) and opens a
//! window, so it needs a Wayland compositor and a GPU. Run it only in a
//! compositor of its own, never the owner's session: a headless sway
//! (`WLR_BACKENDS=headless`, its own `XDG_RUNTIME_DIR`, no D-Bus; every
//! window floating) is what the numbers in `docs/status-history.md` used.
//! `PD_LIVE_SECS` (15) is how long it runs, `PD_LIVE_HEVC=1` lets B send
//! HEVC, `PD_LIVE_HW=0` decodes on the processor (the app's own default is
//! the card first), `PD_LIVE_VSYNC=0` turns the window's vsync off.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::latency_probe;
use pocketdesk_core::streaming::latency_trace::{self, Event};

use crate::computer_view::ComputerView;
use crate::view_live::{StopViewOnDrop, start_pair, trace_file};

/// The stages, in the order a picture takes them.
const STAGES: [&str; 12] = [
    "host frame > encoder out",
    "encoder out > written",
    "written > viewer AU",
    "AU > picture (decoder)",
    "picture > window's pass",
    "pass > window built",
    "built > paint callback",
    "callback > planes uploaded",
    "uploaded > drawn",
    "pass > GPU done",
    "total (frame > GPU done)",
    "pictures painted / decoded",
];

/// One picture's steps, microseconds on the wall clock.
#[derive(Debug, Default, Clone, Copy)]
struct Steps {
    frame: Option<u64>,
    encoded: Option<u64>,
    written: Option<u64>,
    au: Option<u64>,
    picture: Option<u64>,
    pass: Option<u64>,
    built: Option<u64>,
    callback: Option<u64>,
    uploaded: Option<u64>,
    painted: Option<u64>,
}

/// The nearest-rank `p`th of `v`.
fn percentile(v: &mut [i64], p: f64) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[((v.len() - 1) as f64 * p).round() as usize])
}

fn ms(us: Option<i64>) -> String {
    us.map_or_else(|| "-".to_string(), |u| format!("{:.1}", u as f64 / 1000.0))
}

/// The window's trace lines: kind, time, what.
fn viewer_lines(text: &str) -> Vec<(&str, u64, &str)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let kind = parts.next()?;
            let at = parts.next()?.parse().ok()?;
            Some((kind, at, parts.next()?))
        })
        .collect()
}

/// Join the host's events and the window's lines by frame number, for
/// the pictures painted after `began` (wall clock microseconds).
fn join(host: &[Event], viewer: &str, began: u64) -> (Vec<Steps>, usize) {
    let mut frame_at: HashMap<u16, (u64, u64)> = HashMap::new();
    let mut out_of: HashMap<u64, (u64, u64)> = HashMap::new();
    let mut wrote: HashMap<u64, u64> = HashMap::new();
    for e in host {
        match *e {
            Event::Pattern { at, n, ts, .. } => {
                frame_at.insert(n as u16, (at, ts));
            }
            Event::EncoderOut { at, ts, seq, .. } => {
                out_of.insert(ts, (at, seq));
            }
            Event::Written { at, seq } => {
                wrote.insert(seq, at);
            }
            _ => {}
        }
    }
    let lines = viewer_lines(viewer);
    let mut au_at: HashMap<u64, u64> = HashMap::new();
    let mut by_n: HashMap<u16, Steps> = HashMap::new();
    let mut decoded = 0;
    for (kind, at, what) in &lines {
        if *at < began {
            continue;
        }
        match *kind {
            "vau" => {
                if let Ok(seq) = what.parse() {
                    au_at.insert(seq, *at);
                }
            }
            "vpic" => {
                decoded += 1;
                if let Ok(n) = what.parse::<u16>() {
                    by_n.entry(n).or_default().picture = Some(*at);
                }
            }
            "vpass" | "vui" | "vcb" | "vupl" | "vpaint" => {
                // The first of each: later passes repeat the picture's number.
                if let Ok(n) = what.parse::<u16>() {
                    let s = by_n.entry(n).or_default();
                    let slot = match *kind {
                        "vpass" => &mut s.pass,
                        "vui" => &mut s.built,
                        "vcb" => &mut s.callback,
                        "vupl" => &mut s.uploaded,
                        _ => &mut s.painted,
                    };
                    if slot.is_none() {
                        *slot = Some(*at);
                    }
                }
            }
            _ => {}
        }
    }
    let mut steps = Vec::new();
    for (n, mut s) in by_n {
        if s.painted.is_none() {
            continue;
        }
        if let Some(&(at, ts)) = frame_at.get(&n) {
            s.frame = Some(at);
            if let Some(&(out, seq)) = out_of.get(&ts) {
                s.encoded = Some(out);
                s.written = wrote.get(&seq).copied();
                s.au = au_at.get(&seq).copied();
            }
        }
        steps.push(s);
    }
    (steps, decoded)
}

/// The table's lines for `steps` (`decoded` pictures reached the window).
fn table(steps: &[Steps], decoded: usize) -> Vec<String> {
    let gap = |a: fn(&Steps) -> Option<u64>, b: fn(&Steps) -> Option<u64>| {
        let mut v: Vec<i64> = steps
            .iter()
            .filter_map(|s| Some(b(s)? as i64 - a(s)? as i64))
            .collect();
        (percentile(&mut v.clone(), 0.5), percentile(&mut v, 0.95))
    };
    let rows = [
        gap(|s| s.frame, |s| s.encoded),
        gap(|s| s.encoded, |s| s.written),
        gap(|s| s.written, |s| s.au),
        gap(|s| s.au, |s| s.picture),
        gap(|s| s.picture, |s| s.pass),
        gap(|s| s.pass, |s| s.built),
        gap(|s| s.built, |s| s.callback),
        gap(|s| s.callback, |s| s.uploaded),
        gap(|s| s.uploaded, |s| s.painted),
        gap(|s| s.pass, |s| s.painted),
        gap(|s| s.frame, |s| s.painted),
    ];
    let mut lines = Vec::new();
    for (name, (p50, p95)) in STAGES.iter().zip(rows) {
        lines.push(format!("{name}: {} / {}", ms(p50), ms(p95)));
    }
    lines.push(format!("{}: {} / {}", STAGES[11], steps.len(), decoded));
    lines
}

struct Show {
    view: Option<ComputerView>,
    device: String,
    state: std::path::PathBuf,
    hevc: bool,
    hw: bool,
    began: Instant,
    secs: u64,
    opened: bool,
}

static DONE: AtomicBool = AtomicBool::new(false);

impl eframe::App for Show {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let view = self
            .view
            .get_or_insert_with(|| ComputerView::new(&ctx, self.state.clone(), None));
        if !self.opened {
            crate::video::start_hevc_probe();
            view.set_hevc(self.hevc);
            view.set_hw_decode(self.hw);
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
            self.began = Instant::now();
        }
        view.show(&ctx);
        if self.began.elapsed() > Duration::from_secs(self.secs) {
            DONE.store(true, Ordering::Relaxed);
            ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "starts two real servers and opens a window: needs a compositor of its own"]
fn present_stages() {
    use winit::platform::wayland::EventLoopBuilderExtWayland as _;
    let hevc = std::env::var("PD_LIVE_HEVC").is_ok_and(|v| v == "1");
    let hw = std::env::var("PD_LIVE_HW").map_or(true, |v| v != "0");
    let vsync = std::env::var("PD_LIVE_VSYNC").map_or(true, |v| v != "0");
    let secs: u64 = std::env::var("PD_LIVE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let pair = start_pair((19536, 19636), Some(latency_probe::MODE), hevc, false);
    let viewer = pair.a_base.join("viewer-trace.log");
    crate::view_present_trace::start(&viewer);
    let began = latency_trace::wall_us();
    let shown = Mutex::new(Some(Show {
        view: None,
        device: pair.b_id.clone(),
        state: pair.a_state.clone(),
        hevc,
        hw,
        began: Instant::now(),
        secs,
        opened: false,
    }));
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
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
    {
        let _stop = StopViewOnDrop(&pair.a_state);
        eframe::run_native(
            "present probe",
            options,
            Box::new(move |cc| {
                crate::nv12::set_glow(cc.gl.is_some());
                if let Some(gl) = cc.gl.as_ref() {
                    use eframe::glow::HasContext as _;
                    // SAFETY: the context eframe made, current on this thread.
                    let renderer = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                    println!("GL renderer: {renderer}");
                }
                Ok(Box::new(shown.lock().unwrap().take().unwrap()))
            }),
        )
        .unwrap();
    }
    assert!(DONE.load(Ordering::Relaxed), "the window ran its time");
    let host = latency_trace::read(&trace_file(&pair.b_base));
    let text = std::fs::read_to_string(&viewer).unwrap_or_default();
    let (steps, decoded) = join(&host, &text, began);
    println!(
        "window stages ({}, {}, vsync {}), p50 / p95 ms over {} pictures:",
        if hevc { "HEVC" } else { "H.264" },
        if hw { "card first" } else { "processor" },
        if vsync { "on" } else { "off" },
        steps.len()
    );
    for line in table(&steps, decoded) {
        println!("  {line}");
    }
    assert!(steps.len() > 100, "pictures were painted");
    drop(pair);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_followed_from_the_hosts_frame_to_its_paint() {
        let host = [
            Event::Pattern {
                at: 1_000,
                n: 70_001,
                ts: 500,
                x_norm: None,
            },
            Event::EncoderOut {
                at: 4_000,
                ts: 500,
                seq: 9,
                bytes: 10,
                key: false,
            },
            Event::Written { at: 4_100, seq: 9 },
        ];
        let viewer =
            "vau 4300 9\nvpic 8000 4465\nvpass 9000 4465\nvpaint 12000 4465\nvpic 8100 -\n";
        let (steps, decoded) = join(&host, viewer, 0);
        assert_eq!(decoded, 2);
        assert_eq!(steps.len(), 1);
        let s = steps[0];
        assert_eq!(
            (s.frame, s.encoded, s.written, s.au),
            (Some(1_000), Some(4_000), Some(4_100), Some(4_300))
        );
        assert_eq!(
            (s.picture, s.pass, s.painted),
            (Some(8000), Some(9000), Some(12000))
        );
        let lines = table(&steps, decoded);
        assert!(lines[10].ends_with("11.0 / 11.0"), "{lines:?}");
        assert!(lines[11].ends_with("1 / 2"), "{lines:?}");
        // A picture before the run began is left out.
        assert_eq!(join(&host, viewer, 20_000).0.len(), 0);
    }
}
