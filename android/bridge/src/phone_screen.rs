//! This phone as another screen for its computer's keyboard and mouse
//! (BK5): `kdeconnect.pocketdesk.shared_input.input` batches, kept in order
//! with the computer they came from for the app's pointer thread, which
//! waits for them rather than polling; each ping in one answered at once
//! (`shared_input.pong`) while that thread is taking and the app does not
//! refuse it ([`refuse`]), so the computer brings its mouse home when the
//! app stops or refuses; and what the phone says of itself
//! (`shared_input.screen`).

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use pocketdesk_core::protocol::phone_screen::{
    PONG, PhoneEvent, SCREEN, ScreenReport, events_of, pong_body,
};

/// The most batches kept: past it, the pointer thread is not taking, so
/// the oldest go (a pointer's moves, stale by then).
const MAX_WAITING: usize = 64;

/// How long a batch waits to be done: later, a tap would land where the
/// pointer no longer is.
const STALE: Duration = Duration::from_secs(1);

/// The pointer thread took last this recently: it is taking, so input is
/// kept and pings answered. It waits at most a quarter of a second a turn.
const TAKING_WITHIN: Duration = Duration::from_secs(1);

struct Batch {
    host: String,
    port: u16,
    events: Vec<PhoneEvent>,
    at: Instant,
}

static WAITING: Mutex<VecDeque<Batch>> = Mutex::new(VecDeque::new());
static ARRIVED: Condvar = Condvar::new();
static TAKEN_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// Until when the app refuses what comes (its switch off, its service
/// gone, or not its own computer): nothing is kept and no ping answered,
/// so the computer brings its mouse home within a second.
static REFUSED_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// How long one refusal holds: the app says it again while it refuses.
const REFUSAL_HOLDS: Duration = Duration::from_secs(2);

