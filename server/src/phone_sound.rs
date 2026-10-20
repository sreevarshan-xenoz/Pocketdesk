//! The phone's sound on this computer (K5): a cast's Opus packets (48 kHz
//! stereo, what the phone's apps play) decoded and played on the default
//! output, for the life of the session. Windows plays through WASAPI,
//! Linux through `pw-cat`; with `POCKETDESK_PHONE_AUDIO_FILE` set, the
//! sound goes to that WAV file instead (a test seam).

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::mpsc::Receiver;
use tracing::{info, warn};

use pocketdesk_core::streaming::AudioDecoder;

/// The sound's rate and channels, as the phone encodes it.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// Where the sound goes instead of the speakers, for tests.
pub const SEAM: &str = "POCKETDESK_PHONE_AUDIO_FILE";

/// How long a player that would not open waits before it is tried again.
const OPEN_RETRY: Duration = Duration::from_secs(5);

/// Something that plays decoded, interleaved stereo samples.
trait Sink {
    fn play(&mut self, samples: &[f32]) -> Result<()>;
}

/// A 16-bit PCM WAV file's header for `data_bytes` of samples.
pub fn wav_header(data_bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let block = CHANNELS * 2;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * u32::from(block)).to_le_bytes());
    h[32..34].copy_from_slice(&block.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

/// The test seam: a WAV file whose header says how much it holds after
/// each packet, so a reader may open it while the session lasts.
struct WavSink {
    file: std::fs::File,
    data_bytes: u32,
}

impl WavSink {
    fn create(path: &Path) -> Result<Self> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&wav_header(0))?;
        Ok(Self {
            file,
            data_bytes: 0,
        })
    }
}

impl Sink for WavSink {
    fn play(&mut self, samples: &[f32]) -> Result<()> {
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|s| ((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).to_le_bytes())
            .collect();
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&pcm)?;
        self.data_bytes = self.data_bytes.saturating_add(pcm.len() as u32);
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(self.data_bytes))?;
        Ok(())
    }
}

/// The default output on Linux: `pw-cat` reading float samples.
#[cfg(not(windows))]
struct PwCat {
    child: std::process::Child,
}

#[cfg(not(windows))]
impl PwCat {
    fn open() -> Result<Self> {
        let child = std::process::Command::new("pw-cat")
            .args([
                "--playback",
                "--rate",
                "48000",
                "--channels",
                "2",
                "--format",
                "f32",
                "--media-role",
                "Music",
                "-",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(Self { child })
    }
}

#[cfg(not(windows))]
impl Sink for PwCat {
    fn play(&mut self, samples: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("pw-cat has no input"))?;
        stdin.write_all(&bytes)?;
        Ok(())
    }
}

#[cfg(not(windows))]
impl Drop for PwCat {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The default output on Windows: WASAPI, shared, float stereo.
#[cfg(windows)]
struct Speakers {
    client: windows::Win32::Media::Audio::IAudioClient,
    render: windows::Win32::Media::Audio::IAudioRenderClient,
    buffer_frames: u32,
}

#[cfg(windows)]
impl Speakers {
    fn open() -> Result<Self> {
        use windows::Win32::Media::Audio::{
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient,
            WAVEFORMATEX,
        };
        use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
        use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let device = crate::platform::audio_windows::default_output()?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: CHANNELS,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4 * u32::from(CHANNELS),
            nBlockAlign: 4 * CHANNELS,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                // 200 ms, in 100 ns units: room for the network's jitter.
                2_000_000,
                0,
                &format,
                None,
            )
        }?;
        let buffer_frames = unsafe { client.GetBufferSize() }?;
        let render: IAudioRenderClient = unsafe { client.GetService() }?;
        unsafe { client.Start() }?;
        Ok(Self {
            client,
            render,
            buffer_frames,
        })
    }
}

