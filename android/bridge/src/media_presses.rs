//! Presses from the computer for the phone's player (roadmap T8d):
//! `kdeconnect.pocketdesk.phone_media_control {command}`, kept for the app,
//! which passes them to the media session it reported.

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

use pocketdesk_core::protocol::phone_media::is_command;

/// How many presses wait for the app at most.
const MAX_WAITING: usize = 8;

/// Presses the computer sent, oldest first, for the app to act on.
static WAITING: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(Default::default);

/// Keep a press the computer sent, if it is one.
pub(crate) fn receive(body: &serde_json::Value) {
    let Some(command) = body.get("command").and_then(|v| v.as_str()) else {
        return;
    };
    if !is_command(command) {
        tracing::warn!("A media press from the computer was dropped: {command:?}");
        return;
    }
    let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
    waiting.push_back(command.to_string());
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
pub(crate) fn take_json() -> String {
    let taken: Vec<String> = WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
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
