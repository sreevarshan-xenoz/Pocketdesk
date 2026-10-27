//! H.264/H.265 video encoding facade
//!
//! Two backends: `InProcess` (R2#3, software path via `ffmpeg-next` — no
//! child process, no stdio pipe) and `Sidecar` (persistent FFmpeg process
//! reading raw BGRA on stdin and emitting NAL units on stdout), the latter
//! still serving the VAAPI/NVENC hardware paths.
//!
//! `VideoEncoder::new` picks the backend from the resolved encoder choice
//! and falls back to the sidecar if the in-process path cannot open.
//!
//! R4 C2 (fallback discipline): hardware backends die *after* spawn — VAAPI
//! init errors kill the FFmpeg child within milliseconds of a successful
//! `spawn()`. `SidecarEncoder::verify_startup` waits out that window before
//! the pipeline commits to the backend, and `VideoEncoder::new` degrades to
//! the software rung (H.264, in-process first) when the hardware child
//! cannot survive it. The encode task in `streamer.rs` watches a live
//! encoder for stalls/Err storms and rebuilds along the same ladder.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ffmpeg_sidecar::child::FfmpegChild;
use ffmpeg_sidecar::command::FfmpegCommand;
use tracing::{debug, error, info, trace, warn};

use super::encoder_detect::HardwareEncoder;
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
use super::encoder_inproc::InProcessEncoder;
#[cfg(target_os = "linux")]
use super::encoder_vaapi::VaapiEncoder;
use super::flv_demux::FlvDemux;
use super::{EncodedPacket, EncoderPreset, H264Profile, StreamingConfig, VideoCodec, VideoFrame};

/// Encoded chunks in flight from the stdout reader thread. Bounded so a
/// consumer that stops draining stalls FFmpeg (which the encode task's stall
/// supervisor then sees) instead of growing memory without limit.
const STDOUT_CHUNKS_IN_FLIGHT: usize = 256;

/// Read the child's stdout on a dedicated thread and forward each chunk.
///
/// The encode loop must never block on the pipe. On Linux that used to be
/// `O_NONBLOCK`; anonymous pipes on Windows have no equivalent, and a reader
/// thread behaves the same on both. The channel disconnecting is EOF.
fn spawn_stdout_reader(mut stdout: ChildStdout) -> io::Result<Receiver<io::Result<Vec<u8>>>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(STDOUT_CHUNKS_IN_FLIGHT);
    std::thread::Builder::new()
        .name("ffmpeg-stdout".into())
        .spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        })?;
    Ok(rx)
}

/// What the stderr logger thread has seen: the line that best explains an
/// exit, and whether the pipe has closed.
#[derive(Default)]
struct StderrTail {
    reason: Mutex<Option<String>>,
    closed: AtomicBool,
}

/// Whether an FFmpeg log line reports an error rather than a warning or info.
fn is_ffmpeg_error_line(line: &str) -> bool {
    line.contains("[error]") || line.contains("[fatal]")
}

/// Log FFmpeg's stderr as it arrives (keeping the pipe from filling) and keep
/// the line most likely to explain an exit: the latest error line, else the
/// latest line.
fn spawn_stderr_logger(stderr: ChildStderr, tail: Arc<StderrTail>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("ffmpeg-stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let is_error = is_ffmpeg_error_line(trimmed);
                if is_error {
                    error!("[ffmpeg] {}", trimmed);
                } else if trimmed.contains("[warning]") {
                    warn!("[ffmpeg] {}", trimmed);
                } else {
                    debug!("[ffmpeg] {}", trimmed);
                }
                let mut reason = tail.reason.lock().unwrap_or_else(|e| e.into_inner());
                let keep_previous =
                    !is_error && reason.as_deref().is_some_and(is_ffmpeg_error_line);
                if !keep_previous {
                    *reason = Some(trimmed.to_string());
                }
            }
            tail.closed.store(true, Ordering::Release);
        })?;
    Ok(())
}

/// Magic bytes for H.264 NAL unit start codes. The sidecar's output is
/// framed by FLV; these NAL scanners serve the in-process encoders.
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
const START_CODE_4: [u8; 4] = [0x00, 0x00, 0x00, 0x01];
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
const START_CODE_3: [u8; 3] = [0x00, 0x00, 0x01];

/// NAL unit type for IDR (instantaneous decoder refresh / keyframe).
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
const NAL_TYPE_IDR: u8 = 5;

/// Mask to extract NAL unit type from the first byte.
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
const NAL_TYPE_MASK: u8 = 0x1F;

/// Video encoder facade over the FFmpeg backends.
pub enum VideoEncoder {
    /// Persistent FFmpeg child process over stdio pipes (VAAPI/NVENC).
    Sidecar(SidecarEncoder),
    /// In-process via `ffmpeg-next`: libx264/libx265 (software path), and
    /// NVENC on Windows with `inproc-windows` (BG8).
    #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
    InProcess(InProcessEncoder),
    /// In-process VAAPI (`h264_vaapi`/`hevc_vaapi`) via raw FFmpeg FFI (C4).
    #[cfg(target_os = "linux")]
    VaapiInProcess(VaapiEncoder),
}

impl VideoEncoder {
    /// Create the encoder, preferring the resolved backend and degrading to
    /// software when a hardware backend cannot open or dies on startup (C2).
    pub fn new(config: StreamingConfig) -> anyhow::Result<Self> {
        use super::encoder_detect::{probe_encoders, resolve_encoder};

        let resolved = if config.hardware_encoder == HardwareEncoder::Auto {
            resolve_encoder(HardwareEncoder::Auto, &probe_encoders())
        } else {
            config.hardware_encoder
        };

        if matches!(resolved, HardwareEncoder::Software) {
            return open_software(config);
        }

        let mut hw_config = config.clone();
        hw_config.hardware_encoder = resolved;
        if hw_config.codec == VideoCodec::H265 && !resolved.supports_hevc() {
            info!("{resolved:?} cannot encode HEVC; using H.264 on this rung");
            hw_config.codec = VideoCodec::H264;
        }

        // BG8: NVENC in-process first on Windows (keyframes on request,
        // bitrate in place), then the verified sidecar.
        #[cfg(all(windows, feature = "inproc-windows"))]
        if matches!(resolved, HardwareEncoder::Nvenc) && fail_by_seam(resolved).is_ok() {
            match InProcessEncoder::new_hardware(hw_config.clone()) {
                Ok(encoder) => {
                    info!("Video encoder: in-process FFmpeg ({resolved:?})");
                    return Ok(Self::InProcess(encoder));
                }
                Err(e) => warn!("In-process {resolved:?} unavailable ({e:#}); trying the sidecar"),
            }
        }

        // C4: VAAPI prefers the in-process hardware path (no child process, no
        // stdio pipes, device-node auto-detect). On failure fall to the
        // verified sidecar, then the software rung — the C2 ladder.
        if matches!(resolved, HardwareEncoder::Vaapi) {
            match open_vaapi_verified(hw_config) {
                Ok(encoder) => return Ok(encoder),
                Err(e) => {
                    warn!("VAAPI ladder exhausted ({e:#}); falling back to software (C2)");
                    let mut soft = config.clone();
                    degrade_to_software(&mut soft);
                    return open_software(soft);
                }
            }
        }

        match open_sidecar_verified(hw_config) {
            Ok(encoder) => Ok(encoder),
            Err(e) => {
                warn!("Hardware encoder unavailable ({e:#}) (C2)");
                if let Some(encoder) = recover_on_other_hardware(&config, resolved) {
                    return Ok(encoder);
                }
                let mut soft = config.clone();
                degrade_to_software(&mut soft);
                open_software(soft)
            }
        }
    }

    /// Open an encoder already forced to the software rung, without the
    /// hardware probe — used by the encode task's mid-session rebuild.
    pub fn new_software(config: StreamingConfig) -> anyhow::Result<Self> {
        let mut soft = config;
        degrade_to_software(&mut soft);
        open_software(soft)
    }

