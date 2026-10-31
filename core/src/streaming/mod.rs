//! Screen capture and streaming module
//!
//! Provides screen capture via PipeWire/XDG Desktop Portal,
//! video encoding via FFmpeg, and QUIC-based streaming transport.

pub mod audio;
pub mod bitrate;
pub mod cast;
pub mod cast_control;
pub mod chaos;
pub mod client;
pub mod connection;
#[cfg(feature = "wan")]
pub mod dht;
pub mod encoder_detect;
pub mod ffmpeg_flags;
mod frame_pool;
#[cfg(all(windows, feature = "capture", feature = "inproc-windows"))]
mod gpu_frame;
pub mod h264_sps;
#[cfg(all(windows, feature = "capture", feature = "inproc-windows"))]
pub use gpu_frame::GpuFrame;
pub mod in_flight;
pub mod input_packet;
#[cfg(feature = "wan")]
pub mod iroh_connection;
pub mod latest_frame;
pub mod portal_input;
pub mod session;
pub mod session_telemetry;
pub mod sessions;
pub mod transport;

// Server-only modules. Feature gates say *what* is built; the
// `target_os = "linux"` gates mark backends whose dependencies only exist there.
#[cfg(feature = "capture")]
pub mod audio_capture;
#[cfg(all(feature = "capture", target_os = "linux"))]
mod audio_capture_pipewire;
#[cfg(all(feature = "capture", windows))]
mod audio_capture_wasapi;
#[cfg(feature = "encode")]
pub mod bench;
#[cfg(feature = "capture")]
pub mod capture;
pub mod capture_backend;
#[cfg(all(feature = "capture", windows))]
mod capture_d3d;
#[cfg(all(feature = "capture", windows))]
pub mod capture_dxgi;
#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod capture_linux;
#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod capture_screencopy;
#[cfg(all(feature = "capture", windows))]
pub mod capture_wgc;
#[cfg(feature = "capture")]
pub mod capture_x11;
#[cfg(feature = "encode")]
pub mod encoder;
#[cfg(any(
    all(target_os = "linux", feature = "inproc"),
    all(windows, feature = "inproc-windows")
))]
pub mod encoder_inproc;
#[cfg(all(target_os = "linux", feature = "inproc"))]
pub mod encoder_vaapi;
#[cfg(feature = "encode")]
mod flv_demux;
#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod laser_dot;
#[cfg(feature = "encode")]
pub mod nvenc_power;
#[cfg(all(feature = "capture", target_os = "linux"))]
pub mod shield;
#[cfg(feature = "server")]
pub mod streamer;
#[cfg(feature = "capture")]
pub mod test_pattern;

