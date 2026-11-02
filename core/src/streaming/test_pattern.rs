//! A moving picture that stands in for the screen (BG): with
//! `POCKETDESK_TEST_PATTERN` set, capture makes this instead of reading the
//! desktop. For benches that need a picture that changes every frame (an
//! idle desktop repeats one frame a second, which measures nothing), and
//! for throwaway servers whose sessions should show no one's screen. The
//! server warns at start when it is set.
//!
//! The picture is a still gradient with a bar that moves across it, a step
//! each frame, and a strip of blocks counting the frame number in binary, so
//! every frame differs and a dropped one can be told from the picture.
//!
//! With the variable at `probe` (the input-to-picture latency probe, see
//! [`super::latency_probe`]) the picture's last rows are a dark band, kept
//! clear of the bar and the strip, and a bright square in it at the last
//! absolute pointer move a session sent; the server then injects none of its
//! sessions' input.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::capture::CaptureSession;
use super::latency_probe;
use super::latest_frame::FrameSender;
use super::{StreamingConfig, VideoFrame};

/// The seam's variable.
pub const VAR: &str = latency_probe::PATTERN_VAR;

/// How far the bar moves each frame, in pixels.
const BAR_STEP: u32 = 8;
/// The bar's width, in pixels.
const BAR_WIDTH: u32 = 64;
/// The counting strip: this many blocks of this many pixels.
const COUNT_BITS: u32 = 16;
const COUNT_BLOCK: u32 = 16;

/// Whether the pattern stands in for the screen: the variable set to
/// anything but empty, `0`, `false`, `off` or `no`.
pub fn wanted() -> bool {
    wanted_by(std::env::var_os(VAR))
}

fn wanted_by(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|v| {
        let v = v.to_string_lossy().trim().to_ascii_lowercase();
        !matches!(v.as_str(), "" | "0" | "false" | "off" | "no")
    })
}

/// Frame `n` of a `width` x `height` pattern, BGRA (sides made even, at
/// least 64 x 64).
pub fn frame(width: u32, height: u32, n: u64) -> VideoFrame {
    let mut frame = background(width, height);
    draw(&mut frame, n);
    frame
}

/// Frame `n` as [`frame`], with the probe's band and, once a pointer is
/// known (`pointer`: the last absolute move, `x_norm` and `y_norm`), its
/// marker.
pub fn frame_probe(width: u32, height: u32, n: u64, pointer: Option<(u16, u16)>) -> VideoFrame {
    let mut frame = frame(width, height, n);
    draw_probe(&mut frame, pointer);
    frame
}

