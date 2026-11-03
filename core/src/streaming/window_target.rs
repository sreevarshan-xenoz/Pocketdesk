//! A window of this computer as what a session captures (window handoff,
//! phase 5: Windows as the source).
//!
//! A session names what it shows by a monitor index. A window lent to another
//! computer (the server's `lend_window`) has no monitor, so the server
//! registers the window here and gives the borrowing computer an index past
//! every monitor's ([`FIRST_INDEX`] on): the borrower dials that index as it
//! dials any screen's, and the session that starts for it finds the window by
//! the index ([`hwnd`]) and captures that window alone, and lands the pointer
//! in its frame (the streamer's `PointerSource::Window`). Nothing on the
//! borrowing side knows the difference.
//!
//! A session bound to a window never falls back to a monitor: the lend is made
//! for one window, and a monitor would show whatever its owner has open. An
//! index in this range that no window is registered under (the window was
//! given back) captures nothing and places no input. The capture ends when the
//! window closes.
//!
//! The table is the only thing here that is the same on every system, so it is
//! tested everywhere; the two calls that reach Windows ([`frame_rect`],
//! [`raise`]) answer nothing off Windows.

use std::sync::Mutex;

/// The first monitor index that names a lent window rather than a monitor. No
/// computer has a thousand monitors.
pub const FIRST_INDEX: u32 = 1000;

/// How many windows may be registered at once.
pub const MOST: u32 = 8;

/// A rectangle as `(x, y, width, height)` in the desktop's pixels.
pub type Rect = (i32, i32, u32, u32);

/// The windows registered, each under the index its session captures it by.
#[derive(Default)]
struct Table(Vec<(u32, isize)>);

impl Table {
    fn register(&mut self, hwnd: isize) -> Option<u32> {
        if hwnd == 0 {
            return None;
        }
        if let Some((index, _)) = self.0.iter().find(|(_, h)| *h == hwnd) {
            return Some(*index);
        }
        let index =
            (FIRST_INDEX..FIRST_INDEX + MOST).find(|i| !self.0.iter().any(|(t, _)| t == i))?;
        self.0.push((index, hwnd));
        Some(index)
    }

    fn unregister(&mut self, index: u32) {
        self.0.retain(|(i, _)| *i != index);
    }

    fn hwnd(&self, index: u32) -> Option<isize> {
        self.0.iter().find(|(i, _)| *i == index).map(|(_, h)| *h)
    }
}

static TABLE: Mutex<Table> = Mutex::new(Table(Vec::new()));

fn table() -> std::sync::MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether `index` is one a registered window may have (it names no monitor).
pub fn is_window_index(index: u32) -> bool {
    (FIRST_INDEX..FIRST_INDEX + MOST).contains(&index)
}

/// Register the window `hwnd` and give the index a session captures it by: the
/// one it already has, or the lowest free; `None` when all [`MOST`] are used or
/// `hwnd` is no handle.
pub fn register(hwnd: isize) -> Option<u32> {
    table().register(hwnd)
}

/// Forget the window registered under `index`.
pub fn unregister(index: u32) {
    table().unregister(index);
}

/// The window registered under `index`, if any.
pub fn hwnd(index: u32) -> Option<isize> {
    table().hwnd(index)
}

/// The frame of the window registered under `index` in the desktop's pixels,
/// as the capture shows it (DWM's frame without its shadow): `None` off
/// Windows, for a window that is gone or one with no size.
pub fn frame_rect(index: u32) -> Option<Rect> {
    frame_of(hwnd(index)?)
}

/// Bring the window registered under `index` to the front, so a click lands on
/// it and not on what covers it; best effort (Windows may refuse to give the
/// focus to a program that is not in front). Nothing off Windows.
pub fn raise(index: u32) {
    if let Some(hwnd) = hwnd(index) {
        raise_hwnd(hwnd);
    }
}

/// The frame of the window `hwnd` as the capture shows it (see [`frame_rect`]).
#[cfg(windows)]
pub fn frame_of(hwnd: isize) -> Option<Rect> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    let mut rect = RECT::default();
    // SAFETY: a window handle (a stale one is refused, not followed) and an
    // out value of the size the attribute is told.
    unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    (width > 0 && height > 0).then_some((rect.left, rect.top, width as u32, height as u32))
}

/// The frame of the window `hwnd` as the capture shows it (see [`frame_rect`]).
#[cfg(not(windows))]
pub fn frame_of(_hwnd: isize) -> Option<Rect> {
    None
}

