//! Sound from another device played on this computer's default output: a
//! stream of Opus packets (48 kHz stereo) decoded and played, the output
//! opened at the first packet and again after it fails. Two uses share it:
//! the phone's sound in a session (K5, [`crate::phone_sound`]) and the
//! sound of the computer this one drives (BK6). Windows plays through WASAPI, Linux
//! through `pw-cat`; each use has its own test seam, a WAV file the sound
//! goes to instead of the speakers.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use pocketdesk_core::streaming::AudioDecoder;

/// The sound's rate and channels, as both senders encode it.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// How long a player that would not open waits before it is tried again.
const OPEN_RETRY: Duration = Duration::from_secs(5);

/// The most sound kept queued at the output: 80 ms. Sound past it is
/// dropped a packet at a time (K5 review: a burst after a stall had kept
/// the output 200 ms behind for the rest of the cast, and a full buffer cut
/// packets short, which clicked).
pub const TARGET_FRAMES: u32 = RATE / 1000 * 80;

/// Whether a packet of `frames` plays, with `queued` frames waiting at the
/// output and `room` free: whole, and only while the queue is under its
/// target.
pub fn plays(queued: u32, frames: u32, room: u32) -> bool {
    queued < TARGET_FRAMES && frames <= room
}

/// Something that plays decoded, interleaved stereo samples.
trait Sink {
    fn play(&mut self, samples: &[f32]) -> Result<()>;

    /// How many frames wait at the output now, not yet heard (none known:
    /// 0, and what is played is held back only by the output itself).
    fn queued(&mut self) -> u32 {
        0
    }
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
/// each packet, so a reader may open it while the sound lasts.
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

/// The default output on Linux: `pw-cat` reading float samples, fed by a
/// writer thread through a hand-off of [`PW_QUEUE`] packets (K5 review: a
/// blocking pipe write had added seconds of delay, and a stalled PipeWire
/// held the player thread for the session's life). A full hand-off drops
/// the packet, as the Windows output does past its target.
#[cfg(not(windows))]
struct PwCat {
    child: std::process::Child,
    feed: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

/// Packets waiting for `pw-cat`: 4, 80 ms.
#[cfg(not(windows))]
const PW_QUEUE: usize = 4;

#[cfg(not(windows))]
impl PwCat {
    fn open() -> Result<Self> {
        let mut child = std::process::Command::new("pw-cat")
            .args([
                "--playback",
                "--rate",
                "48000",
                "--channels",
                "2",
                "--format",
                "f32",
                "--latency",
                "60ms",
                "--media-role",
                "Music",
                "-",
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("pw-cat has no input"))?;
        let (feed, packets) = std::sync::mpsc::sync_channel::<Vec<u8>>(PW_QUEUE);
        // Ends when the hand-off closes, or when the pipe breaks as the
        // child is killed.
        std::thread::Builder::new()
            .name("remote-sound-pw-cat".into())
            .spawn(move || {
                while let Ok(bytes) = packets.recv() {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            feed: Some(feed),
        })
    }
}

#[cfg(not(windows))]
impl Sink for PwCat {
    fn play(&mut self, samples: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let feed = self
            .feed
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("pw-cat has no input"))?;
        match feed.try_send(bytes) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                Err(anyhow::anyhow!("pw-cat stopped"))
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for PwCat {
    fn drop(&mut self) {
        self.feed.take();
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
    /// Late sound is dropped a whole packet at a time ([`plays`]): a gap
    /// is better than a growing delay.
    fn play(&mut self, samples: &[f32]) -> Result<()> {
        let channels = usize::from(CHANNELS);
        let padding = unsafe { self.client.GetCurrentPadding() }?;
        let frames = samples.len() / channels;
        if frames == 0 || !plays(padding, frames as u32, self.buffer_frames - padding) {
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

    fn queued(&mut self) -> u32 {
        unsafe { self.client.GetCurrentPadding() }.unwrap_or(0)
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

/// Opus packets decoded and played on the default output (or the seam's
/// file), on the thread that owns it: the output opens at the first packet
/// that decodes, is opened again at once when it fails, and is closed when
/// this is dropped.
pub struct Player {
    /// What is played, for the log ("the phone's sound").
    what: &'static str,
    seam: Option<PathBuf>,
    decoder: AudioDecoder,
    sink: Option<Box<dyn Sink>>,
    retry_after: Option<Instant>,
}

impl Player {
    /// A player of `what`, writing to the WAV file `seam` instead of the
    /// speakers when given. Nothing opens until the first packet.
    pub fn new(what: &'static str, seam: Option<PathBuf>) -> Result<Player> {
        Ok(Player {
            what,
            seam,
            decoder: AudioDecoder::new(RATE, CHANNELS)?,
            sink: None,
            retry_after: None,
        })
    }

    /// Decode `opus` and play it; a packet that does not decode is skipped.
    pub fn play(&mut self, opus: &[u8]) {
        let samples = match self.decoder.decode(opus) {
            Ok(samples) => samples,
            Err(e) => {
                tracing::debug!("A packet of {} was not decoded: {e:#}", self.what);
                return;
            }
        };
        if self.sink.is_none() && self.retry_after.is_none_or(|t| Instant::now() >= t) {
            match open(self.seam.as_deref()) {
                Ok(s) => {
                    info!("Playing {} here", self.what);
                    self.sink = Some(s);
                    self.retry_after = None;
                }
                Err(e) => {
                    warn!("{} cannot play here: {e:#}", capitalized(self.what));
                    self.retry_after = Some(Instant::now() + OPEN_RETRY);
                }
            }
        }
        if let Some(s) = self.sink.as_mut()
            && let Err(e) = s.play(&samples)
        {
            // Opened again at once (K5 review: an output switched or
            // unplugged had cost five seconds); a failed open then waits.
            warn!(
                "{} stopped playing ({e:#}); opening the output again",
                capitalized(self.what)
            );
            self.sink = None;
            self.retry_after = None;
        }
    }

    /// How many frames wait at the output now (0 with none open, or none
    /// known).
    pub fn queued(&mut self) -> u32 {
        self.sink.as_mut().map_or(0, |s| s.queued())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if self.sink.is_some() {
            info!("{} ended", capitalized(self.what));
        }
    }
}

/// `what` with its first letter a capital, to start a log line.
fn capitalized(what: &str) -> String {
    let mut chars = what.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K5 review: a packet plays whole, and only under the target.
    #[test]
    fn sound_past_the_target_is_dropped_whole() {
        assert!(plays(0, 960, 9600));
        assert!(plays(TARGET_FRAMES - 1, 960, 9600));
        assert!(!plays(TARGET_FRAMES, 960, 9600), "behind: dropped");
        assert!(!plays(0, 960, 959), "never cut short");
    }

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

    #[test]
    fn a_log_line_starts_with_a_capital() {
        assert_eq!(capitalized("the phone's sound"), "The phone's sound");
        assert_eq!(capitalized(""), "");
    }
}
