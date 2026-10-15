//! Native input injection with fallback chain: enigo (X11/XWayland) -> uinput (kernel-level)
//!
//! Tries enigo first (works on X11 and XWayland sessions).
//! Falls back to evdev/uinput (works on ALL compositors, requires /dev/uinput access).
//! uinput creates virtual HID devices at the kernel level, so every compositor sees them as real input devices.
//!
//! On Windows the only backend is `SendInput` (`input_sendinput.rs`): scancodes
//! for keys, UTF-16 for text, virtual-desktop coordinates for absolute moves.

use anyhow::{Context, Result};
#[cfg(not(windows))]
use enigo::Settings;
use enigo::{Coordinate, Enigo, Key, Keyboard, Mouse};
#[cfg(target_os = "linux")]
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
#[cfg(target_os = "linux")]
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, InputEvent, KeyCode, RelativeAxisCode, UinputAbsSetup,
};
// Off Linux the wire's evdev codes still need names; the core shim has the
// same `KeyCode::KEY_*.0` shape as the evdev crate.
#[cfg(not(target_os = "linux"))]
use pocketdesk_core::input::evdev_codes::KeyCode;
use pocketdesk_core::streaming::input_packet::InputPacket;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use tracing::warn;
use tracing::{debug, info};

/// Event type constants from evdev kernel API
#[cfg(target_os = "linux")]
const EV_KEY: u16 = 0x01;
#[cfg(target_os = "linux")]
const EV_REL: u16 = 0x02;
#[cfg(target_os = "linux")]
const EV_SYN: u16 = 0x00;
#[cfg(target_os = "linux")]
const SYN_REPORT: u16 = 0;

/// Canonical mapping between Linux evdev keycodes and enigo `Key` values.
///
/// This is the single source of truth for key translation between backends:
/// `keycode_to_enigo` and `key_to_evdev` read it in opposite directions, and
/// `tests::every_key_the_phone_can_send_has_a_mapping` fails the moment it stops
/// covering `core::input::keys`, the set the Android client emits.
///
/// Codes are named through `evdev::KeyCode` instead of written as numbers
/// because a wrong number here silently injects a different key than the one the
/// phone pressed; a wrong *name* is a compile error.
///
/// Letters, digits and punctuation map to `Key::Unicode`: enigo has no
/// per-letter variant on Linux (`Key::A` and friends are Windows-only) and its
/// X11 backend resolves `Unicode(c)` through that character's keysym, which is
/// what the evdev code stands for anyway. Capitalisation is not this table's
/// problem — the phone sends Shift as its own key event and X applies it.
const KEYCODE_MAP: &[(u16, Key)] = &[
    // Editing and control
    (KeyCode::KEY_ESC.0, Key::Escape),
    (KeyCode::KEY_BACKSPACE.0, Key::Backspace),
    (KeyCode::KEY_TAB.0, Key::Tab),
    (KeyCode::KEY_ENTER.0, Key::Return),
    (KeyCode::KEY_SPACE.0, Key::Space),
    (KeyCode::KEY_CAPSLOCK.0, Key::CapsLock),
    (KeyCode::KEY_NUMLOCK.0, Key::Numlock),
    (KeyCode::KEY_INSERT.0, Key::Insert),
    (KeyCode::KEY_DELETE.0, Key::Delete),
    // Navigation
    (KeyCode::KEY_HOME.0, Key::Home),
    (KeyCode::KEY_UP.0, Key::UpArrow),
    (KeyCode::KEY_PAGEUP.0, Key::PageUp),
    (KeyCode::KEY_LEFT.0, Key::LeftArrow),
    (KeyCode::KEY_RIGHT.0, Key::RightArrow),
    (KeyCode::KEY_END.0, Key::End),
    (KeyCode::KEY_DOWN.0, Key::DownArrow),
    (KeyCode::KEY_PAGEDOWN.0, Key::PageDown),
    // Modifiers. evdev names left and right separately; enigo has a right-hand
    // variant only for Shift and Control, so Right Alt and Right Super ride
    // `Key::Other`, which its X11 backend reads as a raw keysym.
    (KeyCode::KEY_LEFTSHIFT.0, Key::LShift),
    (KeyCode::KEY_RIGHTSHIFT.0, Key::RShift),
    (KeyCode::KEY_LEFTCTRL.0, Key::LControl),
    (KeyCode::KEY_RIGHTCTRL.0, Key::RControl),
    (KeyCode::KEY_LEFTALT.0, Key::Alt),
    #[cfg(not(windows))]
    (KeyCode::KEY_RIGHTALT.0, Key::Other(0xFFEA)), // XK_Alt_R
    #[cfg(windows)]
    (KeyCode::KEY_RIGHTALT.0, Key::RMenu),
    (KeyCode::KEY_LEFTMETA.0, Key::Meta), // Super
    #[cfg(not(windows))]
    (KeyCode::KEY_RIGHTMETA.0, Key::Other(0xFFEC)), // XK_Super_R
    #[cfg(windows)]
    (KeyCode::KEY_RIGHTMETA.0, Key::RWin),
    // Function keys. F11/F12 are 87/88, not the two numbers after F10, because
    // 69 and 70 are Num Lock and Scroll Lock. F13-F24 stay in this table so the
    // X11 rung is no weaker than uinput, which will emit any code it declares.
    (KeyCode::KEY_F1.0, Key::F1),
    (KeyCode::KEY_F2.0, Key::F2),
    (KeyCode::KEY_F3.0, Key::F3),
    (KeyCode::KEY_F4.0, Key::F4),
    (KeyCode::KEY_F5.0, Key::F5),
    (KeyCode::KEY_F6.0, Key::F6),
    (KeyCode::KEY_F7.0, Key::F7),
    (KeyCode::KEY_F8.0, Key::F8),
    (KeyCode::KEY_F9.0, Key::F9),
    (KeyCode::KEY_F10.0, Key::F10),
    (KeyCode::KEY_F11.0, Key::F11),
    (KeyCode::KEY_F12.0, Key::F12),
    (KeyCode::KEY_F13.0, Key::F13),
    (KeyCode::KEY_F14.0, Key::F14),
    (KeyCode::KEY_F15.0, Key::F15),
    (KeyCode::KEY_F16.0, Key::F16),
    (KeyCode::KEY_F17.0, Key::F17),
    (KeyCode::KEY_F18.0, Key::F18),
    (KeyCode::KEY_F19.0, Key::F19),
    (KeyCode::KEY_F20.0, Key::F20),
    (KeyCode::KEY_F21.0, Key::F21),
    (KeyCode::KEY_F22.0, Key::F22),
    (KeyCode::KEY_F23.0, Key::F23),
    (KeyCode::KEY_F24.0, Key::F24),
    // Keys the phone's system bar and shortcut sheet send. The Print key arrives
    // as KEY_SYSRQ, which is the code XKB binds to the `Print` keysym.
    (KeyCode::KEY_SYSRQ.0, Key::PrintScr),
    (KeyCode::KEY_VOLUMEDOWN.0, Key::VolumeDown),
    (KeyCode::KEY_VOLUMEUP.0, Key::VolumeUp),
    (KeyCode::KEY_NEXTSONG.0, Key::MediaNextTrack),
    (KeyCode::KEY_PLAYPAUSE.0, Key::MediaPlayPause),
    (KeyCode::KEY_PREVIOUSSONG.0, Key::MediaPrevTrack),
    // enigo calls the X `Menu` keysym `LMenu`; the application-keys key has no
    // left/right form. (evdev KEY_COMPOSE is 127, a different key.)
    // X11 resolves `LMenu` to the Menu keysym; on Windows it is VK_LMENU, the
    // left Alt, and the application key is `Apps`.
    #[cfg(not(windows))]
    (KeyCode::KEY_MENU.0, Key::LMenu),
    #[cfg(windows)]
    (KeyCode::KEY_MENU.0, Key::Apps),
    // The dialpad's star and plus are keypad codes, so they get the keypad
    // keysyms; the plain '*' and '+' characters reach the same codes through
    // `char_to_keycode`.
    (KeyCode::KEY_KPASTERISK.0, Key::Multiply),
    (KeyCode::KEY_KPPLUS.0, Key::Add),
    // Digits, in the order they are printed rather than their codes.
    (KeyCode::KEY_1.0, Key::Unicode('1')),
    (KeyCode::KEY_2.0, Key::Unicode('2')),
    (KeyCode::KEY_3.0, Key::Unicode('3')),
    (KeyCode::KEY_4.0, Key::Unicode('4')),
    (KeyCode::KEY_5.0, Key::Unicode('5')),
    (KeyCode::KEY_6.0, Key::Unicode('6')),
    (KeyCode::KEY_7.0, Key::Unicode('7')),
    (KeyCode::KEY_8.0, Key::Unicode('8')),
    (KeyCode::KEY_9.0, Key::Unicode('9')),
    (KeyCode::KEY_0.0, Key::Unicode('0')),
    // Letters, QWERTY rows.
    (KeyCode::KEY_Q.0, Key::Unicode('q')),
    (KeyCode::KEY_W.0, Key::Unicode('w')),
    (KeyCode::KEY_E.0, Key::Unicode('e')),
    (KeyCode::KEY_R.0, Key::Unicode('r')),
    (KeyCode::KEY_T.0, Key::Unicode('t')),
    (KeyCode::KEY_Y.0, Key::Unicode('y')),
    (KeyCode::KEY_U.0, Key::Unicode('u')),
    (KeyCode::KEY_I.0, Key::Unicode('i')),
    (KeyCode::KEY_O.0, Key::Unicode('o')),
    (KeyCode::KEY_P.0, Key::Unicode('p')),
    (KeyCode::KEY_A.0, Key::Unicode('a')),
    (KeyCode::KEY_S.0, Key::Unicode('s')),
    (KeyCode::KEY_D.0, Key::Unicode('d')),
    (KeyCode::KEY_F.0, Key::Unicode('f')),
    (KeyCode::KEY_G.0, Key::Unicode('g')),
    (KeyCode::KEY_H.0, Key::Unicode('h')),
    (KeyCode::KEY_J.0, Key::Unicode('j')),
    (KeyCode::KEY_K.0, Key::Unicode('k')),
    (KeyCode::KEY_L.0, Key::Unicode('l')),
    (KeyCode::KEY_Z.0, Key::Unicode('z')),
    (KeyCode::KEY_X.0, Key::Unicode('x')),
    (KeyCode::KEY_C.0, Key::Unicode('c')),
    (KeyCode::KEY_V.0, Key::Unicode('v')),
    (KeyCode::KEY_B.0, Key::Unicode('b')),
    (KeyCode::KEY_N.0, Key::Unicode('n')),
    (KeyCode::KEY_M.0, Key::Unicode('m')),
    // Punctuation the phone's symbol sheet reaches for most often.
    (KeyCode::KEY_MINUS.0, Key::Unicode('-')),
    (KeyCode::KEY_EQUAL.0, Key::Unicode('=')),
    (KeyCode::KEY_LEFTBRACE.0, Key::Unicode('[')),
    (KeyCode::KEY_RIGHTBRACE.0, Key::Unicode(']')),
    (KeyCode::KEY_SEMICOLON.0, Key::Unicode(';')),
    (KeyCode::KEY_APOSTROPHE.0, Key::Unicode('\'')),
    (KeyCode::KEY_GRAVE.0, Key::Unicode('`')),
    (KeyCode::KEY_COMMA.0, Key::Unicode(',')),
    (KeyCode::KEY_DOT.0, Key::Unicode('.')),
    (KeyCode::KEY_SLASH.0, Key::Unicode('/')),
    (KeyCode::KEY_BACKSLASH.0, Key::Unicode('\\')),
];

