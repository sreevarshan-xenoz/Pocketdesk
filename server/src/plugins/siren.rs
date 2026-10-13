use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};

/// Find-my-device siren (KDE Connect parity, Tier-2 #11).
///
/// Answers `kdeconnect.findmydevice` `{ring: true}` by playing a looping
/// system sound for a bounded time, so a phone can make an unattended
/// desktop audible. The same packet pushed *to* a client makes the phone
/// ring; this plugin is the receiving side on the desktop.
#[derive(Debug)]
pub struct SirenPlugin;

impl Default for SirenPlugin {
    fn default() -> Self {
        Self
    }
}

impl SirenPlugin {
    pub fn new() -> Self {
        Self
    }
}

/// How long the desktop siren rings before self-stopping.
const SIREN_SECS: u64 = 30;

/// Candidate sounds, best first: a loopable alarm-ish file from the
/// freedesktop sound theme.
#[cfg_attr(windows, allow(dead_code))]
const SOUND_CANDIDATES: &[&str] = &[
    "/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga",
    "/usr/share/sounds/freedesktop/stereo/bell.oga",
];

#[async_trait::async_trait]
impl Plugin for SirenPlugin {
    fn name(&self) -> &'static str {
        "siren"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &["kdeconnect.findmydevice"]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    async fn handle_packet(
        &self,
        packet: &NetworkPacket,
        _sender: &dyn DeviceSender,
    ) -> Result<()> {
        if packet.packet_type == "kdeconnect.findmydevice"
            && packet
                .body
                .get("ring")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        {
            tokio::spawn(async { ring_desktop(SIREN_SECS).await });
        }
        Ok(())
    }
}

/// Play a looping sound for `secs`, trying each player and file. Never
/// errors out of the plugin: a headless box with no sound setup simply
/// stays quiet.
#[cfg(not(windows))]
async fn ring_desktop(secs: u64) {
    for player in ["pw-play", "paplay"] {
        for sound in SOUND_CANDIDATES {
            if !std::path::Path::new(sound).exists() {
                continue;
            }
            // `--loop` exists on both pw-play and paplay.
            let mut child = tokio::process::Command::new(player)
                .args(["--loop", sound])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match child.as_mut() {
                Ok(c) => {
                    tracing::info!("Find-my-device: ringing desktop via {player} {sound}");
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(secs), c.wait()).await;
                    let _ = c.kill().await;
                    return;
                }
                Err(_) => continue,
            }
        }
    }
    tracing::warn!("Find-my-device: no playable sound found, desktop stays quiet");
}

/// Windows: loop a synthesized alarm tone for `secs`. Windows ships no
/// alarm sound with a stable path, so the tone is generated in memory.
#[cfg(windows)]
async fn ring_desktop(secs: u64) {
    use windows::Win32::Media::Audio::{
        PlaySoundW, SND_ASYNC, SND_LOOP, SND_MEMORY, SND_NODEFAULT,
    };
    use windows::core::PCWSTR;

    // PlaySound reads the buffer while it loops, so it lives for the process.
    static TONE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    let tone = TONE.get_or_init(alarm_wav);
    let started = unsafe {
        PlaySoundW(
            PCWSTR(tone.as_ptr().cast()),
            None,
            SND_MEMORY | SND_ASYNC | SND_LOOP | SND_NODEFAULT,
        )
    }
    .as_bool();
    if !started {
        tracing::warn!("Find-my-device: the desktop could not play the alarm");
        return;
    }
    tracing::info!("Find-my-device: ringing desktop for {secs} s");
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    let _ = unsafe { PlaySoundW(PCWSTR::null(), None, SND_NODEFAULT) };
}

/// One second of alarm, as a complete 16-bit mono 22.05 kHz WAV file:
/// 880 Hz and 660 Hz beeps, then a pause, so it loops into a two-tone siren.
#[cfg(windows)]
fn alarm_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let mut samples = Vec::with_capacity(RATE as usize);
    for i in 0..RATE {
        let t = f64::from(i) / f64::from(RATE);
        let freq = match t {
            t if t < 0.25 => 880.0,
            t if t < 0.5 => 660.0,
            _ => 0.0,
        };
        let value = (t * freq * std::f64::consts::TAU).sin() * 0.6 * f64::from(i16::MAX);
        samples.push(value as i16);
    }
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_candidates_are_absolute_oga_paths() {
        assert!(
            SOUND_CANDIDATES
                .iter()
                .all(|s| s.starts_with('/') && s.ends_with(".oga"))
        );
    }

    #[test]
    fn plugin_declares_findmydevice_capability() {
        assert!(
            SirenPlugin::new()
                .incoming_capabilities()
                .contains(&"kdeconnect.findmydevice")
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// Checks the generated file only; nothing is played.
    #[test]
    fn the_alarm_is_a_well_formed_one_second_wav() {
        let wav = alarm_wav();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len + 8, wav.len());
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, 22_050 * 2, "one second of 16-bit mono");
        // Audible first half, silent pause at the end.
        let sample = |n: usize| i16::from_le_bytes([wav[44 + n * 2], wav[45 + n * 2]]);
        assert!((0..2000).any(|n| sample(n).abs() > 10_000));
        assert!((20_000..22_050).all(|n| sample(n) == 0));
    }
}
