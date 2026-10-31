//! A bench of the Linux driver loop (`portal_drive`), run by hand: a
//! scripted portal backend feeds the real loop what libei would (a
//! crossing, then a 1 kHz mouse and some keys), and a second end of a
//! loopback link stands in for the computer driven, noting when each move
//! and key comes off the link. Both ends are in this process, so one clock
//! times capture to off the link with no offset to guess.
//!
//! `cargo test -p pocketdesk-server --lib driver_bench -- --ignored --nocapture`
//! prints each stage at p50/p95/p99/most, the slowest moves with when
//! they came (a stall every two seconds shows as a row of them), moves a
//! second on the link, and the driver threads' wake-ups at rest.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::barriers::Barrier;
use super::link::{self, Channel};
use super::portal_glue::{Backend, Portal};
use super::wire::{Msg, Screen};

/// The scripted portal's sender, handed over by [`start`] on the loop's
/// thread.
static SAID: Mutex<Option<Sender<Portal>>> = Mutex::new(None);

/// The barriers the loop armed last.
static ARMED: Mutex<Vec<Barrier>> = Mutex::new(Vec::new());

struct Script;

impl Backend for Script {
    fn arm(&mut self, barriers: &[Barrier]) {
        *ARMED.lock().unwrap() = barriers.to_vec();
    }
    fn disarm(&mut self) {
        ARMED.lock().unwrap().clear();
    }
    fn release(&mut self, _activation: Option<u32>, _at: (i32, i32)) {}
}

fn start(_state: Option<PathBuf>, said: Sender<Portal>) -> Option<Script> {
    *SAID.lock().unwrap() = Some(said);
    Some(Script)
}

fn screen() -> Screen {
    Screen {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    }
}

/// The context switches of this process's threads named `pd-shared*` (the
/// driver loop and what reads for it), voluntary and not.
fn wakeups() -> u64 {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return 0;
    };
    let mut total = 0;
    for task in tasks.flatten() {
        let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        if !comm.starts_with("pd-shared") {
            continue;
        }
        let status = std::fs::read_to_string(task.path().join("status")).unwrap_or_default();
        for line in status.lines() {
            if let Some(n) = line
                .strip_prefix("voluntary_ctxt_switches:")
                .or_else(|| line.strip_prefix("nonvoluntary_ctxt_switches:"))
            {
                total += n.trim().parse::<u64>().unwrap_or(0);
            }
        }
    }
    total
}

fn pct(sorted: &[u64], p: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) * p).div_ceil(100).min(sorted.len() - 1)]
}

fn stage(name: &str, mut v: Vec<u64>) {
    v.sort_unstable();
    println!(
        "{name}: n {} p50 {} p95 {} p99 {} most {} µs",
        v.len(),
        pct(&v, 50),
        pct(&v, 95),
        pct(&v, 99),
        v.last().copied().unwrap_or(0)
    );
}

