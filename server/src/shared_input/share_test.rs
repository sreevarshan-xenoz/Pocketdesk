//! `pocketdesk share-test` (hidden): send a paired computer a script of
//! shared input over the link, and print how long its pings took to come
//! back and how long its keys and moves took ([`Report`], also kept in
//! `shared_input_latency.json`): Pocketdesk's share (here, from taking each
//! to handing it to the link; there, from taking it off the link to
//! injecting it, which that computer sends back when asked,
//! [`Msg::AskTimings`]) and the network's (one way, the two clocks lined
//! up by the pings' answers, [`clock_offset`]). And its other end, a server started with `POCKETDESK_SHARE_TEST`
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

use super::link::{self, Channel, Pong};
use super::quick::{KeepQuick, Nudge};
use super::wire::{self, Msg, Timing};
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

/// A ping goes with every this many events: enough answers to line the
/// clocks up by.
const PING_EVERY: usize = 5;

/// The longest pause between events `--pause` takes: a Wi-Fi card saving
/// power dozes within a tenth of a second, so a longer one shows nothing
/// more, and the server runs one test at a time.
const PAUSE_MOST_MS: u32 = 5_000;

/// How often an activity note goes while the script runs, as a driving
/// computer sends one: the receiver lets go of a driver silent for a second.
const ACTIVE_EVERY: Duration = Duration::from_millis(200);

/// How long the last pings' answers, and the timings, are waited for.
const PONG_WAIT: Duration = Duration::from_secs(2);

/// A list of round trips longer than this is not printed whole.
const LISTED_MOST: usize = 50;

/// The keys a script presses and lets go of in turn: a to l on the home row,
/// as evdev codes.
const KEYS: [u16; 9] = [30, 31, 32, 33, 34, 35, 36, 37, 38];

/// What `share-test` measured: printed, and kept in
/// `shared_input_latency.json` for the desktop app's Diagnostics.
pub use pocketdesk_core::local::shared_input::LatencyReport as Report;

/// One key or move as this computer sent it: a move (`n` its `seq`) or a
/// key (`n` the count since the cursor arrived, 1 the first), when the
/// script captured it (as a hook would take it, before the loop's other
/// work for that event: the nudge, a ping, an activity note) and when it
/// was handed to the link, on this computer's clock ([`link::now_us`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sent {
    moved: bool,
    n: u32,
    taken_us: u64,
    sent_us: u64,
}

/// How far the other computer's clock is ahead of this one's, in
/// microseconds, from the answers to this computer's pings: the answer
/// with the least round trip, taken to have been made halfway through it.
/// It is off by half how uneven that round trip was, which nothing a round
/// trip shows can tell.
fn clock_offset(pongs: &[Pong]) -> Option<i64> {
    let best = pongs
        .iter()
        .min_by_key(|p| p.back_us.saturating_sub(p.sent_us))?;
    let halfway =
        i128::from(best.sent_us) + i128::from(best.back_us.saturating_sub(best.sent_us)) / 2;
    i64::try_from(i128::from(best.at_us) - halfway).ok()
}

/// How near a key or move, on this computer's clock, the pings are that
/// line the two clocks up for it: two computers' clocks drift apart by
/// milliseconds over a minute, which a paused script lasts.
const CLOCKS_NEAR_US: u64 = 2_000_000;

/// [`clock_offset`] from the pings sent within [`CLOCKS_NEAR_US`] of
/// `at_us`, or from them all when none was.
fn offset_near(pongs: &[Pong], at_us: u64) -> Option<i64> {
    let near: Vec<Pong> = pongs
        .iter()
        .filter(|p| p.sent_us.abs_diff(at_us) <= CLOCKS_NEAR_US)
        .copied()
        .collect();
    clock_offset(&near).or_else(|| clock_offset(pongs))
}

/// One stage's times over a run, in microseconds: the median, the 95th and
/// 99th percentiles and the most.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

impl Stage {
    /// The stage over `sorted` times.
    fn of(sorted: &[u64]) -> Stage {
        Stage {
            p50_us: percentile(sorted, 50),
            p95_us: percentile(sorted, 95),
            p99_us: percentile(sorted, 99),
            max_us: sorted.last().copied().unwrap_or(0),
        }
    }
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "p50 {} µs, p95 {} µs, p99 {} µs, most {} µs",
            self.p50_us, self.p95_us, self.p99_us, self.max_us
        )
    }
}

