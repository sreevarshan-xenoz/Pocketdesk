//! In-process VAAPI hardware encoding via raw FFmpeg FFI (roadmap R4 C4 /
//! R2#3 remainder).
//!
//! The software path ([`super::encoder_inproc`]) already drives an
//! `AVCodecContext` in-process, but hardware-accelerated encode needs the
//! `AVHWDeviceContext`/`AVHWFramesContext` pipeline that `ffmpeg-next` 9.0
//! does *not* expose as safe bindings — so this module hand-rolls it through
//! `ffmpeg::ffi` (the re-exported `ffmpeg_sys_next`).
//!
//! Pipeline (per the FFmpeg `hw_transfer`/vaapi-encode reference), by
//! default with the colour conversion on the card:
//! `BGRA → av_hwframe_transfer_data (hwupload, a BGR0 surface) →
//! scale_vaapi (the VA-API video processor, to NV12) → avcodec_send_frame →
//! h264_vaapi / hevc_vaapi`. The processor does no conversion; the frame's
//! bytes go from the capture's buffer to the surface without a copy of ours.
//! Where the card's processor cannot take that (a driver without BGR0
//! surfaces or without `scale_vaapi`) the encoder logs once and converts on
//! the processor as it always did: `BGRA → sws_scale → NV12 → hwupload`.
//! `POCKETDESK_VAAPI_CPU_CONVERT=1` forces that path (for A/B timing).
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
use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tracing::{debug, info, warn};

use super::encoder::detect_keyframe;
use super::latency_trace::{self, Step};
use super::{H264Profile, StreamingConfig, VideoCodec, VideoFrame};
use crate::streaming::EncodedPacket;
use ffmpeg_next as ffmpeg;

static FFMPEG_INIT: Once = Once::new();

pub use super::encoder::VAAPI_DEVICE_ENV;

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
    /// Reusable VAAPI NV12 frame handed to the codec (the upload target on
    /// the processor path, the video processor's output on the card path).
    hw_frame: *mut ffmpeg::ffi::AVFrame,
    /// Reusable Annex-B output packet.
    packet: *mut ffmpeg::ffi::AVPacket,
    convert: Convert,
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

/// How a captured BGRA frame becomes the NV12 surface the codec takes.
enum Convert {
    /// `sws_scale` on the processor, then `hwupload` of the NV12.
    Cpu(CpuConvert),
    /// Upload of the BGRA and the conversion by the card's video processor.
    Gpu(GpuConvert),
}

/// The conversion on the processor (the path every host can take).
struct CpuConvert {
    /// BGRA scratch frame fed to the scaler.
    bgra: ffmpeg::frame::Video,
    /// NV12 scaler output, source for `av_hwframe_transfer_data`.
    nv12: ffmpeg::frame::Video,
    scaler: ffmpeg::software::scaling::Context,
}

