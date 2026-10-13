//! In-process H.264/H.265 encoding via `ffmpeg-next` (roadmap R2#3).
//!
//! Replaces the sidecar's stdin/stdout pipe for the software path: raw BGRA
//! frames go straight into an `AVCodecContext` (sws_scale → libx264/libx265),
//! removing a process hop, two pipe buffers, and the NAL re-framing heuristics
//! from the hot path.
//!
//! VAAPI/NVENC still route through the sidecar backend: hw-accelerated
//! encoding needs a full `AVHWDeviceContext`/`AVHWFramesContext` pipeline
//! (hwupload equivalent), which is a follow-up on top of this module.

use std::collections::VecDeque;
use std::sync::Once;
use std::time::Instant;

use tracing::{trace, warn};

use super::encoder::detect_keyframe;
use super::{EncodedPacket, EncoderPreset, H264Profile, StreamingConfig, VideoCodec, VideoFrame};
use ffmpeg_next as ffmpeg;

static FFMPEG_INIT: Once = Once::new();

/// One FFmpeg encode context, fed from the capture side and drained of
/// codec packets on the streamer side. Zero-latency by construction:
/// `bframes=0`, `rc-lookahead=0`, `tune=zerolatency`, so packets emerge
/// 1:1 with frames (a short FIFO absorbs any residual lag).
pub struct InProcessEncoder {
    config: StreamingConfig,
    encoder: ffmpeg::encoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    bgra: ffmpeg::frame::Video,
    scaled: ffmpeg::frame::Video,
    /// (sequence, timestamp) of frames submitted but not yet emitted.
    queue: VecDeque<(u64, Instant)>,
    /// Packets received early relative to `encode_frame` calls.
    pending: VecDeque<EncodedPacket>,
    sequence: u64,
    keyframe_interval: u64,
    frames_since_keyframe: u64,
}

// SAFETY: the AVCodecContext is only ever touched through `&mut self`, and
// the streamer confines all calls to the single `video_encode` task. FFmpeg
// contexts are thread-affine but move-safe between sequential uses.
unsafe impl Send for InProcessEncoder {}

fn preset_name(preset: &EncoderPreset) -> &'static str {
    match preset {
        EncoderPreset::UltraFast => "ultrafast",
        EncoderPreset::SuperFast => "superfast",
        EncoderPreset::VeryFast => "veryfast",
        EncoderPreset::Faster => "faster",
        EncoderPreset::Fast => "fast",
        EncoderPreset::Medium => "medium",
        EncoderPreset::Slow => "slow",
    }
}

fn profile_name(profile: &H264Profile) -> &'static str {
    match profile {
        H264Profile::Baseline => "baseline",
        H264Profile::Main => "main",
        H264Profile::High => "high",
    }
}

