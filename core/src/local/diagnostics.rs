//! Desktop diagnostics (roadmap F10): checks that turn "it doesn't connect"
//! into a reason and a next step, and a debug report to paste in an issue.
//! The desktop app runs them; everything here is synchronous and takes its
//! ports, paths and state as arguments, so it is tested on loopback.

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::paths::{ENDPOINT_FILE, PID_FILE, SESSIONS_FILE, WAN_STATUS_FILE};

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

/// Whether the server runs, and whether it has had time to open its ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Stopped,
    /// Running for less than [`STARTUP_GRACE`]: it writes its pid file
    /// first and opens its ports after, so a port check now would report a
    /// problem that is not there (and a stream-port probe could get in the
    /// way of the server's own bind).
    Starting,
    Running,
}

/// How long after its pid file a server may still be opening its ports.
pub const STARTUP_GRACE: Duration = Duration::from_secs(30);

/// The firewall as it applies to phones (Windows), from the desktop's query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallState {
    /// Enabled inbound Allow rules for the server that apply to the active
    /// network profile.
    pub allow: usize,
    /// Enabled inbound Block rules for it that apply (a block wins).
    pub block: usize,
    /// The active network profiles, as Windows names them ("Private").
    pub profiles: String,
}

/// What the server's internet path looks like from outside (N1), from its
/// own measurement: yes/no facts only, never an address, since the debug
/// report built on it may be pasted publicly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WanStatus {
    pub unix_secs: u64,
    /// UDP reaches the internet (direct connections are possible at all).
    pub udp: bool,
    /// The public address changes with the destination (a "symmetric" NAT,
    /// which defeats hole punching). `None` until measured.
    pub nat_varies: Option<bool>,
    /// A public IPv4 or IPv6 address was seen.
    pub public_v4: bool,
    pub public_v6: bool,
    /// A sign-in page holds the network.
    pub captive_portal: Option<bool>,
}

/// Record the server's measurement (temp file, then rename).
pub fn write_wan_status(state_dir: &Path, status: &WanStatus) -> std::io::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("{WAN_STATUS_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(status)?)?;
    std::fs::rename(&tmp, state_dir.join(WAN_STATUS_FILE))
}

