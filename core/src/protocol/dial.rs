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

/// The number a `tel:` or `callto:` link names (M8), cleaned as
/// [`clean_number`]: the scheme in any case (and `//` after it, as web
/// pages often write `tel://`),
/// percent-escapes decoded. `None` for another scheme, a link with
/// parameters (`;ext=`, `;phone-context=`) or a query, a bad escape, or
/// what [`clean_number`] refuses.
pub fn tel_number(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let (scheme, rest) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("tel") && !scheme.eq_ignore_ascii_case("callto") {
        return None;
    }
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    if rest.contains([';', '?', '#']) {
        return None;
    }
    let decoded = percent_decode(rest)?;
    // Decoded: an escaped `;`, `?`, `*` or `#` is refused too.
    if decoded.contains([';', '?']) {
        return None;
    }
    clean_number(&decoded)
}

/// `s` with its `%XX` escapes decoded; `None` for a bad escape or bytes
/// that are not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
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

    /// M8: a `tel:` or `callto:` link's number, and nothing else.
    #[test]
    fn a_tel_link_s_number() {
        assert_eq!(tel_number("tel:+1-555-0100").as_deref(), Some("+15550100"));
        assert_eq!(tel_number("TEL:+1.555.0100").as_deref(), Some("+15550100"));
        assert_eq!(
            tel_number("tel:%2B1%20(555)%20010-0100").as_deref(),
            Some("+15550100100")
        );
        assert_eq!(
            tel_number("callto://+15550100").as_deref(),
            Some("+15550100")
        );
        assert_eq!(tel_number("callto:5550100").as_deref(), Some("5550100"));
        assert_eq!(tel_number("tel://5550100").as_deref(), Some("5550100"));
        assert_eq!(tel_number(" tel:112 ").as_deref(), Some("112"));
        assert_eq!(tel_number("tel:5550100;ext=12"), None, "parameters");
        assert_eq!(tel_number("tel:5550100;phone-context=example.com"), None);
        assert_eq!(tel_number("tel:5550100?x=1"), None, "a query");
        assert_eq!(
            tel_number("tel:5550100%3Bext=1"),
            None,
            "an escaped parameter"
        );
        assert_eq!(tel_number("tel:*%2306%23"), None, "a code");
        assert_eq!(
            tel_number("tel:%2A%2A21%2A5550100%23"),
            None,
            "escaped stars"
        );
        assert_eq!(tel_number("tel:555%2"), None, "a bad escape");
        assert_eq!(tel_number("tel:555%ZZ0100"), None);
        assert_eq!(tel_number("tel:555%FF0100"), None, "not UTF-8");
        assert_eq!(tel_number("sms:5550100"), None, "another scheme");
        assert_eq!(tel_number("5550100"), None, "no scheme");
        assert_eq!(tel_number("tel:"), None);
    }
}
