//! A spare phone at home as the wake helper: an old Android phone left on a
//! charger at home sends the magic packet onto the home network for the
//! owner's other phones when they are away, so waking a sleeping computer
//! needs no forwarded port and no second computer left on.
//!
//! Who says what:
//!
//! - The helper phone, paired with the computer as one of its owner's phones,
//!   offers itself ([`OFFER`]: its own helper key and relays). The computer
//!   takes it only from an owner phone, and the key it keeps for the helper
//!   is never one a guest or a stranger named.
//! - The computer, while awake, tells the helper what to do and for whom
//!   ([`DUTY`]: the computer's own MAC and broadcast address, learned by the
//!   computer itself, and the iroh keys of the owner phones it holds), and
//!   tells each owner phone where its helper is ([`INFO`]).
//! - An owner phone, away, dials the helper over iroh ([`ALPN_WAKE`]) with
//!   its own device key and sends one line, [`Request`]; the helper answers
//!   with one line, [`Answer`].
//!
//! What the helper serves is one thing: "send the magic packet for this
//! computer". The MAC and the address it goes to come only from the
//! computer's own [`Duty`], never from the request, and a request is read
//! only from a key the computer introduced for that computer, at most once
//! every [`EVERY`] seconds with a burst of [`BURST`]. Anything else (an
//! unknown key, an unreadable line, a computer it holds no duty for) is
//! dropped or refused before a packet is sent. This module is the wire and
//! the rules; the listener is `streaming::wake_helper_net`, and the
//! computer's side is `server/src/wake_helper.rs`.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Helper phone → its computer: `{on, fresh, iroh, relays}`.
pub const OFFER: &str = "kdeconnect.pocketdesk.wake_helper";

/// Computer → its helper phone: `{computer, mac, broadcast, requesters}`, or
/// `{computer, refused: true}` when the owner removed the phone as helper.
pub const DUTY: &str = "kdeconnect.pocketdesk.wake_helper_duty";

/// Computer → an owner phone: `{computer, helper: {iroh, relays, name}}`, or
/// `{computer, helper: null}` when it has none.
pub const INFO: &str = "kdeconnect.pocketdesk.wake_helper_info";

/// The ALPN an owner phone dials the helper on.
pub const ALPN_WAKE: &[u8] = b"pocketdesk-wake/1";

/// A requester may ask at most once in this many seconds, after its burst.
pub const EVERY: u64 = 30;

/// A requester may ask this many times at once before [`EVERY`] applies.
pub const BURST: u32 = 2;

/// All requesters together: this many at once, then one every
/// [`ALL_EVERY`] seconds.
pub const ALL_BURST: u32 = 6;

/// See [`ALL_BURST`].
pub const ALL_EVERY: u64 = 10;

/// The most keys a duty may name, and so the most requesters a helper holds.
pub const MAX_REQUESTERS: usize = 64;

/// The most relay URLs an offer or an info may carry.
pub const MAX_RELAYS: usize = 4;

/// The longest relay URL, name or id taken, in bytes.
const MAX_FIELD: usize = 256;

/// The longest request line a helper reads, in bytes.
pub const MAX_REQUEST_BYTES: usize = 512;

/// Where a magic packet goes on a network.
pub const MAGIC_PORT: u16 = 9;

/// Whether `key` reads as an iroh endpoint id: 64 hex digits.
pub fn valid_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `key` as the one spelling the rules compare: lower case.
pub fn key_form(key: &str) -> String {
    key.to_ascii_lowercase()
}

/// A relay URL as the packets carry it: short, printable ASCII, `https`
/// only (the helper runs on n0's relays, and a plain `http` one is no road
/// an owner phone should be sent down). iroh reads it again before it is
/// used.
fn relay_reads(url: &str) -> bool {
    url.len() <= MAX_FIELD
        && url.starts_with("https://")
        && url.bytes().all(|b| b.is_ascii_graphic())
}

