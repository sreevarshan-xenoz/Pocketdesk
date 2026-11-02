//! How often the host's encoders send a whole picture (an IDR keyframe), and
//! what takes its place: the "Smooth keyframes" setting (`smooth_keyframes`
//! in `config.toml`).
//!
//! A keyframe at 1080p is several times the size of a delta frame, so it
//! takes longer to encode, send and decode: a hitch in the picture's timing
//! at every one. Today an encoder sends one every two seconds, a safety net
//! that heals a picture a decoder spoiled without saying so. The stream does
//! not need that rhythm to start or recover: every session opens its own
//! encoder, whose first frame is a keyframe (a viewer that joins, or joins
//! again, starts clean), and a viewer that loses a frame asks for one
//! (`InputPacket::RequestKeyframe`), which an in-process encoder answers on
//! its next frame.
//!
//! So with the setting on:
//!
//! - An encoder that can refresh the picture in waves (libx264, and NVENC
//!   through FFmpeg) spreads one keyframe's worth of intra-coded blocks over
//!   a refresh period ([`REFRESH_SECS`], continuously), instead of sending
//!   it whole. A picture a decoder spoiled is healed within one period, with
//!   no spike. A whole keyframe still comes on request, and every
//!   [`SAFETY_SECS_REFRESHING`] as a last resort.
//! - An encoder that cannot (VA-API: FFmpeg's `h264_vaapi` offers no intra
//!   refresh, though the card's driver may) sends a keyframe on request and
//!   every [`SAFETY_SECS_PLAIN`] seconds.
//! - The FFmpeg sidecar keeps its two seconds. It cannot be told to send a
//!   keyframe, so a viewer's ask is answered by the next scheduled one, and
//!   the phone's decoder, once it has dropped frames, waits for a keyframe.
//!
//! The setting off (the default) changes nothing: a keyframe every
//! [`PERIODIC_SECS`] seconds. `POCKETDESK_KEYFRAMES=smooth` or `periodic`
//! overrides the setting, for measuring one against the other.

use std::sync::atomic::{AtomicBool, Ordering};

/// The environment variable that overrides the setting.
pub const VAR: &str = "POCKETDESK_KEYFRAMES";

/// Seconds between keyframes with the setting off.
pub const PERIODIC_SECS: u64 = 2;
/// The length of one wave of intra refresh, in seconds, for an encoder that
/// refreshes in waves: the most a spoiled picture stays so.
pub const REFRESH_SECS: u64 = 2;
/// Seconds between whole keyframes with the setting on, for an encoder that
/// refreshes in waves.
pub const SAFETY_SECS_REFRESHING: u64 = 30;
/// Seconds between whole keyframes with the setting on, for an encoder that
/// cannot: with nothing else to heal a spoiled picture, a shorter wait.
pub const SAFETY_SECS_PLAIN: u64 = 10;

static SMOOTH: AtomicBool = AtomicBool::new(false);

/// Set from the owner's setting when the server starts; read as each session
/// opens its encoder.
pub fn set_smooth(on: bool) {
    SMOOTH.store(on, Ordering::Relaxed);
}

/// Whether the encoders being opened should smooth their keyframes.
pub fn smooth() -> bool {
    smooth_by(
        std::env::var(VAR).ok().as_deref(),
        SMOOTH.load(Ordering::Relaxed),
    )
}

fn smooth_by(var: Option<&str>, set: bool) -> bool {
    match var.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("smooth") => true,
        Some("periodic") => false,
        _ => set,
    }
}

/// How an encoder is asked to keep its picture whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Frames between the whole keyframes the encoder is told to send, and
    /// the least between those it sends on its own.
    pub idr_every: u64,
    /// Frames in one wave of intra refresh, for an encoder that spreads a
    /// keyframe's work over the stream; `None` for one that sends whole
    /// keyframes only.
    pub refresh_period: Option<u64>,
}

impl Plan {
    /// The plan for an encoder at `fps`, by the process's setting.
    /// `refreshes` is whether the encoder can do intra refresh at all (a
    /// libx264 or NVENC context).
    pub fn new(fps: u32, refreshes: bool) -> Self {
        Self::choose(smooth(), fps, refreshes)
    }

