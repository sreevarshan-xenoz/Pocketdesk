//! `pocketdesk done` (BF1): tell the phone when a command ends. The command
//! runs here with its output passing through as it comes (progress bars
//! too), or a process already running is waited on by its id; then an
//! alert saying how it ended, how long it took and its last lines is left
//! for the server, which shows it as one of this computer's notifications.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pocketdesk_core::local::alerts::{self, Ended, TAIL_LINES};

/// The exit code for bad arguments, as the other commands use.
const BAD_ARGUMENTS: i32 = 2;

/// The exit code for a command that could not be started, as a shell's.
const NOT_STARTED: i32 = 127;

/// Run `command`, or wait on `pid`, then leave the alert; the exit code is
/// the command's own (a process waited on: its own on Windows, else 0).
pub fn run(pid: Option<u32>, command: &[String], with_output: bool) -> i32 {
    let started = Instant::now();
    let (what, ended, tail) = match (pid, command.split_first()) {
        (Some(pid), _) => match wait_pid(pid) {
            Ok((name, ended)) => (name, ended, Vec::new()),
            Err(why) => {
                eprintln!("pocketdesk done: {why}");
                return BAD_ARGUMENTS;
            }
        },
        (None, Some((program, args))) => {
            let what = command.join(" ");
            match run_command(program, args) {
                Ok((ended, tail)) => (what, ended, tail),
                Err(e) => {
                    eprintln!("pocketdesk done: could not start {program}: {e}");
                    (what, Ended::NotStarted, Vec::new())
                }
            }
        }
        (None, None) => {
            eprintln!("pocketdesk done: name a command after `--`, or a process with --pid");
            return BAD_ARGUMENTS;
        }
    };
    let tail = if with_output { tail } else { Vec::new() };
    let (title, text) = alerts::words(&what, ended, started.elapsed(), &tail);
    tell(&title, &text);
    exit_code(ended)
}

/// What `pocketdesk done` exits with when the command ended as `ended`.
fn exit_code(ended: Ended) -> i32 {
    match ended {
        Ended::Exit(code) => code,
        Ended::Gone => 0,
        Ended::Stopped => 1,
        Ended::NotStarted => NOT_STARTED,
    }
}

/// Leave the alert for the server, or say why it could not be.
fn tell(title: &str, text: &str) {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("pocketdesk done: no state folder: {e:#}");
            return;
        }
    };
    if !pocketdesk_core::local::server::is_running(&dir) {
        eprintln!("pocketdesk done: no Pocketdesk server is running to tell the phone");
        return;
    }
    if let Err(e) = alerts::request(&dir, title, text, pocketdesk_core::local::pin::now_unix()) {
        eprintln!("pocketdesk done: could not leave the alert: {e}");
    }
}

/// Run `program` with `args`, its output passing through and its last
/// lines kept.
fn run_command(program: &str, args: &[String]) -> std::io::Result<(Ended, Vec<String>)> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let tail = Arc::new(Mutex::new(Tail::default()));
    let mut copies = Vec::new();
    if let Some(out) = child.stdout.take() {
        copies.push(copy(out, std::io::stdout(), tail.clone()));
    }
    if let Some(err) = child.stderr.take() {
        copies.push(copy(err, std::io::stderr(), tail.clone()));
    }
    let status = child.wait()?;
    for copy in copies {
        let _ = copy.join();
    }
    let ended = match status.code() {
        Some(code) => Ended::Exit(code),
        None => Ended::Stopped,
    };
    let lines = tail.lock().unwrap_or_else(|e| e.into_inner()).lines();
    Ok((ended, lines))
}

/// Pass `from` through to `to` as it comes, keeping its last lines.
fn copy(
    mut from: impl Read + Send + 'static,
    mut to: impl Write + Send + 'static,
    tail: Arc<Mutex<Tail>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(n) = from.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let _ = to.write_all(&buffer[..n]);
            let _ = to.flush();
            tail.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(&buffer[..n]);
        }
    })
}

/// The last lines of a command's output, both streams as they came. A
/// carriage return starts the line again, as a terminal shows a progress
/// bar, unless a newline follows it (Windows ends its lines so).
#[derive(Default)]
struct Tail {
    lines: VecDeque<String>,
    open: Vec<u8>,
    /// A carriage return was the last byte: what follows decides.
    returned: bool,
}

impl Tail {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if std::mem::take(&mut self.returned) && byte != b'\n' {
                self.open.clear();
            }
            match byte {
                b'\n' => {
                    let line = String::from_utf8_lossy(&self.open).into_owned();
                    self.open.clear();
                    if !line.trim().is_empty() {
                        self.lines.push_back(line);
                        while self.lines.len() > TAIL_LINES {
                            self.lines.pop_front();
                        }
                    }
                }
                b'\r' => self.returned = true,
                // A line that never ends is kept to a bound.
                _ if self.open.len() < 4096 => self.open.push(byte),
                _ => {}
            }
        }
    }

    fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.lines.iter().cloned().collect();
        let open = String::from_utf8_lossy(&self.open).into_owned();
        if !open.trim().is_empty() {
            lines.push(open);
        }
        let excess = lines.len().saturating_sub(TAIL_LINES);
        lines.split_off(excess)
    }
}

