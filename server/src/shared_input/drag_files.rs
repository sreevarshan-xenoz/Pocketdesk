//! Files dragged across the edge (BK4): a drag of files in Windows'
//! Explorer, carried with the cursor onto the computer driven and let go
//! there, sends them to that computer, where any file from it lands.
//!
//! Windows drops a drag where the cursor is as the button comes up, and
//! while the keyboard and mouse are away this computer's cursor is parked
//! at the centre of the screen it left (at an edge, the moves measured from
//! it would be cut short). So while a drag held here is carried away, a
//! small drop target of Pocketdesk's own, the catcher, sits under the
//! parked cursor: Windows' drag finds it there, and the button coming up
//! drops the files on it, not on whatever window lies under the park.
//! Coming home takes it away first, so a drag carried back drops where
//! Windows' own targets are.
//!
//! Pure: what the catcher does as the cursor parks, comes home and the
//! button comes up ([`Catcher`]), what a drop does ([`on_drop`]), the words
//! for the owner, and Esc during such a drag. The window, its drop target
//! and the sends are the Windows glue's (`drop_windows`, `capture_windows`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use pocketdesk_core::local::sent::{SentFile, SentState};

/// The catcher's side, in physical pixels, centred on the parked cursor.
pub const CATCHER_PX: i32 = 64;

/// How long the catcher stays after the button came up with no drag of
/// files over it: Windows' drag may not have looked at the cursor since it
/// was parked, and looks once more as the button comes up.
pub const SETTLE: Duration = Duration::from_millis(500);

/// How long, after the button came up with a drag of files over it, the
/// catcher waits for the drop before it goes anyway.
pub const DROP_WAIT: Duration = Duration::from_secs(3);

/// At most this many files and folders go from one drop, as from one copy
/// (P7).
pub const MOST_ITEMS: usize = 100;

/// Esc's virtual key.
pub const VK_ESCAPE: u32 = 0x1B;

/// Windows' `DROPEFFECT_NONE` and `DROPEFFECT_COPY`.
pub const EFFECT_NONE: u32 = 0;
pub const EFFECT_COPY: u32 = 1;

/// The owner's words when this computer's "Files over the internet" is off.
pub const FILES_OFF_HERE: &str =
    "Not sent: \"Files over the internet\" is off on this computer (Settings)";

/// What to do with the catcher's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catch {
    /// Put it up, centred on this point (physical).
    Show((i32, i32)),
    /// Take it down.
    Hide,
}

/// The catcher's window around `at` (physical): its left, top, width and
/// height.
pub fn rect(at: (i32, i32)) -> (i32, i32, i32, i32) {
    let half = CATCHER_PX / 2;
    (
        at.0.saturating_sub(half),
        at.1.saturating_sub(half),
        CATCHER_PX,
        CATCHER_PX,
    )
}

/// Where the catcher is, and since when the button that carried the drag
/// has been up.
#[derive(Debug, Default)]
pub struct Catcher {
    at: Option<(i32, i32)>,
    up_since: Option<Instant>,
}

impl Catcher {
    /// Whether the catcher is up.
    pub fn shown(&self) -> bool {
        self.at.is_some()
    }

    /// The cursor is about to be parked at `park` (physical) as it goes
    /// away: with the setting on and a mouse button held here (a drag
    /// carried along), the catcher goes up there first, so the drag finds
    /// it under the cursor as it lands there.
    pub fn parking(&mut self, park: (i32, i32), files_on: bool, held: bool) -> Option<Catch> {
        if !(files_on && held) || self.at == Some(park) {
            return None;
        }
        self.at = Some(park);
        self.up_since = None;
        Some(Catch::Show(park))
    }

    /// The keyboard and mouse are coming home: the catcher goes first, so
    /// a drag carried back drops on Windows' own.
    pub fn home(&mut self) -> Option<Catch> {
        self.up_since = None;
        self.at.take().map(|_| Catch::Hide)
    }

