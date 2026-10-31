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

use pocketdesk_core::streaming::AbsoluteSpace;
use pocketdesk_core::streaming::capture;

use crate::input_injector::{AbsoluteKind, absolute_kind};
use crate::shared_input::screens::WaylandOutput;
use crate::shared_input::screens_wayland;

/// A rectangle as `(x, y, width, height)`, in the compositor's logical
/// pixels.
type Rect = (i32, i32, u32, u32);

/// Tell the streamer where this computer's absolute moves land (at start).
pub fn install() {
    pocketdesk_core::streaming::set_absolute_space(absolute_space);
}

/// [`AbsoluteSpace`] for the session showing monitor `index`. Blocking
/// (a compositor round trip, bounded by [`screens_wayland::DEADLINE`]).
fn absolute_space(index: u32) -> AbsoluteSpace {
    match absolute_kind() {
        AbsoluteKind::Unknown => AbsoluteSpace::Unknown,
        AbsoluteKind::Layout => {
            let Some(outputs) = screens_wayland::outputs() else {
                return AbsoluteSpace::Unknown;
            };
            let space = space_for(index, &capture::monitor_names(), &outputs);
            tracing::info!(index, ?space, "Where this screen's absolute moves land");
            space
        }
    }
}

/// The space for monitor `index`, xcap's monitors being named `names`
/// (empty with no X server), over the compositor's `outputs` in its order.
/// Pure, so measured on every system.
pub fn space_for(index: u32, names: &[String], outputs: &[WaylandOutput]) -> AbsoluteSpace {
    let placed: Vec<(Option<&str>, Rect)> = outputs
        .iter()
        .filter_map(|o| {
            let s = o.screen().filter(|s| s.is_sound())?;
            Some((o.name.as_deref(), (s.x, s.y, s.width, s.height)))
        })
        .collect();
    let monitor = if names.is_empty() {
        // No X: a capture takes the compositor's outputs by position.
        outputs
            .get(index as usize)
            .and_then(WaylandOutput::screen)
            .filter(|s| s.is_sound())
            .map(|s| (s.x, s.y, s.width, s.height))
    } else {
        names
            .get(index as usize)
            .filter(|name| !name.is_empty())
            .and_then(|name| placed.iter().find(|(said, _)| *said == Some(name.as_str())))
            .map(|(_, rect)| *rect)
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

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
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
        assert_eq!(space_for(0, &[], &outputs), AbsoluteSpace::Screen);
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
            space_for(1, &[], &outputs),
            AbsoluteSpace::Layout {
                monitor: (-1280, 0, 1280, 720),
                layout: (-1280, 0, 3200, 1080),
            }
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
        assert_eq!(space_for(9, &[], &outputs), AbsoluteSpace::Unknown);
        assert_eq!(space_for(0, &[], &[]), AbsoluteSpace::Unknown);
    }
}
