//! The Settings tab: edit `config.toml` (the server reads it at startup).

use std::path::Path;

use eframe::egui::{self, RichText};
use pocketdesk_core::local::config::{
    Config, ConfigError, DhtPublish, InputControlMode, relay_url_problem,
};
use pocketdesk_core::local::deck::{DeckButton, MAX_BUTTONS as DECK_MAX_BUTTONS};
use pocketdesk_core::local::events::{EventsDetail, KINDS, hookable};
use pocketdesk_core::local::home_assistant as ha;
use pocketdesk_core::local::tasks::{
    DEFAULT_DEADLINE_SECS, MAX_DEADLINE_SECS, TaskSpec, name_problem,
};
use pocketdesk_core::local::wake::WakeFacts;
use pocketdesk_core::streaming::{CaptureBackend, VideoQualityPreset};

/// Settings' sections, in page order (U5): a flat grid of about 35 rows
/// had everyday choices beside developer ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Connection,
    Phones,
    Notifications,
    Wake,
    Automation,
    Computers,
    KdeConnect,
    Advanced,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Connection,
        Section::Phones,
        Section::Notifications,
        Section::Wake,
        Section::Automation,
        Section::Computers,
        Section::KdeConnect,
        Section::Advanced,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Connection => "Connection",
            Section::Phones => "Phones",
            Section::Notifications => "Notifications and texts",
            Section::Wake => "Wake",
            Section::Automation => "Automation",
            Section::Computers => "Other computers",
            Section::KdeConnect => "KDE Connect",
            Section::Advanced => "Advanced",
        }
    }

    /// What is in it, in a line beside the title.
    pub fn summary(self) -> &'static str {
        match self {
            Section::Connection => "ports, pairing, this network, files",
            Section::Phones => "video, control, clipboard, drive, locking, links, batteries",
            Section::Notifications => "phones' notifications and texts here",
            Section::Wake => "waking this computer from a phone",
            Section::Automation => "event log, hooks, tasks, Home Assistant",
            Section::Computers => "pairing and sharing with your other computers",
            Section::KdeConnect => "devices that run KDE Connect",
            Section::Advanced => "screen capture, logs, relays, the DHT",
        }
    }

    /// Open when the page first shows.
    pub fn opens(self) -> bool {
        matches!(self, Section::Connection | Section::Phones)
    }
}

/// Every setting the form edits: its key in config.toml, its name on the
/// page and its section. One table, so the save bar names a change as its
/// row does, and a test holds it to the config's keys (none lost in a move).
pub const SETTINGS: &[(&str, &str, Section)] = &[
    ("control_port", "Control port (TCP)", Section::Connection),
    (
        "streaming_port",
        "Streaming port (UDP)",
        Section::Connection,
    ),
    ("pairing_required", "Require pairing", Section::Connection),
    (
        "lan_discovery",
        "Visible on this network",
        Section::Connection,
    ),
    (
        "remember_screen_grant",
        "Remember the screen-sharing grant",
        Section::Connection,
    ),
    (
        "internet_files",
        "Files over the internet",
        Section::Connection,
    ),
    (
        "transfer_keep_hours",
        "Unfinished files",
        Section::Connection,
    ),
    ("video_quality", "Video quality", Section::Phones),
    ("allow_hevc", "H.265 (HEVC)", Section::Phones),
    ("slow_link_frame_rate", "Slow links", Section::Phones),
    (
        "laser_pointer",
        "Laser pointer from phones",
        Section::Phones,
    ),
    ("input_control", "Several phones at once", Section::Phones),
    (
        "input_idle_secs",
        "Hand control back after",
        Section::Phones,
    ),
    ("clipboard_push", "Clipboard to phones", Section::Phones),
    ("clipboard_history", "Clipboard history", Section::Phones),
    (
        "clipboard_hotkey",
        "Send the clipboard with a key",
        Section::Phones,
    ),
    (
        "phone_drive",
        "The phone's folders as a drive",
        Section::Phones,
    ),
    (
        "away_lock_minutes",
        "Lock after the phone leaves",
        Section::Phones,
    ),
    ("open_links", "Links from phones", Section::Phones),
    ("share_state", "This computer's state", Section::Phones),
    ("phone_battery_alerts", "Phones' batteries", Section::Phones),
    (
        "virtual_screens",
        "Phones may add a screen",
        Section::Phones,
    ),
    ("mic_relay_device", "Phone mic plays into", Section::Phones),
    (
        "downloads_folder",
        "Received files land in",
        Section::Phones,
    ),
    (
        "cast_control",
        "Control phones from their screen window",
        Section::Phones,
    ),
    (
        "phone_notifications",
        "Phone notifications",
        Section::Notifications,
    ),
    (
        "notifications_per_minute",
        "Notifications a minute, per phone",
        Section::Notifications,
    ),
    (
        "toast_answers",
        "Answer in the notification",
        Section::Notifications,
    ),
    ("sms_notify", "Text notifications", Section::Notifications),
    (
        "sms_names",
        "Contact names in texts",
        Section::Notifications,
    ),
    ("sms_keep", "Texts kept", Section::Notifications),
    ("wake_announce", "Wake from the phone", Section::Wake),
    ("wake_port", "Outside wake port", Section::Wake),
    ("events", "Event log", Section::Automation),
    ("events_detail", "Details in events", Section::Automation),
    ("hooks", "Hooks", Section::Automation),
    ("tasks", "Tasks", Section::Automation),
    ("deck_enabled", "Deck", Section::Automation),
    ("deck", "Deck buttons", Section::Automation),
    ("home_assistant", "Home Assistant", Section::Automation),
    (
        "mqtt_broker",
        "Home Assistant's broker",
        Section::Automation,
    ),
    ("mqtt_user", "Home Assistant's user", Section::Automation),
    (
        "mqtt_password",
        "Home Assistant's password",
        Section::Automation,
    ),
    (
        "computer_clipboard",
        "Clipboard with my computers",
        Section::Computers,
    ),
    (
        "kdeconnect_compat",
        "KDE Connect devices",
        Section::KdeConnect,
    ),
    ("capture_backend", "Screen capture", Section::Advanced),
    ("log_level", "Log level", Section::Advanced),
    ("relay_urls", "Your own relays", Section::Advanced),
    ("dht_publish", "Found by id (DHT)", Section::Advanced),
];

