//! The DMA-BUF capture (phase 2 of the low-latency window handoff) against a
//! real wlroots compositor and a real VA-API card: the picture it gives is
//! the copy's picture, no descriptor or surface is left behind, and what it
//! costs next to the copy through memory.
//!
//! Ignored: it needs a compositor, a render node with a VA-API driver, and
//! something on the screen. Run it only in a headless sway of its own
//! (`WLR_BACKENDS=headless`, `WLR_RENDERER=gles2`, `WLR_RENDER_DRM_DEVICE`
//! and `POCKETDESK_VAAPI_DEVICE` both naming the same render node, its own
//! `XDG_RUNTIME_DIR` under `/tmp`), never the owner's session: the test
//! refuses to run unless `SWAYSOCK` and an `XDG_RUNTIME_DIR` under `/tmp` are
//! set. Put a window on its screen first: a still one for the picture tests
//! (a terminal with text), a moving one for the cost test (the example
//! `pd_source`, a full-screen window that draws the test pattern at the
//! display's rate: `PD_SOURCE_SECS` is how long it runs).
//!
//! `cargo test -p pocketdesk-core --release --test zero_copy_live -- --ignored
//! --nocapture --test-threads=1`. `PD_LIVE_SECS` (15) is how long the cost
//! test runs, `PD_LIVE_COMPOSITOR_PID` names the compositor whose processor
//! time is counted too, and `POCKETDESK_CAPTURE_COPY=1` runs the cost test
//! on the copy through memory for the other half of the comparison.
#![cfg(all(target_os = "linux", feature = "capture", feature = "inproc"))]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::capture_screencopy::start_screencopy_capture;
use pocketdesk_core::streaming::dmabuf;
use pocketdesk_core::streaming::encoder::VideoEncoder;
use pocketdesk_core::streaming::{
    EncoderPreset, H264Profile, HardwareEncoder, StreamingConfig, VideoCodec, VideoFrame,
    latest_frame,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Refuse to run anywhere but a throwaway compositor.
fn guard() {
    // `PD_LIVE_LOG=1` shows the capture's own log.
    if std::env::var_os("PD_LIVE_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    assert!(
        std::env::var_os("SWAYSOCK").is_some() && runtime.starts_with("/tmp/"),
        "run this in a headless sway of its own (SWAYSOCK set, XDG_RUNTIME_DIR under /tmp)"
    );
}

fn config() -> StreamingConfig {
    StreamingConfig {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_bps: 5_000_000,
        codec: VideoCodec::H264,
        profile: H264Profile::Main,
        preset: EncoderPreset::VeryFast,
        hardware_encoder: HardwareEncoder::Auto,
        monitor_index: 0,
    }
}

struct Capture {
    rx: latest_frame::FrameReceiver<VideoFrame>,
    cancel: CancellationToken,
    _session: pocketdesk_core::streaming::capture::CaptureSession,
    _window_tx: watch::Sender<u64>,
}

impl Capture {
    fn start() -> Self {
        let (tx, rx) = latest_frame::channel::<VideoFrame>();
        let cancel = CancellationToken::new();
        let (window_tx, window_rx) = watch::channel(0u64);
        let session = start_screencopy_capture(
            config(),
            tx,
            cancel.clone(),
            window_rx,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .expect("the compositor's screencopy");
        Self {
            rx,
            cancel,
            _session: session,
            _window_tx: window_tx,
        }
    }

    /// The next frame, within `within`.
    fn next(&mut self, within: Duration) -> Option<VideoFrame> {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if let Some(frame) = self.rx.try_recv() {
                return Some(frame);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.cancel.cancel();
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The first access unit the encoder makes of `frame`: a keyframe.
fn first_packet(frame: &VideoFrame) -> Vec<u8> {
    let mut encoder = VideoEncoder::new(config()).expect("an encoder");
    assert!(
        frame.gpu.is_none() || encoder.takes_dmabuf(),
        "a frame on the card goes only to the encoder that takes it"
    );
    for _ in 0..5 {
        if let Some(packet) = encoder
            .encode_frame(frame)
            .expect("encode")
            .into_iter()
            .next()
        {
            assert!(packet.is_keyframe, "the first packet is a keyframe");
            return packet.data;
        }
    }
    panic!("the encoder gave no packet");
}

#[test]
#[ignore = "needs a headless sway of its own and a VA-API render node"]
fn the_card_picture_encodes_as_the_copys_picture() {
    guard();
    // The picture must not move between the two grabs: a still window.
    // The copy through memory first.
    dmabuf::set_copy(true);
    let copy = {
        let mut capture = Capture::start();
        let frame = capture.next(Duration::from_secs(10)).expect("a frame");
        assert!(frame.gpu.is_none(), "copy mode sent a frame on the card");
        first_packet(&frame)
    };
    // Then the card's buffers.
    dmabuf::set_copy(false);
    let card = {
        let mut capture = Capture::start();
        let frame = capture.next(Duration::from_secs(10)).expect("a frame");
        assert!(
            frame.gpu.is_some(),
            "no frame on the card: the capture copied (see the log: PD_LIVE_LOG=1)"
        );
        first_packet(&frame)
    };
    let alike = copy.iter().zip(&card).filter(|(a, b)| a == b).count();
    println!(
        "keyframe of the copy {} bytes, of the card's buffer {} bytes, {alike} bytes alike",
        copy.len(),
        card.len()
    );
    assert!(
        copy.len() > 2000,
        "an empty picture: put a window on the screen first"
    );
    // The same pixels through the same processor and encoder: the same bytes.
    assert_eq!(copy, card, "the card's picture is not the copy's");
}

fn cpu_seconds() -> f64 {
    // SAFETY: getrusage fills the struct we pass; it is plain data.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(usage.ru_utime) + t(usage.ru_stime)
}

/// A process's processor time, from `/proc/<pid>/stat` (fields 14 and 15, in
/// clock ticks).
fn pid_cpu_seconds(pid: u32) -> Option<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    // SAFETY: sysconf with a valid name.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    Some((utime + stime) / ticks)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore = "needs a headless sway of its own, a moving window and a VA-API render node"]
fn what_the_capture_costs_and_what_it_leaves() {
    guard();
    let secs: u64 = std::env::var("PD_LIVE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let compositor: Option<u32> = std::env::var("PD_LIVE_COMPOSITOR_PID")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut capture = Capture::start();
    let mut encoder = VideoEncoder::new(config()).expect("an encoder");
    println!("encoder: {}", encoder.backend_name());

    let warm = Duration::from_secs(3);
    let started = Instant::now();
    let mut counted_from: Option<(Instant, f64, Option<f64>)> = None;
    let mut fds_after_warmup = 0usize;
    let (mut on_card, mut in_memory, mut repeats, mut packets) = (0u32, 0u32, 0u32, 0u32);
    let mut encode_ms = Vec::new();
    let mut frame_to_packet_ms = Vec::new();
    let mut gap_ms = Vec::new();
    let mut last_frame_at: Option<Instant> = None;
    while started.elapsed() < Duration::from_secs(secs) + warm {
        let Some(frame) = capture.next(Duration::from_secs(5)) else {
            panic!("no frame for five seconds");
        };
        let measuring = started.elapsed() >= warm;
        if measuring && counted_from.is_none() {
            fds_after_warmup = dmabuf::open_fds();
            counted_from = Some((
                Instant::now(),
                cpu_seconds(),
                compositor.and_then(pid_cpu_seconds),
            ));
        }
        if frame.gpu.is_some() {
            assert!(
                encoder.takes_dmabuf(),
                "a frame on the card for an encoder that cannot take it"
            );
        }
        let t = Instant::now();
        let out = encoder.encode_frame(&frame).expect("encode");
        let spent = t.elapsed().as_secs_f64() * 1e3;
        if measuring {
            if frame.gpu.is_some() {
                on_card += 1;
            } else {
                in_memory += 1;
            }
            if frame.repeat {
                repeats += 1;
            }
            if !out.is_empty() {
                packets += out.len() as u32;
                encode_ms.push(spent);
                frame_to_packet_ms.push(frame.timestamp.elapsed().as_secs_f64() * 1e3);
            }
            if let Some(last) = last_frame_at {
                gap_ms.push(last.elapsed().as_secs_f64() * 1e3);
            }
        }
        last_frame_at = Some(Instant::now());
    }
    let (from, cpu0, comp0) = counted_from.expect("the run measured something");
    let wall = from.elapsed().as_secs_f64();
    let ours = (cpu_seconds() - cpu0) / wall * 100.0;
    let theirs = comp0
        .zip(compositor.and_then(pid_cpu_seconds))
        .map(|(a, b)| (b - a) / wall * 100.0);
    let fds_end = dmabuf::open_fds();
    println!(
        "frames {} ({} on the card, {} in memory, {} repeats) in {wall:.1} s = {:.1}/s, packets {packets}",
        on_card + in_memory,
        on_card,
        in_memory,
        repeats,
        f64::from(on_card + in_memory) / wall
    );
    println!(
        "encode call ms p50 {:.2} p95 {:.2} max {:.2}",
        percentile(&mut encode_ms.clone(), 0.5),
        percentile(&mut encode_ms.clone(), 0.95),
        percentile(&mut encode_ms, 1.0)
    );
    println!(
        "frame to packet out ms p50 {:.2} p95 {:.2} max {:.2}",
        percentile(&mut frame_to_packet_ms.clone(), 0.5),
        percentile(&mut frame_to_packet_ms.clone(), 0.95),
        percentile(&mut frame_to_packet_ms, 1.0)
    );
    println!(
        "gap between frames ms p50 {:.2} p95 {:.2} max {:.2}",
        percentile(&mut gap_ms.clone(), 0.5),
        percentile(&mut gap_ms.clone(), 0.95),
        percentile(&mut gap_ms, 1.0)
    );
    println!("this process {ours:.1} % of a core");
    if let Some(theirs) = theirs {
        println!("the compositor {theirs:.1} % of a core");
    }
    println!("descriptors open: {fds_after_warmup} after warm-up, {fds_end} at the end");
    assert!(
        fds_end <= fds_after_warmup + 2,
        "descriptors leaked: {fds_after_warmup} then {fds_end}"
    );
}

#[test]
#[ignore = "needs a headless sway of its own with a still window and a VA-API render node"]
fn a_still_screen_repeats_its_last_frame_on_the_card() {
    guard();
    dmabuf::set_copy(false);
    let mut capture = Capture::start();
    let first = capture.next(Duration::from_secs(10)).expect("a frame");
    assert!(first.gpu.is_some(), "the capture copied (PD_LIVE_LOG=1)");
    assert!(!first.repeat);
    let started = Instant::now();
    let (mut frames, mut repeats, mut on_card) = (0u32, 0u32, 0u32);
    while started.elapsed() < Duration::from_secs(6) {
        if let Some(frame) = capture.next(Duration::from_millis(200)) {
            frames += 1;
            repeats += u32::from(frame.repeat);
            on_card += u32::from(frame.gpu.is_some());
        }
    }
    println!("{frames} frames in 6 s of a still screen, {repeats} repeats, {on_card} on the card");
    assert!(
        (3..=12).contains(&frames),
        "a still screen sends about one a second, not {frames}"
    );
    assert_eq!(
        on_card, frames,
        "every repeat is the last frame on the card"
    );
    assert!(repeats >= 3, "the repeats are marked ({repeats})");
}

#[test]
#[ignore = "needs a headless sway of its own with a still window and a VA-API render node"]
fn the_switch_turns_the_capture_back_to_copies() {
    guard();
    dmabuf::set_copy(false);
    let mut capture = Capture::start();
    let first = capture.next(Duration::from_secs(10)).expect("a frame");
    assert!(first.gpu.is_some(), "the capture copied (PD_LIVE_LOG=1)");
    let switch = capture
        ._session
        .zero_copy_switch()
        .expect("the wlroots capture has the switch");
    switch.store(false, std::sync::atomic::Ordering::Relaxed);
    // A few frames already on their way may still be on the card; then only
    // copies, and one comes at once even on a still screen.
    let until = Instant::now() + Duration::from_secs(5);
    let mut copy = None;
    while Instant::now() < until && copy.is_none() {
        copy = capture
            .next(Duration::from_millis(500))
            .filter(|f| f.gpu.is_none());
    }
    let copy = copy.expect("a copy through memory within five seconds");
    assert_eq!(copy.data.len(), (copy.stride * copy.height) as usize);
    for _ in 0..3 {
        if let Some(frame) = capture.next(Duration::from_millis(1500)) {
            assert!(frame.gpu.is_none(), "a frame on the card after the switch");
        }
    }
}
