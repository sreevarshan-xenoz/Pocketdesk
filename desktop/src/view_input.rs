//! Another computer's screen in a window (BK7): what the window's pointer
//! and keys become for that computer, as the input packets a phone sends.
//!
//! The pointer goes as absolute moves over the picture (0..=65535 each
//! way), at most one each [`MOVE_EVERY`], with the buttons and the wheel.
//! Typing goes as text; the keys that type nothing (Enter, the arrows, F1
//! and the rest) and every key pressed with Ctrl, Alt or the Windows key go
//! as Linux evdev key codes, the modifiers held as keys. Shift is held on the
//! other computer only while such a key, a button or the wheel needs it, so
//! text typed with Shift arrives as its capitals and not shifted twice.
//! egui turns Ctrl+C, Ctrl+X and Ctrl+V (and on Windows Ctrl+Insert,
//! Shift+Insert and Shift+Delete, and the keyboard's own Copy, Cut and
//! Paste keys) into copy, cut and paste, keeping the key's press, and says
//! paste only when this computer's clipboard holds text: they go back to
//! being those keys there, whatever this clipboard holds. Whatever is held is let go when the window
//! loses focus, closes or stops controlling.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::streaming::InputPacket;

use crate::video::fraction;

/// Moves go at most this often: a 1000 Hz mouse sends one a pass, not one a
/// millisecond.
pub const MOVE_EVERY: Duration = Duration::from_millis(8);

/// Linux evdev codes for the modifiers.
pub const LEFT_CTRL: u16 = 29;
pub const LEFT_SHIFT: u16 = 42;
pub const RIGHT_SHIFT: u16 = 54;
pub const LEFT_ALT: u16 = 56;
pub const RIGHT_CTRL: u16 = 97;
pub const RIGHT_ALT: u16 = 100;
pub const LEFT_META: u16 = 125;
pub const RIGHT_META: u16 = 126;

const KEY_C: u16 = 46;
const KEY_V: u16 = 47;
const KEY_X: u16 = 45;
const KEY_INSERT: u16 = 110;
const KEY_DELETE: u16 = 111;

/// What egui made of a copy, cut or paste key: it keeps the key's press to
/// itself, and says only this (BK7 review I2, I3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clip {
    Copy,
    Cut,
    Paste,
}

impl Clip {
    /// The keys it may have been pressed as, with Ctrl or Shift held.
    fn keys(self) -> [u16; 2] {
        match self {
            Clip::Copy => [KEY_C, KEY_INSERT],
            Clip::Cut => [KEY_X, KEY_DELETE],
            Clip::Paste => [KEY_V, KEY_INSERT],
        }
    }

    fn letter(self) -> u16 {
        self.keys()[0]
    }
}

/// The Linux evdev code of `key` (where it sits on a US keyboard), or
/// `None` for one with none here.
pub fn evdev(key: egui::Key) -> Option<u16> {
    use egui::Key::*;
    Some(match key {
        Escape => 1,
        Num1 => 2,
        Num2 => 3,
        Num3 => 4,
        Num4 => 5,
        Num5 => 6,
        Num6 => 7,
        Num7 => 8,
        Num8 => 9,
        Num9 => 10,
        Num0 => 11,
        Minus => 12,
        Equals | Plus => 13,
        Backspace => 14,
        Tab => 15,
        Q => 16,
        W => 17,
        E => 18,
        R => 19,
        T => 20,
        Y => 21,
        U => 22,
        I => 23,
        O => 24,
        P => 25,
        OpenBracket | OpenCurlyBracket => 26,
        CloseBracket | CloseCurlyBracket => 27,
        Enter => 28,
        ControlLeft => LEFT_CTRL,
        A => 30,
        S => 31,
        D => 32,
        F => 33,
        G => 34,
        H => 35,
        J => 36,
        K => 37,
        L => 38,
        Semicolon | Colon => 39,
        Quote => 40,
        Backtick => 41,
        ShiftLeft => LEFT_SHIFT,
        Backslash | Pipe => 43,
        Z => 44,
        X => KEY_X,
        C => KEY_C,
        V => KEY_V,
        B => 48,
        N => 49,
        M => 50,
        Comma => 51,
        Period => 52,
        Slash | Questionmark => 53,
        ShiftRight => RIGHT_SHIFT,
        AltLeft => LEFT_ALT,
        Space => 57,
        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        IntlBackslash => 86,
        F11 => 87,
        F12 => 88,
        ControlRight => RIGHT_CTRL,
        AltRight => RIGHT_ALT,
        Home => 102,
        ArrowUp => 103,
        PageUp => 104,
        ArrowLeft => 105,
        ArrowRight => 106,
        End => 107,
        ArrowDown => 108,
        PageDown => 109,
        Insert => 110,
        Delete => 111,
        SuperLeft => LEFT_META,
        SuperRight => RIGHT_META,
        F13 => 183,
        F14 => 184,
        F15 => 185,
        F16 => 186,
        F17 => 187,
        F18 => 188,
        F19 => 189,
        F20 => 190,
        F21 => 191,
        F22 => 192,
        F23 => 193,
        F24 => 194,
        BrowserBack => 158,
        _ => return None,
    })
}