/// The server's last measurement, if it made one.
pub fn read_wan_status(state_dir: &Path) -> Option<WanStatus> {
    let raw = std::fs::read(state_dir.join(WAN_STATUS_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Can phones away from home connect directly, or only through a relay?
/// And can phones on IPv6 networks reach this computer directly?
pub fn check_outside(status: Option<&WanStatus>, state: ServerState) -> Vec<Finding> {
    let check = "From outside";
    if state != ServerState::Running {
        return vec![finding(
            check,
            Status::Info,
            "not checked: the server is not running yet",
        )];
    }
    let Some(s) = status else {
        return vec![with_hint(
            finding(
                check,
                Status::Info,
                "the server has not measured its network yet",
            ),
            "Run the checks again in a minute.",
        )];
    };
    let mut findings = Vec::new();
    findings.push(if s.captive_portal == Some(true) {
        with_hint(
            finding(check, Status::Warn, "a sign-in page holds this network"),
            "Sign in to the network in a browser.",
        )
    } else if !s.udp {
        with_hint(
            finding(check, Status::Warn, "UDP to the internet is blocked: phones away from home go through the relay (slower)"),
            "A firewall or network policy blocks outgoing UDP.",
        )
    } else {
        match s.nat_varies {
            Some(false) => finding(
                check,
                Status::Good,
                "friendly NAT: phones away from home can usually connect directly",
            ),
            Some(true) => with_hint(
                finding(check, Status::Warn, "strict NAT: phones away from home will usually go through the relay"),
                "Turning on UPnP (or NAT-PMP) on the router lets them connect directly.",
            ),
            None => finding(check, Status::Info, "the NAT type is not measured yet"),
        }
    });
    findings.push(if s.public_v6 {
        finding(
            "IPv6",
            Status::Good,
            "a public IPv6 address: phones on IPv6 networks can connect directly",
        )
    } else {
        finding("IPv6", Status::Info, "no public IPv6 address")
    });
    findings
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

/// The server's state: [`super::server::is_running`], and how old its pid
/// file is.
pub fn server_state(state_dir: &Path) -> ServerState {
    let running = super::server::is_running(state_dir);
    let age = std::fs::metadata(state_dir.join(PID_FILE))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok());
    server_state_from(running, age)
}

fn server_state_from(running: bool, pid_file_age: Option<Duration>) -> ServerState {
    match (running, pid_file_age) {
        (false, _) => ServerState::Stopped,
        (true, Some(age)) if age < STARTUP_GRACE => ServerState::Starting,
        (true, _) => ServerState::Running,
    }
}

const STARTING: &str = "not checked yet: the server is starting";
const AGAIN: &str = "Run the checks again in a moment.";

/// Is the server process running?
pub fn check_server(state: ServerState) -> Finding {
    match state {
        ServerState::Running => finding("Server", Status::Good, "running"),
        ServerState::Starting => with_hint(finding("Server", Status::Info, "starting"), AGAIN),
        ServerState::Stopped => with_hint(
            finding("Server", Status::Bad, "not running"),
            "Start it from the Status tab; phones cannot connect until it runs.",
        ),
    }
}

/// Does the control port answer on this computer?
pub fn check_control_port(port: u16, state: ServerState) -> Finding {
    let check = "Control port";
    match state {
        ServerState::Stopped => {
            return finding(
                check,
                Status::Info,
                format!("TCP {port}: not checked, the server is stopped"),
            );
        }
        ServerState::Starting => {
            return with_hint(
                finding(check, Status::Info, format!("TCP {port}: {STARTING}")),
                AGAIN,
            );
        }
        ServerState::Running => {}
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

/// Is the stream port taken (by the server)? Probed by binding it, which
/// only happens once the server has had time to bind it first.
pub fn check_stream_port(port: u16, state: ServerState) -> Finding {
    let check = "Stream port";
    if state == ServerState::Starting {
        return with_hint(
            finding(check, Status::Info, format!("UDP {port}: {STARTING}")),
            AGAIN,
        );
    }
    let running = state == ServerState::Running;
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

/// Can phones away from home reach this computer? Only once the running
/// server has reached a relay (the file is written before that, and kept
/// after the server stops).
pub fn check_internet_path(state_dir: &Path, state: ServerState) -> Finding {
    let check = "Internet path";
    if state != ServerState::Running {
        return finding(
            check,
            Status::Info,
            "not checked: the server is not running yet",
        );
    }
    let relays = std::fs::read_to_string(state_dir.join(ENDPOINT_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("relayUrls").and_then(|r| r.as_array()).map(Vec::len))
        .unwrap_or(0);
    if relays > 0 {
        finding(
            check,
            Status::Good,
            "ready (a relay is reached; phones away from home can connect)",
        )
    } else {
        with_hint(
            finding(check, Status::Warn, "not ready: no relay reached yet"),
            "It needs internet access; until then only this network works.",
        )
    }
}

/// Read the desktop's firewall query: `allow,block,profiles`.
pub fn parse_firewall(answer: &str) -> Result<FirewallState, String> {
    let mut parts = answer.trim().splitn(3, ',');
    let mut count = || -> Result<usize, String> {
        parts
            .next()
            .and_then(|n| n.trim().parse().ok())
            .ok_or_else(|| format!("an answer that is not a count: {}", answer.trim()))
    };
    let allow = count()?;
    let block = count()?;
    let profiles = parts.next().unwrap_or("").trim().to_string();
    Ok(FirewallState {
        allow,
        block,
        profiles,
    })
}

/// The firewall (Windows): `None` where not checked.
pub fn check_firewall(state: Option<Result<FirewallState, String>>) -> Option<Finding> {
    let check = "Firewall";
    Some(match state? {
        Err(e) => finding(
            check,
            Status::Warn,
            format!("could not read the rules ({e})"),
        ),
        Ok(fw) if fw.block > 0 => with_hint(
            finding(
                check,
                Status::Bad,
                format!("a rule blocks the server on this {} network", fw.profiles),
            ),
            "In Windows Defender Firewall, allow linux-link (or remove its block rule) for this network.",
        ),
        Ok(fw) if fw.allow == 0 => with_hint(
            finding(
                check,
                Status::Bad,
                format!("no rule lets phones in on this {} network", fw.profiles),
            ),
            "Run `linux-link install` (it allows phones on private networks), or allow the control and stream ports for this network.",
        ),
        Ok(fw) => finding(
            check,
            Status::Good,
            format!(
                "{} rules let phones in on this {} network",
                fw.allow, fw.profiles
            ),
        ),
    })
}

/// The newest session record that reads, if any (a torn last line does not
/// hide the ones before it).
pub fn last_session(state_dir: &Path) -> Option<LastSession> {
    let raw = std::fs::read_to_string(state_dir.join(SESSIONS_FILE)).ok()?;
    raw.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .find_map(parse_session)
}

fn parse_session(line: &str) -> Option<LastSession> {
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
/// phone's id is cut to 8 characters. This computer's addresses are
/// included (local ones, and any public or Tailscale ones it has).
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

    const UP: ServerState = ServerState::Running;

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
    fn a_server_that_just_started_is_given_time_to_open_its_ports() {
        assert_eq!(
            server_state_from(false, Some(Duration::from_secs(1))),
            ServerState::Stopped
        );
        assert_eq!(
            server_state_from(true, Some(Duration::from_secs(5))),
            ServerState::Starting
        );
        assert_eq!(
            server_state_from(true, Some(Duration::from_secs(60))),
            ServerState::Running
        );
        assert_eq!(server_state_from(true, None), ServerState::Running);
        // While it starts, neither port is judged (nor the stream port bound).
        let silent = free_tcp_port();
        assert_eq!(
            check_control_port(silent, ServerState::Starting).status,
            Status::Info
        );
        let held = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        drop(held);
        assert_eq!(
            check_stream_port(port, ServerState::Starting).status,
            Status::Info
        );
    }

    #[test]
    fn the_server_check_says_to_start_it() {
        assert_eq!(check_server(UP).status, Status::Good);
        let stopped = check_server(ServerState::Stopped);
        assert_eq!(stopped.status, Status::Bad);
        assert!(stopped.hint.is_some());
    }

    #[test]
    fn the_control_port_must_answer_while_the_server_runs() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(check_control_port(port, UP).status, Status::Good);
        let silent = check_control_port(free_tcp_port(), UP);
        assert_eq!(silent.status, Status::Bad);
        assert!(silent.hint.is_some());
        assert_eq!(
            check_control_port(free_tcp_port(), ServerState::Stopped).status,
            Status::Info,
            "nothing to judge with the server stopped"
        );
    }

    #[test]
    fn the_stream_port_is_taken_while_the_server_runs() {
        let held = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(check_stream_port(port, UP).status, Status::Good);
        drop(held);
        assert_eq!(
            check_stream_port(port, UP).status,
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
    fn the_internet_path_is_ready_once_a_relay_is_reached() {
        let dir = temp_dir("endpoint");
        assert_eq!(
            check_internet_path(&dir, UP).status,
            Status::Warn,
            "no file"
        );
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":[],"directAddrs":[]}"#,
        )
        .unwrap();
        assert_eq!(
            check_internet_path(&dir, UP).status,
            Status::Warn,
            "no relay yet"
        );
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":["https://relay.example/"],"directAddrs":[]}"#,
        )
        .unwrap();
        assert_eq!(check_internet_path(&dir, UP).status, Status::Good);
        assert_eq!(
            check_internet_path(&dir, ServerState::Stopped).status,
            Status::Info,
            "a file left by a stopped server proves nothing"
        );
    }

    #[test]
    fn the_firewall_must_let_phones_in_on_the_network_in_use() {
        assert_eq!(check_firewall(None), None, "not checked off Windows");
        let state = |allow, block| {
            Some(Ok(FirewallState {
                allow,
                block,
                profiles: "Public".into(),
            }))
        };
        assert_eq!(check_firewall(state(2, 0)).unwrap().status, Status::Good);
        let none = check_firewall(state(0, 0)).unwrap();
        assert_eq!(none.status, Status::Bad);
        assert!(none.detail.contains("Public"));
        assert!(none.hint.unwrap().contains("linux-link install"));
        assert_eq!(
            check_firewall(state(2, 1)).unwrap().status,
            Status::Bad,
            "a block wins"
        );
        assert_eq!(
            check_firewall(Some(Err("access denied".into())))
                .unwrap()
                .status,
            Status::Warn
        );
    }

    fn wan(udp: bool, nat_varies: Option<bool>, public_v6: bool) -> WanStatus {
        WanStatus {
            unix_secs: 1,
            udp,
            nat_varies,
            public_v4: true,
            public_v6,
            captive_portal: Some(false),
        }
    }

    #[test]
    fn from_outside_says_whether_phones_connect_directly() {
        let friendly = check_outside(Some(&wan(true, Some(false), false)), UP);
        assert_eq!(friendly[0].status, Status::Good);
        assert_eq!(friendly[1].check, "IPv6");
        assert_eq!(friendly[1].status, Status::Info);
        let strict = check_outside(Some(&wan(true, Some(true), true)), UP);
        assert_eq!(strict[0].status, Status::Warn);
        assert!(strict[0].hint.as_deref().unwrap().contains("UPnP"));
        assert_eq!(
            strict[1].status,
            Status::Good,
            "IPv6 still connects directly"
        );
        assert_eq!(
            check_outside(Some(&wan(false, None, false)), UP)[0].status,
            Status::Warn,
            "no UDP"
        );
        let mut portal = wan(true, Some(false), false);
        portal.captive_portal = Some(true);
        assert!(
            check_outside(Some(&portal), UP)[0]
                .detail
                .contains("sign-in")
        );
        assert_eq!(
            check_outside(None, UP)[0].status,
            Status::Info,
            "not measured yet"
        );
        assert_eq!(check_outside(None, ServerState::Starting).len(), 1);
    }

    #[test]
    fn the_wan_status_is_written_and_read_back() {
        let dir = temp_dir("wan");
        assert_eq!(read_wan_status(&dir), None);
        let status = wan(true, Some(false), true);
        write_wan_status(&dir, &status).unwrap();
        assert_eq!(read_wan_status(&dir), Some(status));
        let raw = std::fs::read_to_string(dir.join(WAN_STATUS_FILE)).unwrap();
        assert!(
            !raw.contains('.') || !raw.contains(':'),
            "no address in it: {raw}"
        );
    }

    #[test]
    fn the_firewall_answer_is_read() {
        assert_eq!(
            parse_firewall("2,0,Private/Public\r\n"),
            Ok(FirewallState {
                allow: 2,
                block: 0,
                profiles: "Private/Public".into()
            })
        );
        assert!(parse_firewall("").is_err());
        assert!(parse_firewall("access denied").is_err());
    }

    const SESSION: &str = r#"{"unix_secs":1700000000,"outcome":"lan_direct","ever_relayed":false,"duration_secs":95,"rtt_avg_ms":4,"goodput_kbps":9000,"device_id":"0d7c3a52-91e4-4b6f-8a2e-5f1c9b3d6e70","rtt_tail":{"count":9,"mean_ms":4,"p50_ms":4,"p90_ms":5,"p95_ms":6,"p99_ms":7,"max_ms":8},"encode_tail":{"count":900,"mean_ms":5,"p50_ms":4,"p90_ms":6,"p95_ms":7,"p99_ms":9,"max_ms":12},"decode_tail":null,"render_tail":null,"e2e_tail":{"count":900,"mean_ms":18,"p50_ms":16,"p90_ms":21,"p95_ms":24,"p99_ms":31,"max_ms":40},"link":null,"encoder":"ffmpeg-sidecar-nvenc"}"#;

    #[test]
    fn the_last_session_is_the_newest_record_that_reads() {
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
        // A torn last line does not hide the one before it.
        std::fs::write(
            dir.join(SESSIONS_FILE),
            format!("{older}\n{{\"unix_secs\":17"),
        )
        .unwrap();
        assert_eq!(last_session(&dir).unwrap().outcome, "wan_relayed");
    }

    #[test]
    fn the_debug_report_says_what_was_found_and_nothing_private() {
        let dir = temp_dir("report");
        std::fs::write(dir.join(SESSIONS_FILE), SESSION).unwrap();
        let report = Report {
            findings: vec![
                check_server(UP),
                check_firewall(Some(Ok(FirewallState {
                    allow: 0,
                    block: 0,
                    profiles: "Public".into(),
                })))
                .unwrap(),
            ],
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
