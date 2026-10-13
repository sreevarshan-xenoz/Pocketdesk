//! Desktop diagnostics (roadmap F10): checks that turn "it doesn't connect"
//! into a reason and a next step, and a debug report safe to paste in an
//! issue. The desktop app runs them; everything here is synchronous and
//! takes its ports and paths as arguments, so it is tested on loopback.

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::Duration;

use super::paths::{ENDPOINT_FILE, SESSIONS_FILE};

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// As it should be.
    Good,
    /// Works, but something is missing or unusual.
    Warn,
    /// This is why phones cannot connect.
    Bad,
    /// Not judged (nothing to compare against).
    Info,
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: &'static str,
    pub status: Status,
    pub detail: String,
    /// What to do about it, when something is wrong.
    pub hint: Option<String>,
}

/// A latency distribution's median and 99th percentile, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tail {
    pub p50_ms: u64,
    pub p99_ms: u64,
}

/// The newest session the server recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSession {
    pub unix_secs: u64,
    pub outcome: String,
    pub duration_secs: u64,
    pub ever_relayed: bool,
    pub encoder: Option<String>,
    /// The phone's id, cut to 8 characters.
    pub device: Option<String>,
    pub e2e: Option<Tail>,
    pub encode: Option<Tail>,
    pub rtt: Option<Tail>,
}

/// Everything the checks found, and what the report needs besides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub last_session: Option<LastSession>,
    pub control_port: u16,
    pub streaming_port: u16,
    pub pairing_required: bool,
}

fn finding(check: &'static str, status: Status, detail: impl Into<String>) -> Finding {
    Finding {
        check,
        status,
        detail: detail.into(),
        hint: None,
    }
}

fn with_hint(mut finding: Finding, hint: impl Into<String>) -> Finding {
    finding.hint = Some(hint.into());
    finding
}

/// Is the server process running?
pub fn check_server(running: bool) -> Finding {
    if running {
        finding("Server", Status::Good, "running")
    } else {
        with_hint(
            finding("Server", Status::Bad, "not running"),
            "Start it from the Status tab; phones cannot connect until it runs.",
        )
    }
}

/// Does the control port answer on this computer?
pub fn check_control_port(port: u16, running: bool) -> Finding {
    let check = "Control port";
    if !running {
        return finding(
            check,
            Status::Info,
            format!("TCP {port}: not checked, the server is stopped"),
        );
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Ok(_) => finding(check, Status::Good, format!("TCP {port} answers")),
        Err(e) => with_hint(
            finding(
                check,
                Status::Bad,
                format!("TCP {port} does not answer ({e})"),
            ),
            "Another program may hold the port, or the server failed to start its listener; see the Logs tab.",
        ),
    }
}

/// Is the stream port taken (by the server)?
pub fn check_stream_port(port: u16, running: bool) -> Finding {
    let check = "Stream port";
    match UdpSocket::bind(("0.0.0.0", port)) {
        Err(_) if running => finding(
            check,
            Status::Good,
            format!("UDP {port} in use by the server"),
        ),
        Err(e) => with_hint(
            finding(
                check,
                Status::Warn,
                format!("UDP {port} is held by another program ({e})"),
            ),
            "The server cannot stream while another program holds it.",
        ),
        Ok(_) if running => with_hint(
            finding(
                check,
                Status::Bad,
                format!("UDP {port} is free: the stream listener is down"),
            ),
            "Restart the server from the Status tab, then check the Logs tab.",
        ),
        Ok(_) => finding(
            check,
            Status::Info,
            format!("UDP {port} is free (the server is stopped)"),
        ),
    }
}

/// The addresses a phone can use to reach this computer.
pub fn check_addresses(ips: &[String]) -> Finding {
    if ips.is_empty() {
        with_hint(
            finding(
                "Addresses",
                Status::Bad,
                "none: this computer has no network address",
            ),
            "Connect it to a network.",
        )
    } else {
        finding("Addresses", Status::Good, ips.join(", "))
    }
}

/// Can phones find this computer over the internet?
pub fn check_internet_path(state_dir: &Path) -> Finding {
    if state_dir.join(ENDPOINT_FILE).is_file() {
        finding(
            "Internet path",
            Status::Good,
            "published (phones away from home can reach it)",
        )
    } else {
        with_hint(
            finding("Internet path", Status::Warn, "not published yet"),
            "It appears once the server has run with internet access; until then only this network works.",
        )
    }
}

