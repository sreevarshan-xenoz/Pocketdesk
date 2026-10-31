//! A Linux packet filter's rules (nftables, or iptables) read for the ports
//! Pocketdesk needs, never changed: Diagnostics' firewall row and the Pair
//! page's warning. The rules come from `nft list ruleset` or `iptables-save`
//! when they may be read (root), or else from the file the system loads them
//! from (`/etc/nftables.conf`, `/etc/iptables/iptables.rules`).
//!
//! Rules can be arbitrarily complex, so a rule this does not understand,
//! where it could decide a packet's fate, makes the answer
//! [`Verdict::Unsure`], never [`Verdict::Open`].

use std::collections::HashMap;

/// KDE Connect's port range, TCP and UDP (its discovery broadcasts on UDP
/// 1716).
pub const KDE_PORTS: (u16, u16) = (1714, 1764);
/// mDNS, how phones on the network find this computer by themselves.
pub const MDNS_PORT: u16 = 5353;
/// The ports of [`KDE_PORTS`] tried: both ends, the one KDE Connect listens
/// on, and one in the middle.
const KDE_SAMPLES: [u16; 4] = [1714, 1716, 1739, 1764];

/// What a firewall does with what a phone or another computer sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Open,
    Shut,
    /// A rule that could decide it was not understood, or the rules shown
    /// are not all there is.
    Unsure,
}

impl Verdict {
    /// The worse of the two: shut beats unsure beats open.
    pub fn and(self, other: Verdict) -> Verdict {
        match (self, other) {
            (Verdict::Shut, _) | (_, Verdict::Shut) => Verdict::Shut,
            (Verdict::Unsure, _) | (_, Verdict::Unsure) => Verdict::Unsure,
            _ => Verdict::Open,
        }
    }

    pub fn from_open(open: bool) -> Verdict {
        if open { Verdict::Open } else { Verdict::Shut }
    }
}

/// Whether each kind of traffic Pocketdesk needs gets in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortsOpen {
    /// The control port, TCP.
    pub control: Verdict,
    /// The stream port, UDP.
    pub stream: Verdict,
    /// KDE Connect's range, TCP and UDP, and its broadcasts.
    pub kde: Verdict,
    /// mDNS (UDP 5353, multicast).
    pub mdns: Verdict,
}

impl PortsOpen {
    pub fn all(v: Verdict) -> PortsOpen {
        PortsOpen {
            control: v,
            stream: v,
            kde: v,
            mdns: v,
        }
    }

