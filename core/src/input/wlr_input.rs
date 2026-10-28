//! Pointer and keys on a wlroots compositor (Sway, Hyprland, river, labwc,
//! Wayfire) without `/dev/uinput` (T10f): `zwlr_virtual_pointer_v1` and a
//! `zwp_virtual_keyboard_v1` with a US layout keymap, so the evdev codes the
//! phone sends go straight through as they do on uinput. Most distributions
//! make `/dev/uinput` root-only, and with no uinput a desktop had no input
//! at all; any client of the session may use these.

use std::os::fd::{AsFd, AsRawFd};
use std::time::{Duration, Instant};

use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_pointer::{Axis, ButtonState};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use super::vkeyboard::{ConnectError, State, keymap_fd, monotonic_ms, wait, within};

/// `wl_keyboard.keymap_format.xkb_v1`.
const XKB_V1: u32 = 1;

/// One wheel notch, in the axis units compositors expect from a mouse.
pub const NOTCH: f64 = 15.0;

/// The keymap keys are typed with: the compositor's own evdev keycodes and
/// a US layout, as a uinput keyboard is read (the phone sends positions).
pub fn keys_keymap() -> &'static str {
    "xkb_keymap {\n\
     xkb_keycodes { include \"evdev+aliases(qwerty)\" };\n\
     xkb_types { include \"complete\" };\n\
     xkb_compat { include \"complete\" };\n\
     xkb_symbols { include \"pc+us+inet(evdev)\" };\n\
     };\n"
}

/// A wheel turn of `notches` (uinput's sign: positive is up, or right) as
/// the axis value and discrete steps Wayland expects (positive is down, or
/// right).
pub fn wheel(axis_vertical: bool, notches: i32) -> (f64, i32) {
    let steps = if axis_vertical { -notches } else { notches };
    (f64::from(steps) * NOTCH, steps)
}

/// One connection's virtual pointer and keyboard.
struct Link {
    conn: Connection,
    queue: EventQueue<State>,
    pointer: ZwlrVirtualPointerV1,
    keyboard: ZwpVirtualKeyboardV1,
}

impl Link {
    fn open() -> Result<Self, ConnectError> {
        let failed = |e: &dyn std::fmt::Display| ConnectError::Failed(e.to_string());
        let conn = Connection::connect_to_env().map_err(|e| failed(&e))?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| failed(&e))?;
        let qh: QueueHandle<State> = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).map_err(|e| failed(&e))?;
        let pointers: ZwlrVirtualPointerManagerV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|_| ConnectError::Unsupported)?;
        let keyboards: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ConnectError::Unsupported)?;
        let pointer = pointers.create_virtual_pointer(Some(&seat), &qh, ());
        let keyboard = keyboards.create_virtual_keyboard(&seat, &qh, ());
        let map = keys_keymap();
        let fd = keymap_fd(map).map_err(|e| failed(&e))?;
        keyboard.keymap(XKB_V1, fd.as_fd(), map.len() as u32 + 1);
        wait(&conn, &mut queue).map_err(|e| failed(&e))?;
        Ok(Self {
            conn,
            queue,
            pointer,
            keyboard,
        })
    }

    /// Send what is queued, read what the compositor sent without waiting
    /// (so its side of the socket never fills, and an error or a closed
    /// connection is seen now), and dispatch it.
    fn flush(&mut self) -> std::io::Result<()> {
        self.conn.flush().map_err(io_error)?;
        if let Some(guard) = self.queue.prepare_read() {
            let mut poll = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd, for the length given; no wait.
            let ready = unsafe { libc::poll(&mut poll, 1, 0) };
            if ready < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if ready > 0 {
                match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(io_error(e)),
                }
            }
        }
        self.queue
            .dispatch_pending(&mut State)
            .map_err(std::io::Error::other)?;
        Ok(())
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.pointer.destroy();
        self.keyboard.destroy();
        let _ = self.conn.flush();
    }
}

