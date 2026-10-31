//! This computer's screens from its Wayland compositor (BK8 live): each
//! output's logical box from `xdg_output` (or, where the compositor has
//! none, `wl_output`'s mode over its scale), for a desktop whose screens
//! neither Hyprland's IPC nor X can say: sway with `xwayland disable`, for
//! one, where xcap finds no screens and nothing could cross onto this
//! computer.
//!
//! Bounded: a connect that never waits (a full backlog is no compositor)
//! and two round trips (the globals, then the outputs) under one
//! [`DEADLINE`], each waiting on the socket with `poll`, so a compositor
//! that never answers costs at most that, never a hang. It runs off the
//! receive loop anyway (`receive::ScreensLook`).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

use super::screens::{WaylandOutput, from_wayland};
use super::wire::Screen;

/// The longest the compositor is waited for, both round trips together.
pub const DEADLINE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct State {
    /// The globals the registry named: each one's name, interface and
    /// version.
    globals: Vec<(u32, String, u32)>,
    outputs: Vec<WaylandOutput>,
    /// How many of this queue's syncs the compositor answered.
    synced: u32,
}

/// The outputs as the compositor lays them out, in logical pixels; `None`
/// with no Wayland display, no outputs, or no answer by [`DEADLINE`].
/// Blocking.
pub fn screens() -> Option<Vec<Screen>> {
    let screens = from_wayland(&outputs()?);
    (!screens.is_empty()).then_some(screens)
}

/// Every output as it said itself, in the compositor's (the registry's)
/// order, the one a capture without X counts a monitor index in, with
/// each one's name; `None` as for [`screens`]. Blocking.
pub fn outputs() -> Option<Vec<WaylandOutput>> {
    let deadline = Instant::now() + DEADLINE;
    let conn = Connection::from_socket(connect()?).ok()?;
    outputs_on(&conn, deadline)
}

/// The compositor's socket as `Connection::connect_to_env` finds it
/// (`WAYLAND_DISPLAY`, absolute or under `XDG_RUNTIME_DIR`; `wayland-0`
/// when unset), connected without waiting. `WAYLAND_SOCKET` is never
/// taken: that fd is the process's to use once.
pub(crate) fn connect() -> Option<UnixStream> {
    let name =
        PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into()));
    let path = if name.is_absolute() {
        name
    } else {
        let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
        if !dir.is_absolute() {
            return None;
        }
        dir.join(name)
    };
    connect_now(&path)
}

/// Connect to the Unix socket at `path` without waiting: a blocking
/// connect waits while the listener's backlog is full, a non-blocking one
/// fails at once (`EAGAIN`; Linux never leaves a Unix connect in
/// progress). `None` when it is not taken now. The stream stays
/// non-blocking, as the Wayland backend reads and writes it anyway.
fn connect_now(path: &Path) -> Option<UnixStream> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: an all-zero sockaddr_un is a valid one.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return None;
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (to, from) in addr.sun_path.iter_mut().zip(bytes) {
        *to = *from as libc::c_char;
    }
    // SAFETY: a plain socket call; its fd is checked before use.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return None;
    }
    // SAFETY: a new socket that nothing else owns.
    let socket = unsafe { OwnedFd::from_raw_fd(fd) };
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    // SAFETY: `addr` is a sockaddr_un holding the path and its NUL within
    // `len` bytes.
    let connected = unsafe {
        libc::connect(
            socket.as_raw_fd(),
            std::ptr::from_ref(&addr).cast(),
            len as libc::socklen_t,
        )
    };
    (connected == 0).then(|| UnixStream::from(socket))
}

/// [`outputs`] on `conn`, answered by `deadline`.
fn outputs_on(conn: &Connection, deadline: Instant) -> Option<Vec<WaylandOutput>> {
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();
    let registry = conn.display().get_registry(&qh, ());
    let mut state = State::default();
    roundtrip(conn, &mut queue, &mut state, deadline)?;
    let manager: Option<ZxdgOutputManagerV1> = state
        .globals
        .iter()
        .find(|(_, interface, _)| interface == "zxdg_output_manager_v1")
        .map(|(name, _, version)| registry.bind(*name, (*version).min(3), &qh, ()));
    let found: Vec<(u32, u32)> = state
        .globals
        .iter()
        .filter(|(_, interface, _)| interface == "wl_output")
        .map(|(name, _, version)| (*name, *version))
        .collect();
    if found.is_empty() {
        return None;
    }
    // Kept until the answers are read; the connection's end frees them.
    let mut bound = Vec::new();
    for (index, (name, version)) in found.into_iter().enumerate() {
        let output: WlOutput = registry.bind(name, version.min(4), &qh, index);
        let xdg = manager
            .as_ref()
            .map(|manager| manager.get_xdg_output(&output, &qh, index));
        state.outputs.push(WaylandOutput::default());
        bound.push((output, xdg));
    }
    roundtrip(conn, &mut queue, &mut state, deadline)?;
    Some(state.outputs)
}

