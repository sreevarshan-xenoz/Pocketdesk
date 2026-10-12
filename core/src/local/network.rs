//! This machine's addresses, without an async runtime (the desktop app has
//! none): the adapters' IPs and the Tailscale IP when Tailscale is up.

/// Every adapter address except loopback and link-local.
pub fn local_ips() -> Vec<String> {
    #[cfg(not(target_os = "linux"))]
    {
        if_addrs::get_if_addrs()
            .map(|interfaces| {
                interfaces
                    .iter()
                    .filter(|i| !i.is_loopback() && !i.is_link_local())
                    .map(|i| i.ip().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("hostname")
            .arg("-I")
            .output()
            .map(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter(|ip| {
                        ip.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
                            std::net::IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
                            std::net::IpAddr::V6(v6) => {
                                !v6.is_loopback() && v6.segments()[0] & 0xffc0 != 0xfe80
                            }
                        })
                    })
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// This machine's Tailscale IPv4 (`tailscale ip -4`), when Tailscale runs.
pub fn tailscale_ip() -> Option<String> {
    let out = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_tailscale_ip(&String::from_utf8_lossy(&out.stdout))
}

fn parse_tailscale_ip(raw: &str) -> Option<String> {
    let first = raw.lines().next()?.trim();
    first
        .parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ips_parse_and_skip_loopback_and_link_local() {
        for ip in local_ips() {
            let ip: std::net::IpAddr = ip.parse().expect("an IP address");
            assert!(!ip.is_loopback(), "{ip}");
            if let std::net::IpAddr::V4(v4) = ip {
                assert!(!v4.is_link_local(), "{ip}");
            }
        }
    }

    #[test]
    fn the_tailscale_answer_is_parsed_strictly() {
        assert_eq!(
            parse_tailscale_ip("100.82.1.9\nfd7a::1\n").as_deref(),
            Some("100.82.1.9")
        );
        assert_eq!(
            parse_tailscale_ip("failed to connect to local tailscaled\n"),
            None
        );
        assert_eq!(parse_tailscale_ip(""), None);
    }
}
