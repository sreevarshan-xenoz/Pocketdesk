//! The Settings tab: edit `config.toml` (the server reads it at startup).

use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use pocketdesk_core::local::config::{
    Config, ConfigError, DhtPublish, InputControlMode, relay_url_problem,
};
use pocketdesk_core::local::events::{EventsDetail, KINDS, hookable};
use pocketdesk_core::local::home_assistant as ha;
use pocketdesk_core::local::tasks::{
    DEFAULT_DEADLINE_SECS, MAX_DEADLINE_SECS, TaskSpec, name_problem,
};
use pocketdesk_core::local::wake::WakeFacts;
use pocketdesk_core::streaming::{CaptureBackend, VideoQualityPreset};

/// The settings being edited against what is saved.
pub struct SettingsForm {
    saved: Config,
    pub edited: Config,
    /// The text box behind `mic_relay_device` (blank means the default).
    pub mic_device_text: String,
    /// The text box behind `relay_urls`: one per line, or comma separated.
    pub relay_urls_text: String,
    /// The text box behind `[hooks]`: `kind = command`, one per line.
    pub hooks_text: String,
    /// Saved since the server last read it.
    pub restart_needed: bool,
    /// The adapter a phone on this network would wake this computer
    /// through, read when the tab opens (N6).
    wake: Option<WakeFacts>,
    /// The Home Assistant bridge's status as last read, and when (it is
    /// read again every 2 s, not every frame).
    home_assistant: Option<(std::time::Instant, Option<ha::Status>)>,
}

impl SettingsForm {
    pub fn from_config(config: Config) -> Self {
        Self {
            mic_device_text: config.mic_relay_device.clone().unwrap_or_default(),
            relay_urls_text: config.relay_urls.join("\n"),
            hooks_text: hooks_text(&config.hooks),
            saved: config.clone(),
            edited: config,
            restart_needed: false,
            wake: pocketdesk_core::local::wake::wake_facts(None),
            home_assistant: None,
        }
    }

    /// Copy the free-text fields into the config being edited.
    pub fn apply_text_fields(&mut self) {
        let text = self.mic_device_text.trim();
        self.edited.mic_relay_device = (!text.is_empty()).then(|| text.to_string());
        self.edited.relay_urls = self
            .relay_urls_text
            .split(['\n', ','])
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .collect();
        self.edited.hooks = parse_hooks(&self.hooks_text).0;
    }

    pub fn dirty(&self) -> bool {
        self.edited != self.saved
    }