/// Ask the compositor for a sync and read until it answers, by `deadline`:
/// `None` past it, or when the connection fails.
fn roundtrip(
    conn: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Option<()> {
    let wanted = state.synced + 1;
    conn.display().sync(&queue.handle(), ());
    loop {
        queue.dispatch_pending(state).ok()?;
        if state.synced >= wanted {
            return Some(());
        }
        match conn.flush() {
            Ok(()) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return None,
        }
        let Some(guard) = queue.prepare_read() else {
            // Events already queued: dispatch them first.
            continue;
        };
        // Past the deadline: no answer in time.
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())?;
        let mut fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Rounded up, so the wait never ends short of the deadline.
        let ms = i32::try_from(left.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd, for the length of the call.
        let ready = unsafe { libc::poll(&mut fd, 1, ms) };
        if ready <= 0 {
            // Timed out or interrupted: the deadline is looked at again.
            continue;
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return None,
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlCallback,
        event: wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced += 1;
        }
    }
}

impl Dispatch<WlOutput, usize> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: wl_output::Event,
        index: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                output.at = (x, y);
                output.turned = matches!(
                    transform,
                    WEnum::Value(
                        wl_output::Transform::_90
                            | wl_output::Transform::_270
                            | wl_output::Transform::Flipped90
                            | wl_output::Transform::Flipped270
                    )
                );
            }
            wl_output::Event::Mode {
                flags,
                width,
                height,
                ..
            } => {
                if let WEnum::Value(flags) = flags
                    && flags.contains(wl_output::Mode::Current)
                {
                    output.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => output.scale = factor,
            // Version 4; xdg-output's own name, where said, is the same.
            wl_output::Event::Name { name } => output.name = Some(name),
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.logical_at = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.logical_size = Some((width, height));
            }
            zxdg_output_v1::Event::Name { name } => output.name = Some(name),
            _ => {}
        }
    }
}

delegate_noop!(State: ignore ZxdgOutputManagerV1);

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// A compositor that never answers costs the deadline, never a hang;
    /// one that hangs up costs nothing.
    #[test]
    fn a_compositor_that_never_answers_is_given_up_on() {
        let (ours, _silent) = UnixStream::pair().unwrap();
        let conn = Connection::from_socket(ours).unwrap();
        let started = Instant::now();
        let wait = Duration::from_millis(200);
        assert_eq!(outputs_on(&conn, started + wait), None);
        let took = started.elapsed();
        assert!(took >= wait && took < Duration::from_secs(2), "{took:?}");

        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let conn = Connection::from_socket(ours).unwrap();
        let started = Instant::now();
        assert_eq!(outputs_on(&conn, started + Duration::from_secs(5)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Review M3: the connect never waits either. A Unix socket's connect
    /// blocks while the listener's backlog is full; this one says so at
    /// once. A path with no listener is no compositor.
    #[test]
    fn a_full_backlog_is_no_wait() {
        let dir = std::env::temp_dir().join(format!("pd-wl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wayland-test");
        assert!(connect_now(&path).is_none(), "no listener");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        // SAFETY: the listener's own fd; a backlog of none.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        let started = Instant::now();
        let mut kept = Vec::new();
        let mut refused = false;
        for _ in 0..16 {
            match connect_now(&path) {
                Some(stream) => kept.push(stream),
                None => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(refused, "the backlog never filled");
        assert!(!kept.is_empty(), "the first connect was taken");
        assert!(started.elapsed() < Duration::from_secs(1));
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live, on a Wayland desktop that is not Hyprland (owed where it
    /// runs): the outputs of the compositor `WAYLAND_DISPLAY` names, as it
    /// lays them out, and this computer's screens are those.
    #[test]
    #[ignore = "needs a Wayland compositor"]
    fn live_screens_from_the_compositor() {
        let screens = screens();
        println!("screens: {screens:?}");
        assert!(screens.as_ref().is_some_and(|screens| !screens.is_empty()));
        let current = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(super::super::screens::current());
        assert_eq!(Some(current), screens);
    }
}