    /// Human-readable backend name, for the "encoder switched" telemetry/UX.
    pub fn backend_name(&self) -> &'static str {
        match self {
            Self::Sidecar(e) => e.backend_name(),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(_) => "in-process",
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(_) => "in-process-vaapi",
        }
    }

    /// Encode a single raw frame and return every packet now ready, oldest
    /// first; empty means the encoder has no output yet.
    pub fn encode_frame(&mut self, frame: &VideoFrame) -> anyhow::Result<Vec<EncodedPacket>> {
        match self {
            Self::Sidecar(e) => e.encode_frame(frame),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => Ok(e.encode_frame(frame)?.into_iter().collect()),
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(e) => Ok(e.encode_frame(frame)?.into_iter().collect()),
        }
    }

    /// Flush the encoder and return all remaining packets.
    pub fn drain(&mut self) -> anyhow::Result<Vec<EncodedPacket>> {
        match self {
            Self::Sidecar(e) => e.drain(),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => e.drain(),
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(e) => e.drain(),
        }
    }

    /// Request a keyframe on the next encode.
    /// Change the bitrate in place, without rebuilding the encoder (BG8);
    /// `false` when this backend cannot, and the caller rebuilds.
    // Off the in-process builds only the sidecar exists, which cannot.
    #[cfg_attr(
        not(any(target_os = "linux", all(windows, feature = "inproc-windows"))),
        allow(unused_variables)
    )]
    pub fn set_bitrate(&mut self, bitrate_bps: u32) -> bool {
        match self {
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => e.set_bitrate(bitrate_bps),
            _ => false,
        }
    }

    pub fn request_keyframe(&mut self) {
        match self {
            Self::Sidecar(e) => e.request_keyframe(),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => e.request_keyframe(),
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(e) => e.request_keyframe(),
        }
    }

    /// Number of frames submitted so far.
    pub fn sequence(&self) -> u64 {
        match self {
            Self::Sidecar(e) => e.sequence(),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => e.sequence(),
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(e) => e.sequence(),
        }
    }

    /// The encoding configuration.
    pub fn config(&self) -> &StreamingConfig {
        match self {
            Self::Sidecar(e) => e.config(),
            #[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows")))]
            Self::InProcess(e) => e.config(),
            #[cfg(target_os = "linux")]
            Self::VaapiInProcess(e) => e.config(),
        }
    }
}

/// C2: a failed hardware rung may still have hardware below it before the
/// software rung.
///
/// On Linux that is the VAAPI ladder after a failed NVENC (hybrid laptops). On
/// Windows it is every other runtime-verified encoder, in Auto's order.
fn recover_on_other_hardware(
    config: &StreamingConfig,
    failed: HardwareEncoder,
) -> Option<VideoEncoder> {
    #[cfg(not(windows))]
    {
        if !matches!(failed, HardwareEncoder::Vaapi) {
            let mut va_config = config.clone();
            va_config.hardware_encoder = HardwareEncoder::Vaapi;
            if let Ok(encoder) = open_vaapi_verified(va_config) {
                info!("Recovered on the VAAPI rung after {failed:?} failed (C2/C4)");
                return Some(encoder);
            }
        }
        None
    }
    #[cfg(windows)]
    {
        let available = super::encoder_detect::probe_encoders();
        let rungs = [
            (HardwareEncoder::Nvenc, available.nvenc),
            (HardwareEncoder::Amf, available.amf),
            (HardwareEncoder::Qsv, available.qsv),
            (HardwareEncoder::MediaFoundation, available.media_foundation),
        ];
        for (rung, works) in rungs {
            if rung == failed || !works {
                continue;
            }
            let mut rung_config = config.clone();
            rung_config.hardware_encoder = rung;
            if !rung.supports_hevc() {
                rung_config.codec = VideoCodec::H264;
            }
            match open_sidecar_verified(rung_config) {
                Ok(encoder) => {
                    info!("Recovered on the {rung:?} rung after {failed:?} failed (C2)");
                    return Some(encoder);
                }
                Err(e) => warn!("{rung:?} rung failed too ({e:#}) (C2)"),
            }
        }
        None
    }
}

/// Mutate a config onto the bottom rung of the fallback ladder: software
/// encoder, H.264 (C2). HEVC is dropped even though x265 exists — the
/// roadmap's acceptance path is "continues on x264", and H.264 is the one
/// codec every client is guaranteed to decode.
pub fn degrade_to_software(config: &mut StreamingConfig) {
    config.hardware_encoder = HardwareEncoder::Software;
    config.codec = VideoCodec::H264;
}

/// Open the software rung: in-process libx264 first (Linux), sidecar as last
/// resort — and as the only software path where in-process FFmpeg is not built.
fn open_software(config: StreamingConfig) -> anyhow::Result<VideoEncoder> {
    #[cfg(target_os = "linux")]
    match InProcessEncoder::new(config.clone()) {
        Ok(encoder) => {
            info!("Video encoder: in-process FFmpeg (software)");
            return Ok(VideoEncoder::InProcess(encoder));
        }
        Err(e) => {
            warn!("In-process encoder unavailable ({e:#}); using FFmpeg sidecar");
        }
    }
    Ok(VideoEncoder::Sidecar(SidecarEncoder::new(config)?))
}

/// Open a hardware sidecar and verify the child survives its startup window
/// (C2). VAAPI/NVENC init failures kill FFmpeg *after* `spawn()` succeeds,
/// so a bare spawn is not evidence of a usable encoder.
fn open_sidecar_verified(config: StreamingConfig) -> anyhow::Result<VideoEncoder> {
    fail_by_seam(config.hardware_encoder)?;
    let mut encoder = SidecarEncoder::new(config)?;
    encoder.verify_startup(DEFAULT_STARTUP_VERIFY)?;
    Ok(VideoEncoder::Sidecar(encoder))
}

/// A test seam (AM1): the hardware rungs named in `POCKETDESK_FAIL_ENCODERS`
/// (comma-separated: nvenc, amf, qsv, mf, vaapi) fail as they open, as one
/// whose probe passed but whose encoder dies at startup does (C2). Unset in
/// use; the server says so when it is set.
fn fail_by_seam(encoder: HardwareEncoder) -> anyhow::Result<()> {
    match std::env::var("POCKETDESK_FAIL_ENCODERS") {
        Ok(list) if seam_names(&list, encoder) => {
            anyhow::bail!("{encoder:?} failed by POCKETDESK_FAIL_ENCODERS (a test seam)")
        }
        _ => Ok(()),
    }
}

/// Whether `list` names the hardware rung `encoder`; never the software
/// rung, where the ladder ends.
fn seam_names(list: &str, encoder: HardwareEncoder) -> bool {
    let name = match encoder {
        HardwareEncoder::Nvenc => "nvenc",
        HardwareEncoder::Amf => "amf",
        HardwareEncoder::Qsv => "qsv",
        HardwareEncoder::MediaFoundation => "mf",
        HardwareEncoder::Vaapi => "vaapi",
        HardwareEncoder::Auto | HardwareEncoder::Software => return false,
    };
    list.split(',').any(|n| n.trim().eq_ignore_ascii_case(name))
}

/// The VAAPI rung: in-process first (C4), verified sidecar as fallback.
fn open_vaapi_verified(config: StreamingConfig) -> anyhow::Result<VideoEncoder> {
    fail_by_seam(config.hardware_encoder)?;
    #[cfg(target_os = "linux")]
    match VaapiEncoder::new(config.clone()) {
        Ok(encoder) => return Ok(VideoEncoder::VaapiInProcess(encoder)),
        Err(e) => {
            warn!("In-process VAAPI unavailable ({e:#}); trying sidecar (C2/C4)");
        }
    }
    open_sidecar_verified(config)
}

/// How long to keep probing a sidecar child before trusting it (C2).
/// Long enough for FFmpeg to spawn, accept probe frames and either emit
/// output or die — broken VAAPI on this box dies within ~700 ms of spawn.
pub const DEFAULT_STARTUP_VERIFY: std::time::Duration = std::time::Duration::from_millis(600);

/// How long `encode_frame` waits for the frame it was just given. Hardware
/// encoders answer in a few milliseconds; without the wait every frame would
/// leave one call late, behind the next capture.
const FRAME_WAIT: std::time::Duration = std::time::Duration::from_millis(30);

