//! Tests of [`super`]: every refusal on both computers, the happy path end
//! to end in one process, and removal following the introduction.

use super::*;
use pocketdesk_core::protocol::introductions::{PhoneAnswer, read_phone_answer};
use serde_json::json;

// The introduction PINs, and the pace counters (`LISTS`, `PAIRS`,
// `TAKEN`), are one for the process: the tests that mint or take PINs take
// turns (`PIN_TURN`), and each test paces under ids of its own.

fn pin_turn() -> std::sync::MutexGuard<'static, ()> {
    PIN_TURN.lock().unwrap_or_else(|e| e.into_inner())
}

const PHONE_TLS: &str = "11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa";
const PHONE_IROH: &str = "22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb";
const B_CERT: &str = "33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc";

/// A computer's state folder, removed after.
struct Dirs {
    dir: std::path::PathBuf,
}

impl Dirs {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pd-intro-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn trust(&self) -> std::path::PathBuf {
        self.dir.join("trusted_devices.json")
    }

    fn store(&self) -> TrustStore {
        TrustStore::read(self.trust()).unwrap()
    }

    fn file(&self) -> ComputersFile {
        computers::read(&self.dir)
    }

    /// A computer listed here, paired both ways (`mine`) or not.
    fn list_computer(&self, id: &str, mine: bool) {
        computers::update(&self.dir, |file| {
            file.computers.push(PeerComputer {
                device_id: id.to_string(),
                name: format!("Computer {id}"),
                addresses: vec!["192.0.2.10".to_string()],
                control_port: 1716,
                stream_port: 4716,
                fingerprint: B_CERT.to_string(),
                endpoint: Some(json!({ "endpointId": "ee".repeat(32) })),
                paired_unix: 1,
                link: String::new(),
                link_unix: 0,
                lets_drive: false,
                mine,
                wake: None,
                viewed_unix: 0,
                keys_by_position: None,
            });
        })
        .unwrap();
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Computer A: the owner's phone `owner` (its keys bound), someone else's,
/// a guest, an owner phone with no key, and computers paired both ways,
/// waiting, lapsed and one way.
fn household(d: &Dirs, owner: &str) {
    TrustStore::update(d.trust(), |s| {
        s.trust_phone(owner, true)?;
        s.trust_phone("keyless-phone", true)?;
        s.trust_phone("someone-else", false)?;
        s.trust_device_with_ttl("guest", Duration::from_secs(600))?;
        s.trust_computer("mine-pc")?;
        s.trust_computer("pending-pc")?;
        s.trust_computer("lapsed-pc")?;
        s.trust_computer("one-way-pc")?;
        Ok(())
    })
    .unwrap();
    device_keys::replace(&d.dir, owner, PHONE_TLS).unwrap();
    device_keys::register(&d.dir, owner, Slot::Iroh, PHONE_IROH).unwrap();
    d.list_computer("mine-pc", true);
    d.list_computer("pending-pc", true);
    d.list_computer("lapsed-pc", true);
    computers::update(&d.dir, |file| {
        file.pending.push("pending-pc".to_string());
        file.lapsed.push("lapsed-pc".to_string());
    })
    .unwrap();
}

/// Computer B, paired both ways with `a`, one computer one way, and a
/// phone of its own.
fn computer_b(d: &Dirs, a: &str) {
    TrustStore::update(d.trust(), |s| {
        s.trust_computer(a)?;
        s.trust_computer("one-way-pc")?;
        s.trust_phone("b-phone", true)?;
        Ok(())
    })
    .unwrap();
    d.list_computer(a, true);
}

fn list(request: &str) -> Value {
    PhoneRequest {
        request: request.to_string(),
        ask: PhoneAsk::List,
    }
    .body()
}

fn pair(request: &str, computer: &str) -> Value {
    PhoneRequest {
        request: request.to_string(),
        ask: PhoneAsk::Pair {
            device_id: computer.to_string(),
        },
    }
    .body()
}

/// An ask of the other computer that must not happen.
async fn never(_: String, _: Introduction) -> Result<(String, Value), Refusal> {
    panic!("the other computer was asked")
}

async fn ask_phone(
    d: &Dirs,
    asker: &str,
    body: Value,
    reachable: impl Fn(&str) -> bool,
) -> PhoneAnswer {
    let answer = answer_phone_in(&d.dir, &d.trust(), asker, &body, reachable, never)
        .await
        .expect("an answer");
    read_phone_answer(&answer).expect("it reads").1
}

/// [`decide_in`], its permissions aside.
fn decide(
    dir: &std::path::Path,
    trust: &std::path::Path,
    from: &str,
    introduction: &Introduction,
    enabled: bool,
    own_id: &str,
) -> Result<(), Refusal> {
    decide_in(dir, trust, from, introduction, enabled, own_id).map(|_| ())
}

/// Let `from` introduce again at once (the pace is the process's).
fn unpace(from: &str) {
    TAKEN.lock().unwrap().asks.remove(from);
}

/// The bytes of a hex key.
fn key_of(hex_key: &str) -> PeerKey {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex_key[i * 2..i * 2 + 2], 16).unwrap();
    }
    PeerKey::Tls(bytes)
}

#[tokio::test]
async fn only_an_owner_phone_is_answered() {
    let d = Dirs::new("owner");
    household(&d, "owner-1");
    for asker in ["someone-else", "guest", "mine-pc", "one-way-pc", "stranger"] {
        for body in [list("r"), pair("r", "mine-pc")] {
            assert_eq!(
                ask_phone(&d, asker, body, |_| true).await,
                PhoneAnswer::Refused(Refusal::NotOwner),
                "{asker}"
            );
        }
    }
    assert!(matches!(
        ask_phone(&d, "owner-1", list("r"), |_| true).await,
        PhoneAnswer::Listed(_)
    ));
}

