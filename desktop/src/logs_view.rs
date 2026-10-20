//! The Logs page (J1): the newest server log's last lines, read only when
//! the file changed, with a filter, the warnings alone, colour by level,
//! Copy and the way to the file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{self, RichText};

/// How many lines are kept from the log's end.
const LINES: usize = 400;
/// How often the file's size and time are looked at while the page is open.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// A line's level, as the server's log writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Other,
}

/// The level a log line was written at: the first level word on it.
pub fn level(line: &str) -> Level {
    for word in line.split_whitespace().take(4) {
        match word {
            "ERROR" => return Level::Error,
            "WARN" => return Level::Warn,
            // The line's own level came first: its text may say WARN later.
            "INFO" | "DEBUG" | "TRACE" => return Level::Other,
            _ => {}
        }
    }
    Level::Other
}

/// The lines shown: those holding every word of `filter` (case ignored),
/// and only warnings and errors with `warnings_only`.
pub fn shown<'a>(lines: &'a [String], filter: &str, warnings_only: bool) -> Vec<&'a str> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| !warnings_only || level(line) != Level::Other)
        .filter(|line| {
            let lower = line.to_lowercase();
            words.iter().all(|w| lower.contains(w.as_str()))
        })
        .collect()
}

#[derive(Default)]
pub struct LogsView {
    lines: Vec<String>,
    /// What the file was when it was last read.
    read: Option<(PathBuf, u64, SystemTime)>,
    looked_at: Option<Instant>,
    filter: String,
    warnings_only: bool,
    copied: Option<Instant>,
    /// What it said happened, until the app takes it (E5).
    notice: Option<crate::notice::Notice>,
}

impl LogsView {
    /// What this page said happened since the last call, for the app's one
    /// notice (E5).
    pub fn take_notice(&mut self) -> Option<crate::notice::Notice> {
        self.notice.take()
    }

    /// Read the log again if it changed since it was last read.
    fn refresh(&mut self, state_dir: &Path) {
        if self.looked_at.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked_at = Some(Instant::now());
        let stamp = crate::model::log_stamp(state_dir);
        if stamp != self.read {
            self.lines = crate::model::log_tail(state_dir, LINES);
            self.read = stamp;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state_dir: &Path) {
        self.refresh(state_dir);
        ui.ctx().request_repaint_after(LOOK_EVERY);
        if self.lines.is_empty() {
            ui.label(
                "No log file yet. The server writes one when it starts at logon \
                 (`start --background`); started from a terminal it logs there instead.",
            );
            return;
        }
        let shown = shown(&self.lines, &self.filter, self.warnings_only);
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter")
                    .desired_width(220.0),
            );
            ui.checkbox(&mut self.warnings_only, "Warnings and errors only");
            if ui.button("Copy").on_hover_text("The lines shown").clicked() {
                ui.ctx().copy_text(shown.join("\n"));
                self.copied = Some(Instant::now());
            }
            crate::notice::copied(ui, &mut self.copied);
            if let Some((path, _, _)) = &self.read
                && ui.button("Open log folder").clicked()
                && let Err(e) = crate::reveal::show_in_folder(path)
            {
                self.notice =
                    crate::notice::Notice::error(format!("Could not open the folder: {e}"));
            }
        });
        ui.label(
            RichText::new(format!(
                "{} of the last {} lines",
                shown.len(),
                self.lines.len()
            ))
            .color(crate::theme::muted(ui)),
        );
        ui.add_space(4.0);
        let (warn, bad) = (crate::theme::warn(ui), crate::theme::bad(ui));
        egui::ScrollArea::both()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in shown {
                    let text = RichText::new(line).monospace();
                    ui.label(match level(line) {
                        Level::Error => text.color(bad),
                        Level::Warn => text.color(warn),
                        Level::Other => text,
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARN: &str =
        "2026-10-20T06:13:25Z  WARN pocketdesk_server::plugins::pair: Session refused";
    const ERROR: &str =
        "2026-10-20T06:13:25Z ERROR pocketdesk_server::iroh_endpoint: WAN session error";
    const INFO: &str = "2026-10-20T06:13:25Z  INFO pocketdesk: Pocketdesk server starting";

    #[test]
    fn a_lines_level_is_the_word_the_server_wrote() {
        assert_eq!(level(WARN), Level::Warn);
        assert_eq!(level(ERROR), Level::Error);
        assert_eq!(level(INFO), Level::Other);
        assert_eq!(
            level("INFO something said WARN later in its text"),
            Level::Other
        );
    }

    #[test]
    fn the_filter_and_the_warnings_switch_narrow_the_lines() {
        let lines: Vec<String> = [WARN, ERROR, INFO].iter().map(|s| s.to_string()).collect();
        assert_eq!(shown(&lines, "", false).len(), 3);
        assert_eq!(
            shown(&lines, "", true),
            [WARN, ERROR],
            "warnings and errors only"
        );
        assert_eq!(
            shown(&lines, "server STARTING", false),
            [INFO],
            "every word, case ignored"
        );
        assert_eq!(
            shown(&lines, "session", true),
            [WARN, ERROR],
            "both together"
        );
        assert!(shown(&lines, "nothing like this", false).is_empty());
    }
}