impl InProcessEncoder {
    pub fn new(config: StreamingConfig) -> anyhow::Result<Self> {
        FFMPEG_INIT.call_once(|| {
            if let Err(e) = ffmpeg::init() {
                warn!("FFmpeg init failed (continuing): {}", e);
            }
            ffmpeg::log::set_level(ffmpeg::log::Level::Warning);
        });

        let is_hevc = matches!(config.codec, VideoCodec::H265);
        let codec = ffmpeg::encoder::find_by_name(if is_hevc { "libx265" } else { "libx264" })
            .or_else(|| {
                ffmpeg::encoder::find(if is_hevc {
                    ffmpeg::codec::Id::HEVC
                } else {
                    ffmpeg::codec::Id::H264
                })
            })
            .ok_or_else(|| anyhow::anyhow!("no software H.264/H.265 encoder in FFmpeg"))?;

        let keyframe_interval = (config.fps as u64) * 2;
        let (maxrate_kbps, bufsize_kbits) =
            super::encoder::vbv_kbit_bounds(config.bitrate_bps, config.fps);

        let context = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut video = context.encoder().video()?;
        video.set_width(config.width);
        video.set_height(config.height);
        video.set_format(ffmpeg::format::Pixel::YUV420P);
        video.set_frame_rate(Some(ffmpeg::Rational::new(config.fps as i32, 1)));
        video.set_time_base(ffmpeg::Rational(1, config.fps as i32));
        video.set_gop(keyframe_interval.min(u32::MAX as u64) as u32);
        video.set_bit_rate(config.bitrate_bps as usize);

        let mut options = ffmpeg::Dictionary::new();
        options.set("preset", preset_name(&config.preset));
        if is_hevc {
            options.set(
                "x265-params",
                &format!(
                    "keyint={}:min-keyint=1:bframes=0:rc-lookahead=0:vbv-maxrate={}:vbv-bufsize={}",
                    keyframe_interval, maxrate_kbps, bufsize_kbits
                ),
            );
        } else {
            options.set("tune", "zerolatency");
            options.set(
                "x264-params",
                &format!(
                    "profile={}:keyint={}:min-keyint=1:bframes=0:rc-lookahead=0:vbv-maxrate={}:vbv-bufsize={}",
                    profile_name(&config.profile),
                    keyframe_interval,
                    maxrate_kbps,
                    bufsize_kbits
                ),
            );
        }
        let encoder = video.open_with(options)?;

        let bgra =
            ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, config.width, config.height);
        let scaled =
            ffmpeg::frame::Video::new(ffmpeg::format::Pixel::YUV420P, config.width, config.height);
        let scaler = ffmpeg::software::scaling::Context::get(
            ffmpeg::format::Pixel::BGRA,
            config.width,
            config.height,
            ffmpeg::format::Pixel::YUV420P,
            config.width,
            config.height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )?;

