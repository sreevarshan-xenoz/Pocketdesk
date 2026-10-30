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
//!
//! While the window holds the pointer (PC7, [`crate::view_pointer`]) the
//! pointer goes as relative moves instead: the mouse's raw movement, summed
//! over a pass and sent as whole steps, what is left of a step kept for the
//! next; the buttons go without a position.
//!
//! With **Keys by position** (scancode mode, [`WindowInput::set_by_position`])
//! every key, letters and digits among them, goes as the evdev code of where
//! it sits on the keyboard ([`by_position`]), Shift at once like any key, and
//! the text this computer's layout makes of it stays here: the other
//! computer's own layout says what the key types, and a game reads WASD
//! where WASD sit, whatever either layout prints on them. Text that came
//! from no key (a keypad key egui does not place) still goes as text. The
//! third of Ctrl, Alt and Shift waits for a key, a button or the wheel, so
//! this window's own Ctrl+Alt+Shift chords never reach the other computer
//! whole; Ctrl or Shift and C, X, V, Insert or Delete go as keys held, not
//! as egui's copy, cut and paste.

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

/// Keys by position: each key egui places on the keyboard (its
/// `physical_key`, named after what a US keyboard prints there) and the
/// Linux evdev code of that place on a standard 104/105-key keyboard. The
/// other computer presses that code as itself: Windows by its scancode
/// (`KEYEVENTF_SCANCODE`), Linux as the evdev key, so its own layout says
/// what it types.
///
/// What egui cannot tell is not here: it reports the keypad's digits,
/// Enter, minus and slash as their main-block twins (so they go as those),
/// the keypad's `+` only as [`egui::Key::Plus`] (the keypad's, as no main
/// key is Plus by position), and no key at all for Caps Lock, Num Lock,
/// Scroll Lock, Print Screen, Pause, the Menu key or the keypad's `*` and
/// `.` (their text, if any, goes as text). The keys egui names that sit
/// nowhere are in `UNPLACED` (the table's test holds the two together).
pub const BY_POSITION: &[(egui::Key, u16)] = {
    use egui::Key::*;
    &[
        (Escape, 1),
        (Num1, 2),
        (Num2, 3),
        (Num3, 4),
        (Num4, 5),
        (Num5, 6),
        (Num6, 7),
        (Num7, 8),
        (Num8, 9),
        (Num9, 10),
        (Num0, 11),
        (Minus, 12),
        (Equals, 13),
        (Backspace, 14),
        (Tab, 15),
        (Q, 16),
        (W, 17),
        (E, 18),
        (R, 19),
        (T, 20),
        (Y, 21),
        (U, 22),
        (I, 23),
        (O, 24),
        (P, 25),
        (OpenBracket, 26),
        (CloseBracket, 27),
        (Enter, 28),
        (ControlLeft, LEFT_CTRL),
        (A, 30),
        (S, 31),
        (D, 32),
        (F, 33),
        (G, 34),
        (H, 35),
        (J, 36),
        (K, 37),
        (L, 38),
        (Semicolon, 39),
        (Quote, 40),
        (Backtick, 41),
        (ShiftLeft, LEFT_SHIFT),
        (Backslash, 43),
        (Z, 44),
        (X, KEY_X),
        (C, KEY_C),
        (V, KEY_V),
        (B, 48),
        (N, 49),
        (M, 50),
        (Comma, 51),
        (Period, 52),
        (Slash, 53),
        (ShiftRight, RIGHT_SHIFT),
        (AltLeft, LEFT_ALT),
        (Space, 57),
        (F1, 59),
        (F2, 60),
        (F3, 61),
        (F4, 62),
        (F5, 63),
        (F6, 64),
        (F7, 65),
        (F8, 66),
        (F9, 67),
        (F10, 68),
        // KEY_KPPLUS: the only key egui calls Plus by its place.
        (Plus, 78),
        // KEY_102ND, the ISO key beside the left Shift.
        (IntlBackslash, 86),
        (F11, 87),
        (F12, 88),
        (ControlRight, RIGHT_CTRL),
        (AltRight, RIGHT_ALT),
        (Home, 102),
        (ArrowUp, 103),
        (PageUp, 104),
        (ArrowLeft, 105),
        (ArrowRight, 106),
        (End, 107),
        (ArrowDown, 108),
        (PageDown, 109),
        (Insert, KEY_INSERT),
        (Delete, KEY_DELETE),
        (SuperLeft, LEFT_META),
        (SuperRight, RIGHT_META),
        (F13, 183),
        (F14, 184),
        (F15, 185),
        (F16, 186),
        (F17, 187),
        (F18, 188),
        (F19, 189),
        (F20, 190),
        (F21, 191),
        (F22, 192),
        (F23, 193),
        (F24, 194),
    ]
};