/// A setting's name on the page.
pub fn name(key: &str) -> &'static str {
    SETTINGS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, name, _)| *name)
        .unwrap_or("A setting")
}

fn section_of(key: &str) -> Option<Section> {
    SETTINGS
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, _, section)| *section)
}

/// Whether the running server reads `key` again on its own: the event log
/// and hooks at each event, tasks at each start, Home Assistant every 2 s,
/// files over the internet at each transfer, answers in notifications at
/// each notification. The rest wait for a restart.
pub fn applies_live(key: &str) -> bool {
    matches!(
        key,
        "events"
            | "events_detail"
            | "hooks"
            | "tasks"
            | "deck"
            | "deck_enabled"
            | "share_state"
            | "home_assistant"
            | "mqtt_broker"
            | "mqtt_user"
            | "mqtt_password"
            | "internet_files"
            | "toast_answers"
            | "cast_control"
            | "downloads_folder"
            | "remember_screen_grant"
            | "clipboard_history"
            | "clipboard_hotkey"
            | "phone_drive"
            | "away_lock_minutes"
    )
}

/// What changes from `saved` to `edited`, by name: those that apply now,
/// and those after a restart.
pub fn split_changes(saved: &Config, edited: &Config) -> (Vec<&'static str>, Vec<&'static str>) {
    let (now, later): (Vec<_>, Vec<_>) = saved
        .changed_keys(edited)
        .into_iter()
        .partition(|key| applies_live(key));
    let names = |keys: Vec<&str>| {
        let mut names: Vec<&'static str> = keys.into_iter().map(name).collect();
        names.dedup();
        names
    };
    (names(now), names(later))
}

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
    /// The settings as the running server read them: saved changes wait
    /// for a restart (the KDE section's buttons follow this, not the box).
    running: Config,
    /// The adapter a phone on this network would wake this computer
    /// through, read when the tab opens (N6).
    wake: Option<WakeFacts>,
    /// The Home Assistant bridge's status as last read, and when (it is
    /// read again every 2 s, not every frame).
    home_assistant: Option<(std::time::Instant, Option<ha::Status>)>,
    /// A folder being picked for received files (C2).
    picking_downloads: Option<std::thread::JoinHandle<Option<std::path::PathBuf>>>,
}

impl SettingsForm {
    /// The settings as the running server read them.
    pub fn running(&self) -> &Config {
        &self.running
    }

    /// The server was restarted: it has read what is saved.
    pub fn restarted(&mut self) {
        self.running = self.saved.clone();
    }

