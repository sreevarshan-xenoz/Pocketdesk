//! Which spelling of a flag this machine's `ffmpeg` takes.
//!
//! The frame-rate mode is `-fps_mode` since FFmpeg 5.1, which deprecated
//! `-vsync`; FFmpeg 9 removed `-vsync`, so a sidecar that passes it exits at
//! once ("Unrecognized option 'vsync'") and shows no picture. FFmpeg 4.4
//! (Ubuntu 22.04) knows only `-vsync`. Each binary is asked once, through the
//! options `ffmpeg -h long` lists, and its answer is remembered.
//!
//! The encoders a binary was built with are asked the same way, once, through
//! `ffmpeg -encoders`: Fedora's `ffmpeg-free` has no libx264, but has
//! libopenh264, so the software rung has to know which one it can run.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// The frame-rate mode flag of FFmpeg 5.1 and later.
pub const FPS_MODE: &str = "-fps_mode";
/// The frame-rate mode flag before FFmpeg 5.1, gone in FFmpeg 9.
pub const VSYNC: &str = "-vsync";

/// The frame-rate mode flag of an ffmpeg whose `-h long` printed `help`:
/// `-fps_mode` when it is listed, `-vsync` when only that is, and
/// `-fps_mode` when neither is (an answer that could not be read is taken to
/// be a current ffmpeg's).
pub fn fps_mode_flag_in(help: &str) -> &'static str {
    listed_fps_mode_flag(help).unwrap_or(FPS_MODE)
}

/// The frame-rate mode flag `help` lists, if it lists either.
fn listed_fps_mode_flag(help: &str) -> Option<&'static str> {
    let lists = |flag: &str| {
        help.lines().any(|line| {
            line.trim_start()
                .strip_prefix(flag)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '[']))
        })
    };
    if lists(FPS_MODE) {
        Some(FPS_MODE)
    } else if lists(VSYNC) {
        Some(VSYNC)
    } else {
        None
    }
}

/// How long `ffmpeg -h long` may take before its answer is given up on.
const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The frame-rate mode flag `program` takes, asked the first time and
/// remembered for the rest of the process. A program that cannot be run is
/// not remembered (its spawn fails anyway) and gets `-fps_mode`.
pub fn fps_mode_flag(program: impl AsRef<OsStr>) -> &'static str {
    static KNOWN: OnceLock<Mutex<HashMap<OsString, &'static str>>> = OnceLock::new();
    let program = program.as_ref();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(flag) = known.lock().unwrap_or_else(|e| e.into_inner()).get(program) {
        return flag;
    }
    let Some(help) = probe(quiet_command(program, &["-hide_banner", "-h", "long"])) else {
        return FPS_MODE;
    };
    // Only an answer that names a flag is kept: an ffmpeg that started but
    // failed (a missing library, a wedged run) is asked again next time,
    // never remembered as a current one.
    let Some(flag) = listed_fps_mode_flag(&help) else {
        return FPS_MODE;
    };
    known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(program.to_os_string(), flag);
    flag
}

/// The encoder names an `ffmpeg -encoders` listing gives: the second word of
/// each line whose first is the six-letter flag field (`V....D libx264 ...`).
/// The legend above the `------` line (`V..... = Video`) is not a listing.
pub fn listed_encoders_in(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let flags = words.next()?;
            let name = words.next()?;
            let is_flags = flags.len() == 6
                && flags.starts_with(['V', 'A', 'S', '.'])
                && flags.chars().all(|c| c == '.' || c.is_ascii_uppercase());
            (is_flags && name != "=").then(|| name.to_string())
        })
        .collect()
}

/// The encoders `program` lists, asked the first time and remembered for the
/// rest of the process. `None` when it could not be run or listed nothing (a
/// missing library, a wedged run): that answer is not remembered, so the next
/// ask tries again.
pub fn listed_encoders(program: impl AsRef<OsStr>) -> Option<Arc<BTreeSet<String>>> {
    static KNOWN: OnceLock<Mutex<HashMap<OsString, Arc<BTreeSet<String>>>>> = OnceLock::new();
    let program = program.as_ref();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(listed) = known.lock().unwrap_or_else(|e| e.into_inner()).get(program) {
        return Some(listed.clone());
    }
    let listing = probe(quiet_command(program, &["-hide_banner", "-encoders"]))?;
    let listed = listed_encoders_in(&listing);
    if listed.is_empty() {
        return None;
    }
    let listed = Arc::new(listed);
    known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(program.to_os_string(), listed.clone());
    Some(listed)
}