/// The keys egui names that are no place on the keyboard, on purpose not
/// in [`BY_POSITION`]: characters it reports only by what a key typed
/// (Colon, Pipe, the curly brackets...), Copy, Cut and Paste (egui makes
/// them its clipboard events, which go as Ctrl+C, X and V), the browser's
/// Back (by meaning only), and F25 to F35, which Linux has no code for.
#[cfg(test)]
pub const UNPLACED: &[egui::Key] = {
    use egui::Key::*;
    &[
        Copy,
        Cut,
        Paste,
        Colon,
        Pipe,
        Questionmark,
        Exclamationmark,
        OpenCurlyBracket,
        CloseCurlyBracket,
        BrowserBack,
        F25,
        F26,
        F27,
        F28,
        F29,
        F30,
        F31,
        F32,
        F33,
        F34,
        F35,
    ]
};

/// The evdev code of the place `key` (egui's `physical_key`) sits on the
/// keyboard, or `None` for one that sits nowhere.
pub fn by_position(key: egui::Key) -> Option<u16> {
    BY_POSITION
        .iter()
        .find(|(k, _)| *k == key)
        .map(|&(_, code)| code)
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
    /// Print Screen, never with Alt held there (Alt+SysRq is Linux's magic
    /// SysRq).
    PrintScreen,
    /// Windows' Task Manager: Windows takes Ctrl+Alt+Del only from a
    /// SYSTEM service. No Ctrl+Alt+Del for Linux either: on a text console
    /// it reboots the computer (PR #559 review I4).
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
            SendKey::CtrlShiftEsc => &[LEFT_CTRL, LEFT_SHIFT, KEY_ESC],
            SendKey::Lock => &[LEFT_META, KEY_L],
        }
    }
}

