//! Hardware encoder detection for cross-platform video encoding.
//!
//! Probes the system for available hardware-accelerated encoders (VAAPI and
//! NVENC on Linux; NVENC, AMF, Quick Sync and Media Foundation on Windows) and
//! falls back to software encoding (libx264, or libopenh264 where FFmpeg has
//! no libx264: [`software_h264`]). A hardware encoder only counts
//! as available once a throwaway one-frame encode through it succeeds: FFmpeg
//! lists every encoder it was *built* with, whether or not this machine's GPU
//! and driver can run it.

use super::ffmpeg_flags::listed_encoders;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info, warn};

/// Available hardware encoder backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HardwareEncoder {
    /// Auto-detect the best available encoder
    #[default]
    Auto,
    /// VAAPI (Intel/AMD GPUs on Linux)
    Vaapi,
    /// NVENC (NVIDIA GPUs)
    Nvenc,
    /// AMF (AMD GPUs on Windows)
    Amf,
    /// Quick Sync (Intel GPUs on Windows)
    Qsv,
    /// Windows Media Foundation: whichever encoder MFT the system offers (any
    /// vendor). The last hardware rung on Windows.
    MediaFoundation,
    /// Software encoding via libx264 (libopenh264 on an FFmpeg without it,
    /// on the sidecar)
    Software,
}

impl HardwareEncoder {
    /// Human-readable name
    pub fn display_name(&self) -> &'static str {
        match self {
            HardwareEncoder::Auto => "Auto-detect",
            HardwareEncoder::Vaapi => "VAAPI (Intel/AMD)",
            HardwareEncoder::Nvenc => "NVENC (NVIDIA)",
            HardwareEncoder::Amf => "AMF (AMD)",
            HardwareEncoder::Qsv => "Quick Sync (Intel)",
            HardwareEncoder::MediaFoundation => "Media Foundation",
            HardwareEncoder::Software => "Software (libx264)",
        }
    }

    /// FFmpeg encoder name for this selection and codec.
    /// Returns both the encoder name and whether it supports HEVC.
    pub fn supports_hevc(&self) -> bool {
        match self {
            HardwareEncoder::Vaapi => true,
            HardwareEncoder::Nvenc => true,
            HardwareEncoder::Amf => true,
            HardwareEncoder::Qsv => true,
            // hevc_mf exists in FFmpeg but its MFT rejects input on the GPUs
            // tried (0x80004005); H.264 is what this rung can promise.
            HardwareEncoder::MediaFoundation => false,
            HardwareEncoder::Auto | HardwareEncoder::Software => true, // libx265
        }
    }

    /// FFmpeg encoder name for H.264 encoding
    pub fn ffmpeg_encoder(&self) -> &'static str {
        match self {
            HardwareEncoder::Auto | HardwareEncoder::Software => "libx264",
            HardwareEncoder::Vaapi => "h264_vaapi",
            HardwareEncoder::Nvenc => "h264_nvenc",
            HardwareEncoder::Amf => "h264_amf",
            HardwareEncoder::Qsv => "h264_qsv",
            HardwareEncoder::MediaFoundation => "h264_mf",
        }
    }

    /// FFmpeg encoder name for HEVC encoding
    pub fn ffmpeg_encoder_hevc(&self) -> &'static str {
        match self {
            HardwareEncoder::Auto | HardwareEncoder::Software => "libx265",
            HardwareEncoder::Vaapi => "hevc_vaapi",
            HardwareEncoder::Nvenc => "hevc_nvenc",
            HardwareEncoder::Amf => "hevc_amf",
            HardwareEncoder::Qsv => "hevc_qsv",
            HardwareEncoder::MediaFoundation => "hevc_mf",
        }
    }
}

/// Result of probing system for available encoders.
#[derive(Debug, Clone)]
pub struct AvailableEncoders {
    pub vaapi: bool,
    pub nvenc: bool,
    pub amf: bool,
    pub qsv: bool,
    pub media_foundation: bool,
    pub software: bool,
}

impl AvailableEncoders {
    /// Nothing but the software encoder — the result when FFmpeg cannot even
    /// be run, and the base every probe adds to.
    pub fn software_only() -> Self {
        Self {
            vaapi: false,
            nvenc: false,
            amf: false,
            qsv: false,
            media_foundation: false,
            software: true,
        }
    }

    fn any_hardware(&self) -> bool {
        self.vaapi || self.nvenc || self.amf || self.qsv || self.media_foundation
    }
}

