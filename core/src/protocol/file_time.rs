//! A file's modified time as it travels with the file (K1): `lastModified`,
//! milliseconds since 1970, on the share request and the offer, as KDE
//! Connect's share packet has it. One rule for every road: none for 0, a
//! negative or a value that is no number, now for a time after now.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The body field that carries it.
pub const FIELD: &str = "lastModified";

/// The time a body's `lastModified` names, held to `now`.
pub fn from_body(body: &serde_json::Value, now: SystemTime) -> Option<SystemTime> {
    let value = body.get(FIELD)?;
    let ms = value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(|f| f as u64)
    })?;
    from_millis(ms, now)
}

/// `ms` since 1970 as a time, held to `now`; none for 0.
pub fn from_millis(ms: u64, now: SystemTime) -> Option<SystemTime> {
    if ms == 0 {
        return None;
    }
    // Past what a time can hold is after now too.
    Some(
        UNIX_EPOCH
            .checked_add(Duration::from_millis(ms))
            .map_or(now, |at| at.min(now)),
    )
}

/// `at` as milliseconds since 1970, for a body; none before 1970.
pub fn to_millis(at: SystemTime) -> Option<u64> {
    let ms = at.duration_since(UNIX_EPOCH).ok()?.as_millis();
    u64::try_from(ms).ok().filter(|ms| *ms > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    #[test]
    fn a_date_is_read_from_its_body() {
        let june = UNIX_EPOCH + Duration::from_millis(1_780_000_000_123);
        assert_eq!(
            from_body(&json!({ "lastModified": 1_780_000_000_123u64 }), now()),
            Some(june)
        );
        assert_eq!(
            from_body(&json!({ "lastModified": 1_780_000_000_123.0 }), now()),
            Some(june),
            "a float, as some ends send"
        );
    }

    #[test]
    fn no_date_or_a_wrong_one_is_none() {
        for body in [
            json!({}),
            json!({ "lastModified": 0 }),
            json!({ "lastModified": -5 }),
            json!({ "lastModified": "yesterday" }),
            json!({ "lastModified": null }),
        ] {
            assert_eq!(from_body(&body, now()), None, "{body}");
        }
    }

    #[test]
    fn a_date_after_now_is_now() {
        assert_eq!(
            from_body(&json!({ "lastModified": u64::MAX }), now()),
            Some(now())
        );
        assert_eq!(
            from_body(&json!({ "lastModified": 1_900_000_000_000u64 }), now()),
            Some(now())
        );
    }

    #[test]
    fn a_time_goes_out_as_milliseconds() {
        let at = UNIX_EPOCH + Duration::from_millis(1_780_000_000_123);
        assert_eq!(to_millis(at), Some(1_780_000_000_123));
        assert_eq!(to_millis(UNIX_EPOCH), None);
        assert_eq!(from_millis(to_millis(at).unwrap(), now()), Some(at));
    }
}
