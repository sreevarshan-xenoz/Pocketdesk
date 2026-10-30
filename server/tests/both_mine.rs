//! "Both are mine" (PC1): two real servers side by side (R6a's seams: each
//! with its own config and state folders and ports) pair once from a link
//! made for another of the owner's computers, and then each lists the
//! other, trusts it to see and control it without asking, the two keep one
//! link between them, and each shows the other's screen with no prompt.
//!
//! A link that says `mine=1` when the code was not minted for that pairs
//! one way, as before; so does a link for both ways pasted without the
//! pasting owner's yes (a hostile link looks just like it: review C1), and
//! the pasting computer then trusts the link's maker with nothing. With
//! the yes, it pairs both ways, the host's grant waiting for the link. A
//! link that reuses a listed computer's id with another certificate is
//! refused, and a computer unpaired on Devices is forgotten there too. The
//! host's grant is absent until the real link is up (re-review I-2), and a
//! stranger with one of the host's codes, claiming the paster's id with
//! another certificate, is refused as a computer and as a phone, the
//! paster's keys there untouched (re-review I-1). Each screen is the test pattern (`POCKETDESK_TEST_PATTERN`), so no
//! one's desktop is read; the stand-in windows send nothing at all, so
//! nothing is injected though control is allowed. No sound is sent (no
//! Audio permission either way), and any that came would go to a file. No
//! picture is kept: frame counts and sizes are the evidence. Each server's
//! log goes to its throwaway folder and is only counted, never printed.
//!
//! Ignored: it starts two real servers with a real encoder, which reach
//! iroh's relays. Run it with
//! `cargo test -p pocketdesk-server --test both_mine -- --ignored --nocapture --test-threads=1`.

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::device_keys::{self, Slot, Verdict};
use pocketdesk_core::local::pair_link::PairLink;
use pocketdesk_core::local::paths::{LIVE_FILE, PENDING_APPROVALS_FILE, TRUST_FILE};
use pocketdesk_core::local::view::{self as wire, Message, Phase};
use pocketdesk_core::local::{identity, pin, sessions};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::DevicePermissions;

/// What a stand-in window saw of one view.
#[derive(Debug, Default)]
struct Seen {
    frames: u64,
    bytes: u64,
    size: Option<(u16, u16)>,
    route: String,
    control: Option<bool>,
    ended: Option<String>,
}

