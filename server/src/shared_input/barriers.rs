//! Linux driving through the input-capture portal (BK8): this computer's
//! edges that lead to another computer, as the portal's pointer barriers,
//! and the breaker that turns an edge that keeps flipping into a wall.
//!
//! The portal says this computer's screens as *zones* (logical pixels, in
//! the space its own [`Layout`] entry has them); a barrier is a line on the
//! outer edge of one zone that the compositor watches, and when the pointer
//! pushes past it the portal says which barrier it was and where
//! (`Activated`). [`barriers`] walks each zone's four edges in the layout:
//! every run of pixels that crosses onto the same computer (as
//! [`Layout::crossing`] has it, so the driver and the barriers agree)
//! becomes one barrier. An edge that meets another of this computer's own
//! zones, or nothing, or a computer that may not be crossed onto now, has
//! none. [`Barrier::push`] turns the portal's word back into the
//! [`Captured::Edge`] the driver crosses on.
//!
//! xdg-desktop-portal-hyprland #435: on a fractionally scaled output the
//! pointer the compositor lets go of lands past the barrier again, and the
//! capture flips on and off many times a second. [`FlipBreaker`] counts the
//! activations of each edge: a third within a second makes it a wall for
//! half a minute, or until the screens change.
//!
//! Pure: no I/O, tested on every system.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use super::drive::Captured;
use super::layout::Layout;

/// A barrier as the portal takes it, and what is past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Barrier {
    /// Its id with the portal: 1 and up, new at each arming.
    pub id: u32,
    /// The line, x1, y1, x2, y2, inclusive: on the zone's first row or
    /// column for its top and left edges, one past its last for its bottom
    /// and right (the portal's outer boundary).
    pub line: Line,
    /// The first and last pixel of the zone along that edge (its own last
    /// row or column), where a push from it crosses.
    pub from: ((i32, i32), (i32, i32)),
    /// The way across: (1, 0) right, (-1, 0) left, (0, 1) down, (0, -1) up.
    pub way: Way,
    /// The computer past it.
    pub to: String,
}

/// A barrier's line as the portal takes it: x1, y1, x2, y2.
pub type Line = (i32, i32, i32, i32);

/// A way across an edge: (1, 0) right, (-1, 0) left, (0, 1) down, (0, -1) up.
pub type Way = (i32, i32);

/// An edge as the breaker remembers it across armings (ids change).
pub type EdgeKey = (Line, Way);

impl Barrier {
    pub fn key(&self) -> EdgeKey {
        (self.line, self.way)
    }

    /// The push the portal said, at `at` (where the pointer was, in the
    /// zones' space, if it said): the driver's [`Captured::Edge`], from the
    /// pixel of this edge nearest to it.
    pub fn push(&self, at: Option<(f64, f64)>) -> Captured {
        let ((x0, y0), (x1, y1)) = self.from;
        let (x, y) = match at {
            Some((ax, ay)) if ax.is_finite() && ay.is_finite() => {
                let near = |v: f64, lo: i32, hi: i32| {
                    (v.floor() as i64).clamp(lo.into(), hi.into()) as i32
                };
                (near(ax, x0, x1), near(ay, y0, y1))
            }
            // No position: the middle of the edge.
            _ => (x0 + (x1 - x0) / 2, y0 + (y1 - y0) / 2),
        };
        Captured::Edge {
            x,
            y,
            dx: self.way.0,
            dy: self.way.1,
        }
    }

    /// How far `at` is from this barrier's line (squared), for an
    /// activation that did not say which barrier it was.
    fn distance(&self, (ax, ay): (f64, f64)) -> f64 {
        let (x1, y1, x2, y2) = self.line;
        let dx = (ax - ax.clamp(f64::from(x1), f64::from(x2))).abs();
        let dy = (ay - ay.clamp(f64::from(y1), f64::from(y2))).abs();
        dx * dx + dy * dy
    }
}

