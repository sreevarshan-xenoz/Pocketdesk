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
    /// The public IPv4 address changes with the destination (a "symmetric"
    /// NAT, which defeats hole punching). `None` until measured.
    pub nat_varies: Option<bool>,
    /// A public IPv6 address was seen (an outbound round trip; inbound may
    /// still be firewalled, and hole punching then usually gets through).
    pub public_v6: bool,
    /// A sign-in page holds the network.
    pub captive_portal: Option<bool>,
    /// This computer's record on the Mainline DHT (roadmap N3): what it was
    /// read back holding ("relay", or "addresses" when an IP was among it),
    /// else the `dht_publish` setting while checking or "off"; "empty" when
    /// no address may be published, "unjoined" when the DHT has not started.
    /// Absent from an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dht: Option<String>,
    /// The server read its own record back from the DHT. Absent until its
    /// first check, about a minute after it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dht_found: Option<bool>,
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
pub fn check_outside(
    status: Option<&WanStatus>,
    state: ServerState,
    own_relay: bool,
) -> Vec<Finding> {
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
            "Run the checks again in a minute; if it stays like this, see Internet path above.",
        )];
    };
    let mut findings = Vec::new();
    findings.push(if s.captive_portal == Some(true) {
        with_hint(
            finding(check, Status::Warn, "a sign-in page holds this network"),
            "Sign in to the network in a browser.",
        )
    } else if !s.udp && own_relay {
        // The UDP and NAT facts come from the relay's QUIC address
        // discovery, which a plain relay of your own does not offer.
        finding(
            check,
            Status::Info,
            "not measured: your relay does not offer address discovery (direct connections may still work)",
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
                "this computer's NAT allows direct connections (the phone's network decides the rest)",
            ),
            Some(true) => with_hint(
                finding(check, Status::Warn, "strict NAT: phones away from home may need the relay"),
                "If UPnP or NAT-PMP is off on the router, turning it on lets phones connect directly.",
            ),
            None => finding(check, Status::Info, "the NAT type is not measured yet"),
        }
    });
    findings.push(if s.public_v6 {
        finding(
            "IPv6",
            Status::Good,
            "a public IPv6 address: phones on IPv6 networks can usually connect directly",
        )
    } else {
        finding("IPv6", Status::Info, "no public IPv6 address")
    });
    findings
}

/// Can a phone away from home find this computer by its id on the Mainline
/// DHT (roadmap N3), after its relay or address changed?
pub fn check_dht(status: Option<&WanStatus>, state: ServerState) -> Finding {
    let check = "Found by id";
    if state != ServerState::Running {
        return finding(
            check,
            Status::Info,
            "not checked: the server is not running yet",
        );
    }
    let Some(mode) = status.and_then(|s| s.dht.as_deref()) else {
        return finding(check, Status::Info, "the server has not reported it yet");
    };
    if mode == "off" {
        return finding(
            check,
            Status::Info,
            "off: a phone away from home dials the address it learned at home",
        );
    }
    if mode == "unjoined" {
        return with_hint(
            finding(
                check,
                Status::Warn,
                "not on the Mainline DHT yet: its bootstrap servers do not resolve",
            ),
            "The server retries each minute. If this stays, this computer has no working DNS or internet.",
        );
    }
    if mode == "empty" {
        return with_hint(
            finding(
                check,
                Status::Warn,
                "nothing to publish: no relay with a public name or address, and public addresses are off",
            ),
            "Name your relay by a public name or address in relay_urls, or set dht_publish = \"addresses\" in config.toml.",
        );
    }
    let holds = if mode == "addresses" {
        "with public addresses"
    } else {
        "relay address only"
    };
    match status.and_then(|s| s.dht_found) {
        Some(true) => finding(
            check,
            Status::Good,
            format!("published on the Mainline DHT and read back ({holds})"),
        ),
        Some(false) => with_hint(
            finding(
                check,
                Status::Warn,
                "published, but not read back from the Mainline DHT",
            ),
            "The DHT needs outgoing UDP, which this network may block. Phones then dial the address they learned at home.",
        ),
        None => finding(
            check,
            Status::Info,
            "checking that the DHT has this computer's record (each minute until it does)",
        ),
    }
}

