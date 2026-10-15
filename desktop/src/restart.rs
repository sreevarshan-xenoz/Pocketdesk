//! Start and restart the server from the app.
//!
//! Stopping goes through `pocketdesk stop` (the CLI's graceful-stop path,
//! with its own safety checks); starting prefers how the server is
//! installed — the logon task on Windows, the systemd user unit on Linux —
//! and otherwise launches `pocketdesk start --background`. Never run without
//! the user's click: this app does not stop the server on its own.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pocketdesk_core::local::server;

pub const START_ARGS: [&str; 2] = ["start", "--background"];
pub const STOP_ARGS: [&str; 1] = ["stop"];

/// The logon task `pocketdesk install` registers on Windows.
#[cfg(windows)]
const TASK_NAME: &str = "Pocketdesk";
/// The systemd user unit on Linux.
const UNIT_NAME: &str = "pocketdesk";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPlan {
    /// Run the logon task (Windows), so the server runs as installed.
    LogonTask,
    /// `systemctl --user start` the unit (Linux).
    SystemdUnit,
    /// Launch `<exe> start --background` directly.
    Spawn(PathBuf),
}

/// How to start the server on `os`, given whether it is installed there.
pub fn start_plan(os: Os, installed: bool, exe: PathBuf) -> StartPlan {
    match (os, installed) {
        (Os::Windows, true) => StartPlan::LogonTask,
        (Os::Linux, true) => StartPlan::SystemdUnit,
        (_, false) => StartPlan::Spawn(exe),
    }
}

pub fn server_file_name() -> &'static str {
    if cfg!(windows) {
        "pocketdesk.exe"
    } else {
        "pocketdesk"
    }
}

/// The server executable: the one installed beside this app, else whatever
/// `pocketdesk` is on PATH.
pub fn server_exe(app_dir: &Path) -> PathBuf {
    let beside = app_dir.join(server_file_name());
    if beside.is_file() {
        beside
    } else {
        PathBuf::from(server_file_name())
    }
}

/// The server program beside this app, if it is there (T10e: the file
/// manager's "Send to phone" runs it).
pub fn server_exe_beside_app() -> Option<PathBuf> {
    let exe = server_exe(&app_dir());
    exe.is_file().then_some(exe)
}

fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// A command that shows no console window on Windows.
pub(crate) fn quiet(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[cfg(windows)]
pub(crate) fn system32(tool: &str) -> PathBuf {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..len.min(buf.len())])).join(tool)
}

fn installed(os: Os) -> bool {
    let status = match os {
        #[cfg(windows)]
        Os::Windows => quiet(system32("schtasks.exe"))
            .args(["/query", "/tn", TASK_NAME])
            .output(),
        Os::Linux => Command::new("systemctl")
            .args(["--user", "cat", UNIT_NAME])
            .output(),
        #[cfg(not(windows))]
        Os::Windows => return false,
    };
    status.is_ok_and(|out| out.status.success())
}

fn run(mut command: Command, what: &str) -> Result<(), String> {
    match command.output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Err(format!("{what} failed: {e}")),
    }
}

/// Start the server.
pub fn start() -> Result<(), String> {
    let os = Os::current();
    match start_plan(os, installed(os), server_exe(&app_dir())) {
        #[cfg(windows)]
        StartPlan::LogonTask => {
            let mut command = quiet(system32("schtasks.exe"));
            command.args(["/run", "/tn", TASK_NAME]);
            run(command, "starting the logon task")
        }
        #[cfg(not(windows))]
        StartPlan::LogonTask => Err("no logon task on this OS".into()),
        StartPlan::SystemdUnit => {
            let mut command = Command::new("systemctl");
            command.args(["--user", "start", UNIT_NAME]);
            run(command, "starting the systemd unit")
        }
        StartPlan::Spawn(exe) => quiet(&exe)
            .args(START_ARGS)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("could not run {}: {e}", exe.display())),
    }
}

/// Stop the server, then start it again; waits up to 10 s for it to exit.
pub fn restart(state_dir: &Path) -> Result<(), String> {
    if server::is_running(state_dir) {
        let mut command = quiet(server_exe(&app_dir()));
        command.args(STOP_ARGS);
        run(command, "stopping the server")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while server::is_running(state_dir) {
            if Instant::now() >= deadline {
                return Err("the server did not stop within 10 s".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    start()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-restart-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_installed_server_is_started_the_way_it_was_installed() {
        let exe = PathBuf::from("pocketdesk");
        assert_eq!(
            start_plan(Os::Windows, true, exe.clone()),
            StartPlan::LogonTask
        );
        assert_eq!(
            start_plan(Os::Linux, true, exe.clone()),
            StartPlan::SystemdUnit
        );
        assert_eq!(
            start_plan(Os::Windows, false, exe.clone()),
            StartPlan::Spawn(exe.clone())
        );
        assert_eq!(
            start_plan(Os::Linux, false, exe.clone()),
            StartPlan::Spawn(exe)
        );
    }

    #[test]
    fn the_server_next_to_the_app_wins_over_path() {
        let dir = temp_dir("beside");
        let name = server_file_name();
        std::fs::write(dir.join(name), b"").unwrap();
        assert_eq!(server_exe(&dir), dir.join(name));
        let empty = temp_dir("path");
        assert_eq!(server_exe(&empty), Path::new(name), "falls back to PATH");
    }

    #[test]
    fn the_spawn_arguments_run_it_in_the_background() {
        assert_eq!(START_ARGS, ["start", "--background"]);
        assert_eq!(STOP_ARGS, ["stop"]);
    }
}