#[tokio::test]
async fn a_body_that_does_not_read_is_not_answered() {
    let d = Dirs::new("unread");
    household(&d, "owner-0");
    for body in [json!({}), json!({ "request": "r", "ask": "wipe" })] {
        assert!(
            answer_phone_in(&d.dir, &d.trust(), "owner-0", &body, |_| true, never)
                .await
                .is_none()
        );
    }
}

#[tokio::test]
async fn the_list_offers_only_computers_paired_both_ways_and_linked() {
    let d = Dirs::new("offers");
    household(&d, "owner-2");
    let listed = ask_phone(&d, "owner-2", list("r"), |_| true).await;
    assert_eq!(
        listed,
        PhoneAnswer::Listed(vec![Offer {
            id: "mine-pc".to_string(),
            name: "Computer mine-pc".to_string(),
        }])
    );
    // Not linked now: not offered.
    assert_eq!(
        ask_phone(&d, "owner-2", list("r"), |_| false).await,
        PhoneAnswer::Listed(vec![])
    );
}

#[tokio::test]
async fn a_computer_not_paired_both_ways_is_never_introduced_to() {
    let d = Dirs::new("not-mine");
    household(&d, "owner-3");
    for computer in [
        "one-way-pc",
        "pending-pc",
        "lapsed-pc",
        "unknown-pc",
        "someone-else",
    ] {
        assert_eq!(
            ask_phone(&d, "owner-3", pair("r", computer), |_| true).await,
            PhoneAnswer::Refused(Refusal::NotMine),
            "{computer}"
        );
    }
}

#[tokio::test]
async fn a_phone_with_no_key_bound_here_is_not_vouched_for() {
    let d = Dirs::new("no-key");
    household(&d, "owner-4");
    assert_eq!(
        ask_phone(&d, "keyless-phone", pair("r", "mine-pc"), |_| true).await,
        PhoneAnswer::Refused(Refusal::NoKey)
    );
}

#[tokio::test]
async fn a_computer_not_linked_now_is_not_asked() {
    let d = Dirs::new("unlinked");
    household(&d, "owner-5");
    assert_eq!(
        ask_phone(&d, "owner-5", pair("r", "mine-pc"), |_| false).await,
        PhoneAnswer::Refused(Refusal::NotLinked)
    );
}

#[tokio::test]
async fn the_other_computers_refusal_reaches_the_phone_and_nothing_is_kept() {
    let d = Dirs::new("refused-there");
    household(&d, "owner-6");
    for refusal in [
        Refusal::Off,
        Refusal::Taken,
        Refusal::NotMine,
        Refusal::NoAnswer,
    ] {
        let answer = answer_phone_in(
            &d.dir,
            &d.trust(),
            "owner-6",
            &pair("r", "mine-pc"),
            |_| true,
            |_, _| async move { Err(refusal) },
        )
        .await
        .unwrap();
        assert_eq!(
            read_phone_answer(&answer).unwrap().1,
            PhoneAnswer::Refused(refusal)
        );
    }
    assert!(d.file().introduced.is_empty());
}

#[tokio::test]
async fn a_phones_introductions_are_paced() {
    let d = Dirs::new("pace");
    household(&d, "owner-7");
    for _ in 0..INTRODUCTIONS_PER_WINDOW {
        assert_eq!(
            ask_phone(&d, "owner-7", pair("r", "mine-pc"), |_| false).await,
            PhoneAnswer::Refused(Refusal::NotLinked)
        );
    }
    assert_eq!(
        ask_phone(&d, "owner-7", pair("r", "mine-pc"), |_| false).await,
        PhoneAnswer::Refused(Refusal::Busy)
    );
}

#[tokio::test]
async fn the_phone_gets_a_link_made_of_the_pin_and_what_this_computer_pinned() {
    let d = Dirs::new("link");
    household(&d, "owner-8");
    let asked = Mutex::new(None);
    let answer = answer_phone_in(
        &d.dir,
        &d.trust(),
        "owner-8",
        &pair("r-8", "mine-pc"),
        |_| true,
        |to, introduction| {
            *asked.lock().unwrap() = Some((to, introduction));
            async {
                Ok((
                    "042917".to_string(),
                    json!({
                        "name": "B's own word",
                        "addresses": ["192.0.2.20", "192.0.2.10"],
                        "controlPort": 1817,
                        "streamPort": 4817,
                        "endpoint": { "endpointId": "ff".repeat(32) },
                    }),
                ))
            }
        },
    )
    .await
    .unwrap();
    // What the other computer was told: the phone and its keys here.
    let (to, introduction) = asked.lock().unwrap().clone().unwrap();
    assert_eq!(to, "mine-pc");
    assert_eq!(introduction.phone.device_id, "owner-8");
    assert_eq!(introduction.phone.tls, PHONE_TLS);
    assert_eq!(introduction.phone.iroh.as_deref(), Some(PHONE_IROH));
    let (request, said) = read_phone_answer(&answer).unwrap();
    assert_eq!(request, "r-8");
    let PhoneAnswer::Link { device_id, link } = said else {
        panic!("no link: {said:?}");
    };
    assert_eq!(device_id, "mine-pc");
    let link = PairLink::from_uri(&link).unwrap();
    assert_eq!(link.device_id, "mine-pc");
    assert_eq!(link.pin, "042917");
    // The certificate, endpoint and ports this computer pinned and lists,
    // never the other's word for them.
    assert_eq!(link.fingerprint.as_deref(), Some(B_CERT));
    let endpoint = link.endpoint.unwrap();
    assert!(endpoint.contains(&"ee".repeat(32)));
    assert!(!endpoint.contains(&"ff".repeat(32)));
    assert_eq!((link.control_port, link.stream_port), (1716, 4716));
    // Those listed here first, then any more it names, once each.
    assert_eq!(link.addresses, vec!["192.0.2.10", "192.0.2.20"]);
    assert_eq!(link.name, "Computer mine-pc");
    assert!(!link.mine);
    // Kept as waiting for the other computer's word that it paired.
    let file = d.file();
    assert!(file.introduced.is_empty());
    assert_eq!(file.pending_introductions.len(), 1);
    let pending = &file.pending_introductions[0];
    assert_eq!(
        (pending.phone.as_str(), pending.computer.as_str()),
        ("owner-8", "mine-pc")
    );
    assert_eq!(pending.request, introduction.request);
}

