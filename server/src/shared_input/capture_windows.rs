//! Driving another computer from Windows: the input thread that watches and
//! swallows this computer's keyboard and mouse, the hidden cursor, and the
//! loop that runs a [`Driver`] on what it captures.
//!
//! The input thread owns the hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`, as
//! privacy mode's), installed and removed on the thread itself as its
//! [`Mode`] changes (a thread message), and the switch key
//! (`RegisterHotKey`, which works in every mode but [`Mode::Away`], where
//! the keyboard hook swallows the key before Windows sees it, so the hook
//! spots it instead). A hidden window of its own hears the session lock
//! (WTS session notifications) and sleep (`WM_POWERBROADCAST`). Injected
//! input always passes: what another computer sends this one is never
//! carried off again. Ctrl+Alt+Del is Windows' own, which no hook sees,
//! and the lock that follows brings the keyboard and mouse home; Win+L
//! while away is swallowed, brings them home, and then locks this computer
//! ([`locks_here`]). The
//! loop looks at what is in front every half second for the gaming lock
//! ([`game_lock`]), and at once when a window comes to the front (an
//! out-of-context `EVENT_SYSTEM_FOREGROUND` hook, in only with the mouse
//! hook): a game in front puts the input thread in [`Mode::Off`], with no
//! hooks of any kind. The probes run on the loop's own thread. A drag of
//! files held as the cursor goes away finds the catcher under the parked
//! cursor, and what is dropped on it is sent to the computer driven
//! ([`drag_files`], [`drop_windows`]).
//!
//! The hook reports physical pixels; the loop works in logical ones
//! ([`Desktop`]), as the layout does. Nothing here is run by the tests: they
//! must not hook, hide the cursor or move it on the owner's computer.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pocketdesk_core::local::hotkey::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetUserObjectInformationW,
    OpenInputDesktop, UOI_NAME,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, HOT_KEY_MODIFIERS, LASTINPUTINFO, RegisterHotKey,
    UnregisterHotKey, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateCursor, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    EVENT_SYSTEM_FOREGROUND, GetCursorPos, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_INJECTED, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, OCR_APPSTARTING,
    OCR_CROSS, OCR_HAND, OCR_IBEAM, OCR_NO, OCR_NORMAL, OCR_SIZEALL, OCR_SIZENESW, OCR_SIZENS,
    OCR_SIZENWSE, OCR_SIZEWE, OCR_UP, OCR_WAIT, PostThreadMessageW, RegisterClassW, SPI_SETCURSORS,
    SYSTEM_CURSOR_ID, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetCursorPos, SetSystemCursor,
    SetWindowsHookExW, SystemParametersInfoW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
    WS_POPUP,
};
use windows::core::w;

use super::awake::KeepAwake;
use super::badge;
use super::drag_files::{self, Catch, Catcher, Dropped, Outcome};
use super::drive::{Action, Driver, OpenChannel, SavedLayout, layout_for, reachable};
pub use super::drive::{Captured, Mode};
use super::drop_windows::{self, DropCatcher};
use super::game_lock;
use super::link::{self, Channel};
use super::screens::{Desktop, Motion};
use super::status;
use super::wire::{self, Msg};

/// The thread message that sets the mode (its `WPARAM`: 0 off, 1 watch,
/// 2 away).
const WM_SET_MODE: u32 = WM_APP + 1;

/// The thread message that puts the cursor somewhere (its `LPARAM`: the
/// point, [`pack`]ed; its `WPARAM`: 1 to park it there while away, 0 to put
/// it there and stop parking).
const WM_PUT_CURSOR: u32 = WM_APP + 2;

/// A point in one `LPARAM`: x in the low 32 bits, y in the high.
pub(super) fn pack((x, y): (i32, i32)) -> isize {
    ((u64::from(y as u32) << 32) | u64::from(x as u32)) as isize
}

pub(super) fn unpack(packed: isize) -> (i32, i32) {
    let bits = packed as u64;
    (bits as u32 as i32, (bits >> 32) as u32 as i32)
}

/// The switch key's id with `RegisterHotKey`.
const HOTKEY_ID: i32 = 0x5049;

/// The session-change and power messages the hidden window hears.
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;
const WM_POWERBROADCAST: u32 = 0x0218;
const PBT_APMSUSPEND: usize = 0x4;
const PBT_APMRESUMESUSPEND: usize = 0x7;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;

/// One notch of a wheel.
const WHEEL_DELTA: i32 = 120;

/// The input thread: its hooks, the switch key and the hidden window. The
/// system cursors are restored when it starts (a crash while away must not
/// leave the owner without a cursor) and when it is dropped.
pub struct InputThread {
    worker: Option<Worker>,
    restore: fn(),
}

/// The thread itself, once it is running.
struct Worker {
    thread_id: u32,
    thread: JoinHandle<()>,
}

/// How the input thread is started and the cursors restored: the system's,
/// or a test's stand-ins (a test must not hook the owner's computer).
struct Seams {
    restore: fn(),
    spawn: fn(Sender<Captured>, Option<Hotkey>) -> Option<Worker>,
}

impl InputThread {
    /// Start the input thread in [`Mode::Off`], listening for `hotkey`; what
    /// it sees goes to `events` (pointer positions in physical pixels).
    pub fn start(events: Sender<Captured>, hotkey: Option<Hotkey>) -> InputThread {
        Self::start_with(
            events,
            hotkey,
            Seams {
                restore: restore_cursors,
                spawn: spawn_input_thread,
            },
        )
    }

    fn start_with(events: Sender<Captured>, hotkey: Option<Hotkey>, seams: Seams) -> InputThread {
        (seams.restore)();
        InputThread {
            worker: (seams.spawn)(events, hotkey),
            restore: seams.restore,
        }
    }

    /// Whether the thread is running (its hooks could be installed).
    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    /// Whether the input thread is still running (it stops only on drop, so
    /// `false` here means it died and the keyboard and mouse must come home).
    pub fn is_alive(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|w| !w.thread.is_finished())
    }

    /// Off: no hooks, only the switch key. Watch: the mouse hook watches and
    /// swallows nothing. Away: both hooks swallow all but injected input.
    pub fn set_mode(&self, mode: Mode) {
        let Some(worker) = &self.worker else {
            return;
        };
        let code = match mode {
            Mode::Off => 0,
            Mode::Watch => 1,
            Mode::Away => 2,
        };
        // SAFETY: a plain message to a thread this owns.
        if let Err(e) =
            unsafe { PostThreadMessageW(worker.thread_id, WM_SET_MODE, WPARAM(code), LPARAM(0)) }
        {
            tracing::warn!("Shared input: the input thread was not told its mode: {e}");
        }
    }

    /// Put the cursor at `at` (physical) on the input thread, in order with
    /// its mode: `parked`, it is held there while away and moves are
    /// measured from it (a move at `at` itself is sent first, so the moves
    /// after the park can be told from those before); else it is put there
    /// and no longer held.
    pub fn put_cursor(&self, at: (i32, i32), parked: bool) {
        let Some(worker) = &self.worker else {
            return;
        };
        // SAFETY: as in `set_mode`.
        if let Err(e) = unsafe {
            PostThreadMessageW(
                worker.thread_id,
                WM_PUT_CURSOR,
                WPARAM(usize::from(parked)),
                LPARAM(pack(at)),
            )
        } {
            tracing::warn!("Shared input: the cursor was not put back: {e}");
        }
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            // SAFETY: as in `set_mode`.
            let _ = unsafe { PostThreadMessageW(worker.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = worker.thread.join();
        }
        (self.restore)();
    }
}

/// The modifier bits of `RegisterHotKey` held now, as the keyboard hook
/// tracked them (`tracked`) or Windows has them.
fn modifiers_down(tracked: u32) -> u32 {
    let down = |vk: u16| {
        // SAFETY: reads a key's state.
        (unsafe { GetAsyncKeyState(i32::from(vk)) } as u16) & 0x8000 != 0
    };
    let mut mods = tracked;
    if down(VK_CONTROL.0) {
        mods |= MOD_CONTROL;
    }
    if down(VK_MENU.0) {
        mods |= MOD_ALT;
    }
    if down(VK_SHIFT.0) {
        mods |= MOD_SHIFT;
    }
    if down(VK_LWIN.0) || down(VK_RWIN.0) {
        mods |= MOD_WIN;
    }
    mods
}

