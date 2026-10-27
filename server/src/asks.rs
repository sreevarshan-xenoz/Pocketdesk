//! Questions for the owner's phones (BF3). `pocketdesk ask` leaves one in
//! `asks_out/` and waits; [`run`] puts it to every connected phone with the
//! Ask permission, and to one that connects while it waits; the first
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
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;

/// How often `pocketdesk ask` looks for its answer.
const POLL: Duration = Duration::from_millis(250);

/// A question put to the phones, and which it was put to.
struct Pending {
    request: AskRequest,
    asked: HashSet<String>,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

/// `pocketdesk ask`: put `question` to the phones and wait up to
/// `wait_secs` for an answer. Exits 0 for Yes, 1 for No, 2 for no answer in
/// time (or bad arguments), 3 when the question could not be left for the
/// server.
pub fn run_cli(question: &str, wait_secs: u64) -> i32 {
    let dir = match crate::state::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("pocketdesk ask: {e:#}");
            return 3;
        }
    };
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
    match wait_for_answer(&dir, &asked, std::thread::sleep) {
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

/// The answer to `asked`, looked for until it expires; `None` (the
/// question withdrawn) when none came.
fn wait_for_answer(dir: &Path, asked: &AskRequest, sleep: impl Fn(Duration)) -> Option<Answer> {
    loop {
        if let Some(answer) = asks::take_answer(dir, &asked.id) {
            return Some(answer.answer);
        }
        if now_unix() >= asked.expires_unix {
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
                    },
                );
            }
            pending.retain(|_, p| now < p.request.expires_unix);
            for (id, p) in pending.iter() {
                for client in &clients {
                    let device = client.device_id().to_string();
                    if !p.asked.contains(&device)
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
            ));
            match client.send_packet(&packet).await {
                Ok(()) => {
                    tracing::info!("A question was put to a phone");
                    if let Some(p) = PENDING
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_mut(&id)
                    {
                        p.asked.insert(client.device_id().to_string());
                    }
                }
                Err(e) => tracing::warn!("A question was not put to a phone: {e}"),
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
            },
        );
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
        assert_eq!(wait_for_answer(&dir, &asked, |_| {}), Some(Answer::Yes));
        // No answer: it waits out its time, then takes the question back.
        let late = asks::request(&dir, "Deploy?", 1, now).unwrap();
        let slept = std::cell::Cell::new(0);
        let gave_up = wait_for_answer(&dir, &late, |d| {
            slept.set(slept.get() + 1);
            std::thread::sleep(d);
        });
        assert_eq!(gave_up, None);
        assert!(slept.get() >= 1);
        assert!(asks::take_all(&dir, now).is_empty(), "withdrawn");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