        Ok(Self {
            config,
            encoder,
            scaler,
            bgra,
            scaled,
            queue: VecDeque::new(),
            pending: VecDeque::new(),
            sequence: 0,
            keyframe_interval,
            frames_since_keyframe: 0,
        })
    }

    pub fn encode_frame(&mut self, frame: &VideoFrame) -> anyhow::Result<Option<EncodedPacket>> {
        self.frames_since_keyframe += 1;
        let force_key = self.frames_since_keyframe >= self.keyframe_interval;
        if force_key {
            self.frames_since_keyframe = 0;
        }

        self.copy_frame(frame);
        self.scaler.run(&self.bgra, &mut self.scaled)?;
        let sequence = self.sequence;
        self.scaled.set_pts(Some(sequence as i64));
        // libx264/x265 honor AVFrame.pict_type=I as "force IDR on this frame".
        unsafe {
            (*self.scaled.as_mut_ptr()).pict_type = if force_key {
                ffmpeg::ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffmpeg::ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
        }
        self.encoder.send_frame(&self.scaled)?;
        self.queue.push_back((sequence, frame.timestamp));
        self.sequence += 1;

        self.receive_packets()?;
        Ok(self.pending.pop_front())
    }

    pub fn drain(&mut self) -> anyhow::Result<Vec<EncodedPacket>> {
        self.encoder.send_eof()?;
        self.receive_packets()?;
        Ok(self.pending.drain(..).collect())
    }

    pub fn request_keyframe(&mut self) {
        self.frames_since_keyframe = self.keyframe_interval;
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }

    fn copy_frame(&mut self, frame: &VideoFrame) {
        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let src_stride = frame.stride as usize;
        let dst_stride = self.bgra.stride(0);
        let row = width * 4;
        let dst = self.bgra.data_mut(0);
        for y in 0..height {
            let src_range = y * src_stride..y * src_stride + row;
            let dst_range = y * dst_stride..y * dst_stride + row;
            if src_range.end > frame.data.len() || dst_range.end > dst.len() {
                break;
            }
            dst[dst_range].copy_from_slice(&frame.data[src_range]);
        }
    }

    fn receive_packets(&mut self) -> anyhow::Result<()> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    let (sequence, timestamp) = self
                        .queue
                        .pop_front()
                        .unwrap_or((self.sequence, Instant::now()));
                    let data = packet.data().unwrap_or_default().to_vec();
                    let is_keyframe = packet.is_key() || detect_keyframe(&data);
                    trace!(
                        "in-process packet seq={} {}B key={}",
                        sequence,
                        data.len(),
                        is_keyframe
                    );
                    self.pending.push_back(EncodedPacket {
                        data,
                        is_keyframe,
                        timestamp,
                        sequence,
                    });
                }
                Err(ffmpeg::Error::Other {
                    errno: libc::EAGAIN,
                }) => break,
                // Reached end of stream after drain() flushed the encoder.
                Err(ffmpeg::Error::Eof) => break,
                Err(e) => return Err(anyhow::anyhow!("FFmpeg receive_packet: {}", e)),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::HardwareEncoder;
    use crate::streaming::encoder::find_start_code;

    fn gradient_frame(w: u32, h: u32, shift: usize) -> VideoFrame {
        let stride = (w * 4) as usize;
        let mut data = vec![0u8; stride * h as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i + shift * 7) % 251) as u8;
        }
        VideoFrame {
            data,
            width: w,
            height: h,
            stride: stride as u32,
            timestamp: Instant::now(),
        }
    }

    fn software_config(codec: VideoCodec) -> StreamingConfig {
        StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Software,
            monitor_index: 0,
        }
    }

    /// Real libx264 encode through the in-process path (needs FFmpeg with
    /// x264 at runtime). Run with: cargo test -p pocketdesk-core -- --ignored
    #[test]
    #[ignore]
    fn test_inprocess_encode_annexb_roundtrip() {
        let mut encoder = InProcessEncoder::new(software_config(VideoCodec::H264)).unwrap();

        let mut packets = Vec::new();
        for i in 0..70 {
            let frame = gradient_frame(320, 240, i);
            if let Some(packet) = encoder.encode_frame(&frame).unwrap() {
                packets.push(packet);
            }
        }
        packets.extend(encoder.drain().unwrap());

        assert!(
            packets.len() >= 65,
            "zero-latency path should emit ~1 packet/frame, got {}",
            packets.len()
        );

        // Every packet must be Annex-B (start code) — that is what the
        // MediaCodec client consumes verbatim.
        for packet in &packets {
            assert!(!packet.data.is_empty());
            assert!(
                find_start_code(&packet.data, 0).is_some(),
                "packet seq={} lacks an Annex-B start code",
                packet.sequence
            );
        }

        // First packet is the IDR (with in-band SPS/PPS from the wrapper).
        assert!(packets[0].is_keyframe);
        // Keyframe interval (fps*2 = 60 frames here) must recur.
        assert!(
            packets.iter().any(|p| p.is_keyframe && p.sequence >= 59),
            "expected a periodic keyframe at the 60-frame interval"
        );
    }

    #[test]
    #[ignore]
    fn test_inprocess_request_keyframe() {
        let mut encoder = InProcessEncoder::new(software_config(VideoCodec::H264)).unwrap();
        for i in 0..5 {
            let _ = encoder.encode_frame(&gradient_frame(320, 240, i)).unwrap();
        }
        encoder.request_keyframe();
        let packet = encoder
            .encode_frame(&gradient_frame(320, 240, 99))
            .unwrap()
            .expect("zero-latency: packet ready");
        assert!(packet.is_keyframe, "forced frame should encode as IDR");
    }

    #[test]
    #[ignore]
    fn test_inprocess_hevc_roundtrip() {
        let mut encoder = InProcessEncoder::new(software_config(VideoCodec::H265)).unwrap();
        for i in 0..10 {
            let _ = encoder.encode_frame(&gradient_frame(320, 240, i)).unwrap();
        }
        let mut emitted = 0usize;
        let mut bytes = 0usize;
        for i in 0..10 {
            if let Some(packet) = encoder.encode_frame(&gradient_frame(320, 240, i)).unwrap() {
                emitted += 1;
                bytes += packet.data.len();
            }
        }
        let drained = encoder.drain().unwrap();
        let drained_bytes: usize = drained.iter().map(|p| p.data.len()).sum();
        assert!(
            emitted + drained.len() >= 9 && bytes + drained_bytes > 0,
            "libx265: emitted={emitted}B{bytes} drained={}B{drained_bytes}",
            drained.len()
        );
    }
}
