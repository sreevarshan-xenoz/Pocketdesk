//! Which spelling of a flag this machine's `ffmpeg` takes.
//!
//! The frame-rate mode is `-fps_mode` since FFmpeg 5.1, which deprecated
//! `-vsync`; FFmpeg 9 removed `-vsync`, so a sidecar that passes it exits at
//! once ("Unrecognized option 'vsync'") and shows no picture. FFmpeg 4.4
//! (Ubuntu 22.04) knows only `-vsync`. Each binary is asked once, through the
//! options `ffmpeg -h long` lists, and its answer is remembered.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// The frame-rate mode flag of FFmpeg 5.1 and later.
pub const FPS_MODE: &str = "-fps_mode";
/// The frame-rate mode flag before FFmpeg 5.1, gone in FFmpeg 9.
pub const VSYNC: &str = "-vsync";

/// The frame-rate mode flag of an ffmpeg whose `-h long` printed `help`:
/// `-fps_mode` when it is listed, `-vsync` when only that is, and
/// `-fps_mode` when neither is (an answer that could not be read is taken to
/// be a current ffmpeg's).
pub fn fps_mode_flag_in(help: &str) -> &'static str {
    let lists = |flag: &str| {
        help.lines().any(|line| {
            line.trim_start()
                .strip_prefix(flag)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '[']))
        })
    };
    if lists(FPS_MODE) {
        FPS_MODE
    } else if lists(VSYNC) {
        VSYNC
    } else {
        FPS_MODE
    }
}

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
    let mut command = Command::new(program);
    command
        .args(["-hide_banner", "-h", "long"])
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
    let Ok(out) = command.output() else {
        return FPS_MODE;
    };
    let flag = fps_mode_flag_in(&String::from_utf8_lossy(&out.stdout));
    known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(program.to_os_string(), flag);
    flag
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

    #[test]
    fn a_program_that_cannot_run_gets_the_current_flag() {
        assert_eq!(fps_mode_flag("no-such-program-pocketdesk"), FPS_MODE);
    }
}
