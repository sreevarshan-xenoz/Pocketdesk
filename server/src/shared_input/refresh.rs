//! Reads for a driver loop, done on a thread of their own (plan item 3c).
//! Every two seconds the loops read `shared_screens.json`,
//! `shared_layout.json`, `computers.json`, the trust store and the config,
//! asked for the monitors (and on Hyprland its IPC), and every half second
//! what is in front: all on the thread that carries the keyboard and mouse,
//! so a slow disk or a busy compositor showed as a stall in the moves every
//! two seconds. A [`Refresher`] reads on its own thread on its own clock,
//! and the loop takes the newest it read whenever it looks, never waiting.
//!
//! What only the driver checks (a phone's Keyboard and mouse permission,
//! the clipboard following the mouse) must not ride an old read for ever:
//! the loop asks [`stale`] and fails closed for those once no read has
//! come back for [`STALE_AFTER`] (a read that hangs, or a reader that died,
//! which [`Refresher::died`] says so it can be started again).

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// With no read come back for this long (one should every two seconds),
/// what only the driver lets through is refused until one does: as the
/// receive side's gate fails closed (PR #595).
pub const STALE_AFTER: Duration = Duration::from_secs(5);

/// How long a dropped refresher waits for its thread to end (a read under
/// way finishing) before it lets it go, said in the log.
const JOIN_WAIT: Duration = Duration::from_millis(300);

/// A value read again every so often on a thread of its own: the newest
/// read waits to be taken, stamped with when its read started.
pub struct Refresher<T> {
    newest: Arc<Mutex<Option<(T, Instant)>>>,
    /// When the last read came back (or when the reader started, before
    /// the first): timed from its end, so steady slow reads (2.5 s on a
    /// slow disk) never read as stale between them (as PR #595 learned).
    last: Arc<Mutex<Instant>>,
    /// Read again now ([`Refresher::ask`]); dropped, the thread ends after
    /// the read under way.
    asks: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    name: String,
}

impl<T: Send + 'static> Refresher<T> {
    /// `read` run every `every` on a thread named `name`, the first at
    /// once. A read that panics is logged and the next goes on. `None`
    /// when the thread could not start (the loop treats that as no read
    /// coming back).
    pub fn start(
        name: &str,
        every: Duration,
        mut read: impl FnMut() -> T + Send + 'static,
    ) -> Option<Refresher<T>> {
        let newest: Arc<Mutex<Option<(T, Instant)>>> = Arc::default();
        let last = Arc::new(Mutex::new(Instant::now()));
        let (asks, asked) = mpsc::channel::<()>();
        let (into, last_into) = (Arc::clone(&newest), Arc::clone(&last));
        let thread_name = name.to_string();
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                loop {
                    let started = Instant::now();
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut read)) {
                        Ok(value) => {
                            // The value first: a loop that finds the reads
                            // fresh again finds this read waiting.
                            *into.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some((value, started));
                            *last_into.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                        }
                        Err(panic) => {
                            let why = panic
                                .downcast_ref::<&str>()
                                .map(|s| (*s).to_string())
                                .or_else(|| panic.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "no message".to_string());
                            tracing::warn!(
                                reader = thread_name,
                                "Shared input: a read for the driver failed ({why}); trying again"
                            );
                        }
                    }
                    match asked.recv_timeout(every) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {
                            // Asks that piled up meanwhile are one read.
                            while asked.try_recv().is_ok() {}
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            });
        match spawned {
            Ok(thread) => Some(Refresher {
                newest,
                last,
                asks: Some(asks),
                thread: Some(thread),
                name: name.to_string(),
            }),
            Err(e) => {
                tracing::warn!("Shared input: the reads for the driver did not start: {e}");
                None
            }
        }
    }
}