/// The Send keys menu for a computer that runs Windows (`Some(true)`),
/// Linux (`Some(false)`), or one that has not said (`None`): the keys every
/// system takes, and Windows' Task Manager and Lock.
pub fn send_key_choices(windows: Option<bool>) -> Vec<SendKey> {
    let mut keys = vec![
        SendKey::Win,
        SendKey::AltTab,
        SendKey::AltF4,
        SendKey::PrintScreen,
    ];
    if windows == Some(true) {
        keys.extend([SendKey::CtrlShiftEsc, SendKey::Lock]);
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

/// Ctrl, Shift or Alt, either side, as one kind each.
fn modifier_kind(code: u16) -> Option<u8> {
    match code {
        LEFT_CTRL | RIGHT_CTRL => Some(0),
        LEFT_SHIFT | RIGHT_SHIFT => Some(1),
        LEFT_ALT | RIGHT_ALT => Some(2),
        _ => None,
    }
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
    /// While the pointer is locked (PC7): the raw movement not yet sent as
    /// a whole step, each way. `None` while it is not.
    relative: Option<egui::Vec2>,
    /// Keys by position (scancode mode): every key goes as where it sits.
    by_position: bool,
    /// A key that types just went by its position: the text egui made of
    /// it here, which follows it in the same pass, is not sent as well.
    keyed: bool,
    /// By position, the key pressed there for a copy, cut or paste egui
    /// made of Ctrl or Shift and C, X, V, Insert or Delete (review I4),
    /// held until that key is let go.
    clip_held: Option<u16>,
}

impl WindowInput {
    /// Whether anything is held on the other computer.
    pub fn holds(&self) -> bool {
        !self.down_there.is_empty() || !self.buttons.is_empty()
    }

    /// The pointer locked (PC7): its raw movement goes as relative moves,
    /// and where it is no longer counts; freed, it goes as absolute moves
    /// again. What was left of a step is forgotten either way.
    pub fn set_relative(&mut self, on: bool) {
        if on != self.relative.is_some() {
            self.relative = on.then_some(egui::Vec2::ZERO);
            self.pending = None;
        }
    }

    /// Keys by position on or off: what is held there is let go first, so
    /// nothing pressed one way is left down the other way.
    pub fn set_by_position(&mut self, on: bool) -> Vec<InputPacket> {
        if on == self.by_position {
            return Vec::new();
        }
        let out = self.release_all();
        self.by_position = on;
        out
    }

    /// Whether keys go by their position.
    #[cfg(test)]
    pub fn by_position(&self) -> bool {
        self.by_position
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
            // The text of a key that went by its position comes right after
            // it, in the same pass.
            let keyed = std::mem::take(&mut self.keyed);
            match event {
                egui::Event::MouseMoved(delta) => {
                    if let Some(sum) = self.relative.as_mut() {
                        *sum += *delta;
                    }
                }
                // Locked, where the hidden pointer is says nothing.
                egui::Event::PointerMoved(_) if self.relative.is_some() => {}
                egui::Event::PointerButton {
                    button: which,
                    pressed,
                    ..
                } if self.relative.is_some() => {
                    // Its movement so far goes first, then the button, where
                    // the other computer's pointer is.
                    self.flush_relative(&mut out);
                    let code = button(*which);
                    if *pressed {
                        self.hold_modifiers(&mut out);
                        self.buttons.insert(code);
                    } else if !self.buttons.remove(&code) {
                        continue;
                    }
                    out.push(InputPacket::MouseClick {
                        button: code,
                        pressed: *pressed,
                    });
                }
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
                        self.hold_modifiers(&mut out);
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
                    self.hold_modifiers(&mut out);
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
                    // By position, the key where it sits; one egui places
                    // nowhere goes as before, its text as text.
                    let (key, code, placed) = match physical_key {
                        Some(at) if self.by_position => (*at, by_position(*at), true),
                        _ => {
                            let key = physical_key.unwrap_or(*key);
                            (key, evdev(key), false)
                        }
                    };
                    let Some(code) = code else { continue };
                    self.key(key, code, *pressed, placed, &mut out);
                    self.keyed = placed && *pressed && types(key);
                }
                // That computer's layout makes this key's text there.
                egui::Event::Text(_) if keyed => {}
                egui::Event::Text(text) if !text.is_empty() => {
                    // With Ctrl or Alt held it is a shortcut, sent as keys.
                    if self.down_here.iter().any(|c| is_chord_modifier(*c)) {
                        continue;
                    }
                    // As text, Shift is let go there first, so the capitals
                    // are not shifted twice; by position it is a key held
                    // (review N3), and stays.
                    if !self.by_position {
                        for shift in [LEFT_SHIFT, RIGHT_SHIFT] {
                            if self.down_there.remove(&shift) {
                                out.push(InputPacket::KeyEvent {
                                    key: shift,
                                    pressed: false,
                                });
                            }
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
        // A key's text comes in its own pass: none is waited for after it
        // (review I2).
        self.keyed = false;
        self.flush(now, &mut out, false);
        self.flush_relative(&mut out);
        out
    }

    /// The whole steps of the raw movement summed so far, as relative
    /// moves; what is left of a step is kept.
    fn flush_relative(&mut self, out: &mut Vec<InputPacket>) {
        if let Some(sum) = self.relative.as_mut() {
            out.extend(whole_moves(sum));
        }
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

    /// The modifiers held here and not yet there go down there before what
    /// needs them: Shift as text left it, and by position the third of
    /// Ctrl, Alt and Shift held back ([`Self::key`]).
    fn hold_modifiers(&mut self, out: &mut Vec<InputPacket>) {
        for code in [
            LEFT_CTRL,
            RIGHT_CTRL,
            LEFT_SHIFT,
            RIGHT_SHIFT,
            LEFT_ALT,
            RIGHT_ALT,
            LEFT_META,
            RIGHT_META,
        ] {
            if self.down_here.contains(&code) && self.down_there.insert(code) {
                out.push(InputPacket::KeyEvent {
                    key: code,
                    pressed: true,
                });
            }
        }
    }

    /// Whether, by position, the modifier `code` waits for a key: it would
    /// make all three of Ctrl, Alt and Shift, the two other kinds held here
    /// already (PR #573 review I1, re-review I-A). Ctrl+Alt+Shift is this
    /// window's own chord (with M, S or a number, or alone): its third
    /// modifier never reaches the other computer by itself. Two kinds go at
    /// once, as a game's crouch and run.
    fn waits(&self, code: u16) -> bool {
        let Some(kind) = modifier_kind(code) else {
            return false;
        };
        let others: BTreeSet<u8> = self
            .down_here
            .iter()
            .filter_map(|c| modifier_kind(*c))
            .filter(|k| *k != kind)
            .collect();
        others.len() == 2
    }

    /// `key` (`code` there) going down or up; `placed`, by its position:
    /// it goes as a key whatever it types, Shift at once.
    fn key(
        &mut self,
        key: egui::Key,
        code: u16,
        pressed: bool,
        placed: bool,
        out: &mut Vec<InputPacket>,
    ) {
        if !pressed {
            // By position, the key a copy, cut or paste pressed there goes
            // up with the key egui kept: whichever of C, X, V, Insert or
            // Delete (Ctrl+Insert copies as Ctrl+C does), or any key not
            // held there, as the key in C's place on a Dvorak keyboard is
            // (re-review C-A: its press never came, its code is its own).
            if let Some(held) = self.clip_held
                && !is_shift(code)
                && !is_chord_modifier(code)
                && (matches!(code, KEY_C | KEY_X | KEY_V | KEY_INSERT | KEY_DELETE)
                    || !self.down_there.contains(&code))
            {
                self.clip_held = None;
                self.down_here.remove(&code);
                if self.down_there.remove(&held) {
                    out.push(InputPacket::KeyEvent {
                        key: held,
                        pressed: false,
                    });
                }
                return;
            }
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
        if is_shift(code) && !placed {
            // Held there once something needs it.
            return;
        }
        if placed && self.waits(code) {
            // Held there once a key, a button or the wheel needs it.
            return;
        }
        let chord = self.down_here.iter().any(|c| is_chord_modifier(*c));
        if types(key) && !chord && !placed {
            // It arrives as text.
            return;
        }
        if !is_chord_modifier(code) && !is_shift(code) {
            self.hold_modifiers(out);
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
    ///
    /// By position (review I4) the press goes at once and is held, as the
    /// key egui kept: C, X or V with Ctrl, else Delete or Insert with Shift,
    /// at the place a QWERTY keyboard has them (egui says no more; Ctrl and
    /// Insert copies as Ctrl+C), let go with the key.
    fn clip(&mut self, clip: Clip, out: &mut Vec<InputPacket>) {
        let held = self.ctrl_here() || (self.shift_here() && clip != Clip::Copy);
        if held && self.by_position {
            let code = match (clip, self.ctrl_here()) {
                (_, true) => clip.letter(),
                (Clip::Paste, false) => KEY_INSERT,
                (_, false) => KEY_DELETE,
            };
            self.hold_modifiers(out);
            // A press repeated is pressed again, as a key held is.
            self.down_there.insert(code);
            self.clip_held = Some(code);
            out.push(InputPacket::KeyEvent {
                key: code,
                pressed: true,
            });
            return;
        }
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
        self.hold_modifiers(out);
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
                self.hold_modifiers(&mut out);
            }
            self.down_there.insert(code);
            out.push(InputPacket::KeyEvent { key: code, pressed });
        } else if self.down_there.remove(&code) {
            out.push(InputPacket::KeyEvent { key: code, pressed });
        }
        out
    }

    /// `keys` pressed in order and let go in reverse there, as a chord from
    /// the Send keys menu (PC4); a key held there already stays held. To a
    /// computer that is not Windows (`windows` not `Some(true)`), Print
    /// Screen goes with no Alt held there: Alt held is let go first and
    /// pressed again after, so it is never Alt+SysRq, Linux's magic SysRq
    /// (PR #559 review I4). Windows keeps Alt+Print Screen, its picture of
    /// the window in front (re-review N4).
    pub fn chord(&mut self, keys: &[u16], windows: Option<bool>) -> Vec<InputPacket> {
        let lifted: Vec<u16> = if keys.contains(&KEY_SYSRQ) && windows != Some(true) {
            [LEFT_ALT, RIGHT_ALT]
                .into_iter()
                .filter(|alt| self.down_there.contains(alt))
                .collect()
        } else {
            Vec::new()
        };
        let pressed: Vec<u16> = keys
            .iter()
            .copied()
            .filter(|code| !self.down_there.contains(code))
            .collect();
        let mut out: Vec<InputPacket> = lifted
            .iter()
            .map(|&key| InputPacket::KeyEvent {
                key,
                pressed: false,
            })
            .collect();
        out.extend(
            pressed
                .iter()
                .map(|&key| InputPacket::KeyEvent { key, pressed: true }),
        );
        out.extend(pressed.iter().rev().map(|&key| InputPacket::KeyEvent {
            key,
            pressed: false,
        }));
        out.extend(
            lifted
                .iter()
                .map(|&key| InputPacket::KeyEvent { key, pressed: true }),
        );
        out
    }

    /// Let go of everything held on the other computer, and forget what is
    /// held here (the window lost focus, closed or stopped controlling).
    /// A lock's movement not yet sent is forgotten; the lock is the
    /// window's to free ([`Self::set_relative`]).
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
        self.keyed = false;
        self.clip_held = None;
        self.pending = None;
        self.wheel = egui::Vec2::ZERO;
        if let Some(sum) = self.relative.as_mut() {
            *sum = egui::Vec2::ZERO;
        }
        out
    }
}

/// The whole steps of `sum` (raw movement, each way) as relative moves, as
/// many as the wire's `i16` needs; the part of a step left stays in `sum`,
/// so slow movement is not lost to rounding.
pub fn whole_moves(sum: &mut egui::Vec2) -> Vec<InputPacket> {
    let mut out = Vec::new();
    if !sum.x.is_finite() || !sum.y.is_finite() {
        *sum = egui::Vec2::ZERO;
        return out;
    }
    let (dx, dy) = (sum.x.trunc(), sum.y.trunc());
    *sum -= egui::vec2(dx, dy);
    let most = f32::from(i16::MAX);
    // A pass never moves this far; a wild one is cut to a few packets.
    let cap = most * 4.0;
    let (mut dx, mut dy) = (dx.clamp(-cap, cap), dy.clamp(-cap, cap));
    while dx != 0.0 || dy != 0.0 {
        let (x, y) = (dx.clamp(-most, most), dy.clamp(-most, most));
        out.push(InputPacket::MouseMove {
            dx: x as i16,
            dy: y as i16,
        });
        dx -= x;
        dy -= y;
    }
    out
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

    /// PC4: every host has Win, Alt+Tab, Alt+F4 and Print Screen, and a
    /// Windows one Ctrl+Shift+Esc and Lock too; no host has Ctrl+Alt+Del
    /// (Windows' takes it only from a system service, and on a Linux text
    /// console it reboots, PR #559 review I4).
    #[test]
    fn the_send_keys_menu_offers_what_the_system_takes() {
        let common = [
            SendKey::Win,
            SendKey::AltTab,
            SendKey::AltF4,
            SendKey::PrintScreen,
        ];
        assert_eq!(send_key_choices(None), common);
        assert_eq!(send_key_choices(Some(false)), common, "Linux");
        let windows = send_key_choices(Some(true));
        assert_eq!(windows[..4], common);
        assert_eq!(windows[4..], [SendKey::CtrlShiftEsc, SendKey::Lock]);
        for key in send_key_choices(Some(true)) {
            let keys = key.keys();
            assert!(
                !(keys.contains(&LEFT_CTRL)
                    && keys.contains(&LEFT_ALT)
                    && keys.contains(&KEY_DELETE)),
                "no Ctrl+Alt+Del"
            );
        }
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
                    },
                    false,
                    false
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
            words(&input.chord(SendKey::CtrlShiftEsc.keys(), None)),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 1, pressed: true }",
                "KeyEvent { key: 1, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
        assert!(!input.holds(), "nothing is left held");
        assert_eq!(
            words(&input.chord(SendKey::Lock.keys(), None)),
            [
                "KeyEvent { key: 125, pressed: true }",
                "KeyEvent { key: 38, pressed: true }",
                "KeyEvent { key: 38, pressed: false }",
                "KeyEvent { key: 125, pressed: false }",
            ]
        );
        assert_eq!(
            words(&input.chord(SendKey::PrintScreen.keys(), None)),
            [
                "KeyEvent { key: 99, pressed: true }",
                "KeyEvent { key: 99, pressed: false }",
            ]
        );
        // Alt held through the window: Alt+Tab taps Tab alone.
        input.take(&[key(egui::Key::AltLeft, true)], picture(), Instant::now());
        assert_eq!(
            words(&input.chord(SendKey::AltTab.keys(), None)),
            [
                "KeyEvent { key: 15, pressed: true }",
                "KeyEvent { key: 15, pressed: false }",
            ]
        );
        assert!(input.holds(), "Alt is still held there");
        // Review I4: Print Screen with Alt held there lets Alt go first and
        // presses it again after, never Alt+SysRq.
        assert_eq!(
            words(&input.chord(SendKey::PrintScreen.keys(), None)),
            [
                "KeyEvent { key: 56, pressed: false }",
                "KeyEvent { key: 99, pressed: true }",
                "KeyEvent { key: 99, pressed: false }",
                "KeyEvent { key: 56, pressed: true }",
            ]
        );
        assert!(input.holds(), "Alt is held there again, as it is here");
        // Re-review N4: to Windows, Alt+Print Screen stays itself.
        assert_eq!(
            words(&input.chord(SendKey::PrintScreen.keys(), Some(true))),
            [
                "KeyEvent { key: 99, pressed: true }",
                "KeyEvent { key: 99, pressed: false }",
            ]
        );
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

    fn moved(dx: f32, dy: f32) -> egui::Event {
        egui::Event::MouseMoved(egui::vec2(dx, dy))
    }

    /// PC7: locked, the raw movement of a pass goes as one relative move
    /// in whole steps, the part of a step kept for the next pass; where the
    /// hidden pointer is says nothing.
    #[test]
    fn locked_the_movement_goes_as_whole_relative_steps() {
        let now = Instant::now();
        let mut input = WindowInput::default();
        // Not locked: the raw movement is not sent.
        assert!(input.take(&[moved(5.0, 5.0)], picture(), now).is_empty());
        input.set_relative(true);
        assert!(input.relative.is_some());
        let packets = input.take(
            &[
                moved(1.5, -0.25),
                egui::Event::PointerMoved(egui::pos2(150.0, 70.0)),
                moved(2.25, -0.5),
            ],
            picture(),
            now,
        );
        assert_eq!(words(&packets), ["MouseMove { dx: 3, dy: 0 }"]);
        // 0.75 and -0.75 were kept: a quarter more each way is a step.
        let packets = input.take(&[moved(0.25, -0.25)], picture(), now);
        assert_eq!(words(&packets), ["MouseMove { dx: 1, dy: -1 }"]);
        assert!(input.take(&[moved(0.5, 0.0)], picture(), now).is_empty());
        // Freed: the half step is forgotten and moves are absolute again.
        input.set_relative(false);
        input.set_relative(true);
        assert!(input.take(&[moved(0.5, 0.0)], picture(), now).is_empty());
        input.set_relative(false);
        let packets = input.take(
            &[
                moved(9.0, 9.0),
                egui::Event::PointerMoved(egui::pos2(200.0, 100.0)),
            ],
            picture(),
            now,
        );
        assert!(
            matches!(packets.as_slice(), [InputPacket::MouseMoveAbs { .. }]),
            "{packets:?}"
        );
    }

    /// PC7: locked, a button goes without a position, after the movement
    /// before it, wherever the hidden pointer is (even off the picture).
    #[test]
    fn locked_a_button_goes_after_its_movement_without_a_position() {
        let now = Instant::now();
        let mut input = WindowInput::default();
        input.set_relative(true);
        let off_picture = egui::pos2(10.0, 10.0);
        let packets = input.take(
            &[moved(4.0, 0.0), click(off_picture, true), moved(0.0, 2.0)],
            picture(),
            now,
        );
        assert_eq!(
            words(&packets),
            [
                "MouseMove { dx: 4, dy: 0 }",
                "MouseClick { button: 0, pressed: true }",
                "MouseMove { dx: 0, dy: 2 }",
            ]
        );
        assert!(input.holds());
        let packets = input.take(&[click(off_picture, false)], picture(), now);
        assert_eq!(
            words(&packets),
            ["MouseClick { button: 0, pressed: false }"]
        );
        // A release never pressed there is nothing.
        assert!(
            input
                .take(&[click(off_picture, false)], picture(), now)
                .is_empty()
        );
    }

    /// PC7: a pass's movement past the wire's `i16` goes as several moves;
    /// a wild one is cut short, and one that is not a number is dropped.
    #[test]
    fn a_long_movement_is_split_and_a_wild_one_cut() {
        let mut sum = egui::vec2(40_000.0, -10.0);
        assert_eq!(
            words(&whole_moves(&mut sum)),
            [
                "MouseMove { dx: 32767, dy: -10 }",
                "MouseMove { dx: 7233, dy: 0 }",
            ]
        );
        assert_eq!(sum, egui::Vec2::ZERO);
        let mut wild = egui::vec2(1e30, 0.0);
        assert_eq!(whole_moves(&mut wild).len(), 4);
        let mut nan = egui::vec2(f32::NAN, 1.0);
        assert!(whole_moves(&mut nan).is_empty());
        assert_eq!(nan, egui::Vec2::ZERO);
    }

    /// Keys by position: every key egui names is placed once or is on the
    /// list of keys that sit nowhere; no two keys share a place; and every
    /// place is one a Windows keyboard names by its scancode, which the
    /// Windows host presses as that scancode again (the server's
    /// `every_scancode_read_back_is_pressed_as_itself`).
    #[test]
    fn every_key_is_placed_once_or_unplaced_on_purpose() {
        use pocketdesk_core::input::windows_scan::evdev_from_scan;
        for key in egui::Key::ALL {
            let placed = BY_POSITION.iter().filter(|(k, _)| k == key).count();
            let unplaced = UNPLACED.iter().filter(|k| *k == key).count();
            assert_eq!(placed + unplaced, 1, "{key:?}");
        }
        assert_eq!(BY_POSITION.len() + UNPLACED.len(), egui::Key::ALL.len());
        let codes: BTreeSet<u16> = BY_POSITION.iter().map(|&(_, c)| c).collect();
        assert_eq!(codes.len(), BY_POSITION.len(), "a place used twice");
        let scanned: BTreeSet<u16> = [false, true]
            .into_iter()
            .flat_map(|ext| (0..=0x7Fu16).filter_map(move |s| evdev_from_scan(s, ext)))
            .collect();
        for (key, code) in BY_POSITION {
            assert!(scanned.contains(code), "{key:?} ({code}) has no scancode");
        }
        for (key, code) in [
            (egui::Key::W, 17),
            (egui::Key::A, 30),
            (egui::Key::Z, 44),
            (egui::Key::Num1, 2),
            (egui::Key::Semicolon, 39),
            (egui::Key::IntlBackslash, 86),
            (egui::Key::Plus, 78),
            (egui::Key::Equals, 13),
            (egui::Key::F24, 194),
            (egui::Key::ShiftRight, 54),
        ] {
            assert_eq!(by_position(key), Some(code), "{key:?}");
        }
        assert_eq!(by_position(egui::Key::Colon), None);
        assert_eq!(by_position(egui::Key::F25), None);
    }

    /// Keys by position: a letter goes as its key and the text this layout
    /// made of it stays here (an AZERTY keyboard's A key types "q" here,
    /// and goes as A's place); Shift goes at once, as a game's run key.
    #[test]
    fn by_position_every_key_goes_as_itself_and_its_text_stays_here() {
        let mut input = WindowInput::default();
        assert!(input.set_by_position(true).is_empty());
        assert!(input.by_position());
        let now = Instant::now();
        let out = input.take(
            &[
                key(egui::Key::A, true),
                egui::Event::Text("q".into()),
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::W, true),
                egui::Event::Text("Z".into()),
                key(egui::Key::W, true),
                egui::Event::Text("Z".into()),
                key(egui::Key::W, false),
                key(egui::Key::A, false),
                key(egui::Key::Space, true),
                egui::Event::Text(" ".into()),
                key(egui::Key::Space, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 30, pressed: true }",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 17, pressed: true }",
                "KeyEvent { key: 17, pressed: true }",
                "KeyEvent { key: 17, pressed: false }",
                "KeyEvent { key: 30, pressed: false }",
                "KeyEvent { key: 57, pressed: true }",
                "KeyEvent { key: 57, pressed: false }",
            ],
            "no text: that computer's layout types it"
        );
        assert!(input.holds(), "Shift is held there");
        let out = input.take(&[key(egui::Key::ShiftLeft, false)], picture(), now);
        assert_eq!(words(&out), ["KeyEvent { key: 42, pressed: false }"]);
        assert!(!input.holds());
    }

    /// Keys by position: text that came from no key, an input method's
    /// or a keypad key egui does not place, still goes as text; a key egui
    /// knows only by what it typed goes as before.
    #[test]
    fn by_position_text_from_no_key_still_goes() {
        let mut input = WindowInput::default();
        let _ = input.set_by_position(true);
        let unplaced = egui::Event::Key {
            key: egui::Key::Period,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let out = input.take(
            &[
                egui::Event::Text("*".into()),
                egui::Event::Ime(egui::ImeEvent::Commit("\u{65e5}".into())),
                egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "x".into(),
                    active_range_chars: None,
                }),
                unplaced,
                egui::Event::Text(".".into()),
            ],
            picture(),
            Instant::now(),
        );
        assert_eq!(
            words(&out),
            ["Text(\"*\")", "Text(\".\")"],
            "the picture asks for no input method: none commits here (review N1)"
        );
    }

    /// Keys by position off: typing is as before, and an input method's
    /// text is nothing new.
    #[test]
    fn off_typing_goes_as_text_as_before() {
        let out = run(&[
            key(egui::Key::A, true),
            egui::Event::Text("q".into()),
            key(egui::Key::A, false),
            egui::Event::Ime(egui::ImeEvent::Commit("x".into())),
        ]);
        assert_eq!(out, ["Text(\"q\")"]);
    }

    /// Switching Keys by position lets go of everything held there first,
    /// either way, so a key pressed one way is never left down the other;
    /// switching to what it is already sends nothing.
    #[test]
    fn switching_by_position_lets_go_of_what_is_held() {
        let mut input = WindowInput::default();
        let _ = input.set_by_position(true);
        let now = Instant::now();
        let held = input.take(
            &[key(egui::Key::ShiftLeft, true), key(egui::Key::W, true)],
            picture(),
            now,
        );
        assert_eq!(held.len(), 2);
        assert!(input.set_by_position(true).is_empty());
        assert_eq!(
            words(&input.set_by_position(false)),
            [
                "KeyEvent { key: 17, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
            ]
        );
        assert!(!input.holds());
        // The fingers let go later: nothing, as nothing is held there.
        let out = input.take(
            &[key(egui::Key::W, false), key(egui::Key::ShiftLeft, false)],
            picture(),
            now,
        );
        assert!(out.is_empty(), "{out:?}");
        // And back on, a key held as text there (Ctrl) is let go too.
        let _ = input.take(&[key(egui::Key::ControlLeft, true)], picture(), now);
        assert_eq!(
            words(&input.set_by_position(true)),
            ["KeyEvent { key: 29, pressed: false }"]
        );
        // Everything let go at once (focus lost, closed, view only).
        let _ = input.take(&[key(egui::Key::D, true)], picture(), now);
        assert_eq!(
            words(&input.release_all()),
            ["KeyEvent { key: 32, pressed: false }"]
        );
        assert!(input.by_position(), "the mode outlives a release");
    }

    fn by_position_input() -> WindowInput {
        let mut input = WindowInput::default();
        let _ = input.set_by_position(true);
        input
    }

    /// PR #573 review I1, re-review I-A: by position, the third of Ctrl,
    /// Alt and Shift waits for a key, so this window's own chord
    /// (Ctrl+Alt+Shift and M, the M kept here) never reaches the other
    /// computer whole, in any order; two kinds go at once.
    #[test]
    fn by_position_the_windows_chord_never_goes_whole() {
        let now = Instant::now();
        for order in [
            [
                egui::Key::ControlLeft,
                egui::Key::AltLeft,
                egui::Key::ShiftLeft,
            ],
            [
                egui::Key::ShiftLeft,
                egui::Key::ControlLeft,
                egui::Key::AltLeft,
            ],
            [
                egui::Key::AltLeft,
                egui::Key::ShiftLeft,
                egui::Key::ControlLeft,
            ],
        ] {
            let mut input = by_position_input();
            let mut events: Vec<egui::Event> = order.iter().map(|k| key(*k, true)).collect();
            // The window kept M (keep_chord_here): no event for it.
            events.extend(order.iter().rev().map(|k| key(*k, false)));
            let out = input.take(&events, picture(), now);
            let first_two: Vec<u16> = order[..2].iter().filter_map(|k| by_position(*k)).collect();
            let third = by_position(order[2]).unwrap();
            let sent: Vec<u16> = out
                .iter()
                .filter_map(|p| match p {
                    InputPacket::KeyEvent { key, pressed: true } => Some(*key),
                    _ => None,
                })
                .collect();
            assert_eq!(sent, first_two, "{order:?}: the third never goes");
            assert!(!sent.contains(&third));
            assert!(!input.holds(), "{order:?}");
        }
        // With a key, the chord goes whole, its modifiers first.
        let mut input = by_position_input();
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                key(egui::Key::AltLeft, true),
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::T, true),
                key(egui::Key::T, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 56, pressed: true }",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 20, pressed: true }",
                "KeyEvent { key: 20, pressed: false }",
            ]
        );
    }

    /// Re-review I-A: a game's W held, then crouch (Ctrl) and run (Shift):
    /// all three reach the other computer as pressed, with nothing else
    /// needed; Ctrl+Shift and Alt+Shift alone go too.
    #[test]
    fn by_position_crouch_then_run_reach_the_game() {
        let mut input = by_position_input();
        let now = Instant::now();
        let out = input.take(
            &[
                key(egui::Key::W, true),
                key(egui::Key::ControlLeft, true),
                key(egui::Key::ShiftLeft, true),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 17, pressed: true }",
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 42, pressed: true }",
            ]
        );
        let _ = input.release_all();
        let out = input.take(
            &[
                key(egui::Key::AltLeft, true),
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::ShiftLeft, false),
                key(egui::Key::AltLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 56, pressed: true }",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 42, pressed: false }",
                "KeyEvent { key: 56, pressed: false }",
            ],
            "two kinds go at once"
        );
    }

    /// Re-review C-A: on a Dvorak keyboard Ctrl and the key printed C is
    /// QWERTY's J: egui says copy, the C place goes down there, and the J
    /// let go lets it go; nothing stays held.
    #[test]
    fn by_position_a_copy_from_another_place_is_let_go_with_its_key() {
        let mut input = by_position_input();
        let now = Instant::now();
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                egui::Event::Copy,
                key(egui::Key::J, false),
                key(egui::Key::ControlLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 46, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
        assert!(!input.holds());
        // A key held there beside it keeps its own release.
        let out = input.take(
            &[
                key(egui::Key::W, true),
                key(egui::Key::ControlLeft, true),
                egui::Event::Copy,
                key(egui::Key::W, false),
                key(egui::Key::J, false),
                key(egui::Key::ControlLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 17, pressed: true }",
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 17, pressed: false }",
                "KeyEvent { key: 46, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
        assert!(!input.holds());
    }

    /// PR #573 review I2: the text of a key that types nothing is never
    /// waited for: a keypad `*` (text only) after F5 or an arrow, in the
    /// same pass or the next, arrives.
    #[test]
    fn by_position_text_after_a_key_that_types_nothing_goes() {
        let mut input = by_position_input();
        let now = Instant::now();
        let _ = input.take(&[key(egui::Key::F5, true)], picture(), now);
        let out = input.take(&[egui::Event::Text("*".into())], picture(), now);
        assert_eq!(words(&out), ["Text(\"*\")"]);
        let out = input.take(
            &[
                key(egui::Key::ArrowLeft, true),
                egui::Event::Text("*".into()),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            ["KeyEvent { key: 105, pressed: true }", "Text(\"*\")"]
        );
        // A letter's text is its own, in its pass; the next pass's is not.
        let _ = input.take(&[key(egui::Key::A, true)], picture(), now);
        let out = input.take(&[egui::Event::Text("*".into())], picture(), now);
        assert_eq!(words(&out), ["Text(\"*\")"]);
    }

    /// PR #573 review I4: by position, Ctrl or Shift and C, X, V, Insert or
    /// Delete, which egui turns into copy, cut and paste, go as the key
    /// held: pressed as egui says it, again as it repeats, let go with the
    /// key (Ctrl+Insert as Ctrl+C), the modifier free to go first.
    #[test]
    fn by_position_copy_cut_and_paste_keys_are_held() {
        let mut input = by_position_input();
        let now = Instant::now();
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                egui::Event::Copy,
                egui::Event::Copy,
                key(egui::Key::ControlLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 29, pressed: false }",
            ],
            "C held there past Ctrl, as a crouching game's key"
        );
        assert!(input.holds());
        let out = input.take(&[key(egui::Key::C, false)], picture(), now);
        assert_eq!(words(&out), ["KeyEvent { key: 46, pressed: false }"]);
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                egui::Event::Copy,
                key(egui::Key::Insert, false),
                key(egui::Key::ShiftLeft, true),
                key(egui::Key::ControlLeft, false),
                egui::Event::Cut,
                key(egui::Key::Delete, false),
                key(egui::Key::ShiftLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 46, pressed: true }",
                "KeyEvent { key: 46, pressed: false }",
                "KeyEvent { key: 42, pressed: true }",
                "KeyEvent { key: 29, pressed: false }",
                "KeyEvent { key: 111, pressed: true }",
                "KeyEvent { key: 111, pressed: false }",
                "KeyEvent { key: 42, pressed: false }",
            ]
        );
        assert!(!input.holds());
        // A paste with nothing to paste here says nothing: its release
        // still goes, as before.
        let out = input.take(
            &[
                key(egui::Key::ControlLeft, true),
                key(egui::Key::V, false),
                key(egui::Key::ControlLeft, false),
            ],
            picture(),
            now,
        );
        assert_eq!(
            words(&out),
            [
                "KeyEvent { key: 29, pressed: true }",
                "KeyEvent { key: 47, pressed: true }",
                "KeyEvent { key: 47, pressed: false }",
                "KeyEvent { key: 29, pressed: false }",
            ]
        );
    }

    /// PR #573 review N3: by position, text from no key leaves Shift held
    /// there (a run key stays down).
    #[test]
    fn by_position_text_leaves_shift_held() {
        let mut input = by_position_input();
        let out = input.take(
            &[
                key(egui::Key::ShiftLeft, true),
                egui::Event::Text("*".into()),
            ],
            picture(),
            Instant::now(),
        );
        assert_eq!(
            words(&out),
            ["KeyEvent { key: 42, pressed: true }", "Text(\"*\")"]
        );
        assert!(input.holds());
    }

    /// PR #573 review N7: a rejoin takes a fresh input after letting go of
    /// what was held; the fresh one sends nothing as it takes the mode, and
    /// holds nothing.
    #[test]
    fn a_rejoin_lets_go_and_starts_fresh_by_position() {
        let mut input = by_position_input();
        let now = Instant::now();
        let _ = input.take(&[key(egui::Key::W, true)], picture(), now);
        assert_eq!(
            words(&input.release_all()),
            ["KeyEvent { key: 17, pressed: false }"]
        );
        let mut fresh = WindowInput::default();
        assert!(fresh.set_by_position(true).is_empty());
        assert!(!fresh.holds() && fresh.by_position());
    }
}
