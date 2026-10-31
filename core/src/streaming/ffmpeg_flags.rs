//! Which spelling of a flag this machine's `ffmpeg` takes.
//!
//! The frame-rate mode is `-fps_mode` since FFmpeg 5.1, which deprecated
//! `-vsync`; FFmpeg 9 removed `-vsync`, so a sidecar that passes it exits at
//! once ("Unrecognized option 'vsync'") and shows no picture. FFmpeg 4.4
//! (Ubuntu 22.04) knows only `-vsync`. Each binary is asked once, through the
//! options `ffmpeg -h long` lists, and its answer is remembered.
//!
//! The encoders a binary was built with are asked through `ffmpeg -encoders`
//! and remembered for [`LISTING_TTL`], or until the binary on disk changes:
//! Fedora's `ffmpeg-free` has no libx264, but has libopenh264, so the software
//! rung has to know which one it can run.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

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
                && flags.starts_with(['V', 'A', 'S'])
                && flags.chars().all(|c| c == '.' || c.is_ascii_uppercase());
            (is_flags && name != "=").then(|| name.to_string())
        })
        .collect()
}

/// How long a listing is trusted before `ffmpeg -encoders` is asked again,
/// even when the binary looks unchanged (a library it loads may have been
/// swapped under it).
pub const LISTING_TTL: Duration = Duration::from_secs(10 * 60);

/// One build of a program on disk: the file it resolves to, its modification
/// time and its size. `None` when it cannot be found.
pub type Stamp = Option<(PathBuf, Option<SystemTime>, u64)>;

/// The [`Stamp`] of `program` now: the path itself when it names one, else
/// the first match on `PATH` (with `.exe` on Windows).
pub fn binary_stamp(program: &OsStr) -> Stamp {
    let path = resolve_program(program)?;
    let meta = std::fs::metadata(&path).ok()?;
    Some((path, meta.modified().ok(), meta.len()))
}

fn resolve_program(program: &OsStr) -> Option<PathBuf> {
    let named = Path::new(program);
    if named.components().count() > 1 || named.is_absolute() {
        return named.is_file().then(|| named.to_path_buf());
    }
    let mut names = vec![program.to_os_string()];
    if cfg!(windows) {
        let mut exe = program.to_os_string();
        exe.push(".exe");
        names.push(exe);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Whether an answer remembered at `then` for the binary stamped `was` still
/// holds at `now`, when the binary is stamped `is`: the same file, unchanged,
/// and asked less than `ttl` ago.
pub fn still_fresh(was: &Stamp, then: Instant, is: &Stamp, now: Instant, ttl: Duration) -> bool {
    was == is && now.saturating_duration_since(then) < ttl
}

/// The encoders `program` lists, remembered for [`LISTING_TTL`] or until the
/// binary changes on disk. `None` when it could not be run or listed nothing
/// (a missing library, a wedged run): that answer is not remembered, so the
/// next ask tries again.
pub fn listed_encoders(program: impl AsRef<OsStr>) -> Option<Arc<BTreeSet<String>>> {
    type Entry = (Stamp, Instant, Arc<BTreeSet<String>>);
    static KNOWN: OnceLock<Mutex<HashMap<OsString, Entry>>> = OnceLock::new();
    let program = program.as_ref();
    let known = KNOWN.get_or_init(Default::default);
    let stamp = binary_stamp(program);
    if let Some((was, then, listed)) = known.lock().unwrap_or_else(|e| e.into_inner()).get(program)
        && still_fresh(was, *then, &stamp, Instant::now(), LISTING_TTL)
    {
        return Some(listed.clone());
    }
    let listing = probe(quiet_command(program, &["-hide_banner", "-encoders"]))?;
    let listed = listed_encoders_in(&listing);
    if listed.is_empty() {
        return None;
    }
    let listed = Arc::new(listed);
    known.lock().unwrap_or_else(|e| e.into_inner()).insert(
        program.to_os_string(),
        (stamp, Instant::now(), listed.clone()),
    );
    Some(listed)
}

/// Whether a throwaway one-frame encode through `encoder` succeeds in the
/// ffmpeg at `program` within [`PROBE_DEADLINE`]; a run that takes longer is
/// killed and counts as a failure. No console window on Windows.
pub fn one_frame_encodes(program: &OsStr, encoder: &str) -> bool {
    probe(quiet_command(
        program,
        &[
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
        ],
    ))
    .is_some()
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

    /// FFmpeg 4.4's listing (Ubuntu 22.04): the same columns, libx264rgb
    /// beside libx264, and a slice-threaded flag field.
    const FFMPEG_4_ENCODERS: &str = "\
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
 V..... libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V..... libx264rgb           libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 RGB (codec h264)
 V.S... mpeg2video           MPEG-2 video
 V..... h264_vaapi           H.264/AVC (VAAPI) (codec h264)
 A..X.. opus                 Opus
";

    #[test]
    fn an_ffmpeg_4_listing_reads_the_same() {
        let listed = listed_encoders_in(FFMPEG_4_ENCODERS);
        let expected = ["libx264", "libx264rgb", "mpeg2video", "h264_vaapi", "opus"];
        assert_eq!(listed.len(), expected.len(), "{listed:?}");
        for name in expected {
            assert!(listed.contains(name), "{name}");
        }
    }

    #[test]
    fn a_remembered_listing_is_asked_again_when_old_or_the_binary_changed() {
        let now = Instant::now();
        let then = now - Duration::from_secs(60);
        let at = |len| -> Stamp { Some((PathBuf::from("/usr/bin/ffmpeg"), None, len)) };
        assert!(still_fresh(&at(1), then, &at(1), now, LISTING_TTL));
        assert!(
            !still_fresh(&at(1), then, &at(2), now, LISTING_TTL),
            "replaced"
        );
        let moved: Stamp = Some((PathBuf::from("/opt/ffmpeg/ffmpeg"), None, 1));
        assert!(
            !still_fresh(&at(1), then, &moved, now, LISTING_TTL),
            "moved"
        );
        assert!(
            !still_fresh(&at(1), then, &None, now, LISTING_TTL),
            "removed"
        );
        let long_ago = now - LISTING_TTL - Duration::from_secs(1);
        assert!(
            !still_fresh(&at(1), long_ago, &at(1), now, LISTING_TTL),
            "old"
        );
    }

    #[test]
    fn a_program_is_stamped_by_its_file() {
        let exe = std::env::current_exe().unwrap();
        let stamp = binary_stamp(exe.as_os_str()).expect("this test binary");
        assert_eq!(stamp.0, exe);
        assert_eq!(stamp.2, std::fs::metadata(&exe).unwrap().len());
        assert_eq!(binary_stamp(OsStr::new("no-such-program-pocketdesk")), None);
    }

    #[test]
    fn a_one_frame_encode_through_a_missing_program_fails() {
        assert!(!one_frame_encodes(
            OsStr::new("no-such-program-pocketdesk"),
            "libopenh264"
        ));
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