fn relays_in(value: Option<&Value>) -> Option<Vec<String>> {
    let Some(list) = value else {
        return Some(Vec::new());
    };
    let list = list.as_array()?;
    if list.len() > MAX_RELAYS {
        return None;
    }
    list.iter()
        .map(|v| v.as_str().filter(|u| relay_reads(u)).map(str::to_string))
        .collect()
}

/// A MAC as `AA:BB:CC:DD:EE:FF` or with dashes, as six bytes; none for
/// all-zero or all-one.
pub fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (slot, part) in out.iter_mut().zip(&parts) {
        if part.len() != 2 {
            return None;
        }
        *slot = u8::from_str_radix(part, 16).ok()?;
    }
    (out != [0; 6] && out != [0xFF; 6]).then_some(out)
}

// ---- The helper phone's offer ----

/// What a helper phone tells its computer about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Whether the phone is a helper now (`false`: it stopped).
    pub on: bool,
    /// The owner just turned the switch on, rather than the phone saying
    /// again that it still is: only that undoes a removal at the computer.
    pub fresh: bool,
    /// The helper endpoint's key.
    pub iroh: String,
    pub relays: Vec<String>,
}

impl Offer {
    pub fn body(&self) -> Value {
        json!({
            "on": self.on,
            "fresh": self.fresh,
            "iroh": self.iroh,
            "relays": self.relays,
        })
    }

    /// The offer in `body`, or `None` for one that does not read. An off
    /// offer needs no key.
    pub fn parse(body: &Value) -> Option<Self> {
        let on = body.get("on")?.as_bool()?;
        let fresh = body.get("fresh").and_then(Value::as_bool).unwrap_or(false);
        if !on {
            return Some(Self {
                on,
                fresh: false,
                iroh: String::new(),
                relays: Vec::new(),
            });
        }
        let iroh = body.get("iroh")?.as_str()?;
        if !valid_key(iroh) {
            return None;
        }
        Some(Self {
            on,
            fresh,
            iroh: key_form(iroh),
            relays: relays_in(body.get("relays"))?,
        })
    }
}

// ---- The computer's duty for its helper ----

/// What a helper does for one computer, from that computer.
#[derive(Clone, PartialEq, Eq)]
pub struct Duty {
    /// The computer's own device id, which a request names.
    pub computer: String,
    pub mac: [u8; 6],
    /// The broadcast address of the network the helper is on with the
    /// computer.
    pub broadcast: Ipv4Addr,
    /// The iroh keys of the owner phones that may ask, lower case.
    pub requesters: Vec<String>,
}

// Never printed whole: a MAC and keys stay out of logs.
impl std::fmt::Debug for Duty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Duty({} requesters)", self.requesters.len())
    }
}

/// What the computer says to its helper phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DutyMessage {
    Duty(Duty),
    /// The owner removed this phone as the computer's helper: it stops
    /// offering until its owner turns it on again.
    Refused {
        computer: String,
    },
    /// The computer cannot give this helper a duty now (it is on no network
    /// of the computer's, or only on a tunnel): the helper drops what it
    /// held for that computer, so a requester revoked meanwhile is not left
    /// served. A later duty replaces this.
    Withdrawn {
        computer: String,
    },
}

impl DutyMessage {
    pub fn body(&self) -> Value {
        match self {
            DutyMessage::Duty(d) => json!({
                "computer": d.computer,
                "mac": d.mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"),
                "broadcast": d.broadcast.to_string(),
                "requesters": d.requesters,
            }),
            DutyMessage::Refused { computer } => json!({ "computer": computer, "refused": true }),
            DutyMessage::Withdrawn { computer } => {
                json!({ "computer": computer, "withdrawn": true })
            }
        }
    }

