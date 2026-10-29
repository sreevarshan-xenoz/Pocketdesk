//! The clipboard and files follow the window that shows another computer's
//! screen (PC3), between two real servers side by side, as BK7's
//! `computer_view_live` runs them: A pairs with B, B lets A in without
//! asking and takes its input (Control on), and A shows B's screen. A
//! stand-in for A's window reads A's view relay and says its focus as the
//! window would; it sends no input at all, so nothing is injected anywhere.
//! Each server's clipboard is a file (`POCKETDESK_CLIPBOARD_FILE`) and its
//! Downloads a folder (`POCKETDESK_DOWNLOAD_DIR`), so neither touches this
//! computer's own; B's screen is the test pattern. With "Clipboard with my
//! computers" off on both, so only the window moves the clipboard:
//!
//! - the focus coming takes A's copy to B; going, brings B's back to A;
//! - the focus coming again with nothing new copied on A sends B nothing
//!   back (no echo);
//! - a file dropped on the window (queued as the window queues it) lands in
//!   B's Downloads whole, its SHA-256 the same;
//! - with Files off for A on B, the send is refused at once, by name;
//! - with `computer_view_clipboard` off on B, a focus moves nothing.
//!
//! Ignored: it starts two real servers with a real encoder, which reach
//! iroh's relays. Run it with
//! `cargo test -p pocketdesk-server --test view_follow_live -- --ignored --nocapture`.

mod common;

use std::io::Write;
use std::net::{Ipv4Addr, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{Cleanup, clipboard, downloads, folders, pair, pairing_link, pocketdesk, wait_for};
use pocketdesk_core::local::computers::{self, ComputerAsk};
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::sent::{self, SentState};
use pocketdesk_core::local::view::{self as wire, Message, Phase};
use pocketdesk_core::local::{identity, outbox, pause, pin};
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;
use sha2::{Digest, Sha256};

/// What the stand-in window has heard: whether B let A in and takes its
/// input, frames counted, and how the view ended.
#[derive(Debug, Default)]
struct Heard {
    showing: bool,
    control: Option<bool>,
    frames: u64,
    ended: Option<String>,
}

/// The stand-in window: joined to A's relay, read on a thread of its own
/// (so the relay never finds it slow), its writing half kept for focus.
fn join(a_state: &Path, b_id: &str) -> (TcpStream, Arc<Mutex<Heard>>) {
    let file = wait_for(30, "A's view.json for B", || {
        wire::read(a_state).filter(|f| f.device_id == b_id)
    });
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, file.port)).unwrap();
    stream.write_all(&file.token_bytes().unwrap()).unwrap();
    let heard = Arc::new(Mutex::new(Heard::default()));
    let mut reader = stream.try_clone().unwrap();
    let kept = heard.clone();
    std::thread::spawn(move || {
        loop {
            let message = Message::read(&mut reader);
            let mut heard = kept.lock().unwrap();
            match message {
                Ok(Message::Status(status)) => {
                    heard.showing = status.phase == Phase::Showing;
                    heard.control = status.control;
                }
                Ok(Message::Frame { .. }) => heard.frames += 1,
                Ok(Message::End { why }) => {
                    heard.ended = Some(why);
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    heard
                        .ended
                        .get_or_insert_with(|| format!("the relay went: {e}"));
                    return;
                }
            }
        }
    });
    (stream, heard)
}

fn say_focus(window: &mut TcpStream, focused: bool) {
    Message::Focus(focused).write(window).unwrap();
}

fn holds(state: &Path) -> String {
    std::fs::read_to_string(clipboard(state)).unwrap_or_default()
}

/// Wait for `state`'s clipboard to hold `text`.
fn comes(state: &Path, text: &str, what: &str) {
    wait_for(20, what, || (holds(state) == text).then_some(()));
}

