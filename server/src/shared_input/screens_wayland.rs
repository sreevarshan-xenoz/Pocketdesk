//! This computer's screens from its Wayland compositor (BK8 live): each
//! output's logical box from `xdg_output` (or, where the compositor has
//! none, `wl_output`'s mode over its scale), for a desktop whose screens
//! neither Hyprland's IPC nor X can say: sway with `xwayland disable`, for
//! one, where xcap finds no screens and nothing could cross onto this
//! computer.
//!
//! Bounded: two round trips (the globals, then the outputs) under one
//! [`DEADLINE`], each waiting on the socket with `poll`, so a compositor
//! that never answers costs at most that, never a hang.

use std::os::fd::AsRawFd;
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
    let deadline = Instant::now() + DEADLINE;
    let conn = Connection::connect_to_env().ok()?;
    screens_on(&conn, deadline)
}

/// [`screens`] on `conn`, answered by `deadline`.
fn screens_on(conn: &Connection, deadline: Instant) -> Option<Vec<Screen>> {
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
    let screens = from_wayland(&state.outputs);
    (!screens.is_empty()).then_some(screens)
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
        assert_eq!(screens_on(&conn, started + wait), None);
        let took = started.elapsed();
        assert!(took >= wait && took < Duration::from_secs(2), "{took:?}");

        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(theirs);
        let conn = Connection::from_socket(ours).unwrap();
        let started = Instant::now();
        assert_eq!(screens_on(&conn, started + Duration::from_secs(5)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
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
