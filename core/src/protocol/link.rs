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
    // A port follows the last colon; an IPv6 host keeps its colons inside
    // brackets.
    let host = match authority.rsplit_once(':') {
        Some((host, port))
            if port.chars().all(|c| c.is_ascii_digit())
                && (!host.starts_with('[') || host.ends_with(']')) =>
        {
            host
        }
        _ => authority,
    };
    if host.is_empty() {
        return Some("a link with no host".to_string());
    }
    if is_loopback_host(host) {
        return Some("a link to the machine's own address (localhost)".to_string());
    }
    None
}

/// The host of a link [`link_problem`] accepts (lowercase, port left out),
/// as the event log names it without the rest of the address.
pub fn link_host(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((host, port))
            if port.chars().all(|c| c.is_ascii_digit())
                && (!host.starts_with('[') || host.ends_with(']')) =>
        {
            host
        }
        _ => authority,
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// Whether a browser would take `host` (lowercase) for its own machine:
/// `localhost` and its subdomains, 127/8, 0.0.0.0, `::1`, `::`, in any
/// spelling a browser reads as one of those. A host a browser rewrites
/// (percent-encoding, fullwidth or other compatibility characters) counts
/// too, since it cannot be told apart without the rewrite.
fn is_loopback_host(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    if host.contains('%')
        || host.chars().any(|c| {
            ('\u{ff00}'..='\u{ffef}').contains(&c)
                || c == '\u{3002}'
                || (!c.is_ascii() && c.is_numeric())
        })
    {
        return true;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return match inner.parse::<std::net::Ipv6Addr>() {
            Ok(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip
                        .to_ipv4_mapped()
                        .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
            }
            Err(_) => true,
        };
    }
    // A browser reads a host of numbers (`127.1`, `2130706433`,
    // `0x7f.0.0.1`, `0177.0.0.1`) as an IPv4 address. Only the plain dotted
    // form is let through, and only when it is not this machine.
    let numeric = host.split('.').all(|part| {
        !part.is_empty() && (part.chars().all(|c| c.is_ascii_digit()) || part.starts_with("0x"))
    });
    if numeric {
        return match host.parse::<std::net::Ipv4Addr>() {
            Ok(ip) => ip.is_loopback() || ip.is_unspecified(),
            Err(_) => true,
        };
    }
    false
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

    /// The other end's own loopback is never what a link means, and opening
    /// it there is a request to that machine's local services.
    #[test]
    fn a_link_to_a_loopback_host_or_no_host_is_refused() {
        for bad in [
            "https://:80/",
            "http://localhost:8080/admin",
            "http://LOCALHOST./",
            "http://app.localhost/",
            "http://127.0.0.1/",
            "http://127.9.9.9:3000/",
            "http://127.1/",
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0x7f.0.0.1/",
            "http://0177.0.0.1/",
            "http://0.0.0.0:8000/",
            "http://[::1]:8000/",
            "http://[::]/",
            "http://[::ffff:127.0.0.1]/",
            "http://%6c%6fcalhost/",
            "http://\u{ff11}\u{ff12}\u{ff17}.0.0.1/",
            "http://127\u{3002}0\u{3002}0\u{3002}1/",
        ] {
            assert!(link_problem(bad).is_some(), "{bad:?}");
        }
        for fine in [
            "http://192.0.2.7/",
            "http://[2001:db8::1]:8080/",
            "https://localhost.example.org/",
            "https://cafe.de/",
            "https://1password.com/",
            "https://m\u{fc}nchen.de/",
        ] {
            assert_eq!(link_problem(fine), None, "{fine}");
        }
    }

    #[test]
    fn a_links_host_is_named_without_the_rest() {
        assert_eq!(
            link_host("https://Example.org:8443/a?b#c").as_deref(),
            Some("example.org")
        );
        assert_eq!(
            link_host("http://[2001:db8::1]/x").as_deref(),
            Some("[2001:db8::1]")
        );
        assert_eq!(link_host("ftp://example.org"), None);
    }
}