/// The Windows encoders, in Auto's preference order after NVENC: each is
/// listed by `ffmpeg -encoders` on any FFmpeg built with it, so each one is
/// runtime-verified before it is offered.
const WINDOWS_ENCODERS: [(HardwareEncoder, &str); 3] = [
    (HardwareEncoder::Amf, "h264_amf"),
    (HardwareEncoder::Qsv, "h264_qsv"),
    (HardwareEncoder::MediaFoundation, "h264_mf"),
];

/// Probe the system for available hardware encoders.
///
/// Reads `ffmpeg -encoders` (asked once per process, see
/// [`listed_encoders`]) for VAAPI and NVENC encoders (both H.264 and HEVC
/// variants) and the Windows rungs. Always reports software as available.
pub fn probe_encoders() -> AvailableEncoders {
    let mut available = AvailableEncoders::software_only();

    let Some(listed) = listed_encoders("ffmpeg") else {
        warn!("Could not list ffmpeg's encoders; assuming software encoding only");
        return available;
    };
    let lists = |name: &str| listed.contains(name);

    if lists("h264_vaapi") || lists("hevc_vaapi") {
        available.vaapi = true;
        debug!("Detected VAAPI encoder");
    }
    let nvenc_listed = lists("h264_nvenc") || lists("hevc_nvenc");
    if nvenc_listed {
        debug!("Detected NVENC encoder");
    }
    let windows_listed = WINDOWS_ENCODERS.map(|(_, name)| lists(name));

    // FFmpeg lists VAAPI whenever it was built with it, Windows builds and
    // GPU-less machines included. It can only run on Linux with a render
    // node; elsewhere, Auto picking it skipped every working rung after it
    // (AMF, Quick Sync, Media Foundation) for software.
    if available.vaapi && !vaapi_usable(cfg!(target_os = "linux"), render_node_count()) {
        available.vaapi = false;
        info!(
            "VAAPI compiled into FFmpeg but this machine has no VA render node; not selecting VAAPI (Auto)"
        );
    }

    if nvenc_listed {
        // FFmpeg was *built* with NVENC, but the driver may predate this
        // FFmpeg's NVENC API level (e.g. 580.x vs FFmpeg 9's API-13), in which
        // case every open fails at runtime. One 1-frame encode settles it.
        if encoder_runtime_ok("h264_nvenc") {
            available.nvenc = true;
        } else {
            available.nvenc = false;
            info!("NVENC compiled into FFmpeg but driver rejects it; not selecting NVENC (Auto)");
        }
    }

    // AMF, Quick Sync and Media Foundation are the Windows rungs. Linux keeps
    // its historical NVENC/VAAPI-only probe (and its startup cost).
    if cfg!(windows) {
        for (listed, (encoder, name)) in windows_listed.into_iter().zip(WINDOWS_ENCODERS) {
            let works = listed && encoder_runtime_ok(name);
            if listed && !works {
                info!("{name} compiled into FFmpeg but this machine cannot run it");
            }
            match encoder {
                HardwareEncoder::Amf => available.amf = works,
                HardwareEncoder::Qsv => available.qsv = works,
                HardwareEncoder::MediaFoundation => available.media_foundation = works,
                _ => unreachable!("WINDOWS_ENCODERS lists only Windows rungs"),
            }
        }
    }

    if !available.any_hardware() {
        info!("No hardware encoder detected, using software encoding");
    } else {
        info!(
            "Hardware encoders available: VAAPI={}, NVENC={}, AMF={}, QSV={}, MF={}",
            available.vaapi,
            available.nvenc,
            available.amf,
            available.qsv,
            available.media_foundation
        );
    }

    available
}

/// Run a throwaway 1-frame encode through `encoder` to prove this machine can
/// actually run it: an NVIDIA driver older than the FFmpeg build's NVENC API, a
/// Quick Sync encoder without an Intel GPU, and every other "built but cannot
/// run" case exits non-zero.
fn encoder_runtime_ok(encoder: &str) -> bool {
    encoder_runs_in(OsStr::new("ffmpeg"), encoder)
}

/// [`encoder_runtime_ok`] through the ffmpeg at `program`.
fn encoder_runs_in(program: &OsStr, encoder: &str) -> bool {
    match Command::new(program)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=black:s=256x256:r=10",
            "-c:v",
            encoder,
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ])
        .output()
    {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Whether VAAPI can run here: on Linux, with a render node to open.
fn vaapi_usable(linux: bool, render_nodes: usize) -> bool {
    linux && render_nodes > 0
}

/// How many `/dev/dri/renderD*` nodes exist.
fn render_node_count() -> usize {
    std::fs::read_dir("/dev/dri")
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("renderD"))
                .count()
        })
        .unwrap_or(0)
}