    /// The message in `body`, or `None` for one that does not read: sizes
    /// are held before anything is copied.
    pub fn parse(body: &Value) -> Option<Self> {
        let computer = body.get("computer")?.as_str()?;
        if !super::kdeconnect::valid_device_id(computer) {
            return None;
        }
        if body.get("refused").and_then(Value::as_bool) == Some(true) {
            return Some(DutyMessage::Refused {
                computer: computer.to_string(),
            });
        }
        if body.get("withdrawn").and_then(Value::as_bool) == Some(true) {
            return Some(DutyMessage::Withdrawn {
                computer: computer.to_string(),
            });
        }
        let mac = parse_mac(body.get("mac")?.as_str()?)?;
        let broadcast: Ipv4Addr = body.get("broadcast")?.as_str()?.parse().ok()?;
        let list = body.get("requesters")?.as_array()?;
        if list.len() > MAX_REQUESTERS {
            return None;
        }
        let mut requesters = Vec::with_capacity(list.len());
        for key in list {
            let key = key.as_str().filter(|k| valid_key(k))?;
            requesters.push(key_form(key));
        }
        Some(DutyMessage::Duty(Duty {
            computer: computer.to_string(),
            mac,
            broadcast,
            requesters,
        }))
    }
}

// ---- The computer's word to an owner phone ----

/// Where an owner phone finds the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperInfo {
    pub iroh: String,
    pub relays: Vec<String>,
    /// The helper phone's name as the computer shows it.
    pub name: String,
}

/// What the computer tells an owner phone: its own id, and the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub computer: String,
    pub helper: Option<HelperInfo>,
}

impl Info {
    pub fn body(&self) -> Value {
        match &self.helper {
            Some(h) => json!({
                "computer": self.computer,
                "helper": { "iroh": h.iroh, "relays": h.relays, "name": h.name },
            }),
            None => json!({ "computer": self.computer, "helper": null }),
        }
    }

    pub fn parse(body: &Value) -> Option<Self> {
        let computer = body.get("computer")?.as_str()?;
        if !super::kdeconnect::valid_device_id(computer) {
            return None;
        }
        let helper = match body.get("helper")? {
            Value::Null => None,
            h => {
                let iroh = h.get("iroh")?.as_str()?;
                if !valid_key(iroh) {
                    return None;
                }
                let name = h.get("name").and_then(Value::as_str).unwrap_or("");
                if name.len() > MAX_FIELD || name.chars().any(char::is_control) {
                    return None;
                }
                Some(HelperInfo {
                    iroh: key_form(iroh),
                    relays: relays_in(h.get("relays"))?,
                    name: name.to_string(),
                })
            }
        };
        Some(Self {
            computer: computer.to_string(),
            helper,
        })
    }
}

// ---- The request and its answer, over iroh ----

/// What an owner phone asks the helper: to wake `computer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub computer: String,
}

impl Request {
    /// The line that asks it, newline included.
    pub fn line(&self) -> String {
        format!("{}\n", json!({ "wake": self.computer }))
    }

    /// The request in `line`, or `None` for anything else: nothing but the
    /// computer's id is read, and a line of any other shape is refused.
    pub fn parse(line: &[u8]) -> Option<Self> {
        if line.len() > MAX_REQUEST_BYTES {
            return None;
        }
        let value: Value = serde_json::from_slice(line).ok()?;
        let object = value.as_object()?;
        if object.len() != 1 {
            return None;
        }
        let computer = object.get("wake")?.as_str()?;
        super::kdeconnect::valid_device_id(computer).then(|| Self {
            computer: computer.to_string(),
        })
    }
}

/// What a helper answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The magic packet went onto the network.
    Sent,
    /// Too soon after the last: try again in a while.
    SlowDown,
    /// The helper holds nothing to wake that computer with.
    Unknown,
    /// The packet could not be sent.
    Failed,
}