    /// The plan with the setting taken as given: a setting that is off, or
    /// an encoder that cannot refresh, gets whole keyframes only.
    pub fn choose(smooth: bool, fps: u32, refreshes: bool) -> Self {
        let fps = u64::from(fps.max(1));
        match (smooth, refreshes) {
            (true, true) => Self {
                idr_every: fps * SAFETY_SECS_REFRESHING,
                refresh_period: Some(fps * REFRESH_SECS),
            },
            (true, false) => Self {
                idr_every: fps * SAFETY_SECS_PLAIN,
                refresh_period: None,
            },
            (false, _) => Self {
                idr_every: fps * PERIODIC_SECS,
                refresh_period: None,
            },
        }
    }

    /// The group-of-pictures length the encoder opens with: the refresh wave
    /// where it has one (the encoder's own keyint is the wave's period), else
    /// the distance between whole keyframes.
    pub fn gop(&self) -> u64 {
        self.refresh_period.unwrap_or(self.idr_every)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_overrides_the_setting_either_way() {
        assert!(!smooth_by(None, false));
        assert!(smooth_by(None, true));
        assert!(smooth_by(Some(" Smooth "), false));
        assert!(!smooth_by(Some("periodic"), true));
        assert!(
            smooth_by(Some("what"), true),
            "anything else is the setting"
        );
        assert!(!smooth_by(Some(""), false));
    }

    #[test]
    fn a_plan_spreads_the_work_only_where_the_encoder_can() {
        let off = Plan::choose(false, 60, true);
        assert_eq!(off.gop(), 120, "today's keyframe every two seconds");
        assert_eq!(off, Plan::choose(false, 60, false), "whatever the encoder");
        assert_eq!(off.refresh_period, None);
        let waves = Plan::choose(true, 60, true);
        assert_eq!(waves.gop(), 120, "the wave's period, not the safety net's");
        assert_eq!(waves.refresh_period, Some(120));
        assert_eq!(waves.idr_every, 1800);
        let plain = Plan::choose(true, 60, false);
        assert_eq!(plain.gop(), 600);
        assert_eq!(plain.refresh_period, None);
        assert_eq!(Plan::choose(true, 0, false).idr_every, 10, "no divide by 0");
    }
}

/// The cost of a keyframe as the encoders pay it, measured: an ignored test
/// that prints, for each in-process encoder with the setting off and on, the
/// size and the time of the keyframes against the other frames.
#[cfg(all(test, target_os = "linux", feature = "inproc"))]
mod cost {
    use std::time::Instant;

    use super::Plan;
    use crate::streaming::encoder_inproc::InProcessEncoder;
    use crate::streaming::encoder_vaapi::VaapiEncoder;
    use crate::streaming::{EncodedPacket, StreamingConfig, VideoFrame};

    const W: u32 = 1920;
    const H: u32 = 1080;
    const FPS: u32 = 60;
    const FRAMES: usize = 1800;
    /// A frame, part way, that a viewer that lost one asks a keyframe at.
    const ASK: usize = 900;
    /// The frames left out at the start: the first keyframe, the encoder warming up.
    const WARMUP: usize = 60;

    fn hash(mut x: u32) -> u32 {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^ (x >> 16)
    }

    /// A desktop of text, still, with a textured patch (a video, say) that
    /// moves and changes in every frame: a keyframe has the whole page to
    /// code, the frames between only the patch.
    fn page() -> Vec<u8> {
        let mut data = vec![255u8; (W * H * 4) as usize];
        let (cell_w, cell_h) = (9u32, 16u32);
        for row in 0..H / cell_h {
            for col in 0..W / cell_w {
                let glyph = hash(row * 1000 + col);
                if glyph.is_multiple_of(5) {
                    continue; // a space
                }
                for y in 0..10 {
                    for x in 0..6 {
                        if !hash(glyph ^ (y * 8 + x)).is_multiple_of(3) {
                            continue;
                        }
                        let at = (((row * cell_h + 3 + y) * W + col * cell_w + 1 + x) * 4) as usize;
                        data[at..at + 3].copy_from_slice(&[30, 30, 30]);
                    }
                }
            }
        }
        data
    }