/// The software H.264 encoders the sidecar can run, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SoftwareH264 {
    /// libx264: the software rung wherever FFmpeg has it.
    X264,
    /// Cisco's OpenH264, for an FFmpeg built without libx264 (Fedora's
    /// `ffmpeg-free`). Constrained Baseline only, and lower quality per bit
    /// than x264, but every phone decodes it.
    OpenH264,
}

impl SoftwareH264 {
    /// The FFmpeg encoder name.
    pub fn ffmpeg_encoder(self) -> &'static str {
        match self {
            SoftwareH264::X264 => "libx264",
            SoftwareH264::OpenH264 => "libopenh264",
        }
    }
}

/// The software H.264 encoder an FFmpeg that lists `listed` offers: libx264
/// when it has it, else libopenh264, else none.
pub fn pick_software_h264(listed: &BTreeSet<String>) -> Option<SoftwareH264> {
    [SoftwareH264::X264, SoftwareH264::OpenH264]
        .into_iter()
        .find(|encoder| listed.contains(encoder.ffmpeg_encoder()))
}

/// What the software rung says when the system ffmpeg has no software H.264
/// encoder at all.
pub const NO_SOFTWARE_H264: &str = "the system ffmpeg has no software H.264 encoder (neither libx264 nor libopenh264); install an FFmpeg built with one (on Fedora, RPM Fusion's ffmpeg: sudo dnf swap ffmpeg-free ffmpeg --allowerasing)";

/// What the software rung says when ffmpeg lists libopenh264 but cannot open
/// it: Fedora ships a stub library in its place until Cisco's is installed.
pub const OPENH264_UNUSABLE: &str = "the system ffmpeg lists libopenh264 but cannot open it (on Fedora the library may be the noopenh264 stub: sudo dnf install openh264, from the fedora-cisco-openh264 repository) and has no libx264";

/// The software H.264 encoder the ffmpeg at `program` can run.
///
/// libx264 when listed. libopenh264 when only it is, and a throwaway
/// one-frame encode through it succeeds (Fedora's `noopenh264` stub is
/// listed but cannot encode). An ffmpeg whose encoders could not be listed is
/// taken to have libx264, the historical default, so the error that reaches
/// the user is the spawn's own.
pub fn software_h264(program: &OsStr) -> Result<SoftwareH264, &'static str> {
    let Some(listed) = listed_encoders(program) else {
        return Ok(SoftwareH264::X264);
    };
    match pick_software_h264(&listed) {
        Some(SoftwareH264::X264) => Ok(SoftwareH264::X264),
        Some(SoftwareH264::OpenH264) => {
            // A check that passed is remembered: a session rebuilds its
            // encoder on every crop, and each would pay another spawn. One
            // that failed is asked again, so installing the library helps
            // without a restart.
            static RUNS: OnceLock<Mutex<HashSet<OsString>>> = OnceLock::new();
            let runs = RUNS.get_or_init(Default::default);
            if runs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(program)
            {
                return Ok(SoftwareH264::OpenH264);
            }
            if encoder_runs_in(program, SoftwareH264::OpenH264.ffmpeg_encoder()) {
                runs.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(program.to_os_string());
                Ok(SoftwareH264::OpenH264)
            } else {
                Err(OPENH264_UNUSABLE)
            }
        }
        None => Err(NO_SOFTWARE_H264),
    }
}

/// Whether the ffmpeg at `program` lists libx265; `true` when its encoders
/// could not be listed (the historical assumption).
pub fn lists_libx265(program: &OsStr) -> bool {
    listed_encoders(program).is_none_or(|listed| listed.contains("libx265"))
}

/// Resolve the `HardwareEncoder` selection to the actual encoder to use,
/// falling back through the chain if the selected encoder is not available.
pub fn resolve_encoder(
    selected: HardwareEncoder,
    available: &AvailableEncoders,
) -> HardwareEncoder {
    match selected {
        HardwareEncoder::Auto => {
            if available.nvenc {
                info!("Auto-detect: selecting NVENC");
                HardwareEncoder::Nvenc
            } else if available.vaapi {
                info!("Auto-detect: selecting VAAPI");
                HardwareEncoder::Vaapi
            } else if available.amf {
                info!("Auto-detect: selecting AMF");
                HardwareEncoder::Amf
            } else if available.qsv {
                info!("Auto-detect: selecting Quick Sync");
                HardwareEncoder::Qsv
            } else if available.media_foundation {
                info!("Auto-detect: selecting Media Foundation");
                HardwareEncoder::MediaFoundation
            } else {
                info!("Auto-detect: no hardware encoder, using software");
                HardwareEncoder::Software
            }
        }
        HardwareEncoder::Vaapi if !available.vaapi => {
            warn!("VAAPI selected but not available, falling back to software");
            HardwareEncoder::Software
        }
        HardwareEncoder::Nvenc if !available.nvenc => {
            warn!("NVENC selected but not available, falling back to software");
            HardwareEncoder::Software
        }
        HardwareEncoder::Amf if !available.amf => {
            warn!("AMF selected but not available, falling back to software");
            HardwareEncoder::Software
        }
        HardwareEncoder::Qsv if !available.qsv => {
            warn!("Quick Sync selected but not available, falling back to software");
            HardwareEncoder::Software
        }
        HardwareEncoder::MediaFoundation if !available.media_foundation => {
            warn!("Media Foundation selected but not available, falling back to software");
            HardwareEncoder::Software
        }
        other => other,
    }
}

