//! What the shared keyboard and mouse knows of the screens of the owner's
//! computers.
//!
//! The server keeps `<state>/shared_screens.json` (owner-only): this
//! computer's own screens first, then each paired computer's as it last
//! said them on the link (its `Hello` or `Screens`). Only the server writes
//! it; the layout is built from it. What another computer says is its to
//! claim, so it is clipped here before it is kept ([`clean_name`],
//! [`clean_screens`]). It holds only computers paired now: the server
//! drops the others each time it writes ([`ScreensFile::keep_only`]).
//!
//! The desktop app's Keyboard and mouse page keeps
//! `<state>/shared_layout.json` ([`LayoutFile`]): where the owner put each
//! computer, which the server's driver uses in place of the default row
//! ([`arrange`]). The server keeps `<state>/shared_input_status.json`
//! ([`StatusFile`]) for the page: where the keyboard and mouse are, and how
//! each computer's link stands. The page asks the server through
//! `<state>/shared_input_out/<id>.json`, one request per file
//! ([`ShareAsk`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::{
    SHARED_INPUT_DIR, SHARED_INPUT_STATUS_FILE, SHARED_LAYOUT_FILE, SHARED_SCREENS_FILE,
};

/// The longest name kept or sent for a computer, in bytes; a longer one is
/// cut short at a character.
pub const MAX_NAME_BYTES: usize = 255;

/// The most screens kept or sent for one computer; the rest are left off.
pub const MAX_SCREENS: usize = 64;

/// A monitor, in its computer's own desktop space and logical pixels (what a
/// 125% display shows, not what it has).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Screen {
    /// Whether it has an area and its far edges are still in `i32` (so
    /// `x + width` and `y + height` never wrap).
    pub fn is_sound(&self) -> bool {
        let fits = |start: i32, len: u32| {
            len > 0
                && i32::try_from(len)
                    .ok()
                    .and_then(|len| start.checked_add(len))
                    .is_some()
        };
        fits(self.x, self.width) && fits(self.y, self.height)
    }
}

/// `name` cut to at most [`MAX_NAME_BYTES`] at a character, without its
/// control characters.
pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let mut end = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

/// `screens` without the unsound ones ([`Screen::is_sound`]), at most
/// [`MAX_SCREENS`] of them.
pub fn clean_screens(screens: &[Screen]) -> Vec<Screen> {
    screens
        .iter()
        .filter(|s| s.is_sound())
        .take(MAX_SCREENS)
        .copied()
        .collect()
}

/// One computer's screens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerScreens {
    pub device_id: String,
    pub name: String,
    pub screens: Vec<Screen>,
    /// When its name or screens last changed, in Unix seconds.
    pub changed_unix: u64,
    /// Whether this is this computer's own entry (the first).
    #[serde(default)]
    pub this_computer: bool,
}

/// This computer's screens first, then the other computers' in the order
/// they were first heard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreensFile {
    #[serde(default)]
    pub computers: Vec<ComputerScreens>,
}

impl ScreensFile {
    /// This computer's entry, if it has one.
    pub fn this_computer(&self) -> Option<&ComputerScreens> {
        self.computers.first().filter(|c| c.this_computer)
    }

    /// The computer `device_id`'s entry.
    pub fn get(&self, device_id: &str) -> Option<&ComputerScreens> {
        self.computers.iter().find(|c| c.device_id == device_id)
    }

    /// Set this computer's own screens, as the first entry.
    pub fn set_own(&mut self, device_id: &str, name: &str, screens: &[Screen], now_unix: u64) {
        let unchanged = self.this_computer().is_some_and(|own| {
            own.device_id == device_id && own.name == name && own.screens == screens
        });
        if unchanged {
            return;
        }
        self.computers
            .retain(|c| !c.this_computer && c.device_id != device_id);
        self.computers.insert(
            0,
            ComputerScreens {
                device_id: device_id.to_string(),
                name: clean_name(name),
                screens: clean_screens(screens),
                changed_unix: now_unix,
                this_computer: true,
            },
        );
    }