/// A computer listed (one way), for the owed-removal tests.
fn listed(id: &str) -> PeerComputer {
    PeerComputer {
        device_id: id.to_string(),
        name: id.to_string(),
        addresses: vec![],
        control_port: 1716,
        stream_port: 4716,
        fingerprint: B_CERT.to_string(),
        endpoint: None,
        paired_unix: 1,
        link: String::new(),
        link_unix: 0,
        lets_drive: false,
        mine: true,
        wake: None,
        viewed_unix: 0,
        keys_by_position: None,
    }
}

fn introduction(phone: &str) -> Introduction {
    Introduction {
        request: "r".to_string(),
        phone: IntroducedPhone {
            device_id: phone.to_string(),
            name: "Pixel".to_string(),
            tls: PHONE_TLS.to_string(),
            iroh: Some(PHONE_IROH.to_string()),
        },
        permissions: DevicePermissions::default(),
    }
}

#[test]
fn an_introduction_is_taken_only_from_a_computer_paired_both_ways() {
    let d = Dirs::new("from");
    household(&d, "owner-9");
    for from in [
        "one-way-pc",
        "pending-pc",
        "lapsed-pc",
        "stranger",
        "owner-9",
        "someone-else",
    ] {
        unpace(from);
        assert_eq!(
            decide(
                &d.dir,
                &d.trust(),
                from,
                &introduction("new-1"),
                true,
                "b-self"
            ),
            Err(Refusal::NotMine),
            "{from}"
        );
    }
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "mine-pc",
            &introduction("new-1"),
            true,
            "b-self"
        ),
        Ok(())
    );
}

#[test]
fn the_owners_switch_turns_introductions_away() {
    let d = Dirs::new("off");
    computer_b(&d, "a-off");
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "a-off",
            &introduction("new-2"),
            false,
            "b-self"
        ),
        Err(Refusal::Off)
    );
}

#[test]
fn an_id_in_use_here_is_never_introduced() {
    let d = Dirs::new("taken");
    computer_b(&d, "a-taken");
    d.list_computer("listed-pc", false);
    device_keys::replace(&d.dir, "bound-phone", &"99".repeat(32)).unwrap();
    for id in [
        "b-phone",
        "one-way-pc",
        "listed-pc",
        "b-self",
        "a-taken",
        "bound-phone",
    ] {
        unpace("a-taken");
        assert_eq!(
            decide(
                &d.dir,
                &d.trust(),
                "a-taken",
                &introduction(id),
                true,
                "b-self"
            ),
            Err(Refusal::Taken),
            "{id}"
        );
    }
    // Bound to the same key (keys left behind): the same phone.
    unpace("a-taken");
    device_keys::replace(&d.dir, "left-phone", PHONE_TLS).unwrap();
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "a-taken",
            &introduction("left-phone"),
            true,
            "b-self"
        ),
        Ok(())
    );
}

#[test]
fn a_computers_introductions_are_paced() {
    let d = Dirs::new("taken-pace");
    computer_b(&d, "a-pace");
    for _ in 0..INTRODUCTIONS_PER_WINDOW {
        assert_eq!(
            decide(
                &d.dir,
                &d.trust(),
                "a-pace",
                &introduction("new-3"),
                true,
                "b-self"
            ),
            Ok(())
        );
    }
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "a-pace",
            &introduction("new-3"),
            true,
            "b-self"
        ),
        Err(Refusal::Busy)
    );
}

#[test]
fn a_pin_pairs_only_its_phone_with_its_key_once_and_in_time() {
    let _turn = pin_turn();
    PINS.lock().unwrap().clear();
    let pin = mint("a", introduction("pin-phone")).unwrap();
    let right = key_of(PHONE_TLS);
    let wrong = key_of(&"44".repeat(32));
    assert!(take_pin("other-phone", Some(&right), &pin).is_none());
    assert!(take_pin("pin-phone", Some(&wrong), &pin).is_none());
    assert!(take_pin("pin-phone", None, &pin).is_none());
    assert!(
        take_pin(
            "pin-phone",
            Some(&PeerKey::Iroh(PHONE_IROH.to_string())),
            &pin
        )
        .is_none()
    );
    let other = if pin == "000000" { "000001" } else { "000000" };
    assert!(take_pin("pin-phone", Some(&right), other).is_none());
    let taken = take_pin("pin-phone", Some(&right), &pin).expect("its PIN");
    assert_eq!(taken.introducer, "a");
    assert!(
        take_pin("pin-phone", Some(&right), &pin).is_none(),
        "once only"
    );
    // One past its time pairs nothing.
    let pin = mint("a", introduction("late-phone")).unwrap();
    for p in PINS.lock().unwrap().iter_mut() {
        p.minted = Instant::now() - PIN_FOR - Duration::from_secs(1);
    }
    assert!(take_pin("late-phone", Some(&right), &pin).is_none());
    // At most PINS_WAITING at once.
    PINS.lock().unwrap().clear();
    for n in 0..PINS_WAITING {
        assert!(mint("a", introduction(&format!("cap-{n}"))).is_ok());
    }
    assert_eq!(mint("a", introduction("cap-over")), Err(Refusal::Busy));
    // A phone already waiting replaces its own PIN, cap or not.
    assert!(mint("a", introduction("cap-0")).is_ok());
    assert_eq!(PINS.lock().unwrap().len(), PINS_WAITING);
    // The same phone introduced again replaces its own waiting PIN.
    PINS.lock().unwrap().clear();
    let first = mint("a", introduction("again-phone")).unwrap();
    let second = mint("a", introduction("again-phone")).unwrap();
    assert_eq!(PINS.lock().unwrap().len(), 1);
    if first != second {
        assert!(take_pin("again-phone", Some(&right), &first).is_none());
    }
    assert!(take_pin("again-phone", Some(&right), &second).is_some());
    PINS.lock().unwrap().clear();
}

