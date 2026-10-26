//! Named tasks (ecosystem phase 1c): the commands the owner names in
//! config.toml, listed to a phone with the Tasks permission (never their
//! command lines) and started at its asking. A task that asks first waits
//! for the owner's answer in the desktop app (`core::local::tasks`). Each
//! run is held to its deadline and its output to its last 64 KiB, read as it
//! comes (never all of it in memory), at most 2 at once and one of each
//! task, and 10 starts a minute per phone; it is answered `running`, then
//! `done`, or `refused` with why, on the connection that asked, and logged
//! as events.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::local::home_assistant::HOME_ASSISTANT_DEVICE;
use pocketdesk_core::local::tasks::{self, TaskRequest, TaskSpec, TaskVerdict};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

pub const TASKS: &str = "kdeconnect.pocketdesk.tasks";
pub const TASK_RUN: &str = "kdeconnect.pocketdesk.task_run";
pub const TASK_RESULT: &str = "kdeconnect.pocketdesk.task_result";

/// Runs at once, at most.
const MAX_RUNNING: usize = 2;
/// Starts a phone may ask for in a minute, refused ones included.
const MAX_STARTS_PER_MINUTE: usize = 10;
/// A result not taken by its connection within this is not waited on.
const SEND_DEADLINE: Duration = Duration::from_secs(10);

/// The tasks running now (or waiting for the owner), by name.
static RUNNING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// The starts held for the owner, as `task_requests.json` lists them.
static HELD: LazyLock<Mutex<Vec<TaskRequest>>> = LazyLock::new(Default::default);
/// Each phone's recent starts, for [`MAX_STARTS_PER_MINUTE`].
static STARTS: LazyLock<Mutex<HashMap<String, VecDeque<Instant>>>> =
    LazyLock::new(Default::default);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// A phone's id for a run: short printable text, echoed back.
fn valid_run_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 64 && !id.chars().any(char::is_control)
}

/// Whether `spec` is a task a phone may see and start.
fn offered(spec: &TaskSpec) -> bool {
    tasks::name_problem(&spec.name).is_none() && !spec.command.trim().is_empty()
}

/// The list a phone is shown: names and whether each asks first.
pub(crate) fn listing(specs: &[TaskSpec]) -> Value {
    let tasks: Vec<Value> = specs
        .iter()
        .filter(|t| offered(t))
        .map(|t| json!({ "name": t.name, "ask": t.ask }))
        .collect();
    json!({ "tasks": tasks })
}

/// The names of the tasks a phone may start (Home Assistant's buttons).
pub(crate) fn offered_names(specs: &[TaskSpec]) -> Vec<String> {
    specs
        .iter()
        .filter(|t| offered(t))
        .map(|t| t.name.clone())
        .collect()
}

/// The task a phone names, if it is one it may start.
fn find<'a>(specs: &'a [TaskSpec], name: &str) -> Option<&'a TaskSpec> {
    specs.iter().find(|t| t.name == name && offered(t))
}

/// Whether `device` may ask for another start at `now`; counts it.
fn within_rate(device: &str, now: Instant) -> bool {
    let mut starts = STARTS.lock().unwrap_or_else(|e| e.into_inner());
    let times = starts.entry(device.to_string()).or_default();
    while times
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= MAX_STARTS_PER_MINUTE {
        return false;
    }
    times.push_back(now);
    true
}

fn result(id: &str, body: Value) -> NetworkPacket {
    let mut body = body;
    if let Some(map) = body.as_object_mut() {
        map.insert("id".into(), json!(id));
    }
    NetworkPacket::new(TASK_RESULT).with_body(body)
}

/// Why a phone's tasks request was refused, in the Devices page's words.
const NOT_PERMITTED: &str = "\"Start my tasks\" is turned off for this phone on the computer";