/// Each stage of a run, end to end: captured to handed to the link here,
/// the network one way, off the link to injected there, and all three; and
/// how many keys and moves were sent and injected (a move the computer
/// driven dropped as stale, or one lost on the way, is sent and not
/// injected).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stages {
    pub sending: Stage,
    pub network: Stage,
    pub receiving: Stage,
    pub total: Stage,
    pub keys_sent: u32,
    pub keys_injected: u32,
    pub moves_sent: u32,
    pub moves_injected: u32,
}

/// The `pct`th percentile of `sorted` (nearest rank); 0 for none.
fn percentile(sorted: &[u64], pct: usize) -> u64 {
    let rank = (sorted.len() * pct).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or(0)
}

/// The report for the keys and moves `sent` and the `timings` the other
/// computer sent back, its clock lined up by the answers to the `pongs`
/// sent near each ([`offset_near`]): each key
/// and move timed on both sides (a move by its `seq`, a key by its count)
/// is Pocketdesk's share (taken to handed to the link here, plus off the
/// link to injected there) and the network's (handed to the link here to
/// off it there, one way; under 0, by the clocks' error, is 0). None when
/// nothing was timed on both sides.
#[cfg(test)]
fn split(sent: &[Sent], timings: &[Timing], pongs: &[Pong], now_unix: u64) -> Option<Report> {
    split_stages(sent, timings, pongs, now_unix).map(|(report, _)| report)
}