/// The probe's dark band over the picture's last rows (over the bar too),
/// and the marker in it at `pointer`'s `x_norm`, white. The marker follows
/// the pointer's column only; the band is where it moves.
fn draw_probe(frame: &mut VideoFrame, pointer: Option<(u16, u16)>) {
    let (width, height, stride) = (frame.width, frame.height, frame.stride);
    let dark = latency_probe::DARK;
    for y in latency_probe::band_top(height)..height {
        let row = (y * stride) as usize;
        for x in 0..width {
            let px = row + (x * 4) as usize;
            frame.data[px..px + 4].copy_from_slice(&[dark, dark, dark, 255]);
        }
    }
    if let Some((x_norm, _)) = pointer {
        let left = latency_probe::marker_left(width, x_norm);
        let top = latency_probe::marker_top(height);
        for y in top..(top + latency_probe::SIZE).min(height) {
            let row = (y * stride) as usize;
            for x in left..(left + latency_probe::SIZE).min(width) {
                let px = row + (x * 4) as usize;
                frame.data[px..px + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
}

/// The still gradient every frame starts from.
fn background(width: u32, height: u32) -> VideoFrame {
    let width = (width.max(64) + 1) & !1;
    let height = (height.max(64) + 1) & !1;
    let stride = width * 4;
    let mut data = vec![0u8; (stride * height) as usize];
    for y in 0..height {
        let row = (y * stride) as usize;
        for x in 0..width {
            let px = row + (x * 4) as usize;
            let (b, g, r) = ((x * 255 / width) as u8, (y * 255 / height) as u8, 96);
            data[px..px + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    VideoFrame {
        data,
        width,
        height,
        stride,
        timestamp: Instant::now(),
        gpu: None,
    }
}

/// Frame `n`'s moving bar and counting strip, over `frame` (a copy of the
/// background).
fn draw(frame: &mut VideoFrame, n: u64) {
    let (width, height, stride) = (frame.width, frame.height, frame.stride);
    let travel = width + BAR_WIDTH;
    let bar_end = ((n * u64::from(BAR_STEP)) % u64::from(travel)) as u32;
    let bar_start = bar_end.saturating_sub(BAR_WIDTH);
    let bar_end = bar_end.min(width);
    for y in 0..height {
        let row = (y * stride) as usize;
        for x in bar_start..bar_end {
            let px = row + (x * 4) as usize;
            frame.data[px..px + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    for y in 0..COUNT_BLOCK.min(height) {
        let row = (y * stride) as usize;
        for x in 0..(COUNT_BITS * COUNT_BLOCK).min(width) {
            if (bar_start..bar_end).contains(&x) {
                continue;
            }
            // Bit (x / block) of n, most significant first.
            let bit = COUNT_BITS - 1 - x / COUNT_BLOCK;
            let (b, g, r) = if (n >> bit) & 1 == 1 {
                (40, 220, 40)
            } else {
                (20, 20, 20)
            };
            let px = row + (x * 4) as usize;
            frame.data[px..px + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    frame.timestamp = Instant::now();
}

/// Make the pattern at `config`'s size and frame rate into `frame_tx`
/// until `cancel`, or until the encoder is gone.
pub fn start(
    config: StreamingConfig,
    frame_tx: FrameSender<VideoFrame>,
    cancel: CancellationToken,
) -> CaptureSession {
    tracing::warn!("{VAR} is set: a test pattern stands in for the screen");
    let period = Duration::from_secs_f64(1.0 / f64::from(config.fps.max(1)));
    let (width, height) = (config.width, config.height);
    let token = cancel.clone();
    let probe = latency_probe::mode();
    if probe {
        tracing::warn!("{VAR}=probe: the pattern draws the latency marker; no input is injected");
    }
    std::thread::Builder::new()
        .name("test-pattern".into())
        .spawn(move || {
            // Drawn once; each frame copies it and draws what moves.
            let base = background(width, height);
            let mut next = Instant::now();
            for n in 0u64.. {
                let mut current = VideoFrame {
                    data: base.data.clone(),
                    timestamp: Instant::now(),
                    gpu: None,
                    ..base
                };
                draw(&mut current, n);
                if probe {
                    draw_probe(&mut current, latency_probe::POINTER.get());
                }
                if token.is_cancelled() || frame_tx.try_send(current).is_err() {
                    break;
                }
                next += period;
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    // Behind: start again from now rather than burst.
                    next = now;
                }
            }
        })
        .expect("spawn the test pattern's thread");
    CaptureSession::new(config, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seam_is_on_for_any_value_but_empty_or_0() {
        assert!(wanted_by(Some("1".into())));
        assert!(wanted_by(Some("yes".into())));
        assert!(!wanted_by(Some("".into())));
        assert!(!wanted_by(Some("0".into())));
        assert!(!wanted_by(Some("false".into())));
        assert!(!wanted_by(Some(" Off ".into())));
        assert!(!wanted_by(Some("no".into())));
        assert!(!wanted_by(None));
    }

    #[test]
    fn every_frame_differs_and_has_the_asked_size() {
        let a = frame(320, 180, 0);
        let b = frame(320, 180, 1);
        assert_eq!((a.width, a.height, a.stride), (320, 180, 1280));
        assert_eq!(a.data.len(), 1280 * 180);
        assert_ne!(a.data, b.data, "the bar moved and the count changed");
        assert_eq!(
            frame(320, 180, 7).data,
            frame(320, 180, 7).data,
            "the same frame twice"
        );
        let odd = frame(101, 63, 0);
        assert_eq!((odd.width, odd.height), (102, 64), "even, at least 64");
    }

    #[test]
    fn the_count_strip_says_the_frame_number() {
        let f = frame(320, 180, 0b101);
        // The last three blocks of the strip, at y = 0: 1, 0, 1.
        let block = |bit: u32| {
            let x = (COUNT_BITS - 1 - bit) * COUNT_BLOCK + COUNT_BLOCK / 2;
            f.data[(x * 4 + 1) as usize] // green
        };
        assert_eq!((block(2), block(1), block(0)), (220, 20, 220));
    }

    /// The luma the probe's finder reads, from a BGRA frame (the pattern's
    /// band and marker are grey, so one channel is their luma).
    fn luma(f: &VideoFrame) -> Vec<u8> {
        f.data.as_chunks::<4>().0.iter().map(|px| px[1]).collect()
    }

    #[test]
    fn the_probe_draws_a_marker_the_finder_finds_whatever_the_bar_does() {
        let (w, h) = (640, 360);
        for x_norm in [0u16, 1000, 32768, 50000, u16::MAX] {
            // Every bar place, and so the bar over the marker's columns.
            for n in (0..90).step_by(7) {
                let f = frame_probe(w, h, n, Some((x_norm, 7)));
                assert_eq!(
                    latency_probe::find(&luma(&f), w as usize, h as usize),
                    Some(latency_probe::marker_left(w, x_norm)),
                    "x_norm {x_norm}, frame {n}"
                );
            }
        }
    }

    #[test]
    fn the_probe_shows_no_marker_before_any_pointer_and_leaves_the_rest_alone() {
        let (w, h) = (640, 360);
        let f = frame_probe(w, h, 3, None);
        assert_eq!(latency_probe::find(&luma(&f), w as usize, h as usize), None);
        // Above the band the picture is the pattern's own, bar and strip.
        let plain = frame(w, h, 3);
        let above = (latency_probe::band_top(h) * plain.stride) as usize;
        assert_eq!(f.data[..above], plain.data[..above]);
        assert_ne!(f.data[above..], plain.data[above..], "the band is dark");
        // Without the probe the pattern has no band: its last rows are the
        // gradient.
        assert_ne!(plain.data[plain.data.len() - 4..], [16, 16, 16, 255]);
    }

    #[test]
    fn a_small_pattern_keeps_its_strip_clear_of_the_band() {
        // 64 rows: the counting strip (16) and the band (48) just fit.
        let f = frame_probe(64, 64, 0b101, Some((0, 0)));
        let plain = frame(64, 64, 0b101);
        let strip = (COUNT_BLOCK * plain.stride) as usize;
        assert_eq!(f.data[..strip], plain.data[..strip], "the strip is whole");
        assert_eq!(
            latency_probe::find(&luma(&f), 64, 64),
            Some(0),
            "found at the smallest size"
        );
    }

    #[tokio::test]
    async fn it_runs_at_its_frame_rate_until_cancelled() {
        let (tx, mut rx) = super::super::latest_frame::channel();
        let cancel = CancellationToken::new();
        let config = StreamingConfig {
            width: 128,
            height: 72,
            fps: 50,
            ..StreamingConfig::default()
        };
        let session = start(config, tx, cancel.clone());
        let mut seen = 0;
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(400) {
            if tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .ok()
                .flatten()
                .is_some()
            {
                seen += 1;
            }
        }
        assert!(
            seen >= 10,
            "about 20 frames in 400 ms at 50 fps, saw {seen}"
        );
        drop(session);
        cancel.cancel();
    }
}
