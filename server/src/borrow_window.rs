//! Borrowing a window of another computer of the owner's (phase 3b of the
//! window handoff): the viewing computer's side. The host's side is
//! [`crate::lend_window`]: this asks it, with `kdeconnect.pocketdesk.virtual_screen`'s
//! `list`, `lend` and `return` on the connection or link this computer keeps
//! to it ([`crate::fit_screen`]'s asks and their answers carry an `id`, an
//! answer has no `action`), and a view of the screen it lends is an ordinary
//! computer view of that screen's monitor index.
//!
//! - [`ask_list`] answers the app's "Borrow a window…" picker: the host's
//!   windows are written to `computers.json` (`windows_there`, titles
//!   included) until the app closes the picker.
//! - [`lend`] is asked as a borrowed view starts, and again as it dials once
//!   more (the host answers with the same screen while the window is still
//!   lent); [`give_back`] is asked as the view ends.
//! - The host says a window it lent is gone (closed, the switch turned off)
//!   with an answer-shaped packet, `{lent_gone: <window>, reason}`; a view of
//!   that window ends ([`wait_gone`]). Only the computer the window was
//!   borrowed from can end the view of it.
//!
//! A window's title is personal: it is written for the app and never logged.

use std::path::Path;
use std::time::Duration;

use pocketdesk_core::local::computers::{self, FitSize, LendableWindow, WindowsThere};
use pocketdesk_core::protocol::kdeconnect::NetworkPacket;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::fit_screen::{Fitted, ask_body_via, fitted};

/// How long an asked list is kept as "asking" before the app gives up on
/// it (the server's own ask waits [`crate::fit_screen::ANSWER_WAIT`] and a
/// link a few seconds).
pub const LIST_WAIT: Duration = Duration::from_secs(20);

/// The windows a host answered, or why not.
pub fn windows(body: &Value) -> Result<Vec<LendableWindow>, String> {
    if let Some(why) = body.get("error").and_then(Value::as_str) {
        return Err(why.to_string());
    }
    let list = body
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| "it answered with no list of windows".to_string())?;
    // A window that does not read is left out, never the whole list.
    Ok(list
        .iter()
        .filter_map(|w| serde_json::from_value::<LendableWindow>(w.clone()).ok())
        .filter(|w| !w.address.is_empty() && w.width > 0 && w.height > 0)
        .take(200)
        .collect())
}

/// Why a window was not lent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LendError {
    /// The host answered no (the switch is off, the window closed, another
    /// computer has it, its screens are used): asking again will not help.
    Refused(String),
    /// The host could not be asked or did not answer (no link, a short
    /// network gap): the view tries again as it dials again.
    Unreached(String),
}

impl std::fmt::Display for LendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) | Self::Unreached(why) => f.write_str(why),
        }
    }
}

/// The screen a host lent a window on, or why not: what [`fitted`] reads
/// (`{name, index, width, height, window}`), but for the window asked for.
pub fn lent(body: &Value, window: &str) -> Result<Fitted, String> {
    let screen = fitted(body)?;
    match body.get("window").and_then(Value::as_str) {
        Some(said) if said != window => Err("it lent another window".to_string()),
        _ => Ok(screen),
    }
}

/// Ask the computer `device_id` for its lendable windows.
pub async fn list(device_id: &str) -> Result<Vec<LendableWindow>, String> {
    list_via(crate::fit_screen::link_to(device_id), device_id).await
}

/// [`list`], sent as `send` sends.
async fn list_via<F, Fut>(send: F, device_id: &str) -> Result<Vec<LendableWindow>, String>
where
    F: Fn(NetworkPacket) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let body = json!({ "action": "list" });
    windows(&ask_body_via(send, device_id, body).await?)
}

