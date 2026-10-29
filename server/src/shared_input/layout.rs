//! Where each computer's screens sit relative to the others, and where the
//! cursor crosses from one computer to the next.
//!
//! Three spaces, all in logical pixels. A computer's *own desktop space* is
//! the one its [`Screen`]s are in. Its [`Placed::origin`] puts that space into
//! the shared *layout space*, where the screens of different computers meet
//! edge to edge. A [`Crossing`] is in the receiving computer's own space, the
//! one it will move its cursor in.

use std::collections::HashMap;

use super::wire::Screen;

/// One computer in the layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub device_id: String,
    pub name: String,
    /// Where this computer's own desktop space starts in the layout space.
    pub origin: (i32, i32),
    pub screens: Vec<Screen>,
}

/// All the computers and where they sit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub computers: Vec<Placed>,
}

/// The cursor leaving one computer for another.
#[derive(Debug, Clone, PartialEq)]
pub struct Crossing {
    /// The computer the cursor goes to.
    pub to: String,
    /// Where it lands, in that computer's own desktop space.
    pub at: (i32, i32),
}

impl Layout {
    /// The computers in a row: `this` where it is, the others to its right in
    /// the order given, each touching the one before, with their tops level
    /// with `this` (their origins are set here, whatever they were).
    pub fn default_for(this: Placed, others: Vec<Placed>) -> Layout {
        let (mut right, top) = match this.bounds() {
            Some(b) => {
                let b = b.shifted(this.origin);
                (b.x1, b.y0)
            }
            None => (i64::from(this.origin.0), i64::from(this.origin.1)),
        };
        let mut computers = vec![this];
        for mut other in others {
            let (left, other_top, width) = match other.bounds() {
                Some(b) => (b.x0, b.y0, b.x1 - b.x0),
                None => (0, 0, 0),
            };
            other.origin = ((right - left) as i32, (top - other_top) as i32);
            right += width;
            computers.push(other);
        }
        Layout { computers }
    }

    fn placed(&self, device_id: &str) -> Option<&Placed> {
        self.computers.iter().find(|c| c.device_id == device_id)
    }

    /// Where the cursor goes if it is at `at` (in `from`'s own desktop space)
    /// and moves by `(dx, dy)`, if that takes it onto another computer.
    ///
    /// It does only from the last pixel of a screen in the direction it is
    /// heading (the larger of the two movements; sideways if they tie), and
    /// only if none of `from`'s other screens carries on past that edge. The
    /// other computer's screen must touch that edge (to within a pixel) and
    /// face the cursor, whose place along the edge must be within the part
    /// where the two overlap; anywhere else the edge is a wall.
    ///
    /// The cursor lands on the first row or column of the screen it enters,
    /// at the same place along the edge in layout space, as Windows' Display
    /// settings arrange screens: an offset between the two screens shifts
    /// where it lands, and with logical pixels different DPIs already line up.
    pub fn crossing(&self, from: &str, at: (i32, i32), dx: i32, dy: i32) -> Option<Crossing> {
        let dir = Dir::of(dx, dy)?;
        let source = self.placed(from)?;
        let pos = (i64::from(at.0), i64::from(at.1));
        let screen = source
            .screens
            .iter()
            .map(Rect::of)
            .find(|r| r.contains(pos))?;
        let (step_x, step_y) = dir.step();
        let next = (pos.0 + step_x, pos.1 + step_y);
        if pos != screen.last_pixel(dir, pos)
            || source.screens.iter().any(|s| Rect::of(s).contains(next))
        {
            return None;
        }

        let edge = screen.shifted(source.origin);
        let line = edge.far(dir);
        let (lo, hi) = edge.span(dir);
        let along = if dir.horizontal() {
            pos.1 + i64::from(source.origin.1)
        } else {
            pos.0 + i64::from(source.origin.0)
        };

        for other in self.computers.iter().filter(|c| c.device_id != from) {
            for target in other.screens.iter().map(Rect::of) {
                let placed = target.shifted(other.origin);
                let (t_lo, t_hi) = placed.span(dir);
                if (placed.near(dir) - line).abs() > 1
                    || along < lo.max(t_lo)
                    || along >= hi.min(t_hi)
                {
                    continue;
                }
                let entry = target.entry(dir);
                let (x, y) = if dir.horizontal() {
                    (entry, along - i64::from(other.origin.1))
                } else {
                    (along - i64::from(other.origin.0), entry)
                };
                return Some(Crossing {
                    to: other.device_id.clone(),
                    at: (x as i32, y as i32),
                });
            }
        }
        None
    }