    /// What stops the edit from being saved, in words for the user.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, port) in [
            ("control", self.edited.control_port),
            ("streaming", self.edited.streaming_port),
        ] {
            if port < 1024 {
                problems.push(format!(
                    "The {name} port must be 1024 or higher (lower ports need administrator rights)."
                ));
            }
        }
        if self.edited.control_port == self.edited.streaming_port {
            problems.push("The control and streaming ports must be different.".to_string());
        }
        if self.edited.wake_port != 0 && self.edited.wake_port == self.edited.streaming_port {
            problems.push(
                "The outside wake port must differ from the streaming port, so the two router forwards do not clash."
                    .to_string(),
            );
        }
        if self.edited.input_control == InputControlMode::One
            && !(3..=120).contains(&self.edited.input_idle_secs)
        {
            problems.push("Control is handed back after 3 to 120 seconds.".to_string());
        }
        for url in &self.edited.relay_urls {
            if let Some(problem) = relay_url_problem(url) {
                problems.push(format!("Relay: {problem}."));
            }
        }
        for problem in parse_hooks(&self.hooks_text).1 {
            problems.push(format!("Hooks: {problem}."));
        }
        problems.extend(task_problems(&self.edited.tasks));
        if self.edited.home_assistant
            && let Err(why) = ha::parse_broker(&self.edited.mqtt_broker)
        {
            problems.push(format!("Home Assistant: {why}."));
        }
        problems
    }

    /// The bridge's status, read again when 2 s old.
    fn home_assistant_status(&mut self) -> Option<ha::Status> {
        let stale = self
            .home_assistant
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(2));
        if stale {
            // A file a stopped server left says nothing about now.
            let status = pocketdesk_core::local::paths::state_dir()
                .ok()
                .filter(|dir| pocketdesk_core::local::server::is_running(dir))
                .and_then(|dir| ha::read_status(&dir));
            self.home_assistant = Some((std::time::Instant::now(), status));
        }
        self.home_assistant.as_ref().and_then(|(_, s)| s.clone())
    }

    /// Save the edit to `path` if it is valid.
    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        self.apply_text_fields();
        let problems = self.problems();
        if !problems.is_empty() {
            return Err(problems.join(" "));
        }
        self.edited
            .save_to(path)
            .map_err(|e: ConfigError| e.to_string())?;
        // The server reads the event log's settings and hooks again at the
        // next event; only the others wait for a restart.
        if !same_but_events(&self.saved, &self.edited) {
            self.restart_needed = true;
        }
        self.saved = self.edited.clone();
        Ok(())
    }

    /// Draw the form. Returns true when the user asked to save.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let home_assistant = if self.saved.home_assistant {
            self.home_assistant_status()
        } else {
            None
        };
        let e = &mut self.edited;
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([24.0, 10.0])
            .show(ui, |ui| {
                ui.label("Control port (TCP)");
                ui.add(egui::DragValue::new(&mut e.control_port).range(1..=65535));
                ui.end_row();
                ui.label("Streaming port (UDP)");
                ui.add(egui::DragValue::new(&mut e.streaming_port).range(1..=65535));
                ui.end_row();
                ui.label("Require pairing");
                ui.checkbox(&mut e.pairing_required, "only paired phones may connect");
                ui.end_row();
                ui.label("Visible on this network");
                ui.checkbox(
                    &mut e.lan_discovery,
                    "phones on the same network list this computer",
                );
                ui.end_row();
                ui.label("Video quality");
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(format!("{:?}", e.video_quality))
                    .show_ui(ui, |ui| {
                        for preset in [
                            VideoQualityPreset::Low,
                            VideoQualityPreset::Balanced,
                            VideoQualityPreset::High,
                        ] {
                            let label = format!("{preset:?}");
                            ui.selectable_value(&mut e.video_quality, preset, label);
                        }
                    });
                ui.end_row();
                ui.label("H.265 (HEVC)");
                ui.checkbox(&mut e.allow_hevc, "use it with phones that can decode it");
                ui.end_row();
                ui.label("Screen capture");
                egui::ComboBox::from_id_salt("capture")
                    .selected_text(e.capture_backend.config_key())
                    .show_ui(ui, |ui| {
                        for backend in CaptureBackend::ALL {
                            ui.selectable_value(
                                &mut e.capture_backend,
                                *backend,
                                backend.config_key(),
                            );
                        }
                    });
                ui.end_row();
                ui.label("Log level");
                egui::ComboBox::from_id_salt("log")
                    .selected_text(e.log_level.clone())
                    .show_ui(ui, |ui| {
                        for level in ["error", "warn", "info", "debug", "trace"] {
                            ui.selectable_value(&mut e.log_level, level.to_string(), level);
                        }
                    });
                ui.end_row();
                ui.label("Your own relays");
                ui.add(
                    egui::TextEdit::multiline(&mut self.relay_urls_text)
                        .desired_rows(2)
                        .hint_text("blank: n0's public relays\nhttps://relay.example.org"),
                );
                ui.end_row();
                ui.label("Found by id (DHT)");
                egui::ComboBox::from_id_salt("dht")
                    .selected_text(dht_label(e.dht_publish))
                    .show_ui(ui, |ui| {
                        for mode in [DhtPublish::Relay, DhtPublish::Addresses, DhtPublish::Off] {
                            ui.selectable_value(&mut e.dht_publish, mode, dht_label(mode));
                        }
                    });
                ui.end_row();
                ui.label("Phone notifications");
                ui.checkbox(
                    &mut e.phone_notifications,
                    "show the notifications phones share",
                );
                ui.end_row();
                ui.label("At most, per phone");
                ui.add(
                    egui::DragValue::new(&mut e.notifications_per_minute)
                        .range(1..=600)
                        .suffix(" a minute"),
                );
                ui.end_row();
                ui.label("Texts");
                ui.checkbox(&mut e.sms_notify, "a notification for each text");
                ui.end_row();
                ui.label("");
                ui.checkbox(
                    &mut e.sms_names,
                    "keep the contact names phones send with texts (off forgets those kept, when the server restarts)",
                );
                ui.end_row();
                ui.label("Clipboard to phones");
                ui.checkbox(
                    &mut e.clipboard_push,
                    "send what I copy to connected phones (those with Clipboard on)",
                );
                ui.end_row();
                ui.label("Phones may add a screen");
                ui.checkbox(
                    &mut e.virtual_screens,
                    "a phone with Input can add a screen of its own shape (Sway)",
                );
                ui.end_row();
                ui.label("Links from phones");
                ui.checkbox(
                    &mut e.open_links,
                    "open in the browser (off: show as a notification)",
                );
                ui.end_row();
                ui.label("Phones' batteries");
                ui.checkbox(
                    &mut e.phone_battery_alerts,
                    "a notification when a phone runs low",
                );
                ui.end_row();
                ui.label("Event log");
                ui.checkbox(
                    &mut e.events,
                    "keep a log of what happens, for `pocketdesk events` and scripts",
                );
                ui.end_row();
                ui.label("");
                let mut full = e.events_detail == EventsDetail::Full;
                if ui
                    .checkbox(
                        &mut full,
                        "details in events: notification and text content, file names, whole links",
                    )
                    .changed()
                {
                    e.events_detail = if full {
                        EventsDetail::Full
                    } else {
                        EventsDetail::Metadata
                    };
                }
                ui.end_row();
                ui.label("Hooks");
                ui.vertical(|ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.hooks_text)
                            .desired_rows(3)
                            .hint_text("file_received = notify-send \"A file came\""),
                    );
                    ui.label(
                        RichText::new(format!(
                            "One per line: an event, \"=\", a command. It runs as you, with the event on stdin. Events: {}.",
                            KINDS
                                .iter()
                                .copied()
                                .filter(|k| hookable(k))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .small()
                        .weak(),
                    );
                    let unknown = unknown_hook_kinds(&parse_hooks(&self.hooks_text).0)
                        .into_iter()
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    if !unknown.is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "No such event here, so never run: {}.",
                                unknown.join(", ")
                            ))
                            .small()
                            .color(Color32::from_rgb(0xc0, 0x80, 0x00)),
                        );
                    }
                });
                ui.end_row();
                ui.label("Tasks");
                ui.vertical(|ui| {
                    let mut remove = None;
                    for (i, task) in e.tasks.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut task.name)
                                    .desired_width(110.0)
                                    .hint_text("name"),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut task.command)
                                    .desired_width(240.0)
                                    .hint_text("command"),
                            );
                            ui.checkbox(&mut task.ask, "ask first");
                            ui.add(
                                egui::DragValue::new(&mut task.deadline_secs)
                                    .range(1..=MAX_DEADLINE_SECS)
                                    .suffix(" s"),
                            )
                            .on_hover_text("Stopped if still running after this.");
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        e.tasks.remove(i);
                    }
                    if ui.button("Add task").clicked() {
                        e.tasks.push(TaskSpec {
                            name: String::new(),
                            command: String::new(),
                            ask: true,
                            deadline_secs: DEFAULT_DEADLINE_SECS,
                        });
                    }
                    ui.label(
                        RichText::new(
                            "A phone with \"Start my tasks\" sees the names (never the commands) and can start one. It runs as you; \"ask first\" waits for your yes here.",
                        )
                        .small()
                        .weak(),
                    );
                });
                ui.end_row();
                ui.label("Home Assistant");
                ui.vertical(|ui| {
                    ui.checkbox(
                        &mut e.home_assistant,
                        "publish this computer, its phones and its tasks through an MQTT broker",
                    );
                    if e.home_assistant {
                        egui::Grid::new("home_assistant").show(ui, |ui| {
                            ui.label("Broker");
                            ui.add(
                                egui::TextEdit::singleline(&mut e.mqtt_broker)
                                    .desired_width(240.0)
                                    .hint_text("mqtt://homeassistant.local:1883"),
                            );
                            ui.end_row();
                            ui.label("User");
                            ui.add(
                                egui::TextEdit::singleline(&mut e.mqtt_user)
                                    .desired_width(160.0)
                                    .hint_text("none"),
                            );
                            ui.end_row();
                            ui.label("Password");
                            ui.add(
                                egui::TextEdit::singleline(&mut e.mqtt_password)
                                    .password(true)
                                    .desired_width(160.0),
                            );
                            ui.end_row();
                        });
                    }
                    let note = match (&home_assistant, self.saved.home_assistant) {
                        (Some(status), true) if status.connected => match &status.error {
                            Some(why) => format!("Connected to the broker, but {why}."),
                            None => "Connected to the broker.".to_string(),
                        },
                        (Some(status), true) => format!(
                            "Not connected: {}",
                            status.error.as_deref().unwrap_or("trying")
                        ),
                        (None, true) => "Not connected yet (is the server running?)".to_string(),
                        _ => "Battery, connection, sessions and an Activity entity appear in Home Assistant by MQTT discovery, and a button per task (one that asks first still asks here). No notification, text, file name or link goes to the broker; the password is kept in config.toml, readable only by you.".to_string(),
                    };
                    ui.label(RichText::new(note).small().weak());
                });
                ui.end_row();
                ui.label("Messages history");
                ui.add(
                    egui::DragValue::new(&mut e.sms_keep)
                        .range(50..=100_000)
                        .suffix(" texts"),
                );
                ui.end_row();
                ui.label("Wake from the phone");
                ui.checkbox(
                    &mut e.wake_announce,
                    "tell paired phones how to wake this computer",
                );
                ui.end_row();
                ui.label("Adapter");
                ui.label(match &self.wake {
                    Some(w) => format!(
                        "{} ({}), {}",
                        w.adapter,
                        if w.wired {
                            "wired"
                        } else {
                            "Wi-Fi: usually wakes only from sleep"
                        },
                        w.mac
                    ),
                    None => "none found with a hardware address".to_string(),
                });
                ui.end_row();
                ui.label("Outside wake port");
                ui.vertical(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut e.wake_port)
                            .range(0..=65535)
                            .custom_formatter(|n, _| {
                                if n == 0.0 {
                                    "none".to_string()
                                } else {
                                    format!("{n}")
                                }
                            }),
                    );
                    ui.label(
                        RichText::new(
                            "To wake it from outside, forward this UDP port on the router to this computer's address. \
                             It reaches the computer while the router still remembers it (minutes to hours after it sleeps), \
                             or always when the router can forward to the network's broadcast address.",
                        )
                        .small()
                        .weak(),
                    );
                });
                ui.end_row();
                ui.label("Unfinished files");
                ui.vertical(|ui| {
                    egui::ComboBox::from_id_salt("transfer-keep")
                        .selected_text(keep_label(e.transfer_keep_hours))
                        .show_ui(ui, |ui| {
                            for hours in [0, 1, 24] {
                                ui.selectable_value(
                                    &mut e.transfer_keep_hours,
                                    hours,
                                    keep_label(hours),
                                );
                            }
                        });
                    ui.label(
                        RichText::new(
                            "A file from a phone that stops part way is kept this long, so the phone's retry continues it.",
                        )
                        .small()
                        .weak(),
                    );
                });
                ui.end_row();
                ui.label("Several phones");
                egui::ComboBox::from_id_salt("input-control")
                    .selected_text(input_control_label(e.input_control))
                    .show_ui(ui, |ui| {
                        for mode in [InputControlMode::One, InputControlMode::Shared] {
                            ui.selectable_value(
                                &mut e.input_control,
                                mode,
                                input_control_label(mode),
                            );
                        }
                    });
                ui.end_row();
                ui.label("Hand control back after");
                ui.add_enabled(
                    e.input_control == InputControlMode::One,
                    egui::DragValue::new(&mut e.input_idle_secs)
                        .range(3..=120)
                        .suffix(" s without input"),
                );
                ui.end_row();
                if cfg!(windows) {
                    ui.label("Phone mic plays into");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mic_device_text)
                            .hint_text("CABLE Input (VB-CABLE)"),
                    );
                    ui.end_row();
                }
            });
        self.apply_text_fields();
        ui.add_space(10.0);
        for problem in self.problems() {
            ui.label(RichText::new(problem).color(Color32::from_rgb(0xd9, 0x5c, 0x4a)));
        }
        let can_save = self.dirty() && self.problems().is_empty();
        let mut save = false;
        ui.horizontal(|ui| {
            save = ui
                .add_enabled(can_save, egui::Button::new("Save"))
                .clicked();
            if ui
                .add_enabled(self.dirty(), egui::Button::new("Undo changes"))
                .clicked()
            {
                *self = Self::from_config(self.saved.clone());
            }
        });
        save
    }
}