    /// Each kind asked of `verdict`, for the packets it takes.
    pub fn from_check(
        control_port: u16,
        stream_port: u16,
        verdict: impl Fn(Packet) -> Verdict,
    ) -> PortsOpen {
        let unicast = |proto, port| Packet {
            proto,
            port,
            cast: Cast::Unicast,
        };
        let kde = KDE_SAMPLES
            .iter()
            .flat_map(|&p| [unicast(Proto::Tcp, p), unicast(Proto::Udp, p)])
            .chain([Packet {
                proto: Proto::Udp,
                port: 1716,
                cast: Cast::Broadcast,
            }])
            .map(&verdict)
            .fold(Verdict::Open, Verdict::and);
        PortsOpen {
            control: verdict(unicast(Proto::Tcp, control_port)),
            stream: verdict(unicast(Proto::Udp, stream_port)),
            kde,
            mdns: verdict(Packet {
                proto: Proto::Udp,
                port: MDNS_PORT,
                cast: Cast::Multicast,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    fn name(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
    fn number(self) -> &'static str {
        match self {
            Proto::Tcp => "6",
            Proto::Udp => "17",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cast {
    Unicast,
    Multicast,
    Broadcast,
}

/// A new packet (no connection yet) over IPv4, from a device on the
/// network: not from loopback, a container bridge or a VPN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub proto: Proto,
    pub port: u16,
    pub cast: Cast,
}

/// Whether one condition of a rule holds for a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tri {
    Yes,
    No,
    Maybe,
}

impl Tri {
    fn and(self, other: Tri) -> Tri {
        match (self, other) {
            (Tri::No, _) | (_, Tri::No) => Tri::No,
            (Tri::Maybe, _) | (_, Tri::Maybe) => Tri::Maybe,
            _ => Tri::Yes,
        }
    }
    fn not(self, neg: bool) -> Tri {
        match (self, neg) {
            (Tri::Yes, true) => Tri::No,
            (Tri::No, true) => Tri::Yes,
            (t, _) => t,
        }
    }
    fn of(yes: bool) -> Tri {
        if yes { Tri::Yes } else { Tri::No }
    }
}

/// Ports a rule names: known ranges, and whether it names any it could not
/// read (a service name not known here, a set not found).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ports {
    ranges: Vec<(u16, u16)>,
    unknown: bool,
}

impl Ports {
    /// From `22`, `1714-1764`, `1714:1764` or a service name.
    fn from_values(values: Option<&[String]>) -> Ports {
        let mut ports = Ports::default();
        let Some(values) = values else {
            ports.unknown = true;
            return ports;
        };
        for v in values {
            for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                // A service name may hold a dash (`netbios-ns`).
                let range = port_number(part).map(|p| (p, p)).or_else(|| {
                    part.split_once(['-', ':'])
                        .and_then(|(a, b)| port_number(a).zip(port_number(b)))
                });
                match range {
                    Some(r) => ports.ranges.push(r),
                    None => ports.unknown = true,
                }
            }
        }
        ports
    }

    fn holds(&self, port: u16) -> Tri {
        if self.ranges.iter().any(|&(a, b)| (a..=b).contains(&port)) {
            Tri::Yes
        } else if self.unknown {
            Tri::Maybe
        } else {
            Tri::No
        }
    }
}

/// A port by number or by its common service name.
fn port_number(word: &str) -> Option<u16> {
    let word = word.trim().trim_matches('"');
    if let Ok(n) = word.parse() {
        return Some(n);
    }
    Some(match word {
        "ftp" => 21,
        "ssh" => 22,
        "telnet" => 23,
        "smtp" => 25,
        "domain" => 53,
        "bootps" => 67,
        "bootpc" => 68,
        "http" | "www" => 80,
        "ntp" => 123,
        "netbios-ns" => 137,
        "netbios-dgm" => 138,
        "netbios-ssn" => 139,
        "https" => 443,
        "microsoft-ds" => 445,
        "dhcpv6-client" => 546,
        "dhcpv6-server" => 547,
        "ipp" => 631,
        "mdns" => 5353,
        "llmnr" => 5355,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cond {
    /// `meta l4proto`, `ip protocol`, `-p`.
    Proto {
        protos: Vec<String>,
        neg: bool,
    },
    /// A destination port; `proto` when the match itself names one (`tcp
    /// dport`), which a packet of the other kind never meets.
    DPort {
        proto: Option<Proto>,
        ports: Ports,
        neg: bool,
    },
    /// The interface it came in on.
    Iface {
        names: Vec<String>,
        neg: bool,
    },
    CtState {
        states: Vec<String>,
        neg: bool,
    },
    /// host/unicast, multicast, broadcast.
    PktType {
        kinds: Vec<String>,
        neg: bool,
    },
    DAddr {
        addrs: Vec<String>,
        neg: bool,
    },
    /// `meta nfproto ipv4` (true) or `ipv6`.
    Ipv4(bool),
    /// Never this packet (IPv6, ICMP).
    Never,
    /// Not understood.
    Unknown,
}

/// Whether an interface name could be the one a device on the network
/// comes in on: loopback, container bridges and VPNs are not.
fn lan_iface(name: &str) -> Tri {
    let name = name.trim_matches('"');
    const NOT_LAN: [&str; 12] = [
        "docker",
        "br-",
        "veth",
        "virbr",
        "vnet",
        "tailscale",
        "wg",
        "tun",
        "tap",
        "podman",
        "cni",
        "lxc",
    ];
    if name == "lo" || NOT_LAN.iter().any(|p| name.starts_with(p)) {
        Tri::No
    } else {
        Tri::Maybe
    }
}

impl Cond {
    fn holds(&self, p: Packet) -> Tri {
        match self {
            Cond::Proto { protos, neg } => Tri::of(
                protos
                    .iter()
                    .any(|v| v == p.proto.name() || v == p.proto.number()),
            )
            .not(*neg),
            Cond::DPort { proto, ports, neg } => {
                if proto.is_some_and(|proto| proto != p.proto) {
                    Tri::No
                } else {
                    ports.holds(p.port).not(*neg)
                }
            }
            Cond::Iface { names, neg } => {
                let any_maybe = names.iter().any(|n| lan_iface(n) == Tri::Maybe);
                match (any_maybe, *neg) {
                    (false, false) => Tri::No,
                    (false, true) => Tri::Yes,
                    _ => Tri::Maybe,
                }
            }
            Cond::CtState { states, neg } => {
                Tri::of(states.iter().any(|s| s.eq_ignore_ascii_case("new"))).not(*neg)
            }
            Cond::PktType { kinds, neg } => {
                let kind = |k: &str| match k.to_ascii_lowercase().as_str() {
                    "host" | "unicast" | "local" => Some(Cast::Unicast),
                    "multicast" => Some(Cast::Multicast),
                    "broadcast" => Some(Cast::Broadcast),
                    _ => None,
                };
                if kinds.iter().any(|k| kind(k).is_none()) {
                    return Tri::Maybe;
                }
                Tri::of(kinds.iter().any(|k| kind(k) == Some(p.cast))).not(*neg)
            }
            Cond::DAddr { addrs, neg } => addrs
                .iter()
                .map(|a| daddr_holds(a, p.cast))
                .fold(Tri::No, |acc, t| match (acc, t) {
                    (Tri::Yes, _) | (_, Tri::Yes) => Tri::Yes,
                    (Tri::Maybe, _) | (_, Tri::Maybe) => Tri::Maybe,
                    _ => Tri::No,
                })
                .not(*neg),
            Cond::Ipv4(v4) => Tri::of(*v4),
            Cond::Never => Tri::No,
            Cond::Unknown => Tri::Maybe,
        }
    }
}

/// Whether a destination address (or prefix) is the one a packet of this
/// kind is sent to: mDNS's group, the broadcast address, or (unicast)
/// this computer's own, which is not known here.
fn daddr_holds(addr: &str, cast: Cast) -> Tri {
    let addr = addr.trim_matches('"');
    let first: Option<u8> = addr.split('.').next().and_then(|o| o.parse().ok());
    let multicast = first.is_some_and(|o| (224..=239).contains(&o));
    let broadcast = addr == "255.255.255.255";
    match cast {
        Cast::Multicast if addr == "224.0.0.251" || addr.starts_with("224.0.0.0/") => Tri::Yes,
        Cast::Multicast if multicast => Tri::Maybe,
        Cast::Multicast if broadcast => Tri::No,
        Cast::Broadcast if broadcast => Tri::Yes,
        Cast::Broadcast if multicast => Tri::No,
        Cast::Unicast if multicast || broadcast => Tri::No,
        _ => Tri::Maybe,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Accept,
    Drop,
    Jump(String),
    Goto(String),
    Return,
    /// No verdict: counts, logs, marks; the next rule follows.
    Next,
    /// A verdict not understood (a verdict map, a queue).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    conds: Vec<Cond>,
    action: Action,
}

impl Rule {
    fn holds(&self, p: Packet) -> Tri {
        self.conds
            .iter()
            .fold(Tri::Yes, |acc, c| acc.and(c.holds(p)))
    }
}

#[derive(Debug, Clone, Default)]
struct Chain {
    /// The table it is in, which a jump names chains within.
    table: String,
    rules: Vec<Rule>,
    /// A filter chain a packet for this computer passes and may be dropped
    /// in: the input hook, and (PR #594 review I2) prerouting and ingress,
    /// or iptables' raw and mangle PREROUTING.
    input: bool,
    /// A base chain's policy; true when it accepts (nftables' default).
    policy_accept: bool,
}

/// How a chain ends for a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Accept,
    Drop,
    /// Back to the chain that jumped here.
    Return,
    Unsure,
}

/// The rules one packet may be checked against before the answer is
/// "unsure" (PR #594 review I3: a blocklist of thousands of addresses).
const MAX_RULES: usize = 10_000;
/// The jumps deep a packet may go before the answer is "unsure".
const MAX_DEPTH: usize = 64;

/// A ruleset as read: its chains, by table and name.
#[derive(Debug, Clone, Default)]
pub struct Ruleset {
    chains: HashMap<String, Chain>,
    /// The input chains, in the order read.
    inputs: Vec<String>,
    /// It holds something not read here (an `include`, script commands), so
    /// it is never judged open.
    pub partial: bool,
    /// Arch's stock `/etc/nftables.conf` (or its rules loaded): lets in ssh
    /// only.
    pub stock_arch: bool,
}

fn chain_key(table: &str, name: &str) -> String {
    format!("{table} {name}")
}

impl Ruleset {
    /// No chain filters what comes in.
    pub fn filters_nothing(&self) -> bool {
        self.inputs.is_empty()
    }

    /// What the input chains do with `p`: shut if any drops it (a drop is
    /// final), open only if each accepts it.
    pub fn verdict(&self, p: Packet) -> Verdict {
        let v = self
            .inputs
            .iter()
            .map(|key| self.base_verdict(key, p))
            .fold(Verdict::Open, Verdict::and);
        if self.partial && v == Verdict::Open {
            Verdict::Unsure
        } else {
            v
        }
    }

    /// Every base chain that does not let `p` in, as `nft` names it
    /// (`inet filter input`), for the lines that open it: each must take
    /// them (PR #594 review I4).
    pub fn stopping_chains(&self, p: Packet) -> Vec<String> {
        self.inputs
            .iter()
            .filter(|key| self.base_verdict(key, p) != Verdict::Open)
            .cloned()
            .collect()
    }

    /// One base chain's verdict, walked without recursion (PR #594 review
    /// I3): a stack of the chains jumped to, at most [`MAX_DEPTH`] deep,
    /// and at most [`MAX_RULES`] rules checked.
    ///
    /// A rule that may or may not hold leaves the answer known only if
    /// every way ends alike, so each chain keeps the one ending its
    /// may-hold rules all agree on (`common`); one that differs is unsure.
    fn base_verdict(&self, key: &str, p: Packet) -> Verdict {
        let Some(chain) = self.chains.get(key) else {
            return Verdict::Unsure;
        };
        let policy = if chain.policy_accept {
            Flow::Accept
        } else {
            Flow::Drop
        };
        let flow = self.walk(chain, p, policy);
        match flow {
            Flow::Accept => Verdict::Open,
            Flow::Drop => Verdict::Shut,
            Flow::Return | Flow::Unsure => Verdict::Unsure,
        }
    }

    fn walk(&self, base: &Chain, p: Packet, policy: Flow) -> Flow {
        let mut stack = vec![Frame::new(base, policy)];
        let mut checked = 0usize;
        // What the frame just finished came to, for the one under it.
        let mut finished: Option<Flow> = None;
        loop {
            let Some(top) = stack.last_mut() else {
                return Flow::Unsure;
            };
            if let Some(child) = finished.take() {
                // Back from a jump or goto made by the rule before `at`.
                let (holds, jumped) = top.waiting.take().unwrap_or((Tri::Yes, true));
                let taken = match child {
                    Flow::Return if jumped => None,
                    flow => Some(flow),
                };
                if let Some(flow) = taken
                    && let Some(done) = top.take(holds, flow)
                {
                    stack.pop();
                    if stack.is_empty() {
                        return done;
                    }
                    finished = Some(done);
                    continue;
                }
            }
            // Run the top chain until it ends or calls another.
            let mut call: Option<(&Chain, Flow, Tri, bool)> = None;
            let mut done: Option<Flow> = None;
            let chain = top.chain;
            while let Some(rule) = chain.rules.get(top.at) {
                top.at += 1;
                checked += 1;
                if checked > MAX_RULES {
                    return Flow::Unsure;
                }
                let holds = rule.holds(p);
                if holds == Tri::No {
                    continue;
                }
                let flow = match &rule.action {
                    Action::Next => continue,
                    Action::Accept => Flow::Accept,
                    Action::Drop => Flow::Drop,
                    Action::Return => top.end,
                    Action::Unknown => Flow::Unsure,
                    Action::Jump(name) | Action::Goto(name) => {
                        let jump = matches!(rule.action, Action::Jump(_));
                        match self.chains.get(&chain_key(&chain.table, name)) {
                            Some(next) if stack_len_ok(top.depth) => {
                                let end = if jump { Flow::Return } else { top.end };
                                call = Some((next, end, holds, jump));
                                break;
                            }
                            _ => Flow::Unsure,
                        }
                    }
                };
                if let Some(result) = top.take(holds, flow) {
                    done = Some(result);
                    break;
                }
            }
            if let Some((next, end, holds, jump)) = call {
                top.waiting = Some((holds, jump));
                let depth = top.depth + 1;
                let mut frame = Frame::new(next, end);
                frame.depth = depth;
                stack.push(frame);
                continue;
            }
            let result = done.unwrap_or_else(|| top.finish(top.end));
            stack.pop();
            if stack.is_empty() {
                return result;
            }
            finished = Some(result);
        }
    }
}

fn stack_len_ok(depth: usize) -> bool {
    depth + 1 < MAX_DEPTH
}

/// One chain being walked.
struct Frame<'a> {
    chain: &'a Chain,
    /// The next rule.
    at: usize,
    /// What falling off it (or `return`) means: a base chain's policy,
    /// back to the chain that jumped here, or (gone to) whatever ends the
    /// chain that went.
    end: Flow,
    /// The ending its may-hold rules so far all agree on.
    common: Option<Flow>,
    /// The rule that called the chain above: whether it holds, and whether
    /// it jumped (or went).
    waiting: Option<(Tri, bool)>,
    depth: usize,
}

impl<'a> Frame<'a> {
    fn new(chain: &'a Chain, end: Flow) -> Self {
        Frame {
            chain,
            at: 0,
            end,
            common: None,
            waiting: None,
            depth: 0,
        }
    }

    /// The chain's ending when this is its last word.
    fn finish(&self, flow: Flow) -> Flow {
        match self.common {
            Some(c) if c != flow => Flow::Unsure,
            _ => flow,
        }
    }

    /// A rule that would end the chain with `flow`: the chain's ending if
    /// it surely holds (or the may-hold rules already disagree), else None
    /// and on to the next rule.
    fn take(&mut self, holds: Tri, flow: Flow) -> Option<Flow> {
        if holds == Tri::Yes {
            return Some(self.finish(flow));
        }
        match self.common {
            Some(c) if c != flow => Some(Flow::Unsure),
            _ => {
                self.common = Some(flow);
                None
            }
        }
    }
}

// ---------------------------------------------------------------- nftables

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Str(String),
    Open,
    Close,
    Comma,
    /// A newline or `;`.
    End,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '#' => {
                while chars.peek().is_some_and(|&c| c != '\n') {
                    chars.next();
                }
            }
            '\\' => {
                // A line continued.
                chars.next();
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    s.push(c);
                }
                toks.push(Tok::Str(s));
            }
            '{' | '}' | ',' | ';' | '\n' => {
                chars.next();
                toks.push(match c {
                    '{' => Tok::Open,
                    '}' => Tok::Close,
                    ',' => Tok::Comma,
                    _ => Tok::End,
                });
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let mut w = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | ',' | ';' | '"' | '#') {
                        break;
                    }
                    w.push(c);
                    chars.next();
                }
                toks.push(Tok::Word(w));
            }
        }
    }
    toks
}