    pub fn from_config(config: Config) -> Self {
        Self {
            mic_device_text: config.mic_relay_device.clone().unwrap_or_default(),
            relay_urls_text: config.relay_urls.join("\n"),
            hooks_text: hooks_text(&config.hooks),
            saved: config.clone(),
            running: config.clone(),
            edited: config,
            wake: pocketdesk_core::local::wake::wake_facts(None),
            home_assistant: None,
            picking_downloads: None,
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

    /// Whether the save bar shows: for changes, or for a problem in what
    /// is saved (a hand-edited config), which would otherwise go unsaid
    /// (U1 review).
    pub fn shows_save_bar(&self) -> bool {
        self.dirty() || !self.problems().is_empty()
    }

    /// Back to what is saved, keeping what the server read and whether a
    /// saved change waits for a restart (U1 review: Undo had forgotten
    /// both).
    pub fn undo(&mut self) {
        let running = std::mem::take(&mut self.running);
        *self = Self::from_config(self.saved.clone());
        self.running = running;
    }

    /// What stops the edit from being saved, in words for the user.
    pub fn problems(&self) -> Vec<String> {
        self.problems_by_key()
            .into_iter()
            .map(|(_, problem)| problem)
            .collect()
    }

    /// The problems, each with the key it is about, so its section opens.
    fn problems_by_key(&self) -> Vec<(&'static str, String)> {
        let mut problems = Vec::new();
        for (name, port) in [
            ("control", self.edited.control_port),
            ("streaming", self.edited.streaming_port),
        ] {
            if port < 1024 {
                let key = if name == "control" {
                    "control_port"
                } else {
                    "streaming_port"
                };
                problems.push((key, format!(
                    "The {name} port must be 1024 or higher (lower ports need administrator rights)."
                )));
            }
        }
        if self.edited.control_port == self.edited.streaming_port {
            problems.push((
                "control_port",
                "The control and streaming ports must be different.".to_string(),
            ));
        }
        if self.edited.wake_port != 0 && self.edited.wake_port == self.edited.streaming_port {
            problems.push((
                "wake_port",
                "The outside wake port must differ from the streaming port, so the two router forwards do not clash."
                    .to_string(),
            ));
        }
        if self.edited.input_control == InputControlMode::One
            && !(3..=120).contains(&self.edited.input_idle_secs)
        {
            problems.push((
                "input_idle_secs",
                "Control is handed back after 3 to 120 seconds.".to_string(),
            ));
        }
        for url in &self.edited.relay_urls {
            if let Some(problem) = relay_url_problem(url) {
                problems.push(("relay_urls", format!("Relay: {problem}.")));
            }
        }
        for problem in parse_hooks(&self.hooks_text).1 {
            problems.push(("hooks", format!("Hooks: {problem}.")));
        }
        problems.extend(
            task_problems(&self.edited.tasks)
                .into_iter()
                .map(|problem| ("tasks", problem)),
        );
        problems.extend(
            deck_problems(&self.edited.deck)
                .into_iter()
                .map(|problem| ("deck", problem)),
        );
        if self.edited.home_assistant
            && let Err(why) = ha::parse_broker(&self.edited.mqtt_broker)
        {
            problems.push(("mqtt_broker", format!("Home Assistant: {why}.")));
        }
        problems
    }

    /// Whether a problem is about a setting in `section`, which then opens.
    pub fn section_has_problem(&self, section: Section) -> bool {
        self.problems_by_key()
            .iter()
            .any(|(key, _)| section_of(key) == Some(section))
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

    /// Whether saving turns the clipboard's history off (D3): it is emptied then.
    pub fn turns_history_off(&self) -> bool {
        self.saved.clipboard_history > 0 && self.edited.clipboard_history == 0
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
        self.saved = self.edited.clone();
        Ok(())
    }

    /// What is saved but not yet read by the server, by name: the settings
    /// it reads only at start (applies_live) that differ from what it read.
    /// Saved back to what it runs, a setting no longer waits (U5 review).
    pub fn restart_names(&self) -> Vec<&'static str> {
        split_changes(&self.running, &self.saved).1
    }

    /// Whether a saved change waits for a restart.
    pub fn restart_needed(&self) -> bool {
        !self.restart_names().is_empty()
    }

    /// The rows of one section; Save and Undo are in [`Self::save_bar`],
    /// shown in view while there are changes.
    pub fn show_section(&mut self, ui: &mut egui::Ui, section: Section) {
        let home_assistant = if section == Section::Automation && self.saved.home_assistant {
            self.home_assistant_status()
        } else {
            None
        };
        let e = &mut self.edited;
        match section {
            Section::Connection => {
                row(ui, "control_port", |ui| {
                    ui.add(egui::DragValue::new(&mut e.control_port).range(1..=65535));
                });
                row(ui, "streaming_port", |ui| {
                    ui.add(egui::DragValue::new(&mut e.streaming_port).range(1..=65535));
                });
                row(ui, "pairing_required", |ui| {
                    ui.checkbox(&mut e.pairing_required, "only paired phones may connect");
                });
                row(ui, "lan_discovery", |ui| {
                    ui.checkbox(
                        &mut e.lan_discovery,
                        "phones on the same network list this computer",
                    );
                });
                // GNOME and KDE only: nothing to remember elsewhere.
                if cfg!(target_os = "linux") {
                    row(ui, "remember_screen_grant", |ui| {
                        ui.checkbox(
                        &mut e.remember_screen_grant,
                        "after you allow screen sharing once, sessions start with no one at this computer",
                    );
                        note(
                            ui,
                            "GNOME and KDE: their screen-sharing dialog asks once, then sessions start without it until you revoke it in the desktop's own privacy settings, or Forget it here. Every session still needs a paired phone and its permissions.",
                        );
                        let state = pocketdesk_core::local::paths::state_dir().ok();
                        let kept = state.as_deref().is_some_and(|dir| {
                            pocketdesk_core::portal_grant::read(dir).is_some()
                                || pocketdesk_core::portal_grant::read_remote(dir).is_some()
                        });
                        if ui
                            .add_enabled(kept, egui::Button::new("Forget the grant"))
                            .on_hover_text("The next session asks at this computer again.")
                            .on_disabled_hover_text("No grant is kept.")
                            .clicked()
                            && let Some(dir) = state
                        {
                            let _ = pocketdesk_core::portal_grant::forget(&dir);
                        }
                    });
                }
                row(ui, "internet_files", |ui| {
                    ui.checkbox(
                        &mut e.internet_files,
                        "move files with a phone linked from away",
                    );
                    note(
                        ui,
                        "On the phone's own link, so nothing dials it back. Relays see sizes and timing, never content.",
                    );
                });
                row(ui, "transfer_keep_hours", |ui| {
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
                    note(
                        ui,
                        "A file from a phone that stops part way is kept this long, so the phone's retry continues it.",
                    );
                });
            }
            Section::Phones => {
                row(ui, "video_quality", |ui| {
                    egui::ComboBox::from_id_salt("quality")
                        .selected_text(quality_label(&e.video_quality))
                        .show_ui(ui, |ui| {
                            for preset in [
                                VideoQualityPreset::Low,
                                VideoQualityPreset::Balanced,
                                VideoQualityPreset::High,
                            ] {
                                let label = quality_label(&preset);
                                ui.selectable_value(&mut e.video_quality, preset, label);
                            }
                        });
                    // What each means, since the phone's Quality caps the
                    // bitrate at the computer's resolution (U5 review).
                    note(
                        ui,
                        "Economy: 720p at 2 Mbit/s. Balanced: 1080p at 5. Max: 1080p at 10. \
                         A phone's own Quality choice can lower the bitrate for its session.",
                    );
                });
                row(ui, "allow_hevc", |ui| {
                    ui.checkbox(&mut e.allow_hevc, "use it with phones that can decode it");
                });
                row(ui, "slow_link_frame_rate", |ui| {
                    ui.checkbox(
                        &mut e.slow_link_frame_rate,
                        "lower the frame rate to keep the picture moving",
                    );
                });
                row(ui, "laser_pointer", |ui| {
                    ui.checkbox(
                        &mut e.laser_pointer,
                        "the air mouse in a phone's Remote can show a red dot here",
                    )
                    .on_hover_text(
                        "A dot above every window that never takes a click, for a talk. \
                         Drawn on Windows; elsewhere the phone is not offered it yet.",
                    );
                });
                row(ui, "input_control", |ui| {
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
                });
                row(ui, "input_idle_secs", |ui| {
                    ui.add_enabled(
                        e.input_control == InputControlMode::One,
                        egui::DragValue::new(&mut e.input_idle_secs)
                            .range(3..=120)
                            .suffix(" s without input"),
                    );
                });
                row(ui, "clipboard_push", |ui| {
                    ui.checkbox(
                        &mut e.clipboard_push,
                        "send what I copy to connected phones (those with \"Share the clipboard\" on in Devices)",
                    );
                });
                row(ui, "clipboard_history", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut e.clipboard_history)
                            .range(0..=pocketdesk_core::local::clip_history::MAX_ENTRIES)
                            .suffix(" texts"),
                    );
                    note(
                        ui,
                        "The last texts that crossed between this computer and its phones, either way, kept on this computer for the Clipboard page; 0 keeps none, and emptying it there or here forgets them.",
                    );
                });
                row(ui, "clipboard_hotkey", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.clipboard_hotkey)
                            .hint_text("ctrl+alt+p")
                            .desired_width(140.0),
                    );
                    let text = e.clipboard_hotkey.trim();
                    if !text.is_empty() && pocketdesk_desktop::hotkey::parse(text).is_none() {
                        ui.label(
                            egui::RichText::new(
                                "Ctrl, Alt or Win, then a letter, a digit or F1 to F12, joined by +.",
                            )
                            .color(crate::theme::bad(ui)),
                        );
                    }
                    note(
                        ui,
                        "Pressed anywhere, sends what this computer has copied to the phone the clipboard reaches (the one in the newest session when several do). Windows, through the tray; empty is off.",
                    );
                });
                row(ui, "phone_drive", |ui| {
                    ui.checkbox(
                        &mut e.phone_drive,
                        "open the folders a phone shares in the file manager, read-only",
                    );
                    note(
                        ui,
                        "Served on this computer only, at an address with a secret in it that the Phone page shows. Any program you run can read those folders while this is on.",
                    );
                });
                // Only where it locks (E5): Linux cannot tell yet whether
                // someone is at the keyboard, so it never does.
                if cfg!(windows) {
                    row(ui, "away_lock_minutes", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut e.away_lock_minutes)
                                .range(2..=60)
                                .suffix(" min"),
                        );
                        note(
                            ui,
                            "How long a phone with \"Lock this computer when this phone leaves\" on (Devices) must be gone, with nobody at the keyboard or mouse, before this computer locks. A sleeping phone can drop its link, so it can lock while you read.",
                        );
                    });
                }
                row(ui, "open_links", |ui| {
                    ui.checkbox(
                        &mut e.open_links,
                        "open in the browser (off: show as a notification)",
                    );
                });
                row(ui, "share_state", |ui| {
                    ui.checkbox(
                        &mut e.share_state,
                        "phones may see this computer's CPU, memory, disks, battery and uptime",
                    );
                });
                row(ui, "phone_battery_alerts", |ui| {
                    ui.checkbox(
                        &mut e.phone_battery_alerts,
                        "a notification when a phone runs low",
                    );
                });
                row(ui, "virtual_screens", |ui| {
                    ui.checkbox(
                        &mut e.virtual_screens,
                        "a phone with Input can add a screen of its own shape (Sway)",
                    );
                });
                // A folder picked for received files (C2) fills the box.
                if self
                    .picking_downloads
                    .as_ref()
                    .is_some_and(|h| h.is_finished())
                    && let Some(Ok(Some(path))) = self.picking_downloads.take().map(|h| h.join())
                {
                    e.downloads_folder = path.display().to_string();
                }
                let picking = &mut self.picking_downloads;
                row(ui, "downloads_folder", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut e.downloads_folder)
                            .hint_text("Downloads")
                            .desired_width(320.0),
                    );
                    if crate::pick_file::AVAILABLE
                        && ui
                            .add_enabled(picking.is_none(), egui::Button::new("Choose folder…"))
                            .clicked()
                    {
                        *picking = Some(crate::pick_file::spawn(true));
                    }
                    if !e.downloads_folder.trim().is_empty() && ui.button("Use Downloads").clicked()
                    {
                        e.downloads_folder.clear();
                    }
                    note(
                        ui,
                        "Files from phones, other computers and KDE Connect devices. A folder that is gone means Downloads.",
                    );
                });
                row(ui, "cast_control", |ui| {
                    ui.checkbox(
                        &mut e.cast_control,
                        "tap, drag, scroll and type on a phone in the window showing its screen",
                    );
                    note(
                        ui,
                        "Only once the phone allows it: \"Let the computer control this phone\" in Pocketdesk's Settings on the phone.",
                    );
                });
                if cfg!(windows) {
                    let mic = &mut self.mic_device_text;
                    row(ui, "mic_relay_device", |ui| {
                        ui.add(egui::TextEdit::singleline(mic).hint_text("CABLE Input (VB-CABLE)"));
                    });
                }
            }
            Section::Notifications => {
                row(ui, "phone_notifications", |ui| {
                    ui.checkbox(
                        &mut e.phone_notifications,
                        "show the notifications phones and KDE Connect devices share",
                    );
                });
                row(ui, "notifications_per_minute", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut e.notifications_per_minute)
                            .range(1..=600)
                            .suffix(" a minute"),
                    );
                });
                row(ui, "toast_answers", |ui| {
                    ui.checkbox(
                        &mut e.toast_answers,
                        "a phone notification you may answer shows its buttons and a reply box",
                    );
                    note(
                        ui,
                        "Only for notifications the phone lets this computer answer, while this server runs. The Notifications page answers them too.",
                    );
                });
                row(ui, "sms_notify", |ui| {
                    ui.checkbox(&mut e.sms_notify, "a notification for each text");
                });
                row(ui, "sms_names", |ui| {
                    ui.checkbox(
                        &mut e.sms_names,
                        "keep the contact names phones send with texts (off forgets those kept, when the server restarts)",
                    );
                });
                row(ui, "sms_keep", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut e.sms_keep)
                            .range(50..=100_000)
                            .suffix(" texts"),
                    );
                });
            }
            Section::Wake => {
                row(ui, "wake_announce", |ui| {
                    ui.checkbox(
                        &mut e.wake_announce,
                        "tell paired phones how to wake this computer",
                    );
                    note(
                        ui,
                        match &self.wake {
                            Some(w) => format!(
                                "Through {} ({}), {}.",
                                w.adapter,
                                if w.wired {
                                    "wired"
                                } else {
                                    "Wi-Fi: usually wakes only from sleep"
                                },
                                w.mac
                            ),
                            None => "No adapter with a hardware address was found.".to_string(),
                        },
                    );
                });
                row(ui, "wake_port", |ui| {
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
                    note(
                        ui,
                        "To wake it from outside, forward this UDP port on the router to this computer's address. \
                         It reaches the computer while the router still remembers it (minutes to hours after it sleeps), \
                         or always when the router can forward to the network's broadcast address.",
                    );
                });
            }
            Section::Automation => {
                row(ui, "events", |ui| {
                    ui.checkbox(
                        &mut e.events,
                        "keep a log of what happens, for scripts and hooks",
                    );
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
                });
                let hooks = &mut self.hooks_text;
                row(ui, "hooks", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(hooks)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY)
                            .hint_text("file_received = notify-send \"A file came\""),
                    );
                    note(
                        ui,
                        format!(
                            "One per line: an event, \"=\", a command. It runs as you, with the event on stdin. Events: {}.",
                            KINDS
                                .iter()
                                .copied()
                                .filter(|k| hookable(k))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    );
                    let unknown = unknown_hook_kinds(&parse_hooks(hooks).0)
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
                            .color(crate::theme::warn(ui)),
                        );
                    }
                });
                row(ui, "tasks", |ui| {
                    let mut remove = None;
                    for (i, task) in e.tasks.iter_mut().enumerate() {
                        // Wrapped: name, command, ask, deadline and Remove
                        // are wider than a narrow page (U1 review).
                        ui.horizontal_wrapped(|ui| {
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
                row(ui, "deck_enabled", |ui| {
                    ui.checkbox(
                        &mut e.deck_enabled,
                        "phones may show and press the buttons below",
                    );
                });
                row(ui, "deck", |ui| {
                    let mut remove = None;
                    for (i, button) in e.deck.iter_mut().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut button.label)
                                    .desired_width(110.0)
                                    .hint_text("label"),
                            );
                            let mut kind = deck_kind(button);
                            egui::ComboBox::from_id_salt(("deck-kind", i))
                                .selected_text(deck_kind_words(kind))
                                .show_ui(ui, |ui| {
                                    for k in DECK_KINDS {
                                        ui.selectable_value(&mut kind, k, deck_kind_words(k));
                                    }
                                });
                            if kind != deck_kind(button) {
                                set_deck_kind(button, kind);
                            }
                            if kind == "media" {
                                let value = deck_value_mut(button);
                                egui::ComboBox::from_id_salt(("deck-media", i))
                                    .selected_text(media_words(value))
                                    .show_ui(ui, |ui| {
                                        for m in ["playPause", "next", "previous"] {
                                            ui.selectable_value(
                                                value,
                                                m.to_string(),
                                                media_words(m),
                                            );
                                        }
                                    });
                            } else {
                                ui.add(
                                    egui::TextEdit::singleline(deck_value_mut(button))
                                        .desired_width(240.0)
                                        .hint_text(deck_hint(kind)),
                                );
                            }
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        e.deck.remove(i);
                    }
                    if e.deck.len() < DECK_MAX_BUTTONS && ui.button("Add button").clicked() {
                        e.deck.push(DeckButton {
                            keys: Some(String::new()),
                            ..Default::default()
                        });
                    }
                    ui.label(
                        RichText::new(
                            "Each button shows on the phone's Deck and does its action here when pressed, if the phone may: keys and text need \"Control the mouse and keyboard\", a task \"Start my tasks\", media Audio, a link \"Open its links here\". The phone sees the labels, never the keys, links or text.",
                        )
                        .small()
                        .weak(),
                    );
                });
                row(ui, "home_assistant", |ui| {
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
            }
            Section::Computers => {
                row(ui, "computer_clipboard", |ui| {
                    ui.checkbox(
                        &mut e.computer_clipboard,
                        "share what I copy with my other paired computers, both ways (turn it on on each)",
                    );
                });
            }
            Section::KdeConnect => {
                row(ui, "kdeconnect_compat", |ui| {
                    ui.checkbox(
                        &mut e.kdeconnect_compat,
                        "work with devices that run KDE Connect (pairing asks you first)",
                    );
                });
            }
            Section::Advanced => {
                row(ui, "capture_backend", |ui| {
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
                });
                row(ui, "log_level", |ui| {
                    egui::ComboBox::from_id_salt("log")
                        .selected_text(e.log_level.clone())
                        .show_ui(ui, |ui| {
                            for level in ["error", "warn", "info", "debug", "trace"] {
                                ui.selectable_value(&mut e.log_level, level.to_string(), level);
                            }
                        });
                });
                let relays = &mut self.relay_urls_text;
                row(ui, "relay_urls", |ui| {
                    ui.add(
                        egui::TextEdit::multiline(relays)
                            .desired_rows(2)
                            .hint_text("blank: n0's public relays\nhttps://relay.example.org"),
                    );
                });
                row(ui, "dht_publish", |ui| {
                    egui::ComboBox::from_id_salt("dht")
                        .selected_text(dht_label(e.dht_publish))
                        .show_ui(ui, |ui| {
                            for mode in [DhtPublish::Relay, DhtPublish::Addresses, DhtPublish::Off]
                            {
                                ui.selectable_value(&mut e.dht_publish, mode, dht_label(mode));
                            }
                        });
                });
            }
        }
        self.apply_text_fields();
    }

    /// Save and Undo, with what keeps Save off, for a bar that stays in
    /// view while the form has changes (U1: at the end of the form they
    /// were out of reach); whether Save was pressed.
    pub fn save_bar(&mut self, ui: &mut egui::Ui) -> bool {
        let problems = self.problems();
        for problem in &problems {
            ui.label(RichText::new(problem).color(crate::theme::bad(ui)));
        }
        if !self.dirty() {
            ui.label(
                RichText::new("The saved settings have a problem: change them here and save.")
                    .weak(),
            );
            return false;
        }
        let (now, later) = split_changes(&self.saved, &self.edited);
        if !now.is_empty() {
            ui.label(RichText::new(format!("Applies now: {}.", now.join(", "))).weak());
        }
        if !later.is_empty() {
            ui.label(
                RichText::new(format!("After the server restarts: {}.", later.join(", "))).weak(),
            );
        }
        let mut save = false;
        ui.horizontal_wrapped(|ui| {
            save = ui
                .add_enabled(problems.is_empty(), egui::Button::new("Save"))
                .clicked();
            if ui.button("Undo changes").clicked() {
                self.undo();
            }
            ui.label(RichText::new("Settings have changes not saved yet.").weak());
        });
        save
    }
}

