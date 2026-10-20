//! A page's Search box (J4): one look and one set of keys on every page
//! that searches what it lists.

use eframe::egui;

/// The box, filling `query`. Esc in it empties it, and that press does
/// nothing else on the page.
pub fn show(ui: &mut egui::Ui, query: &mut String) -> egui::Response {
    let response = ui.add(
        egui::TextEdit::singleline(query)
            .hint_text("Search")
            .desired_width(200.0),
    );
    if response.lost_focus() && !query.is_empty() && crate::keys::escaped(ui) {
        query.clear();
    }
    response
}
