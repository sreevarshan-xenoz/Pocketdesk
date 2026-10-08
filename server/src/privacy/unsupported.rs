//! Privacy mode where this build has no backend: every action reports why,
//! so the phone shows "unavailable" instead of a grab that never happened.

use std::time::Duration;

/// How long a grab lives without a refresh from the phone (kept identical to
/// the Linux backend so callers need not care which one they got).
pub const GRAB_TTL: Duration = Duration::from_secs(600);

const UNSUPPORTED: &str = "privacy mode is not supported on this OS yet";

/// Nothing to grab: reports the reason.
pub fn grab_input(_ttl: Duration) -> Result<usize, String> {
    Err(UNSUPPORTED.to_string())
}

/// Nothing is ever held, so nothing is released.
pub fn release_input() -> usize {
    0
}

/// `(held device count, seconds left before auto-release)` — always idle.
pub fn privacy_status() -> (usize, Option<u64>) {
    (0, None)
}

/// No locker backend: reports the reason.
pub fn lock_screen() -> Result<&'static str, String> {
    Err("screen lock is not supported on this OS yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_reports_instead_of_pretending() {
        assert!(grab_input(GRAB_TTL).is_err());
        assert_eq!(release_input(), 0);
        assert_eq!(privacy_status(), (0, None));
        assert!(lock_screen().is_err());
    }
}