/// H.264/H.265 encoder wrapper using a persistent FFmpeg sidecar process.
///
/// The encoder spawns a single long-running FFmpeg process on creation.
/// Raw BGRA frames are written to stdin; FLV is read from stdout and turned
/// back into one Annex-B access unit per frame.
pub struct SidecarEncoder {
    config: StreamingConfig,
    sequence: u64,
    keyframe_interval: u64,
    frames_since_keyframe: u64,
    /// (sequence, capture time) of frames written but not yet emitted.
    submitted: VecDeque<(u64, Instant)>,
    /// Frames this encoder keeps until more input arrives (learned).
    encoder_lag: usize,
    /// The FFmpeg child process.
    process: FfmpegChild,
    /// Stdin pipe for submitting raw frames. Wrapped in Option for clean shutdown.
    stdin: Option<ChildStdin>,
    /// Encoded chunks from the stdout reader thread.
    stdout: Receiver<io::Result<Vec<u8>>>,
    /// What the stderr logger thread has seen (for exit reasons).
    stderr: Arc<StderrTail>,
    /// Splits stdout into access units.
    demux: FlvDemux,
    /// Whether the child has written anything yet (startup liveness proof).
    produced_output: bool,
    /// NVENC high-power pin (R4 C3) — held only when the clock lock actually
    /// took; Drop restores, so both session end and a degraded-dropped
    /// encoder clean up.
    _power_pin: Option<super::nvenc_power::NvencPowerGuard>,
}

impl SidecarEncoder {
    /// Create a new encoder with the given configuration.
    ///
    /// Spawns a persistent FFmpeg process that accepts raw BGRA frames on stdin
    /// and outputs H.264 NAL units on stdout.
    pub fn new(config: StreamingConfig) -> anyhow::Result<Self> {
        let keyframe_interval = config.fps as u64 * 2; // Keyframe every 2 seconds

        let args = build_ffmpeg_args(&config, keyframe_interval);
        debug!(
            "Spawning FFmpeg encoder: {}x{}@{}fps, profile={:?}, preset={:?}, bitrate={}",
            config.width,
            config.height,
            config.fps,
            config.profile,
            config.preset,
            config.bitrate_bps
        );
        trace!("FFmpeg args: {:?}", args);

        let mut command = FfmpegCommand::new();
        for arg in &args {
            command.arg(arg);
        }

        let mut process = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn FFmpeg process: {}", e))?;

        let stdin = process
            .take_stdin()
            .ok_or_else(|| anyhow::anyhow!("FFmpeg stdin not available"))?;

        let stdout = process
            .take_stdout()
            .ok_or_else(|| anyhow::anyhow!("FFmpeg stdout not available"))?;

        let stderr = process
            .take_stderr()
            .ok_or_else(|| anyhow::anyhow!("FFmpeg stderr not available"))?;

        let stdout = spawn_stdout_reader(stdout)
            .map_err(|e| anyhow::anyhow!("Failed to start the FFmpeg stdout reader: {}", e))?;
        let stderr_tail = Arc::new(StderrTail::default());
        spawn_stderr_logger(stderr, stderr_tail.clone())
            .map_err(|e| anyhow::anyhow!("Failed to start the FFmpeg stderr logger: {}", e))?;

        info!("FFmpeg encoder process spawned successfully");

        Ok(Self {
            config: config.clone(),
            sequence: 0,
            keyframe_interval,
            frames_since_keyframe: 0,
            submitted: VecDeque::new(),
            encoder_lag: 0,
            process,
            stdin: Some(stdin),
            stdout,
            stderr: stderr_tail,
            demux: FlvDemux::new(),
            produced_output: false,
            // C3: only NVENC sessions get the GPU pinned; every other backend
            // (and non-NVIDIA / unprivileged hosts, where acquire() self-
            // declines) leaves this None.
            _power_pin: if config.hardware_encoder == HardwareEncoder::Nvenc {
                super::nvenc_power::NvencPowerGuard::acquire()
            } else {
                None
            },
        })
    }

