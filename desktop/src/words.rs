//! Words the app's pages share, so one thing reads one way on each (U5).

/// How long ago `at` was, from `now` (both Unix seconds): "just now",
/// "45 s ago", "3 min ago", "2 h ago", "4 days ago". A time ahead of the
/// clock is "just now".
pub fn ago(now: u64, at: u64) -> String {
    match now.saturating_sub(at) {
        0..10 => "just now".to_string(),
        s @ 10..60 => format!("{s} s ago"),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s @ 3600..86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "1 day ago".to_string(),
        s => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::ago;

    #[test]
    fn an_age_reads_the_same_on_every_page() {
        assert_eq!(ago(100, 95), "just now");
        assert_eq!(
            ago(100, 200),
            "just now",
            "a clock behind is never negative"
        );
        assert_eq!(ago(145, 100), "45 s ago");
        assert_eq!(ago(1_000, 100), "15 min ago");
        assert_eq!(ago(10_000, 100), "2 h ago");
        assert_eq!(ago(90_000, 0), "1 day ago");
        assert_eq!(ago(300_000, 100), "3 days ago");
    }
}
