//! X11 screen capture via xcap (XShm screenshots).
//!
//! Used as a fallback when running on X11 or when the Wayland portal is unavailable.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::{StreamingConfig, VideoFrame};

/// Convert RGBA to BGRA in-place.
fn rgba_to_bgra(data: &mut [u8]) {
    for chunk in data.as_chunks_mut::<4>().0 {
        chunk.swap(0, 2);
    }
}

/// Capture a single frame via xcap and convert to VideoFrame.
fn capture_frame(monitor_idx: usize) -> Result<VideoFrame> {
    let monitors = xcap::Monitor::all().context("Failed to enumerate X11 monitors")?;
    let monitor = monitors
        .get(monitor_idx)
        .context("Monitor index out of range")?;

    let image = monitor
        .capture_image()
        .context("Failed to capture X11 screenshot")?;

    let width = image.width();
    let height = image.height();
    let stride = width * 4; // 4 bytes per pixel (RGBA)

    // xcap returns RGBA, but our encoder expects BGRA (PipeWire's default).
    // Convert in-place.
    let mut data = image.into_raw();
    rgba_to_bgra(&mut data);

    Ok(VideoFrame {
        data,
        width,
        height,
        stride,
        timestamp: Instant::now(),
        gpu: None,
    })
}

/// Start X11 screen capture using xcap.
///
/// Spawns a tokio task that captures frames at the target FPS.
/// Frames are sent through `frame_tx`.
/// Returns a join handle that can be aborted via `cancel`.
pub async fn start_x11_capture(
    config: StreamingConfig,
    frame_tx: crate::streaming::latest_frame::FrameSender<VideoFrame>,
    cancel: CancellationToken,
) -> Result<()> {
    info!(
        "Starting X11 screen capture: {}x{}@{}fps",
        config.width, config.height, config.fps
    );

    let frame_interval = Duration::from_secs_f64(1.0 / config.fps as f64);
    let monitor_idx = config.monitor_index as usize;
    info!("Using X11 monitor index: {}", monitor_idx);

    tokio::spawn(async move {
        let mut frame_count: u64 = 0;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("X11 capture cancelled");
                    break;
                }
                _ = tokio::time::sleep(frame_interval) => {
                    match capture_frame(monitor_idx) {
                        Ok(frame) => {
                            match frame_tx.try_send(frame) {
                                Ok(_) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    debug!("Frame channel closed, stopping X11 capture");
                                    break;
                                }
                            }
                            frame_count += 1;
                            if frame_count.is_multiple_of(30) {
                                debug!("X11 captured frame #{frame_count}");
                            }
                        }
                        Err(e) => {
                            error!("X11 capture error: {e}");
                        }
                    }
                }
            }
        }
        info!("X11 capture loop exited");
    });

    Ok(())
}

/// Check if X11 capture is available.
pub fn check_x11_availability() -> bool {
    // Try to enumerate monitors — if this fails, X11 is not available
    match xcap::Monitor::all() {
        Ok(monitors) => !monitors.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rgba_to_bgra_conversion() {
        let mut data = vec![0x11, 0x22, 0x33, 0xFF, 0x44, 0x55, 0x66, 0xFF];
        rgba_to_bgra(&mut data);
        // First pixel: RGBA(0x11,0x22,0x33) → BGRA(0x33,0x22,0x11)
        assert_eq!(&data[0..4], &[0x33, 0x22, 0x11, 0xFF]);
        // Second pixel: RGBA(0x44,0x55,0x66) → BGRA(0x66,0x55,0x44)
        assert_eq!(&data[4..8], &[0x66, 0x55, 0x44, 0xFF]);
    }
}