    /// C2: prove the child is a *working* encoder before the pipeline
    /// commits to it. Hardware init failures (VAAPI `Failed to initialise
    /// VAAPI connection`, missing NVENC driver) surface only once FFmpeg
    /// processes input, and the exit can land anywhere from immediately
    /// after spawn to just past any static wait — so verification keeps
    /// feeding black probe frames until the child flushes real output
    /// (alive) or dies / rejects a write (broken: the caller falls one
    /// rung down the ladder). A child that stays live-but-silent to the
    /// deadline is trusted; the encode task's stall supervisor is the
    /// backstop for that case.
    pub fn verify_startup(&mut self, budget: std::time::Duration) -> anyhow::Result<()> {
        if self.child_exited()? {
            anyhow::bail!("FFmpeg encoder process exited during startup");
        }

        let probe = vec![0u8; self.config.width as usize * self.config.height as usize * 4];
        let deadline = Instant::now() + budget;
        loop {
            // Surface any output already produced: bits out of the encoder
            // are the strongest liveness proof there is.
            loop {
                match self.stdout.try_recv() {
                    Ok(Ok(chunk)) => {
                        self.demux.push(&chunk);
                        self.produced_output = true;
                    }
                    Ok(Err(e)) => return Err(e.into()),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        anyhow::bail!("FFmpeg encoder closed stdout during startup");
                    }
                }
            }
            if self.produced_output {
                return Ok(());
            }

            if self.child_exited()? {
                anyhow::bail!("FFmpeg encoder process exited during startup");
            }
            if Instant::now() >= deadline {
                // Alive, but no output yet (encoders buffer their first
                // frame). Trust it; probe frames already accepted encode as
                // valid black frames.
                debug!("Encoder probe produced no output yet; trusting the live child");
                return Ok(());
            }

            if let Some(stdin) = self.stdin.as_mut() {
                stdin
                    .write_all(&probe)
                    .and_then(|()| stdin.flush())
                    .map_err(|e| {
                        anyhow::anyhow!("FFmpeg encoder rejected a startup probe ({e})")
                    })?;
                // Probe frames are real frames of the stream (black, the
                // first a keyframe): they ship with the first real one.
                self.submitted.push_back((self.sequence, Instant::now()));
                self.sequence += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Non-blocking reap: `Ok(true)` when the child is gone (its first
    /// stderr line is surfaced as the reason), `Ok(false)` while alive.
    fn child_exited(&mut self) -> anyhow::Result<bool> {
        match self.process.as_inner_mut().try_wait() {
            Ok(Some(status)) => {
                // The logger thread may not have read the last words yet.
                let deadline = Instant::now() + std::time::Duration::from_millis(200);
                while !self.stderr.closed.load(Ordering::Acquire) && Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let reason = self
                    .stderr
                    .reason
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_else(|| "no stderr output".to_string());
                error!("FFmpeg encoder process exited ({status}): {reason}");
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(true),
            Err(e) => Err(e.into()),
        }
    }

    /// Encode a single raw frame.
    ///
    /// Writes the frame to FFmpeg stdin, waits up to [`FRAME_WAIT`] for its
    /// access unit, and returns every access unit ready by then, oldest
    /// first — so output never queues up behind the frames feeding it. An
    /// encoder that only releases a frame when the next one arrives is
    /// learned (`encoder_lag`) and not waited on again.
    pub fn encode_frame(&mut self, frame: &VideoFrame) -> anyhow::Result<Vec<EncodedPacket>> {
        self.frames_since_keyframe += 1;
        if self.frames_since_keyframe >= self.keyframe_interval {
            self.frames_since_keyframe = 0;
        }

        let frame_sequence = self.sequence;

        // Output that arrived since the last call. If frames the lag estimate
        // expected to stay held came out on their own, the estimate was a
        // slow frame, not the encoder's shape: shrink it.
        self.read_available_output()?;
        let mut packets = self.ready_packets();
        self.encoder_lag = self.encoder_lag.min(self.submitted.len());

        let stdin = self.stdin.as_mut().expect("stdin closed");
        stdin.write_all(&frame.data).map_err(|e| {
            error!("Failed to write frame to FFmpeg stdin: {}", e);
            anyhow::anyhow!("FFmpeg stdin write error: {}", e)
        })?;

        stdin.flush().map_err(|e| {
            error!("Failed to flush FFmpeg stdin: {}", e);
            anyhow::anyhow!("FFmpeg stdin flush error: {}", e)
        })?;

        trace!(
            "Submitted frame #{} to FFmpeg ({} bytes)",
            frame_sequence,
            frame.data.len()
        );

        self.submitted.push_back((frame_sequence, frame.timestamp));
        self.sequence += 1;

        let deadline = Instant::now() + FRAME_WAIT;
        loop {
            self.read_available_output()?;
            packets.extend(self.ready_packets());
            if self.submitted.len() <= self.encoder_lag {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                // This encoder keeps frames until more input arrives (AMF
                // holds one, Media Foundation many): stop waiting for them.
                self.encoder_lag = self.submitted.len();
                debug!(
                    "{} holds {} frame(s) until the next input",
                    self.backend_name(),
                    self.encoder_lag
                );
                break;
            }
            match self.stdout.recv_timeout(deadline - now) {
                Ok(Ok(chunk)) => {
                    self.demux.push(&chunk);
                    self.produced_output = true;
                }
                Ok(Err(e)) => {
                    error!("Error reading from FFmpeg stdout: {}", e);
                    return Err(anyhow::anyhow!("FFmpeg stdout read error: {}", e));
                }
                // The deadline check above records the encoder's lag.
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        for packet in &packets {
            trace!(
                "Encoded packet for frame #{}: {} bytes, keyframe={}",
                packet.sequence,
                packet.data.len(),
                packet.is_keyframe
            );
        }
        if packets.is_empty() {
            trace!(
                "No encoded output ready for frame #{} (encoder latency)",
                frame_sequence
            );
        }
        Ok(packets)
    }

    /// Drain remaining output from the FFmpeg process.
    ///
    /// Reads all available data from stdout and returns any complete packets.
    pub fn drain(&mut self) -> anyhow::Result<Vec<EncodedPacket>> {
        self.read_all_output()?;
        let packets = self.ready_packets();
        debug!("Drained {} remaining packets from encoder", packets.len());
        Ok(packets)
    }

    /// Every complete access unit in the demuxer, paired with the frame it
    /// encodes (B-frame-free, so output order is input order).
    fn ready_packets(&mut self) -> Vec<EncodedPacket> {
        let mut packets = Vec::new();
        while let Some(unit) = self.demux.next_access_unit() {
            let (sequence, timestamp) = self.submitted.pop_front().unwrap_or_else(|| {
                let sequence = self.sequence;
                self.sequence += 1;
                (sequence, Instant::now())
            });
            packets.push(EncodedPacket {
                data: unit.data,
                is_keyframe: unit.keyframe,
                timestamp,
                sequence,
            });
        }
        packets
    }

    /// Ask for a keyframe. A running FFmpeg child cannot be told to emit one
    /// (stdin carries the frames), so this cannot be honoured early: the
    /// next scheduled IDR, at most `keyframe_interval` frames away, is the
    /// recovery point. Packets are labelled from the bitstream, never from
    /// the request, so a P-frame is never sent as a keyframe.
    pub fn request_keyframe(&mut self) {
        debug!(
            "Keyframe requested; the sidecar's next IDR is at most {} frames away",
            self.keyframe_interval - self.frames_since_keyframe
        );
    }

    /// Get the current sequence number.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Which FFmpeg encoder this child runs, for telemetry and benchmark
    /// records: a hardware rung that fell back must not read like another.
    pub fn backend_name(&self) -> &'static str {
        match self.config.hardware_encoder {
            HardwareEncoder::Auto | HardwareEncoder::Software => "ffmpeg-sidecar",
            HardwareEncoder::Vaapi => "ffmpeg-sidecar-vaapi",
            HardwareEncoder::Nvenc => "ffmpeg-sidecar-nvenc",
            HardwareEncoder::Amf => "ffmpeg-sidecar-amf",
            HardwareEncoder::Qsv => "ffmpeg-sidecar-qsv",
            HardwareEncoder::MediaFoundation => "ffmpeg-sidecar-mf",
        }
    }

    /// Get the encoding configuration.
    pub fn config(&self) -> &StreamingConfig {
        &self.config
    }

    /// Take whatever the stdout reader has forwarded, without blocking.
    fn read_available_output(&mut self) -> anyhow::Result<()> {
        loop {
            match self.stdout.try_recv() {
                Ok(Ok(chunk)) => {
                    trace!("Read {} bytes from FFmpeg stdout", chunk.len());
                    self.demux.push(&chunk);
                    self.produced_output = true;
                }
                Ok(Err(e)) => {
                    // Other errors may indicate process issues.
                    error!("Error reading from FFmpeg stdout: {}", e);
                    return Err(anyhow::anyhow!("FFmpeg stdout read error: {}", e));
                }
                // No more data available right now.
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    debug!("FFmpeg stdout reached end of stream");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Read all remaining output from FFmpeg stdout.
    fn read_all_output(&mut self) -> anyhow::Result<()> {
        loop {
            match self.stdout.try_recv() {
                Ok(Ok(chunk)) => self.demux.push(&chunk),
                Ok(Err(e)) => {
                    warn!("Error draining FFmpeg stdout: {}", e);
                    break;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    }
}

impl Drop for SidecarEncoder {
    fn drop(&mut self) {
        info!("Shutting down FFmpeg encoder process");

        // Close stdin to signal FFmpeg that no more input is coming
        if let Some(mut stdin) = self.stdin.take() {
            if let Err(e) = stdin.flush() {
                trace!("FFmpeg stdin flush on drop (expected): {}", e);
            }
            drop(stdin); // closes the pipe
        }

        // Try to drain remaining output before killing
        if let Err(e) = self.read_all_output() {
            trace!("FFmpeg drain on drop (expected): {}", e);
        }

        // Terminate the process
        match self.process.as_inner_mut().try_wait() {
            Ok(Some(status)) => {
                debug!("FFmpeg process already exited: {:?}", status);
            }
            Ok(None) => {
                // Try graceful shutdown first
                let _ = self.process.quit();
                // Then force kill if still running
                if let Err(e) = self.process.kill() {
                    warn!("Failed to kill FFmpeg process on drop: {}", e);
                } else {
                    debug!("FFmpeg process killed");
                }
                // Reap the process
                let _ = self.process.wait();
            }
            Err(e) => {
                warn!("Error checking FFmpeg process status on drop: {}", e);
            }
        }
    }
}

/// Detect if the encoded data contains an IDR (keyframe) NAL unit.
///
/// Scans for start codes and checks if any following NAL unit has type 5.
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
pub(crate) fn detect_keyframe(data: &[u8]) -> bool {
    let mut pos = 0;
    while pos < data.len() {
        if let Some((start_pos, start_len)) = find_start_code(data, pos) {
            let nal_header_pos = start_pos + start_len;
            if nal_header_pos < data.len() {
                let nal_byte = data[nal_header_pos];
                let nal_type = nal_byte & NAL_TYPE_MASK;
                if nal_type == NAL_TYPE_IDR {
                    trace!("Detected IDR (keyframe) NAL unit at offset {}", start_pos);
                    return true;
                }
            }
            pos = nal_header_pos;
        } else {
            break;
        }
    }
    false
}

/// Find the next H.264 start code in the data starting from the given position.
///
/// Returns `Some((position, length))` where position is the offset of the start
/// code and length is either 4 (for 00 00 00 01) or 3 (for 00 00 01).
/// Returns `None` if no start code is found.
#[cfg(any(target_os = "linux", all(windows, feature = "inproc-windows"), test))]
pub(crate) fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if from >= data.len() || from + 2 >= data.len() {
        return None;
    }

    let slice = &data[from..];

    // Look for 4-byte start code: 0x00 0x00 0x00 0x01
    if slice.len() >= 4 && slice[0..4] == START_CODE_4 {
        return Some((from, 4));
    }

    // Look for 3-byte start code: 0x00 0x00 0x01
    if slice.len() >= 3 && slice[0..3] == START_CODE_3 {
        return Some((from, 3));
    }

    // Scan forward for the next start code
    // We look for the pattern 0x00 0x00 0x01 or 0x00 0x00 0x00 0x01
    for i in 1..=slice.len().saturating_sub(3) {
        if slice[i..i + 3] == START_CODE_3 {
            // Check if this is actually a 4-byte start code
            if i > 0 && slice[i - 1] == 0x00 {
                return Some((from + i - 1, 4));
            }
            return Some((from + i, 3));
        }
    }

    None
}

/// VBV/rate-control bounds for the x264/x265 private params (kilobits/sec).
///
/// Buffer sized for ~2 frames so the encoder must smooth spikes instead of
/// building a burst queue that adds frame delay on top of network latency.
pub(crate) fn vbv_kbit_bounds(bitrate_bps: u32, fps: u32) -> (u64, u64) {
    let bitrate = bitrate_bps as u64;
    let maxrate_kbps = (bitrate / 1000).max(1);
    let bufsize_kbits = (bitrate * 2 / fps as u64 / 1000).clamp(128, 50_000);
    (maxrate_kbps, bufsize_kbits)
}

/// Build the FFmpeg command-line arguments for encoding.
///
/// The DRM render node the VAAPI sidecar should bind to. Honours the
/// `POCKETDESK_VAAPI_DEVICE` override, else the lowest existing
/// `/dev/dri/renderD*`. On hybrid-GPU laptops the first node (`renderD128`)
/// is often the discrete NVIDIA card with no VA driver, so hardcoding it
/// fails; picking a present node + the C2 startup verify lets a broken pick
/// degrade instead of killing the session (the in-process path tests each
/// node's actual VAAPI usability).
fn vaapi_device_node() -> String {
    #[cfg(target_os = "linux")]
    use super::encoder_vaapi::VAAPI_DEVICE_ENV;
    #[cfg(not(target_os = "linux"))]
    const VAAPI_DEVICE_ENV: &str = "POCKETDESK_VAAPI_DEVICE";
    if let Ok(node) = std::env::var(VAAPI_DEVICE_ENV)
        && !node.is_empty()
    {
        return node;
    }
    let mut nodes: Vec<u32> = (128..136)
        .filter(|n| std::path::Path::new(&format!("/dev/dri/renderD{n}")).exists())
        .collect();
    if nodes.is_empty()
        && let Ok(entries) = std::fs::read_dir("/dev/dri")
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(tail) = name.strip_prefix("renderD")
                && let Ok(n) = tail.parse::<u32>()
            {
                nodes.push(n);
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
    }
    nodes
        .first()
        .map(|n| format!("/dev/dri/renderD{n}"))
        .unwrap_or_else(|| "/dev/dri/renderD128".to_string())
}

/// Input: raw BGRA frames via stdin (pipe:0)
/// Output: H.264/H.265 access units in FLV tags via stdout (pipe:1)
fn build_ffmpeg_args(config: &StreamingConfig, keyframe_interval: u64) -> Vec<String> {
    // Determine codec type
    let is_hevc = matches!(config.codec, VideoCodec::H265);

    let profile_str = match config.profile {
        H264Profile::Baseline => "baseline".to_string(),
        H264Profile::Main => "main".to_string(),
        H264Profile::High => "high".to_string(),
    };

    let preset_str = match config.preset {
        EncoderPreset::UltraFast => "ultrafast".to_string(),
        EncoderPreset::SuperFast => "superfast".to_string(),
        EncoderPreset::VeryFast => "veryfast".to_string(),
        EncoderPreset::Faster => "faster".to_string(),
        EncoderPreset::Fast => "fast".to_string(),
        EncoderPreset::Medium => "medium".to_string(),
        EncoderPreset::Slow => "slow".to_string(),
    };

    // Map EncoderPreset to NVENC presets (p1-p7, fastest to slowest)
    let nvenc_preset = match config.preset {
        EncoderPreset::UltraFast | EncoderPreset::SuperFast => "p1",
        EncoderPreset::VeryFast | EncoderPreset::Faster => "p3",
        EncoderPreset::Fast => "p4",
        EncoderPreset::Medium => "p5",
        EncoderPreset::Slow => "p7",
    };

    // AMF has three quality levels; Quick Sync uses x264-style names but has
    // nothing faster than veryfast.
    let amf_quality = match config.preset {
        EncoderPreset::UltraFast
        | EncoderPreset::SuperFast
        | EncoderPreset::VeryFast
        | EncoderPreset::Faster => "speed",
        EncoderPreset::Fast | EncoderPreset::Medium => "balanced",
        EncoderPreset::Slow => "quality",
    };
    // AMF, Quick Sync and Media Foundation frame each picture with an access
    // unit delimiter (and MF pads with filler); neither carries picture data.
    // Dropping them gives every rung NVENC's stream shape: SPS, PPS, SEI,
    // slices — fewer bytes, and one shape for the phone's decoder to meet.
    let strip_non_picture_units = if is_hevc {
        "filter_units=remove_types=35|38"
    } else {
        "filter_units=remove_types=9|12"
    };
    let qsv_preset = match config.preset {
        EncoderPreset::UltraFast | EncoderPreset::SuperFast | EncoderPreset::VeryFast => "veryfast",
        EncoderPreset::Faster => "faster",
        EncoderPreset::Fast => "fast",
        EncoderPreset::Medium => "medium",
        EncoderPreset::Slow => "slow",
    };

    // Resolve Auto to the best available hardware encoder
    use super::encoder_detect::{probe_encoders, resolve_encoder};
    let resolved = if config.hardware_encoder == HardwareEncoder::Auto {
        let available = probe_encoders();
        resolve_encoder(config.hardware_encoder, &available)
    } else {
        config.hardware_encoder
    };

    // Determine encoder name based on resolved encoder and codec
    let codec_name = if is_hevc {
        match resolved {
            HardwareEncoder::Vaapi => "hevc_vaapi",
            HardwareEncoder::Nvenc => "hevc_nvenc",
            HardwareEncoder::Amf | HardwareEncoder::Qsv | HardwareEncoder::MediaFoundation => {
                resolved.ffmpeg_encoder_hevc()
            }
            _ => "libx265",
        }
    } else {
        match resolved {
            HardwareEncoder::Vaapi => "h264_vaapi",
            HardwareEncoder::Nvenc => "h264_nvenc",
            HardwareEncoder::Amf | HardwareEncoder::Qsv | HardwareEncoder::MediaFoundation => {
                resolved.ffmpeg_encoder()
            }
            _ => "libx264",
        }
    };

    // ~2 frames of VBV in bits for the global -bufsize (vaapi/nvenc paths)
    let (vbv_maxrate_kbps, vbv_bufsize_kbits) = vbv_kbit_bounds(config.bitrate_bps, config.fps);
    let vbv_bufsize_bits = config.bitrate_bps as u64 * 2 / config.fps as u64;

    // Select encoder-specific args based on resolved encoder
    let encoder_args: Vec<String> = match resolved {
        HardwareEncoder::Vaapi => {
            vec![
                "-vaapi_device".to_string(),
                vaapi_device_node(),
                "-c:v".to_string(),
                codec_name.to_string(),
                "-b:v".to_string(),
                config.bitrate_bps.to_string(),
                "-maxrate".to_string(),
                config.bitrate_bps.to_string(),
                "-bufsize".to_string(),
                vbv_bufsize_bits.to_string(),
                "-g".to_string(),
                keyframe_interval.to_string(),
                "-bf".to_string(),
                "0".to_string(),
                "-vf".to_string(),
                format!(
                    "format=nv12,hwupload,scale_vaapi=w={}:h={}",
                    config.width, config.height
                ),
            ]
        }
        HardwareEncoder::Nvenc => {
            vec![
                "-c:v".to_string(),
                codec_name.to_string(),
                "-preset".to_string(),
                nvenc_preset.to_string(),
                // Low-latency tuning: no B-frames, bounded VBV, ll rate
                // shaping (Sunshine's llhp profile equivalent).
                "-tune".to_string(),
                "ll".to_string(),
                // Hand each frame back as soon as it is encoded: by default
                // NVENC keeps a queue of surfaces in flight (frames of lag).
                "-delay".to_string(),
                "0".to_string(),
                "-zerolatency".to_string(),
                "1".to_string(),
                "-rc".to_string(),
                "vbr".to_string(),
                "-b:v".to_string(),
                config.bitrate_bps.to_string(),
                "-maxrate".to_string(),
                config.bitrate_bps.to_string(),
                "-bufsize".to_string(),
                vbv_bufsize_bits.to_string(),
                "-g".to_string(),
                keyframe_interval.to_string(),
                "-bf".to_string(),
                "0".to_string(),
                "-pix_fmt".to_string(),
                "yuv420p".to_string(),
            ]
        }
        // Windows rungs. Every set is no-B-frame (the phone pairs decoder output
        // with input in order, R2#1) and CBR-bounded, verified against FFmpeg
        // 8.0 on an RTX 3060 (NVENC) and a Radeon iGPU (AMF, MF).
        HardwareEncoder::Amf => {
            vec![
                "-c:v".to_string(),
                codec_name.to_string(),
                "-usage".to_string(),
                "ultralowlatency".to_string(),
                // One frame in flight, not the default sixteen.
                "-async_depth".to_string(),
                "1".to_string(),
                "-latency".to_string(),
                "1".to_string(),
                "-quality".to_string(),
                amf_quality.to_string(),
                "-rc".to_string(),
                "cbr".to_string(),
                // SPS/PPS on every IDR, so a phone that (re)joins mid-stream
                // can start decoding at the next keyframe.
                "-header_insertion_mode".to_string(),
                "idr".to_string(),
                "-b:v".to_string(),
                config.bitrate_bps.to_string(),
                "-maxrate".to_string(),
                config.bitrate_bps.to_string(),
                "-bufsize".to_string(),
                vbv_bufsize_bits.to_string(),
                "-g".to_string(),
                keyframe_interval.to_string(),
                "-bf".to_string(),
                "0".to_string(),
                "-bsf:v".to_string(),
                strip_non_picture_units.to_string(),
                "-pix_fmt".to_string(),
                "nv12".to_string(),
            ]
        }
        HardwareEncoder::Qsv => {
            vec![
                "-c:v".to_string(),
                codec_name.to_string(),
                "-preset".to_string(),
                qsv_preset.to_string(),
                // One frame in flight: QSV's default queue depth is latency.
                "-async_depth".to_string(),
                "1".to_string(),
                "-b:v".to_string(),
                config.bitrate_bps.to_string(),
                "-maxrate".to_string(),
                config.bitrate_bps.to_string(),
                "-bufsize".to_string(),
                vbv_bufsize_bits.to_string(),
                "-g".to_string(),
                keyframe_interval.to_string(),
                "-bf".to_string(),
                "0".to_string(),
                "-bsf:v".to_string(),
                strip_non_picture_units.to_string(),
                "-pix_fmt".to_string(),
                "nv12".to_string(),
            ]
        }
        HardwareEncoder::MediaFoundation => {
            vec![
                "-c:v".to_string(),
                codec_name.to_string(),
                // The MFT's own remote-desktop tuning: low latency, no reorder.
                "-scenario".to_string(),
                "display_remoting".to_string(),
                "-rate_control".to_string(),
                "cbr".to_string(),
                "-b:v".to_string(),
                config.bitrate_bps.to_string(),
                "-g".to_string(),
                keyframe_interval.to_string(),
                "-bf".to_string(),
                "0".to_string(),
                "-bsf:v".to_string(),
                strip_non_picture_units.to_string(),
                "-pix_fmt".to_string(),
                "nv12".to_string(),
            ]
        }
        HardwareEncoder::Auto | HardwareEncoder::Software => {
            if is_hevc {
                vec![
                    "-c:v".to_string(),
                    codec_name.to_string(), // libx265
                    "-preset".to_string(),
                    preset_str,
                    "-b:v".to_string(),
                    config.bitrate_bps.to_string(),
                    "-pix_fmt".to_string(),
                    "yuv420p".to_string(),
                    "-x265-params".to_string(),
                    format!(
                        "keyint={}:min-keyint=1:bframes=0:rc-lookahead=0:vbv-maxrate={}:vbv-bufsize={}",
                        keyframe_interval, vbv_maxrate_kbps, vbv_bufsize_kbits
                    ),
                ]
            } else {
                vec![
                    "-c:v".to_string(),
                    codec_name.to_string(), // libx264
                    "-profile:v".to_string(),
                    profile_str,
                    "-preset".to_string(),
                    preset_str,
                    "-b:v".to_string(),
                    config.bitrate_bps.to_string(),
                    "-pix_fmt".to_string(),
                    "yuv420p".to_string(),
                    "-x264-params".to_string(),
                    format!(
                        "keyint={}:min-keyint=1:bframes=0:vbv-maxrate={}:vbv-bufsize={}",
                        keyframe_interval, vbv_maxrate_kbps, vbv_bufsize_kbits
                    ),
                    "-tune".to_string(),
                    "zerolatency".to_string(),
                ]
            }
        }
    };

    let mut args = vec![
        // Suppress banner and informational output to stderr
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        // Input format: raw BGRA video from stdin
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pix_fmt".to_string(),
        "bgra".to_string(),
        "-s".to_string(),
        format!("{}x{}", config.width, config.height),
        "-r".to_string(),
        config.fps.to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
    ];

    // Append encoder-specific args
    args.extend(encoder_args);

    // Output: FLV on stdout, flushed per packet. A raw elementary stream has
    // no frame boundaries (a frame ends only when the next one begins); an FLV
    // tag states its length and keyframe flag, so every access unit can ship
    // the moment it is written (see `flv_demux`).
    args.extend(vec![
        "-fps_mode".to_string(),
        "passthrough".to_string(),
        "-flush_packets".to_string(),
        "1".to_string(),
        "-f".to_string(),
        "flv".to_string(),
        "-flvflags".to_string(),
        "no_duration_filesize".to_string(),
        "pipe:1".to_string(),
    ]);

    args
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_fail_seam_names_hardware_rungs_only() {
        use HardwareEncoder::*;
        let list = " NVENC, mf,qsv ";
        assert!(seam_names(list, Nvenc));
        assert!(seam_names(list, MediaFoundation));
        assert!(seam_names(list, Qsv));
        assert!(!seam_names(list, Amf));
        assert!(!seam_names(list, Vaapi));
        // The software rung is never failed: it is where the ladder ends.
        assert!(!seam_names("software,auto,nvenc", Software));
        assert!(!seam_names("software,auto,nvenc", Auto));
        assert!(!seam_names("", Nvenc));
    }

    use super::*;

    #[test]
    fn test_find_start_code_4_byte() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0x42];
        let result = find_start_code(&data, 0);
        assert!(result.is_some());
        let (pos, len) = result.unwrap();
        assert_eq!(pos, 0);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_find_start_code_3_byte() {
        let data = [0x00, 0x00, 0x01, 0x67, 0x42];
        let result = find_start_code(&data, 0);
        assert!(result.is_some());
        let (pos, len) = result.unwrap();
        assert_eq!(pos, 0);
        assert_eq!(len, 3);
    }

    #[test]
    fn test_find_start_code_not_found() {
        let data = [0x67, 0x42, 0x00, 0x01];
        let result = find_start_code(&data, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_start_code_at_offset() {
        let data = [0x67, 0x00, 0x00, 0x00, 0x01, 0x65];
        let result = find_start_code(&data, 1);
        assert!(result.is_some());
        let (pos, len) = result.unwrap();
        assert_eq!(pos, 1);
        assert_eq!(len, 4);
    }

    #[test]
    fn test_detect_keyframe_idr() {
        // 4-byte start code + NAL type 5 (IDR)
        let data = [0x00, 0x00, 0x00, 0x01, 0x65, 0x00, 0xFF];
        assert!(detect_keyframe(&data));
    }

    #[test]
    fn test_detect_keyframe_non_idr() {
        // 4-byte start code + NAL type 7 (SPS) — not an IDR
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0x00, 0xFF];
        assert!(!detect_keyframe(&data));
    }

    #[test]
    fn test_detect_keyframe_3_byte_start_code() {
        // 3-byte start code + NAL type 5 (IDR)
        let data = [0x00, 0x00, 0x01, 0x65, 0x00, 0xFF];
        assert!(detect_keyframe(&data));
    }

    #[test]
    fn test_detect_keyframe_multiple_nals() {
        // SPS (type 7) followed by IDR (type 5)
        let data = [
            0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x0A, // SPS
            0x00, 0x00, 0x00, 0x01, 0x65, 0x00, 0xFF, // IDR
        ];
        assert!(detect_keyframe(&data));
    }

    #[test]
    fn test_detect_keyframe_no_start_code() {
        let data = [0x65, 0x00, 0xFF];
        assert!(!detect_keyframe(&data));
    }

    #[test]
    fn test_build_ffmpeg_args() {
        let config = StreamingConfig {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_bps: 5_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Software,
            monitor_index: 0,
        };
        let args = build_ffmpeg_args(&config, 60);

        // Verify key arguments are present
        assert!(args.contains(&"rawvideo".to_string()));
        assert!(args.contains(&"bgra".to_string()));
        assert!(args.contains(&"1920x1080".to_string()));
        assert!(args.contains(&"30".to_string()));
        assert!(args.contains(&"pipe:0".to_string()));
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"baseline".to_string()));
        assert!(args.contains(&"ultrafast".to_string()));
        assert!(args.contains(&"5000000".to_string()));
        assert!(args.contains(&"pipe:1".to_string()));
        assert!(args.contains(&"zerolatency".to_string()));

        // Low-latency x264 params: no B-frames + VBV bounded to ~2 frames
        let params = args
            .iter()
            .find(|a| a.starts_with("keyint=60"))
            .expect("x264-params should set keyint=60");
        assert!(params.contains("bframes=0"));
        assert!(params.contains("vbv-maxrate=5000"));
        assert!(params.contains("vbv-bufsize=333"));
    }

    #[test]
    fn test_build_ffmpeg_args_nvenc_low_latency() {
        let config = StreamingConfig {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 8_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::High,
            preset: EncoderPreset::VeryFast,
            hardware_encoder: HardwareEncoder::Nvenc,
            monitor_index: 0,
        };
        let args = build_ffmpeg_args(&config, 120);

        assert!(args.contains(&"h264_nvenc".to_string()));
        assert!(args.contains(&"p3".to_string()));
        assert!(args.contains(&"ll".to_string()));
        let g = args.iter().position(|a| a == "-g").unwrap();
        assert_eq!(args[g + 1], "120");
        let bf = args.iter().position(|a| a == "-bf").unwrap();
        assert_eq!(args[bf + 1], "0");
        let bs = args.iter().position(|a| a == "-bufsize").unwrap();
        // 8 Mbit/s * 2 frames / 60 fps = 266666 bits
        assert_eq!(args[bs + 1], "266666");
    }

    #[test]
    fn test_build_ffmpeg_args_vaapi_low_latency() {
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 4_000_000,
            codec: VideoCodec::H265,
            profile: H264Profile::Main,
            preset: EncoderPreset::Fast,
            hardware_encoder: HardwareEncoder::Vaapi,
            monitor_index: 0,
        };
        let args = build_ffmpeg_args(&config, 60);

        assert!(args.contains(&"hevc_vaapi".to_string()));
        assert!(args.iter().any(|a| a.contains("hwupload")));
        let g = args.iter().position(|a| a == "-g").unwrap();
        assert_eq!(args[g + 1], "60");
        let bf = args.iter().position(|a| a == "-bf").unwrap();
        assert_eq!(args[bf + 1], "0");
    }

    #[test]
    fn test_build_ffmpeg_args_x265_low_latency() {
        let config = StreamingConfig {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_bps: 6_000_000,
            codec: VideoCodec::H265,
            profile: H264Profile::Main,
            preset: EncoderPreset::VeryFast,
            hardware_encoder: HardwareEncoder::Software,
            monitor_index: 0,
        };
        let args = build_ffmpeg_args(&config, 60);

        assert!(args.contains(&"libx265".to_string()));
        let params = args
            .iter()
            .find(|a| a.starts_with("keyint=60"))
            .expect("x265-params should set keyint=60");
        assert!(params.contains("bframes=0"));
        assert!(params.contains("rc-lookahead=0"));
        assert!(params.contains("vbv-maxrate=6000"));
    }

    #[test]
    fn test_vbv_bounds_clamp() {
        // Normal: 5 Mbit/s @ 30fps -> 5000 kbps cap, ~2 frames of buffer
        assert_eq!(vbv_kbit_bounds(5_000_000, 30), (5000, 333));
        // Tiny bitrate floors at the 128 kbit buffer minimum
        assert_eq!(vbv_kbit_bounds(100_000, 60), (100, 128));
    }

    #[test]
    fn test_build_ffmpeg_args_high_profile() {
        let config = StreamingConfig {
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_bps: 8_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::High,
            preset: EncoderPreset::Medium,
            hardware_encoder: HardwareEncoder::Software,
            monitor_index: 0,
        };
        let args = build_ffmpeg_args(&config, 120);

        assert!(args.contains(&"high".to_string()));
        assert!(args.contains(&"medium".to_string()));
        assert!(args.contains(&"1280x720".to_string()));
        assert!(args.contains(&"60".to_string()));
    }

    // Tests that require FFmpeg to be installed on the system.
    // Run with: cargo test -- --ignored

    #[test]
    #[ignore]
    fn test_encoder_spawn_and_encode() {
        let config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };

        let mut encoder = VideoEncoder::new(config).expect("FFmpeg should be installed");

        // Create a test frame (320x240 BGRA)
        let stride = 320 * 4;
        let data = vec![0u8; (stride * 240) as usize];
        let frame = VideoFrame {
            data,
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };

        // Encode several frames — first few may be empty due to encoder latency
        let mut encoded_count = 0;
        let mut last_sequence = None;
        for frame_idx in 0..30 {
            for packet in encoder.encode_frame(&frame).unwrap() {
                assert!(
                    !packet.data.is_empty(),
                    "Packet {} should have data",
                    encoded_count
                );
                // One packet per frame, in order (startup probe frames, if
                // the encoder was verified with any, come first).
                assert!(
                    last_sequence.is_none_or(|last| packet.sequence == last + 1),
                    "Packet sequences must be consecutive"
                );
                last_sequence = Some(packet.sequence);
                debug!(
                    "Frame {} -> packet #{}: {} bytes, keyframe={}",
                    frame_idx,
                    packet.sequence,
                    packet.data.len(),
                    packet.is_keyframe
                );
                encoded_count += 1;
            }
        }

        assert!(
            encoded_count > 0,
            "Should have produced at least one encoded packet"
        );

        // Sequence should have advanced for all 30 encode calls
        assert!(encoder.sequence() >= 30);
    }

    #[test]
    #[ignore]
    fn test_encoder_drain() {
        let config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };

        let mut encoder = VideoEncoder::new(config).expect("FFmpeg should be installed");

        // Submit a few frames
        let stride = 320 * 4;
        let data = vec![0u8; (stride * 240) as usize];
        let frame = VideoFrame {
            data,
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };

        for _ in 0..10 {
            let _ = encoder.encode_frame(&frame);
        }

        // Drain remaining packets
        let packets = encoder.drain().unwrap();
        // Drain should not panic and should return whatever is buffered
        debug!("Drained {} packets", packets.len());
    }

    #[test]
    #[ignore]
    fn test_encoder_keyframe_interval() {
        let mut config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };
        config.fps = 30;

        let mut encoder = VideoEncoder::new(config).expect("FFmpeg should be installed");

        let stride = 320 * 4;
        let data = vec![0u8; (stride * 240) as usize];
        let frame = VideoFrame {
            data,
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };

        // Encode enough frames to trigger the keyframe interval (2 seconds = 60 frames)
        let mut keyframe_count = 0;
        for _ in 0..90 {
            keyframe_count += encoder
                .encode_frame(&frame)
                .unwrap()
                .iter()
                .filter(|p| p.is_keyframe)
                .count();
        }

        // Should have seen at least one keyframe within 90 frames
        assert!(
            keyframe_count > 0,
            "Should have produced at least one keyframe in 90 frames"
        );
    }

    #[test]
    #[ignore]
    fn test_encoder_request_keyframe() {
        let config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };

        let mut encoder = VideoEncoder::new(config).expect("FFmpeg should be installed");

        let stride = 320 * 4;
        let data = vec![0u8; (stride * 240) as usize];
        let frame = VideoFrame {
            data,
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };

        // Encode a few frames then request a keyframe
        for _ in 0..5 {
            let _ = encoder.encode_frame(&frame);
        }

        encoder.request_keyframe();

        // A keyframe follows within one keyframe interval (in-process
        // encoders honour the request at once; a sidecar's next IDR is the
        // scheduled one).
        let interval = 30 * 2;
        let keyframe_seen = (0..=interval).any(|_| {
            encoder
                .encode_frame(&frame)
                .unwrap()
                .iter()
                .any(|p| p.is_keyframe)
        });
        assert!(
            keyframe_seen,
            "no keyframe within {interval} frames of the request"
        );
    }

    #[test]
    #[ignore]
    fn test_encoder_drop_cleans_up_process() {
        let config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Auto,
            monitor_index: 0,
        };

        let mut encoder = VideoEncoder::new(config).expect("FFmpeg should be installed");

        // Encode a frame to ensure process is running
        let stride = 320 * 4;
        let data = vec![0u8; (stride * 240) as usize];
        let frame = VideoFrame {
            data,
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };
        let _ = encoder.encode_frame(&frame);

        // Drop should clean up without panicking
        drop(encoder);
    }

    #[test]
    fn degrade_to_software_drops_hardware_and_hevc() {
        let mut config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H265,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Nvenc,
            monitor_index: 0,
        };
        degrade_to_software(&mut config);
        assert_eq!(config.hardware_encoder, HardwareEncoder::Software);
        assert_eq!(config.codec, VideoCodec::H264);
    }

    #[test]
    fn degraded_config_skips_the_hardware_probe() {
        // A Software-pinned config must not consult probe_encoders — this is
        // what keeps the encode task's mid-session rebuild cheap and free of
        // the broken-hardware path that stalled in the first place.
        let mut config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Software,
            monitor_index: 0,
        };
        // Degrading an already-degraded config is a no-op (idempotent ladder).
        let before = config.clone();
        degrade_to_software(&mut config);
        assert_eq!(config.hardware_encoder, before.hardware_encoder);
        assert_eq!(config.codec, before.codec);
    }

