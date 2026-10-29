//! The QR pairing link: everything a phone needs to find, trust and pair
//! with this desktop, as one `pocketdesk://pair?...` URI.
//!
//! Keys: `v` (format, 1), `name`, `id` (desktop device id), `a` (addresses,
//! comma-separated, best first), `cp`/`sp` (control/stream ports), `pin`,
//! `fp` (SHA-256 of the desktop's TLS certificate, hex), `ep` (the iroh
//! endpoint JSON, base64url), and `mine=1` on a link for another of the
//! owner's computers that may see and control this one without asking
//! (PC1). Values are percent-encoded.
//!
//! The link itself is not signed: anyone holding it can change it. So
//! `mine` is only a request. The computer that made the link pairs both
//! ways only when its own PIN file says the PIN was minted for that
//! (`pin::FilePin::mine`), and the computer that pastes it only when the
//! link asked: adding `mine=1` to a link, or taking it off, gives a plain
//! one-way pairing.

use std::fmt;
use std::net::Ipv4Addr;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

const PREFIX: &str = "pocketdesk://pair?";
const VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLink {
    pub name: String,
    pub device_id: String,
    /// Best first (see [`order_addresses`]).
    pub addresses: Vec<String>,
    pub control_port: u16,
    pub stream_port: u16,
    pub pin: String,
    /// Hex SHA-256 of the desktop certificate; the phone pins it before it
    /// first connects. Absent when the server has not created one yet.
    pub fingerprint: Option<String>,
    /// The iroh endpoint JSON, for WAN; absent until the server records it.
    pub endpoint: Option<String>,
    /// The link is for another of the owner's computers, which may see and
    /// control this one without asking, and this one it (PC1). A request
    /// only: see the module's note.
    pub mine: bool,
}

/// Why a link could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLinkError(String);

impl fmt::Display for PairLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PairLinkError {}

fn invalid(reason: impl Into<String>) -> PairLinkError {
    PairLinkError(reason.into())
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b',') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(value: &str) -> Result<String, PairLinkError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = value
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| invalid("bad percent-encoding"))?;
                out.push(hex);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| invalid("not UTF-8"))
}

impl PairLink {
    pub fn to_uri(&self) -> String {
        let mut parts = vec![
            format!("v={VERSION}"),
            format!("name={}", encode(&self.name)),
            format!("id={}", encode(&self.device_id)),
            format!("a={}", encode(&self.addresses.join(","))),
            format!("cp={}", self.control_port),
            format!("sp={}", self.stream_port),
            format!("pin={}", encode(&self.pin)),
        ];
        if let Some(fp) = &self.fingerprint {
            parts.push(format!("fp={}", encode(fp)));
        }
        if let Some(ep) = &self.endpoint {
            parts.push(format!("ep={}", URL_SAFE_NO_PAD.encode(ep)));
        }
        if self.mine {
            parts.push("mine=1".to_string());
        }
        format!("{PREFIX}{}", parts.join("&"))
    }

    pub fn from_uri(uri: &str) -> Result<Self, PairLinkError> {
        let query = clean_link(uri)
            .strip_prefix(PREFIX)
            .ok_or_else(|| invalid("not a Pocketdesk pairing link"))?;
        let mut fields = std::collections::HashMap::new();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            fields.insert(key.to_string(), decode(value)?);
        }
        if fields.get("v").map(String::as_str) != Some(VERSION) {
            return Err(invalid("unsupported pairing link version"));
        }
        let pin = fields.remove("pin").unwrap_or_default();
        if !super::pin::is_valid(&pin) {
            return Err(invalid("the PIN must be 6 digits"));
        }
        let addresses: Vec<String> = fields
            .remove("a")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect();
        if addresses.is_empty() {
            return Err(invalid("the link names no address"));
        }
        let port = |key: &str, default: u16| -> Result<u16, PairLinkError> {
            match fields.get(key) {
                None => Ok(default),
                Some(v) => v.parse().map_err(|_| invalid(format!("bad port '{key}'"))),
            }
        };
        let control_port = port("cp", crate::DEFAULT_CONTROL_PORT)?;
        let stream_port = port("sp", crate::streaming::client::DEFAULT_STREAMING_PORT)?;
        let endpoint = match fields.get("ep") {
            None => None,
            Some(ep) => Some(
                URL_SAFE_NO_PAD
                    .decode(ep)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .ok_or_else(|| invalid("bad endpoint"))?,
            ),
        };
        Ok(Self {
            name: fields.remove("name").unwrap_or_default(),
            device_id: fields.remove("id").unwrap_or_default(),
            addresses,
            control_port,
            stream_port,
            pin,
            fingerprint: fields.remove("fp"),
            endpoint,
            // Only `1` asks; anything else, or none, is a plain pairing.
            mine: fields.get("mine").map(String::as_str) == Some("1"),
        })
    }
}

