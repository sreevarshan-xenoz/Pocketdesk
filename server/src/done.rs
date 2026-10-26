//! `pocketdesk done` (BF1): tell the phone when a command ends. The command
//! runs here with its output passing through as it comes, or a process
//! already running is waited on by its id; then an alert saying how it
//! ended, how long it took and its last lines is left for the server, which
//! shows it as one of this computer's notifications. The output goes
//! through a pipe, not a terminal: a program that draws progress or colours
//! only on a terminal prints plainly.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use pocketdesk_core::local::alerts::{self, Ended, TAIL_LINES};

/// The exit code for bad arguments, as the other commands use.
const BAD_ARGUMENTS: i32 = 2;

/// The exit code for a command that could not be started, as a shell's.
const NOT_STARTED: i32 = 127;

/// How long the output may run on after the command ended (review: a
/// program it left running in the background keeps the pipes open, and
/// `done` waited on that, never telling the phone).
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

/// Run `command`, or wait on `pid`, then leave the alert, named `title`
/// when given (review: the command line can carry a secret, and the alert
/// shows on the phone, maybe on its lock screen). The exit code is the
/// command's own (a process waited on: its own on Windows, else 0).
pub fn run(pid: Option<u32>, command: &[String], with_output: bool, title: Option<&str>) -> i32 {
    let started = Instant::now();
    let (what, ended, code, tail) = match (pid, command.split_first()) {
        (Some(pid), _) => match wait_pid(pid) {
            Ok((name, ended)) => (name, ended, exit_code(ended), Vec::new()),
            Err(why) => {
                eprintln!("pocketdesk done: {why}");
                return BAD_ARGUMENTS;
            }
        },
        (None, Some((program, args))) => {
            let what = command.join(" ");
            match run_command(program, args) {
                Ok(ran) => (what, ran.ended, ran.code, ran.tail),
                Err(e) => {
                    eprintln!("pocketdesk done: could not start {program}: {e}");
                    (what, Ended::NotStarted, NOT_STARTED, Vec::new())
                }
            }
        }
        (None, None) => {
            eprintln!("pocketdesk done: name a command after `--`, or a process with --pid");
            return BAD_ARGUMENTS;
        }
    };
    let what = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(&what);
    let tail = if with_output { tail } else { Vec::new() };
    let (title, text) = alerts::words(what, ended, started.elapsed(), &tail);
    tell(&title, &text);
    code
}

/// What `pocketdesk done` exits with when a process it waited on ended as
/// `ended`.
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

/// How a command run here ended.
struct Ran {
    ended: Ended,
    /// What `done` exits with: the command's code, or 128 and the signal
    /// that stopped it, as a shell says.
    code: i32,
    tail: Vec<String>,
}

/// Run `program` with `args`, its output passing through and its last
/// lines kept.
fn run_command(program: &str, args: &[String]) -> std::io::Result<Ran> {
    let mut child = Command::new(resolve(program))
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let tail = Arc::new(Mutex::new(Tail::default()));
    let (finished, copied) = mpsc::channel();
    let mut copies = 0;
    if let Some(out) = child.stdout.take() {
        copy(out, std::io::stdout(), tail.clone(), 0, finished.clone());
        copies += 1;
    }
    if let Some(err) = child.stderr.take() {
        copy(err, std::io::stderr(), tail.clone(), 1, finished);
        copies += 1;
    }
    let status = child.wait()?;
    // The output to its end, but not past a grace: what is still copying
    // then is left to go on alone.
    let until = Instant::now() + OUTPUT_GRACE;
    for _ in 0..copies {
        let left = until.saturating_duration_since(Instant::now());
        if copied.recv_timeout(left).is_err() {
            break;
        }
    }
    let (ended, code) = match status.code() {
        Some(code) => (Ended::Exit(code), code),
        None => (Ended::Stopped, signal_code(&status)),
    };
    let lines = tail.lock().unwrap_or_else(|e| e.into_inner()).lines();
    Ok(Ran {
        ended,
        code,
        tail: lines,
    })
}

/// 128 and the signal that stopped the command, as a shell says.
#[cfg(unix)]
fn signal_code(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map_or(1, |signal| 128 + signal)
}

#[cfg(not(unix))]
fn signal_code(_status: &std::process::ExitStatus) -> i32 {
    1
}

/// `program` as Windows finds it (review: `npm`, `gradlew` and `yarn` are
/// `.cmd` scripts, which only a name with its extension starts): the first
/// match on PATH by PATHEXT, else the name as it is.
#[cfg(windows)]
fn resolve(program: &str) -> std::path::PathBuf {
    resolve_in(
        program,
        &std::env::var_os("PATH").unwrap_or_default(),
        &std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string()),
    )
}

/// [`resolve`] on `path` with `extensions`.
#[cfg(windows)]
fn resolve_in(program: &str, path: &std::ffi::OsStr, extensions: &str) -> std::path::PathBuf {
    let named = std::path::Path::new(program);
    if named.extension().is_some() || named.components().count() > 1 {
        return named.to_path_buf();
    }
    for dir in std::env::split_paths(path) {
        for extension in extensions.split(';').filter(|e| !e.is_empty()) {
            let candidate = dir.join(format!("{program}{extension}"));
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    named.to_path_buf()
}

#[cfg(not(windows))]
fn resolve(program: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(program)
}

/// Pass `from` through to `to` as it comes, keeping its last lines as
/// stream `stream`, and say so on `finished` at its end.
fn copy(
    mut from: impl Read + Send + 'static,
    mut to: impl Write + Send + 'static,
    tail: Arc<Mutex<Tail>>,
    stream: usize,
    finished: mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(n) = from.read(&mut buffer) {
            if n == 0 {
                break;
            }
            // A Windows console refuses what is not UTF-8 (review: a tool
            // in the console's code page printed nothing): shown as best
            // it reads.
            if let Err(e) = to.write_all(&buffer[..n])
                && e.kind() == std::io::ErrorKind::InvalidData
            {
                let _ = to.write_all(String::from_utf8_lossy(&buffer[..n]).as_bytes());
            }
            let _ = to.flush();
            tail.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(stream, &buffer[..n]);
        }
        let _ = finished.send(());
    });
}

