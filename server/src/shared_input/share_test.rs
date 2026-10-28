//! `pocketdesk share-test` (hidden): send a paired computer a script of
//! shared input over the link, and print how long its pings took to come
//! back; and its other end, a server started with `POCKETDESK_SHARE_TEST`
//! set to a file, which records each message that arrives there, with when
//! it arrived, instead of using it (the receiver, [`super::receive`], does
//! not start then). The command injects nothing itself; a receiving server
//! that lets the sending computer drive it types and moves what it sends.
//!
//! The command asks the running server, which holds the link: a request in
//! `share_test_out/`, its outcome in `share_test_in/`, one file each under
//! the request's id.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pocketdesk_core::local::pin::now_unix;
use serde::{Deserialize, Serialize};

use super::link::{self, Channel};
use super::wire::{self, Msg};
use crate::local_api::exit;

/// The variable that makes a server record what other computers send it
/// (to the file it names) instead of using it.
pub const RECORD_ENV: &str = "POCKETDESK_SHARE_TEST";

/// Where the command leaves its requests for the server.
pub const REQUESTS_DIR: &str = "share_test_out";

/// Where the server leaves each request's outcome.
const OUTCOMES_DIR: &str = "share_test_in";

/// A request older than this, in seconds, was left by a command that gave
/// up on it; dropped.
const REQUEST_LIFE: u64 = 60;

/// Without a folder watcher, how often the requests are looked for.
const POLL: Duration = Duration::from_secs(1);

/// How long the command waits for the server's answer, besides the script.
const OUTCOME_WAIT: Duration = Duration::from_secs(45);

/// How long a link just up may still ride the relay (while iroh punches a
/// direct path) before the test is refused.
const DIRECT_WAIT: Duration = Duration::from_secs(10);

/// The time between scripted events: a fast typist's, a 1 kHz mouse's.
const GAP: Duration = Duration::from_millis(1);

/// A ping goes with every this many events.
const PING_EVERY: usize = 20;

/// How long the last pings' answers are waited for.
const PONG_WAIT: Duration = Duration::from_secs(2);

/// The keys a script presses and lets go of in turn: a to l on the home row,
/// as evdev codes.
const KEYS: [u16; 9] = [30, 31, 32, 33, 34, 35, 36, 37, 38];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Request {
    id: String,
    computer: String,
    keys: u32,
    moves: u32,
    created_unix: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Outcome {
    /// Why nothing (or not all) was sent.
    error: Option<String>,
    /// The computer, by its id.
    computer: String,
    /// The messages sent, pings included.
    sent: usize,
    pings: usize,
    /// Each answered ping's round trip, in microseconds, in the order they
    /// came back.
    round_trips_us: Vec<u64>,
}

/// What `share-test` sends after its `Hello` (pings aside): the cursor
/// coming onto the other computer, `keys` key events (each key pressed, then
/// let go) and `moves` pointer moves taken in turn, everything let go of
/// (an odd `keys` ends on a press), and the cursor leaving.
pub fn script(keys: u32, moves: u32) -> Vec<Msg> {
    let mut out = vec![Msg::Enter { x: 32768, y: 32768 }];
    for i in 0..keys.max(moves) {
        if i < moves {
            // Along the diagonal and round again.
            let at = u16::try_from(i % 512).unwrap_or(0) * 128;
            out.push(Msg::Move {
                seq: i + 1,
                x: at,
                y: at,
            });
        }
        if i < keys {
            out.push(Msg::Key {
                code: KEYS[(i / 2) as usize % KEYS.len()],
                pressed: i % 2 == 0,
            });
        }
    }
    out.push(Msg::ReleaseAll);
    out.push(Msg::Leave);
    out
}

/// The command: ask the running server to send `computer` the script, and
/// print what came of it. The exit code: 0 sent, 1 failed, 3 no server
/// running, 4 no answer from it in time.
pub fn run_cli(computer: &str, keys: u32, moves: u32) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("share-test: no state folder: {e:#}");
            return exit::FAILED;
        }
    };
    if !pocketdesk_core::local::server::is_running(&dir) {
        eprintln!("share-test: no Pocketdesk server is running");
        return exit::NO_SERVER;
    }
    let request = Request {
        id: uuid::Uuid::new_v4().simple().to_string(),
        computer: computer.to_string(),
        keys,
        moves,
        created_unix: now_unix(),
    };
    if let Err(e) = leave(&dir, &request) {
        eprintln!("share-test: the request could not be left for the server: {e}");
        return exit::FAILED;
    }
    // The script takes a millisecond an event, more where timers are coarse.
    let events = u64::from(keys.max(moves));
    let deadline = std::time::Instant::now()
        + OUTCOME_WAIT
        + DIRECT_WAIT
        + Duration::from_millis(events.saturating_mul(4));
    loop {
        if let Some(outcome) = take_outcome(&dir, &request.id) {
            print!("{}", report(&outcome));
            return if outcome.error.is_some() {
                exit::FAILED
            } else {
                exit::DONE
            };
        }
        if std::time::Instant::now() >= deadline {
            let _ = std::fs::remove_file(request_path(&dir, &request.id));
            eprintln!("share-test: no answer from the server in time");
            return exit::STILL_WAITING;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the command prints for `outcome`.
fn report(outcome: &Outcome) -> String {
    let mut out = String::new();
    if let Some(error) = &outcome.error {
        out.push_str(&format!("share-test: {error}\n"));
    }
    if outcome.sent == 0 {
        return out;
    }
    out.push_str(&format!(
        "sent {} messages to {}\n",
        outcome.sent, outcome.computer
    ));
    let mut sorted = outcome.round_trips_us.clone();
    sorted.sort_unstable();
    let (Some(least), Some(most)) = (sorted.first(), sorted.last()) else {
        out.push_str(&format!(
            "round trips: none of {} pings answered\n",
            outcome.pings
        ));
        return out;
    };
    let listed: Vec<String> = outcome
        .round_trips_us
        .iter()
        .map(|us| us.to_string())
        .collect();
    out.push_str(&format!("round trips (µs): {}\n", listed.join(" ")));
    out.push_str(&format!(
        "round trips: {} of {} pings answered; least {least} µs, median {} µs, most {most} µs\n",
        sorted.len(),
        outcome.pings,
        sorted[sorted.len() / 2],
    ));
    out
}

fn request_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(REQUESTS_DIR).join(format!("{id}.json"))
}

fn outcome_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(OUTCOMES_DIR).join(format!("{id}.json"))
}

