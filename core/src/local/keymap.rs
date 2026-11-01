//! Keys to taps: play a phone's game from this computer's keyboard and mouse
//! in the phone's app window.
//!
//! A [`Keymap`] is what the owner drew for one phone app (its package): keys
//! that tap a point, a stick (four keys held as one finger that leaves the
//! middle of an area and moves toward the way they point), a swipe (a
//! start to an end), and an aim area the locked mouse drags in. It is saved
//! under the state folder, one small file per package, on this computer
//! only. The [`Runner`] turns key presses and mouse movement into the
//! several fingers at once ([`ControlEvent::Pointer`]) the phone's helper
//! injects into the window's own display.
//!
//! Points are fractions of the window's picture, 0 to 65535 on each axis, as
//! every control event; a radius is a fraction of the picture's width, so a
//! stick is round whatever shape the phone's screen is. Finger 0 is the
//! mouse's own touch; the runner uses fingers 1 to 9 only.
//!
//! Nothing here touches the network or reads the phone; the model, its file
//! and the runner are pure std + serde, tested in this module.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::streaming::cast_control::{ControlEvent, MAX_POINTERS, TouchAction};

/// The folder under the state folder where keymaps are kept.
pub const KEYMAP_DIR: &str = "keymaps";
/// Most bindings in one keymap.
pub const MAX_BINDINGS: usize = 24;
/// The largest keymap file read, in bytes.
pub const FILE_MAX: u64 = 64 * 1024;
/// The file format's version.
pub const VERSION: u8 = 1;
/// A radius's range, as a fraction of the picture's width (of 65535).
pub const RADIUS_RANGE: std::ops::RangeInclusive<u16> = 600..=32768;
/// The aim's speed, in percent: 100 moves the finger as far as the mouse moved.
pub const SENSITIVITY_RANGE: std::ops::RangeInclusive<u16> = 10..=2000;
/// A swipe's length, in milliseconds.
pub const SWIPE_MS_RANGE: std::ops::RangeInclusive<u16> = 50..=3000;
/// The aim's finger is lifted after the mouse has been still this long.
pub const AIM_IDLE: Duration = Duration::from_millis(150);

/// The mouse buttons a binding may use, as tokens.
pub const MOUSE_LEFT: &str = "MouseLeft";
pub const MOUSE_RIGHT: &str = "MouseRight";
pub const MOUSE_MIDDLE: &str = "MouseMiddle";

/// The four keys a new stick starts with.
pub const WASD: [&str; 4] = ["W", "S", "A", "D"];

/// A point of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

impl Point {
    pub const CENTER: Point = Point { x: 32768, y: 32768 };
}

/// One thing a key or the mouse does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Binding {
    /// The key holds a finger down at `at` while it is held.
    Tap { key: String, at: Point },
    /// Four keys move one finger from `center` toward the way they point,
    /// `radius` away at most.
    Stick {
        up: String,
        down: String,
        left: String,
        right: String,
        center: Point,
        radius: u16,
    },
    /// The key swipes from `from` to `to` over `ms` milliseconds.
    Swipe {
        key: String,
        from: Point,
        to: Point,
        ms: u16,
    },
    /// The locked mouse drags a finger around `center`, not past `radius`;
    /// `sensitivity` is a percent.
    Aim {
        center: Point,
        radius: u16,
        sensitivity: u16,
    },
}

impl Binding {
    /// The keys it listens to, in slot order.
    pub fn keys(&self) -> Vec<&str> {
        match self {
            Binding::Tap { key, .. } | Binding::Swipe { key, .. } => vec![key],
            Binding::Stick {
                up,
                down,
                left,
                right,
                ..
            } => vec![up, down, left, right],
            Binding::Aim { .. } => vec![],
        }
    }

    /// What the owner calls it.
    pub fn name(&self) -> &'static str {
        match self {
            Binding::Tap { .. } => "Tap",
            Binding::Stick { .. } => "Stick",
            Binding::Swipe { .. } => "Swipe",
            Binding::Aim { .. } => "Aim",
        }
    }

    /// The point the owner drags to move it.
    pub fn anchor(&self) -> Point {
        match self {
            Binding::Tap { at, .. } => *at,
            Binding::Stick { center, .. } | Binding::Aim { center, .. } => *center,
            Binding::Swipe { from, .. } => *from,
        }
    }

    fn key_slot_mut(&mut self, slot: usize) -> Option<&mut String> {
        match (self, slot) {
            (Binding::Tap { key, .. } | Binding::Swipe { key, .. }, 0) => Some(key),
            (Binding::Stick { up, .. }, 0) => Some(up),
            (Binding::Stick { down, .. }, 1) => Some(down),
            (Binding::Stick { left, .. }, 2) => Some(left),
            (Binding::Stick { right, .. }, 3) => Some(right),
            _ => None,
        }
    }

    fn check(&self) -> Result<(), String> {
        match self {
            Binding::Tap { .. } => {}
            Binding::Stick { radius, .. } | Binding::Aim { radius, .. } => {
                if !RADIUS_RANGE.contains(radius) {
                    return Err("The radius is out of range.".into());
                }
            }
            Binding::Swipe { ms, .. } => {
                if !SWIPE_MS_RANGE.contains(ms) {
                    return Err("The swipe's time is out of range.".into());
                }
            }
        }
        if let Binding::Aim { sensitivity, .. } = self
            && !SENSITIVITY_RANGE.contains(sensitivity)
        {
            return Err("The aim's speed is out of range.".into());
        }
        for key in self.keys() {
            if !token_ok(key) {
                return Err(format!("{} cannot be used as a key.", token_label(key)));
            }
        }
        Ok(())
    }
}

/// What the owner drew for one phone app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keymap {
    #[serde(default = "version")]
    pub version: u8,
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

fn version() -> u8 {
    VERSION
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            version: VERSION,
            bindings: Vec::new(),
        }
    }
}

