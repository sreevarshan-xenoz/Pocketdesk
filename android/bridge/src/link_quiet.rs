//! How long a session's link has been quiet (I1b): the time since the count
//! of datagrams the phone has received last rose. While the computer lives
//! it acknowledges what a remote sends (its link report every 5 s), so the
//! count rises; a computer gone leaves it still. A remote has no frames for
//! a stall watch, and the transport's idle timeout is 45 s.

use std::time::{Duration, Instant};

/// The count last seen and when it last rose.
#[derive(Debug, Default)]
pub struct Quiet {
    last: Option<(u64, Instant)>,
}

impl Quiet {
    /// Note `received`, the datagrams received so far, at `now`: how long
    /// since the count last changed. A change either way (a new
    /// connection counts from 0) starts it over.
    pub fn note(&mut self, received: u64, now: Instant) -> Duration {
        match self.last {
            Some((count, since)) if count == received => now.saturating_duration_since(since),
            _ => {
                self.last = Some((received, now));
                Duration::ZERO
            }
        }
    }

    /// No session: nothing has been quiet.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rising_count_is_never_quiet_and_a_still_one_ages() {
        let start = Instant::now();
        let mut quiet = Quiet::default();
        assert_eq!(quiet.note(10, start), Duration::ZERO);
        assert_eq!(
            quiet.note(14, start + Duration::from_secs(5)),
            Duration::ZERO,
            "it rose"
        );
        assert_eq!(
            quiet.note(14, start + Duration::from_secs(17)),
            Duration::from_secs(12),
            "still since it last rose"
        );
    }

    #[test]
    fn a_new_connection_counting_from_zero_starts_over() {
        let start = Instant::now();
        let mut quiet = Quiet::default();
        quiet.note(500, start);
        assert_eq!(
            quiet.note(3, start + Duration::from_secs(20)),
            Duration::ZERO
        );
        quiet.reset();
        assert_eq!(
            quiet.note(3, start + Duration::from_secs(40)),
            Duration::ZERO,
            "reset forgets"
        );
    }
}
