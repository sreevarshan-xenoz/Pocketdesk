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

use super::vkeyboard::{ConnectError, State, keymap_fd, monotonic_ms, wait};

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

/// The compositor's virtual pointer and keyboard. When the connection
/// breaks (the compositor restarted), it is opened again: at once, then
/// every [`WlrInput::RETRY`] while input arrives; input in between is
/// dropped, as it would be with no compositor. The compositor releases
/// what a closed connection's devices held.
pub struct WlrInput {
    link: Option<Link>,
    retry_at: Instant,
}

impl WlrInput {
    /// How long a lost connection rests before it is opened again.
    pub const RETRY: Duration = Duration::from_secs(5);

    /// Connect to the compositor `WAYLAND_DISPLAY` names and make both, or
    /// say why not ([`ConnectError::Unsupported`] where it offers neither).
    pub fn connect() -> Result<Self, ConnectError> {
        Ok(Self {
            link: Some(Link::open()?),
            retry_at: Instant::now(),
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
        match Link::open() {
            Ok(link) => {
                tracing::info!("The compositor's virtual pointer and keyboard are back");
                self.link = Some(link);
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
    /// the uinput absolute pointer does.
    pub fn motion_absolute(&mut self, x: u16, y: u16) -> std::io::Result<()> {
        let time = monotonic_ms();
        self.send(|l| {
            l.pointer
                .motion_absolute(time, x.into(), y.into(), 65_535, 65_535);
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

    /// Press or release the evdev key `code`.
    pub fn key(&mut self, code: u16, pressed: bool) -> std::io::Result<()> {
        let time = monotonic_ms();
        self.send(|l| l.keyboard.key(time, code.into(), u32::from(pressed)))
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
    fn keys_go_through_a_us_evdev_keymap() {
        let map = keys_keymap();
        assert!(map.contains("evdev+aliases(qwerty)"));
        assert!(map.contains("pc+us+inet(evdev)"));
    }
}