/// The modifier bit a virtual key is, if it is one.
fn modifier_of(vk: u32) -> u32 {
    let is = |keys: &[u16]| keys.iter().any(|k| u32::from(*k) == vk);
    if is(&[VK_CONTROL.0, VK_LCONTROL.0, VK_RCONTROL.0]) {
        MOD_CONTROL
    } else if is(&[VK_MENU.0, VK_LMENU.0, VK_RMENU.0]) {
        MOD_ALT
    } else if is(&[VK_SHIFT.0, VK_LSHIFT.0, VK_RSHIFT.0]) {
        MOD_SHIFT
    } else if is(&[VK_LWIN.0, VK_RWIN.0]) {
        MOD_WIN
    } else {
        0
    }
}

/// Whether the key `vk` going down with modifiers `mods` held is `hotkey`:
/// the same key with exactly its modifiers (MOD_NOREPEAT aside).
pub(crate) fn hotkey_hit(mods: u32, vk: u32, hotkey: &Hotkey) -> bool {
    let wanted = hotkey.modifiers & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN);
    vk == hotkey.key && mods & (MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN) == wanted
}

/// A wheel's turn (`WHEEL_DELTA` units, as the hook has them) added to what
/// was left of the last, as whole notches; the rest is kept.
pub(crate) fn notches(rest: &mut i32, delta: i32) -> i16 {
    *rest = rest.saturating_add(delta);
    let whole = *rest / WHEEL_DELTA;
    *rest -= whole * WHEEL_DELTA;
    whole.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// What the hooks and the hidden window need, on the input thread.
struct ThreadState {
    events: Sender<Captured>,
    hotkey: Option<Hotkey>,
    mode: Mode,
    mouse: Option<HHOOK>,
    keyboard: Option<HHOOK>,
    /// The out-of-context hook that hears a window come to the front, set
    /// and removed with the mouse hook.
    foreground: Option<HWINEVENTHOOK>,
    instance: HINSTANCE,
    /// Modifiers seen down by the keyboard hook.
    mods: u32,
    /// The switch key was taken by the hook: its key-up is swallowed too.
    hotkey_down: bool,
    wheel: (i32, i32),
    /// The session is locked; the computer is going to sleep.
    locked: bool,
    asleep: bool,
    /// Where the cursor is parked while away (physical).
    park: Option<(i32, i32)>,
    /// Which input thread this is: each started counts on.
    generation: u64,
}

// SAFETY: the hook and module handles are plain values, used only on the
// input thread (the hooks run on the thread that installed them); there is
// one input thread at a time in STATE (by its generation).
unsafe impl Send for ThreadState {}

/// The input thread's state, while it runs.
static STATE: Mutex<Option<ThreadState>> = Mutex::new(None);

/// The input threads started so far: each takes the next as its
/// generation.
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

/// Put `state` in `slot`, unless a newer input thread's is there: one left
/// behind, stuck as it started, may get going after a newer one did.
fn install_state(slot: &mut Option<ThreadState>, state: ThreadState) {
    if slot
        .as_ref()
        .is_none_or(|s| s.generation < state.generation)
    {
        *slot = Some(state);
    }
}

/// Clear `slot` if it is the input thread `generation`'s, never a newer
/// one's.
fn clear_own(slot: &mut Option<ThreadState>, generation: u64) {
    if slot.as_ref().is_some_and(|s| s.generation == generation) {
        *slot = None;
    }
}

/// `f` on the input thread `generation`'s state, if it is the one in place.
fn with_own_state<T>(generation: u64, f: impl FnOnce(&mut ThreadState) -> T) -> Option<T> {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .filter(|s| s.generation == generation)
        .map(f)
}

/// `f` on the input thread's state; `None` when it is not running. Never
/// held across a call that runs a hook or the window (those run only while
/// the thread waits for a message).
fn with_state<T>(f: impl FnOnce(&mut ThreadState) -> T) -> Option<T> {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if info.flags & LLMHF_INJECTED == 0 {
            let (swallow, repark) =
                with_state(|s| on_mouse(s, wparam.0 as u32, info)).unwrap_or((false, None));
            // Re-park outside the lock: SetCursorPos makes another move that
            // re-enters this hook, which would take the lock again.
            if let Some(park) = repark {
                set_cursor(park);
            }
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A mouse event from a device: told, whether to swallow it, and where the
/// cursor should be re-parked (done by the caller outside the state lock).
fn on_mouse(
    s: &mut ThreadState,
    message: u32,
    info: &MSLLHOOKSTRUCT,
) -> (bool, Option<(i32, i32)>) {
    let away = s.mode == Mode::Away;
    let wheel_delta = i32::from((info.mouseData >> 16) as u16 as i16);
    let mut repark = None;
    let captured = match message {
        WM_MOUSEMOVE => Some(match s.park.filter(|_| away) {
            // Away the cursor stays put: the move is how far the mouse went
            // from where the cursor is, told from the park point.
            Some(park) => {
                let now = cursor_at().unwrap_or(park);
                repark = (now != park).then_some(park);
                Captured::Moved {
                    x: park.0.saturating_add(info.pt.x.saturating_sub(now.0)),
                    y: park.1.saturating_add(info.pt.y.saturating_sub(now.1)),
                }
            }
            None => Captured::Moved {
                x: info.pt.x,
                y: info.pt.y,
            },
        }),
        _ if !away => None,
        WM_LBUTTONDOWN | WM_LBUTTONUP => Some(Captured::Button {
            button: 0,
            down: message == WM_LBUTTONDOWN,
        }),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some(Captured::Button {
            button: 1,
            down: message == WM_MBUTTONDOWN,
        }),
        WM_RBUTTONDOWN | WM_RBUTTONUP => Some(Captured::Button {
            button: 2,
            down: message == WM_RBUTTONDOWN,
        }),
        WM_XBUTTONDOWN | WM_XBUTTONUP => match info.mouseData >> 16 {
            1 => Some(Captured::Button {
                button: 3,
                down: message == WM_XBUTTONDOWN,
            }),
            2 => Some(Captured::Button {
                button: 4,
                down: message == WM_XBUTTONDOWN,
            }),
            _ => None,
        },
        // Windows' forward turn is up; the wire's positive is down.
        WM_MOUSEWHEEL => match notches(&mut s.wheel.1, wheel_delta) {
            0 => None,
            n => Some(Captured::Wheel {
                dx: 0,
                dy: n.saturating_neg(),
            }),
        },
        WM_MOUSEHWHEEL => match notches(&mut s.wheel.0, wheel_delta) {
            0 => None,
            n => Some(Captured::Wheel { dx: n, dy: 0 }),
        },
        _ => None,
    };
    // A button held here when the mouse went away (a drag) comes up here
    // too, or Windows would hold it.
    let up_held_here = match message {
        WM_LBUTTONUP => windows_holds(0x01),
        WM_RBUTTONUP => windows_holds(0x02),
        WM_MBUTTONUP => windows_holds(0x04),
        WM_XBUTTONUP => match info.mouseData >> 16 {
            1 => windows_holds(0x05),
            2 => windows_holds(0x06),
            _ => false,
        },
        _ => false,
    };
    if let Some(captured) = captured {
        let _ = s.events.send(captured);
    }
    (away && !up_held_here, repark)
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            let swallow = with_state(|s| on_key(s, wparam.0 as u32, info)).unwrap_or(false);
            if swallow {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: as in `mouse_hook`.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A key from a keyboard: told, and whether to swallow it.
fn on_key(s: &mut ThreadState, message: u32, info: &KBDLLHOOKSTRUCT) -> bool {
    if s.mode != Mode::Away {
        return false;
    }
    let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let modifier = modifier_of(info.vkCode);
    if modifier != 0 {
        if down {
            s.mods |= modifier;
        } else {
            s.mods &= !modifier;
        }
    }
    if let Some(hotkey) = s.hotkey
        && info.vkCode == hotkey.key
    {
        if down && hotkey_hit(modifiers_down(s.mods), info.vkCode, &hotkey) {
            if !s.hotkey_down {
                s.hotkey_down = true;
                let _ = s.events.send(Captured::Hotkey);
            }
            return true;
        }
        if !down && std::mem::take(&mut s.hotkey_down) {
            return true;
        }
    }
    // Win+L: swallowed like any key (Windows never saw the Win key go
    // down, so passed on it would type an "l"), never sent; the driver
    // brings the keyboard and mouse home and locks this computer. Its
    // key-up is swallowed below if the hook is still in then; once home
    // the hook is out, and the key-up reaches Windows on its own.
    if down && locks_here(modifiers_down(s.mods), info.vkCode) {
        let _ = s.events.send(Captured::LockKey);
        return true;
    }
    // Esc while files dragged across are over the catcher: Windows' own,
    // cancelling the drag (BK4), and never sent.
    if drag_files::esc_cancels_drag(info.vkCode, down, drop_windows::files_over()) {
        return false;
    }
    let _ = s.events.send(Captured::Key {
        scan: info.scanCode as u16,
        extended: info.flags.0 & LLKHF_EXTENDED.0 != 0,
        down,
    });
    // A key pressed here before the keyboard went away (Ctrl and Alt of
    // the switch key, say) comes up here too, or Windows would hold it.
    down || !windows_holds(info.vkCode)
}

/// Whether a send to `to` may open (and greet) a channel for it, with the
/// keyboard and mouse on `away_on`: never for the computer driven, where a
/// missing channel is a failed send and brings them home (a new channel's
/// hello and leave would end the visit there, and its pongs would keep the
/// watchdog quiet while nothing reached it).
fn may_open(away_on: Option<&str>, to: &str) -> bool {
    away_on != Some(to)
}

/// Whether the key `vk` going down with modifiers `mods` held while away
/// is Win+L, which locks this computer: the driver brings the keyboard and
/// mouse home and then locks it ([`Captured::LockKey`]).
pub(crate) fn locks_here(mods: u32, vk: u32) -> bool {
    vk == u32::from(b'L') && mods & MOD_WIN != 0
}

/// Whether Windows has the key or button `vk` down: pressed before the
/// hooks swallowed anything (what they swallow never reaches it).
fn windows_holds(vk: u32) -> bool {
    let Ok(vk) = i32::try_from(vk) else {
        return false;
    };
    // SAFETY: reads a key's state.
    (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 == 0x8000
}

/// Another window came to the front: told, so the loop looks at once.
unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    with_state(|s| {
        let _ = s.events.send(Captured::ForegroundChanged);
    });
}

/// Whether `mode` lacks a hook it needs, with the mouse's and the
/// keyboard's in or not: away takes both, so without either this
/// computer's own input would act here and on the other computer at once.
fn hooks_missing(mode: Mode, mouse: bool, keyboard: bool) -> bool {
    mode == Mode::Away && !(mouse && keyboard)
}

/// Put the hooks `mode` wants in place, on the input thread.
fn apply_mode(s: &mut ThreadState, mode: Mode) {
    s.mode = mode;
    let want_mouse = mode != Mode::Off;
    let want_keyboard = mode == Mode::Away;
    if want_mouse && s.mouse.is_none() {
        // SAFETY: a hook procedure of this module, on this thread.
        match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(s.instance), 0) } {
            Ok(hook) => s.mouse = Some(hook),
            Err(e) => tracing::warn!("Shared input: the mouse hook was not installed: {e}"),
        }
    }
    if want_keyboard && s.keyboard.is_none() {
        s.mods = 0;
        s.hotkey_down = false;
        // SAFETY: as above.
        match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(s.instance), 0) }
        {
            Ok(hook) => s.keyboard = Some(hook),
            Err(e) => tracing::warn!("Shared input: the keyboard hook was not installed: {e}"),
        }
    }
    if !want_mouse && let Some(hook) = s.mouse.take() {
        // SAFETY: a hook this thread installed.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    if !want_keyboard && let Some(hook) = s.keyboard.take() {
        // SAFETY: as above.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
    // A window coming to the front is heard at once while the mouse hook
    // is in (out of context: nothing is loaded into other programs; told
    // through this thread's message loop). Off, with a game in front, there
    // is no hook of any kind; the loop's poll sees the game leave.
    if want_mouse && s.foreground.is_none() {
        // SAFETY: a callback of this module, on this thread.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.0.is_null() {
            tracing::warn!("Shared input: a window coming to the front will not be heard at once");
        } else {
            s.foreground = Some(hook);
        }
    }
    if !want_mouse && let Some(hook) = s.foreground.take() {
        // SAFETY: a hook this thread set.
        let _ = unsafe { UnhookWinEvent(hook) };
    }
    if mode != Mode::Away {
        s.wheel = (0, 0);
    }
    // The driver brings the keyboard and mouse home, the cursor shown.
    if hooks_missing(mode, s.mouse.is_some(), s.keyboard.is_some()) {
        let _ = s.events.send(Captured::HookFailed);
    }
}

/// The hidden window: the session's lock and the computer's sleep.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let changed = match (msg, wparam.0) {
        (WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK) => Some((Some(true), None)),
        (WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK) => Some((Some(false), None)),
        (WM_POWERBROADCAST, PBT_APMSUSPEND) => Some((None, Some(true))),
        (WM_POWERBROADCAST, PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC) => {
            Some((None, Some(false)))
        }
        _ => None,
    };
    if let Some((locked, asleep)) = changed {
        with_state(|s| {
            let was = s.locked || s.asleep;
            s.locked = locked.unwrap_or(s.locked);
            s.asleep = asleep.unwrap_or(s.asleep);
            let now = s.locked || s.asleep;
            if now != was {
                let _ = s.events.send(if now {
                    Captured::Locked
                } else {
                    Captured::Unlocked
                });
            }
        });
    }
    // SAFETY: the default handling for this window's messages.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Start the input thread: its hidden window, the switch key, and its
