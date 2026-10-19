//! The laser dot on Windows (G4): a small round window above every other,
//! click-through and never activated, moved where [`crate::laser::Dot`]
//! says. One thread owns it and pumps its messages; the input task sends
//! it moves, so a slow window never holds up input.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateEllipticRgn, CreateSolidBrush, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, HWND_TOPMOST, LWA_ALPHA, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

use crate::laser::{Area, DOT_PX, Dot, dot_px};

/// The dot's colour (a presenter's red) and opacity.
const RGB: (u8, u8, u8) = (230, 30, 40);
const ALPHA: u8 = 230;

enum Move {
    By(i16, i16),
    Hide,
}

static SENDER: OnceLock<Mutex<Option<Sender<Move>>>> = OnceLock::new();

/// The dot's thread, started at the first move; `None` when it could not start.
fn sender() -> Option<Sender<Move>> {
    let slot = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("laser-dot".into())
            .spawn(move || run(rx))
            .is_ok();
        Mutex::new(started.then_some(tx))
    });
    slot.lock().ok()?.clone()
}

/// The phone's laser packet: queued for the dot's thread.
pub fn handle(dx: i16, dy: i16, show: bool) {
    // A hide with no dot yet starts no thread and no window (G4 review).
    if !show && SENDER.get().is_none() {
        return;
    }
    if let Some(tx) = sender() {
        let _ = tx.send(if show { Move::By(dx, dy) } else { Move::Hide });
    }
}

unsafe extern "system" fn dot_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window() -> Option<HWND> {
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.ok()?.into();
    let (r, g, b) = RGB;
    let class = WNDCLASSW {
        lpfnWndProc: Some(dot_proc),
        hInstance: instance,
        lpszClassName: w!("PocketdeskLaserDot"),
        hbrBackground: unsafe {
            CreateSolidBrush(COLORREF(
                u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16,
            ))
        },
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("PocketdeskLaserDot"),
            w!("Pocketdesk laser pointer"),
            WS_POPUP,
            0,
            0,
            DOT_PX,
            DOT_PX,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .ok()?;
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA);
        shape(hwnd, DOT_PX);
    }
    Some(hwnd)
}

/// The window's round shape at `size`; the system owns the region after.
unsafe fn shape(hwnd: HWND, size: i32) {
    unsafe {
        SetWindowRgn(
            hwnd,
            Some(CreateEllipticRgn(0, 0, size + 1, size + 1)),
            true,
        );
    }
}

fn screens() -> Vec<Area> {
    crate::windows_desktop::monitors()
        .into_iter()
        .map(|m| Area {
            left: m.x,
            top: m.y,
            width: m.width,
            height: m.height,
        })
        .collect()
}

fn pointer() -> (i32, i32) {
    let mut at = POINT::default();
    let _ = unsafe { GetCursorPos(&mut at) };
    (at.x, at.y)
}

/// The dot's thread: moves as they come, its window's messages pumped
/// between them, hidden when told or still for [`crate::laser::IDLE`].
fn run(moves: Receiver<Move>) {
    let Some(hwnd) = create_window() else {
        tracing::warn!("The laser dot's window could not be made");
        return;
    };
    let mut dot = Dot::default();
    // Read once per showing: the screens do not change under a moving dot.
    let mut known: Vec<Area> = Vec::new();
    let mut size = DOT_PX;
    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe { DispatchMessageW(&msg) };
        }
        // Hidden, it wakes only to pump its window's messages (G4 review);
        // shown, often enough to hide a dot whose release was lost.
        let wait = if dot.idle_soon() { 50 } else { 250 };
        let next = moves.recv_timeout(Duration::from_millis(wait));
        let now = Instant::now();
        match next {
            Ok(Move::By(dx, dy)) => {
                if !dot.shown() {
                    known = screens();
                }
                if let Some((x, y)) = dot.moved(dx, dy, &known, pointer(), now) {
                    let wanted = dot_px(&known, (x, y));
                    unsafe {
                        if wanted != size {
                            size = wanted;
                            shape(hwnd, size);
                        }
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_TOPMOST),
                            x - size / 2,
                            y - size / 2,
                            size,
                            size,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                }
            }
            Ok(Move::Hide) => {
                dot.hide();
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Still too long: the phone's release was likely lost. The
                // window goes, the place is kept (G4 review).
                if dot.idle(now) {
                    dot.rest();
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