/// The barriers of this computer (`this`) in `layout`, whose screens there
/// are the portal's zones: one for each run of edge pixels that leads onto
/// the same other computer that `may_go` allows (reachable now), none on an
/// edge in `walls`. Their ids count from 1 (the glue numbers them anew,
/// rising across armings).
pub fn barriers(
    layout: &Layout,
    this: &str,
    may_go: impl Fn(&str) -> bool,
    walls: &HashSet<EdgeKey>,
) -> Vec<Barrier> {
    let Some(own) = layout.computers.iter().find(|c| c.device_id == this) else {
        return Vec::new();
    };
    let mut out: Vec<Barrier> = Vec::new();
    for s in own.screens.iter().filter(|s| s.width > 0 && s.height > 0) {
        let (w, h) = (
            i32::try_from(s.width).unwrap_or(i32::MAX),
            i32::try_from(s.height).unwrap_or(i32::MAX),
        );
        let (left, top) = (s.x, s.y);
        let (right, bottom) = (s.x.saturating_add(w - 1), s.y.saturating_add(h - 1));
        // Each edge: the way across, and the span of its pixels.
        for (way, lo, hi) in [
            ((-1, 0), top, bottom),
            ((1, 0), top, bottom),
            ((0, -1), left, right),
            ((0, 1), left, right),
        ] {
            // The `v`th pixel along this edge, on the zone's own last row or
            // column that way.
            let pixel = |v: i32| match way {
                (-1, 0) => (left, v),
                (1, 0) => (right, v),
                (0, -1) => (v, top),
                _ => (v, bottom),
            };
            let mut runs: Vec<(String, i32, i32)> = Vec::new();
            for v in lo..=hi {
                let Some(to) = layout
                    .crossing(this, pixel(v), way.0, way.1)
                    .map(|c| c.to)
                    .filter(|to| to != this && may_go(to))
                else {
                    continue;
                };
                match runs.last_mut() {
                    Some((now, _, end)) if *now == to && *end == v - 1 => *end = v,
                    _ => runs.push((to, v, v)),
                }
            }
            for (to, a, b) in runs {
                let (first, last) = (pixel(a), pixel(b));
                // The bottom and right lines lie one past the zone.
                let past = |(x, y): (i32, i32)| match way {
                    (1, 0) => (x.saturating_add(1), y),
                    (0, 1) => (x, y.saturating_add(1)),
                    _ => (x, y),
                };
                let (p, q) = (past(first), past(last));
                let barrier = Barrier {
                    id: 0,
                    line: (p.0, p.1, q.0, q.1),
                    from: (first, last),
                    way,
                    to,
                };
                if !walls.contains(&barrier.key()) {
                    out.push(barrier);
                }
            }
        }
    }
    for (n, b) in out.iter_mut().enumerate() {
        b.id = u32::try_from(n + 1).unwrap_or(u32::MAX);
    }
    out
}

/// The barrier the portal's activation names (`id`), or with none (the
/// portal could not tell, several at one place) the one nearest to where
/// the pointer was (`at`).
pub fn activated(
    barriers: &[Barrier],
    id: Option<u32>,
    at: Option<(f64, f64)>,
) -> Option<&Barrier> {
    if let Some(id) = id {
        return barriers.iter().find(|b| b.id == id);
    }
    let at = at.filter(|(x, y)| x.is_finite() && y.is_finite())?;
    barriers
        .iter()
        .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
}

/// Activations of one edge within this long that make it a wall.
pub const FLIP_WINDOW: Duration = Duration::from_secs(1);

/// How many activations of one edge within [`FLIP_WINDOW`] make it a wall.
pub const FLIPS: usize = 3;

/// How long a wall stands before the edge is tried again.
pub const WALL_FOR: Duration = Duration::from_secs(30);

/// What the breaker makes of an activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flip {
    /// Cross as usual.
    Cross,
    /// The edge flips: it is a wall from now on. `first`: the first edge
    /// walled this run, to be said (and logged) once.
    Wall { first: bool },
}

/// Counts each edge's activations, and walls one that flips (xdph #435).
#[derive(Debug, Default)]
pub struct FlipBreaker {
    seen: HashMap<EdgeKey, VecDeque<Instant>>,
    walls: HashSet<EdgeKey>,
    /// When each wall went up.
    walled_at: HashMap<EdgeKey, Instant>,
    told: bool,
}

impl FlipBreaker {
    /// The edge `key` was activated at `now`.
    pub fn activated(&mut self, key: EdgeKey, now: Instant) -> Flip {
        if self.walls.contains(&key) {
            return Flip::Wall { first: false };
        }
        let times = self.seen.entry(key).or_default();
        times.retain(|at| now.saturating_duration_since(*at) < FLIP_WINDOW);
        times.push_back(now);
        if times.len() < FLIPS {
            return Flip::Cross;
        }
        self.seen.remove(&key);
        self.walls.insert(key);
        self.walled_at.insert(key, now);
        Flip::Wall {
            first: !std::mem::replace(&mut self.told, true),
        }
    }

    /// The edges walled now, left out of the barriers.
    pub fn walls(&self) -> &HashSet<EdgeKey> {
        &self.walls
    }

    /// Whether an edge has been walled this run.
    pub fn any(&self) -> bool {
        self.told
    }

    /// The time is `now`: walls up for [`WALL_FOR`] lift (someone
    /// jiggling at an edge can trip one too); one that still flips is
    /// walled again.
    pub fn expire(&mut self, now: Instant) {
        let lifted: Vec<EdgeKey> = self
            .walled_at
            .iter()
            .filter(|(_, at)| now.saturating_duration_since(**at) >= WALL_FOR)
            .map(|(key, _)| *key)
            .collect();
        for key in lifted {
            self.walls.remove(&key);
            self.walled_at.remove(&key);
        }
    }