/// The firewall (Windows): `None` where not checked, else how many enabled
/// rules let phones in (`linux-link install`'s, or a program rule Windows
/// added), or why they could not be read.
pub fn check_firewall(rules: Option<Result<usize, String>>) -> Option<Finding> {
    let check = "Firewall";
    Some(match rules? {
        Ok(0) => with_hint(
            finding(check, Status::Bad, "no rule lets phones in"),
            "Run `linux-link install` (it adds the rules), or allow the control and stream ports.",
        ),
        Ok(n) => finding(check, Status::Good, format!("{n} rules let phones in")),
        Err(e) => finding(
            check,
            Status::Warn,
            format!("could not read the rules ({e})"),
        ),
    })
}

/// The newest session the server recorded, if any.
pub fn last_session(state_dir: &Path) -> Option<LastSession> {
    let raw = std::fs::read_to_string(state_dir.join(SESSIONS_FILE)).ok()?;
    let line = raw.lines().rev().find(|l| !l.trim().is_empty())?;
    let record: serde_json::Value = serde_json::from_str(line).ok()?;
    let tail = |key: &str| {
        let t = record.get(key)?;
        Some(Tail {
            p50_ms: t.get("p50_ms")?.as_u64()?,
            p99_ms: t.get("p99_ms")?.as_u64()?,
        })
    };
    Some(LastSession {
        unix_secs: record.get("unix_secs")?.as_u64()?,
        outcome: record.get("outcome")?.as_str()?.to_string(),
        duration_secs: record
            .get("duration_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        ever_relayed: record
            .get("ever_relayed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        encoder: record
            .get("encoder")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        device: record
            .get("device_id")
            .and_then(|v| v.as_str())
            .map(|id| id.chars().take(8).collect()),
        e2e: tail("e2e_tail"),
        encode: tail("encode_tail"),
        rtt: tail("rtt_tail"),
    })
}

fn status_mark(status: Status) -> &'static str {
    match status {
        Status::Good => "ok  ",
        Status::Warn => "warn",
        Status::Bad => "FAIL",
        Status::Info => "info",
    }
}

/// Plain text to paste in an issue: what was checked and found, the ports,
/// and the last session. No PINs, keys, fingerprints or file names; a
/// phone's id is cut to 8 characters. Local addresses are included.
pub fn report_text(report: &Report, version: &str, os: &str, now_unix: u64) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(text, "Pocketdesk {version} on {os}");
    let _ = writeln!(
        text,
        "Ports: control {} (TCP), stream {} (UDP); pairing required: {}",
        report.control_port,
        report.streaming_port,
        if report.pairing_required { "yes" } else { "no" }
    );
    let _ = writeln!(text, "\nChecks:");
    for f in &report.findings {
        let _ = writeln!(
            text,
            "  [{}] {}: {}",
            status_mark(f.status),
            f.check,
            f.detail
        );
        if let Some(hint) = &f.hint {
            let _ = writeln!(text, "         -> {hint}");
        }
    }
    let _ = writeln!(text, "\nLast session:");
    match &report.last_session {
        None => {
            let _ = writeln!(text, "  none recorded");
        }
        Some(s) => {
            let ago = now_unix.saturating_sub(s.unix_secs);
            let _ = writeln!(
                text,
                "  {} ago, {} s, {}{}",
                human_ago(ago),
                s.duration_secs,
                s.outcome,
                if s.ever_relayed { " (relayed)" } else { "" }
            );
            let _ = writeln!(
                text,
                "  encoder: {}; phone: {}",
                s.encoder.as_deref().unwrap_or("not recorded"),
                s.device.as_deref().unwrap_or("unknown")
            );
            for (name, tail) in [("e2e", s.e2e), ("encode", s.encode), ("rtt", s.rtt)] {
                if let Some(t) = tail {
                    let _ = writeln!(text, "  {name} p50 {} ms, p99 {} ms", t.p50_ms, t.p99_ms);
                }
            }
        }
    }
    text
}

