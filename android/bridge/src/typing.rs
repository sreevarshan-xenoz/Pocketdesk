//! What the computer types on this phone (M4): `kdeconnect.pocketdesk.type`
//! batches, kept in order with the computer they came from, for the app's
//! typing thread to take as they come, waiting rather than polling.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use pocketdesk_core::protocol::typing::{TypeEvent, events_of};

/// The most batches kept: a thread that stopped taking keeps no backlog.
const MAX_WAITING: usize = 32;

struct Batch {
    host: String,
    port: u16,
    events: Vec<TypeEvent>,
}

static WAITING: Mutex<VecDeque<Batch>> = Mutex::new(VecDeque::new());
static ARRIVED: Condvar = Condvar::new();

/// Keep `body`'s batch from the computer at `host:port`, if it is one.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) {
    let Some(events) = events_of(body) else {
        return;
    };
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(Batch {
        host: host.to_string(),
        port,
        events,
    });
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
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