/// Tokens to the end of a statement at this depth (a newline or `;`, or a
/// `}` closing the block it is in, which is left unread), braces inside it
/// kept.
fn statement(toks: &[Tok], at: &mut usize) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    while let Some(t) = toks.get(*at) {
        match t {
            Tok::End if depth == 0 => {
                *at += 1;
                break;
            }
            Tok::Close if depth == 0 => break,
            Tok::Open => depth += 1,
            Tok::Close => depth -= 1,
            _ => {}
        }
        out.push(t.clone());
        *at += 1;
    }
    out
}

fn word(t: Option<&Tok>) -> Option<&str> {
    match t {
        Some(Tok::Word(w)) => Some(w),
        _ => None,
    }
}

struct NftReader {
    rs: Ruleset,
    /// Named sets, by table and name: their elements.
    sets: HashMap<String, Vec<String>>,
    defines: HashMap<String, Vec<String>>,
}

/// Read nftables rules: `nft list ruleset`'s output, or the file it loads
/// (`/etc/nftables.conf`).
pub fn parse_nft(text: &str) -> Ruleset {
    let toks = tokenize(text);
    let mut r = NftReader {
        rs: Ruleset::default(),
        sets: HashMap::new(),
        defines: HashMap::new(),
    };
    // Its header in the file, or its comments in the rules loaded.
    r.rs.stock_arch = text.contains("Simple & Safe firewall")
        || (text.contains("\"allow sshd\"") && text.contains("\"allow tracked connections\""));
    // Sets first, so a rule may name one defined below it.
    let mut pending: Vec<(String, String, Vec<Vec<Tok>>)> = Vec::new();
    let mut at = 0;
    while at < toks.len() {
        match &toks[at] {
            Tok::End | Tok::Close | Tok::Comma => at += 1,
            Tok::Word(w) if w == "table" => {
                at += 1;
                let mut names = Vec::new();
                while let Some(Tok::Word(w)) = toks.get(at) {
                    names.push(w.clone());
                    at += 1;
                }
                if toks.get(at) != Some(&Tok::Open) {
                    // `table inet filter` alone, or `delete table ...`.
                    statement(&toks, &mut at);
                    continue;
                }
                at += 1;
                let table = match names.as_slice() {
                    [name] => format!("ip {name}"),
                    [family, name, ..] => format!("{family} {name}"),
                    [] => "ip ?".to_string(),
                };
                r.table(&toks, &mut at, &table, &mut pending);
            }
            Tok::Word(w) if w == "define" => {
                at += 1;
                let st = statement(&toks, &mut at);
                if let Some(Tok::Word(name)) = st.first() {
                    let mut i = 1;
                    if word(st.get(i)) == Some("=") {
                        i += 1;
                    }
                    let values = r.values(&st, &mut i, "");
                    if let Some(values) = values {
                        r.defines.insert(name.clone(), values);
                    }
                }
            }
            Tok::Word(w) if matches!(w.as_str(), "flush" | "destroy" | "delete") => {
                statement(&toks, &mut at);
            }
            _ => {
                // `include`, `add rule ...` and the like: rules not read.
                r.rs.partial = true;
                let st = statement(&toks, &mut at);
                if st.is_empty() {
                    at += 1;
                }
            }
        }
    }
    for (table, name, statements) in pending {
        r.chain(&table, &name, statements);
    }
    r.rs
}

