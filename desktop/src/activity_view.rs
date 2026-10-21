//! The Activity tab (ecosystem phase 1b): one timeline of what Pocketdesk
//! did, newest first, read from the event log the server keeps
//! (`core::local::events`), so it says the same as `pocketdesk events`.

use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText};
use pocketdesk_core::local::events::{self, Event, Tail};
use serde_json::Value;

/// The events kept on screen.
const KEEP: usize = 500;

/// Which events the timeline shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Sessions,
    Phones,
    FilesAndLinks,
    Messages,
    Hooks,
}

impl Filter {
    const ALL: [(Filter, &'static str); 6] = [
        (Filter::All, "All"),
        (Filter::Sessions, "Sessions"),
        (Filter::Phones, "Phones"),
        (Filter::FilesAndLinks, "Files and links"),
        (Filter::Messages, "Messages"),
        (Filter::Hooks, "Tasks, deck and hooks"),
    ];

    fn admits(self, kind: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Sessions => kind.starts_with("session_"),
            // A lock when a phone left is about phones too (E5).
            Filter::Phones => kind.starts_with("phone_") || kind == "away_locked",
            Filter::FilesAndLinks => matches!(kind, "file_received" | "link_received"),
            Filter::Messages => matches!(kind, "notification" | "text_received"),
            Filter::Hooks => {
                kind == "hook_ran" || kind == "deck_pressed" || kind.starts_with("task_")
            }
        }
    }
}

/// What a row offers to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Show a received file in its folder.
    ShowFile(PathBuf),
    /// Open Settings (the log is off).
    OpenSettings,
    /// Mute this phone app's notifications here (K2b).
    Mute(String),
}

/// The app a phone notification's event names, for "Not from this app"
/// (K2b); none for any other event.
pub fn notifying_app(event: &Event) -> Option<&str> {
    (event.event == "notification")
        .then(|| text(event, "app"))
        .flatten()
        .filter(|app| !app.trim().is_empty())
}

#[derive(Default)]
pub struct ActivityView {
    /// Oldest first, at most [`KEEP`].
    events: Vec<Event>,
    tail: Option<Tail>,
    /// The first read after the history, which may repeat its last events.
    fresh: bool,
    /// `received.json`, as last read, for Show in folder.
    received: Vec<pocketdesk_core::local::received::ReceivedFile>,
    filter: Filter,
    /// The apps muted here (K2b), read with the events.
    muted: Vec<pocketdesk_core::local::muted_apps::MutedApp>,
    /// What the Search box holds (J4).
    search: String,
}

/// The events shown, newest first: of the chip's kind, and holding every
/// word of `search` in their line as shown, the phone's name and the
/// event's words (J4).
pub fn shown<'a>(
    events: &'a [Event],
    filter: Filter,
    search: &str,
    devices: &[crate::model::Device],
) -> Vec<&'a Event> {
    events
        .iter()
        .rev()
        .filter(|e| filter.admits(&e.event))
        // An event's words are built only for a search (J4 review).
        .filter(|e| {
            search.trim().is_empty()
                || crate::words::holds_all(search, &[&phone(e, devices), &describe(e)])
        })
        .collect()
}

impl ActivityView {
    /// Read what was written since the last look (the history, the first
    /// time).
    pub fn refresh(&mut self, state_dir: &Path) {
        match &mut self.tail {
            Some(tail) => {
                let new = tail.poll();
                if std::mem::take(&mut self.fresh) {
                    // Written between the tail's start and the history's
                    // read: already shown.
                    let recent = self.events.len().saturating_sub(new.len());
                    let shown = self.events[recent..].to_vec();
                    self.events
                        .extend(new.into_iter().filter(|e| !shown.contains(e)));
                } else {
                    self.events.extend(new);
                }
            }
            None => {
                self.tail = Some(Tail::at_end(state_dir));
                self.events = events::history(state_dir, KEEP);
                self.fresh = true;
            }
        }
        let extra = self.events.len().saturating_sub(KEEP);
        self.events.drain(..extra);
        self.received = pocketdesk_core::local::received::read(state_dir);
        self.muted = pocketdesk_core::local::muted_apps::read(state_dir);
    }

    /// Opened again: read the history afresh (a tail kept while another tab
    /// was open could have missed two rotations).
    pub fn reopen(&mut self) {
        self.tail = None;
    }

