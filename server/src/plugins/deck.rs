//! The deck (F1): a phone lists the buttons the owner set up
//! (`[[deck]]`) and presses one by its index; the computer does the
//! button's action without a session, under that phone's own permission
//! for it (keys and text: Input, and the phone may drive; a task: Tasks;
//! media: Audio; a link: Links). Every press is answered.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::local::deck::{DeckAction, DeckButton, usable};
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::streaming::input_packet::InputPacket;
use serde_json::{Value, json};

use crate::input_injector::InputInjector;

pub const DECK: &str = "kdeconnect.pocketdesk.deck";
pub const DECK_PRESS: &str = "kdeconnect.pocketdesk.deck_press";
pub const DECK_RESULT: &str = "kdeconnect.pocketdesk.deck_result";

/// Presses a phone may make in a minute, refused ones included.
const MAX_PRESSES_PER_MINUTE: usize = 30;

/// Each phone's recent presses, for [`MAX_PRESSES_PER_MINUTE`].
static PRESSES: LazyLock<StdMutex<HashMap<String, VecDeque<Instant>>>> =
    LazyLock::new(Default::default);

/// The listing a phone is shown: the usable buttons by their config index,
/// with their kind and never the keys, link or text themselves.
pub(crate) fn listing(enabled: bool, buttons: &[DeckButton]) -> Value {
    if !enabled {
        return json!({ "buttons": [] });
    }
    let shown: Vec<Value> = usable(buttons)
        .into_iter()
        .map(|(index, button, action)| {
            json!({ "index": index, "label": button.label, "kind": action.kind() })
        })
        .collect();
    json!({ "buttons": shown })
}

/// The permission a button's action needs, and how its refusal reads.
fn needs(action: &DeckAction) -> (Capability, &'static str) {
    match action {
        DeckAction::Keys(_) | DeckAction::Text(_) => (
            Capability::Input,
            "\"Control the mouse and keyboard\" is turned off for this phone on the computer",
        ),
        DeckAction::Task(_) => (
            Capability::Tasks,
            "\"Start my tasks\" is turned off for this phone on the computer",
        ),
        DeckAction::Media(_) => (
            Capability::Audio,
            "Audio is turned off for this phone on the computer",
        ),
        DeckAction::Link(_) => (
            Capability::Links,
            "\"Open its links here\" is turned off for this phone on the computer",
        ),
    }
}

/// The action a press asks for, or why it is refused: the deck on, the
/// button at `index` still labelled `label` and usable, and the phone
/// allowed its action (`allows`).
pub(crate) fn decide(
    enabled: bool,
    buttons: &[DeckButton],
    index: usize,
    label: &str,
    allows: impl Fn(Capability) -> bool,
) -> std::result::Result<DeckAction, String> {
    if !enabled {
        return Err("the deck is turned off on the computer".into());
    }
    let Some(button) = buttons.get(index).filter(|b| b.label == label) else {
        return Err("the computer's deck has changed: open it again".into());
    };
    let action = button.action()?;
    let (capability, refusal) = needs(&action);
    if !allows(capability) {
        return Err(refusal.to_string());
    }
    Ok(action)
}

