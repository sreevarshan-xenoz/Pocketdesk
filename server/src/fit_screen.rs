//! "Fit the screen to this window" (item 8 of the plan after 0.8.3), the
//! asking computer's side: a window onto another computer's screen asks that
//! computer, with `kdeconnect.pocketdesk.virtual_screen`'s `fit`, for a
//! headless screen of the window's size ([`crate::virtual_screen`] makes it
//! there, on Hyprland or Sway) and shows that in place of a real screen. The
//! ask goes on the connection or the link this computer keeps to the other
//! ([`crate::clipboard_push::send_to_computer`]), and its answer comes back
//! on the same road, to [`heard`].
//!
//! Each window has one screen there (`slot` is the window's number), asked
//! for as the view starts and again, with the new size, as the window is
//! resized; [`release`] takes it away as the view ends. A computer before
//! this ignores the ask (it never answers on its link) or answers "unknown
//! action" on a connection, and the view then shows the screen asked for.
//! The screen is an empty extra desktop: nothing is moved onto it.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pocketdesk_core::local::computers::FitSize;
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use tokio::sync::oneshot;

use crate::virtual_screen::{PACKET, is_request};

/// How long an ask waits for its answer.
pub const ANSWER_WAIT: Duration = Duration::from_secs(8);

/// How long an ask waits for the link to the other computer to come up (a
/// view asked for as the app starts may find none yet), and how often it
/// looks.
const LINK_WAIT: Duration = Duration::from_secs(3);
const LINK_LOOK: Duration = Duration::from_millis(250);

/// The screen the other computer made, as it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
    /// The monitor index a session asks for to show it.
    pub index: u32,
    pub width: u32,
    pub height: u32,
}

/// An ask whose answer is awaited.
struct Pending {
    id: u64,
    device_id: String,
    tx: oneshot::Sender<serde_json::Value>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Await an answer from `device_id`: the ask's number, and where the answer
/// comes.
fn register(device_id: &str) -> (u64, oneshot::Receiver<serde_json::Value>) {
    let (tx, rx) = oneshot::channel();
    let id = next_id();
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Pending {
            id,
            device_id: device_id.to_string(),
            tx,
        });
    (id, rx)
}

/// Stop awaiting the answer to ask `id`.
fn forget(id: u64) {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p| p.id != id);
}

/// Whether `packet` is another computer's answer to one of this computer's
/// asks (a virtual-screen packet that asks nothing): the gates let it in
/// before any permission, as it asks nothing of this computer.
pub fn is_answer(packet: &NetworkPacket) -> bool {
    packet.packet_type == PACKET && !is_request(&packet.body)
}

/// An answer `body` from the computer `device_id`: given to the ask it
/// names, or, from a computer that says no number, to that computer's oldest
/// ask. Whether any ask took it; one from a computer never asked, or for an
/// ask given up, is dropped.
pub fn heard(device_id: &str, body: &serde_json::Value) -> bool {
    // A computer saying a window it lent is gone is no answer to an ask.
    if crate::borrow_window::note_gone(device_id, body) {
        return true;
    }
    // Nor is a window it offers as it is dragged here (window handoff 4).
    if crate::shared_input::drag_window::note_offer(device_id, body) {
        return true;
    }
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let at = match body.get("id").and_then(serde_json::Value::as_u64) {
        Some(id) => pending
            .iter()
            .position(|p| p.id == id && p.device_id == device_id),
        None => pending.iter().position(|p| p.device_id == device_id),
    };
    match at {
        Some(i) => {
            let _ = pending.remove(i).tx.send(body.clone());
            true
        }
        None => {
            tracing::debug!(
                device_id,
                "A screen answer nothing was waiting for; dropped"
            );
            false
        }
    }
}

/// What an answer says: the screen made, or why not.
pub fn fitted(body: &serde_json::Value) -> Result<Fitted, String> {
    if let Some(why) = body.get("error").and_then(serde_json::Value::as_str) {
        return Err(why.to_string());
    }
    let number = |key: &str| {
        body.get(key)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    match (number("index"), number("width"), number("height")) {
        (Some(index), Some(width), Some(height)) if width > 0 && height > 0 => Ok(Fitted {
            index,
            width,
            height,
        }),
        _ => Err("it answered with no screen".to_string()),
    }
}

/// The words for an ask that got no answer.
const NO_ANSWER: &str =
    "it did not answer (an older Pocketdesk there cannot make a screen to fit a window)";

/// The ask `action` for the window `slot` (`size` for a `fit`), sent through
/// `send` and awaited at most [`ANSWER_WAIT`]; the answer's body. A link
/// that is not up yet is waited for up to [`LINK_WAIT`].
async fn ask_via<F, Fut>(
    send: F,
    device_id: &str,
    slot: u8,
    action: &str,
    size: Option<FitSize>,
) -> Result<serde_json::Value, String>
where
    F: Fn(NetworkPacket) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut body = serde_json::json!({ "action": action, "slot": slot });
    if let Some(size) = size {
        body["width"] = size.width.into();
        body["height"] = size.height.into();
    }
    ask_body_via(send, device_id, body).await
}