    /// Draw the tab; what the person asked for, if anything.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        log_on: bool,
        devices: &[crate::model::Device],
        now_unix: u64,
    ) -> Option<Action> {
        let mut action = None;
        if !log_on {
            ui.horizontal_wrapped(|ui| {
                ui.label("The event log is off, so nothing new shows here.");
                if ui.button("Turn it on in Settings").clicked() {
                    action = Some(Action::OpenSettings);
                }
            });
            ui.add_space(4.0);
        }
        ui.horizontal_wrapped(|ui| {
            for (filter, label) in Filter::ALL {
                if ui.selectable_label(self.filter == filter, label).clicked() {
                    self.filter = filter;
                }
            }
            crate::search_box::show(ui, &mut self.search);
        });
        ui.add_space(6.0);
        let shown = shown(&self.events, self.filter, &self.search, devices);
        if shown.is_empty() {
            let query = self.search.trim().to_string();
            if !query.is_empty() && !self.events.is_empty() {
                // J4: a search that finds nothing says so, with the way back.
                ui.horizontal_wrapped(|ui| {
                    // Under a kind chip, it says the chip narrows too (J4
                    // review).
                    let said = if self.filter == Filter::All {
                        format!("Nothing here matches \"{query}\".")
                    } else {
                        format!("Nothing of this kind matches \"{query}\".")
                    };
                    ui.label(RichText::new(said).color(crate::theme::muted(ui)));
                    if ui.button("Clear search").clicked() {
                        self.search.clear();
                    }
                });
            } else if self.filter != Filter::All && !self.events.is_empty() {
                // A filter that hides everything says so, with the way back (E5).
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Nothing of this kind yet.").color(crate::theme::muted(ui)),
                    );
                    if ui.button("Show all").clicked() {
                        self.filter = Filter::All;
                    }
                });
            } else {
                ui.label(
                    RichText::new("Nothing yet. Sessions, phones, files, links, notifications and hooks show here as they happen.")
                        .color(crate::theme::muted(ui)),
                );
            }
            return action;
        }
        let received = &self.received;
        let muted = &self.muted;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // E5: a row per event that wraps; a grid of four columns
                // ran a long link or title off the page.
                for event in shown {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(crate::words::ago(now_unix, event.at)).weak());
                        ui.label(phone(event, devices));
                        ui.label(describe(event));
                        if let Some(path) = file_path(event, received)
                            && ui.small_button("Show in folder").clicked()
                        {
                            action = Some(Action::ShowFile(path));
                        }
                        if let Some(app) = notifying_app(event) {
                            if pocketdesk_core::local::muted_apps::is_muted(muted, app) {
                                ui.label(RichText::new("muted here").weak());
                            } else if ui
                                .small_button("Not from this app")
                                .on_hover_text(format!(
                                    "Stop showing {app}'s notifications from phones on this computer"
                                ))
                                .clicked()
                            {
                                action = Some(Action::Mute(app.to_string()));
                            }
                        }
                    });
                    ui.separator();
                }
            });
        action
    }
}

/// The phone by its name (U3), else by the first characters of its id;
/// Home Assistant by name.
fn phone(event: &Event, devices: &[crate::model::Device]) -> String {
    match &event.device_id {
        Some(id) if id == pocketdesk_core::local::home_assistant::HOME_ASSISTANT_DEVICE => {
            "Home Assistant".to_string()
        }
        Some(id) if devices.iter().any(|d| &d.id == id && d.name.is_some()) => {
            crate::model::device_label(devices, id)
        }
        Some(id) if id.chars().count() > 8 => id.chars().take(8).collect(),
        Some(id) => id.clone(),
        None => String::new(),
    }
}

fn text<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event
        .fields
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn number(event: &Event, key: &str) -> Option<i64> {
    event.fields.get(key).and_then(Value::as_i64)
}

/// A size as every page says it (E5: Files and Activity had told one
/// file's size two ways).
fn size(bytes: i64) -> String {
    crate::words::human_size(bytes.max(0) as u64)
}

