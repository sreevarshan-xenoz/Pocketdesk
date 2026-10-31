//! Where a screen session's absolute moves land on a Linux computer with
//! several screens (PC11's screen windows): the streamer remaps each
//! session's moves from its picture onto its own screen's rectangle in the
//! space the input backend spans.
//!
//! uinput's absolute device and the compositor's virtual pointer (wlroots,
//! Hyprland) both span the box around every output, in the compositor's
//! logical pixels: an output's place there is `xdg_output`'s logical box,
//! which already holds a fractional scale and a turned output, and may sit
//! left of or above the origin. The session's monitor index counts in
//! xcap's order, so the output is found by xcap's name for it (Xwayland
//! names its outputs after the Wayland ones); with no X server, a capture
//! counts the compositor's outputs, so the index does too. enigo's X screen
//! is not placed, and the RemoteDesktop portal (GNOME, KDE) sends moves to
//! whichever capture runs its session, so their moves go on as before.
//!
//! The outputs are followed (review I1): a thread keeps a connection to the
//! compositor and hears outputs come, go and change, and re-reads xcap's
//! names every [`REFRESH`]. The streamer asks at each absolute move, and is
//! answered from what the thread last saw, at once. From an output's first
//! changed property until the compositor says it is done, nothing is known,
//! so moves go on over the whole desktop as before, never to where a screen
//! was.

use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::AbsoluteSpace;
use pocketdesk_core::streaming::capture;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

use crate::input_injector::{AbsoluteKind, absolute_kind};
use crate::shared_input::screens::WaylandOutput;
use crate::shared_input::screens_wayland;

/// A rectangle as `(x, y, width, height)`, in the compositor's logical
/// pixels.
type Rect = (i32, i32, u32, u32);

/// How often xcap's names are read again, and the compositor tried again
/// once its connection is gone.
const REFRESH: Duration = Duration::from_secs(2);

/// xcap's monitors, which a session's monitor index counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Names {
    /// No X server (`DISPLAY` unset): a capture counts the compositor's
    /// outputs in its order, so the index does too.
    NoX,
    /// An X server whose monitors could not be listed (or listed none): the
    /// order a capture counts in is not known, so nothing is guessed.
    Failed,
    /// Each monitor's name, in xcap's order.
    List(Vec<String>),
}

/// What the compositor said last, once every output finished saying it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub names: Names,
    pub outputs: Vec<WaylandOutput>,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// Tell the streamer where this computer's absolute moves land, and follow
/// the compositor's outputs (at start).
pub fn install() {
    pocketdesk_core::streaming::set_absolute_space(absolute_space);
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("pointer-space".into())
        .spawn(watch)
    {
        tracing::warn!(error = %e, "Could not follow the screens: clicks go over the whole desktop");
    }
}

/// [`AbsoluteSpace`] for the session showing monitor `index`, from what
/// the watching thread last saw; never blocks. The injector is asked each
/// time, so one made late counts once it is made.
fn absolute_space(index: u32) -> AbsoluteSpace {
    if absolute_kind() != AbsoluteKind::Layout {
        return AbsoluteSpace::Unknown;
    }
    let snapshot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    match snapshot.as_ref() {
        Some(s) => space_for(index, &s.names, &s.outputs),
        None => AbsoluteSpace::Unknown,
    }
}

fn publish(next: Option<Snapshot>) {
    let mut snapshot = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    if *snapshot == next {
        return;
    }
    match &next {
        None => tracing::debug!("The screens are changing: clicks go over the whole desktop"),
        Some(s) => {
            if s.names == Names::Failed {
                tracing::info!(
                    "X could not list the screens: a screen window's clicks go over the whole desktop"
                );
            } else if let Names::List(names) = &s.names
                && has_duplicates(names.iter().map(String::as_str))
            {
                tracing::info!(
                    "Two screens share a name: a screen window's clicks go over the whole desktop"
                );
            }
            tracing::debug!(outputs = s.outputs.len(), "The screens as laid out now");
        }
    }
    *snapshot = next;
}

/// xcap's names now.
fn names_now() -> Names {
    if std::env::var_os("DISPLAY").is_none() {
        return Names::NoX;
    }
    match capture::monitor_names() {
        Some(names) if !names.is_empty() => Names::List(names),
        _ => Names::Failed,
    }
}

/// The thread: follow the compositor while it answers, then try again.
fn watch() {
    loop {
        if let Some(stream) = screens_wayland::connect()
            && let Ok(conn) = Connection::from_socket(stream)
        {
            follow(&conn);
        }
        publish(None);
        std::thread::sleep(REFRESH);
    }
}

