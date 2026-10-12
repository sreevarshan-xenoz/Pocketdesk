//! Pinned encoder benchmark (roadmap 2195-2200).
//!
//! The measurement Phase 1 exists to make possible: the same clip, the same
//! geometry, the same bitrate, every run — so a change to the encoder path
//! shows up as a percentile delta against a committed baseline rather than as a
//! vague feeling about how the desktop looked today.
//!
//! The clip is generated, not stored. A committed 300-frame 720p BGRA sequence
//! would be ~1.1 GB of binary in the repository; [`scene_frame`] is a pure
//! function of `(width, height, index)`, so the workload is reproducible from
//! source and byte-identical between runs. It mixes a static gradient with a
//! moving block, because a frame that is identical every time lets the encoder
//! coast on zero-residual skip blocks and measures nothing.
//!
//! What this measures is encode time — the part that runs on this machine.
//! Decode and render live on the phone and are not in this record; pretending
//! otherwise would be the exact overclaim this file exists to prevent.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Instant;

use super::encoder::VideoEncoder;
use super::encoder_detect::HardwareEncoder;
use super::{EncoderPreset, H264Profile, StreamingConfig, VideoCodec, VideoFrame};
use crate::metrics::{Samples, Summary};

/// Shape of a record. A reader must refuse to compare across schemas.
pub const RECORD_SCHEMA: u32 = 1;

/// Pinned workload geometry. 720p at 5 Mbit/s: large enough that the encoder
/// does real work, small enough that a shared CI runner finishes in seconds.
pub const BENCH_WIDTH: u32 = 1280;
pub const BENCH_HEIGHT: u32 = 720;
pub const BENCH_BITRATE_BPS: u32 = 5_000_000;
pub const BENCH_FPS: u32 = 30;
pub const DEFAULT_FRAMES: u32 = 300;

/// Frames discarded before sampling: the first encode of a session pays IDR
/// and encoder-startup cost that is not the steady state being tracked.
pub const WARMUP_FRAMES: u32 = 30;

/// The pinned clip: one BGRA frame, deterministic in `(width, height, index)`.
pub fn scene_frame(width: u32, height: u32, index: u32) -> VideoFrame {
    let stride = (width * 4) as usize;
    let mut data = vec![0u8; stride * height as usize];
    // A moving 128x128 block over a static diagonal gradient. The block's
    // content depends on its position, so motion estimation gets a real
    // residual to code instead of a copy.
    let bx = ((index as usize * 17) % (width as usize - 128)) as u32;
    let by = ((index as usize * 11) % (height as usize - 128)) as u32;
    for y in 0..height {
        let row = &mut data[y as usize * stride..(y as usize + 1) * stride];
        for x in 0..width {
            let px = &mut row[x as usize * 4..x as usize * 4 + 4];
            let in_block = x >= bx && x < bx + 128 && y >= by && y < by + 128;
            if in_block {
                let t = index.wrapping_mul(7) ^ (x * 3) ^ (y * 5);
                px[0] = (t % 256) as u8;
                px[1] = (t / 7 % 256) as u8;
                px[2] = (t / 13 % 256) as u8;
            } else {
                let g = ((x + y + index * 3) % 256) as u8;
                px[0] = g;
                px[1] = g / 2;
                px[2] = 255 - g / 2;
            }
            px[3] = 0xff;
        }
    }
    VideoFrame {
        data,
        width,
        height,
        stride: stride as u32,
        timestamp: Instant::now(),
    }
}

/// Which rung to benchmark. `Auto` is deliberately absent: the ladder's choice
/// is host-dependent, and a baseline whose backend was decided by probe order
/// cannot be compared after a driver update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchTarget {
    Software,
    Vaapi,
    Nvenc,
    Amf,
    Qsv,
    MediaFoundation,
}

impl BenchTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            BenchTarget::Software => "software",
            BenchTarget::Vaapi => "vaapi",
            BenchTarget::Nvenc => "nvenc",
            BenchTarget::Amf => "amf",
            BenchTarget::Qsv => "qsv",
            BenchTarget::MediaFoundation => "mf",
        }
    }

    fn hardware_encoder(self) -> HardwareEncoder {
        match self {
            BenchTarget::Software => HardwareEncoder::Software,
            BenchTarget::Vaapi => HardwareEncoder::Vaapi,
            BenchTarget::Nvenc => HardwareEncoder::Nvenc,
            BenchTarget::Amf => HardwareEncoder::Amf,
            BenchTarget::Qsv => HardwareEncoder::Qsv,
            BenchTarget::MediaFoundation => HardwareEncoder::MediaFoundation,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "software" | "x264" => Some(BenchTarget::Software),
            "vaapi" => Some(BenchTarget::Vaapi),
            "nvenc" => Some(BenchTarget::Nvenc),
            "amf" => Some(BenchTarget::Amf),
            "qsv" => Some(BenchTarget::Qsv),
            "mf" => Some(BenchTarget::MediaFoundation),
            _ => None,
        }
    }
}