pub use audio::{AudioConfig, AudioPacket};
#[cfg(feature = "opus")]
pub use audio::{AudioDecoder, AudioEncoder};
pub use bitrate::LossCeiling;
#[cfg(feature = "capture")]
pub use capture::start_capture_auto;
pub use capture_backend::CaptureBackend;
#[cfg(feature = "capture")]
pub use capture_x11::{check_x11_availability, start_x11_capture};
pub use client::DEFAULT_STREAMING_PORT;
pub use client::RemoteFlags;
pub use client::SessionMode;
pub use client::StreamingClient;
/// Entry point for a client-side measurement source that is not the
/// [`StreamingClient`] itself (the Android decoder): see
/// [`client::report_samples`].
pub use client::report_samples;
pub use client::{
    REMOTE_FEATURE_LASER, REMOTE_FEATURES_SAID, SCREEN_FEATURE_INPUT, SCREEN_FEATURE_MIC,
    SCREEN_FEATURE_MIC_SAID, SCREEN_FEATURE_PEN, SCREEN_FEATURE_PER_SCREEN,
    SCREEN_FEATURE_SYSTEM_SAID, SCREEN_FEATURE_WINDOWS, SCREEN_FEATURES_SAID, SOUND_GRANT_AUDIO,
    SOUND_GRANT_MIC, SOUND_GRANT_SAID, remote_features, screen_features, screen_input_per_screen,
    screen_runs_windows, screen_takes_input, screen_takes_mic, sound_grant,
};
pub use connection::{
    Connection, ConnectionError, ConnectionStats, InStream, OutStream, PeerKey, QuinnConnection,
    SharedConnection, TransportFamily,
};
#[cfg(feature = "wan")]
pub use dht::{DhtAddressLookup, dht_resolver};
pub use encoder_detect::{AvailableEncoders, HardwareEncoder, probe_encoders, resolve_encoder};
pub use input_packet::{
    InputPacket, MAX_SAMPLES_PER_FRAME, PEN_BARREL, PEN_CONTACT, PEN_ERASER, PEN_IN_RANGE,
    PenSample, SAMPLE_DECODE, SAMPLE_E2E, SAMPLE_RENDER, SampleBatch,
};
#[cfg(feature = "wan")]
pub use iroh::SecretKey as IrohSecretKey;
#[cfg(feature = "wan")]
pub use iroh_connection::{
    IrohConnection, IrohControlStream, IrohDial, LinkAcceptor, load_or_create_device_key,
    network_changed,
};
pub use session::{SessionType, detect_session_type};
pub use session_telemetry::{
    LinkReport, SessionGuard, SessionOutcome, SessionRecorder, SessionReport,
    set_session_telemetry_callback,
};
#[cfg(feature = "server")]
pub use streamer::{
    AbsoluteSpace, AbsoluteSpaces, SessionClaim, SessionInput, StreamingServer, set_absolute_space,
};

use serde::{Deserialize, Serialize};

/// Streaming configuration for capture and encoding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    /// Target width in pixels
    pub width: u32,
    /// Target height in pixels
    pub height: u32,
    /// Target frames per second
    pub fps: u32,
    /// Target bitrate in bits per second
    pub bitrate_bps: u32,
    /// Video codec (H.264 or H.265/HEVC)
    pub codec: VideoCodec,
    /// H.264 profile (baseline, main, high)
    pub profile: H264Profile,
    /// Encoder preset (speed vs quality tradeoff)
    pub preset: EncoderPreset,
    /// Hardware encoder selection
    pub hardware_encoder: HardwareEncoder,
    /// Monitor index for multi-monitor support (F2).
    /// 0 = primary monitor.
    pub monitor_index: u32,
}

/// Metadata about a physical or virtual monitor available for streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Monitor index (0-based)
    pub index: u32,
    /// Human-readable name (e.g., "eDP-1", "HDMI-A-1")
    pub name: String,
    /// Horizontal resolution in pixels
    pub width: u32,
    /// Vertical resolution in pixels
    pub height: u32,
    /// Whether this is the primary monitor
    pub is_primary: bool,
    /// Where its top-left corner sits in the desktop's layout, in the units
    /// of its size (PC11: the window that shows another computer's screen
    /// lays its screen buttons out so). `None` when not known, and from a
    /// server before PC11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
}

/// H.264 encoding profile
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum H264Profile {
    Baseline,
    #[default]
    Main,
    High,
}

/// Encoder preset for speed/quality tradeoff
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum EncoderPreset {
    UltraFast,
    SuperFast,
    #[default]
    VeryFast,
    Faster,
    Fast,
    Medium,
    Slow,
}

/// Video codec selection (F3: H.265/HEVC support).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VideoCodec {
    /// H.264/AVC (widest compatibility)
    #[default]
    H264,
    /// H.265/HEVC (up to 50% bandwidth savings)
    H265,
}