    /// `at` as `InputPacket::MouseMoveAbs` says a position: 0..=65535 across
    /// the box around all of the computer's screens, the box's last pixel
    /// being 65535. A point outside the box is held at its edge. A computer
    /// that is not here, or has no screens, gives `(0, 0)`.
    pub fn normalized(&self, device_id: &str, at: (i32, i32)) -> (u16, u16) {
        let Some(b) = self.placed(device_id).and_then(Placed::bounds) else {
            return (0, 0);
        };
        (
            normalize(i64::from(at.0), b.x0, b.x1),
            normalize(i64::from(at.1), b.y0, b.y1),
        )
    }

    /// `at` if it is on one of the computer's screens, else the nearest point
    /// that is, for a cursor left on a screen that has since gone. A computer
    /// that is not here, or has no screens, leaves it as it is.
    pub fn clamp(&self, device_id: &str, at: (i32, i32)) -> (i32, i32) {
        let Some(placed) = self.placed(device_id) else {
            return at;
        };
        let pos = (i64::from(at.0), i64::from(at.1));
        placed
            .screens
            .iter()
            .map(Rect::of)
            .filter(|r| !r.is_empty())
            .map(|r| {
                let near = (pos.0.clamp(r.x0, r.x1 - 1), pos.1.clamp(r.y0, r.y1 - 1));
                let (dx, dy) = (i128::from(near.0 - pos.0), i128::from(near.1 - pos.1));
                (dx * dx + dy * dy, near)
            })
            .min_by_key(|(distance, _)| *distance)
            .map_or(at, |(_, near)| (near.0 as i32, near.1 as i32))
    }
}

impl Layout {
    /// Name each computer as the owner paired it (`paired`: its pairing
    /// link's name, or the trust store's rename, else what it called itself
    /// there), whatever it calls itself now (review M1: a computer's own
    /// name, said in its hello or `Name`, could be anything, "this
    /// computer" too). One not paired by a name keeps its own, unless that
    /// is no name once cleaned for showing (zero-width only, review M2) or
    /// claims to be this computer: it is then unnamed, and the badge says
    /// "another computer" (BK8 live: one that only takes input never said
    /// its name at all).
    pub fn name_as_paired(&mut self, paired: &HashMap<String, String>) {
        let shown = |name: &str| super::badge::clip(name, super::badge::NAME_CHARS);
        for computer in &mut self.computers {
            match paired
                .get(&computer.device_id)
                .filter(|name| !shown(name).is_empty())
            {
                Some(name) => computer.name = name.clone(),
                None => {
                    let own = shown(&computer.name);
                    if own.is_empty() || own.eq_ignore_ascii_case("this computer") {
                        computer.name = String::new();
                    }
                }
            }
        }
    }
}

impl Placed {
    /// The box around the screens, in this computer's own desktop space.
    fn bounds(&self) -> Option<Rect> {
        self.screens
            .iter()
            .map(Rect::of)
            .filter(|r| !r.is_empty())
            .reduce(Rect::union)
    }
}

