//! The phone's location for this computer's apps, through GeoClue (Linux).
//!
//! With "Location from my phone" on (`phone_location`, off by default), this
//! computer serves one NMEA source on its loopback ([`crate::nmea`]) and
//! tells GeoClue about it over Avahi ([`crate::nmea_announce`]). When an app
//! wants a place, GeoClue connects; the first client asks the linked phone for
//! one fix (`phone_location.ask`), the phone, with its own "Give my computer
//! my location" on, answers (`phone_location`), and the fix, rounded to about
//! a kilometre unless exact was chosen on both ends, is sent to the client
//! again every few seconds. The phone is asked again at most every ten
//! minutes, and only while a client is connected.
//!
//! A fix lives in memory here, nowhere else: never in a file, never in the
//! log (the log says only "fix sent, accuracy N m"), never in an event
//! beyond its accuracy, and it is dropped when the setting is turned off.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::phone_location::{ASK, FIX, Fix, Precision, ask_body};
use serde_json::Value;

use crate::nmea::FixSource;

/// A fix is fresh this long: the phone is asked at most this often.
pub const REFRESH: Duration = Duration::from_secs(10 * 60);

/// After an ask that got no fix, the next waits this long.
pub const RETRY: Duration = Duration::from_secs(60);

/// A fix older than [`REFRESH`] is still given, while a new one cannot be
/// had, until it is this old.
pub const KEEP_STALE: Duration = Duration::from_secs(30 * 60);

/// How long the phone has to answer an ask.
const ANSWER_WITHIN: Duration = Duration::from_secs(30);

/// Whether this computer gives its phone's location now: the owner's
/// setting, and only where GeoClue is (Linux).
pub fn offered() -> bool {
    cfg!(target_os = "linux") && crate::events::current_config().phone_location
}

/// The precision the owner chose, as the settings name it.
pub fn chosen_precision() -> Precision {
    Precision::from_name(&crate::events::current_config().phone_location_precision)
}

/// What is known of the last fix and when to ask again. Time is given, so
/// the rules are testable.
#[derive(Debug, Default)]
pub struct Cache {
    last: Option<(Fix, Instant, Precision)>,
    next_ask: Option<Instant>,
}

impl Cache {
    /// The fix at `precision`, if one was got in the last [`REFRESH`] by an
    /// ask for the same precision.
    pub fn fresh(&self, precision: Precision, now: Instant) -> Option<Fix> {
        let (fix, at, asked) = self.last.as_ref()?;
        (*asked == precision && now.saturating_duration_since(*at) < REFRESH)
            .then(|| fix.at(precision))
    }

    /// Whether the phone may be asked at `now`; if so, the next ask waits
    /// [`RETRY`] unless an answer ([`Cache::keep`]) moves it on.
    pub fn may_ask(&mut self, now: Instant) -> bool {
        if self.next_ask.is_some_and(|at| now < at) {
            return false;
        }
        self.next_ask = Some(now + RETRY);
        true
    }

    /// An answer to an ask for `asked`: kept, and the next ask [`REFRESH`] on.
    pub fn keep(&mut self, fix: Fix, asked: Precision, now: Instant) {
        self.last = Some((fix, now, asked));
        self.next_ask = Some(now + REFRESH);
    }

    /// The last fix at `precision` while it is under [`KEEP_STALE`] old,
    /// whatever it was asked for (a fix is only ever given the farther of the
    /// two precisions).
    pub fn stale(&self, precision: Precision, now: Instant) -> Option<Fix> {
        let (fix, at, _) = self.last.as_ref()?;
        (now.saturating_duration_since(*at) < KEEP_STALE).then(|| fix.at(precision))
    }

    /// Forget the fix and the pace.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Who asks the phone.
#[async_trait::async_trait]
pub trait Asker: Send + Sync + 'static {
    /// One fix from the phone at `precision`, with the id of the phone that
    /// gave it, or why there is none (in words that say nothing of where
    /// anything is).
    async fn ask(&self, precision: Precision) -> std::result::Result<(String, Fix), String>;
}

/// What is told of a new fix: the fix as given, and the phone it came from.
type Told = Box<dyn Fn(&Fix, &str) + Send + Sync>;

