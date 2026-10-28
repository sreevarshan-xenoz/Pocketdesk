//! This computer's screens, as another computer sharing its keyboard and
//! mouse needs them: in logical pixels, in this computer's own desktop
//! space, whose bounding box is what a position of 0..=65535 spans (as
//! `InputPacket::MouseMoveAbs`'s does).

use super::wire::Screen;
use crate::hyprland::HyprMonitor;

/// This computer's screens now: Windows' monitors, in logical pixels
/// ([`Desktop`]); Hyprland's, at their logical size; else the one rectangle
/// the capture's layout spans. Empty when none can be read.
pub async fn current() -> Vec<Screen> {
    #[cfg(windows)]
    {
        Desktop::arrange(&crate::windows_desktop::monitors_scaled()).screens()
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        if let Ok(ipc) = crate::hyprland::HyprlandIpc::from_env()
            && let Ok(monitors) = ipc.monitors().await
        {
            return logical(&monitors);
        }
        let layout = tokio::task::spawn_blocking(pocketdesk_core::streaming::capture::layout_rect)
            .await
            .ok()
            .flatten();
        layout
            .map(|(x, y, width, height)| Screen {
                x,
                y,
                width,
                height,
            })
            .into_iter()
            .filter(Screen::is_sound)
            .collect()
    }
}

/// `monitors` as screens of their logical size: the mode divided by the
/// scale (rounded), on its side for an odd transform. Hyprland already
/// places them in logical pixels (Windows does not: [`Desktop`]). One
/// without a size is left out.
pub fn logical(monitors: &[HyprMonitor]) -> Vec<Screen> {
    monitors
        .iter()
        .filter_map(|m| {
            let scale = if m.scale.is_finite() && m.scale > 0.0 {
                m.scale
            } else {
                1.0
            };
            let size = |px: i32| {
                let logical = (f64::from(px) / scale).round();
                // A float past u32 saturates; one of zero is dropped below.
                (logical >= 1.0).then_some(logical as u32)
            };
            let (mut width, mut height) = (size(m.width)?, size(m.height)?);
            if m.transform % 2 != 0 {
                std::mem::swap(&mut width, &mut height);
            }
            Some(Screen {
                x: m.x,
                y: m.y,
                width,
                height,
            })
        })
        .filter(Screen::is_sound)
        .collect()
}

/// Windows' monitors in both kinds of pixel. Windows places its monitors in
/// physical pixels (what the hook's points and `SetCursorPos` use under
/// per-monitor DPI awareness); the layout's units are logical, as Linux's
/// are, so a 150% monitor of 2560x1440 is 1707x960 (its size divided by its
/// scale, rounded half away from zero).
///
/// Dividing the positions too would pull monitors of different scales apart
/// or over each other, so they are placed by what they touch: the monitor at
/// the origin (else the first) at its position divided by its scale, then
/// each monitor touching one placed already, against the same edge of it,
/// its offset along that edge divided by the placed one's scale. One that
/// touches none is placed at its position divided by its own scale.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desktop {
    monitors: Vec<Mapped>,
}

/// One monitor, both ways.
#[derive(Debug, Clone, PartialEq)]
struct Mapped {
    physical: Rect,
    scale: f64,
    logical: Rect,
}

/// A rectangle: its corner and size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x.saturating_add(self.w)
    }

    fn bottom(&self) -> i32 {
        self.y.saturating_add(self.h)
    }

    fn contains(&self, (x, y): (i32, i32)) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
    }

    /// How far `p` is from this rectangle (0 inside), squared.
    fn distance(&self, (x, y): (i32, i32)) -> i64 {
        let gap = |v: i32, lo: i32, hi: i32| {
            let v = i64::from(v);
            (i64::from(lo) - v).max(v - (i64::from(hi) - 1)).max(0)
        };
        let (dx, dy) = (gap(x, self.x, self.right()), gap(y, self.y, self.bottom()));
        dx * dx + dy * dy
    }
}

/// `v` divided by `scale`, rounded half away from zero.
fn div(v: i32, scale: f64) -> i32 {
    (f64::from(v) / scale).round() as i32
}

