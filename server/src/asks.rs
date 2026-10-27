//! Questions for the owner's phones (BF3). `pocketdesk ask` leaves one in
//! `asks_out/` and waits; [`run`] puts it to every connected phone with the
//! Ask permission, and again on each link a phone makes while it waits; the first
//! answer from a phone it was put to is kept in `asks_in/` for
//! `pocketdesk ask`, and a later one is dropped. The question's words are
//! never logged.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::error::Result;
use pocketdesk_core::local::asks::{self, AskRequest};
use pocketdesk_core::protocol::ask::{ASK, ASK_ANSWER, Answer, answer_of, ask_body};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin, TrustStore};
use pocketdesk_core::protocol::permissions::Capability;

/// How often `pocketdesk ask` looks for its answer.
const POLL: Duration = Duration::from_millis(250);

/// A question put to the phones, and which it was put to.
struct Pending {
    request: AskRequest,
    /// The phones it was put to, which may answer it.
    asked: HashSet<String>,
    /// The connections it was sent on: a phone that links again is asked
    /// again on its new one (BF3 review: a write into a link already dead
    /// had counted as asked, and it was never put to the phone).
    sent_on: HashSet<String>,
    /// The connections a send failed on, said once.
    failed_on: HashSet<String>,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// `pocketdesk ask`: put `question` to the phones and wait up to
/// `wait_secs` for an answer. Exits 0 for Yes, 1 for No, 2 for no answer in
/// time (or bad arguments, or no phone that may answer), 3 when the
/// question could not be left for the server.
pub fn run_cli(question: &str, wait_secs: u64) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("pocketdesk ask: {e:#}");
            return 3;
        }
    };
    // Said at once rather than after the whole wait (BF3 follow-up): Ask is
    // off for every phone until the owner turns it on.
    let store = crate::state::trust_store_path()
        .ok()
        .and_then(|path| TrustStore::read(path).ok());
    if !anyone_may_answer(store.as_ref()) {
        eprintln!(
            "pocketdesk ask: no paired phone may answer. Turn on \"Answer my questions\" for your phone in Pocketdesk (Devices)."
        );
        return 2;
    }
    let asked = match asks::request(&dir, question, wait_secs, now_unix()) {
        Ok(asked) => asked,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            eprintln!("pocketdesk ask: {e}");
            return 2;
        }
        Err(e) => {
            eprintln!("pocketdesk ask: the question could not be left for the server: {e}");
            return 3;
        }
    };
    eprintln!("Waiting up to {wait_secs} s for an answer on your phone…");
    let deadline = std::time::Instant::now() + Duration::from_secs(wait_secs);
    match wait_for_answer(&dir, &asked, deadline, std::thread::sleep) {
        Some(Answer::Yes) => {
            println!("yes");
            0
        }
        Some(Answer::No) => {
            println!("no");
            1
        }
        None => {
            eprintln!("No answer in time.");
            2
        }
    }
}

/// Whether any paired device may answer: has the Ask permission. A store
/// that does not read is not taken as "none" (the server decides).
fn anyone_may_answer(store: Option<&TrustStore>) -> bool {
    store.is_none_or(|store| {
        store
            .trusted_devices()
            .iter()
            .any(|device| store.permissions(device).allows(Capability::Ask))
    })
}

/// The answer to `asked`, looked for until `deadline` (on the monotonic
/// clock, so a clock set back does not stretch the wait); `None` (the
/// question withdrawn) when none came.
fn wait_for_answer(
    dir: &Path,
    asked: &AskRequest,
    deadline: std::time::Instant,
    sleep: impl Fn(Duration),
) -> Option<Answer> {
    loop {
        if let Some(answer) = asks::take_answer(dir, &asked.id) {
            return Some(answer.answer);
        }
        if std::time::Instant::now() >= deadline {
            asks::withdraw(dir, &asked.id);
            return None;
        }
        sleep(POLL);
    }
}

/// Put the questions left in `asks_out/` to the phones, for the server's
/// life.
pub async fn run() {
    let Ok(dir) = crate::state::state_dir() else {
        return;
    };
    loop {
        let waiting = !PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        if waiting {
            tokio::time::sleep(Duration::from_millis(500)).await;
        } else {
            crate::inbox::wait(
                pocketdesk_core::local::paths::ASKS_DIR,
                Duration::from_millis(500),
            )
            .await;
        }
        let now = now_unix();
        let taken = asks::take_all(&dir, now);
        let clients = crate::state::clone_clients().await;
        let mut sends = Vec::new();
        {
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            for request in taken {
                pending.insert(
                    request.id.clone(),
                    Pending {
                        request,
                        asked: HashSet::new(),
                        sent_on: HashSet::new(),
                        failed_on: HashSet::new(),
                    },
                );
            }
            pending.retain(|_, p| now < p.request.expires_unix);
            for (id, p) in pending.iter() {
                for client in &clients {
                    let device = client.device_id().to_string();
                    if !p.sent_on.contains(client.connection_id())
                        && crate::plugins::pair::allows(&device, Capability::Ask)
                    {
                        sends.push((id.clone(), client.clone(), p.request.clone()));
                    }
                }
            }
        }
        for (id, client, request) in sends {
            let packet = NetworkPacket::new(ASK).with_body(ask_body(
                &request.id,
                &request.question,
                request.expires_unix,
                now_unix(),
            ));
            let sent = client.send_packet(&packet).await;
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            let Some(p) = pending.get_mut(&id) else {
                continue;
            };
            match sent {
                Ok(()) => {
                    tracing::info!("A question was put to a phone");
                    p.asked.insert(client.device_id().to_string());
                    p.sent_on.insert(client.connection_id().to_string());
                }
                // Tried again each turn while it waits, said once.
                Err(e) if p.failed_on.insert(client.connection_id().to_string()) => {
                    tracing::warn!("A question was not put to a phone: {e}");
                }
                Err(_) => {}
            }
        }
    }
}