/// The fix GeoClue's clients are given: the cache and the asking, one ask at
/// a time (clients that come together wait for the one answer).
pub struct Fixes {
    asker: Arc<dyn Asker>,
    cache: tokio::sync::Mutex<Cache>,
    /// Told when a new fix goes out: the fix as given, and the phone it came
    /// from.
    told: Told,
}

impl Fixes {
    pub fn new(asker: Arc<dyn Asker>, told: impl Fn(&Fix, &str) + Send + Sync + 'static) -> Self {
        Self {
            asker,
            cache: Default::default(),
            told: Box::new(told),
        }
    }

    /// Drop the fix: the setting went off.
    pub async fn clear(&self) {
        self.cache.lock().await.clear();
    }
}

#[async_trait::async_trait]
impl FixSource for Fixes {
    async fn fix(&self, precision: Precision) -> Option<Fix> {
        // Held across the ask: a second client waits for this answer, and
        // then finds it fresh.
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        if let Some(fix) = cache.fresh(precision, now) {
            return Some(fix);
        }
        if cache.may_ask(now) {
            match self.asker.ask(precision).await {
                Ok((device, fix)) => {
                    let given = fix.at(precision);
                    tracing::info!("Phone location: fix sent, accuracy {} m", given.accuracy_m);
                    (self.told)(&given, &device);
                    cache.keep(given, precision, Instant::now());
                    return Some(given);
                }
                Err(reason) => {
                    tracing::info!("Phone location: no fix ({reason})");
                    // The owner turned it off on the phone: nothing it gave
                    // before is handed on, however recent (review).
                    if reason == TURNED_OFF {
                        cache.clear();
                        return None;
                    }
                }
            }
        }
        cache.stale(precision, Instant::now())
    }
}

/// The phone that has been asked and the sender of its answer.
struct Pending {
    device_id: String,
    answer: tokio::sync::oneshot::Sender<std::result::Result<(String, Fix), String>>,
}

fn pending() -> &'static Mutex<Option<Pending>> {
    static PENDING: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The phone to ask among `clients`: a connected one of the owner's own
/// (by `is_phone`), the one `chosen` names, or the first when none is named.
pub fn choose_phone<'a>(
    clients: &'a [Arc<dyn DeviceSender>],
    chosen: &str,
    is_phone: &dyn Fn(&str) -> bool,
) -> Option<&'a Arc<dyn DeviceSender>> {
    clients
        .iter()
        .filter(|c| is_phone(c.device_id()))
        .find(|c| chosen.is_empty() || c.device_id() == chosen)
}

/// The phone's own switch is off: what it gave before goes too (review).
const TURNED_OFF: &str = "its Give my computer my location is off";

/// Why a phone's answer was no fix, in words of this computer's own: what
/// the phone says is held to a few known reasons.
fn refusal(body: &Value) -> String {
    match body.get("error").and_then(Value::as_str) {
        Some("off") => TURNED_OFF,
        Some("permission") => "it has no location permission",
        Some("background") => "it needs Pocketdesk open, or location allowed all the time",
        Some("unavailable") => "it found no place",
        Some(_) => "it did not give one",
        None => "its answer was not a place",
    }
    .to_string()
}

/// A phone's answer to an ask, handed to whoever asked: only from the phone
/// that was asked, only once.
fn deliver(device_id: &str, body: &Value) -> bool {
    let waiting = {
        let mut slot = pending().lock().unwrap_or_else(|e| e.into_inner());
        match slot.take() {
            Some(p) if p.device_id == device_id => p,
            other => {
                *slot = other;
                return false;
            }
        }
    };
    let result = match Fix::from_body(body) {
        Some(fix) => Ok((device_id.to_string(), fix)),
        None => Err(refusal(body)),
    };
    waiting.answer.send(result).is_ok()
}

/// Whether `device_id` is one of the owner's own phones: never a guest's,
/// someone else's or a time-limited one, whose place is not this
/// computer's (review).
fn is_owner_phone(device_id: &str) -> bool {
    crate::state::trust_store_path()
        .ok()
        .and_then(|path| pocketdesk_core::protocol::kdeconnect::TrustStore::read(path).ok())
        .is_some_and(|store| store.is_owner_phone(device_id))
}

/// Asks the linked phone.
pub struct PhoneAsker;