    /// Keep what the computer `device_id` said of itself: its screens, and
    /// its name when it said one. Both are clipped first. A computer that
    /// names this one's own id is ignored.
    pub fn heard(
        &mut self,
        device_id: &str,
        name: Option<&str>,
        screens: &[Screen],
        now_unix: u64,
    ) {
        let screens = clean_screens(screens);
        let name = name.map(clean_name);
        match self.computers.iter_mut().find(|c| c.device_id == device_id) {
            Some(own) if own.this_computer => {}
            Some(known) => {
                let name = name.unwrap_or_else(|| known.name.clone());
                if known.name != name || known.screens != screens {
                    known.name = name;
                    known.screens = screens;
                    known.changed_unix = now_unix;
                }
            }
            None => self.computers.push(ComputerScreens {
                device_id: device_id.to_string(),
                name: name.unwrap_or_default(),
                screens,
                changed_unix: now_unix,
                this_computer: false,
            }),
        }
    }

    /// Drop every computer not in `paired` (unpaired or forgotten since it
    /// was heard); this computer's own entry stays.
    pub fn keep_only(&mut self, paired: &BTreeSet<String>) {
        self.computers
            .retain(|c| c.this_computer || paired.contains(&c.device_id));
    }
}

/// The screens known. A missing or unreadable file is empty.
pub fn read(state_dir: &Path) -> ScreensFile {
    try_read(state_dir).unwrap_or_default()
}

/// [`read`], but a file that is there and does not read is an error.
pub fn try_read(state_dir: &Path) -> io::Result<ScreensFile> {
    match std::fs::read_to_string(state_dir.join(SHARED_SCREENS_FILE)) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ScreensFile::default()),
        Err(e) => Err(e),
    }
}

/// The server's writes, one at a time.
static WRITING: Mutex<()> = Mutex::new(());