/// `program args`, its output read and nothing else attached; never a
/// console window on Windows.
fn quiet_command(program: &OsStr, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // The desktop app is a GUI program: no console may flash.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// What `command` printed, if it ran and exited well within
/// [`PROBE_DEADLINE`]; one that takes longer is killed.
fn probe(mut command: Command) -> Option<String> {
    use std::io::Read;
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let Ok(out) = rx.recv_timeout(PROBE_DEADLINE) else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let status = child.wait().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FFmpeg 9's `-h long`: `-vsync` is gone.
    const FFMPEG_9: &str = "\
Advanced per-stream options:
-fps_mode[:<stream_spec>]  set framerate mode for matching video streams
-force_fps                  force the selected framerate
";
    /// FFmpeg 5.1 to 8: both, `-vsync` deprecated.
    const FFMPEG_8: &str = "\
Advanced global options:
-vsync <>           set video sync method globally; deprecated, use -fps_mode
Advanced per-stream options:
-fps_mode[:<stream_spec>]  set framerate mode for matching video streams; overrides vsync
";
    /// FFmpeg 4.4 (Ubuntu 22.04): `-vsync` only, and a filter whose name
    /// holds the word, which is not the flag.
    const FFMPEG_4: &str = "\
Advanced Video options:
-vsync              video sync method
-async              audio sync method
avsynctest AVOptions:
";

    #[test]
    fn the_frame_rate_flag_follows_what_ffmpeg_lists() {
        assert_eq!(fps_mode_flag_in(FFMPEG_9), FPS_MODE);
        assert_eq!(fps_mode_flag_in(FFMPEG_8), FPS_MODE);
        assert_eq!(fps_mode_flag_in(FFMPEG_4), VSYNC);
    }

    #[test]
    fn an_answer_that_names_neither_flag_gets_the_current_one() {
        assert_eq!(fps_mode_flag_in(""), FPS_MODE);
        assert_eq!(fps_mode_flag_in("ffmpeg: command not found\n"), FPS_MODE);
        // A mention inside another option's words is not a listing.
        assert_eq!(
            fps_mode_flag_in("-async  audio sync; see -vsync too\n"),
            FPS_MODE
        );
    }

    /// The head and a few lines of FFmpeg 7.1's listing, as Fedora's
    /// `ffmpeg-free` prints it: libopenh264, no libx264.
    const FEDORA_ENCODERS: &str = "\
Encoders:
 V..... = Video
 A..... = Audio
 S..... = Subtitle
 .F.... = Frame-level multithreading
 ..S... = Slice-level multithreading
 ...X.. = Codec is experimental
 ....B. = Supports draw_horiz_band
 .....D = Supports direct rendering method 1
 ------
 V....D libopenh264          OpenH264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D h264_vaapi           H.264/AVC (VAAPI) (codec h264)
 V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 VF...D hevc_vaapi           H.265/HEVC (VAAPI) (codec hevc)
 A....D libopus              libopus Opus (codec opus)
 S..... ass                  ASS (Advanced SubStation Alpha) subtitle
";

    #[test]
    fn the_encoder_listing_is_read_by_name_not_legend() {
        let listed = listed_encoders_in(FEDORA_ENCODERS);
        for name in [
            "libopenh264",
            "h264_vaapi",
            "h264_nvenc",
            "hevc_vaapi",
            "libopus",
            "ass",
        ] {
            assert!(listed.contains(name), "{name}");
        }
        assert!(!listed.contains("libx264"));
        assert!(!listed.contains("="), "the legend is not a listing");
        assert!(!listed.contains("Video"));
        assert_eq!(listed.len(), 6);
    }

    #[test]
    fn a_listing_with_no_encoders_names_none() {
        assert!(listed_encoders_in("").is_empty());
        assert!(listed_encoders_in("ffmpeg: error while loading shared libraries\n").is_empty());
        // A name mentioned in a description is not a listing of its own.
        assert!(!listed_encoders_in(FEDORA_ENCODERS).contains("OpenH264"));
    }

    #[test]
    fn a_program_that_cannot_run_lists_nothing() {
        assert_eq!(listed_encoders("no-such-program-pocketdesk"), None);
    }

    #[test]
    fn a_program_that_cannot_run_gets_the_current_flag() {
        assert_eq!(fps_mode_flag("no-such-program-pocketdesk"), FPS_MODE);
    }

    /// A run that fails is no answer, so it is never remembered (the PR
    /// review: an ffmpeg missing a library would otherwise be taken for a
    /// current one for the whole process).
    #[test]
    fn a_failed_run_is_no_answer() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .arg("--no-such-flag-pocketdesk")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        assert_eq!(probe(command), None);
    }
}
