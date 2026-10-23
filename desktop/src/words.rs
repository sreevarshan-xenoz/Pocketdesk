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

/// `n` things, one or many: "1 contact", "3 contacts" (Q2 review: "1
/// contacts").
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A device id short enough for a table: the first 12 characters.
pub fn short(id: &str) -> String {
    if id.chars().count() > 12 {
        format!("{}…", id.chars().take(12).collect::<String>())
    } else {
        id.to_string()
    }
}

/// `2_100_000` → "2.0 MB".
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Every word of `query`, case ignored, is in one of `texts` (J4): the
/// pages' search boxes. Nothing typed holds everywhere.
pub fn holds_all(query: &str, texts: &[&str]) -> bool {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    // Nothing typed: nothing lowercased (J4 review: a page's every text was,
    // each frame, for a search no one made).
    if words.is_empty() {
        return true;
    }
    let texts: Vec<String> = texts.iter().map(|t| t.to_lowercase()).collect();
    words
        .iter()
        .all(|word| texts.iter().any(|t| t.contains(word.as_str())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_count_says_one_or_many() {
        assert_eq!(super::count(1, "contact", "contacts"), "1 contact");
        assert_eq!(super::count(0, "contact", "contacts"), "0 contacts");
        assert_eq!(super::count(3, "app", "apps"), "3 apps");
    }

    use super::{ago, holds_all};

    /// J4: a search's words, each anywhere in any of the texts.
    #[test]
    fn a_search_holds_every_word_in_any_text() {
        assert!(holds_all("", &["anything"]));
        assert!(holds_all("   ", &[]), "only spaces is nothing typed");
        assert!(holds_all("PIXEL file", &["From Pixel 8", "a file arrived"]));
        assert!(!holds_all(
            "pixel photo",
            &["From Pixel 8", "a file arrived"]
        ));
        assert!(!holds_all("x", &[]));
    }

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
