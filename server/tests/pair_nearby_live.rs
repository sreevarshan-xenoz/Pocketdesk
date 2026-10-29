//! Pair a computer nearby, between two real servers side by side (R6a's
//! seams: each with its own config and state folders and ports). A asks B
//! as the app asks for a computer it found (the address and port its
//! announcement gave; discovery itself is mDNS on the real network, so
//! the address is given here), and both show the same matching code. A
//! decline pairs nothing. Asked again and accepted, with both owners
//! saying both are theirs, the two pair both ways from that one ask: A
//! pairs as from a pasted link, and each then lists the other.
//!
//! No notification is shown (`POCKETDESK_NEARBY_QUIET`), nothing is
//! injected and no sound is made. Each server's log goes to its throwaway
//! folder and is never printed.
//!
//! Ignored: it starts two real servers, which reach iroh's relays. Run it
//! with
//! `cargo test -p pocketdesk-server --test pair_nearby_live -- --ignored --nocapture --test-threads=1`.

// Pairing from a link is the other tests' (this one pairs from an ask).
#[allow(dead_code)]
mod common;

use std::path::Path;

use common::{Cleanup, folders, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::nearby::AskState;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::{identity, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;

const A_ID: &str = "00000000-0000-4000-8000-0000000000a7";
const B_ID: &str = "00000000-0000-4000-8000-0000000000b7";

/// Ask `a_state`'s server to pair with B nearby, as the app does; the
/// request's id.
fn ask(a_state: &Path, b_control: u16, mine: bool) -> String {
    computers::request(
        a_state,
        ComputerAsk::PairNearby {
            device_id: B_ID.to_string(),
            name: "Computer B".to_string(),
            address: "127.0.0.1".to_string(),
            port: b_control,
            mine,
        },
        pin::now_unix(),
    )
    .unwrap()
    .id
}

/// The code A shows for its ask `request`, and B's ask on screen (its id
/// and code), once both show one.
fn codes(a_state: &Path, b_state: &Path, request: &str) -> (String, String, String) {
    wait_for(30, "both codes", || {
        let out = computers::read(a_state)
            .nearby_out
            .filter(|o| o.request == request && o.state == AskState::Waiting)?;
        let shown = computers::read(b_state).nearby_in?;
        Some((out.code?, shown.id, shown.code))
    })
}

fn answer(b_state: &Path, id: &str, accept: bool, mine: bool) {
    computers::request(
        b_state,
        ComputerAsk::AnswerNearby {
            nearby: id.to_string(),
            accept,
            mine,
        },
        pin::now_unix(),
    )
    .unwrap();
}

#[test]
#[ignore = "starts two real servers, which reach iroh's relays"]
fn two_computers_nearby_pair_with_a_matching_code() {
    let (a_control, b_control) = (19516u16, 19616u16);
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("nearby", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("nearby", "b", b_control, b_control + 1);
    std::fs::write(a_state.join("device_id"), A_ID).unwrap();
    std::fs::write(b_state.join("device_id"), B_ID).unwrap();
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let log = state.parent().unwrap().join("server.log");
        run.servers.push(
            pocketdesk(config, state)
                .env("POCKETDESK_NEARBY_QUIET", "1")
                .env(pocketdesk_core::streaming::test_pattern::VAR, "1")
                .stderr(std::fs::File::create(log).unwrap())
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

    // Asked and declined: the same code on both screens, and nothing paired.
    let first = ask(&a_state, b_control, true);
    let (a_code, id, b_code) = codes(&a_state, &b_state, &first);
    assert_eq!(a_code, b_code, "both screens show the same code");
    assert_eq!(a_code.len(), 6);
    answer(&b_state, &id, false, true);
    wait_for(30, "the decline", || {
        computers::read(&a_state)
            .nearby_out
            .filter(|o| o.request == first && o.state == AskState::Declined)
    });
    assert!(
        computers::read(&b_state).nearby_in.is_none(),
        "B's prompt goes"
    );
    assert!(computers::read(&a_state).computers.is_empty());
    let trust = |state: &Path| TrustStore::read(state.join(TRUST_FILE)).unwrap();
    assert!(!trust(&b_state).is_trusted(A_ID), "a decline pairs nothing");

    // Asked again, accepted with both owners' yes: paired both ways.
    let second = ask(&a_state, b_control, true);
    let (a_code, id, b_code) = codes(&a_state, &b_state, &second);
    assert_eq!(a_code, b_code);
    answer(&b_state, &id, true, true);
    let outcome = wait_for(60, "the pairing's outcome", || {
        computers::read(&a_state)
            .last_pairing
            .filter(|p| p.request == second)
            .map(|p| p.outcome)
    });
    assert_eq!(outcome, "paired");
    let listed = |state: &Path, id: &str| {
        computers::read(state)
            .computers
            .iter()
            .any(|c| c.device_id == id && c.mine)
    };
    assert!(listed(&a_state, B_ID), "A lists B");
    wait_for(30, "B listing A", || listed(&b_state, A_ID).then_some(()));
    assert!(trust(&b_state).is_trusted(A_ID));
    assert!(trust(&a_state).is_trusted(B_ID));
    drop(run);
}
