//! Pair a computer nearby: one found on this network by its mDNS
//! announcement, asked to pair, and the pairing confirmed on both screens
//! by a matching code (a SAS, as KDE Connect's verification key or
//! Bluetooth's numeric comparison), so no link is carried between them.
//!
//! The computer asking (the requester) dials the other's control port,
//! unpaired, over TLS, and both see each other's certificate there. Each
//! picks a random nonce. The requester commits to its own first (a hash of
//! it), the other answers with its nonce, and only then is the requester's
//! revealed: a computer in the middle, which sees two TLS sessions with two
//! pairs of certificates, must choose its nonces before it learns the ones
//! that make the codes, so the two codes it causes agree only by a one in a
//! million chance. The code is six digits from both certificates and both
//! nonces ([`matching_code`]). Both owners compare it, as Bluetooth's
//! numeric comparison has each side do: the requester's owner confirms the
//! code on its screen, the other's accepts on its own, and only once both
//! have does the other hand the requester a PIN for that certificate only;
//! the two then pair as if its link had been pasted. The other's Accept
//! alone pairs nothing: a computer in the middle could send one.
//!
//! What the two keep while it goes on ([`NearbyAsk`], [`NearbyRequest`]) is
//! in `computers.json`, for the app to show; the other computer's own word
//! ([`HostDesk`]) and the requests it takes per address ([`Pace`]) are kept
//! in memory.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The packet a nearby pairing's messages ride: the pairing one, which a
/// computer takes from a device it has not paired with. Its body's
/// [`STEP`] says which message it is.
pub const PACKET: &str = "kdeconnect.pocketdesk.pair";
/// The body's key naming the step.
pub const STEP: &str = "nearby";

/// The steps, as [`STEP`] names them.
pub mod step {
    /// Requester: "may I pair?", its name and its nonce's commitment.
    pub const ASK: &str = "ask";
    /// Host: its nonce, once it takes the ask.
    pub const NONCE: &str = "nonce";
    /// Requester: its nonce, which matches the commitment.
    pub const REVEAL: &str = "reveal";
    /// Requester: its owner confirmed the code on its screen.
    pub const CONFIRMED: &str = "confirmed";
    /// Host: its owner accepted and the requester confirmed; the PIN and
    /// what pairing needs. Sent only after [`CONFIRMED`].
    pub const ACCEPTED: &str = "accepted";
    /// Host: its owner declined, or did not answer in time.
    pub const DECLINED: &str = "declined";
    /// Host: another computer's ask is on its screen now.
    pub const BUSY: &str = "busy";
    /// Host: it takes no ask now (its owner turned asks off, or this
    /// address asked too often). One word for both, so a requester learns
    /// no more than the announcement says.
    pub const UNAVAILABLE: &str = "unavailable";
}

/// How long the other computer's owner has to answer.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(60);

/// The most asks taken from one address in [`PACE_WINDOW`].
pub const PACE_MOST: usize = 3;
/// The window [`PACE_MOST`] counts in.
pub const PACE_WINDOW: Duration = Duration::from_secs(60);

/// A fresh random nonce.
pub fn nonce() -> [u8; 32] {
    use rand::RngCore;
    let mut n = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut n);
    n
}

/// The commitment to `nonce`, sent before it.
pub fn commitment(nonce: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"pocketdesk nearby commitment v1");
    h.update(nonce);
    h.finalize().into()
}

/// The matching code both screens show: six digits from the requester's
/// and the host's certificates (SHA-256 of each, as TLS saw them) and
/// their nonces. Each side computes it from its own certificate and the
/// one its connection proved, so a different certificate on either side
/// (someone in the middle) gives a different code.
pub fn matching_code(
    requester_cert: &[u8; 32],
    host_cert: &[u8; 32],
    requester_nonce: &[u8; 32],
    host_nonce: &[u8; 32],
) -> String {
    let mut h = Sha256::new();
    h.update(b"pocketdesk nearby pairing v1");
    h.update(requester_cert);
    h.update(host_cert);
    h.update(requester_nonce);
    h.update(host_nonce);
    let digest: [u8; 32] = h.finalize().into();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    format!("{:06}", u64::from_be_bytes(first) % 1_000_000)
}

