//! One pairing for all the owner's devices (ecosystem phase 4): a computer
//! introduces the owner's phone to the owner's other computers.
//!
//! Computer A and computer B are paired both ways (PC1, "both mine"). A
//! phone paired with A's own code (an owner phone, remote revoke's `owners`
//! mark) asks A which of A's computers it could pair with too
//! ([`INTRODUCE`] `list`), and, once its owner confirmed on the phone,
//! asks A to introduce it to one of them (`pair`). A asks B over the link
//! between the two ([`INTRODUCE_PHONE`]), naming the phone, the keys A
//! holds it to and what A allows it. B, when its owner lets its computers
//! add phones there, answers with a one-time PIN bound to that phone and
//! key ([`INTRODUCE_PHONE_ANSWER`]); A hands the phone a pairing link to B
//! made of that PIN and what A pinned for B ([`INTRODUCE_ANSWER`]), and
//! the phone pairs with B as from a scanned code.
//!
//! Nothing here is new cryptography: every message rides a connection that
//! already proved both ends (the phone's TLS or iroh connection to A, the
//! link between A and B), and the phone then pairs with B on a connection
//! that must prove the key A named. The rules are the server's
//! (`server/src/introductions.rs`); this is what goes on the wire, so the
//! phone's bridge and both computers read it alike. An older computer or
//! phone has no plugin for these packet types, and drops them.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::permissions::DevicePermissions;

/// Owner phone → its computer: `{request, ask: "list"}` or `{request, ask:
/// "pair", deviceId}`.
pub const INTRODUCE: &str = "kdeconnect.pocketdesk.introduce";

/// Computer → the phone that asked: `{request, ask: "list", computers:
/// [{id, name}]}`, `{request, ask: "pair", deviceId, link}`, or `{request,
/// ask, refused}`.
pub const INTRODUCE_ANSWER: &str = "kdeconnect.pocketdesk.introduce_answer";

/// Computer A → computer B, on the link between the two: `{request, phone:
/// {deviceId, name, tls, iroh}, permissions}`.
pub const INTRODUCE_PHONE: &str = "kdeconnect.pocketdesk.introduce_phone";

/// Computer B → computer A: `{request, pin, facts}` or `{request,
/// refused}`.
pub const INTRODUCE_PHONE_ANSWER: &str = "kdeconnect.pocketdesk.introduce_phone_answer";

/// How long the PIN B hands out pairs the phone it names.
pub const PIN_FOR_SECS: u64 = 120;

/// At most this many introductions are taken from one asker (a phone at A,
/// or computer A at B) in [`PACE_WINDOW_SECS`].
pub const INTRODUCTIONS_PER_WINDOW: usize = 5;

/// The window [`INTRODUCTIONS_PER_WINDOW`] counts in: ten minutes.
pub const PACE_WINDOW_SECS: u64 = 10 * 60;

/// At most this many lists a minute are taken from one phone.
pub const LISTS_PER_MINUTE: usize = 30;

/// At most this many introduction PINs wait at B at once.
pub const PINS_WAITING: usize = 4;

/// The longest request id taken: an id is the asker's own, echoed back.
const MAX_REQUEST: usize = 64;

/// The most computers a list answer may offer a phone; more is refused
/// whole, before any is read.
pub const MAX_OFFERS: usize = 64;

/// The longest name an offer, or an id an answer, may carry, in bytes.
const MAX_FIELD: usize = 256;

/// The longest pairing link an answer may carry, in bytes: the pairing
/// link's own limit (`local::pair_link::MAX_LINK_BYTES`).
pub const MAX_LINK: usize = 8 * 1024;

/// Whether `request` is a request id this protocol takes: 1 to 64
/// printable ASCII characters.
fn request_reads(request: &str) -> bool {
    !request.is_empty()
        && request.len() <= MAX_REQUEST
        && request.bytes().all(|b| b.is_ascii_graphic())
}

/// What an owner phone asks its computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneAsk {
    /// Which of the computer's computers it could pair with too.
    List,
    /// Introduce it to the computer `device_id`.
    Pair { device_id: String },
}

impl PhoneAsk {
    fn word(&self) -> &'static str {
        match self {
            PhoneAsk::List => "list",
            PhoneAsk::Pair { .. } => "pair",
        }
    }
}

/// A request read from an [`INTRODUCE`] body: its id and its ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneRequest {
    pub request: String,
    pub ask: PhoneAsk,
}