/// `state`'s clipboard still holds `text` after `secs`.
fn stays(state: &Path, text: &str, secs: u64, what: &str) {
    std::thread::sleep(Duration::from_secs(secs));
    assert_eq!(holds(state), text, "{what}");
}

/// Queue `file` for B as the window queues a drop, and wait for its row.
fn drop_on_window(a_state: &Path, b_id: &str, file: &Path) -> SentState {
    let asked = outbox::request(a_state, b_id, file.to_str().unwrap(), pin::now_unix()).unwrap();
    wait_for(90, "the drop's send to end", || {
        sent::read(a_state)
            .into_iter()
            .find(|f| f.id == asked.id && f.state != SentState::Offered)
            .map(|f| f.state)
    })
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Turn `key` to `value` in a throwaway's config.toml (the servers read it
/// again as it changes).
fn set_config(config: &Path, key: &str, value: bool) {
    let path = config.join("config.toml");
    let before = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = before
        .lines()
        .filter(|l| !l.starts_with(&format!("{key} =")))
        .map(str::to_string)
        .collect();
    lines.push(format!("{key} = {value}"));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
}

#[test]
#[ignore = "starts two real servers and a real encoder, which reach iroh's relays"]
fn the_clipboard_and_files_follow_the_window() {
    let (a_control, b_control) = (19136, 19236);
    for port in [a_control, b_control] {
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "something already listens on {port}"
        );
    }
    let (a_config, a_state) = folders("follow", "a", a_control, a_control + 1);
    let (b_config, b_state) = folders("follow", "b", b_control, b_control + 1);
    let mut run = Cleanup {
        servers: Vec::new(),
        folders: vec![
            a_config.parent().unwrap().to_path_buf(),
            b_config.parent().unwrap().to_path_buf(),
        ],
    };
    // Only the window moves the clipboard: the always-on sharing off.
    for config in [&a_config, &b_config] {
        set_config(config, "computer_clipboard", false);
    }
    for state in [&a_state, &b_state] {
        std::fs::create_dir_all(downloads(state)).unwrap();
        std::fs::write(clipboard(state), "").unwrap();
        // Paused, so a file arriving is listed, not announced with a toast.
        pause::pause(state, None, pin::now_unix()).unwrap();
    }
    for (config, state) in [(&a_config, &a_state), (&b_config, &b_state)] {
        run.servers.push(
            pocketdesk(config, state)
                .env(pocketdesk_core::streaming::test_pattern::VAR, "1")
                // No sound is sent (B's entry for A has Audio off); were
                // any to come, A writes it here, never to the speakers.
                .env(
                    pocketdesk_server::computer_view::SOUND_SEAM,
                    state.parent().unwrap().join("view-sound.wav"),
                )
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
    let code = pin::mint_value();
    pin::write(&b_state, &code, pin::now_unix(), None).unwrap();
    let link = pairing_link(&b_state, b_control, b_control + 1, &code);
    assert_eq!(pair(&a_state, &link), "paired");
    // B lets A in without asking, and takes its input: the window
    // controls. The stand-in sends none.
    let trust_path = b_state.join(TRUST_FILE);
    let mut trust = TrustStore::read(&trust_path).unwrap();
    let mut permissions = trust.permissions(&a_id);
    assert!(permissions.allows(Capability::Clipboard));
    assert!(permissions.allows(Capability::Files));
    assert!(!permissions.allows(Capability::Audio), "no sound is sent");
    permissions.ask_first = false;
    permissions.set(Capability::Input, true);
    trust.set_permissions(&a_id, permissions).unwrap();
    wait_for(60, "A's link to B", || {
        (computers::read(&a_state).computers.first()?.link == "linked").then_some(())
    });

    computers::request(
        &a_state,
        ComputerAsk::View {
            device_id: b_id.clone(),
            monitor: 0,
        },
        pin::now_unix(),
    )
    .unwrap();
    let (mut window, heard) = join(&a_state, &b_id);
    wait_for(60, "B's screen, controlled", || {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.ended, None);
        (heard.showing && heard.control == Some(true) && heard.frames > 0).then_some(())
    });

    // The focus comes: A's copy goes to B.
    std::fs::write(clipboard(&a_state), "pc3 copied on A").unwrap();
    std::fs::write(clipboard(&b_state), "pc3 B's own").unwrap();
    say_focus(&mut window, true);
    comes(&b_state, "pc3 copied on A", "A's copy on B");
    println!("focus in: A's copy reached B");

    // The focus goes: B's copy comes back to A.
    std::fs::write(clipboard(&b_state), "pc3 copied on B").unwrap();
    say_focus(&mut window, false);
    comes(&a_state, "pc3 copied on B", "B's copy back on A");
    println!("focus out: B's copy came back to A");

    // In again with nothing new copied on A: what A holds came from B, so
    // nothing goes back to B over what B copied since.
    std::fs::write(clipboard(&b_state), "pc3 copied on B later").unwrap();
    say_focus(&mut window, true);
    stays(&b_state, "pc3 copied on B later", 4, "no echo back to B");
    say_focus(&mut window, false);
    comes(
        &a_state,
        "pc3 copied on B later",
        "B's later copy back on A",
    );
    println!("no echo: B kept its later copy, and it came back to A");

    // A file dropped on the window lands in B's Downloads whole.
    let dropped = a_state.parent().unwrap().join("pc3-dropped.bin");
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write(&dropped, &bytes).unwrap();
    assert_eq!(drop_on_window(&a_state, &b_id, &dropped), SentState::Sent);
    let landed = std::fs::read(downloads(&b_state).join("pc3-dropped.bin")).unwrap();
    assert_eq!(landed.len(), bytes.len());
    assert_eq!(sha256(&landed), sha256(&bytes), "the same bytes");
    println!(
        "drop: {} bytes landed in B's Downloads, SHA-256 {}",
        landed.len(),
        &sha256(&landed)[..16]
    );

    // Files off for A on B: refused at once, by name.
    let mut trust = TrustStore::read(&trust_path).unwrap();
    let mut permissions = trust.permissions(&a_id);
    permissions.set(Capability::Files, false);
    trust.set_permissions(&a_id, permissions).unwrap();
    let refused = a_state.parent().unwrap().join("pc3-refused.bin");
    std::fs::write(&refused, b"not wanted there").unwrap();
    let started = std::time::Instant::now();
    match drop_on_window(&a_state, &b_id, &refused) {
        SentState::Failed(why) => {
            assert!(
                why.contains("Files is turned off for that computer"),
                "{why}"
            );
            println!("files off: refused in {:?}: {why}", started.elapsed());
        }
        other => panic!("not refused: {other:?}"),
    }
    assert!(!downloads(&b_state).join("pc3-refused.bin").exists());

    // The setting off on B: a focus moves nothing.
    set_config(&b_config, "computer_view_clipboard", false);
    std::thread::sleep(Duration::from_secs(2));
    std::fs::write(clipboard(&a_state), "pc3 not for B").unwrap();
    std::fs::write(clipboard(&b_state), "pc3 B with it off").unwrap();
    say_focus(&mut window, true);
    stays(
        &b_state,
        "pc3 B with it off",
        4,
        "B takes nothing with it off",
    );
    say_focus(&mut window, false);
    stays(&a_state, "pc3 not for B", 4, "nor answers with it off");
    println!("setting off on B: nothing moved");

    let frames = heard.lock().unwrap().frames;
    println!("{frames} frames came while the window followed");
    assert_eq!(heard.lock().unwrap().ended, None);
    let session_id = wire::read(&a_state).map(|v| v.session_id);
    computers::request(
        &a_state,
        ComputerAsk::StopView { session_id },
        pin::now_unix(),
    )
    .unwrap();
    assert!(
        !a_state.parent().unwrap().join("view-sound.wav").exists(),
        "no sound came to A"
    );
    drop(run);
}
