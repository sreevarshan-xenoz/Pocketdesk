//! The local API (roadmap T7): what this program reports and does for the
//! person's own scripts, as JSON. There is no listener: it is this program,
//! run as the user, over the state folder, which is already the user's own.
//! `docs/local-api.md` is the contract.

use std::path::Path;

use pocketdesk_core::local::device_keys;
use pocketdesk_core::local::paths::{LIVE_FILE, TRUST_FILE};
use pocketdesk_core::local::sessions::{self, MirrorSession};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;
use serde_json::{Value, json};

/// The API's version, in every object it prints. Within one version fields
/// are only ever added.
pub const API: u64 = 1;

/// `value` (an object) with the API's version in it.
pub fn versioned(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("api".to_string(), json!(API));
    }
    value
}

/// The name a capability goes by in the API.
pub fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::Input => "input",
        Capability::Clipboard => "clipboard",
        Capability::Files => "files",
        Capability::Audio => "audio",
        Capability::Mic => "mic",
        Capability::Power => "power",
        Capability::Exec => "exec",
        Capability::Notifications => "notifications",
        Capability::Texts => "texts",
        Capability::Links => "links",
        Capability::Media => "media",
    }
}

/// The live sessions the running server reports, if it runs and its report
/// is fresh.
pub fn live_sessions(state_dir: &Path, now_unix: u64, running: bool) -> Vec<MirrorSession> {
    std::fs::read_to_string(state_dir.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .filter(|m| running && m.is_fresh(now_unix))
        .map(|m| m.sessions)
        .unwrap_or_default()
}

/// The paired phones: what each may do, whether a key is bound to it, and
/// whether it is in a session now.
pub fn devices(state_dir: &Path, now_unix: u64, running: bool) -> Result<Value, String> {
    let live = live_sessions(state_dir, now_unix, running);
    let keys = device_keys::read(state_dir).unwrap_or_default();
    let batteries = pocketdesk_core::local::phone_status::read(state_dir);
    let store = trust_store(state_dir)?;
    let devices: Vec<Value> = store
        .trusted_devices()
        .into_iter()
        .map(|id| {
            let permissions = store.permissions(&id);
            let allowed: serde_json::Map<String, Value> = Capability::ALL
                .iter()
                .map(|c| {
                    (
                        capability_name(*c).to_string(),
                        json!(permissions.allows(*c)),
                    )
                })
                .collect();
            let bound = keys.get(&id);
            json!({
                "id": id,
                "grant_remaining_secs": store.grant_remaining(&id),
                "in_session": live.iter().any(|s| s.device_id.as_deref() == Some(id.as_str())),
                "key_bound": bound.is_some_and(|k| k.tls.is_some()),
                "internet_key_bound": bound.is_some_and(|k| k.iroh.is_some()),
                "permissions": allowed,
                // The phone's last battery report (T8b), or null.
                "battery": batteries.get(&id),
            })
        })
        .collect();
    Ok(versioned(
        json!({ "server_running": running, "devices": devices }),
    ))
}

/// The paired phones' store, read without writing it.
fn trust_store(state_dir: &Path) -> Result<TrustStore, String> {
    TrustStore::read(state_dir.join(TRUST_FILE))
        .map_err(|e| format!("the paired phones could not be read: {e:#}"))
}

/// Whether the server runs, the phones in a session now, and how phones
/// reach this computer from outside (`endpoint.json`, when the server
/// keeps one).
pub fn status(state_dir: &Path, now_unix: u64, running: bool) -> Value {
    let pid = running
        .then(|| pocketdesk_core::local::server::server_pid(state_dir))
        .flatten();
    let sessions: Vec<Value> = live_sessions(state_dir, now_unix, running)
        .into_iter()
        .map(|s| {
            json!({
                "session_id": s.id,
                "device_id": s.device_id,
                "peer": s.peer,
                "transport": s.transport,
                "started_unix": s.started_unix,
                "driving": s.driving,
            })
        })
        .collect();
    let endpoint = running
        .then(|| pocketdesk_core::local::identity::read_endpoint(state_dir))
        .flatten();
    versioned(json!({
        "server": {
            "running": running,
            "pid": pid,
        },
        // This program's: the running server may be an older one.
        "pocketdesk_version": env!("CARGO_PKG_VERSION"),
        "sessions": sessions,
        "endpoint": endpoint,
    }))
}

/// `pocketdesk status --json`.
pub fn print_status() -> anyhow::Result<()> {
    let dir = crate::state::state_dir()?;
    let running = pocketdesk_core::local::server::is_running(&dir);
    print(&status(
        &dir,
        pocketdesk_core::local::pin::now_unix(),
        running,
    ));
    Ok(())
}

/// The last `count` files sent to phones, newest first, and how each went.
pub fn sent_list(state_dir: &Path, count: usize) -> Value {
    use pocketdesk_core::local::sent::{self, SentState};
    let rows: Vec<Value> = sent::read(state_dir)
        .into_iter()
        .rev()
        .take(count)
        .map(|file| {
            let (state, reason) = match &file.state {
                SentState::Offered => ("offered", None),
                SentState::Sent => ("sent", None),
                SentState::Failed(reason) => ("failed", Some(reason.clone())),
            };
            json!({
                "id": file.id,
                "name": file.name,
                "path": file.path,
                "size": file.size,
                "device_id": file.device_id,
                "sent_unix": file.sent_unix,
                "state": state,
                "reason": reason,
                "note": file.note,
            })
        })
        .collect();
    versioned(json!({ "sent": rows }))
}

/// The last `count` files received from phones, newest first.
pub fn received_list(state_dir: &Path, count: usize) -> Value {
    let rows: Vec<Value> = pocketdesk_core::local::received::read(state_dir)
        .into_iter()
        .rev()
        .take(count)
        .map(|file| {
            json!({
                "name": file.name,
                "still_there": Path::new(&file.path).exists(),
                "path": file.path,
                "size": file.size,
                "received_unix": file.received_unix,
                "from": file.from,
            })
        })
        .collect();
    versioned(json!({ "received": rows }))
}

/// `pocketdesk sent` and `pocketdesk received`.
pub fn print_files(sent: bool, count: usize, as_json: bool) -> anyhow::Result<()> {
    let dir = crate::state::state_dir()?;
    let listed = if sent {
        sent_list(&dir, count)
    } else {
        received_list(&dir, count)
    };
    if as_json {
        print(&listed);
        return Ok(());
    }
    let key = if sent { "sent" } else { "received" };
    let rows = listed[key].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("Nothing {key} yet");
    }
    for row in rows {
        let name = row["name"].as_str().unwrap_or_default();
        if sent {
            let how = match row["state"].as_str() {
                Some("failed") => format!("not sent: {}", row["reason"].as_str().unwrap_or("")),
                Some("sent") => "saved on the phone".to_string(),
                _ => "waiting for the phone".to_string(),
            };
            let note = row["note"]
                .as_str()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
            println!("{name}  {how}{note}");
        } else {
            let gone = if row["still_there"] == true {
                ""
            } else {
                "  (moved or deleted)"
            };
            println!("{name}  {}{gone}", row["path"].as_str().unwrap_or_default());
        }
    }
    Ok(())
}