impl CpuConvert {
    /// Copy the capture's rows into the scaler's own BGRA frame.
    fn copy_frame(&mut self, frame: &VideoFrame, width: u32, height: u32) {
        let width = width as usize;
        let height = height as usize;
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
}

/// The conversion on the card: the capture's BGRA bytes are uploaded to a
/// BGR0 VA surface (the alpha byte is ignored) and FFmpeg's `scale_vaapi`
/// runs the VA-API video processor from there to an NV12 surface, which is
/// what the codec takes. The processor does no per-pixel work.
struct GpuConvert {
    graph: *mut ffmpeg::ffi::AVFilterGraph,
    src: *mut ffmpeg::ffi::AVFilterContext,
    sink: *mut ffmpeg::ffi::AVFilterContext,
    /// The BGR0 surface pool the bytes are uploaded to.
    upload_frames_ref: *mut ffmpeg::ffi::AVBufferRef,
    /// Reusable BGR0 surface (handed to the graph, which takes it).
    upload: *mut ffmpeg::ffi::AVFrame,
    /// A host-memory view of the capture's bytes: pointers only, no buffer.
    view: *mut ffmpeg::ffi::AVFrame,
    /// Set when a frame went into the graph but none came out: the graph
    /// may still hold it, and every later frame would come out one behind.
    /// Each call then fails, so the streamer opens a new encoder.
    stuck: bool,
}

/// The conversion matrix and range the processor path has always produced:
/// swscale's default for RGB to YUV is BT.601, limited range. The stream
/// carries no colour tags, so a viewer reads it as it always did.
const VPP_ARGS: &CStr = c"format=nv12:out_color_matrix=smpte170m:out_range=tv";

impl GpuConvert {
    /// Build the graph on `device_ref` and push one blank frame through it,
    /// so a driver that cannot do this fails here and not on the first frame.
    /// Returns the converter and a new reference to the NV12 frames context
    /// the processor makes its output in, which the codec must be opened
    /// with (the caller owns that reference).
    fn new(
        device_ref: *mut ffmpeg::ffi::AVBufferRef,
        width: u32,
        height: u32,
        fps: u32,
    ) -> anyhow::Result<(Self, *mut ffmpeg::ffi::AVBufferRef)> {
        use ffmpeg::ffi;
        let mut gc = Self {
            graph: std::ptr::null_mut(),
            src: std::ptr::null_mut(),
            sink: std::ptr::null_mut(),
            upload_frames_ref: std::ptr::null_mut(),
            upload: std::ptr::null_mut(),
            view: std::ptr::null_mut(),
            stuck: false,
        };
        unsafe {
            gc.upload_frames_ref = ffi::av_hwframe_ctx_alloc(device_ref);
            anyhow::ensure!(
                !gc.upload_frames_ref.is_null(),
                "av_hwframe_ctx_alloc failed"
            );
            let frames = (*gc.upload_frames_ref).data as *mut ffi::AVHWFramesContext;
            (*frames).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
            (*frames).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_BGR0;
            (*frames).width = width as c_int;
            (*frames).height = height as c_int;
            (*frames).initial_pool_size = 4;
            let r = ffi::av_hwframe_ctx_init(gc.upload_frames_ref);
            anyhow::ensure!(r >= 0, "BGR0 surfaces: {}", describe_err(r));

            gc.graph = ffi::avfilter_graph_alloc();
            anyhow::ensure!(!gc.graph.is_null(), "avfilter_graph_alloc failed");
            let buffer = ffi::avfilter_get_by_name(c"buffer".as_ptr());
            let scale = ffi::avfilter_get_by_name(c"scale_vaapi".as_ptr());
            let buffersink = ffi::avfilter_get_by_name(c"buffersink".as_ptr());
            anyhow::ensure!(
                !buffer.is_null() && !scale.is_null() && !buffersink.is_null(),
                "this FFmpeg has no scale_vaapi filter"
            );

            // The source says its frames are surfaces of the BGR0 pool.
            gc.src = ffi::avfilter_graph_alloc_filter(gc.graph, buffer, c"in".as_ptr());
            anyhow::ensure!(!gc.src.is_null(), "buffer filter not allocated");
            let params = ffi::av_buffersrc_parameters_alloc();
            anyhow::ensure!(!params.is_null(), "av_buffersrc_parameters_alloc failed");
            (*params).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI as c_int;
            (*params).width = width as c_int;
            (*params).height = height as c_int;
            (*params).time_base = ffi::AVRational {
                num: 1,
                den: fps as c_int,
            };
            (*params).frame_rate = ffi::AVRational {
                num: fps as c_int,
                den: 1,
            };
            (*params).hw_frames_ctx = gc.upload_frames_ref;
            let r = ffi::av_buffersrc_parameters_set(gc.src, params);
            ffi::av_free(params as *mut _);
            anyhow::ensure!(r >= 0, "buffer source: {}", describe_err(r));
            let r = ffi::avfilter_init_str(gc.src, std::ptr::null());
            anyhow::ensure!(r >= 0, "buffer source: {}", describe_err(r));

            let mut convert: *mut ffi::AVFilterContext = std::ptr::null_mut();
            let r = ffi::avfilter_graph_create_filter(
                &mut convert,
                scale,
                c"convert".as_ptr(),
                VPP_ARGS.as_ptr(),
                std::ptr::null_mut(),
                gc.graph,
            );
            anyhow::ensure!(r >= 0, "scale_vaapi: {}", describe_err(r));
            let r = ffi::avfilter_graph_create_filter(
                &mut gc.sink,
                buffersink,
                c"out".as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                gc.graph,
            );
            anyhow::ensure!(r >= 0, "buffer sink: {}", describe_err(r));
            let r = ffi::avfilter_link(gc.src, 0, convert, 0);
            anyhow::ensure!(r >= 0, "link: {}", describe_err(r));
            let r = ffi::avfilter_link(convert, 0, gc.sink, 0);
            anyhow::ensure!(r >= 0, "link: {}", describe_err(r));
            let r = ffi::avfilter_graph_config(gc.graph, std::ptr::null_mut());
            anyhow::ensure!(r >= 0, "graph config: {}", describe_err(r));

            let out = ffi::av_buffersink_get_hw_frames_ctx(gc.sink);
            anyhow::ensure!(!out.is_null(), "the processor made no output surfaces");
            gc.upload = ffi::av_frame_alloc();
            gc.view = ffi::av_frame_alloc();
            anyhow::ensure!(
                !gc.upload.is_null() && !gc.view.is_null(),
                "av_frame_alloc failed"
            );

            // The probe: one blank frame, upload, convert, take it back.
            let blank = vec![0u8; width as usize * 4 * height as usize];
            let mut probe = ffi::av_frame_alloc();
            anyhow::ensure!(!probe.is_null(), "av_frame_alloc failed");
            let r = gc.convert(&blank, width * 4, width, height, 0, probe, None);
            ffi::av_frame_free(&mut probe);
            r?;
            let out_ref = ffi::av_buffer_ref(out);
            anyhow::ensure!(!out_ref.is_null(), "av_buffer_ref failed");
            Ok((gc, out_ref))
        }
    }