/// The home relay `endpoint.json` announces, if any.
fn announced_relay(state_dir: &Path) -> Option<String> {
    std::fs::read_to_string(state_dir.join(ENDPOINT_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("relayUrls")?
                .as_array()?
                .first()?
                .as_str()
                .map(str::to_string)
        })
}

/// n0's public relays live under `iroh.link`; anything else is the user's own.
fn is_n0_relay(url: &str) -> bool {
    url.contains("iroh.link")
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

/// Whether a network adapter lets a magic packet wake the computer, as the
/// system says (N6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeAllowed {
    Yes,
    No,
    Unknown,
}

/// Read what the system said: Windows' `WakeOnMagicPacket` (`Enabled`,
/// `Disabled`, `Unsupported`), `ethtool`'s `Wake-on:` line (`g` is the
/// magic packet), or sysfs' `power/wakeup` (`enabled`, `disabled`).
pub fn parse_wake_allowed(answer: &str) -> WakeAllowed {
    for line in answer.lines() {
        let line = line.trim();
        if let Some(modes) = line.strip_prefix("Wake-on:") {
            return if modes.trim().contains('g') {
                WakeAllowed::Yes
            } else {
                WakeAllowed::No
            };
        }
    }
    match answer.trim().to_ascii_lowercase().as_str() {
        "enabled" => WakeAllowed::Yes,
        "disabled" | "unsupported" => WakeAllowed::No,
        _ => WakeAllowed::Unknown,
    }
}

