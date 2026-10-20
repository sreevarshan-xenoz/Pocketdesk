//! Keys that act in the window (I3): Enter in a box sends what it holds.

use eframe::egui;

/// A new line in a multi-line box that sends on Enter.
pub const NEW_LINE: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter);

/// Enter pressed in `field`, a single-line box (it gives up focus as it is
/// pressed).
pub fn entered(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Enter without Shift pressed while `field`, a multi-line box whose
/// `return_key` is [`NEW_LINE`], has focus.
pub fn enter_in(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift)
}

/// Ctrl+F: the page's search box (J2).
pub const FIND: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::F);

/// Esc pressed: an open question's Cancel. Taken, so one press answers one
/// question.
pub fn escaped(ui: &egui::Ui) -> bool {
    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
}
