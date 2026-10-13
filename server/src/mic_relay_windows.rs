//! R4 E2 on Windows: phone microphone → a virtual audio cable.
//!
//! Windows has no `pw-loopback`: a program cannot create a microphone without
//! shipping an audio driver. A virtual audio cable is that driver — VB-Audio's
//! free "VB-CABLE" is the common one — and it exposes a playback device
//! ("CABLE Input") whose audio comes out of a recording device ("CABLE
//! Output"). The relay decodes the phone's Opus frames and plays them into the
//! cable's playback side; meeting apps pick "CABLE Output" as their mic.
//!
//! Which playback device is the cable is matched by name: `mic_relay_device`
//! in config.toml, else "CABLE Input". With no match the relay says so (on a
//! cooldown) and drops the audio; the session is otherwise unaffected.
//!
//! COM objects stay on one thread, so the whole relay runs on a blocking
//! thread fed by the session's channel. WASAPI converts our 48 kHz mono float
//! to whatever the cable runs at (`AUTOCONVERTPCM`).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc::Receiver;
use tracing::{info, warn};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVEFORMATEX,
};
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::HSTRING;

use pocketdesk_core::streaming::{AudioDecoder, InputPacket};

pub use pocketdesk_core::streaming::audio::{MIC_CHANNELS, MIC_SAMPLE_RATE};

use crate::platform::audio_windows;

/// The playback device a stock VB-CABLE install creates.
pub const DEFAULT_CABLE: &str = "CABLE Input";

/// Minimum spacing between attempts to open the cable after a failure.
const OPEN_RETRY: Duration = Duration::from_secs(5);

/// The render endpoint whose friendly name contains `hint` (case-insensitive).
pub(crate) fn pick_cable<'a>(
    outputs: &'a [audio_windows::Endpoint],
    hint: &str,
) -> Option<&'a audio_windows::Endpoint> {
    let hint = hint.to_lowercase();
    outputs
        .iter()
        .find(|e| e.name.to_lowercase().contains(&hint))
}

/// A WASAPI render client on the cable, plus the Opus decoder feeding it.
struct CableSink {
    client: IAudioClient,
    render: IAudioRenderClient,
    buffer_frames: u32,
    decoder: AudioDecoder,
}

impl CableSink {
    fn open(hint: &str) -> Result<(Self, String)> {
        let outputs = audio_windows::outputs()?;
        let cable = pick_cable(&outputs, hint).ok_or_else(|| {
            anyhow!(
                "no playback device named like {hint:?}: install a virtual audio cable \
                 (e.g. VB-CABLE) or set mic_relay_device in config.toml"
            )
        })?;
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?;
        let device = unsafe { enumerator.GetDevice(&HSTRING::from(cable.id.as_str())) }?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }?;
        let channels = MIC_CHANNELS;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: channels,
            nSamplesPerSec: MIC_SAMPLE_RATE,
            nAvgBytesPerSec: MIC_SAMPLE_RATE * 4 * u32::from(channels),
            nBlockAlign: 4 * channels,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                1_000_000, // 100 ms, in 100 ns units: bounded latency
                0,
                &format,
                None,
            )
        }
        .context("opening the cable for playback")?;
        let buffer_frames = unsafe { client.GetBufferSize() }?;
        let render: IAudioRenderClient = unsafe { client.GetService() }?;
        unsafe { client.Start() }?;
        Ok((
            Self {
                client,
                render,
                buffer_frames,
                decoder: AudioDecoder::new(MIC_SAMPLE_RATE, MIC_CHANNELS)?,
            },
            cable.name.clone(),
        ))
    }

    /// Decode one Opus frame and play it. When the cable's buffer is full
    /// the excess is dropped: late mic audio is worse than a gap.
    fn feed(&mut self, opus: &[u8]) -> Result<()> {
        let samples = self.decoder.decode(opus)?;
        let channels = usize::from(MIC_CHANNELS);
        let padding = unsafe { self.client.GetCurrentPadding() }?;
        let room = (self.buffer_frames - padding) as usize;
        let frames = (samples.len() / channels).min(room);
        if frames == 0 {
            return Ok(());
        }
        let target = unsafe { self.render.GetBuffer(frames as u32) }? as *mut f32;
        // SAFETY: GetBuffer returned room for `frames` frames of
        // `channels` f32 samples, the format the client was opened with.
        unsafe {
            std::ptr::copy_nonoverlapping(samples.as_ptr(), target, frames * channels);
            self.render.ReleaseBuffer(frames as u32, 0)?;
        }
        Ok(())
    }
}