/// Hear `conn`'s outputs until it fails, publishing each settled state.
fn follow(conn: &Connection) {
    let mut queue: EventQueue<Watch> = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut state = Watch::default();
    let mut names_read: Option<Instant> = None;
    loop {
        if queue.dispatch_pending(&mut state).is_err() {
            return;
        }
        match conn.flush() {
            Ok(()) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
        if state.changed || names_read.is_none_or(|t| t.elapsed() >= REFRESH) {
            let said: Vec<(bool, WaylandOutput)> = state
                .outputs
                .iter()
                .map(|o| (o.settled, o.said.clone()))
                .collect();
            publish(settled(&said, names_now()));
            state.changed = false;
            names_read = Some(Instant::now());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(REFRESH.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: one valid pollfd, for the length of the call.
        if unsafe { libc::poll(&mut fd, 1, ms) } > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return,
            }
        }
    }
}

/// The state to publish: `None` while any output is part way through
/// saying a change (or there are none), else every output with `names`.
pub fn settled(outputs: &[(bool, WaylandOutput)], names: Names) -> Option<Snapshot> {
    if outputs.is_empty() || outputs.iter().any(|(settled, _)| !settled) {
        return None;
    }
    Some(Snapshot {
        names,
        outputs: outputs.iter().map(|(_, o)| o.clone()).collect(),
    })
}

/// One output as the watching thread hears it.
struct Live {
    global: u32,
    output: WlOutput,
    xdg: Option<ZxdgOutputV1>,
    said: WaylandOutput,
    /// Its last change is complete (its `done`); an output that cannot say
    /// `done` (version 1) always is.
    settled: bool,
}

#[derive(Default)]
struct Watch {
    manager: Option<ZxdgOutputManagerV1>,
    outputs: Vec<Live>,
    /// Something was heard since the last publish.
    changed: bool,
}

impl Watch {
    fn get(&mut self, global: u32) -> Option<&mut Live> {
        self.changed = true;
        self.outputs.iter_mut().find(|o| o.global == global)
    }
}

impl Dispatch<WlRegistry, ()> for Watch {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_output" => {
                    let output: WlOutput = registry.bind(name, version.min(4), qh, name);
                    let xdg = state
                        .manager
                        .as_ref()
                        .map(|m| m.get_xdg_output(&output, qh, name));
                    state.outputs.push(Live {
                        global: name,
                        output,
                        xdg,
                        said: WaylandOutput::default(),
                        settled: version < 2,
                    });
                    state.changed = true;
                }
                "zxdg_output_manager_v1" if state.manager.is_none() => {
                    let manager: ZxdgOutputManagerV1 = registry.bind(name, version.min(3), qh, ());
                    for live in &mut state.outputs {
                        live.xdg = Some(manager.get_xdg_output(&live.output, qh, live.global));
                        if live.output.version() >= 2 {
                            live.settled = false;
                        }
                    }
                    state.manager = Some(manager);
                    state.changed = true;
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(at) = state.outputs.iter().position(|o| o.global == name) {
                    let gone = state.outputs.remove(at);
                    if let Some(xdg) = gone.xdg {
                        xdg.destroy();
                    }
                    if gone.output.version() >= 3 {
                        gone.output.release();
                    }
                    state.changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for Watch {
    fn event(
        state: &mut Self,
        proxy: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let can_say_done = proxy.version() >= 2;
        let Some(live) = state.get(*global) else {
            return;
        };
        let out = &mut live.said;
        match event {
            wl_output::Event::Geometry {
                x, y, transform, ..
            } => {
                out.at = (x, y);
                out.turned = matches!(
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
                    out.mode = (width, height);
                }
            }
            wl_output::Event::Scale { factor } => out.scale = factor,
            wl_output::Event::Name { name } => out.name = Some(name),
            wl_output::Event::Done => {
                live.settled = true;
                return;
            }
            _ => return,
        }
        live.settled = !can_say_done;
    }
}

impl Dispatch<ZxdgOutputV1, u32> for Watch {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(live) = state.get(*global) else {
            return;
        };
        let can_say_done = live.output.version() >= 2;
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => live.said.logical_at = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                live.said.logical_size = Some((width, height));
            }
            zxdg_output_v1::Event::Name { name } => live.said.name = Some(name),
            // Before version 3, xdg-output says its own done.
            zxdg_output_v1::Event::Done => {
                live.settled = true;
                return;
            }
            _ => return,
        }
        live.settled = !can_say_done;
    }
}

delegate_noop!(Watch: ignore ZxdgOutputManagerV1);

fn has_duplicates<'a>(mut names: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = std::collections::HashSet::new();
    names.any(|name| !seen.insert(name))
}