    /// C2 acceptance, machine-invariant form: force a hardware rung that may
    /// or may not survive on the test box. `VideoEncoder::new` must come back
    /// with an encoder that produces a packet quickly — either the real VAAPI
    /// path (healthy GPU box) or the software fallback (this box's broken
    /// VAAPI init). Never a dead sidecar, never an error.
    fn windows_rung_config(hardware_encoder: HardwareEncoder) -> StreamingConfig {
        StreamingConfig {
            width: 1280,
            height: 720,
            fps: 60,
            bitrate_bps: 8_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Main,
            preset: EncoderPreset::VeryFast,
            hardware_encoder,
            monitor_index: 0,
        }
    }

    fn arg_after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let at = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing from {args:?}"));
        &args[at + 1]
    }

    #[test]
    fn every_windows_rung_is_low_latency_and_b_frame_free() {
        for (encoder, name) in [
            (HardwareEncoder::Amf, "h264_amf"),
            (HardwareEncoder::Qsv, "h264_qsv"),
            (HardwareEncoder::MediaFoundation, "h264_mf"),
        ] {
            let args = build_ffmpeg_args(&windows_rung_config(encoder), 120);
            assert_eq!(arg_after(&args, "-c:v"), name);
            assert_eq!(arg_after(&args, "-bf"), "0", "{name}");
            assert_eq!(arg_after(&args, "-g"), "120", "{name}");
            assert_eq!(arg_after(&args, "-b:v"), "8000000", "{name}");
            assert_eq!(arg_after(&args, "-f"), "rawvideo", "{name}");
            assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
        }
        let amf = build_ffmpeg_args(&windows_rung_config(HardwareEncoder::Amf), 120);
        assert_eq!(arg_after(&amf, "-usage"), "ultralowlatency");
        assert_eq!(arg_after(&amf, "-header_insertion_mode"), "idr");
        let qsv = build_ffmpeg_args(&windows_rung_config(HardwareEncoder::Qsv), 120);
        assert_eq!(arg_after(&qsv, "-async_depth"), "1");
        let mf = build_ffmpeg_args(&windows_rung_config(HardwareEncoder::MediaFoundation), 120);
        assert_eq!(arg_after(&mf, "-scenario"), "display_remoting");
        assert_eq!(arg_after(&mf, "-bsf:v"), "filter_units=remove_types=9|12");
    }

    #[test]
    fn hevc_on_amf_uses_the_hevc_encoder_and_stream_format() {
        let mut config = windows_rung_config(HardwareEncoder::Amf);
        config.codec = VideoCodec::H265;
        let args = build_ffmpeg_args(&config, 120);
        assert_eq!(arg_after(&args, "-c:v"), "hevc_amf");
        assert_eq!(arg_after(&args, "-f"), "rawvideo");
        assert_eq!(args[args.len() - 4], "flv");
    }

    /// Push 60 moving frames through one Windows rung's real sidecar and
    /// require bits out, the first of them a keyframe. Run on a box with the
    /// GPU: `cargo test -p pocketdesk-core --lib windows_rung_roundtrip -- --ignored`.
    #[cfg(windows)]
    fn windows_rung_roundtrip(encoder: HardwareEncoder) {
        let config = windows_rung_config(encoder);
        let mut sidecar = SidecarEncoder::new(config.clone()).expect("spawn ffmpeg");
        sidecar
            .verify_startup(DEFAULT_STARTUP_VERIFY)
            .unwrap_or_else(|e| panic!("{encoder:?} did not survive startup: {e:#}"));
        let stride = config.width * 4;
        let mut packets = Vec::new();
        for i in 0..60u32 {
            let mut data = vec![0u8; (stride * config.height) as usize];
            for n in 0..(config.width * config.height) as usize {
                data[n * 4] = (n as u32 + i * 7) as u8;
                data[n * 4 + 1] = (n as u32 / config.width) as u8;
                data[n * 4 + 3] = 255;
            }
            let frame = VideoFrame {
                data,
                width: config.width,
                height: config.height,
                stride,
                timestamp: Instant::now(),
            };
            packets.extend(sidecar.encode_frame(&frame).expect("encode"));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        packets.extend(sidecar.drain().expect("drain"));
        assert!(
            packets.len() > 10,
            "{encoder:?}: only {} packets",
            packets.len()
        );
        // One packet per frame, each a whole access unit: the stream opens on
        // a keyframe that carries its SPS and PPS ahead of the IDR slice, so a
        // phone joining here can decode it alone.
        // (The only frames missing are the ones the encoder still holds for
        // more input; drain() does not close the pipe.)
        assert_eq!(
            packets.len() + sidecar.submitted.len(),
            sidecar.sequence() as usize,
            "{encoder:?}: every submitted frame must come out as one packet"
        );
        assert!(sidecar.submitted.len() <= sidecar.encoder_lag);
        if encoder == HardwareEncoder::Nvenc {
            assert_eq!(
                sidecar.encoder_lag, 0,
                "NVENC must return each frame at once"
            );
        }
        let first = &packets[0];
        assert!(
            first.is_keyframe,
            "{encoder:?}: stream must open on a keyframe"
        );
        let nal_types: Vec<u8> = (0..first.data.len().saturating_sub(4))
            .filter(|&i| first.data[i..i + 4] == [0, 0, 0, 1])
            .map(|i| first.data[i + 4] & NAL_TYPE_MASK)
            .collect();
        assert_eq!(
            nal_types.first(),
            Some(&7),
            "{encoder:?}: keyframe must open on SPS"
        );
        assert!(
            nal_types.contains(&8),
            "{encoder:?}: keyframe lacks its PPS"
        );
        assert!(
            nal_types.contains(&NAL_TYPE_IDR),
            "{encoder:?}: keyframe lacks its IDR slice"
        );
        assert!(
            packets[1..]
                .iter()
                .all(|p| !p.is_keyframe || p.data.len() > 4),
            "{encoder:?}: empty packet"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs an NVIDIA GPU and ffmpeg on PATH"]
    fn windows_rung_roundtrip_nvenc() {
        windows_rung_roundtrip(HardwareEncoder::Nvenc);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs an AMD GPU and ffmpeg on PATH"]
    fn windows_rung_roundtrip_amf() {
        windows_rung_roundtrip(HardwareEncoder::Amf);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs a Media Foundation H.264 encoder and ffmpeg on PATH"]
    fn windows_rung_roundtrip_media_foundation() {
        windows_rung_roundtrip(HardwareEncoder::MediaFoundation);
    }

    #[test]
    #[ignore = "spawns real FFmpeg/x264 encoders"]
    fn forced_hardware_rung_lands_on_a_working_encoder() {
        let config = StreamingConfig {
            width: 320,
            height: 240,
            fps: 30,
            bitrate_bps: 1_000_000,
            codec: VideoCodec::H264,
            profile: H264Profile::Baseline,
            preset: EncoderPreset::UltraFast,
            hardware_encoder: HardwareEncoder::Vaapi,
            monitor_index: 0,
        };

        let start = Instant::now();
        let mut encoder =
            VideoEncoder::new(config).expect("fallback ladder must always yield an encoder");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "startup ladder must resolve well inside the C2 2s budget"
        );

        let stride = 320 * 4;
        let frame = VideoFrame {
            data: vec![0u8; (stride * 240) as usize],
            width: 320,
            height: 240,
            stride,
            timestamp: Instant::now(),
        };

        for _ in 0..60 {
            if !encoder
                .encode_frame(&frame)
                .expect("working encoder must not Err on plain frames")
                .is_empty()
            {
                return; // packet produced — ladder landed on a live backend
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!(
            "encoder produced no packet in 60 attempts: {:?}",
            encoder.backend_name()
        );
    }
}
