//! PipeWire audio loopback capture for system audio streaming.
//!
//! Captures the system audio output (what you hear) via PipeWire's audio
//! loopback interface and provides PCM frames for Opus encoding.
//!
//! The implementation creates a PipeWire audio stream that connects to the
//! default audio sink as a loopback, receiving the mixed system audio.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pipewire::stream::StreamFlags;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::audio_capture::PcmBuffer;

/// Spawn the PipeWire capture thread; it runs until `cancel` fires.
pub(super) fn spawn_capture_thread(
    sample_rate: u32,
    channels: u16,
    frame_duration_ms: u32,
    pcm_tx: mpsc::Sender<PcmBuffer>,
    cancel: CancellationToken,
) -> Result<()> {
    let pw_cancel = cancel;
    std::thread::Builder::new()
        .name("pipewire-audio-capture".into())
        .spawn(move || {
            if let Err(e) = run_pipewire_audio_capture(
                sample_rate,
                channels,
                frame_duration_ms,
                pcm_tx,
                pw_cancel,
            ) {
                error!("PipeWire audio capture thread exited with error: {e}");
            } else {
                info!("PipeWire audio capture thread exited normally");
            }
        })
        .context("Failed to spawn PipeWire audio capture thread")?;

    Ok(())
}

/// Run the PipeWire audio capture loop in a dedicated thread.
fn run_pipewire_audio_capture(
    sample_rate: u32,
    channels: u16,
    frame_duration_ms: u32,
    pcm_tx: mpsc::Sender<PcmBuffer>,
    cancel: CancellationToken,
) -> Result<()> {
    info!("PipeWire audio capture thread started");

    pipewire::init();

    let mainloop = pipewire::main_loop::MainLoopBox::new(None)
        .context("Failed to create PipeWire main loop")?;

    let context = pipewire::context::ContextBox::new(mainloop.loop_(), None)
        .context("Failed to create PipeWire context")?;

    let core = context
        .connect(None)
        .context("Failed to connect to PipeWire daemon")?;

    // Stream properties for audio loopback capture.
    // Using Audio/Sink as the media type with the loopback flag captures
    // the system audio output (what you hear).
    let stream_props = pipewire::properties::properties! {
        *pipewire::keys::MEDIA_TYPE => "Audio",
        *pipewire::keys::MEDIA_CATEGORY => "Capture",
        *pipewire::keys::MEDIA_ROLE => "Music",
        *pipewire::keys::NODE_NAME => "pocketdesk-audio-capture",
        *pipewire::keys::STREAM_CAPTURE_SINK => "true",
    };

    let stream = pipewire::stream::StreamBox::new(&core, "pocketdesk-audio-capture", stream_props)
        .context("Failed to create PipeWire audio stream")?;

    // Calculate frames per buffer
    let _frame_bytes =
        (sample_rate as usize * frame_duration_ms as usize) / 1000 * channels as usize * 2; // s16 = 2 bytes
    let frame_count = AtomicU64::new(0);
    let has_audio = AtomicBool::new(false);

    let user_data = AudioStreamData {
        pcm_tx,
        cancel: cancel.clone(),
        sample_rate,
        channels,
        frame_count: &frame_count,
        has_audio: &has_audio,
    };

    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .process(|stream, ud| {
            on_audio_process(stream, ud);
        })
        .register()
        .context("Failed to register audio stream listener")?;

    // Let PipeWire auto-negotiate the audio format (defaults to S16LE @ 48kHz).
    // Stream will adapt to the system's audio pipeline.
    let mut params: [&libspa::pod::Pod; 0] = [];
    stream
        .connect(
            libspa::utils::Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .context("Failed to connect audio stream")?;

    stream
        .set_active(true)
        .context("Failed to activate audio stream")?;

    info!("PipeWire audio stream connected and active");

    loop {
        mainloop
            .loop_()
            .iterate(Duration::from_millis(frame_duration_ms as u64));
        if cancel.is_cancelled() {
            info!("Cancellation received, exiting PipeWire audio main loop");
            break;
        }
    }

    let _ = stream.set_active(false);
    let _ = stream.disconnect();

    info!("PipeWire audio capture thread shut down");
    Ok(())
}

/// User data for the PipeWire audio stream callbacks.
struct AudioStreamData<'a> {
    pcm_tx: mpsc::Sender<PcmBuffer>,
    cancel: CancellationToken,
    sample_rate: u32,
    channels: u16,
    frame_count: &'a AtomicU64,
    has_audio: &'a AtomicBool,
}

/// Handle the process callback — dequeue audio buffers and send PCM data.
fn on_audio_process(stream: &pipewire::stream::Stream, ud: &mut AudioStreamData) {
    let mut buffer = match stream.dequeue_buffer() {
        Some(b) => b,
        None => return,
    };

    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }

    let data = &mut datas[0];

    let (size, _stride) = {
        let chunk = data.chunk();
        (chunk.size() as usize, chunk.stride())
    };

    if size == 0 {
        return;
    }

    let Some(data_slice) = data.data() else {
        return;
    };

    if data_slice.is_empty() {
        return;
    }

    // PipeWire gives us interleaved s16 samples.
    // Cap the read to the actual available data length.
    let byte_count = size.min(data_slice.len());
    // Ensure we read an even number of bytes (each sample is 2 bytes).
    let byte_count = byte_count & !1;
    let pcm_data: Vec<i16> = data_slice[..byte_count]
        .chunks(2)
        .map(|chunk| i16::from_ne_bytes([chunk[0], chunk[1]]))
        .collect();

    let count = ud.frame_count.fetch_add(1, Ordering::Relaxed);
    if count == 0 && !pcm_data.is_empty() {
        ud.has_audio.store(true, Ordering::Relaxed);
        info!("First audio frame captured: {} samples", pcm_data.len());
    }

    let buffer = PcmBuffer {
        data: pcm_data,
        sample_rate: ud.sample_rate,
        channels: ud.channels,
        timestamp: Instant::now(),
    };

    if ud.pcm_tx.blocking_send(buffer).is_err() {
        debug!("PCM channel closed, cancelling audio capture");
        ud.cancel.cancel();
    }
}
