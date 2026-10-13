//! Is the server running? `<state>/server.pid` names its process; a pid file
//! left by a crash names a dead (or reused) pid, so the process is checked.

use std::path::Path;

use super::paths::PID_FILE;

/// The pid recorded in `<state_dir>/server.pid`, if any.
pub fn server_pid(state_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(state_dir.join(PID_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether the server is running: the pid file names a live process whose
/// executable is `pocketdesk`. On an OS where the image cannot be read, the
/// pid file alone answers.
pub fn is_running(state_dir: &Path) -> bool {
    server_pid(state_dir).is_some_and(process_is_server)
}

/// The names the server runs under: the `pocketdesk` command and its short
/// form `pd`. The desktop app (`pocketdesk-app`) is not one of them.
pub const SERVER_PROGRAMS: [&str; 2] = ["pocketdesk", "pd"];

/// Whether an executable's file name is the server's, with or without
/// `.exe`, in any case.
pub fn is_server_image(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    SERVER_PROGRAMS.contains(&stem)
}

#[cfg(windows)]
fn process_is_server(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let named = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    if named.is_err() {
        return false;
    }
    let image = String::from_utf16_lossy(&buf[..len as usize]);
    Path::new(&image)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_server_image)
}

#[cfg(target_os = "linux")]
fn process_is_server(pid: u32) -> bool {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|exe| exe.file_name()?.to_str().map(is_server_image))
        .unwrap_or(false)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_is_server(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::paths::PID_FILE;

    #[test]
    fn the_server_runs_as_pocketdesk_or_pd_and_nothing_else() {
        for name in [
            "pocketdesk",
            "pocketdesk.exe",
            "POCKETDESK.EXE",
            "pd",
            "pd.exe",
            "PD.EXE",
        ] {
            assert!(is_server_image(name), "{name}");
        }
        for name in [
            "pocketdesk-app",
            "pocketdesk-app.exe",
            "pdf.exe",
            "explorer.exe",
            "",
        ] {
            assert!(!is_server_image(name), "{name}");
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-srv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_pid_file_naming_no_live_process_is_not_running() {
        let dir = temp_dir("dead");
        std::fs::write(dir.join(PID_FILE), "4294967").unwrap();
        assert_eq!(server_pid(&dir), Some(4294967));
        assert!(!is_running(&dir));
    }

    #[test]
    fn a_live_process_that_is_not_the_server_is_not_running() {
        // This test binary is alive, but it is not `pocketdesk`: a reused pid.
        let dir = temp_dir("reused");
        std::fs::write(dir.join(PID_FILE), std::process::id().to_string()).unwrap();
        assert!(!is_running(&dir));
    }

    #[test]
    fn no_pid_file_is_not_running() {
        let dir = temp_dir("none");
        assert_eq!(server_pid(&dir), None);
        assert!(!is_running(&dir));
    }
}
