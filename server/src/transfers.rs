//! The transfers under way, shown in the desktop app and stopped from it (C1).
//!
//! Each download or share `share::running()` runs also has a row here while
//! it runs ([`track`]): which way it goes, the phone, the file, and the bytes
//! so far, counted by the transfer's own progress. [`run`] writes the rows
//! to `transfers.json` for the app and takes the app's cancel requests. A
//! cancelled transfer is stopped as a retry's supersede stops it, and its
//! file is remembered as cancelled ([`is_cancelled`]) so the phone's next
//! attempt is refused with why instead of starting it again; the first
//! refusal uses the memory up, so the same file sent again later goes.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::local::transfers::{self as mirror, Direction, Mirror, Transfer};

/// Why the phone's next attempt at a cancelled file is refused.
pub(crate) const CANCELLED_HERE: &str = "cancelled on the computer";
/// What an offer cancelled here is settled as, in the Sent list.
pub(crate) const OFFER_CANCELLED: &str = "cancelled on this computer";
/// How long a cancelled file is refused, at most.
const CANCELLED_FOR: Duration = Duration::from_secs(600);
/// How many cancelled files are remembered.
const MAX_CANCELLED: usize = 256;
/// The rate is the bytes over about this long.
const RATE_WINDOW: Duration = Duration::from_secs(4);
/// How often the list is rewritten while anything moves.
const TICK: Duration = Duration::from_secs(1);

/// What a transfer shows of itself.
#[derive(Debug, Clone)]
pub(crate) struct Shown {
    /// The key it runs under in `share::running()`.
    pub key: String,
    pub direction: Direction,
    pub device_id: String,
    pub name: String,
    pub folder: Option<String>,
    pub size: u64,
}

struct Row {
    shown: Shown,
    done: Arc<AtomicU64>,
    started_unix: u64,
    /// Readings of `done`, oldest first, over the last [`RATE_WINDOW`].
    samples: VecDeque<(Instant, u64)>,
}

#[derive(Default)]
struct Table {
    rows: HashMap<String, Row>,
    /// Files cancelled here, by key, and when.
    cancelled: HashMap<String, Instant>,
}

fn table() -> &'static Mutex<Table> {
    static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, Table> {
    table().lock().unwrap_or_else(|e| e.into_inner())
}

/// Wakes [`run`] when a row comes, so an idle server need not tick.
fn added() -> &'static tokio::sync::Notify {
    static ADDED: OnceLock<tokio::sync::Notify> = OnceLock::new();
    ADDED.get_or_init(tokio::sync::Notify::new)
}

/// A transfer's row, for as long as this lives.
pub(crate) struct Tracker {
    id: String,
    done: Arc<AtomicU64>,
}

impl Tracker {
    /// The transfer's progress callback: how much of the file the receiving
    /// end has.
    pub(crate) fn progress(&self) -> impl FnMut(u64) + Send + 'static {
        let done = self.done.clone();
        move |n| done.store(n, Ordering::Relaxed)
    }

    #[cfg(test)]
    fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        lock().rows.remove(&self.id);
    }
}

/// Show `shown` until the returned tracker is dropped.
pub(crate) fn track(shown: Shown) -> Tracker {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let done = Arc::new(AtomicU64::new(0));
    lock().rows.insert(
        id.clone(),
        Row {
            shown,
            done: done.clone(),
            started_unix: pocketdesk_core::local::pin::now_unix(),
            samples: VecDeque::new(),
        },
    );
    added().notify_one();
    Tracker { id, done }
}

/// Bytes a second from readings `(when, done)`, oldest first: what moved
/// between the oldest and the newest. Too short a span says nothing yet.
fn rate(samples: &VecDeque<(Instant, u64)>) -> u64 {
    let (Some(&(first_at, first)), Some(&(last_at, last))) = (samples.front(), samples.back())
    else {
        return 0;
    };
    let span = last_at.saturating_duration_since(first_at);
    if span < Duration::from_millis(500) {
        return 0;
    }
    (last.saturating_sub(first) as f64 / span.as_secs_f64()) as u64
}