fn io_error(e: wayland_client::backend::WaylandError) -> std::io::Error {
    match e {
        wayland_client::backend::WaylandError::Io(e) => e,
        other => std::io::Error::other(other),
    }
}

/// The modifier state a virtual keyboard must send itself: wlroots does not
/// derive it from a virtual keyboard's keys, so without it Ctrl+C reaches
/// the window as a plain "c". Real modifier bits of the keys keymap (xkb:
/// Shift, Lock, Control, Mod1 = Alt, Mod4 = Super).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mods {
    pub depressed: u32,
    pub locked: u32,
}

const SHIFT: u32 = 1 << 0;
const LOCK: u32 = 1 << 1;
const CONTROL: u32 = 1 << 2;
const MOD1: u32 = 1 << 3;
const MOD4: u32 = 1 << 6;

impl Mods {
    /// The modifier bit the evdev key `code` holds while down, if any.
    fn held_bit(code: u16) -> Option<u32> {
        match code {
            42 | 54 => Some(SHIFT),   // KEY_LEFTSHIFT, KEY_RIGHTSHIFT
            29 | 97 => Some(CONTROL), // KEY_LEFTCTRL, KEY_RIGHTCTRL
            56 | 100 => Some(MOD1),   // KEY_LEFTALT, KEY_RIGHTALT (Alt_R on us)
            125 | 126 => Some(MOD4),  // KEY_LEFTMETA, KEY_RIGHTMETA
            _ => None,
        }
    }

    /// Follow the evdev key `code` going down or up; whether the state
    /// changed. Caps Lock toggles on its press.
    pub fn key(&mut self, code: u16, pressed: bool) -> bool {
        let before = *self;
        if let Some(bit) = Self::held_bit(code) {
            if pressed {
                self.depressed |= bit;
            } else {
                self.depressed &= !bit;
            }
        } else if code == 58 && pressed {
            // KEY_CAPSLOCK
            self.locked ^= LOCK;
        }
        *self != before
    }
}

/// The compositor's virtual pointer and keyboard. When the connection
/// breaks (the compositor restarted), it is opened again: at once, then
/// every [`WlrInput::RETRY`] while input arrives; input in between is
/// dropped, as it would be with no compositor. The compositor releases
/// what a closed connection's devices held.
pub struct WlrInput {
    link: Option<Link>,
    retry_at: Instant,
    /// The modifiers held through this connection's keyboard.
    mods: Mods,
}

impl WlrInput {
    /// How long a lost connection rests before it is opened again.
    pub const RETRY: Duration = Duration::from_secs(5);

    /// Connect to the compositor `WAYLAND_DISPLAY` names and make both, or
    /// say why not ([`ConnectError::Unsupported`] where it offers neither).
    pub fn connect() -> Result<Self, ConnectError> {
        Ok(Self {
            link: Some(within(Link::open)?),
            retry_at: Instant::now(),
            mods: Mods::default(),
        })
    }

    /// Make the requests `op` makes and send them, opening the connection
    /// again first when it was lost, and once more when it breaks now.
    fn send(&mut self, op: impl Fn(&Link)) -> std::io::Result<()> {
        if self.link.is_none() {
            if Instant::now() < self.retry_at {
                return Ok(());
            }
            self.reopen()?;
        }
        let Some(link) = self.link.as_mut() else {
            return Ok(());
        };
        op(link);
        match link.flush() {
            Ok(()) => Ok(()),
            // A full socket: the compositor is slow, not gone.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(e),
            Err(e) => {
                tracing::warn!(
                    "The compositor's virtual pointer and keyboard went away ({e}): opening them again"
                );
                self.link = None;
                self.reopen()?;
                let Some(link) = self.link.as_mut() else {
                    return Err(e);
                };
                op(link);
                link.flush()
            }
        }
    }

    fn reopen(&mut self) -> std::io::Result<()> {
        match within(Link::open) {
            Ok(link) => {
                tracing::info!("The compositor's virtual pointer and keyboard are back");
                self.link = Some(link);
                // The compositor released what the old keyboard held.
                self.mods = Mods::default();
                Ok(())
            }
            Err(e) => {
                self.retry_at = Instant::now() + Self::RETRY;
                Err(std::io::Error::other(format!(
                    "the compositor's virtual pointer and keyboard: {e:?}"
                )))
            }
        }
    }

