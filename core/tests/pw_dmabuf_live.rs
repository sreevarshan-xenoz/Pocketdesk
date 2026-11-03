//! The PipeWire capture's DMA-BUF buffers against a real compositor: mutter's
//! own ScreenCast (the unattended GNOME path) in a headless GNOME Shell, and
//! the real VA-API card. The picture reaches the in-process encoder without a
//! copy, the capture turns back to copies when told to, nothing is left open,
//! and what it costs next to the copy through memory.
//!
//! Ignored: it needs a compositor, a render node with a VA-API driver and a
//! moving window on the compositor's screen. The compositor is a throwaway
//! one (docs/development-setup.md §3a-3: a container with GNOME Shell, mutter
//! and PipeWire, `renderD128` only), never the owner's session: the test
//! refuses to run unless `PD_PW_RIG=1` and both `DBUS_SESSION_BUS_ADDRESS`
//! and `XDG_RUNTIME_DIR` name the container's (a path under `/proc/<pid>/root`),
//! which is how this process reaches the rig's session bus and PipeWire socket.
//!
//! `PD_PW_RIG=1 DBUS_SESSION_BUS_ADDRESS=unix:path=/proc/<pid>/root/run/user/1000/bus
//! XDG_RUNTIME_DIR=/proc/<pid>/root/run/user/1000 POCKETDESK_VAAPI_DEVICE=/dev/dri/renderD128
//! cargo test -p pocketdesk-core --release --test pw_dmabuf_live -- --ignored
//! --nocapture --test-threads=1`. `PD_LIVE_SECS` (15) is how long the cost test
//! runs, `PD_LIVE_COMPOSITOR_PID` names the compositor whose processor time is
//! counted too, `POCKETDESK_CAPTURE_COPY=1` runs the cost test on the copy
//! through memory for the other half of the comparison, `PD_LIVE_OUT` is a
//! file the first keyframe is written to, `PD_LIVE_LOG=1` shows the capture's
//! log.
#![cfg(all(target_os = "linux", feature = "capture", feature = "inproc"))]

use std::time::{Duration, Instant};

use pocketdesk_core::streaming::capture::CaptureSession;
use pocketdesk_core::streaming::dmabuf;
use pocketdesk_core::streaming::encoder::VideoEncoder;
use pocketdesk_core::streaming::{
    EncoderPreset, H264Profile, HardwareEncoder, StreamingConfig, VideoCodec, VideoFrame,
    capture_mutter, latest_frame,
};
use tokio_util::sync::CancellationToken;

/// Refuse to run anywhere but a throwaway compositor's session.
fn guard() {
    if std::env::var_os("PD_LIVE_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init();
    }
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    assert!(
        std::env::var("PD_PW_RIG").as_deref() == Ok("1")
            && bus.contains("/proc/")
            && runtime.starts_with("/proc/"),
        "run this against a throwaway compositor's session (PD_PW_RIG=1, the bus and the runtime \
         folder under /proc/<pid>/root)"
    );
}

