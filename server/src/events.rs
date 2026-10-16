//! Events and hooks (ecosystem phase 1a): the server's half. Each event is
//! appended to the owner-only log other programs read (`pocketdesk events`,
//! `core::local::events`), and runs the command the owner set for its kind
//! in `[hooks]`, through the system shell, with the event's JSON on stdin and
//! in `POCKETDESK_EVENT`. config.toml is read again whenever it changed, so
//! a change in Settings applies at once.

use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::events::{self, Event};
use serde_json::Value;

/// Hooks running at once, at most.
const MAX_HOOKS: usize = 4;
/// Hooks waiting for one of those; past this, a hook is skipped (and logged).
const HOOK_QUEUE: usize = 32;
/// A hook still running after this is killed.
const HOOK_DEADLINE: Duration = Duration::from_secs(30);
/// The longest event also put in `POCKETDESK_EVENT`: one variable may hold
/// at most 128 KiB on Linux and 32 767 characters on Windows, and a longer
/// one would stop the hook from starting. Stdin always has the event.
const MAX_EVENT_VAR: usize = 32 * 1024;

/// One writer at a time appends to the log.
static WRITE: Mutex<()> = Mutex::new(());

/// config.toml as last read, and when it was last changed.
static CONFIG: Mutex<Option<(Option<SystemTime>, Config)>> = Mutex::new(None);

/// config.toml now: read again only once it changed, and the last good one
/// kept while it does not read (a save half done, a hand edit gone wrong).
fn current_config() -> Config {
    let path = pocketdesk_core::local::paths::config_dir()
        .map(|dir| dir.join(pocketdesk_core::local::paths::CONFIG_FILE));
    let modified = path
        .as_ref()
        .ok()
        .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let mut cached = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((when, config)) = cached.as_ref()
        && *when == modified
    {
        return config.clone();
    }
    let read = match &path {
        Ok(path) => Config::load_from(path),
        Err(_) => Ok(Config::default()),
    };
    match read {
        Ok(config) => {
            *cached = Some((modified, config.clone()));
            config
        }
        Err(e) => {
            tracing::warn!(error = %e, "config.toml did not read; events go by the last good one");
            cached
                .as_ref()
                .map(|(_, config)| config.clone())
                .unwrap_or_default()
        }
    }
}

/// A hook to run: its kind, its command line, the event's JSON.
type Job = (String, String, String);

/// The queue hooks wait in, and the [`MAX_HOOKS`] threads that run them.
static HOOKS: LazyLock<SyncSender<Job>> = LazyLock::new(|| {
    let (tx, rx) = sync_channel::<Job>(HOOK_QUEUE);
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..MAX_HOOKS {
        let rx = rx.clone();
        let _ = std::thread::Builder::new()
            .name("event-hook".into())
            .spawn(move || run_hooks(&rx));
    }
    tx
});

