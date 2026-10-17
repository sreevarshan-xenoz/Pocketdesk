//! One line saying what just happened (U1): done, information or an error,
//! each with its colour; gone after a while, at the next one, or with its
//! close button. And "Copied." beside a copy button.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

/// What a notice says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Done,
    Info,
    Error,
}

impl Kind {
    /// The kind a message's words say: the app's failures begin "Not …",
    /// "Could not …" or say "failed", and work under way ends in "…".
    pub fn of(text: &str) -> Kind {
        let lower = text.to_lowercase();
        if lower.starts_with("not ")
            || lower.starts_with("could not")
            || lower.contains("failed")
            || lower.contains(": not ")
        {
            Kind::Error
        } else if text.trim_end().ends_with('…') {
            Kind::Info
        } else {
            Kind::Done
        }
    }

    /// How long a notice of this kind stays.
    pub fn lasts(self) -> Duration {
        match self {
            Kind::Error => Duration::from_secs(12),
            Kind::Done | Kind::Info => Duration::from_secs(6),
        }
    }
}

/// A notice and when it was given.
#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub kind: Kind,
    pub since: Instant,
}

impl Notice {
    /// A notice saying `text`, its kind from its words.
    pub fn say(text: impl Into<String>) -> Option<Notice> {
        let text = text.into();
        Some(Notice {
            kind: Kind::of(&text),
            text,
            since: Instant::now(),
        })
    }

    /// Whether it still shows at `now`. Work under way stays until what
    /// follows it replaces it.
    pub fn showing(&self, now: Instant) -> bool {
        self.kind == Kind::Info && self.text.trim_end().ends_with('…')
            || now.duration_since(self.since) < self.kind.lasts()
    }
}

/// Show `notice` while it lasts, with a close button; it is cleared once
/// gone or closed.
pub fn show(ui: &mut egui::Ui, notice: &mut Option<Notice>) {
    let now = Instant::now();
    let Some(shown) = notice.as_ref().filter(|n| n.showing(now)) else {
        *notice = None;
        return;
    };
    let color = match shown.kind {
        Kind::Done => crate::theme::good(ui),
        Kind::Info => ui.visuals().text_color(),
        Kind::Error => crate::theme::bad(ui),
    };
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(&shown.text).color(color));
        close = ui.small_button("✕").on_hover_text("Close").clicked();
    });
    if close {
        *notice = None;
    } else {
        // Repaint when it is due to go.
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

/// How long "Copied." shows beside a copy button.
const COPIED_FOR: Duration = Duration::from_secs(3);

/// "Copied." beside a copy button pressed at `at`, for a few seconds.
pub fn copied(ui: &mut egui::Ui, at: &mut Option<Instant>) {
    if at.is_some_and(|t| t.elapsed() < COPIED_FOR) {
        ui.label(RichText::new("Copied.").color(crate::theme::good(ui)));
        ui.ctx().request_repaint_after(COPIED_FOR);
    } else {
        *at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_an_error_done_or_under_way_by_its_words() {
        for failed in [
            "Not saved: the disk is full",
            "Not changed: the server program is not beside this app",
            "Could not show the file: gone",
            "Restart: failed.",
            "Send: not sent",
        ] {
            assert_eq!(Kind::of(failed), Kind::Error, "{failed}");
        }
        for done in ["Saved.", "Restart: done.", "Added to the file manager."] {
            assert_eq!(Kind::of(done), Kind::Done, "{done}");
        }
        assert_eq!(Kind::of("Restart…"), Kind::Info);
    }

    #[test]
    fn a_notice_goes_after_its_time_and_work_under_way_stays() {
        let start = Instant::now();
        let done = Notice {
            text: "Saved.".into(),
            kind: Kind::Done,
            since: start,
        };
        assert!(done.showing(start + Duration::from_secs(5)));
        assert!(!done.showing(start + Duration::from_secs(7)));
        let failed = Notice {
            text: "Not saved: x".into(),
            kind: Kind::Error,
            since: start,
        };
        assert!(failed.showing(start + Duration::from_secs(11)));
        assert!(!failed.showing(start + Duration::from_secs(13)));
        let busy = Notice {
            text: "Restart…".into(),
            kind: Kind::Info,
            since: start,
        };
        assert!(busy.showing(start + Duration::from_secs(60)));
    }
}
