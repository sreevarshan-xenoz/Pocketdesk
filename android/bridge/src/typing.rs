//! What the computer types on this phone (M4): `kdeconnect.pocketdesk.type`
//! batches, kept in order with the computer they came from, for the app's
//! typing thread to take as they come, waiting rather than polling.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::typing::{TypeEvent, events_of};

/// The most batches kept: past it, the typing thread is not taking, so
/// all are dropped (M4 review: dropping the oldest left a hole mid-text).
const MAX_WAITING: usize = 32;

/// How long a batch waits to be typed: later, its keys would land in
/// whatever field is in front by then (M4 review).
const STALE: Duration = Duration::from_secs(10);

struct Batch {
    host: String,
    port: u16,
    events: Vec<TypeEvent>,
    at: Instant,
}

static WAITING: Mutex<VecDeque<Batch>> = Mutex::new(VecDeque::new());
static ARRIVED: Condvar = Condvar::new();

/// Keep `body`'s batch from the computer at `host:port`, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(events) = events_of(body) else {
        return;
    };
    keep(Batch {
        host: host.to_string(),
        port,
        events,
        at: Instant::now(),
    });
}

fn keep(batch: Batch) {
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    if waiting.len() >= MAX_WAITING {
        waiting.clear();
    }
    waiting.push_back(batch);
    ARRIVED.notify_all();
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The oldest batch as `{"host", "port", "events": [...]}`, waiting up to
/// `timeout` for one; `""` when none came.
pub(crate) fn take_json(timeout: Duration) -> String {
    let waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    let (mut waiting, _) = ARRIVED
        .wait_timeout_while(waiting, timeout, |w| w.is_empty())
        .unwrap_or_else(|e| e.into_inner());
    while waiting.front().is_some_and(|b| b.at.elapsed() > STALE) {
        waiting.pop_front();
    }
    waiting
        .pop_front()
        .map(|b| {
            serde_json::json!({
                "host": b.host,
                "port": b.port,
                "events": b.events.iter().map(TypeEvent::to_json).collect::<Vec<_>>(),
            })
            .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn batches_are_taken_in_order_and_a_bad_one_is_not_kept() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        receive(
            &serde_json::json!({ "events": [{ "text": "a" }] }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "events": [{ "key": "tab" }] }),
            "192.0.2.7",
            1716,
        );
        receive(
            &serde_json::json!({ "events": [{ "key": "enter" }] }),
            "192.0.2.7",
            1716,
        );
        let first: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(first["events"][0]["text"], "a");
        assert_eq!(first["host"], "192.0.2.7");
        let second: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(second["events"][0]["key"], "enter");
        assert_eq!(take_json(Duration::from_millis(10)), "", "nothing waits");
    }

    #[tokio::test]
    async fn a_stale_batch_is_dropped_and_a_backlog_is_cleared() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let batch = |text: &str, at| Batch {
            host: "192.0.2.7".into(),
            port: 1716,
            events: vec![TypeEvent::Text(text.into())],
            at,
        };
        let Some(old) = Instant::now().checked_sub(STALE * 2) else {
            return;
        };
        keep(batch("old", old));
        keep(batch("new", Instant::now()));
        let taken: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(
            taken["events"][0]["text"], "new",
            "the stale one is dropped"
        );
        for i in 0..MAX_WAITING {
            keep(batch(&i.to_string(), Instant::now()));
        }
        keep(batch("after", Instant::now()));
        let taken: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(
            taken["events"][0]["text"], "after",
            "a full backlog is cleared"
        );
        assert_eq!(take_json(Duration::ZERO), "");
    }

    #[tokio::test]
    async fn a_waiting_take_wakes_when_a_batch_comes() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let taker = std::thread::spawn(|| take_json(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        receive(
            &serde_json::json!({ "events": [{ "global": "home" }] }),
            "192.0.2.7",
            1716,
        );
        let taken: serde_json::Value = serde_json::from_str(&taker.join().unwrap()).unwrap();
        assert_eq!(taken["events"][0]["global"], "home");
    }
}