impl VideoCodec {
    /// Every codec this build knows about, in negotiation-preference order.
    /// `pocketdesk capabilities` enumerates from here (roadmap item 4), so a new
    /// variant cannot be added without showing up in the reported set.
    pub const ALL: [VideoCodec; 2] = [VideoCodec::H264, VideoCodec::H265];

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265 (HEVC)",
        }
    }

    /// The R4 C1 client-capability bit that advertises decodability of this
    /// codec, or `None` when the codec needs no advertisement (H.264 is assumed
    /// for every client, so its absence from the caps byte still means yes).
    pub fn codec_cap_bit(&self) -> Option<u8> {
        match self {
            VideoCodec::H264 => None,
            VideoCodec::H265 => Some(crate::streaming::client::CODEC_CAP_HEVC),
        }
    }

    /// MIME type for MediaCodec.
    pub fn mime_type(&self) -> &'static str {
        match self {
            VideoCodec::H264 => "video/avc",
            VideoCodec::H265 => "video/hevc",
        }
    }

    /// FFmpeg codec name for encoding.
    pub fn ffmpeg_codec(&self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "hevc",
        }
    }

    /// Bitrate multiplier: H.265 can deliver similar quality at ~50% bitrate.
    pub fn bitrate_multiplier(&self) -> f64 {
        match self {
            VideoCodec::H264 => 1.0,
            VideoCodec::H265 => 0.5,
        }
    }
}

/// User-facing video quality preset with concrete encoding parameters.
/// Written in lowercase in config.toml, as the docs and the example config
/// spell it (`video_quality = "balanced"`); the capitalised spelling earlier
/// versions wrote still reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoQualityPreset {
    /// 720p, 2 Mbps, veryfast encoder preset
    #[serde(alias = "Low")]
    Low,
    /// 1080p, 5 Mbps, superfast encoder preset
    #[default]
    #[serde(alias = "Balanced")]
    Balanced,
    /// 1080p, 10 Mbps, medium encoder preset
    #[serde(alias = "High")]
    High,
}

impl VideoQualityPreset {
    /// Convert to a `StreamingConfig` with appropriate parameters.
    pub fn to_streaming_config(&self) -> StreamingConfig {
        match self {
            VideoQualityPreset::Low => StreamingConfig {
                width: 1280,
                height: 720,
                fps: 30,
                bitrate_bps: 2_000_000,
                preset: EncoderPreset::VeryFast,
                ..StreamingConfig::default()
            },
            VideoQualityPreset::Balanced => StreamingConfig {
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_bps: 5_000_000,
                preset: EncoderPreset::SuperFast,
                ..StreamingConfig::default()
            },
            VideoQualityPreset::High => StreamingConfig {
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_bps: 10_000_000,
                preset: EncoderPreset::Medium,
                ..StreamingConfig::default()
            },
        }
    }
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 8_000_000, // 8 Mbps
            codec: VideoCodec::H264,
            profile: H264Profile::Main,
            preset: EncoderPreset::VeryFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        }
    }
}

/// Runtime statistics for an active streaming session.
#[derive(Debug, Clone, Default)]
pub struct StreamingStats {
    /// Current output framerate in frames per second.
    pub fps: f64,
    /// Current encoder bitrate in kilobits per second.
    pub bitrate_kbps: u64,
    /// End-to-end latency from capture to render in milliseconds.
    pub e2e_latency_ms: u64,
    /// Frames dropped due to channel full.
    pub frame_drops: u64,
}

/// A captured video frame with metadata
#[derive(Debug)]
pub struct VideoFrame {
    /// Raw frame data (BGRA format from PipeWire)
    pub data: Vec<u8>,
    /// Frame width
    pub width: u32,
    /// Frame height
    pub height: u32,
    /// Frame stride (bytes per row)
    pub stride: u32,
    /// Frame timestamp (monotonic clock)
    pub timestamp: std::time::Instant,
    /// The frame still on the GPU, `data` empty (BG10): Windows captures on
    /// an NVIDIA adapter with `inproc-windows`. Read back with
    /// [`VideoFrame::downloaded`] where the pixels are needed.
    pub gpu: Option<GpuFrame>,
}

