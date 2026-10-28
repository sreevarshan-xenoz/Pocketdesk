//! `pocketdesk phone-screen-test` (hidden): send the owner's phone a
//! script of what a keyboard and mouse would do on it as a screen (BK5),
//! through the same channel the driver uses ([`PhoneChannel`]), and print
//! what was sent and how many of its pings the phone answered. It moves no
//! cursor and reads no key here: the script is the input. The phone must
//! stand ready ([`phone::standing_now`]): it allows it, and this computer's
//! Keyboard and mouse permission for it is on.
//!
//! The command asks the running server, which holds the phone's link: a
//! request in `phone_test_out/`, its outcome in `phone_test_in/`, one file
//! each under the request's id.
//!
//! A script is steps with `;` between ([`parse`]): `enter X Y`, `move X Y`
//! and `glide X Y N` (N moves there, 16 ms apart), positions as fractions
//! of the phone's screen from 0 to 1; `down`, `up` and `click` (the left
//! button), `right` and `middle` (a click of either); `wheel N` (notches,
//! positive down); `type TEXT` (on a US keyboard); `key NAME` (enter,
//! backspace, delete, escape, left, right, up, down); `wait MS`; `leave`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pocketdesk_core::local::pin::now_unix;
use pocketdesk_core::local::shared_input::PHONE_READY;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use serde::{Deserialize, Serialize};

use super::phone::{self, PhoneChannel, code_of};
use super::wire::Msg;
use crate::local_api::exit;

/// Where the command leaves its requests for the server.
pub const REQUESTS_DIR: &str = "phone_test_out";

/// Where the server leaves each request's outcome.
const OUTCOMES_DIR: &str = "phone_test_in";

/// A request older than this, in seconds, was given up on.
const REQUEST_LIFE: u64 = 60;

/// Without a folder watcher, how often the requests are looked for.
const POLL: Duration = Duration::from_secs(1);

/// How long the command waits for the server, besides the script's waits.
const OUTCOME_WAIT: Duration = Duration::from_secs(30);

/// How often a ping goes while the script runs, as the driver's do.
const PING_EVERY: Duration = Duration::from_millis(200);

/// The gap between the moves of a glide.
const GLIDE_GAP: Duration = Duration::from_millis(16);

/// The Shift key, as an evdev code.
const SHIFT: u16 = 42;

/// One step of a script: a message, or a wait.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Send(Msg),
    Wait(Duration),
}

/// A fraction from 0 to 1 as a position out of 65535.
fn fraction(word: Option<&str>) -> Result<u16, String> {
    let word = word.ok_or("a position is missing")?;
    let f: f64 = word
        .parse()
        .map_err(|_| format!("{word} is not a fraction"))?;
    if !(0.0..=1.0).contains(&f) {
        return Err(format!("{word} is not between 0 and 1"));
    }
    Ok((f * 65535.0).round() as u16)
}

/// A count in `word`.
fn number<T: std::str::FromStr>(word: Option<&str>) -> Result<T, String> {
    let word = word.ok_or("a number is missing")?;
    word.parse().map_err(|_| format!("{word} is not a number"))
}