impl PhoneRequest {
    /// The body that asks this.
    pub fn body(&self) -> Value {
        match &self.ask {
            PhoneAsk::List => json!({ "request": self.request, "ask": "list" }),
            PhoneAsk::Pair { device_id } => {
                json!({ "request": self.request, "ask": "pair", "deviceId": device_id })
            }
        }
    }

    /// The request in `body`, or `None` for one that does not read.
    pub fn parse(body: &Value) -> Option<Self> {
        let request = body.get("request")?.as_str()?;
        if !request_reads(request) {
            return None;
        }
        let ask = match body.get("ask")?.as_str()? {
            "list" => PhoneAsk::List,
            "pair" => {
                let device_id = body.get("deviceId")?.as_str()?;
                if !super::kdeconnect::valid_device_id(device_id) {
                    return None;
                }
                PhoneAsk::Pair {
                    device_id: device_id.to_string(),
                }
            }
            _ => return None,
        };
        Some(Self {
            request: request.to_string(),
            ask,
        })
    }
}

/// One computer an owner phone could be introduced to: its id and the
/// name the introducing computer knows it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// Why an introduction was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The phone is not an owner phone at A (someone else's, a profile's,
    /// a guest's, or a computer).
    NotOwner,
    /// The computer named is not one A is paired with both ways, or A is
    /// not one B is paired with both ways.
    NotMine,
    /// No link between A and B is up now.
    NotLinked,
    /// B's owner turned "Let my computers add my phones here" off.
    Off,
    /// The phone's id is already in use at B (paired, a computer, or bound
    /// to another key).
    Taken,
    /// A holds no key for the phone yet to vouch for it with.
    NoKey,
    /// Too many asks in a while.
    Busy,
    /// B did not answer in time (an older Pocketdesk has no answer).
    NoAnswer,
    /// Something could not be read or written.
    Failed,
}

impl Refusal {
    /// Its word on the wire.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NotOwner => "not_owner",
            Refusal::NotMine => "not_mine",
            Refusal::NotLinked => "not_linked",
            Refusal::Off => "off",
            Refusal::Taken => "taken",
            Refusal::NoKey => "no_key",
            Refusal::Busy => "busy",
            Refusal::NoAnswer => "no_answer",
            Refusal::Failed => "failed",
        }
    }

    /// The refusal a code names; an unknown code is a failure.
    pub fn from_code(code: &str) -> Self {
        match code {
            "not_owner" => Refusal::NotOwner,
            "not_mine" => Refusal::NotMine,
            "not_linked" => Refusal::NotLinked,
            "off" => Refusal::Off,
            "taken" => Refusal::Taken,
            "no_key" => Refusal::NoKey,
            "busy" => Refusal::Busy,
            "no_answer" => Refusal::NoAnswer,
            _ => Refusal::Failed,
        }
    }
}

/// The answer to a phone's list.
pub fn list_answer(request: &str, offers: &[Offer]) -> Value {
    json!({ "request": request, "ask": "list", "computers": offers })
}

/// The answer to a phone's pairing ask: the pairing link to the computer.
pub fn pair_answer(request: &str, device_id: &str, link: &str) -> Value {
    json!({ "request": request, "ask": "pair", "deviceId": device_id, "link": link })
}

/// The answer to a phone's ask that was not done.
pub fn refused_answer(request: &str, ask: &PhoneAsk, refusal: Refusal) -> Value {
    let mut body = json!({ "request": request, "ask": ask.word(), "refused": refusal.code() });
    if let PhoneAsk::Pair { device_id } = ask {
        body["deviceId"] = device_id.as_str().into();
    }
    body
}

/// What a phone's answer says, read on the phone's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneAnswer {
    Listed(Vec<Offer>),
    Link { device_id: String, link: String },
    Refused(Refusal),
}

