//! Presses from the computer for the phone's player (roadmap T8d):
//! `kdeconnect.pocketdesk.phone_media_control {command}`, kept for the app,
//! which passes them to the media session it reported.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::phone_media::is_command;

/// How many presses wait for the app at most.
const MAX_WAITING: usize = 8;

/// How long a press waits for the app: a press is for now, and one the app
/// did not take in time (it was in the background) must not act later.
const PRESS_TTL: Duration = Duration::from_secs(10);

/// Presses the computer sent, oldest first, with when they came.
static WAITING: LazyLock<Mutex<VecDeque<(Instant, String)>>> = LazyLock::new(Default::default);

/// Keep a press the computer sent, if it is one.
pub(crate) fn receive(body: &serde_json::Value) {
    receive_at(body, Instant::now());
}

fn receive_at(body: &serde_json::Value, now: Instant) {
    let Some(command) = body.get("command").and_then(|v| v.as_str()) else {
        return;
    };
    if !is_command(command) {
        tracing::warn!("A media press from the computer was dropped: {command:?}");
        return;
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back((now, command.to_string()));
    while waiting.len() > MAX_WAITING {
        waiting.pop_front();
    }
}

/// Drop the presses waiting: they came from a computer this phone is no
/// longer connected to.
pub(crate) fn forget() {
    WAITING.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The presses waiting for the app, as a JSON array, oldest first; taken.
/// One older than [`PRESS_TTL`] is dropped.
pub(crate) fn take_json() -> String {
    take_json_at(Instant::now())
}

fn take_json_at(now: Instant) -> String {
    let taken: Vec<String> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .filter(|(at, _)| now.saturating_duration_since(*at) <= PRESS_TTL)
        .map(|(_, command)| command)
        .collect();
    serde_json::to_string(&taken).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is process-wide: these tests take it one at a time.
    static QUEUE: Mutex<()> = Mutex::new(());

    #[test]
    fn a_press_waits_once_and_only_if_it_is_one() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_json();
        receive(&serde_json::json!({ "command": "next" }));
        receive(&serde_json::json!({ "command": "seek" }));
        receive(&serde_json::json!({ "nothing": true }));
        let taken: Vec<String> = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken, ["next"]);
        assert_eq!(take_json(), "[]");
    }

    /// A press is for now: one the app did not take within 10 s (the app
    /// was in the background) must not act when it comes back.
    #[test]
    fn a_press_the_app_did_not_take_in_time_is_dropped() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_json();
        let then = Instant::now();
        receive_at(&serde_json::json!({ "command": "playPause" }), then);
        receive_at(&serde_json::json!({ "command": "next" }), then + PRESS_TTL);
        let taken: Vec<String> =
            serde_json::from_str(&take_json_at(then + PRESS_TTL + Duration::from_secs(1))).unwrap();
        assert_eq!(taken, ["next"]);
    }

    #[test]
    fn presses_are_forgotten_and_a_flood_keeps_the_newest() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take_json();
        receive(&serde_json::json!({ "command": "playPause" }));
        forget();
        assert_eq!(take_json(), "[]");
        for _ in 0..MAX_WAITING + 3 {
            receive(&serde_json::json!({ "command": "previous" }));
        }
        let taken: Vec<String> = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken.len(), MAX_WAITING);
    }
}