/// Backend for input injection.
#[derive(Debug)]
enum InputBackend {
    /// enigo (X11/XWayland via XTEST)
    #[cfg_attr(windows, allow(dead_code))]
    Enigo(Box<Mutex<Enigo>>),
    /// Windows `SendInput`
    #[cfg(windows)]
    SendInput(crate::input_sendinput::SendInputBackend),
    /// uinput (universal, kernel-level)
    #[cfg(target_os = "linux")]
    Uinput(Mutex<UinputState>),
}

/// Kernel-level backend state: the keyboard/mouse device plus an
/// absolute-pointer device used for absolute (normalized) pointer injection.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct UinputState {
    main: VirtualDevice,
    /// `ABS_X`/`ABS_Y` virtual pointer. Hyprland's libinput drops uinput MT
    /// touchscreens on this box, but an ABS pointer maps 1:1 onto the layout
    /// and moves the real cursor (verified with `hyprctl cursorpos`).
    abs: Option<VirtualDevice>,
}

#[cfg(target_os = "linux")]
const EV_ABS: u16 = 0x03;
/// Direct-touch axis range; normalized wire coordinates (0..=65535) map 1:1.
const ABS_MAX_COORD: i32 = 65535;

/// Cross-distro input injector.
///
/// Tries enigo (X11) first, falls back to uinput (kernel-level virtual device).
/// uinput works on ALL compositors but requires /dev/uinput access (root or uinput group).
pub struct InputInjector {
    backend: InputBackend,
    /// Button mask of the previous `Gamepad` packet. That packet is a state
    /// snapshot, so every bit must be treated as an edge (see 2063).
    gamepad_buttons: u16,
    /// The compositor's virtual keyboard for text (roadmap F4): not tried
    /// yet (`None`), not offered (`Some(None)`), or ready.
    #[cfg(target_os = "linux")]
    vkeyboard: VkState,
}

/// The compositor's virtual keyboard for text (roadmap F4).
#[cfg(target_os = "linux")]
enum VkState {
    Untried,
    Ready(pocketdesk_core::input::vkeyboard::VirtualKeyboard),
    /// The compositor does not offer one (GNOME, KDE).
    Unsupported,
    /// Connecting or typing failed: tried again after this.
    RetryAfter(std::time::Instant),
}

#[cfg(target_os = "linux")]
impl VkState {
    /// How long a failed virtual keyboard rests before it is tried again.
    const RETRY: std::time::Duration = std::time::Duration::from_secs(30);

    fn retry_later() -> Self {
        VkState::RetryAfter(std::time::Instant::now() + Self::RETRY)
    }

    /// The keyboard, connecting first when it is time to.
    fn ready(&mut self) -> Option<&mut pocketdesk_core::input::vkeyboard::VirtualKeyboard> {
        use pocketdesk_core::input::vkeyboard::{ConnectError, VirtualKeyboard};
        let due = match self {
            VkState::Untried => true,
            VkState::RetryAfter(at) => std::time::Instant::now() >= *at,
            VkState::Ready(_) | VkState::Unsupported => false,
        };
        if due {
            *self = match VirtualKeyboard::connect() {
                Ok(vk) => {
                    info!("Text is typed through the compositor's virtual keyboard");
                    VkState::Ready(vk)
                }
                Err(ConnectError::Unsupported) => {
                    info!("No virtual keyboard: text is typed as US-layout keys");
                    VkState::Unsupported
                }
                Err(ConnectError::Failed(e)) => {
                    warn!("Virtual keyboard unavailable for now: {e}");
                    Self::retry_later()
                }
            };
        }
        match self {
            VkState::Ready(vk) => Some(vk),
            _ => None,
        }
    }
}

/// What a gamepad button bit stands for on the desktop. The gamepad is a wire
/// format plus toy emulation until a real source exists (roadmap 2695); this
/// is the whole mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GamepadAction {
    Key(Key),
    LeftClick,
}

/// Gamepad button bits to desktop action, in the order the old hand-written
/// `if` chain applied them.
const GAMEPAD_BUTTONS: [(u16, GamepadAction); 7] = [
    (0x0001, GamepadAction::Key(Key::Return)),     // A
    (0x0002, GamepadAction::Key(Key::Escape)),     // B
    (0x0080, GamepadAction::LeftClick),            // Start
    (0x0800, GamepadAction::Key(Key::UpArrow)),    // DPad up
    (0x1000, GamepadAction::Key(Key::DownArrow)),  // DPad down
    (0x2000, GamepadAction::Key(Key::LeftArrow)),  // DPad left
    (0x4000, GamepadAction::Key(Key::RightArrow)), // DPad right
];