/// A way the cursor can leave a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    /// The way a movement heads: along its larger part, sideways when they
    /// tie, nowhere when there is none.
    fn of(dx: i32, dy: i32) -> Option<Dir> {
        if dx == 0 && dy == 0 {
            None
        } else if dx.unsigned_abs() >= dy.unsigned_abs() {
            Some(if dx > 0 { Dir::Right } else { Dir::Left })
        } else {
            Some(if dy > 0 { Dir::Down } else { Dir::Up })
        }
    }

    fn horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }

    fn step(self) -> (i64, i64) {
        match self {
            Dir::Left => (-1, 0),
            Dir::Right => (1, 0),
            Dir::Up => (0, -1),
            Dir::Down => (0, 1),
        }
    }
}

/// A rectangle of pixels: from `x0`, `y0` up to but not including `x1`, `y1`.
/// Wide enough that sums of `i32`s never overflow.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Rect {
    fn of(s: &Screen) -> Rect {
        let (x0, y0) = (i64::from(s.x), i64::from(s.y));
        Rect {
            x0,
            y0,
            x1: x0 + i64::from(s.width),
            y1: y0 + i64::from(s.height),
        }
    }

    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    fn contains(&self, (x, y): (i64, i64)) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }

    fn shifted(&self, (dx, dy): (i32, i32)) -> Rect {
        let (dx, dy) = (i64::from(dx), i64::from(dy));
        Rect {
            x0: self.x0 + dx,
            y0: self.y0 + dy,
            x1: self.x1 + dx,
            y1: self.y1 + dy,
        }
    }

    fn union(self, o: Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// `pos` pushed out to the last pixel of this rectangle in `dir`.
    fn last_pixel(&self, dir: Dir, pos: (i64, i64)) -> (i64, i64) {
        match dir {
            Dir::Left => (self.x0, pos.1),
            Dir::Right => (self.x1 - 1, pos.1),
            Dir::Up => (pos.0, self.y0),
            Dir::Down => (pos.0, self.y1 - 1),
        }
    }

    /// The line this rectangle ends at going `dir`.
    fn far(&self, dir: Dir) -> i64 {
        match dir {
            Dir::Left => self.x0,
            Dir::Right => self.x1,
            Dir::Up => self.y0,
            Dir::Down => self.y1,
        }
    }

    /// The line this rectangle starts at when entered going `dir`.
    fn near(&self, dir: Dir) -> i64 {
        match dir {
            Dir::Left => self.x1,
            Dir::Right => self.x0,
            Dir::Up => self.y1,
            Dir::Down => self.y0,
        }
    }

    /// The row or column of pixels a cursor lands on entering going `dir`.
    fn entry(&self, dir: Dir) -> i64 {
        match dir {
            Dir::Left => self.x1 - 1,
            Dir::Right => self.x0,
            Dir::Up => self.y1 - 1,
            Dir::Down => self.y0,
        }
    }

    /// How far this rectangle runs along an edge it is left or entered by
    /// going `dir`: from, up to but not including.
    fn span(&self, dir: Dir) -> (i64, i64) {
        if dir.horizontal() {
            (self.y0, self.y1)
        } else {
            (self.x0, self.x1)
        }
    }
}