    /// A look at `now`: the catcher goes with the setting turned off, and
    /// once the button is up, after [`SETTLE`] with no drag of files
    /// over it (`files_over`), or after [`DROP_WAIT`] whatever.
    pub fn look(
        &mut self,
        files_on: bool,
        held: bool,
        files_over: bool,
        now: Instant,
    ) -> Option<Catch> {
        self.at?;
        if !files_on {
            return self.home();
        }
        if held {
            self.up_since = None;
            return None;
        }
        let up_for = now.saturating_duration_since(*self.up_since.get_or_insert(now));
        if (!files_over && up_for >= SETTLE) || up_for >= DROP_WAIT {
            return self.home();
        }
        None
    }
}

/// What the catcher answers a drag over it, whose source allows the
/// effects `allowed`: a copy for files (never a move, Shift held or not:
/// the files stay where they are) where a copy is allowed, nothing for
/// anything else.
pub fn effect(has_files: bool, allowed: u32) -> u32 {
    if has_files && allowed & EFFECT_COPY != 0 {
        EFFECT_COPY
    } else {
        EFFECT_NONE
    }
}

/// Whether Esc going down is left to Windows rather than sent to the
/// computer driven: while a drag of files is over the catcher, Esc is
/// Windows' own, and cancels the drag.
pub fn esc_cancels_drag(vk: u32, down: bool, files_over: bool) -> bool {
    files_over && down && vk == VK_ESCAPE
}

/// What a drop on the catcher does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dropped {
    /// Send these files and folders to that computer; `left_out` more were
    /// dropped than go at once.
    Send {
        to: String,
        items: Vec<PathBuf>,
        left_out: usize,
    },
    /// Nothing goes, with a word for the owner when there is one.
    Keep(Option<&'static str>),
}

/// `items` dropped on the catcher (read of the `total` the drop held),
/// with the setting (`files_on`), this computer's "Files over the
/// internet", and the computer driven now: sent to that computer, at most
/// [`MOST_ITEMS`] of them, the rest counted. Nothing goes at home (the drag
/// came back before the button came up) or with the setting off.
pub fn on_drop(
    mut items: Vec<PathBuf>,
    total: usize,
    files_on: bool,
    internet_files: bool,
    away_on: Option<&str>,
) -> Dropped {
    let Some(to) = away_on.filter(|_| files_on && !items.is_empty()) else {
        return Dropped::Keep(None);
    };
    if !internet_files {
        return Dropped::Keep(Some(FILES_OFF_HERE));
    }
    items.truncate(MOST_ITEMS);
    let left_out = total.max(items.len()) - items.len();
    Dropped::Send {
        to: to.to_string(),
        items,
        left_out,
    }
}

/// "1 file", "3 files".
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The owner's words for a drop sent on its way: what goes, to which
/// computer, and how many were left out.
pub fn sending(files: usize, folders: usize, to: &str, left_out: usize) -> String {
    let what = match (files, folders) {
        (_, 0) => count(files, "file", "files"),
        (0, _) => count(folders, "folder", "folders"),
        _ => format!(
            "{} and {}",
            count(folders, "folder", "folders"),
            count(files, "file", "files")
        ),
    };
    let mut words = format!("Sending {what} to {to}");
    if left_out > 0 {
        words.push_str(&format!(
            "\n{left_out} more left out: at most {MOST_ITEMS} go at once"
        ));
    }
    words
}

/// The owner's words for a drop that did not reach `to`.
pub fn not_sent(to: &str, reason: &str) -> String {
    format!("Not sent to {to}: {reason}")
}

/// How a drop's sends (`ids`, the outbox requests) stand in the sent list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Some are not done yet.
    Waiting,
    /// All went.
    Sent,
    /// One did not go, and why (the first, in the drop's order).
    Failed(String),
}

