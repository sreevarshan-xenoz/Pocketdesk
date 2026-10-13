//! In-process VAAPI hardware encoding via raw FFmpeg FFI (roadmap R4 C4 /
//! R2#3 remainder).
//!
//! The software path ([`super::encoder_inproc`]) already drives an
//! `AVCodecContext` in-process, but hardware-accelerated encode needs the
//! `AVHWDeviceContext`/`AVHWFramesContext` pipeline that `ffmpeg-next` 9.0
//! does *not* expose as safe bindings — so this module hand-rolls it through
//! `ffmpeg::ffi` (the re-exported `ffmpeg_sys_next`).
//!
//! Pipeline (per the FFmpeg `hw_transfer`/vaapi-encode reference):
//! `BGRA → sws_scale → NV12 → av_hwframe_transfer_data (hwupload) →
//! avcodec_send_frame → h264_vaapi / hevc_vaapi`.
//!
//! Device selection (the real blocker on hybrid-GPU laptops): FFmpeg's VAAPI
//! backend binds to a specific DRM render node, and only nodes whose driver
//! ships a VA implementation work. On an Intel-iGPU/NVIDIA-dGPU box
//! `/dev/dri/renderD128` is the NVIDIA card (no VA driver →
//! `Failed to initialise VAAPI connection`) while the Intel iHD driver lives
//! on `renderD129`. `open_vaapi_device` therefore *tries* each node and keeps
//! the first that opens, honouring a `POCKETDESK_VAAPI_DEVICE` override for
//! operators who want to pin one. This is why the encoder works out of the box
//! here instead of dying on the hardcoded `renderD128` the sidecar used.

use std::collections::VecDeque;
use std::ffi::CString;
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::Once;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::encoder::detect_keyframe;
use super::{H264Profile, StreamingConfig, VideoCodec, VideoFrame};
use crate::streaming::EncodedPacket;
use ffmpeg_next as ffmpeg;

static FFMPEG_INIT: Once = Once::new();

/// Environment override for the VAAPI render node (e.g. an operator pinning
/// `/dev/dri/renderD130`). When set, only that node is attempted.
pub const VAAPI_DEVICE_ENV: &str = "POCKETDESK_VAAPI_DEVICE";

/// First DRM render node index on Linux; subsequent ones are probed for
/// multi-GPU boxes.
const FIRST_RENDER_NODE: u32 = 128;
const RENDER_NODE_SCAN: u32 = 8;

/// `AVERROR_EOF` (`FFERRTAG('E','O','F',' ')`) — bindgen does not emit the
/// macro form, so reconstruct it: `-(MKTAG)`, `MKTAG(a,b,c,d)=a|b<<8|c<<16|d<<24`.
const AVERROR_EOF: c_int = {
    let mktag = (b'E' as u32) | (b'O' as u32) << 8 | (b'F' as u32) << 16 | (b' ' as u32) << 24;
    -(mktag as i32)
};

/// `AVERROR(EAGAIN)` on Linux is `-EAGAIN`.
fn averror_eagain() -> c_int {
    -libc::EAGAIN
}

/// Translate an FFmpeg `AVERROR` code into its human string (diagnostics).
fn describe_err(code: c_int) -> String {
    use ffmpeg::ffi;
    let mut buf = [0i8; 128];
    unsafe {
        if ffi::av_strerror(code, buf.as_mut_ptr(), buf.len()) == 0 {
            let cstr = std::ffi::CStr::from_ptr(buf.as_ptr());
            cstr.to_string_lossy().into_owned()
        } else {
            format!("AVERROR({code})")
        }
    }
}

/// Order the DRM render nodes to attempt. A `Some(override)` short-circuits to
/// a single candidate; otherwise the ascending scan of the standard indices
/// that the caller reports as present.
///
/// Pure so the hybrid-GPU ordering bug (never assume `renderD128` is usable)
/// is unit-testable without a GPU.
fn vaapi_device_candidates(present: &[PathBuf], override_node: Option<&str>) -> Vec<PathBuf> {
    if let Some(node) = override_node.filter(|s| !s.is_empty()) {
        return vec![PathBuf::from(node)];
    }
    let mut candidates: Vec<PathBuf> = (FIRST_RENDER_NODE..FIRST_RENDER_NODE + RENDER_NODE_SCAN)
        .map(|n| PathBuf::from(format!("/dev/dri/renderD{n}")))
        .filter(|p| present.iter().any(|e| e == p))
        .collect();
    if candidates.is_empty() {
        // Fall back to the caller's enumeration (covers non-standard indices)
        // so an oddly-numbered node is still tried rather than skipped.
        candidates = present.to_vec();
        candidates.sort();
    }
    candidates
}