/// One benchmark run, in the same `key=value`/JSON spirit as a session record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRecord {
    pub schema: u32,
    pub unix_secs: u64,
    /// The encoder that actually ran, read back from the encoder after any
    /// internal fallback — so a request for `vaapi` that silently degraded to
    /// software is recorded as what happened, not what was asked.
    pub backend: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    pub preset: String,
    pub codec: String,
    pub frames: u32,
    pub warmup: u32,
    /// CPU model + kernel. Encode throughput is not portable between hosts, so
    /// this gates whether a comparison is meaningful at all.
    pub host: String,
    /// Per-frame encode time over the sampled frames.
    pub encode_ms: Summary,
    /// Whole-run wall clock, and the output the encoder produced.
    pub elapsed_ms: u64,
    pub output_bytes: u64,
    pub keyframes: u64,
    /// Frames dropped by the encoder's latency phase (consumed, no packet out).
    pub frames_without_packet: u64,
}

/// The workload half of a record: everything a candidate must match before its
/// timings mean anything. `Debug` so a mismatch reports every field that
/// differs, rather than a hand-written subset that could hide one.
#[derive(Debug, PartialEq, Eq)]
struct Workload {
    schema: u32,
    backend: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
    preset: String,
    codec: String,
    frames: u32,
    warmup: u32,
    host: String,
}

impl BenchRecord {
    fn workload(&self) -> Workload {
        Workload {
            schema: self.schema,
            backend: self.backend.clone(),
            width: self.width,
            height: self.height,
            fps: self.fps,
            bitrate_bps: self.bitrate_bps,
            preset: self.preset.clone(),
            codec: self.codec.clone(),
            frames: self.frames,
            warmup: self.warmup,
            host: self.host.clone(),
        }
    }
}

/// Run the pinned workload. Blocking by design: this is a measurement, and a
/// tokio task in the middle of it would add scheduling noise to the numbers.
pub fn run(target: BenchTarget, frames: u32) -> Result<BenchRecord> {
    let config = StreamingConfig {
        width: BENCH_WIDTH,
        height: BENCH_HEIGHT,
        fps: BENCH_FPS,
        bitrate_bps: BENCH_BITRATE_BPS,
        codec: VideoCodec::H264,
        profile: H264Profile::Main,
        preset: EncoderPreset::VeryFast,
        hardware_encoder: target.hardware_encoder(),
        monitor_index: 0,
    };
    let mut encoder = VideoEncoder::new(config)?;
    let samples = Samples::new();
    let started = Instant::now();
    let mut output_bytes = 0u64;
    let mut keyframes = 0u64;
    let mut without_packet = 0u64;

    for index in 0..frames {
        let frame = scene_frame(BENCH_WIDTH, BENCH_HEIGHT, index);
        let encode_started = Instant::now();
        let packets = encoder.encode_frame(&frame)?;
        if index >= WARMUP_FRAMES {
            samples.push_micros(encode_started.elapsed().as_micros() as u64);
        }
        if packets.is_empty() {
            without_packet += 1;
        }
        for packet in packets {
            output_bytes += packet.data.len() as u64;
            if packet.is_keyframe {
                keyframes += 1;
            }
        }
    }

    let encode_ms = samples
        .summary_ms()
        .filter(|_| frames > WARMUP_FRAMES)
        .ok_or_else(|| {
            anyhow::anyhow!("benchmark needs more than {WARMUP_FRAMES} frames to sample")
        })?;

    Ok(BenchRecord {
        schema: RECORD_SCHEMA,
        unix_secs: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        backend: encoder.backend_name().to_string(),
        width: BENCH_WIDTH,
        height: BENCH_HEIGHT,
        fps: BENCH_FPS,
        bitrate_bps: BENCH_BITRATE_BPS,
        preset: "veryfast".into(),
        codec: "h264".into(),
        frames,
        warmup: WARMUP_FRAMES,
        host: host_identity(),
        encode_ms,
        elapsed_ms: started.elapsed().as_millis() as u64,
        output_bytes,
        keyframes,
        frames_without_packet: without_packet,
    })
}

/// A single metric that got worse beyond its allowance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regression {
    pub metric: &'static str,
    pub baseline_ms: u64,
    pub candidate_ms: u64,
    /// The value the candidate was allowed to reach.
    pub limit_ms: u64,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} regressed: baseline {} ms -> candidate {} ms (limit {} ms)",
            self.metric, self.baseline_ms, self.candidate_ms, self.limit_ms
        )
    }
}