/// Whether a phone can wake this computer: the adapter it would wake through
/// (named, never its MAC: findings go into the debug report), whether the
/// system lets a magic packet wake it, and whether phones are told how.
pub fn check_wake(
    facts: Option<&super::wake::WakeFacts>,
    allowed: WakeAllowed,
    announce: bool,
) -> Finding {
    const CHECK: &str = "Wake";
    let Some(facts) = facts else {
        return with_hint(
            finding(
                CHECK,
                Status::Info,
                "No network adapter with a hardware address was found",
            ),
            "Waking from the phone needs a wired or Wi-Fi adapter.",
        );
    };
    if !announce {
        return with_hint(
            finding(
                CHECK,
                Status::Info,
                "Phones are not told how to wake this computer",
            ),
            "Turn on \"Tell paired phones how to wake this computer\" in Settings.",
        );
    }
    let adapter = &facts.adapter;
    match (allowed, facts.wired) {
        (WakeAllowed::No, _) => with_hint(
            finding(
                CHECK,
                Status::Warn,
                format!("{adapter} does not allow waking by magic packet"),
            ),
            "Turn on \"Wake on Magic Packet\" in the adapter's power settings, and Wake-on-LAN in the computer's firmware (BIOS/UEFI).",
        ),
        (_, false) => with_hint(
            finding(
                CHECK,
                Status::Warn,
                format!("{adapter} is Wi-Fi: it can usually wake this computer only from sleep"),
            ),
            "A wired connection can wake it from shutdown or hibernation too.",
        ),
        (WakeAllowed::Yes, true) => finding(
            CHECK,
            Status::Good,
            format!("{adapter} (wired) allows waking by magic packet"),
        ),
        (WakeAllowed::Unknown, true) => with_hint(
            finding(
                CHECK,
                Status::Info,
                format!("{adapter} is wired; whether it allows waking could not be read"),
            ),
            "Check \"Wake on Magic Packet\" in the adapter's power settings, and Wake-on-LAN in the firmware (BIOS/UEFI).",
        ),
    }
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
pub fn check_internet_path(state_dir: &Path, state: ServerState, configured: &[String]) -> Finding {
    let check = "Internet path";
    if state != ServerState::Running {
        return finding(
            check,
            Status::Info,
            "not checked: the server is not running yet",
        );
    }
    if let Some(problem) = configured
        .iter()
        .find_map(|url| super::config::relay_url_problem(url))
    {
        return with_hint(
            finding(
                check,
                Status::Bad,
                format!(
                    "relay_urls has an entry the server refuses ({problem}): away from home is off"
                ),
            ),
            "Write it as http://<address>:3340 or https://<name> in config.toml, then restart the server.",
        );
    }
    let relay = announced_relay(state_dir);
    if let Some(url) = relay {
        // Named, not shown: the report may be pasted publicly (N2).
        let which = if is_n0_relay(&url) {
            "n0's public relay"
        } else {
            "your relay"
        };
        finding(
            check,
            Status::Good,
            format!("ready through {which} (phones away from home can connect)"),
        )
    } else if !configured.is_empty() {
        with_hint(
            finding(check, Status::Warn, "not ready: your relay is not reached"),
            "Is `pocketdesk relay` running, and can this computer reach the address in relay_urls? Until then only this network works.",
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
            "In Windows Defender Firewall, allow pocketdesk (or remove its block rule) for this network.",
        ),
        Ok(fw) if fw.allow == 0 => with_hint(
            finding(
                check,
                Status::Bad,
                format!("no rule lets phones in on this {} network", fw.profiles),
            ),
            "Run `pocketdesk install` (it allows phones on private networks), or allow the control and stream ports for this network.",
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

/// O9: a Linux firewall as it applies to phones: ufw's or firewalld's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxFirewall {
    /// Neither is installed.
    None,
    /// `tool` is installed but not running.
    Off { tool: &'static str },
    /// `tool` runs, and its rules could not be read (ufw needs root).
    Unreadable { tool: &'static str },
    /// `tool` runs: whether it lets in the control port (TCP) and the
    /// stream port (UDP).
    On {
        tool: &'static str,
        control: bool,
        stream: bool,
    },
}

/// Whether `rule`'s port part (`1716`, `1716/tcp`, `1714:1764/udp`,
/// `1714-1764/tcp`, `1716,4716/tcp`) takes `port` over `proto`.
fn rule_takes(rule: &str, port: u16, proto: &str) -> bool {
    let (ports, rule_proto) = match rule.split_once('/') {
        Some((ports, p)) => (ports, Some(p)),
        None => (rule, None),
    };
    if rule_proto.is_some_and(|p| p != proto) {
        return false;
    }
    ports.split(',').any(|part| {
        let part = part.trim();
        match part.split_once([':', '-']) {
            Some((a, b)) => match (a.parse::<u16>(), b.parse::<u16>()) {
                (Ok(a), Ok(b)) => (a..=b).contains(&port),
                _ => false,
            },
            None => part.parse::<u16>() == Ok(port),
        }
    })
}

/// A ufw rule line: what it is to (the words before the action, without
/// "(v6)" and "on <interface>"), its action, and its direction (IN, OUT,
/// FWD), if it says one. None for a line that is no rule.
fn ufw_rule(line: &str) -> Option<(Vec<&str>, &str, Option<&str>)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words
        .iter()
        .position(|w| matches!(*w, "ALLOW" | "DENY" | "REJECT" | "LIMIT"))?;
    let mut to = Vec::new();
    let mut before = words[..at].iter();
    while let Some(w) = before.next() {
        match *w {
            "(v6)" => {}
            "on" => {
                before.next();
            }
            _ => to.push(*w),
        }
    }
    let direction = words
        .get(at + 1)
        .copied()
        .filter(|w| matches!(*w, "IN" | "OUT" | "FWD"));
    Some((to, words[at], direction))
}

/// Whether `word` is a ufw port part: `1716`, `1716/tcp`, `1714:1764/udp`,
/// `1716,4716/tcp`; not an address.
fn ufw_ports(word: &str) -> bool {
    let ports = word
        .strip_suffix("/tcp")
        .or_else(|| word.strip_suffix("/udp"))
        .unwrap_or(word);
    !ports.is_empty()
        && ports.starts_with(|c: char| c.is_ascii_digit())
        && ports
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == ':')
        && ports.split(',').all(|p| p.matches(':').count() <= 1)
}

/// O9: `ufw status verbose` read for the control and stream ports: off, or
/// on with which of the two an ALLOW rule (or an allowing default) lets in.
pub fn parse_ufw(status: &str, control_port: u16, stream_port: u16) -> LinuxFirewall {
    let tool = "ufw";
    if status.lines().any(|l| l.trim() == "Status: inactive") {
        return LinuxFirewall::Off { tool };
    }
    if !status.lines().any(|l| l.trim() == "Status: active") {
        return LinuxFirewall::Unreadable { tool };
    }
    // "Default: deny (incoming), allow (outgoing), ..."
    let allows_in = status
        .lines()
        .any(|l| l.trim_start().starts_with("Default:") && l.contains("allow (incoming)"));
    let mut control = allows_in;
    let mut stream = allows_in;
    for line in status.lines() {
        let Some((to, action, direction)) = ufw_rule(line) else {
            continue;
        };
        // ALLOW and LIMIT let in; only rules for what comes in count (O9
        // review: an OUT or FWD rule had read as open).
        if !matches!(action, "ALLOW" | "LIMIT") || !matches!(direction, None | Some("IN")) {
            continue;
        }
        let port = to.iter().find(|t| ufw_ports(t));
        match port {
            Some(port) => {
                control |= rule_takes(port, control_port, "tcp");
                stream |= rule_takes(port, stream_port, "udp");
            }
            // "Anywhere ALLOW IN 192.0.2.0/24", or one address: every port
            // to it. An app profile ("OpenSSH") names no port read here.
            None => {
                let every = to
                    .first()
                    .is_some_and(|t| *t == "Anywhere" || t.contains(['.', ':', '/']));
                control |= every;
                stream |= every;
            }
        }
    }
    LinuxFirewall::On {
        tool,
        control,
        stream,
    }
}

/// O9: firewalld read for the two ports, from `firewall-cmd --state` and
/// `--list-all` of each active zone (their lists one after another; a port
/// let in by any of them counts): its `ports:`, rich rules that accept a
/// port, an ACCEPT target, and the `kdeconnect` service, which opens
/// 1714-1764 both ways.
pub fn parse_firewalld(
    state: &str,
    list_all: Option<&str>,
    control_port: u16,
    stream_port: u16,
) -> LinuxFirewall {
    let tool = "firewalld";
    if state.trim() != "running" {
        return LinuxFirewall::Off { tool };
    }
    let Some(list) = list_all else {
        return LinuxFirewall::Unreadable { tool };
    };
    let field = |name: &str| -> Vec<String> {
        list.lines()
            .filter_map(|l| l.trim().strip_prefix(&format!("{name}:")))
            .flat_map(|rest| {
                rest.split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let target_accepts = field("target").iter().any(|t| t == "ACCEPT");
    let mut ports = field("ports");
    // Rich rules (O9 review): `rule ... port port="1716" protocol="tcp" accept`.
    for rule in list
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("rule "))
    {
        if !rule.split_whitespace().any(|w| w == "accept") {
            continue;
        }
        let value = |key: &str| {
            rule.split_whitespace()
                .find_map(|w| w.strip_prefix(key))
                .map(|v| v.trim_matches('"').to_string())
        };
        if let (Some(port), Some(proto)) = (value("port="), value("protocol=")) {
            ports.push(format!("{}/{proto}", port.replace('-', ":")));
        }
    }
    let kde = field("services").iter().any(|s| s == "kdeconnect");
    let takes = |port: u16, proto: &str| {
        target_accepts
            || ports.iter().any(|r| rule_takes(r, port, proto))
            || (kde && (1714..=1764).contains(&port))
    };
    LinuxFirewall::On {
        tool,
        control: takes(control_port, "tcp"),
        stream: takes(stream_port, "udp"),
    }
}

/// O9: the firewall on Linux; never a failure without one.
pub fn check_linux_firewall(
    firewall: &LinuxFirewall,
    control_port: u16,
    stream_port: u16,
) -> Finding {
    let check = "Firewall";
    match firewall {
        LinuxFirewall::None => finding(check, Status::Good, "no firewall found (ufw or firewalld)"),
        LinuxFirewall::Off { tool } => {
            finding(check, Status::Good, format!("{tool} is installed but off"))
        }
        LinuxFirewall::Unreadable { tool } => with_hint(
            finding(
                check,
                Status::Warn,
                format!("{tool} is on, and its rules could not be read (they may need root)"),
            ),
            match *tool {
                "ufw" => format!(
                    "Run `sudo ufw status` and check that {control_port}/tcp and {stream_port}/udp are allowed."
                ),
                _ => format!(
                    "Run `sudo firewall-cmd --list-all` and check that {control_port}/tcp and {stream_port}/udp are allowed."
                ),
            },
        ),
        LinuxFirewall::On {
            tool,
            control: true,
            stream: true,
        } => finding(
            check,
            Status::Good,
            format!("{tool} lets phones in on {control_port}/tcp and {stream_port}/udp"),
        ),
        LinuxFirewall::On {
            tool,
            control,
            stream,
        } => {
            let shut: Vec<String> = [
                (!control).then(|| format!("{control_port}/tcp")),
                (!stream).then(|| format!("{stream_port}/udp")),
            ]
            .into_iter()
            .flatten()
            .collect();
            let hint = match *tool {
                "ufw" => format!(
                    "Run {}.",
                    shut.iter()
                        .map(|p| format!("`sudo ufw allow {p}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                _ => format!(
                    "Run `sudo firewall-cmd --permanent {}` and then `sudo firewall-cmd --reload`.",
                    shut.iter()
                        .map(|p| format!("--add-port={p}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            };
            with_hint(
                finding(
                    check,
                    Status::Bad,
                    format!("{tool} does not let phones in on {}", shut.join(" or ")),
                ),
                hint,
            )
        }
    }
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

    fn facts(wired: bool) -> super::super::wake::WakeFacts {
        super::super::wake::WakeFacts {
            adapter: "Ethernet 2".into(),
            mac: "02:11:22:33:44:55".into(),
            broadcast: std::net::Ipv4Addr::new(192, 168, 1, 255),
            wired,
        }
    }

    #[test]
    fn what_the_system_says_about_waking_is_read() {
        assert_eq!(parse_wake_allowed("Enabled\r\n"), WakeAllowed::Yes);
        assert_eq!(parse_wake_allowed("Disabled"), WakeAllowed::No);
        assert_eq!(parse_wake_allowed("Unsupported"), WakeAllowed::No);
        assert_eq!(
            parse_wake_allowed("Settings for eth0:\n\tSupports Wake-on: pumbg\n\tWake-on: g\n"),
            WakeAllowed::Yes
        );
        assert_eq!(
            parse_wake_allowed("\tSupports Wake-on: pumbg\n\tWake-on: d\n"),
            WakeAllowed::No
        );
        assert_eq!(parse_wake_allowed("enabled\n"), WakeAllowed::Yes);
        assert_eq!(parse_wake_allowed(""), WakeAllowed::Unknown);
        assert_eq!(parse_wake_allowed("something else"), WakeAllowed::Unknown);
    }

    #[test]
    fn a_wired_adapter_that_allows_wake_is_good_and_names_no_mac() {
        let good = check_wake(Some(&facts(true)), WakeAllowed::Yes, true);
        assert_eq!(good.status, Status::Good);
        assert!(good.detail.contains("Ethernet 2"));
        assert!(
            !good.detail.contains("02:11"),
            "the MAC stays out of reports"
        );
        assert!(good.hint.is_none());
    }

    #[test]
    fn wake_turned_off_or_wifi_is_a_warning_with_a_next_step() {
        let off = check_wake(Some(&facts(true)), WakeAllowed::No, true);
        assert_eq!(off.status, Status::Warn);
        assert!(off.hint.unwrap().contains("Magic Packet"));
        let wifi = check_wake(Some(&facts(false)), WakeAllowed::Yes, true);
        assert_eq!(wifi.status, Status::Warn);
        assert!(wifi.detail.contains("Wi-Fi"));
    }

    #[test]
    fn unknown_unannounced_or_no_adapter_is_only_information() {
        assert_eq!(
            check_wake(Some(&facts(true)), WakeAllowed::Unknown, true).status,
            Status::Info
        );
        assert_eq!(
            check_wake(Some(&facts(true)), WakeAllowed::Yes, false).status,
            Status::Info
        );
        assert_eq!(
            check_wake(None, WakeAllowed::Unknown, true).status,
            Status::Info
        );
    }

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
            check_internet_path(&dir, UP, &[]).status,
            Status::Warn,
            "no file"
        );
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":[],"directAddrs":[]}"#,
        )
        .unwrap();
        assert_eq!(
            check_internet_path(&dir, UP, &[]).status,
            Status::Warn,
            "no relay yet"
        );
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":["https://relay.example/"],"directAddrs":[]}"#,
        )
        .unwrap();
        assert_eq!(check_internet_path(&dir, UP, &[]).status, Status::Good);
        assert_eq!(
            check_internet_path(&dir, ServerState::Stopped, &[]).status,
            Status::Info,
            "a file left by a stopped server proves nothing"
        );
    }

    /// O9: ufw's status read for the two ports.
    #[test]
    fn ufw_says_which_ports_it_lets_in() {
        let on = |rules: &str| {
            format!(
                "Status: active\nLogging: on (low)\nDefault: deny (incoming), allow (outgoing), disabled (routed)\nNew profiles: skip\n\nTo                         Action      From\n--                         ------      ----\n{rules}"
            )
        };
        assert_eq!(
            parse_ufw(
                &on(
                    "22/tcp ALLOW IN Anywhere\n1716/tcp ALLOW IN Anywhere\n4716/udp ALLOW IN Anywhere\n"
                ),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: true
            }
        );
        assert_eq!(
            parse_ufw(
                &on("1714:1764/tcp ALLOW IN Anywhere\n1716 (v6) ALLOW IN Anywhere (v6)\n"),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: false
            },
            "a range, and a v6 rule's own port"
        );
        assert_eq!(
            parse_ufw(
                &on("Anywhere ALLOW IN 192.0.2.0/24
"),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: true
            },
            "every port from the home network"
        );
        assert_eq!(
            parse_ufw(&on("4716 DENY IN Anywhere\n"), 1716, 4716),
            LinuxFirewall::On {
                tool: "ufw",
                control: false,
                stream: false
            }
        );
        assert_eq!(
            parse_ufw("Status: inactive\n", 1716, 4716),
            LinuxFirewall::Off { tool: "ufw" }
        );
        assert_eq!(
            parse_ufw(
                "ERROR: You need to be root to run this script\n",
                1716,
                4716
            ),
            LinuxFirewall::Unreadable { tool: "ufw" }
        );
        // O9 review: what an OUT or FWD rule, an interface or an address does.
        assert_eq!(
            parse_ufw(
                &on("Anywhere ALLOW OUT Anywhere\nAnywhere ALLOW FWD 10.0.0.0/8\n"),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "ufw",
                control: false,
                stream: false
            },
            "not for what comes in"
        );
        assert_eq!(
            parse_ufw(
                &on("1716/tcp on wlan0 ALLOW IN Anywhere\n192.0.2.5 4716/udp LIMIT IN Anywhere\n"),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: true
            },
            "an interface, an address and a limit"
        );
        assert_eq!(
            parse_ufw(&on("OpenSSH ALLOW IN Anywhere\n"), 1716, 4716),
            LinuxFirewall::On {
                tool: "ufw",
                control: false,
                stream: false
            },
            "an app profile"
        );
        let open = "Status: active\nDefault: allow (incoming), allow (outgoing)\n";
        assert_eq!(
            parse_ufw(open, 1716, 4716),
            LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: true
            }
        );
    }

    /// O9: firewalld's zone read for the two ports.
    #[test]
    fn firewalld_says_which_ports_it_lets_in() {
        let zone = |ports: &str, services: &str| {
            format!(
                "public (active)\n  target: default\n  interfaces: eth0\n  services: {services}\n  ports: {ports}\n  protocols:\n"
            )
        };
        assert_eq!(
            parse_firewalld(
                "running\n",
                Some(&zone("1716/tcp 4716/udp", "ssh")),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "firewalld",
                control: true,
                stream: true
            }
        );
        assert_eq!(
            parse_firewalld("running", Some(&zone("", "ssh kdeconnect")), 1716, 4716),
            LinuxFirewall::On {
                tool: "firewalld",
                control: true,
                stream: false
            },
            "KDE Connect's service opens 1714-1764 only"
        );
        assert_eq!(
            parse_firewalld(
                "running",
                Some(&zone("1700-1800/tcp 4000-5000/udp", "")),
                1716,
                4716
            ),
            LinuxFirewall::On {
                tool: "firewalld",
                control: true,
                stream: true
            }
        );
        let rich = "home (active)\n  ports:\n  rich rules:\n\trule family=\"ipv4\" port port=\"1716\" protocol=\"tcp\" accept\n\trule family=\"ipv4\" port port=\"4700-4800\" protocol=\"udp\" accept\n";
        assert_eq!(
            parse_firewalld("running", Some(rich), 1716, 4716),
            LinuxFirewall::On {
                tool: "firewalld",
                control: true,
                stream: true
            },
            "rich rules"
        );
        assert_eq!(
            parse_firewalld("not running", None, 1716, 4716),
            LinuxFirewall::Off { tool: "firewalld" }
        );
        assert_eq!(
            parse_firewalld("running", None, 1716, 4716),
            LinuxFirewall::Unreadable { tool: "firewalld" }
        );
    }

    /// O9: no firewall is never a failure; a shut port says what to run.
    #[test]
    fn the_linux_firewall_finding_says_what_to_run() {
        assert_eq!(
            check_linux_firewall(&LinuxFirewall::None, 1716, 4716).status,
            Status::Good
        );
        assert_eq!(
            check_linux_firewall(&LinuxFirewall::Off { tool: "ufw" }, 1716, 4716).status,
            Status::Good
        );
        let unread = check_linux_firewall(&LinuxFirewall::Unreadable { tool: "ufw" }, 1716, 4716);
        assert_eq!(unread.status, Status::Warn);
        assert!(unread.hint.unwrap().contains("sudo ufw status"));
        let shut = check_linux_firewall(
            &LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: false,
            },
            1716,
            4716,
        );
        assert_eq!(shut.status, Status::Bad);
        assert!(shut.detail.contains("4716/udp") && !shut.detail.contains("1716/tcp"));
        assert_eq!(shut.hint.as_deref(), Some("Run `sudo ufw allow 4716/udp`."));
        let fw = check_linux_firewall(
            &LinuxFirewall::On {
                tool: "firewalld",
                control: false,
                stream: false,
            },
            1716,
            4716,
        );
        assert_eq!(
            fw.hint.as_deref(),
            Some(
                "Run `sudo firewall-cmd --permanent --add-port=1716/tcp --add-port=4716/udp` and then `sudo firewall-cmd --reload`."
            )
        );
        let open = check_linux_firewall(
            &LinuxFirewall::On {
                tool: "ufw",
                control: true,
                stream: true,
            },
            1716,
            4716,
        );
        assert_eq!(open.status, Status::Good);
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
        assert!(none.hint.unwrap().contains("pocketdesk install"));
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
            public_v6,
            captive_portal: Some(false),
            dht: None,
            dht_found: None,
        }
    }

    fn dht(mode: &str, found: Option<bool>) -> WanStatus {
        let mut status = wan(true, Some(false), false);
        status.dht = Some(mode.to_string());
        status.dht_found = found;
        status
    }

    #[test]
    fn found_by_id_says_whether_the_dht_has_this_computer() {
        let read_back = check_dht(Some(&dht("relay", Some(true))), UP);
        assert_eq!(read_back.check, "Found by id");
        assert_eq!(read_back.status, Status::Good);
        assert!(read_back.detail.contains("relay address only"));
        assert!(
            check_dht(Some(&dht("addresses", Some(true))), UP)
                .detail
                .contains("public addresses")
        );
        let missing = check_dht(Some(&dht("relay", Some(false))), UP);
        assert_eq!(missing.status, Status::Warn);
        assert!(missing.hint.as_deref().unwrap().contains("UDP"));
        let checking = check_dht(Some(&dht("relay", None)), UP);
        assert_eq!(checking.status, Status::Info);
        assert!(checking.detail.contains("checking"));
        let unjoined = check_dht(Some(&dht("unjoined", None)), UP);
        assert_eq!(unjoined.status, Status::Warn);
        assert!(unjoined.detail.contains("not on the Mainline DHT"));
        let empty = check_dht(Some(&dht("empty", None)), UP);
        assert_eq!(empty.status, Status::Warn);
        assert!(empty.hint.as_deref().unwrap().contains("relay_urls"));
        let off = check_dht(Some(&dht("off", None)), UP);
        assert_eq!(off.status, Status::Info);
        assert!(off.detail.starts_with("off"));
        assert_eq!(
            check_dht(Some(&wan(true, None, false)), UP).status,
            Status::Info,
            "a server that does not report it"
        );
        assert_eq!(check_dht(None, UP).status, Status::Info);
        assert!(
            check_dht(Some(&dht("relay", Some(true))), ServerState::Starting)
                .detail
                .contains("not running")
        );
    }

    #[test]
    fn an_older_wan_status_without_the_dht_still_reads() {
        let dir = temp_dir("wan-old");
        std::fs::write(
            dir.join(WAN_STATUS_FILE),
            r#"{"unix_secs":1,"udp":true,"nat_varies":false,"public_v6":false,"captive_portal":null}"#,
        )
        .unwrap();
        let status = read_wan_status(&dir).expect("read");
        assert_eq!((status.dht, status.dht_found), (None, None));
    }

    #[test]
    fn from_outside_says_whether_phones_connect_directly() {
        let friendly = check_outside(Some(&wan(true, Some(false), false)), UP, false);
        assert_eq!(friendly[0].status, Status::Good);
        assert_eq!(friendly[1].check, "IPv6");
        assert_eq!(friendly[1].status, Status::Info);
        let strict = check_outside(Some(&wan(true, Some(true), true)), UP, false);
        assert_eq!(strict[0].status, Status::Warn);
        assert!(strict[0].hint.as_deref().unwrap().contains("UPnP"));
        assert_eq!(
            strict[1].status,
            Status::Good,
            "IPv6 still connects directly"
        );
        assert_eq!(
            check_outside(Some(&wan(false, None, false)), UP, false)[0].status,
            Status::Warn,
            "no UDP"
        );
        let unmeasured = check_outside(Some(&wan(false, None, false)), UP, true);
        assert_eq!(
            unmeasured[0].status,
            Status::Info,
            "your relay: not measured, not blocked"
        );
        assert!(unmeasured[0].detail.contains("not measured"));
        let mut portal = wan(true, Some(false), false);
        portal.captive_portal = Some(true);
        assert!(
            check_outside(Some(&portal), UP, false)[0]
                .detail
                .contains("sign-in")
        );
        assert_eq!(
            check_outside(None, UP, false)[0].status,
            Status::Info,
            "not measured yet"
        );
        assert_eq!(check_outside(None, ServerState::Starting, false).len(), 1);
    }

    #[test]
    fn the_wan_status_is_written_and_read_back() {
        let dir = temp_dir("wan");
        assert_eq!(read_wan_status(&dir), None);
        let status = wan(true, Some(false), true);
        write_wan_status(&dir, &status).unwrap();
        assert_eq!(read_wan_status(&dir), Some(status));
    }

    #[test]
    fn the_internet_path_names_the_relay_in_use() {
        let dir = temp_dir("relay-name");
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":["https://use1-1.relay.n0.iroh.link./"],"directAddrs":[]}"#,
        )
        .unwrap();
        assert!(
            check_internet_path(&dir, UP, &[])
                .detail
                .contains("n0's public relay")
        );
        std::fs::write(
            dir.join(ENDPOINT_FILE),
            r#"{"endpointId":"x","relayUrls":["http://203.0.113.7:3340/"],"directAddrs":[]}"#,
        )
        .unwrap();
        let own = check_internet_path(&dir, UP, &[]);
        assert_eq!(own.status, Status::Good);
        assert!(own.detail.contains("your relay"), "{}", own.detail);
    }

    #[test]
    fn the_internet_path_says_when_your_relay_is_not_reached() {
        let dir = temp_dir("relay-unreached");
        let mine = ["http://192.0.2.10:3340".to_string()];
        let unreached = check_internet_path(&dir, UP, &mine);
        assert_eq!(unreached.status, Status::Warn);
        assert!(
            unreached.detail.contains("your relay is not reached"),
            "{}",
            unreached.detail
        );
        assert!(
            unreached
                .hint
                .as_deref()
                .unwrap()
                .contains("pocketdesk relay")
        );
        let typo = check_internet_path(&dir, UP, &["relay.example.org:3340".to_string()]);
        assert_eq!(typo.status, Status::Bad);
        assert!(typo.detail.contains("relay_urls"), "{}", typo.detail);
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
            "pocketdesk install",
            "ffmpeg-sidecar-nvenc",
            "e2e p50 16 ms, p99 31 ms",
            "0d7c3a52",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        assert!(!text.contains("0d7c3a52-91e4"), "a whole device id");
    }
}