#[async_trait::async_trait]
impl Asker for PhoneAsker {
    async fn ask(&self, precision: Precision) -> std::result::Result<(String, Fix), String> {
        let chosen = crate::events::current_config().phone_location_phone;
        let clients = crate::state::clone_clients().await;
        let phone = choose_phone(&clients, &chosen, &is_owner_phone).ok_or("no phone is linked")?;
        let (answer, answered) = tokio::sync::oneshot::channel();
        *pending().lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            device_id: phone.device_id().to_string(),
            answer,
        });
        let packet = NetworkPacket::new(ASK).with_body(ask_body(precision));
        let sent = phone.send_packet(&packet).await;
        let outcome = match sent {
            Err(_) => Err("the ask could not be sent".to_string()),
            Ok(()) => match tokio::time::timeout(ANSWER_WITHIN, answered).await {
                Ok(Ok(result)) => result,
                _ => Err("the phone did not answer".to_string()),
            },
        };
        *pending().lock().unwrap_or_else(|e| e.into_inner()) = None;
        outcome
    }
}

/// Takes the phone's answers to asks.
pub struct PhoneLocationPlugin;

#[async_trait::async_trait]
impl Plugin for PhoneLocationPlugin {
    fn name(&self) -> &'static str {
        "phone_location"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[FIX]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[ASK]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != FIX {
            return Ok(());
        }
        // One of the owner's phones, and only the one asked: a fix nobody
        // asked for is dropped unread, never kept.
        let device = sender.device_id();
        if !is_owner_phone(device) || !offered() || !deliver(device, &packet.body) {
            tracing::info!("A phone's location that was not asked for was dropped");
        }
        Ok(())
    }
}

/// How often the setting is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// What runs while the setting is on.
struct Serving {
    task: tokio::task::JoinHandle<()>,
    announcement: crate::nmea_announce::Announcement,
    /// The port, to announce again where Avahi lost it.
    port: u16,
    precision: Precision,
}

/// Daemon task: serve the NMEA source and announce it while the setting is
/// on; stop, and forget the fix, when it goes off. Runs for the server's
/// life; on a system without GeoClue's platform it ends at once.
pub async fn run() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let fixes = Arc::new(Fixes::new(Arc::new(PhoneAsker), |fix, device| {
        crate::events::emit(
            "phone_location",
            Some(device),
            serde_json::json!({
                "accuracy": fix.accuracy_m,
                "precision": fix.precision.name(),
            }),
            serde_json::json!({}),
        );
    }));
    let mut serving: Option<Serving> = None;
    let mut said = String::new();
    let mut look = tokio::time::interval(LOOK_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        look.tick().await;
        let on = offered();
        match (on, serving.take()) {
            (false, Some(running)) => {
                running.task.abort();
                running.announcement.withdraw().await;
                fixes.clear().await;
                tracing::info!("Phone location is off: the source is withdrawn");
                said.clear();
            }
            (true, None) => match start(fixes.clone()).await {
                Ok(running) => {
                    tracing::info!("Phone location: GeoClue is told of this computer's source");
                    said.clear();
                    serving = Some(running);
                }
                // Said once while it stays the same.
                Err(why) if said != why => {
                    tracing::warn!("Phone location is on but could not start: {why}");
                    said = why;
                }
                Err(_) => {}
            },
            (true, Some(mut running)) => {
                let precision = chosen_precision();
                // Avahi restarted, or the accuracy to say changed.
                if !running.announcement.alive().await || running.precision != precision {
                    running.announcement.withdraw().await;
                    match crate::nmea_announce::Announcement::publish(running.port, precision).await
                    {
                        Ok(announcement) => {
                            running.announcement = announcement;
                            running.precision = precision;
                            serving = Some(running);
                        }
                        Err(why) => {
                            tracing::warn!(
                                "Phone location: the source was not announced again: {why}"
                            );
                            running.task.abort();
                            fixes.clear().await;
                        }
                    }
                } else {
                    serving = Some(running);
                }
            }
            (false, None) => {}
        }
    }
}

