//! Named tasks (ecosystem phase 1c): the commands the owner names in
//! config.toml, listed to a phone with the Tasks permission (never their
//! command lines) and started at its asking. A task that asks first waits
//! for the owner's answer in the desktop app (`core::local::tasks`). Each
//! run is held to its deadline and its output to its last 64 KiB, at most 2
//! at once and one of each task; it is answered `running`, then `done`, or
//! `refused` with why, and logged as events.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::tasks::{self, TaskRequest, TaskSpec, TaskVerdict};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::{Value, json};

pub const TASKS: &str = "kdeconnect.pocketdesk.tasks";
pub const TASK_RUN: &str = "kdeconnect.pocketdesk.task_run";
pub const TASK_RESULT: &str = "kdeconnect.pocketdesk.task_result";

/// Runs at once, at most.
const MAX_RUNNING: usize = 2;

/// The tasks running now, by name.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// The starts held for the owner, as `task_requests.json` lists them.
static HELD: LazyLock<Mutex<Vec<TaskRequest>>> = LazyLock::new(Default::default);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// A phone's id for a run: short printable text, echoed back.
fn valid_run_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 64 && !id.chars().any(char::is_control)
}

/// The list a phone is shown: names and whether each asks first.
pub(crate) fn listing(specs: &[TaskSpec]) -> Value {
    let tasks: Vec<Value> = specs
        .iter()
        .filter(|t| tasks::name_problem(&t.name).is_none() && !t.command.trim().is_empty())
        .map(|t| json!({ "name": t.name, "ask": t.ask }))
        .collect();
    json!({ "tasks": tasks })
}

fn result(id: &str, body: Value) -> NetworkPacket {
    let mut body = body;
    if let Some(map) = body.as_object_mut() {
        map.insert("id".into(), json!(id));
    }
    NetworkPacket::new(TASK_RESULT).with_body(body)
}

fn refused(id: &str, reason: &str) -> NetworkPacket {
    result(id, json!({ "state": "refused", "reason": reason }))
}

/// Take a place for `name`: none when it runs already, or 2 tasks do.
fn claim(name: &str) -> bool {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.len() >= MAX_RUNNING || running.contains(name) {
        return false;
    }
    running.insert(name.to_string());
    true
}

fn release(name: &str) {
    RUNNING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(name);
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ran {
    pub code: Option<i32>,
    pub output: String,
    pub truncated: bool,
    pub timed_out: bool,
}

/// Run `command` through the system shell to its end or `deadline`: its
/// stdout, then its stderr, kept to their last 64 KiB together.
pub(crate) async fn run(command: &str, deadline: Duration) -> std::io::Result<Ran> {
    let child = shell(command)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    match tokio::time::timeout(deadline, child.wait_with_output()).await {
        Ok(out) => {
            let out = out?;
            let mut both = out.stdout;
            if !out.stderr.is_empty() {
                if !both.is_empty() && !both.ends_with(b"\n") {
                    both.push(b'\n');
                }
                both.extend_from_slice(&out.stderr);
            }
            let (output, truncated) = tasks::output_tail(&both);
            Ok(Ran {
                code: out.status.code(),
                output,
                truncated,
                timed_out: false,
            })
        }
        // Past the deadline the child is dropped, and so killed.
        Err(_) => Ok(Ran {
            code: None,
            output: String::new(),
            truncated: false,
            timed_out: true,
        }),
    }
}

/// The system shell, as the exec plugin's: `sh -c`, or System32's
/// `cmd.exe /S /C "line"` with no window.
fn shell(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut shell = tokio::process::Command::new(crate::platform::system32("cmd.exe"));
        shell
            .raw_arg(format!("/S /C \"{command}\""))
            .creation_flags(CREATE_NO_WINDOW);
        shell
    }
    #[cfg(not(windows))]
    {
        let mut shell = tokio::process::Command::new("sh");
        shell.arg("-c").arg(command);
        shell
    }
}

