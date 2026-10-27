//! In-process H.264/H.265 encoding via `ffmpeg-next` (roadmap R2#3).
//!
//! Replaces the sidecar's stdin/stdout pipe for the software path: raw BGRA
//! frames go straight into an `AVCodecContext` (sws_scale → libx264/libx265),
//! removing a process hop, two pipe buffers, and the NAL re-framing heuristics
//! from the hot path.
//!
//! On Linux, VAAPI has its own in-process path (`encoder_vaapi`) and NVENC
//! routes through the sidecar. On Windows with the `inproc-windows` feature
//! (BG8), NVENC runs here: BGRA goes straight to `h264_nvenc`/`hevc_nvenc`
//! (NVENC converts on the GPU), a keyframe comes on the frame asked for
//! (`forced-idr`), and the bitrate changes in place ([`InProcessEncoder::set_bitrate`])
//! instead of rebuilding a child process.

use std::collections::VecDeque;
use std::sync::Once;
use std::time::Instant;

use tracing::{trace, warn};

use super::encoder::detect_keyframe;
use super::{EncodedPacket, EncoderPreset, H264Profile, StreamingConfig, VideoCodec, VideoFrame};
#[cfg(target_os = "linux")]
use ffmpeg_next as ffmpeg;
#[cfg(windows)]
use pocketdesk_ffmpeg::ffmpeg_next as ffmpeg;

static FFMPEG_INIT: Once = Once::new();

fn init_ffmpeg() {
    FFMPEG_INIT.call_once(|| {
        if let Err(e) = ffmpeg::init() {
            warn!("FFmpeg init failed (continuing): {}", e);
        }
        ffmpeg::log::set_level(ffmpeg::log::Level::Warning);
    });
}

/// NVENC's preset for the session's (`p1` fastest to `p7` best), as the
/// sidecar maps it.
#[cfg(windows)]
fn nvenc_preset(preset: &EncoderPreset) -> &'static str {
    match preset {
        EncoderPreset::UltraFast | EncoderPreset::SuperFast => "p1",
        EncoderPreset::VeryFast => "p3",
        EncoderPreset::Faster | EncoderPreset::Fast => "p4",
        EncoderPreset::Medium => "p5",
        EncoderPreset::Slow => "p7",
    }
}