/// Enumerate `/dev/dri/renderD*` as it exists right now.
fn list_render_nodes() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("renderD") {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

/// Try each candidate render node until `av_hwdevice_ctx_create` succeeds,
/// returning the live device buffer pointer and the node that worked.
///
/// # Safety
/// Returns a fresh, solely-owned `AVBufferRef` the caller must `av_buffer_unref`
/// (this module does so in `Drop`).
unsafe fn open_vaapi_device() -> anyhow::Result<(*mut ffmpeg::ffi::AVBufferRef, String)> {
    use ffmpeg::ffi;

    let override_node = std::env::var(VAAPI_DEVICE_ENV).ok();
    let present = list_render_nodes();
    let candidates = vaapi_device_candidates(&present, override_node.as_deref());

    if candidates.is_empty() {
        anyhow::bail!("no /dev/dri/renderD* nodes present for VAAPI");
    }

    let mut last_err = String::from("none attempted");
    for node in &candidates {
        let Ok(c_node) = CString::new(node.to_string_lossy().as_ref()) else {
            continue;
        };
        let mut ref_ptr: *mut ffi::AVBufferRef = std::ptr::null_mut();
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut ref_ptr,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                c_node.as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 && !ref_ptr.is_null() {
            let device = node.display().to_string();
            info!("VAAPI device opened on {device} (C4 in-process)");
            return Ok((ref_ptr, device));
        }
        last_err = format!("av_hwdevice_ctx_create({}) -> {ret}", node.display());
        debug!("{last_err}");
    }
    anyhow::bail!("no usable VAAPI device ({last_err})")
}

/// In-process VAAPI H.264/HEVC encoder backed by hand-rolled FFmpeg FFI.
pub struct VaapiEncoder {
    config: StreamingConfig,
    device_path: String,
    codec_name: &'static str,
    encoder: *mut ffmpeg::ffi::AVCodecContext,
    hw_device_ref: *mut ffmpeg::ffi::AVBufferRef,
    hw_frames_ref: *mut ffmpeg::ffi::AVBufferRef,
    /// Reusable VAAPI upload target (hardware format).
    hw_frame: *mut ffmpeg::ffi::AVFrame,
    /// Reusable Annex-B output packet.
    packet: *mut ffmpeg::ffi::AVPacket,
    /// BGRA scratch frame fed to the scaler.
    bgra: ffmpeg::frame::Video,
    /// NV12 scaler output, source for `av_hwframe_transfer_data`.
    nv12: ffmpeg::frame::Video,
    scaler: ffmpeg::software::scaling::Context,
    queue: VecDeque<(u64, Instant)>,
    pending: VecDeque<EncodedPacket>,
    sequence: u64,
    keyframe_interval: u64,
    frames_since_keyframe: u64,
}

// SAFETY: the AVCodecContext and its hardware device/frames are only touched
// through `&mut self`, and the streamer confines all calls to the single
// `video_encode` task. FFmpeg contexts are thread-affine but move-safe
// between sequential uses — same posture as `InProcessEncoder`.
unsafe impl Send for VaapiEncoder {}

fn h264_profile_vaapi(profile: &H264Profile) -> &'static str {
    match profile {
        H264Profile::Baseline => "constrained_baseline",
        H264Profile::Main => "main",
        H264Profile::High => "high",
    }
}