/// message loop; `None` when it could not start.
fn spawn_input_thread(events: Sender<Captured>, hotkey: Option<Hotkey>) -> Option<Worker> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let tid = Arc::new(AtomicU32::new(0));
    let tid_thread = Arc::clone(&tid);
    // Set by a parent that gave up waiting: the thread then skips its loop
    // and tears down, even if the WM_QUIT sent to it was lost (no queue yet).
    let abandoned = Arc::new(AtomicBool::new(false));
    let abandoned_thread = Arc::clone(&abandoned);
    let generation = GENERATIONS.fetch_add(1, Ordering::SeqCst) + 1;
    let spawned = std::thread::Builder::new()
        .name("pd-shared-input".into())
        .spawn(move || {
            // Its id first, so a parent that times out can still tell it to
            // quit and clean STATE up.
            // SAFETY: this thread's id.
            tid_thread.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
            // SAFETY: plain Win32 calls on this thread's own state.
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            let instance: HINSTANCE = match unsafe { GetModuleHandleW(None) } {
                Ok(module) => module.into(),
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("module handle: {e}")));
                    return;
                }
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("PocketdeskSharedInput"),
                ..Default::default()
            };
            // SAFETY: a class of this module's window procedure; registering
            // it again (a later start) fails harmlessly.
            unsafe { RegisterClassW(&class) };
            // A top-level window that is never shown: it hears the power
            // broadcasts a message-only window would not.
            // SAFETY: the class just registered.
            let window = unsafe {
                CreateWindowExW(
                    Default::default(),
                    w!("PocketdeskSharedInput"),
                    w!("Pocketdesk shared input"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance),
                    None,
                )
            };
            let window = match window {
                Ok(window) => {
                    // NOTIFY_FOR_THIS_SESSION.
                    // SAFETY: a window this thread made.
                    if let Err(e) = unsafe { WTSRegisterSessionNotification(window, 0) } {
                        tracing::warn!("Shared input: the session's lock will not be heard: {e}");
                    }
                    Some(window)
                }
                Err(e) => {
                    tracing::warn!("Shared input: no window to hear the session's lock: {e}");
                    None
                }
            };
            let state = ThreadState {
                events,
                hotkey,
                mode: Mode::Off,
                mouse: None,
                keyboard: None,
                foreground: None,
                instance,
                mods: 0,
                hotkey_down: false,
                wheel: (0, 0),
                locked: false,
                asleep: false,
                park: None,
                generation,
            };
            install_state(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()), state);
            let registered = hotkey.is_some_and(|key| {
                // SAFETY: a key for this thread's queue.
                let taken = unsafe {
                    RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(key.modifiers), key.key)
                };
                if let Err(e) = &taken {
                    tracing::warn!(
                        "Shared input: the switch key is held by another app, so only the \
                         screen's edge switches: {e}"
                    );
                }
                taken.is_ok()
            });
            let _ = ready_tx.send(Ok(()));
            let mut msg = MSG::default();
            // SAFETY: the loop the hooks and the window run in; it ends on
            // WM_QUIT (and is skipped when the parent already gave up).
            while !abandoned_thread.load(Ordering::SeqCst)
                && unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool()
            {
                match msg.message {
                    WM_SET_MODE if msg.hwnd.is_invalid() => {
                        let mode = match msg.wParam.0 {
                            2 => Mode::Away,
                            1 => Mode::Watch,
                            _ => Mode::Off,
                        };
                        with_state(|s| apply_mode(s, mode));
                    }
                    WM_PUT_CURSOR if msg.hwnd.is_invalid() => {
                        let at = unpack(msg.lParam.0);
                        let parked = msg.wParam.0 == 1;
                        set_cursor(at);
                        with_state(|s| {
                            s.park = parked.then_some(at);
                            if parked {
                                let _ = s.events.send(Captured::Moved { x: at.0, y: at.1 });
                            }
                        });
                    }
                    WM_HOTKEY if msg.hwnd.is_invalid() => {
                        with_state(|s| {
                            let _ = s.events.send(Captured::Hotkey);
                        });
                    }
                    // SAFETY: the hidden window's messages.
                    _ => unsafe {
                        DispatchMessageW(&msg);
                    },
                }
            }
            // Only its own state: a thread left behind that gets here late
            // must not take a newer one's hooks out or clear its state.
            with_own_state(generation, |s| apply_mode(s, Mode::Off));
            if registered {
                // SAFETY: the key registered above.
                let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
            }
            if let Some(window) = window {
                // SAFETY: the window this thread made.
                let _ = unsafe { WTSUnRegisterSessionNotification(window) };
                let _ = unsafe { DestroyWindow(window) };
            }
            clear_own(
                &mut STATE.lock().unwrap_or_else(|e| e.into_inner()),
                generation,
            );
        });
    let thread = match spawned {
        Ok(thread) => thread,
        Err(e) => {
            tracing::warn!("Shared input: no input thread: {e}");
            return None;
        }
    };
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Ok(())) => Some(Worker {
            thread_id: tid.load(Ordering::SeqCst),
            thread,
        }),
        Ok(Err(e)) => {
            tracing::warn!("Shared input: the input thread did not start: {e}");
            let _ = thread.join();
            None
        }
        Err(_) => {
            tracing::warn!("Shared input: the input thread did not start in time");
            // It may yet be running with STATE set: tell it to quit, and wait
            // a bounded time so a stuck thread cannot hang the driver.
            abandoned.store(true, Ordering::SeqCst);
            let id = tid.load(Ordering::SeqCst);
            if id != 0 {
                // SAFETY: a plain message to that thread.
                let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            }
            let deadline = Instant::now() + QUIT_WAIT;
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                tracing::error!(
                    "Shared input: the input thread is stuck and was left behind; \
                     sharing stays off until it is started again"
                );
            }
            None
        }
    }
}

