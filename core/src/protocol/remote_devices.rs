//! Removing a lost device from another of the owner's devices (remote
//! revoke): an owner device asks a computer which devices are paired
//! there, and asks it to remove one, as the computer's own Devices page
//! Unpair would. The computer answers only an owner device: a phone paired
//! with the owner's own code (never someone else's, a profile's or a
//! guest's), or another of the owner's computers paired both ways whose
//! grant has not lapsed. The rules are the server's
//! (`server/src/remote_devices.rs`); this is what goes on the wire, so the
//! phone's bridge and both computers read it alike.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Owner device → computer: `{ask: "list"}` or `{ask: "remove", deviceId}`,
/// each with the asker's `request` id, which the answer names.
pub const REMOTE_DEVICES: &str = "kdeconnect.pocketdesk.remote_devices";

/// Computer → the device that asked: `{request, ask: "list", devices}`,
/// `{request, ask: "remove", deviceId, removed, reason}`, or `{request,
/// ask, refused}` when the asker may not ask at all.
pub const REMOTE_DEVICES_ANSWER: &str = "kdeconnect.pocketdesk.remote_devices_answer";

/// At most this many removals a minute are taken from one asker.
pub const REMOVALS_PER_MINUTE: usize = 5;

/// The longest request id taken: an id is the asker's own, echoed back.
const MAX_REQUEST: usize = 64;

/// The most devices a list answer may name; more is refused whole, before
/// any is read.
pub const MAX_LISTED: usize = 512;

/// The longest device id or name an answer may carry, in bytes (a removal
/// names at most this many, too).
const MAX_FIELD: usize = 256;

/// Whether `request` is a request id this protocol takes: 1 to
/// [`MAX_REQUEST`] printable ASCII characters.
fn request_reads(request: &str) -> bool {
    !request.is_empty()
        && request.len() <= MAX_REQUEST
        && request.bytes().all(|b| b.is_ascii_graphic())
}

/// What an owner device asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The devices paired with the computer.
    List,
    /// Remove the paired device `device_id`.
    Remove { device_id: String },
}

/// A request read from a [`REMOTE_DEVICES`] body: its id and its ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub request: String,
    pub ask: Ask,
}

impl Request {
    /// The body that asks this.
    pub fn body(&self) -> Value {
        match &self.ask {
            Ask::List => json!({ "request": self.request, "ask": "list" }),
            Ask::Remove { device_id } => {
                json!({ "request": self.request, "ask": "remove", "deviceId": device_id })
            }
        }
    }

    /// The request in `body`, or `None` for one that does not read: an
    /// unknown ask, a removal that names no device, a request id that is
    /// missing, too long or not printable ASCII.
    pub fn parse(body: &Value) -> Option<Self> {
        let request = body.get("request")?.as_str()?;
        if !request_reads(request) {
            return None;
        }
        let ask = match body.get("ask")?.as_str()? {
            "list" => Ask::List,
            "remove" => {
                let device_id = body.get("deviceId")?.as_str()?;
                if device_id.is_empty() || device_id.len() > MAX_FIELD {
                    return None;
                }
                Ask::Remove {
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

/// One device paired with the computer, as an owner device is told of it:
/// what its Devices page shows, never a key, an address or a permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThereDevice {
    /// Its device id, which a removal names.
    pub id: String,
    /// Its name there (the owner's rename, else its own), or empty.
    #[serde(default)]
    pub name: String,
    /// A computer, not a phone.
    #[serde(default)]
    pub computer: bool,
    /// An owner device there, which may itself ask this.
    #[serde(default)]
    pub owner: bool,
    /// A guest's time-boxed grant.
    #[serde(default)]
    pub guest: bool,
    /// Connected to the computer now (a link, or a session).
    #[serde(default)]
    pub connected: bool,
    /// The device that asked.
    #[serde(default)]
    pub you: bool,
}

/// Why a computer did not do what an owner device asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The asker is not an owner device there.
    NotOwner,
    /// More than [`REMOVALS_PER_MINUTE`] removals asked in a minute.
    TooMany,
    /// The asker named itself.
    Yourself,
    /// The device named is not paired there.
    NotPaired,
    /// Removing it would leave the computer with no owner device. The
    /// asker is an owner device that stays, so a computer never says it
    /// today; kept as a guard and on the wire.
    LastOwner,
    /// The computer's owner turned removal from afar off
    /// ("Let my devices remove others from afar", `remote_revoke`).
    Off,
    /// Too many lists asked in a minute.
    Busy,
    /// The computer could not write its trust store.
    Failed,
}

impl Refusal {
    /// Its word on the wire.
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NotOwner => "not_owner",
            Refusal::TooMany => "too_many",
            Refusal::Yourself => "yourself",
            Refusal::NotPaired => "not_paired",
            Refusal::LastOwner => "last_owner",
            Refusal::Off => "off",
            Refusal::Busy => "busy",
            Refusal::Failed => "failed",
        }
    }

    /// The refusal a code names; an unknown code is a failure.
    pub fn from_code(code: &str) -> Self {
        match code {
            "not_owner" => Refusal::NotOwner,
            "too_many" => Refusal::TooMany,
            "yourself" => Refusal::Yourself,
            "not_paired" => Refusal::NotPaired,
            "last_owner" => Refusal::LastOwner,
            "off" => Refusal::Off,
            "busy" => Refusal::Busy,
            _ => Refusal::Failed,
        }
    }

    /// What the owner reads.
    pub fn words(self) -> &'static str {
        match self {
            Refusal::NotOwner => {
                "That computer takes this only from a device of yours: a phone paired with its own code, or a computer paired both ways."
            }
            Refusal::TooMany => "Too many removals in a minute: wait a minute and try again.",
            Refusal::Yourself => "A device cannot remove itself this way: unpair it there.",
            Refusal::NotPaired => "That device is not paired there any more.",
            Refusal::LastOwner => {
                "That is the last device of yours there: removing it would leave none to manage it."
            }
            Refusal::Off => {
                "That computer's owner turned off removing devices from afar: remove it at that computer (Devices → Unpair)."
            }
            Refusal::Busy => "Too many asks in a minute: wait a minute and try again.",
            Refusal::Failed => "That computer could not do it: try again.",
        }
    }
}