impl Answer {
    pub fn code(self) -> &'static str {
        match self {
            Answer::Sent => "sent",
            Answer::SlowDown => "slow_down",
            Answer::Unknown => "unknown",
            Answer::Failed => "failed",
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code {
            "sent" => Answer::Sent,
            "slow_down" => Answer::SlowDown,
            "unknown" => Answer::Unknown,
            _ => Answer::Failed,
        }
    }

    /// The line that says it, newline included.
    pub fn line(self) -> String {
        format!("{}\n", json!({ "answer": self.code() }))
    }

    /// The answer in `line`; a line that does not read is a failure.
    pub fn parse(line: &[u8]) -> Self {
        if line.len() > MAX_REQUEST_BYTES {
            return Answer::Failed;
        }
        serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|v| v.get("answer").and_then(Value::as_str).map(Self::from_code))
            .unwrap_or(Answer::Failed)
    }
}

// ---- The rate limit ----

/// A token bucket: [`Bucket::take`] gives one token if any is left, and a
/// token comes back every `every`, up to `burst`.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(burst: u32, at: Instant) -> Self {
        Self {
            tokens: f64::from(burst),
            at,
        }
    }

    fn take(&mut self, burst: u32, every: Duration, now: Instant) -> bool {
        let gained = now.saturating_duration_since(self.at).as_secs_f64() / every.as_secs_f64();
        self.tokens = (self.tokens + gained).min(f64::from(burst));
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// How often requests are taken: per requester, and all together. Time is
/// given, so the rules are testable.
#[derive(Debug)]
pub struct Limiter {
    each: HashMap<String, Bucket>,
    all: Bucket,
}

impl Limiter {
    pub fn new(now: Instant) -> Self {
        Self {
            each: HashMap::new(),
            all: Bucket::full(ALL_BURST, now),
        }
    }

    /// Whether `requester` may wake something at `now`. Counted if so; a
    /// refusal by its own count does not use up a token of the shared one.
    pub fn take(&mut self, requester: &str, now: Instant) -> bool {
        // A bucket back to full holds nothing to remember.
        let every = Duration::from_secs(EVERY);
        self.each
            .retain(|_, b| now.saturating_duration_since(b.at) < every * BURST);
        let bucket = self
            .each
            .entry(key_form(requester))
            .or_insert_with(|| Bucket::full(BURST, now));
        if !bucket.take(BURST, every, now) {
            return false;
        }
        if !self
            .all
            .take(ALL_BURST, Duration::from_secs(ALL_EVERY), now)
        {
            // Give the requester's token back: it was not served.
            bucket.tokens += 1.0;
            return false;
        }
        true
    }
}

// ---- What the helper holds, and what it decides ----

/// The duties a helper holds, one per computer.
#[derive(Debug, Default, Clone)]
pub struct Duties {
    by_computer: HashMap<String, Duty>,
}

/// What a helper does with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not a key any computer introduced: close, answering nothing.
    Drop,
    /// Answer without sending.
    Answer(Answer),
    /// Send the magic packet for `mac` to `broadcast`.
    Send { mac: [u8; 6], broadcast: Ipv4Addr },
}

impl Duties {
    /// Hold `duty`, replacing the computer's earlier one. Past
    /// [`MAX_COMPUTERS`] an unknown computer's is not taken.
    pub fn set(&mut self, duty: Duty) -> bool {
        if !self.by_computer.contains_key(&duty.computer) && self.by_computer.len() >= MAX_COMPUTERS
        {
            return false;
        }
        self.by_computer.insert(duty.computer.clone(), duty);
        true
    }

    /// Forget a computer's duty: it removed this phone, or is unpaired.
    pub fn forget(&mut self, computer: &str) {
        self.by_computer.remove(computer);
    }