/// A code as the screens show it: two groups of three ("482 913").
pub fn spaced(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// Lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 32 bytes from 64 hex digits.
pub fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// How this computer's ask of a nearby one stands, for the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskState {
    /// Reaching it.
    Asking,
    /// Both screens show the code: waiting for the owner here to confirm
    /// it ("Same code on both?"). Nothing pairs before.
    Confirm,
    /// Confirmed here; waiting for the other's owner to accept.
    Waiting,
    /// Accepted: pairing as from its link (the last pairing's outcome
    /// says how it went).
    Accepted,
    Declined,
    /// Another computer's ask was on its screen.
    Busy,
    /// It takes no ask now.
    Unavailable,
    /// Cancelled here.
    Cancelled,
    /// It could not be reached, or broke off.
    Failed(String),
}

impl AskState {
    /// Whether it is over.
    pub fn is_over(&self) -> bool {
        !matches!(
            self,
            AskState::Asking | AskState::Confirm | AskState::Waiting
        )
    }
}

/// This computer's ask of a nearby one (the requester's side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearbyAsk {
    /// The app's request ([`super::computers::ComputerRequest::id`]); the
    /// pairing it leads to records its outcome under it.
    pub request: String,
    pub device_id: String,
    pub name: String,
    /// The matching code, once both nonces are known.
    #[serde(default)]
    pub code: Option<String>,
    pub state: AskState,
    pub unix: u64,
}

/// A nearby computer's ask on this one's screen (the host's side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearbyRequest {
    /// The ask, as the app answers it.
    pub id: String,
    /// The id and name it gave itself: its own words, shown as such.
    pub device_id: String,
    pub name: String,
    /// The address it asked from.
    pub address: String,
    pub code: String,
    pub unix: u64,
}

/// Why the host takes no ask now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Another ask is on its screen.
    Busy,
    /// Asks are off here.
    Off,
    /// That address asked too often.
    TooOften,
}

impl Refusal {
    /// The step the requester is told: busy, or one word for the rest.
    pub fn step(self) -> &'static str {
        match self {
            Refusal::Busy => step::BUSY,
            Refusal::Off | Refusal::TooOften => step::UNAVAILABLE,
        }
    }
}

/// How an answer went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    /// Accepted here; it pairs once the requester has confirmed too
    /// ([`HostDesk::both`]).
    Accepted,
    Declined,
    /// Not the ask on screen now (answered late, or twice).
    Stale,
}

/// The ask on the host's screen: when it was taken (or shown), and what
/// each owner said of it.
#[derive(Debug)]
struct Shown {
    id: String,
    at: Instant,
    /// Its requester revealed a nonce that matched: the owner sees it.
    prompted: bool,
    /// The owner here accepted, and whether both are theirs (PC1).
    host: Option<bool>,
    /// The requester's owner confirmed the code there.
    requester: bool,
}

/// The asks one address made lately, to take at most [`PACE_MOST`] in
/// [`PACE_WINDOW`] from each.
#[derive(Debug, Default)]
pub struct Pace {
    asks: HashMap<IpAddr, Vec<Instant>>,
}

impl Pace {
    /// Whether an ask from `from` at `now` is taken; if so it counts.
    pub fn allow(&mut self, from: IpAddr, now: Instant) -> bool {
        self.asks.retain(|_, at| {
            at.iter()
                .any(|t| now.saturating_duration_since(*t) < PACE_WINDOW)
        });
        let at = self.asks.entry(from).or_default();
        at.retain(|t| now.saturating_duration_since(*t) < PACE_WINDOW);
        if at.len() >= PACE_MOST {
            return false;
        }
        at.push(now);
        true
    }
}

/// The host's side: one ask at a time on its screen, answered by both
/// owners within [`ANSWER_WITHIN`], and the pace per address.
#[derive(Debug, Default)]
pub struct HostDesk {
    shown: Option<Shown>,
    pace: Pace,
}

impl HostDesk {
    /// Take an ask from `from` at `now` (`enabled`: asks are on here); its
    /// id, to answer by. Nothing can come of an ask but its owner's Accept.
    pub fn ask(&mut self, from: IpAddr, enabled: bool, now: Instant) -> Result<String, Refusal> {
        if !enabled {
            return Err(Refusal::Off);
        }
        self.expire(now);
        if self.shown.is_some() {
            return Err(Refusal::Busy);
        }
        if !self.pace.allow(from, now) {
            return Err(Refusal::TooOften);
        }
        let id = to_hex(&nonce()[..8]);
        self.shown = Some(Shown {
            id: id.clone(),
            at: now,
            prompted: false,
            host: None,
            requester: false,
        });
        Ok(id)
    }