/// Which mapped gamepad buttons changed state between two snapshots, and
/// whether they are now held. A packet replayed while a button is still down
/// produces nothing, and a button that came up produces its release — which is
/// the half the old code never emitted.
impl InputInjector {
    /// Put `buttons` in as the gamepad's last state and return the one it
    /// replaces: the server keeps one state per phone (roadmap N7).
    pub fn swap_gamepad_buttons(&mut self, buttons: u16) -> u16 {
        std::mem::replace(&mut self.gamepad_buttons, buttons)
    }
}

fn gamepad_edges(previous: u16, current: u16) -> Vec<(GamepadAction, bool)> {
    GAMEPAD_BUTTONS
        .iter()
        .filter_map(|&(bit, action)| {
            let was_held = previous & bit != 0;
            let is_held = current & bit != 0;
            (was_held != is_held).then_some((action, is_held))
        })
        .collect()
}

impl Drop for InputInjector {
    fn drop(&mut self) {
        let backend_type = match &self.backend {
            InputBackend::Enigo(_) => "enigo",
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(_) => "uinput",
            #[cfg(windows)]
            InputBackend::SendInput(_) => "sendinput",
        };
        info!(backend = %backend_type, "InputInjector dropped, closing backend resources");
    }
}

impl InputInjector {
    /// Create a new input injector.
    ///
    /// Under a Wayland compositor uinput is tried first: enigo's XTEST only
    /// reaches XWayland's internal pointer, so native clients never see it
    /// (verified on Hyprland — cursor warps logged fine, real cursor stuck).
    /// Otherwise (bare X11) or if /dev/uinput is inaccessible, enigo is
    /// tried first with uinput as the fallback.
    pub fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            info!("Input injector: using SendInput");
            Ok(Self {
                backend: InputBackend::SendInput(crate::input_sendinput::SendInputBackend::new()),
                gamepad_buttons: 0,
            })
        }
        #[cfg(not(windows))]
        Self::new_desktop()
    }

    /// The Linux/X11 constructor: uinput under Wayland, else enigo, else uinput.
    #[cfg(not(windows))]
    fn new_desktop() -> Result<Self> {
        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            match Self::new_uinput() {
                Ok(inj) => return Ok(inj),
                Err(e) => {
                    warn!(error = %e, "uinput unavailable under Wayland, falling back to enigo (XWayland-only injection)");
                }
            }
        }

        let enigo_err = match Enigo::new(&Settings::default()) {
            Ok(enigo) => {
                info!("Input injector: using enigo (X11/XWayland, or SendInput on Windows)");
                return Ok(Self {
                    backend: InputBackend::Enigo(Box::new(Mutex::new(enigo))),
                    gamepad_buttons: 0,
                    #[cfg(target_os = "linux")]
                    vkeyboard: VkState::Untried,
                });
            }
            Err(e) => e,
        };

        // Fall back to uinput
        #[cfg(target_os = "linux")]
        {
            let _ = enigo_err;
            Self::new_uinput()
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!("no input injection backend: enigo failed ({enigo_err})")
        }
    }

    /// Create a uinput-based input injector.
    #[cfg(target_os = "linux")]
    fn new_uinput() -> Result<Self> {
        if !Path::new("/dev/uinput").exists() {
            anyhow::bail!(
                "/dev/uinput not found. Input injection requires either:\n\
                 - X11/XWayland session (for enigo/XTEST), or\n\
                 - /dev/uinput device (run: sudo modprobe uinput)"
            );
        }

        // Build a virtual keyboard + mouse device, plus the absolute pointer
        // direct-touch motion rides.
        let device = build_main_device()?;

        info!("Input injector: using uinput (kernel-level, works on all compositors)");
        report_layout_assumption();
        // The abs pointer is built eagerly: libinput opens new devices
        // asynchronously after UI_DEV_CREATE, so a device created on the
        // first absolute motion loses that very event (the phone's first
        // tap silently no-op'd).
        Ok(Self {
            backend: InputBackend::Uinput(Mutex::new(UinputState {
                main: device,
                abs: Some(build_abs_device()?),
            })),
            gamepad_buttons: 0,
            vkeyboard: VkState::Untried,
        })
    }

    /// Move mouse by relative delta
    pub fn move_mouse_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => backend.move_relative(dx, dy),
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                e.move_mouse(dx, dy, Coordinate::Rel)
                    .context("enigo mouse move failed")?;
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let events = [
                    InputEvent::new(EV_REL, RelativeAxisCode::REL_X.0, dx),
                    InputEvent::new(EV_REL, RelativeAxisCode::REL_Y.0, dy),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0), // SYN_REPORT
                ];
                emit_uinput(guard, UinputDevice::Main, &events, "mouse move")
            }
        }
    }

    /// Move the pointer to a normalized absolute position (0..=65535 per axis).
    ///
    /// uinput injects through the ABS pointer device, so no
    /// display resolution is needed on either side of the wire.
    pub fn move_mouse_normalized_abs(&mut self, x_norm: u16, y_norm: u16) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => backend.move_absolute(x_norm, y_norm),
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                let (w, h) = e.main_display().context("enigo main_display failed")?;
                e.move_mouse(
                    x_norm as i32 * w / ABS_MAX_COORD,
                    y_norm as i32 * h / ABS_MAX_COORD,
                    Coordinate::Abs,
                )
                .context("enigo absolute mouse move failed")?;
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let events = [
                    InputEvent::new(EV_ABS, AbsoluteAxisCode::ABS_X.0, x_norm as i32),
                    InputEvent::new(EV_ABS, AbsoluteAxisCode::ABS_Y.0, y_norm as i32),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0),
                ];
                emit_uinput(guard, UinputDevice::Abs, &events, "absolute move")
            }
        }
    }

    /// Press or release a mouse button
    pub fn mouse_button(&mut self, button: MouseKey, pressed: bool) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => {
                backend.button(button.as_sendinput_button(), pressed)
            }
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                if pressed {
                    e.button(button.as_enigo_button(), enigo::Direction::Press)
                        .context("enigo mouse press failed")?;
                } else {
                    e.button(button.as_enigo_button(), enigo::Direction::Release)
                        .context("enigo mouse release failed")?;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let key = button.as_evdev_key();
                let value = if pressed { 1 } else { 0 };
                let events = [
                    InputEvent::new(EV_KEY, key.0, value),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0),
                ];
                emit_uinput(guard, UinputDevice::Main, &events, "mouse click")
            }
        }
    }

    /// Scroll the mouse wheel
    pub fn scroll(&mut self, x: i32, y: i32) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => backend.scroll(x, y),
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                if y != 0 {
                    e.scroll((y.abs() / 10).max(1), enigo::Axis::Vertical)
                        .context("enigo vertical scroll failed")?;
                }
                if x != 0 {
                    e.scroll((x.abs() / 10).max(1), enigo::Axis::Horizontal)
                        .context("enigo horizontal scroll failed")?;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let events = [
                    InputEvent::new(EV_REL, RelativeAxisCode::REL_WHEEL.0, y),
                    InputEvent::new(EV_REL, RelativeAxisCode::REL_HWHEEL.0, x),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0),
                ];
                emit_uinput(guard, UinputDevice::Main, &events, "scroll")
            }
        }
    }

    /// Inject one Linux evdev keycode — what the wire actually carries.
    ///
    /// The uinput backend emits the code straight to the kernel. Routing it
    /// through enigo's `Key` first (as this used to) made every key depend on a
    /// table that exists for the X11 rung's benefit: an unmapped code became a
    /// control character, that control character became `KEY_UNKNOWN`, and a
    /// letter, a digit or Shift arriving from the phone was simply never seen by
    /// the desktop. The virtual keyboard declares codes 0..255, so anything the
    /// phone can name is already registrable here.
    fn inject_keycode(&mut self, code: u16, pressed: bool) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => backend.key(code, pressed),
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let events = [
                    InputEvent::new(EV_KEY, code, i32::from(pressed)),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0),
                ];
                emit_uinput(guard, UinputDevice::Main, &events, "keycode")
            }
            InputBackend::Enigo(_) => match keycode_to_enigo(code) {
                Some(key) => self.key(key, pressed),
                None => {
                    debug!(code, "no enigo key for this evdev keycode, not injecting");
                    Ok(())
                }
            },
        }
    }

    /// Press and release a single key
    pub fn key(&mut self, key: Key, pressed: bool) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => match key_to_evdev(key) {
                KeyCode::KEY_UNKNOWN => match key {
                    // A character the table cannot place still types.
                    Key::Unicode(ch) if pressed => backend.text(&ch.to_string()),
                    _ => Ok(()),
                },
                code => backend.key(code.0, pressed),
            },
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                if pressed {
                    e.key(key, enigo::Direction::Press)
                        .context("enigo key press failed")?;
                } else {
                    e.key(key, enigo::Direction::Release)
                        .context("enigo key release failed")?;
                }
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                let guard = state.get_mut().unwrap();
                let evdev_key = key_to_evdev(key);
                let value = if pressed { 1 } else { 0 };
                let events = [
                    InputEvent::new(EV_KEY, evdev_key.0, value),
                    InputEvent::new(EV_SYN, SYN_REPORT, 0),
                ];
                emit_uinput(guard, UinputDevice::Main, &events, "key")
            }
        }
    }

    /// Type text by entering it as unicode characters
    pub fn text(&mut self, text: &str) -> Result<()> {
        match &mut self.backend {
            #[cfg(windows)]
            InputBackend::SendInput(backend) => backend.text(text),
            InputBackend::Enigo(enigo) => {
                let e = enigo.get_mut().unwrap();
                e.text(text).context("enigo text input failed")?;
                Ok(())
            }
            #[cfg(target_os = "linux")]
            InputBackend::Uinput(state) => {
                // Any character, whatever the layout, where the compositor
                // offers a virtual keyboard (wlroots: Sway, Hyprland; F4).
                let mut rest = std::borrow::Cow::Borrowed(text);
                if let Some(vk) = self.vkeyboard.ready() {
                    match vk.type_text(text) {
                        Ok(()) => return Ok(()),
                        Err((sent, e)) => {
                            // What went out is not typed again; the rest goes
                            // as keys, and the keyboard is tried again later.
                            warn!("Virtual keyboard failed, typing the rest as keys: {e}");
                            self.vkeyboard = VkState::retry_later();
                            rest = std::borrow::Cow::Owned(
                                text.chars()
                                    .filter(|&c| {
                                        pocketdesk_core::input::xkb_keymap::keysym_name(c).is_some()
                                    })
                                    .skip(sent)
                                    .collect(),
                            );
                        }
                    }
                }
                // Otherwise keycodes for what a US layout types, Shift
                // included; other characters cannot be typed this way.
                let guard = state.get_mut().unwrap();
                for ch in rest.chars() {
                    if let Some((keycode, shift)) = char_to_key(ch) {
                        let key = KeyCode(keycode);
                        let shift_key = KeyCode::KEY_LEFTSHIFT.0;
                        let mut events = Vec::with_capacity(8);
                        if shift {
                            events.push(InputEvent::new(EV_KEY, shift_key, 1));
                            events.push(InputEvent::new(EV_SYN, SYN_REPORT, 0));
                        }
                        events.extend([
                            InputEvent::new(EV_KEY, key.0, 1), // press
                            InputEvent::new(EV_SYN, SYN_REPORT, 0),
                            InputEvent::new(EV_KEY, key.0, 0), // release
                            InputEvent::new(EV_SYN, SYN_REPORT, 0),
                        ]);
                        if shift {
                            events.push(InputEvent::new(EV_KEY, shift_key, 0));
                            events.push(InputEvent::new(EV_SYN, SYN_REPORT, 0));
                        }
                        emit_uinput(guard, UinputDevice::Main, &events, "text input")?;
                    } else {
                        debug!("Cannot type character with uinput: {ch:?}");
                    }
                }
                Ok(())
            }
        }
    }

    /// Handle an `InputPacket` by dispatching to the appropriate injection method.
    ///
    /// Supports mouse, keyboard, scroll, text, and gamepad events.
    /// Gamepad events are mapped to keyboard/mouse input for broad compatibility.
    pub fn handle_input_packet(&mut self, packet: &InputPacket) -> Result<()> {
        match packet {
            InputPacket::MouseMove { dx, dy } => self.move_mouse_relative(*dx as i32, *dy as i32),
            InputPacket::MouseMoveAbs { x_norm, y_norm } => {
                self.move_mouse_normalized_abs(*x_norm, *y_norm)
            }
            InputPacket::MouseClick { button, pressed } => {
                let mouse_key = match button {
                    0 => MouseKey::Left,
                    1 => MouseKey::Middle,
                    2 => MouseKey::Right,
                    3 => MouseKey::Button4,
                    4 => MouseKey::Button5,
                    _ => MouseKey::Left,
                };
                self.mouse_button(mouse_key, *pressed)
            }
            InputPacket::MouseScroll { dx, dy } => self.scroll(*dx as i32, *dy as i32),
            InputPacket::KeyEvent { key, pressed } => self.inject_keycode(*key, *pressed),
            InputPacket::Text(text) => self.text(text),
            // Control-plane request, intercepted by the streaming server before
            // the input channel; nothing to inject.
            InputPacket::RequestKeyframe => Ok(()),
            // Likewise: crop rects are consumed by the streaming pipeline, and
            // the client pre-maps absolute input into desktop space itself.
            InputPacket::WindowCrop { .. } => Ok(()),
            // View-only toggles are enforced in the streaming server's input
            // relay (which simply stops forwarding); nothing to inject.
            InputPacket::ViewOnly { .. } => Ok(()),
            // Likewise: the relay bitrate floor is server-side quality
            // control, never an input event.
            InputPacket::FullQuality { .. } => Ok(()),
            // Link-profile preset (R4 E5) is server-side bitrate control,
            // handled by the streaming server's arbiter, never injected.
            InputPacket::QualityPreset { .. } => Ok(()),
            // Mic audio is consumed by the streaming server's mic relay task
            // (R4 E2) before the input channel; never injected.
            InputPacket::Mic { .. } => Ok(()),
            // The client's own link reading and latency samples are consumed by
            // the streaming server's connection monitor and folded into the
            // session record; never injected.
            InputPacket::LinkFeedback { .. } | InputPacket::ClientSamples { .. } => Ok(()),
            InputPacket::Gamepad { axes, buttons } => {
                // Map gamepad axes/buttons to keyboard/mouse for broad compatibility.
                // Left stick -> mouse movement
                let mx = axes[0] as i32 / 256; // Scale from [-32768,32767] to [-128,127]
                let my = axes[1] as i32 / 256;
                if mx != 0 || my != 0 {
                    self.move_mouse_relative(mx, my)?;
                }

                // Gamepad buttons arrive as a state snapshot, so each mapped bit
                // is an edge: press on the rising one, release on the falling
                // one. Pressing on every packet and never releasing left the
                // DPad held down forever after any arrow tap (roadmap 2063).
                let previous = std::mem::replace(&mut self.gamepad_buttons, *buttons);
                for (action, held) in gamepad_edges(previous, *buttons) {
                    match action {
                        GamepadAction::Key(key) => self.key(key, held)?,
                        GamepadAction::LeftClick => {
                            self.mouse_button(MouseKey::Left, held)?;
                        }
                    }
                }

                // SYN_REPORT
                #[cfg(target_os = "linux")]
                if let InputBackend::Uinput(state) = &mut self.backend {
                    let guard = state.get_mut().unwrap();
                    emit_uinput(
                        guard,
                        UinputDevice::Main,
                        &[InputEvent::new(EV_SYN, SYN_REPORT, 0)],
                        "gamepad sync",
                    )?;
                }

                Ok(())
            }
        }
    }
}

