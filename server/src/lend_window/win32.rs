//! What lending a window asks of Windows: the real [`Desktop`]. Kept to the
//! calls themselves; every rule that can be tried without a desktop is in
//! [`super::desktop`] and tested there. This file is compiled, and its checks
//! run, on Windows only (the owner's live checks are listed in the
//! device checklist).

use std::io;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::window_target;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowPlacement, GetWindowRect, IsIconic, IsWindow, IsZoomed, SW_RESTORE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos,
    ShowWindowAsync, WINDOWPLACEMENT, WINDOWPLACEMENT_FLAGS,
};

use super::desktop::{Desktop, Facts, Placement, Show, needs_resize, outer_for_picture};
use crate::hyprland::HyprWindow;

/// How long a window is given to take a size or to come out of the taskbar.
const SETTLE: Duration = Duration::from_millis(1000);

/// How often the window is looked at while that is waited for.
const POLL: Duration = Duration::from_millis(25);

/// The windows of this desktop.
pub struct Win32;

fn handle(hwnd: isize) -> HWND {
    HWND(hwnd as *mut _)
}

fn is_window(hwnd: HWND) -> bool {
    // SAFETY: a handle is only asked about; one that is no window says no.
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

fn is_minimized(hwnd: HWND) -> bool {
    // SAFETY: as `is_window`.
    unsafe { IsIconic(hwnd) }.as_bool()
}

fn is_maximized(hwnd: HWND) -> bool {
    // SAFETY: as `is_window`.
    unsafe { IsZoomed(hwnd) }.as_bool()
}

fn read_placement(hwnd: HWND) -> io::Result<WINDOWPLACEMENT> {
    let mut wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // SAFETY: a window handle and an out value whose `length` is set, as the
    // call requires.
    unsafe { GetWindowPlacement(hwnd, &mut wp) }.map_err(io::Error::other)?;
    Ok(wp)
}

/// Wait up to [`SETTLE`] for `done` to hold; whether it did.
fn wait_until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

impl Desktop for Win32 {
    fn can_capture(&self) -> bool {
        windows::Graphics::Capture::GraphicsCaptureSession::IsSupported().unwrap_or(false)
    }

    fn windows(&self) -> io::Result<Vec<HyprWindow>> {
        Ok(crate::windows_desktop::visible_windows().0)
    }

    fn facts(&self, hwnd: isize) -> Option<Facts> {
        let h = handle(hwnd);
        if !is_window(h) {
            return None;
        }
        let minimized = is_minimized(h);
        let frame = if minimized {
            // A minimised window has no frame on the desktop: it has the
            // size it will have when shown.
            let n = read_placement(h).ok()?.rcNormalPosition;
            [n.left, n.top, n.right - n.left, n.bottom - n.top]
        } else {
            let (x, y, width, height) = window_target::frame_of(hwnd)?;
            [x, y, width as i32, height as i32]
        };
        (frame[2] > 0 && frame[3] > 0).then_some(Facts { frame, minimized })
    }

    fn placement(&self, hwnd: isize) -> io::Result<Placement> {
        let wp = read_placement(handle(hwnd))?;
        let n = wp.rcNormalPosition;
        Ok(Placement {
            show: Show::from_cmd(wp.showCmd),
            normal: [n.left, n.top, n.right, n.bottom],
            min_at: [wp.ptMinPosition.x, wp.ptMinPosition.y],
            max_at: [wp.ptMaxPosition.x, wp.ptMaxPosition.y],
            flags: wp.flags.0,
        })
    }

    fn set_placement(&self, hwnd: isize, placement: &Placement) -> io::Result<()> {
        let wp = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            flags: WINDOWPLACEMENT_FLAGS(placement.flags),
            showCmd: placement.show.to_cmd(),
            ptMinPosition: POINT {
                x: placement.min_at[0],
                y: placement.min_at[1],
            },
            ptMaxPosition: POINT {
                x: placement.max_at[0],
                y: placement.max_at[1],
            },
            rcNormalPosition: RECT {
                left: placement.normal[0],
                top: placement.normal[1],
                right: placement.normal[2],
                bottom: placement.normal[3],
            },
        };
        // SAFETY: a window handle and a fully set placement whose `length` is
        // the structure's size, as the call requires.
        unsafe { SetWindowPlacement(handle(hwnd), &wp) }.map_err(io::Error::other)
    }

    fn restore(&self, hwnd: isize) -> io::Result<()> {
        let h = handle(hwnd);
        // The asynchronous form: a window whose program has hung would hold
        // this thread otherwise. What is waited for is the result.
        // SAFETY: a call on a window handle; a handle that is no window is
        // refused.
        let _ = unsafe { ShowWindowAsync(h, SW_RESTORE) };
        if wait_until(|| !is_window(h) || (!is_minimized(h) && !is_maximized(h))) {
            Ok(())
        } else {
            Err(io::Error::other(
                "the window did not come out of the taskbar",
            ))
        }
    }

    fn size_picture(&self, hwnd: isize, width: u32, height: u32) -> io::Result<()> {
        let h = handle(hwnd);
        // A minimised window has no frame to size, and a maximised one stays
        // as large as its screen lets it: neither is sized.
        if is_minimized(h) || is_maximized(h) {
            return Err(io::Error::other(
                "a minimised or maximised window is not sized",
            ));
        }
        let frame = window_target::frame_of(hwnd)
            .ok_or_else(|| io::Error::other("the window has no frame"))?;
        let mut outer = RECT::default();
        // SAFETY: a window handle and an out value.
        unsafe { GetWindowRect(h, &mut outer) }.map_err(io::Error::other)?;
        let (cx, cy) = outer_for_picture(
            (width, height),
            (outer.right - outer.left, outer.bottom - outer.top),
            (frame.2 as i32, frame.3 as i32),
        );
        // The size only; the place and the stacking are left alone. The
        // asynchronous form so a window whose program has hung does not hold
        // this thread; what it came to is read below.
        // SAFETY: a window handle and plain numbers; `None` leaves the
        // stacking (SWP_NOZORDER) as it is.
        unsafe {
            SetWindowPos(
                h,
                None,
                0,
                0,
                cx,
                cy,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        }
        .map_err(io::Error::other)?;
        // Give the window a moment to take it. A window that will not (a
        // least size, a fixed one) never gets there: the wait ends when it has
        // stopped changing, and the caller reads the size it came to.
        let mut last = None;
        let mut still = 0;
        wait_until(|| {
            let now = window_target::frame_of(hwnd).map(|f| (f.2 as i32, f.3 as i32));
            let Some(now) = now else { return true };
            if !needs_resize(now, (width, height)) {
                return true;
            }
            still = if last == Some(now) { still + 1 } else { 0 };
            last = Some(now);
            still >= 6
        });
        Ok(())
    }
}