impl<T> Refresher<T> {
    /// The newest value read since the last taken, with when its read
    /// started; never waits on a read.
    pub fn take(&self) -> Option<(T, Instant)> {
        self.newest.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Read again now, not at the next turn.
    pub fn ask(&self) {
        if let Some(asks) = &self.asks {
            let _ = asks.send(());
        }
    }

    /// Whether a read came back within `within` (or the reader started that
    /// recently).
    pub fn fresh(&self, within: Duration) -> bool {
        self.last
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed()
            < within
    }

    /// Whether its thread ended (it is started again, not waited on).
    pub fn died(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

/// Whether what `reads` brings is too old to let through what only the
/// driver checks: no reader, or no read back within [`STALE_AFTER`].
pub fn stale<T>(reads: Option<&Refresher<T>>) -> bool {
    stale_after(reads, STALE_AFTER)
}

fn stale_after<T>(reads: Option<&Refresher<T>>, after: Duration) -> bool {
    reads.is_none_or(|reads| !reads.fresh(after))
}

/// A loop's view of its reads: what came back since the last turn, and
/// whether they went stale or fresh again (said once in the log each way).
#[derive(Debug)]
pub struct Freshness {
    stale: bool,
    /// [`STALE_AFTER`] (shorter in tests).
    after: Duration,
}

impl Default for Freshness {
    fn default() -> Freshness {
        Freshness {
            stale: false,
            after: STALE_AFTER,
        }
    }
}

impl Freshness {
    /// One turn of the loop (every tick, never only on the layout pass, so
    /// a setting lands within a read and a tick): the newest read, if one
    /// came back, and whether anything changed that wants laying out now
    /// (a read, or the reads going stale or fresh).
    pub fn turn<T>(&mut self, reads: Option<&Refresher<T>>) -> (Option<T>, bool) {
        // Staleness first, then the read: a reader that just came back
        // stored its value before marking itself fresh, so reads found fresh
        // here are taken in this same turn and the gates never reopen on
        // the old read.
        let stale_now = stale_after(reads, self.after);
        let read = reads.and_then(Refresher::take).map(|(value, _)| value);
        let flipped = stale_now != self.stale;
        if flipped {
            self.stale = stale_now;
            if stale_now {
                tracing::warn!(
                    "Shared input: the settings and files could not be read for a while; a \
                     phone and the clipboard following the mouse are left out until they are"
                );
            } else {
                tracing::info!("Shared input: the settings and files are read again");
            }
        }
        let changed = read.is_some() || flipped;
        (read, changed)
    }

    /// Whether what only the driver checks is to be refused now.
    pub fn stale(&self) -> bool {
        self.stale
    }
}

impl<T> Drop for Refresher<T> {
    /// The thread is told to end and waited for a moment: a read under way
    /// finishes first; one that does not is let go, said in the log.
    fn drop(&mut self) {
        drop(self.asks.take());
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + JOIN_WAIT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            tracing::warn!(
                reader = self.name,
                "Shared input: a read for the driver is still under way; let go"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn wait_for<T>(refresher: &Refresher<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((value, _)) = refresher.take() {
                return value;
            }
            assert!(Instant::now() < deadline, "a read in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The first read comes at once, later ones on the clock or when asked;
    /// a take never waits and gives each value once.
    #[test]
    fn reads_on_its_own_thread_and_hands_on_the_newest() {
        let count = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&count);
        let refresher = Refresher::start("test-refresh", Duration::from_secs(60), move || {
            counting.fetch_add(1, Ordering::SeqCst) + 1
        })
        .unwrap();
        assert_eq!(wait_for(&refresher), 1);
        assert!(refresher.take().is_none(), "each value once");
        refresher.ask();
        assert_eq!(
            wait_for(&refresher),
            2,
            "asked, it read again before its minute"
        );
        assert!(refresher.fresh(STALE_AFTER));
        assert!(!stale(Some(&refresher)));
    }

    /// A read that hangs holds up nobody who takes, and once nothing came
    /// back for the limit what it brings is stale (the driver fails closed
    /// for its own gates); dropping it lets the hung thread go.
    #[test]
    fn a_hung_read_never_holds_up_a_take_and_goes_stale() {
        let (release, hold) = mpsc::channel::<()>();
        let hold = Mutex::new(hold);
        let refresher = Refresher::start("test-refresh", Duration::from_secs(60), move || {
            // Until the test drops its end.
            let _ = hold.lock().unwrap().recv();
        })
        .unwrap();
        let started = Instant::now();
        assert!(refresher.take().is_none());
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(refresher.fresh(Duration::from_secs(60)), "just started");
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !refresher.fresh(Duration::from_millis(100)),
            "nothing came back"
        );
        assert!(!refresher.died());
        drop(release);
        drop(refresher);
        assert!(stale::<()>(None), "no reader is stale");
    }

    /// A loop turning every tick takes a changed value within the read's
    /// clock and a tick, not on a later pass (review I2); a reader that
    /// stops coming back turns it stale, said as a change (review I1).
    #[test]
    fn each_turn_takes_a_new_read_and_notices_staleness() {
        let value = Arc::new(AtomicU32::new(1));
        let reading = Arc::clone(&value);
        let refresher = Refresher::start("test-refresh", Duration::from_millis(40), move || {
            reading.load(Ordering::SeqCst)
        })
        .unwrap();
        let mut fresh = Freshness::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = 0;
        while seen != 1 {
            assert!(Instant::now() < deadline);
            seen = fresh.turn(Some(&refresher)).0.unwrap_or(seen);
            std::thread::sleep(Duration::from_millis(10));
        }
        value.store(2, Ordering::SeqCst);
        let changed = Instant::now();
        while seen != 2 {
            assert!(Instant::now() < deadline);
            let (read, laid_out) = fresh.turn(Some(&refresher));
            if let Some(read) = read {
                assert!(laid_out, "a read is a change");
                seen = read;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            changed.elapsed() < Duration::from_millis(40 + 10 + 500),
            "within the read's clock and a tick"
        );
        assert!(!fresh.stale());
        // No reader: stale, said as a change once.
        assert_eq!(fresh.turn::<u32>(None), (None, true));
        assert!(fresh.stale());
        assert_eq!(fresh.turn::<u32>(None), (None, false));
    }

    /// Steady slow reads (2.5 s against a 5 s limit, scaled down here by
    /// ten: 250 ms reads every 200 ms against 500 ms) never read as stale
    /// between them: freshness counts from a read's end (review R2).
    #[test]
    fn steady_slow_reads_stay_fresh() {
        let refresher = Refresher::start("test-refresh", Duration::from_millis(200), || {
            std::thread::sleep(Duration::from_millis(250));
        })
        .unwrap();
        wait_for(&refresher);
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            assert!(
                refresher.fresh(Duration::from_millis(500)),
                "between steady reads"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Coming back from stale, the turn that finds the reads fresh again
    /// also takes the read that made them fresh: no turn reopens the gates
    /// on the old read (review R1).
    #[test]
    fn the_turn_that_turns_fresh_takes_the_new_read() {
        let (release, hold) = mpsc::channel::<()>();
        let hold = Mutex::new(hold);
        let mut first = true;
        let refresher = Refresher::start("test-refresh", Duration::from_millis(5), move || {
            if std::mem::take(&mut first) {
                // The first read hangs until the test lets it go.
                let _ = hold.lock().unwrap().recv();
            }
            7u32
        })
        .unwrap();
        let mut fresh = Freshness {
            stale: false,
            after: Duration::from_millis(50),
        };
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(fresh.turn(Some(&refresher)), (None, true), "stale");
        assert!(fresh.stale());
        drop(release);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline);
            let (read, changed) = fresh.turn(Some(&refresher));
            if !fresh.stale() {
                assert!(changed);
                assert_eq!(read, Some(7), "the read that made them fresh, in that turn");
                break;
            }
            assert!(read.is_none(), "nothing came back yet");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A read that panics is logged and the next goes on: the reader does
    /// not die with it.
    #[test]
    fn a_panicking_read_does_not_end_the_reader() {
        let count = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&count);
        let refresher = Refresher::start("test-refresh", Duration::from_millis(10), move || {
            let n = counting.fetch_add(1, Ordering::SeqCst);
            assert!(n != 0, "the first read fails");
            n
        })
        .unwrap();
        assert!(wait_for(&refresher) >= 1);
        assert!(!refresher.died());
    }
}