/// The keyboard layout the desktop is typing into, as far as the injector can
/// tell.
///
/// uinput injects *evdev codes*, which are physical key positions: the code for
/// `A` produces whatever the active layout binds to that position. Every code
/// this file emits was chosen against a `us` layout (`char_to_keycode`, and the
/// letter/digit/punctuation rows of `KEYCODE_MAP`), so what reaches the desktop
/// is only what the phone meant when the desktop agrees. Roadmap 2065 names that
/// assumption; honouring it on any other layout is 2665.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum KeyboardLayout {
    /// The layout the injector's codes are chosen for.
    Us,
    /// A layout that was actually reported, and is not `us`.
    Other(String),
    /// Nothing reported one, so the assumption is unverified.
    Unknown,
}

/// Decide from a layout string a probe reported.
///
/// Both probes report comma-separated per-group lists, and a variant is written
/// `us(intl)`, so only the first group's base name decides: a `us` layout with
/// any variant still binds the letters and punctuation this file emits.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn classify_layout(reported: Option<&str>) -> KeyboardLayout {
    let Some(first_group) = reported.and_then(|raw| raw.split(',').next()) else {
        return KeyboardLayout::Unknown;
    };
    let base = first_group.split('(').next().unwrap_or(first_group).trim();
    if base.is_empty() {
        KeyboardLayout::Unknown
    } else if base == "us" {
        KeyboardLayout::Us
    } else {
        KeyboardLayout::Other(first_group.trim().to_string())
    }
}