    /// The screens changed: every edge may be tried again.
    pub fn clear(&mut self) {
        self.seen.clear();
        self.walls.clear();
        self.walled_at.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared_input::layout::Placed;
    use crate::shared_input::wire::Screen;

    fn screen(x: i32, y: i32, width: u32, height: u32) -> Screen {
        Screen {
            x,
            y,
            width,
            height,
        }
    }

    fn placed(id: &str, origin: (i32, i32), screens: Vec<Screen>) -> Placed {
        Placed {
            device_id: id.to_string(),
            name: id.to_string(),
            origin,
            screens,
        }
    }

    fn all(_: &str) -> bool {
        true
    }

    fn lines(barriers: &[Barrier]) -> Vec<(u32, Line, Way, &str)> {
        barriers
            .iter()
            .map(|b| (b.id, b.line, b.way, b.to.as_str()))
            .collect()
    }

    /// The owner's desk seen from the Linux laptop ("L", one 1600x1000
    /// zone): Windows ("W", 3456 wide) to its left.
    fn desk() -> Layout {
        Layout {
            computers: vec![
                placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)]),
                placed(
                    "W",
                    (-3456, 0),
                    vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)],
                ),
            ],
        }
    }

    #[test]
    fn an_edge_facing_another_computer_is_one_barrier() {
        let b = barriers(&desk(), "L", all, &HashSet::new());
        // W's laptop screen is 960 high: the left edge leads to it there,
        // and below that it is a wall.
        assert_eq!(lines(&b), vec![(1, (0, 0, 0, 959), (-1, 0), "W")]);
        assert_eq!(b[0].from, ((0, 0), (0, 959)));
    }

    #[test]
    fn the_right_and_bottom_lines_are_one_past_the_zone() {
        let layout = Layout {
            computers: vec![
                placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)]),
                placed("W", (1600, 0), vec![screen(0, 0, 1920, 1080)]),
                // BK5's phone, below: placed as a computer is.
                placed("P", (400, 1000), vec![screen(0, 0, 400, 800)]),
            ],
        };
        let b = barriers(&layout, "L", all, &HashSet::new());
        assert_eq!(
            lines(&b),
            vec![
                (1, (1600, 0, 1600, 999), (1, 0), "W"),
                (2, (400, 1000, 799, 1000), (0, 1), "P"),
            ]
        );
        assert_eq!(b[0].from, ((1599, 0), (1599, 999)));
        assert_eq!(b[1].from, ((400, 999), (799, 999)));
    }

    #[test]
    fn edges_between_own_zones_and_non_contiguous_zones() {
        // Two zones side by side (the second lower and shorter), a third
        // apart from them on its own, and W right of the second.
        let layout = Layout {
            computers: vec![
                placed(
                    "L",
                    (0, 0),
                    vec![
                        screen(0, 0, 1920, 1080),
                        screen(1920, 200, 1280, 720),
                        screen(5000, 0, 800, 600),
                    ],
                ),
                placed("W", (3200, 0), vec![screen(0, 0, 1000, 2000)]),
                placed("M", (5800, 0), vec![screen(0, 0, 1000, 600)]),
            ],
        };
        let b = barriers(&layout, "L", all, &HashSet::new());
        // Nothing where the first two zones meet; the second's right edge
        // leads to W, and the far zone's right edge to M (its left edge
        // touches nothing: W ends at 4200).
        assert_eq!(
            lines(&b),
            vec![
                (1, (3200, 200, 3200, 919), (1, 0), "W"),
                (2, (5800, 0, 5800, 599), (1, 0), "M"),
            ]
        );
    }

    #[test]
    fn one_edge_onto_two_computers_is_two_barriers() {
        let layout = Layout {
            computers: vec![
                placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)]),
                placed("W", (1600, 0), vec![screen(0, 0, 1920, 400)]),
                placed("M", (1600, 400), vec![screen(0, 0, 1280, 800)]),
            ],
        };
        let b = barriers(&layout, "L", all, &HashSet::new());
        assert_eq!(
            lines(&b),
            vec![
                (1, (1600, 0, 1600, 399), (1, 0), "W"),
                (2, (1600, 400, 1600, 999), (1, 0), "M"),
            ]
        );
        // One not reachable now has none; the other keeps its own.
        let b = barriers(&layout, "L", |to| to != "W", &HashSet::new());
        assert_eq!(lines(&b), vec![(1, (1600, 400, 1600, 999), (1, 0), "M")]);
        // A walled edge is left out, and the ids count from 1 again.
        let walls: HashSet<EdgeKey> = [((1600, 0, 1600, 399), (1, 0))].into();
        let b = barriers(&layout, "L", all, &walls);
        assert_eq!(lines(&b), vec![(1, (1600, 400, 1600, 999), (1, 0), "M")]);
    }

    #[test]
    fn no_other_computer_no_barrier() {
        let layout = Layout {
            computers: vec![placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)])],
        };
        assert!(barriers(&layout, "L", all, &HashSet::new()).is_empty());
        assert!(barriers(&layout, "X", all, &HashSet::new()).is_empty());
    }

    #[test]
    fn an_activation_is_a_push_from_the_edge_where_it_was() {
        let b = barriers(&desk(), "L", all, &HashSet::new());
        let push = |id, at| activated(&b, id, at).map(|b| b.push(at));
        let edge = |x, y| {
            Some(Captured::Edge {
                x,
                y,
                dx: -1,
                dy: 0,
            })
        };
        // Where the portal said, held onto the edge's pixels.
        assert_eq!(push(Some(1), Some((-0.4, 300.7))), edge(0, 300));
        assert_eq!(push(Some(1), Some((0.0, 2000.0))), edge(0, 959));
        // No position: the middle of the edge.
        assert_eq!(push(Some(1), None), edge(0, 479));
        assert_eq!(push(Some(1), Some((f64::NAN, 1.0))), edge(0, 479));
        // An id it does not know is no barrier of this arming.
        assert_eq!(push(Some(9), Some((0.0, 1.0))), None);
        // No id: the nearest barrier to where it was.
        let layout = Layout {
            computers: vec![
                placed("L", (0, 0), vec![screen(0, 0, 1600, 1000)]),
                placed("W", (1600, 0), vec![screen(0, 0, 1920, 1080)]),
                placed("P", (-400, 0), vec![screen(0, 0, 400, 800)]),
            ],
        };
        let b = barriers(&layout, "L", all, &HashSet::new());
        assert_eq!(
            activated(&b, None, Some((1601.0, 10.0))).map(|b| b.to.as_str()),
            Some("W")
        );
        assert_eq!(
            activated(&b, None, Some((-1.0, 10.0))).map(|b| b.to.as_str()),
            Some("P")
        );
        assert_eq!(activated(&b, None, None), None);
    }

    #[test]
    fn an_edge_that_flips_three_times_in_a_second_is_a_wall() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let right: EdgeKey = ((1600, 0, 1600, 999), (1, 0));
        let left: EdgeKey = ((0, 0, 0, 999), (-1, 0));
        let mut breaker = FlipBreaker::default();
        // Crossings a person makes, seconds apart, never wall it.
        for s in 0..10 {
            assert_eq!(breaker.activated(right, at(s * 1500)), Flip::Cross);
        }
        // Three quick ones over a full second: still fine.
        let mut breaker = FlipBreaker::default();
        assert_eq!(breaker.activated(right, at(0)), Flip::Cross);
        assert_eq!(breaker.activated(right, at(400)), Flip::Cross);
        assert_eq!(breaker.activated(right, at(1000)), Flip::Cross);
        // The third within a second of the first: a wall, said once.
        assert_eq!(
            breaker.activated(right, at(1100)),
            Flip::Wall { first: true }
        );
        assert!(breaker.walls().contains(&right));
        assert_eq!(
            breaker.activated(right, at(5000)),
            Flip::Wall { first: false }
        );
        // Another edge counts on its own, and its wall is not said again.
        assert_eq!(breaker.activated(left, at(5000)), Flip::Cross);
        assert_eq!(breaker.activated(left, at(5100)), Flip::Cross);
        assert_eq!(
            breaker.activated(left, at(5200)),
            Flip::Wall { first: false }
        );
        assert!(breaker.any());
        // New screens: every edge is tried again.
        breaker.clear();
        assert!(breaker.walls().is_empty());
        assert_eq!(breaker.activated(right, at(6000)), Flip::Cross);
    }

    /// BK8 review M1: a wall is not for the rest of the run: someone
    /// jiggling at an edge can trip it too, so it lifts after a while and
    /// the edge is tried again (walled again if it still flips).
    #[test]
    fn a_wall_lifts_after_a_while() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let right: EdgeKey = ((1600, 0, 1600, 999), (1, 0));
        let mut breaker = FlipBreaker::default();
        for ms in [0, 200, 400] {
            breaker.activated(right, at(ms));
        }
        assert!(breaker.walls().contains(&right));
        breaker.expire(at(400) + WALL_FOR - Duration::from_millis(1));
        assert!(breaker.walls().contains(&right), "not yet");
        breaker.expire(at(400) + WALL_FOR);
        assert!(breaker.walls().is_empty(), "lifted");
        // Tried again: crossings count anew.
        let later = at(400) + WALL_FOR;
        assert_eq!(breaker.activated(right, later), Flip::Cross);
    }
}
