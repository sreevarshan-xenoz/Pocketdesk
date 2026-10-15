//! The Settings tab: edit `config.toml` (the server reads it at startup).

use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use pocketdesk_core::local::config::{
    Config, ConfigError, DhtPublish, InputControlMode, relay_url_problem,
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
    /// Saved since the server last read it.
    pub restart_needed: bool,
    /// The adapter a phone on this network would wake this computer
    /// through, read when the tab opens (N6).
    wake: Option<WakeFacts>,
}

impl SettingsForm {
    pub fn from_config(config: Config) -> Self {
        Self {
            mic_device_text: config.mic_relay_device.clone().unwrap_or_default(),
            relay_urls_text: config.relay_urls.join("\n"),
            saved: config.clone(),
            edited: config,
            restart_needed: false,
            wake: pocketdesk_core::local::wake::wake_facts(None),
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
        problems
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
        self.restart_needed = true;
        Ok(())
    }

    /// Draw the form. Returns true when the user asked to save.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::config::Config;

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