/// How long a parent that gave up on the input thread waits for it to quit.
const QUIT_WAIT: Duration = Duration::from_secs(2);

/// The standard cursors, each made blank while away.
const CURSORS: [SYSTEM_CURSOR_ID; 13] = [
    OCR_NORMAL,
    OCR_IBEAM,
    OCR_WAIT,
    OCR_CROSS,
    OCR_UP,
    OCR_SIZENWSE,
    OCR_SIZENESW,
    OCR_SIZEWE,
    OCR_SIZENS,
    OCR_SIZEALL,
    OCR_NO,
    OCR_HAND,
    OCR_APPSTARTING,
];

/// Make every standard cursor blank: the parked cursor is not seen.
fn hide_cursors() {
    // A 32x32 cursor whose AND mask keeps the screen and XOR mask changes
    // nothing.
    let and = [0xFFu8; 128];
    let xor = [0u8; 128];
    for id in CURSORS {
        // SAFETY: the masks outlive the call; SetSystemCursor takes the
        // cursor made for it (and destroys it).
        let made =
            unsafe { CreateCursor(None, 0, 0, 32, 32, and.as_ptr().cast(), xor.as_ptr().cast()) };
        match made {
            Ok(cursor) => {
                if let Err(e) = unsafe { SetSystemCursor(cursor, id) } {
                    tracing::debug!("Shared input: cursor {} not hidden: {e}", id.0);
                }
            }
            Err(e) => tracing::debug!("Shared input: no blank cursor: {e}"),
        }
    }
}

/// The owner's cursors back, as their scheme has them.
fn restore_cursors() {
    // SAFETY: reloads the system cursors; no pointer is passed.
    if let Err(e) = unsafe {
        SystemParametersInfoW(
            SPI_SETCURSORS,
            0,
            None,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    } {
        tracing::warn!("Shared input: the cursors were not restored: {e}");
    }
}

/// Where the cursor is now (physical).
fn cursor_at() -> Option<(i32, i32)> {
    let mut at = POINT::default();
    // SAFETY: an out value on this stack.
    unsafe { GetCursorPos(&mut at) }.ok()?;
    Some((at.x, at.y))
}

fn set_cursor(p: (i32, i32)) {
    // SAFETY: moves the cursor; nothing is borrowed.
    if let Err(e) = unsafe { SetCursorPos(p.0, p.1) } {
        tracing::debug!("Shared input: the cursor was not moved: {e}");
    }
}

/// Whether the input is going to this session's normal desktop (named
/// "Default"), not a secure one (a UAC prompt, Ctrl+Alt+Del, the lock or
/// screensaver desktop), whose switches hide key-ups from the hooks. An
/// input desktop that cannot be opened counts as a switch (the secure
/// desktop is exactly where opening fails; the first such failure is logged
/// once, so a desk check can tell if it ever happens on the normal one). A
/// desktop that opens but whose name cannot be read counts as the normal one.
fn on_default_desktop() -> bool {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    // SAFETY: opens the input desktop for reading and closes it.
    let desktop =
        match unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) } {
            Ok(desktop) => desktop,
            Err(e) => {
                if !LOGGED.swap(true, Ordering::SeqCst) {
                    tracing::info!(
                        "Shared input: the input desktop could not be opened (taken as a \
                     secure desktop; logged once): {e}"
                    );
                }
                return false;
            }
        };
    let mut name = [0u16; 64];
    let mut needed = 0u32;
    // SAFETY: the handle just opened, and a buffer on this stack.
    let read = unsafe {
        GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            (name.len() * 2) as u32,
            Some(&mut needed),
        )
    };
    // SAFETY: the handle this function opened.
    let _ = unsafe { CloseDesktop(desktop) };
    if read.is_err() {
        return true;
    }
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case("default")
}

/// How long ago the owner last touched this computer, as Windows has it.
fn idle_ms() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: an out value on this stack.
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        // SAFETY: reads the tick count.
        unsafe { GetTickCount() }.wrapping_sub(info.dwTime)
    } else {
        u32::MAX
    }
}

/// How often what the owner does is told to every computer with a channel.
const ACTIVE_EVERY: Duration = Duration::from_millis(200);
/// How often, while away, the link is looked at.
const LINK_EVERY: Duration = Duration::from_millis(250);
/// How often the screens and the computers are read again.
const LAYOUT_EVERY: Duration = Duration::from_secs(2);
/// How long after the driver ended by itself it is started again.
const RESTART_AFTER: Duration = Duration::from_secs(30);
/// How long moves are dropped while waiting for the input thread to park
/// the cursor.
const PARKING_WAIT: Duration = Duration::from_millis(250);
/// How long after a channel could not be opened it is tried again.
const REOPEN_AFTER: Duration = Duration::from_secs(5);
/// How often what is in front is looked at, for the gaming lock (a game
/// leaving the front is seen this way: no hook is in while it is there).
const GAME_EVERY: Duration = Duration::from_millis(500);
/// After a window comes to the front, how often it is looked at again and
/// for how long: Windows can say "full screen" a little after the window
/// takes the front.
const GAME_SETTLE_EVERY: Duration = Duration::from_millis(50);
const GAME_SETTLE_FOR: Duration = Duration::from_millis(500);

/// What is in front now (the cursor in logical pixels) and whether a
/// kernel-level anti-cheat runs, for the driver. Nothing is probed with the
/// gaming lock off, and the services only while a game is in front (the
/// only time the answer matters).
fn foreground_now(game_lock: bool, lock_programs: &[String], desktop: &Desktop) -> Captured {
    if !game_lock {
        return Captured::Foreground {
            fg: game_lock::Foreground::default(),
            kernel_anti_cheat: false,
        };
    }
    let mut fg = game_lock::foreground();
    fg.cursor = fg.cursor.map(|at| desktop.to_logical(at));
    let kernel_anti_cheat = game_lock::in_front(&fg, lock_programs, game_lock)
        && game_lock::kernel_anti_cheat_running();
    Captured::Foreground {
        fg,
        kernel_anti_cheat,
    }
}

/// Where the cursor is parked while away: logical, physical, and its
/// monitor's scale.
type Park = ((i32, i32), (i32, i32), f64);

/// The loop's state besides the driver: the channels, the cursor's park and
/// the keep-awake.
struct Glue {
    input: InputThread,
    desktop: Desktop,
    channels: HashMap<String, Channel>,
    /// When each channel in `channels` was opened (before its `Hello`), so
    /// only an answer since then makes its computer reachable.
    opened: HashMap<String, Instant>,
    /// When opening a channel to a computer last failed, or it last
    /// refused to be driven, and how long it waits to be greeted again.
    refused: HashMap<String, (Instant, Duration)>,
    /// The refusals in a row from each computer, for that wait.
    refusals: status::RefusalLog,
    /// This computer's name and logical screens, for a `Hello`.
    name: String,
    own: Vec<wire::Screen>,
    park: Option<Park>,
    /// Since when a new park waits for the input thread's word that it is
    /// parked: moves before it were measured from elsewhere.
    parking: Option<Instant>,
    motion: Motion,
    awake: KeepAwake,
    /// Files dragged across the edge (BK4): the setting, the catcher's
    /// state, its window's thread once a drag first needs it (or that it
    /// would not start), and where its drops come in.
    files_on: bool,
    catcher: Catcher,
    drop_catcher: Option<DropCatcher>,
    catcher_failed: bool,
    drops: (
        Sender<(Vec<PathBuf>, usize)>,
        Receiver<(Vec<PathBuf>, usize)>,
    ),
}

