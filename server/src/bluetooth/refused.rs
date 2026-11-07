//! A phone turned away on the Bluetooth road, told to the owner: logged
//! always, and at most once a minute per device in Activity
//! (`bluetooth_refused`) with a desktop notification at most once a minute
//! for them all, so a phone dialing again on its back-off, or a bonded
//! device that is not Pocketdesk's, does not fill the screen.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Each device's refusal is told at most this often, and the notification
/// for any of them too.
const EVERY: Duration = Duration::from_secs(60);

/// Past this many devices told, the expired are let go.
const PRUNE_AT: usize = 256;

/// Why a control connection over Bluetooth was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Not a phone paired here.
    NotPaired,
    /// A paired phone, but not with the TLS key bound to it here (or with
    /// none bound yet).
    OtherKey,
    /// One of the owner's computers, or one being recorded as paired both
    /// ways: the road is for phones only.
    Computer,
}

impl Refused {
    /// The event's `reason`.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Refused::NotPaired => "not_paired",
            Refused::OtherKey => "other_key",
            Refused::Computer => "computer",
        }
    }

    /// Why, in the log's words.
    fn reason(self) -> &'static str {
        match self {
            Refused::NotPaired => "not paired here",
            Refused::OtherKey => "paired, but not with this key",
            Refused::Computer => "a computer, and the road is for phones only",
        }
    }
}

/// When each device's refusal was last told, and the last notification
/// (under `None`).
static TOLD: LazyLock<Mutex<HashMap<Option<String>, Instant>>> = LazyLock::new(Default::default);

/// Whether a notice under `key` is due at `now`, noting it told if it is.
fn due(told: &mut HashMap<Option<String>, Instant>, key: Option<String>, now: Instant) -> bool {
    if told
        .get(&key)
        .is_some_and(|at| now.duration_since(*at) < EVERY)
    {
        return false;
    }
    // A bonded device that claims a new id each time adds one each: the
    // expired go first, which changes nothing they would say.
    if told.len() > PRUNE_AT {
        told.retain(|_, at| now.duration_since(*at) < EVERY);
    }
    told.insert(key, now);
    true
}

/// The notification's summary and body for `why`, the phone called `name`.
fn words(why: Refused, name: &str) -> (&'static str, String) {
    let summary = "A connection over Bluetooth was refused";
    let body = match why {
        Refused::NotPaired => "A device that is not paired here tried to connect over Bluetooth. \
             Pair a phone at home first."
            .to_string(),
        Refused::OtherKey => format!(
            "A device claiming to be {name} tried to connect over Bluetooth with a key this \
             computer does not know. If it was your phone, connect it at home once, or pair it \
             again."
        ),
        Refused::Computer => {
            format!("{name} tried to connect over Bluetooth, which is for phones only.")
        }
    };
    (summary, body)
}

/// `device_id` was refused over Bluetooth for `why`: logged, and told to
/// the owner when due.
pub(crate) fn refused(device_id: &str, why: Refused) {
    tracing::warn!(
        "Refused {device_id}'s control connection over Bluetooth: {}",
        why.reason()
    );
    let now = Instant::now();
    let (event, notify) = {
        let mut told = TOLD.lock().unwrap_or_else(|e| e.into_inner());
        let event = due(&mut told, Some(device_id.to_string()), now);
        (event, event && due(&mut told, None, now))
    };
    if !event || cfg!(test) {
        return;
    }
    crate::events::emit(
        "bluetooth_refused",
        Some(device_id),
        serde_json::json!({ "reason": why.code() }),
        serde_json::json!({}),
    );
    if notify {
        let device_id = device_id.to_string();
        std::thread::spawn(move || {
            let name = crate::state::trust_store_path()
                .ok()
                .and_then(|path| pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok())
                .and_then(|store| store.name(&device_id).map(str::to_string))
                .unwrap_or_else(|| {
                    format!("Device {}", device_id.chars().take(6).collect::<String>())
                });
            let (summary, body) = words(why, &name);
            crate::platform::notify(summary, &body, Duration::from_secs(10));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_device_is_told_once_a_minute_and_the_notification_once_for_all() {
        let mut told = HashMap::new();
        let start = Instant::now();
        let a = || Some("phone-a".to_string());
        let b = || Some("phone-b".to_string());
        assert!(due(&mut told, a(), start));
        assert!(due(&mut told, None, start), "the first notification");
        assert!(
            !due(&mut told, a(), start + Duration::from_secs(30)),
            "a again within the minute"
        );
        assert!(
            due(&mut told, b(), start + Duration::from_secs(30)),
            "b has its own minute"
        );
        assert!(
            !due(&mut told, None, start + Duration::from_secs(30)),
            "but no second notification within the minute"
        );
        assert!(due(&mut told, a(), start + EVERY), "a after the minute");
        assert!(due(&mut told, None, start + EVERY));
    }

    #[test]
    fn the_expired_are_let_go_past_the_limit() {
        let mut told = HashMap::new();
        let start = Instant::now();
        for n in 0..=PRUNE_AT {
            assert!(due(&mut told, Some(format!("id-{n}")), start));
        }
        let later = start + EVERY;
        assert!(due(&mut told, Some("fresh".to_string()), later));
        assert_eq!(told.len(), 1, "only the fresh one is left");
        assert!(
            !due(&mut told, Some("fresh".to_string()), later),
            "and still counts"
        );
    }

    #[test]
    fn the_reasons_and_words() {
        assert_eq!(
            [Refused::NotPaired, Refused::OtherKey, Refused::Computer].map(Refused::code),
            ["not_paired", "other_key", "computer"]
        );
        let (_, body) = words(Refused::NotPaired, "Pixel");
        assert!(
            !body.contains("Pixel"),
            "a stranger's claimed name is never said: {body}"
        );
        let (_, body) = words(Refused::OtherKey, "Pixel");
        assert!(
            body.contains("claiming to be Pixel") && body.contains("at home"),
            "{body}"
        );
        let (_, body) = words(Refused::Computer, "Laptop");
        assert!(body.contains("phones only"), "{body}");
    }
}