/// The `lend` ask for `window`: with a `size` (the picture area of the window
/// that shows it, in pixels) the host makes the window's screen that size,
/// and gives an already lent window's screen it. A host before that ignores
/// the two fields and lends the window at its own size, which the view then
/// scales.
fn lend_body(window: &str, size: Option<FitSize>) -> Value {
    let mut body = json!({ "action": "lend", "window": window });
    if let Some(size) = size {
        body["width"] = size.width.into();
        body["height"] = size.height.into();
    }
    body
}

/// Ask the computer `device_id` to lend `window`: the screen it was moved
/// to, at `size` when the window that shows it has one it wants (the host
/// follows). Asked again for a window already lent to this computer, the host
/// answers with the same screen (resized when `size` is new).
pub async fn lend(
    device_id: &str,
    window: &str,
    size: Option<FitSize>,
) -> Result<Fitted, LendError> {
    lend_via(
        crate::fit_screen::link_to(device_id),
        device_id,
        window,
        size,
    )
    .await
}

/// [`lend`], sent as `send` sends.
async fn lend_via<F, Fut>(
    send: F,
    device_id: &str,
    window: &str,
    size: Option<FitSize>,
) -> Result<Fitted, LendError>
where
    F: Fn(NetworkPacket) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let body = lend_body(window, size);
    let answer = ask_body_via(send, device_id, body)
        .await
        .map_err(LendError::Unreached)?;
    lent(&answer, window).map_err(LendError::Refused)
}

/// Give `window` back to the computer `device_id`, as its view ends. Not
/// waited for; without a runtime nothing happens. The host puts a window
/// back on its own too (it keeps no borrower past a gone link).
pub fn give_back(device_id: &str, window: &str) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let (device, window) = (device_id.to_string(), window.to_string());
    runtime.spawn(async move {
        let body = json!({ "action": "return", "window": window });
        match crate::fit_screen::ask_body(&device, body).await {
            Ok(said) if said.get("error").is_none() => {
                tracing::debug!(device_id = device, "A borrowed window given back");
            }
            Ok(said) => tracing::debug!(
                device_id = device,
                "A borrowed window not given back: {}",
                said["error"].as_str().unwrap_or("it said no")
            ),
            Err(e) => tracing::debug!(device_id = device, "A borrowed window not given back: {e}"),
        }
    });
}

// ---- The picker's list ----

/// Record in `file` that the list is asked of `computer` at `now`.
fn record_ask(file: &mut computers::ComputersFile, computer: &str, now: u64) {
    // What the computer listed before stays until the answer, so a refresh
    // does not blank the picker.
    let kept = file
        .windows_there
        .take()
        .filter(|t| t.device_id == computer)
        .map(|t| (t.windows, t.listed_unix))
        .unwrap_or_default();
    file.windows_there = Some(WindowsThere {
        device_id: computer.to_string(),
        asking: true,
        asked_unix: now,
        windows: kept.0,
        listed_unix: kept.1,
        ..WindowsThere::default()
    });
}

/// Record what `computer` answered, if its list is still the one asked for
/// (a picker closed meanwhile is not brought back).
fn record_answer(
    file: &mut computers::ComputersFile,
    computer: &str,
    asked: u64,
    answer: Result<Vec<LendableWindow>, String>,
    now: u64,
) {
    let Some(there) = file
        .windows_there
        .as_mut()
        .filter(|t| t.device_id == computer && t.asking && t.asked_unix == asked)
    else {
        return;
    };
    there.asking = false;
    match answer {
        Ok(windows) => {
            there.windows = windows;
            there.listed_unix = now;
            there.error = None;
        }
        Err(why) => {
            there.windows.clear();
            there.error = Some(why);
        }
    }
}