/// How the sends `ids` stand in `rows`, the sent list.
pub fn outcome(rows: &[SentFile], ids: &[String]) -> Outcome {
    let state = |id: &String| rows.iter().find(|r| r.id == *id).map(|r| &r.state);
    if let Some(reason) = ids.iter().find_map(|id| match state(id) {
        Some(SentState::Failed(reason)) => Some(reason.clone()),
        _ => None,
    }) {
        return Outcome::Failed(reason);
    }
    if ids.iter().all(|id| state(id) == Some(&SentState::Sent)) {
        Outcome::Sent
    } else {
        Outcome::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARK: (i32, i32) = (2688, 480);

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn the_catcher_sits_centred_on_the_park() {
        assert_eq!(rect(PARK), (2656, 448, 64, 64));
        assert_eq!(rect((0, 0)), (-32, -32, 64, 64));
        // A monitor left of or above the main one has negative points.
        assert_eq!(rect((-960, -540)), (-992, -572, 64, 64));
    }

    /// The catcher goes up as the cursor parks only for a drag carried
    /// along (a button held here) with the setting on, once.
    #[test]
    fn the_catcher_goes_up_only_for_a_drag_carried_away() {
        let mut c = Catcher::default();
        assert_eq!(c.parking(PARK, true, false), None, "no button held");
        assert_eq!(c.parking(PARK, false, true), None, "the setting off");
        assert!(!c.shown());
        assert_eq!(c.parking(PARK, true, true), Some(Catch::Show(PARK)));
        assert!(c.shown());
        // Moving on to a third computer parks at the same point: once.
        assert_eq!(c.parking(PARK, true, true), None);
        // A park elsewhere (the screens changed) moves it there.
        assert_eq!(
            c.parking((960, 540), true, true),
            Some(Catch::Show((960, 540)))
        );
    }

    /// Coming home takes it down first, every way home; and only once.
    #[test]
    fn coming_home_takes_the_catcher_down() {
        let mut c = Catcher::default();
        assert_eq!(c.home(), None, "nothing up");
        c.parking(PARK, true, true);
        assert_eq!(c.home(), Some(Catch::Hide));
        assert!(!c.shown());
        assert_eq!(c.home(), None);
        // Carried away again, it goes up again.
        assert_eq!(c.parking(PARK, true, true), Some(Catch::Show(PARK)));
    }

    /// While the button is held it stays, however long.
    #[test]
    fn the_catcher_stays_while_the_button_is_held() {
        let start = Instant::now();
        let mut c = Catcher::default();
        assert_eq!(c.look(true, true, false, start), None, "nothing up");
        c.parking(PARK, true, true);
        for ms in [0, 400, 5_000, 60_000] {
            assert_eq!(c.look(true, true, false, at(start, ms)), None);
            assert_eq!(c.look(true, true, true, at(start, ms)), None);
        }
        assert!(c.shown());
    }

    /// The button up with no drag of files over it (a window moved, text
    /// dragged, a drop already done): the catcher waits a moment for
    /// Windows' last look, then goes.
    #[test]
    fn the_button_up_with_no_files_over_takes_it_down_after_a_moment() {
        let start = Instant::now();
        let mut c = Catcher::default();
        c.parking(PARK, true, true);
        assert_eq!(c.look(true, false, false, start), None);
        assert_eq!(c.look(true, false, false, at(start, 499)), None);
        assert_eq!(
            c.look(true, false, false, at(start, 500)),
            Some(Catch::Hide)
        );
        assert_eq!(c.look(true, false, false, at(start, 600)), None, "once");
    }

    /// The button up with files over it waits for their drop (which clears
    /// "over"), or goes after a few seconds whatever.
    #[test]
    fn the_button_up_with_files_over_waits_for_the_drop() {
        let start = Instant::now();
        let mut c = Catcher::default();
        c.parking(PARK, true, true);
        assert_eq!(c.look(true, false, true, start), None);
        assert_eq!(c.look(true, false, true, at(start, 2_000)), None);
        assert_eq!(
            c.look(true, false, false, at(start, 2_100)),
            Some(Catch::Hide)
        );

        let mut stuck = Catcher::default();
        stuck.parking(PARK, true, true);
        assert_eq!(stuck.look(true, false, true, start), None);
        assert_eq!(stuck.look(true, false, true, at(start, 2_999)), None);
        assert_eq!(
            stuck.look(true, false, true, at(start, 3_000)),
            Some(Catch::Hide),
            "no drop came"
        );
    }

    /// A press again before the moment is out starts it over: the button
    /// up counts from its last coming up.
    #[test]
    fn the_button_held_again_starts_the_wait_over() {
        let start = Instant::now();
        let mut c = Catcher::default();
        c.parking(PARK, true, true);
        assert_eq!(c.look(true, false, false, start), None);
        assert_eq!(c.look(true, true, false, at(start, 300)), None);
        assert_eq!(c.look(true, false, false, at(start, 600)), None);
        assert_eq!(c.look(true, false, false, at(start, 1_099)), None);
        assert_eq!(
            c.look(true, false, false, at(start, 1_100)),
            Some(Catch::Hide)
        );
    }

    /// The setting turned off mid-drag takes it down at once.
    #[test]
    fn the_setting_turned_off_takes_it_down() {
        let start = Instant::now();
        let mut c = Catcher::default();
        c.parking(PARK, true, true);
        assert_eq!(c.look(false, true, true, start), Some(Catch::Hide));
        assert!(!c.shown());
    }

    /// Files are taken as a copy, never a move, and only where the drag's
    /// source allows a copy; anything else not at all.
    #[test]
    fn files_are_taken_as_a_copy_and_nothing_else_is_taken() {
        const MOVE: u32 = 2;
        const LINK: u32 = 4;
        assert_eq!(effect(true, EFFECT_COPY | MOVE | LINK), EFFECT_COPY);
        assert_eq!(effect(true, EFFECT_COPY), EFFECT_COPY);
        assert_eq!(effect(true, MOVE), EFFECT_NONE, "a move only");
        assert_eq!(effect(true, MOVE | LINK), EFFECT_NONE);
        assert_eq!(effect(true, EFFECT_NONE), EFFECT_NONE);
        assert_eq!(effect(false, EFFECT_COPY | MOVE), EFFECT_NONE);
    }

    /// Esc is Windows' own only going down while files are over the
    /// catcher; otherwise it goes to the computer driven as any key.
    #[test]
    fn esc_cancels_only_a_drag_of_files_over_the_catcher() {
        assert!(esc_cancels_drag(VK_ESCAPE, true, true));
        assert!(!esc_cancels_drag(VK_ESCAPE, true, false), "no drag");
        assert!(!esc_cancels_drag(VK_ESCAPE, false, true), "its key-up");
        assert!(!esc_cancels_drag(0x41, true, true), "another key");
    }

    fn items(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("C:\\Users\\me\\f{i}.txt")))
            .collect()
    }

    /// The decision table: what was dropped × the setting × this computer's
    /// "Files over the internet" × where the keyboard and mouse are.
    #[test]
    fn a_drop_goes_to_the_computer_driven_only() {
        assert_eq!(
            on_drop(items(2), 2, true, true, Some("L")),
            Dropped::Send {
                to: "L".into(),
                items: items(2),
                left_out: 0
            }
        );
        // Come home before the button came up: Windows' own.
        assert_eq!(on_drop(items(2), 2, true, true, None), Dropped::Keep(None));
        assert_eq!(on_drop(items(2), 2, true, false, None), Dropped::Keep(None));
        // The setting off (turned off mid-drag).
        assert_eq!(
            on_drop(items(2), 2, false, true, Some("L")),
            Dropped::Keep(None)
        );
        assert_eq!(
            on_drop(items(2), 2, false, false, Some("L")),
            Dropped::Keep(None)
        );
        // Nothing in it.
        assert_eq!(
            on_drop(vec![], 0, true, true, Some("L")),
            Dropped::Keep(None)
        );
        // Files over the internet off here: said why.
        assert_eq!(
            on_drop(items(2), 2, true, false, Some("L")),
            Dropped::Keep(Some(FILES_OFF_HERE))
        );
    }

    /// A hop on to a third computer before letting go sends to that one.
    #[test]
    fn a_drop_goes_to_whichever_computer_is_driven_as_it_lands() {
        assert!(matches!(
            on_drop(items(1), 1, true, true, Some("M")),
            Dropped::Send { to, .. } if to == "M"
        ));
    }

    /// At most a hundred go at once; the rest are counted, not sent: those
    /// never read (the drop reads only the first hundred of its count) as
    /// well as any read past the hundred.
    #[test]
    fn a_large_drop_sends_the_first_hundred() {
        let Dropped::Send {
            items: sent,
            left_out,
            ..
        } = on_drop(items(100), 130, true, true, Some("L"))
        else {
            panic!("sent");
        };
        assert_eq!(sent, items(100));
        assert_eq!(left_out, 30);
        let Dropped::Send {
            items: sent,
            left_out,
            ..
        } = on_drop(items(130), 130, true, true, Some("L"))
        else {
            panic!("sent");
        };
        assert_eq!(sent, items(100));
        assert_eq!(left_out, 30, "read past the hundred");
        // A count below what was read (it cannot be) counts nothing out.
        assert!(matches!(
            on_drop(items(3), 0, true, true, Some("L")),
            Dropped::Send { left_out: 0, .. }
        ));
    }

    #[test]
    fn the_owner_is_told_what_goes_where() {
        assert_eq!(sending(1, 0, "Laptop", 0), "Sending 1 file to Laptop");
        assert_eq!(sending(3, 0, "Laptop", 0), "Sending 3 files to Laptop");
        assert_eq!(sending(0, 1, "Laptop", 0), "Sending 1 folder to Laptop");
        assert_eq!(
            sending(2, 1, "Laptop", 0),
            "Sending 1 folder and 2 files to Laptop"
        );
        assert_eq!(
            sending(100, 0, "Laptop", 30),
            "Sending 100 files to Laptop\n30 more left out: at most 100 go at once"
        );
        assert_eq!(
            not_sent("Laptop", "refused: files are off for this computer"),
            "Not sent to Laptop: refused: files are off for this computer"
        );
    }

    fn row(id: &str, state: SentState) -> SentFile {
        SentFile {
            id: id.to_string(),
            name: "f.txt".to_string(),
            path: "C:\\f.txt".to_string(),
            size: 1,
            device_id: "L".to_string(),
            sent_unix: 0,
            state,
            note: None,
        }
    }

    /// A drop's sends are watched in the sent list: the first that failed
    /// is told; all sent is done; anything else waits.
    #[test]
    fn a_drops_sends_are_told_by_the_sent_list() {
        let ids = vec!["a".to_string(), "b".to_string()];
        assert_eq!(outcome(&[], &ids), Outcome::Waiting, "not taken yet");
        let offered = [row("a", SentState::Offered), row("b", SentState::Sent)];
        assert_eq!(outcome(&offered, &ids), Outcome::Waiting);
        let done = [
            row("x", SentState::Failed("another send's".into())),
            row("a", SentState::Sent),
            row("b", SentState::Sent),
        ];
        assert_eq!(outcome(&done, &ids), Outcome::Sent);
        let failed = [
            row("a", SentState::Offered),
            row("b", SentState::Failed("refused: busy".into())),
        ];
        assert_eq!(
            outcome(&failed, &ids),
            Outcome::Failed("refused: busy".into())
        );
        let both = [
            row("b", SentState::Failed("second".into())),
            row("a", SentState::Failed("first".into())),
        ];
        assert_eq!(
            outcome(&both, &ids),
            Outcome::Failed("first".into()),
            "in the drop's order"
        );
    }
}