/// The steps of `script`.
pub fn parse(script: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut at = (32768u16, 32768u16);
    let mut seq = 0u32;
    let button = |button: u8, pressed: bool| Step::Send(Msg::Button { button, pressed });
    for raw in script.split(';') {
        let raw = raw.trim();
        let (word, rest) = raw.split_once(' ').unwrap_or((raw, ""));
        let mut words = rest.split_whitespace();
        match word {
            "" => {}
            "enter" => {
                at = (fraction(words.next())?, fraction(words.next())?);
                steps.push(Step::Send(Msg::Enter { x: at.0, y: at.1 }));
            }
            "move" => {
                at = (fraction(words.next())?, fraction(words.next())?);
                seq += 1;
                steps.push(Step::Send(Msg::Move {
                    seq,
                    x: at.0,
                    y: at.1,
                }));
            }
            "glide" => {
                let to = (fraction(words.next())?, fraction(words.next())?);
                let n: u32 = number(words.next())?;
                if n == 0 || n > 500 {
                    return Err("a glide takes 1 to 500 moves".to_string());
                }
                let from = at;
                for i in 1..=n {
                    let step = |a: u16, b: u16| {
                        (f64::from(a) + (f64::from(b) - f64::from(a)) * f64::from(i) / f64::from(n))
                            .round() as u16
                    };
                    at = (step(from.0, to.0), step(from.1, to.1));
                    seq += 1;
                    steps.push(Step::Send(Msg::Move {
                        seq,
                        x: at.0,
                        y: at.1,
                    }));
                    steps.push(Step::Wait(GLIDE_GAP));
                }
            }
            "down" => steps.push(button(0, true)),
            "up" => steps.push(button(0, false)),
            "click" | "right" | "middle" => {
                let which = match word {
                    "click" => 0,
                    "middle" => 1,
                    _ => 2,
                };
                steps.push(button(which, true));
                steps.push(Step::Wait(Duration::from_millis(30)));
                steps.push(button(which, false));
            }
            "wheel" => {
                let dy: i16 = number(words.next())?;
                steps.push(Step::Send(Msg::Wheel { dx: 0, dy }));
            }
            "type" => {
                for c in rest.chars() {
                    let (code, shift) =
                        code_of(c).ok_or(format!("{c:?} is not on a US keyboard"))?;
                    let key = |code, pressed| Step::Send(Msg::Key { code, pressed });
                    if shift {
                        steps.push(key(SHIFT, true));
                    }
                    steps.push(key(code, true));
                    steps.push(key(code, false));
                    if shift {
                        steps.push(key(SHIFT, false));
                    }
                }
            }
            "key" => {
                let code = match words.next() {
                    Some("enter") => 28,
                    Some("backspace") => 14,
                    Some("delete") => 111,
                    Some("escape") => 1,
                    Some("left") => 105,
                    Some("right") => 106,
                    Some("up") => 103,
                    Some("down") => 108,
                    other => return Err(format!("no key {}", other.unwrap_or("named"))),
                };
                steps.push(Step::Send(Msg::Key {
                    code,
                    pressed: true,
                }));
                steps.push(Step::Send(Msg::Key {
                    code,
                    pressed: false,
                }));
            }
            "wait" => {
                let ms: u64 = number(words.next())?;
                if ms > 10_000 {
                    return Err("a wait is at most 10000 ms".to_string());
                }
                steps.push(Step::Wait(Duration::from_millis(ms)));
            }
            "leave" => steps.push(Step::Send(Msg::Leave)),
            other => return Err(format!("no step {other}")),
        }
    }
    Ok(steps)
}

/// What the command asks of the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    id: String,
    phone: String,
    script: String,
    created_unix: u64,
}

/// What came of it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Outcome {
    #[serde(default)]
    phone: String,
    #[serde(default)]
    sent: u32,
    #[serde(default)]
    pings: u32,
    #[serde(default)]
    pongs: u64,
    #[serde(default)]
    error: Option<String>,
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