/// Without zero-copy (BG10) no frame is on the GPU.
#[cfg(not(all(windows, feature = "capture", feature = "inproc-windows")))]
#[derive(Debug, Clone)]
pub enum GpuFrame {}

#[cfg(not(all(windows, feature = "capture", feature = "inproc-windows")))]
impl GpuFrame {
    fn offset(&mut self, _x: u32, _y: u32) {
        match *self {}
    }

    fn download(&self, _width: u32, _height: u32) -> anyhow::Result<Vec<u8>> {
        match *self {}
    }
}

/// A frame's buffer goes back to the pool for the next capture (BG9).
impl Drop for VideoFrame {
    fn drop(&mut self) {
        frame_pool::give_back(std::mem::take(&mut self.data));
    }
}

impl VideoFrame {
    /// This frame read back to the CPU when it is on the GPU (BG10), for an
    /// encoder that needs its pixels; `None` when it already holds them.
    pub fn downloaded(&self) -> anyhow::Result<Option<VideoFrame>> {
        let Some(gpu) = &self.gpu else {
            return Ok(None);
        };
        Ok(Some(VideoFrame {
            data: gpu.download(self.width, self.height)?,
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            timestamp: self.timestamp,
            gpu: None,
        }))
    }

    /// Trim an odd width or height down to even (AI1), the last column or
    /// row dropped: `true` when it trimmed. A window captured whole can be
    /// odd-sized, which H.264's 4:2:0 cannot carry, and a crop's first frames
    /// (the software crop, already even) would otherwise rebuild the encoder
    /// once more at the window's own odd size.
    pub fn to_even(&mut self) -> bool {
        if (self.width | self.height) & 1 == 0 {
            return false;
        }
        self.crop_region(0, 0, self.width, self.height)
    }

    /// Crop this frame in place to `x, y, width, height`, repacking BGRA rows.
    ///
    /// The rectangle is clamped to the frame bounds and the size is rounded
    /// down to even values (H.264 4:2:0 chroma requires even dimensions).
    /// Returns `false` (frame untouched) when the clamped rect is empty.
    pub fn crop_region(&mut self, x: u32, y: u32, width: u32, height: u32) -> bool {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let w = width.min(self.width - x) & !1;
        let h = height.min(self.height - y) & !1;
        if w == 0 || h == 0 {
            return false;
        }
        // A frame on the GPU (BG10) is cropped by narrowing its region; the
        // encoder copies only that.
        if let Some(gpu) = &mut self.gpu {
            gpu.offset(x, y);
            self.width = w;
            self.height = h;
            self.stride = w * 4;
            return true;
        }

        let bpp = 4usize; // BGRA from PipeWire/X11 capture
        let src_stride = self.stride as usize;
        let dst_stride = (w as usize) * bpp;
        let start = |row: usize| (y as usize + row) * src_stride + (x as usize) * bpp;
        if dst_stride > src_stride || start(h as usize - 1) + dst_stride > self.data.len() {
            return false;
        }
        // Each row moves to or before where it was, so the rows are packed in
        // place, first to last, with no second buffer (BG9).
        for row in 0..h as usize {
            self.data
                .copy_within(start(row)..start(row) + dst_stride, row * dst_stride);
        }
        self.data.truncate(dst_stride * h as usize);

        self.width = w;
        self.height = h;
        self.stride = w * 4;
        true
    }
}

/// Encoded video packet ready for transmission
#[derive(Debug)]
pub struct EncodedPacket {
    /// Encoded frame data (H.264 NAL units)
    pub data: Vec<u8>,
    /// Whether this is a keyframe
    pub is_keyframe: bool,
    /// Frame timestamp (monotonic clock)
    pub timestamp: std::time::Instant,
    /// Sequence number for ordering
    pub sequence: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// 8x6 BGRA frame, pixel (px,py) = [px, py, 0, 255] per channel group,
    /// with a padded stride to exercise row repacking.
    fn pattern_frame() -> VideoFrame {
        let (w, h, stride) = (8u32, 6u32, 8 * 4 + 16u32);
        let mut data = vec![0u8; (stride * h) as usize];
        for py in 0..h as usize {
            for px in 0..w as usize {
                let o = py * stride as usize + px * 4;
                data[o] = px as u8;
                data[o + 1] = py as u8;
                data[o + 3] = 255;
            }
        }
        VideoFrame {
            data,
            width: w,
            height: h,
            stride,
            timestamp: Instant::now(),
            gpu: None,
        }
    }