fn config() -> StreamingConfig {
    StreamingConfig {
        width: 1280,
        height: 720,
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
    session: CaptureSession,
    runtime: tokio::runtime::Runtime,
}

impl Capture {
    fn start() -> Self {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (tx, rx) = latest_frame::channel::<VideoFrame>();
        let cancel = CancellationToken::new();
        let session = runtime
            .block_on(capture_mutter::start_capture(
                config(),
                tx,
                cancel.clone(),
                false,
                None,
            ))
            .expect("mutter's ScreenCast");
        Self {
            rx,
            cancel,
            session,
            runtime,
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

    /// The first frame on the card within `within`.
    fn next_on_card(&mut self, within: Duration) -> Option<VideoFrame> {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if let Some(frame) = self.next(Duration::from_millis(500))
                && frame.gpu.is_some()
            {
                return Some(frame);
            }
        }
        None
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.cancel.cancel();
        std::thread::sleep(Duration::from_millis(300));
        let _ = &self.runtime;
        let _ = &self.session;
    }
}

fn cpu_seconds() -> f64 {
    // SAFETY: getrusage fills the struct we pass; it is plain data.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(usage.ru_utime) + t(usage.ru_stime)
}

/// A process's processor time, from `/proc/<pid>/stat`.
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

/// Frames come on the card when the compositor gave DMA-BUFs and the card
/// took them: the first keyframe is encoded from one with no copy, and (with
/// `PD_LIVE_OUT`) written to a file to look at.
#[test]
#[ignore = "needs a throwaway GNOME Shell rig and a VA-API render node"]
fn mutters_buffers_reach_the_encoder_on_the_card() {
    guard();
    dmabuf::set_copy(false);
    let mut capture = Capture::start();
    let frame = capture
        .next_on_card(Duration::from_secs(20))
        .expect("no frame on the card: the capture copied (PD_LIVE_LOG=1 says why)");
    let mut encoder = VideoEncoder::new(config()).expect("an encoder");
    assert!(
        encoder.takes_dmabuf(),
        "a frame on the card for an encoder that cannot take it"
    );
    let mut keyframe = None;
    for _ in 0..30 {
        if let Some(packet) = encoder
            .encode_frame(&frame)
            .expect("encode")
            .into_iter()
            .next()
        {
            keyframe = Some(packet);
            break;
        }
    }
    let keyframe = keyframe.expect("the encoder gave no packet");
    println!(
        "frame {}x{} on the card, keyframe {} bytes",
        frame.width,
        frame.height,
        keyframe.data.len()
    );
    assert!(keyframe.is_keyframe);
    assert!(keyframe.data.len() > 2000, "an empty picture");
    if let Some(path) = std::env::var_os("PD_LIVE_OUT") {
        std::fs::write(path, &keyframe.data).unwrap();
    }
}

#[test]
#[ignore = "needs a throwaway GNOME Shell rig, a moving window and a VA-API render node"]
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

    let warm = Duration::from_secs(4);
    let started = Instant::now();
    let mut counted_from: Option<(Instant, f64, Option<f64>)> = None;
    let mut fds_after_warmup = 0usize;
    let (mut on_card, mut in_memory, mut packets) = (0u32, 0u32, 0u32);
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
        "frames {} ({} on the card, {} in memory) in {wall:.1} s = {:.1}/s, packets {packets}",
        on_card + in_memory,
        on_card,
        in_memory,
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

/// The streamer's switch turns the capture back to copies: the stream is
/// asked again for memory buffers, and within seconds only copies come.
#[test]
#[ignore = "needs a throwaway GNOME Shell rig, a moving window and a VA-API render node"]
fn the_switch_turns_the_capture_back_to_copies() {
    guard();
    dmabuf::set_copy(false);
    let mut capture = Capture::start();
    capture
        .next_on_card(Duration::from_secs(20))
        .expect("a frame on the card: the capture copied (PD_LIVE_LOG=1)");
    let switch = capture
        .session
        .zero_copy_switch()
        .expect("the PipeWire capture has the switch");
    switch.store(false, std::sync::atomic::Ordering::Relaxed);
    let until = Instant::now() + Duration::from_secs(10);
    let mut copy = None;
    while Instant::now() < until && copy.is_none() {
        copy = capture
            .next(Duration::from_millis(500))
            .filter(|f| f.gpu.is_none());
    }
    let copy = copy.expect("a copy through memory within ten seconds");
    assert_eq!(
        copy.data.len(),
        (copy.stride * copy.height) as usize,
        "a whole picture"
    );
    let (mut copies, mut cards) = (0, 0);
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        if let Some(frame) = capture.next(Duration::from_millis(500)) {
            if frame.gpu.is_some() {
                cards += 1;
            } else {
                copies += 1;
            }
        }
    }
    println!("after the switch: {copies} copies, {cards} on the card");
    assert_eq!(cards, 0, "a frame on the card after the switch settled");
    assert!(copies > 0);
}

/// The owner's setting (or the variable) keeps the capture on copies from the
/// start.
#[test]
#[ignore = "needs a throwaway GNOME Shell rig and a VA-API render node"]
fn the_setting_keeps_the_capture_on_copies() {
    guard();
    dmabuf::set_copy(true);
    let mut capture = Capture::start();
    for _ in 0..30 {
        if let Some(frame) = capture.next(Duration::from_secs(10)) {
            assert!(
                frame.gpu.is_none(),
                "a frame on the card with copies forced"
            );
        }
    }
}