/// "40 s", "3 min", "2 h", "5 days".
pub fn human_ago(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ll-diag-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A port nothing listens on: bound, read, released.
    fn free_tcp_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn the_server_check_says_to_start_it() {
        assert_eq!(check_server(true).status, Status::Good);
        let stopped = check_server(false);
        assert_eq!(stopped.status, Status::Bad);
        assert!(stopped.hint.is_some());
    }

    #[test]
    fn the_control_port_must_answer_while_the_server_runs() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(check_control_port(port, true).status, Status::Good);
        let silent = check_control_port(free_tcp_port(), true);
        assert_eq!(silent.status, Status::Bad);
        assert!(silent.hint.is_some());
        assert_eq!(
            check_control_port(free_tcp_port(), false).status,
            Status::Info,
            "nothing to judge with the server stopped"
        );
    }

    #[test]
    fn the_stream_port_is_taken_while_the_server_runs() {
        let held = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(check_stream_port(port, true).status, Status::Good);
        drop(held);
        assert_eq!(
            check_stream_port(port, true).status,
            Status::Bad,
            "the listener is down"
        );
    }

    #[test]
    fn no_address_means_no_network() {
        assert_eq!(check_addresses(&[]).status, Status::Bad);
        let found = check_addresses(&["192.0.2.4".to_string(), "100.64.0.9".to_string()]);
        assert_eq!(found.status, Status::Good);
        assert!(found.detail.contains("192.0.2.4") && found.detail.contains("100.64.0.9"));
    }

    #[test]
    fn the_internet_path_needs_the_endpoint_file() {
        let dir = temp_dir("endpoint");
        assert_eq!(check_internet_path(&dir).status, Status::Warn);
        std::fs::write(dir.join(ENDPOINT_FILE), "{}").unwrap();
        assert_eq!(check_internet_path(&dir).status, Status::Good);
    }

    #[test]
    fn the_firewall_needs_the_rules_install_adds() {
        assert_eq!(check_firewall(None), None, "not checked off Windows");
        assert_eq!(check_firewall(Some(Ok(4))).unwrap().status, Status::Good);
        let none = check_firewall(Some(Ok(0))).unwrap();
        assert_eq!(none.status, Status::Bad);
        assert!(none.hint.unwrap().contains("linux-link install"));
        assert_eq!(
            check_firewall(Some(Err("access denied".into())))
                .unwrap()
                .status,
            Status::Warn
        );
    }

    const SESSION: &str = r#"{"unix_secs":1700000000,"outcome":"lan_direct","ever_relayed":false,"duration_secs":95,"rtt_avg_ms":4,"goodput_kbps":9000,"device_id":"0d7c3a52-91e4-4b6f-8a2e-5f1c9b3d6e70","rtt_tail":{"count":9,"mean_ms":4,"p50_ms":4,"p90_ms":5,"p95_ms":6,"p99_ms":7,"max_ms":8},"encode_tail":{"count":900,"mean_ms":5,"p50_ms":4,"p90_ms":6,"p95_ms":7,"p99_ms":9,"max_ms":12},"decode_tail":null,"render_tail":null,"e2e_tail":{"count":900,"mean_ms":18,"p50_ms":16,"p90_ms":21,"p95_ms":24,"p99_ms":31,"max_ms":40},"link":null,"encoder":"ffmpeg-sidecar-nvenc"}"#;

    #[test]
    fn the_last_session_is_the_newest_record() {
        let dir = temp_dir("sessions");
        assert_eq!(last_session(&dir), None);
        let older = SESSION.replace("lan_direct", "wan_relayed");
        std::fs::write(dir.join(SESSIONS_FILE), format!("{older}\n{SESSION}\n\n")).unwrap();
        let last = last_session(&dir).unwrap();
        assert_eq!(last.outcome, "lan_direct");
        assert_eq!(last.duration_secs, 95);
        assert_eq!(last.encoder.as_deref(), Some("ffmpeg-sidecar-nvenc"));
        assert_eq!(
            last.device.as_deref(),
            Some("0d7c3a52"),
            "cut to 8 characters"
        );
        assert_eq!(
            last.e2e,
            Some(Tail {
                p50_ms: 16,
                p99_ms: 31
            })
        );
        assert_eq!(
            last.encode,
            Some(Tail {
                p50_ms: 4,
                p99_ms: 9
            })
        );
        assert_eq!(
            last.rtt,
            Some(Tail {
                p50_ms: 4,
                p99_ms: 7
            })
        );
    }

    #[test]
    fn the_debug_report_says_what_was_found_and_nothing_private() {
        let dir = temp_dir("report");
        std::fs::write(dir.join(SESSIONS_FILE), SESSION).unwrap();
        let report = Report {
            findings: vec![check_server(true), check_firewall(Some(Ok(0))).unwrap()],
            last_session: last_session(&dir),
            control_port: 1716,
            streaming_port: 4716,
            pairing_required: true,
        };
        let text = report_text(&report, "0.2.0", "windows", 1_700_000_100);
        for expected in [
            "Pocketdesk 0.2.0",
            "windows",
            "control 1716",
            "stream 4716",
            "pairing required: yes",
            "Server",
            "Firewall",
            "linux-link install",
            "ffmpeg-sidecar-nvenc",
            "e2e p50 16 ms, p99 31 ms",
            "0d7c3a52",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        assert!(!text.contains("0d7c3a52-91e4"), "a whole device id");
    }
}
