//! Who drives when several phones are connected (roadmap N7): the rule the
//! server's input task asks before it injects a phone's input. Pure; the
//! caller passes the time.

use std::time::{Duration, Instant};

/// How input from several phones is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// One phone at a time: the first to send input drives, until it has
    /// sent nothing for `idle` or its session ends.
    One { idle: Duration },
    /// Every phone with the Input permission drives (before N7).
    Shared,
}

/// What became of one input packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Inject it. `took` is true when this packet gave the session control.
    Yes { took: bool },
    /// Another session drives: drop it.
    No { driver: u64 },
}

/// The current driver, if any.
#[derive(Debug, Clone)]
pub struct InputControl {
    policy: Policy,
    driver: Option<(u64, Instant)>,
}

impl InputControl {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            driver: None,
        }
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Whether input from `session` at `now` is injected.
    pub fn admit(&mut self, session: u64, now: Instant) -> Admit {
        if self.policy == Policy::Shared {
            return Admit::Yes { took: false };
        }
        match self.driver(now) {
            Some(driver) if driver != session => Admit::No { driver },
            held => {
                self.driver = Some((session, now));
                Admit::Yes {
                    took: held.is_none(),
                }
            }
        }
    }

    /// The desktop hands control to `session`. Returns whether it moved;
    /// with Shared there is nothing to hand over.
    pub fn give(&mut self, session: u64, now: Instant) -> bool {
        if self.policy == Policy::Shared {
            return false;
        }
        let moved = self.driver(now) != Some(session);
        self.driver = Some((session, now));
        moved
    }

    /// `session` ended: it no longer drives. Returns whether it did.
    pub fn release(&mut self, session: u64) -> bool {
        if self.driver.is_some_and(|(driver, _)| driver == session) {
            self.driver = None;
            return true;
        }
        false
    }

    /// The session driving at `now`: `None` when no one is, control has
    /// gone idle, or every phone drives (Shared).
    pub fn driver(&self, now: Instant) -> Option<u64> {
        let Policy::One { idle } = self.policy else {
            return None;
        };
        self.driver
            .filter(|(_, last)| now.saturating_duration_since(*last) < idle)
            .map(|(driver, _)| driver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Duration = Duration::from_secs(10);

    fn one() -> InputControl {
        InputControl::new(Policy::One { idle: IDLE })
    }

    #[test]
    fn the_first_phone_to_send_input_drives_and_the_others_wait() {
        let t = Instant::now();
        let mut control = one();
        assert_eq!(control.admit(1, t), Admit::Yes { took: true });
        assert_eq!(
            control.admit(1, t + Duration::from_secs(1)),
            Admit::Yes { took: false }
        );
        assert_eq!(
            control.admit(2, t + Duration::from_secs(2)),
            Admit::No { driver: 1 }
        );
        assert_eq!(control.driver(t + Duration::from_secs(2)), Some(1));
    }

    #[test]
    fn control_frees_itself_after_the_driver_goes_quiet() {
        let t = Instant::now();
        let mut control = one();
        control.admit(1, t);
        // Input keeps it: idle counts from the driver's last packet.
        control.admit(1, t + Duration::from_secs(8));
        assert_eq!(
            control.admit(2, t + Duration::from_secs(17)),
            Admit::No { driver: 1 }
        );
        assert_eq!(control.driver(t + Duration::from_secs(18)), None);
        assert_eq!(
            control.admit(2, t + Duration::from_secs(18)),
            Admit::Yes { took: true }
        );
    }

    #[test]
    fn the_desktop_can_hand_control_over() {
        let t = Instant::now();
        let mut control = one();
        control.admit(1, t);
        assert!(control.give(2, t));
        assert!(!control.give(2, t), "already driving");
        assert_eq!(control.admit(1, t), Admit::No { driver: 2 });
        assert_eq!(control.admit(2, t), Admit::Yes { took: false });
    }

    #[test]
    fn a_session_that_ends_lets_go() {
        let t = Instant::now();
        let mut control = one();
        control.admit(1, t);
        assert!(!control.release(2), "2 never drove");
        assert!(control.release(1));
        assert_eq!(control.driver(t), None);
        assert_eq!(control.admit(2, t), Admit::Yes { took: true });
    }

    #[test]
    fn shared_lets_everyone_drive() {
        let t = Instant::now();
        let mut control = InputControl::new(Policy::Shared);
        assert_eq!(control.admit(1, t), Admit::Yes { took: false });
        assert_eq!(control.admit(2, t), Admit::Yes { took: false });
        assert_eq!(control.driver(t), None);
        assert!(
            !control.give(2, t),
            "nothing to hand over when everyone drives"
        );
        assert_eq!(control.driver(t), None);
    }
}
