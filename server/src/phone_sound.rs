//! The phone's sound on this computer (K5): a cast's Opus packets (48 kHz
//! stereo, what the phone's apps play) decoded and played on the default
//! output, for the life of the session, by the shared
//! [`remote_sound::Player`]; with `POCKETDESK_PHONE_AUDIO_FILE` set, the
//! sound goes to that WAV file instead (a test seam).

use std::path::PathBuf;

use tokio::sync::mpsc::Receiver;
use tracing::warn;

use crate::remote_sound::Player;

/// Where the sound goes instead of the speakers, for tests.
pub const SEAM: &str = "POCKETDESK_PHONE_AUDIO_FILE";

fn play_blocking(mut rx: Receiver<Vec<u8>>, seam: Option<PathBuf>, mut first: Option<Vec<u8>>) {
    let mut player = match Player::new("the phone's sound", seam) {
        Ok(player) => player,
        Err(e) => {
            warn!("The phone's sound cannot be decoded here: {e:#}");
            return;
        }
    };
    while let Some(opus) = first.take().or_else(|| rx.blocking_recv()) {
        player.play(&opus);
    }
}

/// Play a session's sound packets until the session ends (its channel
/// closes). The seam is read once, at the session's start.
pub async fn run(mut rx: Receiver<Vec<u8>>) {
    // Most sessions carry no sound: no thread or decoder until one does
    // (K5 review).
    let Some(first) = rx.recv().await else {
        return;
    };
    let seam = std::env::var_os(SEAM).map(PathBuf::from);
    let _ = tokio::task::spawn_blocking(move || play_blocking(rx, seam, Some(first))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

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
                tokio::task::spawn_blocking(move || play_blocking(rx, Some(path), None))
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