/// Hold `task`'s start for the owner; their answer, or a deny after a minute.
async fn ask_owner(device_id: &str, task: &str) -> TaskVerdict {
    let Ok(dir) = crate::state::state_dir() else {
        return TaskVerdict::Deny;
    };
    let request = TaskRequest {
        id: uuid::Uuid::new_v4().simple().to_string(),
        device_id: device_id.to_string(),
        task: task.to_string(),
        asked_unix: now_unix(),
    };
    let write_held = |held: &[TaskRequest]| {
        if let Err(e) = tasks::write_requests(&dir, held) {
            tracing::warn!("Could not list a task waiting for you: {e}");
        }
    };
    {
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|r| r.is_open(now_unix()));
        held.push(request.clone());
        write_held(&held);
    }
    let summary = format!("Pocketdesk: a phone wants to run \"{task}\"");
    std::thread::spawn(move || {
        crate::platform::notify(
            &summary,
            "Allow or deny it in the Pocketdesk app.",
            Duration::from_secs(15),
        );
    });
    let mut verdict = TaskVerdict::Deny;
    while request.is_open(now_unix()) {
        if let Some(answer) = tasks::take_answer(&dir, &request.id, now_unix()) {
            verdict = answer;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    held.retain(|r| r.id != request.id && r.is_open(now_unix()));
    write_held(&held);
    verdict
}

/// Send `packet` to `device_id` on its live connection, if it has one.
async fn send_to(device_id: &str, packet: &NetworkPacket) {
    for client in crate::state::clone_clients().await {
        if client.device_id() == device_id {
            let _ = client.send_packet(packet).await;
            return;
        }
    }
}

#[derive(Debug, Default)]
pub struct TasksPlugin;

#[async_trait::async_trait]
impl Plugin for TasksPlugin {
    fn name(&self) -> &'static str {
        "tasks"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[TASKS, TASK_RUN]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[TASKS, TASK_RESULT]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let specs = crate::events::current_config().tasks;
        match packet.packet_type.as_str() {
            TASKS => {
                let reply = NetworkPacket::new(TASKS).with_body(listing(&specs));
                sender.send_packet(&reply).await?;
            }
            TASK_RUN => {
                let id = packet.body.get("id").and_then(Value::as_str).unwrap_or("");
                let name = packet
                    .body
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if !valid_run_id(id) {
                    return Ok(());
                }
                let device = sender.device_id().to_string();
                let refuse = |reason: &'static str| {
                    crate::events::emit(
                        "task_refused",
                        Some(&device),
                        json!({ "task": name, "reason": reason }),
                        json!({}),
                    );
                    refused(id, reason)
                };
                let Some(spec) = specs.iter().find(|t| t.name == name).cloned() else {
                    sender.send_packet(&refuse("unknown")).await?;
                    return Ok(());
                };
                if !claim(&spec.name) {
                    sender.send_packet(&refuse("busy")).await?;
                    return Ok(());
                }
                let id = id.to_string();
                tokio::spawn(async move {
                    if spec.ask && ask_owner(&device, &spec.name).await == TaskVerdict::Deny {
                        release(&spec.name);
                        crate::events::emit(
                            "task_refused",
                            Some(&device),
                            json!({ "task": spec.name, "reason": "denied" }),
                            json!({}),
                        );
                        send_to(&device, &refused(&id, "denied")).await;
                        return;
                    }
                    crate::events::emit(
                        "task_started",
                        Some(&device),
                        json!({ "task": spec.name }),
                        json!({}),
                    );
                    send_to(&device, &result(&id, json!({ "state": "running" }))).await;
                    let ran = run(&spec.command, spec.deadline()).await;
                    release(&spec.name);
                    let body = match ran {
                        Ok(ran) => {
                            crate::events::emit(
                                "task_finished",
                                Some(&device),
                                json!({ "task": spec.name, "code": ran.code, "timed_out": ran.timed_out }),
                                json!({}),
                            );
                            json!({
                                "state": "done",
                                "code": ran.code,
                                "output": ran.output,
                                "truncated": ran.truncated,
                                "timed_out": ran.timed_out,
                            })
                        }
                        Err(e) => {
                            tracing::warn!(task = %spec.name, "A task could not start: {e}");
                            crate::events::emit(
                                "task_finished",
                                Some(&device),
                                json!({ "task": spec.name, "code": null, "timed_out": false }),
                                json!({}),
                            );
                            json!({ "state": "done", "code": null, "output": format!("Could not start: {e}"), "truncated": false, "timed_out": false })
                        }
                    };
                    send_to(&device, &result(&id, body)).await;
                });
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, command: &str) -> TaskSpec {
        TaskSpec {
            name: name.into(),
            command: command.into(),
            ask: true,
            deadline_secs: 60,
        }
    }

    #[test]
    fn a_phone_is_shown_names_never_commands() {
        let listed = listing(&[
            spec("Back up", "secret-backup.sh --token X"),
            spec("", "x"),
            spec("Empty", "  "),
        ]);
        let text = listed.to_string();
        assert!(text.contains("Back up"));
        assert!(
            !text.contains("secret-backup"),
            "no command line reaches the phone"
        );
        assert_eq!(
            listed["tasks"].as_array().unwrap().len(),
            1,
            "unnamed and empty tasks are left out"
        );
    }

    #[test]
    fn one_run_of_a_task_and_two_tasks_at_once() {
        // Names of their own, so other tests running alongside do not count.
        assert!(claim("t-a"));
        assert!(!claim("t-a"), "already running");
        assert!(claim("t-b"));
        assert!(!claim("t-c"), "two run already");
        release("t-a");
        assert!(claim("t-c"));
        release("t-b");
        release("t-c");
    }

    #[test]
    fn a_run_id_is_short_printable_text() {
        assert!(valid_run_id("run-1"));
        assert!(!valid_run_id(""));
        assert!(!valid_run_id(&"x".repeat(65)));
        assert!(!valid_run_id("a\nb"));
    }

    #[tokio::test]
    async fn a_run_reports_its_output_and_code_and_stops_at_its_deadline() {
        let ran = run("echo hello", Duration::from_secs(10)).await.unwrap();
        assert_eq!(ran.code, Some(0));
        assert!(ran.output.contains("hello"));
        assert!(!ran.timed_out);
        let failing = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
        assert_eq!(
            run(failing, Duration::from_secs(10)).await.unwrap().code,
            Some(3)
        );
        let slow = if cfg!(windows) {
            "ping -n 30 127.0.0.1 > NUL"
        } else {
            "sleep 30"
        };
        let started = std::time::Instant::now();
        let stopped = run(slow, Duration::from_millis(500)).await.unwrap();
        assert!(stopped.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