    fn px(frame: &VideoFrame, x: usize, y: usize) -> (u8, u8) {
        let o = y * frame.stride as usize + x * 4;
        (frame.data[o], frame.data[o + 1])
    }

    #[test]
    fn crop_region_repacks_rows() {
        let mut f = pattern_frame();
        assert!(f.crop_region(2, 1, 4, 4));
        assert_eq!((f.width, f.height, f.stride), (4, 4, 16));
        assert_eq!(f.data.len(), 64);
        assert_eq!(px(&f, 0, 0), (2, 1));
        assert_eq!(px(&f, 3, 3), (5, 4));
    }

    #[test]
    fn crop_region_rounds_odd_size_down_to_even() {
        let mut f = pattern_frame();
        assert!(f.crop_region(0, 0, 5, 5));
        assert_eq!((f.width, f.height), (4, 4));
    }

    #[test]
    fn an_odd_frame_is_trimmed_to_even_and_an_even_one_left_alone() {
        // AI1: a window captured whole can be odd-sized (1395 wide), which
        // H.264's 4:2:0 cannot carry; the software crop already rounds down.
        let mut odd = pattern_frame();
        odd.width = 7;
        odd.height = 5;
        assert!(odd.to_even());
        assert_eq!((odd.width, odd.height), (6, 4));
        assert_eq!(
            odd.stride, 24,
            "repacked tight: the sidecar encoders ignore stride"
        );
        assert_eq!(
            px(&odd, 5, 3),
            (5, 3),
            "the same pixels, the last column and row dropped"
        );
        let mut even = pattern_frame();
        assert!(!even.to_even(), "nothing to do");
        assert_eq!(
            (even.width, even.height, even.stride),
            (8, 6, 48),
            "not repacked"
        );
    }

    #[test]
    fn crop_region_clamps_to_bounds() {
        let mut f = pattern_frame();
        assert!(f.crop_region(6, 4, 100, 100));
        // Remaining 2x2 region rounds to even fine.
        assert_eq!((f.width, f.height), (2, 2));
        assert_eq!(px(&f, 0, 0), (6, 4));
    }

    #[test]
    fn crop_region_rejects_degenerate_rects() {
        let mut f = pattern_frame();
        assert!(!f.crop_region(0, 0, 1, 4)); // rounds to width 0
        assert_eq!((f.width, f.height), (8, 6)); // untouched
        let mut f2 = pattern_frame();
        assert!(!f2.crop_region(0, 0, 0, 0));
        assert_eq!((f2.width, f2.height), (8, 6));
    }

    #[test]
    fn a_crop_keeps_the_frame_in_its_own_buffer() {
        // BG9: a window crop runs on every frame of a session; it packs the
        // rows in place instead of into a new buffer.
        let mut f = pattern_frame();
        let ptr = f.data.as_ptr();
        assert!(f.crop_region(2, 1, 4, 4));
        assert_eq!(f.data.as_ptr(), ptr);
        assert_eq!(f.data.len(), 4 * 4 * 4);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(px(&f, x, y), (x as u8 + 2, y as u8 + 1), "at {x},{y}");
            }
        }
    }

    #[test]
    fn crop_region_rejects_rect_beyond_buffer() {
        // Stride promises more rows than the buffer holds.
        let mut f = pattern_frame();
        f.data.truncate(4 * 4 * 2);
        assert!(!f.crop_region(0, 2, 4, 4));
    }
}
