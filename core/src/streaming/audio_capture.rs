//! Desktop audio capture: what the user hears, as 48 kHz PCM for Opus.
//!
//! [`start_audio_capture`] picks the OS backend. On Linux that is a PipeWire
//! loopback stream on the default sink (`audio_capture_pipewire.rs`); other
//! targets report that no backend exists, and the streamer carries on without
//! audio.

use std::time::Instant;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// A buffer of PCM audio data (16-bit signed, interleaved stereo).
#[derive(Debug, Clone)]
pub struct PcmBuffer {
    pub data: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp: Instant,
}

/// Desktop audio capture session; dropping it stops the capture.
pub struct AudioCaptureSession {
    cancel: CancellationToken,
}

impl Drop for AudioCaptureSession {
    fn drop(&mut self) {
        info!("AudioCaptureSession dropped, cancelling capture");
        self.cancel.cancel();
    }
}

/// Start system audio loopback capture.
///
/// PCM frames are sent through the provided `mpsc::Sender<PcmBuffer>` channel
/// at the configured frame interval. Errors when this OS has no capture
/// backend; the caller treats that as "no desktop audio", not as a failed
/// session.
pub async fn start_audio_capture(
    sample_rate: u32,
    channels: u16,
    frame_duration_ms: u32,
    pcm_tx: mpsc::Sender<PcmBuffer>,
    cancel: CancellationToken,
) -> Result<AudioCaptureSession> {
    info!(
        "Starting desktop audio capture: {} Hz, {} ch, {} ms frames",
        sample_rate, channels, frame_duration_ms
    );

    #[cfg(target_os = "linux")]
    {
        super::audio_capture_pipewire::spawn_capture_thread(
            sample_rate,
            channels,
            frame_duration_ms,
            pcm_tx,
            cancel.clone(),
        )?;
        Ok(AudioCaptureSession { cancel })
    }
    #[cfg(not(target_os = "linux"))]
    {
        drop((pcm_tx, cancel));
        anyhow::bail!("this build has no desktop audio capture backend for this OS")
    }
}
