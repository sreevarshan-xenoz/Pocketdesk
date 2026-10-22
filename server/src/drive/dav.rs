//! The WebDAV the drive speaks (D8), pure: a request's head, a path's
//! parts, and the multistatus a `PROPFIND` answers. Class 1, and class 2's
//! answers with writing on (N6).

use std::fmt::Write as _;

/// A request's head, at most.
pub const MAX_HEAD: usize = 16 * 1024;

/// A request's body, at most (a `PROPFIND` names properties; it is read
/// and set aside: every answer carries the same few).
pub const MAX_BODY: u64 = 64 * 1024;

/// A request as far as the drive reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The path, before decoding, without a query.
    pub path: String,
    /// `Depth`: 0, or 1 for anything else (`infinity` is answered as 1).
    pub depth: u8,
    pub content_length: u64,
    /// Whether `Content-Length` was given (a copy needs it, N6).
    pub has_length: bool,
    pub chunked: bool,
    /// `Range`, as sent.
    pub range: Option<String>,
}

/// The head of a request, `None` when it does not read.
pub fn parse_head(head: &[u8]) -> Option<Request> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    if !first.next()?.starts_with("HTTP/1.") || method.is_empty() {
        return None;
    }
    // An absolute target names this host: only its path counts.
    let target = match target.find("://") {
        Some(i) => {
            &target[target[i + 3..]
                .find('/')
                .map_or(target.len(), |j| i + 3 + j)..]
        }
        None => target,
    };
    let path = target.split(['?', '#']).next().unwrap_or("");
    if !path.starts_with('/') {
        return None;
    }
    let mut request = Request {
        method,
        path: path.to_string(),
        depth: 1,
        content_length: 0,
        has_length: false,
        chunked: false,
        range: None,
    };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "depth" => request.depth = u8::from(value != "0"),
            "content-length" => {
                request.content_length = value.parse().ok()?;
                request.has_length = true;
            }
            "transfer-encoding" => request.chunked = !value.eq_ignore_ascii_case("identity"),
            "range" => request.range = Some(value.to_string()),
            _ => {}
        }
    }
    Some(request)
}

/// The bytes a `Range` header asks of a file of `size` bytes, first and
/// last included: `None` for the whole file (no header, or one this does
/// not take: units other than bytes, several ranges); `Some(None)` when it
/// asks for nothing the file has (416).
pub fn byte_range(header: Option<&str>, size: u64) -> Option<Option<(u64, u64)>> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let range = if start.is_empty() {
        // The last `n` bytes.
        let n: u64 = end.parse().ok()?;
        (n > 0 && size > 0).then(|| (size.saturating_sub(n), size - 1))
    } else {
        let first: u64 = start.parse().ok()?;
        let last = if end.is_empty() {
            size.saturating_sub(1)
        } else {
            end.parse::<u64>().ok()?.min(size.saturating_sub(1))
        };
        (first < size && first <= last).then_some((first, last))
    };
    Some(range)
}

/// The decoded parts of `path`, empty ones dropped; `None` when a part does
/// not decode to UTF-8, or names a separator, `.` or `..`, or a control.
pub fn segments(path: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for raw in path.split('/').filter(|p| !p.is_empty()) {
        let bytes = percent_decode(raw)?;
        let part = String::from_utf8(bytes).ok()?;
        if part == "."
            || part == ".."
            || part
                .chars()
                .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return None;
        }
        parts.push(part);
    }
    Some(parts)
}

fn percent_decode(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

/// `parts` as an href: each part percent-encoded but for the unreserved
/// characters, a collection's ending in `/`.
pub fn href(parts: &[String], collection: bool) -> String {
    let mut out = String::from("/");
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        for b in part.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                out.push(b as char);
            } else {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    if collection && !parts.is_empty() {
        out.push('/');
    }
    out
}

/// One resource a multistatus describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub href: String,
    pub name: String,
    /// `None`: a collection; else a file's size.
    pub size: Option<u64>,
    /// Unix seconds, 0 when unknown.
    pub modified: u64,
}

/// The `207 Multi-Status` body describing `props`.
pub fn multistatus(props: &[Prop]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">\n",
    );
    for prop in props {
        let _ = write!(
            out,
            "<D:response><D:href>{}</D:href><D:propstat><D:prop><D:displayname>{}</D:displayname>",
            escape(&prop.href),
            escape(&prop.name)
        );
        match prop.size {
            None => out.push_str("<D:resourcetype><D:collection/></D:resourcetype>"),
            Some(size) => {
                let _ = write!(
                    out,
                    "<D:resourcetype/><D:getcontentlength>{size}</D:getcontentlength><D:getcontenttype>application/octet-stream</D:getcontenttype>"
                );
            }
        }
        if prop.modified != 0 {
            let _ = write!(
                out,
                "<D:getlastmodified>{}</D:getlastmodified>",
                http_date(prop.modified)
            );
        }
        out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>\n");
    }
    out.push_str("</D:multistatus>\n");
    out
}

/// `text` safe inside an XML element.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Unix seconds as HTTP writes a date: `Sun, 06 Nov 1994 08:49:37 GMT`.
pub fn http_date(unix: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = unix / 86_400;
    let secs = unix % 86_400;
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[(days % 7) as usize],
        MONTHS[(month - 1) as usize],
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// A response's head, `Connection: close`: one request per connection.
pub fn head(status: u16, headers: &[(&str, String)], length: u64) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        416 => "Range Not Satisfiable",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let mut out =
        format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {length}\r\nConnection: close\r\n");
    for (name, value) in headers {
        let _ = write!(out, "{name}: {value}\r\n");
    }
    out.push_str("\r\n");
    out
}