    pub fn motion(&mut self, dx: i32, dy: i32) -> std::io::Result<()> {
        let time = monotonic_ms();
        self.send(|l| {
            l.pointer.motion(time, f64::from(dx), f64::from(dy));
            l.pointer.frame();
        })
    }

    /// Move to (`x`, `y`) of 0..=65535 over the whole output layout, as
    /// the uinput absolute pointer does. The extent is one past the largest
    /// value, as libinput scales the uinput axis, so 65535 lands on the last
    /// pixel rather than one past it.
    pub fn motion_absolute(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        let time = monotonic_ms();
        self.send(|l| {
            l.pointer
                .motion_absolute(time, x.into(), y.into(), 65_536, 65_536);
            l.pointer.frame();
        })
    }

    /// Press or release the evdev button `code` (BTN_LEFT, ...).
    pub fn button(&mut self, code: u16, pressed: bool) -> std::io::Result<()> {
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let time = monotonic_ms();
        self.send(|l| {
            l.pointer.button(time, code.into(), state);
            l.pointer.frame();
        })
    }

    /// Turn the wheel `x` notches right and `y` notches up.
    pub fn scroll(&mut self, x: i32, y: i32) -> std::io::Result<()> {
        let time = monotonic_ms();
        self.send(|l| {
            for (axis, notches, vertical) in [
                (Axis::VerticalScroll, y, true),
                (Axis::HorizontalScroll, x, false),
            ] {
                if notches != 0 {
                    let (value, steps) = wheel(vertical, notches);
                    l.pointer.axis_discrete(time, axis, value, steps);
                }
            }
            l.pointer.frame();
        })
    }

    /// Press or release the evdev key `code`, and say the modifier state
    /// when it changes.
    pub fn key(&mut self, code: u16, pressed: bool) -> std::io::Result<()> {
        let time = monotonic_ms();
        let mut mods = self.mods;
        let changed = mods.key(code, pressed);
        self.send(|l| {
            l.keyboard.key(time, code.into(), u32::from(pressed));
            if changed {
                l.keyboard.modifiers(mods.depressed, 0, mods.locked, 0);
            }
        })?;
        self.mods = mods;
        Ok(())
    }
}

impl std::fmt::Debug for WlrInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WlrInput")
            .field("connected", &self.link.is_some())
            .finish_non_exhaustive()
    }
}

wayland_client::delegate_noop!(State: ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ZwlrVirtualPointerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_notch_up_is_a_negative_wayland_step() {
        assert_eq!(wheel(true, 1), (-NOTCH, -1));
        assert_eq!(wheel(true, -2), (2.0 * NOTCH, 2));
        assert_eq!(wheel(false, 1), (NOTCH, 1));
    }

    #[test]
    fn held_modifiers_and_caps_lock_make_the_state_sent() {
        let mut mods = Mods::default();
        assert!(mods.key(29, true), "Ctrl down");
        assert!(!mods.key(46, true), "C is no modifier");
        assert_eq!(mods.depressed, CONTROL);
        assert!(mods.key(42, true));
        assert_eq!(mods.depressed, CONTROL | SHIFT);
        assert!(mods.key(29, false));
        assert!(mods.key(42, false));
        assert_eq!(mods, Mods::default());
        assert!(mods.key(58, true));
        assert!(!mods.key(58, false));
        assert_eq!(mods.locked, LOCK);
        assert!(mods.key(125, true));
        assert_eq!(mods.depressed, MOD4);
        assert!(mods.key(56, true));
        assert_eq!(mods.depressed, MOD4 | MOD1);
    }

    #[test]
    fn keys_go_through_a_us_evdev_keymap() {
        let map = keys_keymap();
        assert!(map.contains("evdev+aliases(qwerty)"));
        assert!(map.contains("pc+us+inet(evdev)"));
    }
}