    fn shown_mut(&mut self, id: &str) -> Option<&mut Shown> {
        self.shown.as_mut().filter(|s| s.id == id)
    }

    /// The ask `id`'s requester revealed a nonce that matched: its owner
    /// sees it now, for [`ANSWER_WITHIN`] from `now`. False when it is not
    /// the ask on screen, or was shown already.
    pub fn prompted(&mut self, id: &str, now: Instant) -> bool {
        match self.shown_mut(id) {
            Some(shown) if !shown.prompted => {
                shown.prompted = true;
                shown.at = now;
                true
            }
            _ => false,
        }
    }

    /// The owner's answer to the ask `id`, once. A Decline frees the
    /// screen; an Accept waits for the requester's confirmation
    /// ([`HostDesk::both`]).
    pub fn answer(&mut self, id: &str, accept: bool, mine: bool, now: Instant) -> Answered {
        self.expire(now);
        match self.shown_mut(id) {
            Some(shown) if shown.prompted && shown.host.is_none() => {
                if accept {
                    shown.host = Some(mine);
                    Answered::Accepted
                } else {
                    self.shown = None;
                    Answered::Declined
                }
            }
            _ => Answered::Stale,
        }
    }

    /// The ask `id`'s requester said its owner confirmed the code there.
    /// Only once it is shown here: a confirmation before its nonce was
    /// even revealed confirms nothing.
    pub fn confirmed(&mut self, id: &str, now: Instant) -> bool {
        self.expire(now);
        match self.shown_mut(id) {
            Some(shown) if shown.prompted => {
                shown.requester = true;
                true
            }
            _ => false,
        }
    }

    /// Both owners said yes to the ask `id` (the owner here accepted, the
    /// requester's confirmed): the screen is free again, and whether both
    /// are theirs is returned, for the PIN. None until both have.
    pub fn both(&mut self, id: &str) -> Option<bool> {
        let shown = self.shown.as_ref().filter(|s| s.id == id)?;
        let mine = shown.host.filter(|_| shown.requester)?;
        self.shown = None;
        Some(mine)
    }

    /// The ask `id` is over without both answers (its requester left, or a
    /// step failed): the screen is free again.
    pub fn drop_ask(&mut self, id: &str) {
        if self.shown.as_ref().is_some_and(|s| s.id == id) {
            self.shown = None;
        }
    }

    /// The ask on screen, if it has not run out at `now`; one that has is
    /// dropped, and its id returned.
    pub fn expire(&mut self, now: Instant) -> Option<String> {
        if self
            .shown
            .as_ref()
            .is_some_and(|s| now.saturating_duration_since(s.at) >= ANSWER_WITHIN)
        {
            return self.shown.take().map(|s| s.id);
        }
        None
    }

    /// The ask on screen now.
    pub fn shown(&self) -> Option<&str> {
        self.shown.as_ref().map(|s| s.id.as_str())
    }
}

/// A Pocketdesk computer found on this network, as its announcement says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub device_id: String,
    pub name: String,
    /// Its addresses, IPv4 first.
    pub addresses: Vec<String>,
    pub control_port: u16,
    pub stream_port: u16,
}

/// One row of the Nearby list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyRow {
    pub found: Found,
    /// Paired already, either way.
    pub paired: bool,
}

