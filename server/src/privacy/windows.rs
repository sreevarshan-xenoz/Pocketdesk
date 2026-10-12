//! Privacy mode on Windows: block the local keyboard and mouse while the phone
//! drives the desktop, show a "session active" ring, lock on request.
//!
//! Blocking uses low-level keyboard and mouse hooks (`WH_KEYBOARD_LL`,
//! `WH_MOUSE_LL`) that swallow every event *not* marked injected — the
//! phone's `SendInput` events carry the injected flag and pass. No
//! administrator rights are needed, and the hooks cannot outlive the process.
//! Ctrl+Alt+Del is a secure attention sequence no hook can see, so the person
//! at the machine always keeps a way in; the TTL watchdog (same contract as
//! the Linux grab) releases everything if the phone stops refreshing.
//!
//! The ring is a topmost, click-through, layered window per monitor, shaped
//! into a frame and excluded from capture (`WDA_EXCLUDEFROMCAPTURE`) so the
//! phone does not see its own warning.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, HGDIOBJ, RGN_DIFF, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, LWA_ALPHA, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, RegisterClassW, SW_SHOWNOACTIVATE,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowsHookExW, ShowWindow,
    UnhookWindowsHookEx, WDA_EXCLUDEFROMCAPTURE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::w;

/// How long a grab lives without a refresh from the phone (as on Linux).
pub const GRAB_TTL: Duration = Duration::from_secs(600);

/// Watchdog sweep interval.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

/// What `privacy_status` reports as "devices held": the keyboard and mouse.
const HELD: usize = 2;

/// The ring's colour (#FF3B30, the Linux shield's red) and opacity.
const SHIELD_RGB: (u8, u8, u8) = (0xFF, 0x3B, 0x30);
const SHIELD_ALPHA: u8 = 0xE6;

/// Whether a hook should swallow an event: everything a physical device
/// produced, nothing that was injected (the phone's input).
pub(crate) fn swallows(injected: bool) -> bool {
    !injected
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if swallows(info.flags.0 & LLKHF_INJECTED.0 != 0) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if swallows(info.flags & LLMHF_INJECTED != 0) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The ring's thickness for a monitor of `width` x `height`.
pub(crate) fn ring_thickness(width: i32, height: i32) -> i32 {
    (width.min(height) / 100).clamp(6, 24)
}

/// The frame region for a `width` x `height` window: the whole rect minus
/// the inset centre. The caller hands it to `SetWindowRgn`, which owns it.
fn ring_region(width: i32, height: i32) -> windows::Win32::Graphics::Gdi::HRGN {
    let t = ring_thickness(width, height);
    unsafe {
        let outer = CreateRectRgn(0, 0, width, height);
        let inner = CreateRectRgn(t, t, width - t, height - t);
        CombineRgn(Some(outer), Some(outer), Some(inner), RGN_DIFF);
        let _ = DeleteObject(HGDIOBJ(inner.0));
        outer
    }
}

/// The ring windows do nothing of their own.
unsafe extern "system" fn shield_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Paint the ring on every monitor. Must run on the thread that pumps
/// messages (the hook thread).
fn show_shield(instance: HINSTANCE) -> Vec<HWND> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        let (r, g, b) = SHIELD_RGB;
        let class = WNDCLASSW {
            lpfnWndProc: Some(shield_proc),
            hInstance: instance,
            lpszClassName: w!("LinuxLinkPrivacyShield"),
            hbrBackground: unsafe {
                CreateSolidBrush(COLORREF(
                    u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16,
                ))
            },
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
    });
    let mut windows = Vec::new();
    for monitor in crate::windows_desktop::monitors() {
        let Ok(hwnd) = (unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                w!("LinuxLinkPrivacyShield"),
                w!("Pocketdesk session active"),
                WS_POPUP,
                monitor.x,
                monitor.y,
                monitor.width,
                monitor.height,
                None,
                None,
                Some(instance),
                None,
            )
        }) else {
            continue;
        };
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), SHIELD_ALPHA, LWA_ALPHA);
            SetWindowRgn(
                hwnd,
                Some(ring_region(monitor.width, monitor.height)),
                false,
            );
            // The phone must not see its own warning in the stream.
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        windows.push(hwnd);
    }
    windows
}

/// A live grab: the hook thread (which also owns the ring) and its expiry.
struct Grab {
    thread_id: u32,
    thread: JoinHandle<()>,
    expires: Instant,
}