/// Sleep until `due` on this thread, finely.
fn sleep_until(due: Instant) {
    let now = Instant::now();
    if due > now {
        std::thread::sleep(due - now);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a bench, run by hand on Linux"]
async fn the_driver_loop_bench() {
    let moves: u32 = std::env::var("PD_BENCH_MOVES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6000);
    let keys: u32 = 400;
    let rest = Duration::from_secs(5);
    let state = crate::state::use_test_dirs();
    let config_dir = pocketdesk_core::local::paths::config_dir().unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = pocketdesk_core::local::config::Config {
        shared_input: true,
        shared_input_drive: true,
        shared_input_clipboard: false,
        shared_input_sound: false,
        // PD_BENCH_QUICK=0: "Keep the connection quick" off.
        shared_input_low_latency: std::env::var("PD_BENCH_QUICK").map_or(true, |v| v != "0"),
        ..Default::default()
    };
    config
        .save_to(&config_dir.join(pocketdesk_core::local::paths::CONFIG_FILE))
        .unwrap();
    // The computer driven, said as heard, to the right of this one.
    pocketdesk_core::local::shared_input::update(&state, |file| {
        file.computers = vec![pocketdesk_core::local::shared_input::ComputerScreens {
            device_id: "bench-b".into(),
            name: "B".into(),
            screens: vec![screen()],
            changed_unix: 1,
            this_computer: false,
            phone: None,
        }];
    })
    .unwrap();

    let (a, b, out, back) = link::tests::linked_pair().await;
    let mut arriving = link::incoming();
    let _to_b = link::hold("bench-b", out);
    let _to_a = link::hold("bench-a", back);

    // B: answers A's hello with its screens, and notes when each move and
    // key comes off the link.
    type Arrived = Vec<(Msg, u64)>;
    let noted: Arc<Mutex<Arrived>> = Arc::default();
    let noting = Arc::clone(&noted);
    let receiver = tokio::spawn(async move {
        let mut answer: Option<Channel> = None;
        while let Some((from, msg, at_us, link_id)) = arriving.recv().await {
            // B's screens, come back to A: an answer, as A's receive loop
            // notes it.
            if from == "bench-b" {
                if matches!(msg, Msg::Screens(_)) {
                    link::note_screens(&from, link_id);
                }
                continue;
            }
            if from != "bench-a" {
                continue;
            }
            match &msg {
                Msg::Hello { .. } => {
                    let mut channel = Channel::open("bench-a").unwrap();
                    channel.send(&Msg::Screens(vec![screen()])).unwrap();
                    answer = Some(channel);
                }
                Msg::Move { .. } | Msg::Key { .. } | Msg::Enter { .. } | Msg::Leave => {
                    noting.lock().unwrap().push((msg, at_us));
                }
                _ => {}
            }
        }
        drop(answer);
    });

    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let runtime = tokio::runtime::Handle::current();
    let driver = std::thread::Builder::new()
        .name("pd-shared-drive".into())
        .spawn(move || super::portal_drive::drive(flag, runtime, start))
        .unwrap();

    let said = loop {
        if let Some(said) = SAID.lock().unwrap().clone() {
            break said;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    said.send(Portal::Zones(vec![screen()])).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let barrier = loop {
        let found = ARMED
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.to == "bench-b" && b.way == (1, 0))
            .cloned();
        if let Some(found) = found {
            break found;
        }
        assert!(Instant::now() < deadline, "no barrier toward B in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    // At rest at home.
    let before = wakeups();
    tokio::time::sleep(rest).await;
    let home_wakes = wakeups() - before;

    // Across, then at rest away (pinged, nothing moved).
    let edge = barrier.from.0;
    said.send(Portal::Activated {
        activation: Some(1),
        barrier: Some(barrier.id),
        at: Some((f64::from(edge.0), 540.0)),
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !noted
        .lock()
        .unwrap()
        .iter()
        .any(|(m, _)| matches!(m, Msg::Enter { .. }))
    {
        assert!(Instant::now() < deadline, "no crossing in time");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let before = wakeups();
    tokio::time::sleep(rest).await;
    let away_wakes = wakeups() - before;

    *link::SENT_PROBE.lock().unwrap() = Some(Vec::new());
    // A 1 kHz mouse up and down 250 px (every event a new position: move
    // `seq` n is event n), then keys at the same pace.
    let script = said.clone();
    let taken = tokio::task::spawn_blocking(move || {
        let mut moves_at = Vec::with_capacity(moves as usize);
        let mut keys_at = Vec::with_capacity(keys as usize);
        let t0 = Instant::now();
        for i in 0..moves {
            sleep_until(t0 + Duration::from_millis(u64::from(i)));
            let dy = if (i / 250) % 2 == 0 { 1.0 } else { -1.0 };
            moves_at.push(link::now_us());
            script.send(Portal::Motion { dx: 0.0, dy }).unwrap();
        }
        let t1 = Instant::now();
        for i in 0..keys {
            sleep_until(t1 + Duration::from_millis(u64::from(i)));
            keys_at.push(link::now_us());
            script
                .send(Portal::Key {
                    code: 30 + (i / 2) % 9,
                    down: i % 2 == 0,
                })
                .unwrap();
        }
        (moves_at, keys_at)
    })
    .await
    .unwrap();
    let (moves_at, keys_at) = taken;
    tokio::time::sleep(Duration::from_millis(500)).await;
    said.send(Portal::Deactivated {
        activation: Some(1),
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    stop.store(true, Ordering::SeqCst);
    let _ = tokio::task::spawn_blocking(move || driver.join()).await;

    let noted = noted.lock().unwrap().clone();
    let sent = link::SENT_PROBE.lock().unwrap().take().unwrap_or_default();
    let mut to_link = Vec::new();
    let mut slow_send: Vec<(u64, u64)> = Vec::new();
    for (seq, sent_us) in &sent {
        if let Some(taken) = moves_at.get((*seq as usize).wrapping_sub(1)) {
            let lat = sent_us.saturating_sub(*taken);
            to_link.push(lat);
            slow_send.push((lat, taken.saturating_sub(moves_at[0])));
        }
    }
    let start_us = moves_at.first().copied().unwrap_or(0);
    let mut move_lat = Vec::new();
    let mut slow: Vec<(u64, u64, u32)> = Vec::new();
    let mut move_arrivals = Vec::new();
    let mut key_lat = Vec::new();
    let mut slow_keys: Vec<(u64, usize)> = Vec::new();
    let mut key_n = 0usize;
    for (msg, at_us) in &noted {
        match msg {
            Msg::Move { seq, .. } => {
                let Some(taken) = moves_at.get((*seq as usize).wrapping_sub(1)) else {
                    continue;
                };
                let lat = at_us.saturating_sub(*taken);
                move_lat.push(lat);
                move_arrivals.push(*at_us);
                slow.push((lat, taken.saturating_sub(start_us), *seq));
            }
            Msg::Key { .. } => {
                if let Some(taken) = keys_at.get(key_n) {
                    key_lat.push(at_us.saturating_sub(*taken));
                    slow_keys.push((at_us.saturating_sub(*taken), key_n));
                }
                key_n += 1;
            }
            _ => {}
        }
    }
    let span_s = (moves_at.last().unwrap() - start_us) as f64 / 1e6;
    println!("--- driver bench: {moves} moves at 1 kHz over {span_s:.2} s, then {keys} keys ---");
    println!(
        "moves on the link: {} of {} events, {:.0} a second",
        move_lat.len(),
        moves,
        move_lat.len() as f64 / span_s
    );
    stage("move: captured to off the link", move_lat.clone());
    stage("key: captured to off the link", key_lat);
    stage("move: captured to handed to the link", to_link);
    slow_send.sort_unstable_by(|x, y| y.cmp(x));
    let listed: Vec<String> = slow_send
        .iter()
        .take(6)
        .map(|(lat, at)| format!("{lat} µs at {:.3} s", *at as f64 / 1e6))
        .collect();
    println!("slowest to the link: {}", listed.join(", "));
    slow_keys.sort_unstable_by(|x, y| y.cmp(x));
    let listed: Vec<String> = slow_keys
        .iter()
        .take(6)
        .map(|(lat, n)| format!("{lat} µs (key {n})"))
        .collect();
    println!("slowest keys: {}", listed.join(", "));
    for limit in [1_000u64, 2_000, 5_000, 10_000] {
        println!(
            "moves over {} ms: {}",
            limit / 1000,
            move_lat.iter().filter(|l| **l > limit).count()
        );
    }
    slow.sort_unstable_by(|x, y| y.cmp(x));
    let listed: Vec<String> = slow
        .iter()
        .take(12)
        .map(|(lat, at, _)| format!("{lat} µs at {:.3} s", *at as f64 / 1e6))
        .collect();
    println!("slowest moves: {}", listed.join(", "));
    let mut gaps: Vec<u64> = move_arrivals
        .windows(2)
        .map(|w| w[1].saturating_sub(w[0]))
        .collect();
    gaps.sort_unstable();
    println!(
        "gap between moves on the link: p50 {} p99 {} most {} µs",
        pct(&gaps, 50),
        pct(&gaps, 99),
        gaps.last().copied().unwrap_or(0)
    );
    println!(
        "driver thread wake-ups at rest: home {:.1}/s, away {:.1}/s",
        home_wakes as f64 / rest.as_secs_f64(),
        away_wakes as f64 / rest.as_secs_f64()
    );
    assert!(!move_lat.is_empty(), "no move came across");
    receiver.abort();
    a.close().await;
    b.close().await;
}
