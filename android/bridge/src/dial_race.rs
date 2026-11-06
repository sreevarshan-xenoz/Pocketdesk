//! The home and internet dials of a session, raced (BG6).
//!
//! A phone that left its Wi-Fi tried the home road first and waited out its
//! dial (10 s for a QUIC handshake to an address nothing answers) before the
//! internet road was even started, so a session from away, or back after a
//! network change, took that long to come up. Now both are started: home at
//! once, the internet road after a short head start, so a computer on the
//! same network is still reached at home without a relay or the DHT hearing
//! of it. The first to give a session wins; the other is dropped where it
//! stands (a dial dropped mid-handshake closes with it) and never installs.

use std::future::Future;
use std::time::Duration;

/// How long home has to answer before the internet road starts as well. A
/// handshake on the same network takes tens of milliseconds, so the road
/// through the relays is only used when home is slow or silent.
pub(crate) const INTERNET_HEAD_START: Duration = Duration::from_millis(300);

/// Which road a session came up by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Road {
    Home,
    Internet,
}

/// Why neither road gave a session: the reason each gave.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BothFailed<E> {
    pub home: E,
    pub internet: E,
}

/// Run `home` and `internet`, the internet one starting after `head_start`
/// or as soon as home has failed, and return the first session either
/// gives, with the road it came by. When both fail, both reasons. Home wins
/// a tie. The dial that does not win is dropped before it can finish.
pub(crate) async fn race<T, E>(
    home: impl Future<Output = Result<T, E>>,
    internet: impl Future<Output = Result<T, E>>,
    head_start: Duration,
) -> Result<(Road, T), BothFailed<E>> {
    tokio::pin!(home);
    tokio::pin!(internet);
    let head = tokio::time::sleep(head_start);
    tokio::pin!(head);
    let mut internet_started = false;
    let mut home_failed: Option<E> = None;
    let mut internet_failed: Option<E> = None;
    loop {
        // Home failing starts the other at once: nothing is left to prefer.
        if home_failed.is_some() {
            internet_started = true;
        }
        tokio::select! {
            biased;
            dialed = &mut home, if home_failed.is_none() => match dialed {
                Ok(session) => return Ok((Road::Home, session)),
                Err(reason) => home_failed = Some(reason),
            },
            () = &mut head, if !internet_started => internet_started = true,
            dialed = &mut internet, if internet_started && internet_failed.is_none() => {
                match dialed {
                    Ok(session) => return Ok((Road::Internet, session)),
                    Err(reason) => internet_failed = Some(reason),
                }
            }
        }
        match (home_failed.take(), internet_failed.take()) {
            (Some(home), Some(internet)) => return Err(BothFailed { home, internet }),
            (home, internet) => {
                home_failed = home;
                internet_failed = internet;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::time::{Instant, sleep};

    /// A dial that gives `outcome` after `after`, and says when it is dropped
    /// before it got there.
    fn dial(
        after: Duration,
        outcome: Result<&'static str, &'static str>,
        dropped: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
    ) -> impl Future<Output = Result<&'static str, String>> {
        struct DropFlag(Arc<AtomicBool>, bool);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                if !self.1 {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
        }
        async move {
            started.store(true, Ordering::SeqCst);
            let mut flag = DropFlag(dropped, false);
            sleep(after).await;
            flag.1 = true;
            outcome.map_err(String::from)
        }
    }

    fn flags() -> (Arc<AtomicBool>, Arc<AtomicBool>) {
        (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    const SECS: fn(u64) -> Duration = Duration::from_secs;
    const MS: fn(u64) -> Duration = Duration::from_millis;

    /// A computer on the same network is reached at home, and the internet
    /// road, which was never started, is never dialed.
    #[tokio::test(start_paused = true)]
    async fn home_answering_at_once_wins_and_the_internet_is_not_started() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let won = race(
            dial(MS(40), Ok("home"), home_dropped, home_started),
            dial(SECS(1), Ok("net"), net_dropped, net_started.clone()),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Home, "home")));
        assert_eq!(began.elapsed(), MS(40));
        assert!(!net_started.load(Ordering::SeqCst), "never dialed");
    }

    /// Home slow past the head start: both run, and the internet road that
    /// answers first wins; home, still dialing, is dropped.
    #[tokio::test(start_paused = true)]
    async fn the_internet_answering_first_wins_and_home_is_dropped() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let won = race(
            dial(SECS(10), Ok("home"), home_dropped.clone(), home_started),
            dial(SECS(1), Ok("net"), net_dropped.clone(), net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Internet, "net")));
        // The head start, then the internet road's own dial: not 10 s.
        assert_eq!(began.elapsed(), INTERNET_HEAD_START + SECS(1));
        assert!(home_dropped.load(Ordering::SeqCst), "home was closed");
        assert!(!net_dropped.load(Ordering::SeqCst));
    }

    /// Both roads are up and home is the quicker: home wins, the internet
    /// road is dropped.
    #[tokio::test(start_paused = true)]
    async fn home_answering_after_the_head_start_still_beats_a_slower_internet() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let won = race(
            dial(MS(500), Ok("home"), home_dropped, home_started),
            dial(SECS(2), Ok("net"), net_dropped.clone(), net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Home, "home")));
        assert!(
            net_dropped.load(Ordering::SeqCst),
            "the internet dial was closed"
        );
    }

    /// Home refusing at once starts the internet road without the head
    /// start: nothing is left to prefer.
    #[tokio::test(start_paused = true)]
    async fn home_failing_starts_the_internet_road_at_once() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let won = race(
            dial(MS(20), Err("refused"), home_dropped, home_started),
            dial(SECS(1), Ok("net"), net_dropped, net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Internet, "net")));
        assert_eq!(began.elapsed(), MS(20) + SECS(1));
    }

    /// The internet road failing leaves home to finish, as slow as it is.
    #[tokio::test(start_paused = true)]
    async fn the_internet_failing_leaves_home_to_finish() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let won = race(
            dial(SECS(4), Ok("home"), home_dropped, home_started),
            dial(MS(100), Err("no relay"), net_dropped, net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Home, "home")));
        assert_eq!(began.elapsed(), SECS(4));
    }

    /// Neither gives a session: both reasons come back, once the later one
    /// has failed (not the sum of the two).
    #[tokio::test(start_paused = true)]
    async fn both_failing_says_why_each_did() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let lost = race(
            dial(SECS(10), Err("home timed out"), home_dropped, home_started),
            dial(SECS(3), Err("relay refused"), net_dropped, net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(
            lost,
            Err(BothFailed {
                home: "home timed out".to_string(),
                internet: "relay refused".to_string(),
            })
        );
        assert_eq!(began.elapsed(), SECS(10), "the slower failure ends it");
    }

    /// What the race saves: with home silent for its 10 s handshake timeout
    /// and the internet road up in 1.5 s, a session that took 11.5 s one road
    /// after the other comes up in 1.8 s.
    #[tokio::test(start_paused = true)]
    async fn a_silent_home_costs_the_head_start_not_the_handshake_timeout() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let began = Instant::now();
        let sequential = SECS(10) + MS(1500);
        let won = race(
            dial(SECS(10), Err("timed out"), home_dropped, home_started),
            dial(MS(1500), Ok("net"), net_dropped, net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Internet, "net")));
        assert_eq!(began.elapsed(), MS(1800));
        assert!(began.elapsed() * 6 < sequential);
    }

    /// Both answer in the same instant: home wins the tie.
    #[tokio::test(start_paused = true)]
    async fn a_tie_goes_to_home() {
        let (home_dropped, home_started) = flags();
        let (net_dropped, net_started) = flags();
        let won = race(
            dial(
                INTERNET_HEAD_START + MS(500),
                Ok("home"),
                home_dropped,
                home_started,
            ),
            dial(MS(500), Ok("net"), net_dropped, net_started),
            INTERNET_HEAD_START,
        )
        .await;
        assert_eq!(won, Ok((Road::Home, "home")));
    }
}
