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
pub fn devices(state_dir: &Path, now_unix: u64, running: bool) -> Value {
    let live = live_sessions(state_dir, now_unix, running);
    let keys = device_keys::read(state_dir).unwrap_or_default();
    let devices: Vec<Value> = match TrustStore::load_or_create(state_dir.join(TRUST_FILE)) {
        Ok(store) => store
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
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    versioned(json!({ "server_running": running, "devices": devices }))
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
            "version": env!("CARGO_PKG_VERSION"),
        },
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
    let listed = devices(&dir, pocketdesk_core::local::pin::now_unix(), running);
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

/// Print `value` as one line of JSON on stdout.
pub fn print(value: &Value) {
    println!("{value}");
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
        let listed = devices(&dir, 101, true);
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
        assert_eq!(b["in_session"], false);
        assert!(b["grant_remaining_secs"].as_u64().is_some_and(|s| s <= 60));
        // A server that is not running has no sessions, whatever its file says.
        let stopped = devices(&dir, 101, false);
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
        assert_eq!(stopped["server"]["version"], env!("CARGO_PKG_VERSION"));
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
