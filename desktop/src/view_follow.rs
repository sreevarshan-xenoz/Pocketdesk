//! The clipboard follows the window that shows another computer's screen
//! (PC3).
//!
//! **Clipboard:** while the window controls that computer, its focus is
//! said to the server ([`pocketdesk_core::local::view::FOCUS`]) as it comes
//! and goes, settled first ([`FocusEdges`]) so a flicker says nothing. The
//! server pushes this computer's clipboard there as the focus comes, and
//! asks for that computer's back as it goes.

use std::time::{Duration, Instant};

/// How long the focus holds before it is said: a flicker (a menu, a
/// notification taking it for a moment) says nothing.
pub const SETTLE: Duration = Duration::from_millis(300);

/// A change of the window's focus, as said to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The window gained the focus while it controls the other computer.
    Gain,
    /// It lost it (or stopped controlling) after a [`Edge::Gain`].
    Lose,
}

/// Turns each pass's "inside" (focused while it controls the other
/// computer) into edges for the server, each only once it has held for
/// [`SETTLE`]. A [`Edge::Lose`] comes only after a [`Edge::Gain`].
#[derive(Debug, Default)]
pub struct FocusEdges {
    /// What was said last: inside or not.
    said: bool,
    /// What the passes see now, and since when.
    seen: bool,
    since: Option<Instant>,
}

impl FocusEdges {
    /// One pass: `inside` at `now`. The edge to say, if any.
    pub fn observe(&mut self, inside: bool, now: Instant) -> Option<Edge> {
        if inside != self.seen || self.since.is_none() {
            self.seen = inside;
            self.since = Some(now);
        }
        let held = self
            .since
            .is_some_and(|since| now.saturating_duration_since(since) >= SETTLE);
        if self.seen == self.said || !held {
            return None;
        }
        self.said = self.seen;
        Some(if self.said { Edge::Gain } else { Edge::Lose })
    }

    /// Whether a change waits to settle: the window looks again after
    /// [`SETTLE`].
    pub fn pending(&self) -> bool {
        self.seen != self.said
    }

    /// The window closes or its view stops: a [`Edge::Lose`] when a
    /// [`Edge::Gain`] was said, at once; either way it starts afresh.
    pub fn leave(&mut self) -> Option<Edge> {
        let was = self.said;
        *self = Self::default();
        was.then_some(Edge::Lose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_focus_that_holds_is_said_once_as_it_comes_and_as_it_goes() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        assert_eq!(edges.observe(false, t), None, "outside: nothing to say");
        assert_eq!(edges.observe(true, t + ms(10)), None, "not settled yet");
        assert!(edges.pending());
        assert_eq!(edges.observe(true, t + ms(200)), None);
        assert_eq!(edges.observe(true, t + ms(310)), Some(Edge::Gain));
        assert!(!edges.pending());
        assert_eq!(edges.observe(true, t + ms(900)), None, "said once");
        assert_eq!(edges.observe(false, t + ms(1000)), None);
        assert_eq!(edges.observe(false, t + ms(1300)), Some(Edge::Lose));
        assert_eq!(edges.observe(false, t + ms(5000)), None);
    }

    #[test]
    fn a_flicker_of_the_focus_says_nothing() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        edges.observe(true, t);
        assert_eq!(edges.observe(true, t + SETTLE), Some(Edge::Gain));
        // A menu takes the focus for a moment, and gives it back.
        assert_eq!(edges.observe(false, t + ms(1000)), None);
        assert_eq!(edges.observe(true, t + ms(1100)), None);
        assert_eq!(edges.observe(true, t + ms(2000)), None, "still inside");
        assert!(!edges.pending());
        // The window opened unfocused and flickered in: nothing either.
        let mut edges = FocusEdges::default();
        edges.observe(true, t);
        assert_eq!(edges.observe(false, t + ms(100)), None);
        assert_eq!(
            edges.observe(false, t + ms(1000)),
            None,
            "no lose before a gain"
        );
    }

    #[test]
    fn leaving_says_lose_only_after_a_gain() {
        let t = Instant::now();
        let mut edges = FocusEdges::default();
        assert_eq!(edges.leave(), None);
        edges.observe(true, t);
        edges.observe(true, t + SETTLE);
        assert_eq!(edges.leave(), Some(Edge::Lose));
        assert_eq!(edges.leave(), None, "once");
        // Afresh: the next view gains again.
        edges.observe(true, t + ms(1000));
        assert_eq!(edges.observe(true, t + ms(1000) + SETTLE), Some(Edge::Gain));
    }
}