/// The rows as the app sees them, each read now.
fn rows_at(table: &mut Table, now: Instant) -> Vec<Transfer> {
    let mut rows: Vec<Transfer> = table
        .rows
        .iter_mut()
        .map(|(id, row)| {
            let done = row.done.load(Ordering::Relaxed);
            // A new attempt that resumed reads lower than the last, and the
            // first reading above nothing counts what the other end already
            // had: either way the rate starts over (C1 review).
            if row
                .samples
                .back()
                .is_some_and(|&(_, d)| d > done || (d == 0 && done > 0))
            {
                row.samples.clear();
            }
            row.samples.push_back((now, done));
            while row
                .samples
                .front()
                .is_some_and(|&(at, _)| now.saturating_duration_since(at) > RATE_WINDOW)
            {
                row.samples.pop_front();
            }
            Transfer {
                id: id.clone(),
                direction: row.shown.direction,
                device_id: row.shown.device_id.clone(),
                name: row.shown.name.clone(),
                folder: row.shown.folder.clone(),
                size: row.shown.size,
                // Never more than the file, whatever a counter says.
                done: if row.shown.size > 0 {
                    done.min(row.shown.size)
                } else {
                    done
                },
                rate: rate(&row.samples),
                started_unix: row.started_unix,
            }
        })
        .collect();
    rows.sort_by(|a, b| a.started_unix.cmp(&b.started_unix).then(a.id.cmp(&b.id)));
    rows
}

/// Remember `key` as cancelled at `now`.
fn remember(table: &mut Table, key: String, now: Instant) {
    table
        .cancelled
        .retain(|_, at| now.saturating_duration_since(*at) < CANCELLED_FOR);
    if table.cancelled.len() >= MAX_CANCELLED
        && let Some(oldest) = table
            .cancelled
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(k, _)| k.clone())
    {
        table.cancelled.remove(&oldest);
    }
    table.cancelled.insert(key, now);
}

/// Mark the transfer `id` cancelled: the key it runs under, for the caller
/// to stop it; `None` when it has ended already.
fn cancel_at(table: &mut Table, id: &str, now: Instant) -> Option<String> {
    let row = table.rows.get(id)?;
    // All of it is there: the other end may have kept it already, so it is
    // left to finish (C1 review).
    let size = row.shown.size;
    if size > 0 && row.done.load(Ordering::Relaxed) >= size {
        return None;
    }
    let key = row.shown.key.clone();
    remember(table, key.clone(), now);
    Some(key)
}

/// Whether the file under `key` was cancelled here and its phone not yet
/// told: true once, then forgotten, so the same file sent again later
/// goes.
pub(crate) fn take_cancelled(key: &str) -> bool {
    take_cancelled_at(&mut lock(), key, Instant::now())
}

fn take_cancelled_at(table: &mut Table, key: &str, now: Instant) -> bool {
    table
        .cancelled
        .remove(key)
        .is_some_and(|at| now.saturating_duration_since(at) < CANCELLED_FOR)
}

/// Remember `key` as cancelled, as the app's Cancel does (for handler tests).
#[cfg(test)]
pub(crate) fn cancelled_for_test(key: &str) {
    remember(&mut lock(), key.to_string(), Instant::now());
}