/// The video quality in the phone's words (U5: the phone said Economy,
/// Balanced and Max where this page said Low, Balanced and High); the
/// config's keys stay as they are.
fn quality_label(preset: &VideoQualityPreset) -> &'static str {
    match preset {
        VideoQualityPreset::Low => "Economy",
        VideoQualityPreset::Balanced => "Balanced",
        VideoQualityPreset::High => "Max",
    }
}

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

/// The kinds of deck button, in the editor's order.
const DECK_KINDS: [&str; 5] = ["keys", "task", "media", "link", "text"];

/// A deck button's kind: which of its actions it holds (keys when none).
fn deck_kind(button: &DeckButton) -> &'static str {
    if button.task.is_some() {
        "task"
    } else if button.media.is_some() {
        "media"
    } else if button.link.is_some() {
        "link"
    } else if button.text.is_some() {
        "text"
    } else {
        "keys"
    }
}

/// Make `button` a `kind` button, keeping what was typed when it fits.
fn set_deck_kind(button: &mut DeckButton, kind: &str) {
    let old = [
        button.keys.take(),
        button.task.take(),
        button.media.take(),
        button.link.take(),
        button.text.take(),
    ]
    .into_iter()
    .flatten()
    .next()
    .unwrap_or_default();
    let value = if kind == "media" {
        "playPause".to_string()
    } else {
        old
    };
    match kind {
        "task" => button.task = Some(value),
        "media" => button.media = Some(value),
        "link" => button.link = Some(value),
        "text" => button.text = Some(value),
        _ => button.keys = Some(value),
    }
}