fn intro_pin(introducer: &str, phone: &str, permissions: DevicePermissions) -> IntroPin {
    let introduced = introduction(phone);
    IntroPin {
        pin: "042917".to_string(),
        phone: introduced.phone,
        permissions,
        introducer: introducer.to_string(),
        request: "r-pin".to_string(),
        minted: Instant::now(),
        misses: 0,
    }
}

#[test]
fn an_admitted_phone_is_an_owner_phone_with_its_keys_and_no_more_than_both_allow() {
    let d = Dirs::new("admit");
    computer_b(&d, "a-admit");
    let mut at_a = DevicePermissions::default();
    at_a.set(Capability::Exec, false);
    at_a.set(Capability::SharedInput, true);
    let admitted = admit_in(
        &d.dir,
        &d.trust(),
        &intro_pin("a-admit", "new-phone", at_a),
        true,
        "b-self",
        7,
    )
    .unwrap();
    assert_eq!(admitted.phone, "Pixel");
    assert!(admitted.introducer.contains("Computer a-admit"));
    assert_eq!(
        added_words(&admitted),
        format!("{} added Pixel to this computer", admitted.introducer)
    );
    let store = d.store();
    assert!(store.is_owner_phone("new-phone"));
    assert!(!store.is_computer("new-phone"));
    let permissions = store.permissions("new-phone");
    assert!(!permissions.allows(Capability::Exec), "what A turned off");
    assert!(
        !permissions.allows(Capability::SharedInput),
        "only its own page sets"
    );
    assert_eq!(
        permissions.allows(Capability::Clipboard),
        DevicePermissions::default().allows(Capability::Clipboard)
    );
    let keys = device_keys::read(&d.dir)
        .unwrap()
        .remove("new-phone")
        .unwrap();
    assert_eq!(keys.tls.as_deref(), Some(PHONE_TLS));
    assert_eq!(keys.iroh.as_deref(), Some(PHONE_IROH));
    assert_eq!(store.claimed_name("new-phone"), Some("Pixel"));
    assert_eq!(
        d.file().introduced,
        vec![Introduced {
            phone: "new-phone".to_string(),
            computer: "a-admit".to_string(),
            unix: 7,
        }]
    );
}

#[test]
fn an_introduction_that_no_longer_holds_admits_nothing() {
    let d = Dirs::new("admit-no");
    computer_b(&d, "a-gone");
    let pin = intro_pin("a-gone", "late-phone", DevicePermissions::default());
    // The switch turned off since.
    assert_eq!(
        admit_in(&d.dir, &d.trust(), &pin, false, "b-self", 1),
        Err(Refusal::Off)
    );
    // The id paired here since.
    TrustStore::update(d.trust(), |s| s.trust_phone("late-phone", false)).unwrap();
    assert_eq!(
        admit_in(&d.dir, &d.trust(), &pin, true, "b-self", 1),
        Err(Refusal::Taken)
    );
    TrustStore::update(d.trust(), |s| s.untrust_device("late-phone")).unwrap();
    // The introducer removed here since (unpaired, or revoked from afar).
    TrustStore::update(d.trust(), |s| s.untrust_device("a-gone")).unwrap();
    assert_eq!(
        admit_in(&d.dir, &d.trust(), &pin, true, "b-self", 1),
        Err(Refusal::NotMine)
    );
    assert!(!d.store().is_trusted("late-phone"));
    assert!(
        !device_keys::read(&d.dir)
            .unwrap()
            .contains_key("late-phone")
    );
    assert!(d.file().introduced.is_empty());
    // No computers file at all: no computer is paired both ways, and none
    // is made.
    let e = Dirs::new("admit-empty");
    assert_eq!(
        admit_in(&e.dir, &e.trust(), &pin, true, "b-self", 1),
        Err(Refusal::NotMine)
    );
    assert!(
        !e.dir
            .join(pocketdesk_core::local::paths::COMPUTERS_FILE)
            .exists()
    );
}

/// A's ask, B's answer and the phone's pairing at B, end to end in one
/// process: two state folders, with the link and the phone's connection
/// played by calls.
#[tokio::test]
// The turn is held through the test on purpose: it mints and takes a PIN.
#[allow(clippy::await_holding_lock)]
async fn a_phone_introduced_by_a_pairs_with_b_as_an_owner_phone() {
    let _turn = pin_turn();
    let a = Dirs::new("loop-a");
    let b = Dirs::new("loop-b");
    household(&a, "loop-phone");
    // B lists A as paired both ways (A lists B as "mine-pc").
    computer_b(&b, "a-loop");
    let (b_dir, b_trust) = (b.dir.clone(), b.trust());
    let answer = answer_phone_in(
        &a.dir,
        &a.trust(),
        "loop-phone",
        &pair("r-loop", "mine-pc"),
        |_| true,
        |_to, introduction| async move {
            // On the link, as B's answer_computer reads, decides and mints.
            let introduction = Introduction::parse(&introduction.body()).ok_or(Refusal::Failed)?;
            decide_in(&b_dir, &b_trust, "a-loop", &introduction, true, "mine-pc")?;
            let pin = mint("a-loop", introduction)?;
            Ok((pin, json!({ "addresses": ["192.0.2.30"] })))
        },
    )
    .await
    .unwrap();
    let PhoneAnswer::Link { link, .. } = read_phone_answer(&answer).unwrap().1 else {
        panic!("no link");
    };
    let link = PairLink::from_uri(&link).unwrap();
    assert_eq!(link.addresses, vec!["192.0.2.10", "192.0.2.30"]);
    // The phone pairs at B with the link's PIN, proving its key.
    let intro = take_pin("loop-phone", Some(&key_of(PHONE_TLS)), &link.pin).expect("the PIN");
    admit_in(&b.dir, &b.trust(), &intro, true, "mine-pc", 9).unwrap();
    assert!(b.store().is_owner_phone("loop-phone"));
    assert_eq!(b.store().introducer("loop-phone"), Some("a-loop"));
    assert!(!b.store().is_own_code_owner("loop-phone"));
    assert_eq!(b.file().introduced[0].computer, "a-loop");
    // B owes A its word, and A keeps the introduction only on it.
    let owed = b.file().owed_confirmations;
    assert_eq!(owed.len(), 1);
    assert!(a.file().introduced.is_empty());
    computers::update(&a.dir, |file| {
        assert!(!confirm_in(
            file,
            "another-pc",
            &owed[0].request,
            "loop-phone",
            10
        ));
        assert!(confirm_in(
            file,
            "mine-pc",
            &owed[0].request,
            "loop-phone",
            10
        ));
    })
    .unwrap();
    let file = a.file();
    assert_eq!(file.introduced[0].computer, "mine-pc");
    assert!(file.pending_introductions.is_empty());
}

