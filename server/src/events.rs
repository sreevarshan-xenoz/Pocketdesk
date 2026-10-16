//! Events and hooks (ecosystem phase 1a): the server's half. Each event is
//! appended to the owner-only log other programs read (`pocketdesk events`,
//! `core::local::events`), and runs the command the owner set for its kind
//! in `[hooks]`, through the system shell, with the event's JSON on stdin and
//! in `POCKETDESK_EVENT`. config.toml is read at each event, so a change in
//! Settings applies at once.

use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pocketdesk_core::local::config::Config;
use pocketdesk_core::local::events::{self, Event};
use serde_json::Value;

/// Hooks running at once, at most; another is skipped (and logged).
const MAX_HOOKS: usize = 4;
/// A hook still running after this is killed.
const HOOK_DEADLINE: Duration = Duration::from_secs(30);

static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// One writer at a time appends to the log.
static WRITE: Mutex<()> = Mutex::new(());

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
    let config = Config::load().unwrap_or_default();
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

/// Run `command` for `event` on a thread of its own, unless 4 already run.
fn start_hook(kind: &str, command: &str, event: &Event) {
    if RUNNING.fetch_add(1, Ordering::SeqCst) >= MAX_HOOKS {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        tracing::warn!(kind, "A hook was skipped: {MAX_HOOKS} are already running");
        return;
    }
    let (kind, command, json) = (kind.to_string(), command.to_string(), event.to_line());
    let spawned = std::thread::Builder::new()
        .name("event-hook".into())
        .spawn(move || {
            match run_hook(&command, &json, HOOK_DEADLINE) {
                Ok(HookEnd::Exited(code)) => tracing::info!(kind, ?code, "Hook ran"),
                Ok(HookEnd::TimedOut) => {
                    tracing::warn!(kind, "Hook killed after {}s", HOOK_DEADLINE.as_secs());
                }
                Err(e) => tracing::warn!(kind, error = %e, "Hook could not run"),
            }
            RUNNING.fetch_sub(1, Ordering::SeqCst);
        });
    if spawned.is_err() {
        RUNNING.fetch_sub(1, Ordering::SeqCst);
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
/// on Windows the real `System32\cmd.exe /C` (never found on `PATH`), the
/// line passed verbatim, with no console window.
fn shell(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut shell = std::process::Command::new(crate::platform::system32("cmd.exe"));
        shell
            .arg("/C")
            .raw_arg(command)
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
/// on stdin and in `POCKETDESK_EVENT`, its output discarded.
pub(crate) fn run_hook(command: &str, json: &str, deadline: Duration) -> io::Result<HookEnd> {
    let mut child = shell(command)
        .env("POCKETDESK_EVENT", json)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read its stdin is no failure.
        let _ = stdin.write_all(json.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let started = Instant::now();
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
}