/// The exit codes the local API's commands end with (`docs/local-api.md`).
pub mod exit {
    pub const DONE: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const BAD_ARGUMENTS: i32 = 2;
    pub const NO_SERVER: i32 = 3;
    pub const STILL_WAITING: i32 = 4;
}

/// Ask the server in `state_dir` to send `path` to the phone `query`
/// names, as the desktop app's Files tab does (an outbox request), and,
/// with `wait`, wait up to that long for the sent list to say how it went.
/// What to print, and the exit code.
pub fn send_in(
    state_dir: &Path,
    query: &str,
    path: &Path,
    running: &dyn Fn() -> bool,
    wait: Option<std::time::Duration>,
) -> (Value, i32) {
    // A refusal is a failure like any other (`error`), and a send's state
    // (`state`, `reason`) too.
    let refused = |code: i32, reason: String| {
        (
            versioned(json!({ "state": "refused", "reason": reason, "error": reason })),
            code,
        )
    };
    if !running() {
        return refused(
            exit::NO_SERVER,
            "no Pocketdesk server is running to send it".to_string(),
        );
    }
    let ids = match trust_store(state_dir) {
        Ok(store) => store.trusted_devices(),
        Err(reason) => return refused(exit::FAILED, reason),
    };
    let device_id = match resolve_phone(&ids, query) {
        Ok(id) => id,
        Err(reason) => return refused(exit::BAD_ARGUMENTS, reason),
    };
    // An offer reaches only a phone in a session: say so now, not after
    // queueing a send the server can only fail.
    let now = pocketdesk_core::local::pin::now_unix();
    if !live_sessions(state_dir, now, true)
        .iter()
        .any(|s| s.device_id.as_deref() == Some(device_id.as_str()))
    {
        return refused(
            exit::FAILED,
            format!("{device_id} is not in a session: open one on the phone first"),
        );
    }
    let Ok(full) = std::path::absolute(path) else {
        return refused(
            exit::BAD_ARGUMENTS,
            format!("{} cannot be named", path.display()),
        );
    };
    if !full.exists() {
        return refused(
            exit::BAD_ARGUMENTS,
            format!("{} is not there", full.display()),
        );
    }
    let request = match pocketdesk_core::local::outbox::request(
        state_dir,
        &device_id,
        &full.to_string_lossy(),
        now,
    ) {
        Ok(request) => request,
        Err(e) => return refused(exit::FAILED, format!("could not queue it: {e}")),
    };
    let mut out = json!({
        "id": request.id,
        "device_id": device_id,
        "path": request.path,
        "state": "queued",
        "reason": null,
        "note": null,
    });
    let Some(wait) = wait else {
        return (versioned(out), exit::DONE);
    };
    let deadline = wait_deadline(wait);
    loop {
        let row = sent_list(state_dir, usize::MAX)["sent"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["id"] == out["id"]).cloned());
        // Once the server has it, its row says where it is.
        if let Some(row) = &row {
            for key in ["state", "reason", "note"] {
                out[key] = row[key].clone();
            }
            if row["state"] != "offered" {
                let code = if row["state"] == "sent" {
                    exit::DONE
                } else {
                    exit::FAILED
                };
                return (versioned(out), code);
            }
        }
        if !running() {
            out["reason"] = json!("the Pocketdesk server stopped while waiting");
            return (versioned(out), exit::NO_SERVER);
        }
        if std::time::Instant::now() >= deadline {
            return (versioned(out), exit::STILL_WAITING);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// When a wait of `wait` from now ends; a wait too long to count ends in a
/// year, which no phone outlasts.
fn wait_deadline(wait: std::time::Duration) -> std::time::Instant {
    let now = std::time::Instant::now();
    now.checked_add(wait)
        .unwrap_or_else(|| now + std::time::Duration::from_secs(365 * 24 * 3600))
}

/// Ask the running server in `state_dir` to hand the link `url` to the
/// phone `query` names (T8a): a phone in a session, a link that may be
/// opened. What to print, and the exit code.
pub fn link_in(state_dir: &Path, query: &str, url: &str, running: bool) -> (Value, i32) {
    if !running {
        return (
            error("no Pocketdesk server is running to send it"),
            exit::NO_SERVER,
        );
    }
    if let Some(problem) = pocketdesk_core::protocol::link::link_problem(url) {
        return (error(problem), exit::BAD_ARGUMENTS);
    }
    let store = match trust_store(state_dir) {
        Ok(store) => store,
        Err(reason) => return (error(reason), exit::FAILED),
    };
    let device_id = match resolve_phone(&store.trusted_devices(), query) {
        Ok(id) => id,
        Err(reason) => return (error(reason), exit::BAD_ARGUMENTS),
    };
    // The server would drop it: say so now, not "queued".
    if !store.permissions(&device_id).allows(Capability::Links) {
        return (
            error(format!("Links is off for {device_id} (the Devices tab)")),
            exit::FAILED,
        );
    }
    let now = pocketdesk_core::local::pin::now_unix();
    if !live_sessions(state_dir, now, true)
        .iter()
        .any(|s| s.device_id.as_deref() == Some(device_id.as_str()))
    {
        return (
            error(format!(
                "{device_id} is not in a session: open one on the phone first"
            )),
            exit::FAILED,
        );
    }
    match pocketdesk_core::local::links::request(state_dir, &device_id, url, now) {
        Ok(request) => (
            versioned(json!({
                "id": request.id,
                "device_id": device_id,
                "url": request.url,
                "state": "queued",
            })),
            exit::DONE,
        ),
        Err(e) => (error(format!("could not queue it: {e}")), exit::FAILED),
    }
}

/// `pocketdesk link`: prints the outcome and returns the exit code.
pub fn run_link(query: &str, url: &str, as_json: bool) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            if as_json {
                print(&error(format!("no state folder: {e:#}")));
            } else {
                eprintln!("No state folder: {e:#}");
            }
            return exit::FAILED;
        }
    };
    let running = pocketdesk_core::local::server::is_running(&dir);
    let (out, code) = link_in(&dir, query, url, running);
    if as_json {
        print(&out);
    } else if let Some(reason) = out["error"].as_str() {
        eprintln!("Error: {reason}");
    } else {
        println!("sent to the phone, where it waits as a notification");
    }
    code
}

