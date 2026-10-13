//! The Settings tab: edit `config.toml` (the server reads it at startup).

use std::path::Path;

use eframe::egui::{self, Color32, RichText};
use linux_link_core::local::config::{Config, ConfigError};
use linux_link_core::streaming::{CaptureBackend, VideoQualityPreset};

/// The settings being edited against what is saved.
pub struct SettingsForm {
    saved: Config,
    pub edited: Config,
    /// The text box behind `mic_relay_device` (blank means the default).
    pub mic_device_text: String,
    /// Saved since the server last read it.
    pub restart_needed: bool,
}

impl SettingsForm {
    pub fn from_config(config: Config) -> Self {
        Self {
            mic_device_text: config.mic_relay_device.clone().unwrap_or_default(),
            saved: config.clone(),
            edited: config,
            restart_needed: false,
        }
    }

    /// Copy the free-text fields into the config being edited.
    pub fn apply_text_fields(&mut self) {
        let text = self.mic_device_text.trim();
        self.edited.mic_relay_device = (!text.is_empty()).then(|| text.to_string());
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

#[cfg(test)]
mod tests {
    use super::*;
    use linux_link_core::local::config::Config;

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