/// Addresses a phone should try, best first: home/office LANs, then other
/// private ranges (often virtual adapters: WSL, Hyper-V, Docker), then the
/// tailnet. Loopback, link-local, IPv6 and duplicates are dropped.
pub fn order_addresses(addresses: Vec<String>) -> Vec<String> {
    fn rank(ip: Ipv4Addr) -> Option<u8> {
        let [a, b, ..] = ip.octets();
        if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
            None
        } else if a == 192 && b == 168 {
            Some(0)
        } else if a == 10 {
            Some(1)
        } else if a == 172 && (16..32).contains(&b) {
            Some(2)
        } else if a == 100 && (64..128).contains(&b) {
            Some(4)
        } else {
            Some(3)
        }
    }
    let mut ranked: Vec<(u8, usize, String)> = Vec::new();
    for (i, address) in addresses.into_iter().enumerate() {
        let Ok(ip) = address.parse::<Ipv4Addr>() else {
            continue;
        };
        if ranked.iter().any(|(_, _, a)| *a == address) {
            continue;
        }
        if let Some(r) = rank(ip) {
            ranked.push((r, i, address));
        }
    }
    ranked.sort();
    ranked.into_iter().map(|(_, _, a)| a).collect()
}

/// A pasted pairing link without what a copy can carry around it unseen:
/// whitespace and line breaks, a byte-order mark, and zero-width spaces and
/// joiners (the owner's first live test pasted one with a byte-order mark,
/// and Pair stayed greyed out). Only the ends are trimmed.
pub fn clean_link(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || invisible(c))
}