/// A pixel `v` of the run `lo..hi` as 0..=65535, the last being 65535.
fn normalize(v: i64, lo: i64, hi: i64) -> u16 {
    let last = hi - lo - 1;
    if last <= 0 {
        return 0;
    }
    let offset = (v - lo).clamp(0, last);
    ((offset * 65535 + last / 2) / last) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The owner's Windows computer: a monitor and, to its right, the laptop's
    /// own screen (logical, at 125%).
    fn windows() -> Placed {
        placed(
            "W",
            (0, 0),
            vec![screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)],
        )
    }

    /// The Linux laptop, placed so its screen touches the Windows laptop's
    /// right edge (1920 + 1536 = 3456).
    fn linux() -> Placed {
        placed("L", (3456, 0), vec![screen(0, 0, 1600, 1000)])
    }

    fn desk() -> Layout {
        Layout {
            computers: vec![windows(), linux()],
        }
    }

    fn crossing(to: &str, at: (i32, i32)) -> Option<Crossing> {
        Some(Crossing {
            to: to.to_string(),
            at,
        })
    }

    #[test]
    fn right_edge_of_laptop_crosses_to_linux() {
        // The screens touch at the same height, so the cursor stays level.
        assert_eq!(
            desk().crossing("W", (3455, 480), 5, 0),
            crossing("L", (0, 480))
        );
    }

    #[test]
    fn edge_between_own_screens_is_not_a_crossing() {
        assert_eq!(desk().crossing("W", (1919, 500), 5, 0), None);
        // Nor is the way back across it.
        assert_eq!(desk().crossing("W", (1920, 500), -5, 0), None);
    }

    #[test]
    fn walls_do_not_cross() {
        let d = desk();
        // The top of the monitor, the bottom of the monitor, the left of
        // everything, and the far side of the Linux screen.
        assert_eq!(d.crossing("W", (900, 0), 0, -5), None);
        assert_eq!(d.crossing("W", (900, 1079), 0, 5), None);
        assert_eq!(d.crossing("W", (0, 500), -5, 0), None);
        assert_eq!(d.crossing("L", (1599, 500), 5, 0), None);
    }

    #[test]
    fn only_the_edge_pixel_in_the_direction_of_travel_crosses() {
        let d = desk();
        // One step short of the edge.
        assert_eq!(d.crossing("W", (3454, 480), 5, 0), None);
        // On the edge but moving away from it, or not moving.
        assert_eq!(d.crossing("W", (3455, 480), -5, 0), None);
        assert_eq!(d.crossing("W", (3455, 480), 0, 0), None);
        // Off every screen of that computer.
        assert_eq!(d.crossing("W", (3456, 480), 5, 0), None);
        // A computer the layout does not know.
        assert_eq!(d.crossing("X", (3455, 480), 5, 0), None);
    }

    #[test]
    fn the_larger_movement_picks_the_edge_at_a_corner() {
        // The Linux screen's top-left pixel touches only the laptop to its
        // left, so a push mostly sideways crosses and one mostly upward
        // meets the wall, however small the sideways part of it.
        let d = desk();
        assert!(d.crossing("L", (0, 0), -5, -1).is_some());
        assert_eq!(d.crossing("L", (0, 0), -1, -5), None);
        // Equal pushes go by the sideways one.
        assert!(d.crossing("L", (0, 0), -3, -3).is_some());
    }

    #[test]
    fn crossing_back_lands_on_the_edge() {
        assert_eq!(
            desk().crossing("L", (0, 500), -5, 0),
            crossing("W", (3455, 500))
        );
    }

    #[test]
    fn only_the_touching_part_crosses() {
        // A 1200-high Linux screen 100 lower than the 960-high laptop screen
        // it is beside: they face each other from layout height 100 to 960.
        let layout = Layout {
            computers: vec![
                windows(),
                placed("L", (3456, 100), vec![screen(0, 0, 1600, 1200)]),
            ],
        };
        // Above where the Linux screen starts is a wall.
        assert_eq!(layout.crossing("W", (3455, 50), 5, 0), None);
        // On the touching part the cursor keeps its height in the layout: 500
        // down is 400 down the Linux screen, which starts at 100.
        assert_eq!(
            layout.crossing("W", (3455, 500), 5, 0),
            crossing("L", (0, 400))
        );
        // And back the same way.
        assert_eq!(
            layout.crossing("L", (0, 400), -5, 0),
            crossing("W", (3455, 500))
        );
        // Below the bottom of the laptop screen is a wall too.
        assert_eq!(layout.crossing("L", (0, 1000), -5, 0), None);
    }

    #[test]
    fn crossings_need_the_screens_to_touch_and_overlap() {
        let beside = |origin: (i32, i32)| Layout {
            computers: vec![
                windows(),
                placed("L", origin, vec![screen(0, 0, 1600, 1000)]),
            ],
        };
        let from_laptop = |l: &Layout, y| l.crossing("W", (3455, y), 5, 0).map(|c| c.to);
        // A gap of one pixel still touches; two do not.
        assert_eq!(from_laptop(&beside((3457, 0)), 480), Some("L".to_string()));
        assert_eq!(from_laptop(&beside((3458, 0)), 480), None);
        // Touching only at a corner is not facing.
        assert_eq!(from_laptop(&beside((3456, 960)), 480), None);
        assert_eq!(from_laptop(&beside((3456, 960)), 959), None);
        // Facing on part of the edge: only that part crosses.
        let low = beside((3456, 500));
        assert_eq!(from_laptop(&low, 480), None);
        assert_eq!(from_laptop(&low, 600), Some("L".to_string()));
    }

    #[test]
    fn vertical_crossing_uses_the_bottom_edge() {
        let layout = Layout {
            computers: vec![
                placed("A", (0, 0), vec![screen(0, 0, 1920, 1080)]),
                placed("B", (0, 1080), vec![screen(0, 0, 1280, 720)]),
            ],
        };
        assert_eq!(
            layout.crossing("A", (960, 1079), 0, 5),
            crossing("B", (960, 0))
        );
        assert_eq!(
            layout.crossing("B", (640, 0), 0, -5),
            crossing("A", (640, 1079))
        );
        // The bottom of the lower screen is a wall, and so is the part of the
        // upper one's bottom with no lower screen under it.
        assert_eq!(layout.crossing("B", (640, 719), 0, 5), None);
        assert_eq!(layout.crossing("A", (1500, 1079), 0, 5), None);
    }

    #[test]
    fn crossing_picks_the_screen_beside_the_cursor() {
        // Two Linux screens stacked beside the laptop.
        let layout = Layout {
            computers: vec![
                placed("W", (0, 0), vec![screen(0, 0, 1536, 960)]),
                placed(
                    "L",
                    (1536, 0),
                    vec![screen(0, 0, 1536, 480), screen(0, 480, 1536, 480)],
                ),
            ],
        };
        let to = |y| layout.crossing("W", (1535, y), 5, 0).map(|c| c.at.1);
        assert_eq!(to(100), Some(100));
        assert_eq!(to(800), Some(800));
    }

    #[test]
    fn clamp_after_screen_removed() {
        // The monitor is unplugged: only the laptop's screen is left.
        let mut w = windows();
        w.screens.remove(0);
        let layout = Layout {
            computers: vec![w, linux()],
        };
        // Inside what is left, a point stays.
        assert_eq!(layout.clamp("W", (2000, 500)), (2000, 500));
        // On the removed screen it goes to the nearest edge of the one left.
        assert_eq!(layout.clamp("W", (500, 500)), (1920, 500));
        // Past the far corner, to the corner.
        assert_eq!(layout.clamp("W", (9000, 9000)), (3455, 959));
        assert_eq!(layout.clamp("W", (-50, -50)), (1920, 0));
        // Unknown computers and ones with no screens are left alone.
        assert_eq!(layout.clamp("X", (-5, -5)), (-5, -5));
        let none = Layout {
            computers: vec![placed("N", (0, 0), Vec::new())],
        };
        assert_eq!(none.clamp("N", (-5, -5)), (-5, -5));
    }

    #[test]
    fn clamp_picks_the_nearest_of_several_screens() {
        // The gap between the two screens: nearer the left one, then the
        // right one.
        let layout = Layout {
            computers: vec![placed(
                "W",
                (0, 0),
                vec![screen(0, 0, 1000, 800), screen(1200, 0, 1000, 800)],
            )],
        };
        assert_eq!(layout.clamp("W", (1090, 400)), (999, 400));
        assert_eq!(layout.clamp("W", (1110, 400)), (1200, 400));
    }

    #[test]
    fn normalized_spans_the_bounding_box() {
        let d = desk();
        assert_eq!(d.normalized("L", (0, 0)), (0, 0));
        assert_eq!(d.normalized("L", (1599, 999)), (65535, 65535));
        let (mid_x, mid_y) = d.normalized("L", (799, 499));
        assert!(mid_x.abs_diff(32768) <= 32 && mid_y.abs_diff(32768) <= 64);
        // Both Windows screens count: the box is 3456 wide.
        assert_eq!(d.normalized("W", (3455, 1079)), (65535, 65535));
        assert_eq!(d.normalized("W", (0, 0)), (0, 0));
        // Outside the box is held at its edge, not wrapped.
        assert_eq!(d.normalized("L", (-10, 5000)), (0, 65535));
        // Screens left of and above the origin start the box there.
        let off = Layout {
            computers: vec![placed("N", (0, 0), vec![screen(-1280, -200, 1280, 1024)])],
        };
        assert_eq!(off.normalized("N", (-1280, -200)), (0, 0));
        assert_eq!(off.normalized("N", (-1, 823)), (65535, 65535));
        // No such computer, or no screens: the corner.
        assert_eq!(d.normalized("X", (5, 5)), (0, 0));
    }

    #[test]
    fn default_puts_the_others_to_the_right_with_tops_level() {
        // The computer's screens start 100 above its origin and the others
        // are given stale origins.
        let this = placed("W", (0, 0), vec![screen(0, -100, 1920, 1080)]);
        let stale = placed("L", (9, 9), vec![screen(0, 0, 1600, 1000)]);
        let third = placed("M", (7, 7), vec![screen(-10, 20, 800, 600)]);
        let layout = Layout::default_for(this.clone(), vec![stale, third]);
        let ids: Vec<&str> = layout
            .computers
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(ids, ["W", "L", "M"]);
        assert_eq!(layout.computers[0], this);
        // L's left edge meets W's right edge (1920), its top is W's (-100).
        assert_eq!(layout.computers[1].origin, (1920, -100));
        // M's left edge (origin - 10) meets L's right edge (1920 + 1600).
        assert_eq!(layout.computers[2].origin, (3530, -120));
    }

    #[test]
    fn default_for_the_owners_desk_is_the_desk() {
        let stale = Placed {
            origin: (0, 0),
            ..linux()
        };
        assert_eq!(Layout::default_for(windows(), vec![stale]), desk());
        // And what it makes can be crossed.
        assert_eq!(
            Layout::default_for(windows(), vec![linux()]).crossing("W", (3455, 480), 5, 0),
            crossing("L", (0, 480))
        );
    }

    /// BK8 live, and review M1/M2: each computer is named as the owner
    /// paired it, whatever it calls itself (one that only takes input never
    /// says hello, and one could call itself "this computer"); a name blank
    /// once cleaned for showing (zero-width only) is no name.
    #[test]
    fn a_computer_is_named_as_the_owner_paired_it() {
        let named = |id: &str, y: i32, name: &str| {
            let mut c = placed(id, (0, y), vec![screen(0, 0, 800, 600)]);
            c.name = name.to_string();
            c
        };
        let mut layout = Layout {
            computers: vec![
                windows(),
                named("L", 1000, " "),
                named("N", 2000, "this computer"),
                named("Z", 3000, "\u{200B}"),
                named("S", 4000, "Said"),
                named("B", 5000, "Its own"),
                named("T", 6000, "This Computer"),
                named("U", 7000, "\u{200B}\u{2066}"),
            ],
        };
        let paired = HashMap::from([
            ("L".to_string(), "Arch laptop".to_string()),
            ("N".to_string(), "Desk".to_string()),
            ("Z".to_string(), "Spare".to_string()),
            // A paired name blank once cleaned names nothing.
            ("B".to_string(), "\u{200B}".to_string()),
        ]);
        layout.name_as_paired(&paired);
        let names: Vec<&str> = layout.computers.iter().map(|c| c.name.as_str()).collect();
        // Not paired: its own name stays, unless that is no name or claims
        // to be this computer (the badge then says "another computer").
        assert_eq!(
            names,
            [
                "W",
                "Arch laptop",
                "Desk",
                "Spare",
                "Said",
                "Its own",
                "",
                ""
            ]
        );
    }
}
