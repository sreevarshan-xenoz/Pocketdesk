//! The keys and buttons a computer has sent as down, so they can be let go of.
//!
//! A computer driving another keeps one of these for what it has sent, and
//! one for what it has injected when it is the one driven. Whenever control
//! moves, or the link fails, [`HeldInput::releases`] says what to let go of
//! so nothing is left stuck down on the other side.

use super::wire::Msg;

/// What is down: keys (evdev codes) and buttons, each in the order they went
/// down.
#[derive(Debug, Default)]
pub struct HeldInput {
    keys: Vec<u16>,
    buttons: Vec<u8>,
}

impl HeldInput {
    /// Take note of a message that is sent or injected. Only keys, buttons
    /// and [`Msg::ReleaseAll`] change what is held; a key going down while it
    /// is already down (a repeat) changes nothing.
    pub fn note(&mut self, msg: &Msg) {
        match *msg {
            Msg::Key { code, pressed } => set(&mut self.keys, code, pressed),
            Msg::Button { button, pressed } => set(&mut self.buttons, button, pressed),
            Msg::ReleaseAll => {
                self.keys.clear();
                self.buttons.clear();
            }
            _ => {}
        }
    }

    /// The messages that let go of everything held, keys first, and then
    /// nothing is held. Each group is released last pressed first, so a
    /// modifier comes up after the key it was modifying.
    pub fn releases(&mut self) -> Vec<Msg> {
        let keys = self.keys.drain(..).rev().map(|code| Msg::Key {
            code,
            pressed: false,
        });
        let buttons = self.buttons.drain(..).rev().map(|button| Msg::Button {
            button,
            pressed: false,
        });
        keys.chain(buttons).collect()
    }
}

fn set<T: PartialEq>(held: &mut Vec<T>, item: T, down: bool) {
    let at = held.iter().position(|h| *h == item);
    match (down, at) {
        (true, None) => held.push(item),
        (false, Some(i)) => {
            held.remove(i);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u16, pressed: bool) -> Msg {
        Msg::Key { code, pressed }
    }

    fn button(button: u8, pressed: bool) -> Msg {
        Msg::Button { button, pressed }
    }

    #[test]
    fn releases_everything_held_once() {
        let mut held = HeldInput::default();
        held.note(&key(30, true));
        held.note(&key(42, true));
        held.note(&button(0, true));
        held.note(&key(30, false));
        assert_eq!(held.releases(), vec![key(42, false), button(0, false)]);
        assert_eq!(held.releases(), Vec::<Msg>::new());
    }

    #[test]
    fn a_repeating_key_is_released_once() {
        let mut held = HeldInput::default();
        for _ in 0..5 {
            held.note(&key(30, true));
            held.note(&button(1, true));
            held.note(&button(1, true));
        }
        assert_eq!(held.releases(), vec![key(30, false), button(1, false)]);
    }

    #[test]
    fn the_last_pressed_is_released_first() {
        // Shift down, then a letter: the letter comes up before Shift does.
        let mut held = HeldInput::default();
        held.note(&key(42, true));
        held.note(&key(30, true));
        held.note(&button(2, true));
        held.note(&button(0, true));
        assert_eq!(
            held.releases(),
            vec![
                key(30, false),
                key(42, false),
                button(0, false),
                button(2, false)
            ]
        );
    }

    #[test]
    fn release_all_and_other_messages() {
        let mut held = HeldInput::default();
        held.note(&key(30, true));
        held.note(&button(0, true));
        // Nothing but keys, buttons and the release changes what is held;
        // in particular leaving a screen does not forget what to release.
        held.note(&Msg::Leave);
        held.note(&Msg::Move { seq: 1, x: 2, y: 3 });
        held.note(&Msg::Wheel { dx: 0, dy: 1 });
        assert_eq!(held.releases().len(), 2);

        held.note(&key(30, true));
        held.note(&button(0, true));
        held.note(&Msg::ReleaseAll);
        assert_eq!(held.releases(), Vec::<Msg>::new());
    }

    #[test]
    fn releasing_what_was_never_pressed_is_ignored() {
        let mut held = HeldInput::default();
        held.note(&key(30, false));
        held.note(&button(3, false));
        assert_eq!(held.releases(), Vec::<Msg>::new());
    }
}