/// Press a combination through `inject` (a code, pressed or not): each key
/// pressed in order, then those that went down let go in reverse, so
/// modifiers hold while the last key is tapped. A press that fails stops
/// the presses, and every key already down is still let go, a failed
/// release not stopping the rest: no modifier is left held.
pub(crate) fn press_combination(
    codes: &[u16],
    inject: &mut dyn FnMut(u16, bool) -> std::result::Result<(), String>,
) -> std::result::Result<(), String> {
    let mut down = Vec::with_capacity(codes.len());
    let mut failed = None;
    for code in codes {
        match inject(*code, true) {
            Ok(()) => down.push(*code),
            Err(why) => {
                failed = Some(why);
                break;
            }
        }
    }
    for code in down.iter().rev() {
        if let Err(why) = inject(*code, false) {
            failed.get_or_insert(why);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Whether `device` may press again at `now`; counts it.
fn within_rate(device: &str, now: Instant) -> bool {
    let mut presses = PRESSES.lock().unwrap_or_else(|e| e.into_inner());
    let times = presses.entry(device.to_string()).or_default();
    while times
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(60))
    {
        times.pop_front();
    }
    if times.len() >= MAX_PRESSES_PER_MINUTE {
        return false;
    }
    times.push_back(now);
    true
}

/// The injector the deck's keys and text go through, one for the server,
/// used on blocking threads only.
static INJECTOR: LazyLock<StdMutex<Option<InputInjector>>> = LazyLock::new(Default::default);

/// Run `use_it` with the deck's injector, made on first use. Blocks: call
/// it on a blocking thread, never the runtime's (making an injector runs a
/// program on Linux, and typing a long text takes a while).
fn with_injector(
    use_it: impl FnOnce(&mut InputInjector) -> std::result::Result<(), String>,
) -> std::result::Result<(), String> {
    let mut slot = INJECTOR.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot =
            Some(InputInjector::new().map_err(|e| format!("the computer cannot type here: {e}"))?);
    }
    use_it(slot.as_mut().expect("made above"))
}

/// Make the deck's injector at start when the deck has buttons (`wanted`):
/// on Linux a uinput device made at the moment of a press can miss its
/// first events, so a press's first key (the Ctrl of Ctrl+C) was lost.
pub fn warm_up(wanted: bool) {
    if !wanted {
        return;
    }
    tokio::task::spawn_blocking(|| {
        if let Err(why) = with_injector(|_| Ok(())) {
            tracing::warn!("The deck cannot type here: {why}");
        }
    });
}

/// Run `use_it` with the injector on a blocking thread.
async fn inject(
    use_it: impl FnOnce(&mut InputInjector) -> std::result::Result<(), String> + Send + 'static,
) -> std::result::Result<(), String> {
    tokio::task::spawn_blocking(move || with_injector(use_it))
        .await
        .map_err(|e| format!("the keys did not go through: {e}"))?
}

/// The deck's plugin.
#[derive(Default)]
pub struct DeckPlugin;

impl DeckPlugin {
    /// Do `action` for `device`.
    async fn perform(&self, device: &str, action: &DeckAction) -> std::result::Result<(), String> {
        match action {
            DeckAction::Keys(codes) => {
                if !crate::driving::may_drive(device) {
                    return Err("another phone is driving the computer".into());
                }
                let codes: Vec<u16> = codes.iter().map(|c| c.0).collect();
                inject(move |injector| {
                    press_combination(&codes, &mut |key, pressed| {
                        injector
                            .handle_input_packet(&InputPacket::KeyEvent { key, pressed })
                            .map_err(|e| format!("the keys did not go through: {e}"))
                    })
                })
                .await
            }
            DeckAction::Text(text) => {
                if !crate::driving::may_drive(device) {
                    return Err("another phone is driving the computer".into());
                }
                let text = text.clone();
                inject(move |injector| {
                    injector
                        .handle_input_packet(&InputPacket::Text(text))
                        .map_err(|e| format!("the text did not go through: {e}"))
                })
                .await
            }
            DeckAction::Task(name) => super::tasks::start_from_deck(device, name)
                .map_err(|why| format!("the task did not start: {why}")),
            DeckAction::Media(key) => {
                let command = crate::media::Command::parse(key.as_str())
                    .ok_or_else(|| "not a media button".to_string())?;
                tokio::task::spawn_blocking(move || crate::media::command(command))
                    .await
                    .map_err(|e| format!("the media controls failed: {e}"))?
            }
            DeckAction::Link(url) => {
                if let Some(problem) = pocketdesk_core::protocol::link::link_problem(url) {
                    return Err(format!("the link cannot be opened: {problem}"));
                }
                super::links::open_detached(url)
            }
        }
    }
}

fn result(index: usize, outcome: &std::result::Result<(), String>) -> NetworkPacket {
    let body = match outcome {
        Ok(()) => json!({ "index": index, "ok": true }),
        Err(reason) => json!({ "index": index, "ok": false, "reason": reason }),
    };
    NetworkPacket::new(DECK_RESULT).with_body(body)
}

#[async_trait::async_trait]
impl Plugin for DeckPlugin {
    fn name(&self) -> &'static str {
        "deck"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[DECK, DECK_PRESS]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[DECK, DECK_RESULT]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        let config = crate::events::current_config();
        match packet.packet_type.as_str() {
            DECK => {
                let reply =
                    NetworkPacket::new(DECK).with_body(listing(config.deck_enabled, &config.deck));
                sender.send_packet(&reply).await?;
            }
            DECK_PRESS => {
                let Some(index) = packet
                    .body
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                else {
                    return Ok(());
                };
                let label = packet
                    .body
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let device = sender.device_id().to_string();
                let outcome = if !within_rate(&device, Instant::now()) {
                    Err("too many presses: wait a minute".to_string())
                } else {
                    match decide(config.deck_enabled, &config.deck, index, label, |c| {
                        super::pair::allows(&device, c)
                    }) {
                        Ok(action) => {
                            let done = self.perform(&device, &action).await;
                            crate::events::emit(
                                "deck_pressed",
                                Some(&device),
                                json!({ "kind": action.kind(), "ok": done.is_ok() }),
                                json!({ "label": label }),
                            );
                            done
                        }
                        Err(why) => Err(why),
                    }
                };
                if let Err(why) = &outcome {
                    tracing::info!("Deck press from {device} refused: {why}");
                }
                sender.send_packet(&result(index, &outcome)).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(label: &str, keys: &str) -> DeckButton {
        DeckButton {
            label: label.into(),
            keys: Some(keys.into()),
            ..Default::default()
        }
    }

    #[test]
    fn the_listing_shows_usable_buttons_by_kind_and_nothing_when_off() {
        let buttons = vec![
            keys("Copy", "ctrl+c"),
            DeckButton {
                label: "Broken".into(),
                ..Default::default()
            },
            DeckButton {
                label: "Site".into(),
                link: Some("https://example.org/secret".into()),
                ..Default::default()
            },
        ];
        let shown = listing(true, &buttons);
        assert_eq!(
            shown,
            json!({"buttons": [
                {"index": 0, "label": "Copy", "kind": "keys"},
                {"index": 2, "label": "Site", "kind": "link"},
            ]})
        );
        assert!(
            !shown.to_string().contains("secret"),
            "never the link itself"
        );
        assert!(
            !shown.to_string().contains("ctrl"),
            "never the keys themselves"
        );
        assert_eq!(listing(false, &buttons), json!({"buttons": []}));
    }

    #[test]
    fn a_press_is_held_to_the_switch_the_label_and_the_permission() {
        let buttons = vec![
            keys("Copy", "ctrl+c"),
            DeckButton {
                label: "Build".into(),
                task: Some("build".into()),
                ..Default::default()
            },
        ];
        let all = |_| true;
        assert!(matches!(
            decide(true, &buttons, 0, "Copy", all),
            Ok(DeckAction::Keys(_))
        ));
        assert!(
            decide(false, &buttons, 0, "Copy", all)
                .unwrap_err()
                .contains("turned off")
        );
        assert!(
            decide(true, &buttons, 0, "Paste", all)
                .unwrap_err()
                .contains("changed")
        );
        assert!(
            decide(true, &buttons, 7, "Copy", all)
                .unwrap_err()
                .contains("changed")
        );
        let no_input = |c| c != Capability::Input;
        assert!(
            decide(true, &buttons, 0, "Copy", no_input)
                .unwrap_err()
                .contains("Control the mouse and keyboard")
        );
        assert_eq!(
            decide(true, &buttons, 1, "Build", no_input),
            Ok(DeckAction::Task("build".into()))
        );
        let no_tasks = |c| c != Capability::Tasks;
        assert!(
            decide(true, &buttons, 1, "Build", no_tasks)
                .unwrap_err()
                .contains("Start my tasks")
        );
    }

    /// Runs a combination against a fake injector that fails where told,
    /// returning what it was asked to do.
    fn run(codes: &[u16], fail: &[(u16, bool)]) -> (Vec<(u16, bool)>, bool) {
        let mut seen = Vec::new();
        let outcome = press_combination(codes, &mut |key, pressed| {
            seen.push((key, pressed));
            if fail.contains(&(key, pressed)) {
                Err(format!("{key} failed"))
            } else {
                Ok(())
            }
        });
        (seen, outcome.is_ok())
    }

    #[test]
    fn a_combination_presses_in_order_and_lets_go_in_reverse() {
        assert_eq!(
            run(&[29, 42, 50], &[]),
            (
                vec![
                    (29, true),
                    (42, true),
                    (50, true),
                    (50, false),
                    (42, false),
                    (29, false)
                ],
                true
            )
        );
    }

    #[test]
    fn a_failed_press_still_lets_go_of_every_key_down() {
        // Ctrl and Shift went down, M failed: both are let go, in reverse.
        assert_eq!(
            run(&[29, 42, 50], &[(50, true)]),
            (
                vec![(29, true), (42, true), (50, true), (42, false), (29, false)],
                false
            )
        );
        // A release that fails does not stop the others.
        assert_eq!(
            run(&[29, 50], &[(50, false)]),
            (
                vec![(29, true), (50, true), (50, false), (29, false)],
                false
            )
        );
    }

    #[test]
    fn presses_are_paced_per_phone() {
        let now = Instant::now();
        for _ in 0..MAX_PRESSES_PER_MINUTE {
            assert!(within_rate("deck-test-phone", now));
        }
        assert!(!within_rate("deck-test-phone", now));
        assert!(within_rate("deck-test-other", now));
        assert!(within_rate(
            "deck-test-phone",
            now + Duration::from_secs(61)
        ));
    }
}