/// How the several-phones choice reads in the form.
/// How the "Unfinished files" choice reads.
fn keep_label(hours: u32) -> String {
    match hours {
        0 => "not kept: a retry starts over".to_string(),
        1 => "kept 1 hour".to_string(),
        h => format!("kept {h} hours"),
    }
}

fn input_control_label(mode: InputControlMode) -> &'static str {
    match mode {
        InputControlMode::One => "one phone drives at a time",
        InputControlMode::Shared => "every phone drives",
    }
}

/// How the DHT choice reads in the form.
fn dht_label(mode: DhtPublish) -> &'static str {
    match mode {
        DhtPublish::Relay => "publish the relay only",
        DhtPublish::Addresses => "publish the relay and public addresses",
        DhtPublish::Off => "off",
    }
}

/// The hooks for events this version does not have: kept (a newer server
/// may), but said, as a mistyped name would never run.
fn unknown_hook_kinds(hooks: &std::collections::BTreeMap<String, String>) -> Vec<&str> {
    hooks
        .keys()
        .map(String::as_str)
        .filter(|kind| !hookable(kind))
        .collect()
}

/// What is wrong with the tasks, in words for the user.
fn task_problems(tasks: &[TaskSpec]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for task in tasks {
        if let Some(problem) = name_problem(&task.name) {
            problems.push(format!("Tasks: {problem}."));
        } else if !seen.insert(task.name.trim().to_string()) {
            problems.push(format!("Tasks: two are named \"{}\".", task.name.trim()));
        }
        if task.command.trim().is_empty() {
            problems.push(format!("Tasks: \"{}\" has no command.", task.name.trim()));
        }
    }
    problems
}