    /// Keep only the duties of the computers `keep` says are still held.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.by_computer.retain(|id, _| keep(id));
    }

    pub fn is_empty(&self) -> bool {
        self.by_computer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_computer.len()
    }

    /// Whether any computer introduced `requester`: the one check made
    /// before a connection is read at all.
    pub fn knows(&self, requester: &str) -> bool {
        let key = key_form(requester);
        self.by_computer
            .values()
            .any(|d| d.requesters.contains(&key))
    }

    /// What to do with `request` from `requester` (the key its connection
    /// proved) at `now`. The MAC and the address come from the duty, never
    /// from the request.
    pub fn decide(
        &self,
        requester: &str,
        request: &Request,
        limiter: &mut Limiter,
        now: Instant,
    ) -> Decision {
        let key = key_form(requester);
        // Introduced for this computer, and no other: a key the computer
        // named for a different one is asked of that one only.
        let Some(duty) = self.by_computer.get(&request.computer) else {
            return if self.knows(&key) {
                Decision::Answer(Answer::Unknown)
            } else {
                Decision::Drop
            };
        };
        if !duty.requesters.contains(&key) {
            return if self.knows(&key) {
                Decision::Answer(Answer::Unknown)
            } else {
                Decision::Drop
            };
        }
        if !limiter.take(&key, now) {
            return Decision::Answer(Answer::SlowDown);
        }
        Decision::Send {
            mac: duty.mac,
            broadcast: duty.broadcast,
        }
    }
}

/// The most computers a helper holds a duty for.
pub const MAX_COMPUTERS: usize = 8;

/// Where the magic packet goes: `broadcast` when it is the limited
/// broadcast or one of this phone's own networks' (`local`), and the
/// limited broadcast always. A computer cannot make a helper send a packet
/// to an address outside the helper's own networks.
pub fn targets(broadcast: Ipv4Addr, local: &[Ipv4Addr]) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    if broadcast != Ipv4Addr::BROADCAST && local.contains(&broadcast) {
        out.push(SocketAddr::from((broadcast, MAGIC_PORT)));
    }
    out.push(SocketAddr::from((Ipv4Addr::BROADCAST, MAGIC_PORT)));
    out
}

// ---- When the helper runs ----

/// What the phone is doing, for whether it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conditions {
    /// "Wake helper for this home" is on.
    pub on: bool,
    /// On a charger.
    pub charging: bool,
    /// On a Wi-Fi (or Ethernet) network, not mobile data alone.
    pub wifi: bool,
}