/// `pocketdesk send`: prints the outcome and returns the exit code.
pub fn run_send(query: &str, path: &Path, wait_secs: Option<u64>, as_json: bool) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            if as_json {
                print(&error(format!("no state folder: {e:#}")));
            } else {
                eprintln!("No state folder: {e:#}");
            }
            return exit::FAILED;
        }
    };
    let running = || pocketdesk_core::local::server::is_running(&dir);
    let (out, code) = send_in(
        &dir,
        query,
        path,
        &running,
        wait_secs.map(std::time::Duration::from_secs),
    );
    if as_json {
        print(&out);
    } else {
        let how = match out["state"].as_str() {
            Some("queued") => "queued for the phone".to_string(),
            Some("offered") => "offered, the phone has not taken it yet".to_string(),
            Some("sent") => "saved on the phone".to_string(),
            Some(state) => format!("{state}: {}", out["reason"].as_str().unwrap_or("")),
            None => "unknown".to_string(),
        };
        let note = out["note"]
            .as_str()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default();
        // A send that did not go is an error: on stderr, as errors are.
        if matches!(code, exit::DONE | exit::STILL_WAITING) {
            println!("{how}{note}");
        } else {
            eprintln!("{how}{note}");
        }
    }
    code
}

/// The paired phone `query` names: its id, or a unique prefix of at least
/// six characters of it.
pub fn resolve_phone(ids: &[String], query: &str) -> Result<String, String> {
    let query = query.trim();
    if let Some(id) = ids.iter().find(|id| *id == query) {
        return Ok(id.clone());
    }
    if query.chars().count() < 6 {
        return Err(format!(
            "no paired phone is {query:?} (give its id, or 6 or more of its first characters)"
        ));
    }
    let matches: Vec<&String> = ids.iter().filter(|id| id.starts_with(query)).collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no paired phone's id starts with {query:?}")),
        _ => Err(format!(
            "{} paired phones' ids start with {query:?}: give more of it",
            matches.len()
        )),
    }
}

