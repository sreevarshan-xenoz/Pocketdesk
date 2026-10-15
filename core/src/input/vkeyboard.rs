//! Typing any character on a wlroots compositor (Sway, Hyprland, river,
//! labwc...) through `zwp_virtual_keyboard_v1` (roadmap F4): each run of
//! text gets a keymap with one key per character, uploaded before its keys
//! are pressed, as wtype does. Layout-independent: a French or Tamil
//! desktop types what the phone sent. GNOME and KDE do not offer the
//! protocol; [`VirtualKeyboard::connect`] says so with
//! [`ConnectError::Unsupported`].

use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use super::xkb_keymap::{keymap_for, runs};

/// `wl_keyboard.keymap_format.xkb_v1`.
const XKB_V1: u32 = 1;

/// How long the compositor has to answer: a frozen one must not hold the
/// input task, which also carries the pointer and keys.
const DEADLINE: Duration = Duration::from_millis(500);

/// Keys sent between two waits for the compositor, so a long text never
/// fills the socket.
const KEYS_PER_WAIT: usize = 200;

/// The event state every Wayland input object here shares.
pub(super) struct State;

/// Why there is no virtual keyboard.
#[derive(Debug)]
pub enum ConnectError {
    /// The compositor does not offer virtual keyboards (GNOME, KDE): no
    /// use trying again.
    Unsupported,
    /// No compositor reached, or it did not answer: worth trying again.
    Failed(String),
}

/// A virtual keyboard on the session's compositor.
pub struct VirtualKeyboard {
    conn: Connection,
    queue: EventQueue<State>,
    keyboard: ZwpVirtualKeyboardV1,
    /// The characters of the keymap uploaded last, so the same set is not
    /// uploaded again (every focused app recompiles a keymap).
    uploaded: Vec<char>,
}

impl VirtualKeyboard {
    /// Connect to the compositor `WAYLAND_DISPLAY` names and make a virtual
    /// keyboard.
    pub fn connect() -> Result<Self, ConnectError> {
        let failed = |e: &dyn std::fmt::Display| ConnectError::Failed(e.to_string());
        let conn = Connection::connect_to_env().map_err(|e| failed(&e))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| failed(&e))?;
        let qh: QueueHandle<State> = queue.handle();
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).map_err(|e| failed(&e))?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ConnectError::Unsupported)?;
        let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
        let mut vk = Self {
            conn,
            queue,
            keyboard,
            uploaded: Vec::new(),
        };
        vk.wait().map_err(|e| failed(&e))?;
        Ok(vk)
    }

    /// Type `text`. On an error, how many of `text`'s typable characters
    /// (those [`super::xkb_keymap::keysym_name`] names) were already sent,
    /// with the error: a key that went out is never sent again.
    pub fn type_text(&mut self, text: &str) -> Result<(), (usize, std::io::Error)> {
        let mut sent = 0;
        for (keys, presses) in runs(text) {
            let Some(map) = keymap_for(&keys) else {
                continue;
            };
            if keys != self.uploaded {
                let fd = keymap_fd(&map).map_err(|e| (sent, e))?;
                // The size counts the terminating NUL, which the fd holds.
                self.keyboard
                    .keymap(XKB_V1, fd.as_fd(), map.len() as u32 + 1);
                self.wait().map_err(|e| (sent, e))?;
                self.uploaded = keys;
            }
            for chunk in presses.chunks(KEYS_PER_WAIT) {
                for &index in chunk {
                    // evdev code = xkb keycode - 8; the keymap's keys start at 9.
                    let key = index as u32 + 1;
                    let time = monotonic_ms();
                    self.keyboard.key(time, key, 1);
                    self.keyboard.key(time.wrapping_add(1), key, 0);
                }
                // Sent: counted even if the wait fails, so it is not retyped.
                sent += chunk.len();
                self.wait().map_err(|e| (sent, e))?;
            }
        }
        Ok(())
    }

    fn wait(&mut self) -> std::io::Result<()> {
        wait(&self.conn, &mut self.queue)
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        self.keyboard.destroy();
        let _ = self.queue.flush();
    }
}

/// Wait up to [`DEADLINE`] for the compositor to have handled all that was
/// sent on `conn` (a `wl_display.sync`).
pub(super) fn wait(conn: &Connection, queue: &mut EventQueue<State>) -> std::io::Result<()> {
    let done = Arc::new(AtomicBool::new(false));
    let _callback = conn.display().sync(&queue.handle(), Arc::clone(&done));
    let until = Instant::now() + DEADLINE;
    while !done.load(Ordering::Acquire) {
        queue.flush().map_err(std::io::Error::other)?;
        if let Some(guard) = queue.prepare_read() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the compositor did not answer",
                ));
            }
            let mut poll = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd, for the length given.
            let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis() as libc::c_int) };
            if ready < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if ready > 0 {
                guard.read().map_err(std::io::Error::other)?;
            }
        }
        queue
            .dispatch_pending(&mut State)
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

/// Milliseconds on the monotonic clock, the base compositors use for input.
pub(super) fn monotonic_ms() -> u32 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    (now.tv_sec as u64 * 1000 + now.tv_nsec as u64 / 1_000_000) as u32
}

/// An anonymous file holding `map` and its terminating NUL.
pub(super) fn keymap_fd(map: &str) -> std::io::Result<OwnedFd> {
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

impl Dispatch<WlCallback, Arc<AtomicBool>> for State {
    fn event(
        _: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        done: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            done.store(true, Ordering::Release);
        }
    }
}

wayland_client::delegate_noop!(State: ignore WlSeat);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(State: ZwpVirtualKeyboardV1);