#[test]
fn a_phone_removed_here_is_owed_to_the_computers_it_was_introduced_between() {
    let mut file = ComputersFile::default();
    note_introduced(&mut file, "p1", "b", 1);
    note_introduced(&mut file, "p1", "c", 1);
    note_introduced(&mut file, "p2", "b", 1);
    note_introduced(&mut file, "p2", "b", 2);
    assert_eq!(file.introduced.len(), 3, "one entry per phone and computer");
    let paired = vec!["p2".to_string()];
    assert!(owe_in(&mut file, &paired, 100));
    assert_eq!(file.introduced.len(), 1);
    assert_eq!(file.owed_removals.len(), 2);
    assert!(
        file.owed_removals
            .iter()
            .all(|o| o.phone == "p1" && o.unix == 100 && o.sent_unix == 0)
    );
    // Nothing more owed the second time.
    assert!(!owe_in(&mut file, &paired, 101));
    assert_eq!(file.owed_removals.len(), 2);
}

#[test]
fn an_owed_removal_goes_when_its_computer_is_reachable_and_again_unanswered() {
    let mut file = ComputersFile::default();
    note_introduced(&mut file, "p", "b", 1);
    note_introduced(&mut file, "p", "c", 1);
    owe_in(&mut file, &[], 1000);
    for id in ["b", "c"] {
        file.computers.push(listed(id));
    }
    let (due, dropped) = due_in(&mut file, 1000, |id| id == "b");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].computer, "b");
    assert!(dropped.is_empty());
    // Not sent: due again at once (PR #581 N1).
    assert_eq!(due_in(&mut file, 1001, |id| id == "b").0.len(), 1);
    // Sent: not again before its wait.
    mark_sent(&mut file, &due[0].request, 1001);
    assert!(due_in(&mut file, 1002, |id| id == "b").0.is_empty());
    assert_eq!(
        due_in(&mut file, 1001 + OWED_RESEND, |id| id == "b")
            .0
            .len(),
        1
    );
    // Past its time, dropped unsent, for the caller to say.
    let (due, dropped) = due_in(&mut file, 1000 + OWED_FOR, |_| true);
    assert!(due.is_empty());
    assert_eq!(dropped.len(), 2);
    assert!(file.owed_removals.is_empty());
}

#[test]
fn an_owed_removal_is_dropped_once_answered_unless_to_be_tried_again() {
    let mut file = ComputersFile::default();
    note_introduced(&mut file, "p", "b", 1);
    owe_in(&mut file, &[], 1);
    let request = file.owed_removals[0].request.clone();
    let removed = |outcome| revoke::Answer::Removed {
        device_id: "p".to_string(),
        outcome,
    };
    // Another computer's answer, or another request's: not this one.
    assert_eq!(
        answered_in(&mut file, "c", &request, &removed(Ok(()))),
        None
    );
    assert_eq!(answered_in(&mut file, "b", "other", &removed(Ok(()))), None);
    for again in [
        revoke::Refusal::TooMany,
        revoke::Refusal::Busy,
        revoke::Refusal::Failed,
    ] {
        assert_eq!(
            answered_in(&mut file, "b", &request, &removed(Err(again))),
            Some(Err(again))
        );
        assert_eq!(file.owed_removals.len(), 1, "kept to try again");
    }
    assert_eq!(
        answered_in(
            &mut file,
            "b",
            &request,
            &revoke::Answer::Refused(revoke::Refusal::Off)
        ),
        Some(Err(revoke::Refusal::Off))
    );
    assert!(file.owed_removals.is_empty(), "its owner said no: dropped");
    note_introduced(&mut file, "p", "b", 1);
    owe_in(&mut file, &[], 1);
    let request = file.owed_removals[0].request.clone();
    assert_eq!(
        answered_in(&mut file, "b", &request, &removed(Ok(()))),
        Some(Ok(()))
    );
    assert!(file.owed_removals.is_empty());
}

#[test]
fn a_removal_asked_by_a_computer_owes_it_none_back() {
    let d = Dirs::new("no-echo");
    computer_b(&d, "a-echo");
    computers::update(&d.dir, |file| {
        note_introduced(file, "b-phone", "a-echo", 1);
        note_introduced(file, "b-phone", "c-other", 1);
    })
    .unwrap();
    crate::remote_devices::remove_in(&d.dir, &d.trust(), "a-echo", "b-phone").unwrap();
    let mut file = d.file();
    assert_eq!(file.introduced.len(), 1);
    assert_eq!(file.introduced[0].computer, "c-other");
    let paired = d.store().trusted_devices();
    assert!(owe_in(&mut file, &paired, 2));
    assert_eq!(file.owed_removals.len(), 1);
    assert_eq!(file.owed_removals[0].computer, "c-other");
}