/// The app asked for `computer`'s windows: marked as asking, then asked, and
/// what came back (or why not) written for the app. The windows' titles
/// are never logged.
pub async fn ask_list(dir: &Path, computer: &str) {
    let now = pocketdesk_core::local::pin::now_unix();
    let recorded = {
        let (dir, computer) = (dir.to_path_buf(), computer.to_string());
        tokio::task::spawn_blocking(move || {
            computers::update(&dir, |file| record_ask(file, &computer, now))
        })
        .await
    };
    if !matches!(recorded, Ok(Ok(()))) {
        tracing::warn!("Could not record an ask of another computer's windows");
        return;
    }
    let answer = list(computer).await;
    match &answer {
        Ok(windows) => tracing::debug!("Another computer listed {} windows", windows.len()),
        Err(e) => tracing::debug!("Another computer's windows not listed: {e}"),
    }
    let (dir, computer) = (dir.to_path_buf(), computer.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        let done = pocketdesk_core::local::pin::now_unix();
        computers::update(&dir, |file| {
            record_answer(file, &computer, now, answer, done)
        })
    })
    .await;
}

/// How long a list is kept without being asked for again: the app's open
/// picker asks again well within it, so a list left behind (the app quit or
/// crashed, the page left) does not keep window titles on disk.
pub const LIST_KEPT: u64 = 60;

/// Whether `there` has been left longer than [`LIST_KEPT`] at `now`.
fn list_expired(there: &WindowsThere, now: u64) -> bool {
    now.saturating_sub(there.listed_unix.max(there.asked_unix)) > LIST_KEPT
}

/// Drop a list nobody asked for again within [`LIST_KEPT`] (run every pass
/// of the computers loop), and, at start (`all`), any a stopped server left.
pub fn expire_lists(dir: &Path, all: bool) {
    let now = pocketdesk_core::local::pin::now_unix();
    let saved = computers::update(dir, |file| {
        if file
            .windows_there
            .as_ref()
            .is_some_and(|t| all || list_expired(t, now))
        {
            file.windows_there = None;
        }
    });
    if let Err(e) = saved {
        tracing::debug!("Could not drop an old list of another computer's windows: {e}");
    }
}

/// Forget what `computer` listed, the picker closed: the titles are not
/// kept past that.
pub fn close_list(dir: &Path, computer: &str) {
    let saved = computers::update(dir, |file| {
        if file
            .windows_there
            .as_ref()
            .is_some_and(|t| t.device_id == computer)
        {
            file.windows_there = None;
        }
    });
    if let Err(e) = saved {
        tracing::debug!("Could not forget another computer's windows: {e}");
    }
}

// ---- A window gone ----

/// What a host says of a window gone: the host, the window and the reason
/// it gave (`closed`, `off`, or `returned` for a window dragged back).
type Gone = (String, String, Option<String>);

/// A window a host says is gone.
fn gone_events() -> &'static broadcast::Sender<Gone> {
    static GONE: std::sync::LazyLock<broadcast::Sender<Gone>> =
        std::sync::LazyLock::new(|| broadcast::channel(16).0);
    &GONE
}

/// `body`, from the computer `device_id`, says a window it lent is gone
/// (`{lent_gone: <window>}`): the views of it are told. Whether it was such
/// a packet.
pub fn note_gone(device_id: &str, body: &Value) -> bool {
    let Some(window) = body.get("lent_gone").and_then(Value::as_str) else {
        return false;
    };
    // Nobody listening is fine: no view shows that window.
    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string);
    let _ = gone_events().send((device_id.to_string(), window.to_string(), reason));
    true
}

/// Hear of windows gone from now on: take one before the view starts, so
/// none is missed.
pub fn gone() -> broadcast::Receiver<Gone> {
    gone_events().subscribe()
}

