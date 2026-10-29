//! The shared keyboard and mouse's options (UX 2): on its own page, under
//! Options, where Settings once had them. Each is saved as it changes,
//! that key alone, as the page's sharing switch is; the two text boxes as
//! they are left (Enter, or a click elsewhere).

use eframe::egui::{self, RichText};
use pocketdesk_core::local::config::Config;

/// The options, by their keys in config.toml, in page order. Settings'
/// table names each as its row here does.
#[cfg(test)]
const KEYS: [&str; 8] = [
    "shared_input_drive",
    "shared_input_hotkey",
    "shared_input_game_lock",
    "shared_input_lock_programs",
    "shared_input_clipboard",
    "shared_input_files",
    "shared_input_sound",
    "shared_input_low_latency",
];

/// The heading they sit under.
pub const OPTIONS: &str = "Options";

/// Why they are off while sharing is (UX 1); its fix is Setup's.
pub const SHARING_OFF: &str = "Off while sharing is off.";

/// Copy `key`'s value from `from` to `to`: the sharing switch's key and
/// the options' (any other key is left).
pub fn copy_key(from: &Config, to: &mut Config, key: &str) {
    match key {
        "shared_input" => to.shared_input = from.shared_input,
        "shared_input_drive" => to.shared_input_drive = from.shared_input_drive,
        "shared_input_hotkey" => to.shared_input_hotkey = from.shared_input_hotkey.clone(),
        "shared_input_game_lock" => to.shared_input_game_lock = from.shared_input_game_lock,
        "shared_input_lock_programs" => {
            to.shared_input_lock_programs = from.shared_input_lock_programs.clone()
        }
        "shared_input_clipboard" => to.shared_input_clipboard = from.shared_input_clipboard,
        "shared_input_files" => to.shared_input_files = from.shared_input_files,
        "shared_input_sound" => to.shared_input_sound = from.shared_input_sound,
        "shared_input_low_latency" => to.shared_input_low_latency = from.shared_input_low_latency,
        _ => {}
    }
}

/// The text boxes' words while they are typed in, and what they were made
/// from, so a change saved elsewhere shows once nobody is typing.
#[derive(Default)]
pub struct InputOptions {
    hotkey: String,
    programs: String,
    from: Option<(String, Vec<String>)>,
}

/// The programs in the box: one a line, trimmed, blank ones left out.
fn programs(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

impl InputOptions {
    /// Draw the options for `config` (as saved), changing it; the keys
    /// changed, to save. `on`: sharing is on, else they are off, and say
    /// why.
    pub fn show(&mut self, ui: &mut egui::Ui, config: &mut Config, on: bool) -> Vec<&'static str> {
        let saved = (
            config.shared_input_hotkey.clone(),
            config.shared_input_lock_programs.clone(),
        );
        if self.from.as_ref() != Some(&saved) {
            self.hotkey = saved.0.clone();
            self.programs = saved.1.join("\n");
            self.from = Some(saved);
        }
        let mut changed = Vec::new();
        ui.add_space(8.0);
        ui.heading(OPTIONS);
        if !on {
            crate::widgets::why_off(ui, SHARING_OFF, None);
        }
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).small().color(crate::theme::muted(ui)));
        };
        let mut check = |ui: &mut egui::Ui, key: &'static str, value: &mut bool, tip: &str| {
            if ui
                .add_enabled(
                    on,
                    egui::Checkbox::new(value, crate::settings_view::name(key)),
                )
                .on_hover_text(tip)
                .changed()
            {
                changed.push(key);
            }
        };
        check(
            ui,
            "shared_input_drive",
            &mut config.shared_input_drive,
            "Off, this computer only takes the others' keyboard and mouse: its own stay here.",
        );
        check(
            ui,
            "shared_input_game_lock",
            &mut config.shared_input_game_lock,
            "So a game's mouse is never carried off by a push at the edge.",
        );
        check(
            ui,
            "shared_input_clipboard",
            &mut config.shared_input_clipboard,
            "What you copy here is ready to paste where the mouse goes.",
        );
        if crate::settings_view::drawn_here("shared_input_files") {
            check(
                ui,
                "shared_input_files",
                &mut config.shared_input_files,
                "Files dragged in Explorer and let go on another computer land in its received files. Files over the internet must be on on both.",
            );
        }
        if crate::settings_view::drawn_here("shared_input_sound") {
            check(
                ui,
                "shared_input_sound",
                &mut config.shared_input_sound,
                "The sound of the computer the mouse is on plays here until it comes home.",
            );
        }
        check(
            ui,
            "shared_input_low_latency",
            &mut config.shared_input_low_latency,
            crate::settings_view::quick_note(),
        );
        ui.horizontal(|ui| {
            ui.label(crate::settings_view::name("shared_input_hotkey"));
            let edit = ui.add_enabled(
                on,
                egui::TextEdit::singleline(&mut self.hotkey)
                    .hint_text("ctrl+alt+right")
                    .desired_width(160.0),
            );
            if edit.lost_focus() && self.hotkey.trim() != config.shared_input_hotkey {
                config.shared_input_hotkey = self.hotkey.trim().to_string();
                changed.push("shared_input_hotkey");
            }
        });
        note(
            ui,
            if cfg!(windows) {
                "Empty is off. Read when the server starts."
            } else {
                "Moves on from another computer. Read at start."
            },
        );
        ui.label(crate::settings_view::name("shared_input_lock_programs"));
        let edit = ui.add_enabled(
            on,
            egui::TextEdit::multiline(&mut self.programs)
                .desired_rows(2)
                .hint_text("Blender.exe or blender, one a line"),
        );
        if edit.lost_focus() && programs(&self.programs) != config.shared_input_lock_programs {
            config.shared_input_lock_programs = programs(&self.programs);
            changed.push("shared_input_lock_programs");
        }
        note(ui, "The pointer stays while one is in front.");
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each option is copied by its key, and nothing else with it.
    #[test]
    fn each_key_copies_its_own_value() {
        let mut from = Config {
            shared_input_drive: false,
            shared_input_hotkey: "ctrl+alt+right".to_string(),
            ..Config::default()
        };
        from.shared_input_lock_programs = vec!["Blender.exe".to_string()];
        let mut to = Config::default();
        copy_key(&from, &mut to, "shared_input_hotkey");
        assert_eq!(to.shared_input_hotkey, "ctrl+alt+right");
        assert_eq!(to.shared_input_drive, Config::default().shared_input_drive);
        copy_key(&from, &mut to, "shared_input_lock_programs");
        assert_eq!(to.shared_input_lock_programs, ["Blender.exe"]);
        copy_key(&from, &mut to, "control_port");
        assert_eq!(to.control_port, Config::default().control_port);
    }

    /// The programs' box keeps a program a line, blank lines left out.
    #[test]
    fn the_programs_box_reads_a_program_a_line() {
        assert_eq!(programs(" a.exe \n\n b \n"), ["a.exe", "b"]);
    }

    /// Every option is one of Settings' rows, so the save bar and search
    /// name it as this page does.
    #[test]
    fn every_option_is_a_settings_row() {
        for key in KEYS {
            assert_ne!(crate::settings_view::name(key), "A setting", "{key}");
        }
    }
}