/// `pocketdesk devices`.
pub fn print_devices(as_json: bool) -> anyhow::Result<()> {
    let dir = crate::state::state_dir()?;
    let running = pocketdesk_core::local::server::is_running(&dir);
    let listed = devices(&dir, pocketdesk_core::local::pin::now_unix(), running)
        .map_err(anyhow::Error::msg)?;
    if as_json {
        print(&listed);
        return Ok(());
    }
    let all = listed["devices"].as_array().cloned().unwrap_or_default();
    if all.is_empty() {
        println!("No paired phones");
    }
    for device in all {
        let off: Vec<&str> = device["permissions"]
            .as_object()
            .map(|p| {
                p.iter()
                    .filter(|(_, allowed)| *allowed == false)
                    .map(|(name, _)| name.as_str())
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "{}  {}{}  not allowed: {}",
            device["id"].as_str().unwrap_or_default(),
            if device["in_session"] == true {
                "in a session"
            } else {
                "not connected"
            },
            device["grant_remaining_secs"]
                .as_u64()
                .map(|s| format!(", paired for {s} s more"))
                .unwrap_or_default(),
            if off.is_empty() {
                "nothing".to_string()
            } else {
                off.join(", ")
            },
        );
    }
    Ok(())
}

/// A failure, as a command run with `--json` prints it (exit code 1, or the
/// command's own).
pub fn error(reason: impl std::fmt::Display) -> Value {
    versioned(json!({ "error": reason.to_string() }))
}

/// A usage error (a missing argument, a bad number) as the local API's
/// JSON, when `--json` was asked for; `None` for clap's own text (no
/// `--json`, or help and version).
pub fn usage_error(args: &[std::ffi::OsString], parsed: &clap::Error) -> Option<Value> {
    use clap::error::ErrorKind;
    if !args.iter().any(|a| a == "--json") {
        return None;
    }
    if matches!(
        parsed.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return None;
    }
    let text = parsed.render().to_string();
    // The error's first paragraph (clap names missing arguments on the
    // lines under its first), as one line.
    let first: Vec<&str> = text
        .lines()
        .skip_while(|l| l.trim().is_empty())
        .take_while(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect();
    let first = first.join(" ");
    let first = first.trim_start_matches("error: ");
    Some(error(first))
}

/// Ask the running server in `state_dir` to end the sessions `target`
/// names (a device id, a prefix, a peer address, or "all"). What to print,
/// and the exit code.
pub fn kick_in(state_dir: &Path, target: &str, running: bool) -> (Value, i32) {
    if !running {
        return (
            error("no running Pocketdesk server to end sessions through"),
            exit::NO_SERVER,
        );
    }
    let target = target.trim();
    if target.is_empty() {
        return (error("no session named"), exit::BAD_ARGUMENTS);
    }
    match pocketdesk_core::local::sessions::request_kick(
        state_dir,
        target,
        pocketdesk_core::local::pin::now_unix(),
    ) {
        Ok(()) => (versioned(json!({ "kick_requested": target })), exit::DONE),
        Err(e) => (
            error(format!("could not ask the server: {e}")),
            exit::FAILED,
        ),
    }
}

/// `pocketdesk kick`: prints the outcome and returns the exit code.
pub fn run_kick(target: &str, as_json: bool) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            if as_json {
                print(&error(format!("no state folder: {e:#}")));
            } else {
                eprintln!("No state folder: {e:#}");
            }
            return exit::FAILED;
        }
    };
    let running = pocketdesk_core::local::server::is_running(&dir);
    let (out, code) = kick_in(&dir, target, running);
    if as_json {
        print(&out);
    } else if let Some(reason) = out["error"].as_str() {
        eprintln!("Error: {reason}");
    } else {
        println!(
            "Kick request sent for '{}' — matching sessions end within ~1 s \
             (verify with: pocketdesk status)",
            target.trim()
        );
    }
    code
}