/// A character a copy may carry that shows as nothing: the byte-order mark
/// (U+FEFF), zero-width space, non-joiner and joiner (U+200B to U+200D) and
/// the word joiner (U+2060).
fn invisible(c: char) -> bool {
    matches!(u32::from(c), 0xFEFF | 0x200B..=0x200D | 0x2060)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(code: u32) -> char {
        char::from_u32(code).unwrap()
    }

    #[test]
    fn a_pasted_link_is_cleaned_of_what_does_not_show() {
        let link = "pocketdesk://pair?v=1&a=192.0.2.7&pin=123456";
        let bom = ch(0xFEFF);
        for pasted in [
            format!("{bom}{link}"),
            format!("  {link}\r\n"),
            format!("\n{bom}{}{link}{}{}\t", ch(0x200B), ch(0x200D), ch(0x2060)),
            format!("{}{link}{}", ch(0x200C), ch(0x200B)),
        ] {
            assert_eq!(clean_link(&pasted), link, "{pasted:?}");
            assert_eq!(
                PairLink::from_uri(&pasted).unwrap().addresses,
                ["192.0.2.7"],
                "{pasted:?}"
            );
        }
        // Inside the link nothing is touched.
        let inner = format!("pocketdesk://pair?v=1{bom}&a=192.0.2.7");
        assert_eq!(clean_link(&inner), inner);
        assert_eq!(clean_link(&format!("{bom} ")), "");
    }

    fn sample() -> PairLink {
        PairLink {
            name: "Desk & Co".into(),
            device_id: "d-1".into(),
            addresses: vec!["192.168.1.5".into(), "100.80.1.2".into()],
            control_port: 1716,
            stream_port: 4716,
            pin: "012345".into(),
            fingerprint: Some("ab".repeat(32)),
            endpoint: Some(r#"{"endpointId":"x","relayUrls":[],"directAddrs":[]}"#.into()),
            mine: false,
        }
    }

    #[test]
    fn a_link_round_trips_through_its_uri() {
        let link = sample();
        let uri = link.to_uri();
        assert!(uri.starts_with("pocketdesk://pair?v=1&"), "{uri}");
        assert!(!uri.contains(' ') && !uri.contains('{'), "{uri}");
        assert_eq!(PairLink::from_uri(&uri).unwrap(), link);
    }

    /// The phone's parser (android PairLinkTest) reads this exact string;
    /// if the encoding changes, both tests must change together.
    pub(crate) const FIXTURE: &str = "pocketdesk://pair?v=1&name=Desk%20%26%20Co&id=d-1&a=192.168.1.5,100.80.1.2&cp=1716&sp=4716&pin=012345&fp=abababababababababababababababababababababababababababababababab&ep=eyJlbmRwb2ludElkIjoieCIsInJlbGF5VXJscyI6W10sImRpcmVjdEFkZHJzIjpbXX0";

    #[test]
    fn the_encoding_matches_the_cross_language_fixture() {
        assert_eq!(sample().to_uri(), FIXTURE);
    }

    #[test]
    fn optional_parts_may_be_absent() {
        let link = PairLink {
            fingerprint: None,
            endpoint: None,
            ..sample()
        };
        assert_eq!(PairLink::from_uri(&link.to_uri()).unwrap(), link);
    }

    /// PC1: a link for another of the owner's computers says so, and reads
    /// back; one without the word, or with another value, does not ask.
    #[test]
    fn a_link_for_another_of_my_computers_says_so_and_reads_back() {
        let link = PairLink {
            mine: true,
            ..sample()
        };
        let uri = link.to_uri();
        assert!(uri.ends_with("&mine=1"), "{uri}");
        assert_eq!(PairLink::from_uri(&uri).unwrap(), link);
        assert!(!PairLink::from_uri(FIXTURE).unwrap().mine, "an old link");
        assert!(!sample().to_uri().contains("mine"), "said only when asked");
        let other = format!("{FIXTURE}&mine=yes");
        assert!(!PairLink::from_uri(&other).unwrap().mine);
    }

    #[test]
    fn virtual_adapters_and_link_local_are_ordered_or_dropped() {
        let ordered = order_addresses(vec![
            "100.82.1.9".into(),
            "172.20.240.1".into(),
            "169.254.3.4".into(),
            "10.1.2.3".into(),
            "192.168.1.20".into(),
            "127.0.0.1".into(),
            "fe80::1".into(),
            "192.168.1.20".into(),
        ]);
        assert_eq!(
            ordered,
            ["192.168.1.20", "10.1.2.3", "172.20.240.1", "100.82.1.9"]
        );
    }

    #[test]
    fn malformed_links_are_rejected_with_a_reason() {
        assert!(PairLink::from_uri("https://example.com").is_err());
        assert!(PairLink::from_uri("pocketdesk://pair?v=2&a=1.2.3.4&pin=123456").is_err());
        assert!(PairLink::from_uri("pocketdesk://pair?v=1&a=1.2.3.4&pin=12").is_err());
        assert!(
            PairLink::from_uri("pocketdesk://pair?v=1&pin=123456").is_err(),
            "no address"
        );
        assert!(PairLink::from_uri("pocketdesk://pair?v=1&a=1.2.3.4&pin=123456&cp=x").is_err());
        let err = PairLink::from_uri("pocketdesk://pair?v=1&a=1.2.3.4&pin=12").unwrap_err();
        assert!(err.to_string().contains("PIN"), "{err}");
    }
}