/// What the desktop's keyboard layout is, best effort, cheapest and most
/// session-specific source first.
#[cfg(target_os = "linux")]
fn detect_keyboard_layout() -> KeyboardLayout {
    // What the compositor handed xkbcommon for this session.
    if let Ok(layout) = std::env::var("XKB_DEFAULT_LAYOUT") {
        let detected = classify_layout(Some(&layout));
        if detected != KeyboardLayout::Unknown {
            return detected;
        }
    }
    classify_layout(localectl_x11_layout().as_deref())
}

/// systemd's `X11 Layout:` line, or `None` on a host without `localectl` (runit,
/// a container) or without that line.
#[cfg(target_os = "linux")]
fn localectl_x11_layout() -> Option<String> {
    let output = std::process::Command::new("localectl")
        .arg("status")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_localectl_layout(&String::from_utf8_lossy(&output.stdout))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_localectl_layout(status: &str) -> Option<String> {
    status.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "X11 Layout").then(|| value.trim().to_string())
    })
}

/// Say out loud what the injector is assuming about the desktop's keyboard.
///
/// Deliberately a report, not a refusal: the phone's ordinary key events are
/// position-based too, so rejecting a non-`us` layout would take the keyboard
/// away from every non-US desktop rather than fix them.
#[cfg(target_os = "linux")]
fn report_layout_assumption() {
    match detect_keyboard_layout() {
        KeyboardLayout::Us => {
            debug!("uinput key codes are chosen for the `us` layout, which this desktop reports");
        }
        KeyboardLayout::Other(layout) => warn!(
            layout = %layout,
            "desktop keyboard layout is not `us`: injected key codes land at their `us` \
             positions, so typed characters differ from what the phone shows (roadmap 2665)"
        ),
        KeyboardLayout::Unknown => warn!(
            "could not determine the desktop keyboard layout; assuming `us`, which is what \
             every key code in the injector is chosen for"
        ),
    }
}

/// Emit events on one of the injector's uinput devices, rebuilding it once if
/// the write fails.
///
/// Both devices used to be created at startup and never revisited, so a fault
/// on the uinput file was terminal for the session's whole input path: every
/// later packet failed the same way, and the only trace was an error string
/// (roadmap 2064).
///
/// The retried event may itself be dropped, because libinput opens a newly
/// `UI_DEV_CREATE`d device asynchronously — the same reason both devices are
/// built eagerly at startup rather than on first use. What the rebuild buys is
/// recovery from the *next* event, not survival of the failing one.
#[cfg(target_os = "linux")]
fn emit_uinput(
    state: &mut UinputState,
    which: UinputDevice,
    events: &[InputEvent],
    what: &str,
) -> Result<()> {
    if which == UinputDevice::Abs && state.abs.is_none() {
        state.abs = Some(build_abs_device()?);
    }
    let first = match which {
        UinputDevice::Main => state.main.emit(events),
        UinputDevice::Abs => state
            .abs
            .as_mut()
            .expect("absent abs device was just built")
            .emit(events),
    };
    match first {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!(
                error = %e,
                device = which.label(),
                action = what,
                "uinput emit failed, rebuilding the device and retrying"
            );
            let fresh = build_uinput(which)?;
            let dev = match which {
                UinputDevice::Main => {
                    state.main = fresh;
                    &mut state.main
                }
                UinputDevice::Abs => {
                    state.abs = Some(fresh);
                    state.abs.as_mut().unwrap()
                }
            };
            dev.emit(events)
                .with_context(|| format!("uinput {what} failed again after its device was rebuilt"))
        }
    }
}

/// Which of the two uinput devices an emit is aimed at.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UinputDevice {
    /// Keyboard + relative pointer + buttons.
    Main,
    /// The `ABS_X`/`ABS_Y` pointer direct-touch motion rides.
    Abs,
}

#[cfg(target_os = "linux")]
impl UinputDevice {
    fn label(self) -> &'static str {
        match self {
            UinputDevice::Main => "Pocketdesk Virtual Input",
            UinputDevice::Abs => "Pocketdesk Virtual Abs Pointer",
        }
    }
}

#[cfg(target_os = "linux")]
fn build_uinput(which: UinputDevice) -> Result<VirtualDevice> {
    match which {
        UinputDevice::Main => build_main_device(),
        UinputDevice::Abs => build_abs_device(),
    }
}

/// Build the keyboard + relative-pointer + button device.
#[cfg(target_os = "linux")]
fn build_main_device() -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    // Every common key; the virtual keyboard declares 0..255 so any code the
    // phone can name is registrable (see inject_keycode).
    for keycode in 0..256u16 {
        keys.insert(KeyCode(keycode));
    }
    // Mouse buttons BTN_LEFT..BTN_EXTRA (272..=276): undeclared codes are
    // rejected by the kernel, so mouse clicks need explicit registration.
    for keycode in 272..=276u16 {
        keys.insert(KeyCode(keycode));
    }

    let mut rel = AttributeSet::<RelativeAxisCode>::new();
    rel.insert(RelativeAxisCode::REL_X);
    rel.insert(RelativeAxisCode::REL_Y);
    rel.insert(RelativeAxisCode::REL_WHEEL);
    rel.insert(RelativeAxisCode::REL_HWHEEL);

    #[allow(deprecated)]
    let device = VirtualDeviceBuilder::new()
        .context("Failed to create virtual device builder")?
        .with_keys(&keys)
        .context("Failed to set up virtual keys")?
        .with_relative_axes(&rel)
        .context("Failed to set up relative axes")?
        .name(b"Pocketdesk Virtual Input")
        .build()
        .context(
            "Failed to build virtual device. \
                 Ensure /dev/uinput is accessible (add user to 'uinput' group).",
        )?;
    Ok(device)
}

/// Build an `ABS_X`/`ABS_Y` virtual pointer. Coordinates arrive normalized
/// 0..=65535 from the client, so the axis ranges map the wire values 1:1 and
/// libinput scales them onto the layout. (An MT touchscreen device is the
/// other obvious choice, but Hyprland's libinput silently drops uinput
/// touchscreens on this box; the ABS pointer path is what verifiably moves
/// the real cursor.)
#[cfg(target_os = "linux")]
fn build_abs_device() -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    // Buttons registered so the device classifies as a full pointer;
    // clicks are emitted on the main device (same merged core pointer).
    for keycode in 272..=276u16 {
        keys.insert(KeyCode(keycode));
    }

    let abs_x = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_X,
        AbsInfo::new(0, 0, ABS_MAX_COORD, 0, 0, 0),
    );
    let abs_y = UinputAbsSetup::new(
        AbsoluteAxisCode::ABS_Y,
        AbsInfo::new(0, 0, ABS_MAX_COORD, 0, 0, 0),
    );

    #[allow(deprecated)]
    let device = VirtualDeviceBuilder::new()
        .context("Failed to create abs pointer builder")?
        .with_keys(&keys)
        .context("Failed to set up abs pointer buttons")?
        .with_absolute_axis(&abs_x)
        .context("Failed to set up ABS_X")?
        .with_absolute_axis(&abs_y)
        .context("Failed to set up ABS_Y")?
        .name(b"Pocketdesk Virtual Abs Pointer")
        .build()
        .context("Failed to build uinput absolute pointer device")?;

    Ok(device)
}

/// Mouse button mapping for enigo/uinput
#[derive(Debug)]
pub enum MouseKey {
    Left,
    Middle,
    Right,
    Button4,
    Button5,
}

