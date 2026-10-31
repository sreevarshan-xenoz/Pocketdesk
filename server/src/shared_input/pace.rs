//! Moves paced at the driver (plan item 3a): a 1 kHz mouse made about a
//! thousand datagrams a second, each a send on the link and an injection
//! there. A move to the computer driven goes at most every
//! [`MOVE_EVERY`]; one that comes sooner waits, and a newer move replaces
//! it. That is safe because a [`Msg::Move`] is where the virtual cursor is
//! (normalised over that computer's screens), not how far it went: the
//! driver still carries every captured move onto its cursor, and only the
//! sends between are left out. Anything else that goes to another
//! computer (a key, a button, the wheel, a crossing, a ping) lets a
//! waiting move go first, so the order the driver meant is the order on
//! the wire. Behind "Keep the connection quick" (`shared_input_low_latency`):
//! with it off every move goes as it comes.

use std::time::{Duration, Instant};

use super::drive::Action;
use super::wire::Msg;

/// The least time between two moves sent: 500 a second, still finer than
/// a 240 Hz screen draws.
pub const MOVE_EVERY: Duration = Duration::from_millis(2);

/// Holds back moves sent sooner than [`MOVE_EVERY`] after the last.
#[derive(Debug, Default)]
pub struct MovePacer {
    /// [`MOVE_EVERY`], or zero while pacing is off.
    every: Duration,
    /// The newest move waiting, and the computer it is for.
    held: Option<(String, Msg)>,
    /// When the last move went.
    sent_at: Option<Instant>,
    /// Moves left out for a newer one, for the log.
    folded: u64,
}

impl MovePacer {
    /// Pacing on (`on`, the low-latency switch) or every move as it comes.
    pub fn new(on: bool) -> MovePacer {
        let mut pacer = MovePacer::default();
        pacer.set(on);
        pacer
    }

    /// Turn pacing on or off: a move waiting goes at the next [`due`]
    /// once it is off.
    ///
    /// [`due`]: MovePacer::due
    pub fn set(&mut self, on: bool) {
        self.every = if on { MOVE_EVERY } else { Duration::ZERO };
    }

    /// What of `actions` is done now, at `now`: a move waits while the last
    /// went less than [`MOVE_EVERY`] ago (a newer one in its place), and
    /// anything that goes to another computer lets it go first.
    pub fn pace(&mut self, actions: Vec<Action>, now: Instant) -> Vec<Action> {
        let mut out = Vec::with_capacity(actions.len() + 1);
        for action in actions {
            match action {
                Action::Send(to, msg @ Msg::Move { .. }) => {
                    if self.held.as_ref().is_some_and(|(held, _)| *held != to) {
                        self.release(now, &mut out);
                    }
                    if let Some((_, waiting)) = self.held.as_mut() {
                        *waiting = msg;
                        self.folded += 1;
                    } else if self.may_send(now) {
                        self.sent_at = Some(now);
                        out.push(Action::Send(to, msg));
                    } else {
                        self.held = Some((to, msg));
                    }
                }
                // Done here: their order against a move is no other
                // computer's concern.
                Action::ParkCursor(..)
                | Action::HideCursor(_)
                | Action::KeepAwake(_)
                | Action::Badge(_) => out.push(action),
                // Home with nothing sent (a link gone, a refusal): a move
                // still waiting is for the computer left, and goes nowhere.
                Action::WarpHome(..) => {
                    self.held = None;
                    out.push(action);
                }
                other => {
                    self.release(now, &mut out);
                    out.push(other);
                }
            }
        }
        out
    }

    /// The move waiting, once its time came.
    pub fn due(&mut self, now: Instant) -> Option<Action> {
        if self.held.is_some() && self.may_send(now) {
            let mut out = Vec::with_capacity(1);
            self.release(now, &mut out);
            return out.pop();
        }
        None
    }

    /// When the move waiting may go, if one waits.
    pub fn deadline(&self) -> Option<Instant> {
        self.held.as_ref()?;
        Some(self.sent_at.map_or_else(Instant::now, |at| at + self.every))
    }

    /// A send to `to` failed: its move waiting goes nowhere.
    pub fn forget(&mut self, to: &str) {
        if self.held.as_ref().is_some_and(|(held, _)| held == to) {
            self.held = None;
        }
    }

    /// How many moves were left out for a newer one so far.
    pub fn folded(&self) -> u64 {
        self.folded
    }

    fn may_send(&self, now: Instant) -> bool {
        self.sent_at
            .is_none_or(|at| now.saturating_duration_since(at) >= self.every)
    }