/// The answer to a list.
pub fn list_answer(request: &str, devices: &[ThereDevice]) -> Value {
    json!({ "request": request, "ask": "list", "devices": devices })
}

/// The answer to a removal of `device_id`: removed, or why not.
pub fn remove_answer(request: &str, device_id: &str, outcome: Result<(), Refusal>) -> Value {
    match outcome {
        Ok(()) => json!({
            "request": request, "ask": "remove", "deviceId": device_id, "removed": true,
        }),
        Err(refusal) => json!({
            "request": request, "ask": "remove", "deviceId": device_id, "removed": false,
            "reason": refusal.code(),
        }),
    }
}

/// The answer to any ask the computer does not take at all: from a device
/// that may not ask (`NotOwner`), with removal from afar turned off
/// (`Off`), too many at once (`Busy`), or a store it could not read
/// (`Failed`).
pub fn refused_answer(request: &str, ask: &Ask, refusal: Refusal) -> Value {
    let word = match ask {
        Ask::List => "list",
        Ask::Remove { .. } => "remove",
    };
    json!({ "request": request, "ask": word, "refused": refusal.code() })
}

/// What an answer says, read on the asking side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The devices there.
    Listed(Vec<ThereDevice>),
    /// `device_id` was removed, or why not.
    Removed {
        device_id: String,
        outcome: Result<(), Refusal>,
    },
    /// The asker may not ask.
    Refused(Refusal),
}