/// Whether `token` may be a key of a binding: a key's name (as the window
/// names it), or a mouse button; never Escape, which is the phone's Back.
pub fn token_ok(token: &str) -> bool {
    if matches!(token, MOUSE_LEFT | MOUSE_RIGHT | MOUSE_MIDDLE) {
        return true;
    }
    !token.is_empty()
        && token.len() <= 16
        && token.chars().all(|c| c.is_ascii_graphic())
        && token != "Escape"
}

/// A token as the owner reads it on a marker or a button.
pub fn token_label(token: &str) -> String {
    match token {
        MOUSE_LEFT => "Mouse left".into(),
        MOUSE_RIGHT => "Mouse right".into(),
        MOUSE_MIDDLE => "Mouse middle".into(),
        other => other.to_string(),
    }
}

impl Keymap {
    /// Whether the keymap is sound: within the caps, every key usable and
    /// used once, one aim at most.
    pub fn validate(&self) -> Result<(), String> {
        if self.bindings.len() > MAX_BINDINGS {
            return Err(format!("A keymap holds at most {MAX_BINDINGS} bindings."));
        }
        let mut seen: Vec<&str> = Vec::new();
        for binding in &self.bindings {
            binding.check()?;
            for key in binding.keys() {
                if seen.contains(&key) {
                    return Err(format!("{} is used twice.", token_label(key)));
                }
                seen.push(key);
            }
        }
        if self
            .bindings
            .iter()
            .filter(|b| matches!(b, Binding::Aim { .. }))
            .count()
            > 1
        {
            return Err("A keymap has one aim area.".into());
        }
        Ok(())
    }

    /// Whether some binding already uses `token`.
    pub fn uses(&self, token: &str) -> bool {
        self.bindings.iter().any(|b| b.keys().contains(&token))
    }

    /// Whether the keymap has an aim area.
    pub fn has_aim(&self) -> bool {
        self.bindings
            .iter()
            .any(|b| matches!(b, Binding::Aim { .. }))
    }

    /// Add `binding`; its place, or why not (the keymap is then unchanged).
    pub fn add(&mut self, binding: Binding) -> Result<usize, String> {
        self.bindings.push(binding);
        if let Err(e) = self.validate() {
            self.bindings.pop();
            return Err(e);
        }
        Ok(self.bindings.len() - 1)
    }

    /// A tap of `key` at `at`.
    pub fn add_tap(&mut self, key: &str, at: Point) -> Result<usize, String> {
        self.add(Binding::Tap {
            key: key.to_string(),
            at,
        })
    }

    /// A stick on W, A, S and D around `center`.
    pub fn add_stick(&mut self, center: Point) -> Result<usize, String> {
        let [up, down, left, right] = WASD.map(str::to_string);
        self.add(Binding::Stick {
            up,
            down,
            left,
            right,
            center,
            radius: 6000,
        })
    }

    /// A swipe of `key` from `from` to `to`.
    pub fn add_swipe(&mut self, key: &str, from: Point, to: Point) -> Result<usize, String> {
        self.add(Binding::Swipe {
            key: key.to_string(),
            from,
            to,
            ms: 250,
        })
    }

    /// The aim area, around `center`.
    pub fn add_aim(&mut self, center: Point) -> Result<usize, String> {
        self.add(Binding::Aim {
            center,
            radius: 8000,
            sensitivity: 300,
        })
    }

    /// Take binding `index` away.
    pub fn remove(&mut self, index: usize) {
        if index < self.bindings.len() {
            self.bindings.remove(index);
        }
    }

    /// Give key slot `slot` of binding `index` the key `token`; why not
    /// when it cannot (the keymap is then unchanged).
    pub fn rebind(&mut self, index: usize, slot: usize, token: &str) -> Result<(), String> {
        let before = self.clone();
        let Some(key) = self
            .bindings
            .get_mut(index)
            .and_then(|b| b.key_slot_mut(slot))
        else {
            return Err("There is no such key.".into());
        };
        *key = token.to_string();
        if let Err(e) = self.validate() {
            *self = before;
            return Err(e);
        }
        Ok(())
    }

    /// Move binding `index`'s point (`end` false) or a swipe's end (`end`
    /// true) to `to`.
    pub fn move_point(&mut self, index: usize, end: bool, to: Point) {
        match self.bindings.get_mut(index) {
            Some(Binding::Tap { at, .. }) => *at = to,
            Some(Binding::Stick { center, .. } | Binding::Aim { center, .. }) => *center = to,
            Some(Binding::Swipe {
                from, to: end_at, ..
            }) => {
                if end {
                    *end_at = to;
                } else {
                    *from = to;
                }
            }
            None => {}
        }
    }

    /// Set a stick's or the aim's radius, held to its range.
    pub fn set_radius(&mut self, index: usize, value: u16) {
        if let Some(Binding::Stick { radius, .. } | Binding::Aim { radius, .. }) =
            self.bindings.get_mut(index)
        {
            *radius = value.clamp(*RADIUS_RANGE.start(), *RADIUS_RANGE.end());
        }
    }

    /// Set the aim's speed (percent), held to its range.
    pub fn set_sensitivity(&mut self, index: usize, value: u16) {
        if let Some(Binding::Aim { sensitivity, .. }) = self.bindings.get_mut(index) {
            *sensitivity = value.clamp(*SENSITIVITY_RANGE.start(), *SENSITIVITY_RANGE.end());
        }
    }

    /// Set a swipe's time, held to its range.
    pub fn set_swipe_ms(&mut self, index: usize, value: u16) {
        if let Some(Binding::Swipe { ms, .. }) = self.bindings.get_mut(index) {
            *ms = value.clamp(*SWIPE_MS_RANGE.start(), *SWIPE_MS_RANGE.end());
        }
    }
}