/// The request id an answer to a phone names and what it says.
///
/// Held to the sizes the computer ever sends: a request id that reads,
/// at most [`MAX_OFFERS`] computers of short ids and names, and a link of
/// at most [`MAX_LINK`] bytes; anything longer is `None`, judged before it
/// is copied.
pub fn read_phone_answer(body: &Value) -> Option<(String, PhoneAnswer)> {
    let request = body.get("request")?.as_str()?;
    if !request_reads(request) {
        return None;
    }
    let request = request.to_string();
    if let Some(code) = body.get("refused").and_then(Value::as_str) {
        return Some((request, PhoneAnswer::Refused(Refusal::from_code(code))));
    }
    let answer = match body.get("ask")?.as_str()? {
        "list" => {
            let computers = body.get("computers")?.as_array()?;
            if computers.len() > MAX_OFFERS {
                return None;
            }
            let offers: Vec<Offer> =
                serde_json::from_value(Value::Array(computers.clone())).ok()?;
            if offers
                .iter()
                .any(|o| o.id.len() > MAX_FIELD || o.name.len() > MAX_FIELD)
            {
                return None;
            }
            PhoneAnswer::Listed(offers)
        }
        "pair" => {
            let device_id = body.get("deviceId")?.as_str()?;
            let link = body.get("link")?.as_str()?;
            if device_id.len() > MAX_FIELD || link.len() > MAX_LINK {
                return None;
            }
            PhoneAnswer::Link {
                device_id: device_id.to_string(),
                link: link.to_string(),
            }
        }
        _ => return None,
    };
    Some((request, answer))
}

/// The phone A introduces, as A holds it: its id, the name A shows it by,
/// and the keys it proved to A.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntroducedPhone {
    pub device_id: String,
    #[serde(default)]
    pub name: String,
    /// Hex SHA-256 of its TLS certificate, as bound at A.
    pub tls: String,
    /// Its iroh endpoint id, when A registered one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iroh: Option<String>,
}

/// A's ask of B: introduce `phone`, which A allows `permissions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Introduction {
    pub request: String,
    pub phone: IntroducedPhone,
    pub permissions: DevicePermissions,
}

