//! One line saying what just happened (U1): done, information or an error,
//! each with its colour; gone after a while, at the next one, or with its
//! close button. And "Copied." beside a copy button.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};

/// What a notice says happened, given where it is made (U1 review: guessed
/// from its words, real errors had shown as done, and successes naming a
/// file such as "failed-login.log" as errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// It worked, or was asked for.
    Done,
    /// It did not work.
    Error,
    /// Work under way (a start, a restart, a setting's change), shown until
    /// its outcome replaces it.
    Working,
}

impl Kind {
    /// How long a notice of this kind stays. Work under way stays until its
    /// outcome replaces it, but never past a few minutes (a program that
    /// hung would otherwise leave it for good).
    pub fn lasts(self) -> Duration {
        match self {
            Kind::Done => Duration::from_secs(6),
            Kind::Error => Duration::from_secs(12),
            Kind::Working => Duration::from_secs(300),
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
    fn new(kind: Kind, text: impl Into<String>) -> Option<Notice> {
        Some(Notice {
            text: text.into(),
            kind,
            since: Instant::now(),
        })
    }

    /// It worked, or was asked for.
    pub fn done(text: impl Into<String>) -> Option<Notice> {
        Self::new(Kind::Done, text)
    }

    /// It did not work.
    pub fn error(text: impl Into<String>) -> Option<Notice> {
        Self::new(Kind::Error, text)
    }

    /// Work under way, until its outcome replaces it.
    pub fn working(text: impl Into<String>) -> Option<Notice> {
        Self::new(Kind::Working, text)
    }

    /// Done or an error, as `outcome` says.
    pub fn outcome(outcome: Result<String, String>) -> Option<Notice> {
        match outcome {
            Ok(text) => Self::done(text),
            Err(text) => Self::error(text),
        }
    }

    /// Whether it still shows at `now`.
    pub fn showing(&self, now: Instant) -> bool {
        now.duration_since(self.since) < self.lasts()
    }

    /// Its kind's time, and for a done or an error a second more per 40
    /// characters, so a long one can be read (I3), to at most 20 s (30 s
    /// for an error).
    fn lasts(&self) -> Duration {
        let base = self.kind.lasts();
        let cap = match self.kind {
            Kind::Done => Duration::from_secs(20),
            Kind::Error => Duration::from_secs(30),
            Kind::Working => return base,
        };
        let extra = Duration::from_secs((self.text.chars().count() / 40) as u64);
        (base + extra).min(cap)
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
        Kind::Working => ui.visuals().text_color(),
        Kind::Error => crate::theme::bad(ui),
    };
    let mut close = false;
    ui.horizontal(|ui| {
        // Work under way says so as it goes (I3).
        if shown.kind == Kind::Working {
            ui.spinner();
        }
        ui.add(egui::Label::new(RichText::new(&shown.text).color(color)).wrap());
        close = ui.small_button("Dismiss").clicked();
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
    fn each_kind_lasts_its_time() {
        let start = Instant::now();
        let at = |kind, secs| {
            Notice {
                text: String::new(),
                kind,
                since: start,
            }
            .showing(start + Duration::from_secs(secs))
        };
        assert!(at(Kind::Done, 5) && !at(Kind::Done, 7));
        assert!(at(Kind::Error, 11) && !at(Kind::Error, 13));
        assert!(
            at(Kind::Working, 60),
            "work under way waits for its outcome"
        );
        assert!(!at(Kind::Working, 301), "but not for good");
    }

    /// I3: a long notice stays long enough to be read, within a bound.
    #[test]
    fn a_long_notice_stays_longer() {
        let start = Instant::now();
        let at = |kind, text: &str, secs| {
            Notice {
                text: text.to_string(),
                kind,
                since: start,
            }
            .showing(start + Duration::from_secs(secs))
        };
        let long = "x".repeat(200);
        assert!(!at(Kind::Done, "Saved.", 7), "a short one as before");
        assert!(at(Kind::Done, &long, 10), "200 characters: about 5 s more");
        assert!(!at(Kind::Done, &long, 12));
        assert!(at(Kind::Error, &long, 16));
        let endless = "x".repeat(10_000);
        assert!(!at(Kind::Done, &endless, 21), "never past 20 s");
    }

    #[test]
    fn an_outcome_is_done_or_an_error() {
        assert_eq!(
            Notice::outcome(Ok("Saved.".into())).unwrap().kind,
            Kind::Done
        );
        let failed = Notice::outcome(Err("Sending failed-login.log: x".into())).unwrap();
        assert_eq!(failed.kind, Kind::Error);
        assert_eq!(
            Notice::done("Sent failed-login.log.").unwrap().kind,
            Kind::Done,
            "a file's name says nothing of the kind"
        );
    }
}
