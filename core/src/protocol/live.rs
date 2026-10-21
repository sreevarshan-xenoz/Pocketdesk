//! A phone's ongoing update (N4): the progress or timer an ongoing
//! notification shows, carried as `live` in its `kdeconnect.notification`,
//! and the words a card on the computer says of it.

use serde::{Deserialize, Serialize};

/// The largest whole a progress may have: an app's bytes fit, a forged
/// number does not overflow anything.
pub const MAX_WHOLE: u64 = 1 << 40;

/// What an ongoing notification shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    /// How far along, out of [`Live::max`]; 0 with no bar.
    pub progress: u64,
    /// The whole; 0 when it shows no measured bar.
    pub max: u64,
    /// A bar that moves without saying how far.
    pub indeterminate: bool,
    /// A timer's base, Unix ms, when it shows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chronometer: Option<u64>,
    /// Whether the timer counts down to its base.
    #[serde(default)]
    pub countdown: bool,
    /// What the phone says of its timer, in its own clock and words
    /// ("Ends at 14:32"); at most [`MAX_WORDS`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<String>,
}

/// The longest timer words kept.
pub const MAX_WORDS: usize = 60;

impl Live {
    /// The `live` of a notification's body, held to its bounds: `None`
    /// when there is none, or it shows neither a bar nor a timer.
    pub fn from_body(body: &serde_json::Value) -> Option<Self> {
        let live = body.get("live")?.as_object()?;
        let number = |key: &str| live.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
        let max = number("max").min(MAX_WHOLE);
        let live = Self {
            progress: number("progress").min(max),
            max,
            indeterminate: live
                .get("indeterminate")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            chronometer: live
                .get("chronometer")
                .and_then(|v| v.as_u64())
                .filter(|ms| *ms > 0),
            countdown: live
                .get("countdown")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            words: live
                .get("words")
                .and_then(|v| v.as_str())
                .map(|w| {
                    w.chars()
                        .filter(|c| !c.is_control() && !super::kdeconnect::is_invisible_format(*c))
                        .take(MAX_WORDS)
                        .collect::<String>()
                        .trim()
                        .to_string()
                })
                .filter(|w| !w.is_empty()),
        };
        (live.max > 0 || live.indeterminate || live.chronometer.is_some()).then_some(live)
    }

    /// How far along, 0 to 100, for a measured bar.
    pub fn percent(&self) -> Option<u8> {
        (self.max > 0 && !self.indeterminate)
            .then(|| (self.progress.saturating_mul(100) / self.max).min(100) as u8)
    }

    /// What the card says of it at `now_ms`: "42 %", the phone's own words
    /// for a timer ("Ends at 14:32", N4 review: words worked out here froze
    /// as the timer ran), else "8 min left", "Started 5 min ago", "Ended",
    /// or "Under way".
    pub fn status(&self, now_ms: u64) -> String {
        if let Some(percent) = self.percent() {
            return format!("{percent} %");
        }
        if let Some(words) = &self.words {
            return words.clone();
        }
        if let Some(base) = self.chronometer {
            let minutes = |ms: u64| ms / 60_000;
            return if self.countdown {
                if now_ms >= base {
                    return "Ended".to_string();
                }
                match minutes(base - now_ms) {
                    0 => "Under a minute left".to_string(),
                    m => format!("{m} min left"),
                }
            } else {
                match minutes(now_ms.saturating_sub(base)) {
                    0 => "Started just now".to_string(),
                    m => format!("Started {m} min ago"),
                }
            };
        }
        "Under way".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_live_update_is_read_within_its_bounds() {
        let live = Live::from_body(&json!({"live": {"progress": 42, "max": 100}})).unwrap();
        assert_eq!(live.percent(), Some(42));
        let over = Live::from_body(&json!({"live": {"progress": 900, "max": 100}})).unwrap();
        assert_eq!(over.percent(), Some(100), "held to its whole");
        let huge =
            Live::from_body(&json!({"live": {"progress": u64::MAX, "max": u64::MAX}})).unwrap();
        assert_eq!(huge.max, MAX_WHOLE);
        assert_eq!(huge.percent(), Some(100));
        assert!(
            Live::from_body(&json!({"live": {}})).is_none(),
            "neither a bar nor a timer"
        );
        assert!(Live::from_body(&json!({"title": "x"})).is_none());
        assert!(Live::from_body(&json!({"live": {"indeterminate": true}})).is_some());
    }

    #[test]
    fn a_card_says_where_it_is() {
        let now = 10 * 60_000;
        let bar = |p: u64, m: u64| Live {
            progress: p,
            max: m,
            indeterminate: false,
            chronometer: None,
            countdown: false,
            words: None,
        };
        assert_eq!(bar(1, 4).status(now), "25 %");
        let timer = |base: u64, countdown: bool| Live {
            progress: 0,
            max: 0,
            indeterminate: false,
            chronometer: Some(base),
            countdown,
            words: None,
        };
        assert_eq!(
            timer(now - 5 * 60_000, false).status(now),
            "Started 5 min ago"
        );
        assert_eq!(timer(now, false).status(now), "Started just now");
        assert_eq!(timer(now + 8 * 60_000 + 5, true).status(now), "8 min left");
        assert_eq!(timer(now + 30_000, true).status(now), "Under a minute left");
        assert_eq!(timer(now - 1, true).status(now), "Ended");
        let worded = Live {
            words: Some("Ends at 14:32".to_string()),
            ..timer(now + 30_000, true)
        };
        assert_eq!(worded.status(now), "Ends at 14:32");
        let read = Live::from_body(&json!({"live": {"chronometer": 5, "words": "Ends\nat 14:32"}}))
            .unwrap();
        assert_eq!(
            read.words.as_deref(),
            Some("Endsat 14:32"),
            "control characters go"
        );
        let moving = Live {
            indeterminate: true,
            ..bar(0, 0)
        };
        assert_eq!(moving.status(now), "Under way");
    }
}