/// Whether `key` is a hex SHA-256 (64 hex digits).
fn is_hex_hash(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `id` reads as an iroh endpoint id (64 hex digits, or z-base-32
/// text of at most 128 characters).
fn is_endpoint_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Introduction {
    /// The body that asks this.
    pub fn body(&self) -> Value {
        json!({
            "request": self.request,
            "phone": self.phone,
            "permissions": self.permissions,
        })
    }

    /// The introduction in `body`, or `None` for one that does not read:
    /// no request id, an invalid device id, a TLS key that is no hash, an
    /// iroh id that does not read, or permissions that do not read.
    pub fn parse(body: &Value) -> Option<Self> {
        let request = body.get("request")?.as_str()?;
        if !request_reads(request) {
            return None;
        }
        let mut phone: IntroducedPhone = serde_json::from_value(body.get("phone")?.clone()).ok()?;
        if !super::kdeconnect::valid_device_id(&phone.device_id) || !is_hex_hash(&phone.tls) {
            return None;
        }
        if phone.iroh.as_deref().is_some_and(|id| !is_endpoint_id(id)) {
            return None;
        }
        phone.tls = phone.tls.to_ascii_lowercase();
        phone.name = super::kdeconnect::clean_device_name(&phone.name).unwrap_or_default();
        let permissions = serde_json::from_value(body.get("permissions")?.clone()).ok()?;
        Some(Self {
            request: request.to_string(),
            phone,
            permissions,
        })
    }
}

/// What B tells A to reach it by: B's own facts (its name, addresses,
/// ports and iroh endpoint), as it tells a computer it pairs with both
/// ways.
pub type Facts = Value;

/// B's answer: the PIN, and B's own facts.
pub fn accepted_answer(request: &str, pin: &str, facts: Facts) -> Value {
    json!({ "request": request, "pin": pin, "facts": facts })
}

/// B's answer when it does not introduce the phone.
pub fn declined_answer(request: &str, refusal: Refusal) -> Value {
    json!({ "request": request, "refused": refusal.code() })
}

/// B's word that the phone it was introduced under `request` paired:
/// A keeps the introduction only then.
pub fn confirmed_answer(request: &str, device_id: &str) -> Value {
    json!({ "request": request, "paired": device_id })
}

/// A's word that the introduction `request` is off (the phone was removed
/// at A): B drops the PIN it waits with.
pub fn cancel_body(request: &str) -> Value {
    json!({ "cancel": request })
}

/// The introduction an [`INTRODUCE_PHONE`] body cancels, if it is a
/// cancel.
pub fn read_cancel(body: &Value) -> Option<String> {
    let request = body.get("cancel")?.as_str()?;
    request_reads(request).then(|| request.to_string())
}

/// What B's answer says, read at A.
#[derive(Debug, Clone, PartialEq)]
pub enum ComputerAnswer {
    Accepted {
        pin: String,
        facts: Facts,
    },
    Refused(Refusal),
    /// The phone introduced under this request paired.
    Paired {
        device_id: String,
    },
}

/// The request id B's answer names and what it says.
///
/// A request id that does not read, or a paired id over 256 bytes, is
/// `None`.
pub fn read_computer_answer(body: &Value) -> Option<(String, ComputerAnswer)> {
    let request = body.get("request")?.as_str()?;
    if !request_reads(request) {
        return None;
    }
    let request = request.to_string();
    if let Some(device_id) = body.get("paired").and_then(Value::as_str) {
        if device_id.len() > MAX_FIELD {
            return None;
        }
        return Some((
            request,
            ComputerAnswer::Paired {
                device_id: device_id.to_string(),
            },
        ));
    }
    if let Some(code) = body.get("refused").and_then(Value::as_str) {
        return Some((request, ComputerAnswer::Refused(Refusal::from_code(code))));
    }
    let pin = body.get("pin")?.as_str()?;
    if pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        request,
        ComputerAnswer::Accepted {
            pin: pin.to_string(),
            facts: body.get("facts").cloned().unwrap_or(Value::Null),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::permissions::Capability;

    fn phone() -> IntroducedPhone {
        IntroducedPhone {
            device_id: "0d7c3a52-91e4-4b6f-8a2e-00000000aa01".to_string(),
            name: "Pixel".to_string(),
            tls: "ab".repeat(32),
            iroh: Some("cd".repeat(32)),
        }
    }

    #[test]
    fn a_phone_request_reads_back_as_it_was_asked() {
        for ask in [
            PhoneAsk::List,
            PhoneAsk::Pair {
                device_id: "computer-b".to_string(),
            },
        ] {
            let request = PhoneRequest {
                request: "r-1".to_string(),
                ask,
            };
            assert_eq!(PhoneRequest::parse(&request.body()), Some(request));
        }
    }

    #[test]
    fn a_phone_request_that_does_not_read_is_none() {
        for body in [
            json!({ "ask": "list" }),
            json!({ "request": "", "ask": "list" }),
            json!({ "request": "a b", "ask": "list" }),
            json!({ "request": "x".repeat(65), "ask": "list" }),
            json!({ "request": "r", "ask": "everything" }),
            json!({ "request": "r", "ask": "pair" }),
            json!({ "request": "r", "ask": "pair", "deviceId": "" }),
            json!({ "request": "r", "ask": "pair", "deviceId": "a\u{202e}b" }),
        ] {
            assert_eq!(PhoneRequest::parse(&body), None, "{body}");
        }
    }

    /// An answer longer than a computer ever sends is refused whole: too
    /// many computers, a long id, name or link, a request id that does not
    /// read.
    #[test]
    fn an_answer_over_the_limits_is_none() {
        let offer = |id: &str, name: &str| json!({ "id": id, "name": name });
        let list =
            |offers: Vec<Value>| json!({ "request": "r", "ask": "list", "computers": offers });
        let most = vec![offer("b", "Arch"); MAX_OFFERS];
        assert!(
            matches!(read_phone_answer(&list(most)), Some((_, PhoneAnswer::Listed(o))) if o.len() == MAX_OFFERS)
        );
        assert_eq!(
            read_phone_answer(&list(vec![offer("b", "Arch"); MAX_OFFERS + 1])),
            None
        );
        let long = "x".repeat(MAX_FIELD + 1);
        assert_eq!(read_phone_answer(&list(vec![offer(&long, "Arch")])), None);
        assert_eq!(read_phone_answer(&list(vec![offer("b", &long)])), None);
        let link = "x".repeat(MAX_LINK + 1);
        assert_eq!(read_phone_answer(&pair_answer("r", "b", &link)), None);
        assert_eq!(
            read_phone_answer(&pair_answer("r", &long, "pocketdesk://pair?v=1")),
            None
        );
        assert_eq!(
            read_phone_answer(&refused_answer("", &PhoneAsk::List, Refusal::Off)),
            None
        );
        assert_eq!(read_computer_answer(&confirmed_answer("r", &long)), None);
        assert_eq!(read_computer_answer(&confirmed_answer("a b", "p")), None);
        assert_eq!(
            read_computer_answer(&declined_answer(&"x".repeat(65), Refusal::Off)),
            None
        );
    }

    #[test]
    fn each_phone_answer_reads_back() {
        let offer = Offer {
            id: "b".to_string(),
            name: "Arch".to_string(),
        };
        assert_eq!(
            read_phone_answer(&list_answer("r", std::slice::from_ref(&offer))),
            Some(("r".to_string(), PhoneAnswer::Listed(vec![offer])))
        );
        assert_eq!(
            read_phone_answer(&pair_answer("r", "b", "pocketdesk://pair?v=1")),
            Some((
                "r".to_string(),
                PhoneAnswer::Link {
                    device_id: "b".to_string(),
                    link: "pocketdesk://pair?v=1".to_string()
                }
            ))
        );
        for refusal in [
            Refusal::NotOwner,
            Refusal::NotMine,
            Refusal::NotLinked,
            Refusal::Off,
            Refusal::Taken,
            Refusal::NoKey,
            Refusal::Busy,
            Refusal::NoAnswer,
            Refusal::Failed,
        ] {
            assert_eq!(Refusal::from_code(refusal.code()), refusal);
            let ask = PhoneAsk::Pair {
                device_id: "b".to_string(),
            };
            let body = refused_answer("r", &ask, refusal);
            assert_eq!(body["deviceId"], "b");
            assert_eq!(
                read_phone_answer(&body),
                Some(("r".to_string(), PhoneAnswer::Refused(refusal)))
            );
        }
        assert_eq!(Refusal::from_code("something new"), Refusal::Failed);
    }

    #[test]
    fn an_introduction_reads_back_with_its_permissions() {
        let mut permissions = DevicePermissions::default();
        permissions.set(Capability::Exec, false);
        let introduction = Introduction {
            request: "r-2".to_string(),
            phone: phone(),
            permissions,
        };
        assert_eq!(
            Introduction::parse(&introduction.body()),
            Some(introduction)
        );
    }

    #[test]
    fn an_introduction_that_does_not_read_is_none() {
        let good = Introduction {
            request: "r".to_string(),
            phone: phone(),
            permissions: DevicePermissions::default(),
        }
        .body();
        let mut no_request = good.clone();
        no_request["request"] = "".into();
        let mut bad_id = good.clone();
        bad_id["phone"]["deviceId"] = "".into();
        let mut short_key = good.clone();
        short_key["phone"]["tls"] = "abcd".into();
        let mut odd_key = good.clone();
        odd_key["phone"]["tls"] = "zz".repeat(32).into();
        let mut bad_iroh = good.clone();
        bad_iroh["phone"]["iroh"] = "not an id!".into();
        let mut no_permissions = good.clone();
        no_permissions
            .as_object_mut()
            .unwrap()
            .remove("permissions");
        for body in [
            no_request,
            bad_id,
            short_key,
            odd_key,
            bad_iroh,
            no_permissions,
        ] {
            assert_eq!(Introduction::parse(&body), None, "{body}");
        }
    }

    #[test]
    fn a_phones_name_is_cleaned_as_it_is_read() {
        let mut body = Introduction {
            request: "r".to_string(),
            phone: phone(),
            permissions: DevicePermissions::default(),
        }
        .body();
        body["phone"]["name"] = "Pix\u{202e}el".into();
        let read = Introduction::parse(&body).unwrap();
        assert!(!read.phone.name.contains('\u{202e}'));
    }

    #[test]
    fn each_computer_answer_reads_back() {
        let facts = json!({ "name": "Arch", "addresses": ["192.0.2.20"] });
        assert_eq!(
            read_computer_answer(&accepted_answer("r", "042917", facts.clone())),
            Some((
                "r".to_string(),
                ComputerAnswer::Accepted {
                    pin: "042917".to_string(),
                    facts
                }
            ))
        );
        assert_eq!(
            read_computer_answer(&declined_answer("r", Refusal::Off)),
            Some(("r".to_string(), ComputerAnswer::Refused(Refusal::Off)))
        );
        assert_eq!(
            read_computer_answer(&confirmed_answer("r", "p")),
            Some((
                "r".to_string(),
                ComputerAnswer::Paired {
                    device_id: "p".to_string()
                }
            ))
        );
        assert_eq!(read_cancel(&cancel_body("r-9")), Some("r-9".to_string()));
        assert_eq!(read_cancel(&cancel_body("")), None);
        assert_eq!(read_cancel(&json!({ "request": "r" })), None);
        // A PIN that is not six digits does not read.
        assert_eq!(
            read_computer_answer(&json!({ "request": "r", "pin": "12345" })),
            None
        );
    }
}
