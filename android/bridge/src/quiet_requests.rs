//! The computer's mutes for this phone (D2): `kdeconnect.pocketdesk.quiet
//! {minutes}`, kept with the computer it came from for the app, which acts
//! only for its own computer with its switch on. Only the newest counts: a
//! mute and then its end, read together, end it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::quiet::{MAX_MINUTES, QUIET};

/// How long a mute waits for the app: the listener not running, a mute an
/// hour old must not start when it does (D2 review).
const WAIT: Duration = Duration::from_secs(60);

static WAITING: Mutex<Option<(Instant, serde_json::Value)>> = Mutex::new(None);

/// Keep a mute of `packet_type` from the computer at `host:port`, if it is
/// one with minutes in range.
pub(crate) fn receive(packet_type: &str, body: &serde_json::Value, host: &str, port: u16) {
    if packet_type != QUIET {
        return;
    }
    let Some(minutes) = body
        .get("minutes")
        .and_then(|v| v.as_u64())
        .filter(|m| *m <= u64::from(MAX_MINUTES))
    else {
        return;
    };
    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = Some((
        Instant::now(),
        serde_json::json!({ "host": host, "port": port, "minutes": minutes }),
    ));
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    *WAITING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The newest mute waiting, as JSON, or `null`; taken.
pub(crate) fn take_json() -> String {
    take_within(WAIT)
}

fn take_within(wait: Duration) -> String {
    WAITING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .filter(|(at, _)| at.elapsed() <= wait)
        .map(|(_, mute)| mute)
        .unwrap_or(serde_json::Value::Null)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_newest_mute_in_range_waits_with_its_computer() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let host = "192.0.2.7";
        assert_eq!(take_json(), "null");
        receive(QUIET, &serde_json::json!({ "minutes": 60 }), host, 1716);
        receive(QUIET, &serde_json::json!({ "minutes": 0 }), host, 1716);
        let taken: serde_json::Value = serde_json::from_str(&take_json()).unwrap();
        assert_eq!(taken["minutes"], 0, "the newest");
        assert_eq!(taken["host"], host);
        assert_eq!(take_json(), "null", "taken");
        receive(QUIET, &serde_json::json!({ "minutes": 5000 }), host, 1716);
        receive(QUIET, &serde_json::json!({ "minutes": "60" }), host, 1716);
        receive(
            "kdeconnect.pocketdesk.quiet_state",
            &serde_json::json!({ "minutes": 1 }),
            host,
            1716,
        );
        assert_eq!(take_json(), "null", "none of those");
        receive(QUIET, &serde_json::json!({ "minutes": 5 }), host, 1716);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(take_within(Duration::from_millis(1)), "null", "too old");
    }
}