/// What happened, in words.
pub fn describe(event: &Event) -> String {
    let flag = |key: &str| event.fields.get(key).and_then(Value::as_bool) == Some(true);
    match event.event.as_str() {
        "session_started" => {
            let what = match text(event, "mode") {
                Some("cast") => "Showing its screen or camera here",
                Some("remote") => "Using this computer as a remote",
                Some("sound") => "Being this computer's speaker and mic",
                _ => "Viewing this computer",
            };
            if text(event, "transport") == Some("wan") {
                format!("{what} (over the internet)")
            } else {
                what.to_string()
            }
        }
        "session_ended" => "Session ended".to_string(),
        "session_refused" => "Session refused".to_string(),
        "phone_linked" => "Connected".to_string(),
        "phone_unlinked" => "Disconnected".to_string(),
        "phone_paired" => "Paired".to_string(),
        "phone_unpaired" => "Unpaired".to_string(),
        "phone_battery" => format!(
            "Battery {} %{}",
            number(event, "charge").unwrap_or(0),
            if flag("charging") { ", charging" } else { "" }
        ),
        "phone_battery_low" => format!("Battery low: {} %", number(event, "charge").unwrap_or(0)),
        "away_locked" => if flag("failed") {
            "Left, but locking this computer failed"
        } else {
            "Left: this computer was locked"
        }
        .to_string(),
        "phone_quiet" => if flag("quiet") {
            "Do Not Disturb on"
        } else {
            "Do Not Disturb off"
        }
        .to_string(),
        "notification" => {
            let mut line = format!(
                "Notification from {}",
                text(event, "app").unwrap_or("an app")
            );
            if let Some(title) = text(event, "title") {
                line.push_str(&format!(": {title}"));
            }
            // M6: why no toast showed it (M6 review).
            if event.fields.get("phone_in_use").and_then(Value::as_bool) == Some(true) {
                line.push_str(" (not shown: the phone was in use)");
            }
            line
        }
        "text_received" => match text(event, "from") {
            Some(from) => format!("Text from {from}"),
            None => "A text".to_string(),
        },
        "file_received" if flag("folder") => match text(event, "name") {
            Some(name) => format!("Folder arriving: {}", name.trim_end_matches('/')),
            None => "Folder arriving".to_string(),
        },
        "file_received" => {
            let size = number(event, "size").map(size).unwrap_or_default();
            match text(event, "name") {
                Some(name) => format!("File received: {name} ({size})"),
                None => format!("File received ({size})"),
            }
        }
        "link_received" => match (text(event, "url"), text(event, "host")) {
            (Some(url), _) => format!("Link: {url}"),
            (None, Some(host)) => format!("Link from {host}"),
            _ => "A link".to_string(),
        },
        "hook_ran" => {
            let hook = text(event, "hook").unwrap_or("an event");
            if flag("timed_out") {
                format!("Hook for {hook} was stopped after 30 s")
            } else {
                match number(event, "code") {
                    Some(code) => format!("Hook for {hook} ran (exit {code})"),
                    None => format!("Hook for {hook} could not run, or a signal ended it"),
                }
            }
        }
        "task_started" => format!("Task \"{}\" started", text(event, "task").unwrap_or("?")),
        "task_finished" => {
            let task = text(event, "task").unwrap_or("?");
            if flag("timed_out") {
                format!("Task \"{task}\" was stopped at its deadline")
            } else {
                match number(event, "code") {
                    Some(code) => format!("Task \"{task}\" finished (exit {code})"),
                    None => format!("Task \"{task}\" could not run"),
                }
            }
        }
        "task_refused" => {
            let task = text(event, "task").unwrap_or("?");
            match text(event, "reason") {
                Some("denied") => format!("Task \"{task}\" denied"),
                Some("busy") => {
                    format!("Task \"{task}\" refused: already running, or two tasks are")
                }
                _ => format!("Task \"{task}\" refused: no such task"),
            }
        }
        // H1: a deck press (F1), by its label when details are kept.
        "deck_pressed" => {
            // One noun per kind (H1 review).
            let kind = match text(event, "kind") {
                Some("keys") => "keys",
                Some("task") => "a task",
                Some("media") => "a media key",
                Some("link") => "a link",
                Some("text") => "typed text",
                _ => "?",
            };
            match (text(event, "label"), flag("ok")) {
                (Some(label), true) => format!("Deck \"{label}\" pressed ({kind})"),
                (Some(label), false) => format!("Deck \"{label}\" could not run ({kind})"),
                (None, true) => format!("A deck button pressed ({kind})"),
                (None, false) => format!("A deck button could not run ({kind})"),
            }
        }
        other => other.replace('_', " "),
    }
}