/// [`ask_via`] for a request `body` that names its own action and fields:
/// the ask's number is added to it. Borrowing a window asks `list`, `lend`
/// and `return` this way ([`crate::borrow_window`]).
pub(crate) async fn ask_body_via<F, Fut>(
    send: F,
    device_id: &str,
    mut body: serde_json::Value,
) -> Result<serde_json::Value, String>
where
    F: Fn(NetworkPacket) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (id, mut rx) = register(device_id);
    body["id"] = id.into();
    let packet = NetworkPacket::new(PACKET).with_body(body);
    let started = std::time::Instant::now();
    loop {
        match send(packet.clone()).await {
            Ok(()) => break,
            Err(e) if started.elapsed() < LINK_WAIT => {
                tracing::debug!(device_id, "Screen ask not sent yet: {e}");
                tokio::time::sleep(LINK_LOOK).await;
            }
            Err(e) => {
                forget(id);
                return Err(format!("it was not asked: {e}"));
            }
        }
    }
    match tokio::time::timeout(ANSWER_WAIT, &mut rx).await {
        Ok(Ok(body)) => Ok(body),
        _ => {
            forget(id);
            Err(NO_ANSWER.to_string())
        }
    }
}

/// [`ask_via`] to the computer `device_id`, on its connection or the link.
async fn ask(
    device_id: &str,
    slot: u8,
    action: &str,
    size: Option<FitSize>,
) -> Result<serde_json::Value, String> {
    let device = device_id.to_string();
    ask_via(
        move |packet: NetworkPacket| {
            let device = device.clone();
            async move { crate::clipboard_push::send_to_computer(&device, &packet).await }
        },
        device_id,
        slot,
        action,
        size,
    )
    .await
}

/// How a packet goes to the computer `device_id`: on its connection or the
/// link ([`crate::clipboard_push::send_to_computer`]).
pub(crate) fn link_to(
    device_id: &str,
) -> impl Fn(
    NetworkPacket,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
    let device = device_id.to_string();
    move |packet: NetworkPacket| {
        let device = device.clone();
        Box::pin(async move { crate::clipboard_push::send_to_computer(&device, &packet).await })
    }
}

/// [`ask_body_via`] to the computer `device_id`, on its connection or the
/// link.
pub(crate) async fn ask_body(
    device_id: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    ask_body_via(link_to(device_id), device_id, body).await
}

/// One at a time for a computer's window: a resize and a release never
/// cross.
type WindowLock = Arc<tokio::sync::Mutex<()>>;

static KEYS: LazyLock<Mutex<HashMap<(String, u8), WindowLock>>> = LazyLock::new(Default::default);

fn key_lock(device_id: &str, slot: u8) -> WindowLock {
    KEYS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry((device_id.to_string(), slot))
        .or_default()
        .clone()
}

/// Ask the computer `device_id` for the window `slot`'s screen at `size`, or
/// to give the one it has that size; the screen it made, or why not. The ask
/// runs on a task of its own, so a view ended meanwhile does not leave one
/// half-made: [`release`] waits for it. The lock is taken before the task
/// starts, so a release that comes after this call always runs after the
/// ask, never before it (review).
pub async fn fit(device_id: &str, slot: u8, size: FitSize) -> Result<Fitted, String> {
    let one = key_lock(device_id, slot).lock_owned().await;
    let device = device_id.to_string();
    tokio::spawn(async move {
        let _one = one;
        fitted(&ask(&device, slot, "fit", Some(size)).await?)
    })
    .await
    .unwrap_or_else(|_| Err("the ask failed".to_string()))
}

