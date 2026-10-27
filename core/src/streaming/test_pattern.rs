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

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::capture::CaptureSession;
use super::latest_frame::FrameSender;
use super::{StreamingConfig, VideoFrame};

/// The seam's variable.
pub const VAR: &str = "POCKETDESK_TEST_PATTERN";

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
                    ..base
                };
                draw(&mut current, n);
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