const KEY_ESC: u16 = 1;
const KEY_TAB: u16 = 15;
const KEY_L: u16 = 38;
const KEY_F4: u16 = 62;
const KEY_SYSRQ: u16 = 99;

/// A chord the window's Send keys menu sends to the other computer (PC4):
/// the keys this computer keeps to itself, or that a person cannot press
/// through the window at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKey {
    Win,
    AltTab,
    AltF4,
    PrintScreen,
    /// Linux only: Windows takes Ctrl+Alt+Del only from a SYSTEM service.
    CtrlAltDel,
    /// Windows' Task Manager, in place of Ctrl+Alt+Del.
    CtrlShiftEsc,
    /// Windows+L.
    Lock,
}

impl SendKey {
    /// The menu's words for it.
    pub fn label(self) -> &'static str {
        match self {
            SendKey::Win => "Windows key (Super)",
            SendKey::AltTab => "Alt+Tab",
            SendKey::AltF4 => "Alt+F4",
            SendKey::PrintScreen => "Print Screen",
            SendKey::CtrlAltDel => "Ctrl+Alt+Del",
            SendKey::CtrlShiftEsc => "Ctrl+Shift+Esc (Task Manager)",
            SendKey::Lock => "Lock (Windows+L)",
        }
    }

    /// Its keys as evdev codes, in the order they go down.
    pub fn keys(self) -> &'static [u16] {
        match self {
            SendKey::Win => &[LEFT_META],
            SendKey::AltTab => &[LEFT_ALT, KEY_TAB],
            SendKey::AltF4 => &[LEFT_ALT, KEY_F4],
            SendKey::PrintScreen => &[KEY_SYSRQ],
            SendKey::CtrlAltDel => &[LEFT_CTRL, LEFT_ALT, KEY_DELETE],
            SendKey::CtrlShiftEsc => &[LEFT_CTRL, LEFT_SHIFT, KEY_ESC],
            SendKey::Lock => &[LEFT_META, KEY_L],
        }
    }
}

/// The Send keys menu for a computer that runs Windows (`Some(true)`),
/// Linux (`Some(false)`), or one that has not said (`None`: only the keys
/// every system takes).
pub fn send_key_choices(windows: Option<bool>) -> Vec<SendKey> {
    let mut keys = vec![
        SendKey::Win,
        SendKey::AltTab,
        SendKey::AltF4,
        SendKey::PrintScreen,
    ];
    match windows {
        Some(true) => keys.extend([SendKey::CtrlShiftEsc, SendKey::Lock]),
        Some(false) => keys.push(SendKey::CtrlAltDel),
        None => {}
    }
    keys
}

/// Whether pressing `key` alone types something (it arrives as text then).
pub fn types(key: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        key,
        A | B
            | C
            | D
            | E
            | F
            | G
            | H
            | I
            | J
            | K
            | L
            | M
            | N
            | O
            | P
            | Q
            | R
            | S
            | T
            | U
            | V
            | W
            | X
            | Y
            | Z
            | Num0
            | Num1
            | Num2
            | Num3
            | Num4
            | Num5
            | Num6
            | Num7
            | Num8
            | Num9
            | Space
            | Minus
            | Plus
            | Equals
            | OpenBracket
            | CloseBracket
            | OpenCurlyBracket
            | CloseCurlyBracket
            | Semicolon
            | Colon
            | Quote
            | Backtick
            | Backslash
            | Pipe
            | Comma
            | Period
            | Slash
            | Questionmark
            | Exclamationmark
            | IntlBackslash
    )
}

fn is_shift(code: u16) -> bool {
    matches!(code, LEFT_SHIFT | RIGHT_SHIFT)
}

/// Ctrl, Alt or the Windows key: pressed with one of them, a key is a
/// shortcut, not typing.
fn is_chord_modifier(code: u16) -> bool {
    matches!(
        code,
        LEFT_CTRL | RIGHT_CTRL | LEFT_ALT | RIGHT_ALT | LEFT_META | RIGHT_META
    )
}

/// The wire's button for egui's: 0 left, 1 middle, 2 right, 3 back, 4 forward.
pub fn button(button: egui::PointerButton) -> u8 {
    match button {
        egui::PointerButton::Primary => 0,
        egui::PointerButton::Middle => 1,
        egui::PointerButton::Secondary => 2,
        egui::PointerButton::Extra1 => 3,
        egui::PointerButton::Extra2 => 4,
    }
}