/// Write `json` to `path` whole: to a temporary file first, then renamed.
fn write_whole(path: &Path, json: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Leave `request` for the server.
fn leave(dir: &Path, request: &Request) -> std::io::Result<()> {
    let json = serde_json::to_string(request).map_err(std::io::Error::other)?;
    write_whole(&request_path(dir, &request.id), &json)
}

/// The outcome of the request `id`, once the server left it (taken).
fn take_outcome(dir: &Path, id: &str) -> Option<Outcome> {
    let path = outcome_path(dir, id);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(serde_json::from_str(&text).unwrap_or_else(|e| Outcome {
        error: Some(format!("the server's answer did not read: {e}")),
        ..Outcome::default()
    }))
}

/// Every request waiting, oldest first, each removed once read; one that
/// is stale or does not read is dropped, and so is a temporary file left
/// past a request's life.
fn take_requests(dir: &Path, now: u64) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(dir.join(REQUESTS_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > REQUEST_LIFE);
            if stale {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let text = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = text
            .ok()
            .and_then(|t| serde_json::from_str::<Request>(&t).ok())
        else {
            continue;
        };
        if now.abs_diff(request.created_unix) <= REQUEST_LIFE {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

/// The server's side: run each `share-test` request as it comes, for the
/// server's life.
pub async fn run_requests() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        for request in take_requests(&dir, now_unix()) {
            let outcome = run_one(&dir, &request).await;
            let written = serde_json::to_string(&outcome)
                .map_err(std::io::Error::other)
                .and_then(|json| write_whole(&outcome_path(&dir, &request.id), &json));
            if let Err(e) = written {
                tracing::warn!("share-test: the outcome could not be left: {e}");
            }
        }
        crate::inbox::wait(REQUESTS_DIR, POLL).await;
    }
}

/// Run one request: open the channel, send the Hello and the script with
/// pings among it, and gather the answers.
async fn run_one(dir: &Path, request: &Request) -> Outcome {
    let mut outcome = Outcome::default();
    let device_id = match pick(&link::linked(), &names(dir), &request.computer) {
        Ok(id) => id,
        Err(why) => {
            outcome.error = Some(why);
            return outcome;
        }
    };
    outcome.computer = device_id.clone();
    let mut channel = match open_when_direct(&device_id).await {
        Ok(channel) => channel,
        Err(why) => {
            outcome.error = Some(why);
            return outcome;
        }
    };
    tracing::info!(device_id, "share-test: sending a script of shared input");
    let me = crate::kde::host_identity();
    let hello = Msg::Hello {
        version: wire::VERSION,
        name: me.device_name,
        screens: Vec::new(),
    };
    let ping = |channel: &mut Channel, outcome: &mut Outcome| {
        channel.send(&Msg::Ping {
            t_us: link::now_us(),
        })?;
        outcome.sent += 1;
        outcome.pings += 1;
        Ok::<(), String>(())
    };
    let sending = async {
        channel.send(&hello)?;
        outcome.sent += 1;
        let started = tokio::time::Instant::now();
        for (i, msg) in script(request.keys, request.moves).iter().enumerate() {
            let due = started + GAP * u32::try_from(i).unwrap_or(u32::MAX);
            tokio::time::sleep_until(due).await;
            if i % PING_EVERY == 0 {
                ping(&mut channel, &mut outcome)?;
            }
            channel.send(msg)?;
            outcome.sent += 1;
        }
        ping(&mut channel, &mut outcome)
    }
    .await;
    if let Err(why) = sending {
        outcome.error = Some(why);
    }
    while outcome.round_trips_us.len() < outcome.pings {
        let Some(pong) = channel.pong(PONG_WAIT).await else {
            break;
        };
        let us = u64::try_from(pong.round_trip().as_micros()).unwrap_or(u64::MAX);
        outcome.round_trips_us.push(us);
    }
    outcome
}

/// [`Channel::open`], waiting up to [`DIRECT_WAIT`] while the link rides
/// the relay (a link just up may, until iroh punches a direct path).
async fn open_when_direct(device_id: &str) -> Result<Channel, String> {
    let deadline = tokio::time::Instant::now() + DIRECT_WAIT;
    loop {
        match Channel::open(device_id) {
            Ok(channel) => return Ok(channel),
            Err(why) if why.contains("relay") && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(why) => return Err(why),
        }
    }
}

/// The names this computer knows its computers by: the ones it paired with
/// (`computers.json`), and the ones that paired with it (the trust store).
fn names(dir: &Path) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = pocketdesk_core::local::computers::read(dir)
        .computers
        .into_iter()
        .map(|c| (c.device_id, c.name))
        .collect();
    let store = crate::state::trust_store_path()
        .ok()
        .and_then(|path| pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok());
    if let Some(store) = store {
        for id in store.trusted_devices() {
            if store.is_computer(&id)
                && let Some(name) = store.name(&id)
            {
                names.push((id.clone(), name.to_string()));
            }
        }
    }
    names
}

/// The computer `query` names among those `linked` now: by its id, or by
/// its name in `names` (any case).
fn pick(linked: &[String], names: &[(String, String)], query: &str) -> Result<String, String> {
    if linked.iter().any(|id| id == query) {
        return Ok(query.to_string());
    }
    let named: Vec<&String> = linked
        .iter()
        .filter(|id| {
            names
                .iter()
                .any(|(known, name)| known == *id && name.eq_ignore_ascii_case(query))
        })
        .collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no computer linked now is {query:?}")),
        _ => Err(format!(
            "more than one computer linked now is called {query:?}: name it by its id"
        )),
    }
}