/// Stop the transfer `id` (the app's Cancel), and settle what it was.
async fn cancel(id: &str) {
    let Some(key) = cancel_at(&mut lock(), id, Instant::now()) else {
        tracing::info!("A cancel for a transfer that has ended or is ending");
        return;
    };
    tracing::info!("Cancelling the transfer {key} for the owner");
    let Some(offer) = key.strip_prefix("offer:") else {
        crate::plugins::share::running().supersede(&key).await;
        return;
    };
    // The rest of its folder send goes too (C1 review): each refused with
    // why when the phone asks for it, and settled.
    let siblings = crate::offers::folder_siblings(offer);
    {
        let mut table = lock();
        for sibling in &siblings {
            remember(&mut table, format!("offer:{sibling}"), Instant::now());
        }
    }
    crate::plugins::share::running().supersede(&key).await;
    // An offer's task ended before it could settle it: the Sent list says why.
    crate::offers::fail(offer, OFFER_CANCELLED.to_string());
    for sibling in siblings {
        crate::plugins::share::running()
            .supersede(&format!("offer:{sibling}"))
            .await;
        crate::offers::fail(&sibling, OFFER_CANCELLED.to_string());
    }
}

/// Daemon task: keep `transfers.json` current and take the app's cancels.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        tracing::warn!("No state directory: transfers under way are not shown");
        return;
    };
    // A list a stopped server left is replaced at once.
    let mut written_empty = write(&dir, Vec::new());
    loop {
        let moving = !lock().rows.is_empty();
        if moving {
            crate::inbox::wait_at_most(
                pocketdesk_core::local::paths::TRANSFER_CANCELS_DIR,
                TICK,
                TICK,
            )
            .await;
        } else {
            tokio::select! {
                _ = added().notified() => {}
                _ = crate::inbox::wait(pocketdesk_core::local::paths::TRANSFER_CANCELS_DIR, TICK) => {}
            }
        }
        let now = pocketdesk_core::local::pin::now_unix();
        for request in mirror::take_cancels(&dir, now) {
            cancel(&request.transfer).await;
        }
        let rows = rows_at(&mut lock(), Instant::now());
        if rows.is_empty() {
            if !written_empty {
                written_empty = write(&dir, rows);
            }
        } else {
            write(&dir, rows);
            written_empty = false;
        }
    }
}

