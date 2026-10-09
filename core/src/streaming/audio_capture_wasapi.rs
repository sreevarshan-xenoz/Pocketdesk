//! WASAPI loopback capture: what the default output device is playing, as
//! the same 48 kHz interleaved `i16` [`PcmBuffer`]s the PipeWire backend
//! emits, cut into exact Opus frames.
//!
//! The shared-mode mix format is whatever the device runs at (usually 32-bit
//! float, often 48 kHz, sometimes 44.1 kHz with more than two channels), so a
//! [`Converter`] downmixes to stereo, resamples linearly and reframes. Loopback
//! delivers nothing while nothing plays; that silence is not synthesized
//! (roadmap 2054) — the audio task simply sends nothing.
//!
//! When the default output changes (headphones plugged in) or the endpoint is
//! invalidated, the client is rebuilt on the new default device.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};

use super::audio_capture::PcmBuffer;

const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// How often the capture thread drains the device and checks for a new
/// default output.
const POLL: Duration = Duration::from_millis(10);
const DEFAULT_DEVICE_CHECK: Duration = Duration::from_secs(2);

/// How a device's samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SampleFormat {
    F32,
    I16,
    I32,
}

/// Downmix to stereo, resample linearly, and cut exact output frames.
pub(crate) struct Converter {
    in_channels: usize,
    format: SampleFormat,
    /// Input frames advanced per output frame.
    step: f64,
    /// Position of the next output frame within `src`, in input frames.
    pos: f64,
    /// Stereo input frames not yet fully consumed.
    src: Vec<[f32; 2]>,
    /// Interleaved stereo output not yet a full frame.
    pending: Vec<i16>,
    /// Interleaved samples per output frame.
    frame_len: usize,
}

impl Converter {
    pub(crate) fn new(
        in_rate: u32,
        in_channels: usize,
        format: SampleFormat,
        out_rate: u32,
        frame_ms: u32,
    ) -> Self {
        Self {
            in_channels: in_channels.max(1),
            format,
            step: f64::from(in_rate) / f64::from(out_rate),
            pos: 0.0,
            src: Vec::new(),
            pending: Vec::new(),
            frame_len: (out_rate * frame_ms / 1000) as usize * 2,
        }
    }

    fn sample(&self, bytes: &[u8], index: usize) -> f32 {
        match self.format {
            SampleFormat::F32 => {
                let b = &bytes[index * 4..index * 4 + 4];
                f32::from_le_bytes([b[0], b[1], b[2], b[3]])
            }
            SampleFormat::I16 => {
                let b = &bytes[index * 2..index * 2 + 2];
                f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0
            }
            SampleFormat::I32 => {
                let b = &bytes[index * 4..index * 4 + 4];
                i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
            }
        }
    }

    fn bytes_per_sample(&self) -> usize {
        match self.format {
            SampleFormat::I16 => 2,
            SampleFormat::F32 | SampleFormat::I32 => 4,
        }
    }

    /// Feed raw device bytes (whole frames); returns every completed frame.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<Vec<i16>> {
        let frames = bytes.len() / (self.bytes_per_sample() * self.in_channels);
        for f in 0..frames {
            let base = f * self.in_channels;
            let left = self.sample(bytes, base);
            let right = if self.in_channels > 1 {
                self.sample(bytes, base + 1)
            } else {
                left
            };
            self.src.push([left, right]);
        }
        self.resample()
    }

    /// Feed `frames` frames of digital silence (a SILENT-flagged packet).
    pub(crate) fn push_silence(&mut self, frames: usize) -> Vec<Vec<i16>> {
        self.src.extend(std::iter::repeat_n([0.0, 0.0], frames));
        self.resample()
    }

    fn resample(&mut self) -> Vec<Vec<i16>> {
        let to_i16 = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        while self.pos + 1.0 < self.src.len() as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            for ch in 0..2 {
                let a = self.src[i][ch];
                let b = self.src[i + 1][ch];
                self.pending.push(to_i16(a + (b - a) * t));
            }
            self.pos += self.step;
        }
        let consumed = (self.pos.floor() as usize).min(self.src.len());
        self.src.drain(..consumed);
        self.pos -= consumed as f64;

        let mut out = Vec::new();
        while self.pending.len() >= self.frame_len {
            out.push(self.pending.drain(..self.frame_len).collect());
        }
        out
    }
}