/// Keep `device`'s answer to question `id` for `pocketdesk ask` if the
/// question was put to that phone and still waits; the first answer ends
/// it. Whether it was kept.
fn heard(dir: &Path, device: &str, id: &str, answer: Answer, now: u64) -> bool {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let open = pending
        .get(id)
        .is_some_and(|p| p.asked.contains(device) && now < p.request.expires_unix);
    if !open {
        return false;
    }
    pending.remove(id);
    match asks::answer(dir, id, answer, device) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("An answer could not be kept: {e}");
            false
        }
    }
}

/// The plugin hearing the phones' answers.
pub struct AskPlugin;

#[async_trait::async_trait]
impl Plugin for AskPlugin {
    fn name(&self) -> &'static str {
        "ask"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[ASK_ANSWER]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[ASK]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != ASK_ANSWER {
            return Ok(());
        }
        let Some((id, answer)) = answer_of(&packet.body) else {
            return Ok(());
        };
        let Ok(dir) = crate::state::state_dir() else {
            return Ok(());
        };
        if heard(&dir, sender.device_id(), &id, answer, now_unix()) {
            tracing::info!("A phone answered a question: {}", answer.word());
        } else {
            tracing::info!("An answer was dropped: not a question put to that phone and waiting");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-asks-srv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(request: &AskRequest, to: &[&str]) {
        PENDING.lock().unwrap().insert(
            request.id.clone(),
            Pending {
                request: request.clone(),
                asked: to.iter().map(|d| d.to_string()).collect(),
                sent_on: HashSet::new(),
                failed_on: HashSet::new(),
            },
        );
    }

    #[test]
    fn the_cli_says_at_once_when_no_paired_phone_may_answer() {
        let dir = dir("anyone");
        let path = dir.join("trusted_devices.json");
        let mut store = TrustStore::load_or_create(&path).unwrap();
        assert!(!anyone_may_answer(Some(&store)), "nobody paired");
        store.trust_device("phone-a").unwrap();
        assert!(!anyone_may_answer(Some(&store)), "paired, Ask off");
        let mut allowed = store.permissions("phone-a");
        allowed.set(Capability::Ask, true);
        store.set_permissions("phone-a", allowed).unwrap();
        assert!(anyone_may_answer(Some(&store)));
        assert!(anyone_may_answer(None), "an unread store is not none");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_answer_from_a_phone_asked_is_kept_and_ends_the_question() {
        let dir = dir("first");
        let now = now_unix();
        let asked = asks::request(&dir, "Deploy?", 60, now).unwrap();
        asks::take_all(&dir, now);
        put(&asked, &["phone-a", "phone-b"]);
        assert!(heard(&dir, "phone-b", &asked.id, Answer::No, now));
        assert!(
            !heard(&dir, "phone-a", &asked.id, Answer::Yes, now),
            "already answered"
        );
        assert_eq!(
            asks::take_answer(&dir, &asked.id).unwrap().answer,
            Answer::No
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_phone_not_asked_or_an_answer_too_late_is_dropped() {
        let dir = dir("dropped");
        let now = now_unix();
        let asked = asks::request(&dir, "Deploy?", 60, now).unwrap();
        put(&asked, &["phone-a"]);
        assert!(
            !heard(&dir, "phone-c", &asked.id, Answer::Yes, now),
            "not asked"
        );
        assert!(
            !heard(&dir, "phone-a", &asked.id, Answer::Yes, now + 60),
            "too late"
        );
        assert!(!heard(
            &dir,
            "phone-a",
            "no-such-question",
            Answer::Yes,
            now
        ));
        assert_eq!(asks::take_answer(&dir, &asked.id), None);
        PENDING.lock().unwrap().remove(&asked.id);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cli_hears_the_answer_or_gives_up_and_withdraws() {
        let dir = dir("cli");
        let now = now_unix();
        let asked = asks::request(&dir, "Deploy?", 1, now).unwrap();
        asks::take_all(&dir, now); // the server took it
        asks::answer(&dir, &asked.id, Answer::Yes, "phone-a").unwrap();
        let soon = std::time::Instant::now() + Duration::from_secs(1);
        assert_eq!(
            wait_for_answer(&dir, &asked, soon, |_| {}),
            Some(Answer::Yes)
        );
        // No answer: it waits out its time, then takes the question back.
        let late = asks::request(&dir, "Deploy?", 1, now).unwrap();
        let slept = std::cell::Cell::new(0);
        let soon = std::time::Instant::now() + Duration::from_secs(1);
        let gave_up = wait_for_answer(&dir, &late, soon, |d| {
            slept.set(slept.get() + 1);
            std::thread::sleep(d);
        });
        assert_eq!(gave_up, None);
        assert!(slept.get() >= 1);
        assert!(asks::take_all(&dir, now).is_empty(), "withdrawn");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