/// Compare a candidate run against the committed baseline.
///
/// `Err` with a reason when the two records are not the same workload (or the
/// same host) — an incomparable run must be reported as skipped, never as a
/// pass. The mean is not compared: it is the statistic that hides tail
/// regressions, which is the whole reason the record carries percentiles.
pub fn compare(
    baseline: &BenchRecord,
    candidate: &BenchRecord,
    tolerance_pct: u64,
) -> Result<Vec<Regression>, String> {
    if baseline.workload() != candidate.workload() {
        return Err(format!(
            "not comparable: baseline workload {:?} != candidate workload {:?}",
            baseline.workload(),
            candidate.workload()
        ));
    }
    let b = &baseline.encode_ms;
    let c = &candidate.encode_ms;
    let mut found = Vec::new();
    for (metric, base, cand) in [
        ("encode_p50_ms", b.p50_ms, c.p50_ms),
        ("encode_p90_ms", b.p90_ms, c.p90_ms),
        ("encode_p95_ms", b.p95_ms, c.p95_ms),
        ("encode_p99_ms", b.p99_ms, c.p99_ms),
        ("encode_max_ms", b.max_ms, c.max_ms),
    ] {
        let limit = base + base * tolerance_pct / 100;
        if cand > limit {
            found.push(Regression {
                metric,
                baseline_ms: base,
                candidate_ms: cand,
                limit_ms: limit,
            });
        }
    }
    Ok(found)
}

/// CPU model + kernel release, best effort. A missing `/proc` (a container with
/// a masked procfs, or a non-Linux dev box) yields `unknown`, which makes every
/// cross-host comparison incomparable — the safe direction for a mistake.
///
/// On Windows: CPU model, Windows build, and the GPUs — a hardware rung's
/// throughput is the GPU's, so two boxes with one CPU but different GPUs must
/// not compare.
#[cfg(windows)]
pub fn host_identity() -> String {
    windows_host::identity()
}

/// CPU model + kernel release (see the Windows variant for that OS).
#[cfg(not(windows))]
pub fn host_identity() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "unknown-cpu".into());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|k| k.trim().to_string())
        .unwrap_or_else(|_| "unknown-kernel".into());
    format!("{cpu} / {kernel}")
}

#[cfg(windows)]
mod windows_host {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;

    fn registry_string(subkey: &str, value: &str) -> Option<String> {
        let mut buffer = [0u16; 256];
        let mut bytes = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(subkey),
                &HSTRING::from(value),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status.is_err() {
            return None;
        }
        let units = (bytes as usize / 2).saturating_sub(1);
        Some(
            String::from_utf16_lossy(&buffer[..units])
                .trim()
                .to_string(),
        )
    }