/// The file or folder a `file_received` event was, from the received
/// list: the row whose saved path has the event's key.
fn file_path(
    event: &Event,
    received: &[pocketdesk_core::local::received::ReceivedFile],
) -> Option<PathBuf> {
    if event.event != "file_received" {
        return None;
    }
    let key = text(event, "key")?;
    received
        .iter()
        .find(|r| pocketdesk_core::local::received::path_key(&r.path) == key)
        .map(|r| PathBuf::from(&r.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::events::EventsDetail;

    /// H1: a deck press reads as one, under Tasks, deck and hooks.
    #[test]
    fn a_deck_press_reads_as_a_sentence() {
        assert!(Filter::Hooks.admits("deck_pressed"));
        assert_eq!(
            describe(&event("deck_pressed", json!({"kind": "keys", "ok": true}))),
            "A deck button pressed (keys)"
        );
        assert_eq!(
            describe(&event("deck_pressed", json!({"kind": "text", "ok": true}))),
            "A deck button pressed (typed text)"
        );
        assert_eq!(
            describe(&event("deck_pressed", json!({"kind": "task", "ok": false}))),
            "A deck button could not run (a task)"
        );
        let mut named = event("deck_pressed", json!({"kind": "media", "ok": true}));
        named.fields.insert("label".into(), json!("Pause"));
        assert_eq!(describe(&named), "Deck \"Pause\" pressed (a media key)");
    }

    /// J4: Search narrows by the line as shown, with the kind chip.
    #[test]
    fn a_search_finds_events_by_their_line() {
        let devices = [crate::model::Device {
            id: "0123456789ab".to_string(),
            name: Some("Pixel 8".to_string()),
            ..Default::default()
        }];
        let mut linked = event("phone_linked", json!({}));
        linked.at = 1;
        let mut link = event("link_received", json!({"host": "example.org"}));
        link.at = 2;
        let mut unpaired = event("phone_unpaired", json!({}));
        unpaired.at = 3;
        let events = [linked.clone(), link.clone(), unpaired.clone()];
        let kinds = |found: Vec<&Event>| found.iter().map(|e| e.event.clone()).collect::<Vec<_>>();
        assert_eq!(
            kinds(shown(&events, Filter::All, "", &devices)),
            ["phone_unpaired", "link_received", "phone_linked"],
            "newest first"
        );
        assert_eq!(
            kinds(shown(&events, Filter::All, "pixel CONNECTED", &devices)),
            ["phone_linked"]
        );
        assert_eq!(
            kinds(shown(&events, Filter::All, "example.org", &devices)),
            ["link_received"]
        );
        assert_eq!(
            kinds(shown(&events, Filter::Phones, "pixel", &devices)),
            ["phone_unpaired", "phone_linked"],
            "with the chip"
        );
        assert!(shown(&events, Filter::FilesAndLinks, "paired", &devices).is_empty());
    }

    /// K2b: a notification's event names its app for "Not from this app".
    #[test]
    fn a_notification_names_its_app_to_mute() {
        let note = event("notification", json!({"app": "Gmail"}));
        assert_eq!(notifying_app(&note), Some("Gmail"));
        assert_eq!(
            notifying_app(&event("notification", json!({"app": "  "}))),
            None
        );
        assert_eq!(
            notifying_app(&event("phone_linked", json!({"app": "Gmail"}))),
            None
        );
    }

    #[test]
    fn a_lock_when_a_phone_left_shows_under_phones() {
        assert!(Filter::Phones.admits("away_locked"));
        assert!(Filter::Phones.admits("phone_linked"));
        assert!(!Filter::Sessions.admits("away_locked"));
    }
    use pocketdesk_core::local::received::ReceivedFile;
    use serde_json::json;

    fn event(kind: &str, meta: Value) -> Event {
        Event::new(
            kind,
            Some("0123456789ab"),
            100,
            meta,
            json!({}),
            EventsDetail::Metadata,
        )
    }

    #[test]
    fn each_event_reads_as_a_sentence() {
        assert_eq!(
            describe(&event(
                "session_started",
                json!({"mode": "cast", "transport": "wan"})
            )),
            "Showing its screen or camera here (over the internet)"
        );
        assert_eq!(
            describe(&event(
                "phone_battery",
                json!({"charge": 54, "charging": true})
            )),
            "Battery 54 %, charging"
        );
        assert_eq!(
            describe(&event("phone_quiet", json!({"quiet": true}))),
            "Do Not Disturb on"
        );
        assert_eq!(
            describe(&event("file_received", json!({"size": 2_100_000}))),
            "File received (2.0 MB)"
        );
        assert_eq!(
            describe(&event("file_received", json!({"folder": true}))),
            "Folder arriving"
        );
        assert_eq!(
            describe(&event("link_received", json!({"host": "example.org"}))),
            "Link from example.org"
        );
        assert_eq!(
            describe(&event("notification", json!({"app": "Signal"}))),
            "Notification from Signal"
        );
        assert_eq!(
            describe(&event(
                "notification",
                json!({"app": "Signal", "phone_in_use": true})
            )),
            "Notification from Signal (not shown: the phone was in use)"
        );
        assert_eq!(describe(&event("text_received", json!({}))), "A text");
        assert_eq!(
            describe(&event(
                "hook_ran",
                json!({"hook": "file_received", "code": 0, "timed_out": false})
            )),
            "Hook for file_received ran (exit 0)"
        );
        assert_eq!(
            describe(&event(
                "hook_ran",
                json!({"hook": "x", "code": null, "timed_out": true})
            )),
            "Hook for x was stopped after 30 s"
        );
        assert_eq!(
            describe(&event(
                "task_finished",
                json!({"task": "Back up", "code": 0, "timed_out": false})
            )),
            "Task \"Back up\" finished (exit 0)"
        );
        assert_eq!(
            describe(&event(
                "task_refused",
                json!({"task": "Back up", "reason": "denied"})
            )),
            "Task \"Back up\" denied"
        );
        assert_eq!(
            describe(&event("a_future_event", json!({}))),
            "a future event"
        );
    }

    #[test]
    fn content_shows_when_the_log_has_it() {
        let full = Event::new(
            "file_received",
            None,
            1,
            json!({"size": 900}),
            json!({"name": "a.txt"}),
            EventsDetail::Full,
        );
        assert_eq!(describe(&full), "File received: a.txt (900 B)");
    }

    #[test]
    fn each_filter_shows_its_own() {
        assert!(Filter::Sessions.admits("session_refused"));
        assert!(Filter::Phones.admits("phone_battery_low"));
        assert!(!Filter::Phones.admits("file_received"));
        assert!(Filter::FilesAndLinks.admits("link_received"));
        assert!(Filter::Messages.admits("notification"));
        assert!(Filter::Hooks.admits("hook_ran"));
        assert!(Filter::Hooks.admits("task_finished"));
        assert!(Filter::All.admits("anything"));
    }

    #[test]
    fn a_received_file_is_found_for_its_row() {
        let row = |name: &str, size: u64, at: u64| ReceivedFile {
            name: name.into(),
            path: format!("/d/{name}"),
            size,
            received_unix: at,
            from: "0123456789ab".into(),
        };
        let key = pocketdesk_core::local::received::path_key;
        // Two files of one size at once, and a folder that went on arriving
        // long after it started: each found by its own key.
        let received = vec![
            row("a.txt", 900, 100),
            row("b.txt", 900, 100),
            row("Photos/", 0, 400),
        ];
        let file = event(
            "file_received",
            json!({"size": 900, "key": key("/d/b.txt")}),
        );
        assert_eq!(file_path(&file, &received), Some(PathBuf::from("/d/b.txt")));
        let folder = event(
            "file_received",
            json!({"folder": true, "key": key("/d/Photos/")}),
        );
        assert_eq!(
            file_path(&folder, &received),
            Some(PathBuf::from("/d/Photos/"))
        );
        let gone = event(
            "file_received",
            json!({"size": 900, "key": key("/d/gone.txt")}),
        );
        assert_eq!(file_path(&gone, &received), None, "no longer in the list");
        let old = event("file_received", json!({"size": 900}));
        assert_eq!(
            file_path(&old, &received),
            None,
            "an event from before keys"
        );
    }

    #[test]
    fn the_timeline_keeps_the_newest_500() {
        let dir = std::env::temp_dir().join(format!("pd-activity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for at in 0..520u64 {
            events::append(
                &dir,
                &Event::new(
                    "session_ended",
                    None,
                    at,
                    json!({}),
                    json!({}),
                    EventsDetail::Metadata,
                ),
            )
            .unwrap();
        }
        let mut view = ActivityView::default();
        view.refresh(&dir);
        assert_eq!(view.events.len(), KEEP);
        assert_eq!(view.events.first().map(|e| e.at), Some(20));
        events::append(
            &dir,
            &Event::new(
                "session_ended",
                None,
                999,
                json!({}),
                json!({}),
                EventsDetail::Metadata,
            ),
        )
        .unwrap();
        view.refresh(&dir);
        assert_eq!(view.events.len(), KEEP);
        assert_eq!(view.events.last().map(|e| e.at), Some(999));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_the_tab_shows_each_event_once() {
        let dir = std::env::temp_dir().join(format!("pd-activity-once-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ended = |at: u64| {
            Event::new(
                "session_ended",
                None,
                at,
                json!({}),
                json!({}),
                EventsDetail::Metadata,
            )
        };
        events::append(&dir, &ended(1)).unwrap();
        // The tail starts, then an event lands, then the history is read.
        let tail = Tail::at_end(&dir);
        events::append(&dir, &ended(2)).unwrap();
        let mut view = ActivityView {
            tail: Some(tail),
            events: events::history(&dir, KEEP),
            fresh: true,
            ..Default::default()
        };
        view.refresh(&dir);
        let ats: Vec<u64> = view.events.iter().map(|e| e.at).collect();
        assert_eq!(ats, vec![1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
