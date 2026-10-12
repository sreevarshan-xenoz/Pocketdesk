//! Route the bridge's `tracing` output to logcat.
//!
//! The bridge and core log through `tracing`, but without a subscriber every
//! message was dropped, so a failing connect on a phone left no trace at all.
//! This installs one that writes each event as a logcat line under the tag
//! `LinuxLinkRust`, through Android's own `liblog` (no extra crate).

use std::io::Write;

/// Install the subscriber once; later calls are no-ops.
pub(crate) fn init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::new(
            "info,linux_link_core=info,linux_link_android_bridge=debug",
        );
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .without_time() // logcat stamps every line itself
            .with_target(true)
            .with_writer(|| LineWriter(Vec::new()))
            .try_init();
    });
}

/// Buffers one formatted event and hands it to logcat when dropped.
struct LineWriter(Vec<u8>);

impl Write for LineWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        let line = String::from_utf8_lossy(&self.0);
        let line = line.trim_end();
        if !line.is_empty() {
            write_logcat(priority_of(line), line);
        }
    }
}

/// The logcat priority for a formatted line (the level is its first word).
fn priority_of(line: &str) -> i32 {
    const DEBUG: i32 = 3;
    const INFO: i32 = 4;
    const WARN: i32 = 5;
    const ERROR: i32 = 6;
    match line.split_whitespace().next() {
        Some("ERROR") => ERROR,
        Some("WARN") => WARN,
        Some("INFO") => INFO,
        _ => DEBUG,
    }
}

#[cfg(target_os = "android")]
fn write_logcat(priority: i32, line: &str) {
    use std::ffi::CString;
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(
            prio: std::ffi::c_int,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> std::ffi::c_int;
    }
    let Ok(text) = CString::new(line.replace('\0', " ")) else {
        return;
    };
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    unsafe {
        __android_log_write(priority, c"LinuxLinkRust".as_ptr(), text.as_ptr());
    }
}

/// Host builds (unit tests) have no logcat: stderr stands in.
#[cfg(not(target_os = "android"))]
fn write_logcat(_priority: i32, line: &str) {
    eprintln!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_to_logcat_priorities() {
        assert_eq!(priority_of("ERROR core: boom"), 6);
        assert_eq!(priority_of("WARN core: hmm"), 5);
        assert_eq!(priority_of("INFO core: ok"), 4);
        assert_eq!(priority_of("DEBUG core: detail"), 3);
        assert_eq!(priority_of("TRACE core: noise"), 3);
    }
}