/// The space for monitor `index`, xcap's monitors being `names`, over the
/// compositor's `outputs` in its order. Pure, so measured on every system.
/// Only where a capture provably counts the same way is an output taken by
/// position (no X); a name that is missing, empty or shared by two is not
/// guessed, nor an X server whose monitors could not be listed.
pub fn space_for(index: u32, names: &Names, outputs: &[WaylandOutput]) -> AbsoluteSpace {
    let placed: Vec<(Option<&str>, Rect)> = outputs
        .iter()
        .filter_map(|o| {
            let s = o.screen().filter(|s| s.is_sound())?;
            Some((o.name.as_deref(), (s.x, s.y, s.width, s.height)))
        })
        .collect();
    let monitor = match names {
        Names::NoX => outputs
            .get(index as usize)
            .and_then(WaylandOutput::screen)
            .filter(|s| s.is_sound())
            .map(|s| (s.x, s.y, s.width, s.height)),
        Names::Failed => None,
        Names::List(names) => {
            let wanted = names
                .get(index as usize)
                .filter(|name| !name.is_empty())
                .filter(|_| !has_duplicates(names.iter().map(String::as_str)));
            wanted.and_then(|name| {
                let mut same = placed
                    .iter()
                    .filter(|(said, _)| *said == Some(name.as_str()));
                match (same.next(), same.next()) {
                    (Some((_, rect)), None) => Some(*rect),
                    _ => None,
                }
            })
        }
    };
    let (Some(monitor), Some(layout)) = (monitor, bounds(placed.iter().map(|(_, r)| *r))) else {
        return AbsoluteSpace::Unknown;
    };
    if monitor == layout {
        AbsoluteSpace::Screen
    } else {
        AbsoluteSpace::Layout { monitor, layout }
    }
}