impl Drop for CableSink {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

fn relay_blocking(mut rx: Receiver<InputPacket>, hint: String) {
    let mut sink: Option<CableSink> = None;
    let mut retry_after: Option<Instant> = None;
    while let Some(packet) = rx.blocking_recv() {
        let InputPacket::Mic { enabled, opus } = packet else {
            continue;
        };
        if !enabled {
            if sink.take().is_some() {
                info!("Phone mic off — stopped playing into the virtual cable");
            }
            continue;
        }
        if sink.is_none() && retry_after.is_none_or(|t| Instant::now() >= t) {
            match CableSink::open(&hint) {
                Ok((s, name)) => {
                    info!("Phone mic on — playing into {name:?}");
                    sink = Some(s);
                    retry_after = None;
                }
                Err(e) => {
                    warn!("Phone mic unavailable: {e:#}");
                    retry_after = Some(Instant::now() + OPEN_RETRY);
                }
            }
        }
        if opus.is_empty() {
            continue; // start/keep-alive frame
        }
        let Some(s) = sink.as_mut() else {
            continue;
        };
        if let Err(e) = s.feed(&opus) {
            warn!("Mic playback failed ({e:#}) — reopening the cable");
            sink = None;
            retry_after = Some(Instant::now() + OPEN_RETRY);
        }
    }
    if sink.is_some() {
        info!("Mic session ended — stopped playing into the virtual cable");
    }
}

/// Relay the session's mic frames into the cable named like `device`
/// (default [`DEFAULT_CABLE`]) until the channel closes.
pub async fn run_mic_relay(rx: Receiver<InputPacket>, device: Option<String>) {
    let hint = device.unwrap_or_else(|| DEFAULT_CABLE.to_string());
    let _ = tokio::task::spawn_blocking(move || relay_blocking(rx, hint)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn endpoint(name: &str) -> audio_windows::Endpoint {
        audio_windows::Endpoint {
            id: format!("{{id}}.{name}"),
            name: name.to_string(),
            is_default: false,
        }
    }

    #[test]
    fn the_cable_is_found_by_a_case_insensitive_name_fragment() {
        let outputs = [
            endpoint("Speakers (Realtek(R) Audio)"),
            endpoint("CABLE Input (VB-Audio Virtual Cable)"),
        ];
        assert_eq!(
            pick_cable(&outputs, DEFAULT_CABLE).unwrap().name,
            "CABLE Input (VB-Audio Virtual Cable)"
        );
        assert_eq!(
            pick_cable(&outputs, "realtek").unwrap().name,
            "Speakers (Realtek(R) Audio)"
        );
        assert!(pick_cable(&outputs, "VoiceMeeter").is_none());
    }

    /// Without a matching cable the relay drops frames and still ends when
    /// the session does.
    #[tokio::test]
    async fn a_missing_cable_never_wedges_the_relay() {
        let (tx, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run_mic_relay(rx, Some("no such cable ✗".into())));
        for _ in 0..10 {
            let _ = tx.try_send(InputPacket::Mic {
                enabled: true,
                opus: vec![0xF8, 0xFF, 0xFE],
            });
        }
        tx.send(InputPacket::Mic {
            enabled: false,
            opus: Vec::new(),
        })
        .await
        .ok();
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), relay)
            .await
            .expect("relay hangs after channel close")
            .unwrap();
    }
}
