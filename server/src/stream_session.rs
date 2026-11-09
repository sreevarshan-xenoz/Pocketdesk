//! The one place a stream session's server is configured (the screen, sound
//! and input of a phone's session), for every road it arrives on: the LAN's
//! QUIC, the internet's iroh and Bluetooth's byte stream. Each road judges
//! the session with a gate of its own; everything else is the same, so it is
//! built here once and the roads cannot drift apart.

use std::sync::Arc;

use pocketdesk_core::protocol::permissions::DevicePermissions;
use pocketdesk_core::streaming::SessionClaim;
use pocketdesk_core::streaming::transport::{CertManager, StreamTransportConfig};
use pocketdesk_core::streaming::{
    CaptureBackend, SessionInput, StreamingConfig, StreamingServer, cast::CastEvent,
};
use tokio::sync::{broadcast, mpsc};

/// What a stream session's server is built from: the computer's settings,
/// read once as the server starts, and the channels its sessions feed.
#[derive(Clone)]
pub(crate) struct SessionSetup {
    pub(crate) config: StreamingConfig,
    pub(crate) input_tx: broadcast::Sender<SessionInput>,
    pub(crate) cert_manager: Arc<CertManager>,
    pub(crate) allow_hevc: bool,
    /// Whether a slow link may lower the frame rate (R6f).
    pub(crate) slow_link_frame_rate: bool,
    /// Whether a remote is offered the laser dot (G4).
    pub(crate) laser: bool,
    pub(crate) capture_backend: CaptureBackend,
    pub(crate) mic_relay_device: Option<String>,
    /// Where phones' casts go (T10k); `None` refuses them.
    pub(crate) cast: Option<mpsc::Sender<CastEvent>>,
}

impl SessionSetup {
    /// A server for one session, judged by `gate`: it is given the claim the
    /// session makes and the key its connection proved, and answers what the
    /// session may do, or `None` to refuse it. Spawns the session's own
    /// relays (the phone's microphone, a cast's sound), which end with it.
    pub(crate) fn server<F, Fut>(&self, gate: F) -> StreamingServer
    where
        F: Fn(SessionClaim) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<DevicePermissions>> + Send + 'static,
    {
        let mut server = StreamingServer::new(
            self.config.clone(),
            StreamTransportConfig::default(),
            Arc::clone(&self.cert_manager),
        );
        server.set_input_channel(self.input_tx.clone());
        // R4 E2: phone-mic frames feed a pw-loopback virtual source for the
        // life of this session.
        let (mic_tx, mic_rx) = mpsc::channel(64);
        tokio::spawn(crate::mic_relay::run_mic_relay(
            mic_rx,
            self.mic_relay_device.clone(),
        ));
        server.set_mic_channel(mic_tx);
        // K5: a cast's sound plays here.
        let (sound_tx, sound_rx) = mpsc::channel(64);
        tokio::spawn(crate::phone_sound::run(sound_rx));
        server.set_phone_sound_channel(sound_tx);
        if let Some(cast) = &self.cast {
            server.set_cast_channel(cast.clone());
        }
        // F6 part 2: a camera cast, while the owner offers phones as a camera.
        server.set_webcam_offer(crate::phone_camera::offered);
        server.set_session_telemetry(true);
        server.set_hevc_allowed(self.allow_hevc);
        server.set_slow_link_frame_rate(self.slow_link_frame_rate);
        server.set_laser(self.laser);
        server.set_capture_backend(self.capture_backend);
        server.set_screen_grant(crate::state::screen_grant());
        crate::state::apply_gnome_unattended();
        server.set_session_gate(move |claim| Box::pin(gate(claim)));
        server
    }

    /// This setup for a session over Bluetooth: the picture's frame rate and
    /// bitrate at the road's low setting (see [`bluetooth_config`]).
    pub(crate) fn for_bluetooth(&self) -> Self {
        Self {
            config: bluetooth_config(&self.config),
            ..self.clone()
        }
    }
}

/// The most frames a second a session over Bluetooth asks of the encoder.
pub(crate) const BLUETOOTH_FPS: u32 = 12;

/// The most bits a second a session over Bluetooth's picture takes: an
/// RFCOMM link carries about a megabit at best, and the control link, the
/// sound and the input share it.
pub(crate) const BLUETOOTH_BITRATE_BPS: u32 = 800_000;

/// `config` at the Bluetooth road's low setting: its frame rate and bitrate
/// capped at [`BLUETOOTH_FPS`] and [`BLUETOOTH_BITRATE_BPS`], and never
/// raised (a lower setting stays). The picture's size is the encoder's own:
/// scaling it down is not built yet.
pub(crate) fn bluetooth_config(config: &StreamingConfig) -> StreamingConfig {
    StreamingConfig {
        fps: config.fps.min(BLUETOOTH_FPS),
        bitrate_bps: config.bitrate_bps.min(BLUETOOTH_BITRATE_BPS),
        ..config.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bluetooth_setting_caps_the_rate_and_never_raises_it() {
        let high = StreamingConfig {
            fps: 60,
            bitrate_bps: 8_000_000,
            ..StreamingConfig::default()
        };
        let low = bluetooth_config(&high);
        assert_eq!(low.fps, BLUETOOTH_FPS);
        assert_eq!(low.bitrate_bps, BLUETOOTH_BITRATE_BPS);
        assert_eq!((low.width, low.height), (high.width, high.height));

        let lower = StreamingConfig {
            fps: 5,
            bitrate_bps: 300_000,
            ..StreamingConfig::default()
        };
        let kept = bluetooth_config(&lower);
        assert_eq!((kept.fps, kept.bitrate_bps), (5, 300_000));
    }
}