/// The value the editor's box edits for `button`'s kind.
fn deck_value_mut(button: &mut DeckButton) -> &mut String {
    let slot = match deck_kind(button) {
        "task" => &mut button.task,
        "media" => &mut button.media,
        "link" => &mut button.link,
        "text" => &mut button.text,
        _ => &mut button.keys,
    };
    slot.get_or_insert_with(String::new)
}

fn deck_kind_words(kind: &str) -> &'static str {
    match kind {
        "task" => "Start a task",
        "media" => "Media",
        "link" => "Open a link",
        "text" => "Type text",
        _ => "Press keys",
    }
}

fn deck_hint(kind: &str) -> &'static str {
    match kind {
        "task" => "the task's name",
        "link" => "https://…",
        "text" => "the text to type",
        _ => "ctrl+shift+m",
    }
}

fn media_words(media: &str) -> &'static str {
    match media {
        "next" => "Next",
        "previous" => "Previous",
        _ => "Play/Pause",
    }
}

/// What is wrong with the deck's buttons, in words for the user.
fn deck_problems(buttons: &[DeckButton]) -> Vec<String> {
    let mut problems = Vec::new();
    if buttons.len() > DECK_MAX_BUTTONS {
        problems.push(format!("Deck: at most {DECK_MAX_BUTTONS} buttons."));
    }
    for (i, button) in buttons.iter().enumerate() {
        if let Err(why) = button.action() {
            let which = if button.label.trim().is_empty() {
                format!("button {}", i + 1)
            } else {
                format!("\"{}\"", button.label.trim())
            };
            problems.push(format!("Deck: {which}: {why}."));
        }
    }
    problems
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

/// One setting: its name, then its control and any note under it, wrapping
/// at the page's width (U5: a grid's long labels widened the page).
fn row(ui: &mut egui::Ui, key: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(RichText::new(name(key)).strong());
    add(ui);
}

/// A row's note, small and quiet.
fn note(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).small().weak());
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
    fn undo_keeps_a_saved_change_waiting_for_a_restart() {
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.control_port = 1800;
        form.saved = form.edited.clone();
        form.edited.streaming_port = 4800;
        form.undo();
        assert_eq!(form.edited, form.saved, "the edit is undone");
        assert!(form.restart_needed(), "the saved change still waits");
        assert_eq!(
            form.running().control_port,
            Config::default().control_port,
            "the server still runs what it read"
        );
    }

    #[test]
    fn saved_settings_with_a_problem_show_the_bar_unedited() {
        let mut saved = Config::default();
        saved.streaming_port = saved.control_port;
        let form = SettingsForm::from_config(saved);
        assert!(!form.dirty());
        assert!(
            form.shows_save_bar(),
            "a hand-edited config's problem is said"
        );
        assert!(!SettingsForm::from_config(Config::default()).shows_save_bar());
    }

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
        assert!(split_changes(&Config::default(), &form.edited).1.is_empty());
    }

    #[test]
    fn video_quality_reads_as_the_phone_says_it() {
        assert_eq!(quality_label(&VideoQualityPreset::Low), "Economy");
        assert_eq!(quality_label(&VideoQualityPreset::Balanced), "Balanced");
        assert_eq!(quality_label(&VideoQualityPreset::High), "Max");
    }

    #[test]
    fn every_setting_has_one_place_on_the_page() {
        for key in Config::KNOWN_KEYS {
            let rows = SETTINGS.iter().filter(|(k, _, _)| k == key).count();
            assert_eq!(rows, 1, "{key} is in {rows} rows");
        }
        assert_eq!(SETTINGS.len(), Config::KNOWN_KEYS.len(), "a row for no key");
        for section in Section::ALL {
            assert!(
                SETTINGS.iter().any(|(_, _, s)| *s == section),
                "{section:?} is empty"
            );
        }
    }

    #[test]
    fn the_save_bar_says_what_applies_now_and_what_after_a_restart() {
        let saved = Config::default();
        let mut edited = saved.clone();
        edited.internet_files = !saved.internet_files;
        edited.events = !saved.events;
        edited.control_port = 1800;
        edited.clipboard_push = !saved.clipboard_push;
        let (now, later) = split_changes(&saved, &edited);
        assert_eq!(now, ["Files over the internet", "Event log"]);
        assert_eq!(later, ["Control port (TCP)", "Clipboard to phones"]);
    }

    #[test]
    fn files_over_the_internet_apply_without_a_restart() {
        let dir = std::env::temp_dir().join(format!("pd-settings-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut form = SettingsForm::from_config(Config::default());
        form.edited.internet_files = !form.edited.internet_files;
        form.save_to(&path).unwrap();
        assert!(
            !form.restart_needed(),
            "the server reads it at each transfer"
        );
        form.edited.lan_discovery = !form.edited.lan_discovery;
        form.save_to(&path).unwrap();
        assert!(form.restart_needed());
        assert_eq!(form.restart_names(), ["Visible on this network"]);
        // Saved back to what the server runs: nothing waits any more.
        form.edited.lan_discovery = !form.edited.lan_discovery;
        form.save_to(&path).unwrap();
        assert!(!form.restart_needed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_problem_opens_its_section() {
        let mut form = SettingsForm::from_config(Config::default());
        assert!(!form.section_has_problem(Section::Connection));
        form.edited.control_port = 80;
        assert!(form.section_has_problem(Section::Connection));
        assert!(!form.section_has_problem(Section::Advanced));
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
    fn a_deck_button_changes_kind_keeping_what_was_typed() {
        let mut button = DeckButton {
            label: "Site".into(),
            keys: Some("https://example.org/".into()),
            ..Default::default()
        };
        assert_eq!(deck_kind(&button), "keys");
        set_deck_kind(&mut button, "link");
        assert_eq!(deck_kind(&button), "link");
        assert_eq!(button.link.as_deref(), Some("https://example.org/"));
        assert!(button.keys.is_none(), "one action at a time");
        set_deck_kind(&mut button, "media");
        assert_eq!(button.media.as_deref(), Some("playPause"));
        deck_value_mut(&mut button).replace_range(.., "next");
        assert_eq!(button.media.as_deref(), Some("next"));
    }

    #[test]
    fn deck_problems_name_the_button() {
        let good = DeckButton {
            label: "Copy".into(),
            keys: Some("ctrl+c".into()),
            ..Default::default()
        };
        let bad = DeckButton {
            label: "Oops".into(),
            keys: Some("ctrl+banana".into()),
            ..Default::default()
        };
        let unnamed = DeckButton {
            keys: Some("a".into()),
            ..Default::default()
        };
        assert!(deck_problems(std::slice::from_ref(&good)).is_empty());
        let problems = deck_problems(&[good, bad, unnamed]);
        assert_eq!(problems.len(), 2);
        assert!(problems[0].contains("Oops") && problems[0].contains("banana"));
        assert!(problems[1].contains("button 3"));
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
        assert!(!form.restart_needed(), "events apply at the next event");
        form.edited.allow_hevc = true;
        form.save_to(&path).unwrap();
        assert!(form.restart_needed());
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
        form.edited.kdeconnect_compat = false;
        form.save_to(&path).unwrap();
        assert!(!form.dirty() && form.restart_needed());
        assert!(
            form.running().kdeconnect_compat,
            "the running server read it before the save"
        );
        form.restarted();
        assert!(!form.running().kdeconnect_compat && !form.restart_needed());
        form.edited.kdeconnect_compat = true;
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
