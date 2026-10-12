//! This machine's addresses, without an async runtime (the desktop app has
//! none): the adapters' IPs and the Tailscale IP when Tailscale is up.

use std::time::{Duration, Instant};

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
        output_within("hostname", &["-I"], COMMAND_LIMIT)
            .map(|out| {
                String::from_utf8_lossy(&out)
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
    let out = output_within("tailscale", &["ip", "-4"], COMMAND_LIMIT)?;
    parse_tailscale_ip(&String::from_utf8_lossy(&out))
}

/// How long an address lookup may take before it counts as no answer.
const COMMAND_LIMIT: Duration = Duration::from_secs(2);

/// `program args`' stdout when it exits successfully within `limit`; `None`
/// otherwise, and a child that overruns is killed, so a wedged daemon (a
/// `tailscaled` that stopped answering) cannot hang the caller. On Windows
/// it opens no console: the desktop app is a GUI program, and each run would
/// flash one. Only for commands with a few lines of output, which the pipe
/// holds until the child exits.
fn output_within(program: &str, args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut out = Vec::new();
                child.stdout.take()?.read_to_end(&mut out).ok()?;
                return Some(out);
            }
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
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

    /// A shell for the tests below: `cmd` from System32 on Windows (never by
    /// bare name), `sh` elsewhere.
    fn shell(script: &str) -> (String, Vec<String>) {
        if cfg!(windows) {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            (
                format!(r"{root}\System32\cmd.exe"),
                vec!["/C".into(), script.into()],
            )
        } else {
            ("sh".into(), vec!["-c".into(), script.into()])
        }
    }

    fn run(script: &str, limit: Duration) -> Option<Vec<u8>> {
        let (program, args) = shell(script);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        output_within(&program, &args, limit)
    }

    #[test]
    fn a_command_answers_with_its_output_or_not_at_all() {
        let out = run("echo 100.64.0.9", Duration::from_secs(10)).expect("echo answers");
        assert_eq!(String::from_utf8_lossy(&out).trim(), "100.64.0.9");
        assert!(run("exit 3", Duration::from_secs(10)).is_none(), "failure");
        assert!(output_within("no-such-program-pocketdesk", &[], Duration::from_secs(1)).is_none());
    }

    #[test]
    fn a_command_that_hangs_is_killed_at_the_limit() {
        let hang = if cfg!(windows) {
            "ping -n 4 127.0.0.1"
        } else {
            "sleep 3"
        };
        let started = Instant::now();
        assert!(run(hang, Duration::from_millis(300)).is_none());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "gave up after {:?}",
            started.elapsed()
        );
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