impl Desktop {
    /// `monitors` (physical, each with its scale) placed in logical pixels.
    /// One without a size is left out; a scale that is none is 1.
    pub fn arrange(monitors: &[HyprMonitor]) -> Desktop {
        let monitors: Vec<(Rect, f64)> = monitors
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| {
                let scale = if m.scale.is_finite() && m.scale > 0.0 {
                    m.scale
                } else {
                    1.0
                };
                let r = Rect {
                    x: m.x,
                    y: m.y,
                    w: m.width,
                    h: m.height,
                };
                (r, scale)
            })
            .collect();
        let size = |(r, scale): (Rect, f64)| (div(r.w, scale).max(1), div(r.h, scale).max(1));
        let mut placed: Vec<Option<Rect>> = vec![None; monitors.len()];
        let first = monitors
            .iter()
            .position(|(r, _)| r.contains((0, 0)))
            .unwrap_or(0);
        let mut next = (!monitors.is_empty()).then_some(first);
        while let Some(i) = next {
            let (r, scale) = monitors[i];
            let (w, h) = size(monitors[i]);
            // Against a placed monitor it touches, else where it is.
            let touching = placed.iter().enumerate().find_map(|(j, l)| {
                let l = (*l)?;
                let (p, p_scale) = monitors[j];
                let across = r.y < p.bottom() && p.y < r.bottom();
                let along = r.x < p.right() && p.x < r.right();
                let dy = l.y + div(r.y - p.y, p_scale);
                let dx = l.x + div(r.x - p.x, p_scale);
                if across && r.x == p.right() {
                    Some((l.right(), dy))
                } else if across && r.right() == p.x {
                    Some((l.x - w, dy))
                } else if along && r.y == p.bottom() {
                    Some((dx, l.bottom()))
                } else if along && r.bottom() == p.y {
                    Some((dx, l.y - h))
                } else {
                    None
                }
            });
            let (x, y) = touching.unwrap_or((div(r.x, scale), div(r.y, scale)));
            placed[i] = Some(Rect { x, y, w, h });
            // The next to place: one touching a placed monitor, else the
            // first left.
            let unplaced = || (0..monitors.len()).filter(|&k| placed[k].is_none());
            next = unplaced()
                .find(|&k| {
                    let r = monitors[k].0;
                    placed.iter().enumerate().any(|(j, l)| {
                        l.is_some() && {
                            let p = monitors[j].0;
                            let across = r.y < p.bottom() && p.y < r.bottom();
                            let along = r.x < p.right() && p.x < r.right();
                            (across && (r.x == p.right() || r.right() == p.x))
                                || (along && (r.y == p.bottom() || r.bottom() == p.y))
                        }
                    })
                })
                .or_else(|| unplaced().next());
        }
        Desktop {
            monitors: monitors
                .into_iter()
                .zip(placed)
                .filter_map(|((physical, scale), logical)| {
                    Some(Mapped {
                        physical,
                        scale,
                        logical: logical?,
                    })
                })
                .collect(),
        }
    }

    /// The monitors as logical screens, in the order they were given.
    pub fn screens(&self) -> Vec<Screen> {
        self.monitors
            .iter()
            .map(|m| Screen {
                x: m.logical.x,
                y: m.logical.y,
                width: m.logical.w.unsigned_abs(),
                height: m.logical.h.unsigned_abs(),
            })
            .filter(Screen::is_sound)
            .collect()
    }

    /// The monitor `p` is on in `rect`'s pixels, else the nearest.
    fn nearest(&self, p: (i32, i32), rect: impl Fn(&Mapped) -> Rect) -> Option<&Mapped> {
        self.monitors.iter().min_by_key(|m| rect(m).distance(p))
    }

    /// A physical point as a logical one: on the monitor it is on, else
    /// carried on from the nearest (a point pushed past an edge stays past
    /// it). Rounded down, so a monitor's last pixel is its last. With no
    /// monitors, as it is.
    pub fn to_logical(&self, p: (i32, i32)) -> (i32, i32) {
        let Some(m) = self.nearest(p, |m| m.physical) else {
            return p;
        };
        let along = |v: i32, from: i32, to: i32| {
            to.saturating_add((f64::from(v.saturating_sub(from)) / m.scale).floor() as i32)
        };
        (
            along(p.0, m.physical.x, m.logical.x),
            along(p.1, m.physical.y, m.logical.y),
        )
    }

    /// A logical point as a physical one (rounded), on the monitor it is
    /// on, else from the nearest. With no monitors, as it is.
    pub fn to_physical(&self, p: (i32, i32)) -> (i32, i32) {
        let Some(m) = self.nearest(p, |m| m.logical) else {
            return p;
        };
        let along = |v: i32, from: i32, to: i32| {
            to.saturating_add((f64::from(v.saturating_sub(from)) * m.scale).round() as i32)
        };
        (
            along(p.0, m.logical.x, m.physical.x),
            along(p.1, m.logical.y, m.physical.y),
        )
    }

    /// The scale of the monitor a physical point is on (or the nearest).
    pub fn scale_at(&self, p: (i32, i32)) -> f64 {
        self.nearest(p, |m| m.physical).map_or(1.0, |m| m.scale)
    }

    /// The box around all the monitors in `rect`'s pixels.
    fn bounds(&self, rect: impl Fn(&Mapped) -> Rect) -> Option<Rect> {
        self.monitors.iter().map(rect).reduce(|a, b| {
            let (x, y) = (a.x.min(b.x), a.y.min(b.y));
            Rect {
                x,
                y,
                w: a.right().max(b.right()) - x,
                h: a.bottom().max(b.bottom()) - y,
            }
        })
    }

    /// A position of 0..=65535 across the logical screens' bounding box as
    /// one across the physical monitors' (what Windows' absolute input
    /// spans), each box's last pixel being 65535. With no monitors, as it is.
    pub fn physical_norm(&self, x: u16, y: u16) -> (u16, u16) {
        let (Some(logical), Some(physical)) =
            (self.bounds(|m| m.logical), self.bounds(|m| m.physical))
        else {
            return (x, y);
        };
        let point = |n: u16, lo: i32, len: i32| {
            let last = i64::from(len - 1).max(0);
            lo + ((i64::from(n) * last + 32767) / 65535) as i32
        };
        let norm = |v: i32, lo: i32, len: i32| {
            let last = i64::from(len - 1);
            if last <= 0 {
                return 0;
            }
            let offset = (i64::from(v) - i64::from(lo)).clamp(0, last);
            ((offset * 65535 + last / 2) / last) as u16
        };
        let p = self.to_physical((
            point(x, logical.x, logical.w),
            point(y, logical.y, logical.h),
        ));
        (
            norm(p.0, physical.x, physical.w),
            norm(p.1, physical.y, physical.h),
        )
    }
}