/// Answer a listing or a start the phone's permissions refused, so it is
/// told why instead of waiting on an answer that never comes (E7).
pub(crate) async fn answer_denied(packet: &NetworkPacket, sender: &dyn DeviceSender) {
    let reply = match packet.packet_type.as_str() {
        TASKS => {
            NetworkPacket::new(TASKS).with_body(json!({ "tasks": [], "error": NOT_PERMITTED }))
        }
        TASK_RUN => {
            let id = packet.body.get("id").and_then(Value::as_str).unwrap_or("");
            if !valid_run_id(id) {
                return;
            }
            // A code, as the phone's run follower reads refusals.
            refused(id, "not_permitted")
        }
        _ => return,
    };
    let _ = sender.send_packet(&reply).await;
}

fn refused(id: &str, reason: &str) -> NetworkPacket {
    result(id, json!({ "state": "refused", "reason": reason }))
}

/// A task's place among the running: given back when dropped, whatever
/// ends the run (a panic included).
struct Claim(String);

impl Drop for Claim {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Take a place for `name`: none when it runs already, or 2 tasks do.
fn claim(name: &str) -> Option<Claim> {
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    if running.len() >= MAX_RUNNING || running.contains(name) {
        return None;
    }
    running.insert(name.to_string());
    Some(Claim(name.to_string()))
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ran {
    pub code: Option<i32>,
    pub output: String,
    pub truncated: bool,
    pub timed_out: bool,
}

/// What a command wrote, kept to the last `max` bytes of each stream as it
/// comes, so a command that writes without end never fills the server's
/// memory.
pub(crate) struct Captured {
    pub status: Option<std::process::ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub cut: bool,
    pub timed_out: bool,
}

/// Run `command` to its end or `deadline`, keeping the last `max` bytes of
/// its stdout and of its stderr; on the deadline, what it wrote so far.
pub(crate) async fn run_capped(
    mut command: tokio::process::Command,
    deadline: Duration,
    max: usize,
) -> std::io::Result<Captured> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let tail = || Arc::new(Mutex::new((VecDeque::<u8>::new(), false)));
    let (out_tail, err_tail) = (tail(), tail());
    async fn keep(
        mut pipe: impl tokio::io::AsyncRead + Unpin,
        into: Arc<Mutex<(VecDeque<u8>, bool)>>,
        max: usize,
    ) {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = pipe.read(&mut buf).await {
            let mut kept = into.lock().unwrap_or_else(|e| e.into_inner());
            kept.0.extend(&buf[..n]);
            if kept.0.len() > max {
                let extra = kept.0.len() - max;
                kept.0.drain(..extra);
                kept.1 = true;
            }
        }
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let reading = async {
        let out = async {
            if let Some(pipe) = stdout {
                keep(pipe, out_tail.clone(), max).await;
            }
        };
        let err = async {
            if let Some(pipe) = stderr {
                keep(pipe, err_tail.clone(), max).await;
            }
        };
        let (_, _, status) = tokio::join!(out, err, child.wait());
        status
    };
    let (status, timed_out) = match tokio::time::timeout(deadline, reading).await {
        Ok(status) => (Some(status?), false),
        Err(_) => {
            let _ = child.start_kill();
            (None, true)
        }
    };
    let take = |t: &Arc<Mutex<(VecDeque<u8>, bool)>>| {
        let kept = t.lock().unwrap_or_else(|e| e.into_inner());
        (kept.0.iter().copied().collect::<Vec<u8>>(), kept.1)
    };
    let ((stdout, cut_out), (stderr, cut_err)) = (take(&out_tail), take(&err_tail));
    Ok(Captured {
        status,
        stdout,
        stderr,
        cut: cut_out || cut_err,
        timed_out,
    })
}

/// Run `command` through the system shell to its end or `deadline`: its
/// stdout, then its stderr, kept to their last 64 KiB together.
pub(crate) async fn run(command: &str, deadline: Duration) -> std::io::Result<Ran> {
    let captured = run_capped(shell(command), deadline, tasks::MAX_OUTPUT).await?;
    let mut both = captured.stdout;
    if !captured.stderr.is_empty() {
        if !both.is_empty() && !both.ends_with(b"\n") {
            both.push(b'\n');
        }
        both.extend_from_slice(&captured.stderr);
    }
    let (output, cut) = tasks::output_tail(&both);
    Ok(Ran {
        code: captured.status.and_then(|s| s.code()),
        output,
        truncated: cut || captured.cut,
        timed_out: captured.timed_out,
    })
}

/// The system shell, as the exec plugin's: `sh -c`, or System32's
/// `cmd.exe /S /C "line"` with no window.
pub(crate) fn shell(command: &str) -> tokio::process::Command {
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

/// Forget the held starts of an earlier run (none waits on them now), and
/// any answer left behind. Called at start.
pub fn clear_left_over() {
    if let Ok(dir) = crate::state::state_dir() {
        let _ = tasks::write_requests(&dir, &[]);
        let _ = std::fs::remove_dir_all(dir.join(pocketdesk_core::local::paths::TASK_ANSWERS_DIR));
    }
}

/// Hold `task`'s start for the owner; their answer, or a deny after a
/// minute. `asker` is who asked, as the prompt says it ("a phone").
async fn ask_owner(device_id: &str, task: &str, asker: &str) -> TaskVerdict {
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
    let summary = format!("Pocketdesk: {asker} wants to run \"{task}\"");
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

/// Send `packet` to `device_id` on the connection that asked
/// (`connection_id`) while it is up, else on its newest connection.
async fn send_to(device_id: &str, connection_id: &str, packet: &NetworkPacket) {
    // Only a connection the push filter lets through (AY1 review: a newer
    // one claiming the id with another key, kept only to pair again, would
    // else be given the command's output).
    let target: Option<Arc<dyn DeviceSender>> = {
        let clients = crate::state::ACTIVE_CLIENTS.lock().await;
        clients
            .iter()
            .find(|c| c.connection_id() == connection_id)
            .or_else(|| clients.iter().rev().find(|c| c.device_id() == device_id))
            .filter(|c| crate::state::may_receive_pushes(c.as_ref()))
            .cloned()
    };
    if let Some(client) = target {
        let _ = tokio::time::timeout(SEND_DEADLINE, client.send_packet(packet)).await;
    }
}

/// Log a refusal: the task's name only when it is one of the owner's (a
/// name a phone made up is never written).
fn log_refused(device: &str, specs: &[TaskSpec], name: &str, reason: &str) {
    let task = find(specs, name).map(|t| t.name.clone());
    crate::events::emit(
        "task_refused",
        Some(device),
        json!({ "task": task, "reason": reason }),
        json!({}),
    );
}

/// Where a run's states go: the phone connection that asked, or nowhere
/// but the event log (a start from Home Assistant, ecosystem 1e).
/// Where a task stands when it is about to run or say how it went (AY2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStanding {
    Allowed,
    /// The phone is let in but its tasks are off: it is told so.
    NotPermitted,
    /// The phone is no longer let in: it is told nothing.
    Gone,
}

enum Reply {
    Phone {
        connection: String,
        id: String,
    },
    Nowhere,
    /// A phone's deck button (F1): its results go nowhere.
    Deck,
}

impl Reply {
    /// Who asked, as the owner's prompt says it.
    fn asker(&self) -> &'static str {
        match self {
            Self::Phone { .. } => "a phone",
            Self::Nowhere => "Home Assistant",
            Self::Deck => "a phone's deck",
        }
    }

    async fn send(&self, device: &str, body: Value) {
        // Nothing to a phone no longer let in (AY2).
        if let Self::Phone { connection, id } = self
            && self.standing_now(device) != TaskStanding::Gone
        {
            send_to(device, connection, &result(id, body)).await;
        }
    }

    /// Where a task for `device` stands, the phone `let_in` (paired, or
    /// pairing not required) and `permitted` tasks now (AY2: one the owner
    /// allowed after the phone was unpaired ran, and its output reached the
    /// phone's connection, which an unpair blocks but does not close). Home
    /// Assistant's is not a phone's.
    fn standing(&self, let_in: bool, permitted: bool) -> TaskStanding {
        match self {
            Self::Nowhere => TaskStanding::Allowed,
            Self::Phone { .. } | Self::Deck if !let_in => TaskStanding::Gone,
            Self::Phone { .. } | Self::Deck if !permitted => TaskStanding::NotPermitted,
            Self::Phone { .. } | Self::Deck => TaskStanding::Allowed,
        }
    }

    /// [Self::standing] for `device` now, read only for a phone's task.
    fn standing_now(&self, device: &str) -> TaskStanding {
        match self {
            Self::Nowhere => TaskStanding::Allowed,
            _ => self.standing(
                crate::plugins::pair::is_paired_device(device) || !crate::state::pairing_required(),
                crate::plugins::pair::permits(device, TASK_RUN),
            ),
        }
    }
}

/// Start the task named `name` for Home Assistant (ecosystem 1e), as a
/// phone's start: the same limits, and one that asks first waits for the
/// owner. Why not, when it does not start.
pub fn start_for_home_assistant(name: &str) -> std::result::Result<(), &'static str> {
    start_named(
        &crate::events::current_config().tasks,
        name,
        HOME_ASSISTANT_DEVICE,
        Reply::Nowhere,
    )
}

/// Start the task named `name` from `device`'s deck (F1), as a phone's
/// start: the same limits, and one that asks first waits for the owner.
pub fn start_from_deck(device: &str, name: &str) -> std::result::Result<(), &'static str> {
    start_named(
        &crate::events::current_config().tasks,
        name,
        device,
        Reply::Deck,
    )
}