fn run_hooks(rx: &Mutex<Receiver<Job>>) {
    loop {
        let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        let Ok((kind, command, json)) = job else {
            return;
        };
        match run_hook(&command, &json, HOOK_DEADLINE) {
            Ok(HookEnd::Exited(code)) => tracing::info!(kind, ?code, "Hook ran"),
            Ok(HookEnd::TimedOut) => {
                tracing::warn!(kind, "Hook killed after {}s", HOOK_DEADLINE.as_secs());
            }
            Err(e) => tracing::warn!(kind, error = %e, "Hook could not run"),
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record an event of `kind`: `meta` always, `content` only at full detail.
/// Unit tests of the handlers that call it never reach the user's own log;
/// [`emit_to`] is what they test.
pub fn emit(kind: &str, device_id: Option<&str>, meta: Value, content: Value) {
    if cfg!(test) {
        return;
    }
    let Ok(state_dir) = crate::state::state_dir() else {
        return;
    };
    let config = current_config();
    emit_to(
        &state_dir,
        &config,
        kind,
        device_id,
        meta,
        content,
        now_unix(),
    );
}

/// [`emit`] into `state_dir` under `config`; the event made, if any was.
pub(crate) fn emit_to(
    state_dir: &Path,
    config: &Config,
    kind: &str,
    device_id: Option<&str>,
    meta: Value,
    content: Value,
    at: u64,
) -> Option<Event> {
    let hook = config.hooks.get(kind).filter(|c| !c.trim().is_empty());
    if !config.events && hook.is_none() {
        return None;
    }
    let event = Event::new(kind, device_id, at, meta, content, config.events_detail);
    if config.events {
        let _write = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = events::append(state_dir, &event) {
            tracing::warn!(error = %e, "Could not write an event to the event log");
        }
    }
    if let Some(command) = hook {
        start_hook(kind, command, &event);
    }
    Some(event)
}

/// Queue `command` for `event`: run by one of [`MAX_HOOKS`] threads, or
/// skipped when [`HOOK_QUEUE`] already wait.
fn start_hook(kind: &str, command: &str, event: &Event) {
    let job = (kind.to_string(), command.to_string(), event.to_line());
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = HOOKS.try_send(job) {
        tracing::warn!(kind, "A hook was skipped: {HOOK_QUEUE} are already waiting");
    }
}

/// How a hook ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum HookEnd {
    /// It exited, with this code (none when a signal ended it).
    Exited(Option<i32>),
    /// It ran past its deadline and was killed.
    TimedOut,
}

/// The system shell for `command`, as the exec plugin's: `sh -c` on Unix;
/// on Windows the real `System32\cmd.exe /S /C "line"` (never found on
/// `PATH`), the line kept as typed, with no console window.
fn shell(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut shell = std::process::Command::new(crate::platform::system32("cmd.exe"));
        shell
            // `/S` and the outer quotes: cmd strips just those, so a line
            // that starts with a quoted program and quotes more keeps them.
            .raw_arg(format!("/S /C \"{command}\""))
            .creation_flags(CREATE_NO_WINDOW);
        shell
    }
    #[cfg(not(windows))]
    {
        let mut shell = std::process::Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    }
}

/// Run one hook to its end or `deadline`: the system shell, the event's JSON
/// on stdin and, up to [`MAX_EVENT_VAR`], in `POCKETDESK_EVENT`, its output
/// discarded.
pub(crate) fn run_hook(command: &str, json: &str, deadline: Duration) -> io::Result<HookEnd> {
    let mut shell = shell(command);
    if json.len() <= MAX_EVENT_VAR {
        shell.env("POCKETDESK_EVENT", json);
    }
    let mut child = shell
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    if let Some(mut stdin) = child.stdin.take() {
        // From a thread of its own: a hook that never reads a long event
        // would block the write, and the deadline with it. A hook that does
        // not read its stdin is no failure.
        let json = json.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(json.as_bytes());
            let _ = stdin.write_all(b"\n");
        });
    }
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(HookEnd::Exited(status.code()));
        }
        if started.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(HookEnd::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::events::{EventsDetail, Tail};
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A shell line that copies stdin to `out`.
    fn copy_stdin_to(out: &Path) -> String {
        if cfg!(windows) {
            format!("more > \"{}\"", out.display())
        } else {
            format!("cat > '{}'", out.display())
        }
    }

    /// A shell line that writes the event variable to `out`.
    fn copy_env_to(out: &Path) -> String {
        if cfg!(windows) {
            format!("echo %POCKETDESK_EVENT%> \"{}\"", out.display())
        } else {
            format!("printf '%s' \"$POCKETDESK_EVENT\" > '{}'", out.display())
        }
    }

    #[test]
    fn an_event_is_logged_at_the_configured_detail() {
        let dir = temp("log");
        let mut tail = Tail::at_end(&dir);
        let config = Config::default();
        emit_to(
            &dir,
            &config,
            "file_received",
            Some("p"),
            json!({"size": 3}),
            json!({"name": "a.txt"}),
            5,
        );
        let got = tail.poll();
        assert_eq!(got.len(), 1);
        assert!(
            got[0].fields.get("name").is_none(),
            "metadata only by default"
        );
        let full = Config {
            events_detail: EventsDetail::Full,
            ..Config::default()
        };
        emit_to(
            &dir,
            &full,
            "file_received",
            Some("p"),
            json!({"size": 3}),
            json!({"name": "a.txt"}),
            6,
        );
        assert_eq!(tail.poll()[0].fields.get("name"), Some(&json!("a.txt")));
        let off = Config {
            events: false,
            ..Config::default()
        };
        assert!(emit_to(&dir, &off, "file_received", None, json!({}), json!({}), 7).is_none());
        assert!(tail.poll().is_empty(), "nothing logged with the log off");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_gets_the_event_on_stdin_and_in_its_environment() {
        let dir = temp("stdin");
        let out = dir.join("stdin.txt");
        let json = r#"{"at":1,"event":"link_received","device_id":null,"host":"example.org"}"#;
        assert_eq!(
            run_hook(&copy_stdin_to(&out), json, Duration::from_secs(10)).unwrap(),
            HookEnd::Exited(Some(0))
        );
        let read = std::fs::read_to_string(&out).unwrap();
        assert!(read.contains("\"host\":\"example.org\""), "stdin: {read}");
        let env_out = dir.join("env.txt");
        run_hook(
            &copy_env_to(&env_out),
            r#"{"event":"session_ended"}"#,
            Duration::from_secs(10),
        )
        .unwrap();
        let read = std::fs::read_to_string(&env_out).unwrap();
        assert!(read.contains("session_ended"), "env: {read}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hook_past_its_deadline_is_killed() {
        let slow = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let started = Instant::now();
        assert_eq!(
            run_hook(slow, "{}", Duration::from_millis(500)).unwrap(),
            HookEnd::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_configured_hook_runs_for_its_kind_even_with_the_log_off() {
        let dir = temp("configured");
        let out = dir.join("hooked.txt");
        let mut config = Config {
            events: false,
            ..Config::default()
        };
        config
            .hooks
            .insert("session_started".into(), copy_stdin_to(&out));
        assert!(
            emit_to(
                &dir,
                &config,
                "session_started",
                None,
                json!({"mode": "cast"}),
                json!({}),
                9
            )
            .is_some()
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !out.exists() || std::fs::read_to_string(&out).unwrap_or_default().is_empty() {
            assert!(Instant::now() < deadline, "the hook never ran");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            std::fs::read_to_string(&out)
                .unwrap()
                .contains("\"mode\":\"cast\"")
        );
        assert!(
            !dir.join(pocketdesk_core::local::paths::EVENTS_FILE)
                .exists(),
            "no log with it off"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hook naming a quoted program, with quoted arguments, runs as typed:
    /// cmd's own rule would strip its first and last quote.
    #[cfg(windows)]
    #[test]
    fn a_hook_starting_with_a_quoted_program_runs_as_typed() {
        let dir = temp("quoted");
        let out = dir.join("quoted out.txt");
        let line = format!(
            "\"{}\" /C echo ok> \"{}\"",
            crate::platform::system32("cmd.exe").display(),
            out.display()
        );
        assert_eq!(
            run_hook(&line, "{}", Duration::from_secs(10)).unwrap(),
            HookEnd::Exited(Some(0))
        );
        assert_eq!(std::fs::read_to_string(&out).unwrap().trim(), "ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hook that never reads its stdin is still held to its deadline,
    /// however long the event (a pipe holds only a few KiB).
    #[test]
    fn a_hook_that_ignores_a_long_event_is_still_killed_in_time() {
        let slow = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let long = format!("{{\"text\":\"{}\"}}", "x".repeat(256 * 1024));
        let started = Instant::now();
        assert_eq!(
            run_hook(slow, &long, Duration::from_millis(500)).unwrap(),
            HookEnd::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn many_hooks_at_once_wait_their_turn_instead_of_being_skipped() {
        let dir = temp("queue");
        let mut config = Config {
            events: false,
            ..Config::default()
        };
        let marker = |i: usize| dir.join(format!("hook-{i}.txt"));
        for i in 0..12 {
            config
                .hooks
                .insert("link_received".into(), copy_stdin_to(&marker(i)));
            emit_to(
                &dir,
                &config,
                "link_received",
                None,
                json!({"host": "example.org"}),
                json!({}),
                i as u64,
            );
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while (0..12).any(|i| std::fs::metadata(marker(i)).map(|m| m.len()).unwrap_or(0) == 0) {
            assert!(Instant::now() < deadline, "not every hook ran");
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
