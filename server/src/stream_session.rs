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
}