impl MouseKey {
    fn as_enigo_button(&self) -> enigo::Button {
        match *self {
            MouseKey::Left => enigo::Button::Left,
            MouseKey::Middle => enigo::Button::Middle,
            MouseKey::Right => enigo::Button::Right,
            MouseKey::Button4 => enigo::Button::Back,
            MouseKey::Button5 => enigo::Button::Forward,
        }
    }

    #[cfg(windows)]
    fn as_sendinput_button(&self) -> crate::input_sendinput::Button {
        use crate::input_sendinput::Button;
        match *self {
            MouseKey::Left => Button::Left,
            MouseKey::Middle => Button::Middle,
            MouseKey::Right => Button::Right,
            MouseKey::Button4 => Button::Back,
            MouseKey::Button5 => Button::Forward,
        }
    }

    #[cfg(target_os = "linux")]
    fn as_evdev_key(&self) -> KeyCode {
        match *self {
            MouseKey::Left => KeyCode::BTN_LEFT,
            MouseKey::Middle => KeyCode::BTN_MIDDLE,
            MouseKey::Right => KeyCode::BTN_RIGHT,
            MouseKey::Button4 => KeyCode::BTN_SIDE, // Side button (back)
            MouseKey::Button5 => KeyCode::BTN_EXTRA, // Extra button (forward)
        }
    }
}

/// Map KDE Connect button IDs to mouse buttons
pub fn button_id_to_mouse(button: i32) -> MouseKey {
    match button {
        1 => MouseKey::Left,
        2 => MouseKey::Middle,
        3 => MouseKey::Right,
        8 => MouseKey::Button4, // Back
        9 => MouseKey::Button5, // Forward
        other => {
            debug!("Unknown mouse button {}, defaulting to left", other);
            MouseKey::Left
        }
    }
}

/// Map a Linux evdev keycode to an enigo Key.
/// Uses KEYCODE_MAP as the single source of truth.
///
/// `None` means "this backend cannot express that key". It must stay `None`:
/// inventing a `Key::Unicode` from the raw code number yields the control
/// character that code happens to be (KEY_A = 30 is U+001E), which XTEST then
/// injects as garbage, and the table's own reverse lookup turns it into
/// `KEY_UNKNOWN` on the uinput path.
fn keycode_to_enigo(code: u16) -> Option<Key> {
    KEYCODE_MAP
        .iter()
        .find(|&&(k, _)| k == code)
        .map(|&(_, key)| key)
}

/// Map an enigo Key to an evdev KeyCode for uinput backend.
/// Uses KEYCODE_MAP as the single source of truth (reverse lookup).
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
fn key_to_evdev(key: Key) -> KeyCode {
    // Check the canonical table first
    if let Some(&(code, _)) = KEYCODE_MAP.iter().find(|&(_, k)| *k == key) {
        return KeyCode(code);
    }
    // Fall back to character-based mapping for Unicode keys
    match key {
        Key::Unicode(ch) => char_to_keycode(ch)
            .map(KeyCode)
            .unwrap_or(KeyCode::KEY_UNKNOWN),
        _ => KeyCode::KEY_UNKNOWN,
    }
}

/// The key and whether Shift is held, on a US layout, for `ch`: capitals and
/// the shifted symbols on their unshifted keys (roadmap 2666), the rest as
/// [`char_to_keycode`] names them.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn char_to_key(ch: char) -> Option<(u16, bool)> {
    if ch.is_ascii_uppercase() {
        return char_to_keycode(ch.to_ascii_lowercase()).map(|code| (code, true));
    }
    let base = match ch {
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return char_to_keycode(ch).map(|code| (code, false)),
    };
    char_to_keycode(base).map(|code| (code, true))
}

/// Map a character to the Linux evdev keycode that produces it.
///
/// Reads [`KEYCODE_MAP`] so the table stays the one place a character's key is
/// named, then handles the few characters the table spells as a named key rather
/// than as themselves. Case is folded because this runs on the `text()` path,
/// which sends no Shift, so an uppercase letter shares the lowercase key.
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
fn char_to_keycode(ch: char) -> Option<u16> {
    if let Some(&(code, _)) = KEYCODE_MAP
        .iter()
        .find(|&&(_, key)| key == Key::Unicode(ch.to_ascii_lowercase()))
    {
        return Some(code);
    }
    match ch {
        ' ' => Some(KeyCode::KEY_SPACE.0),
        '\n' => Some(KeyCode::KEY_ENTER.0),
        '\t' => Some(KeyCode::KEY_TAB.0),
        '*' => Some(KeyCode::KEY_KPASTERISK.0),
        '+' => Some(KeyCode::KEY_KPPLUS.0),
        _ => None,
    }
}

/// Map key name to enigo Key
pub fn key_name_to_enigo_key(key: &str) -> Key {
    match key {
        "Enter" | "\n" | "\r" => Key::Return,
        "Escape" => Key::Escape,
        "BackSpace" => Key::Backspace,
        "Tab" => Key::Tab,
        "Delete" => Key::Delete,
        "Insert" => Key::Insert,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ArrowUp" | "Up" => Key::UpArrow,
        "ArrowDown" | "Down" => Key::DownArrow,
        "ArrowLeft" | "Left" => Key::LeftArrow,
        "ArrowRight" | "Right" => Key::RightArrow,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        "space" | "Space" => Key::Space,
        other => {
            // For single character keys, try to use them directly
            if other.len() == 1 {
                Key::Unicode(other.chars().next().unwrap_or('?'))
            } else {
                debug!("Unknown key '{}', passing through", other);
                Key::Unicode('?')
            }
        }
    }
}

#[cfg(test)]
mod shifted_text_tests {
    use super::*;