/// A mouse's movement in physical pixels, as logical ones on a monitor of
/// some scale, the fractions carried to the next movement so a slow mouse
/// on a scaled monitor still moves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Motion {
    rest: (f64, f64),
}

impl Motion {
    /// `d` physical pixels at `scale` (one that is none is 1), as logical
    /// ones.
    pub fn step(&mut self, d: (i32, i32), scale: f64) -> (i32, i32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let one = |d: i32, rest: &mut f64| {
            let total = *rest + f64::from(d) / scale;
            // A hair toward the next whole pixel, so thirds that add up to
            // one are one.
            let whole = (total + total.signum() * 1e-9).trunc();
            *rest = total - whole;
            whole as i32
        };
        let (mut rx, mut ry) = self.rest;
        let out = (one(d.0, &mut rx), one(d.1, &mut ry));
        self.rest = (rx, ry);
        out
    }

    /// Forget the fractions (the mouse is parked anew).
    pub fn reset(&mut self) {
        self.rest = (0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `j/monitors` reply as Hyprland writes it (trimmed): a 2560x1600
    /// laptop panel at 125%, and a 1080p monitor beside it turned on its
    /// side.
    const MONITORS: &str = r#"[
        {"id": 0, "name": "eDP-1", "description": "panel", "make": "x",
         "width": 2560, "height": 1600, "refreshRate": 165.0,
         "x": 0, "y": 0, "activeWorkspace": {"id": 1, "name": "1"},
         "scale": 1.25, "transform": 0, "focused": true},
        {"id": 1, "name": "HDMI-A-1", "width": 1920, "height": 1080,
         "x": 2048, "y": -200, "scale": 1.00, "transform": 1}
    ]"#;

    #[test]
    fn hyprland_scale_gives_logical_size() {
        let monitors: Vec<HyprMonitor> = serde_json::from_str(MONITORS).unwrap();
        assert_eq!(monitors[0].scale, 1.25);
        assert_eq!(
            logical(&monitors),
            vec![
                Screen {
                    x: 0,
                    y: 0,
                    width: 2048,
                    height: 1280,
                },
                Screen {
                    x: 2048,
                    y: -200,
                    width: 1080,
                    height: 1920,
                },
            ]
        );
    }

    #[test]
    fn a_reply_without_a_scale_is_at_one_and_odd_ones_are_dropped() {
        let monitors: Vec<HyprMonitor> = serde_json::from_str(
            r#"[{"name": "A", "width": 1920, "height": 1080, "x": 0, "y": 0},
                {"name": "B", "width": 0, "height": 1080, "x": 0, "y": 0},
                {"name": "C", "width": 1920, "height": 1080, "x": 0, "y": 0, "scale": 0.0},
                {"name": "D", "width": 100, "height": 100, "x": 2147483600, "y": 0}]"#,
        )
        .unwrap();
        assert_eq!(monitors[0].scale, 1.0);
        let screens = logical(&monitors);
        // A has its mode; B has no width; C's scale is no scale; D's far
        // edge is past i32.
        assert_eq!(screens.len(), 2);
        assert!(screens.iter().all(|s| s.width == 1920 && s.height == 1080));
    }

