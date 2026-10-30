//! Small pieces the pages share (UX 1): the line that says why a control
//! is off, next to it, with the fix when there is one; and what a page
//! asks the app to do for it (start the server, open a page).

use eframe::egui::{self, RichText};

/// What a page asks the app to do, taken once a frame by the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Start the server, as the Overview's Start does.
    StartServer,
    /// Open this page.
    Go(crate::app::Tab),
    /// Open Diagnostics and run its checks.
    RunDiagnostics,
    /// Open Pair on Computer (UX 2).
    PairComputer,
}

fn ask_id() -> egui::Id {
    egui::Id::new("pocketdesk-app-ask")
}

/// Ask the app for `ask`, at the end of this frame.
pub fn ask(ctx: &egui::Context, ask: Ask) {
    ctx.data_mut(|d| d.insert_temp(ask_id(), ask));
}

/// What a page asked for this frame, once.
pub fn take_ask(ctx: &egui::Context) -> Option<Ask> {
    ctx.data_mut(|d| {
        let ask = d.get_temp::<Ask>(ask_id());
        d.remove::<Ask>(ask_id());
        ask
    })
}

/// A muted line saying why the control before it is off, and a button for
/// its fix (`fix`, by its label); true when the fix was pressed. Hover text
/// alone reaches neither the keyboard nor a touch screen.
pub fn why_off(ui: &mut egui::Ui, reason: &str, fix: Option<&str>) -> bool {
    let mut pressed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(reason).color(crate::theme::muted(ui)));
        if let Some(fix) = fix {
            pressed = ui.small_button(fix).clicked();
        }
    });
    pressed
}

/// [`why_off`] whose fix the app does: `ask`, under the label `fix`.
pub fn why_off_ask(ui: &mut egui::Ui, reason: &str, fix: &str, ask: Ask) {
    if why_off(ui, reason, Some(fix)) {
        self::ask(ui.ctx(), ask);
    }
}

/// An empty page's card (UX 4): what shows here, as a strong line; how to
/// make it show, as a muted one; then the caller's button, where one
/// action does it.
pub fn empty<R>(
    ui: &mut egui::Ui,
    title: &str,
    how: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    crate::theme::card(ui, |ui| {
        ui.label(
            RichText::new(title)
                .size(crate::theme::CARD_TITLE_TEXT)
                .strong(),
        );
        ui.label(RichText::new(how).color(crate::theme::muted(ui)));
        add(ui)
    })
}

/// The fix for "the server is not running".
pub const START: &str = "Start the server";

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;

    /// The reason shows as a line, and its fix asks the app once.
    #[test]
    fn a_reason_shows_with_its_fix() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            why_off_ask(ui, "The server is not running.", START, Ask::StartServer);
        });
        harness.run();
        assert!(
            harness
                .query_by_label("The server is not running.")
                .is_some()
        );
        harness.get_by_label(START).click();
        harness.run();
        assert_eq!(take_ask(&harness.ctx), Some(Ask::StartServer));
        assert_eq!(take_ask(&harness.ctx), None, "once");
    }
}