impl VaapiEncoder {
    pub fn new(config: StreamingConfig) -> anyhow::Result<Self> {
        FFMPEG_INIT.call_once(|| {
            if let Err(e) = ffmpeg::init() {
                warn!("FFmpeg init failed (continuing): {}", e);
            }
            ffmpeg::log::set_level(ffmpeg::log::Level::Warning);
        });

        use ffmpeg::ffi;

        let is_hevc = matches!(config.codec, VideoCodec::H265);
        let codec_name: &'static str = if is_hevc { "hevc_vaapi" } else { "h264_vaapi" };
        let keyframe_interval = (config.fps as u64) * 2;

        let (mut device_ref, device_path) = unsafe { open_vaapi_device()? };

        // --- codec context ---
        let c_codec = CString::new(codec_name)?;
        let codec = unsafe { ffi::avcodec_find_encoder_by_name(c_codec.as_ptr()) };
        if codec.is_null() {
            unsafe { ffi::av_buffer_unref(&mut device_ref) };
            anyhow::bail!("VAAPI encoder '{codec_name}' not available in FFmpeg");
        }
        let mut ctx = unsafe { ffi::avcodec_alloc_context3(codec) };
        if ctx.is_null() {
            unsafe { ffi::av_buffer_unref(&mut device_ref) };
            anyhow::bail!("avcodec_alloc_context3 failed");
        }

        unsafe {
            (*ctx).bit_rate = config.bitrate_bps as i64;
            (*ctx).rc_max_rate = config.bitrate_bps as i64;
            (*ctx).rc_buffer_size = (config.bitrate_bps as i64 * 2 / config.fps as i64) as c_int;
            (*ctx).global_quality = 0; // rate-control mode, not fixed-QP
            (*ctx).width = config.width as c_int;
            (*ctx).height = config.height as c_int;
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: config.fps as c_int,
            };
            (*ctx).framerate = ffi::AVRational {
                num: config.fps as c_int,
                den: 1,
            };
            (*ctx).gop_size = keyframe_interval.min(c_int::MAX as u64) as c_int;
            (*ctx).max_b_frames = 0; // zero-latency
            (*ctx).pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*ctx).hw_device_ctx = ffi::av_buffer_ref(device_ref);
        }

        // --- hardware frames context (NV12 surfaces on the VA device) ---
        let mut hw_frames_ref = unsafe { ffi::av_hwframe_ctx_alloc(device_ref) };
        if hw_frames_ref.is_null() {
            unsafe {
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut device_ref);
            }
            anyhow::bail!("av_hwframe_ctx_alloc failed");
        }
        let pool_size: c_int = if is_hevc { 32 } else { 20 };
        let init_ret = unsafe {
            let frames = (*hw_frames_ref).data as *mut ffi::AVHWFramesContext;
            (*frames).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*frames).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
            (*frames).width = config.width as c_int;
            (*frames).height = config.height as c_int;
            (*frames).initial_pool_size = pool_size;
            ffi::av_hwframe_ctx_init(hw_frames_ref)
        };
        if init_ret < 0 {
            unsafe {
                ffi::av_buffer_unref(&mut hw_frames_ref);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut device_ref);
            }
            anyhow::bail!("av_hwframe_ctx_init failed ({init_ret})");
        }
        unsafe {
            (*ctx).hw_frames_ctx = ffi::av_buffer_ref(hw_frames_ref);
        }

        // --- open codec ---
        // h264_vaapi honours `profile` as an AVOption; hevc_vaapi does not
        // accept that name and would fail to open, so only set it for H.264.
        let mut opts: *mut ffi::AVDictionary = std::ptr::null_mut();
        if !is_hevc {
            let c_profile = CString::new(h264_profile_vaapi(&config.profile))?;
            unsafe {
                ffi::av_dict_set(&mut opts, c"profile".as_ptr(), c_profile.as_ptr(), 0);
            }
        }
        let open_ret = unsafe { ffi::avcodec_open2(ctx, codec, &mut opts) };
        unsafe {
            if !opts.is_null() {
                ffi::av_dict_free(&mut opts);
            }
        }
        if open_ret < 0 {
            unsafe {
                ffi::av_buffer_unref(&mut hw_frames_ref);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut device_ref);
            }
            anyhow::bail!("avcodec_open2({codec_name}) failed ({open_ret})");
        }

        // --- reusable VAAPI frame + packet ---
        let mut hw_frame = unsafe { ffi::av_frame_alloc() };
        let mut packet = unsafe { ffi::av_packet_alloc() };
        if hw_frame.is_null() || packet.is_null() {
            unsafe {
                if !packet.is_null() {
                    ffi::av_packet_free(&mut packet);
                }
                if !hw_frame.is_null() {
                    ffi::av_frame_free(&mut hw_frame);
                }
                ffi::av_buffer_unref(&mut hw_frames_ref);
                ffi::avcodec_free_context(&mut ctx);
                ffi::av_buffer_unref(&mut device_ref);
            }
            anyhow::bail!("av_frame_alloc/av_packet_alloc failed");
        }
        unsafe {
            (*hw_frame).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as c_int;
        }

        // --- CPU-side scaler + scratch frames ---
        let bgra =
            ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, config.width, config.height);
        let nv12 =
            ffmpeg::frame::Video::new(ffmpeg::format::Pixel::NV12, config.width, config.height);
        let scaler = ffmpeg::software::scaling::Context::get(
            ffmpeg::format::Pixel::BGRA,
            config.width,
            config.height,
            ffmpeg::format::Pixel::NV12,
            config.width,
            config.height,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        )?;

        info!("Video encoder: in-process VAAPI ({codec_name}) on {device_path}");

        Ok(Self {
            config,
            device_path,
            codec_name,
            encoder: ctx,
            hw_device_ref: device_ref,
            hw_frames_ref,
            hw_frame,
            packet,
            bgra,
            nv12,
            scaler,
            queue: VecDeque::new(),
            pending: VecDeque::new(),
            sequence: 0,
            keyframe_interval,
            frames_since_keyframe: 0,
        })
    }

    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// The VAAPI codec name in use (`h264_vaapi` / `hevc_vaapi`).
    pub fn codec_name(&self) -> &'static str {
        self.codec_name
    }

    pub fn encode_frame(&mut self, frame: &VideoFrame) -> anyhow::Result<Option<EncodedPacket>> {
        use ffmpeg::ffi;

        self.frames_since_keyframe += 1;
        let force_key = self.frames_since_keyframe >= self.keyframe_interval;
        if force_key {
            self.frames_since_keyframe = 0;
        }

        self.copy_frame(frame);
        self.scaler.run(&self.bgra, &mut self.nv12)?;

        // Upload NV12 → VAAPI surface: reset the reusable hw_frame, attach the
        // frames context, grab a buffer, then hwupload via transfer_data.
        let hw = self.hw_frame;
        let sw_ptr = unsafe { self.nv12.as_mut_ptr() };
        let sequence = self.sequence;
        unsafe {
            ffi::av_frame_unref(hw);
            // Allocate a fresh VAAPI surface from the pool via the purpose-built
            // helper; it sets format/geometry/hw_frames_ctx itself.
            let gb = ffi::av_hwframe_get_buffer(self.hw_frames_ref, hw, 0);
            if gb < 0 {
                anyhow::bail!(
                    "av_hwframe_get_buffer(hw) failed ({gb}: {})",
                    describe_err(gb)
                );
            }
            let tr = ffi::av_hwframe_transfer_data(hw, sw_ptr, 0);
            if tr < 0 {
                anyhow::bail!("av_hwframe_transfer_data (hwupload) failed ({tr})");
            }
            (*hw).pts = sequence as i64;
            (*hw).pict_type = if force_key {
                ffi::AVPictureType::AV_PICTURE_TYPE_I
            } else {
                ffi::AVPictureType::AV_PICTURE_TYPE_NONE
            };

            let sr = ffi::avcodec_send_frame(self.encoder, hw);
            if sr < 0 && sr != averror_eagain() {
                anyhow::bail!("avcodec_send_frame failed ({sr})");
            }
        }
        self.queue.push_back((sequence, frame.timestamp));
        self.sequence += 1;

        self.receive_packets()?;
        Ok(self.pending.pop_front())
    }

    pub fn drain(&mut self) -> anyhow::Result<Vec<EncodedPacket>> {
        use ffmpeg::ffi;
        unsafe {
            // NULL frame flushes the encoder.
            let _ = ffi::avcodec_send_frame(self.encoder, std::ptr::null());
        }
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
        use ffmpeg::ffi;
        loop {
            let ret = unsafe { ffi::avcodec_receive_packet(self.encoder, self.packet) };
            if ret == averror_eagain() || ret == AVERROR_EOF {
                break;
            }
            if ret < 0 {
                anyhow::bail!("avcodec_receive_packet failed ({ret})");
            }
            let (sequence, timestamp) = self
                .queue
                .pop_front()
                .unwrap_or((self.sequence, Instant::now()));
            let (data, key_flag) = unsafe {
                let pkt = &*self.packet;
                let len = if pkt.size > 0 { pkt.size as usize } else { 0 };
                let data = if pkt.data.is_null() || len == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(pkt.data, len).to_vec()
                };
                let key = pkt.flags & ffi::AV_PKT_FLAG_KEY != 0;
                ffi::av_packet_unref(self.packet);
                (data, key)
            };
            let is_keyframe = key_flag || detect_keyframe(&data);
            debug!(
                "vaapi packet seq={} {}B key={}",
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
        Ok(())
    }
}

