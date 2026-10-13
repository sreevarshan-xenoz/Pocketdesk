//! R4 E2: phone microphone → desktop virtual microphone.
//!
//! The streaming server forwards every `InputPacket::Mic` frame it intercepts
//! (core `streamer.rs` monitor task) onto a bounded channel; this module owns
//! the receiving end. Frames carry 20 ms Opus packets (48 kHz mono). The relay
//! decodes them to float32 PCM and pipes the bytes into a `pw-loopback`
//! capture client, which registers an `Audio/Source/Virtual` node named
//! "Pocketdesk Mic" — a normal microphone from the desktop's point of view
//! (meetings, recorders, `pactl set-default-source`).
//!
//! Format note (verified against PipeWire 1.6.8 on this box): pw-loopback
//! ignores `audio.format` in node props and always negotiates float32 on its
//! stdin — so the Opus decoder's float output is fed verbatim (interleaved,
//! little-endian) and no S16 packing exists anywhere on this path.
//!
//! Lifecycle: the start frame (`enabled`, empty payload) or the first audio
//! frame spawns the node; a stop frame (`!enabled`) closes stdin so
//! pw-loopback exits and the node vanishes; a failed write (PipeWire daemon
//! restart) tears the node down and the next frame respawns it after a short
//! cooldown. The channel closing — session teardown — also stops the node,
//! and `kill_on_drop` is the last-resort guard if the task itself is aborted.

use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::Receiver;
use tracing::{info, warn};

use pocketdesk_core::streaming::{AudioDecoder, InputPacket};

/// Defined in `core::streaming::audio` because the phone's encoder has to match
/// them; re-exported here so the relay reads as one file.
pub use pocketdesk_core::streaming::audio::{MIC_CHANNELS, MIC_SAMPLE_RATE};

/// Minimum spacing between node spawn attempts after a failure (the phone
/// sends a frame every 20 ms — without this a missing pw-loopback would
/// warn 50x/s).
const SPAWN_RETRY: Duration = Duration::from_secs(5);

pub fn mic_node_props() -> String {
    "node.name=pocketdesk_mic node.description=\"Pocketdesk Mic\" \
     node.nick=\"Pocketdesk Mic\" media.class=Audio/Source/Virtual"
        .to_string()
}

/// pw-loopback capture-client arguments: mono, float32 on stdin.
pub fn pw_loopback_args() -> Vec<String> {
    vec![
        "-c".to_string(),
        MIC_CHANNELS.to_string(),
        "-m".to_string(),
        "[[MONO]]".to_string(),
        "-i".to_string(),
        mic_node_props(),
    ]
}

fn mic_command(program: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.kill_on_drop(true);
    cmd
}

struct MicNode {
    child: Child,
    stdin: ChildStdin,
    decoder: AudioDecoder,
}

impl MicNode {
    async fn spawn(program: &str, args: &[String]) -> anyhow::Result<Self> {
        let mut cmd = mic_command(program, args);
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("pw-loopback stdin not piped"))?;
        let decoder = AudioDecoder::new(MIC_SAMPLE_RATE, MIC_CHANNELS)?;
        Ok(Self {
            child,
            stdin,
            decoder,
        })
    }

    async fn feed(&mut self, opus: &[u8]) -> anyhow::Result<()> {
        let samples = self.decoder.decode(opus)?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.stdin.write_all(&bytes).await?;
        Ok(())
    }

    /// Close stdin (EOF → clean pw-loopback exit), kill after a grace period.
    async fn stop(mut self) {
        drop(self.stdin);
        match tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(Ok(status)) => info!(%status, "Mic node (pw-loopback) exited"),
            _ => {
                let _ = self.child.start_kill();
                info!("Mic node (pw-loopback) killed after stop");
            }
        }
    }
}

/// Relay the session's mic frames into a pw-loopback virtual source. The
/// device hint only matters on Windows (which cable to play into).
pub async fn run_mic_relay(rx: Receiver<InputPacket>, _device: Option<String>) {
    run_mic_relay_with("pw-loopback", &pw_loopback_args(), rx).await;
}

