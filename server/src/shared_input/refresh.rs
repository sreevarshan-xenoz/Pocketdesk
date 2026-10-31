//! Reads for a driver loop, done on a thread of their own (plan item 3c).
//! Every two seconds the loops read `shared_screens.json`,
//! `shared_layout.json`, `computers.json`, the trust store and the config,
//! asked for the monitors (and on Hyprland its IPC), and every half second
//! what is in front: all on the thread that carries the keyboard and mouse,
//! so a slow disk or a busy compositor showed as a stall in the moves every
//! two seconds. A [`Refresher`] reads on its own thread on its own clock,
//! and the loop takes the newest it read whenever it looks, never waiting.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A value read again every so often on a thread of its own: the newest
/// read waits to be taken, stamped with when its read started.
pub struct Refresher<T> {
    newest: Arc<Mutex<Option<(T, Instant)>>>,
    /// Read again now ([`Refresher::ask`]); dropped, the thread ends after
    /// the read under way.
    asks: mpsc::Sender<()>,
}

impl<T: Send + 'static> Refresher<T> {
    /// `read` run every `every` on a thread named `name`, the first at
    /// once. `None` when the thread could not start (the loop goes on with
    /// what it read last).
    pub fn start(
        name: &str,
        every: Duration,
        mut read: impl FnMut() -> T + Send + 'static,
    ) -> Option<Refresher<T>> {
        let newest: Arc<Mutex<Option<(T, Instant)>>> = Arc::default();
        let (asks, asked) = mpsc::channel::<()>();
        let into = Arc::clone(&newest);
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                loop {
                    let started = Instant::now();
                    let value = read();
                    *into.lock().unwrap_or_else(|e| e.into_inner()) = Some((value, started));
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
            Ok(_) => Some(Refresher { newest, asks }),
            Err(e) => {
                tracing::warn!("Shared input: the reads for the driver did not start: {e}");
                None
            }
        }
    }

    /// The newest value read since the last taken, with when its read
    /// started; never waits on a read.
    pub fn take(&self) -> Option<(T, Instant)> {
        self.newest.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Read again now, not at the next turn.
    pub fn ask(&self) {
        let _ = self.asks.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The first read comes at once, later ones on the clock or when asked;
    /// a take never waits and gives each value once.
    #[test]
    fn reads_on_its_own_thread_and_hands_on_the_newest() {
        let count = Arc::new(AtomicU32::new(0));
        let counting = Arc::clone(&count);
        let refresher = Refresher::start("pd-shared-test", Duration::from_secs(60), move || {
            counting.fetch_add(1, Ordering::SeqCst) + 1
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            if let Some((value, _)) = refresher.take() {
                break value;
            }
            assert!(Instant::now() < deadline, "the first read in time");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(first, 1);
        assert!(refresher.take().is_none(), "each value once");
        refresher.ask();
        let second = loop {
            if let Some((value, _)) = refresher.take() {
                break value;
            }
            assert!(Instant::now() < deadline, "the asked read in time");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(second, 2, "asked, it read again before its minute");
    }

    /// A read that hangs holds up nobody who takes: the loop goes on with
    /// what it had.
    #[test]
    fn a_hung_read_never_holds_up_a_take() {
        let refresher = Refresher::start("pd-shared-test", Duration::from_secs(60), || {
            std::thread::sleep(Duration::from_secs(30));
        })
        .unwrap();
        let started = Instant::now();
        assert!(refresher.take().is_none());
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