/// Wait for the process `pid` to end: its name, and how it ended.
#[cfg(windows)]
fn wait_pid(pid: u32) -> Result<(String, Ended), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
        WaitForSingleObject,
    };
    let process = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|_| format!("no process {pid} this user may wait on"))?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let named = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let name = named
        .is_ok()
        .then(|| String::from_utf16_lossy(&buffer[..len as usize]))
        .and_then(|path| {
            std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        });
    let _ = unsafe { WaitForSingleObject(process, INFINITE) };
    let mut code = 0u32;
    let exited = unsafe { GetExitCodeProcess(process, &mut code) }.is_ok();
    let _ = unsafe { CloseHandle(process) };
    let what = process_words(pid, name.as_deref());
    Ok((
        what,
        if exited {
            Ended::Exit(code as i32)
        } else {
            Ended::Gone
        },
    ))
}

/// Wait for the process `pid` to end: its name, and that it ended (its
/// code is its parent's to read).
#[cfg(not(windows))]
fn wait_pid(pid: u32) -> Result<(String, Ended), String> {
    // There while /proc has it and it is not a zombie (ended, its parent
    // yet to read its code).
    let alive = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start().to_string())
            })
            .is_some_and(|state| !state.starts_with('Z') && !state.starts_with('X'))
    };
    if pid == 0 || !alive(pid) {
        return Err(format!("no process {pid}"));
    }
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|n| n.trim().to_string());
    while alive(pid) {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Ok((process_words(pid, name.as_deref()), Ended::Gone))
}

/// "cargo.exe (process 4242)", or "process 4242" with no name.
fn process_words(pid: u32, name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => format!("{name} (process {pid})"),
        None => format!("process {pid}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BF1: the last lines of both streams are kept as a terminal shows
    /// them: a carriage return redraws the line, blank lines do not count.
    #[test]
    fn the_last_lines_are_kept_as_shown() {
        let mut tail = Tail::default();
        tail.push(b"one\ntwo\n\n");
        tail.push(b"progress 10%\rprogress 100%\nthree\nfour");
        assert_eq!(tail.lines(), ["progress 100%", "three", "four"]);
        let mut split = Tail::default();
        split.push(b"hal");
        split.push(b"f a line\n");
        assert_eq!(split.lines(), ["half a line"]);
        // Windows ends a line with a carriage return and a newline, maybe
        // split between two reads.
        let mut crlf = Tail::default();
        crlf.push(b"first \r");
        crlf.push(b"\nlast\r\n");
        assert_eq!(crlf.lines(), ["first ", "last"]);
    }

    /// BF1: `pocketdesk done` exits as its command did.
    #[test]
    fn it_exits_as_the_command_did() {
        assert_eq!(exit_code(Ended::Exit(0)), 0);
        assert_eq!(exit_code(Ended::Exit(3)), 3);
        assert_eq!(exit_code(Ended::Gone), 0);
        assert_eq!(exit_code(Ended::NotStarted), NOT_STARTED);
        assert_eq!(
            process_words(42, Some("cargo.exe")),
            "cargo.exe (process 42)"
        );
        assert_eq!(process_words(42, None), "process 42");
    }

    /// BF1: a command runs with its output passing through, and its last
    /// lines and exit code are what the alert says.
    #[test]
    fn a_command_is_run_and_its_ending_kept() {
        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            ["/C", "echo first & echo last & exit 3"].map(str::to_string),
        );
        #[cfg(not(windows))]
        let (program, args) = (
            "sh",
            ["-c", "echo first; echo last; exit 3"].map(str::to_string),
        );
        let (ended, tail) = run_command(program, &args).unwrap();
        assert_eq!(ended, Ended::Exit(3));
        assert_eq!(
            tail.iter().map(|l| l.trim()).collect::<Vec<_>>(),
            ["first", "last"]
        );
        assert!(run_command("pocketdesk-no-such-program", &[]).is_err());
    }

    /// BF1: a process is waited on until it ends.
    #[test]
    fn a_process_is_waited_on() {
        // Long enough to be found before it ends (ping waits a second).
        #[cfg(windows)]
        let mut child = Command::new("cmd")
            .args(["/C", "ping -n 2 127.0.0.1 >nul & exit 4"])
            .spawn()
            .unwrap();
        #[cfg(not(windows))]
        let mut child = Command::new("sh").args(["-c", "sleep 1"]).spawn().unwrap();
        let pid = child.id();
        // Reaped by a thread here, as a shell would, so it does not linger.
        let reaper = std::thread::spawn(move || child.wait());
        let (what, ended) = wait_pid(pid).unwrap();
        assert!(
            what.ends_with(&format!("(process {pid})")) || what == format!("process {pid}"),
            "{what}"
        );
        #[cfg(windows)]
        assert_eq!(ended, Ended::Exit(4));
        #[cfg(not(windows))]
        assert_eq!(ended, Ended::Gone);
        let _ = reaper.join();
        assert!(wait_pid(u32::MAX - 1).is_err(), "no such process");
    }
}