/// Print `value` as one line of JSON on stdout. A reader that closed the
/// pipe early (`| head`) is not an error worth a panic.
pub fn print(value: &Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-local-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn devices_say_what_each_phone_may_do_and_whether_it_is_here() {
        let dir = temp_dir("devices");
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("phone-a").unwrap();
        store
            .trust_device_with_ttl("phone-b", std::time::Duration::from_secs(60))
            .unwrap();
        device_keys::register(&dir, "phone-a", device_keys::Slot::Tls, "k").unwrap();
        std::fs::write(
            dir.join(LIVE_FILE),
            r#"{"updated_unix":100,"sessions":[{"device_id":"phone-a","peer":"192.0.2.7:1","transport":"lan","started_unix":90}]}"#,
        )
        .unwrap();
        let mut batteries = std::collections::BTreeMap::new();
        batteries.insert(
            "phone-a".to_string(),
            pocketdesk_core::local::phone_status::PhoneBattery {
                charge: 54,
                charging: true,
                updated_unix: 95,
            },
        );
        pocketdesk_core::local::phone_status::write(&dir, &batteries).unwrap();
        let listed = devices(&dir, 101, true).unwrap();
        assert_eq!(listed["api"], API);
        assert_eq!(listed["server_running"], true);
        let all = listed["devices"].as_array().unwrap();
        let a = all.iter().find(|d| d["id"] == "phone-a").unwrap();
        let b = all.iter().find(|d| d["id"] == "phone-b").unwrap();
        assert_eq!(a["in_session"], true);
        assert_eq!(a["key_bound"], true);
        assert_eq!(a["grant_remaining_secs"], Value::Null);
        assert_eq!(a["permissions"]["exec"], false, "off unless granted");
        assert_eq!(a["permissions"]["files"], true);
        assert_eq!(
            a["battery"],
            json!({"charge": 54, "charging": true, "updated_unix": 95})
        );
        assert_eq!(b["battery"], Value::Null, "never reported");
        assert_eq!(b["in_session"], false);
        assert!(b["grant_remaining_secs"].as_u64().is_some_and(|s| s <= 60));
        // A server that is not running has no sessions, whatever its file says.
        let stopped = devices(&dir, 101, false).unwrap();
        assert!(
            stopped["devices"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["in_session"] == false)
        );
    }

    #[test]
    fn sent_and_received_are_listed_newest_first_with_how_each_went() {
        use pocketdesk_core::local::received::{self, ReceivedFile};
        use pocketdesk_core::local::sent::{self, SentFile, SentState};
        let dir = temp_dir("lists");
        for (n, state) in [SentState::Sent, SentState::Failed("no room".to_string())]
            .into_iter()
            .enumerate()
        {
            sent::record(
                &dir,
                SentFile {
                    id: format!("s{n}"),
                    name: format!("f{n}.pdf"),
                    path: format!("/f{n}.pdf"),
                    size: 1,
                    device_id: "phone-a".to_string(),
                    sent_unix: n as u64,
                    state,
                    note: (n == 1).then(|| "1 left out".to_string()),
                },
            )
            .unwrap();
        }
        let listed = sent_list(&dir, 10);
        assert_eq!(listed["api"], API);
        let rows = listed["sent"].as_array().unwrap();
        assert_eq!(rows[0]["id"], "s1", "newest first");
        assert_eq!(rows[0]["state"], "failed");
        assert_eq!(rows[0]["reason"], "no room");
        assert_eq!(rows[0]["note"], "1 left out");
        assert_eq!(rows[1]["state"], "sent");
        assert_eq!(rows[1]["reason"], Value::Null);
        assert_eq!(sent_list(&dir, 1)["sent"].as_array().unwrap().len(), 1);

        let saved = dir.join("photo.jpg");
        std::fs::write(&saved, b"x").unwrap();
        received::record(
            &dir,
            ReceivedFile {
                name: "photo.jpg".to_string(),
                path: saved.display().to_string(),
                size: 1,
                received_unix: 5,
                from: "phone-a".to_string(),
            },
        )
        .unwrap();
        let got = received_list(&dir, 10);
        assert_eq!(got["received"][0]["name"], "photo.jpg");
        assert_eq!(got["received"][0]["still_there"], true);
    }

    #[test]
    fn status_says_whether_the_server_runs_and_who_is_connected() {
        let dir = temp_dir("status");
        let stopped = status(&dir, 101, false);
        assert_eq!(stopped["api"], API);
        assert_eq!(stopped["server"]["running"], false);
        // This program's version: a running server may be an older one.
        assert_eq!(stopped["pocketdesk_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(stopped["server"].get("version"), None);
        assert_eq!(stopped["sessions"], json!([]));
        assert_eq!(stopped["endpoint"], Value::Null);
        std::fs::write(
            dir.join(LIVE_FILE),
            r#"{"updated_unix":100,"sessions":[{"id":7,"device_id":"phone-a","peer":"192.0.2.7:1","transport":"wan","started_unix":90,"driving":true}]}"#,
        )
        .unwrap();
        let running = status(&dir, 101, true);
        let session = &running["sessions"][0];
        assert_eq!(session["device_id"], "phone-a");
        assert_eq!(session["transport"], "wan");
        assert_eq!(session["driving"], true);
    }

    fn send_setup(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = temp_dir(name);
        TrustStore::load_or_create(dir.join(TRUST_FILE))
            .unwrap()
            .trust_device("5f1c9b3d-6e70")
            .unwrap();
        let file = dir.join("report.pdf");
        std::fs::write(&file, b"r").unwrap();
        in_session(&dir, &["5f1c9b3d-6e70"]);
        (dir, file)
    }

    /// The running server's report: `phones` in a session now.
    fn in_session(dir: &Path, phones: &[&str]) {
        let sessions: Vec<Value> = phones
            .iter()
            .map(|p| json!({ "device_id": p, "peer": "192.0.2.7:1", "transport": "lan", "started_unix": 1 }))
            .collect();
        std::fs::write(
            dir.join(LIVE_FILE),
            json!({ "updated_unix": pocketdesk_core::local::pin::now_unix(), "sessions": sessions }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_send_to_a_phone_not_in_a_session_is_refused_at_once() {
        let (dir, file) = send_setup("send-away");
        in_session(&dir, &[]);
        let (out, code) = send_in(&dir, "5f1c9b", &file, &|| true, None);
        assert_eq!(code, exit::FAILED, "{out}");
        assert!(out["error"].as_str().unwrap().contains("session"));
        assert!(
            pocketdesk_core::local::outbox::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .is_empty()
        );
    }

    #[test]
    fn a_send_is_refused_before_it_is_queued_when_it_cannot_go() {
        let (dir, file) = send_setup("send-refused");
        let (out, code) = send_in(&dir, "5f1c9b", &file, &|| false, None);
        assert_eq!(code, exit::NO_SERVER, "{out}");
        let (out, code) = send_in(&dir, "ffffff", &file, &|| true, None);
        assert_eq!(code, exit::BAD_ARGUMENTS, "{out}");
        assert_eq!(out["state"], "refused");
        assert_eq!(
            out["error"], out["reason"],
            "a failure is found where every failure is"
        );
        let (out, code) = send_in(&dir, "5f1c9b", &dir.join("missing.pdf"), &|| true, None);
        assert_eq!(code, exit::BAD_ARGUMENTS, "{out}");
        assert!(
            pocketdesk_core::local::outbox::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .is_empty(),
            "nothing queued"
        );
    }

    #[test]
    fn a_send_is_queued_as_the_desktop_app_queues_one() {
        let (dir, file) = send_setup("send-queued");
        let (out, code) = send_in(&dir, "5f1c9b", &file, &|| true, None);
        assert_eq!(code, exit::DONE);
        assert_eq!(out["api"], API);
        assert_eq!(out["state"], "queued");
        assert_eq!(out["device_id"], "5f1c9b3d-6e70");
        let queued =
            pocketdesk_core::local::outbox::take_all(&dir, pocketdesk_core::local::pin::now_unix());
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, out["id"].as_str().unwrap());
        assert_eq!(queued[0].device_id, "5f1c9b3d-6e70");
    }

    #[test]
    fn a_send_waited_for_ends_as_the_sent_list_says() {
        use pocketdesk_core::local::sent::{self, SentFile, SentState};
        let (dir, file) = send_setup("send-wait");
        // The server's part, played here: take the request, then record it failed.
        let server_dir = dir.clone();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let taken = pocketdesk_core::local::outbox::take_all(
                    &server_dir,
                    pocketdesk_core::local::pin::now_unix(),
                );
                if let Some(request) = taken.into_iter().next() {
                    sent::record(
                        &server_dir,
                        SentFile {
                            id: request.id,
                            name: "report.pdf".to_string(),
                            path: request.path,
                            size: 1,
                            device_id: request.device_id,
                            sent_unix: 1,
                            state: SentState::Failed("phone not connected".to_string()),
                            note: None,
                        },
                    )
                    .unwrap();
                    return;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let (out, code) = send_in(
            &dir,
            "5f1c9b",
            &file,
            &|| true,
            Some(std::time::Duration::from_secs(10)),
        );
        server.join().unwrap();
        assert_eq!(code, exit::FAILED, "{out}");
        assert_eq!(out["state"], "failed");
        assert_eq!(out["reason"], "phone not connected");

        // Nobody takes this one: the wait runs out.
        let (out, code) = send_in(
            &dir,
            "5f1c9b",
            &file,
            &|| true,
            Some(std::time::Duration::from_millis(300)),
        );
        assert_eq!(code, exit::STILL_WAITING, "{out}");
    }

    #[test]
    fn a_failure_in_json_is_an_object_a_script_can_read() {
        let failed = error("no running Pocketdesk server");
        assert_eq!(failed["api"], API);
        assert_eq!(failed["error"], "no running Pocketdesk server");
    }

    #[test]
    fn a_kick_needs_a_running_server_and_says_so() {
        let dir = temp_dir("kick");
        let (out, code) = kick_in(&dir, "all", false);
        assert_eq!(code, exit::NO_SERVER);
        assert!(out["error"].as_str().unwrap().contains("server"));
        let (out, code) = kick_in(&dir, "  ", true);
        assert_eq!(code, exit::BAD_ARGUMENTS, "{out}");
        let (out, code) = kick_in(&dir, "all", true);
        assert_eq!(code, exit::DONE, "{out}");
        assert_eq!(out["kick_requested"], "all");
    }

    #[test]
    fn an_unreadable_trust_store_is_an_error_and_a_missing_one_is_not_made() {
        let dir = temp_dir("trust-unreadable");
        assert_eq!(devices(&dir, 1, false).unwrap()["devices"], json!([]));
        assert!(!dir.join(TRUST_FILE).exists(), "reading wrote nothing");
        std::fs::write(dir.join(TRUST_FILE), "{not json").unwrap();
        assert!(devices(&dir, 1, false).is_err(), "not 'no paired phones'");
        let file = dir.join("a.txt");
        std::fs::write(&file, b"a").unwrap();
        let (out, code) = send_in(&dir, "abcdef", &file, &|| true, None);
        assert_eq!(code, exit::FAILED, "{out}");
    }

    /// Play the server: take the next request and record it as `state`.
    fn serve_one(dir: std::path::PathBuf, state: pocketdesk_core::local::sent::SentState) {
        use pocketdesk_core::local::sent::{self, SentFile};
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                let taken = pocketdesk_core::local::outbox::take_all(
                    &dir,
                    pocketdesk_core::local::pin::now_unix(),
                );
                if let Some(request) = taken.into_iter().next() {
                    sent::record(
                        &dir,
                        SentFile {
                            id: request.id,
                            name: "report.pdf".to_string(),
                            path: request.path,
                            size: 1,
                            device_id: request.device_id,
                            sent_unix: 1,
                            state,
                            note: None,
                        },
                    )
                    .unwrap();
                    return;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
    }

    #[test]
    fn a_wait_that_runs_out_says_the_phone_was_offered_it() {
        let (dir, file) = send_setup("send-offered");
        serve_one(
            dir.clone(),
            pocketdesk_core::local::sent::SentState::Offered,
        );
        let (out, code) = send_in(
            &dir,
            "5f1c9b",
            &file,
            &|| true,
            Some(std::time::Duration::from_millis(1500)),
        );
        assert_eq!(code, exit::STILL_WAITING, "{out}");
        assert_eq!(out["state"], "offered", "not 'queued': the server took it");
    }

    #[test]
    fn a_wait_ends_when_the_server_stops() {
        let (dir, file) = send_setup("send-stopped");
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let running = || checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2;
        let (out, code) = send_in(
            &dir,
            "5f1c9b",
            &file,
            &running,
            Some(std::time::Duration::from_secs(30)),
        );
        assert_eq!(code, exit::NO_SERVER, "{out}");
        assert!(out["reason"].as_str().unwrap().contains("stopped"));
    }

    #[test]
    fn a_huge_wait_does_not_overflow() {
        assert!(wait_deadline(std::time::Duration::MAX) > std::time::Instant::now());
    }

    #[test]
    fn a_usage_error_with_json_asked_for_is_json() {
        use clap::Parser;
        let args = ["pocketdesk", "send", "--json"];
        let err = crate::cli::Cli::try_parse_from(args).unwrap_err();
        let out = usage_error(&args.map(std::ffi::OsString::from), &err).expect("JSON");
        assert_eq!(out["api"], API);
        assert!(!out["error"].as_str().unwrap().is_empty());
        // Without --json, and for help, clap's own text stays.
        let plain = ["pocketdesk", "send"];
        let err = crate::cli::Cli::try_parse_from(plain).unwrap_err();
        assert!(usage_error(&plain.map(std::ffi::OsString::from), &err).is_none());
        let help = ["pocketdesk", "send", "--help", "--json"];
        let err = crate::cli::Cli::try_parse_from(help).unwrap_err();
        assert!(usage_error(&help.map(std::ffi::OsString::from), &err).is_none());
    }

    #[test]
    fn a_link_for_a_phone_with_links_off_is_refused_not_queued() {
        let (dir, _) = send_setup("link-off");
        let mut store = TrustStore::read(dir.join(TRUST_FILE)).unwrap();
        let mut permissions = store.permissions("5f1c9b3d-6e70");
        permissions.links = Some(false);
        store.set_permissions("5f1c9b3d-6e70", permissions).unwrap();
        let (out, code) = link_in(&dir, "5f1c9b", "https://example.org/a", true);
        assert_eq!(code, exit::FAILED, "{out}");
        assert!(out["error"].as_str().unwrap().contains("Links"), "{out}");
        assert!(
            pocketdesk_core::local::links::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .is_empty()
        );
    }

    #[test]
    fn a_link_is_queued_for_a_phone_in_a_session_and_refused_otherwise() {
        let (dir, _) = send_setup("link");
        let (out, code) = link_in(&dir, "5f1c9b", "https://example.org/a", true);
        assert_eq!(code, exit::DONE, "{out}");
        assert_eq!(out["api"], API);
        assert_eq!(out["state"], "queued");
        let queued =
            pocketdesk_core::local::links::take_all(&dir, pocketdesk_core::local::pin::now_unix());
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].device_id, "5f1c9b3d-6e70");

        let (out, code) = link_in(&dir, "5f1c9b", "javascript:alert(1)", true);
        assert_eq!(code, exit::BAD_ARGUMENTS, "{out}");
        assert!(out["error"].as_str().unwrap().contains("web link"));
        let (_, code) = link_in(&dir, "5f1c9b", "https://example.org/", false);
        assert_eq!(code, exit::NO_SERVER);
        in_session(&dir, &[]);
        let (_, code) = link_in(&dir, "5f1c9b", "https://example.org/", true);
        assert_eq!(code, exit::FAILED, "not in a session");
        assert!(
            pocketdesk_core::local::links::take_all(&dir, pocketdesk_core::local::pin::now_unix())
                .is_empty()
        );
    }

    #[test]
    fn a_phone_is_named_by_its_id_or_a_unique_prefix() {
        let ids = vec![
            "0d7c3a52-91e4".to_string(),
            "0d7c3a99-1111".to_string(),
            "5f1c9b3d-6e70".to_string(),
        ];
        assert_eq!(
            resolve_phone(&ids, "5f1c9b3d-6e70").unwrap(),
            "5f1c9b3d-6e70"
        );
        assert_eq!(resolve_phone(&ids, "5f1c9b").unwrap(), "5f1c9b3d-6e70");
        assert!(
            resolve_phone(&ids, "0d7c3a")
                .unwrap_err()
                .contains("2 paired")
        );
        assert!(
            resolve_phone(&ids, "5f1c")
                .unwrap_err()
                .contains("6 or more")
        );
        assert!(
            resolve_phone(&ids, "ffffff")
                .unwrap_err()
                .contains("starts with")
        );
    }
}
