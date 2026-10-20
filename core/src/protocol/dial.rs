//! A number the computer asks the phone to dial (L4): the packet, and what
//! a number must be before either end takes it. The phone only opens its
//! dialer with it; the owner presses Call.

/// Computer → phone: `{number}`. Under the Dial permission.
pub const DIAL: &str = "kdeconnect.pocketdesk.dial";

/// The most characters a number keeps.
pub const MAX_NUMBER: usize = 32;

/// `number` as the dialer takes it: spaces, dashes, dots and brackets
/// dropped; then digits, with at most one `+` at the start, 3 to
/// [`MAX_NUMBER`] characters. `None` for anything else: letters, a `tel:`
/// with parameters, control characters, and `*` and `#`, which make MMI and
/// USSD codes (L4 review: one tap on Call of `**21*…#` forwards every call).
pub fn clean_number(number: &str) -> Option<String> {
    let kept: String = number
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')'))
        .collect();
    let body = kept.strip_prefix('+').unwrap_or(&kept);
    let ok = (3..=MAX_NUMBER).contains(&kept.chars().count())
        && !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit());
    ok.then_some(kept)
}

/// A dial's body.
pub fn dial_body(number: &str) -> serde_json::Value {
    serde_json::json!({ "number": number })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L4: a number is cleaned of its punctuation, and only a number passes.
    #[test]
    fn only_a_number_is_dialled() {
        assert_eq!(
            clean_number("+1 (555) 010-0100").as_deref(),
            Some("+15550100100")
        );
        assert_eq!(clean_number("*#06#"), None, "a code, not a number");
        assert_eq!(clean_number("**21*5550100#"), None, "call forwarding");
        assert_eq!(clean_number("112").as_deref(), Some("112"));
        assert_eq!(clean_number("12"), None, "too short");
        assert_eq!(clean_number(&"1".repeat(33)), None, "too long");
        assert_eq!(clean_number("1+555"), None, "a plus inside");
        assert_eq!(clean_number("++555"), None);
        assert_eq!(clean_number("555-CALL"), None, "letters");
        assert_eq!(clean_number("555\n0100"), None);
        assert_eq!(clean_number(&"1".repeat(32)).map(|n| n.len()), Some(32));
        assert_eq!(dial_body("112"), serde_json::json!({ "number": "112" }));
    }
}