/// Build the FFmpeg encoder argument for the given hardware encoder
/// and resolution. Some encoders require additional flags (e.g., VAAPI
/// requires a device selection).
pub fn encoder_args(hw_encoder: &HardwareEncoder, width: u32, height: u32) -> Vec<String> {
    match hw_encoder {
        HardwareEncoder::Vaapi => {
            vec![
                "-vaapi_device".to_string(),
                "/dev/dri/renderD128".to_string(), // Default VAAPI device
                "-c:v".to_string(),
                "h264_vaapi".to_string(),
                "-vf".to_string(),
                format!("format=nv12,hwupload,scale_vaapi=w={}:h={}", width, height),
            ]
        }
        HardwareEncoder::Nvenc => {
            vec![
                "-c:v".to_string(),
                "h264_nvenc".to_string(),
                "-preset".to_string(),
                "p1".to_string(), // Fastest NVENC preset
                "-rc".to_string(),
                "vbr".to_string(),
            ]
        }
        HardwareEncoder::Amf | HardwareEncoder::Qsv | HardwareEncoder::MediaFoundation => {
            vec!["-c:v".to_string(), hw_encoder.ffmpeg_encoder().to_string()]
        }
        HardwareEncoder::Auto | HardwareEncoder::Software => {
            vec![
                "-c:v".to_string(),
                "libx264".to_string(),
                "-preset".to_string(),
                "ultrafast".to_string(),
                "-tune".to_string(),
                "zerolatency".to_string(),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardware_encoder_display_names() {
        assert_eq!(HardwareEncoder::Auto.display_name(), "Auto-detect");
        assert_eq!(HardwareEncoder::Vaapi.display_name(), "VAAPI (Intel/AMD)");
        assert_eq!(HardwareEncoder::Nvenc.display_name(), "NVENC (NVIDIA)");
        assert_eq!(
            HardwareEncoder::Software.display_name(),
            "Software (libx264)"
        );
    }

    #[test]
    fn test_ffmpeg_encoder_names() {
        assert_eq!(HardwareEncoder::Auto.ffmpeg_encoder(), "libx264");
        assert_eq!(HardwareEncoder::Vaapi.ffmpeg_encoder(), "h264_vaapi");
        assert_eq!(HardwareEncoder::Nvenc.ffmpeg_encoder(), "h264_nvenc");
        assert_eq!(HardwareEncoder::Software.ffmpeg_encoder(), "libx264");
    }

    #[test]
    fn test_resolve_auto_with_nvenc() {
        let available = AvailableEncoders {
            vaapi: false,
            nvenc: true,
            ..AvailableEncoders::software_only()
        };
        assert_eq!(
            resolve_encoder(HardwareEncoder::Auto, &available),
            HardwareEncoder::Nvenc
        );
    }

    #[test]
    fn vaapi_runs_only_on_linux_with_a_render_node() {
        assert!(vaapi_usable(true, 1));
        assert!(!vaapi_usable(true, 0), "a GPU-less Linux machine");
        assert!(!vaapi_usable(false, 0), "Windows, whatever FFmpeg lists");
        assert!(!vaapi_usable(false, 2));
    }

    #[test]
    fn test_resolve_auto_with_vaapi() {
        let available = AvailableEncoders {
            vaapi: true,
            nvenc: false,
            ..AvailableEncoders::software_only()
        };
        assert_eq!(
            resolve_encoder(HardwareEncoder::Auto, &available),
            HardwareEncoder::Vaapi
        );
    }

    #[test]
    fn test_resolve_auto_software_only() {
        let available = AvailableEncoders {
            vaapi: false,
            nvenc: false,
            ..AvailableEncoders::software_only()
        };
        assert_eq!(
            resolve_encoder(HardwareEncoder::Auto, &available),
            HardwareEncoder::Software
        );
    }

    #[test]
    fn test_resolve_vaapi_fallback() {
        let available = AvailableEncoders {
            vaapi: false,
            nvenc: false,
            ..AvailableEncoders::software_only()
        };
        assert_eq!(
            resolve_encoder(HardwareEncoder::Vaapi, &available),
            HardwareEncoder::Software
        );
    }

    #[test]
    fn test_resolve_nvenc_stays() {
        let available = AvailableEncoders {
            vaapi: false,
            nvenc: true,
            ..AvailableEncoders::software_only()
        };
        assert_eq!(
            resolve_encoder(HardwareEncoder::Nvenc, &available),
            HardwareEncoder::Nvenc
        );
    }

    #[test]
    fn auto_prefers_nvenc_then_the_windows_rungs_in_order() {
        let only = |set: fn(&mut AvailableEncoders)| {
            let mut available = AvailableEncoders::software_only();
            set(&mut available);
            resolve_encoder(HardwareEncoder::Auto, &available)
        };
        assert_eq!(only(|a| a.amf = true), HardwareEncoder::Amf);
        assert_eq!(only(|a| a.qsv = true), HardwareEncoder::Qsv);
        assert_eq!(
            only(|a| a.media_foundation = true),
            HardwareEncoder::MediaFoundation
        );
        let mut everything = AvailableEncoders::software_only();
        everything.nvenc = true;
        everything.amf = true;
        everything.media_foundation = true;
        assert_eq!(
            resolve_encoder(HardwareEncoder::Auto, &everything),
            HardwareEncoder::Nvenc
        );
        everything.nvenc = false;
        assert_eq!(
            resolve_encoder(HardwareEncoder::Auto, &everything),
            HardwareEncoder::Amf
        );
    }

    #[test]
    fn an_unavailable_windows_rung_falls_to_software() {
        let none = AvailableEncoders::software_only();
        for encoder in [
            HardwareEncoder::Amf,
            HardwareEncoder::Qsv,
            HardwareEncoder::MediaFoundation,
        ] {
            assert_eq!(resolve_encoder(encoder, &none), HardwareEncoder::Software);
        }
    }

    #[test]
    fn media_foundation_does_not_promise_hevc() {
        assert!(!HardwareEncoder::MediaFoundation.supports_hevc());
        assert_eq!(HardwareEncoder::Amf.ffmpeg_encoder(), "h264_amf");
        assert_eq!(HardwareEncoder::Qsv.ffmpeg_encoder_hevc(), "hevc_qsv");
    }

    #[test]
    fn test_encoder_args_vaapi() {
        let args = encoder_args(&HardwareEncoder::Vaapi, 1920, 1080);
        assert!(args.contains(&"-vaapi_device".to_string()));
        assert!(args.contains(&"h264_vaapi".to_string()));
        assert!(args.contains(&"format=nv12,hwupload,scale_vaapi=w=1920:h=1080".to_string()));
    }

    #[test]
    fn test_encoder_args_nvenc() {
        let args = encoder_args(&HardwareEncoder::Nvenc, 1280, 720);
        assert!(args.contains(&"h264_nvenc".to_string()));
        assert!(args.contains(&"p1".to_string()));
    }

    fn names(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_software_ladder_is_x264_then_openh264_then_nothing() {
        assert_eq!(
            pick_software_h264(&names(&["libx264", "libopenh264", "h264_vaapi"])),
            Some(SoftwareH264::X264)
        );
        // Fedora's ffmpeg-free.
        assert_eq!(
            pick_software_h264(&names(&["libopenh264", "h264_vaapi", "h264_nvenc"])),
            Some(SoftwareH264::OpenH264)
        );
        assert_eq!(
            pick_software_h264(&names(&["h264_vaapi", "h264_nvenc", "libx265"])),
            None
        );
        // libx264rgb encodes RGB, which no phone decodes: not the rung.
        assert_eq!(pick_software_h264(&names(&["libx264rgb"])), None);
        assert_eq!(SoftwareH264::OpenH264.ffmpeg_encoder(), "libopenh264");
        assert_eq!(SoftwareH264::X264.ffmpeg_encoder(), "libx264");
    }

    #[test]
    fn an_ffmpeg_that_cannot_be_listed_keeps_the_historical_software_rung() {
        let missing = OsStr::new("no-such-program-pocketdesk");
        assert_eq!(software_h264(missing), Ok(SoftwareH264::X264));
        assert!(lists_libx265(missing));
    }

    #[test]
    fn test_encoder_args_software() {
        let args = encoder_args(&HardwareEncoder::Software, 1920, 1080);
        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"ultrafast".to_string()));
    }
}