    #[test]
    fn capitals_and_shifted_symbols_hold_shift_on_their_key() {
        assert_eq!(char_to_key('a'), Some((KeyCode::KEY_A.0, false)));
        assert_eq!(char_to_key('A'), Some((KeyCode::KEY_A.0, true)));
        assert_eq!(char_to_key('!'), Some((KeyCode::KEY_1.0, true)));
        assert_eq!(char_to_key('?'), Some((KeyCode::KEY_SLASH.0, true)));
        assert_eq!(char_to_key('"'), Some((KeyCode::KEY_APOSTROPHE.0, true)));
        assert_eq!(char_to_key('|'), Some((KeyCode::KEY_BACKSLASH.0, true)));
        assert_eq!(char_to_key('/'), Some((KeyCode::KEY_SLASH.0, false)));
        assert_eq!(char_to_key(' '), Some((KeyCode::KEY_SPACE.0, false)));
        assert_eq!(char_to_key('é'), None, "not on a US keyboard");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_mapping() {
        assert!(matches!(button_id_to_mouse(1), MouseKey::Left));
        assert!(matches!(button_id_to_mouse(2), MouseKey::Middle));
        assert!(matches!(button_id_to_mouse(3), MouseKey::Right));
    }

    #[test]
    fn test_key_mapping() {
        assert!(matches!(key_name_to_enigo_key("Enter"), Key::Return));
        assert!(matches!(key_name_to_enigo_key("Escape"), Key::Escape));
        assert!(matches!(key_name_to_enigo_key("F1"), Key::F1));
        assert!(matches!(key_name_to_enigo_key("ArrowUp"), Key::UpArrow));
    }

    #[test]
    fn test_key_mapping_unicode() {
        assert!(matches!(key_name_to_enigo_key("a"), Key::Unicode('a')));
        assert!(matches!(key_name_to_enigo_key("Z"), Key::Unicode('Z')));
    }

    #[test]
    fn test_char_to_keycode_lowercase() {
        assert_eq!(char_to_keycode('a'), Some(KeyCode::KEY_A.0));
        assert_eq!(char_to_keycode('z'), Some(KeyCode::KEY_Z.0));
    }

    #[test]
    fn test_char_to_keycode_uppercase() {
        assert_eq!(char_to_keycode('A'), Some(KeyCode::KEY_A.0));
        assert_eq!(char_to_keycode('Z'), Some(KeyCode::KEY_Z.0));
    }

    #[test]
    fn test_char_to_keycode_numbers() {
        assert_eq!(char_to_keycode('0'), Some(KeyCode::KEY_0.0));
        assert_eq!(char_to_keycode('1'), Some(KeyCode::KEY_1.0));
        assert_eq!(char_to_keycode('9'), Some(KeyCode::KEY_9.0));
    }

    #[test]
    fn test_char_to_keycode_space() {
        assert_eq!(char_to_keycode(' '), Some(KeyCode::KEY_SPACE.0));
    }

    #[test]
    fn test_char_to_keycode_special() {
        assert_eq!(char_to_keycode('\n'), Some(KeyCode::KEY_ENTER.0));
        assert_eq!(char_to_keycode('\t'), Some(KeyCode::KEY_TAB.0));
    }

    #[test]
    fn test_char_to_keycode_unsupported() {
        assert_eq!(char_to_keycode('\u{00e9}'), None);
        assert_eq!(char_to_keycode('\u{4e2d}'), None);
    }

    #[test]
    fn test_keycode_to_enigo_function_keys() {
        // F1-F12 must map individually (previously all mapped to F1)
        let function_keys: [(u16, Key); 12] = [
            (59, Key::F1),
            (60, Key::F2),
            (61, Key::F3),
            (62, Key::F4),
            (63, Key::F5),
            (64, Key::F6),
            (65, Key::F7),
            (66, Key::F8),
            (67, Key::F9),
            (68, Key::F10),
            (87, Key::F11),
            (88, Key::F12),
        ];
        for (code, key) in function_keys {
            assert_eq!(keycode_to_enigo(code), Some(key), "evdev {code}");
        }
    }

    #[test]
    fn test_keycode_to_enigo_common() {
        assert_eq!(keycode_to_enigo(28), Some(Key::Return)); // KEY_ENTER
        assert_eq!(keycode_to_enigo(14), Some(Key::Backspace)); // KEY_BACKSPACE
        assert_eq!(keycode_to_enigo(57), Some(Key::Space)); // KEY_SPACE
        assert_eq!(keycode_to_enigo(15), Some(Key::Tab)); // KEY_TAB
        assert_eq!(keycode_to_enigo(1), Some(Key::Escape)); // KEY_ESC
    }

    #[test]
    fn test_keycode_to_enigo_navigation() {
        let navigation: [(u16, Key); 10] = [
            (102, Key::Home),       // KEY_HOME
            (103, Key::UpArrow),    // KEY_UP
            (104, Key::PageUp),     // KEY_PAGEUP
            (105, Key::LeftArrow),  // KEY_LEFT
            (106, Key::RightArrow), // KEY_RIGHT
            (107, Key::End),        // KEY_END
            (108, Key::DownArrow),  // KEY_DOWN
            (109, Key::PageDown),   // KEY_PAGEDOWN
            (110, Key::Insert),     // KEY_INSERT
            (111, Key::Delete),     // KEY_DELETE
        ];
        for (code, key) in navigation {
            assert_eq!(keycode_to_enigo(code), Some(key), "evdev {code}");
        }
    }

    #[test]
    fn an_unmapped_keycode_is_not_injected_as_a_control_character() {
        // A code the table does not name is refused rather than turned into the
        // control character its number happens to be: KEY_SCROLLLOCK = 70 would
        // otherwise reach XTEST as U+0046, the letter 'F'.
        assert_eq!(keycode_to_enigo(70), None, "KEY_SCROLLLOCK");
        assert_eq!(keycode_to_enigo(999), None, "beyond the evdev range");
    }

    /// The server half of the key contract in `core::input::keys`: every code the
    /// Android client can put on the wire must name a key on the X11 rung too, so
    /// a phone without /dev/uinput access degrades the same way as one with it.
    #[test]
    fn every_key_the_phone_can_send_has_a_mapping() {
        for code in pocketdesk_core::input::keys::phone_emittable_codes() {
            assert!(
                keycode_to_enigo(code).is_some(),
                "evdev code {code} is emittable but unmapped"
            );
        }
    }

    #[test]
    fn modifiers_locks_and_media_keys_map_to_their_named_keys() {
        #[cfg(not(windows))]
        let (right_alt, right_super, menu) = (Key::Other(0xFFEA), Key::Other(0xFFEC), Key::LMenu);
        #[cfg(windows)]
        let (right_alt, right_super, menu) = (Key::RMenu, Key::RWin, Key::Apps);
        let named: [(u16, Key); 19] = [
            (42, Key::LShift),          // KEY_LEFTSHIFT
            (54, Key::RShift),          // KEY_RIGHTSHIFT
            (29, Key::LControl),        // KEY_LEFTCTRL
            (97, Key::RControl),        // KEY_RIGHTCTRL
            (56, Key::Alt),             // KEY_LEFTALT
            (100, right_alt),           // KEY_RIGHTALT (XK_Alt_R on X11)
            (125, Key::Meta),           // KEY_LEFTMETA
            (126, right_super),         // KEY_RIGHTMETA (XK_Super_R on X11)
            (58, Key::CapsLock),        // KEY_CAPSLOCK
            (69, Key::Numlock),         // KEY_NUMLOCK
            (99, Key::PrintScr),        // KEY_SYSRQ, the Print key
            (139, menu),                // KEY_MENU, the application key
            (114, Key::VolumeDown),     // KEY_VOLUMEDOWN
            (115, Key::VolumeUp),       // KEY_VOLUMEUP
            (163, Key::MediaNextTrack), // KEY_NEXTSONG
            (164, Key::MediaPlayPause), // KEY_PLAYPAUSE
            (165, Key::MediaPrevTrack), // KEY_PREVIOUSSONG
            (55, Key::Multiply),        // KEY_KPASTERISK
            (78, Key::Add),             // KEY_KPPLUS
        ];
        for (code, key) in named {
            assert_eq!(keycode_to_enigo(code), Some(key), "evdev {code}");
        }
    }

    #[test]
    fn letters_digits_and_punctuation_map_to_their_own_characters() {
        let characters: [(u16, char); 13] = [
            (30, 'a'),  // KEY_A
            (44, 'z'),  // KEY_Z
            (50, 'm'),  // KEY_M
            (2, '1'),   // KEY_1
            (11, '0'),  // KEY_0
            (12, '-'),  // KEY_MINUS
            (41, '`'),  // KEY_GRAVE
            (40, '\''), // KEY_APOSTROPHE
            (51, ','),  // KEY_COMMA
            (52, '.'),  // KEY_DOT
            (53, '/'),  // KEY_SLASH
            (26, '['),  // KEY_LEFTBRACE
            (43, '\\'), // KEY_BACKSLASH
        ];
        for (code, ch) in characters {
            assert_eq!(
                keycode_to_enigo(code),
                Some(Key::Unicode(ch)),
                "evdev {code} should be '{ch}'"
            );
            assert_eq!(char_to_keycode(ch), Some(code), "'{ch}'");
        }
    }

    /// The core shim names the wire's codes on OSes without the evdev crate;
    /// a typo there would make Windows inject a different key than Linux.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_portable_key_code_names_agree_with_the_evdev_crate() {
        use std::str::FromStr;
        for &(name, code) in pocketdesk_core::input::evdev_codes::ALL_NAMED {
            let real =
                KeyCode::from_str(name).unwrap_or_else(|_| panic!("evdev has no key named {name}"));
            assert_eq!(real.0, code.0, "{name}");
        }
    }

    #[test]
    fn test_key_to_evdev_common() {
        assert_eq!(key_to_evdev(Key::Return), KeyCode::KEY_ENTER);
        assert_eq!(key_to_evdev(Key::Backspace), KeyCode::KEY_BACKSPACE);
        assert_eq!(key_to_evdev(Key::Space), KeyCode::KEY_SPACE);
        assert_eq!(key_to_evdev(Key::Escape), KeyCode::KEY_ESC);
        assert_eq!(key_to_evdev(Key::Tab), KeyCode::KEY_TAB);
    }

    #[test]
    fn test_key_to_evdev_function_keys() {
        assert_eq!(key_to_evdev(Key::F1), KeyCode::KEY_F1);
        assert_eq!(key_to_evdev(Key::F6), KeyCode::KEY_F6);
        assert_eq!(key_to_evdev(Key::F10), KeyCode::KEY_F10);
        assert_eq!(key_to_evdev(Key::F11), KeyCode::KEY_F11);
        assert_eq!(key_to_evdev(Key::F12), KeyCode::KEY_F12);
    }

