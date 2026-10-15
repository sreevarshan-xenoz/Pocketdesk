//! Typing any character on a wlroots compositor (Sway, Hyprland, river,
//! labwc...) through `zwp_virtual_keyboard_v1` (roadmap F4): each run of
//! text gets a keymap with one key per character, uploaded before its keys
//! are pressed, as wtype does. Layout-independent: a French or Tamil
//! desktop types what the phone sent. GNOME and KDE do not offer the
//! protocol; [`VirtualKeyboard::connect`] returns `None` there.

use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::Instant;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use super::xkb_keymap::{keymap_for, runs};

/// `wl_keyboard.keymap_format.xkb_v1`.
const XKB_V1: u32 = 1;

struct State;

/// A virtual keyboard on the session's compositor.
pub struct VirtualKeyboard {
    _conn: Connection,
    queue: EventQueue<State>,
    keyboard: ZwpVirtualKeyboardV1,
    started: Instant,
}

impl VirtualKeyboard {
    /// Connect to the compositor `WAYLAND_DISPLAY` names and make a virtual
    /// keyboard, or `None` when there is no compositor or it does not offer
    /// virtual keyboards.
    pub fn connect() -> Option<Self> {
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh: QueueHandle<State> = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).ok()?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        queue.roundtrip(&mut State).ok()?;
        Some(Self {
            _conn: conn,
            queue,
            keyboard,
            started: Instant::now(),
        })
    }

    /// Type `text`: per run, upload its keymap, then press and release each
    /// key.
    pub fn type_text(&mut self, text: &str) -> std::io::Result<()> {
        for (keys, presses) in runs(text) {
            let Some(map) = keymap_for(&keys) else {
                continue;
            };
            let fd = keymap_fd(&map)?;
            // The size counts the terminating NUL, which the fd holds.
            self.keyboard
                .keymap(XKB_V1, fd.as_fd(), map.len() as u32 + 1);
            self.roundtrip()?;
            for index in presses {
                // evdev code = xkb keycode - 8; the keymap's keys start at 9.
                let key = index as u32 + 1;
                let time = self.started.elapsed().as_millis() as u32;
                self.keyboard.key(time, key, 1);
                self.keyboard.key(time, key, 0);
            }
            self.roundtrip()?;
        }
        Ok(())
    }

    fn roundtrip(&mut self) -> std::io::Result<()> {
        self.queue
            .roundtrip(&mut State)
            .map(|_| ())
            .map_err(std::io::Error::other)
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        self.keyboard.destroy();
        let _ = self.queue.flush();
    }
}

/// An anonymous file holding `map` and its terminating NUL.
fn keymap_fd(map: &str) -> std::io::Result<OwnedFd> {
    let name = c"pocketdesk-keymap";
    // SAFETY: memfd_create with a valid C string; the result is checked.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: raw is a fresh valid fd from memfd_create, owned from here on.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let mut file = std::fs::File::from(fd);
    file.write_all(map.as_bytes())?;
    file.write_all(&[0])?;
    Ok(OwnedFd::from(file))
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(State: ignore WlSeat);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardV1);