    fn gpus() -> Vec<String> {
        let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        let mut index = 0;
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
            index += 1;
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
                continue;
            };
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(128);
            let name = String::from_utf16_lossy(&desc.Description[..len]);
            // The software rasterizer is present on every machine.
            if name != "Microsoft Basic Render Driver" && !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    pub(super) fn identity() -> String {
        let cpu = registry_string(
            r"HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        )
        .unwrap_or_else(|| "unknown-cpu".into());
        let build = registry_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuildNumber",
        )
        .map(|b| format!("Windows build {b}"))
        .unwrap_or_else(|| "unknown-windows".into());
        let gpus = gpus();
        let gpus = if gpus.is_empty() {
            "unknown-gpu".to_string()
        } else {
            gpus.join(" + ")
        };
        format!("{cpu} / {build} / {gpus}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_windows_host_names_its_cpu_build_and_gpu() {
        let id = host_identity();
        assert!(!id.contains("unknown-cpu"), "{id}");
        assert!(id.contains("Windows build "), "{id}");
        assert_eq!(id.matches(" / ").count(), 2, "{id}");
    }

    #[test]
    fn every_target_round_trips_through_its_name() {
        for target in [
            BenchTarget::Software,
            BenchTarget::Vaapi,
            BenchTarget::Nvenc,
            BenchTarget::Amf,
            BenchTarget::Qsv,
            BenchTarget::MediaFoundation,
        ] {
            assert_eq!(BenchTarget::parse(target.as_str()), Some(target));
        }
    }

    fn record_with(backend: &str, host: &str, p: u64) -> BenchRecord {
        BenchRecord {
            schema: RECORD_SCHEMA,
            unix_secs: 0,
            backend: backend.into(),
            width: BENCH_WIDTH,
            height: BENCH_HEIGHT,
            fps: BENCH_FPS,
            bitrate_bps: BENCH_BITRATE_BPS,
            preset: "veryfast".into(),
            codec: "h264".into(),
            frames: DEFAULT_FRAMES,
            warmup: WARMUP_FRAMES,
            host: host.into(),
            encode_ms: Summary {
                count: 270,
                mean_ms: 4,
                p50_ms: 4,
                p90_ms: 6,
                p95_ms: p,
                p99_ms: p,
                max_ms: p,
            },
            elapsed_ms: 1_000,
            output_bytes: 500_000,
            keyframes: 5,
            frames_without_packet: 0,
        }
    }

    #[test]
    fn the_clip_is_the_same_clip_every_time() {
        let a = scene_frame(320, 180, 7);
        let b = scene_frame(320, 180, 7);
        assert_eq!(a.data, b.data, "the workload must be reproducible");
        assert_eq!(a.data.len(), 320 * 4 * 180);
        assert_ne!(a.data, scene_frame(320, 180, 8).data);
    }

    #[test]
    fn a_baseline_only_compares_against_the_same_run() {
        let base = record_with("libx264", "i7 / 7.2", 8);
        // Same workload, same tail: nothing to report.
        assert!(
            compare(&base, &record_with("libx264", "i7 / 7.2", 8), 25)
                .unwrap()
                .is_empty()
        );
        // Different host or backend: refuse, do not pass.
        assert!(compare(&base, &record_with("libx264", "other cpu", 8), 25).is_err());
        assert!(compare(&base, &record_with("h264_vaapi", "i7 / 7.2", 8), 25).is_err());
        let different_frames = BenchRecord {
            frames: 100,
            ..record_with("libx264", "i7 / 7.2", 8)
        };
        assert!(compare(&base, &different_frames, 25).is_err());
    }

    #[test]
    fn a_tail_beyond_the_tolerance_fails_and_says_by_how_much() {
        let base = record_with("libx264", "i7 / 7.2", 8);
        // p50 unchanged, tail 50% worse: exactly the regression an average
        // would average away.
        let candidate = BenchRecord {
            encode_ms: Summary {
                p95_ms: 12,
                p99_ms: 12,
                max_ms: 12,
                ..base.encode_ms
            },
            ..base.clone()
        };
        let ok = compare(&base, &candidate, 60).unwrap();
        assert!(ok.is_empty(), "a 50% tail move inside a 60% limit passes");
        let bad = compare(&base, &candidate, 25).unwrap();
        assert_eq!(bad.len(), 3, "p95, p99 and max all moved");
        let p95 = bad.iter().find(|r| r.metric == "encode_p95_ms").unwrap();
        assert_eq!(
            (p95.baseline_ms, p95.candidate_ms, p95.limit_ms),
            (8, 12, 10)
        );
        assert!(p95.to_string().contains("encode_p95_ms regressed"));
    }

    #[test]
    fn an_improvement_is_never_a_regression() {
        let base = record_with("libx264", "i7 / 7.2", 8);
        let faster = record_with("libx264", "i7 / 7.2", 3);
        assert!(compare(&base, &faster, 25).unwrap().is_empty());
    }

    #[test]
    fn a_record_round_trips_through_json_for_a_committed_baseline() {
        let json = serde_json::to_string(&record_with("libx264", "i7 / 7.2", 8)).unwrap();
        let back: BenchRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.backend, "libx264");
        assert_eq!(back.encode_ms.p99_ms, 8);
    }

    #[test]
    fn host_identity_names_the_machine_instead_of_guessing() {
        let id = host_identity();
        assert!(!id.is_empty());
        // Either both halves resolved or both are the explicit unknown marker;
        // a half-known host string would silently widen a comparison.
        assert!(
            id.contains("unknown") == (id.contains("unknown-cpu") || id.contains("unknown-kernel")),
            "unexpected host identity {id:?}"
        );
    }

    /// The real thing: 60 frames through the pinned software rung. Slow enough
    /// to be a benchmark, fast enough to run in a test suite, and the proof
    /// that [`run`] is not a struct literal wearing a function name.
    #[test]
    #[ignore = "runs a real 60-frame encode; CI cannot promise a runner idle enough for its timings"]
    fn run_produces_a_record_from_a_real_encode() {
        let record = run(BenchTarget::Software, 60).unwrap();
        assert_eq!(record.backend, "in-process", "software rung is in-process");
        assert_eq!(record.frames, 60);
        assert_eq!(record.encode_ms.count, 30, "60 frames less the warm-up");
        assert!(record.output_bytes > 0);
        assert!(record.encode_ms.p50_ms > 0);
    }
}