/// Write the list; whether it was written.
fn write(dir: &std::path::Path, transfers: Vec<Transfer>) -> bool {
    let mirror = Mirror {
        updated_unix: pocketdesk_core::local::pin::now_unix(),
        transfers,
    };
    match mirror::write(dir, &mirror) {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("Could not write the transfers under way: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(key: &str) -> Shown {
        Shown {
            key: key.to_string(),
            direction: Direction::FromPhone,
            device_id: "phone-1".to_string(),
            name: "film.mkv".to_string(),
            folder: Some("Holiday".to_string()),
            size: 1_000,
        }
    }

    #[test]
    fn a_row_shows_its_progress_while_its_transfer_runs() {
        let tracker = track(shown("share:test-row:a"));
        let mut progress = tracker.progress();
        progress(400);
        let rows = rows_at(&mut lock(), Instant::now());
        let row = rows
            .iter()
            .find(|r| r.id == tracker.id())
            .expect("shown while it runs");
        assert_eq!(row.done, 400);
        assert_eq!(row.size, 1_000);
        assert_eq!(row.folder.as_deref(), Some("Holiday"));
        let id = tracker.id().to_string();
        drop(tracker);
        assert!(
            rows_at(&mut lock(), Instant::now())
                .iter()
                .all(|r| r.id != id),
            "gone once it ends"
        );
    }

    #[tokio::test]
    async fn a_cancel_stops_the_running_transfer_and_refuses_its_retry() {
        let key = "share:test-cancel:file".to_string();
        let tracker = track(shown(&key));
        let id = tracker.id().to_string();
        crate::plugins::share::running()
            .spawn(key.clone(), async move {
                let _tracker = tracker;
                std::future::pending::<()>().await;
            })
            .await;
        cancel(&id).await;
        assert!(
            rows_at(&mut lock(), Instant::now())
                .iter()
                .all(|r| r.id != id),
            "the task was stopped and its row went with it"
        );
        assert!(take_cancelled(&key), "its retry is refused");
        assert!(!take_cancelled(&key), "once");
    }

    #[test]
    fn the_rate_is_what_moved_over_the_last_seconds() {
        let start = Instant::now();
        let mut samples = VecDeque::new();
        assert_eq!(rate(&samples), 0, "nothing read yet");
        samples.push_back((start, 1_000));
        samples.push_back((start + Duration::from_millis(200), 1_500));
        assert_eq!(rate(&samples), 0, "too short to say");
        samples.push_back((start + Duration::from_secs(2), 5_000));
        assert_eq!(rate(&samples), 2_000);
    }

    #[test]
    fn a_resumed_attempt_starts_its_rate_over() {
        let mut table = Table::default();
        let done = Arc::new(AtomicU64::new(800));
        table.rows.insert(
            "r1".to_string(),
            Row {
                shown: shown("share:p:resumed"),
                done: done.clone(),
                started_unix: 1,
                samples: VecDeque::new(),
            },
        );
        let start = Instant::now();
        rows_at(&mut table, start);
        done.store(300, Ordering::Relaxed);
        let rows = rows_at(&mut table, start + Duration::from_secs(1));
        assert_eq!(rows[0].rate, 0, "not negative, not a stale figure");
    }

    #[test]
    fn what_the_other_end_already_had_is_not_speed() {
        let mut table = Table::default();
        let done = Arc::new(AtomicU64::new(0));
        table.rows.insert(
            "r1".to_string(),
            Row {
                shown: shown("share:p:jump"),
                done: done.clone(),
                started_unix: 1,
                samples: VecDeque::new(),
            },
        );
        let start = Instant::now();
        rows_at(&mut table, start);
        rows_at(&mut table, start + Duration::from_secs(1));
        done.store(800_000_000, Ordering::Relaxed);
        let rows = rows_at(&mut table, start + Duration::from_secs(2));
        assert_eq!(rows[0].rate, 0, "the resumed offset is not bytes moved");
        done.store(800_002_000, Ordering::Relaxed);
        let rows = rows_at(&mut table, start + Duration::from_secs(3));
        assert_eq!(rows[0].rate, 2_000);
    }

    #[test]
    fn a_cancelled_file_is_refused_once_then_forgotten() {
        let mut table = Table::default();
        let now = Instant::now();
        table.rows.insert(
            "r1".to_string(),
            Row {
                shown: shown("offer:abc"),
                done: Arc::new(AtomicU64::new(0)),
                started_unix: 1,
                samples: VecDeque::new(),
            },
        );
        assert_eq!(cancel_at(&mut table, "unknown", now), None);
        // All of it there: left to finish, and nothing remembered.
        table.rows.get("r1").unwrap().done.store(1_000, Ordering::Relaxed);
        assert_eq!(cancel_at(&mut table, "r1", now), None);
        assert!(!take_cancelled_at(&mut table, "offer:abc", now));
        table.rows.get("r1").unwrap().done.store(10, Ordering::Relaxed);
        assert_eq!(
            cancel_at(&mut table, "r1", now).as_deref(),
            Some("offer:abc")
        );
        assert!(take_cancelled_at(&mut table, "offer:abc", now));
        assert!(
            !take_cancelled_at(&mut table, "offer:abc", now),
            "the phone was told; a new send of it goes"
        );
    }

    #[test]
    fn a_cancel_is_forgotten_after_a_while_and_the_memory_is_bounded() {
        let mut table = Table::default();
        let now = Instant::now();
        remember(&mut table, "share:p:old".to_string(), now);
        assert!(!take_cancelled_at(
            &mut table,
            "share:p:old",
            now + CANCELLED_FOR
        ));
        for n in 0..(MAX_CANCELLED + 10) {
            remember(
                &mut table,
                format!("share:p:{n}"),
                now + Duration::from_millis(n as u64),
            );
        }
        assert_eq!(table.cancelled.len(), MAX_CANCELLED);
        assert!(
            !table.cancelled.contains_key("share:p:0"),
            "the oldest went first"
        );
    }
}