impl NftReader {
    fn table(
        &mut self,
        toks: &[Tok],
        at: &mut usize,
        table: &str,
        pending: &mut Vec<(String, String, Vec<Vec<Tok>>)>,
    ) {
        while let Some(t) = toks.get(*at) {
            match t {
                Tok::Close => {
                    *at += 1;
                    return;
                }
                Tok::End | Tok::Comma => *at += 1,
                Tok::Word(w) if w == "chain" || w == "set" => {
                    let kind = w.clone();
                    let name = match toks.get(*at + 1) {
                        Some(Tok::Word(n)) | Some(Tok::Str(n)) => n.clone(),
                        _ => String::new(),
                    };
                    if toks.get(*at + 2) != Some(&Tok::Open) {
                        statement(toks, at);
                        continue;
                    }
                    *at += 3;
                    let mut body = Vec::new();
                    while let Some(t) = toks.get(*at) {
                        if *t == Tok::Close {
                            *at += 1;
                            break;
                        }
                        let st = statement(toks, at);
                        if !st.is_empty() {
                            body.push(st);
                        }
                    }
                    if kind == "chain" {
                        pending.push((table.to_string(), name, body));
                    } else {
                        self.set(table, &name, &body);
                    }
                }
                _ => {
                    // A map, a flowtable, a named counter, flags, a comment.
                    let mut i = *at;
                    statement(toks, &mut i);
                    *at = i.max(*at + 1);
                }
            }
        }
    }

    fn set(&mut self, table: &str, name: &str, body: &[Vec<Tok>]) {
        for st in body {
            if word(st.first()) == Some("elements") {
                let mut i = 1;
                if word(st.get(i)) == Some("=") {
                    i += 1;
                }
                if let Some(values) = self.values(st, &mut i, table) {
                    self.sets.insert(chain_key(table, name), values);
                }
            }
        }
    }

    fn chain(&mut self, table: &str, name: &str, body: Vec<Vec<Tok>>) {
        let mut chain = Chain {
            table: table.to_string(),
            policy_accept: true,
            ..Chain::default()
        };
        // Only IPv4 traffic is judged: `ip` and `inet` tables, and `netdev`
        // ingress, which sees every packet.
        let family = table.split(' ').next().unwrap_or("");
        for st in body {
            let words: Vec<&str> = st.iter().filter_map(|t| word(Some(t))).collect();
            match words.first().copied() {
                Some("type") => {
                    let hook = words
                        .iter()
                        .position(|w| *w == "hook")
                        .and_then(|i| words.get(i + 1))
                        .copied();
                    // A drop in prerouting or ingress is as final as one in
                    // input (PR #594 review I2); an accept there only
                    // passes the packet on.
                    let passes = matches!(
                        (family, hook),
                        ("ip" | "inet", Some("input" | "prerouting" | "ingress"))
                            | ("netdev", Some("ingress"))
                    );
                    chain.input = passes && matches!(words.get(1), Some(&"filter"));
                    if let Some(i) = words.iter().position(|w| *w == "policy") {
                        chain.policy_accept = words.get(i + 1) != Some(&"drop");
                    }
                }
                Some("policy") => chain.policy_accept = words.get(1) != Some(&"drop"),
                Some("comment") | Some("devices") | Some("device") | None => {}
                _ => {
                    let rule = self.rule(&st, table);
                    chain.rules.push(rule);
                }
            }
        }
        let key = chain_key(table, name);
        if chain.input {
            self.rs.inputs.push(key.clone());
        }
        self.rs.chains.insert(key, chain);
    }

    /// A value at `i`: a word (and words after commas), a string, a `{ }`
    /// set, a named `@set` or a `$define`. None when it names a set or
    /// define not found.
    fn values(&self, st: &[Tok], i: &mut usize, table: &str) -> Option<Vec<String>> {
        // A verdict map or a map is no value (PR #594 review C1): left
        // unread for the rule to see.
        if matches!(word(st.get(*i)), Some("vmap" | "map")) {
            return None;
        }
        match st.get(*i) {
            Some(Tok::Open) => {
                *i += 1;
                let mut out = Vec::new();
                let mut known = true;
                let mut element = String::new();
                while let Some(t) = st.get(*i) {
                    *i += 1;
                    match t {
                        Tok::Close => break,
                        Tok::Comma | Tok::End => {
                            if !element.is_empty() {
                                out.push(std::mem::take(&mut element));
                            }
                        }
                        Tok::Word(w) | Tok::Str(w) => {
                            if let Some(inner) = self.reference(w, table) {
                                match inner {
                                    Some(v) => out.extend(v),
                                    None => known = false,
                                }
                            } else {
                                if !element.is_empty() {
                                    element.push(' ');
                                }
                                element.push_str(w);
                            }
                        }
                        Tok::Open => known = false,
                    }
                }
                if !element.is_empty() {
                    out.push(element);
                }
                known.then_some(out)
            }
            Some(Tok::Word(w)) => {
                *i += 1;
                if let Some(inner) = self.reference(w, table) {
                    return inner;
                }
                let mut out = vec![w.clone()];
                while st.get(*i) == Some(&Tok::Comma) {
                    match st.get(*i + 1) {
                        Some(Tok::Word(w)) | Some(Tok::Str(w)) => {
                            out.push(w.clone());
                            *i += 2;
                        }
                        _ => break,
                    }
                }
                Some(out)
            }
            Some(Tok::Str(s)) => {
                *i += 1;
                Some(vec![s.clone()])
            }
            _ => None,
        }
    }

    /// `@set` or `$define`: Some(its values, or None when not found); None
    /// for any other word.
    fn reference(&self, w: &str, table: &str) -> Option<Option<Vec<String>>> {
        if let Some(set) = w.strip_prefix('@') {
            Some(self.sets.get(&chain_key(table, set)).cloned())
        } else {
            w.strip_prefix('$').map(|d| self.defines.get(d).cloned())
        }
    }