async fn start(fixes: Arc<Fixes>) -> std::result::Result<Serving, String> {
    let listeners = crate::nmea::bind()
        .await
        .map_err(|e| format!("no loopback port: {e}"))?;
    let port = listeners[0]
        .local_addr()
        .map_err(|e| format!("no loopback port: {e}"))?
        .port();
    let precision = chosen_precision();
    let announcement = crate::nmea_announce::Announcement::publish(port, precision).await?;
    let task = tokio::spawn(crate::nmea::serve(
        listeners,
        fixes,
        Arc::new(chosen_precision),
        Arc::new(crate::nmea::from_geoclue),
        crate::nmea::SEND_EVERY,
    ));
    Ok(Serving {
        task,
        announcement,
        port,
        precision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::protocol::phone_location::WORST_ACCURACY_M;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fix(precision: Precision) -> Fix {
        Fix {
            lat: 1.23456,
            lon: -20.98765,
            accuracy_m: 12,
            precision,
        }
    }

    #[test]
    fn a_fix_is_fresh_for_ten_minutes_and_for_its_own_precision() {
        let t0 = Instant::now();
        let mut cache = Cache::default();
        assert_eq!(cache.fresh(Precision::City, t0), None);
        cache.keep(fix(Precision::Exact), Precision::City, t0);
        let fresh = cache.fresh(Precision::City, t0 + Duration::from_secs(599));
        assert_eq!(fresh.map(|f| f.precision), Some(Precision::City));
        assert_eq!(cache.fresh(Precision::City, t0 + REFRESH), None);
        assert_eq!(
            cache.fresh(Precision::Exact, t0),
            None,
            "asked for city: exact needs a new ask"
        );
    }

    #[test]
    fn the_phone_is_asked_at_most_every_ten_minutes_and_a_minute_after_a_miss() {
        let t0 = Instant::now();
        let mut cache = Cache::default();
        assert!(cache.may_ask(t0));
        assert!(
            !cache.may_ask(t0 + Duration::from_secs(59)),
            "a miss waits a minute"
        );
        assert!(cache.may_ask(t0 + RETRY));
        cache.keep(fix(Precision::City), Precision::City, t0 + RETRY);
        assert!(!cache.may_ask(t0 + RETRY + Duration::from_secs(599)));
        assert!(cache.may_ask(t0 + RETRY + REFRESH), "ten minutes on");
    }

    #[test]
    fn a_stale_fix_is_given_a_while_and_then_not() {
        let t0 = Instant::now();
        let mut cache = Cache::default();
        cache.keep(fix(Precision::City), Precision::City, t0);
        assert!(cache.stale(Precision::City, t0 + REFRESH * 2).is_some());
        assert!(cache.stale(Precision::City, t0 + KEEP_STALE).is_none());
        cache.clear();
        assert!(cache.stale(Precision::City, t0).is_none());
        assert!(cache.may_ask(t0), "cleared: the pace goes too");
    }

    struct Counting {
        asked: AtomicUsize,
        reply: std::result::Result<Fix, String>,
    }

    #[async_trait::async_trait]
    impl Asker for Counting {
        async fn ask(&self, _: Precision) -> std::result::Result<(String, Fix), String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.reply.clone().map(|fix| ("phone-a".to_string(), fix))
        }
    }

    #[tokio::test]
    async fn clients_share_one_ask_and_its_fix_is_rounded() {
        let asker = Arc::new(Counting {
            asked: AtomicUsize::new(0),
            reply: Ok(fix(Precision::Exact)),
        });
        let told = Arc::new(AtomicUsize::new(0));
        let counted = told.clone();
        let fixes = Fixes::new(asker.clone(), move |f, device| {
            assert_eq!((f.precision, device), (Precision::City, "phone-a"));
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let (a, b) = tokio::join!(fixes.fix(Precision::City), fixes.fix(Precision::City));
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(
            (a.lat, a.lon),
            (1.23, -20.99),
            "rounded here, whatever the phone did"
        );
        assert_eq!(a, b);
        assert_eq!(asker.asked.load(Ordering::SeqCst), 1, "one ask for both");
        assert_eq!(told.load(Ordering::SeqCst), 1, "told once");
        fixes.fix(Precision::City).await.unwrap();
        assert_eq!(
            asker.asked.load(Ordering::SeqCst),
            1,
            "fresh: not asked again"
        );
        fixes.clear().await;
        fixes.fix(Precision::City).await.unwrap();
        assert_eq!(
            asker.asked.load(Ordering::SeqCst),
            2,
            "cleared: asked again"
        );
    }

    #[tokio::test]
    async fn no_fix_is_given_while_the_phone_gives_none() {
        let asker = Arc::new(Counting {
            asked: AtomicUsize::new(0),
            reply: Err("the phone did not answer".to_string()),
        });
        let fixes = Fixes::new(asker.clone(), |_, _| panic!("nothing to tell"));
        assert_eq!(fixes.fix(Precision::City).await, None);
        assert_eq!(fixes.fix(Precision::City).await, None);
        assert_eq!(
            asker.asked.load(Ordering::SeqCst),
            1,
            "a miss is not asked again within the minute"
        );
    }

    struct Phone(&'static str);

    #[async_trait::async_trait]
    impl DeviceSender for Phone {
        fn device_id(&self) -> &str {
            self.0
        }
        fn connection_id(&self) -> &str {
            self.0
        }
        async fn send_packet(&self, _: &NetworkPacket) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_phone_asked_is_the_chosen_one_or_the_first_of_the_owners() {
        let clients: Vec<Arc<dyn DeviceSender>> = vec![
            Arc::new(Phone("computer-a")),
            Arc::new(Phone("phone-a")),
            Arc::new(Phone("phone-b")),
        ];
        let is_phone = |id: &str| id.starts_with("phone-");
        let name = |chosen: &str| {
            choose_phone(&clients, chosen, &is_phone).map(|c| c.device_id().to_string())
        };
        assert_eq!(
            name(""),
            Some("phone-a".to_string()),
            "a computer is never asked"
        );
        assert_eq!(name("phone-b"), Some("phone-b".to_string()));
        assert_eq!(name("computer-a"), None, "not even when named");
        assert_eq!(name("phone-z"), None, "a phone not linked");
        assert!(choose_phone(&[], "", &is_phone).is_none());
    }

    #[tokio::test]
    async fn only_the_phone_asked_is_answered_and_only_once() {
        // The one global slot: this test alone uses it.
        assert!(
            !deliver("phone-a", &json!({"lat": 1.0, "lon": 2.0})),
            "nobody asked"
        );
        let (answer, answered) = tokio::sync::oneshot::channel();
        *pending().lock().unwrap() = Some(Pending {
            device_id: "phone-a".to_string(),
            answer,
        });
        assert!(
            !deliver("phone-b", &json!({"lat": 1.0, "lon": 2.0})),
            "not the phone asked"
        );
        assert!(deliver(
            "phone-a",
            &json!({"lat": 1.0, "lon": 2.0, "accuracy": 9})
        ));
        let (from, got) = answered.await.unwrap().unwrap();
        assert_eq!(
            (from.as_str(), got.lat, got.lon, got.accuracy_m),
            ("phone-a", 1.0, 2.0, 9)
        );
        assert!(
            !deliver("phone-a", &json!({"lat": 1.0, "lon": 2.0})),
            "once"
        );

        // A refusal says why, in this computer's words, and a body that is
        // no place is one too.
        for (body, why) in [
            (json!({"error": "off"}), "off"),
            (json!({"error": "permission"}), "permission"),
            (
                json!({"error": "attacker text with 1.23, 4.56"}),
                "did not give",
            ),
            (json!({"lat": 91.0, "lon": 0.0}), "not a place"),
            (
                json!({"lat": 1.0, "lon": 2.0, "accuracy": -1, "error": null}),
                "",
            ),
        ] {
            let (answer, answered) = tokio::sync::oneshot::channel();
            *pending().lock().unwrap() = Some(Pending {
                device_id: "phone-a".to_string(),
                answer,
            });
            assert!(deliver("phone-a", &body));
            match answered.await.unwrap() {
                Ok((_, fix)) => assert_eq!((why, fix.accuracy_m), ("", WORST_ACCURACY_M)),
                Err(reason) => {
                    assert!(reason.contains(why), "{reason}");
                    assert!(!reason.contains("1.23"), "a phone's words are never echoed");
                }
            }
        }
    }

    #[test]
    fn the_packets_are_not_for_a_phone_without_a_session() {
        use pocketdesk_core::protocol::permissions::capability_for;
        // Gated by the setting and the one phone asked, not by a permission.
        assert_eq!(capability_for(ASK), None);
        assert_eq!(capability_for(FIX), None);
    }
}
