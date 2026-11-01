//! Windows input injection through `SendInput`.
//!
//! The phone sends evdev key codes, which name *physical key positions*. They
//! are injected as PC set-1 scancodes (`KEYEVENTF_SCANCODE`), which also name
//! positions, so the active keyboard layout decides the character exactly as
//! it would for a real keyboard — the right behaviour on any layout. A few keys
//! are safer as virtual keys (NumLock and Pause share scancode 0x45; media,
//! volume and F13-F24 have no universally mapped scancode), and text arrives
//! as UTF-16 through `KEYEVENTF_UNICODE`, so pasted capitals and non-ASCII
//! characters land as typed.
//!
//! Absolute pointer moves arrive normalized to 0..=65535 over the whole desktop
//! layout (the phone pre-maps into desktop space), which is exactly the
//! coordinate space of `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
//!
//! `SendInput` refuses input aimed at a more privileged window (UIPI) or the
//! secure desktop; that is reported once per foreground window rather than
//! per event, and the session carries on.

use anyhow::Result;
use pocketdesk_core::input::evdev_codes::KeyCode;
use tracing::{debug, warn};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayoutNameW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS,
    KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_F13,
    VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_NUMLOCK, VK_PAUSE, VK_RETURN,
    VK_TAB, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, WHEEL_DELTA, XBUTTON1, XBUTTON2,
};

/// How one evdev key is pressed on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stroke {
    /// Set-1 scancode, and whether it carries the E0 (extended) prefix.
    Scan(u16, bool),
    /// A virtual key, for keys whose scancode is ambiguous or unmapped.
    Vk(VIRTUAL_KEY),
}

/// The Windows stroke for an evdev key code, or `None` for a code this table
/// does not know.
pub(crate) fn stroke_for(code: u16) -> Option<Stroke> {
    let ext = |scan| Some(Stroke::Scan(scan, true));
    match KeyCode(code) {
        // Evdev numbered the main block after set-1 scancodes, so these are
        // the same number. NumLock (69) is not: its scancode is shared with
        // Pause and Windows tells them apart by a prefix SendInput cannot set.
        KeyCode::KEY_NUMLOCK => Some(Stroke::Vk(VK_NUMLOCK)),
        KeyCode(c @ 1..=88) => Some(Stroke::Scan(c, false)),
        // The keypad's Enter and slash are the main block's with the
        // prefix, as `windows_scan` reads them back.
        KeyCode(96) => ext(0x1C), // KEY_KPENTER
        KeyCode(98) => ext(0x35), // KEY_KPSLASH
        // The Japanese and Brazilian keys past the main block.
        KeyCode(89) => Some(Stroke::Scan(0x73, false)), // KEY_RO
        KeyCode(92) => Some(Stroke::Scan(0x79, false)), // KEY_HENKAN
        KeyCode(93) => Some(Stroke::Scan(0x70, false)), // KEY_KATAKANAHIRAGANA
        KeyCode(94) => Some(Stroke::Scan(0x7B, false)), // KEY_MUHENKAN
        KeyCode(124) => Some(Stroke::Scan(0x7D, false)), // KEY_YEN
        KeyCode::KEY_RIGHTCTRL => ext(0x1D),
        KeyCode::KEY_SYSRQ => ext(0x37), // Print Screen
        KeyCode::KEY_RIGHTALT => ext(0x38),
        KeyCode::KEY_HOME => ext(0x47),
        KeyCode::KEY_UP => ext(0x48),
        KeyCode::KEY_PAGEUP => ext(0x49),
        KeyCode::KEY_LEFT => ext(0x4B),
        KeyCode::KEY_RIGHT => ext(0x4D),
        KeyCode::KEY_END => ext(0x4F),
        KeyCode::KEY_DOWN => ext(0x50),
        KeyCode::KEY_PAGEDOWN => ext(0x51),
        KeyCode::KEY_INSERT => ext(0x52),
        KeyCode::KEY_DELETE => ext(0x53),
        KeyCode::KEY_LEFTMETA => ext(0x5B),
        KeyCode::KEY_RIGHTMETA => ext(0x5C),
        KeyCode::KEY_MENU => ext(0x5D), // the application key
        KeyCode::KEY_PAUSE => Some(Stroke::Vk(VK_PAUSE)),
        KeyCode::KEY_MUTE => Some(Stroke::Vk(VK_VOLUME_MUTE)),
        KeyCode::KEY_VOLUMEDOWN => Some(Stroke::Vk(VK_VOLUME_DOWN)),
        KeyCode::KEY_VOLUMEUP => Some(Stroke::Vk(VK_VOLUME_UP)),
        KeyCode::KEY_NEXTSONG => Some(Stroke::Vk(VK_MEDIA_NEXT_TRACK)),
        KeyCode::KEY_PLAYPAUSE => Some(Stroke::Vk(VK_MEDIA_PLAY_PAUSE)),
        KeyCode::KEY_PREVIOUSSONG => Some(Stroke::Vk(VK_MEDIA_PREV_TRACK)),
        // F13..F24 are contiguous in both numberings.
        KeyCode(c @ 183..=194) => Some(Stroke::Vk(VIRTUAL_KEY(VK_F13.0 + (c - 183)))),
        _ => None,
    }
}