#[test]
fn unpairing_a_computer_forgets_its_introductions_and_drops_its_owed_removals_said() {
    let d = Dirs::new("unpair-pc");
    computer_b(&d, "a-unpair");
    computers::update(&d.dir, |file| {
        note_introduced(file, "p", "a-unpair", 1);
        owe_in(file, &[], 1);
        note_introduced(file, "q", "a-unpair", 1);
        file.remove("a-unpair");
    })
    .unwrap();
    let mut file = d.file();
    assert!(file.introduced.is_empty());
    // The owed removal is dropped by the server, which says so (N1).
    assert_eq!(file.owed_removals.len(), 1);
    let (due, dropped) = due_in(&mut file, 2, |_| true);
    assert!(due.is_empty());
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].phone, "p");
}

#[test]
fn heard_takes_only_the_answer_of_the_computer_asked() {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    WAITING
        .lock()
        .unwrap()
        .insert("req-heard".to_string(), ("b".to_string(), tx));
    let answer = wire::accepted_answer("req-heard", "042917", json!({}));
    heard("c", &answer);
    assert!(rx.try_recv().is_err(), "another computer's is dropped");
    heard("b", &answer);
    assert!(matches!(rx.try_recv(), Ok(ComputerAnswer::Accepted { .. })));
    assert!(!WAITING.lock().unwrap().contains_key("req-heard"));
}

#[test]
fn the_permissions_given_never_exceed_the_defaults_or_the_introducers() {
    let mut wide = DevicePermissions::default();
    for capability in Capability::ALL {
        wide.set(capability, true);
    }
    let given = permissions_for(&wide, &DevicePermissions::computer_mine());
    for capability in Capability::ALL {
        if capability == Capability::SharedInput {
            assert!(!given.allows(capability));
        } else {
            assert_eq!(
                given.allows(capability),
                capability.default_allowed(),
                "{capability:?}"
            );
        }
    }
    let given = permissions_for(
        &DevicePermissions::someone_else(),
        &DevicePermissions::computer_mine(),
    );
    assert!(given.ask_first);
    assert!(!given.allows(Capability::Input));
}

#[tokio::test]
async fn a_phone_removed_while_the_other_computer_was_asked_is_handed_nothing() {
    let d = Dirs::new("removed-meanwhile");
    household(&d, "owner-10");
    let trust = d.trust();
    let answer = answer_phone_in(
        &d.dir,
        &d.trust(),
        "owner-10",
        &pair("r-10", "mine-pc"),
        |_| true,
        |_, _| async move {
            // Unpaired here while the other computer answered.
            TrustStore::update(&trust, |s| s.untrust_device("owner-10")).unwrap();
            Ok(("042917".to_string(), json!({})))
        },
    )
    .await
    .unwrap();
    assert_eq!(
        read_phone_answer(&answer).unwrap().1,
        PhoneAnswer::Refused(Refusal::NotOwner)
    );
    assert!(d.file().introduced.is_empty());
}

#[test]
fn an_id_bound_to_another_iroh_key_is_never_introduced() {
    let d = Dirs::new("taken-iroh");
    computer_b(&d, "a-iroh");
    device_keys::register(&d.dir, "iroh-phone", Slot::Iroh, &"77".repeat(32)).unwrap();
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "a-iroh",
            &introduction("iroh-phone"),
            true,
            "b-self"
        ),
        Err(Refusal::Taken)
    );
}

// ---- Fix round 1 (PR #581) ----

#[test]
fn a_computer_may_cancel_only_its_own_waiting_pin() {
    let _turn = pin_turn();
    PINS.lock().unwrap().clear();
    let mut first = introduction("cancel-phone");
    first.request = "req-a".to_string();
    mint("a", first).unwrap();
    assert!(!cancel_pin("c", "req-a"), "another computer's cancel");
    assert!(!cancel_pin("a", "req-other"));
    assert!(cancel_pin("a", "req-a"));
    assert!(PINS.lock().unwrap().is_empty());
    // And by the phone it names, from that computer only.
    mint("a", introduction("cancel-phone")).unwrap();
    assert!(!cancel_phone("c", "cancel-phone"));
    assert!(cancel_phone("a", "cancel-phone"));
    PINS.lock().unwrap().clear();
}

#[test]
fn another_computers_waiting_pin_for_the_phone_is_never_replaced() {
    let _turn = pin_turn();
    PINS.lock().unwrap().clear();
    mint("a", introduction("contested")).unwrap();
    assert_eq!(mint("c", introduction("contested")), Err(Refusal::Busy));
    assert_eq!(PINS.lock().unwrap()[0].introducer, "a");
    PINS.lock().unwrap().clear();
}

#[test]
fn three_wrong_pins_discard_the_introduction_pin() {
    let _turn = pin_turn();
    PINS.lock().unwrap().clear();
    let right = key_of(PHONE_TLS);
    assert!(
        !miss("no-pin-phone", Some(&right)),
        "no PIN waits: not its miss"
    );
    let pin = mint("a", introduction("missing-phone")).unwrap();
    // Another key, or none, never touches its count (re-review R2).
    let stranger = key_of(&"44".repeat(32));
    for _ in 0..PIN_MISSES + 1 {
        assert!(!miss("missing-phone", Some(&stranger)));
        assert!(!miss("missing-phone", None));
    }
    assert_eq!(PINS.lock().unwrap()[0].misses, 0);
    assert!(miss("missing-phone", Some(&right)));
    assert!(miss("missing-phone", Some(&right)));
    assert_eq!(PINS.lock().unwrap().len(), 1, "two misses keep it");
    assert!(miss("missing-phone", Some(&right)));
    assert!(PINS.lock().unwrap().is_empty(), "the third discards it");
    assert!(take_pin("missing-phone", Some(&key_of(PHONE_TLS)), &pin).is_none());
    PINS.lock().unwrap().clear();
}