/// Whether Windows has a mouse button down that a drag could be carried
/// by: the left or the right (GetAsyncKeyState reads the buttons as they
/// are, not as swapped for a left hand, and Explorer drags with either).
fn drag_held() -> bool {
    windows_holds(0x01) || windows_holds(0x02)
}

impl Glue {
    /// The channel to `to`, opened (and greeted) if there is none.
    fn channel(&mut self, to: &str) -> Option<&mut Channel> {
        if !self.channels.contains_key(to) {
            let opened = Instant::now();
            match Channel::open(to) {
                Ok(mut channel) => {
                    // A greeting, so it says its screens, and a leave, so a
                    // computer left driven after a failed link lets go.
                    let hello = Msg::Hello {
                        version: wire::VERSION,
                        name: self.name.clone(),
                        screens: self.own.clone(),
                    };
                    if channel.send(&hello).and(channel.send(&Msg::Leave)).is_err() {
                        self.refused
                            .insert(to.to_string(), (Instant::now(), REOPEN_AFTER));
                        return None;
                    }
                    self.refused.remove(to);
                    self.opened.insert(to.to_string(), opened);
                    self.channels.insert(to.to_string(), channel);
                }
                Err(e) => {
                    tracing::debug!(to, "Shared input: no channel: {e}");
                    self.refused
                        .insert(to.to_string(), (Instant::now(), REOPEN_AFTER));
                    return None;
                }
            }
        }
        self.channels.get_mut(to)
    }

    /// Leave each computer (`only` that one, if given) that refused to be
    /// driven since its channel opened: home from it if it is driven, its
    /// releases on that channel, still open; then the channel goes, and it
    /// is greeted again after a wait that grows while it keeps refusing.
    fn take_refusals(&mut self, driver: &mut Driver, only: Option<&str>) {
        let refusals = link::refusals();
        let refusing: Vec<(String, u8)> = self
            .channels
            .keys()
            .filter(|id| only.is_none_or(|only| only == id.as_str()))
            .filter_map(|id| {
                let (reason, at) = refusals.get(id)?;
                (*at >= *self.opened.get(id)?).then(|| (id.clone(), *reason))
            })
            .collect();
        for (id, reason) in refusing {
            let actions = driver.refused(&id, reason);
            self.apply(driver, actions);
            self.channels.remove(&id);
            self.refusals.note(&id, reason);
            let wait = self.refusals.backoff(&id);
            self.refused.insert(id, (Instant::now(), wait));
        }
    }

    /// Do what the driver wants.
    fn apply(&mut self, driver: &mut Driver, actions: Vec<Action>) {
        let mut queue = std::collections::VecDeque::from(actions);
        while let Some(action) = queue.pop_front() {
            match action {
                Action::Send(to, msg) => {
                    // The computer driven is never greeted anew mid-visit:
                    // a new channel's leave would end the visit there.
                    let channel = if may_open(driver.away_on(), &to) {
                        self.channel(&to)
                    } else {
                        self.channels.get_mut(&to)
                    };
                    let sent = channel
                        .map(|channel| channel.send(&msg))
                        .unwrap_or_else(|| Err("no channel".to_string()));
                    if let Err(e) = sent {
                        tracing::debug!(to, "Shared input: not sent: {e}");
                        self.channels.remove(&to);
                        if driver.away_on() == Some(to.as_str()) {
                            // Home now; the rest queued for it is dropped.
                            queue.retain(|a| match a {
                                Action::Send(t, _) | Action::PushClipboard(t) => *t != to,
                                _ => true,
                            });
                            queue.extend(driver.on_send_error());
                        }
                    }
                }
                Action::SetMode(mode) => {
                    self.motion.reset();
                    self.input.set_mode(mode);
                }
                Action::ParkCursor(x, y) => {
                    // The input thread holds it there, and puts it back
                    // after each move: only a new park is sent.
                    if self.park.is_none_or(|(at, _, _)| at != (x, y)) {
                        let physical = self.desktop.to_physical((x, y));
                        self.motion.reset();
                        self.park = Some(((x, y), physical, self.desktop.scale_at(physical)));
                        self.parking = Some(Instant::now());
                        // A drag held here goes with the cursor: the
                        // catcher is under it before it lands (BK4).
                        let catch = self.catcher.parking(physical, self.files_on, drag_held());
                        self.catch(catch);
                        self.input.put_cursor(physical, true);
                    }
                }
                Action::WarpHome(x, y) => {
                    // An image still on its way to the computer left goes
                    // no further (BK3).
                    crate::clipboard_push::came_home();
                    // A drag carried back drops on Windows' own (BK4).
                    let catch = self.catcher.home();
                    self.catch(catch);
                    self.park = None;
                    self.parking = None;
                    self.input
                        .put_cursor(self.desktop.to_physical((x, y)), false);
                }
                Action::HideCursor(true) => hide_cursors(),
                Action::HideCursor(false) => restore_cursors(),
                Action::KeepAwake(true) => self.awake.hold(),
                Action::KeepAwake(false) => self.awake.release(),
                // Only ever after the keyboard and mouse came home.
                Action::LockHere => {
                    if let Err(e) = crate::privacy::lock_screen() {
                        tracing::warn!("Shared input: Win+L did not lock this computer: {e}");
                    }
                }
                // Sent as any message is, so a ping that cannot go brings
                // the keyboard and mouse home as a failed send does.
                Action::Ping(to) => queue.push_front(Action::Send(
                    to,
                    Msg::Ping {
                        t_us: link::now_us(),
                    },
                )),
                Action::PushClipboard(to) => {
                    // Read and sent off this thread: a clipboard another
                    // program holds open must not hold the mouse.
                    tokio::spawn(async move { crate::clipboard_push::push_now(&to).await });
                }
                Action::Badge(said_here) => {
                    let said = said_here.text.lines().collect::<Vec<_>>().join("; ");
                    tracing::info!("Shared input: {said}");
                    // Never over a game in front: a window above it could
                    // change how it draws, and the gaming lock promises a
                    // game runs as it would without Pocketdesk.
                    if !driver.game_held() {
                        super::badge::flash(&said_here, super::badge::SWITCH_FOR);
                    }
                }
            }
        }
    }

    /// Put the catcher up or down, starting its thread the first time it
    /// is wanted (once: one that would not start is not tried again).
    fn catch(&mut self, catch: Option<Catch>) {
        match catch {
            Some(Catch::Show(at)) => {
                if self.drop_catcher.is_none() && !self.catcher_failed {
                    self.drop_catcher = DropCatcher::start(self.drops.0.clone());
                    self.catcher_failed = self.drop_catcher.is_none();
                }
                match self.drop_catcher.as_mut() {
                    Some(catcher) => catcher.show(at),
                    None => {
                        self.catcher.home();
                    }
                }
            }
            Some(Catch::Hide) => {
                if let Some(catcher) = self.drop_catcher.as_mut() {
                    catcher.hide();
                }
            }
            None => {}
        }
    }

    /// The catcher looked at (each turn of the loop while it is up), and
    /// the files dropped on it sent on their way.
    fn files_dropped(&mut self, driver: &Driver) {
        if self.catcher.shown() {
            let catch = self.catcher.look(
                self.files_on,
                drag_held(),
                drop_windows::files_over(),
                Instant::now(),
            );
            self.catch(catch);
        }
        while let Ok((items, total)) = self.drops.1.try_recv() {
            let internet_files = crate::events::current_config().internet_files;
            match drag_files::on_drop(
                items,
                total,
                self.files_on,
                internet_files,
                driver.away_on(),
            ) {
                Dropped::Send {
                    to,
                    items,
                    left_out,
                } => {
                    let name = driver.name_of(&to);
                    tokio::spawn(send_dropped(to, name, items, left_out));
                }
                Dropped::Keep(Some(why)) => tell(why),
                Dropped::Keep(None) => {
                    tracing::debug!("Shared input: files dropped with no computer to go to")
                }
            }
        }
    }

