//! Shared input's channel on the link between two computers: A pairs with
//! B as in `two_computers.rs`, then `pocketdesk share-test` on A sends B a
//! script over their link (the keys on the ordered stream, the moves and
//! pings as datagrams), and B, started with `POCKETDESK_SHARE_TEST`, records
//! what arrives instead of using it. Nothing is injected anywhere.
//!
//! Ignored: it starts two real servers, which reach iroh's relays. Run it
//! with `cargo test -p pocketdesk-server --test shared_input_link -- --ignored`.

mod common;

use std::path::Path;
use std::process::Stdio;

use common::{Cleanup, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers;
use pocketdesk_core::local::{identity, pin};
use pocketdesk_server::shared_input::share_test;
use pocketdesk_server::shared_input::wire::Msg;

/// What B recorded from `from`, in the order it arrived.
fn recorded(record: &Path, from: &str) -> Vec<Msg> {
    let Ok(text) = std::fs::read_to_string(record) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|line| line["from"] == from)
        .map(|line| {
            assert!(line["at_us"].as_u64().is_some(), "an arrival time: {line}");
            serde_json::from_value(line["msg"].clone()).expect("a message")
        })
        .collect()
}

fn is_key(msg: &Msg) -> bool {
    matches!(msg, Msg::Key { .. })
}

#[test]
#[ignore = "starts two real servers, which reach iroh's relays"]
fn messages_arrive_in_order_over_a_paired_link() {
    let (a_control, b_control) = (18936, 19036);
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("share", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("share", "b", b_control, b_control + 1);
    let record = b_state.parent().unwrap().join("share-record.jsonl");
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    run.servers.push(
        pocketdesk(&a_config, &a_state)
            .arg("start")
            .spawn()
            .unwrap(),
    );
    run.servers.push(
        pocketdesk(&b_config, &b_state)
            .env(share_test::RECORD_ENV, &record)
            .arg("start")
            .spawn()
            .unwrap(),
    );
    for (port, state) in [(a_control, &a_state), (b_control, &b_state)] {
        wait_for(60, "the control port and the internet endpoint", || {
            (std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
                && identity::read_endpoint(state).is_some())
            .then_some(())
        });
    }
    let a_id = identity::device_id(&a_state).unwrap();

    // A pairs with B's code, and the link comes up over the internet road.
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = pairing_link(&b_state, b_control, b_control + 1, &code);
    assert_eq!(pair(&a_state, &link), "paired");
    wait_for(60, "the link", || {
        let file = computers::read(&a_state);
        (file.computers.first()?.link == "linked").then_some(())
    });

    // A shares its keyboard and mouse with B, by script.
    let out = pocketdesk(&a_config, &a_state)
        .args([
            "share-test",
            &link.device_id,
            "--keys",
            "200",
            "--moves",
            "50",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "share-test: {said}");
    let printed = String::from_utf8_lossy(&out.stdout);
    assert!(
        printed.contains("round trips") && !printed.contains("none of"),
        "the pings came back: {said}"
    );
    // The round trips, for a run with --nocapture.
    println!("{printed}");

    // B recorded it all, the stream's part in order.
    let got = wait_for(30, "B to record the Leave", || {
        let got = recorded(&record, &a_id);
        got.contains(&Msg::Leave).then_some(got)
    });
    let keys: Vec<&Msg> = got.iter().filter(|m| is_key(m)).collect();
    let script = share_test::script(200, 50);
    let sent_keys: Vec<&Msg> = script.iter().filter(|m| is_key(m)).collect();
    assert_eq!(keys.len(), 200);
    assert_eq!(keys, sent_keys, "every key, in order");
    let enter = got.iter().position(|m| matches!(m, Msg::Enter { .. }));
    let first_key = got.iter().position(is_key);
    let last_key = got.iter().rposition(is_key);
    let leave = got.iter().position(|m| *m == Msg::Leave);
    assert!(
        enter < first_key,
        "Enter before the keys: {enter:?} {first_key:?}"
    );
    assert!(last_key < leave, "Leave after them: {last_key:?} {leave:?}");
    // The stream opens with the Hello (datagrams ride beside it, unordered).
    let on_stream: Vec<&Msg> = got
        .iter()
        .filter(|m| !matches!(m, Msg::Move { .. } | Msg::Ping { .. } | Msg::Active { .. }))
        .collect();
    assert!(
        matches!(on_stream.first(), Some(Msg::Hello { .. })),
        "{on_stream:?}"
    );

    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        let _ = pocketdesk(config, state).arg("stop").status();
    }
    for server in &mut run.servers {
        let _ = server.wait();
    }
}