/// The request id an answer names and what it says, or `None` for one
/// that does not read: a request id that does not read, more than
/// [`MAX_LISTED`] devices, or an id or name over 256 bytes, each measured
/// before any of it is copied.
pub fn read_answer(body: &Value) -> Option<(String, Answer)> {
    let request = body.get("request")?.as_str()?;
    if !request_reads(request) {
        return None;
    }
    let request = request.to_string();
    if let Some(code) = body.get("refused").and_then(Value::as_str) {
        return Some((request, Answer::Refused(Refusal::from_code(code))));
    }
    let answer = match body.get("ask")?.as_str()? {
        "list" => {
            let devices = body.get("devices")?.as_array()?;
            if devices.len() > MAX_LISTED {
                return None;
            }
            // Each id and name measured where it lies, then each device
            // read from it: nothing over the limits is copied.
            let fits = |item: &Value| {
                ["id", "name"].iter().all(|key| {
                    item.get(key)
                        .and_then(Value::as_str)
                        .is_none_or(|v| v.len() <= MAX_FIELD)
                })
            };
            if !devices.iter().all(fits) {
                return None;
            }
            let devices = devices
                .iter()
                .map(ThereDevice::deserialize)
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            Answer::Listed(devices)
        }
        "remove" => {
            let device_id = body.get("deviceId")?.as_str()?;
            if device_id.len() > MAX_FIELD {
                return None;
            }
            let device_id = device_id.to_string();
            let outcome = if body.get("removed")?.as_bool()? {
                Ok(())
            } else {
                Err(Refusal::from_code(
                    body.get("reason").and_then(Value::as_str).unwrap_or(""),
                ))
            };
            Answer::Removed { device_id, outcome }
        }
        _ => return None,
    };
    Some((request, answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_back_as_it_was_asked() {
        for ask in [
            Ask::List,
            Ask::Remove {
                device_id: "phone-1".to_string(),
            },
        ] {
            let request = Request {
                request: "r-1".to_string(),
                ask,
            };
            assert_eq!(Request::parse(&request.body()), Some(request));
        }
    }

    #[test]
    fn a_request_that_does_not_read_is_none() {
        for body in [
            json!({ "ask": "list" }),
            json!({ "request": "", "ask": "list" }),
            json!({ "request": "a b", "ask": "list" }),
            json!({ "request": "x".repeat(65), "ask": "list" }),
            json!({ "request": "r", "ask": "wipe" }),
            json!({ "request": "r", "ask": "remove" }),
            json!({ "request": "r", "ask": "remove", "deviceId": "" }),
        ] {
            assert_eq!(Request::parse(&body), None, "{body}");
        }
    }

    /// An answer longer than a computer ever sends is refused whole: too
    /// many devices, a long id or name, a request id that does not read.
    #[test]
    fn an_answer_over_the_limits_is_none() {
        let device = |id: &str, name: &str| json!({ "id": id, "name": name });
        let list =
            |devices: Vec<Value>| json!({ "request": "r", "ask": "list", "devices": devices });
        let most = vec![device("p", "Pixel"); MAX_LISTED];
        assert!(
            matches!(read_answer(&list(most)), Some((_, Answer::Listed(d))) if d.len() == MAX_LISTED)
        );
        assert_eq!(
            read_answer(&list(vec![device("p", "Pixel"); MAX_LISTED + 1])),
            None
        );
        let long = "x".repeat(MAX_FIELD + 1);
        assert_eq!(read_answer(&list(vec![device(&long, "Pixel")])), None);
        assert_eq!(read_answer(&list(vec![device("p", &long)])), None);
        let removed = json!({ "request": "r", "ask": "remove", "deviceId": long, "removed": true });
        assert_eq!(read_answer(&removed), None);
        for request in [
            String::new(),
            "a b".to_string(),
            "x".repeat(MAX_REQUEST + 1),
        ] {
            let body = json!({ "request": request, "refused": "off" });
            assert_eq!(read_answer(&body), None, "{body}");
        }
    }

    #[test]
    fn each_answer_reads_back() {
        let device = ThereDevice {
            id: "p".to_string(),
            name: "Pixel".to_string(),
            computer: false,
            owner: true,
            guest: false,
            connected: true,
            you: false,
        };
        assert_eq!(
            read_answer(&list_answer("r", std::slice::from_ref(&device))),
            Some(("r".to_string(), Answer::Listed(vec![device])))
        );
        assert_eq!(
            read_answer(&remove_answer("r", "p", Ok(()))),
            Some((
                "r".to_string(),
                Answer::Removed {
                    device_id: "p".to_string(),
                    outcome: Ok(())
                }
            ))
        );
        for refusal in [
            Refusal::NotOwner,
            Refusal::TooMany,
            Refusal::Yourself,
            Refusal::NotPaired,
            Refusal::LastOwner,
            Refusal::Off,
            Refusal::Busy,
            Refusal::Failed,
        ] {
            assert_eq!(Refusal::from_code(refusal.code()), refusal);
            assert_eq!(
                read_answer(&remove_answer("r", "p", Err(refusal))),
                Some((
                    "r".to_string(),
                    Answer::Removed {
                        device_id: "p".to_string(),
                        outcome: Err(refusal)
                    }
                ))
            );
        }
        assert_eq!(
            read_answer(&refused_answer("r", &Ask::List, Refusal::NotOwner)),
            Some(("r".to_string(), Answer::Refused(Refusal::NotOwner)))
        );
        assert_eq!(
            read_answer(&refused_answer("r", &Ask::List, Refusal::Off)),
            Some(("r".to_string(), Answer::Refused(Refusal::Off)))
        );
    }
}