async fn run_mic_relay_with(program: &str, args: &[String], mut rx: Receiver<InputPacket>) {
    let mut node: Option<MicNode> = None;
    let mut retry_after: Option<Instant> = None;

    while let Some(packet) = rx.recv().await {
        let InputPacket::Mic { enabled, opus } = packet else {
            continue;
        };
        if !enabled {
            if let Some(n) = node.take() {
                info!("Phone mic off — removing Pocketdesk Mic source");
                n.stop().await;
            }
            continue;
        }

        if node.is_none() && retry_after.is_none_or(|t| Instant::now() >= t) {
            match MicNode::spawn(program, args).await {
                Ok(n) => {
                    info!("Phone mic on — Pocketdesk Mic source live ({program})");
                    node = Some(n);
                    retry_after = None;
                }
                Err(e) => {
                    warn!(error = %e, "Failed to start mic node (pw-loopback?) — muting mic path");
                    retry_after = Some(Instant::now() + SPAWN_RETRY);
                }
            }
        }

        if opus.is_empty() {
            continue; // start/keep-alive frame
        }
        let Some(n) = node.as_mut() else {
            continue; // spawn on cooldown or failed — drop the frame
        };
        if let Err(e) = n.feed(&opus).await {
            warn!(error = %e, "Mic write failed — tearing node down for respawn");
            retry_after = Some(Instant::now() + SPAWN_RETRY);
            if let Some(dead) = node.take() {
                dead.stop().await;
            }
        }
    }

    if let Some(n) = node.take() {
        info!("Mic session ended — removing Pocketdesk Mic source");
        n.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::streaming::{AudioConfig, AudioEncoder};
    use tokio::sync::mpsc;

    fn mic_frame(enabled: bool, opus: Vec<u8>) -> InputPacket {
        InputPacket::Mic { enabled, opus }
    }

    #[test]
    fn node_props_declare_a_virtual_mic() {
        let props = mic_node_props();
        assert!(props.contains("node.name=pocketdesk_mic"));
        assert!(props.contains("media.class=Audio/Source/Virtual"));
        assert!(props.contains("node.description=\"Pocketdesk Mic\""));
    }

    #[test]
    fn command_is_mono_capture_client() {
        let args = pw_loopback_args();
        assert_eq!(&args[..2], ["-c", "1"]);
        assert!(args.iter().any(|a| a.contains("[[MONO]]")));
        assert!(args.iter().any(|a| a.contains("Audio/Source/Virtual")));
    }

    fn encoded_silence_frame() -> Vec<u8> {
        let config = AudioConfig {
            sample_rate: MIC_SAMPLE_RATE,
            channels: MIC_CHANNELS,
            bitrate_bps: 32_000,
            frame_duration_ms: 20,
        };
        let mut encoder = AudioEncoder::new(config).unwrap();
        let silence = vec![0i16; config.samples_per_frame() * config.channels as usize];
        encoder.encode(&silence).unwrap().unwrap().data
    }

    /// Full lifecycle against a stand-in program (`cat` instead of
    /// pw-loopback: it consumes stdin until EOF, like the real client):
    /// start frame spawns, audio frames decode+write, stop frame tears
    /// down, channel close ends the relay.
    #[tokio::test]
    async fn relay_lifecycle_spawn_write_stop() {
        let (tx, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run_mic_relay_with("cat", &[], rx));

        tx.send(mic_frame(true, Vec::new())).await.unwrap();
        tx.send(mic_frame(true, encoded_silence_frame()))
            .await
            .unwrap();
        // Give the task time to spawn + feed one frame.
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(mic_frame(false, Vec::new())).await.unwrap();
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), relay)
            .await
            .expect("relay hangs after channel close")
            .unwrap();
    }

    /// A missing capture program must not kill the relay — frames inside the
    /// spawn cooldown are simply dropped, and teardown still completes.
    #[tokio::test]
    async fn relay_survives_spawn_failure() {
        let (tx, rx) = mpsc::channel(64);
        let relay = tokio::spawn(run_mic_relay_with("/nonexistent/pw-loopback", &[], rx));
        for _ in 0..10 {
            let _ = tx.try_send(mic_frame(true, encoded_silence_frame()));
        }
        tx.send(mic_frame(false, Vec::new())).await.ok();
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), relay)
            .await
            .expect("relay hangs after channel close")
            .unwrap();
    }
}