    /// One rule's conditions and verdict.
    fn rule(&self, st: &[Tok], table: &str) -> Rule {
        let mut conds = Vec::new();
        let mut action = Action::Next;
        let mut i = 0;
        // `!=` before a value; `==` is the same as none.
        let neg = |st: &[Tok], i: &mut usize| -> bool {
            match word(st.get(*i)) {
                Some("!=") => {
                    *i += 1;
                    true
                }
                Some("==") | Some("eq") => {
                    *i += 1;
                    false
                }
                _ => false,
            }
        };
        while let Some(t) = st.get(i) {
            let Tok::Word(w) = t else {
                if *t == Tok::Open {
                    // A set where none was expected; after `vmap` or `map`
                    // (left by `values`), its verdicts are not read.
                    if matches!(word(i.checked_sub(1).and_then(|j| st.get(j))), Some("vmap")) {
                        action = Action::Unknown;
                    }
                    conds.push(Cond::Unknown);
                    let mut depth = 0;
                    while let Some(t) = st.get(i) {
                        i += 1;
                        match t {
                            Tok::Open => depth += 1,
                            Tok::Close => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                i += 1;
                continue;
            };
            i += 1;
            match w.as_str() {
                "counter" => {
                    if word(st.get(i)) == Some("packets") {
                        i += 4;
                    } else if word(st.get(i)) == Some("name") {
                        i += 2;
                    }
                }
                "log" => {
                    while let Some(k) = word(st.get(i)) {
                        if matches!(
                            k,
                            "prefix" | "level" | "group" | "snaplen" | "queue-threshold" | "flags"
                        ) {
                            i += 1;
                            let _ = self.values(st, &mut i, table);
                        } else {
                            break;
                        }
                    }
                }
                "comment" => {
                    let _ = self.values(st, &mut i, table);
                }
                "limit" => {
                    // Over the rate, packets go on to the next rule, so a
                    // limit holds for some packets only (PR #594 review
                    // N1), either way round.
                    conds.push(Cond::Unknown);
                    if word(st.get(i)) == Some("rate") {
                        i += 1;
                    }
                    if word(st.get(i)) == Some("over") {
                        i += 1;
                    }
                    i += 1; // N/second
                    if word(st.get(i)) == Some("burst") {
                        i += 2;
                        if matches!(
                            word(st.get(i)),
                            Some("packets" | "bytes" | "kbytes" | "mbytes")
                        ) {
                            i += 1;
                        }
                    }
                }
                "ct" => {
                    let key = word(st.get(i)).unwrap_or("").to_string();
                    i += 1;
                    if word(st.get(i)) == Some("set") {
                        i += 1;
                        let _ = self.values(st, &mut i, table);
                        continue;
                    }
                    if key == "state"
                        && word(st.get(i + 1)).is_none()
                        && word(st.get(i)) == Some("vmap")
                    {
                        // `ct state vmap { established : accept, new : drop }`:
                        // a new packet takes the verdict of `new`, if any.
                        i += 1;
                        action = match self.values(st, &mut i, table) {
                            Some(elements) => ct_state_vmap(&elements),
                            None => Action::Unknown,
                        };
                        continue;
                    }
                    let n = neg(st, &mut i);
                    let values = self.values(st, &mut i, table);
                    conds.push(match (key.as_str(), values) {
                        ("state", Some(states)) => Cond::CtState { states, neg: n },
                        _ => Cond::Unknown,
                    });
                }
                "iif" | "iifname" => {
                    let n = neg(st, &mut i);
                    conds.push(match self.values(st, &mut i, table) {
                        Some(names) => Cond::Iface { names, neg: n },
                        None => Cond::Unknown,
                    });
                }
                "pkttype" => {
                    let n = neg(st, &mut i);
                    conds.push(match self.values(st, &mut i, table) {
                        Some(kinds) => Cond::PktType { kinds, neg: n },
                        None => Cond::Unknown,
                    });
                }
                "meta" => {
                    let key = word(st.get(i)).unwrap_or("").to_string();
                    i += 1;
                    if word(st.get(i)) == Some("set") {
                        i += 1;
                        let _ = self.values(st, &mut i, table);
                        continue;
                    }
                    let n = neg(st, &mut i);
                    let values = self.values(st, &mut i, table);
                    conds.push(match (key.as_str(), values) {
                        ("l4proto", Some(protos)) => Cond::Proto { protos, neg: n },
                        ("iif" | "iifname", Some(names)) => Cond::Iface { names, neg: n },
                        ("pkttype", Some(kinds)) => Cond::PktType { kinds, neg: n },
                        ("nfproto", Some(v)) => Cond::Ipv4(v.iter().any(|f| f == "ipv4") != n),
                        _ => Cond::Unknown,
                    });
                }
                "ip" => {
                    let key = word(st.get(i)).unwrap_or("").to_string();
                    i += 1;
                    let n = neg(st, &mut i);
                    let values = self.values(st, &mut i, table);
                    conds.push(match (key.as_str(), values) {
                        ("protocol", Some(protos)) => Cond::Proto { protos, neg: n },
                        ("daddr", Some(addrs)) => Cond::DAddr { addrs, neg: n },
                        _ => Cond::Unknown,
                    });
                }
                "ip6" | "icmp" | "icmpv6" | "sctp" | "dccp" | "udplite" | "igmp" => {
                    // Never an IPv4 TCP or UDP packet.
                    i += 1;
                    let _ = neg(st, &mut i);
                    let _ = self.values(st, &mut i, table);
                    conds.push(Cond::Never);
                }
                "tcp" | "udp" | "th" => {
                    let proto = match w.as_str() {
                        "tcp" => Some(Proto::Tcp),
                        "udp" => Some(Proto::Udp),
                        _ => None,
                    };
                    let key = word(st.get(i)).unwrap_or("").to_string();
                    i += 1;
                    if key == "dport" && word(st.get(i)) == Some("vmap") {
                        i += 1;
                        let _ = self.values(st, &mut i, table);
                        conds.push(Cond::DPort {
                            proto,
                            ports: Ports {
                                ranges: Vec::new(),
                                unknown: true,
                            },
                            neg: false,
                        });
                        action = Action::Unknown;
                        continue;
                    }
                    if key == "dport" {
                        let n = neg(st, &mut i);
                        let values = self.values(st, &mut i, table);
                        conds.push(Cond::DPort {
                            proto,
                            ports: Ports::from_values(values.as_deref()),
                            neg: n,
                        });
                    } else {
                        // sport, flags, ...: of this protocol, else not known.
                        conds.push(Cond::DPort {
                            proto,
                            ports: Ports {
                                ranges: Vec::new(),
                                unknown: true,
                            },
                            neg: false,
                        });
                        if key == "sport" {
                            let _ = neg(st, &mut i);
                            let _ = self.values(st, &mut i, table);
                        }
                    }
                }
                "accept" => action = Action::Accept,
                "drop" => action = Action::Drop,
                "reject" => {
                    action = Action::Drop;
                    break; // `with icmpx type ...`
                }
                "jump" | "goto" => {
                    let target = match st.get(i) {
                        Some(Tok::Word(n)) | Some(Tok::Str(n)) => n.clone(),
                        _ => String::new(),
                    };
                    i += 1;
                    action = if w == "jump" {
                        Action::Jump(target)
                    } else {
                        Action::Goto(target)
                    };
                }
                "return" => action = Action::Return,
                "continue" | "notrack" => {}
                "queue" | "vmap" => {
                    action = Action::Unknown;
                    let _ = self.values(st, &mut i, table);
                }
                "xt" => {
                    // iptables-nft's own matches, which nft cannot show.
                    conds.push(Cond::Unknown);
                    i += 2;
                }
                _ => conds.push(Cond::Unknown),
            }
        }
        Rule { conds, action }
    }
}

/// The verdict a new packet takes from `ct state vmap`'s elements
/// (`established : accept`, `new : jump x`; a key may list several
/// states): its `new` element's, or none (on to the next rule) without one.
fn ct_state_vmap(elements: &[String]) -> Action {
    for element in elements {
        let Some((keys, verdict)) = element.split_once(" : ") else {
            return Action::Unknown;
        };
        if !keys.split_whitespace().any(|k| k == "new") {
            continue;
        }
        let words: Vec<&str> = verdict.split_whitespace().collect();
        return match words.as_slice() {
            ["accept"] => Action::Accept,
            ["drop"] | ["reject", ..] => Action::Drop,
            ["return"] => Action::Return,
            ["continue"] => Action::Next,
            ["jump", chain] => Action::Jump((*chain).to_string()),
            ["goto", chain] => Action::Goto((*chain).to_string()),
            _ => Action::Unknown,
        };
    }
    Action::Next
}

// ---------------------------------------------------------------- iptables

/// Words of an `iptables-save` line, with quotes taken off.
fn shell_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                started = false;
            }
            None => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Read iptables rules in `iptables-save`'s form (its output, or the file
/// `iptables.service` restores): the filter table's INPUT chain, and the
/// raw and mangle tables' PREROUTING (and mangle's INPUT), where a drop
/// is as final (PR #594 review I2). Chains are keyed `ip <table> <chain>`.
pub fn parse_iptables(text: &str) -> Ruleset {
    let mut rs = Ruleset::default();
    let mut table: Option<String> = None;
    let mut rules: Vec<(String, String, Vec<String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line == "COMMIT" {
            continue;
        }
        if let Some(name) = line.strip_prefix('*') {
            let name = name.trim();
            table = matches!(name, "filter" | "raw" | "mangle").then(|| format!("ip {name}"));
            continue;
        }
        let Some(table) = table.as_deref() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix(':') {
            let mut words = rest.split_whitespace();
            let name = words.next().unwrap_or("").to_string();
            let policy = words.next().unwrap_or("-");
            let input = matches!(
                (table, name.as_str()),
                ("ip filter", "INPUT")
                    | ("ip raw", "PREROUTING")
                    | ("ip mangle", "PREROUTING")
                    | ("ip mangle", "INPUT")
            );
            rs.chains.insert(
                chain_key(table, &name),
                Chain {
                    table: table.to_string(),
                    rules: Vec::new(),
                    input,
                    policy_accept: policy != "DROP" && policy != "REJECT",
                },
            );
            if input {
                rs.inputs.push(chain_key(table, &name));
            }
            continue;
        }
        let words = shell_words(line);
        match words.first().map(String::as_str) {
            Some("-A") | Some("--append") if words.len() > 1 => {
                rules.push((table.to_string(), words[1].clone(), words[2..].to_vec()));
            }
            // `-I`, `-N`, `-P` in a hand-written file: not read here.
            _ => rs.partial = true,
        }
    }
    for (table, chain, args) in rules {
        let rule = iptables_rule(&args, |name| {
            rs.chains.contains_key(&chain_key(&table, name))
        });
        match rs.chains.get_mut(&chain_key(&table, &chain)) {
            Some(c) => c.rules.push(rule),
            None => rs.partial = true,
        }
    }
    // A filter table with no INPUT filters nothing coming in; raw or
    // mangle alone (Docker's) do not make a firewall.
    if !rs.inputs.iter().any(|k| k == "ip filter INPUT") {
        rs.inputs.clear();
    }
    rs
}

fn iptables_rule(args: &[String], is_chain: impl Fn(&str) -> bool) -> Rule {
    let mut conds = Vec::new();
    let mut action = Action::Next;
    let mut proto: Option<Proto> = None;
    let mut neg = false;
    let mut i = 0;
    let value = |i: &mut usize| -> String {
        let v = args.get(*i).cloned().unwrap_or_default();
        *i += 1;
        v
    };
    let list = |v: &str| -> Vec<String> { v.split(',').map(|s| s.trim().to_string()).collect() };
    while i < args.len() {
        let opt = args[i].clone();
        i += 1;
        let n = std::mem::take(&mut neg);
        match opt.as_str() {
            "!" => neg = true,
            "-4" | "-6" => {}
            "-p" | "--protocol" => {
                let v = value(&mut i).to_ascii_lowercase();
                proto = match v.as_str() {
                    "tcp" | "6" => Some(Proto::Tcp),
                    "udp" | "17" => Some(Proto::Udp),
                    _ => None,
                };
                if v != "all" && v != "0" {
                    conds.push(Cond::Proto {
                        protos: vec![v],
                        neg: n,
                    });
                }
            }
            "--dport" | "--destination-port" | "--dports" | "--destination-ports" => {
                let v = value(&mut i);
                conds.push(Cond::DPort {
                    proto,
                    ports: Ports::from_values(Some(&list(&v))),
                    neg: n,
                });
            }
            "-m" | "--match" => {
                // A match whose options are read here adds its conditions
                // by them; any other (`socket`, `hashlimit`, `limit`, which
                // holds only under its rate) is a condition not understood
                // (PR #594 review I5, N1).
                let module = value(&mut i);
                if !matches!(
                    module.as_str(),
                    "tcp"
                        | "udp"
                        | "multiport"
                        | "conntrack"
                        | "state"
                        | "comment"
                        | "addrtype"
                        | "pkttype"
                        | "icmp"
                ) {
                    conds.push(Cond::Unknown);
                }
            }
            "--comment" | "--limit" | "--limit-burst" | "--log-prefix" | "--log-level"
            | "--reject-with" | "--icmp-type" | "--sport" | "--source-port" | "--sports"
            | "--source-ports" => {
                let _ = value(&mut i);
                if opt.contains("sport") || opt.contains("source-port") {
                    conds.push(Cond::Unknown);
                }
            }
            "--state" | "--ctstate" => {
                let v = value(&mut i);
                conds.push(Cond::CtState {
                    states: list(&v),
                    neg: n,
                });
            }
            "-i" | "--in-interface" => {
                let v = value(&mut i).replace('+', "*");
                conds.push(Cond::Iface {
                    names: vec![v],
                    neg: n,
                });
            }
            "-d" | "--destination" => {
                let v = value(&mut i);
                conds.push(Cond::DAddr {
                    addrs: list(&v),
                    neg: n,
                });
            }
            "--pkt-type" | "--dst-type" => {
                let v = value(&mut i);
                conds.push(Cond::PktType {
                    kinds: list(&v),
                    neg: n,
                });
            }
            "-j" | "--jump" | "-g" | "--goto" => {
                let target = value(&mut i);
                action = match target.as_str() {
                    "ACCEPT" => Action::Accept,
                    "DROP" | "REJECT" => Action::Drop,
                    "RETURN" => Action::Return,
                    // Targets that let the packet go on (raw and mangle's).
                    "LOG" | "NFLOG" | "ULOG" | "MARK" | "CONNMARK" | "CT" | "TRACE" | "AUDIT"
                    | "NOTRACK" | "TOS" | "DSCP" | "TTL" | "HL" | "TCPMSS" | "CLASSIFY"
                    | "CHECKSUM" | "SECMARK" | "CONNSECMARK" => Action::Next,
                    t if is_chain(t) && matches!(opt.as_str(), "-g" | "--goto") => {
                        Action::Goto(t.to_string())
                    }
                    t if is_chain(t) => Action::Jump(t.to_string()),
                    _ => Action::Unknown,
                };
                // A target's own options follow; nothing after decides.
                break;
            }
            _ => {
                conds.push(Cond::Unknown);
                // Its values, up to the next option.
                while args.get(i).is_some_and(|a| !a.starts_with('-') && a != "!") {
                    i += 1;
                }
            }
        }
    }
    Rule { conds, action }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Arch's stock /etc/nftables.conf (nftables 1.1).
    pub const ARCH_STOCK: &str = r#"#!/usr/bin/nft -f
# vim:set ts=2 sw=2 et:

# IPv4/IPv6 Simple & Safe firewall ruleset.
# More examples in /usr/share/nftables/ and /usr/share/doc/nftables/examples/.

destroy table inet filter
table inet filter {
  chain input {
    type filter hook input priority filter
    policy drop

    ct state invalid drop comment "early drop of invalid connections"
    ct state {established, related} accept comment "allow tracked connections"
    iif lo accept comment "allow from loopback"
    ip protocol icmp accept comment "allow icmp"
    meta l4proto ipv6-icmp accept comment "allow icmp v6"
    tcp dport ssh accept comment "allow sshd"
    pkttype host limit rate 5/second counter reject with icmpx type admin-prohibited
    counter
  }
  chain forward {
    type filter hook forward priority filter
    policy drop
  }
}
"#;

    fn tcp(port: u16) -> Packet {
        Packet {
            proto: Proto::Tcp,
            port,
            cast: Cast::Unicast,
        }
    }
    fn udp(port: u16) -> Packet {
        Packet {
            proto: Proto::Udp,
            port,
            cast: Cast::Unicast,
        }
    }

    #[test]
    fn arch_stock_lets_in_ssh_only() {
        let rs = parse_nft(ARCH_STOCK);
        assert!(rs.stock_arch);
        assert!(!rs.partial);
        assert_eq!(rs.verdict(tcp(22)), Verdict::Open);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Shut);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Shut);
        let open = PortsOpen::from_check(1716, 4716, |p| rs.verdict(p));
        assert_eq!(open, PortsOpen::all(Verdict::Shut));
        assert_eq!(rs.stopping_chains(tcp(1716)), vec!["inet filter input"]);
    }