/// A `LOCK`'s answer for `token` (N6): exclusive, write, an hour.
pub fn lock_discovery(token: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:prop xmlns:D=\"DAV:\"><D:lockdiscovery><D:activelock>\
         <D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope>\
         <D:depth>0</D:depth><D:timeout>Second-3600</D:timeout>\
         <D:locktoken><D:href>{}</D:href></D:locktoken></D:activelock></D:lockdiscovery></D:prop>",
        escape(token)
    )
}

/// A `PROPPATCH`'s answer for `href` (N6): what was set is taken.
pub fn props_set(href: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\"><D:response>\
         <D:href>{}</D:href><D:propstat><D:prop/><D:status>HTTP/1.1 200 OK</D:status></D:propstat>\
         </D:response></D:multistatus>",
        escape(href)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_reads_its_method_path_and_depth() {
        let r = parse_head(
            b"PROPFIND /t/a%20b/?x=1 HTTP/1.1\r\nHost: localhost\r\nDepth: 0\r\nContent-Length: 12\r\n",
        )
        .unwrap();
        assert_eq!(
            r,
            Request {
                method: "PROPFIND".into(),
                path: "/t/a%20b/".into(),
                depth: 0,
                content_length: 12,
                has_length: true,
                chunked: false,
                range: None,
            }
        );
        // N6: a copy needs its length said.
        assert!(!parse_head(b"PUT /x HTTP/1.1\r\n").unwrap().has_length);
        let r = parse_head(b"GET http://127.0.0.1:9/t/x HTTP/1.1\r\nDepth: infinity\r\n").unwrap();
        assert_eq!((r.path.as_str(), r.depth), ("/t/x", 1));
        assert!(
            parse_head(b"GET /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n")
                .unwrap()
                .chunked
        );
        for bad in [
            &b"GET /x\r\n"[..],
            b"GET x HTTP/1.1\r\n",
            b"\xff\xfe",
            b"GET /x SPDY/3\r\n",
        ] {
            assert!(parse_head(bad).is_none(), "{bad:?}");
        }
        assert!(parse_head(b"GET /x HTTP/1.1\r\nContent-Length: lots\r\n").is_none());
    }

    #[test]
    fn a_range_is_one_span_of_bytes_within_the_file() {
        assert_eq!(byte_range(None, 10), None);
        assert_eq!(byte_range(Some("bytes=2-5"), 10), Some(Some((2, 5))));
        assert_eq!(byte_range(Some("bytes=7-"), 10), Some(Some((7, 9))));
        assert_eq!(byte_range(Some("bytes=-3"), 10), Some(Some((7, 9))));
        assert_eq!(byte_range(Some("bytes=8-100"), 10), Some(Some((8, 9))));
        assert_eq!(
            byte_range(Some("bytes=10-"), 10),
            Some(None),
            "past the end"
        );
        assert_eq!(byte_range(Some("bytes=5-2"), 10), Some(None));
        assert_eq!(
            byte_range(Some("bytes=0-1,4-5"), 10),
            None,
            "several: whole"
        );
        assert_eq!(byte_range(Some("items=0-1"), 10), None);
        assert_eq!(byte_range(Some("bytes=x-1"), 10), None);
    }

    #[test]
    fn a_path_decodes_part_by_part_and_never_climbs() {
        assert_eq!(
            segments("/tok//Pixel%208/DCIM/caf%C3%A9.jpg").unwrap(),
            ["tok", "Pixel 8", "DCIM", "café.jpg"]
        );
        assert_eq!(segments("/").unwrap(), Vec::<String>::new());
        for bad in [
            "/a/..",
            "/a/%2E%2E",
            "/a%2Fb",
            "/a%5Cb",
            "/%FF",
            "/a%0Ab",
            "/%4",
        ] {
            assert!(segments(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn an_href_encodes_each_part() {
        let parts = vec![
            "tok".to_string(),
            "Pixel 8".to_string(),
            "café & co".to_string(),
        ];
        assert_eq!(href(&parts, true), "/tok/Pixel%208/caf%C3%A9%20%26%20co/");
        assert_eq!(href(&parts[..1], false), "/tok");
        assert_eq!(href(&[], true), "/");
        assert_eq!(segments(&href(&parts, true)).unwrap(), parts);
    }

    #[test]
    fn a_multistatus_describes_folders_and_files_escaped() {
        let body = multistatus(&[
            Prop {
                href: "/t/".into(),
                name: "t".into(),
                size: None,
                modified: 0,
            },
            Prop {
                href: "/t/a%3Cb".into(),
                name: "a<b & \"c\"".into(),
                size: Some(42),
                modified: 784_111_777,
            },
        ]);
        assert!(body.contains("<D:resourcetype><D:collection/></D:resourcetype>"));
        assert!(body.contains("<D:displayname>a&lt;b &amp; &quot;c&quot;</D:displayname>"));
        assert!(body.contains("<D:getcontentlength>42</D:getcontentlength>"));
        assert!(
            body.contains("<D:getlastmodified>Sun, 06 Nov 1994 08:49:37 GMT</D:getlastmodified>")
        );
        assert_eq!(body.matches("<D:response>").count(), 2);
    }

    #[test]
    fn dates_are_http_dates() {
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(http_date(1_709_164_800), "Thu, 29 Feb 2024 00:00:00 GMT");
    }

    #[test]
    fn a_response_head_closes() {
        let h = head(207, &[("DAV", "1".into())], 5);
        assert!(h.starts_with(
            "HTTP/1.1 207 Multi-Status\r\nContent-Length: 5\r\nConnection: close\r\n"
        ));
        assert!(h.ends_with("DAV: 1\r\n\r\n"));
    }
}