/// The receiving server's side with [`RECORD_ENV`] set: append each
/// message other computers send to `file`, one JSON object a line (`at_us`,
/// microseconds since the Unix epoch as it was taken off the channel;
/// `from`, the computer's id; `msg`), instead of using it.
pub async fn record(file: PathBuf) {
    use tokio::io::AsyncWriteExt;
    let mut arriving = link::incoming();
    let opened = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .await;
    let mut out = match opened {
        Ok(out) => out,
        Err(e) => {
            tracing::error!("{RECORD_ENV}: {} does not open: {e}", file.display());
            return;
        }
    };
    tracing::warn!(
        "{RECORD_ENV}: recording what other computers send to {}, not using it",
        file.display()
    );
    while let Some(first) = arriving.recv().await {
        // Each stamped as it is taken; what waits behind it is taken too
        // before the write.
        let mut lines = line(first);
        while let Ok(next) = arriving.try_recv() {
            lines.push_str(&line(next));
        }
        let written = async {
            out.write_all(lines.as_bytes()).await?;
            out.flush().await
        }
        .await;
        if let Err(e) = written {
            tracing::error!("{RECORD_ENV}: could not write {}: {e}", file.display());
            return;
        }
    }
}

/// One recorded line for `msg` from `from`, taken now.
fn line((from, msg): (String, Msg)) -> String {
    let mut line = serde_json::json!({
        "at_us": link::now_us(),
        "from": from,
        "msg": msg,
    })
    .to_string();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_enters_types_moves_and_leaves() {
        let script = script(5, 3);
        assert_eq!(script.first(), Some(&Msg::Enter { x: 32768, y: 32768 }));
        // The last key is left down (an odd count): everything is let go
        // of before the cursor leaves.
        assert_eq!(script[script.len() - 2..], [Msg::ReleaseAll, Msg::Leave]);
        let keys: Vec<(u16, bool)> = script
            .iter()
            .filter_map(|m| match m {
                Msg::Key { code, pressed } => Some((*code, *pressed)),
                _ => None,
            })
            .collect();
        // Each key pressed, then let go, in turn.
        assert_eq!(
            keys,
            [(30, true), (30, false), (31, true), (31, false), (32, true)]
        );
        let seqs: Vec<u32> = script
            .iter()
            .filter_map(|m| match m {
                Msg::Move { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, [1, 2, 3]);
    }

    #[test]
    fn a_computer_is_picked_by_id_or_name() {
        let linked = vec!["aaaa".to_string(), "bbbb".to_string()];
        let names = vec![
            ("aaaa".to_string(), "Desk".to_string()),
            ("bbbb".to_string(), "Laptop".to_string()),
            ("cccc".to_string(), "Old".to_string()),
        ];
        assert_eq!(pick(&linked, &names, "bbbb"), Ok("bbbb".to_string()));
        assert_eq!(pick(&linked, &names, "laptop"), Ok("bbbb".to_string()));
        // Known, but not linked now.
        assert!(pick(&linked, &names, "Old").is_err());
        assert!(pick(&linked, &names, "nobody").is_err());
        let twice = vec![
            ("aaaa".to_string(), "Same".to_string()),
            ("bbbb".to_string(), "same".to_string()),
        ];
        let err = pick(&linked, &twice, "same").unwrap_err();
        assert!(err.contains("more than one"), "{err}");
    }

    #[test]
    fn requests_are_taken_once_and_stale_ones_dropped() {
        let dir = std::env::temp_dir().join(format!("pd-share-requests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let request = |id: &str, created_unix| Request {
            id: id.to_string(),
            computer: "Laptop".to_string(),
            keys: 1,
            moves: 1,
            created_unix,
        };
        leave(&dir, &request("new", 1_000)).unwrap();
        leave(&dir, &request("old", 1_000 - REQUEST_LIFE - 1)).unwrap();
        assert_eq!(take_requests(&dir, 1_000), vec![request("new", 1_000)]);
        assert!(take_requests(&dir, 1_000).is_empty(), "taken once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_report_lists_the_round_trips() {
        let outcome = Outcome {
            error: None,
            computer: "bbbb".to_string(),
            sent: 10,
            pings: 3,
            round_trips_us: vec![900, 300, 600],
        };
        let said = report(&outcome);
        assert!(said.contains("sent 10 messages to bbbb"), "{said}");
        assert!(said.contains("round trips (µs): 900 300 600"), "{said}");
        assert!(
            said.contains("3 of 3 pings answered; least 300 µs, median 600 µs, most 900 µs"),
            "{said}"
        );
        let unanswered = report(&Outcome {
            round_trips_us: Vec::new(),
            ..outcome
        });
        assert!(
            unanswered.contains("none of 3 pings answered"),
            "{unanswered}"
        );
    }

    /// A recorded line reads back as the message it was.
    #[test]
    fn a_recorded_line_reads_back() {
        let msg = Msg::Key {
            code: 30,
            pressed: true,
        };
        let line = line(("aaaa".to_string(), msg.clone()));
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["from"], "aaaa");
        assert!(value["at_us"].as_u64().is_some());
        assert_eq!(
            serde_json::from_value::<Msg>(value["msg"].clone()).unwrap(),
            msg
        );
    }
}
