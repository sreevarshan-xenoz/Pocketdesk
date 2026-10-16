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
        (Filter::Hooks, "Hooks"),
    ];

    fn admits(self, kind: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Sessions => kind.starts_with("session_"),
            Filter::Phones => kind.starts_with("phone_"),
            Filter::FilesAndLinks => matches!(kind, "file_received" | "link_received"),
            Filter::Messages => matches!(kind, "notification" | "text_received"),
            Filter::Hooks => kind == "hook_ran",
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
}

#[derive(Default)]
pub struct ActivityView {
    /// Oldest first, at most [`KEEP`].
    events: Vec<Event>,
    tail: Option<Tail>,
    filter: Filter,
}

impl ActivityView {
    /// Read what was written since the last look (the history, the first
    /// time).
    pub fn refresh(&mut self, state_dir: &Path) {
        match &mut self.tail {
            Some(tail) => self.events.extend(tail.poll()),
            None => {
                self.tail = Some(Tail::at_end(state_dir));
                self.events = events::history(state_dir, KEEP);
            }
        }
        let extra = self.events.len().saturating_sub(KEEP);
        self.events.drain(..extra);
    }

    /// Draw the tab; what the person asked for, if anything.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state_dir: &Path,
        log_on: bool,
        now_unix: u64,
    ) -> Option<Action> {
        let mut action = None;
        if !log_on {
            ui.horizontal(|ui| {
                ui.label("The event log is off, so nothing new shows here.");
                if ui.button("Settings").clicked() {
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
        });
        ui.add_space(6.0);
        let shown: Vec<&Event> = self
            .events
            .iter()
            .rev()
            .filter(|e| self.filter.admits(&e.event))
            .collect();
        if shown.is_empty() {
            ui.label("Nothing yet. Sessions, phones, files, links, notifications and hooks show here as they happen.");
            return action;
        }
        let received = pocketdesk_core::local::received::read(state_dir);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("activity")
                    .num_columns(4)
                    .striped(true)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        for event in shown {
                            ui.label(RichText::new(ago(now_unix, event.at)).weak());
                            ui.monospace(phone(event));
                            ui.label(describe(event));
                            match file_path(event, &received) {
                                Some(path) if ui.small_button("Show in folder").clicked() => {
                                    action = Some(Action::ShowFile(path));
                                }
                                _ => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        action
    }
}

/// "3 min ago", from `at` to `now`.
fn ago(now: u64, at: u64) -> String {
    match now.saturating_sub(at) {
        0..10 => "just now".to_string(),
        s @ 10..60 => format!("{s} s ago"),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s @ 3600..86_400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86_400),
    }
}

/// The phone, by the first characters of its id.
fn phone(event: &Event) -> String {
    match &event.device_id {
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

/// `2_100_000` -> "2.1 MB".
fn size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    match bytes {
        ..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.0} KB", b / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", b / 1e6),
        _ => format!("{:.1} GB", b / 1e9),
    }
}

/// What happened, in words.
pub fn describe(event: &Event) -> String {
    let flag = |key: &str| event.fields.get(key).and_then(Value::as_bool) == Some(true);
    match event.event.as_str() {
        "session_started" => {
            let what = match text(event, "mode") {
                Some("cast") => "Showing its screen here",
                Some("remote") => "Using this computer as a remote",
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
        "notification" => {
            let mut line = format!(
                "Notification from {}",
                text(event, "app").unwrap_or("an app")
            );
            if let Some(title) = text(event, "title") {
                line.push_str(&format!(": {title}"));
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
                    None => format!("Hook for {hook} could not run"),
                }
            }
        }
        other => other.replace('_', " "),
    }
}

/// The file a `file_received` event was, from the received list: the one
/// of its size kept within a few seconds of it.
fn file_path(
    event: &Event,
    received: &[pocketdesk_core::local::received::ReceivedFile],
) -> Option<PathBuf> {
    if event.event != "file_received" {
        return None;
    }
    let folder = event.fields.get("folder").and_then(Value::as_bool) == Some(true);
    let size = number(event, "size");
    received
        .iter()
        .find(|r| {
            r.received_unix.abs_diff(event.at) <= 5
                && r.from == event.device_id.clone().unwrap_or_default()
                && if folder {
                    r.name.ends_with('/')
                } else {
                    Some(r.size as i64) == size && !r.name.ends_with('/')
                }
        })
        .map(|r| PathBuf::from(&r.path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::events::EventsDetail;
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
            "Showing its screen here (over the internet)"
        );
        assert_eq!(
            describe(&event(
                "phone_battery",
                json!({"charge": 54, "charging": true})
            )),
            "Battery 54 %, charging"
        );
        assert_eq!(
            describe(&event("file_received", json!({"size": 2_100_000}))),
            "File received (2.1 MB)"
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
        let received = vec![
            row("other.bin", 5, 100),
            row("a.txt", 900, 102),
            row("Photos/", 0, 100),
        ];
        let file = event("file_received", json!({"size": 900}));
        assert_eq!(file_path(&file, &received), Some(PathBuf::from("/d/a.txt")));
        let folder = event("file_received", json!({"folder": true}));
        assert_eq!(
            file_path(&folder, &received),
            Some(PathBuf::from("/d/Photos/"))
        );
        let late = Event::new(
            "file_received",
            Some("0123456789ab"),
            200,
            json!({"size": 900}),
            json!({}),
            EventsDetail::Metadata,
        );
        assert_eq!(
            file_path(&late, &received),
            None,
            "not one from long before"
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
    fn time_reads_as_how_long_ago() {
        assert_eq!(ago(100, 95), "just now");
        assert_eq!(ago(200, 100), "1 min ago");
        assert_eq!(
            ago(100, 200),
            "just now",
            "a clock behind is not the future"
        );
        assert_eq!(ago(90_000, 0), "1 d ago");
    }
}