/// Change the file under `change` (the server's side), written whole and
/// owner-only, and only when it changed. A file that does not read is
/// started again: it holds nothing that is not heard again.
pub fn update(state_dir: &Path, change: impl FnOnce(&mut ScreensFile)) -> io::Result<()> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = state_dir.join(SHARED_SCREENS_FILE);
    let mut file = try_read(state_dir).unwrap_or_default();
    let before = file.clone();
    change(&mut file);
    if file == before && path.exists() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `bytes` as `name` in `state_dir`, written whole (a reader never sees it
/// half-written) and owner-only.
fn write_whole(state_dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let path = state_dir.join(name);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, bytes)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Where the owner put each computer (the Keyboard and mouse page's
/// arrangement): each computer's origin, by its id, in the logical pixels
/// of the shared layout space, where this computer's own desktop space
/// starts at (0, 0). Written whole by the desktop app; `rev` counts its
/// saves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayoutFile {
    #[serde(default)]
    pub origins: BTreeMap<String, (i32, i32)>,
    #[serde(default)]
    pub rev: u64,
}

/// The owner's arrangement, if there is one: a missing file, or one that
/// does not read, is none (the default row is used).
pub fn read_layout(state_dir: &Path) -> Option<LayoutFile> {
    let raw = std::fs::read_to_string(state_dir.join(SHARED_LAYOUT_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Keep the owner's arrangement (the app's side), whole and owner-only.
pub fn write_layout(state_dir: &Path, layout: &LayoutFile) -> io::Result<()> {
    let json = serde_json::to_string_pretty(layout).map_err(io::Error::other)?;
    write_whole(state_dir, SHARED_LAYOUT_FILE, json.as_bytes())
}

/// A box of pixels: from `x0`, `y0` up to but not including `x1`, `y1`.
/// Wide enough that sums of `i32`s never overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Bounds {
    /// `s`'s pixels.
    pub fn of(s: &Screen) -> Bounds {
        let (x0, y0) = (i64::from(s.x), i64::from(s.y));
        Bounds {
            x0,
            y0,
            x1: x0 + i64::from(s.width),
            y1: y0 + i64::from(s.height),
        }
    }

    /// Moved by `(dx, dy)`.
    pub fn shifted(self, (dx, dy): (i32, i32)) -> Bounds {
        let (dx, dy) = (i64::from(dx), i64::from(dy));
        Bounds {
            x0: self.x0 + dx,
            y0: self.y0 + dy,
            x1: self.x1 + dx,
            y1: self.y1 + dy,
        }
    }

    pub fn width(&self) -> i64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i64 {
        self.y1 - self.y0
    }

    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }

    /// Whether the two share any pixel (touching edges share none).
    pub fn overlaps(&self, o: &Bounds) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }

    fn union(self, o: Bounds) -> Bounds {
        Bounds {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// The box around `screens`, in their computer's own desktop space; none
/// for a computer with no screens.
pub fn bounds(screens: &[Screen]) -> Option<Bounds> {
    screens
        .iter()
        .map(Bounds::of)
        .filter(|b| !b.is_empty())
        .reduce(Bounds::union)
}

/// Where each of `others` sits (its origin), with this computer's own
/// desktop space at (0, 0) and its screens `this`: `saved`'s origins when
/// it has them and no two computers' screens then overlap, and the default
/// row otherwise (each to the right of the one before, tops level with this
/// computer's). One `saved` leaves out goes to the right of the rest.
pub fn arrange(
    this: &[Screen],
    others: &[(String, Vec<Screen>)],
    saved: Option<&LayoutFile>,
) -> Vec<(i32, i32)> {
    let row = |placed: &[(Vec<Screen>, (i32, i32))], from: &[&Vec<Screen>]| {
        // The right of what is placed already, and this computer's top.
        let mut right = placed
            .iter()
            .filter_map(|(s, origin)| bounds(s).map(|b| b.shifted(*origin).x1))
            .max()
            .unwrap_or(0);
        let top = bounds(this).map_or(0, |b| b.y0);
        from.iter()
            .map(|screens| {
                let (left, other_top, width) =
                    bounds(screens).map_or((0, 0, 0), |b| (b.x0, b.y0, b.width()));
                let origin = (clamp_i32(right - left), clamp_i32(top - other_top));
                right += width;
                origin
            })
            .collect::<Vec<_>>()
    };
    let me = vec![(this.to_vec(), (0, 0))];
    let default = row(&me, &others.iter().map(|(_, s)| s).collect::<Vec<_>>());
    let Some(saved) = saved else {
        return default;
    };
    let mut origins: Vec<Option<(i32, i32)>> = others
        .iter()
        .map(|(id, _)| saved.origins.get(id).copied())
        .collect();
    let mut placed = me;
    placed.extend(
        others
            .iter()
            .zip(&origins)
            .filter_map(|((_, s), o)| o.map(|o| (s.clone(), o))),
    );
    let missing: Vec<&Vec<Screen>> = others
        .iter()
        .zip(&origins)
        .filter(|(_, o)| o.is_none())
        .map(|((_, s), _)| s)
        .collect();
    let mut appended = row(&placed, &missing).into_iter();
    for origin in origins.iter_mut().filter(|o| o.is_none()) {
        *origin = appended.next();
    }
    let origins: Vec<(i32, i32)> = origins.into_iter().flatten().collect();
    let mut all: Vec<Bounds> = this.iter().map(Bounds::of).collect();
    for ((_, screens), origin) in others.iter().zip(&origins) {
        let mine: Vec<Bounds> = screens
            .iter()
            .map(|s| Bounds::of(s).shifted(*origin))
            .collect();
        if mine.iter().any(|b| all.iter().any(|o| b.overlaps(o))) {
            return default;
        }
        all.extend(mine);
    }
    origins
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A computer's link, as [`StatusFile::link`] says it.
pub const LINK_DIRECT: &str = "direct";
pub const LINK_RELAYED: &str = "relayed";
pub const LINK_AWAY: &str = "away";
/// It answered this computer's hello by refusing: shared input is off
/// there.
pub const REFUSED_SHARING_OFF: &str = "refused: sharing off";
/// It answered by refusing: it does not let this computer drive it.
pub const REFUSED_NOT_ALLOWED: &str = "refused: not allowed";

/// How the shared keyboard and mouse stand now, for the desktop app.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    /// The computer this one's keyboard and mouse are on now; empty while
    /// they are here.
    #[serde(default)]
    pub keyboard_on: String,
    /// Each paired computer's link: [`LINK_DIRECT`], [`LINK_RELAYED`],
    /// [`LINK_AWAY`] (none), or a refusal ([`REFUSED_SHARING_OFF`],
    /// [`REFUSED_NOT_ALLOWED`]).
    #[serde(default)]
    pub link: BTreeMap<String, String>,
    /// The computers that answered this one's hello with their screens
    /// since their link last opened: they let it drive them.
    #[serde(default)]
    pub answered: BTreeSet<String>,
}

/// How things stand, as the server last wrote. A missing or unreadable file
/// is nothing known.
pub fn read_status(state_dir: &Path) -> StatusFile {
    std::fs::read_to_string(state_dir.join(SHARED_INPUT_STATUS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Keep how things stand (the server's side), whole and owner-only.
pub fn write_status(state_dir: &Path, status: &StatusFile) -> io::Result<()> {
    let json = serde_json::to_string_pretty(status).map_err(io::Error::other)?;
    write_whole(state_dir, SHARED_INPUT_STATUS_FILE, json.as_bytes())
}

/// How long a request waits for the server before it is dropped.
pub const REQUEST_TTL_SECS: u64 = 30;

/// What the app asks of the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum ShareAsk {
    /// Have each computer this one can drive now show who it is.
    Identify,
}

/// One request, as a file in `shared_input_out/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareRequest {
    pub id: String,
    #[serde(flatten)]
    pub ask: ShareAsk,
    pub created_unix: u64,
}

/// Leave a request for the server (the app's side).
pub fn request(state_dir: &Path, ask: ShareAsk, now_unix: u64) -> io::Result<ShareRequest> {
    let request = ShareRequest {
        id: uuid::Uuid::new_v4().to_string(),
        ask,
        created_unix: now_unix,
    };
    let file = request_path(state_dir, &request.id)?;
    std::fs::create_dir_all(state_dir.join(SHARED_INPUT_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(request)
}

/// Take every waiting request (the server's side), oldest first. Each file
/// is removed once read; one that is stale, from the future, unreadable or
/// named for another request is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<ShareRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(SHARED_INPUT_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue; // a temp file mid-write
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<ShareRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= REQUEST_TTL_SECS;
        if fresh && request.id == stem && is_valid_id(&request.id) {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn request_path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a request id",
        ));
    }
    Ok(state_dir.join(SHARED_INPUT_DIR).join(format!("{id}.json")))
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

    #[test]
    fn what_a_peer_says_is_clipped() {
        let many = vec![screen(0, 0, 10, 10); 500];
        assert_eq!(clean_screens(&many).len(), MAX_SCREENS);
        // No area, or a far edge past i32: dropped.
        let odd = [
            screen(0, 0, 0, 10),
            screen(0, 0, 10, 0),
            screen(i32::MAX, 0, 1, 1),
            screen(0, i32::MAX - 5, 1, 10),
            screen(0, 0, u32::MAX, 1),
            screen(-10, -10, 10, 10),
        ];
        assert_eq!(clean_screens(&odd), vec![screen(-10, -10, 10, 10)]);

        let long = "\u{4e2d}".repeat(200);
        let name = clean_name(&long);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.chars().all(|c| c == '\u{4e2d}'));
        assert_eq!(clean_name("Desk\n\u{7}top"), "Desktop");
    }

    #[test]
    fn this_computer_stays_first() {
        let mut file = ScreensFile::default();
        file.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 5);
        file.set_own("me", "Desk", &[screen(0, 0, 1920, 1080)], 6);
        assert_eq!(
            file.this_computer().map(|c| c.device_id.as_str()),
            Some("me")
        );
        assert_eq!(file.computers.len(), 2);
        file.heard("other", None, &[], 7);
        file.set_own("me", "Desk", &[screen(0, 0, 2560, 1440)], 8);
        let ids: Vec<&str> = file
            .computers
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        assert_eq!(ids, ["me", "peer", "other"]);
        assert_eq!(file.computers[0].changed_unix, 8);
        // A peer claiming this computer's id changes nothing.
        file.heard("me", Some("Impostor"), &[], 9);
        assert_eq!(file.computers[0].name, "Desk");
    }

    #[test]
    fn a_peer_is_changed_in_place_and_keeps_its_name() {
        let mut file = ScreensFile::default();
        file.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 5);
        // The same again is not a change.
        file.heard("peer", None, &[screen(0, 0, 1600, 1000)], 6);
        assert_eq!(file.get("peer").unwrap().changed_unix, 5);
        // New screens without a name keep the name it had.
        file.heard("peer", None, &[screen(0, 0, 1280, 800)], 7);
        let peer = file.get("peer").unwrap();
        assert_eq!(peer.name, "Laptop");
        assert_eq!(peer.screens, vec![screen(0, 0, 1280, 800)]);
        assert_eq!(peer.changed_unix, 7);
    }

    #[test]
    fn only_paired_computers_are_kept() {
        let mut file = ScreensFile::default();
        file.set_own("me", "Desk", &[screen(0, 0, 1920, 1080)], 1);
        file.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 2);
        file.heard("gone", Some("Old"), &[screen(0, 0, 800, 600)], 3);
        file.keep_only(&BTreeSet::from(["peer".to_string()]));
        let ids: Vec<&str> = file
            .computers
            .iter()
            .map(|c| c.device_id.as_str())
            .collect();
        // This computer always stays.
        assert_eq!(ids, ["me", "peer"]);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-shared-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn layout_file_round_trips() {
        let dir = temp("layout");
        assert_eq!(read_layout(&dir), None, "none written yet");
        let layout = LayoutFile {
            origins: BTreeMap::from([
                ("me".to_string(), (0, 0)),
                ("peer".to_string(), (-1600, 80)),
            ]),
            rev: 3,
        };
        write_layout(&dir, &layout).unwrap();
        assert_eq!(read_layout(&dir), Some(layout));
        // One that does not read is none, as if missing.
        std::fs::write(dir.join(SHARED_LAYOUT_FILE), "{not json").unwrap();
        assert_eq!(read_layout(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_layout_uses_the_default() {
        let this = [screen(0, 0, 1920, 1080), screen(1920, 0, 1536, 960)];
        let laptop = vec![screen(0, 0, 1600, 1000)];
        let other = vec![screen(-100, 50, 1280, 800)];
        let others = vec![("L".to_string(), laptop), ("M".to_string(), other)];
        // A row to the right, tops level with this computer's.
        let row = vec![(3456, 0), (3456 + 1600 + 100, -50)];
        assert_eq!(arrange(&this, &others, None), row);
        // A file that places each where it does not overlap is used.
        let saved = LayoutFile {
            origins: BTreeMap::from([
                ("L".to_string(), (-1600, 0)),
                ("M".to_string(), (100, 1080)),
            ]),
            rev: 1,
        };
        assert_eq!(
            arrange(&this, &others, Some(&saved)),
            vec![(-1600, 0), (100, 1080)]
        );
        // One the file leaves out goes to the right of the rest.
        let partial = LayoutFile {
            origins: BTreeMap::from([("L".to_string(), (-1600, 0))]),
            rev: 1,
        };
        assert_eq!(
            arrange(&this, &others, Some(&partial)),
            vec![(-1600, 0), (3456 + 100, -50)]
        );
        // One that would put two computers' screens over each other is not.
        let overlapping = LayoutFile {
            origins: BTreeMap::from([("L".to_string(), (100, 100)), ("M".to_string(), (0, 0))]),
            rev: 1,
        };
        assert_eq!(arrange(&this, &others, Some(&overlapping)), row);
    }

    #[test]
    fn status_file_round_trips() {
        let dir = temp("status");
        assert_eq!(read_status(&dir), StatusFile::default());
        let status = StatusFile {
            keyboard_on: "peer".to_string(),
            link: BTreeMap::from([
                ("peer".to_string(), "direct".to_string()),
                ("other".to_string(), REFUSED_SHARING_OFF.to_string()),
            ]),
            answered: BTreeSet::from(["peer".to_string()]),
        };
        write_status(&dir, &status).unwrap();
        assert_eq!(read_status(&dir), status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_identify_request_is_taken_once() {
        let dir = temp("asks");
        assert!(take_all(&dir, 100).is_empty());
        let asked = request(&dir, ShareAsk::Identify, 100).unwrap();
        assert_eq!(take_all(&dir, 101), [asked]);
        assert!(take_all(&dir, 101).is_empty(), "taken");
        // A stale one is dropped, not left behind.
        request(&dir, ShareAsk::Identify, 100).unwrap();
        assert!(take_all(&dir, 100 + REQUEST_TTL_SECS + 1).is_empty());
        assert!(
            std::fs::read_dir(dir.join(SHARED_INPUT_DIR))
                .unwrap()
                .next()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn written_whole_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pd-shared-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read(&dir), ScreensFile::default());
        update(&dir, |f| {
            f.set_own("me", "Desk", &[screen(0, 0, 1920, 1080)], 1)
        })
        .unwrap();
        update(&dir, |f| {
            f.heard("peer", Some("Laptop"), &[screen(0, 0, 1600, 1000)], 2)
        })
        .unwrap();
        let back = read(&dir);
        assert_eq!(back.computers.len(), 2);
        assert!(back.computers[0].this_computer);
        // A file that does not read is started again.
        std::fs::write(dir.join(SHARED_SCREENS_FILE), "{not json").unwrap();
        update(&dir, |f| f.set_own("me", "Desk", &[], 3)).unwrap();
        assert_eq!(read(&dir).computers.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