/// The command: check the script, ask the running server to send it to
/// `phone`, and print what came of it. The exit code: 0 sent, 1 failed, 2
/// a script that does not read, 3 no server running, 4 no answer in time.
pub fn run_cli(phone: &str, script: &str) -> i32 {
    let steps = match parse(script) {
        Ok(steps) => steps,
        Err(e) => {
            eprintln!("phone-screen-test: {e}");
            return exit::BAD_ARGUMENTS;
        }
    };
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("phone-screen-test: no state folder: {e:#}");
            return exit::FAILED;
        }
    };
    if !pocketdesk_core::local::server::is_running(&dir) {
        eprintln!("phone-screen-test: no Pocketdesk server is running");
        return exit::NO_SERVER;
    }
    let request = Request {
        id: uuid::Uuid::new_v4().simple().to_string(),
        phone: phone.to_string(),
        script: script.to_string(),
        created_unix: now_unix(),
    };
    let left = serde_json::to_string(&request)
        .map_err(std::io::Error::other)
        .and_then(|json| write_whole(&request_path(&dir, &request.id), &json));
    if let Err(e) = left {
        eprintln!("phone-screen-test: the request could not be left for the server: {e}");
        return exit::FAILED;
    }
    let waits: Duration = steps
        .iter()
        .map(|s| match s {
            Step::Wait(d) => *d,
            Step::Send(_) => Duration::ZERO,
        })
        .sum();
    let deadline = std::time::Instant::now() + OUTCOME_WAIT + waits;
    loop {
        let path = outcome_path(&dir, &request.id);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let _ = std::fs::remove_file(&path);
            let outcome: Outcome = serde_json::from_str(&text).unwrap_or_else(|e| Outcome {
                error: Some(format!("the server's answer did not read: {e}")),
                ..Outcome::default()
            });
            if let Some(error) = &outcome.error {
                eprintln!("phone-screen-test: {error}");
                return exit::FAILED;
            }
            println!(
                "sent {} messages to {}; {} of {} pings answered",
                outcome.sent, outcome.phone, outcome.pongs, outcome.pings
            );
            return exit::DONE;
        }
        if std::time::Instant::now() >= deadline {
            let _ = std::fs::remove_file(request_path(&dir, &request.id));
            eprintln!("phone-screen-test: no answer from the server in time");
            return exit::STILL_WAITING;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every request waiting, oldest first, each removed once read.
fn take_requests(dir: &Path, now: u64) -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(dir.join(REQUESTS_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        if let Some(request) = text
            .ok()
            .and_then(|t| serde_json::from_str::<Request>(&t).ok())
            .filter(|r| now.abs_diff(r.created_unix) <= REQUEST_LIFE)
        {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

/// The paired phone `query` names: its id, or its name here (whole,
/// ignoring case). Blocking file work.
fn pick(query: &str) -> Result<String, String> {
    let store = crate::state::trust_store_path()
        .map_err(|e| format!("no trust store: {e:#}"))
        .and_then(|path| TrustStore::read(path).map_err(|e| format!("{e:#}")))?;
    let phones: Vec<String> = store
        .paired_devices()
        .into_iter()
        .filter(|id| !store.is_computer(id))
        .collect();
    phones
        .iter()
        .find(|id| *id == query)
        .or_else(|| {
            phones.iter().find(|id| {
                store
                    .name(id)
                    .is_some_and(|name| name.eq_ignore_ascii_case(query))
            })
        })
        .cloned()
        .ok_or_else(|| format!("no paired phone is {query}"))
}

/// The server's side: run each request as it comes, for the server's life.
pub async fn run_requests() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        for request in take_requests(&dir, now_unix()) {
            let outcome = run_one(&request).await;
            let written = serde_json::to_string(&outcome)
                .map_err(std::io::Error::other)
                .and_then(|json| write_whole(&outcome_path(&dir, &request.id), &json));
            if let Err(e) = written {
                tracing::warn!("phone-screen-test: the outcome could not be left: {e}");
            }
        }
        crate::inbox::wait(REQUESTS_DIR, POLL).await;
    }
}

/// Run one request: the phone must stand ready; the script goes on a
/// channel of its own, a ping every [`PING_EVERY`] among it, and the
/// phone is left at the end whatever the script said.
async fn run_one(request: &Request) -> Outcome {
    let mut outcome = Outcome::default();
    let steps = match parse(&request.script) {
        Ok(steps) => steps,
        Err(e) => {
            outcome.error = Some(e);
            return outcome;
        }
    };
    let query = request.phone.clone();
    let picked = tokio::task::spawn_blocking(move || {
        let id = pick(&query)?;
        let standing = phone::standing_now(&id);
        if standing != PHONE_READY {
            return Err(format!("the phone is not ready: {standing}"));
        }
        Ok(id)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let id = match picked {
        Ok(id) => id,
        Err(e) => {
            outcome.error = Some(e);
            return outcome;
        }
    };
    outcome.phone = id.clone();
    let before = phone::pongs(&id).map_or(0, |(_, n)| n);
    let mut channel = PhoneChannel::open(&id);
    let mut pinged = tokio::time::Instant::now();
    let send = |channel: &mut PhoneChannel, msg: &Msg, outcome: &mut Outcome| {
        channel.send(msg).map(|()| outcome.sent += 1)
    };
    let ping = |outcome: &mut Outcome| {
        outcome.pings += 1;
        Msg::Ping {
            t_us: super::link::now_us(),
        }
    };
    for step in steps.iter().chain([Step::Send(Msg::Leave)].iter()) {
        if pinged.elapsed() >= PING_EVERY {
            pinged = tokio::time::Instant::now();
            let msg = ping(&mut outcome);
            if let Err(e) = send(&mut channel, &msg, &mut outcome) {
                outcome.error = Some(e);
                return outcome;
            }
        }
        match step {
            Step::Send(msg) => {
                if let Err(e) = send(&mut channel, msg, &mut outcome) {
                    outcome.error = Some(e);
                    return outcome;
                }
            }
            Step::Wait(gap) => {
                // Pinged through a long wait, as the driver pings an idle
                // phone it is on.
                let until = tokio::time::Instant::now() + *gap;
                while tokio::time::Instant::now() < until {
                    let next = (pinged + PING_EVERY).min(until);
                    tokio::time::sleep_until(next).await;
                    if pinged.elapsed() >= PING_EVERY && tokio::time::Instant::now() < until {
                        pinged = tokio::time::Instant::now();
                        let msg = ping(&mut outcome);
                        let _ = send(&mut channel, &msg, &mut outcome);
                    }
                }
            }
        }
    }
    // The last answers.
    tokio::time::sleep(Duration::from_secs(1)).await;
    outcome.pongs = phone::pongs(&id).map_or(0, |(_, n)| n) - before;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sends(steps: &[Step]) -> Vec<Msg> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::Send(msg) => Some(msg.clone()),
                Step::Wait(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_script_reads_into_steps() {
        let steps =
            parse("enter 0.5 0; move 1 0.25 ; click; wheel -3; key enter; wait 40; leave").unwrap();
        assert_eq!(
            sends(&steps),
            vec![
                Msg::Enter { x: 32768, y: 0 },
                Msg::Move {
                    seq: 1,
                    x: 65535,
                    y: 16384
                },
                Msg::Button {
                    button: 0,
                    pressed: true
                },
                Msg::Button {
                    button: 0,
                    pressed: false
                },
                Msg::Wheel { dx: 0, dy: -3 },
                Msg::Key {
                    code: 28,
                    pressed: true
                },
                Msg::Key {
                    code: 28,
                    pressed: false
                },
                Msg::Leave,
            ]
        );
        assert!(steps.contains(&Step::Wait(Duration::from_millis(40))));
        assert_eq!(
            sends(&parse("right; middle").unwrap())
                .iter()
                .filter(|m| matches!(m, Msg::Button { pressed: true, .. }))
                .count(),
            2
        );
    }

    #[test]
    fn a_glide_moves_in_even_steps_and_typing_shifts_where_it_must() {
        let steps = parse("enter 0 0; glide 1 1 4").unwrap();
        let moves: Vec<(u16, u16)> = sends(&steps)
            .into_iter()
            .filter_map(|m| match m {
                Msg::Move { x, y, .. } => Some((x, y)),
                _ => None,
            })
            .collect();
        assert_eq!(
            moves,
            vec![
                (16384, 16384),
                (32768, 32768),
                (49151, 49151),
                (65535, 65535)
            ]
        );
        let typed = sends(&parse("type Hi").unwrap());
        assert_eq!(
            typed,
            vec![
                Msg::Key {
                    code: SHIFT,
                    pressed: true
                },
                Msg::Key {
                    code: 35,
                    pressed: true
                },
                Msg::Key {
                    code: 35,
                    pressed: false
                },
                Msg::Key {
                    code: SHIFT,
                    pressed: false
                },
                Msg::Key {
                    code: 23,
                    pressed: true
                },
                Msg::Key {
                    code: 23,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn a_script_that_does_not_read_says_why() {
        for (bad, why) in [
            ("enter 2 0", "between 0 and 1"),
            ("move 0.5", "missing"),
            ("glide 1 1 0", "1 to 500"),
            ("key tab", "no key tab"),
            ("type é", "not on a US keyboard"),
            ("wait 99999", "at most"),
            ("jump", "no step jump"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert_eq!(parse(" ; ").unwrap(), Vec::new());
    }
}