impl Drop for VaapiEncoder {
    fn drop(&mut self) {
        use ffmpeg::ffi;
        unsafe {
            ffi::av_packet_free(&mut self.packet);
            ffi::av_frame_free(&mut self.hw_frame);
            ffi::avcodec_free_context(&mut self.encoder);
            ffi::av_buffer_unref(&mut self.hw_frames_ref);
            ffi::av_buffer_unref(&mut self.hw_device_ref);
        }
    }
}

/// Whether a VAAPI device can be opened right now — used by `VideoEncoder` to
/// decide if the in-process hardware rung is viable before committing.
pub fn vaapi_available() -> bool {
    use ffmpeg::ffi;
    let override_node = std::env::var(VAAPI_DEVICE_ENV).ok();
    let present = list_render_nodes();
    for node in vaapi_device_candidates(&present, override_node.as_deref()) {
        let Ok(c_node) = CString::new(node.to_string_lossy().as_ref()) else {
            continue;
        };
        let mut ref_ptr: *mut ffi::AVBufferRef = std::ptr::null_mut();
        let ret = unsafe {
            ffi::av_hwdevice_ctx_create(
                &mut ref_ptr,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                c_node.as_ptr(),
                std::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 && !ref_ptr.is_null() {
            unsafe {
                ffi::av_buffer_unref(&mut ref_ptr);
            }
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::encoder::find_start_code;

    fn nodes(indices: &[u32]) -> Vec<PathBuf> {
        indices
            .iter()
            .map(|n| PathBuf::from(format!("/dev/dri/renderD{n}")))
            .collect()
    }

    #[test]
    fn candidates_respect_override() {
        let c = vaapi_device_candidates(&nodes(&[128, 129]), Some("/dev/dri/renderD130"));
        assert_eq!(c, vec![PathBuf::from("/dev/dri/renderD130")]);
    }

    #[test]
    fn candidates_scan_in_order_and_drop_absent() {
        // Only 129 exists (the hybrid box where 128 is the NVIDIA node with no
        // VA driver): the scan must still yield 129, and not fabricate 128.
        let present = nodes(&[129]);
        let c = vaapi_device_candidates(&present, None);
        assert_eq!(c, vec![PathBuf::from("/dev/dri/renderD129")]);
    }

    #[test]
    fn candidates_try_all_present_in_ascending_order() {
        let present = nodes(&[130, 128, 129]);
        let c = vaapi_device_candidates(&present, None);
        assert_eq!(
            c,
            vec![
                PathBuf::from("/dev/dri/renderD128"),
                PathBuf::from("/dev/dri/renderD129"),
                PathBuf::from("/dev/dri/renderD130"),
            ]
        );
    }

    #[test]
    fn averror_eof_matches_ffmpeg_value() {
        // FFmpeg: AVERROR_EOF == -541478725
        assert_eq!(AVERROR_EOF, -541478725);
    }

    /// Real in-process VAAPI encode. Runs on any box whose VAAPI device opens;
    /// self-skips (returns Ok) where no VA-capable node exists, so CI and
    /// NVIDIA-only hosts don't fail. On this Intel-iHD host it exercises the
    /// full BGRA→NV12→hwupload→h264_vaapi path and asserts Annex-B output.
    #[test]
    fn test_vaapi_encode_roundtrip() {
        if !vaapi_available() {
            eprintln!("skipping VAAPI encode test: no usable device on this host");
            return;
        }
        // H.264 is the must-work rung; HEVC is best-effort (driver-dependent).
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 2_000_000,
            codec: VideoCodec::H264,
            ..StreamingConfig::default()
        };
        let mut encoder = VaapiEncoder::new(config).expect("H.264 VAAPI encoder must open");
        assert_eq!(encoder.codec_name, "h264_vaapi");

        let mut packets = Vec::new();
        for i in 0..40 {
            let frame = gradient_frame(1280, 720, i);
            if let Some(p) = encoder.encode_frame(&frame).unwrap() {
                packets.push(p);
            }
        }
        packets.extend(encoder.drain().unwrap());
        assert!(!packets.is_empty(), "VAAPI produced no packets");

        let total: usize = packets.iter().map(|p| p.data.len()).sum();
        for p in &packets {
            assert!(!p.data.is_empty());
            assert!(
                find_start_code(&p.data, 0).is_some(),
                "VAAPI packet seq={} lacks an Annex-B start code",
                p.sequence
            );
        }
        assert!(
            packets.iter().any(|p| p.is_keyframe),
            "expected at least one keyframe"
        );
        info!(
            "VAAPI roundtrip ok: {} pkts, {total}B, device={}",
            packets.len(),
            encoder.device_path()
        );
    }

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
}