    /// What the input thread saw, in the driver's logical pixels; `None`
    /// for a movement too small to count yet.
    fn logical(&mut self, driver: &Driver, captured: Captured) -> Option<Captured> {
        let Captured::Moved { x, y } = captured else {
            return Some(captured);
        };
        match self.park {
            Some((at, physical, scale)) if driver.away_on().is_some() => {
                // Moves from before the park are dropped, up to the input
                // thread's word that it parked (a move at the park itself),
                // or for at most a quarter of a second.
                if let Some(since) = self.parking {
                    if (x, y) == physical || since.elapsed() >= PARKING_WAIT {
                        self.parking = None;
                    } else {
                        return None;
                    }
                }
                let d = (x - physical.0, y - physical.1);
                let (dx, dy) = self.motion.step(d, scale);
                (dx != 0 || dy != 0).then_some(Captured::Moved {
                    x: at.0 + dx,
                    y: at.1 + dy,
                })
            }
            _ => {
                let (x, y) = self.desktop.to_logical((x, y));
                Some(Captured::Moved { x, y })
            }
        }
    }
}

/// How long a drop's sends are watched for one that did not go.
const DROP_WATCH: Duration = Duration::from_secs(30);
/// How often the sent list is looked at meanwhile.
const DROP_WATCH_EVERY: Duration = Duration::from_millis(500);

/// Set while a game holds the keyboard and mouse here, for the words on
/// a drop's sends that come after the cursor is home.
static GAME_HELD: AtomicBool = AtomicBool::new(false);

/// A word for the owner on files dragged across: logged, and flashed
/// unless a game is in front by then (as the driver's own badges).
fn tell(words: &str) {
    tracing::info!(
        "Shared input: {}",
        words.lines().collect::<Vec<_>>().join("; ")
    );
    if !GAME_HELD.load(Ordering::SeqCst) {
        badge::flash(&badge::of(&badge::Event::Note(words)), badge::SWITCH_FOR);
    }
}

/// Files dragged across and let go on the computer `to` (named `name` to
/// the owner): each queued for it as the desktop app's Send queues one, so
/// the server's checks, a folder's send and the sent list follow; then
/// watched a while, so one that does not go is told. Logged by count and
/// size, never by name.
async fn send_dropped(to: String, name: String, items: Vec<PathBuf>, left_out: usize) {
    let Ok(dir) = crate::state::state_dir() else {
        tell(&drag_files::not_sent(&name, "no state folder"));
        return;
    };
    let queued = {
        let dir = dir.clone();
        tokio::task::spawn_blocking(move || queue_dropped(&dir, &to, &items)).await
    };
    let Ok(queued) = queued else {
        return;
    };
    tracing::info!(
        files = queued.files,
        folders = queued.folders,
        bytes = queued.bytes,
        left_out,
        "Shared input: files dragged across, queued for the computer driven"
    );
    if queued.ids.is_empty() {
        tell(&drag_files::not_sent(&name, "they could not be queued"));
        return;
    }
    tell(&drag_files::sending(
        queued.files,
        queued.folders,
        &name,
        left_out,
    ));
    let deadline = tokio::time::Instant::now() + DROP_WATCH;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(DROP_WATCH_EVERY).await;
        let rows = {
            let dir = dir.clone();
            tokio::task::spawn_blocking(move || pocketdesk_core::local::sent::read(&dir))
                .await
                .unwrap_or_default()
        };
        match drag_files::outcome(&rows, &queued.ids) {
            Outcome::Waiting => {}
            Outcome::Sent => return,
            Outcome::Failed(reason) => {
                tell(&drag_files::not_sent(&name, &reason));
                return;
            }
        }
    }
}

/// What a drop queued: its requests' ids, how many files and folders, and
/// the files' bytes.
struct Queued {
    ids: Vec<String>,
    files: usize,
    folders: usize,
    bytes: u64,
}

/// Queue each of `items` for the computer `to` in the outbox.
fn queue_dropped(dir: &std::path::Path, to: &str, items: &[PathBuf]) -> Queued {
    let now = pocketdesk_core::local::pin::now_unix();
    let mut queued = Queued {
        ids: Vec::new(),
        files: 0,
        folders: 0,
        bytes: 0,
    };
    for item in items {
        let meta = std::fs::metadata(item).ok();
        match pocketdesk_core::local::outbox::request(dir, to, &item.to_string_lossy(), now) {
            Ok(request) => queued.ids.push(request.id),
            Err(e) => {
                tracing::warn!("Shared input: a file dragged across was not queued: {e}");
                continue;
            }
        }
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            queued.folders += 1;
        } else {
            queued.files += 1;
            queued.bytes += meta.map_or(0, |m| m.len());
        }
    }
    queued
}

/// Set once the server is stopping: no driver starts again, and the one
/// running brings the keyboard and mouse home and ends ([`stop`]).
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Set while a driver's thread runs.
static DRIVING: AtomicBool = AtomicBool::new(false);

/// How long, as a driver ends, what it just sent (the releases and the
/// leave) may take to be written to the link before its channels go.
const FLUSH_WAIT: Duration = Duration::from_millis(300);