    #[test]
    fn test_key_to_evdev_arrows() {
        assert_eq!(key_to_evdev(Key::UpArrow), KeyCode::KEY_UP);
        assert_eq!(key_to_evdev(Key::DownArrow), KeyCode::KEY_DOWN);
        assert_eq!(key_to_evdev(Key::LeftArrow), KeyCode::KEY_LEFT);
        assert_eq!(key_to_evdev(Key::RightArrow), KeyCode::KEY_RIGHT);
    }

    #[test]
    fn test_key_to_evdev_unicode() {
        assert_eq!(key_to_evdev(Key::Unicode('a')), KeyCode::KEY_A);
        assert_eq!(key_to_evdev(Key::Unicode('z')), KeyCode::KEY_Z);
        assert_eq!(key_to_evdev(Key::Unicode('1')), KeyCode::KEY_1);
    }

    #[test]
    fn test_keycode_roundtrip_common() {
        // Verify keycode_to_enigo and key_to_evdev are inverses
        // for all entries in KEYCODE_MAP
        for &(evdev_code, enigo_key) in KEYCODE_MAP {
            assert_eq!(
                keycode_to_enigo(evdev_code),
                Some(enigo_key),
                "keycode_to_enigo({}) should be {:?}",
                evdev_code,
                enigo_key
            );
            assert_eq!(
                key_to_evdev(enigo_key),
                KeyCode(evdev_code),
                "key_to_evdev({:?}) should be KeyCode({})",
                enigo_key,
                evdev_code
            );
        }
    }

    #[test]
    fn a_held_gamepad_button_presses_once_and_its_release_is_emitted() {
        // The packet is a state snapshot: holding Up must produce one press,
        // not a re-press per poll, and lifting it must produce the release the
        // old code never sent (roadmap 2063, the sticking DPad).
        assert_eq!(
            gamepad_edges(0, 0x0800),
            vec![(GamepadAction::Key(Key::UpArrow), true)]
        );
        assert!(gamepad_edges(0x0800, 0x0800).is_empty());
        assert_eq!(
            gamepad_edges(0x0800, 0),
            vec![(GamepadAction::Key(Key::UpArrow), false)]
        );
    }

    #[test]
    fn every_mapped_gamepad_button_that_goes_down_comes_up() {
        let mapped = GAMEPAD_BUTTONS
            .iter()
            .fold(0u16, |acc, &(bit, _)| acc | bit);
        assert_eq!(mapped, 0x7883, "the mapped bits changed shape");

        let down = gamepad_edges(0, mapped);
        let up = gamepad_edges(mapped, 0);
        assert_eq!(down.len(), GAMEPAD_BUTTONS.len());
        assert_eq!(down.len(), up.len());
        for ((action, now_held), (same_action, now_up)) in down.iter().zip(&up) {
            assert_eq!(action, same_action);
            assert!(
                *now_held && !now_up,
                "{action:?} was pressed but has no release"
            );
        }

        // Bits no desktop action is mapped to stay off the wire in both
        // directions, so a future button layout cannot inject a stray key.
        let unmapped = !mapped;
        assert!(gamepad_edges(0, unmapped).is_empty());
        assert!(gamepad_edges(unmapped, 0).is_empty());
    }

    /// Roadmap 2065: the injector's key codes are chosen for a `us` layout, so
    /// the layout it is about to type into has to be named before it is used.
    #[test]
    fn only_the_first_layout_group_has_to_be_us_for_the_codes_to_match() {
        assert_eq!(classify_layout(Some("us")), KeyboardLayout::Us);
        assert_eq!(classify_layout(Some("us(intl)")), KeyboardLayout::Us);
        // A second group is another layout's keyboard, not a different mapping
        // for the keys the phone is pressing right now.
        assert_eq!(classify_layout(Some("us,de")), KeyboardLayout::Us);
        assert_eq!(classify_layout(Some("us , fr")), KeyboardLayout::Us);
    }

    #[test]
    fn a_reported_non_us_layout_is_named_instead_of_assumed_away() {
        assert_eq!(
            classify_layout(Some("fr")),
            KeyboardLayout::Other("fr".to_string())
        );
        assert_eq!(
            classify_layout(Some("de(qwerty),us")),
            KeyboardLayout::Other("de(qwerty)".to_string())
        );
    }

    #[test]
    fn an_unreportable_layout_is_unknown_rather_than_us() {
        // The probes fail in different ways: no tool, no line, an empty value.
        assert_eq!(classify_layout(None), KeyboardLayout::Unknown);
        assert_eq!(classify_layout(Some("")), KeyboardLayout::Unknown);
        assert_eq!(classify_layout(Some("  ")), KeyboardLayout::Unknown);
        assert_eq!(classify_layout(Some(",us")), KeyboardLayout::Unknown);
        assert_eq!(parse_localectl_layout("System Locale: en_US.UTF-8\n"), None);
    }

    #[test]
    fn the_localectl_layout_line_is_the_one_that_is_read() {
        // `localectl status` on this box: the layout sits between a keymap line
        // and a model/options pair that must not be mistaken for it.
        let status = "System Locale: LANG=en_US.UTF-8\n    VC Keymap: us\n   \
                      X11 Layout: us\n    X11 Model: pc105+inet\n";
        assert_eq!(parse_localectl_layout(status).as_deref(), Some("us"));
        assert_eq!(
            classify_layout(parse_localectl_layout(status).as_deref()),
            KeyboardLayout::Us
        );
    }

    /// Roadmap 2064: a uinput device that faults at write time is rebuilt and
    /// the packet retried, instead of ending the session's input path for good.
    ///
    /// Both devices are created for real, so this needs `/dev/uinput` and
    /// self-skips without it (the injector could not run on that host either).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_faulted_uinput_device_is_rebuilt_and_the_packet_retried() {
        // Building a device needs /dev/uinput open for writing, not just
        // present: CI runners have the node but no access to it.
        if let Err(error) = std::fs::OpenOptions::new().write(true).open("/dev/uinput") {
            eprintln!("skipping uinput rebuild test: /dev/uinput not writable here ({error})");
            return;
        }
        let mut state = UinputState {
            main: build_main_device().expect("uinput is present but its device could not be built"),
            abs: None,
        };

        // A bare SYN_REPORT: the kernel accepts it on any device, and the test
        // runs against the live compositor, so it must not move a cursor or
        // type into whatever the user is doing.
        let syn = [InputEvent::new(EV_SYN, SYN_REPORT, 0)];

        emit_uinput(&mut state, UinputDevice::Abs, &syn, "absolute move")
            .expect("the absolute device should be built on first use");
        assert!(
            state.abs.is_some(),
            "the abs device stays absent after an emit aimed at it"
        );

        for which in [UinputDevice::Main, UinputDevice::Abs] {
            fault_device(&mut state, which, &syn);
            emit_uinput(&mut state, which, &syn, "test emit").unwrap_or_else(|error| {
                panic!(
                    "{:?} device did not recover from a faulted write: {error:#}",
                    which
                )
            });
        }
    }

    /// Make the next write to one of the state's devices fail the way a
    /// destroyed uinput device's does, by pointing its descriptor at a
    /// read-only `/dev/null`, and prove that it did fail.
    ///
    /// `dup2` rather than a bare `close`, because the descriptor then stays
    /// valid for this process until the device itself drops it: nothing can be
    /// handed that number in between, so neither the rebuild nor the dropped
    /// old device can end up closing somebody else's file.
    #[cfg(target_os = "linux")]
    fn fault_device(state: &mut UinputState, which: UinputDevice, events: &[InputEvent]) {
        use std::os::fd::AsRawFd;
        let device = match which {
            UinputDevice::Main => &mut state.main,
            UinputDevice::Abs => state.abs.as_mut().expect("abs device already built"),
        };
        let null = std::fs::File::open("/dev/null").expect("no readable /dev/null");
        let redirected = unsafe { libc::dup2(null.as_raw_fd(), device.as_raw_fd()) };
        assert_eq!(
            redirected,
            device.as_raw_fd(),
            "could not retarget the device descriptor"
        );
        let error = device
            .emit(events)
            .expect_err("a device writing to a read-only descriptor should fail");
        assert_eq!(
            error.raw_os_error(),
            Some(libc::EBADF),
            "the injected fault was not the one the rebuild path handles"
        );
    }
}