/// The box around `rects`; `None` with none.
fn bounds(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    let mut edges: Option<(i64, i64, i64, i64)> = None;
    for (x, y, w, h) in rects {
        let (x, y) = (i64::from(x), i64::from(y));
        let (r, b) = (x + i64::from(w), y + i64::from(h));
        edges = Some(match edges {
            None => (x, y, r, b),
            Some((l, t, rr, bb)) => (l.min(x), t.min(y), rr.max(r), bb.max(b)),
        });
    }
    let (l, t, r, b) = edges?;
    Some((
        i32::try_from(l).ok()?,
        i32::try_from(t).ok()?,
        u32::try_from(r - l).ok()?,
        u32::try_from(b - t).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, at: (i32, i32), size: (i32, i32)) -> WaylandOutput {
        WaylandOutput {
            at,
            mode: size,
            scale: 1,
            logical_at: Some(at),
            logical_size: Some(size),
            name: Some(name.to_string()),
            ..Default::default()
        }
    }

    fn names(names: &[&str]) -> Names {
        Names::List(names.iter().map(|n| n.to_string()).collect())
    }

    #[test]
    fn the_screen_is_found_by_xcaps_name_in_the_compositors_layout() {
        // xcap lists them the other way round from the compositor.
        let outputs = [
            output("HEADLESS-1", (0, 0), (1920, 1080)),
            output("HEADLESS-2", (1920, 0), (1280, 1024)),
        ];
        let xcap = names(&["HEADLESS-2", "HEADLESS-1"]);
        assert_eq!(
            space_for(0, &xcap, &outputs),
            AbsoluteSpace::Layout {
                monitor: (1920, 0, 1280, 1024),
                layout: (0, 0, 3200, 1080),
            }
        );
        assert_eq!(
            space_for(1, &xcap, &outputs),
            AbsoluteSpace::Layout {
                monitor: (0, 0, 1920, 1080),
                layout: (0, 0, 3200, 1080),
            }
        );
    }

    /// Fractional scaling: xdg-output's logical box, not the mode, places
    /// the output (a 2560x1600 panel at 1.5 is 1707x1067); a turned one
    /// with no xdg-output is the mode over its scale, on its side; one left
    /// of the origin keeps its negative place.
    #[test]
    fn logical_boxes_scales_and_negative_origins_place_the_screen() {
        let fractional = WaylandOutput {
            at: (0, 0),
            mode: (2560, 1600),
            scale: 2,
            logical_at: Some((0, 0)),
            logical_size: Some((1707, 1067)),
            name: Some("eDP-1".into()),
            ..Default::default()
        };
        let turned_left = WaylandOutput {
            at: (-1080, -200),
            mode: (3840, 2160),
            scale: 2,
            turned: true,
            name: Some("DP-1".into()),
            ..Default::default()
        };
        let outputs = [fractional, turned_left];
        let layout = (-1080, -200, 2787, 1920);
        let xcap = names(&["eDP-1", "DP-1"]);
        assert_eq!(
            space_for(0, &xcap, &outputs),
            AbsoluteSpace::Layout {
                monitor: (0, 0, 1707, 1067),
                layout
            }
        );
        assert_eq!(
            space_for(1, &xcap, &outputs),
            AbsoluteSpace::Layout {
                monitor: (-1080, -200, 1080, 1920),
                layout
            }
        );
    }

    /// One screen is the whole layout: nothing to remap, as before.
    #[test]
    fn a_single_screen_is_its_own_space() {
        let outputs = [output("eDP-1", (0, 0), (1536, 960))];
        assert_eq!(
            space_for(0, &names(&["eDP-1"]), &outputs),
            AbsoluteSpace::Screen
        );
        assert_eq!(space_for(0, &Names::NoX, &outputs), AbsoluteSpace::Screen);
    }

    /// No X server (sway with `xwayland disable`): the index counts the
    /// compositor's outputs, as its capture does.
    #[test]
    fn with_no_x_the_index_counts_the_compositors_outputs() {
        let outputs = [
            output("HEADLESS-1", (0, 0), (1920, 1080)),
            output("HEADLESS-2", (-1280, 0), (1280, 720)),
        ];
        assert_eq!(
            space_for(1, &Names::NoX, &outputs),
            AbsoluteSpace::Layout {
                monitor: (-1280, 0, 1280, 720),
                layout: (-1280, 0, 3200, 1080),
            }
        );
    }

    /// Review I2: an X server whose monitors could not be listed, or listed
    /// none, is not guessed by the compositor's order (the two orders can
    /// differ, as above).
    #[test]
    fn an_empty_or_failed_xcap_list_is_not_guessed_by_order() {
        let outputs = [
            output("HEADLESS-1", (0, 0), (1920, 1080)),
            output("HEADLESS-2", (1920, 0), (1280, 1024)),
        ];
        for index in [0, 1] {
            assert_eq!(
                space_for(index, &Names::Failed, &outputs),
                AbsoluteSpace::Unknown
            );
            assert_eq!(
                space_for(index, &names(&[]), &outputs),
                AbsoluteSpace::Unknown
            );
        }
    }

    /// Review I2: two monitors of one name, in xcap's list or among the
    /// compositor's outputs, are not told apart by guessing.
    #[test]
    fn duplicate_names_are_not_guessed() {
        let outputs = [
            output("HEADLESS-1", (0, 0), (1920, 1080)),
            output("HEADLESS-2", (1920, 0), (1280, 1024)),
        ];
        let twice = names(&["HEADLESS-1", "HEADLESS-1"]);
        assert_eq!(space_for(0, &twice, &outputs), AbsoluteSpace::Unknown);
        assert_eq!(space_for(1, &twice, &outputs), AbsoluteSpace::Unknown);
        let same = [
            output("DP-1", (0, 0), (1920, 1080)),
            output("DP-1", (1920, 0), (1280, 1024)),
        ];
        assert_eq!(
            space_for(0, &names(&["DP-1", "eDP-1"]), &same),
            AbsoluteSpace::Unknown
        );
    }

    /// A name the compositor does not have, an index past the list, or no
    /// outputs at all: not known, so moves go on as they came.
    #[test]
    fn what_cannot_be_placed_is_unknown() {
        let outputs = [
            output("HEADLESS-1", (0, 0), (1920, 1080)),
            output("HEADLESS-2", (1920, 0), (1920, 1080)),
        ];
        assert_eq!(
            space_for(0, &names(&["XWAYLAND0"]), &outputs),
            AbsoluteSpace::Unknown
        );
        assert_eq!(
            space_for(5, &names(&["HEADLESS-1"]), &outputs),
            AbsoluteSpace::Unknown
        );
        assert_eq!(space_for(9, &Names::NoX, &outputs), AbsoluteSpace::Unknown);
        assert_eq!(space_for(0, &Names::NoX, &[]), AbsoluteSpace::Unknown);
    }

    /// Review I1: while any output is part way through a change, nothing is
    /// published (moves go over the whole desktop); once each says done,
    /// the new places are.
    #[test]
    fn nothing_is_published_while_an_output_is_changing() {
        let one = output("HEADLESS-1", (0, 0), (1920, 1080));
        let two = output("HEADLESS-2", (1920, 0), (1280, 1024));
        assert_eq!(
            settled(&[(true, one.clone()), (false, two.clone())], Names::NoX),
            None
        );
        assert_eq!(settled(&[], Names::NoX), None, "no outputs");
        let snapshot =
            settled(&[(true, one.clone()), (true, two.clone())], Names::NoX).expect("both settled");
        assert_eq!(snapshot.outputs, vec![one, two]);
        assert_eq!(snapshot.names, Names::NoX);
    }
}