/// The parts of a mix format the converter needs.
fn describe(format: *const WAVEFORMATEX) -> Result<(u32, usize, SampleFormat)> {
    // SAFETY: GetMixFormat returned a valid WAVEFORMATEX (or the extensible
    // form, which begins with one); the struct is packed, so read unaligned.
    let base = unsafe { std::ptr::read_unaligned(format) };
    let tag = base.wFormatTag;
    let bits = base.wBitsPerSample;
    let float = if tag == WAVE_FORMAT_EXTENSIBLE {
        let ext = unsafe { std::ptr::read_unaligned(format as *const WAVEFORMATEXTENSIBLE) };
        let sub = ext.SubFormat;
        sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        u32::from(tag) == WAVE_FORMAT_IEEE_FLOAT
    };
    let sample = match (float, bits) {
        (true, 32) => SampleFormat::F32,
        (false, 16) => SampleFormat::I16,
        (false, 32) => SampleFormat::I32,
        _ => bail!("unsupported mix format (tag {tag:#x}, {bits} bits)"),
    };
    Ok((base.nSamplesPerSec, usize::from(base.nChannels), sample))
}

/// One live loopback client on the default output device.
struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    block_align: usize,
    device_id: String,
    converter: Converter,
}

fn default_device_id(enumerator: &IMMDeviceEnumerator) -> Result<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
    let id = unsafe { device.GetId() }?;
    let text = unsafe { id.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(id.0 as *const _)) };
    Ok(text)
}

fn open_loopback(
    enumerator: &IMMDeviceEnumerator,
    sample_rate: u32,
    frame_ms: u32,
) -> Result<Loopback> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .context("no default audio output device")?;
    let device_id = default_device_id(enumerator)?;
    let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
    let format = unsafe { client.GetMixFormat() }?;
    let described = describe(format);
    let block_align = usize::from(unsafe { std::ptr::read_unaligned(format) }.nBlockAlign);
    let init = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            2_000_000, // 200 ms buffer, in 100 ns units
            0,
            format,
            None,
        )
    };
    unsafe { CoTaskMemFree(Some(format as *const _)) };
    let (in_rate, in_channels, sample) = described?;
    init.context("initializing the loopback client")?;
    if in_rate != sample_rate {
        info!(
            in_rate,
            sample_rate, "Output device runs at another rate; resampling"
        );
    }
    let capture: IAudioCaptureClient = unsafe { client.GetService() }?;
    unsafe { client.Start() }?;
    info!(
        in_rate,
        in_channels,
        ?sample,
        "WASAPI loopback capture started"
    );
    Ok(Loopback {
        client,
        capture,
        block_align,
        device_id,
        converter: Converter::new(in_rate, in_channels, sample, sample_rate, frame_ms),
    })
}

/// Drain everything the device has, returning completed output frames.
fn drain(loopback: &mut Loopback) -> Result<Vec<Vec<i16>>> {
    let mut out = Vec::new();
    loop {
        let available = unsafe { loopback.capture.GetNextPacketSize() }?;
        if available == 0 {
            return Ok(out);
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        unsafe {
            loopback
                .capture
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
        }?;
        let frames_us = frames as usize;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
            out.extend(loopback.converter.push_silence(frames_us));
        } else {
            // SAFETY: GetBuffer handed out `frames` frames of `block_align` bytes.
            let bytes =
                unsafe { std::slice::from_raw_parts(data, frames_us * loopback.block_align) };
            out.extend(loopback.converter.push(bytes));
        }
        unsafe { loopback.capture.ReleaseBuffer(frames) }?;
    }
}