    /// Upload `data` (BGRA, `stride` bytes a row) and convert it into `out`
    /// as an NV12 surface stamped `pts`. `ts` is the frame the trace names.
    ///
    /// # Safety
    /// `out` is a live, reusable `AVFrame`.
    #[allow(clippy::too_many_arguments)]
    unsafe fn convert(
        &mut self,
        data: &[u8],
        stride: u32,
        width: u32,
        height: u32,
        pts: i64,
        out: *mut ffmpeg::ffi::AVFrame,
        ts: Option<Instant>,
    ) -> anyhow::Result<()> {
        use ffmpeg::ffi;
        anyhow::ensure!(
            !self.stuck,
            "video processor out of step: a frame went in and none came out"
        );
        let needed = (height as usize).saturating_sub(1) * stride as usize + width as usize * 4;
        anyhow::ensure!(
            stride >= width * 4 && data.len() >= needed,
            "frame of {} bytes does not hold {width}x{height} at stride {stride}",
            data.len()
        );
        unsafe {
            let view = &mut *self.view;
            view.format = ffi::AVPixelFormat::AV_PIX_FMT_BGR0 as c_int;
            view.width = width as c_int;
            view.height = height as c_int;
            view.data[0] = data.as_ptr() as *mut u8;
            view.linesize[0] = stride as c_int;

            ffi::av_frame_unref(self.upload);
            let r = ffi::av_hwframe_get_buffer(self.upload_frames_ref, self.upload, 0);
            anyhow::ensure!(r >= 0, "BGR0 surface: {}", describe_err(r));
            let r = ffi::av_hwframe_transfer_data(self.upload, self.view, 0);
            anyhow::ensure!(r >= 0, "upload of the BGRA: {}", describe_err(r));
            if let Some(ts) = ts {
                latency_trace::encoder_step(ts, Step::Uploaded);
            }
            (*self.upload).pts = pts;
            // The graph takes the surface (no KEEP_REF), so `upload` is empty
            // again for the next frame.
            let r = ffi::av_buffersrc_add_frame_flags(self.src, self.upload, 0);
            anyhow::ensure!(r >= 0, "video processor in: {}", describe_err(r));
            ffi::av_frame_unref(out);
            let r = ffi::av_buffersink_get_frame(self.sink, out);
            self.stuck = r < 0;
            anyhow::ensure!(r >= 0, "video processor out: {}", describe_err(r));
            if let Some(ts) = ts {
                latency_trace::encoder_step(ts, Step::Converted);
            }
        }
        Ok(())
    }
}

impl Drop for GpuConvert {
    fn drop(&mut self) {
        use ffmpeg::ffi;
        unsafe {
            ffi::av_frame_free(&mut self.view);
            ffi::av_frame_free(&mut self.upload);
            ffi::avfilter_graph_free(&mut self.graph);
            ffi::av_buffer_unref(&mut self.upload_frames_ref);
        }
    }
}

/// Whether the card path was refused and said so: the reason is a warning
/// once, then a debug line for the encoders opened after (a resolution
/// change opens another).
static GPU_CONVERT_REFUSED: AtomicBool = AtomicBool::new(false);

/// The variable that forces the processor path.
pub const CPU_CONVERT_ENV: &str = "POCKETDESK_VAAPI_CPU_CONVERT";

fn cpu_convert_forced() -> bool {
    std::env::var(CPU_CONVERT_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn h264_profile_vaapi(profile: &H264Profile) -> &'static str {
    match profile {
        H264Profile::Baseline => "constrained_baseline",
        H264Profile::Main => "main",
        H264Profile::High => "high",
    }
}

/// The AVOptions the VA-API encoders open with. `async_depth` is FFmpeg's
/// number of frames in flight (default 2): the encoder hands a packet back
/// only when the next frame arrives, so every picture waits a whole frame
/// period. One keeps it lock-step, each packet out as its frame is encoded
/// (the stage trace's "sent > packet out" went from 16.7 ms to about 0 and
/// the input-to-picture median fell by 9 to 12 ms). `profile` is an AVOption
/// of h264_vaapi only; hevc_vaapi does not accept the name and would fail to
/// open.
fn codec_options(is_hevc: bool, h264_profile: &str) -> Vec<(&'static str, String)> {
    let mut options = vec![("async_depth", "1".to_string())];
    if !is_hevc {
        options.push(("profile", h264_profile.to_string()));
    }
    options
}

impl VaapiEncoder {
    /// Open the encoder with the colour conversion on the card, or on the
    /// processor where the card's video processor cannot take it.
    pub fn new(config: StreamingConfig) -> anyhow::Result<Self> {
        if !cpu_convert_forced() {
            match Self::open(config.clone(), true) {
                Ok(encoder) => return Ok(encoder),
                Err(e) => {
                    if GPU_CONVERT_REFUSED.swap(true, Ordering::Relaxed) {
                        debug!("VAAPI conversion on the card unavailable ({e:#})");
                    } else {
                        warn!(
                            "VAAPI conversion on the card unavailable ({e:#}); \
                             converting on the processor"
                        );
                    }
                }
            }
        }
        Self::open(config, false)
    }

    fn open(config: StreamingConfig, gpu_convert: bool) -> anyhow::Result<Self> {
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

        // --- the conversion, and the hardware frames context the codec takes
        // (NV12 surfaces on the VA device) ---
        let (convert, mut hw_frames_ref) = if gpu_convert {
            match GpuConvert::new(device_ref, config.width, config.height, config.fps) {
                Ok((gc, frames)) => (Convert::Gpu(gc), frames),
                Err(e) => {
                    unsafe {
                        ffi::avcodec_free_context(&mut ctx);
                        ffi::av_buffer_unref(&mut device_ref);
                    }
                    return Err(e.context("video processor"));
                }
            }
        } else {
            let mut frames_ref = unsafe { ffi::av_hwframe_ctx_alloc(device_ref) };
            if frames_ref.is_null() {
                unsafe {
                    ffi::avcodec_free_context(&mut ctx);
                    ffi::av_buffer_unref(&mut device_ref);
                }
                anyhow::bail!("av_hwframe_ctx_alloc failed");
            }
            let pool_size: c_int = if is_hevc { 32 } else { 20 };
            let init_ret = unsafe {
                let frames = (*frames_ref).data as *mut ffi::AVHWFramesContext;
                (*frames).format = ffi::AVPixelFormat::AV_PIX_FMT_VAAPI;
                (*frames).sw_format = ffi::AVPixelFormat::AV_PIX_FMT_NV12;
                (*frames).width = config.width as c_int;
                (*frames).height = config.height as c_int;
                (*frames).initial_pool_size = pool_size;
                ffi::av_hwframe_ctx_init(frames_ref)
            };
            if init_ret < 0 {
                unsafe {
                    ffi::av_buffer_unref(&mut frames_ref);
                    ffi::avcodec_free_context(&mut ctx);
                    ffi::av_buffer_unref(&mut device_ref);
                }
                anyhow::bail!("av_hwframe_ctx_init failed ({init_ret})");
            }
            let bgra =
                ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, config.width, config.height);
            let nv12 =
                ffmpeg::frame::Video::new(ffmpeg::format::Pixel::NV12, config.width, config.height);
            let scaler = match ffmpeg::software::scaling::Context::get(
                ffmpeg::format::Pixel::BGRA,
                config.width,
                config.height,
                ffmpeg::format::Pixel::NV12,
                config.width,
                config.height,
                ffmpeg::software::scaling::flag::Flags::BILINEAR,
            ) {
                Ok(scaler) => scaler,
                Err(e) => {
                    unsafe {
                        ffi::av_buffer_unref(&mut frames_ref);
                        ffi::avcodec_free_context(&mut ctx);
                        ffi::av_buffer_unref(&mut device_ref);
                    }
                    return Err(e.into());
                }
            };
            (Convert::Cpu(CpuConvert { bgra, nv12, scaler }), frames_ref)
        };
        unsafe {
            (*ctx).hw_frames_ctx = ffi::av_buffer_ref(hw_frames_ref);
        }

        // --- open codec ---
        let mut opts: *mut ffi::AVDictionary = std::ptr::null_mut();
        for (key, value) in codec_options(is_hevc, h264_profile_vaapi(&config.profile)) {
            let (key, value) = (CString::new(key)?, CString::new(value)?);
            unsafe {
                ffi::av_dict_set(&mut opts, key.as_ptr(), value.as_ptr(), 0);
            }
        }
        let open_ret = unsafe { ffi::avcodec_open2(ctx, codec, &mut opts) };
        unsafe {
            // An option the encoder did not take stays in the dictionary. An
            // FFmpeg without `async_depth` would hold a frame again, unseen.
            if !opts.is_null()
                && !ffi::av_dict_get(opts, c"async_depth".as_ptr(), std::ptr::null(), 0).is_null()
            {
                warn!("{codec_name} ignored async_depth: it may hold a frame until the next");
            }
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

        info!(
            "Video encoder: in-process VAAPI ({codec_name}) on {device_path}, colour conversion on {}",
            if gpu_convert {
                "the card"
            } else {
                "the processor"
            }
        );

        Ok(Self {
            config,
            device_path,
            codec_name,
            encoder: ctx,
            hw_device_ref: device_ref,
            hw_frames_ref,
            hw_frame,
            packet,
            convert,
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

        let hw = self.hw_frame;
        let sequence = self.sequence;
        match &mut self.convert {
            Convert::Gpu(gc) => unsafe {
                // The capture's bytes go straight to a surface; the card
                // converts them (it says its own steps).
                gc.convert(
                    &frame.data,
                    frame.stride,
                    self.config.width,
                    self.config.height,
                    sequence as i64,
                    hw,
                    Some(frame.timestamp),
                )?;
            },
            Convert::Cpu(cc) => {
                cc.copy_frame(frame, self.config.width, self.config.height);
                latency_trace::encoder_step(frame.timestamp, Step::Copied);
                cc.scaler.run(&cc.bgra, &mut cc.nv12)?;
                latency_trace::encoder_step(frame.timestamp, Step::Scaled);

                // Upload NV12 → VAAPI surface: reset the reusable hw_frame,
                // attach the frames context, grab a buffer, then hwupload via
                // transfer_data.
                let sw_ptr = unsafe { cc.nv12.as_mut_ptr() };
                unsafe {
                    ffi::av_frame_unref(hw);
                    // Allocate a fresh VAAPI surface from the pool via the
                    // purpose-built helper; it sets format/geometry/
                    // hw_frames_ctx itself.
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
                }
                latency_trace::encoder_step(frame.timestamp, Step::Uploaded);
            }
        }
        unsafe {
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
            latency_trace::encoder_step(frame.timestamp, Step::Sent);
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

    #[test]
    fn codec_options_keep_one_frame_in_flight() {
        let h264 = codec_options(false, "high");
        assert!(h264.contains(&("async_depth", "1".to_string())));
        assert!(h264.contains(&("profile", "high".to_string())));
        // hevc_vaapi has no `profile` option; naming it would fail the open.
        let hevc = codec_options(true, "high");
        assert_eq!(hevc, vec![("async_depth", "1".to_string())]);
    }

    /// With one frame in flight each frame's packet is back from its own
    /// `encode_frame`, none held until the next frame (FFmpeg's default
    /// `async_depth` of 2 held one), and a keyframe asked for comes out in
    /// the very next packet. Self-skips where no VA-API device opens; HEVC is
    /// best-effort as in the round trip above.
    #[test]
    fn vaapi_gives_each_frame_back_at_once_and_keeps_keyframe_requests() {
        if !vaapi_available() {
            eprintln!("skipping VAAPI held-frame test: no usable device on this host");
            return;
        }
        for codec in [VideoCodec::H264, VideoCodec::H265] {
            let config = StreamingConfig {
                width: 1280,
                height: 720,
                fps: 30,
                bitrate_bps: 2_000_000,
                codec,
                ..StreamingConfig::default()
            };
            let mut encoder = match VaapiEncoder::new(config) {
                Ok(e) => e,
                Err(e) if codec == VideoCodec::H265 => {
                    eprintln!("skipping HEVC: {e:#}");
                    continue;
                }
                Err(e) => panic!("H.264 VAAPI encoder must open: {e:#}"),
            };
            for i in 0..8 {
                let p = encoder
                    .encode_frame(&gradient_frame(1280, 720, i))
                    .unwrap()
                    .unwrap_or_else(|| panic!("{codec:?}: frame {i} held until the next"));
                assert_eq!(p.sequence, i as u64, "{codec:?}: the packet is its frame's");
            }
            encoder.request_keyframe();
            let p = encoder
                .encode_frame(&gradient_frame(1280, 720, 9))
                .unwrap()
                .expect("a packet for the frame");
            assert!(p.is_keyframe, "{codec:?}: the asked-for keyframe is next");
        }
    }

    /// A frame of one colour (B, G, R) with the alpha byte `alpha`.
    fn flat_frame(w: u32, h: u32, bgr: [u8; 3], alpha: u8) -> VideoFrame {
        let mut frame = gradient_frame(w, h, 0);
        for i in (0..frame.data.len()).step_by(4) {
            frame.data[i..i + 3].copy_from_slice(&bgr);
            frame.data[i + 3] = alpha;
        }
        frame
    }

    /// The mean of each plane (Y, U, V) of the last picture the software
    /// decoder makes from `packets`.
    fn decoded_means(packets: &[EncodedPacket]) -> [f64; 3] {
        let codec = ffmpeg::decoder::find(ffmpeg::codec::Id::H264).expect("an H.264 decoder");
        let mut decoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .decoder()
            .video()
            .expect("decoder opens");
        let mut picture = ffmpeg::frame::Video::empty();
        let mut means = None;
        let mut take = |decoder: &mut ffmpeg::decoder::Video| {
            while decoder.receive_frame(&mut picture).is_ok() {
                let mut m = [0.0; 3];
                for (plane, mean) in m.iter_mut().enumerate() {
                    let (w, h) = (
                        picture.plane_width(plane as _),
                        picture.plane_height(plane as _),
                    );
                    let stride = picture.stride(plane);
                    let data = picture.data(plane);
                    let sum: u64 = (0..h as usize)
                        .flat_map(|y| data[y * stride..y * stride + w as usize].iter())
                        .map(|&b| u64::from(b))
                        .sum();
                    *mean = sum as f64 / f64::from(w * h);
                }
                means = Some(m);
            }
        };
        for p in packets {
            decoder
                .send_packet(&ffmpeg::Packet::copy(&p.data))
                .expect("packet decodes");
            take(&mut decoder);
        }
        let _ = decoder.send_eof();
        take(&mut decoder);
        means.expect("a decoded picture")
    }

    /// The card and the processor make the same colours: a flat frame
    /// through each path, decoded, has the same Y, U and V (alpha is not
    /// read: a capture's alpha byte may be 0 or 255). Self-skips where no
    /// VA-API device opens or the card's processor cannot take BGR0.
    #[test]
    fn card_and_processor_make_the_same_colours() {
        if !vaapi_available() {
            eprintln!("skipping VAAPI colour test: no usable device on this host");
            return;
        }
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 4_000_000,
            codec: VideoCodec::H264,
            ..StreamingConfig::default()
        };
        let encode = |gpu: bool, bgr: [u8; 3], alpha: u8| -> anyhow::Result<[f64; 3]> {
            let mut encoder = VaapiEncoder::open(config.clone(), gpu)?;
            let mut packets = Vec::new();
            for _ in 0..12 {
                packets.extend(encoder.encode_frame(&flat_frame(1280, 720, bgr, alpha))?);
            }
            packets.extend(encoder.drain()?);
            Ok(decoded_means(&packets))
        };
        for (bgr, alpha) in [
            ([30, 110, 200], 255),
            ([30, 110, 200], 0),
            ([220, 40, 90], 255),
        ] {
            let cpu = encode(false, bgr, alpha).expect("the processor path opens");
            let Ok(card) = encode(true, bgr, alpha) else {
                eprintln!("skipping VAAPI colour test: the card path is not available here");
                return;
            };
            eprintln!("bgr={bgr:?} alpha={alpha}: processor {cpu:.1?}, card {card:.1?}");
            for plane in 0..3 {
                assert!(
                    (cpu[plane] - card[plane]).abs() <= 4.0,
                    "plane {plane} differs: processor {cpu:?}, card {card:?}"
                );
            }
        }
    }

    /// The card path hands every frame back at once and over a long run
    /// keeps its surfaces (the pools are not drained), and the processor
    /// path does the same: the paths are interchangeable to the caller.
    #[test]
    fn both_paths_run_many_frames() {
        if !vaapi_available() {
            eprintln!("skipping VAAPI long-run test: no usable device on this host");
            return;
        }
        for gpu in [true, false] {
            let config = StreamingConfig {
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_bps: 8_000_000,
                codec: VideoCodec::H264,
                ..StreamingConfig::default()
            };
            let Ok(mut encoder) = VaapiEncoder::open(config, gpu) else {
                assert!(gpu, "the processor path must open");
                eprintln!("skipping: the card path is not available here");
                continue;
            };
            assert_eq!(matches!(encoder.convert, Convert::Gpu(_)), gpu);
            for i in 0..300 {
                let p = encoder
                    .encode_frame(&gradient_frame(1920, 1080, i))
                    .unwrap()
                    .unwrap_or_else(|| panic!("gpu={gpu}: frame {i} held"));
                assert_eq!(p.sequence, i as u64);
            }
        }
    }

    /// A frame too small for the encoder's size is refused by the card path
    /// (the streamer opens another encoder when the size changes).
    #[test]
    fn card_path_refuses_a_short_frame() {
        if !vaapi_available() {
            return;
        }
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 2_000_000,
            codec: VideoCodec::H264,
            ..StreamingConfig::default()
        };
        let Ok(mut encoder) = VaapiEncoder::open(config, true) else {
            return;
        };
        let small = gradient_frame(640, 360, 0);
        assert!(encoder.encode_frame(&small).is_err());
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
            gpu: None,
            repeat: false,
        }
    }
}