/// One FFmpeg encode context, fed from the capture side and drained of
/// codec packets on the streamer side. Zero-latency by construction:
/// `bframes=0`, `rc-lookahead=0`, `tune=zerolatency`, so packets emerge
/// 1:1 with frames (a short FIFO absorbs any residual lag).
pub struct InProcessEncoder {
    config: StreamingConfig,
    encoder: ffmpeg::encoder::Video,
    /// BGRA to YUV 4:2:0 on the CPU, for the software encoders; `None` for a
    /// hardware encoder that takes BGRA itself.
    scaler: Option<(ffmpeg::software::scaling::Context, ffmpeg::frame::Video)>,
    bgra: ffmpeg::frame::Video,
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
        init_ffmpeg();
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
        let scaling = ffmpeg::software::scaling::Context::get(
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
            scaler: Some((scaling, scaled)),
            bgra,
            queue: VecDeque::new(),
            pending: VecDeque::new(),
            sequence: 0,
            keyframe_interval,
            frames_since_keyframe: 0,
        })
    }

    /// NVENC in-process (BG8): BGRA straight to `h264_nvenc`/`hevc_nvenc`,
    /// zero delay, constant bitrate in a small VBV, a keyframe on the frame
    /// asked for (`forced-idr`).
    #[cfg(windows)]
    pub fn new_hardware(config: StreamingConfig) -> anyhow::Result<Self> {
        init_ffmpeg();
        let is_hevc = matches!(config.codec, VideoCodec::H265);
        let name = if is_hevc { "hevc_nvenc" } else { "h264_nvenc" };
        let codec = ffmpeg::encoder::find_by_name(name)
            .ok_or_else(|| anyhow::anyhow!("FFmpeg has no {name}"))?;
        let keyframe_interval = (config.fps as u64) * 2;
        let (maxrate_kbps, bufsize_kbits) =
            super::encoder::vbv_kbit_bounds(config.bitrate_bps, config.fps);

        let context = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut video = context.encoder().video()?;
        video.set_width(config.width);
        video.set_height(config.height);
        video.set_format(ffmpeg::format::Pixel::BGRA);
        video.set_frame_rate(Some(ffmpeg::Rational::new(config.fps as i32, 1)));
        video.set_time_base(ffmpeg::Rational(1, config.fps as i32));
        video.set_gop(keyframe_interval.min(u32::MAX as u64) as u32);
        video.set_max_b_frames(0);
        video.set_bit_rate(config.bitrate_bps as usize);
        video.set_max_bit_rate(maxrate_kbps as usize * 1000);
        // SAFETY: an encoder context not yet opened, set before open.
        unsafe {
            (*video.as_mut_ptr()).rc_buffer_size =
                i32::try_from(bufsize_kbits * 1000).unwrap_or(i32::MAX);
        }

        let mut options = ffmpeg::Dictionary::new();
        options.set("preset", nvenc_preset(&config.preset));
        options.set("tune", "ll");
        options.set("zerolatency", "1");
        options.set("delay", "0");
        options.set("rc", "cbr");
        options.set("forced-idr", "1");
        if !is_hevc {
            options.set("profile", profile_name(&config.profile));
        }
        let encoder = video.open_with(options)?;
        let bgra =
            ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, config.width, config.height);
        Ok(Self {
            config,
            encoder,
            scaler: None,
            bgra,
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
        let sequence = self.sequence;
        let input = match &mut self.scaler {
            Some((scaling, scaled)) => {
                scaling.run(&self.bgra, scaled)?;
                scaled
            }
            None => &mut self.bgra,
        };
        input.set_pts(Some(sequence as i64));
        // libx264/x265, and NVENC with `forced-idr`, honor AVFrame.pict_type=I
        // as "force IDR on this frame".
        unsafe {
            (*input.as_mut_ptr()).pict_type = if force_key {
                ffmpeg::ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffmpeg::ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };
        }
        self.encoder.send_frame(input)?;
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

    /// Its name for the session's record: the software path, or NVENC (BG8).
    pub fn backend_name(&self) -> &'static str {
        if self.scaler.is_some() {
            "in-process"
        } else {
            "in-process-nvenc"
        }
    }

    /// Change the bitrate in place (BG8): NVENC reconfigures on its next
    /// frame, with no rebuild. `false` for the software path, which is
    /// rebuilt as before: libx265 reads the rate only when it opens (BG8
    /// review), and libx264's reconfiguring is not checked on Linux from
    /// here.
    pub fn set_bitrate(&mut self, bitrate_bps: u32) -> bool {
        if self.scaler.is_some() {
            return false;
        }
        let (maxrate_kbps, bufsize_kbits) =
            super::encoder::vbv_kbit_bounds(bitrate_bps, self.config.fps);
        // SAFETY: the context is open and only touched through `&mut self`;
        // libavcodec reads these fields again at each frame it encodes.
        unsafe {
            let context = self.encoder.as_mut_ptr();
            (*context).bit_rate = i64::from(bitrate_bps);
            (*context).rc_max_rate = (maxrate_kbps * 1000) as i64;
            (*context).rc_buffer_size = i32::try_from(bufsize_kbits * 1000).unwrap_or(i32::MAX);
        }
        self.config.bitrate_bps = bitrate_bps;
        true
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

    /// BG8: NVENC in-process on Windows: a packet per frame with no delay, a
    /// keyframe on the frame asked for, and a lower bitrate in place shows
    /// at once in the frames' size. Needs an NVIDIA GPU and FFmpeg's DLLs on
    /// PATH: cargo test -p pocketdesk-core --features inproc-windows -- --ignored nvenc
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn test_inprocess_nvenc_keyframe_on_ask_and_bitrate_in_place() {
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_bps: 8_000_000,
            hardware_encoder: HardwareEncoder::Nvenc,
            preset: EncoderPreset::VeryFast,
            ..software_config(VideoCodec::H264)
        };
        let mut encoder = InProcessEncoder::new_hardware(config).expect("h264_nvenc opens");
        assert_eq!(encoder.backend_name(), "in-process-nvenc");
        let mut sizes = Vec::new();
        for i in 0..90 {
            if i == 40 {
                encoder.request_keyframe();
            }
            if i == 60 {
                assert!(encoder.set_bitrate(1_000_000));
                assert_eq!(encoder.config().bitrate_bps, 1_000_000);
            }
            let packet = encoder
                .encode_frame(&gradient_frame(1280, 720, i))
                .unwrap()
                .expect("zero delay: a packet for each frame");
            assert_eq!(packet.sequence, i as u64, "out in order, no lag");
            if i == 40 {
                assert!(packet.is_keyframe, "the keyframe asked for, on its frame");
            }
            sizes.push(packet.data.len());
        }
        let before: usize = sizes[45..60].iter().sum::<usize>() / 15;
        let after: usize = sizes[70..90].iter().sum::<usize>() / 20;
        assert!(
            after * 3 < before,
            "1 Mbit/s frames ({after} B) well under 8 Mbit/s ones ({before} B)"
        );
    }

    #[test]
    #[ignore]
    fn test_inprocess_request_keyframe() {
        let mut encoder = InProcessEncoder::new(software_config(VideoCodec::H264)).unwrap();
        // BG8 review: the software path is rebuilt for a new rate, as before.
        assert!(!encoder.set_bitrate(500_000));
        assert_eq!(encoder.backend_name(), "in-process");
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