/// Read `state`'s view relay for `other` as the window does for `secs`
/// after the first frame, keeping only counts and the size. It sends
/// nothing.
fn watch(state: &Path, other: &str, secs: u64) -> Seen {
    let file = wait_for(30, "the view.json", || {
        wire::read(state).filter(|f| f.device_id == other)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut seen = Seen::default();
    let start = Instant::now();
    let mut first: Option<Instant> = None;
    loop {
        if first.is_some_and(|t| t.elapsed() >= Duration::from_secs(secs)) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "no picture within 90 s (a prompt would hold it): {seen:?}"
        );
        match Message::read(&mut stream) {
            Ok(Message::Status(status)) => {
                if status.phase == Phase::Showing {
                    seen.control = status.control;
                }
                seen.route = status.route;
            }
            Ok(Message::Config { width, height }) => seen.size = Some((width, height)),
            Ok(Message::Frame { data, .. }) => {
                first.get_or_insert_with(Instant::now);
                seen.frames += 1;
                seen.bytes += data.len() as u64;
            }
            Ok(Message::End { why }) => {
                seen.ended = Some(why);
                break;
            }
            Ok(_) => {}
            Err(e) => panic!("the relay went: {e} ({seen:?})"),
        }
    }
    seen
}

/// `state`'s live sessions from `device_id`.
fn sessions_from(state: &Path, device_id: &str) -> usize {
    std::fs::read_to_string(state.join(LIVE_FILE))
        .ok()
        .and_then(|raw| sessions::parse_mirror(&raw))
        .map_or(0, |mirror| {
            mirror
                .sessions
                .iter()
                .filter(|s| s.device_id.as_deref() == Some(device_id))
                .count()
        })
}

/// Show `other`'s screen on `state`'s computer as the window asks, watch
/// it, and close it; what was seen.
fn view(state: &Path, other: &str, other_state: &Path, me: &str) -> Seen {
    computers::request(
        state,
        ComputerAsk::View {
            device_id: other.to_string(),
            monitor: 0,
            hevc: false,
            window: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    let seen = watch(state, other, 8);
    assert_eq!(sessions_from(other_state, me), 1, "listed there");
    let session_id = wire::read(state).map(|v| v.session_id);
    computers::request(
        state,
        ComputerAsk::StopView {
            session_id,
            lock: false,
            window: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    wait_for(30, "the session gone", || {
        (sessions_from(other_state, me) == 0).then_some(())
    });
    seen
}

/// How many times `log` says this server linked to another computer: the
/// side that dials says it each time the link comes up; the other never.
fn dialed(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .matches("Linked to another computer")
        .count()
}

/// Whether `state` holds no approval waiting for `device_id`.
fn nothing_asked(state: &Path, device_id: &str) -> bool {
    !std::fs::read_to_string(state.join(PENDING_APPROVALS_FILE))
        .unwrap_or_default()
        .contains(device_id)
}

/// Ask `state`'s server to pair with `link` as the app asks, with its
/// owner's yes to pairing both ways or not; the outcome.
fn pair_as(state: &Path, link: &PairLink, yes: bool) -> String {
    let asked = computers::request(
        state,
        ComputerAsk::Pair {
            link: link.to_uri(),
            mine: yes,
        },
        pin::now_unix(),
    )
    .unwrap();
    wait_for(60, "a pairing outcome", || {
        computers::read(state)
            .last_pairing
            .filter(|p| p.request == asked.id)
            .map(|p| p.outcome)
    })
}

/// Pair with the server at `control` (whose state is `state`) as a stranger
/// claiming the id `claimed`, with a fresh certificate of its own and the
/// owner's `code`, as a computer or a phone; whether it said yes.
fn impostor_pairs(state: &Path, control: u16, claimed: &str, code: &str, computer: bool) -> bool {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let certs = pocketdesk_core::streaming::transport::CertManager::new().unwrap();
        let fingerprint = identity::cert_fingerprint(state).unwrap();
        let label =
            pocketdesk_core::streaming::transport::peer_label_for_address("127.0.0.1").unwrap();
        certs.pin_peer(&label, fingerprint).unwrap();
        let manager = pocketdesk_core::protocol::connection::ConnectionManager::new(
            Duration::from_secs(8),
            certs.control_client_config(),
        );
        let me = pocketdesk_core::protocol::kdeconnect::DeviceIdentity::new(claimed, "Impostor");
        let stream = manager.connect("127.0.0.1", control, &me).await.unwrap();
        let (reader, mut writer) = tokio::io::split(stream);
        let ask = pocketdesk_core::protocol::kdeconnect::NetworkPacket::new("kdeconnect.pair")
            .with_body(serde_json::json!({"pin": code, "computer": computer}));
        writer.write_all(&ask.to_wire().unwrap()).await.unwrap();
        writer.flush().await.unwrap();
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let answer = tokio::time::timeout(Duration::from_secs(15), async {
            while let Ok(Some(line)) = lines.next_line().await {
                let packet = pocketdesk_core::protocol::kdeconnect::NetworkPacket::from_wire(&line);
                if let Ok(packet) = packet
                    && packet.packet_type == "kdeconnect.pair"
                    && let Some(paired) = packet.body.get("pair").and_then(|v| v.as_bool())
                {
                    return paired;
                }
            }
            false
        })
        .await;
        answer.unwrap_or(false)
    })
}

/// When the `nth` (from 0) line of `log` holding `needle` was logged: its
/// timestamp (UTC, ISO 8601, so they compare as text), colours dropped.
fn stamp_of(log: &Path, needle: &str, nth: usize) -> Option<String> {
    let raw = std::fs::read_to_string(log).ok()?;
    let mut plain = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
        .lines()
        .filter(|line| line.contains(needle))
        .nth(nth)
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
}

fn log_of(state: &Path) -> PathBuf {
    state.parent().unwrap().join("server.log")
}

/// Made-up device ids, one sorting before the other, so each test knows
/// which of the two keeps the link.
const FIRST: &str = "00000000-0000-4000-8000-0000000000a1";
const LAST: &str = "00000000-0000-4000-8000-0000000000b2";

/// A, whose id sorts first, pastes B's link: A dials the link, as the
/// computer that pastes always did.
#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn two_of_my_computers_pair_once_both_ways_and_each_shows_the_other() {
    both_ways("mine", (19316, 19416), (FIRST, LAST));
}

/// A, whose id sorts last, pastes B's link: B, the computer whose link it
/// was, dials A from what A said of itself as it paired, and A only waits.
#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn the_computer_whose_link_was_pasted_may_be_the_one_that_dials() {
    both_ways("mine-swapped", (19516, 19616), (LAST, FIRST));
}

/// A pastes B's link, first one that asks for both ways without B's code
/// being minted for it, then one that is; `ids` are A's and B's.
fn both_ways(test: &str, (a_control, b_control): (u16, u16), ids: (&str, &str)) {
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders(test, "a", a_control, a_control + 1);
    let (b_config, b_state) = folders(test, "b", b_control, b_control + 1);
    std::fs::write(a_state.join("device_id"), ids.0).unwrap();
    std::fs::write(b_state.join("device_id"), ids.1).unwrap();
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        run.servers.push(
            pocketdesk(config, state)
                .env(pocketdesk_core::streaming::test_pattern::VAR, "1")
                .env(
                    pocketdesk_server::computer_view::SOUND_SEAM,
                    state.parent().unwrap().join("view-sound.wav"),
                )
                .stderr(std::fs::File::create(log_of(state)).unwrap())
                .arg("start")
                .spawn()
                .unwrap(),
        );
    }
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
                && identity::read_endpoint(state).is_some())
            .then_some(())
        });
    }
    let a_id = identity::device_id(&a_state).unwrap();
    let b_id = identity::device_id(&b_state).unwrap();
    let trust = |state: &Path| TrustStore::read(state.join(TRUST_FILE)).unwrap();

    // `mine=1` added to a link whose code was not minted for it: one way,
    // as before. B trusts A narrowly, A trusts nothing, B lists nothing.
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let mut tampered = pairing_link(&b_state, b_control, b_control + 1, &code);
    tampered.mine = true;
    assert_eq!(pair(&a_state, &tampered), "paired");
    assert_eq!(
        trust(&b_state).permissions(&a_id),
        DevicePermissions::computer()
    );
    assert!(!trust(&a_state).is_trusted(&b_id));
    assert!(!computers::read(&a_state).computers[0].mine);
    std::thread::sleep(Duration::from_secs(2));
    assert!(computers::read(&b_state).computers.is_empty());
    // Paired one way, A dials, as the computer that pasted always did.
    wait_for(60, "the one-way link", || {
        (computers::read(&a_state).computers.first()?.link == "linked").then_some(())
    });
    std::thread::sleep(Duration::from_secs(1));
    let before = (dialed(&log_of(&a_state)), dialed(&log_of(&b_state)));
    assert_eq!(before.1, 0, "B lists nothing to dial");

    // The owner ticks the box on B's Pair page: a code minted for another
    // of the owner's computers, and its link says so.
    let mine_link = |code: &str| {
        let mut link = pairing_link(&b_state, b_control, b_control + 1, code);
        link.mine = true;
        link
    };
    let code = pin::mint_value();
    pin::write_mine(&b_state, &code, pin::now_unix()).unwrap();
    let link = mine_link(&code);
    assert!(link.to_uri().contains("mine=1"));

    // Review C1: pasted on A without A's owner saying it is their own (a
    // hostile link would look just like this one), it pairs one way: A
    // trusts B with nothing, B keeps A narrow and lists nothing.
    assert_eq!(pair_as(&a_state, &link, false), "paired");
    assert!(!trust(&a_state).is_trusted(&b_id), "no trust without a yes");
    assert!(!computers::read(&a_state).computers[0].mine);
    assert_eq!(
        trust(&b_state).permissions(&a_id),
        DevicePermissions::computer()
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(computers::read(&b_state).computers.is_empty());

    // Paired one way again, A dials again: counted from here.
    wait_for(60, "the one-way link again", || {
        (computers::read(&a_state).computers.first()?.link == "linked").then_some(())
    });
    std::thread::sleep(Duration::from_secs(1));
    let before = (dialed(&log_of(&a_state)), dialed(&log_of(&b_state)));

    // With A's owner's yes, both ways.
    let code = pin::mint_value();
    pin::write_mine(&b_state, &code, pin::now_unix()).unwrap();
    let link = mine_link(&code);
    assert_eq!(pair_as(&a_state, &link, true), "paired");

    // A trusts B at once, by its own owner's yes; B's grant waits for the
    // link between them (review M1).
    let a_trust = trust(&a_state);
    assert!(a_trust.is_trusted(&b_id) && a_trust.is_computer(&b_id));
    assert_eq!(
        a_trust.permissions(&b_id),
        DevicePermissions::computer_mine()
    );
    // A holds B's keys: the certificate it pinned and B's iroh key.
    let b_endpoint = identity::read_endpoint(&b_state).unwrap()["endpointId"]
        .as_str()
        .unwrap()
        .to_string();
    let b_tls = identity::fingerprint_hex(identity::cert_fingerprint(&b_state).unwrap());
    assert_eq!(
        device_keys::check(&a_state, &b_id, Slot::Tls, Some(&b_tls)),
        Verdict::Matches
    );
    assert_eq!(
        device_keys::check(&a_state, &b_id, Slot::Iroh, Some(&b_endpoint)),
        Verdict::Matches
    );

    // Each lists the other, paired both ways.
    let a_lists = computers::read(&a_state).computers;
    assert_eq!(a_lists.len(), 1);
    assert!(a_lists[0].device_id == b_id && a_lists[0].mine);
    let b_lists = wait_for(15, "B to list A", || {
        let listed = computers::read(&b_state).computers;
        (listed.len() == 1).then_some(listed)
    });
    assert!(b_lists[0].device_id == a_id && b_lists[0].mine);
    let a_tls = identity::fingerprint_hex(identity::cert_fingerprint(&a_state).unwrap());
    assert_eq!(b_lists[0].fingerprint, a_tls, "by the certificate A proved");
    assert_eq!(b_lists[0].stream_port, a_control + 1);

    // One link between them: both say linked, and only the one whose id
    // sorts first dialed it.
    for state in [&a_state, &b_state] {
        wait_for(60, "the link", || {
            (computers::read(state).computers.first()?.link == "linked").then_some(())
        });
    }
    // The link is up: B gives A what was granted.
    wait_for(15, "B's grant", || {
        let given = trust(&b_state).permissions(&a_id) == DevicePermissions::computer_mine();
        (given && computers::read(&b_state).pending.is_empty()).then_some(())
    });
    // Re-review I-2: B gave its grant only once the new link was up, never
    // on A's pairing connection or a link from before: the link's dial is
    // logged (by whichever of the two dials it) no later than B's grant.
    let (dialer, dialed_before) = if a_id < b_id {
        (&a_state, before.0)
    } else {
        (&b_state, before.1)
    };
    let linked_at = stamp_of(&log_of(dialer), "Linked to another computer", dialed_before)
        .expect("the new link's dial is logged");
    let granted_at =
        stamp_of(&log_of(&b_state), "paired both ways now", 0).expect("B logs its grant");
    assert!(
        linked_at <= granted_at,
        "B's grant ({granted_at}) came before the link ({linked_at})"
    );
    std::thread::sleep(Duration::from_secs(5));
    // Since pairing both ways (A's one-way link ended with it).
    let (a_dialed, b_dialed) = (
        dialed(&log_of(&a_state)) - before.0,
        dialed(&log_of(&b_state)) - before.1,
    );
    println!(
        "links dialed since pairing both ways: A {a_dialed}, B {b_dialed} (A's id sorts first: {})",
        a_id < b_id
    );
    if a_id < b_id {
        assert!(a_dialed >= 1 && b_dialed == 0, "A dials, B waits");
    } else {
        assert!(b_dialed >= 1 && a_dialed == 0, "B dials, A waits");
    }

    // Each shows the other's screen, with no prompt on either: a prompt
    // would hold the picture until it was answered, and nobody answers.
    let on_a = view(&a_state, &b_id, &b_state, &a_id);
    println!(
        "B on A: {} frames ({} KiB) at {:?}, route {:?}, control {:?}",
        on_a.frames,
        on_a.bytes / 1024,
        on_a.size,
        on_a.route,
        on_a.control
    );
    assert_eq!(on_a.ended, None);
    assert!(on_a.frames >= 10, "{on_a:?}");
    assert_eq!(on_a.control, Some(true), "B lets A control it, unasked");
    let on_b = view(&b_state, &a_id, &a_state, &b_id);
    println!(
        "A on B: {} frames ({} KiB) at {:?}, route {:?}, control {:?}",
        on_b.frames,
        on_b.bytes / 1024,
        on_b.size,
        on_b.route,
        on_b.control
    );
    assert_eq!(on_b.ended, None);
    assert!(on_b.frames >= 10, "{on_b:?}");
    assert_eq!(on_b.control, Some(true), "A lets B control it, unasked");
    assert!(nothing_asked(&a_state, &b_id) && nothing_asked(&b_state, &a_id));

    // Review C1: a link reusing B's id with another certificate is refused
    // on A, and takes nothing of B's place.
    let mut impostor = mine_link(&pin::mint_value());
    impostor.fingerprint = Some("cd".repeat(32));
    let outcome = pair_as(&a_state, &impostor, true);
    assert!(outcome.contains("another certificate"), "{outcome}");
    assert_eq!(computers::read(&a_state).computers[0].fingerprint, b_tls);
    assert_eq!(
        device_keys::check(&a_state, &b_id, Slot::Tls, Some(&b_tls)),
        Verdict::Matches,
        "B's keys stand"
    );

    // Re-review I-1: someone with one of B's codes pairs with B under A's
    // id and another certificate, as a computer and as a phone: refused
    // both times, before any key is touched; A keeps its keys on B.
    let a_endpoint = identity::read_endpoint(&a_state).unwrap()["endpointId"]
        .as_str()
        .unwrap()
        .to_string();
    for computer in [true, false] {
        let code = pin::mint_value();
        pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
        assert!(
            !impostor_pairs(&b_state, b_control, &a_id, &code, computer),
            "refused as a computer: {computer}"
        );
    }
    assert_eq!(
        device_keys::check(&b_state, &a_id, Slot::Tls, Some(&a_tls)),
        Verdict::Matches,
        "A's certificate stands on B"
    );
    assert_eq!(
        device_keys::check(&b_state, &a_id, Slot::Iroh, Some(&a_endpoint)),
        Verdict::Matches,
        "A's iroh key stands on B"
    );
    assert_eq!(
        trust(&b_state).permissions(&a_id),
        DevicePermissions::computer_mine()
    );

    // Review M7: unpaired on B's Devices page, B forgets A too.
    TrustStore::update(b_state.join(TRUST_FILE), |store| {
        store.untrust_device(&a_id)
    })
    .unwrap();
    wait_for(30, "A forgotten on B", || {
        computers::read(&b_state).computers.is_empty().then_some(())
    });

    // Unpaired on A: both halves go there.
    computers::request(
        &a_state,
        ComputerAsk::Forget {
            device_id: b_id.clone(),
        },
        pin::now_unix(),
    )
    .unwrap();
    wait_for(30, "B forgotten on A", || {
        computers::read(&a_state).computers.is_empty().then_some(())
    });
    assert!(!trust(&a_state).is_trusted(&b_id), "nor trusted there");
    for state in [&a_state, &b_state] {
        assert!(
            !state.parent().unwrap().join("view-sound.wav").exists(),
            "no sound came"
        );
    }

    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let _ = pocketdesk(config, state).arg("stop").status();
    }
    for server in &mut run.servers {
        let _ = server.wait();
    }
}