    fn monitor(x: i32, y: i32, width: i32, height: i32, scale: f64) -> HyprMonitor {
        HyprMonitor {
            name: String::new(),
            x,
            y,
            width,
            height,
            scale,
            transform: 0,
        }
    }

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn windows_scale_gives_logical_pixels() {
        // 2560x1440 at 150%: 1706.67 rounds to 1707, 960 is exact.
        let desktop = Desktop::arrange(&[monitor(0, 0, 2560, 1440, 1.5)]);
        assert_eq!(desktop.screens(), vec![screen(0, 0, 1707, 960)]);
    }

    #[test]
    fn mixed_scales_stay_side_by_side() {
        // The owner's desk: a 1080p monitor at 100% and, to its right, the
        // laptop's 1920x1200 panel at 125% (Windows puts it at 1920).
        let desk = Desktop::arrange(&[
            monitor(0, 0, 1920, 1080, 1.0),
            monitor(1920, 0, 1920, 1200, 1.25),
        ]);
        assert_eq!(
            desk.screens(),
            vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)]
        );
        // The laptop first and the monitor left of it, 100 px higher: its
        // offset is in the laptop's pixels, 80 logical.
        let left = Desktop::arrange(&[
            monitor(0, 0, 1920, 1200, 1.25),
            monitor(-1920, -100, 1920, 1080, 1.0),
        ]);
        assert_eq!(
            left.screens(),
            vec![screen(0, 0, 1536, 960), screen(-1920, -80, 1920, 1080)]
        );
        // One above and one below a 150% monitor.
        let stacked = Desktop::arrange(&[
            monitor(0, 0, 2560, 1440, 1.5),
            monitor(300, -1080, 1920, 1080, 1.0),
            monitor(0, 1440, 1280, 720, 1.0),
        ]);
        assert_eq!(
            stacked.screens(),
            vec![
                screen(0, 0, 1707, 960),
                screen(200, -1080, 1920, 1080),
                screen(0, 960, 1280, 720),
            ]
        );
        // One touching nothing keeps its own position, scaled.
        let apart = Desktop::arrange(&[
            monitor(0, 0, 1920, 1080, 1.0),
            monitor(5000, 0, 1500, 1000, 1.5),
        ]);
        assert_eq!(apart.screens()[1], screen(3333, 0, 1000, 667));
    }

    #[test]
    fn points_cross_between_physical_and_logical() {
        let desk = Desktop::arrange(&[
            monitor(0, 0, 1920, 1080, 1.0),
            monitor(1920, 0, 1920, 1200, 1.25),
        ]);
        // The laptop's last pixel is its last logical one.
        assert_eq!(desk.to_logical((3839, 1199)), (3455, 959));
        assert_eq!(desk.to_logical((1920, 0)), (1920, 0));
        assert_eq!(desk.to_logical((500, 600)), (500, 600));
        // A point pushed past the right edge stays past it.
        assert_eq!(desk.to_logical((3845, 600)), (3460, 480));
        // And back, rounded.
        assert_eq!(desk.to_physical((3455, 959)), (3839, 1199));
        assert_eq!(desk.to_physical((2688, 480)), (2880, 600));
        assert_eq!(desk.to_logical(desk.to_physical((2688, 480))), (2688, 480));
        assert_eq!(desk.to_physical((960, 540)), (960, 540));
        assert_eq!(desk.scale_at((2000, 10)), 1.25);
        assert_eq!(desk.scale_at((10, 10)), 1.0);
        // The corners of the whole box map to the corners.
        assert_eq!(desk.physical_norm(0, 0), (0, 0));
        assert_eq!(desk.physical_norm(65535, 65535), (65535, 65535));
        // The laptop's centre, logical, lands on its centre, physical.
        let n = |v: i64, last: i64| ((v * 65535 + last / 2) / last) as u16;
        let logical = (n(2688, 3455), n(480, 1079));
        assert_eq!(
            desk.physical_norm(logical.0, logical.1),
            (n(2880, 3839), n(600, 1199))
        );
        // No monitors: left alone.
        assert_eq!(Desktop::default().to_logical((5, 5)), (5, 5));
        assert_eq!(Desktop::default().physical_norm(7, 9), (7, 9));
    }

    #[test]
    fn slow_motion_on_a_scaled_monitor_still_moves() {
        let mut motion = Motion::default();
        // Three single pixels at 150% are two logical ones, not none.
        let steps: Vec<_> = (0..3).map(|_| motion.step((1, -1), 1.5)).collect();
        assert_eq!(steps, vec![(0, 0), (1, -1), (1, -1)]);
        // At 100% a pixel is a pixel.
        assert_eq!(Motion::default().step((7, -3), 1.0), (7, -3));
        // No scale is taken as 1.
        assert_eq!(Motion::default().step((4, 4), 0.0), (4, 4));
    }
}