/// The window's input on its way to the other computer: what it holds
/// there, and the move waiting for its turn.
#[derive(Debug, Default)]
pub struct WindowInput {
    /// Keys held here, modifiers among them, by evdev code.
    down_here: BTreeSet<u16>,
    /// Keys held on the other computer.
    down_there: BTreeSet<u16>,
    /// Buttons held on the other computer.
    buttons: BTreeSet<u8>,
    /// The last move sent, and when.
    moved_at: Option<Instant>,
    /// A move that waits for its turn.
    pending: Option<(u16, u16)>,
    /// What the wheel turned that is not yet a whole step, each way.
    wheel: egui::Vec2,
    /// Copies, cuts and pastes said with Ctrl or Shift held, waiting for
    /// the release that tells which key made them.
    clips: Vec<Clip>,
    /// Keys whose copy, cut or paste went already: their release is theirs.
    clipped: BTreeSet<u16>,
}

impl WindowInput {
    /// Whether anything is held on the other computer.
    pub fn holds(&self) -> bool {
        !self.down_there.is_empty() || !self.buttons.is_empty()
    }

    /// Whether a move waits for its turn (the window should look again in
    /// [`MOVE_EVERY`]).
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// This pass's `events` over the picture at `picture`, at `now`, as
    /// packets for the other computer.
    pub fn take(
        &mut self,
        events: &[egui::Event],
        picture: egui::Rect,
        now: Instant,
    ) -> Vec<InputPacket> {
        let mut out = Vec::new();
        for event in events {
            match event {
                egui::Event::PointerMoved(pos) => {
                    let clamp = !self.buttons.is_empty();
                    if let Some(at) = fraction(picture, *pos, clamp) {
                        self.pending = Some(at);
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button: which,
                    pressed,
                    ..
                } => {
                    let code = button(*which);
                    if *pressed {
                        let Some(at) = fraction(picture, *pos, false) else {
                            continue;
                        };
                        self.hold_shift(&mut out);
                        self.move_to(at, now, &mut out);
                        self.buttons.insert(code);
                        out.push(InputPacket::MouseClick {
                            button: code,
                            pressed: true,
                        });
                    } else if self.buttons.remove(&code) {
                        if let Some(at) = fraction(picture, *pos, true) {
                            self.move_to(at, now, &mut out);
                        }
                        out.push(InputPacket::MouseClick {
                            button: code,
                            pressed: false,
                        });
                    }
                }
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let scale = match unit {
                        egui::MouseWheelUnit::Line => 1.0,
                        egui::MouseWheelUnit::Page => 3.0,
                        egui::MouseWheelUnit::Point => 1.0 / 50.0,
                    };
                    self.wheel += *delta * scale;
                    let (dx, dy) = (self.wheel.x.trunc(), self.wheel.y.trunc());
                    if dx == 0.0 && dy == 0.0 {
                        continue;
                    }
                    self.wheel -= egui::vec2(dx, dy);
                    self.flush(now, &mut out, true);
                    self.hold_shift(&mut out);
                    // Content that moves down is the wheel turned up (the
                    // wire's positive); content that moves right is the
                    // wheel turned left.
                    out.push(InputPacket::MouseScroll {
                        dx: (-dx).clamp(-10.0, 10.0) as i16,
                        dy: dy.clamp(-10.0, 10.0) as i16,
                    });
                }
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed,
                    ..
                } => {
                    let key = physical_key.unwrap_or(*key);
                    let Some(code) = evdev(key) else { continue };
                    self.key(key, code, *pressed, &mut out);
                }
                egui::Event::Text(text) if !text.is_empty() => {
                    // With Ctrl or Alt held it is a shortcut, sent as keys.
                    if self.down_here.iter().any(|c| is_chord_modifier(*c)) {
                        continue;
                    }
                    for shift in [LEFT_SHIFT, RIGHT_SHIFT] {
                        if self.down_there.remove(&shift) {
                            out.push(InputPacket::KeyEvent {
                                key: shift,
                                pressed: false,
                            });
                        }
                    }
                    out.push(InputPacket::Text(text.clone()));
                }
                egui::Event::Copy => self.clip(Clip::Copy, &mut out),
                egui::Event::Cut => self.clip(Clip::Cut, &mut out),
                // The other computer pastes its own clipboard.
                egui::Event::Paste(_) => self.clip(Clip::Paste, &mut out),
                _ => {}
            }
        }
        self.flush(now, &mut out, false);
        out
    }

    /// The move waiting, if its turn has come (or `now`, with `force`).
    pub fn flush(&mut self, now: Instant, out: &mut Vec<InputPacket>, force: bool) {
        let due = force
            || self
                .moved_at
                .is_none_or(|at| now.saturating_duration_since(at) >= MOVE_EVERY);
        if due && let Some(at) = self.pending {
            self.move_to(at, now, out);
        }
    }

    fn move_to(&mut self, (x, y): (u16, u16), now: Instant, out: &mut Vec<InputPacket>) {
        self.pending = None;
        self.moved_at = Some(now);
        out.push(InputPacket::MouseMoveAbs {
            x_norm: x,
            y_norm: y,
        });
    }

    /// Shift held here goes down there before what needs it.
    fn hold_shift(&mut self, out: &mut Vec<InputPacket>) {
        for shift in [LEFT_SHIFT, RIGHT_SHIFT] {
            if self.down_here.contains(&shift) && self.down_there.insert(shift) {
                out.push(InputPacket::KeyEvent {
                    key: shift,
                    pressed: true,
                });
            }
        }
    }

    fn key(&mut self, key: egui::Key, code: u16, pressed: bool, out: &mut Vec<InputPacket>) {
        if !pressed {
            // A modifier let go first: what it made a copy, cut or paste of
            // goes now, with it still held there.
            if (is_shift(code) || is_chord_modifier(code)) && !self.clips.is_empty() {
                let ctrl = self.ctrl_here();
                for clip in std::mem::take(&mut self.clips) {
                    let made = match (ctrl, clip) {
                        (true, _) => clip.letter(),
                        (false, Clip::Paste) => KEY_INSERT,
                        (false, _) => KEY_DELETE,
                    };
                    self.clipped.insert(made);
                    self.tap(made, out);
                }
            }
            // A press egui kept to itself never came here (BK7 re-review
            // N1: a letter typed before Ctrl was taken for one).
            let pressed_here = self.down_here.remove(&code);
            if self.down_there.remove(&code) {
                out.push(InputPacket::KeyEvent {
                    key: code,
                    pressed: false,
                });
            } else if self.clipped.remove(&code) {
                // Its copy, cut or paste went already.
            } else if !pressed_here && self.swallowed(code) {
                // egui kept its press: the release says which key made the
                // copy, cut or paste, or that one came with nothing to
                // paste here, said by nothing else (BK7 review I2).
                if let Some(at) = self.clips.iter().position(|c| c.keys().contains(&code)) {
                    self.clips.remove(at);
                }
                self.tap(code, out);
            }
            if !self
                .down_here
                .iter()
                .any(|c| is_shift(*c) || is_chord_modifier(*c))
            {
                self.clipped.clear();
            }
            return;
        }
        self.down_here.insert(code);
        if is_shift(code) {
            // Held there once something needs it.
            return;
        }
        let chord = self.down_here.iter().any(|c| is_chord_modifier(*c));
        if types(key) && !chord {
            // It arrives as text.
            return;
        }
        if !is_chord_modifier(code) {
            self.hold_shift(out);
        }
        // A key held repeats: each repeat is pressed again.
        self.down_there.insert(code);
        out.push(InputPacket::KeyEvent {
            key: code,
            pressed: true,
        });
    }

    fn ctrl_here(&self) -> bool {
        self.down_here.contains(&LEFT_CTRL) || self.down_here.contains(&RIGHT_CTRL)
    }

    fn shift_here(&self) -> bool {
        self.down_here.contains(&LEFT_SHIFT) || self.down_here.contains(&RIGHT_SHIFT)
    }

    /// Whether `code`'s release, never pressed there, is one whose press
    /// egui kept to itself for a copy, cut or paste: C, X or V with Ctrl,
    /// Insert or Delete with Ctrl or Shift.
    fn swallowed(&self, code: u16) -> bool {
        match code {
            KEY_C | KEY_X | KEY_V => self.ctrl_here(),
            KEY_INSERT | KEY_DELETE => self.ctrl_here() || self.shift_here(),
            _ => false,
        }
    }

    /// A copy, cut or paste egui made of a key (BK7 review I3). With Ctrl
    /// or Shift held it was Ctrl and a letter, Ctrl+Insert, Shift+Insert or
    /// Shift+Delete: which one, the key's release tells, and that key goes
    /// as itself then. With neither it was the keyboard's own Copy, Cut or
    /// Paste key: Ctrl and the letter go at once.
    fn clip(&mut self, clip: Clip, out: &mut Vec<InputPacket>) {
        let held = self.ctrl_here() || (self.shift_here() && clip != Clip::Copy);
        if held {
            // Held, the key repeats its copy, cut or paste: one a press
            // (BK7 re-review N5), as with nothing to paste here, where egui
            // says none.
            if !self.clips.contains(&clip) {
                self.clips.push(clip);
            }
            return;
        }
        out.push(InputPacket::KeyEvent {
            key: LEFT_CTRL,
            pressed: true,
        });
        for pressed in [true, false] {
            out.push(InputPacket::KeyEvent {
                key: clip.letter(),
                pressed,
            });
        }
        out.push(InputPacket::KeyEvent {
            key: LEFT_CTRL,
            pressed: false,
        });
    }

    /// `code` pressed and let go, with what is held.
    fn tap(&mut self, code: u16, out: &mut Vec<InputPacket>) {
        self.hold_shift(out);
        for pressed in [true, false] {
            out.push(InputPacket::KeyEvent { key: code, pressed });
        }
    }

    /// A key Windows' keyboard grab took (PC4: the Windows key, Alt+Tab's
    /// Tab), as `code` going down or up there; a release of one not held
    /// there is nothing.
    #[cfg(windows)]
    pub fn grabbed(&mut self, code: u16, pressed: bool) -> Vec<InputPacket> {
        let mut out = Vec::new();
        if pressed {
            if !is_shift(code) && !is_chord_modifier(code) {
                self.hold_shift(&mut out);
            }
            self.down_there.insert(code);
            out.push(InputPacket::KeyEvent { key: code, pressed });
        } else if self.down_there.remove(&code) {
            out.push(InputPacket::KeyEvent { key: code, pressed });
        }
        out
    }

    /// `keys` pressed in order and let go in reverse there, as a chord from
    /// the Send keys menu (PC4); a key held there already stays held.
    pub fn chord(&mut self, keys: &[u16]) -> Vec<InputPacket> {
        let pressed: Vec<u16> = keys
            .iter()
            .copied()
            .filter(|code| !self.down_there.contains(code))
            .collect();
        let mut out: Vec<InputPacket> = pressed
            .iter()
            .map(|&key| InputPacket::KeyEvent { key, pressed: true })
            .collect();
        out.extend(pressed.iter().rev().map(|&key| InputPacket::KeyEvent {
            key,
            pressed: false,
        }));
        out
    }

    /// Let go of everything held on the other computer, and forget what is
    /// held here (the window lost focus, closed or stopped controlling).
    pub fn release_all(&mut self) -> Vec<InputPacket> {
        let mut out: Vec<InputPacket> = std::mem::take(&mut self.buttons)
            .into_iter()
            .map(|button| InputPacket::MouseClick {
                button,
                pressed: false,
            })
            .collect();
        // Other keys first, the modifiers last, as fingers would.
        let mut keys: Vec<u16> = std::mem::take(&mut self.down_there).into_iter().collect();
        keys.sort_by_key(|code| is_shift(*code) || is_chord_modifier(*code));
        out.extend(keys.into_iter().map(|key| InputPacket::KeyEvent {
            key,
            pressed: false,
        }));
        self.down_here.clear();
        self.clips.clear();
        self.clipped.clear();
        self.pending = None;
        self.wheel = egui::Vec2::ZERO;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> egui::Rect {
        // A 200x100 picture at (100, 50), letterboxed in a wider window.
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(200.0, 100.0))
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn words(packets: &[InputPacket]) -> Vec<String> {
        packets.iter().map(|p| format!("{p:?}")).collect()
    }

    #[test]
    fn the_keys_that_type_nothing_have_their_linux_codes() {
        for (k, code) in [
            (egui::Key::Enter, 28),
            (egui::Key::Tab, 15),
            (egui::Key::Backspace, 14),
            (egui::Key::Escape, 1),
            (egui::Key::ArrowUp, 103),
            (egui::Key::ArrowDown, 108),
            (egui::Key::ArrowLeft, 105),
            (egui::Key::ArrowRight, 106),
            (egui::Key::Home, 102),
            (egui::Key::End, 107),
            (egui::Key::PageUp, 104),
            (egui::Key::PageDown, 109),
            (egui::Key::Delete, 111),
            (egui::Key::Insert, 110),
            (egui::Key::F1, 59),
            (egui::Key::F10, 68),
            (egui::Key::F11, 87),
            (egui::Key::F12, 88),
            (egui::Key::ControlLeft, 29),
            (egui::Key::ShiftLeft, 42),
            (egui::Key::AltLeft, 56),
            (egui::Key::SuperLeft, 125),
            (egui::Key::A, 30),
            (egui::Key::Z, 44),
            (egui::Key::Num0, 11),
        ] {
            assert_eq!(evdev(k), Some(code), "{k:?}");
        }
        assert_eq!(evdev(egui::Key::F35), None);
        assert!(types(egui::Key::A) && types(egui::Key::Space));
        assert!(!types(egui::Key::Enter) && !types(egui::Key::F5));
    }

    /// The picture is letterboxed: a point is a fraction of the picture,
    /// not the window, and the bars beside it take no click.
    #[test]
    fn a_point_is_a_fraction_of_the_picture_with_its_bars() {
        let mut input = WindowInput::default();
        let now = Instant::now();
        let out = input.take(
            &[
                click(egui::pos2(50.0, 100.0), true),
                click(egui::pos2(200.0, 100.0), true),
                click(egui::pos2(400.0, 100.0), false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "MouseMoveAbs { x_norm: 32768, y_norm: 32768 }",
                "MouseClick { button: 0, pressed: true }",
                "MouseMoveAbs { x_norm: 65535, y_norm: 32768 }",
                "MouseClick { button: 0, pressed: false }",
            ],
            "the press on the bar is nothing; a release past the edge keeps to it"
        );
        assert!(!input.holds());
    }

    /// Moves go at most one each 8 ms, the newest; one held back goes
    /// once its turn comes, or at once before a click.
    #[test]
    fn moves_are_coalesced_to_one_each_turn() {
        let mut input = WindowInput::default();
        let t0 = Instant::now();
        let at = |x| egui::Event::PointerMoved(egui::pos2(x, 100.0));
        let out = input.take(&[at(100.0), at(150.0), at(200.0)], picture(), t0);
        assert_eq!(
            words(&out),
            ["MouseMoveAbs { x_norm: 32768, y_norm: 32768 }"]
        );
        let out = input.take(&[at(300.0)], picture(), t0 + Duration::from_millis(3));
        assert!(out.is_empty(), "too soon");
        assert!(input.waiting());
        let mut later = Vec::new();
        input.flush(t0 + MOVE_EVERY, &mut later, false);
        assert_eq!(
            words(&later),
            ["MouseMoveAbs { x_norm: 65535, y_norm: 32768 }"]
        );
        // Outside the picture, with nothing held, a move is nothing.
        let out = input.take(&[at(20.0)], picture(), t0 + Duration::from_secs(1));
        assert!(out.is_empty());
    }

    #[test]
    fn the_wheel_turns_in_whole_steps_both_ways() {
        let mut input = WindowInput::default();
        let wheel = |unit, x, y| egui::Event::MouseWheel {
            unit,
            delta: egui::vec2(x, y),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let now = Instant::now();
        let out = input.take(
            &[wheel(egui::MouseWheelUnit::Line, 0.0, 1.0)],
            picture(),
            now,
        );
        assert_eq!(words(&out), ["MouseScroll { dx: 0, dy: 1 }"], "up");
        let out = input.take(
            &[wheel(egui::MouseWheelUnit::Line, 2.0, -1.0)],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            ["MouseScroll { dx: -2, dy: -1 }"],
            "left, down"
        );
        // A touchpad's small turns add up to a step.
        let small = wheel(egui::MouseWheelUnit::Point, 0.0, 30.0);
        assert!(
            input
                .take(std::slice::from_ref(&small), picture(), now)
                .is_empty()
        );
        assert_eq!(
            words(&input.take(&[small], picture(), now)),
            ["MouseScroll { dx: 0, dy: 1 }"]
        );
    }

    /// Typing goes as text; Enter and the arrows as keys; Ctrl and a letter
    /// as the keys; Shift held only for what needs it.
    #[test]
    fn typing_goes_as_text_and_shortcuts_as_keys() {
        let mut input = WindowInput::default();
        let now = Instant::now();
        let out = input.take(
            &[
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::A, true),
                egui::Event::Text("A".into()),
                key(egui::Key::A, false),
                key(egui::Key::ArrowLeft, true),
                key(egui::Key::ArrowLeft, false),
                egui::Event::Text("B".into()),
                key(egui::Key::ShiftLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "Text(\"A\")",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 105, pressed: true }",
                "KeyEvent { key: 105, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
                "Text(\"B\")",
            ],
            "Shift+Left selects; the capitals are typed, not shifted twice"
        );
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                key(egui::Key::S, true),
                key(egui::Key::S, false),
                egui::Event::Copy,
                egui::Event::Paste("here".into()),
                key(egui::Key::ControlLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 31, pressed: true }",
                "KeyEvent { key: 31, pressed: false }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 46, pressed: false }",
                "KeyEvent { key: 47, pressed: true }",
                "KeyEvent { key: 47, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ],
            "Ctrl+S, Ctrl+C and Ctrl+V reach the other computer as keys"
        );
        assert!(!input.holds());
    }

    fn run(events: &[egui::Event]) -> Vec<String> {
        words(&WindowInput::default().take(events, picture(), Instant::now()))
    }

    /// BK7 review I2: egui keeps Ctrl+V's press to itself and says Paste
    /// only when this computer's clipboard holds text; with an image or
    /// nothing there, only V's release comes. Ctrl+V still reaches the
    /// other computer, once, either way.
    #[test]
    fn ctrl_v_reaches_the_other_computer_whatever_this_clipboard_holds() {
        assert_eq!(
            run(&[
                key(egui::Key::ControlLeft, true),
                key(egui::Key::V, false),
                key(egui::Key::ControlLeft, false),
            ]),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 47, pressed: true }",
                "KeyEvent { key: 47, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ],
            "nothing to paste here"
        );
        assert_eq!(
            run(&[
                key(egui::Key::ControlLeft, true),
                egui::Event::Paste("text here".into()),
                key(egui::Key::V, false),
                key(egui::Key::ControlLeft, false),
            ]),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 47, pressed: true }",
                "KeyEvent { key: 47, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ],
            "text here: one Ctrl+V, not two"
        );
        // V let go with no Ctrl held is a key typed as text: nothing.
        assert!(run(&[key(egui::Key::V, false)]).is_empty());
    }

    /// BK7 review I3: Shift+Insert, Shift+Delete, Ctrl+Insert and the
    /// keyboard's own Copy, Cut and Paste keys are those keys there, never
    /// a letter typed.
    #[test]
    fn the_other_copy_cut_and_paste_keys_are_themselves_there() {
        assert_eq!(
            run(&[
                key(egui::Key::ShiftLeft, true),
                egui::Event::Paste("text here".into()),
                key(egui::Key::Insert, false),
                key(egui::Key::ShiftLeft, false),
            ]),
            [
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 110, pressed: true }",
                "KeyEvent { key: 110, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
            ],
            "Shift+Insert pastes"
        );
        assert_eq!(
            run(&[
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::Insert, false),
                key(egui::Key::ShiftLeft, false),
            ]),
            [
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 110, pressed: true }",
                "KeyEvent { key: 110, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
            ],
            "Shift+Insert with nothing to paste here"
        );
        assert_eq!(
            run(&[
                key(egui::Key::ShiftLeft, true),
                egui::Event::Cut,
                key(egui::Key::Delete, false),
                key(egui::Key::ShiftLeft, false),
            ]),
            [
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 111, pressed: true }",
                "KeyEvent { key: 111, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
            ],
            "Shift+Delete cuts"
        );
        assert_eq!(
            run(&[
                key(egui::Key::ControlLeft, true),
                egui::Event::Copy,
                key(egui::Key::Insert, false),
                key(egui::Key::ControlLeft, false),
            ]),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 110, pressed: true }",
                "KeyEvent { key: 110, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ],
            "Ctrl+Insert copies"
        );
        for (event, letter) in [
            (egui::Event::Copy, 46),
            (egui::Event::Cut, 45),
            (egui::Event::Paste("x".into()), 47),
        ] {
            assert_eq!(
                run(&[event]),
                [
                    "KeyEvent { key: 29, pressed: true }".to_string(),
                    format!("KeyEvent {{ key: {letter}, pressed: true }}"),
                    format!("KeyEvent {{ key: {letter}, pressed: false }}"),
                    "KeyEvent { key: 29, pressed: false }".to_string(),
                ],
                "the keyboard's own key, with no modifier held"
            );
        }
    }

    /// BK7 re-review N1: a letter typed before Ctrl and let go with it
    /// held was typed, not swallowed by egui: its release sends nothing,
    /// never a stray Ctrl+X that cuts the selection there.
    #[test]
    fn a_letter_typed_before_ctrl_is_not_a_shortcut_at_its_release() {
        for letter in [egui::Key::C, egui::Key::X, egui::Key::V] {
            assert_eq!(
                run(&[
                    key(letter, true),
                    key(egui::Key::ControlLeft, true),
                    key(letter, false),
                ]),
                ["KeyEvent { key: 29, pressed: true }"],
                "{letter:?}"
            );
        }
    }

    /// BK7 re-review N5: Ctrl+V held repeats egui's paste (when this
    /// clipboard holds text) but not its release: one paste a press, as
    /// with nothing to paste here, never a burst at the release.
    #[test]
    fn a_held_paste_is_one_paste() {
        for release_first in [false, true] {
            let mut events = vec![key(egui::Key::ControlLeft, true)];
            events.extend((0..4).map(|_| egui::Event::Paste("text here".into())));
            if release_first {
                events.push(key(egui::Key::ControlLeft, false));
                events.push(key(egui::Key::V, false));
            } else {
                events.push(key(egui::Key::V, false));
                events.push(key(egui::Key::ControlLeft, false));
            }
            assert_eq!(
                run(&events),
                [
                    "KeyEvent { key: 29, pressed: true }",
                    "KeyEvent { key: 47, pressed: true }",
                    "KeyEvent { key: 47, pressed: false }",
                    "KeyEvent { key: 29, pressed: false }",
                ],
                "Ctrl let go first: {release_first}"
            );
        }
    }

    #[test]
    fn text_with_alt_held_is_a_shortcut_not_typing() {
        let mut input = WindowInput::default();
        let out = input.take(
            &[
                key(egui::Key::AltLeft, true),
                key(egui::Key::F, true),
                egui::Event::Text("f".into()),
            ],
            picture(),
            Instant::now(),
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 56, pressed: true }",
                "KeyEvent { key: 33, pressed: true }",
            ]
        );
    }

    /// PC4: every host has Win, Alt+Tab, Alt+F4 and Print Screen; Linux
    /// Ctrl+Alt+Del, Windows Ctrl+Shift+Esc and Lock in its place, and a
    /// computer that has not said its system only the keys every one takes.
    #[test]
    fn the_send_keys_menu_offers_what_the_system_takes() {
        let common = [
            SendKey::Win,
            SendKey::AltTab,
            SendKey::AltF4,
            SendKey::PrintScreen,
        ];
        assert_eq!(send_key_choices(None), common);
        let linux = send_key_choices(Some(false));
        assert_eq!(linux[..4], common);
        assert_eq!(linux[4..], [SendKey::CtrlAltDel]);
        let windows = send_key_choices(Some(true));
        assert_eq!(windows[..4], common);
        assert_eq!(windows[4..], [SendKey::CtrlShiftEsc, SendKey::Lock]);
        assert!(!windows.contains(&SendKey::CtrlAltDel));
        for key in send_key_choices(Some(true))
            .into_iter()
            .chain(send_key_choices(Some(false)))
        {
            assert!(!key.label().is_empty());
            // Every key is one the window already sends, with a Windows
            // stroke on a Windows host and a code on a Linux one.
            for code in key.keys() {
                assert!(pocketdesk_core::local::view::input_allowed(
                    &InputPacket::KeyEvent {
                        key: *code,
                        pressed: true
                    }
                ));
            }
        }
    }

    /// A chord goes down in order and comes up in reverse, as fingers
    /// would; a key held there already is neither pressed nor let go.
    #[test]
    fn a_chord_goes_down_in_order_and_up_in_reverse() {
        let mut input = WindowInput::default();
        assert_eq!(
            words(&input.chord(SendKey::CtrlAltDel.keys())),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 56, pressed: true }",
                "KeyEvent { key: 111, pressed: true }",
                "KeyEvent { key: 111, pressed: false }",
                "KeyEvent { key: 56, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
        assert!(!input.holds(), "nothing is left held");
        assert_eq!(
            words(&input.chord(SendKey::Lock.keys())),
            [
                "KeyEvent { key: 125, pressed: true }",
                "KeyEvent { key: 38, pressed: true }",
                "KeyEvent { key: 38, pressed: false }",
                "KeyEvent { key: 125, pressed: false }",
            ]
        );
        assert_eq!(
            words(&input.chord(SendKey::PrintScreen.keys())),
            [
                "KeyEvent { key: 99, pressed: true }",
                "KeyEvent { key: 99, pressed: false }",
            ]
        );
        // Alt held through the window: Alt+Tab taps Tab alone.
        input.take(&[key(egui::Key::AltLeft, true)], picture(), Instant::now());
        assert_eq!(
            words(&input.chord(SendKey::AltTab.keys())),
            [
                "KeyEvent { key: 15, pressed: true }",
                "KeyEvent { key: 15, pressed: false }",
            ]
        );
        assert!(input.holds(), "Alt is still held there");
    }

    /// PC4: a key the grab took is held there until let go, Shift held
    /// here going with it, and is let go with everything else.
    #[cfg(windows)]
    #[test]
    fn a_grabbed_key_is_held_there_until_let_go() {
        let mut input = WindowInput::default();
        input.take(
            &[key(egui::Key::ShiftLeft, true)],
            picture(),
            Instant::now(),
        );
        assert_eq!(
            words(&input.grabbed(LEFT_META, true)),
            ["KeyEvent { key: 125, pressed: true }"]
        );
        assert_eq!(
            words(&input.grabbed(31, true)),
            [
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 31, pressed: true }",
            ],
            "Shift+Win+S: Shift goes before the S"
        );
        assert_eq!(
            words(&input.grabbed(31, false)),
            ["KeyEvent { key: 31, pressed: false }"]
        );
        assert!(input.grabbed(31, false).is_empty(), "once");
        assert_eq!(
            words(&input.release_all()),
            [
                "KeyEvent { key: 42, pressed: false }",
                "KeyEvent { key: 125, pressed: false }",
            ],
            "the Windows key is let go with the rest"
        );
    }

    /// Focus lost mid-press: the button, the keys and the modifiers are let
    /// go there, the keys before the modifiers.
    #[test]
    fn everything_held_is_let_go() {
        let mut input = WindowInput::default();
        input.take(
            &[
                key(egui::Key::ControlLeft, true),
                key(egui::Key::ArrowDown, true),
                click(egui::pos2(150.0, 60.0), true),
            ],
            picture(),
            Instant::now(),
        );
        assert!(input.holds());
        assert_eq!(
            words(&input.release_all()),
            [
                "MouseClick { button: 0, pressed: false }",
                "KeyEvent { key: 108, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
        assert!(!input.holds());
        assert!(input.release_all().is_empty(), "once");
        // A release of a key let go already goes nowhere.
        let out = input.take(
            &[key(egui::Key::ArrowDown, false)],
            picture(),
            Instant::now(),
        );
        assert!(out.is_empty());
    }
}