fn keyboard(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// The `INPUT` that presses or releases one stroke.
pub(crate) fn stroke_input(stroke: Stroke, pressed: bool) -> INPUT {
    let up = if pressed {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    match stroke {
        Stroke::Scan(scan, extended) => {
            let ext = if extended {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            keyboard(VIRTUAL_KEY(0), scan, KEYEVENTF_SCANCODE | ext | up)
        }
        Stroke::Vk(vk) => keyboard(vk, 0, up),
    }
}

/// Down/up pairs that type `text`, one UTF-16 unit at a time (so characters
/// outside the BMP arrive as their surrogate pair). Newline and tab press the
/// real keys, which is what an app expects from typed text.
pub(crate) fn text_inputs(text: &str) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    for ch in text.chars() {
        let key = match ch {
            '\n' | '\r' => Some(VK_RETURN),
            '\t' => Some(VK_TAB),
            _ => None,
        };
        if let Some(vk) = key {
            inputs.push(stroke_input(Stroke::Vk(vk), true));
            inputs.push(stroke_input(Stroke::Vk(vk), false));
            continue;
        }
        let mut units = [0u16; 2];
        for &unit in ch.encode_utf16(&mut units).iter() {
            inputs.push(keyboard(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE));
            inputs.push(keyboard(
                VIRTUAL_KEY(0),
                unit,
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
            ));
        }
    }
    inputs
}

/// Mouse buttons in the injector's numbering (see `MouseKey`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

pub(crate) fn button_input(button: Button, pressed: bool) -> INPUT {
    let (flags, data) = match (button, pressed) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Back, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON1)),
        (Button::Back, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
        (Button::Forward, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON2)),
        (Button::Forward, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
    };
    mouse(0, 0, data, flags)
}