#[test]
fn a_phone_removed_before_its_pairing_was_confirmed_owes_a_cancel_and_a_removal() {
    let mut file = ComputersFile::default();
    file.pending_introductions.push(PendingIntroduction {
        phone: "p".to_string(),
        computer: "b".to_string(),
        request: "req-p".to_string(),
        unix: 1,
    });
    assert!(owe_in(&mut file, &[], 5));
    assert!(file.pending_introductions.is_empty());
    let owed = &file.owed_removals[0];
    assert_eq!(owed.cancel.as_deref(), Some("req-p"));
    let packets = owed_packets(owed);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].packet_type, INTRODUCE_PHONE);
    assert_eq!(
        wire::read_cancel(&packets[0].body).as_deref(),
        Some("req-p")
    );
    assert_eq!(packets[1].packet_type, revoke::REMOTE_DEVICES);
    assert_eq!(packets[1].body["introduced"], true);
    assert_eq!(packets[1].body["deviceId"], "p");
}

#[test]
fn an_owed_removal_is_taken_only_for_a_phone_that_computer_introduced() {
    let _turn = pin_turn();
    PINS.lock().unwrap().clear();
    let d = Dirs::new("gate");
    computer_b(&d, "a-gate");
    // Not paired yet: its waiting PIN from that computer goes (I1).
    mint("a-gate", introduction("waiting-phone")).unwrap();
    assert_eq!(
        owed_gate_in(&d.dir, &d.trust(), "a-gate", "waiting-phone"),
        OwedGate::Cancelled
    );
    assert!(PINS.lock().unwrap().is_empty());
    // Paired with this computer's own code: not that computer's to remove
    // this way (N3).
    assert_eq!(
        owed_gate_in(&d.dir, &d.trust(), "a-gate", "b-phone"),
        OwedGate::NotIntroduced
    );
    // Introduced by it: removed.
    TrustStore::update(d.trust(), |s| {
        s.trust_introduced_phone("intro-phone", "a-gate")
    })
    .unwrap();
    assert_eq!(
        owed_gate_in(&d.dir, &d.trust(), "a-gate", "intro-phone"),
        OwedGate::Remove
    );
    assert_eq!(
        owed_gate_in(&d.dir, &d.trust(), "c-other", "intro-phone"),
        OwedGate::NotIntroduced
    );
}

#[test]
fn what_b_turned_off_for_a_stays_off_for_the_phones_a_introduces() {
    let mut for_a = DevicePermissions::computer_mine();
    for_a.set(Capability::Input, false);
    for_a.set(Capability::Mic, false);
    let given = permissions_for(&DevicePermissions::default(), &for_a);
    assert!(!given.allows(Capability::Input));
    assert!(!given.allows(Capability::Mic));
    // What a computer never has is not "turned off": the phone keeps the
    // defaults for it.
    assert_eq!(
        given.allows(Capability::Notifications),
        DevicePermissions::default().allows(Capability::Notifications)
    );
    // Held for the owner when the introducer's sessions are.
    let mut asked = DevicePermissions::computer_mine();
    asked.ask_first = true;
    assert!(permissions_for(&DevicePermissions::default(), &asked).ask_first);
}

#[test]
fn an_admitted_phone_is_held_to_what_b_allows_a_now() {
    let d = Dirs::new("admit-for-a");
    computer_b(&d, "a-narrow");
    TrustStore::update(d.trust(), |s| {
        let mut for_a = DevicePermissions::computer_mine();
        for_a.set(Capability::Input, false);
        s.set_permissions("a-narrow", for_a)
    })
    .unwrap();
    admit_in(
        &d.dir,
        &d.trust(),
        &intro_pin("a-narrow", "narrow-phone", DevicePermissions::default()),
        true,
        "b-self",
        1,
    )
    .unwrap();
    assert!(
        !d.store()
            .permissions("narrow-phone")
            .allows(Capability::Input)
    );
}

#[tokio::test]
async fn an_introduced_phone_is_never_introduced_onward() {
    let d = Dirs::new("no-chain");
    household(&d, "owner-11");
    TrustStore::update(d.trust(), |s| {
        s.trust_introduced_phone("relayed-phone", "mine-pc")
    })
    .unwrap();
    device_keys::replace(&d.dir, "relayed-phone", PHONE_TLS).unwrap();
    for body in [list("r"), pair("r", "mine-pc")] {
        assert_eq!(
            ask_phone(&d, "relayed-phone", body, |_| true).await,
            PhoneAnswer::Refused(Refusal::NotOwner)
        );
    }
    // Still an owner phone for remote revoke.
    assert!(crate::remote_devices::is_owner_device(
        &d.store(),
        &d.file(),
        "relayed-phone"
    ));
    // Paired again with this computer's own code, it is one of its own.
    TrustStore::update(d.trust(), |s| s.trust_phone("relayed-phone", true)).unwrap();
    assert!(matches!(
        ask_phone(&d, "relayed-phone", list("r"), |_| true).await,
        PhoneAnswer::Listed(_)
    ));
}