/// The last lines of a command's output, both streams as they came, each
/// line its own stream's (review: two streams' halves were spliced). A
/// carriage return starts the line again, as a terminal shows a progress
/// bar, unless a newline follows it (Windows ends its lines so).
#[derive(Default)]
struct Tail {
    lines: VecDeque<String>,
    open: [Vec<u8>; 2],
    /// A carriage return was the stream's last byte: what follows decides.
    returned: [bool; 2],
}

impl Tail {
    fn push(&mut self, stream: usize, bytes: &[u8]) {
        let stream = stream.min(1);
        for &byte in bytes {
            if std::mem::take(&mut self.returned[stream]) && byte != b'\n' {
                self.open[stream].clear();
            }
            match byte {
                b'\n' => {
                    let line = String::from_utf8_lossy(&self.open[stream]).into_owned();
                    self.open[stream].clear();
                    if !line.trim().is_empty() {
                        self.lines.push_back(line);
                        while self.lines.len() > TAIL_LINES {
                            self.lines.pop_front();
                        }
                    }
                }
                b'\r' => self.returned[stream] = true,
                // A line that never ends is kept to a bound.
                _ if self.open[stream].len() < 4096 => self.open[stream].push(byte),
                _ => {}
            }
        }
    }

    fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.lines.iter().cloned().collect();
        for open in &self.open {
            let open = String::from_utf8_lossy(open).into_owned();
            if !open.trim().is_empty() {
                lines.push(open);
            }
        }
        let excess = lines.len().saturating_sub(TAIL_LINES);
        lines.split_off(excess)
    }
}

/// Wait for the process `pid` to end: its name, and how it ended.
#[cfg(windows)]
fn wait_pid(pid: u32) -> Result<(String, Ended), String> {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
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
    .map_err(|_| format!("no process {pid}, or not one this user may wait on"))?;
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
    let waited = unsafe { WaitForSingleObject(process, INFINITE) };
    let mut code = 0u32;
    // Its code only once it ended (review: a failed wait read STILL_ACTIVE).
    let exited =
        waited == WAIT_OBJECT_0 && unsafe { GetExitCodeProcess(process, &mut code) }.is_ok();
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
        std::thread::sleep(Duration::from_millis(500));
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
        tail.push(0, b"one\ntwo\n\n");
        tail.push(0, b"progress 10%\rprogress 100%\nthree\nfour");
        assert_eq!(tail.lines(), ["progress 100%", "three", "four"]);
        let mut split = Tail::default();
        split.push(0, b"hal");
        split.push(0, b"f a line\n");
        assert_eq!(split.lines(), ["half a line"]);
        // Windows ends a line with a carriage return and a newline, maybe
        // split between two reads.
        let mut crlf = Tail::default();
        crlf.push(0, b"first \r");
        crlf.push(0, b"\nlast\r\n");
        assert_eq!(crlf.lines(), ["first ", "last"]);
        // Review: halves of lines from the two streams stay apart.
        let mut both = Tail::default();
        both.push(0, b"out ");
        both.push(1, b"err\n");
        both.push(0, b"line\n");
        assert_eq!(both.lines(), ["err", "out line"]);
    }

    /// BF1: `pocketdesk done` exits as a process it waited on did.
    #[test]
    fn it_exits_as_the_process_did() {
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
        let ran = run_command(program, &args).unwrap();
        assert_eq!(ran.ended, Ended::Exit(3));
        assert_eq!(ran.code, 3);
        assert_eq!(
            ran.tail.iter().map(|l| l.trim()).collect::<Vec<_>>(),
            ["first", "last"]
        );
        assert!(run_command("pocketdesk-no-such-program", &[]).is_err());
    }

    /// Review: a program the command left running in the background, its
    /// output still open, does not hold `done` past the grace.
    #[test]
    fn a_program_left_running_does_not_hold_it() {
        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            ["/C", "start /b ping -n 8 127.0.0.1 >nul & echo started"].map(str::to_string),
        );
        #[cfg(not(windows))]
        let (program, args) = ("sh", ["-c", "sleep 8 & echo started"].map(str::to_string));
        let started = Instant::now();
        let ran = run_command(program, &args).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(ran.ended, Ended::Exit(0));
    }

    /// Review: on Windows a `.cmd` script starts by its bare name, as a
    /// shell finds it.
    #[cfg(windows)]
    #[test]
    fn a_script_is_found_by_its_bare_name() {
        let dir = std::env::temp_dir().join(format!("pd-done-script-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pd-done-hello.cmd"), "@echo hello").unwrap();
        let path = std::env::join_paths([dir.clone()]).unwrap();
        let found = resolve_in("pd-done-hello", &path, ".COM;.EXE;.BAT;.CMD");
        let _ = std::fs::remove_dir_all(&dir);
        // Windows names are without case; PATHEXT spells them in capitals.
        assert_eq!(
            found.file_name().unwrap().to_string_lossy().to_lowercase(),
            "pd-done-hello.cmd"
        );
        assert_eq!(
            resolve_in("cmd.exe", &path, ".EXE"),
            std::path::PathBuf::from("cmd.exe"),
            "named whole"
        );
        assert_eq!(
            resolve_in("nope", &path, ".EXE"),
            std::path::PathBuf::from("nope")
        );
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