impl Conditions {
    /// Whether the endpoint listens: all three.
    pub fn serving(self) -> bool {
        self.on && self.charging && self.wifi
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const KEY_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const MAC: [u8; 6] = [0x02, 0x00, 0x5e, 0x00, 0x53, 0x01];

    fn duty(computer: &str, requesters: &[&str]) -> Duty {
        Duty {
            computer: computer.to_string(),
            mac: MAC,
            broadcast: Ipv4Addr::new(192, 0, 2, 255),
            requesters: requesters.iter().map(|k| k.to_string()).collect(),
        }
    }

    fn ask(computer: &str) -> Request {
        Request {
            computer: computer.to_string(),
        }
    }

    #[test]
    fn an_offer_round_trips_and_an_off_offer_needs_no_key() {
        let offer = Offer {
            on: true,
            fresh: true,
            iroh: KEY_A.to_string(),
            relays: vec!["https://relay.example.com/".to_string()],
        };
        assert_eq!(Offer::parse(&offer.body()), Some(offer));
        let off = Offer::parse(&json!({ "on": false })).unwrap();
        assert!(!off.on);
        assert!(Offer::parse(&json!({ "on": true })).is_none(), "no key");
        assert!(Offer::parse(&json!({ "on": true, "iroh": "short" })).is_none());
        let many = json!({ "on": true, "iroh": KEY_A, "relays": vec!["https://a/"; 5] });
        assert!(Offer::parse(&many).is_none(), "too many relays");
        let odd = json!({ "on": true, "iroh": KEY_A, "relays": ["ftp://x/"] });
        assert!(Offer::parse(&odd).is_none(), "not an https relay");
        let plain = json!({ "on": true, "iroh": KEY_A, "relays": ["http://relay.example.com/"] });
        assert!(Offer::parse(&plain).is_none(), "plain http is refused");
        let upper = Offer::parse(&json!({ "on": true, "iroh": KEY_A.to_uppercase() })).unwrap();
        assert_eq!(upper.iroh, KEY_A, "keys compare in one spelling");
    }

    #[test]
    fn a_duty_round_trips_and_holds_its_sizes() {
        let message = DutyMessage::Duty(duty("computer-1", &[KEY_A, KEY_B]));
        assert_eq!(DutyMessage::parse(&message.body()), Some(message));
        let refused = DutyMessage::Refused {
            computer: "computer-1".to_string(),
        };
        assert_eq!(DutyMessage::parse(&refused.body()), Some(refused));
        let withdrawn = DutyMessage::Withdrawn {
            computer: "computer-1".to_string(),
        };
        assert_eq!(DutyMessage::parse(&withdrawn.body()), Some(withdrawn));
        let mut body = DutyMessage::Duty(duty("computer-1", &[KEY_A])).body();
        body["mac"] = "00:00:00:00:00:00".into();
        assert!(DutyMessage::parse(&body).is_none(), "an all-zero MAC");
        body["mac"] = "02:00:5e:00:53:01".into();
        body["broadcast"] = "not an address".into();
        assert!(DutyMessage::parse(&body).is_none());
        body["broadcast"] = "192.0.2.255".into();
        body["requesters"] = json!(vec![KEY_A; MAX_REQUESTERS + 1]);
        assert!(DutyMessage::parse(&body).is_none(), "too many requesters");
        body["requesters"] = json!(["not a key"]);
        assert!(DutyMessage::parse(&body).is_none());
        body["requesters"] = json!([KEY_A]);
        body["computer"] = "bad\nid".into();
        assert!(DutyMessage::parse(&body).is_none());
    }

    #[test]
    fn an_info_names_the_helper_or_none() {
        let info = Info {
            computer: "computer-1".to_string(),
            helper: Some(HelperInfo {
                iroh: KEY_A.to_string(),
                relays: vec!["https://relay.example.com/".to_string()],
                name: "Spare phone".to_string(),
            }),
        };
        assert_eq!(Info::parse(&info.body()), Some(info));
        let none = Info {
            computer: "computer-1".to_string(),
            helper: None,
        };
        assert_eq!(Info::parse(&none.body()), Some(none));
        assert!(Info::parse(&json!({ "computer": "computer-1" })).is_none());
    }

    #[test]
    fn a_request_carries_the_computer_and_nothing_else() {
        let request = ask("computer-1");
        assert_eq!(
            Request::parse(request.line().trim_end().as_bytes()),
            Some(request)
        );
        // A MAC, an address or anything more is not a request.
        for line in [
            r#"{"wake":"computer-1","mac":"02:00:5e:00:53:01"}"#,
            r#"{"wake":"computer-1","broadcast":"192.0.2.255"}"#,
            r#"{"mac":"02:00:5e:00:53:01"}"#,
            r#"{"wake":7}"#,
            r#"[]"#,
            "not json",
            "",
        ] {
            assert!(Request::parse(line.as_bytes()).is_none(), "{line}");
        }
        let long = format!(r#"{{"wake":"{}"}}"#, "x".repeat(MAX_REQUEST_BYTES));
        assert!(Request::parse(long.as_bytes()).is_none());
    }

    #[test]
    fn answers_round_trip_and_a_bad_line_is_a_failure() {
        for answer in [
            Answer::Sent,
            Answer::SlowDown,
            Answer::Unknown,
            Answer::Failed,
        ] {
            assert_eq!(Answer::parse(answer.line().trim_end().as_bytes()), answer);
        }
        assert_eq!(Answer::parse(b"nonsense"), Answer::Failed);
        assert_eq!(Answer::parse(br#"{"answer":"other"}"#), Answer::Failed);
    }

    #[test]
    fn only_a_key_the_computer_introduced_is_served() {
        let mut duties = Duties::default();
        duties.set(duty("computer-1", &[KEY_A]));
        let mut limiter = Limiter::new(Instant::now());
        let now = Instant::now();
        assert_eq!(
            duties.decide(KEY_A, &ask("computer-1"), &mut limiter, now),
            Decision::Send {
                mac: MAC,
                broadcast: Ipv4Addr::new(192, 0, 2, 255)
            }
        );
        // A stranger's key: dropped, and it uses none of the limit.
        assert_eq!(
            duties.decide(KEY_C, &ask("computer-1"), &mut limiter, now),
            Decision::Drop
        );
        assert!(!duties.knows(KEY_C));
        assert!(duties.knows(KEY_A));
        // The same key spelled in capitals is the same key.
        assert!(duties.knows(&KEY_A.to_uppercase()));
    }

    #[test]
    fn a_key_introduced_for_one_computer_cannot_wake_another() {
        let mut duties = Duties::default();
        duties.set(duty("computer-1", &[KEY_A]));
        duties.set(duty("computer-2", &[KEY_B]));
        let mut limiter = Limiter::new(Instant::now());
        let now = Instant::now();
        assert_eq!(
            duties.decide(KEY_A, &ask("computer-2"), &mut limiter, now),
            Decision::Answer(Answer::Unknown)
        );
        assert_eq!(
            duties.decide(KEY_A, &ask("computer-9"), &mut limiter, now),
            Decision::Answer(Answer::Unknown),
            "a computer the helper holds nothing for"
        );
        assert!(matches!(
            duties.decide(KEY_B, &ask("computer-2"), &mut limiter, now),
            Decision::Send { .. }
        ));
    }

    #[test]
    fn what_is_sent_comes_from_the_duty_and_never_from_the_request() {
        // The request type has no field for a MAC or an address: `parse`
        // refuses a line that has one, and `decide` reads both from the duty.
        let mut duties = Duties::default();
        duties.set(duty("computer-1", &[KEY_A]));
        let parsed = Request::parse(br#"{"wake":"computer-1","mac":"02:00:5e:00:00:99"}"#);
        assert!(parsed.is_none());
        let mut limiter = Limiter::new(Instant::now());
        let Decision::Send { mac, .. } =
            duties.decide(KEY_A, &ask("computer-1"), &mut limiter, Instant::now())
        else {
            panic!("served");
        };
        assert_eq!(mac, MAC);
    }

    #[test]
    fn a_requester_may_ask_a_burst_and_then_once_in_thirty_seconds() {
        let start = Instant::now();
        let mut limiter = Limiter::new(start);
        assert!(limiter.take(KEY_A, start));
        assert!(limiter.take(KEY_A, start));
        assert!(!limiter.take(KEY_A, start), "past the burst");
        assert!(!limiter.take(KEY_A, start + Duration::from_secs(29)));
        assert!(limiter.take(KEY_A, start + Duration::from_secs(31)));
        assert!(!limiter.take(KEY_A, start + Duration::from_secs(32)));
        // Another requester has its own.
        assert!(limiter.take(KEY_B, start + Duration::from_secs(32)));
    }

    #[test]
    fn all_requesters_together_are_limited_too() {
        let start = Instant::now();
        let mut limiter = Limiter::new(start);
        let keys: Vec<String> = (0..10).map(|i| format!("{i:064x}")).collect();
        let served = keys.iter().filter(|k| limiter.take(k, start)).count();
        assert_eq!(served, ALL_BURST as usize);
        // A refusal by the shared limit gave the requester's token back.
        let refused = keys.last().unwrap();
        assert!(!limiter.take(refused, start));
        let later = start + Duration::from_secs(ALL_EVERY + 1);
        assert!(limiter.take(refused, later));
    }

    #[test]
    fn the_limit_is_decided_per_request() {
        let mut duties = Duties::default();
        duties.set(duty("computer-1", &[KEY_A]));
        let start = Instant::now();
        let mut limiter = Limiter::new(start);
        for _ in 0..BURST {
            assert!(matches!(
                duties.decide(KEY_A, &ask("computer-1"), &mut limiter, start),
                Decision::Send { .. }
            ));
        }
        assert_eq!(
            duties.decide(KEY_A, &ask("computer-1"), &mut limiter, start),
            Decision::Answer(Answer::SlowDown)
        );
    }

    #[test]
    fn a_removed_duty_serves_nobody_and_a_revoked_key_goes_with_the_next_duty() {
        let mut duties = Duties::default();
        duties.set(duty("computer-1", &[KEY_A, KEY_B]));
        assert!(duties.knows(KEY_B));
        // The computer sent a duty without B: B is no longer served.
        duties.set(duty("computer-1", &[KEY_A]));
        assert!(!duties.knows(KEY_B));
        let mut limiter = Limiter::new(Instant::now());
        assert_eq!(
            duties.decide(KEY_B, &ask("computer-1"), &mut limiter, Instant::now()),
            Decision::Drop
        );
        duties.forget("computer-1");
        assert!(duties.is_empty());
        assert!(!duties.knows(KEY_A));
    }

    #[test]
    fn a_helper_holds_a_few_computers_only() {
        let mut duties = Duties::default();
        for i in 0..MAX_COMPUTERS {
            assert!(duties.set(duty(&format!("computer-{i}"), &[KEY_A])));
        }
        assert!(!duties.set(duty("computer-extra", &[KEY_A])));
        assert!(duties.set(duty("computer-0", &[KEY_B])), "an update fits");
        assert_eq!(duties.len(), MAX_COMPUTERS);
        duties.retain(|id| id == "computer-0");
        assert_eq!(duties.len(), 1);
    }

    #[test]
    fn a_packet_goes_only_to_the_helpers_own_networks_and_the_limited_broadcast() {
        let own = [Ipv4Addr::new(192, 0, 2, 255)];
        let sent = targets(Ipv4Addr::new(192, 0, 2, 255), &own);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], "192.0.2.255:9".parse().unwrap());
        assert_eq!(sent[1], "255.255.255.255:9".parse().unwrap());
        // An address the computer named that is not one of the helper's
        // networks is never sent to.
        let elsewhere = targets(Ipv4Addr::new(203, 0, 113, 9), &own);
        assert_eq!(elsewhere, vec!["255.255.255.255:9".parse().unwrap()]);
        let limited = targets(Ipv4Addr::BROADCAST, &[]);
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn the_helper_serves_only_on_a_charger_and_wifi_with_the_switch_on() {
        let on = |on, charging, wifi| Conditions { on, charging, wifi }.serving();
        assert!(on(true, true, true));
        assert!(!on(false, true, true), "switch off");
        assert!(!on(true, false, true), "unplugged");
        assert!(!on(true, true, false), "off Wi-Fi");
        assert!(!on(false, false, false));
    }

    #[test]
    fn a_duty_does_not_print_its_mac_or_keys() {
        let text = format!("{:?}", duty("computer-1", &[KEY_A]));
        assert!(!text.contains("aaaa"));
        assert!(!text.contains("0x02"));
        assert!(!text.contains('2'), "{text}");
    }

    #[test]
    fn a_mac_reads_in_either_spelling() {
        assert_eq!(parse_mac("02:00:5e:00:53:01"), Some(MAC));
        assert_eq!(parse_mac("02-00-5E-00-53-01"), Some(MAC));
        assert_eq!(parse_mac("02:00:5e:00:53"), None);
        assert_eq!(parse_mac("ff:ff:ff:ff:ff:ff"), None);
        assert_eq!(parse_mac("zz:00:5e:00:53:01"), None);
    }
}