#[test]
fn phones_a_introduced_lose_the_owner_mark_when_a_is_no_longer_mine() {
    let d = Dirs::new("orphans");
    computer_b(&d, "a-orphan");
    TrustStore::update(d.trust(), |s| {
        s.trust_introduced_phone("orphan-phone", "a-orphan")?;
        s.note_claimed_name("orphan-phone", "Pixel").map(|_| ())
    })
    .unwrap();
    assert!(
        demote_orphans_in(&d.dir, &d.trust()).is_empty(),
        "A still mine"
    );
    // A's grant lapses (it is no longer paired both ways).
    computers::update(&d.dir, |file| file.lapsed.push("a-orphan".to_string())).unwrap();
    let demoted = demote_orphans_in(&d.dir, &d.trust());
    assert_eq!(demoted.len(), 1);
    assert_eq!(demoted[0].0, "Pixel");
    let store = d.store();
    assert!(store.is_trusted("orphan-phone"), "stays paired");
    assert!(!store.is_owner_phone("orphan-phone"));
    assert_eq!(store.introducer("orphan-phone"), None);
    assert!(demote_orphans_in(&d.dir, &d.trust()).is_empty(), "once");
    // A unpaired at B outright, likewise.
    let e = Dirs::new("orphans-unpaired");
    computer_b(&e, "a-gone");
    TrustStore::update(e.trust(), |s| {
        s.trust_introduced_phone("p2", "a-gone")?;
        s.untrust_device("a-gone")
    })
    .unwrap();
    assert_eq!(demote_orphans_in(&e.dir, &e.trust()).len(), 1);
    assert!(e.store().is_trusted("p2"));
    assert!(!e.store().is_owner_phone("p2"));
}

#[test]
fn a_missing_or_unreadable_trust_store_owes_no_removal() {
    let d = Dirs::new("no-store");
    computers::update(&d.dir, |file| note_introduced(file, "p", "b", 1)).unwrap();
    owe_removals(&d.dir, &d.trust());
    assert!(d.file().owed_removals.is_empty(), "missing");
    std::fs::write(d.trust(), b"{ not json").unwrap();
    owe_removals(&d.dir, &d.trust());
    assert!(d.file().owed_removals.is_empty(), "unreadable");
    assert_eq!(d.file().introduced.len(), 1);
    // Read, without the phone: owed.
    std::fs::remove_file(d.trust()).unwrap();
    TrustStore::update(d.trust(), |s| s.trust_phone("other", true)).unwrap();
    owe_removals(&d.dir, &d.trust());
    assert_eq!(d.file().owed_removals.len(), 1);
}

#[test]
fn a_computers_refused_introductions_are_paced_too() {
    let d = Dirs::new("refused-pace");
    computer_b(&d, "a-off-pace");
    for _ in 0..INTRODUCTIONS_PER_WINDOW {
        assert_eq!(
            decide(
                &d.dir,
                &d.trust(),
                "a-off-pace",
                &introduction("x"),
                false,
                "b-self"
            ),
            Err(Refusal::Off)
        );
    }
    assert_eq!(
        decide(
            &d.dir,
            &d.trust(),
            "a-off-pace",
            &introduction("x"),
            false,
            "b-self"
        ),
        Err(Refusal::Busy)
    );
}

#[test]
fn a_waiting_entry_goes_however_the_wait_ends() {
    let (tx, _rx) = tokio::sync::oneshot::channel();
    WAITING
        .lock()
        .unwrap()
        .insert("req-guard".to_string(), ("b".to_string(), tx));
    drop(WaitingEntry("req-guard".to_string()));
    assert!(!WAITING.lock().unwrap().contains_key("req-guard"));
}

#[tokio::test]
async fn owed_removals_are_marked_sent_only_once_they_went_and_confirmations_go_once() {
    let d = Dirs::new("send-owed");
    TrustStore::update(d.trust(), |s| s.trust_phone("kept", true)).unwrap();
    computers::update(&d.dir, |file| {
        file.computers.push(listed("b"));
        note_introduced(file, "gone-phone", "b", 1);
        file.owed_confirmations.push(PendingIntroduction {
            phone: "kept".to_string(),
            computer: "b".to_string(),
            request: "req-confirm".to_string(),
            unix: pocketdesk_core::local::pin::now_unix(),
        });
    })
    .unwrap();
    // The link takes nothing: nothing is marked, nothing confirmed.
    let tried = Mutex::new(0);
    let dropped = send_owed_with(
        &d.dir,
        &d.trust(),
        |_| true,
        |_to, _packet| {
            *tried.lock().unwrap() += 1;
            async { false }
        },
    )
    .await;
    assert!(dropped.is_empty());
    let file = d.file();
    assert_eq!(file.owed_removals.len(), 1);
    assert_eq!(file.owed_removals[0].sent_unix, 0);
    assert_eq!(file.owed_confirmations.len(), 1);
    assert_eq!(*tried.lock().unwrap(), 2);
    // The link takes them: marked, and the confirmation goes once.
    let sent = Mutex::new(Vec::new());
    send_owed_with(
        &d.dir,
        &d.trust(),
        |_| true,
        |_to, packet| {
            sent.lock().unwrap().push(packet.body.clone());
            async { true }
        },
    )
    .await;
    let file = d.file();
    assert!(file.owed_removals[0].sent_unix > 0);
    assert!(file.owed_confirmations.is_empty());
    let sent = sent.lock().unwrap();
    assert!(sent.iter().any(|b| b["paired"] == "kept"));
    assert!(
        sent.iter()
            .any(|b| b["deviceId"] == "gone-phone" && b["introduced"] == true)
    );
}

#[test]
fn answered_drops_an_owed_removal_and_leaves_the_apps_own_asks() {
    let d = Dirs::new("answered");
    computers::update(&d.dir, |file| {
        file.computers.push(listed("b"));
        note_introduced(file, "p", "b", 1);
        owe_in(file, &[], 1);
    })
    .unwrap();
    let request = d.file().owed_removals[0].request.clone();
    let removed = revoke::Answer::Removed {
        device_id: "p".to_string(),
        outcome: Err(revoke::Refusal::Off),
    };
    assert!(!answered(&d.dir, "b", "an-app-ask", &removed));
    assert!(!answered(&d.dir, "c", &request, &removed));
    assert!(answered(&d.dir, "b", &request, &removed));
    assert!(d.file().owed_removals.is_empty());
}