/// Take away the window `slot`'s screen on the computer `device_id`, as its
/// view ends. Not waited for; with none there, nothing happens. Without a
/// runtime it does nothing.
pub fn release(device_id: &str, slot: u8) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let lock = key_lock(device_id, slot);
    let device = device_id.to_string();
    runtime.spawn(async move {
        let _one = lock.lock().await;
        match ask(&device, slot, "remove", None).await {
            Ok(_) => tracing::debug!(
                device_id = device,
                slot,
                "Its screen for this window removed"
            ),
            Err(e) => {
                tracing::debug!(
                    device_id = device,
                    slot,
                    "Its screen for this window not removed: {e}"
                )
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn size() -> FitSize {
        FitSize {
            width: 1280,
            height: 720,
        }
    }

    #[test]
    fn an_answer_is_a_packet_that_asks_nothing() {
        let answer = NetworkPacket::new(PACKET).with_body(json!({ "name": "PD-x-1", "index": 1 }));
        let request = NetworkPacket::new(PACKET).with_body(json!({ "action": "fit" }));
        let other = NetworkPacket::new("kdeconnect.pocketdesk.monitors").with_body(json!({}));
        assert!(is_answer(&answer));
        assert!(!is_answer(&request));
        assert!(!is_answer(&other));
    }

    #[test]
    fn what_an_answer_says_is_the_screen_or_the_reason() {
        assert_eq!(
            fitted(&json!({ "name": "PD-x-1", "index": 2, "width": 1280, "height": 720 })),
            Ok(Fitted {
                index: 2,
                width: 1280,
                height: 720
            })
        );
        assert_eq!(
            fitted(&json!({ "error": "This computer cannot add a screen" })),
            Err("This computer cannot add a screen".to_string())
        );
        for odd in [
            json!({}),
            json!({ "removed": 1 }),
            json!({ "index": 1, "width": 0, "height": 720 }),
            json!({ "index": -1, "width": 10, "height": 10 }),
            json!({ "index": 1, "width": 99999999999u64, "height": 10 }),
        ] {
            assert!(fitted(&odd).is_err(), "{odd}");
        }
    }

    #[test]
    fn an_answer_goes_to_the_ask_it_names_from_the_computer_asked() {
        let (first, mut first_rx) = register("fit-test-desk");
        let (second, mut second_rx) = register("fit-test-desk");
        // Another computer's answer to a number it was never given.
        assert!(!heard(
            "fit-test-other",
            &json!({ "id": first, "index": 1 })
        ));
        assert!(first_rx.try_recv().is_err());
        // The second ask's answer first.
        assert!(heard("fit-test-desk", &json!({ "id": second, "index": 4 })));
        assert_eq!(second_rx.try_recv().unwrap()["index"], 4);
        assert!(first_rx.try_recv().is_err(), "the first still waits");
        // An answer with no number (a computer before it) is the oldest's.
        assert!(heard("fit-test-desk", &json!({ "index": 5 })));
        assert_eq!(first_rx.try_recv().unwrap()["index"], 5);
        // Nothing waits now: an answer is dropped.
        assert!(!heard("fit-test-desk", &json!({ "id": first, "index": 1 })));
    }

    #[tokio::test]
    async fn an_ask_is_sent_with_its_number_and_window_and_gets_its_answer() {
        let sent: Arc<Mutex<Vec<NetworkPacket>>> = Arc::default();
        let log = sent.clone();
        let answered = ask_via(
            move |packet: NetworkPacket| {
                let log = log.clone();
                async move {
                    let id = packet.body["id"].as_u64().unwrap();
                    log.lock().unwrap().push(packet);
                    // The computer answers a moment later, on the same road.
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        heard(
                            "fit-test-ask",
                            &json!({ "id": id, "name": "PD-x-1", "index": 3, "width": 1280, "height": 720 }),
                        );
                    });
                    Ok(())
                }
            },
            "fit-test-ask",
            2,
            "fit",
            Some(size()),
        )
        .await
        .unwrap();
        assert_eq!(fitted(&answered).unwrap().index, 3);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].packet_type, PACKET);
        let body = &sent[0].body;
        assert_eq!(body["action"], "fit");
        assert_eq!(body["slot"], 2);
        assert_eq!(
            (body["width"].as_u64(), body["height"].as_u64()),
            (Some(1280), Some(720))
        );
        assert!(is_request(body));
    }

    #[tokio::test]
    async fn a_remove_asks_no_size() {
        let sent: Arc<Mutex<Vec<NetworkPacket>>> = Arc::default();
        let log = sent.clone();
        let _ = tokio::time::timeout(
            Duration::from_millis(100),
            ask_via(
                move |packet: NetworkPacket| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push(packet);
                        Ok(())
                    }
                },
                "fit-test-remove",
                0,
                "remove",
                None,
            ),
        )
        .await;
        let sent = sent.lock().unwrap();
        assert_eq!(sent[0].body["action"], "remove");
        assert!(sent[0].body.get("width").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn an_ask_nobody_answers_says_so_and_stops_waiting() {
        let said = ask_via(
            |_: NetworkPacket| async { Ok(()) },
            "fit-test-silent",
            0,
            "fit",
            Some(size()),
        )
        .await
        .unwrap_err();
        assert!(said.contains("did not answer"), "{said}");
        assert!(
            !PENDING
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.device_id == "fit-test-silent"),
            "nothing left waiting"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_that_is_not_up_is_waited_for_then_named() {
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = tries.clone();
        let said = ask_via(
            move |_: NetworkPacket| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Err("no link to it".to_string()) }
            },
            "fit-test-nolink",
            0,
            "fit",
            Some(size()),
        )
        .await
        .unwrap_err();
        assert!(said.contains("no link to it"), "{said}");
        assert!(
            tries.load(std::sync::atomic::Ordering::SeqCst) > 3,
            "it looked again"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_link_that_comes_up_meanwhile_carries_the_ask() {
        let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = tries.clone();
        let task = tokio::spawn(ask_via(
            move |packet: NetworkPacket| {
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if n < 2 {
                        return Err("no link to it".to_string());
                    }
                    let id = packet.body["id"].as_u64().unwrap();
                    heard(
                        "fit-test-late",
                        &json!({ "id": id, "index": 1, "width": 10, "height": 10 }),
                    );
                    Ok(())
                }
            },
            "fit-test-late",
            0,
            "fit",
            Some(size()),
        ));
        let answered = task.await.unwrap().unwrap();
        assert_eq!(answered["index"], 1);
    }
}