/// Whether `package` is an Android package name (the phone's own rule,
/// [`crate::protocol::phone_action::valid_app_id`]): dotted words of letters,
/// digits and underscores, each starting with a letter, at least one dot.
pub fn package_ok(package: &str) -> bool {
    crate::protocol::phone_action::valid_app_id(package)
}

/// The file of `package`'s keymap, `app.<package>.json`: the prefix keeps a
/// package from ever being a Windows device name (`con.x.json`); `None` for
/// a name that is not a package.
pub fn path(state_dir: &Path, package: &str) -> Option<PathBuf> {
    package_ok(package).then(|| {
        state_dir
            .join(KEYMAP_DIR)
            .join(format!("app.{package}.json"))
    })
}

/// Where a keymap was kept before the `app.` prefix: `<package>.json`.
fn legacy_path(state_dir: &Path, package: &str) -> Option<PathBuf> {
    package_ok(package).then(|| state_dir.join(KEYMAP_DIR).join(format!("{package}.json")))
}

/// A keymap read from `path`, if the file is small and sound.
fn read(path: &Path) -> Option<Keymap> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > FILE_MAX {
        return None;
    }
    let map: Keymap = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (map.version == VERSION && map.validate().is_ok()).then_some(map)
}

/// `package`'s saved keymap; `None` when there is none, it is too large or
/// it is not sound (a hand-edited file never reaches the phone). A file
/// from before the `app.` prefix is moved to its new name as it is read.
pub fn load(state_dir: &Path, package: &str) -> Option<Keymap> {
    let path = path(state_dir, package)?;
    if path.exists() {
        return read(&path);
    }
    let old = legacy_path(state_dir, package)?;
    let map = read(&old)?;
    if save(state_dir, package, &map).is_ok() {
        let _ = std::fs::remove_file(&old);
    }
    Some(map)
}

/// Save `map` as `package`'s keymap (an empty one removes the file).
pub fn save(state_dir: &Path, package: &str, map: &Keymap) -> io::Result<()> {
    let bad = |what: String| io::Error::new(io::ErrorKind::InvalidInput, what);
    let Some(path) = path(state_dir, package) else {
        return Err(bad("not a package name".into()));
    };
    map.validate().map_err(bad)?;
    // A file under the old name is superseded by this save.
    if let Some(old) = legacy_path(state_dir, package) {
        let _ = std::fs::remove_file(old);
    }
    if map.bindings.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(state_dir.join(KEYMAP_DIR))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(map).map_err(io::Error::other)?,
    )?;
    crate::atomic_file::replace(&tmp, &path)
}

/// What the runner makes of the keys: one finger changing.
pub fn finger(pointer: u8, action: TouchAction, at: Point) -> ControlEvent {
    ControlEvent::Pointer {
        pointer,
        action,
        x: at.x,
        y: at.y,
    }
}

/// What a binding is doing now.
#[derive(Debug, Clone, Default)]
enum Live {
    #[default]
    Idle,
    Tap {
        pointer: u8,
    },
    Stick {
        pointer: Option<u8>,
        /// Up, down, left, right held.
        held: [bool; 4],
        at: Point,
    },
    Swipe {
        pointer: u8,
        start: Instant,
        at: Point,
    },
    Aim {
        pointer: Option<u8>,
        /// Where the finger is, in the picture's own pixels.
        pos: (f32, f32),
        last: Instant,
    },
}

/// Turns keys and mouse movement into fingers, for one window.
#[derive(Debug)]
pub struct Runner {
    map: Keymap,
    /// The picture's size in pixels (a radius is a share of its width).
    picture: (f32, f32),
    /// Which fingers 1 to 9 are down.
    used: [bool; MAX_POINTERS as usize],
    live: Vec<Live>,
}

impl Runner {
    pub fn new(map: Keymap) -> Self {
        let live = vec![Live::Idle; map.bindings.len()];
        Runner {
            map,
            picture: (1.0, 1.0),
            used: [false; MAX_POINTERS as usize],
            live,
        }
    }

    /// The picture's size as it is now, so a radius is round.
    pub fn set_picture(&mut self, width: f32, height: f32) {
        if width >= 1.0 && height >= 1.0 {
            self.picture = (width, height);
        }
    }

    /// A new keymap: every finger down is lifted first (and returned).
    pub fn set_map(&mut self, map: Keymap) -> Vec<ControlEvent> {
        let out = self.release_all();
        self.live = vec![Live::Idle; map.bindings.len()];
        self.map = map;
        out
    }

    /// Whether any finger is down.
    pub fn busy(&self) -> bool {
        self.used.iter().any(|u| *u)
    }

    /// Whether the keymap listens to `token`.
    pub fn listens(&self, token: &str) -> bool {
        self.map.uses(token)
    }

    fn alloc(&mut self) -> Option<u8> {
        let free = (1..MAX_POINTERS as usize).find(|i| !self.used[*i])?;
        self.used[free] = true;
        Some(free as u8)
    }

    fn free(&mut self, pointer: u8) {
        if let Some(slot) = self.used.get_mut(pointer as usize) {
            *slot = false;
        }
    }

    fn px_of(&self, p: Point) -> (f32, f32) {
        (
            f32::from(p.x) / 65535.0 * self.picture.0,
            f32::from(p.y) / 65535.0 * self.picture.1,
        )
    }

    fn frac_of(&self, px: (f32, f32)) -> Point {
        Point {
            x: (px.0 / self.picture.0 * 65535.0)
                .round()
                .clamp(0.0, 65535.0) as u16,
            y: (px.1 / self.picture.1 * 65535.0)
                .round()
                .clamp(0.0, 65535.0) as u16,
        }
    }

    fn radius_px(&self, radius: u16) -> f32 {
        f32::from(radius) / 65535.0 * self.picture.0
    }