/// [`split`], with each stage's percentiles and the counts ([`Stages`]).
fn split_stages(
    sent: &[Sent],
    timings: &[Timing],
    pongs: &[Pong],
    now_unix: u64,
) -> Option<(Report, Stages)> {
    let there: std::collections::HashMap<(bool, u32), &Timing> =
        timings.iter().map(|t| ((t.moved, t.n), t)).collect();
    let (mut ours, mut network, mut sending, mut receiving, mut total) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut stages = Stages::default();
    for s in sent {
        if s.moved {
            stages.moves_sent += 1;
        } else {
            stages.keys_sent += 1;
        }
        let Some(t) = there.get(&(s.moved, s.n)) else {
            continue;
        };
        if s.moved {
            stages.moves_injected += 1;
        } else {
            stages.keys_injected += 1;
        }
        let Some(offset_us) = offset_near(pongs, s.sent_us) else {
            continue;
        };
        let here = s.sent_us.saturating_sub(s.taken_us);
        let arrived = i128::from(t.arrived_us) - i128::from(offset_us);
        let away = u64::try_from((arrived - i128::from(s.sent_us)).max(0)).unwrap_or(u64::MAX);
        sending.push(here);
        receiving.push(u64::from(t.took_us));
        ours.push(here + u64::from(t.took_us));
        network.push(away);
        total.push(here + away + u64::from(t.took_us));
    }
    if ours.is_empty() {
        return None;
    }
    for times in [
        &mut ours,
        &mut network,
        &mut sending,
        &mut receiving,
        &mut total,
    ] {
        times.sort_unstable();
    }
    stages.sending = Stage::of(&sending);
    stages.network = Stage::of(&network);
    stages.receiving = Stage::of(&receiving);
    stages.total = Stage::of(&total);
    let report = Report {
        network_p50_us: percentile(&network, 50),
        network_p95_us: percentile(&network, 95),
        ours_p50_us: percentile(&ours, 50),
        ours_p95_us: percentile(&ours, 95),
        sending_p95_us: percentile(&sending, 95),
        receiving_p95_us: percentile(&receiving, 95),
        samples: u32::try_from(ours.len()).unwrap_or(u32::MAX),
        measured_unix: now_unix,
    };
    Some((report, stages))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Request {
    id: String,
    computer: String,
    keys: u32,
    moves: u32,
    /// Milliseconds between events; 0 is [`GAP`]. With a pause, pings go
    /// as a driver's do while away ([`pinging_every`]), not with every
    /// [`PING_EVERY`] events.
    #[serde(default)]
    pause_ms: u32,
    created_unix: u64,
}

/// How often pings go between the events of a script paused `pause_ms`
/// between them: as a computer driving another pings it while away. None
/// for a script without a pause, whose pings go with its events.
fn pinging_every(pause_ms: u32) -> Option<Duration> {
    (pause_ms > 0).then_some(super::drive::PING_EVERY)
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
    /// What the keys and moves took, when the other computer sent its
    /// timings back.
    #[serde(default)]
    latency: Option<Report>,
    /// Each stage end to end, with the 99th percentile and the most.
    #[serde(default)]
    stages: Option<Stages>,
    /// Why there is no `latency`.
    #[serde(default)]
    unmeasured: Option<String>,
    /// The link was kept quick meanwhile, as a crossing keeps it
    /// (`shared_input_low_latency`, BK10).
    #[serde(default)]
    quick: bool,
}

/// What `share-test` sends after its `Hello` (pings and activity notes,
/// every 200 ms, aside): the cursor
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
pub fn run_cli(computer: &str, keys: u32, moves: u32, pause_ms: u32) -> i32 {
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
        pause_ms: pause_ms.min(PAUSE_MOST_MS),
        created_unix: now_unix(),
    };
    if let Err(e) = leave(&dir, &request) {
        eprintln!("share-test: the request could not be left for the server: {e}");
        return exit::FAILED;
    }
    // The script takes a millisecond an event, more where timers are
    // coarse, or its pause.
    let events = u64::from(keys.max(moves)) * 2 + 3;
    let each = 4 + u64::from(request.pause_ms);
    let deadline = std::time::Instant::now()
        + OUTCOME_WAIT
        + DIRECT_WAIT
        + Duration::from_millis(events.saturating_mul(each));
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
        "sent {} messages to {}, the link kept quick: {}\n",
        outcome.sent,
        outcome.computer,
        if outcome.quick { "yes" } else { "no" }
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
    if outcome.round_trips_us.len() <= LISTED_MOST {
        let listed: Vec<String> = outcome
            .round_trips_us
            .iter()
            .map(|us| us.to_string())
            .collect();
        out.push_str(&format!("round trips (µs): {}\n", listed.join(" ")));
    }
    out.push_str(&format!(
        "round trips: {} of {} pings answered; least {least} µs, median {} µs, most {most} µs\n",
        sorted.len(),
        outcome.pings,
        sorted[sorted.len() / 2],
    ));
    match (&outcome.latency, &outcome.unmeasured) {
        (Some(r), _) => out.push_str(&format!(
            "latency over {} keys and moves:\n  Pocketdesk's share: p50 {} µs, p95 {} µs (sending p95 {} µs, \
             injecting there p95 {} µs)\n  the network's share: p50 {} µs, p95 {} µs (one way)\n",
            r.samples,
            r.ours_p50_us,
            r.ours_p95_us,
            r.sending_p95_us,
            r.receiving_p95_us,
            r.network_p50_us,
            r.network_p95_us,
        )),
        (None, Some(why)) => out.push_str(&format!("latency: not measured: {why}\n")),
        (None, None) => {}
    }
    if let Some(st) = &outcome.stages {
        out.push_str(&format!(
            "end to end, captured to injected: {}\n  captured to the link here: {}\n  \
             the network (one way): {}\n  off the link to injected there: {}\n  \
             injected: {} of {} keys, {} of {} moves\n",
            st.total,
            st.sending,
            st.network,
            st.receiving,
            st.keys_injected,
            st.keys_sent,
            st.moves_injected,
            st.moves_sent,
        ));
    }
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
    pocketdesk_core::atomic_file::replace(&tmp, path)
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
    // Held as a crossing holds it, so the test measures what one gets.
    let mut quick = KeepQuick::default();
    outcome.quick =
        tokio::task::spawn_blocking(|| crate::events::current_config().shared_input_low_latency)
            .await
            .unwrap_or(false);
    quick.set(outcome.quick);
    // Following the computer as the drivers do: its address looked up
    // again as the script goes on (review M5).
    let mut nudge = Nudge::default();
    let following = outcome.quick.then_some(device_id.as_str());
    nudge.follow(following);
    tracing::info!(device_id, "share-test: sending a script of shared input");
    let me = crate::kde::host_identity();
    let hello = Msg::Hello {
        version: wire::VERSION,
        name: me.device_name,
        screens: Vec::new(),
    };
    // Each key and move as it was sent, for its timing there.
    let mut sent: Vec<Sent> = Vec::new();
    let mut keys = 0u32;
    let ping = |channel: &mut Channel, outcome: &mut Outcome| {
        channel.send(&Msg::Ping {
            t_us: link::now_us(),
        })?;
        outcome.sent += 1;
        outcome.pings += 1;
        Ok::<(), String>(())
    };
    let gap = match request.pause_ms {
        0 => GAP,
        ms => Duration::from_millis(u64::from(ms.min(PAUSE_MOST_MS))),
    };
    let pinging = pinging_every(request.pause_ms);
    let sending = async {
        channel.send(&hello)?;
        outcome.sent += 1;
        let started = tokio::time::Instant::now();
        let mut active_due = started;
        let mut ping_due = started;
        for (i, msg) in script(request.keys, request.moves).iter().enumerate() {
            let due = started + gap * u32::try_from(i).unwrap_or(u32::MAX);
            // Paused: pinged meanwhile as a driver pings while away.
            if let Some(every) = pinging {
                while ping_due < due {
                    tokio::time::sleep_until(ping_due).await;
                    nudge.follow(following);
                    ping(&mut channel, &mut outcome)?;
                    ping_due += every;
                    let now = tokio::time::Instant::now();
                    if now >= active_due {
                        channel.send(&Msg::Active { idle_ms: 0 })?;
                        outcome.sent += 1;
                        active_due = now + ACTIVE_EVERY;
                    }
                }
            }
            tokio::time::sleep_until(due).await;
            // Captured, as a hook takes a key or move: the loop's other
            // work for it below counts, as a driver's does.
            let taken_us = link::now_us();
            nudge.follow(following);
            if pinging.is_none() && i % PING_EVERY == 0 {
                ping(&mut channel, &mut outcome)?;
            }
            let now = tokio::time::Instant::now();
            if now >= active_due {
                channel.send(&Msg::Active { idle_ms: 0 })?;
                outcome.sent += 1;
                active_due = now + ACTIVE_EVERY;
            }
            channel.send(msg)?;
            let sent_us = link::now_us();
            outcome.sent += 1;
            match msg {
                Msg::Move { seq, .. } => sent.push(Sent {
                    moved: true,
                    n: *seq,
                    taken_us,
                    sent_us,
                }),
                Msg::Key { .. } => {
                    keys += 1;
                    sent.push(Sent {
                        moved: false,
                        n: keys,
                        taken_us,
                        sent_us,
                    });
                }
                _ => {}
            }
        }
        ping(&mut channel, &mut outcome)?;
        // After the leave, on the same stream: everything is injected by
        // the time it is read there.
        channel.send(&Msg::AskTimings)?;
        outcome.sent += 1;
        Ok(())
    }
    .await;
    if let Err(why) = sending {
        outcome.error = Some(why);
    }
    let mut pongs = Vec::new();
    while pongs.len() < outcome.pings {
        let Some(pong) = channel.pong(PONG_WAIT).await else {
            break;
        };
        let us = u64::try_from(pong.round_trip().as_micros()).unwrap_or(u64::MAX);
        outcome.round_trips_us.push(us);
        pongs.push(pong);
    }
    let timings = channel.timings(PONG_WAIT).await;
    let measured = match (clock_offset(&pongs), timings) {
        (None, _) => Err("no ping was answered".to_string()),
        (Some(_), None) => Err(
            "the other computer sent no timings back (it does not take this one's input, only \
             records it, or runs an older Pocketdesk)"
                .to_string(),
        ),
        (Some(_), Some(timings)) => split_stages(&sent, &timings, &pongs, now_unix())
            .ok_or_else(|| "no key or move was timed on both computers".to_string()),
    };
    match measured {
        Ok((report, stages)) => {
            outcome.stages = Some(stages);
            if let Err(e) = pocketdesk_core::local::shared_input::write_latency(dir, &report) {
                tracing::warn!("share-test: the latency could not be kept: {e}");
            }
            outcome.latency = Some(report);
        }
        Err(why) => outcome.unmeasured = Some(why),
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
/// microseconds since the Unix epoch as it came off the link;
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
        // Each stamped as it came off the link; what waits behind it is
        // taken too before the write.
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

/// One recorded line for `msg` from `from`, come off the link at `at_us`.
fn line((from, msg, at_us, _): link::Arrival) -> String {
    let mut line = serde_json::json!({
        "at_us": at_us,
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

    /// BK10: a paused script pings between its events as a driver pings
    /// while away; one without a pause pings with its events. A request
    /// from before the pause has none.
    #[test]
    fn a_paused_script_pings_as_a_driver_away() {
        use super::super::drive::PING_EVERY as AWAY;
        assert_eq!(pinging_every(0), None);
        assert_eq!(pinging_every(1_000), Some(AWAY));
        let old: Request = serde_json::from_str(
            r#"{"id":"a","computer":"L","keys":1,"moves":0,"created_unix":5}"#,
        )
        .unwrap();
        assert_eq!(old.pause_ms, 0);
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
            pause_ms: 0,
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
            latency: Some(Report {
                network_p50_us: 1_100,
                network_p95_us: 2_400,
                ours_p50_us: 120,
                ours_p95_us: 480,
                sending_p95_us: 30,
                receiving_p95_us: 450,
                samples: 200,
                measured_unix: 1,
            }),
            stages: None,
            unmeasured: None,
            quick: true,
        };
        let said = report(&outcome);
        assert!(
            said.contains("sent 10 messages to bbbb, the link kept quick: yes"),
            "{said}"
        );
        assert!(said.contains("round trips (µs): 900 300 600"), "{said}");
        assert!(
            said.contains("3 of 3 pings answered; least 300 µs, median 600 µs, most 900 µs"),
            "{said}"
        );
        assert!(said.contains("latency over 200 keys and moves"), "{said}");
        assert!(
            said.contains("Pocketdesk's share: p50 120 µs, p95 480 µs"),
            "{said}"
        );
        assert!(
            said.contains("the network's share: p50 1100 µs, p95 2400 µs"),
            "{said}"
        );
        let unmeasured = report(&Outcome {
            latency: None,
            unmeasured: Some("no timings".to_string()),
            ..outcome.clone()
        });
        assert!(
            unmeasured.contains("latency: not measured: no timings"),
            "{unmeasured}"
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

    /// The other clock is put where the fastest answer says: that answer is
    /// taken to have been made halfway through its round trip, and a slow
    /// one (queued on the way out) does not pull it.
    #[test]
    fn clock_offset_from_round_trips() {
        assert_eq!(clock_offset(&[]), None);
        for ahead in [5_000_000i64, -3_000_000, 0] {
            // Sent at `sent_us`, `out` microseconds there and `back` back.
            let pong = |sent_us: u64, out: u64, back: u64| Pong {
                sent_us,
                back_us: sent_us + out + back,
                at_us: (sent_us + out).checked_add_signed(ahead).unwrap(),
            };
            let pongs = [
                pong(10_000_000, 9_000, 100),
                pong(11_000_000, 400, 400),
                pong(12_000_000, 700, 900),
            ];
            assert_eq!(clock_offset(&pongs), Some(ahead), "{ahead}");
        }
        // An uneven fastest answer is off by half its unevenness: the most
        // a round trip can tell.
        let lopsided = [Pong {
            sent_us: 1_000,
            back_us: 2_000,
            at_us: 1_900,
        }];
        assert_eq!(clock_offset(&lopsided), Some(400));
    }

    /// Each key and move timed on both sides splits into Pocketdesk's share
    /// (sending here plus injecting there) and the network's (one way, the
    /// other clock lined up); a key and a move of the same count are told
    /// apart, and what only one side has is left out.
    #[test]
    fn report_splits_network_and_ours() {
        let offset = 7_000_000i64;
        // One ping, even both ways, puts the other clock `offset` ahead.
        let pongs = [Pong {
            sent_us: 100_000,
            back_us: 102_000,
            at_us: 101_000u64.checked_add_signed(offset).unwrap(),
        }];
        let mut sent = Vec::new();
        let mut timings = Vec::new();
        for i in 1..=20u32 {
            let taken_us = u64::from(i) * 10_000;
            let sent_us = taken_us + 10;
            sent.push(Sent {
                moved: false,
                n: i,
                taken_us,
                sent_us,
            });
            let arrived = sent_us + 1_000 + u64::from(i) * 10;
            timings.push(Timing {
                moved: false,
                n: i,
                arrived_us: arrived.checked_add_signed(offset).unwrap(),
                took_us: i * 5,
            });
        }
        // A move with the same count as a key, timed as a move: the
        // slowest by far, and not mixed up with key 1.
        sent.push(Sent {
            moved: true,
            n: 1,
            taken_us: 500_000,
            sent_us: 500_020,
        });
        timings.push(Timing {
            moved: true,
            n: 1,
            arrived_us: 503_020u64.checked_add_signed(offset).unwrap(),
            took_us: 2_000,
        });
        // A move dropped there as stale, and a timing for nothing sent.
        sent.push(Sent {
            moved: true,
            n: 2,
            taken_us: 600_000,
            sent_us: 600_010,
        });
        timings.push(Timing {
            moved: false,
            n: 99,
            arrived_us: 0,
            took_us: 1,
        });
        let report = split(&sent, &timings, &pongs, 1_700_000_000).expect("timed");
        // Ours: keys 15, 20, .. 110 µs, the move 2020; the network: keys
        // 1010 .. 1200 µs, the move 3000.
        assert_eq!(report.samples, 21);
        assert_eq!(report.ours_p50_us, 65);
        assert_eq!(report.ours_p95_us, 110);
        assert_eq!(report.network_p50_us, 1_110);
        assert_eq!(report.network_p95_us, 1_200);
        assert_eq!(report.sending_p95_us, 10);
        assert_eq!(report.receiving_p95_us, 100);
        assert_eq!(report.measured_unix, 1_700_000_000);
        // Each stage end to end, with the counts: the stale move is sent
        // and not injected.
        let (_, stages) = split_stages(&sent, &timings, &pongs, 0).expect("timed");
        assert_eq!((stages.keys_sent, stages.keys_injected), (20, 20));
        assert_eq!((stages.moves_sent, stages.moves_injected), (2, 1));
        assert_eq!(stages.sending.max_us, 20);
        assert_eq!(stages.receiving.p99_us, 2_000);
        assert_eq!(stages.receiving.max_us, 2_000);
        assert_eq!(stages.network.max_us, 3_000);
        // The move: 20 here, 3000 on the way, 2000 there.
        assert_eq!(stages.total.max_us, 5_020);
        let said = super::report(&Outcome {
            sent: 1,
            round_trips_us: vec![1],
            pings: 1,
            stages: Some(stages),
            ..Outcome::default()
        });
        assert!(
            said.contains("injected: 20 of 20 keys, 1 of 2 moves"),
            "{said}"
        );
        assert!(said.contains("most 5020 µs"), "{said}");
        // Nothing timed on both sides: no report.
        assert_eq!(split(&sent, &[], &pongs, 0), None);
        assert_eq!(split(&sent, &timings, &[], 0), None, "no clock to line up");
    }

    /// BK10: over a paused script the clocks drift apart, so each key is
    /// lined up by the pings sent near it, not by the fastest of the run.
    #[test]
    fn each_key_is_lined_up_by_the_pings_near_it() {
        // The other clock 1 ms ahead at first, 21 ms ahead a minute on;
        // the first ping the fastest of all.
        let pong = |sent_us: u64, rtt: u64, ahead: u64| Pong {
            sent_us,
            back_us: sent_us + rtt,
            at_us: sent_us + rtt / 2 + ahead,
        };
        let pongs = [pong(1_000_000, 800, 1_000), pong(61_000_000, 1_200, 21_000)];
        assert_eq!(offset_near(&pongs, 1_500_000), Some(1_000));
        assert_eq!(offset_near(&pongs, 60_000_000), Some(21_000));
        // None near: the fastest of them all.
        assert_eq!(offset_near(&pongs, 30_000_000), Some(1_000));
        assert_eq!(offset_near(&[], 0), None);
        // A key sent at 61 s that took 2 ms reads as 2 ms, not 22.
        let sent = [Sent {
            moved: false,
            n: 1,
            taken_us: 61_000_000,
            sent_us: 61_000_000,
        }];
        let timings = [Timing {
            moved: false,
            n: 1,
            arrived_us: 61_000_000 + 2_000 + 21_000,
            took_us: 50,
        }];
        let report = split(&sent, &timings, &pongs, 0).unwrap();
        assert_eq!(report.network_p50_us, 2_000);
    }

    /// A recorded line reads back as the message it was.
    #[test]
    fn a_recorded_line_reads_back() {
        let msg = Msg::Key {
            code: 30,
            pressed: true,
        };
        let line = line(("aaaa".to_string(), msg.clone(), 42, 0));
        let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["from"], "aaaa");
        assert!(value["at_us"].as_u64().is_some());
        assert_eq!(
            serde_json::from_value::<Msg>(value["msg"].clone()).unwrap(),
            msg
        );
    }
}