fn grab_state() -> &'static Mutex<Option<Grab>> {
    static GRAB: OnceLock<Mutex<Option<Grab>>> = OnceLock::new();
    GRAB.get_or_init(|| Mutex::new(None))
}

/// Install the hooks and the ring on a thread that pumps their messages until
/// it is told to quit; reports its thread id, or why it could not start.
fn spawn_hook_thread() -> Result<(u32, JoinHandle<()>), String> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
    let thread = std::thread::Builder::new()
        .name("privacy-hooks".into())
        .spawn(move || {
            let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
                Ok(module) => module.into(),
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("module handle: {e}")));
                    return;
                }
            };
            let hooks: Vec<HHOOK> = [
                unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0)
                },
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0) },
            ]
            .into_iter()
            .filter_map(Result::ok)
            .collect();
            if hooks.len() != 2 {
                for hook in hooks {
                    let _ = unsafe { UnhookWindowsHookEx(hook) };
                }
                let _ = ready_tx.send(Err("could not install the input hooks".to_string()));
                return;
            }
            let shield = show_shield(instance);
            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
            // The hooks run inside this loop; it ends on WM_QUIT.
            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                unsafe { DispatchMessageW(&msg) };
            }
            for hook in hooks {
                let _ = unsafe { UnhookWindowsHookEx(hook) };
            }
            for hwnd in shield {
                let _ = unsafe { DestroyWindow(hwnd) };
            }
        })
        .map_err(|e| format!("spawning the hook thread: {e}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(thread_id)) => Ok((thread_id, thread)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the hook thread did not start".to_string()),
    }
}

/// Block local input (or refresh the TTL if already blocking). Returns the
/// number of devices held: the keyboard and the mouse.
pub fn grab_input(ttl: Duration) -> Result<usize, String> {
    ensure_watchdog();
    let mut state = grab_state().lock().expect("privacy grab state");
    if let Some(grab) = state.as_mut() {
        grab.expires = Instant::now() + ttl;
        return Ok(HELD);
    }
    let (thread_id, thread) = spawn_hook_thread()?;
    tracing::info!("privacy: blocking local keyboard and mouse for {ttl:?}");
    *state = Some(Grab {
        thread_id,
        thread,
        expires: Instant::now() + ttl,
    });
    Ok(HELD)
}

/// Unblock local input and remove the ring (also on TTL lapse; process exit
/// removes the hooks by itself).
pub fn release_input() -> usize {
    let Some(grab) = grab_state().lock().expect("privacy grab state").take() else {
        return 0;
    };
    let _ = unsafe { PostThreadMessageW(grab.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    let _ = grab.thread.join();
    tracing::info!("privacy: local keyboard and mouse released");
    HELD
}

/// `(held device count, seconds left before auto-release)`.
pub fn privacy_status() -> (usize, Option<u64>) {
    let state = grab_state().lock().expect("privacy grab state");
    match state.as_ref() {
        Some(grab) => (
            HELD,
            Some(
                grab.expires
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
            ),
        ),
        None => (0, None),
    }
}

fn ensure_watchdog() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(WATCHDOG_INTERVAL);
            let expired = grab_state()
                .lock()
                .expect("privacy grab state")
                .as_ref()
                .is_some_and(|g| Instant::now() >= g.expires);
            if expired {
                tracing::info!("privacy: grab TTL lapsed, releasing local input");
                release_input();
            }
        }
    });
}

/// Lock the workstation.
pub fn lock_screen() -> Result<&'static str, String> {
    unsafe { LockWorkStation() }
        .map(|()| "LockWorkStation")
        .map_err(|e| format!("LockWorkStation: {e}"))
}

// No test here installs the hooks or locks the machine: blocking the dev
// box's real keyboard and mouse, or locking it, is not something a test run
// does. They are checklist items.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_input_is_swallowed_and_the_phones_passes() {
        assert!(swallows(false), "a real keystroke must be blocked");
        assert!(!swallows(true), "an injected (phone) keystroke must pass");
    }

    #[test]
    fn idle_state_reports_nothing_held_and_release_is_harmless() {
        assert_eq!(privacy_status(), (0, None));
        assert_eq!(release_input(), 0);
    }

    #[test]
    fn the_ring_is_visible_but_never_covers_the_screen() {
        assert_eq!(ring_thickness(1920, 1080), 10);
        assert_eq!(ring_thickness(800, 600), 6);
        assert_eq!(ring_thickness(7680, 4320), 24);
        for (w, h) in [(1280, 720), (3840, 2160)] {
            assert!(ring_thickness(w, h) * 2 < h / 10);
        }
    }
}