/// Wait until the computer `device_id` says its window `window` is gone:
/// the reason it gave, if it gave one.
pub async fn wait_gone(
    events: &mut broadcast::Receiver<Gone>,
    device_id: &str,
    window: &str,
) -> Option<String> {
    loop {
        match events.recv().await {
            Ok((from, gone, reason)) if from == device_id && gone == window => return reason,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            // No sender left: nothing will say it.
            Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use computers::ComputersFile;

    fn window(address: &str, title: &str) -> Value {
        json!({ "address": address, "title": title, "class": "app", "width": 800, "height": 600, "workspace": "1" })
    }

    #[test]
    fn a_list_is_read_and_a_bad_window_does_not_spoil_it() {
        let body = json!({ "id": 3, "windows": [
            window("5", "Notes"),
            { "address": "6" },
            window("", "No address"),
            { "address": "7", "title": "Zero", "width": 0, "height": 10 },
            window("0x7", "Browser"),
        ]});
        let got = windows(&body).unwrap();
        let addresses: Vec<&str> = got.iter().map(|w| w.address.as_str()).collect();
        assert_eq!(addresses, ["5", "0x7"]);
        assert_eq!(got[0].title, "Notes");
        assert!(!got[0].lent);
    }

    #[test]
    fn a_refusal_or_no_list_is_the_reason() {
        assert_eq!(
            windows(&json!({ "error": "This computer does not lend its windows" })),
            Err("This computer does not lend its windows".to_string())
        );
        assert!(windows(&json!({})).is_err());
        assert!(windows(&json!({ "windows": 4 })).is_err());
        assert_eq!(windows(&json!({ "windows": [] })), Ok(Vec::new()));
    }

    #[test]
    fn a_lend_answer_is_the_screen_of_the_window_asked_for() {
        let said =
            json!({ "name": "PD-x-1", "index": 2, "width": 800, "height": 600, "window": "5" });
        assert_eq!(
            lent(&said, "5"),
            Ok(Fitted {
                index: 2,
                width: 800,
                height: 600
            })
        );
        assert!(lent(&said, "6").is_err(), "another window's screen");
        // A host that names no window (it always does) still gave a screen.
        let bare = json!({ "index": 2, "width": 800, "height": 600 });
        assert!(lent(&bare, "5").is_ok());
        assert_eq!(
            lent(
                &json!({ "error": "Another computer has borrowed that window" }),
                "5"
            ),
            Err("Another computer has borrowed that window".to_string())
        );
    }

    #[test]
    fn the_picker_list_is_recorded_asking_then_answered() {
        let mut file = ComputersFile::default();
        record_ask(&mut file, "desk", 100);
        let there = file.windows_there.clone().unwrap();
        assert!(there.asking && there.windows.is_empty() && there.listed_unix == 0);
        let listed = windows(&json!({ "windows": [window("5", "Notes")] })).unwrap();
        record_answer(&mut file, "desk", 100, Ok(listed), 103);
        let there = file.windows_there.clone().unwrap();
        assert!(!there.asking);
        assert_eq!(there.windows.len(), 1);
        assert_eq!(there.listed_unix, 103);
        assert_eq!(there.error, None);
    }

    #[test]
    fn a_refresh_keeps_the_old_list_until_the_answer() {
        let mut file = ComputersFile::default();
        record_ask(&mut file, "desk", 100);
        let listed = windows(&json!({ "windows": [window("5", "Notes")] })).unwrap();
        record_answer(&mut file, "desk", 100, Ok(listed), 101);
        record_ask(&mut file, "desk", 130);
        let there = file.windows_there.clone().unwrap();
        assert!(there.asking);
        assert_eq!(there.windows.len(), 1, "the picker does not blank");
        assert_eq!(there.listed_unix, 101);
        // Another computer's ask starts clean.
        record_ask(&mut file, "lap", 131);
        assert!(file.windows_there.unwrap().windows.is_empty());
    }

    #[test]
    fn a_list_nobody_asked_for_again_is_dropped_and_a_start_drops_any() {
        let dir = std::env::temp_dir().join(format!("pd-borrow-expire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = pocketdesk_core::local::pin::now_unix();
        let kept = |dir: &Path| computers::read(dir).windows_there.is_some();
        computers::update(&dir, |file| record_ask(file, "desk", now - LIST_KEPT - 5)).unwrap();
        assert!(kept(&dir));
        expire_lists(&dir, false);
        assert!(!kept(&dir), "left past the time kept");
        // A fresh one stays through a pass, and goes at a start.
        computers::update(&dir, |file| record_ask(file, "desk", now)).unwrap();
        expire_lists(&dir, false);
        assert!(kept(&dir));
        expire_lists(&dir, true);
        assert!(!kept(&dir), "a stopped server's list goes at start");
        // A list answered later counts from its answer.
        computers::update(&dir, |file| {
            record_ask(file, "desk", now - LIST_KEPT - 5);
            record_answer(file, "desk", now - LIST_KEPT - 5, Ok(Vec::new()), now - 10);
        })
        .unwrap();
        expire_lists(&dir, false);
        assert!(kept(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refusal_is_recorded_with_no_windows_left_from_before() {
        let mut file = ComputersFile::default();
        record_ask(&mut file, "desk", 100);
        let listed = windows(&json!({ "windows": [window("5", "Notes")] })).unwrap();
        record_answer(&mut file, "desk", 100, Ok(listed), 101);
        record_ask(&mut file, "desk", 200);
        record_answer(&mut file, "desk", 200, Err("no".to_string()), 201);
        let there = file.windows_there.unwrap();
        assert!(
            there.windows.is_empty(),
            "the old list does not outlast a no"
        );
        assert_eq!(there.error.as_deref(), Some("no"));
    }

    #[test]
    fn an_answer_for_a_closed_or_newer_picker_is_dropped() {
        let mut file = ComputersFile::default();
        // Closed meanwhile: not brought back.
        record_answer(&mut file, "desk", 100, Err("x".into()), 101);
        assert!(file.windows_there.is_none());
        // Asked again meanwhile: the older ask's answer does not take its place.
        record_ask(&mut file, "desk", 100);
        record_ask(&mut file, "desk", 200);
        record_answer(&mut file, "desk", 100, Err("old".into()), 201);
        assert!(file.windows_there.as_ref().unwrap().asking);
        // Another computer's answer.
        record_answer(&mut file, "lap", 200, Err("x".into()), 202);
        assert!(file.windows_there.as_ref().unwrap().asking);
    }

    /// A host that answers each ask at once, as it does on its link: the
    /// ask is recorded and its answer comes back with the ask's `id`.
    fn host(
        device: &'static str,
        answer: Value,
        asked: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    ) -> impl Fn(NetworkPacket) -> std::future::Ready<Result<(), String>> {
        move |packet: NetworkPacket| {
            asked.lock().unwrap().push(packet.body.clone());
            let mut said = answer.clone();
            said["id"] = packet.body["id"].clone();
            crate::fit_screen::heard(device, &said);
            std::future::ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn the_list_and_the_lend_are_asked_by_name_and_read_from_the_answer() {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let listed = list_via(
            host(
                "borrow-test-list",
                json!({ "windows": [window("5", "Notes")] }),
                asked.clone(),
            ),
            "borrow-test-list",
        )
        .await
        .unwrap();
        assert_eq!(listed[0].address, "5");
        let lent = lend_via(
            host(
                "borrow-test-lend",
                json!({ "name": "PD-x-1", "index": 3, "width": 800, "height": 600, "window": "5" }),
                asked.clone(),
            ),
            "borrow-test-lend",
            "5",
            None,
        )
        .await
        .unwrap();
        assert_eq!(lent.index, 3);
        let asked = asked.lock().unwrap();
        assert_eq!(asked[0]["action"], "list");
        assert_eq!(
            (asked[1]["action"].as_str(), asked[1]["window"].as_str()),
            (Some("lend"), Some("5"))
        );
        assert!(
            asked[1]["id"].as_u64().is_some(),
            "an id comes back with the answer"
        );
        // Both are asks: a packet with an action, never taken for an answer.
        assert!(crate::virtual_screen::is_request(&asked[0]));
    }

    /// The size rides on the `lend` ask as `width` and `height`, and is left
    /// off when none is wanted (a host before it reads the ask as it did).
    #[tokio::test]
    async fn a_lend_names_the_size_of_the_window_that_shows_it_when_there_is_one() {
        assert_eq!(
            lend_body("5", None),
            json!({ "action": "lend", "window": "5" })
        );
        assert_eq!(
            lend_body(
                "5",
                Some(FitSize {
                    width: 1000,
                    height: 600
                })
            ),
            json!({ "action": "lend", "window": "5", "width": 1000, "height": 600 })
        );
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let lent = lend_via(
            host(
                "borrow-test-sized",
                json!({ "name": "PD-x-1", "index": 3, "width": 1000, "height": 600, "window": "5" }),
                asked.clone(),
            ),
            "borrow-test-sized",
            "5",
            Some(FitSize {
                width: 1000,
                height: 600,
            }),
        )
        .await
        .unwrap();
        assert_eq!((lent.width, lent.height), (1000, 600));
        let asked = asked.lock().unwrap();
        assert_eq!(
            (asked[0]["width"].as_u64(), asked[0]["height"].as_u64()),
            (Some(1000), Some(600))
        );
        assert!(crate::virtual_screen::is_request(&asked[0]));
    }

    #[tokio::test]
    async fn a_host_that_refuses_is_the_reason_given() {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let said = lend_via(
            host(
                "borrow-test-refuse",
                json!({ "error": "This computer does not lend its windows" }),
                asked,
            ),
            "borrow-test-refuse",
            "5",
            None,
        )
        .await;
        assert_eq!(
            said,
            Err(LendError::Refused(
                "This computer does not lend its windows".to_string()
            ))
        );
    }

    /// A host that cannot be asked, or does not answer, has not said no: the
    /// view tries again as it dials again.
    #[tokio::test(start_paused = true)]
    async fn a_host_that_cannot_be_reached_is_not_a_refusal() {
        let no_link = lend_via(
            |_: NetworkPacket| std::future::ready(Err("no link".to_string())),
            "borrow-test-nolink",
            "5",
            None,
        )
        .await;
        assert_eq!(
            no_link,
            Err(LendError::Unreached(
                "it was not asked: no link".to_string()
            ))
        );
        let silent = lend_via(
            |_: NetworkPacket| std::future::ready(Ok(())),
            "borrow-test-silent",
            "5",
            None,
        )
        .await;
        assert!(matches!(silent, Err(LendError::Unreached(why)) if why.contains("did not answer")));
    }

    #[tokio::test]
    async fn a_gone_window_ends_only_the_view_of_that_window_of_that_computer() {
        let mut events = gone();
        assert!(!note_gone("desk", &json!({ "name": "PD-x-1", "index": 1 })));
        assert!(note_gone("lap", &json!({ "lent_gone": "5" })));
        assert!(note_gone("desk", &json!({ "lent_gone": "6" })));
        assert!(note_gone(
            "desk",
            &json!({ "lent_gone": "5", "reason": "closed" })
        ));
        let reason =
            tokio::time::timeout(Duration::from_secs(2), wait_gone(&mut events, "desk", "5"))
                .await
                .expect("desk's window 5 said gone");
        assert_eq!(reason.as_deref(), Some("closed"));
        // A window dragged back says so.
        assert!(note_gone(
            "desk",
            &json!({ "lent_gone": "7", "reason": "returned" })
        ));
        let reason =
            tokio::time::timeout(Duration::from_secs(2), wait_gone(&mut events, "desk", "7"))
                .await
                .unwrap();
        assert_eq!(reason.as_deref(), Some("returned"));
        // Nothing else is left for it: the other two were not its window.
        let mut events = gone();
        assert!(note_gone("lap", &json!({ "lent_gone": "9" })));
        let waited = tokio::time::timeout(
            Duration::from_millis(100),
            wait_gone(&mut events, "desk", "9"),
        )
        .await;
        assert!(waited.is_err(), "another computer's word is not its");
    }
}