/// The Nearby list from what was found: this computer left out, one row
/// per computer (by id; a later find replaces an earlier one), only those
/// with an id and an address, marked when paired already (`paired`, the
/// ids of the computers paired either way), by name.
pub fn nearby_list(found: &[Found], own_id: &str, paired: &[String]) -> Vec<NearbyRow> {
    let mut by_id: HashMap<&str, &Found> = HashMap::new();
    for f in found {
        if f.device_id.is_empty() || f.device_id == own_id || f.addresses.is_empty() {
            continue;
        }
        by_id.insert(&f.device_id, f);
    }
    let mut rows: Vec<NearbyRow> = by_id
        .into_values()
        .map(|f| NearbyRow {
            paired: paired.contains(&f.device_id),
            found: f.clone(),
        })
        .collect();
    rows.sort_by(|a, b| {
        a.found
            .name
            .to_lowercase()
            .cmp(&b.found.name.to_lowercase())
            .then_with(|| a.found.device_id.cmp(&b.found.device_id))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [u8; 32] = [0xa1; 32];
    const B: [u8; 32] = [0xb2; 32];
    const M: [u8; 32] = [0x3c; 32];
    const NA: [u8; 32] = [0x11; 32];
    const NB: [u8; 32] = [0x22; 32];

    /// Both sides compute the same code from the same two certificates and
    /// nonces: the requester from its own and the one the host proved, the
    /// host from the one the requester proved and its own.
    #[test]
    fn both_sides_compute_the_same_code() {
        let requester_sees = matching_code(&A, &B, &NA, &NB);
        let host_sees = matching_code(&A, &B, &NA, &NB);
        assert_eq!(requester_sees, host_sees);
        assert_eq!(requester_sees.len(), 6);
        assert!(requester_sees.bytes().all(|b| b.is_ascii_digit()));
    }

    /// Someone in the middle shows each side another certificate: either
    /// code differs, as does one with the roles swapped or another nonce.
    #[test]
    fn a_different_certificate_or_nonce_gives_a_different_code() {
        let honest = matching_code(&A, &B, &NA, &NB);
        assert_ne!(honest, matching_code(&A, &M, &NA, &NB), "host's swapped");
        assert_ne!(
            honest,
            matching_code(&M, &B, &NA, &NB),
            "requester's swapped"
        );
        assert_ne!(honest, matching_code(&B, &A, &NA, &NB), "roles swapped");
        assert_ne!(honest, matching_code(&A, &B, &NB, &NB), "another nonce");
        assert_ne!(honest, matching_code(&A, &B, &NA, &NA), "another nonce");
    }

    /// The code is spread over its million values.
    #[test]
    fn codes_spread() {
        let codes: std::collections::HashSet<String> = (0u8..=255)
            .map(|i| matching_code(&A, &B, &[i; 32], &NB))
            .collect();
        assert!(codes.len() > 250, "{}", codes.len());
    }

    #[test]
    fn a_commitment_binds_its_nonce() {
        assert_eq!(commitment(&NA), commitment(&NA));
        assert_ne!(commitment(&NA), commitment(&NB));
        assert_ne!(commitment(&NA), NA, "not the nonce itself");
    }

    #[test]
    fn hex_reads_back() {
        assert_eq!(from_hex(&to_hex(&A)), Some(A));
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex(&"g".repeat(64)), None);
        assert_eq!(spaced("482913"), "482 913");
    }

    fn at(secs: u64, start: Instant) -> Instant {
        start + Duration::from_secs(secs)
    }

    const HOME: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 10));
    const OTHER: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 11));

    /// Ask, then Accept: accepted once; a second answer is stale. It pairs
    /// only once the requester's owner has confirmed too, in either order.
    #[test]
    fn an_ask_pairs_only_once_both_owners_said_yes() {
        let start = Instant::now();
        let mut desk = HostDesk::default();
        let id = desk.ask(HOME, true, start).expect("taken");
        assert_eq!(desk.shown(), Some(id.as_str()));
        assert!(desk.prompted(&id, at(1, start)));
        assert_eq!(
            desk.answer(&id, true, true, at(5, start)),
            Answered::Accepted
        );
        assert_eq!(desk.answer(&id, true, true, at(6, start)), Answered::Stale);
        assert_eq!(desk.both(&id), None, "the requester has not confirmed");
        assert_eq!(desk.shown(), Some(id.as_str()), "still on screen");
        assert!(desk.confirmed(&id, at(7, start)));
        assert_eq!(desk.both(&id), Some(true));
        assert_eq!(desk.shown(), None);
        assert_eq!(desk.both(&id), None, "once");

        // The requester first, then the owner here.
        let id = desk.ask(OTHER, true, at(8, start)).expect("taken");
        assert!(desk.prompted(&id, at(8, start)));
        assert!(desk.confirmed(&id, at(9, start)));
        assert_eq!(desk.both(&id), None, "the owner here has not accepted");
        desk.answer(&id, true, false, at(10, start));
        assert_eq!(desk.both(&id), Some(false));
    }

    /// The requester's confirmation counts only once the ask is on screen:
    /// before its nonce was revealed it confirms nothing, nor can the owner
    /// here answer an ask not shown yet.
    #[test]
    fn a_confirmation_before_the_prompt_confirms_nothing() {
        let start = Instant::now();
        let mut desk = HostDesk::default();
        let id = desk.ask(HOME, true, start).expect("taken");
        assert!(!desk.confirmed(&id, start));
        assert_eq!(desk.answer(&id, true, false, start), Answered::Stale);
        assert!(desk.prompted(&id, at(1, start)));
        assert!(!desk.prompted(&id, at(2, start)), "shown once");
        desk.answer(&id, true, false, at(3, start));
        assert_eq!(desk.both(&id), None);
    }

    /// Decline frees the screen for the next ask.
    #[test]
    fn a_declined_ask_frees_the_screen() {
        let start = Instant::now();
        let mut desk = HostDesk::default();
        let id = desk.ask(HOME, true, start).expect("taken");
        assert_eq!(desk.ask(OTHER, true, start), Err(Refusal::Busy));
        desk.prompted(&id, start);
        assert_eq!(
            desk.answer(&id, false, false, at(1, start)),
            Answered::Declined
        );
        assert!(desk.ask(OTHER, true, at(2, start)).is_ok());
    }

    /// No answer within a minute of the prompt: it runs out, and a late
    /// Accept grants nothing.
    #[test]
    fn an_unanswered_ask_runs_out() {
        let start = Instant::now();
        let mut desk = HostDesk::default();
        let id = desk.ask(HOME, true, start).expect("taken");
        desk.prompted(&id, start);
        assert_eq!(desk.expire(at(59, start)), None);
        assert_eq!(desk.expire(at(60, start)), Some(id.clone()));
        assert_eq!(
            desk.answer(&id, true, false, at(61, start)),
            Answered::Stale
        );
    }

    /// With asks turned off, none is taken.
    #[test]
    fn asks_off_takes_none() {
        let mut desk = HostDesk::default();
        assert_eq!(desk.ask(HOME, false, Instant::now()), Err(Refusal::Off));
        assert_eq!(desk.shown(), None);
    }

    /// One address asks at most three times a minute; another is counted
    /// apart; a minute on, it may ask again.
    #[test]
    fn asks_are_paced_per_address() {
        let start = Instant::now();
        let mut desk = HostDesk::default();
        for i in 0..PACE_MOST as u64 {
            let id = desk.ask(HOME, true, at(i, start)).expect("taken");
            desk.drop_ask(&id);
        }
        assert_eq!(desk.ask(HOME, true, at(10, start)), Err(Refusal::TooOften));
        let other = desk
            .ask(OTHER, true, at(10, start))
            .expect("another address");
        desk.drop_ask(&other);
        assert!(desk.ask(HOME, true, at(61, start)).is_ok());
    }

    /// Busy is said as busy; off and too often alike, as unavailable.
    #[test]
    fn a_refusal_says_no_more_than_needed() {
        assert_eq!(Refusal::Busy.step(), step::BUSY);
        assert_eq!(Refusal::Off.step(), Refusal::TooOften.step());
    }

    fn found(id: &str, name: &str) -> Found {
        Found {
            device_id: id.to_string(),
            name: name.to_string(),
            addresses: vec!["192.0.2.20".to_string()],
            control_port: 1716,
            stream_port: 4716,
        }
    }

    /// The list leaves this computer out, has each computer once, marks a
    /// paired one, and sorts by name.
    #[test]
    fn the_nearby_list_is_one_row_per_other_computer() {
        let mut moved = found("d-2", "Test Tower");
        moved.addresses = vec!["192.0.2.21".to_string()];
        let mut silent = found("d-4", "No Address");
        silent.addresses.clear();
        let rows = nearby_list(
            &[
                found("d-1", "this one"),
                found("d-2", "Test Tower"),
                found("d-3", "another desk"),
                moved,
                found("", "No Id"),
                silent,
            ],
            "d-1",
            &["d-3".to_string()],
        );
        let shown: Vec<(&str, bool)> = rows
            .iter()
            .map(|r| (r.found.name.as_str(), r.paired))
            .collect();
        assert_eq!(shown, [("another desk", true), ("Test Tower", false)]);
        assert_eq!(rows[1].found.addresses, ["192.0.2.21"], "the later find");
    }
}