/// An absolute move to a normalized (0..=65535) point on the virtual desktop.
pub(crate) fn absolute_move_input(x_norm: u16, y_norm: u16) -> INPUT {
    mouse(
        i32::from(x_norm),
        i32::from(y_norm),
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

/// Wheel notches, vertical (`y`, positive = away from the user, as on Linux)
/// then horizontal (`x`, positive = right).
pub(crate) fn scroll_inputs(x: i32, y: i32) -> Vec<INPUT> {
    let delta = WHEEL_DELTA as i32;
    fine_scroll_inputs(x.saturating_mul(delta), y.saturating_mul(delta))
}

/// A fine wheel turn in 120ths of a notch, signs as [`scroll_inputs`]:
/// Windows' own unit, so it goes as it is; a turn below `WHEEL_DELTA` is
/// what a precision touchpad sends, and applications add those up.
pub(crate) fn fine_scroll_inputs(x: i32, y: i32) -> Vec<INPUT> {
    let mut inputs = Vec::new();
    if y != 0 {
        inputs.push(mouse(0, 0, y as u32, MOUSEEVENTF_WHEEL));
    }
    if x != 0 {
        inputs.push(mouse(0, 0, x as u32, MOUSEEVENTF_HWHEEL));
    }
    inputs
}

/// The Windows backend's state: just enough to keep refusals quiet.
#[derive(Debug, Default)]
pub(crate) struct SendInputBackend {
    /// The foreground window the last refusal was reported for.
    refused_for: Option<isize>,
}

impl SendInputBackend {
    pub(crate) fn new() -> Self {
        let mut klid = [0u16; 9];
        match unsafe { GetKeyboardLayoutNameW(&mut klid) } {
            Ok(()) => debug!(
                layout = %String::from_utf16_lossy(&klid[..8]),
                "SendInput injects scancodes; the active layout maps them as it would a real keyboard"
            ),
            Err(e) => debug!("keyboard layout unknown ({e})"),
        }
        Self::default()
    }

    /// Send `inputs`; a refusal is logged once per foreground window and is
    /// not an error (the next window may accept).
    pub(crate) fn send(&mut self, inputs: &[INPUT], what: &str) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            self.refused_for = None;
            return Ok(());
        }
        let foreground = unsafe { GetForegroundWindow() }.0 as isize;
        if self.refused_for != Some(foreground) {
            self.refused_for = Some(foreground);
            warn!(
                what,
                sent,
                of = inputs.len(),
                error = %windows::core::Error::from_thread(),
                "Windows refused injected input — the focused window is elevated (UIPI) or the \
                 secure desktop is up; run the server elevated to reach elevated windows"
            );
        }
        Ok(())
    }

    pub(crate) fn key(&mut self, code: u16, pressed: bool) -> Result<()> {
        match stroke_for(code) {
            Some(stroke) => self.send(&[stroke_input(stroke, pressed)], "key"),
            None => {
                // That a key had none, never which: no keystroke in a log.
                debug!("no Windows stroke for a key, not injecting it");
                Ok(())
            }
        }
    }

    pub(crate) fn text(&mut self, text: &str) -> Result<()> {
        self.send(&text_inputs(text), "text")
    }

    pub(crate) fn move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.send(&[mouse(dx, dy, 0, MOUSEEVENTF_MOVE)], "mouse move")
    }

    pub(crate) fn move_absolute(&mut self, x_norm: u16, y_norm: u16) -> Result<()> {
        self.send(&[absolute_move_input(x_norm, y_norm)], "absolute move")
    }

    pub(crate) fn button(&mut self, button: Button, pressed: bool) -> Result<()> {
        self.send(&[button_input(button, pressed)], "mouse button")
    }

    pub(crate) fn scroll(&mut self, x: i32, y: i32) -> Result<()> {
        self.send(&scroll_inputs(x, y), "scroll")
    }

    pub(crate) fn scroll_fine(&mut self, x: i32, y: i32) -> Result<()> {
        self.send(&fine_scroll_inputs(x, y), "scroll")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::input::keys::phone_emittable_codes;

    fn key_flags(input: &INPUT) -> (u16, u16, u32) {
        let ki = unsafe { input.Anonymous.ki };
        (ki.wVk.0, ki.wScan, ki.dwFlags.0)
    }

    /// A key with no stroke is logged as one, never by its code (no
    /// keystroke goes in a log); nothing is sent for it.
    #[test]
    fn a_key_with_no_stroke_is_logged_without_its_code() {
        let unmapped = 0x2fe;
        assert!(stroke_for(unmapped).is_none());
        let (caught, guard) = crate::shared_input::caught_log::catch();
        SendInputBackend::default().key(unmapped, true).unwrap();
        drop(guard);
        let log = caught.text();
        assert!(log.contains("no Windows stroke"), "{log}");
        assert!(!log.contains("766") && !log.contains("code"), "{log}");
    }

    #[test]
    fn every_key_the_phone_can_send_has_a_windows_stroke() {
        for code in phone_emittable_codes() {
            assert!(stroke_for(code).is_some(), "evdev {code} has no stroke");
        }
    }

    /// Keys by position (the window's scancode mode): every key a Windows
    /// keyboard names by its scancode, as `windows_scan` reads it, is
    /// pressed here again, and a key pressed by its scancode is that same
    /// scancode, never one the layout picks.
    #[test]
    fn every_scancode_read_back_is_pressed_as_itself() {
        use pocketdesk_core::input::windows_scan::evdev_from_scan;
        let mut seen = 0;
        for extended in [false, true] {
            for scan in 0..=0x7Fu16 {
                let Some(code) = evdev_from_scan(scan, extended) else {
                    continue;
                };
                seen += 1;
                match stroke_for(code) {
                    Some(Stroke::Scan(s, e)) => assert_eq!(
                        evdev_from_scan(s, e),
                        Some(code),
                        "evdev {code} goes as another key"
                    ),
                    Some(Stroke::Vk(_)) => {}
                    None => panic!("evdev {code} (scan {scan:#x}, ext {extended}) has no stroke"),
                }
            }
        }
        assert!(seen > 100, "{seen}");
        assert_eq!(stroke_for(96), Some(Stroke::Scan(0x1C, true)));
        assert_eq!(stroke_for(98), Some(Stroke::Scan(0x35, true)));
        assert_eq!(stroke_for(86), Some(Stroke::Scan(0x56, false)));
    }

    /// PR #573 review N6: the keys added, held to the set-1 scancodes
    /// Microsoft's keyboard scan code specification gives them, written
    /// here apart from `windows_scan` (the round trip above only says the
    /// two tables agree). Whether a layout without these keys does
    /// anything with them is a device check (§240).
    #[test]
    fn the_keys_added_are_their_documented_set1_scancodes() {
        for (code, stroke) in [
            (96, Stroke::Scan(0x1C, true)),   // KEY_KPENTER: E0 1C
            (98, Stroke::Scan(0x35, true)),   // KEY_KPSLASH: E0 35
            (89, Stroke::Scan(0x73, false)),  // KEY_RO (ABNT slash, JIS ro)
            (92, Stroke::Scan(0x79, false)),  // KEY_HENKAN (Convert)
            (93, Stroke::Scan(0x70, false)),  // KEY_KATAKANAHIRAGANA
            (94, Stroke::Scan(0x7B, false)),  // KEY_MUHENKAN (No convert)
            (124, Stroke::Scan(0x7D, false)), // KEY_YEN
        ] {
            assert_eq!(stroke_for(code), Some(stroke), "evdev {code}");
        }
    }

    #[test]
    fn letters_and_digits_are_their_own_scancodes() {
        assert_eq!(
            stroke_for(KeyCode::KEY_A.0),
            Some(Stroke::Scan(0x1E, false))
        );
        assert_eq!(
            stroke_for(KeyCode::KEY_1.0),
            Some(Stroke::Scan(0x02, false))
        );
        assert_eq!(
            stroke_for(KeyCode::KEY_ESC.0),
            Some(Stroke::Scan(0x01, false))
        );
        assert_eq!(
            stroke_for(KeyCode::KEY_F12.0),
            Some(Stroke::Scan(0x58, false))
        );
    }

    #[test]
    fn navigation_and_right_modifiers_are_extended() {
        for code in [
            KeyCode::KEY_UP,
            KeyCode::KEY_DOWN,
            KeyCode::KEY_LEFT,
            KeyCode::KEY_RIGHT,
            KeyCode::KEY_HOME,
            KeyCode::KEY_END,
            KeyCode::KEY_PAGEUP,
            KeyCode::KEY_PAGEDOWN,
            KeyCode::KEY_DELETE,
            KeyCode::KEY_RIGHTCTRL,
            KeyCode::KEY_RIGHTALT,
            KeyCode::KEY_LEFTMETA,
            KeyCode::KEY_RIGHTMETA,
            KeyCode::KEY_MENU,
        ] {
            assert!(
                matches!(stroke_for(code.0), Some(Stroke::Scan(_, true))),
                "{code:?} must be an extended scancode"
            );
        }
        // The left-hand twins are not.
        assert_eq!(
            stroke_for(KeyCode::KEY_LEFTCTRL.0),
            Some(Stroke::Scan(0x1D, false))
        );
        assert_eq!(
            stroke_for(KeyCode::KEY_LEFTALT.0),
            Some(Stroke::Scan(0x38, false))
        );
    }

    #[test]
    fn numlock_media_and_high_function_keys_use_virtual_keys() {
        assert_eq!(
            stroke_for(KeyCode::KEY_NUMLOCK.0),
            Some(Stroke::Vk(VK_NUMLOCK))
        );
        assert_eq!(
            stroke_for(KeyCode::KEY_PLAYPAUSE.0),
            Some(Stroke::Vk(VK_MEDIA_PLAY_PAUSE))
        );
        assert_eq!(stroke_for(KeyCode::KEY_F13.0), Some(Stroke::Vk(VK_F13)));
        assert_eq!(
            stroke_for(KeyCode::KEY_F24.0),
            Some(Stroke::Vk(VIRTUAL_KEY(VK_F13.0 + 11)))
        );
    }

    #[test]
    fn release_sets_keyup_and_keeps_the_extended_bit() {
        let up = stroke_input(Stroke::Scan(0x48, true), false);
        let (_, scan, flags) = key_flags(&up);
        assert_eq!(scan, 0x48);
        let want = KEYEVENTF_SCANCODE.0 | KEYEVENTF_EXTENDEDKEY.0 | KEYEVENTF_KEYUP.0;
        assert_eq!(flags, want);
    }

    #[test]
    fn text_is_utf16_with_surrogate_pairs_and_real_enter() {
        let inputs = text_inputs("Aé😀\n");
        // A, é: one unit each; the emoji: a surrogate pair; newline: Enter.
        assert_eq!(inputs.len(), 2 + 2 + 4 + 2);
        assert_eq!(key_flags(&inputs[0]).1, u16::from(b'A'));
        assert_eq!(key_flags(&inputs[2]).1, 0xE9);
        assert_eq!(key_flags(&inputs[4]).1, 0xD83D);
        assert_eq!(key_flags(&inputs[6]).1, 0xDE00);
        assert_eq!(key_flags(&inputs[8]).0, VK_RETURN.0);
        assert_eq!(
            key_flags(&inputs[9]).2 & KEYEVENTF_KEYUP.0,
            KEYEVENTF_KEYUP.0
        );
    }

    #[test]
    fn absolute_moves_span_the_virtual_desktop() {
        for (x, y) in [(0u16, 0u16), (65535, 65535), (32768, 100)] {
            let mi = unsafe { absolute_move_input(x, y).Anonymous.mi };
            assert_eq!((mi.dx, mi.dy), (i32::from(x), i32::from(y)));
            let want = MOUSEEVENTF_MOVE.0 | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0;
            assert_eq!(mi.dwFlags.0, want);
        }
    }

    #[test]
    fn wheel_notches_scale_by_wheel_delta_and_keep_their_sign() {
        let inputs = scroll_inputs(-1, 2);
        let wheel = unsafe { inputs[0].Anonymous.mi };
        assert_eq!(wheel.mouseData as i32, 240);
        assert_eq!(wheel.dwFlags, MOUSEEVENTF_WHEEL);
        let hwheel = unsafe { inputs[1].Anonymous.mi };
        assert_eq!(hwheel.mouseData as i32, -120);
        assert_eq!(hwheel.dwFlags, MOUSEEVENTF_HWHEEL);
        assert!(scroll_inputs(0, 0).is_empty());
    }

    #[test]
    fn a_fine_turn_goes_in_wheel_delta_units_as_it_is() {
        let inputs = fine_scroll_inputs(-30, 45);
        let wheel = unsafe { inputs[0].Anonymous.mi };
        assert_eq!(wheel.mouseData as i32, 45);
        assert_eq!(wheel.dwFlags, MOUSEEVENTF_WHEEL);
        let hwheel = unsafe { inputs[1].Anonymous.mi };
        assert_eq!(hwheel.mouseData as i32, -30);
        assert_eq!(hwheel.dwFlags, MOUSEEVENTF_HWHEEL);
        assert!(fine_scroll_inputs(0, 0).is_empty());
        assert_eq!(fine_scroll_inputs(0, 7).len(), 1);
    }

    #[test]
    fn side_buttons_name_their_xbutton() {
        let back = unsafe { button_input(Button::Back, true).Anonymous.mi };
        assert_eq!((back.dwFlags, back.mouseData), (MOUSEEVENTF_XDOWN, 1));
        let fwd = unsafe { button_input(Button::Forward, false).Anonymous.mi };
        assert_eq!((fwd.dwFlags, fwd.mouseData), (MOUSEEVENTF_XUP, 2));
    }

    /// Live: absolute moves land on the pixel the virtual-desktop math says,
    /// and an injected key is really held. Moves the pointer briefly and
    /// restores it; F24 is a key no app binds. Run on a desktop session:
    /// `cargo test -p pocketdesk-server --lib sendinput_drives -- --ignored`.
    #[test]
    #[ignore = "moves the real pointer on an interactive Windows desktop"]
    fn sendinput_drives_the_real_pointer_and_keyboard() {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetCursorPos,
        };
        crate::platform::init_process();
        let mut backend = SendInputBackend::new();
        let mut original = POINT::default();
        unsafe { GetCursorPos(&mut original) }.unwrap();

        let (left, top, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        for (xn, yn) in [(16384u16, 16384u16), (49152, 32768)] {
            backend.move_absolute(xn, yn).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut now = POINT::default();
            unsafe { GetCursorPos(&mut now) }.unwrap();
            let want_x = left + (i64::from(xn) * i64::from(width) / 65536) as i32;
            let want_y = top + (i64::from(yn) * i64::from(height) / 65536) as i32;
            assert!(
                (now.x - want_x).abs() <= 1 && (now.y - want_y).abs() <= 1,
                "({xn},{yn}) landed at ({},{}), expected ({want_x},{want_y})",
                now.x,
                now.y
            );
        }
        unsafe { SetCursorPos(original.x, original.y) }.unwrap();

        backend.key(KeyCode::KEY_F24.0, true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let held = unsafe { GetAsyncKeyState(i32::from(VK_F13.0 + 11)) } as u16 & 0x8000 != 0;
        backend.key(KeyCode::KEY_F24.0, false).unwrap();
        assert!(held, "injected F24 was not seen as held");
    }
}