#[cfg(windows)]
impl Sink for Speakers {
    /// Late sound is dropped where the buffer is full: a gap is better
    /// than a growing delay.
    fn play(&mut self, samples: &[f32]) -> Result<()> {
        let channels = usize::from(CHANNELS);
        let padding = unsafe { self.client.GetCurrentPadding() }?;
        let room = (self.buffer_frames - padding) as usize;
        let frames = (samples.len() / channels).min(room);
        if frames == 0 {
            return Ok(());
        }
        let target = unsafe { self.render.GetBuffer(frames as u32) }? as *mut f32;
        // SAFETY: GetBuffer returned room for `frames` frames of `channels`
        // f32 samples, the format the client was opened with.
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr(), target, frames * channels);
            self.render.ReleaseBuffer(frames as u32, 0)?;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Speakers {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

/// The output to play on: the seam's file when set, else the speakers.
fn open(seam: Option<&Path>) -> Result<Box<dyn Sink>> {
    if let Some(path) = seam {
        return Ok(Box::new(WavSink::create(path)?));
    }
    #[cfg(windows)]
    {
        Ok(Box::new(Speakers::open()?))
    }
    #[cfg(not(windows))]
    {
        Ok(Box::new(PwCat::open()?))
    }
}

fn play_blocking(mut rx: Receiver<Vec<u8>>, seam: Option<PathBuf>) {
    let mut decoder = match AudioDecoder::new(RATE, CHANNELS) {
        Ok(decoder) => decoder,
        Err(e) => {
            warn!("The phone's sound cannot be decoded here: {e:#}");
            return;
        }
    };
    let mut sink: Option<Box<dyn Sink>> = None;
    let mut retry_after: Option<Instant> = None;
    while let Some(opus) = rx.blocking_recv() {
        let samples = match decoder.decode(&opus) {
            Ok(samples) => samples,
            Err(e) => {
                tracing::debug!("A packet of the phone's sound was not decoded: {e:#}");
                continue;
            }
        };
        if sink.is_none() && retry_after.is_none_or(|t| Instant::now() >= t) {
            match open(seam.as_deref()) {
                Ok(s) => {
                    info!("Playing the phone's sound here");
                    sink = Some(s);
                    retry_after = None;
                }
                Err(e) => {
                    warn!("The phone's sound cannot play here: {e:#}");
                    retry_after = Some(Instant::now() + OPEN_RETRY);
                }
            }
        }
        if let Some(s) = sink.as_mut()
            && let Err(e) = s.play(&samples)
        {
            warn!("The phone's sound stopped playing ({e:#}); opening the output again");
            sink = None;
            retry_after = Some(Instant::now() + OPEN_RETRY);
        }
    }
    if sink.is_some() {
        info!("The phone's sound ended");
    }
}

/// Play a session's sound packets until the session ends (its channel
/// closes). The seam is read once, at the session's start.
pub async fn run(rx: Receiver<Vec<u8>>) {
    let seam = std::env::var_os(SEAM).map(PathBuf::from);
    let _ = tokio::task::spawn_blocking(move || play_blocking(rx, seam)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K5: the seam's header is a 48 kHz stereo 16-bit WAV's.
    #[test]
    fn the_seam_writes_a_wav() {
        let h = wav_header(1920);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 1920);
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 1920);
    }

    /// K5: the phone's Opus packets decode into the seam: a 1 kHz tone comes
    /// out as long and as loud as it went in.
    #[tokio::test]
    async fn a_tone_reaches_the_seam() {
        use pocketdesk_core::streaming::audio::{AudioConfig, AudioEncoder};
        let dir = std::env::temp_dir().join(format!("pd-k5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("phone.wav");
        let config = AudioConfig {
            bitrate_bps: 96_000,
            ..AudioConfig::default()
        };
        let mut encoder = AudioEncoder::new(config).unwrap();
        let frame = config.samples_per_frame();
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let player = tokio::spawn({
            let path = path.clone();
            async move {
                tokio::task::spawn_blocking(move || play_blocking(rx, Some(path)))
                    .await
                    .unwrap()
            }
        });
        // 50 frames: one second.
        for n in 0..50 {
            let pcm: Vec<i16> = (0..frame)
                .flat_map(|i| {
                    let t = (n * frame + i) as f32 / 48_000.0;
                    let s = ((t * 1000.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16;
                    [s, s]
                })
                .collect();
            let packet = encoder.encode(&pcm).unwrap().unwrap();
            tx.send(packet.data).await.unwrap();
        }
        drop(tx);
        player.await.unwrap();
        let wav = std::fs::read(&path).unwrap();
        let data = &wav[44..];
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
            data.len()
        );
        assert_eq!(data.len(), 48_000 * 2 * 2, "one second, stereo, 16-bit");
        let samples: Vec<f32> = data
            .chunks(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])))
            .collect();
        // After the codec's start-up, the tone's level and its crossings.
        let tail = &samples[samples.len() / 2..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 8_000.0 && rms < 14_000.0, "level {rms}");
        let left: Vec<f32> = tail.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        let seconds = left.len() as f32 / 48_000.0;
        let hz = crossings as f32 / seconds;
        assert!((950.0..1050.0).contains(&hz), "{hz} Hz");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