/// Whether `a` and `b` differ only in what the server reads at each event.
fn same_but_events(a: &Config, b: &Config) -> bool {
    let strip = |c: &Config| Config {
        events: true,
        events_detail: EventsDetail::Metadata,
        hooks: Default::default(),
        // Tasks are read at each start, as events are.
        tasks: Vec::new(),
        // The Home Assistant bridge reads its settings every 2 s.
        home_assistant: false,
        mqtt_broker: String::new(),
        mqtt_user: String::new(),
        mqtt_password: String::new(),
        ..c.clone()
    };
    strip(a) == strip(b)
}

/// `[hooks]` as the text box shows it.
fn hooks_text(hooks: &std::collections::BTreeMap<String, String>) -> String {
    hooks
        .iter()
        .map(|(kind, command)| format!("{kind} = {command}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The hooks in the text box, and what is wrong with its lines.
fn parse_hooks(text: &str) -> (std::collections::BTreeMap<String, String>, Vec<String>) {
    let mut hooks = std::collections::BTreeMap::new();
    let mut problems = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((kind, command)) = line.split_once('=') else {
            problems.push(format!("\"{line}\" has no \"=\""));
            continue;
        };
        let (kind, command) = (kind.trim(), command.trim());
        if kind.is_empty() {
            problems.push(format!("\"{line}\" names no event"));
        } else if command.is_empty() {
            problems.push(format!("{kind} has no command"));
        } else if hooks
            .insert(kind.to_string(), command.to_string())
            .is_some()
        {
            problems.push(format!("{kind} is given twice"));
        }
    }
    (hooks, problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::config::Config;

    #[test]
    fn hooks_are_one_event_and_command_per_line() {
        let mut form = SettingsForm::from_config(Config::default());
        form.hooks_text =
            " file_received = notify-send \"a = b\" \n\nsession_started=echo hi".into();
        form.apply_text_fields();
        assert!(form.problems().is_empty(), "{:?}", form.problems());
        assert_eq!(
            form.edited.hooks.get("file_received").map(String::as_str),
            Some("notify-send \"a = b\""),
            "the command keeps its own equals signs"
        );
        assert_eq!(form.edited.hooks.len(), 2);
        let back = SettingsForm::from_config(form.edited.clone());
        assert_eq!(parse_hooks(&back.hooks_text).0, form.edited.hooks);
        // An event this version does not know is kept, and said.
        form.hooks_text = "a_newer_event = echo".into();
        assert!(form.problems().is_empty());
        form.apply_text_fields();
        assert_eq!(
            unknown_hook_kinds(&form.edited.hooks),
            vec!["a_newer_event"]
        );
        // A hook's own report has none: said as never run.
        form.hooks_text = "hook_ran = echo".into();
        form.apply_text_fields();
        assert_eq!(unknown_hook_kinds(&form.edited.hooks), vec!["hook_ran"]);
        for bad in [
            "= x",
            "file_received",
            "file_received =",
            "link_received = a\nlink_received = b",
        ] {
            form.hooks_text = bad.into();
            assert!(
                form.problems().iter().any(|p| p.starts_with("Hooks:")),
                "{bad:?} passed"
            );
        }
    }

    #[test]
    fn relays_are_read_one_per_line_or_comma_and_checked() {
        let mut form = SettingsForm::from_config(Config::default());
        form.relay_urls_text = " https://a.example.org \n\nhttp://192.0.2.1:3340, ".into();
        form.apply_text_fields();
        assert_eq!(
            form.edited.relay_urls,
            vec!["https://a.example.org", "http://192.0.2.1:3340"]
        );
        assert!(form.problems().is_empty());
        form.relay_urls_text = "relay.example.org:3340".into();
        form.apply_text_fields();
        assert!(form.problems().iter().any(|p| p.starts_with("Relay:")));
    }

    #[test]
    fn home_assistant_needs_a_broker_and_applies_without_a_restart() {
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.home_assistant = true;
        assert!(
            form.problems().iter().any(|p| p.contains("Home Assistant")),
            "no broker"
        );
        form.edited.mqtt_broker = "mqtts://broker.example.org".into();
        assert!(form.problems().iter().any(|p| p.contains("TLS")));
        form.edited.mqtt_broker = "mqtt://192.0.2.5".into();
        assert!(form.problems().is_empty(), "{:?}", form.problems());
        assert!(same_but_events(&Config::default(), &form.edited));
    }

    #[test]
    fn an_untouched_form_is_clean_and_valid() {
        let form = SettingsForm::from_config(Config::default());
        assert!(!form.dirty());
        assert_eq!(form.problems(), Vec::<String>::new());
    }

    #[test]
    fn ports_must_be_unprivileged_and_distinct() {
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.control_port = 80;
        form.edited.streaming_port = 80;
        let problems = form.problems();
        assert!(form.dirty());
        assert!(problems.iter().any(|p| p.contains("1024")), "{problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("different")),
            "{problems:?}"
        );
    }

    #[test]
    fn tasks_need_a_name_of_their_own_and_a_command() {
        let task = |name: &str, command: &str| TaskSpec {
            name: name.into(),
            command: command.into(),
            ask: true,
            deadline_secs: 60,
        };
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.tasks = vec![task("Back up", "backup.sh"), task("Build", "make")];
        assert!(form.problems().is_empty());
        form.edited.tasks.push(task("Back up", "other.sh"));
        assert!(form.problems().iter().any(|p| p.contains("two are named")));
        form.edited.tasks = vec![task("", "x"), task("Empty", " ")];
        let problems = form.problems();
        assert!(
            problems.iter().any(|p| p.contains("needs a name")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("has no command")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_event_settings_apply_without_a_restart() {
        let dir = std::env::temp_dir().join(format!("pd-settings-ev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut form = SettingsForm::from_config(Config::default());
        form.hooks_text = "file_received = echo".into();
        form.edited.events_detail = EventsDetail::Full;
        form.save_to(&path).unwrap();
        assert!(!form.restart_needed, "events apply at the next event");
        form.edited.allow_hevc = true;
        form.save_to(&path).unwrap();
        assert!(form.restart_needed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_writes_the_edit_and_marks_a_restart_needed() {
        let dir = std::env::temp_dir().join(format!("pd-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.allow_hevc = true;
        form.save_to(&path).unwrap();
        assert!(!form.dirty() && form.restart_needed);
        assert!(Config::load_from(&path).unwrap().allow_hevc);
        form.edited.control_port = 1;
        assert!(form.save_to(&path).is_err(), "an invalid form is not saved");
        assert!(!Config::load_from(&path).unwrap().control_port.eq(&1));
    }

    #[test]
    fn the_hand_back_time_is_3_to_120_seconds() {
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.input_idle_secs = 2;
        assert!(form.problems().iter().any(|p| p.contains("3 to 120")));
        form.edited.input_idle_secs = 120;
        assert!(form.problems().is_empty());
    }

    #[test]
    fn the_outside_wake_port_must_not_be_the_streaming_port() {
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.wake_port = 40009;
        assert!(form.problems().is_empty());
        form.edited.wake_port = form.edited.streaming_port;
        assert!(
            form.problems().iter().any(|p| p.contains("wake port")),
            "{:?}",
            form.problems()
        );
    }

    #[test]
    fn a_blank_mic_device_means_the_default() {
        let mut form = SettingsForm::from_config(Config::default());
        form.mic_device_text = "   ".into();
        form.apply_text_fields();
        assert_eq!(form.edited.mic_relay_device, None);
        form.mic_device_text = " CABLE-A Input ".into();
        form.apply_text_fields();
        assert_eq!(
            form.edited.mic_relay_device.as_deref(),
            Some("CABLE-A Input")
        );
    }
}