#[cfg(windows)]
fn raise_hwnd(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_TOP, IsIconic, SW_RESTORE, SWP_ASYNCWINDOWPOS, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
        SetForegroundWindow, SetWindowPos, ShowWindowAsync,
    };
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: calls on a window handle; a handle that is no window is
    // refused by each, and nothing is read through it. The asynchronous forms
    // are used so a window whose program has hung cannot hold this thread.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS,
        );
        let _ = SetForegroundWindow(hwnd);
    }
}

#[cfg(not(windows))]
fn raise_hwnd(_hwnd: isize) {}

/// The least time between two raises of one window.
pub const RAISE_EVERY: std::time::Duration = std::time::Duration::from_millis(300);

/// Whether input for a window session should bring its window forward now: a
/// press, a scroll or a key (the things that land on whichever window is in
/// front), and not again within [`RAISE_EVERY`] of the last time. Moves alone
/// never do: the pointer may pass over the window without the viewer meaning
/// to use it.
pub fn raise_due(
    last: Option<std::time::Instant>,
    now: std::time::Instant,
    acts_on_front_window: bool,
) -> bool {
    acts_on_front_window && last.is_none_or(|at| now.saturating_duration_since(at) >= RAISE_EVERY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    // The global table is shared by the tests running side by side: each of
    // these takes handles of its own and leaves none behind; the full-table
    // rules are tried on a table of their own.

    #[test]
    fn a_window_gets_an_index_past_every_monitor_and_keeps_it() {
        let first = register(0x7101).expect("a free index");
        assert!(is_window_index(first));
        assert!(first >= FIRST_INDEX);
        assert_eq!(
            register(0x7101),
            Some(first),
            "the same window, the same index"
        );
        assert_eq!(hwnd(first), Some(0x7101));
        unregister(first);
        assert_eq!(hwnd(first), None);
    }

    #[test]
    fn two_windows_never_share_an_index() {
        let a = register(0x7201).unwrap();
        let b = register(0x7202).unwrap();
        assert_ne!(a, b);
        assert_eq!(hwnd(a), Some(0x7201));
        assert_eq!(hwnd(b), Some(0x7202));
        unregister(a);
        unregister(b);
    }

    #[test]
    fn no_handle_is_no_window() {
        assert_eq!(register(0), None);
        assert_eq!(Table::default().register(0), None);
    }

    #[test]
    fn a_monitor_index_is_no_window_index() {
        for monitor in [0, 1, 3, 15, FIRST_INDEX - 1, FIRST_INDEX + MOST, u32::MAX] {
            assert!(!is_window_index(monitor), "{monitor}");
            assert_eq!(hwnd(monitor), None);
        }
        assert!(is_window_index(FIRST_INDEX));
        assert!(is_window_index(FIRST_INDEX + MOST - 1));
    }

    #[test]
    fn a_full_table_says_so_and_a_freed_index_is_used_again() {
        let mut table = Table::default();
        let indexes: Vec<u32> = (0..MOST as isize)
            .map(|n| table.register(0x7300 + n).expect("a free index"))
            .collect();
        let mut sorted = indexes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), MOST as usize, "every window its own index");
        assert!(sorted.iter().all(|i| is_window_index(*i)));
        assert_eq!(table.register(0x7399), None, "no index left");
        // A window already registered still finds its own, full or not.
        assert_eq!(table.register(0x7300), Some(indexes[0]));
        table.unregister(indexes[3]);
        assert_eq!(table.hwnd(indexes[3]), None);
        assert_eq!(table.register(0x7399), Some(indexes[3]), "the freed one");
    }

    #[test]
    fn off_windows_a_window_has_no_frame_and_raising_is_nothing() {
        // The calls that reach Windows answer nothing off it (and for an
        // unregistered index, nothing anywhere).
        assert_eq!(frame_rect(FIRST_INDEX + 5), None);
        raise(FIRST_INDEX + 5);
    }

    #[test]
    fn a_press_raises_once_in_a_while_and_a_move_never_does() {
        let start = Instant::now();
        assert!(raise_due(None, start, true));
        assert!(!raise_due(None, start, false), "a move");
        let later = start + RAISE_EVERY;
        assert!(!raise_due(
            Some(start),
            start + Duration::from_millis(100),
            true
        ));
        assert!(raise_due(Some(start), later, true));
        assert!(!raise_due(Some(start), later, false));
    }
}
