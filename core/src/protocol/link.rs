//! Links handed between the phone and the computer (roadmap T8a): the
//! packet, and what a link must be before either end opens it.

/// A link sent either way: `{url}`. Under the Links permission.
pub const LINK: &str = "kdeconnect.pocketdesk.link";

/// The longest link either end takes, in bytes.
pub const MAX_LINK_BYTES: usize = 2048;

/// Why `url` may not be opened, or `None` when it may: an `http` or
/// `https` link of at most [`MAX_LINK_BYTES`], with a host, no user name or
/// password, and no space or control character anywhere. Anything else
/// (`file:`, `javascript:`, an app's own scheme) could do more than show a
/// page, so neither end opens it.
pub fn link_problem(url: &str) -> Option<String> {
    if url.len() > MAX_LINK_BYTES {
        return Some(format!("a link longer than {MAX_LINK_BYTES} bytes"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("a link with a space or a control character".to_string());
    }
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(rest) = lower.strip_prefix("https://") {
        rest
    } else if let Some(rest) = lower.strip_prefix("http://") {
        rest
    } else {
        return Some("not a web link (http or https)".to_string());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return Some("a link with a user name or password".to_string());
    }
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            // An IPv6 host keeps its colons inside brackets.
            if port.chars().all(|c| c.is_ascii_digit()) && !host.is_empty() {
                host
            } else {
                authority
            }
        });
    if host.is_empty() {
        return Some("a link with no host".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_web_link_may_be_opened() {
        for fine in [
            "https://example.org/a?b=c#d",
            "http://192.0.2.7:8080/",
            "HTTPS://Example.org",
        ] {
            assert_eq!(link_problem(fine), None, "{fine}");
        }
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "https://user:pass@example.org/",
            "https://@example.org/",
            "https://",
            "https:///path",
            "https://example.org/a b",
            "https://example.org/\u{0}",
            " https://example.org",
            "",
        ] {
            assert!(link_problem(bad).is_some(), "{bad:?}");
        }
        let long = format!("https://example.org/{}", "a".repeat(MAX_LINK_BYTES));
        assert!(link_problem(&long).is_some());
    }
}
