//! What lending a window asks of Windows: the real [`Desktop`]. Kept to the
//! calls themselves; every rule that can be tried without a desktop is in
//! [`super::desktop`] and tested there. This file is compiled, and its checks
//! run, on Windows only (the owner's live checks are listed in the
//! device checklist).

use std::io;
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::window_target;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetWindowPlacement, GetWindowRect, IsIconic, IsWindow, IsZoomed, SW_RESTORE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPlacement,
    SetWindowPos, ShowWindowAsync, WINDOWPLACEMENT, WINDOWPLACEMENT_FLAGS,
};

use super::desktop::{
    Desktop, Facts, Kind, Placement, Show, keep_on_screen, needs_resize, outer_for_picture,
};
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

/// Whether the process `process` runs elevated; `None` when that cannot be
/// read.
fn is_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: a process handle this function was given, and an out handle on
    // this stack, closed below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: the token just opened, and an out buffer of the size told.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    // SAFETY: the token opened above, closed once.
    let _ = unsafe { CloseHandle(token) };
    read.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// Whether the program of `pid` runs with more rights than this server, or
/// cannot be looked into (taken as more: when in doubt it is not lent).
fn is_beyond_us(pid: u32) -> bool {
    // SAFETY: the pseudo handle of this process, which needs no closing.
    let ours = is_elevated(unsafe { GetCurrentProcess() }).unwrap_or(true);
    // SAFETY: a process id is only asked about; the handle is closed below.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return true;
    };
    let theirs = is_elevated(process);
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(process) };
    match theirs {
        Some(theirs) => theirs && !ours,
        None => true,
    }
}

/// The window's class name.
fn class_of(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: a window handle and a buffer of the length it is given.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0)])
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
        let identity = window_target::identity_of(hwnd)?;
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
        (frame[2] > 0 && frame[3] > 0).then_some(Facts {
            frame,
            minimized,
            maximized: is_maximized(h),
            identity,
        })
    }

    fn kind(&self, hwnd: isize) -> Option<Kind> {
        let h = handle(hwnd);
        if !is_window(h) {
            return None;
        }
        let identity = window_target::identity_of(hwnd)?;
        Some(Kind {
            class: class_of(h),
            beyond_us: is_beyond_us(identity.pid),
        })
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
        // as large as its screen lets it: neither is asked (the caller does
        // not, and this does not either), and the answer is the size it has.
        if is_minimized(h) || is_maximized(h) {
            return Ok(());
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
        // The size only; the place and the stacking are left alone for now. The
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
        keep_window_on_screen(h);
        Ok(())
    }
}

/// A window that grew may overhang its screen: move it back by what it
/// overhangs (to the work area, the screen without the taskbar), size and
/// stacking untouched. Best effort; a window that is gone or has no screen is
/// left.
fn keep_window_on_screen(h: HWND) {
    let mut outer = RECT::default();
    // SAFETY: a window handle and an out value.
    if unsafe { GetWindowRect(h, &mut outer) }.is_err() {
        return;
    }
    // SAFETY: a window handle; the nearest monitor is always answered.
    let monitor = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: a monitor handle Windows just gave and an out value whose
    // `cbSize` is set, as the call requires.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return;
    }
    let work = info.rcWork;
    let (x, y) = keep_on_screen(
        outer.left,
        outer.top,
        outer.right - outer.left,
        outer.bottom - outer.top,
        [work.left, work.top, work.right, work.bottom],
    );
    if (x, y) != (outer.left, outer.top) {
        // SAFETY: a window handle and plain numbers; the size and stacking are
        // left as they are.
        let _ = unsafe {
            SetWindowPos(
                h,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        };
    }
}