/// `flag` set while this is held, and cleared when it is dropped however
/// its holder ends (a panic too).
struct Raised(&'static AtomicBool);

impl Raised {
    fn raise(flag: &'static AtomicBool) -> Raised {
        flag.store(true, Ordering::SeqCst);
        Raised(flag)
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Wait until `flag` is clear, at most `within`; whether it cleared.
async fn wait_lowered(flag: &AtomicBool, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while flag.load(Ordering::SeqCst) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// Runs `restore` (the owner's cursors back) when dropped: held by the
/// supervisor, so the cursors come back however it ends, the runtime
/// dropping it as the server exits too.
struct RestoreCursors(fn());

impl RestoreCursors {
    fn with(restore: fn()) -> RestoreCursors {
        RestoreCursors(restore)
    }
}

impl Drop for RestoreCursors {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// The server is stopping: the driver running brings the keyboard and
/// mouse home (releases and a leave to the computer driven, while the link
/// is still up) and ends, and none starts again. Waits at most `within`;
/// whether it ended in time.
pub async fn stop(within: Duration) -> bool {
    STOPPING.store(true, Ordering::SeqCst);
    wait_lowered(&DRIVING, within).await
}

/// The driver while `stop` is not set: capture, carry, and on stopping
/// bring everything home. Run on a thread of its own, inside `runtime`.
fn drive(stop: Arc<AtomicBool>, runtime: tokio::runtime::Handle) {
    let _driving = Raised::raise(&DRIVING);
    let _runtime = runtime.enter();
    let config = crate::events::current_config();
    let hotkey = pocketdesk_core::local::hotkey::parse(&config.shared_input_hotkey);
    if hotkey.is_none() && !config.shared_input_hotkey.trim().is_empty() {
        tracing::warn!(
            key = config.shared_input_hotkey,
            "Shared input: the switch key is not one; only the screen's edge switches"
        );
    }
    let (events, captured) = mpsc::channel();
    let input = InputThread::start(events, hotkey);
    if !input.is_running() {
        return;
    }
    let me = crate::kde::host_identity();
    let state_dir = crate::state::state_dir().ok();
    let desktop = Desktop::arrange(&crate::windows_desktop::monitors_scaled());
    let own = desktop.screens();
    let mut glue = Glue {
        input,
        desktop,
        channels: HashMap::new(),
        opened: HashMap::new(),
        refused: HashMap::new(),
        refusals: status::RefusalLog::default(),
        name: me.device_name.clone(),
        own,
        park: None,
        parking: None,
        motion: Motion::default(),
        awake: KeepAwake::default(),
        files_on: config.shared_input_files,
        catcher: Catcher::default(),
        drop_catcher: None,
        catcher_failed: false,
        drops: mpsc::channel(),
    };
    // The catcher's thread ready before a drag first needs it, so the
    // first drag carried away does not wait for it to start.
    if glue.files_on {
        glue.drop_catcher = DropCatcher::start(glue.drops.0.clone());
        glue.catcher_failed = glue.drop_catcher.is_none();
    }
    // The owner's arrangement, read again when its file changes.
    let mut saved = SavedLayout::default();
    let mut read_layout = |glue: &Glue| {
        let file = state_dir
            .as_deref()
            .map(pocketdesk_core::local::shared_input::read)
            .unwrap_or_default();
        layout_for(
            &me.device_id,
            &me.device_name,
            glue.own.clone(),
            &file,
            &link::linked(),
            state_dir.as_deref().and_then(|dir| saved.get(dir)),
        )
    };
    // What is in front is weighed before anything else, so a game already
    // there never sees a hook.
    let mut game_lock_on = config.shared_input_game_lock;
    let mut lock_programs = config.shared_input_lock_programs.clone();
    let mut driver = Driver::new(&me.device_id, read_layout(&glue))
        .with_game_lock(game_lock_on, lock_programs.clone());
    driver.set_clipboard(config.shared_input_clipboard);
    let actions = driver.on(foreground_now(game_lock_on, &lock_programs, &glue.desktop));
    glue.apply(&mut driver, actions);
    let start = driver.start();
    glue.apply(&mut driver, start);
    tracing::info!("Shared input: this computer's keyboard and mouse may go to the others");

    let mut last_input = Instant::now();
    let (mut active_at, mut link_at, mut layout_at, mut game_at) = (
        Instant::now(),
        Instant::now(),
        Instant::now(),
        Instant::now(),
    );
    let mut greeted_at: Option<Instant> = None;
    // Until when what is in front is looked at every 50 ms, after a window
    // came to the front.
    let mut settle_until: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) && !STOPPING.load(Ordering::SeqCst) {
        if !glue.input.is_alive() {
            tracing::warn!(
                "Shared input: the input thread stopped; the keyboard and mouse come home"
            );
            let actions = driver.shutdown();
            glue.apply(&mut driver, actions);
            status::set_keyboard_on(None);
            GAME_HELD.store(false, Ordering::SeqCst);
            // Dropping the glue restores the cursors; the supervisor starts a
            // fresh driver after a pause.
            return;
        }
        match captured.recv_timeout(Duration::from_millis(50)) {
            // Looked at at once (and again for a while), before any move
            // queued behind it can cross an edge.
            Ok(Captured::ForegroundChanged) => {
                let actions = driver.on(Captured::ForegroundChanged);
                glue.apply(&mut driver, actions);
                let now = foreground_now(game_lock_on, &lock_programs, &glue.desktop);
                let actions = driver.on(now);
                glue.apply(&mut driver, actions);
                game_at = Instant::now();
                settle_until = Some(game_at + GAME_SETTLE_FOR);
            }
            Ok(event) => {
                last_input = Instant::now();
                if let Some(event) = glue.logical(&driver, event) {
                    let actions = driver.on(event);
                    glue.apply(&mut driver, actions);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Files dragged across: the catcher looked at, its drops sent.
        glue.files_dropped(&driver);
        // The computer driven answering its pings, or not: one silent for
        // a second brings the keyboard and mouse home.
        let now = Instant::now();
        let answered: Vec<String> = glue
            .channels
            .iter_mut()
            .filter_map(|(id, channel)| {
                let mut any = false;
                while channel.try_pong().is_some() {
                    any = true;
                }
                any.then(|| id.clone())
            })
            .collect();
        for id in answered {
            driver.answered(&id, now);
        }
        let actions = driver.tick(now);
        glue.apply(&mut driver, actions);
        // A game in front keeps the keyboard and mouse here with no hooks;
        // one come to the front while away brings them home.
        let game_every = if settle_until.is_some_and(|until| Instant::now() < until) {
            GAME_SETTLE_EVERY
        } else {
            GAME_EVERY
        };
        if game_at.elapsed() >= game_every {
            game_at = Instant::now();
            let now = foreground_now(game_lock_on, &lock_programs, &glue.desktop);
            let actions = driver.on(now);
            glue.apply(&mut driver, actions);
        }
        // Who is at this computer, to every computer with a channel: the
        // driven one lets go after a second without a word.
        if active_at.elapsed() >= ACTIVE_EVERY {
            active_at = Instant::now();
            let idle =
                idle_ms().min(u32::try_from(last_input.elapsed().as_millis()).unwrap_or(u32::MAX));
            let to: Vec<String> = glue.channels.keys().cloned().collect();
            let actions = to
                .into_iter()
                .map(|to| Action::Send(to, Msg::Active { idle_ms: idle }))
                .collect();
            glue.apply(&mut driver, actions);
        }
        // Where the keyboard and mouse are, for the app's page.
        status::set_keyboard_on(driver.away_on());
        GAME_HELD.store(driver.game_held(), Ordering::SeqCst);
        // Away, a desktop switch (a secure prompt hides key-ups) or a link
        // that turned relayed or closed brings it home.
        if let Some(to) = driver.away_on().map(str::to_string)
            && link_at.elapsed() >= LINK_EVERY
        {
            link_at = Instant::now();
            if !on_default_desktop() {
                let actions = driver.on(Captured::DesktopSwitch);
                glue.apply(&mut driver, actions);
            }
            let why = match link::connection(&to) {
                None => Some("The link to that computer closed"),
                Some(conn) if !link::is_direct(&conn) => Some(
                    "The link to that computer went through a relay: the keyboard and mouse are \
                     shared only on a direct link",
                ),
                Some(_) => None,
            };
            if let Some(why) = why {
                let actions = driver.link_gone(&to, why);
                glue.apply(&mut driver, actions);
                glue.channels.remove(&to);
            }
            // It refused while driven: home within a quarter of a second.
            glue.take_refusals(&mut driver, Some(&to));
        }
        if layout_at.elapsed() >= LAYOUT_EVERY {
            layout_at = Instant::now();
            // The gaming lock's settings, taken without a restart.
            let config = crate::events::current_config();
            game_lock_on = config.shared_input_game_lock;
            lock_programs = config.shared_input_lock_programs;
            driver.set_clipboard(config.shared_input_clipboard);
            glue.files_on = config.shared_input_files;
            let actions = driver.set_game_lock(game_lock_on, lock_programs.clone());
            glue.apply(&mut driver, actions);
            let desktop = Desktop::arrange(&crate::windows_desktop::monitors_scaled());
            if desktop != glue.desktop {
                glue.desktop = desktop;
                glue.own = glue.desktop.screens();
                let screens = Msg::Screens(glue.own.clone());
                let to: Vec<String> = glue.channels.keys().cloned().collect();
                let actions = to
                    .into_iter()
                    .map(|to| Action::Send(to, screens.clone()))
                    .collect();
                glue.apply(&mut driver, actions);
            }
            // Greet the linked computers not greeted yet, so they say their
            // screens; ones refused are tried again a while later.
            if greeted_at.is_none_or(|at| at.elapsed() >= REOPEN_AFTER) {
                greeted_at = Some(Instant::now());
                let linked = link::linked();
                glue.refusals.keep_linked(&linked);
                for id in linked {
                    let refused = glue
                        .refused
                        .get(&id)
                        .is_some_and(|(at, wait)| at.elapsed() < *wait);
                    if !refused && driver.away_on() != Some(id.as_str()) {
                        let _ = glue.channel(&id);
                    }
                }
                // A computer driven whose link went comes home with it.
                if let Some(to) = driver.away_on().map(str::to_string)
                    && link::connection(&to).is_none()
                {
                    let actions = driver.link_gone(&to, "The link to that computer closed");
                    glue.apply(&mut driver, actions);
                }
                glue.channels.retain(|id, _| link::connection(id).is_some());
            }
            let layout = read_layout(&glue);
            let actions = driver.set_layout(layout);
            glue.apply(&mut driver, actions);
            // A computer that refused since its channel opened is left, and
            // greeted again a while later: it answers with its screens once
            // it takes input.
            glue.take_refusals(&mut driver, None);
            // The computers the cursor may cross onto: an open, direct
            // channel whose computer sent its screens since it opened.
            let open: Vec<OpenChannel> = glue
                .channels
                .keys()
                .filter_map(|id| {
                    Some(OpenChannel {
                        device_id: id.clone(),
                        opened: *glue.opened.get(id)?,
                        direct: link::connection(id).is_some_and(|conn| link::is_direct(&conn)),
                    })
                })
                .collect();
            let reachable = reachable(&open, &link::screens_heard());
            // One that answers takes input: its next refusal starts the
            // wait over.
            for id in &reachable {
                glue.refusals.clear(id);
            }
            let actions = driver.set_reachable(reachable.into_iter().collect());
            glue.apply(&mut driver, actions);
        }
    }
    let actions = driver.shutdown();
    glue.apply(&mut driver, actions);
    status::set_keyboard_on(None);
    GAME_HELD.store(false, Ordering::SeqCst);
    // What was just sent goes out before the channels do, for a moment at
    // most: the computer driven hears the leave now, not a second later.
    let deadline = Instant::now() + FLUSH_WAIT;
    for (_, channel) in glue.channels.drain() {
        channel.finish_within(deadline.saturating_duration_since(Instant::now()));
    }
    tracing::info!("Shared input: this computer's keyboard and mouse stay here");
    // Dropping the glue stops the input thread, restores the cursors and
    // lets the computer idle.
}

/// The driver on Windows for the server's life: running while shared input
/// and driving are both on (read again every second); turning either off,
/// or the server stopping ([`stop`]), brings the keyboard and mouse home
/// and stops it.
pub async fn supervise() {
    // Once at every start: a crash while away must not leave the owner
    // without a cursor. And once as it ends, however that is (the server
    // stopping drops it with the runtime, a driver still running or not).
    restore_cursors();
    let _cursors = RestoreCursors::with(restore_cursors);
    let runtime = tokio::runtime::Handle::current();
    let mut running: Option<(Arc<AtomicBool>, JoinHandle<()>)> = None;
    let mut retry_at: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let want = tokio::task::spawn_blocking(|| {
            let config = crate::events::current_config();
            config.shared_input && config.shared_input_drive
        })
        .await
        .unwrap_or(false);
        if running.as_ref().is_some_and(|(_, t)| t.is_finished()) {
            if let Some((_, thread)) = running.take() {
                let _ = thread.join();
            }
            // It ended by itself (its input thread did not start): started
            // again a while later, if still wanted.
            retry_at = Some(Instant::now() + RESTART_AFTER);
            continue;
        }
        match (&running, want && !STOPPING.load(Ordering::SeqCst)) {
            (None, true) if retry_at.is_none_or(|at| Instant::now() >= at) => {
                retry_at = None;
                let stop = Arc::new(AtomicBool::new(false));
                let (flag, handle) = (Arc::clone(&stop), runtime.clone());
                match std::thread::Builder::new()
                    .name("pd-shared-drive".into())
                    .spawn(move || drive(flag, handle))
                {
                    Ok(thread) => running = Some((stop, thread)),
                    Err(e) => tracing::warn!("Shared input: the driver did not start: {e}"),
                }
            }
            (Some(_), false) => {
                if let Some((stop, thread)) = running.take() {
                    stop.store(true, Ordering::SeqCst);
                    let _ = tokio::task::spawn_blocking(move || thread.join()).await;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::hotkey::{MOD_NOREPEAT, parse};
    use std::sync::atomic::AtomicUsize;

    static RESTORED: AtomicUsize = AtomicUsize::new(0);
    static SPAWNED: AtomicUsize = AtomicUsize::new(0);

    /// The cursors are restored when the input thread starts and when it is
    /// dropped; counted by a stand-in, with no thread started (the tests
    /// never hook the owner's computer or touch its cursor).
    #[test]
    fn cursor_restore_on_start_and_drop() {
        let (events, _captured) = mpsc::channel();
        let seams = Seams {
            restore: || {
                RESTORED.fetch_add(1, Ordering::SeqCst);
            },
            spawn: |_, _| {
                SPAWNED.fetch_add(1, Ordering::SeqCst);
                None
            },
        };
        let thread = InputThread::start_with(events, parse("ctrl+alt+right"), seams);
        assert_eq!(RESTORED.load(Ordering::SeqCst), 1, "restored at start");
        assert_eq!(SPAWNED.load(Ordering::SeqCst), 1);
        assert!(!thread.is_running());
        // With no thread, a mode goes nowhere.
        thread.set_mode(Mode::Away);
        drop(thread);
        assert_eq!(RESTORED.load(Ordering::SeqCst), 2, "restored on drop");
    }

    static GUARDED: AtomicUsize = AtomicUsize::new(0);

    /// The cursors come back however the supervisor ends: its guard runs
    /// them back when dropped (counted by a stand-in).
    #[test]
    fn the_cursor_guard_restores_when_dropped() {
        let guard = RestoreCursors::with(|| {
            GUARDED.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(GUARDED.load(Ordering::SeqCst), 0);
        drop(guard);
        assert_eq!(GUARDED.load(Ordering::SeqCst), 1);
    }

    /// The server stopping waits for the driver to end, but only so long.
    #[tokio::test]
    async fn a_stop_waits_for_the_driver_within_its_bound() {
        static RUNNING: AtomicBool = AtomicBool::new(false);
        let raised = Raised::raise(&RUNNING);
        assert!(RUNNING.load(Ordering::SeqCst));
        let ending = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(raised);
        });
        assert!(wait_lowered(&RUNNING, Duration::from_secs(5)).await);
        ending.join().unwrap();
        // One that does not end in time is left.
        let _stuck = Raised::raise(&RUNNING);
        let started = Instant::now();
        assert!(!wait_lowered(&RUNNING, Duration::from_millis(100)).await);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A send to the computer driven never opens a channel for it; to any
    /// other, and at home, it may.
    #[test]
    fn the_computer_driven_is_never_greeted_anew() {
        assert!(!may_open(Some("L"), "L"));
        assert!(may_open(Some("L"), "M"));
        assert!(may_open(None, "L"));
    }

    /// Win+L while away locks this computer, whatever else is held with
    /// the Win key; L alone, or with other modifiers, goes on.
    #[test]
    fn win_l_locks_here() {
        let l = u32::from(b'L');
        assert!(locks_here(MOD_WIN, l));
        assert!(locks_here(MOD_WIN | MOD_SHIFT, l));
        assert!(!locks_here(0, l));
        assert!(!locks_here(MOD_CONTROL | MOD_ALT, l));
        assert!(!locks_here(MOD_WIN, u32::from(b'K')));
    }

    /// Away needs both hooks; watching needs only the mouse's, and a
    /// missing one there is only logged (nothing is swallowed).
    #[test]
    fn away_without_both_hooks_is_a_failure() {
        assert!(!hooks_missing(Mode::Away, true, true));
        assert!(hooks_missing(Mode::Away, true, false));
        assert!(hooks_missing(Mode::Away, false, true));
        assert!(!hooks_missing(Mode::Watch, false, false));
        assert!(!hooks_missing(Mode::Off, false, false));
    }

    fn state(generation: u64) -> ThreadState {
        let (events, _) = mpsc::channel();
        ThreadState {
            events,
            hotkey: None,
            mode: Mode::Off,
            mouse: None,
            keyboard: None,
            foreground: None,
            instance: HINSTANCE::default(),
            mods: 0,
            hotkey_down: false,
            wheel: (0, 0),
            locked: false,
            asleep: false,
            park: None,
            generation,
        }
    }

    fn generation(slot: &Option<ThreadState>) -> Option<u64> {
        slot.as_ref().map(|s| s.generation)
    }

    /// An input thread left behind (stuck as it started) that gets going
    /// later neither takes a newer thread's state nor clears it as it ends.
    #[test]
    fn an_older_input_thread_leaves_a_newer_ones_state_alone() {
        let mut slot = None;
        install_state(&mut slot, state(2));
        install_state(&mut slot, state(1));
        assert_eq!(generation(&slot), Some(2), "not taken by the older");
        clear_own(&mut slot, 1);
        assert_eq!(generation(&slot), Some(2), "not cleared by the older");
        clear_own(&mut slot, 2);
        assert_eq!(generation(&slot), None, "cleared by its own");
        // A newer thread takes over from an older one left in place.
        install_state(&mut slot, state(3));
        install_state(&mut slot, state(4));
        assert_eq!(generation(&slot), Some(4));
    }

    #[test]
    fn the_switch_key_needs_exactly_its_modifiers() {
        let key = parse("ctrl+alt+right").unwrap();
        assert_eq!(key.modifiers & MOD_NOREPEAT, MOD_NOREPEAT);
        assert!(hotkey_hit(MOD_CONTROL | MOD_ALT, 0x27, &key));
        assert!(!hotkey_hit(MOD_CONTROL, 0x27, &key));
        assert!(!hotkey_hit(MOD_CONTROL | MOD_ALT | MOD_SHIFT, 0x27, &key));
        assert!(!hotkey_hit(MOD_CONTROL | MOD_ALT, 0x25, &key));
        assert_eq!(modifier_of(u32::from(VK_LCONTROL.0)), MOD_CONTROL);
        assert_eq!(modifier_of(u32::from(VK_RMENU.0)), MOD_ALT);
        assert_eq!(modifier_of(u32::from(VK_RWIN.0)), MOD_WIN);
        assert_eq!(modifier_of(0x27), 0);
    }

    #[test]
    fn a_point_packs_into_one_message() {
        for at in [(0, 0), (-1920, -80), (3455, 959), (i32::MIN, i32::MAX)] {
            assert_eq!(unpack(pack(at)), at);
        }
    }

    #[test]
    fn a_fine_wheel_turns_by_whole_notches() {
        let mut rest = 0;
        assert_eq!(notches(&mut rest, 120), 1);
        assert_eq!(notches(&mut rest, 40), 0);
        assert_eq!(notches(&mut rest, 40), 0);
        assert_eq!(notches(&mut rest, 40), 1);
        assert_eq!(rest, 0);
        assert_eq!(notches(&mut rest, -240), -2);
        assert_eq!(notches(&mut rest, -60), 0);
        assert_eq!(notches(&mut rest, 60), 0);
        assert_eq!(rest, 0);
    }
}