/// Spawn the loopback thread; it runs until `cancel` fires. Errors only when
/// no loopback client can be opened at all.
pub(super) fn spawn_capture_thread(
    sample_rate: u32,
    channels: u16,
    frame_duration_ms: u32,
    pcm_tx: mpsc::Sender<PcmBuffer>,
    cancel: CancellationToken,
) -> Result<()> {
    if channels != 2 {
        bail!("WASAPI loopback produces stereo; {channels} channels requested");
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    std::thread::Builder::new()
        .name("wasapi-loopback".into())
        .spawn(move || {
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let enumerator: IMMDeviceEnumerator =
                match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
                    Ok(e) => e,
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow::anyhow!("audio device enumerator: {e}")));
                        return;
                    }
                };
            let mut loopback = match open_loopback(&enumerator, sample_rate, frame_duration_ms) {
                Ok(l) => {
                    let _ = ready_tx.send(Ok(()));
                    l
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mut last_check = Instant::now();
            while !cancel.is_cancelled() {
                let rebuild = match drain(&mut loopback) {
                    Ok(frames) => {
                        for data in frames {
                            let buffer = PcmBuffer {
                                data,
                                sample_rate,
                                channels,
                                timestamp: Instant::now(),
                            };
                            if pcm_tx.blocking_send(buffer).is_err() {
                                debug!("PCM channel closed, stopping WASAPI capture");
                                cancel.cancel();
                                break;
                            }
                        }
                        last_check.elapsed() >= DEFAULT_DEVICE_CHECK && {
                            last_check = Instant::now();
                            default_device_id(&enumerator).is_ok_and(|id| id != loopback.device_id)
                        }
                    }
                    Err(e) => {
                        warn!("WASAPI loopback lost its device ({e:#}); reopening");
                        true
                    }
                };
                if rebuild {
                    let _ = unsafe { loopback.client.Stop() };
                    match open_loopback(&enumerator, sample_rate, frame_duration_ms) {
                        Ok(next) => {
                            info!("Following the new default audio output");
                            loopback = next;
                        }
                        Err(e) => {
                            debug!("No usable output device yet ({e:#})");
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    }
                }
                std::thread::sleep(POLL);
            }
            let _ = unsafe { loopback.client.Stop() };
            info!("WASAPI loopback capture stopped");
        })
        .context("failed to spawn the WASAPI capture thread")?;

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(result) => result,
        Err(_) => bail!("WASAPI loopback did not start within 3 s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn same_rate_stereo_float_becomes_exact_20ms_frames() {
        let mut conv = Converter::new(48_000, 2, SampleFormat::F32, 48_000, 20);
        // 1.5 frames of a ramp: one complete frame out, the rest pending.
        let input: Vec<f32> = (0..1440 * 2).map(|i| (i % 2) as f32 * 0.5).collect();
        let frames = conv.push(&f32_bytes(&input));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 960 * 2);
        // Left 0.0, right 0.5 survive as samples.
        assert_eq!(frames[0][0], 0);
        assert_eq!(frames[0][1], (0.5f32 * 32767.0).round() as i16);
    }

    #[test]
    fn a_44k1_device_yields_48k_worth_of_samples() {
        let mut conv = Converter::new(44_100, 2, SampleFormat::F32, 48_000, 20);
        let one_second = vec![0.25f32; 44_100 * 2];
        let frames = conv.push(&f32_bytes(&one_second));
        // One second in is ~50 frames out (the last may still be pending).
        assert!((49..=50).contains(&frames.len()), "{}", frames.len());
        assert!(frames.iter().all(|f| f.iter().all(|&s| s == 8192)));
    }

    #[test]
    fn mono_is_duplicated_and_surround_keeps_front_left_right() {
        let mut mono = Converter::new(48_000, 1, SampleFormat::F32, 48_000, 20);
        let frames = mono.push(&f32_bytes(&vec![1.0; 961]));
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().all(|&s| s == 32767));

        let mut surround = Converter::new(48_000, 6, SampleFormat::F32, 48_000, 20);
        let mut input = Vec::new();
        for _ in 0..961 {
            input.extend([0.5, -0.5, 1.0, 1.0, 1.0, 1.0]);
        }
        let frames = surround.push(&f32_bytes(&input));
        assert_eq!(&frames[0][..2], &[16384, -16384]);
    }

    #[test]
    fn integer_formats_and_clipping_convert_to_i16() {
        let mut i16s = Converter::new(48_000, 2, SampleFormat::I16, 48_000, 20);
        let bytes: Vec<u8> = (0..961 * 2)
            .flat_map(|_| (-32768i16).to_le_bytes())
            .collect();
        assert_eq!(i16s.push(&bytes)[0][0], -32767);

        let mut loud = Converter::new(48_000, 2, SampleFormat::F32, 48_000, 20);
        let frames = loud.push(&f32_bytes(&vec![3.0; 961 * 2]));
        assert!(
            frames[0].iter().all(|&s| s == 32767),
            "clamped, not wrapped"
        );
    }

    #[test]
    fn silent_packets_still_advance_time() {
        let mut conv = Converter::new(48_000, 2, SampleFormat::F32, 48_000, 20);
        let frames = conv.push_silence(961);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().all(|&s| s == 0));
    }

    /// Live: open loopback on the default output and stop it again. Plays
    /// nothing, so it proves the device opens rather than that audio flows.
    #[test]
    #[ignore = "needs an audio output device"]
    fn loopback_opens_on_the_default_output() {
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        spawn_capture_thread(48_000, 2, 20, tx, cancel.clone()).expect("loopback opens");
        cancel.cancel();
    }
}