/// Start `name` among `specs` for `device`, its results to `reply`.
fn start_named(
    specs: &[TaskSpec],
    name: &str,
    device: &str,
    reply: Reply,
) -> std::result::Result<(), &'static str> {
    let device = device.to_string();
    if !within_rate(&device, Instant::now()) {
        return Err("too many starts");
    }
    let Some(spec) = find(specs, name).cloned() else {
        log_refused(&device, specs, name, "unknown");
        return Err("unknown");
    };
    let Some(held) = claim(&spec.name) else {
        log_refused(&device, specs, name, "busy");
        return Err("busy");
    };
    tokio::spawn(run_claimed(spec, device, held, reply));
    Ok(())
}

/// Run `spec`, whose place is `held`, for `device`: the owner's answer
/// first when it asks, then the command; each state to `reply`, and logged.
async fn run_claimed(spec: TaskSpec, device: String, held: Claim, reply: Reply) {
    let _held = held;
    if spec.ask && ask_owner(&device, &spec.name, reply.asker()).await == TaskVerdict::Deny {
        crate::events::emit(
            "task_refused",
            Some(&device),
            json!({ "task": spec.name, "reason": "denied" }),
            json!({}),
        );
        reply
            .send(&device, json!({ "state": "refused", "reason": "denied" }))
            .await;
        return;
    }
    // Allowed after its phone was unpaired, or its tasks turned off: not run;
    // the phone told why, if it is still let in (review).
    match reply.standing_now(&device) {
        TaskStanding::Allowed => {}
        TaskStanding::NotPermitted => {
            crate::events::emit(
                "task_refused",
                Some(&device),
                json!({ "task": spec.name, "reason": "not_permitted" }),
                json!({}),
            );
            reply
                .send(
                    &device,
                    json!({ "state": "refused", "reason": "not_permitted" }),
                )
                .await;
            return;
        }
        TaskStanding::Gone => {
            crate::events::emit(
                "task_refused",
                Some(&device),
                json!({ "task": spec.name, "reason": "unpaired" }),
                json!({}),
            );
            return;
        }
    }
    crate::events::emit(
        "task_started",
        Some(&device),
        json!({ "task": spec.name }),
        json!({}),
    );
    reply.send(&device, json!({ "state": "running" })).await;
    let body = match run(&spec.command, spec.deadline()).await {
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
            json!({
                "state": "done",
                "code": null,
                "output": format!("Could not start: {e}"),
                "truncated": false,
                "timed_out": false,
            })
        }
    };
    reply.send(&device, body).await;
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
                let device = sender.device_id().to_string();
                if !valid_run_id(id) || !within_rate(&device, Instant::now()) {
                    return Ok(());
                }
                let Some(spec) = find(&specs, name).cloned() else {
                    log_refused(&device, &specs, name, "unknown");
                    sender.send_packet(&refused(id, "unknown")).await?;
                    return Ok(());
                };
                let Some(held) = claim(&spec.name) else {
                    log_refused(&device, &specs, name, "busy");
                    sender.send_packet(&refused(id, "busy")).await?;
                    return Ok(());
                };
                let reply = Reply::Phone {
                    connection: sender.connection_id().to_string(),
                    id: id.to_string(),
                };
                tokio::spawn(run_claimed(spec, device, held, reply));
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

    /// Held by the tests that count the running tasks, which are shared.
    static RUNNING_TESTS: Mutex<()> = Mutex::new(());

    #[test]
    fn a_phone_s_task_runs_and_reports_only_while_the_phone_may_run_tasks() {
        // AY2: one the owner allowed after the phone was unpaired ran, and
        // its output reached the phone's connection.
        let phone = Reply::Phone {
            connection: "c".into(),
            id: "1".into(),
        };
        assert_eq!(phone.standing(true, true), TaskStanding::Allowed);
        assert_eq!(phone.standing(false, true), TaskStanding::Gone);
        // Let in, its tasks off: told why (review).
        assert_eq!(phone.standing(true, false), TaskStanding::NotPermitted);
        assert_eq!(Reply::Deck.standing(false, true), TaskStanding::Gone);
        assert_eq!(Reply::Deck.standing(true, true), TaskStanding::Allowed);
        // Home Assistant's is not a phone's.
        assert_eq!(Reply::Nowhere.standing(false, false), TaskStanding::Allowed);
    }

    #[test]
    fn home_assistant_starts_a_task_as_a_phone_would() {
        let _alone = RUNNING_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(home_assistant_starts());
    }

    async fn home_assistant_starts() {
        let specs = [TaskSpec {
            ask: false,
            ..spec("ha-quick", if cfg!(windows) { "exit 0" } else { "true" })
        }];
        assert_eq!(
            start_named(&specs, "not-one", HOME_ASSISTANT_DEVICE, Reply::Nowhere),
            Err("unknown")
        );
        let held = claim("ha-quick").unwrap();
        assert_eq!(
            start_named(&specs, "ha-quick", HOME_ASSISTANT_DEVICE, Reply::Nowhere),
            Err("busy"),
            "one of each at once"
        );
        drop(held);
        assert_eq!(
            start_named(&specs, "ha-quick", HOME_ASSISTANT_DEVICE, Reply::Nowhere),
            Ok(())
        );
        // Up to 10 s: a loaded CI runner starts a shell slowly.
        for _ in 0..100 {
            if !RUNNING.lock().unwrap().contains("ha-quick") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !RUNNING.lock().unwrap().contains("ha-quick"),
            "it ran and ended"
        );
        // Three starts above counted; seven more fill the minute.
        for _ in 0..7 {
            let _ = start_named(&specs, "not-one", HOME_ASSISTANT_DEVICE, Reply::Nowhere);
        }
        assert_eq!(
            start_named(&specs, "ha-quick", HOME_ASSISTANT_DEVICE, Reply::Nowhere),
            Err("too many starts")
        );
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
    fn a_task_left_out_of_the_list_cannot_be_started_by_name() {
        let specs = [
            spec("Empty", "  "),
            spec(&"x".repeat(41), "echo"),
            spec("Ok", "echo"),
        ];
        assert!(find(&specs, "Empty").is_none());
        assert!(find(&specs, &"x".repeat(41)).is_none());
        assert!(find(&specs, "Ok").is_some());
    }

    #[test]
    fn one_run_of_a_task_and_two_tasks_at_once_given_back_when_dropped() {
        let _alone = RUNNING_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // Names of their own, so other tests running alongside do not count.
        let a = claim("t-a").expect("free");
        assert!(claim("t-a").is_none(), "already running");
        let b = claim("t-b").expect("free");
        assert!(claim("t-c").is_none(), "two run already");
        drop(a);
        let c = claim("t-c").expect("a place came back");
        drop((b, c));
    }

    #[test]
    fn a_phone_may_ask_for_ten_starts_a_minute() {
        let t0 = Instant::now();
        for _ in 0..MAX_STARTS_PER_MINUTE {
            assert!(within_rate("t-rate-phone", t0));
        }
        assert!(!within_rate("t-rate-phone", t0));
        assert!(within_rate("t-rate-phone", t0 + Duration::from_secs(61)));
        assert!(within_rate("t-rate-other", t0), "each phone its own");
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
            "echo begun & ping -n 30 127.0.0.1 > NUL"
        } else {
            "echo begun; sleep 30"
        };
        let started = std::time::Instant::now();
        let stopped = run(slow, Duration::from_millis(1500)).await.unwrap();
        assert!(stopped.timed_out);
        assert!(
            stopped.output.contains("begun"),
            "what it wrote before its deadline is kept"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// A command that writes far past the cap is read as it writes: only
    /// the end is kept, never all of it.
    #[tokio::test]
    async fn only_the_end_of_a_long_output_is_kept() {
        let long = if cfg!(windows) {
            "for /L %i in (1,1,40000) do @echo line %i"
        } else {
            "i=0; while [ $i -lt 40000 ]; do echo line $i; i=$((i+1)); done"
        };
        let captured = run_capped(shell(long), Duration::from_secs(60), 1024)
            .await
            .unwrap();
        assert!(captured.cut);
        assert_eq!(captured.stdout.len(), 1024);
        let text = String::from_utf8_lossy(&captured.stdout);
        assert!(
            text.contains("40000") || text.contains("39999"),
            "the end: {text}"
        );
    }

    struct Phone(std::sync::Mutex<Vec<NetworkPacket>>);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            "phone-a"
        }
        fn connection_id(&self) -> &str {
            "phone-a"
        }
        async fn send_packet(&self, packet: &NetworkPacket) -> Result<()> {
            self.0.lock().unwrap().push(packet.clone());
            Ok(())
        }
    }

    /// A phone with "Start my tasks" off is told so at once (E7): it had
    /// waited for a listing that never came, then said the computer did
    /// not answer; and a start it sends anyway is refused by its id.
    #[tokio::test]
    async fn a_refused_tasks_request_is_answered_with_why() {
        let phone = Phone(Default::default());
        crate::plugins::answer_denied(&NetworkPacket::new(TASKS), &phone).await;
        let run = NetworkPacket::new(TASK_RUN).with_body(json!({"name": "build", "id": "t-run-9"}));
        crate::plugins::answer_denied(&run, &phone).await;
        let sent = phone.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].packet_type, TASKS);
        assert_eq!(sent[0].body["tasks"], json!([]));
        assert!(
            sent[0].body["error"]
                .as_str()
                .unwrap()
                .contains("Start my tasks")
        );
        assert_eq!(sent[1].packet_type, TASK_RESULT);
        assert_eq!(sent[1].body["id"], "t-run-9");
        assert_eq!(sent[1].body["state"], "refused");
        assert_eq!(sent[1].body["reason"], "not_permitted");
        // A start with no id gets nothing back.
        let bad = NetworkPacket::new(TASK_RUN).with_body(json!({"name": "build"}));
        crate::plugins::answer_denied(&bad, &phone).await;
        assert_eq!(phone.0.lock().unwrap().len(), 2);
    }
}