    fn frame(page: &[u8], n: usize) -> VideoFrame {
        let mut data = page.to_vec();
        let (px, py) = (((n * 13) % 1500) as u32, ((n * 7) % 700) as u32);
        for y in 0..240 {
            for x in 0..320 {
                let v = hash((n as u32).wrapping_mul(7919) ^ (y * 640 + x)) as u8 | 0x30;
                let at = (((py + y) * W + px + x) * 4) as usize;
                data[at..at + 3].copy_from_slice(&[v, v / 2 + 40, 255 - v / 3]);
            }
        }
        VideoFrame {
            data,
            width: W,
            height: H,
            stride: W * 4,
            timestamp: Instant::now(),
            gpu: None,
            repeat: false,
        }
    }

    fn spread(v: &mut [f64]) -> String {
        if v.is_empty() {
            return "-".into();
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        format!("{:.1} / {:.1} / {:.1}", at(0.5), at(0.95), at(1.0))
    }

    /// (packet, the call's milliseconds) for each frame.
    fn run(
        mut encode: impl FnMut(usize, &VideoFrame) -> Option<EncodedPacket>,
    ) -> Vec<(bool, f64, f64)> {
        let page = page();
        let mut out = Vec::new();
        for n in 0..FRAMES {
            let f = frame(&page, n);
            let began = Instant::now();
            let packet = encode(n, &f).expect("a packet for each frame");
            let ms = began.elapsed().as_secs_f64() * 1000.0;
            if n >= WARMUP {
                out.push((packet.is_keyframe, packet.data.len() as f64 / 1024.0, ms));
            }
        }
        out
    }

    fn report(name: &str, rows: &[(bool, f64, f64)]) {
        for (kind, key) in [("keyframes", true), ("other frames", false)] {
            let mut kib: Vec<f64> = rows.iter().filter(|r| r.0 == key).map(|r| r.1).collect();
            let mut ms: Vec<f64> = rows.iter().filter(|r| r.0 == key).map(|r| r.2).collect();
            println!(
                "{name}: {kind} ({}): KiB {}; encode call ms {}",
                kib.len(),
                spread(&mut kib),
                spread(&mut ms)
            );
        }
        let mut all: Vec<f64> = rows.iter().map(|r| r.2).collect();
        println!("{name}: all frames, encode call ms {}", spread(&mut all));
        if let Some(r) = rows.get(ASK - WARMUP) {
            println!(
                "{name}: the frame asked for: keyframe {}, {:.1} KiB, {:.1} ms",
                r.0, r.1, r.2
            );
        }
    }

    #[test]
    #[ignore = "measures the encoders on this machine (VA-API and libx264); prints a table"]
    fn keyframe_cost() {
        let config = StreamingConfig {
            width: W,
            height: H,
            fps: FPS,
            bitrate_bps: 5_000_000,
            ..StreamingConfig::default()
        };
        for smooth in [false, true] {
            let mode = if smooth { "smooth" } else { "periodic" };
            match VaapiEncoder::with_plan(config.clone(), Plan::choose(smooth, FPS, false)) {
                Ok(mut e) => report(
                    &format!("VA-API {mode}"),
                    &run(|n, f| {
                        if n == ASK {
                            e.request_keyframe();
                        }
                        e.encode_frame(f).unwrap()
                    }),
                ),
                Err(e) => println!("VA-API {mode}: unavailable ({e:#})"),
            }
            match InProcessEncoder::with_smooth(config.clone(), smooth) {
                Ok(mut e) => report(
                    &format!("libx264 {mode}"),
                    &run(|n, f| {
                        if n == ASK {
                            e.request_keyframe();
                        }
                        e.encode_frame(f).unwrap()
                    }),
                ),
                Err(e) => println!("libx264 {mode}: unavailable ({e:#})"),
            }
        }
    }
}