    fn release(&mut self, now: Instant, out: &mut Vec<Action>) {
        if let Some((to, msg)) = self.held.take() {
            self.sent_at = Some(now);
            out.push(Action::Send(to, msg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(to: &str, seq: u32) -> Action {
        Action::Send(to.to_string(), Msg::Move { seq, x: 0, y: 0 })
    }

    fn key(to: &str) -> Action {
        Action::Send(
            to.to_string(),
            Msg::Key {
                code: 30,
                pressed: true,
            },
        )
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// The first move goes at once; those within 2 ms wait, the newest
    /// replacing the rest, and go when the time comes.
    #[test]
    fn moves_go_every_two_ms_the_newest_kept() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(true);
        assert_eq!(pacer.pace(vec![mv("b", 1)], t), vec![mv("b", 1)]);
        assert!(pacer.pace(vec![mv("b", 2)], t + ms(1)).is_empty());
        assert!(pacer.pace(vec![mv("b", 3)], t + ms(1)).is_empty());
        assert_eq!(pacer.deadline(), Some(t + ms(2)));
        assert_eq!(pacer.due(t + ms(1)), None, "not yet");
        assert_eq!(pacer.due(t + ms(2)), Some(mv("b", 3)));
        assert_eq!(pacer.due(t + ms(9)), None, "nothing waits");
        assert_eq!(pacer.deadline(), None);
        assert_eq!(pacer.folded(), 1);
        // A move after a pause goes at once.
        assert_eq!(pacer.pace(vec![mv("b", 4)], t + ms(9)), vec![mv("b", 4)]);
    }

    /// A key, button, wheel turn or crossing never overtakes a move before
    /// it, and nothing but moves is ever left out.
    #[test]
    fn anything_else_lets_the_waiting_move_go_first() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(true);
        pacer.pace(vec![mv("b", 1)], t);
        let out = pacer.pace(
            vec![
                mv("b", 2),
                Action::ParkCursor(5, 5),
                key("b"),
                mv("b", 3),
                Action::Send("b".into(), Msg::Leave),
            ],
            t + ms(1),
        );
        assert_eq!(
            out,
            vec![
                Action::ParkCursor(5, 5),
                mv("b", 2),
                key("b"),
                mv("b", 3),
                Action::Send("b".into(), Msg::Leave),
            ],
            "the park is this computer's own; the rest in order"
        );
        assert_eq!(pacer.due(t + ms(10)), None);
    }

    /// A move for another computer (a crossing on) lets the last
    /// computer's move go first.
    #[test]
    fn a_move_for_another_computer_sends_the_waiting_one() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(true);
        pacer.pace(vec![mv("b", 1)], t);
        pacer.pace(vec![mv("b", 2)], t + ms(1));
        let out = pacer.pace(vec![mv("c", 1)], t + ms(1));
        assert_eq!(out, vec![mv("b", 2)], "c's waits its turn");
        assert_eq!(pacer.due(t + ms(3)), Some(mv("c", 1)));
    }

    /// Off, every move goes as it comes; turned off with one waiting, it
    /// goes at once.
    #[test]
    fn off_every_move_goes() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(false);
        for seq in 1..5 {
            assert_eq!(pacer.pace(vec![mv("b", seq)], t), vec![mv("b", seq)]);
        }
        let mut pacer = MovePacer::new(true);
        pacer.pace(vec![mv("b", 1)], t);
        pacer.pace(vec![mv("b", 2)], t);
        pacer.set(false);
        assert_eq!(pacer.due(t), Some(mv("b", 2)));
    }

    /// Coming home with nothing sent to the computer left (its link went)
    /// drops the move waiting for it; home after a leave sent it first.
    #[test]
    fn home_without_a_leave_drops_the_waiting_move() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(true);
        pacer.pace(vec![mv("b", 1)], t);
        pacer.pace(vec![mv("b", 2)], t);
        let out = pacer.pace(vec![Action::WarpHome(1, 1)], t);
        assert_eq!(out, vec![Action::WarpHome(1, 1)]);
        assert_eq!(pacer.due(t + ms(5)), None);
        pacer.pace(vec![mv("b", 3)], t + ms(5));
        pacer.pace(vec![mv("b", 4)], t + ms(5));
        let leave = Action::Send("b".into(), Msg::Leave);
        let out = pacer.pace(vec![leave.clone(), Action::WarpHome(1, 1)], t + ms(5));
        assert_eq!(out, vec![mv("b", 4), leave, Action::WarpHome(1, 1)]);
    }

    /// A failed send to a computer drops its waiting move.
    #[test]
    fn a_failed_computer_s_move_is_forgotten() {
        let t = Instant::now();
        let mut pacer = MovePacer::new(true);
        pacer.pace(vec![mv("b", 1)], t);
        pacer.pace(vec![mv("b", 2)], t);
        pacer.forget("c");
        assert!(pacer.deadline().is_some());
        pacer.forget("b");
        assert_eq!(pacer.due(t + ms(5)), None);
    }
}