    /// A key or mouse button `token` pressed or let go.
    pub fn key(&mut self, token: &str, pressed: bool, now: Instant) -> Vec<ControlEvent> {
        let mut out = Vec::new();
        for i in 0..self.map.bindings.len() {
            let binding = self.map.bindings[i].clone();
            match &binding {
                Binding::Tap { key, at } if key == token => {
                    if pressed {
                        if matches!(self.live[i], Live::Idle)
                            && let Some(p) = self.alloc()
                        {
                            out.push(finger(p, TouchAction::Down, *at));
                            self.live[i] = Live::Tap { pointer: p };
                        }
                    } else if let Live::Tap { pointer: p } = self.live[i] {
                        out.push(finger(p, TouchAction::Up, *at));
                        self.free(p);
                        self.live[i] = Live::Idle;
                    }
                }
                Binding::Stick {
                    up,
                    down,
                    left,
                    right,
                    ..
                } => {
                    let slot = [up, down, left, right].iter().position(|k| *k == token);
                    if let Some(slot) = slot {
                        if matches!(self.live[i], Live::Idle) {
                            self.live[i] = Live::Stick {
                                pointer: None,
                                held: [false; 4],
                                at: Point::CENTER,
                            };
                        }
                        if let Live::Stick { held, .. } = &mut self.live[i] {
                            held[slot] = pressed;
                        }
                        self.stick(i, &binding, &mut out);
                    }
                }
                Binding::Swipe { key, from, .. } if key == token && pressed => {
                    if matches!(self.live[i], Live::Idle)
                        && let Some(p) = self.alloc()
                    {
                        out.push(finger(p, TouchAction::Down, *from));
                        self.live[i] = Live::Swipe {
                            pointer: p,
                            start: now,
                            at: *from,
                        };
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A stick's finger after one of its keys changed.
    fn stick(&mut self, i: usize, binding: &Binding, out: &mut Vec<ControlEvent>) {
        let Binding::Stick { center, radius, .. } = binding else {
            return;
        };
        let Live::Stick { pointer, held, at } = self.live[i].clone() else {
            return;
        };
        if !held.iter().any(|h| *h) {
            if let Some(p) = pointer {
                out.push(finger(p, TouchAction::Up, at));
                self.free(p);
            }
            self.live[i] = Live::Idle;
            return;
        }
        let mut dx = f32::from(u8::from(held[3])) - f32::from(u8::from(held[2]));
        let mut dy = f32::from(u8::from(held[1])) - f32::from(u8::from(held[0]));
        let length = dx.hypot(dy);
        if length > 0.0 {
            dx /= length;
            dy /= length;
        }
        let middle = self.px_of(*center);
        let reach = self.radius_px(*radius);
        let target = self.frac_of((middle.0 + dx * reach, middle.1 + dy * reach));
        let mut now_at = at;
        let p = match pointer {
            Some(p) => p,
            None => {
                let Some(p) = self.alloc() else {
                    self.live[i] = Live::Idle;
                    return;
                };
                out.push(finger(p, TouchAction::Down, *center));
                now_at = *center;
                p
            }
        };
        if target != now_at {
            out.push(finger(p, TouchAction::Move, target));
            now_at = target;
        }
        self.live[i] = Live::Stick {
            pointer: Some(p),
            held,
            at: now_at,
        };
    }

    /// The mouse moved by (`dx`, `dy`), each a fraction of the picture's
    /// own side (so a move of the same pixels is the same on both axes).
    pub fn mouse(&mut self, dx: f32, dy: f32, now: Instant) -> Vec<ControlEvent> {
        let mut out = Vec::new();
        for i in 0..self.map.bindings.len() {
            let Binding::Aim {
                center,
                radius,
                sensitivity,
            } = self.map.bindings[i].clone()
            else {
                continue;
            };
            let middle = self.px_of(center);
            let reach = self.radius_px(radius);
            let (mut pointer_id, mut pos) = match self.live[i] {
                Live::Aim { pointer, pos, .. } => (pointer, pos),
                _ => (None, middle),
            };
            if pointer_id.is_none() {
                let Some(p) = self.alloc() else { continue };
                out.push(finger(p, TouchAction::Down, center));
                pointer_id = Some(p);
                pos = middle;
            }
            let Some(p) = pointer_id else { continue };
            let scale = f32::from(sensitivity) / 100.0;
            pos.0 += dx * self.picture.0 * scale;
            pos.1 += dy * self.picture.1 * scale;
            let off = (pos.0 - middle.0).hypot(pos.1 - middle.1);
            if off > reach {
                // At the edge: lift there and put the finger down in the
                // middle again, as a thumb that ran out of screen would.
                let k = reach / off;
                let edge = self.frac_of((
                    middle.0 + (pos.0 - middle.0) * k,
                    middle.1 + (pos.1 - middle.1) * k,
                ));
                out.push(finger(p, TouchAction::Move, edge));
                out.push(finger(p, TouchAction::Up, edge));
                out.push(finger(p, TouchAction::Down, center));
                pos = middle;
            } else {
                out.push(finger(p, TouchAction::Move, self.frac_of(pos)));
            }
            self.live[i] = Live::Aim {
                pointer: Some(p),
                pos,
                last: now,
            };
        }
        out
    }

    /// Time passing: swipes go on, and an aim finger the mouse has left
    /// still is lifted. Call it each pass while [`Self::busy`].
    pub fn tick(&mut self, now: Instant) -> Vec<ControlEvent> {
        let mut out = Vec::new();
        for i in 0..self.map.bindings.len() {
            match (self.live[i].clone(), self.map.bindings[i].clone()) {
                (
                    Live::Swipe {
                        pointer: p,
                        start,
                        at,
                    },
                    Binding::Swipe { from, to, ms, .. },
                ) => {
                    let t = now.saturating_duration_since(start).as_secs_f32()
                        / (f32::from(ms) / 1000.0);
                    if t >= 1.0 {
                        if at != to {
                            out.push(finger(p, TouchAction::Move, to));
                        }
                        out.push(finger(p, TouchAction::Up, to));
                        self.free(p);
                        self.live[i] = Live::Idle;
                    } else {
                        let lerp = |a: u16, b: u16| {
                            (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u16
                        };
                        let here = Point {
                            x: lerp(from.x, to.x),
                            y: lerp(from.y, to.y),
                        };
                        if here != at {
                            out.push(finger(p, TouchAction::Move, here));
                            self.live[i] = Live::Swipe {
                                pointer: p,
                                start,
                                at: here,
                            };
                        }
                    }
                }
                (
                    Live::Aim {
                        pointer: Some(p),
                        pos,
                        last,
                    },
                    Binding::Aim { .. },
                ) if now.saturating_duration_since(last) >= AIM_IDLE => {
                    out.push(finger(p, TouchAction::Up, self.frac_of(pos)));
                    self.free(p);
                    self.live[i] = Live::Idle;
                }
                _ => {}
            }
        }
        out
    }

    /// A word to the phone that fingers are still held: each finger's move to
    /// where it already is. It moves nothing there, but the phone lifts
    /// fingers it has heard nothing about for two seconds, and a key held
    /// down says nothing by itself. Empty when no finger is down.
    pub fn keep_alive(&self) -> Vec<ControlEvent> {
        let mut out = Vec::new();
        for (live, binding) in self.live.iter().zip(&self.map.bindings) {
            match (live, binding) {
                (Live::Tap { pointer: p }, Binding::Tap { at, .. }) => {
                    out.push(finger(*p, TouchAction::Move, *at));
                }
                (
                    Live::Stick {
                        pointer: Some(p),
                        at,
                        ..
                    },
                    _,
                )
                | (Live::Swipe { pointer: p, at, .. }, _) => {
                    out.push(finger(*p, TouchAction::Move, *at));
                }
                (
                    Live::Aim {
                        pointer: Some(p),
                        pos,
                        ..
                    },
                    _,
                ) => out.push(finger(*p, TouchAction::Move, self.frac_of(*pos))),
                _ => {}
            }
        }
        out
    }

    /// Lift every finger (the window lost the focus, the lock or the
    /// keymap went off, the window closed), highest finger first.
    pub fn release_all(&mut self) -> Vec<ControlEvent> {
        let mut ups: Vec<(u8, Point)> = Vec::new();
        for i in 0..self.live.len() {
            match (&self.live[i], &self.map.bindings[i]) {
                (Live::Tap { pointer: p }, Binding::Tap { at, .. }) => ups.push((*p, *at)),
                (
                    Live::Stick {
                        pointer: Some(p),
                        at,
                        ..
                    },
                    _,
                ) => ups.push((*p, *at)),
                (Live::Swipe { pointer: p, at, .. }, _) => ups.push((*p, *at)),
                (
                    Live::Aim {
                        pointer: Some(p),
                        pos,
                        ..
                    },
                    _,
                ) => ups.push((*p, self.frac_of(*pos))),
                _ => {}
            }
        }
        ups.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
        self.live.fill(Live::Idle);
        self.used = [false; MAX_POINTERS as usize];
        ups.into_iter()
            .map(|(p, at)| finger(p, TouchAction::Up, at))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: u16, y: u16) -> Point {
        Point { x, y }
    }

    fn map() -> Keymap {
        let mut m = Keymap::default();
        m.add_stick(p(10000, 40000)).unwrap();
        m.add_tap("Space", p(60000, 50000)).unwrap();
        m.add_swipe("Q", p(20000, 20000), p(40000, 20000)).unwrap();
        m.add_aim(p(45000, 25000)).unwrap();
        m.add_tap(MOUSE_LEFT, p(55000, 40000)).unwrap();
        m
    }

    /// (finger, action, x, y) of each pointer event.
    fn shape(events: &[ControlEvent]) -> Vec<(u8, TouchAction, u16, u16)> {
        events
            .iter()
            .map(|e| match e {
                ControlEvent::Pointer {
                    pointer,
                    action,
                    x,
                    y,
                } => (*pointer, *action, *x, *y),
                other => panic!("not a pointer event: {other:?}"),
            })
            .collect()
    }

    fn dir(tmp: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-keymap-{tmp}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_keymap_comes_back_from_its_json() {
        let m = map();
        let text = serde_json::to_string(&m).unwrap();
        let back: Keymap = serde_json::from_str(&text).unwrap();
        assert_eq!(back, m);
        assert!(text.contains(r#""kind":"stick""#));
        // Fields the file leaves out take their defaults.
        let empty: Keymap = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Keymap::default());
    }

    #[test]
    fn a_keymap_is_kept_in_its_caps() {
        let mut m = Keymap::default();
        assert!(m.add_tap("Escape", p(1, 1)).is_err(), "Escape is Back");
        assert!(m.add_tap("", p(1, 1)).is_err());
        assert!(m.add_tap("ThisKeyNameIsFarTooLong", p(1, 1)).is_err());
        assert!(m.add_tap("A\u{e9}", p(1, 1)).is_err(), "no non-ASCII names");
        m.add_tap("E", p(1, 1)).unwrap();
        assert!(m.add_tap("E", p(2, 2)).is_err(), "a key does one thing");
        assert!(m.add_swipe("E", p(1, 1), p(2, 2)).is_err());
        m.add_stick(p(1, 1)).unwrap();
        assert!(m.add_tap("W", p(1, 1)).is_err(), "W is the stick's");
        m.add_aim(p(1, 1)).unwrap();
        assert!(m.add_aim(p(2, 2)).is_err(), "one aim area");
        assert_eq!(m.bindings.len(), 3, "a refused binding is not kept");
        for i in 0..MAX_BINDINGS {
            let _ = m.add_tap(&format!("F{}", i % 12 + 1), p(1, 1));
        }
        assert!(m.bindings.len() <= MAX_BINDINGS);
        // A radius, a speed or a time out of range is refused when read.
        let aim = Keymap {
            version: VERSION,
            bindings: vec![Binding::Aim {
                center: p(1, 1),
                radius: 5,
                sensitivity: 300,
            }],
        };
        assert!(aim.validate().is_err());
        let swipe = Keymap {
            version: VERSION,
            bindings: vec![Binding::Swipe {
                key: "Q".into(),
                from: p(1, 1),
                to: p(2, 2),
                ms: 9,
            }],
        };
        assert!(swipe.validate().is_err());
    }

    #[test]
    fn the_editor_rebinds_moves_and_removes() {
        let mut m = Keymap::default();
        m.add_stick(p(1, 1)).unwrap();
        m.add_tap("E", p(5, 5)).unwrap();
        // The stick's up key becomes the arrow; a taken key is refused and nothing changes.
        m.rebind(0, 0, "Up").unwrap();
        let before = m.clone();
        assert!(m.rebind(0, 1, "E").is_err());
        assert_eq!(m, before);
        assert!(m.rebind(0, 9, "Z").is_err(), "no such slot");
        assert!(m.rebind(7, 0, "Z").is_err(), "no such binding");
        m.move_point(1, false, p(9, 9));
        assert_eq!(m.bindings[1].anchor(), p(9, 9));
        m.set_radius(0, 1);
        assert!(
            matches!(m.bindings[0], Binding::Stick { radius, .. } if radius == *RADIUS_RANGE.start())
        );
        m.add_swipe("Q", p(1, 2), p(3, 4)).unwrap();
        m.move_point(2, true, p(7, 7));
        assert!(
            matches!(m.bindings[2], Binding::Swipe { to, from, .. } if to == p(7, 7) && from == p(1, 2))
        );
        m.set_swipe_ms(2, 60_000);
        assert!(matches!(m.bindings[2], Binding::Swipe { ms, .. } if ms == *SWIPE_MS_RANGE.end()));
        m.remove(0);
        assert_eq!(m.bindings.len(), 2);
        m.remove(99);
        assert_eq!(m.bindings.len(), 2);
    }

    #[test]
    fn a_keymap_is_saved_per_package_and_read_back_only_when_sound() {
        let dir = dir("save");
        let m = map();
        save(&dir, "org.example.game", &m).unwrap();
        assert_eq!(load(&dir, "org.example.game"), Some(m.clone()));
        assert_eq!(load(&dir, "org.example.other"), None);
        // One file per package, under keymaps/.
        let file = dir.join(KEYMAP_DIR).join("app.org.example.game.json");
        assert!(file.is_file());
        // A package that is a Windows device name or a reserved word has no such file name.
        let con = path(&dir, "con.x").unwrap();
        assert_eq!(con.file_name().unwrap(), "app.con.x.json");
        let nul = path(&dir, "nul.exe").unwrap();
        assert_eq!(nul.file_name().unwrap(), "app.nul.exe.json");
        // Digits first, a trailing dot and the like are no package (the phone's own rule).
        for bad in ["1a.b", "a.1b", "a.b.", "a._b"] {
            assert!(path(&dir, bad).is_none(), "{bad}");
        }
        // A name that could leave the folder is no package.
        for bad in [
            "",
            "../x.y",
            "a/b.c",
            "a\\b.c",
            ".hidden.x",
            "nodots",
            "a..b",
            "a b.c",
        ] {
            assert!(path(&dir, bad).is_none(), "{bad}");
            assert!(save(&dir, bad, &m).is_err(), "{bad}");
        }
        // A file edited into something unsound, or too large, is not used.
        std::fs::write(
            &file,
            r#"{"version":1,"bindings":[{"kind":"tap","key":"Escape","at":{"x":1,"y":1}}]}"#,
        )
        .unwrap();
        assert_eq!(load(&dir, "org.example.game"), None);
        std::fs::write(&file, r#"{"version":9,"bindings":[]}"#).unwrap();
        assert_eq!(load(&dir, "org.example.game"), None, "an unknown version");
        std::fs::write(&file, vec![b' '; FILE_MAX as usize + 1]).unwrap();
        assert_eq!(load(&dir, "org.example.game"), None);
        // An unsound keymap is not written; an empty one removes the file.
        let twice = Keymap {
            version: VERSION,
            bindings: vec![
                Binding::Tap {
                    key: "E".into(),
                    at: p(1, 1),
                },
                Binding::Tap {
                    key: "E".into(),
                    at: p(2, 2),
                },
            ],
        };
        assert!(save(&dir, "org.example.game", &twice).is_err());
        save(&dir, "org.example.game", &Keymap::default()).unwrap();
        assert!(!file.exists());
        save(&dir, "org.example.game", &Keymap::default()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_under_the_old_name_is_moved_to_the_new_one() {
        let dir = dir("legacy");
        let folder = dir.join(KEYMAP_DIR);
        std::fs::create_dir_all(&folder).unwrap();
        let old = folder.join("org.example.game.json");
        let m = map();
        std::fs::write(&old, serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(load(&dir, "org.example.game"), Some(m.clone()));
        assert!(!old.exists(), "the old name is gone");
        assert!(folder.join("app.org.example.game.json").is_file());
        assert_eq!(load(&dir, "org.example.game"), Some(m.clone()));
        // An old file that is not sound is left alone and not used; a save drops it.
        let bad = folder.join("org.example.other.json");
        std::fs::write(&bad, "{").unwrap();
        assert_eq!(load(&dir, "org.example.other"), None);
        assert!(bad.exists());
        save(&dir, "org.example.other", &m).unwrap();
        assert!(!bad.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_keep_alive_moves_every_held_finger_to_where_it_is() {
        let now = Instant::now();
        let mut run = Runner::new(map());
        run.set_picture(1000.0, 1000.0);
        assert!(run.keep_alive().is_empty(), "nothing held, nothing said");
        run.key("W", true, now);
        run.key("Space", true, now);
        run.mouse(0.001, 0.0, now);
        run.key("Q", true, now);
        let alive = shape(&run.keep_alive());
        assert_eq!(alive.len(), 4);
        assert!(alive.iter().all(|e| e.1 == TouchAction::Move));
        // The stick's finger, at its place after the W (up from the middle).
        assert!(alive.contains(&(1, TouchAction::Move, 10000, 34000)));
        assert!(alive.contains(&(2, TouchAction::Move, 60000, 50000)));
        // And it says nothing the phone would act on twice: a stick is where it was.
        let again = shape(&run.keep_alive());
        assert_eq!(alive, again);
        run.release_all();
        assert!(run.keep_alive().is_empty());
    }

    #[test]
    fn a_tap_is_a_finger_down_while_its_key_is_held() {
        let now = Instant::now();
        let mut run = Runner::new(map());
        // The mouse button is a key too.
        let down = shape(&run.key("Space", true, now));
        assert_eq!(down, vec![(1, TouchAction::Down, 60000, 50000)]);
        assert!(run.busy());
        // The key repeating does not put a second finger down.
        assert!(run.key("Space", true, now).is_empty());
        let click = shape(&run.key(MOUSE_LEFT, true, now));
        assert_eq!(click, vec![(2, TouchAction::Down, 55000, 40000)]);
        let up = shape(&run.key("Space", false, now));
        assert_eq!(up, vec![(1, TouchAction::Up, 60000, 50000)]);
        let up = shape(&run.key(MOUSE_LEFT, false, now));
        assert_eq!(up, vec![(2, TouchAction::Up, 55000, 40000)]);
        assert!(!run.busy());
        // A key nothing listens to, or let go without a press, does nothing.
        assert!(run.key("Z", true, now).is_empty());
        assert!(run.key("Space", false, now).is_empty());
        assert!(run.listens("Space") && !run.listens("Z"));
    }

    #[test]
    fn a_stick_is_one_finger_that_moves_with_its_keys() {
        let now = Instant::now();
        let mut m = Keymap::default();
        m.add_stick(p(10000, 40000)).unwrap();
        let mut run = Runner::new(m);
        // A square picture: the radius is 6000 of 65535 each way.
        run.set_picture(1000.0, 1000.0);
        // W (up): down in the middle, then toward up.
        let w = shape(&run.key("W", true, now));
        assert_eq!(
            w,
            vec![
                (1, TouchAction::Down, 10000, 40000),
                (1, TouchAction::Move, 10000, 34000),
            ]
        );
        // W and D: one finger, a diagonal of the same reach.
        let wd = shape(&run.key("D", true, now));
        assert_eq!(wd.len(), 1);
        let (finger, action, x, y) = wd[0];
        assert_eq!((finger, action), (1, TouchAction::Move));
        assert!(
            (i32::from(x) - 14243).abs() <= 2 && (i32::from(y) - 35757).abs() <= 2,
            "{x},{y}"
        );
        // W let go: toward the right only.
        let d = shape(&run.key("W", false, now));
        assert_eq!(d, vec![(1, TouchAction::Move, 16000, 40000)]);
        // Opposite keys cancel: the finger stays down, in the middle.
        let both = shape(&run.key("A", true, now));
        assert_eq!(both, vec![(1, TouchAction::Move, 10000, 40000)]);
        // The last key let go lifts it, where it is (a thumb lifts from where it was).
        assert_eq!(
            shape(&run.key("A", false, now)),
            vec![(1, TouchAction::Move, 16000, 40000)]
        );
        let up = shape(&run.key("D", false, now));
        assert_eq!(up, vec![(1, TouchAction::Up, 16000, 40000)]);
        assert!(!run.busy());
    }

    #[test]
    fn a_stick_is_round_on_a_tall_picture() {
        let now = Instant::now();
        let mut m = Keymap::default();
        m.add_stick(p(32768, 32768)).unwrap();
        let mut run = Runner::new(m);
        // 500 wide and 1000 tall: the same pixels up as sideways, so twice the fraction sideways... of the height.
        run.set_picture(500.0, 1000.0);
        let up = shape(&run.key("W", true, now));
        let (_, _, _, y_up) = up[1];
        run.key("W", false, now);
        let right = shape(&run.key("D", true, now));
        let (_, _, x_right, _) = right[1];
        let up_px = (32768.0 - f32::from(y_up)) / 65535.0 * 1000.0;
        let right_px = (f32::from(x_right) - 32768.0) / 65535.0 * 500.0;
        assert!((up_px - right_px).abs() < 0.5, "{up_px} vs {right_px}");
    }

    #[test]
    fn a_swipe_goes_from_its_start_to_its_end_and_lifts() {
        let now = Instant::now();
        let mut m = Keymap::default();
        m.add_swipe("Q", p(20000, 20000), p(40000, 20000)).unwrap();
        m.set_swipe_ms(0, 200);
        let mut run = Runner::new(m);
        assert_eq!(
            shape(&run.key("Q", true, now)),
            vec![(1, TouchAction::Down, 20000, 20000)]
        );
        // Halfway: a move to the middle of the way. The key's release changes nothing.
        assert!(run.key("Q", false, now).is_empty());
        let half = shape(&run.tick(now + Duration::from_millis(100)));
        assert_eq!(half, vec![(1, TouchAction::Move, 30000, 20000)]);
        // Nothing moved since: nothing sent.
        assert!(run.tick(now + Duration::from_millis(100)).is_empty());
        // A key pressed again meanwhile starts no second swipe.
        assert!(
            run.key("Q", true, now + Duration::from_millis(120))
                .is_empty()
        );
        let end = shape(&run.tick(now + Duration::from_millis(250)));
        assert_eq!(
            end,
            vec![
                (1, TouchAction::Move, 40000, 20000),
                (1, TouchAction::Up, 40000, 20000),
            ]
        );
        assert!(!run.busy());
        // Pressed again after it ended: a new swipe.
        assert_eq!(
            run.key("Q", true, now + Duration::from_millis(300)).len(),
            1
        );
    }

    #[test]
    fn the_aim_is_a_finger_the_mouse_drags_and_lifts_when_still() {
        let now = Instant::now();
        let mut m = Keymap::default();
        m.add_aim(p(40000, 30000)).unwrap();
        m.bindings[0] = Binding::Aim {
            center: p(40000, 30000),
            radius: 8000,
            sensitivity: 100,
        };
        let mut run = Runner::new(m);
        run.set_picture(1000.0, 1000.0);
        // 10 px right, 5 px down: down in the middle, then there.
        let first = shape(&run.mouse(0.01, 0.005, now));
        assert_eq!(first[0], (1, TouchAction::Down, 40000, 30000));
        assert_eq!(first[1], (1, TouchAction::Move, 40655, 30328));
        // More of the same goes on from where it was.
        let next = shape(&run.mouse(0.01, 0.0, now + Duration::from_millis(5)));
        assert_eq!(next, vec![(1, TouchAction::Move, 41311, 30328)]);
        // Still for a while: lifted where it is.
        assert!(run.tick(now + Duration::from_millis(50)).is_empty());
        let lift = shape(&run.tick(now + Duration::from_millis(5) + AIM_IDLE));
        assert_eq!(lift, vec![(1, TouchAction::Up, 41311, 30328)]);
        assert!(!run.busy());
        // Moving again puts it down in the middle again.
        let again = shape(&run.mouse(0.001, 0.0, now + Duration::from_secs(1)));
        assert_eq!(again[0], (1, TouchAction::Down, 40000, 30000));
    }

    #[test]
    fn the_aim_runs_out_of_area_at_its_edge_and_starts_again_in_the_middle() {
        let now = Instant::now();
        let mut m = Keymap::default();
        m.add_aim(p(40000, 30000)).unwrap();
        m.bindings[0] = Binding::Aim {
            center: p(40000, 30000),
            radius: 6554,
            sensitivity: 100,
        };
        let mut run = Runner::new(m);
        run.set_picture(1000.0, 1000.0);
        // 100 px is the radius; 150 px right runs past it.
        let out = shape(&run.mouse(0.15, 0.0, now));
        assert_eq!(out[0], (1, TouchAction::Down, 40000, 30000));
        assert_eq!(out[1], (1, TouchAction::Move, 46554, 30000));
        assert_eq!(out[2], (1, TouchAction::Up, 46554, 30000));
        assert_eq!(out[3], (1, TouchAction::Down, 40000, 30000));
        assert_eq!(out.len(), 4);
        assert!(run.busy(), "the finger is down again in the middle");
    }

    #[test]
    fn a_stick_an_aim_and_taps_are_fingers_of_their_own() {
        let now = Instant::now();
        let mut run = Runner::new(map());
        run.set_picture(1000.0, 2000.0);
        let mut seen = Vec::new();
        seen.extend(shape(&run.key("W", true, now)));
        seen.extend(shape(&run.mouse(0.001, 0.001, now)));
        seen.extend(shape(&run.key("Space", true, now)));
        seen.extend(shape(&run.key(MOUSE_LEFT, true, now)));
        let downs: Vec<u8> = seen
            .iter()
            .filter(|e| e.1 == TouchAction::Down)
            .map(|e| e.0)
            .collect();
        assert_eq!(downs, vec![1, 2, 3, 4], "each its own finger");
        assert!(
            downs.iter().all(|f| (1..MAX_POINTERS).contains(f)),
            "never the mouse's own 0"
        );
        // A finger let go is the next one's.
        run.key("Space", false, now);
        let swipe = shape(&run.key("Q", true, now));
        assert_eq!(swipe[0].0, 3);
    }

    #[test]
    fn there_are_never_more_fingers_than_the_cap_allows() {
        let now = Instant::now();
        let mut m = Keymap::default();
        for i in 0..MAX_POINTERS + 3 {
            let _ = m.add_tap(&format!("F{}", i + 1), p(u16::from(i), 5));
        }
        let mut run = Runner::new(m);
        let mut downs = 0;
        for i in 0..MAX_POINTERS + 3 {
            downs += run.key(&format!("F{}", i + 1), true, now).len();
        }
        assert_eq!(downs, usize::from(MAX_POINTERS) - 1, "fingers 1 to 9");
    }

    #[test]
    fn everything_down_is_lifted_when_the_window_lets_go() {
        let now = Instant::now();
        let mut run = Runner::new(map());
        run.set_picture(1000.0, 1000.0);
        run.key("W", true, now);
        run.key("Space", true, now);
        run.mouse(0.001, 0.0, now);
        run.key("Q", true, now);
        let ups = shape(&run.release_all());
        assert_eq!(ups.len(), 4);
        assert!(ups.iter().all(|e| e.1 == TouchAction::Up));
        let order: Vec<u8> = ups.iter().map(|e| e.0).collect();
        let mut sorted = order.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(order, sorted, "the highest finger first");
        assert!(!run.busy());
        assert!(run.release_all().is_empty(), "twice lifts nothing");
        // A swipe or a key left over from before does not come back.
        assert!(run.tick(now + Duration::from_secs(5)).is_empty());
        // A changed keymap lifts what was down first.
        run.key("Space", true, now);
        let swap = shape(&run.set_map(Keymap::default()));
        assert_eq!(swap, vec![(1, TouchAction::Up, 60000, 50000)]);
        assert!(!run.listens("Space"));
    }
}