/// The app refuses what comes now (the review's M2): for
/// [`REFUSAL_HOLDS`], whether or not it goes on taking; what waits goes.
pub(crate) fn refuse() {
    *lock(&REFUSED_UNTIL) = Some(Instant::now() + REFUSAL_HOLDS);
    forget();
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the pointer thread is taking now.
pub(crate) fn taking() -> bool {
    let refused = lock(&REFUSED_UNTIL).is_some_and(|until| Instant::now() < until);
    !refused && lock(&TAKEN_AT).is_some_and(|at| at.elapsed() < TAKING_WITHIN)
}

/// Keep `body`'s batch from the computer at `host:port`, if it is one and
/// the pointer thread is taking; the pings in it, answered, are what
/// [`receive`] returns to send.
pub(crate) fn receive(body: &serde_json::Value, host: &str, port: u16) -> Vec<NetworkPacket> {
    if !taking() {
        return Vec::new();
    }
    let Some(events) = events_of(body) else {
        return Vec::new();
    };
    let pongs = events
        .iter()
        .filter_map(|e| match e {
            PhoneEvent::Ping { t } => Some(NetworkPacket::new(PONG).with_body(pong_body(*t))),
            _ => None,
        })
        .collect();
    keep(Batch {
        host: host.to_string(),
        port,
        events,
        at: Instant::now(),
    });
    pongs
}

fn keep(batch: Batch) {
    let mut waiting = lock(&WAITING);
    while waiting.len() >= MAX_WAITING {
        waiting.pop_front();
    }
    waiting.push_back(batch);
    ARRIVED.notify_all();
}

/// Drop what waits: it came from a computer the phone left.
pub(crate) fn forget() {
    lock(&WAITING).clear();
}

/// The oldest batch as `{"host", "port", "events": [...]}`, waiting up to
/// `timeout` for one; `""` when none came. Taking is what keeps input
/// coming ([`taking`]).
pub(crate) fn take_json(timeout: Duration) -> String {
    *lock(&TAKEN_AT) = Some(Instant::now());
    let waiting = lock(&WAITING);
    let (mut waiting, _) = ARRIVED
        .wait_timeout_while(waiting, timeout, |w| w.is_empty())
        .unwrap_or_else(|e| e.into_inner());
    *lock(&TAKEN_AT) = Some(Instant::now());
    while waiting.front().is_some_and(|b| b.at.elapsed() > STALE) {
        waiting.pop_front();
    }
    waiting
        .pop_front()
        .map(|b| {
            serde_json::json!({
                "host": b.host,
                "port": b.port,
                "events": b.events,
            })
            .to_string()
        })
        .unwrap_or_default()
}

/// The packet saying `report_json` (a [`ScreenReport`]'s body), if it is
/// one.
pub(crate) fn screen_packet(report_json: &str) -> Result<NetworkPacket, String> {
    let body: serde_json::Value =
        serde_json::from_str(report_json).map_err(|e| format!("not a report: {e}"))?;
    let report = ScreenReport::from_body(&body).ok_or("not a report of a screen")?;
    Ok(NetworkPacket::new(SCREEN).with_body(report.to_body()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(events: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "events": events })
    }

    #[tokio::test]
    async fn batches_are_kept_only_while_taken_and_pings_answered() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        *lock(&TAKEN_AT) = None;
        let ping = batch(serde_json::json!([{ "type": "ping", "t": 7 }]));
        assert!(
            receive(&ping, "192.0.2.7", 1716).is_empty(),
            "nobody taking: no answer"
        );
        assert_eq!(take_json(Duration::ZERO), "", "and nothing kept");
        // Taking now.
        let pongs = receive(
            &batch(serde_json::json!([
                { "type": "pointer", "x": 1, "y": 2 },
                { "type": "ping", "t": 9 }
            ])),
            "192.0.2.7",
            1716,
        );
        assert_eq!(pongs.len(), 1);
        assert_eq!(pongs[0].packet_type, PONG);
        assert_eq!(pongs[0].body["t"], 9);
        let taken: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(taken["host"], "192.0.2.7");
        assert_eq!(taken["events"][0]["type"], "pointer");
        assert_eq!(taken["events"][0]["x"], 1);
        // Not a batch: nothing kept, nothing answered.
        assert!(receive(&serde_json::json!({}), "192.0.2.7", 1716).is_empty());
        assert_eq!(take_json(Duration::ZERO), "");
    }

    #[tokio::test]
    async fn a_stale_batch_is_dropped_and_a_backlog_keeps_the_newest() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let at = |i: u16, at| Batch {
            host: "192.0.2.7".into(),
            port: 1716,
            events: vec![PhoneEvent::Pointer { x: i, y: i }],
            at,
        };
        let Some(old) = Instant::now().checked_sub(STALE * 2) else {
            return;
        };
        keep(at(0, old));
        keep(at(1, Instant::now()));
        let taken: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(taken["events"][0]["x"], 1, "the stale one is dropped");
        for i in 0..(MAX_WAITING as u16 + 5) {
            keep(at(i, Instant::now()));
        }
        let taken: serde_json::Value = serde_json::from_str(&take_json(Duration::ZERO)).unwrap();
        assert_eq!(taken["events"][0]["x"], 5, "the oldest made room");
        forget();
    }

    /// The review's M2: once the app refuses (its switch off, its service
    /// gone), pings go unanswered and nothing is kept, at once, so the
    /// computer comes home within its second; taking after the refusal has
    /// held answers again.
    #[tokio::test]
    async fn a_refusal_stops_the_answers_at_once() {
        let _globals = crate::api::TRANSFER_TEST_LOCK.lock().await;
        forget();
        let ping = batch(serde_json::json!([{ "type": "ping", "t": 1 }]));
        take_json(Duration::ZERO);
        assert_eq!(receive(&ping, "192.0.2.7", 1716).len(), 1, "taking");
        forget();
        refuse();
        assert!(receive(&ping, "192.0.2.7", 1716).is_empty(), "refused");
        // The thread still takes while it refuses: still no answer.
        take_json(Duration::ZERO);
        assert!(
            receive(&ping, "192.0.2.7", 1716).is_empty(),
            "still refused"
        );
        assert_eq!(take_json(Duration::ZERO), "", "nothing kept");
        // The refusal over.
        *lock(&REFUSED_UNTIL) = None;
        take_json(Duration::ZERO);
        assert_eq!(receive(&ping, "192.0.2.7", 1716).len(), 1, "taking again");
        forget();
    }

    #[test]
    fn a_report_goes_only_when_it_is_one() {
        let report = r#"{"allows":true,"service":false,"width_px":1080,"height_px":2400,
            "width_dp":411,"height_dp":914,"rotation":1}"#;
        let packet = screen_packet(report).unwrap();
        assert_eq!(packet.packet_type, SCREEN);
        assert_eq!(packet.body["rotation"], 1);
        assert!(screen_packet("{}").is_err());
        assert!(screen_packet("not json").is_err());
    }
}