    /// `nft list ruleset`'s own form of the same rules (semicolons, counters).
    #[test]
    fn the_listed_form_reads_the_same() {
        let listed = r#"table inet filter {
	chain input {
		type filter hook input priority filter; policy drop;
		ct state invalid drop comment "early drop of invalid connections"
		ct state { established, related } accept comment "allow tracked connections"
		iif "lo" accept comment "allow from loopback"
		ip protocol icmp accept comment "allow icmp"
		meta l4proto ipv6-icmp accept comment "allow icmp v6"
		tcp dport 22 accept comment "allow sshd"
		meta pkttype host limit rate 5/second burst 5 packets counter packets 3 bytes 180 reject with icmpx admin-prohibited
		counter packets 12 bytes 900
	}
}
"#;
        let rs = parse_nft(listed);
        assert_eq!(rs.verdict(tcp(22)), Verdict::Open);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Shut);
        let mdns = Packet {
            proto: Proto::Udp,
            port: 5353,
            cast: Cast::Multicast,
        };
        assert_eq!(rs.verdict(mdns), Verdict::Shut, "by the policy");
    }

    #[test]
    fn ports_let_in_by_rules_sets_and_defines() {
        let conf = ARCH_STOCK.replace(
            "    tcp dport ssh accept comment \"allow sshd\"\n",
            "    tcp dport ssh accept comment \"allow sshd\"\n    tcp dport { 1714-1764 } accept\n    udp dport @pd accept\n    meta l4proto udp th dport $kde accept\n",
        );
        let conf = conf.replace(
            "  chain forward {",
            "  set pd {\n    type inet_service\n    elements = { 4716, mdns }\n  }\n  chain forward {",
        );
        let conf = format!("define kde = {{ 1714-1764 }}\n{conf}");
        let rs = parse_nft(&conf);
        let open = PortsOpen::from_check(1716, 4716, |p| rs.verdict(p));
        assert_eq!(open, PortsOpen::all(Verdict::Open));
        assert_eq!(rs.verdict(tcp(1765)), Verdict::Shut);
    }

    /// Debian's stock file: chains with no policy accept everything.
    #[test]
    fn a_chain_without_a_policy_accepts() {
        let debian = "#!/usr/sbin/nft -f\n\nflush ruleset\n\ntable inet filter {\n\tchain input {\n\t\ttype filter hook input priority filter;\n\t}\n\tchain forward {\n\t\ttype filter hook forward priority filter;\n\t}\n}\n";
        let rs = parse_nft(debian);
        assert!(!rs.filters_nothing());
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Open);
        assert!(parse_nft("").filters_nothing());
    }

    #[test]
    fn a_drop_above_an_accept_wins() {
        let conf = "table inet f {\n chain input {\n  type filter hook input priority 0; policy accept;\n  tcp dport 1700-1800 drop\n  tcp dport 1716 accept\n }\n}\n";
        let rs = parse_nft(conf);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Shut);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Open);
    }

    #[test]
    fn jumps_and_gotos_are_followed() {
        let conf = "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  jump allowed\n  tcp dport 1716 goto deny\n }\n chain allowed {\n  udp dport 4716 accept\n  tcp dport 22 accept\n  return\n }\n chain deny {\n  counter\n }\n}\n";
        let rs = parse_nft(conf);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Open);
        assert_eq!(rs.verdict(tcp(22)), Verdict::Open);
        // Off the end of `deny` goes back past `input` to its policy.
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Shut);
        let missing = conf.replace("jump allowed", "jump nowhere");
        assert_eq!(parse_nft(&missing).verdict(udp(4716)), Verdict::Unsure);
    }

    /// A rule not understood is "unsure" only where it could decide.
    #[test]
    fn a_rule_not_understood_is_unsure_not_open() {
        let conf = "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  ip saddr 192.0.2.0/24 accept\n  tcp dport 22 accept\n }\n}\n";
        let rs = parse_nft(conf);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Unsure);
        assert_eq!(rs.verdict(tcp(22)), Verdict::Open);
        // Dropped either way: still shut.
        let both = conf.replace("accept\n  tcp dport 22", "drop\n  tcp dport 22");
        assert_eq!(parse_nft(&both).verdict(tcp(1716)), Verdict::Shut);
        // Other base chains still count: one that drops makes it shut.
        let two = format!(
            "{conf}table ip g {{\n chain in2 {{\n  type filter hook input priority 10; policy drop;\n }}\n}}\n"
        );
        assert_eq!(parse_nft(&two).verdict(tcp(1716)), Verdict::Shut);
        // An include may hold anything.
        let inc = format!(
            "include \"/etc/nftables.d/*.conf\"\n{}",
            ARCH_STOCK.replace("tcp dport ssh", "tcp dport { ssh, 1716, 4716 }")
        );
        let rs = parse_nft(&inc);
        assert!(rs.partial);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Unsure);
    }

    #[test]
    fn ipv6_only_and_loopback_rules_do_not_let_phones_in() {
        let conf = "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  iifname \"lo\" accept\n  iifname \"docker0\" accept\n  ip6 saddr fe80::/10 accept\n  iifname != \"lo\" tcp dport 1716 accept\n }\n}\ntable ip6 v6 {\n chain input {\n  type filter hook input priority 0; policy accept;\n }\n}\n";
        let rs = parse_nft(conf);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Shut);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Open);
    }

    /// PR #594 review C1: a verdict map on any key may decide, so its rule
    /// is never passed over as "no verdict".
    #[test]
    fn a_verdict_map_is_never_passed_over() {
        let chain = |rules: &str| {
            format!(
                "table inet f {{\n chain input {{\n  type filter hook input priority 0; policy drop;\n{rules}\n }}\n chain t {{\n  accept\n }}\n}}\n"
            )
        };
        let after = "  udp dport 4716 accept\n  tcp dport 1716 accept";
        for vmap in [
            "  ip protocol vmap { tcp : jump t, udp : drop }",
            "  meta l4proto vmap { udp : drop }",
            "  iifname vmap { \"eth0\" : drop }",
            "  th dport vmap { 4716 : drop }",
            "  meta mark vmap { 1 : drop }",
        ] {
            let rs = parse_nft(&chain(&format!("{vmap}\n{after}")));
            assert_ne!(rs.verdict(udp(4716)), Verdict::Open, "{vmap}");
        }
        // ct state's is read: a new packet takes `new`'s verdict.
        let ct = |m: &str| parse_nft(&chain(&format!("  ct state vmap {{ {m} }}\n{after}")));
        let rs = ct("established : accept, related : accept, invalid : drop");
        assert_eq!(
            rs.verdict(udp(4716)),
            Verdict::Open,
            "no `new`: on to the next rule"
        );
        let rs = ct("established : accept, new : drop");
        assert_eq!(rs.verdict(udp(4716)), Verdict::Shut);
        let rs = ct("new : jump t");
        assert_eq!(rs.verdict(tcp(9999)), Verdict::Open);
    }

    /// PR #594 review I2: a drop in prerouting or ingress stops the packet
    /// before input lets it in.
    #[test]
    fn a_drop_before_input_counts() {
        let open_input = "table inet f {\n chain input {\n  type filter hook input priority 0; policy accept;\n }\n}\n";
        assert_eq!(parse_nft(open_input).verdict(udp(4716)), Verdict::Open);
        let pre = format!(
            "{open_input}table inet r {{\n chain pre {{\n  type filter hook prerouting priority raw; policy accept;\n  udp dport 4716 drop\n }}\n}}\n"
        );
        assert_eq!(parse_nft(&pre).verdict(udp(4716)), Verdict::Shut);
        assert_eq!(parse_nft(&pre).verdict(tcp(1716)), Verdict::Open);
        let ingress = format!(
            "{open_input}table netdev n {{\n chain ing {{\n  type filter hook ingress device \"eth0\" priority -500; policy accept;\n  ip saddr 192.0.2.0/24 drop\n }}\n}}\n"
        );
        assert_eq!(parse_nft(&ingress).verdict(tcp(1716)), Verdict::Unsure);
        let raw = "*raw\n:PREROUTING ACCEPT [0:0]\n:OUTPUT ACCEPT [0:0]\n-A PREROUTING -p udp --dport 4716 -j DROP\nCOMMIT\n*filter\n:INPUT ACCEPT [0:0]\nCOMMIT\n";
        assert_eq!(parse_iptables(raw).verdict(udp(4716)), Verdict::Shut);
        assert_eq!(
            parse_iptables(raw).stopping_chains(udp(4716)),
            vec!["ip raw PREROUTING"]
        );
        let docker_only = "*raw\n:PREROUTING ACCEPT [0:0]\nCOMMIT\n";
        assert!(parse_iptables(docker_only).filters_nothing());
    }

    /// PR #594 review I3: a blocklist of 50 000 addresses is walked without
    /// recursion, and past the bound is "unsure", never "open".
    #[test]
    fn a_huge_blocklist_is_unsure_not_a_crash() {
        let mut conf = String::from(
            "table inet f {\n chain input {\n  type filter hook input priority 0; policy accept;\n",
        );
        for n in 0..50_000u32 {
            conf.push_str(&format!(
                "  ip saddr 198.51.{}.{} drop\n",
                (n / 256) % 256,
                n % 256
            ));
        }
        conf.push_str(" }\n}\n");
        let rs = parse_nft(&conf);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Unsure);
        // Within the bound, may-hold drops before a sure accept: unsure.
        let few = "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  ip saddr 198.51.100.1 drop\n  ip saddr 198.51.100.2 drop\n  tcp dport 1716 accept\n }\n}\n";
        assert_eq!(parse_nft(few).verdict(tcp(1716)), Verdict::Unsure);
        assert_eq!(parse_nft(few).verdict(udp(4716)), Verdict::Shut);
        // A chain of jumps deeper than the bound.
        let mut deep = String::from(
            "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  jump c0\n }\n",
        );
        for n in 0..100 {
            deep.push_str(&format!(" chain c{n} {{\n  jump c{}\n }}\n", n + 1));
        }
        deep.push_str(" chain c100 {\n  accept\n }\n}\n");
        assert_eq!(parse_nft(&deep).verdict(tcp(1716)), Verdict::Unsure);
    }

    /// PR #594 review N1: a rate limit holds for some packets only.
    #[test]
    fn a_rate_limited_accept_is_not_open() {
        let conf = "table inet f {\n chain input {\n  type filter hook input priority 0; policy drop;\n  tcp dport 1716 limit rate 10/second accept\n }\n}\n";
        assert_eq!(parse_nft(conf).verdict(tcp(1716)), Verdict::Unsure);
        let ipt = "*filter\n:INPUT DROP [0:0]\n-A INPUT -p tcp --dport 1716 -m limit --limit 10/sec -j ACCEPT\nCOMMIT\n";
        assert_eq!(parse_iptables(ipt).verdict(tcp(1716)), Verdict::Unsure);
    }

    /// PR #594 review I5: a match module not read here is a condition.
    #[test]
    fn an_iptables_match_not_read_is_a_condition() {
        let ipt = "*filter\n:INPUT DROP [0:0]\n-A INPUT -m socket -j ACCEPT\nCOMMIT\n";
        assert_eq!(parse_iptables(ipt).verdict(tcp(1716)), Verdict::Unsure);
        let known = "*filter\n:INPUT DROP [0:0]\n-A INPUT -p tcp -m tcp --dport 1716 -m comment --comment \"pd\" -j ACCEPT\nCOMMIT\n";
        assert_eq!(parse_iptables(known).verdict(tcp(1716)), Verdict::Open);
    }

    pub const IPTABLES_DROP: &str = "# Generated by iptables-save v1.8.11\n*filter\n:INPUT DROP [0:0]\n:FORWARD DROP [0:0]\n:OUTPUT ACCEPT [0:0]\n:TCP - [0:0]\n-A INPUT -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT\n-A INPUT -i lo -j ACCEPT\n-A INPUT -m conntrack --ctstate INVALID -j DROP\n-A INPUT -p icmp -m icmp --icmp-type 8 -m conntrack --ctstate NEW -j ACCEPT\n-A INPUT -p tcp --tcp-flags FIN,SYN,RST,ACK SYN -m conntrack --ctstate NEW -j TCP\n-A INPUT -p tcp -j REJECT --reject-with tcp-reset\n-A INPUT -p udp -j REJECT --reject-with icmp-port-unreachable\n-A INPUT -j REJECT --reject-with icmp-proto-unreachable\n-A TCP -p tcp -m tcp --dport 22 -m comment --comment \"allow ssh\" -j ACCEPT\nCOMMIT\n";

    #[test]
    fn iptables_save_is_read_too() {
        let rs = parse_iptables(IPTABLES_DROP);
        assert!(!rs.partial);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Shut);
        // The SYN check is not understood, but every way ends shut for 1716.
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Shut);
        // UDP let in above the REJECTs; TCP's line is in its own chain,
        // reached through the SYN check not understood: unsure.
        let open = IPTABLES_DROP
            .replace(
                "-A INPUT -i lo -j ACCEPT\n",
                "-A INPUT -i lo -j ACCEPT\n-A INPUT -p udp -m multiport --dports 4716,1714:1764,5353 -j ACCEPT\n",
            )
            .replace(
                "-A TCP -p tcp -m tcp --dport 22",
                "-A TCP -p tcp -m tcp --dport 1714:1764 -j ACCEPT\n-A TCP -p tcp -m tcp --dport 22",
            );
        let rs = parse_iptables(&open);
        assert_eq!(rs.verdict(udp(4716)), Verdict::Open);
        assert_eq!(rs.verdict(tcp(1716)), Verdict::Unsure);
        // A UDP line below the REJECTs comes too late.
        let late = format!(
            "{}-A INPUT -p udp --dport 4716 -j ACCEPT\nCOMMIT\n",
            IPTABLES_DROP.trim_end_matches("COMMIT\n")
        );
        assert_eq!(parse_iptables(&late).verdict(udp(4716)), Verdict::Shut);
        let empty =
            "*filter\n:INPUT ACCEPT [0:0]\n:FORWARD ACCEPT [0:0]\n:OUTPUT ACCEPT [0:0]\nCOMMIT\n";
        assert_eq!(parse_iptables(empty).verdict(tcp(1716)), Verdict::Open);
        assert!(parse_iptables("").filters_nothing());
    }
}
