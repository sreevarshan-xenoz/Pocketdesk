//! The keyboard grab of the window that shows another computer's screen,
//! on Windows (PC4).
//!
//! While the window controls that computer, has the focus and fills the
//! screen, and "Send Win and Alt+Tab there" (`computer_view_grab_keys`) is
//! on, a `WH_KEYBOARD_LL` hook sends the Windows key, Alt+Tab, Alt+Esc and
//! Ctrl+Esc to that computer and keeps them from this one, as RDP's
//! full-screen keyboard hook does. While a Windows key it took is held,
//! every key goes there with it, so the other computer's Super shortcuts
//! (a tiling window manager's) work from here. Pressing and letting go of
//! Ctrl+Alt+Shift, with nothing else, gives the keys back until the window
//! leaves full screen or loses the focus.
//!
//! The hook stands down while the shared keyboard and mouse have this
//! keyboard on another computer (the server's own hook has it then), passes
//! every injected key on, and is taken out when the focus goes, the window
//! leaves full screen or closes, or the window's thread stops answering for
//! more than [`STALL`]: a watchdog on the hook's own thread unhooks it, so a
//! stuck window never strands this computer's keyboard.
//!
//! The decisions ([`Grab`], [`HookKeys`], [`stalled`]) are pure and tested
//! here; the hook itself ([`install`]) is never installed by a test.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::input::windows_scan::evdev_from_scan;

/// The window's thread silent this long, and the hook is taken out.
pub const STALL: Duration = Duration::from_secs(1);
/// How often the window says it is alive while the hook is in, and how
/// often the watchdog looks.
pub const BEAT_EVERY: Duration = Duration::from_millis(250);

/// Windows' virtual keys the grab looks at.
const VK_TAB: u32 = 0x09;
const VK_ESCAPE: u32 = 0x1B;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_LMENU: u32 = 0xA4;
const VK_RMENU: u32 = 0xA5;

/// What the grab is decided on, each pass of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Situation {
    /// The window has the focus.
    pub focused: bool,
    /// The window fills the screen.
    pub fullscreen: bool,
    /// "Send Win and Alt+Tab there" is on.
    pub setting: bool,
    /// The window controls the other computer: its Control on, View only
    /// off, connected, and the view not ended.
    pub controls: bool,
    /// The shared keyboard and mouse have this keyboard on another
    /// computer: the server's hook has it, and this one stands down.
    pub shared_across: bool,
}

/// The grab across the window's passes: whether the release chord took the
/// keys back (until the focus goes or the window leaves full screen).
#[derive(Debug, Default)]
pub struct Grab {
    released: bool,
}

impl Grab {
    /// Whether the hook should be in now, in `now`'s situation.
    pub fn wanted(&mut self, now: Situation) -> bool {
        let in_front = now.focused && now.fullscreen;
        if !in_front {
            self.released = false;
        }
        in_front && now.setting && now.controls && !now.shared_across && !self.released
    }

    /// Ctrl+Alt+Shift, or the watchdog: the keys stay here until the
    /// window leaves full screen or loses the focus.
    pub fn release(&mut self) {
        self.released = true;
    }
}

/// A key as the hook saw it (never an injected one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub vk: u32,
    pub scan: u16,
    pub extended: bool,
    pub down: bool,
}

/// What the hook does with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Windows has it, as without the hook.
    Pass,
    /// Kept from Windows, and sent to the other computer as this evdev
    /// code going down or up (`None`: a key with no code, kept all the
    /// same).
    Take(Option<(u16, bool)>),
    /// Windows has it, and the grab lets go: Ctrl+Alt+Shift was pressed and
    /// let go with nothing else.
    Release,
}

fn is_win(vk: u32) -> bool {
    matches!(vk, VK_LWIN | VK_RWIN)
}

fn is_ctrl(vk: u32) -> bool {
    matches!(vk, VK_LCONTROL | VK_RCONTROL)
}

fn is_alt(vk: u32) -> bool {
    matches!(vk, VK_LMENU | VK_RMENU)
}

fn is_shift(vk: u32) -> bool {
    matches!(vk, VK_LSHIFT | VK_RSHIFT)
}

/// The hook's view of the keyboard: the modifiers Windows has down, the
/// keys whose press went to the other computer, and the release chord.
#[derive(Debug, Default)]
pub struct HookKeys {
    /// Ctrl, Alt and Shift down here (Windows has them).
    held: BTreeSet<u32>,
    /// Keys whose press went there: their repeats and release go too.
    taken: BTreeSet<u32>,
    /// Ctrl, Alt and Shift went down together, nothing else since.
    chord: bool,
}

impl HookKeys {
    /// Starting with `held` down already (read as the hook goes in).
    pub fn with_held(held: impl IntoIterator<Item = u32>) -> Self {
        Self {
            held: held
                .into_iter()
                .filter(|vk| is_ctrl(*vk) || is_alt(*vk) || is_shift(*vk))
                .collect(),
            ..Self::default()
        }
    }

    fn holds(&self, which: fn(u32) -> bool) -> bool {
        self.held.iter().any(|vk| which(*vk))
    }

    /// A press taken that could not be said there: Windows has it after
    /// all, and its release too (review I3).
    fn untake(&mut self, vk: u32) {
        self.taken.remove(&vk);
    }

    /// What to do with `stroke`.
    pub fn key(&mut self, stroke: Stroke) -> Verdict {
        let code = evdev_from_scan(stroke.scan, stroke.extended).or(match stroke.vk {
            VK_LWIN => Some(125),
            VK_RWIN => Some(126),
            _ => None,
        });
        let vk = stroke.vk;
        if !stroke.down {
            if self.taken.remove(&vk) {
                return Verdict::Take(code.map(|c| (c, false)));
            }
            let chord = self.chord && (is_ctrl(vk) || is_alt(vk) || is_shift(vk));
            self.held.remove(&vk);
            if chord {
                self.chord = false;
                return Verdict::Release;
            }
            return Verdict::Pass;
        }
        // A key held repeats: a key taken goes on being taken.
        if self.taken.contains(&vk) {
            return Verdict::Take(code.map(|c| (c, true)));
        }
        let win_taken = self.taken.iter().any(|v| is_win(*v));
        let take = is_win(vk)
            || win_taken
            || (vk == VK_TAB && self.holds(is_alt))
            || (vk == VK_ESCAPE && (self.holds(is_alt) || self.holds(is_ctrl)));
        if take {
            self.chord = false;
            self.taken.insert(vk);
            return Verdict::Take(code.map(|c| (c, true)));
        }
        if is_ctrl(vk) || is_alt(vk) || is_shift(vk) {
            self.held.insert(vk);
            if self.holds(is_ctrl) && self.holds(is_alt) && self.holds(is_shift) {
                self.chord = true;
            }
        } else {
            self.chord = false;
        }
        Verdict::Pass
    }
}

/// Whether the window, last alive at `beat_ms`, has stalled at `now_ms`
/// (both on [`clock_ms`]'s clock).
pub fn stalled(beat_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(beat_ms) > STALL.as_millis() as u64
}

/// Milliseconds on a clock of this program's own.
fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// What the hook heard for the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// An evdev code down or up, for the other computer.
    Key(u16, bool),
    /// Ctrl+Alt+Shift: the keys come back here.
    Release,
}

/// The hook, on a thread of its own; dropped, it is taken out.
pub struct Hook {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
    /// Tells the thread to take the hook out at its next message.
    stop: Arc<AtomicBool>,
    beat: Arc<AtomicU64>,
    pub heard: Receiver<Heard>,
}

impl Hook {
    /// The window is alive: the watchdog waits again.
    pub fn beat(&self) {
        self.beat.store(clock_ms(), Ordering::Release);
    }

    /// Whether the hook is still in (the watchdog takes it out on a stall).
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // Only to a thread still running: a finished one's id may be
            // another program's thread now (review M3). The message only
            // wakes it to read the flag.
            if !thread.is_finished() {
                // SAFETY: a plain message to a thread this owns, whose
                // queue exists (it made it before saying it was ready).
                let _ =
                    unsafe { PostThreadMessageW(self.thread_id, WM_STOP, WPARAM(0), LPARAM(0)) };
            }
            let _ = thread.join();
        }
    }
}

/// Wakes the hook's thread to read its stop flag.
const WM_STOP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

/// How the window puts the hook in: the system's, or in a test a stand-in
/// that never does (a test must not hook this computer's keyboard).
pub type Installer = fn(&egui::Context, egui::ViewportId) -> Option<Hook>;

/// The installer for this build: [`install`], or in tests [`refuse`].
pub const INSTALL: Installer = if cfg!(test) { refuse } else { install };

/// A test's installer: counts the ask, puts nothing in.
pub fn refuse(_ctx: &egui::Context, _viewport: egui::ViewportId) -> Option<Hook> {
    REFUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(std::thread::current().id());
    None
}

/// The threads [`refuse`] was asked on, once an ask (each test runs on a
/// thread of its own).
static REFUSED: std::sync::Mutex<Vec<std::thread::ThreadId>> = std::sync::Mutex::new(Vec::new());

/// How many times [`refuse`] was asked to put the hook in on this thread.
#[cfg(test)]
pub fn refused_here() -> usize {
    let here = std::thread::current().id();
    REFUSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|id| **id == here)
        .count()
}

/// What the hook procedure works with, on the hook's thread.
struct HookState {
    keys: HookKeys,
    heard: Sender<Heard>,
    /// Wakes the window, from a small thread of its own: the hook's thread
    /// never calls into egui (review I2).
    wake: Sender<()>,
    beat: Arc<AtomicU64>,
    thread_id: u32,
    /// The window the grab is for (its HWND, as a number): with another
    /// window in front, every key is Windows' (review I1).
    window: isize,
    /// The window stopped listening (its end of `heard` is gone): every key
    /// is Windows' from then on (review I3).
    gone: bool,
}

/// The hook's state, for its procedure: only the hook's thread touches it
/// (one hook at a time; a window has one).
static HOOK_STATE: std::sync::Mutex<Option<HookState>> = std::sync::Mutex::new(None);

/// What the hook procedure does with `stroke`, on the hook's thread `here`
/// at `now_ms` with the window `foreground` in front (its HWND, as a
/// number), with the hook's state `slot`: whether to take it from
/// Windows (the window is woken when something was said to it, by the
/// waker's thread). A key is Windows' when the state is another thread's (a hook
/// left from an earlier window, on its way out), another window is in
/// front (the focus moved before the window's next pass took the hook
/// out), or the window's beat is stale (the window stopped answering; the
/// watchdog takes the hook out).
fn decide(
    slot: &mut Option<HookState>,
    here: u32,
    now_ms: u64,
    foreground: isize,
    stroke: Stroke,
) -> bool {
    let Some(state) = slot.as_mut().filter(|s| s.thread_id == here) else {
        return false;
    };
    if state.gone || foreground != state.window {
        return false;
    }
    if stalled(state.beat.load(Ordering::Acquire), now_ms) {
        return false;
    }
    // A key is taken only when it was said there: one nobody hears stays
    // Windows' (review I3).
    let (heard, take) = match state.keys.key(stroke) {
        Verdict::Pass => return false,
        Verdict::Take(None) => return true,
        Verdict::Take(Some((code, down))) => (Heard::Key(code, down), true),
        Verdict::Release => (Heard::Release, false),
    };
    if state.heard.send(heard).is_err() {
        state.gone = true;
        if stroke.down {
            state.keys.untake(stroke.vk);
        }
        return false;
    }
    let _ = state.wake.send(());
    take
}

unsafe extern "system" fn keyboard_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, WM_KEYDOWN,
        WM_SYSKEYDOWN,
    };
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            let message = wparam.0 as u32;
            let stroke = Stroke {
                vk: info.vkCode,
                scan: info.scanCode as u16,
                extended: info.flags.0 & LLKHF_EXTENDED.0 != 0,
                down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
            };
            // SAFETY: this thread's id.
            let here = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            // SAFETY: reads which window is in front; it does not block.
            let foreground =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0
                    as isize;
            let take = match HOOK_STATE.try_lock() {
                Ok(mut slot) => decide(&mut slot, here, clock_ms(), foreground, stroke),
                Err(_) => false,
            };
            if take {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: passes the key on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The Ctrl, Alt and Shift keys Windows has down now.
fn modifiers_down() -> Vec<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    [
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
    ]
    .into_iter()
    // SAFETY: reads a key's state.
    .filter(|vk| (unsafe { GetAsyncKeyState(*vk as i32) } as u16) & 0x8000 != 0)
    .collect()
}

/// Take the hook's state out, if it is the thread `thread_id`'s: a later
/// hook's is that hook's.
fn clear_own(thread_id: u32) {
    let mut slot = HOOK_STATE.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|s| s.thread_id == thread_id) {
        slot.take();
    }
}

/// The window in front, when it is one of this program's (the computer
/// window, as the grab goes in only while it has the focus): its HWND, as
/// a number.
fn own_foreground() -> Option<isize> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: reads which window is in front, and its process.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    let mut process = 0u32;
    // SAFETY: as above, into a local.
    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
    // SAFETY: this process's id.
    (process == unsafe { GetCurrentProcessId() }).then_some(window.0 as isize)
}

/// Put the hook in, on a thread of its own, for the window `viewport`:
/// `None` if it could not go in (or another program's window is in
/// front). The watchdog on that thread takes it out
/// when the window's beat stops for [`STALL`].
pub fn install(ctx: &egui::Context, viewport: egui::ViewportId) -> Option<Hook> {
    install_for(ctx, viewport, own_foreground()?)
}

/// Put the hook in for the window `window` (its HWND, as a number).
fn install_for(ctx: &egui::Context, viewport: egui::ViewportId, window: isize) -> Option<Hook> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMessageW, KillTimer, MSG, PM_NOREMOVE, PeekMessageW, SetTimer, SetWindowsHookExW,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_TIMER,
    };
    let (heard_tx, heard) = mpsc::channel();
    let (wake_tx, wake) = mpsc::channel::<()>();
    // The waker: the only thread here that calls into egui. It ends when
    // the hook's state goes, waking the window once more to see that.
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("pd-view-grab-wake".into())
        .spawn(move || {
            while wake.recv().is_ok() {
                ctx.request_repaint_of(viewport);
            }
            ctx.request_repaint_of(viewport);
        })
        .ok()?;
    let (ready_tx, ready) = mpsc::channel::<Option<u32>>();
    let beat = Arc::new(AtomicU64::new(clock_ms()));
    let thread_beat = beat.clone();
    // Set by an install that gave up waiting (review I3), or as the hook is
    // dropped (review M3): the thread then puts no hook in, or takes it
    // out at its next message.
    let abandoned = Arc::new(AtomicBool::new(false));
    let thread_abandoned = abandoned.clone();
    let thread = std::thread::Builder::new()
        .name("pd-view-grab".into())
        .spawn(move || {
            // SAFETY: this thread's id, and its message queue made before
            // anyone is told to post to it.
            let thread_id = unsafe { GetCurrentThreadId() };
            let mut msg = MSG::default();
            // SAFETY: as above.
            let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
            // SAFETY: this module's handle.
            let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
                let _ = ready_tx.send(None);
                return;
            };
            if thread_abandoned.load(Ordering::Acquire) {
                let _ = ready_tx.send(None);
                return;
            }
            // The state goes in just before the hook (the procedure needs
            // it from its first key), and out again if the hook does not
            // (review M2).
            *HOOK_STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(HookState {
                keys: HookKeys::with_held(modifiers_down()),
                heard: heard_tx,
                wake: wake_tx,
                beat: thread_beat.clone(),
                thread_id,
                window,
                gone: false,
            });
            // SAFETY: a hook procedure of this module, on this thread, which
            // runs a message loop below.
            let Ok(hook) = (unsafe {
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(module.into()), 0)
            }) else {
                clear_own(thread_id);
                let _ = ready_tx.send(None);
                return;
            };
            // SAFETY: a timer of this thread's own (no window).
            let timer = unsafe { SetTimer(None, 0, BEAT_EVERY.as_millis() as u32, None) };
            let _ = ready_tx.send(Some(thread_id));
            loop {
                // SAFETY: this thread's queue.
                let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if got.0 <= 0 {
                    break;
                }
                if thread_abandoned.load(Ordering::Acquire) {
                    break;
                }
                if msg.message == WM_TIMER
                    && stalled(thread_beat.load(Ordering::Acquire), clock_ms())
                {
                    // The watchdog: the window stopped answering.
                    break;
                }
            }
            // SAFETY: the hook and timer this thread put in.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
            if timer != 0 {
                // SAFETY: as above.
                let _ = unsafe { KillTimer(None, timer) };
            }
            // Its waker ends with its state, and the window looks again.
            clear_own(thread_id);
        })
        .ok()?;
    match ready.recv_timeout(STALL) {
        Ok(Some(thread_id)) => Some(Hook {
            thread_id,
            thread: Some(thread),
            stop: abandoned,
            beat,
            heard,
        }),
        // It could not go in, or did not say in time: told to give up, the
        // thread puts no hook in, or takes it out at its next look.
        _ => {
            abandoned.store(true, Ordering::Release);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front() -> Situation {
        Situation {
            focused: true,
            fullscreen: true,
            setting: true,
            controls: true,
            shared_across: false,
        }
    }

    /// The grab is in only with the window in front, full screen,
    /// controlling, the setting on and shared input home.
    #[test]
    fn the_grab_wants_focus_full_screen_the_setting_and_control() {
        let mut grab = Grab::default();
        assert!(grab.wanted(front()));
        for (what, situation) in [
            (
                "no focus",
                Situation {
                    focused: false,
                    ..front()
                },
            ),
            (
                "windowed",
                Situation {
                    fullscreen: false,
                    ..front()
                },
            ),
            (
                "setting off",
                Situation {
                    setting: false,
                    ..front()
                },
            ),
            (
                "view only, or no control",
                Situation {
                    controls: false,
                    ..front()
                },
            ),
            (
                "shared input across",
                Situation {
                    shared_across: true,
                    ..front()
                },
            ),
        ] {
            assert!(!grab.wanted(situation), "{what}");
        }
        assert!(grab.wanted(front()), "and back");
    }

    /// Ctrl+Alt+Shift (or a stall) gives the keys back until the window
    /// loses the focus or leaves full screen; then the grab is in again.
    #[test]
    fn a_release_holds_until_the_window_leaves_the_front() {
        let mut grab = Grab::default();
        grab.release();
        assert!(!grab.wanted(front()));
        assert!(!grab.wanted(front()), "still");
        assert!(!grab.wanted(Situation {
            focused: false,
            ..front()
        }));
        assert!(grab.wanted(front()), "focus came back");
        grab.release();
        assert!(!grab.wanted(Situation {
            fullscreen: false,
            ..front()
        }));
        assert!(grab.wanted(front()), "full screen again");
        // The setting or control going does not forget a release.
        grab.release();
        assert!(!grab.wanted(Situation {
            setting: false,
            ..front()
        }));
        assert!(!grab.wanted(front()));
    }

    fn stroke(vk: u32, scan: u16, extended: bool, down: bool) -> Stroke {
        Stroke {
            vk,
            scan,
            extended,
            down,
        }
    }
    const WIN: (u32, u16, bool) = (VK_LWIN, 0x5B, true);
    const ALT: (u32, u16, bool) = (VK_LMENU, 0x38, false);
    const CTRL: (u32, u16, bool) = (VK_LCONTROL, 0x1D, false);
    const SHIFT: (u32, u16, bool) = (VK_LSHIFT, 0x2A, false);
    const TAB: (u32, u16, bool) = (VK_TAB, 0x0F, false);
    const ESC: (u32, u16, bool) = (VK_ESCAPE, 0x01, false);
    const KEY_E: (u32, u16, bool) = (0x45, 0x12, false);
    const KEY_S: (u32, u16, bool) = (0x53, 0x1F, false);

    fn run(keys: &mut HookKeys, strokes: &[((u32, u16, bool), bool)]) -> Vec<Verdict> {
        strokes
            .iter()
            .map(|((vk, scan, ext), down)| keys.key(stroke(*vk, *scan, *ext, *down)))
            .collect()
    }

    /// The Windows key goes there, alone or with what is pressed while it
    /// is held (Win+E, Win+Shift+S): a tiling window manager's Super keys.
    #[test]
    fn the_windows_key_and_its_shortcuts_go_there() {
        let mut keys = HookKeys::default();
        assert_eq!(
            run(&mut keys, &[(WIN, true), (WIN, true), (WIN, false)]),
            [
                Verdict::Take(Some((125, true))),
                Verdict::Take(Some((125, true))),
                Verdict::Take(Some((125, false))),
            ],
            "held, it repeats there"
        );
        assert_eq!(
            run(
                &mut keys,
                &[
                    (WIN, true),
                    (SHIFT, true),
                    (KEY_S, true),
                    (KEY_S, false),
                    (SHIFT, false),
                    (WIN, false),
                    (KEY_E, true),
                    (KEY_E, false),
                ]
            ),
            [
                Verdict::Take(Some((125, true))),
                Verdict::Take(Some((42, true))),
                Verdict::Take(Some((31, true))),
                Verdict::Take(Some((31, false))),
                Verdict::Take(Some((42, false))),
                Verdict::Take(Some((125, false))),
                Verdict::Pass,
                Verdict::Pass,
            ],
            "with the Windows key let go, E is typed through the window again"
        );
        let right = keys.key(stroke(VK_RWIN, 0x5C, true, true));
        assert_eq!(right, Verdict::Take(Some((126, true))));
    }

    /// Alt+Tab, Alt+Esc and Ctrl+Esc: the modifier is Windows' (the window
    /// sends it there as it always did), the Tab or Esc goes there alone.
    #[test]
    fn alt_tab_alt_esc_and_ctrl_esc_go_there() {
        let mut keys = HookKeys::default();
        assert_eq!(
            run(
                &mut keys,
                &[
                    (ALT, true),
                    (TAB, true),
                    (TAB, false),
                    (TAB, true),
                    (ALT, false),
                    (TAB, false),
                ]
            ),
            [
                Verdict::Pass,
                Verdict::Take(Some((15, true))),
                Verdict::Take(Some((15, false))),
                Verdict::Take(Some((15, true))),
                Verdict::Pass,
                Verdict::Take(Some((15, false))),
            ],
            "a Tab taken comes up there, even after Alt"
        );
        assert_eq!(
            run(
                &mut keys,
                &[(ALT, true), (ESC, true), (ESC, false), (ALT, false)]
            ),
            [
                Verdict::Pass,
                Verdict::Take(Some((1, true))),
                Verdict::Take(Some((1, false))),
                Verdict::Pass,
            ]
        );
        assert_eq!(
            run(
                &mut keys,
                &[(CTRL, true), (ESC, true), (ESC, false), (CTRL, false)]
            ),
            [
                Verdict::Pass,
                Verdict::Take(Some((1, true))),
                Verdict::Take(Some((1, false))),
                Verdict::Pass,
            ]
        );
        // Tab and Esc alone, or Ctrl+Tab, are the window's as ever.
        assert_eq!(
            run(
                &mut keys,
                &[(TAB, true), (TAB, false), (ESC, true), (ESC, false)]
            ),
            [Verdict::Pass; 4]
        );
        assert_eq!(
            run(
                &mut keys,
                &[(CTRL, true), (TAB, true), (TAB, false), (CTRL, false)]
            ),
            [Verdict::Pass; 4]
        );
    }

    /// Alt held before the hook went in still makes Alt+Tab.
    #[test]
    fn a_modifier_held_as_the_hook_goes_in_counts() {
        let mut keys = HookKeys::with_held([VK_LMENU, 0x41]);
        assert_eq!(
            keys.key(stroke(TAB.0, TAB.1, TAB.2, true)),
            Verdict::Take(Some((15, true)))
        );
    }

    /// Ctrl+Alt+Shift pressed and let go with nothing else gives the keys
    /// back; with a key between (Ctrl+Alt+Shift+S, the stats) it does not.
    #[test]
    fn ctrl_alt_shift_alone_is_the_release() {
        let mut keys = HookKeys::default();
        assert_eq!(
            run(
                &mut keys,
                &[(CTRL, true), (ALT, true), (SHIFT, true), (SHIFT, false)]
            ),
            [
                Verdict::Pass,
                Verdict::Pass,
                Verdict::Pass,
                Verdict::Release
            ]
        );
        assert_eq!(
            run(&mut keys, &[(ALT, false), (CTRL, false)]),
            [Verdict::Pass, Verdict::Pass],
            "once"
        );
        assert_eq!(
            run(
                &mut keys,
                &[
                    (SHIFT, true),
                    (ALT, true),
                    (CTRL, true),
                    (KEY_S, true),
                    (KEY_S, false),
                    (CTRL, false),
                    (ALT, false),
                    (SHIFT, false),
                ]
            ),
            [Verdict::Pass; 8],
            "the stats' chord is not a release"
        );
        assert_eq!(
            run(&mut keys, &[(CTRL, true), (ALT, false), (CTRL, false)]),
            [Verdict::Pass; 3],
            "two of them are not"
        );
    }

    /// The watchdog: more than a second without the window's beat.
    #[test]
    fn a_window_silent_past_a_second_has_stalled() {
        assert!(!stalled(10_000, 10_000));
        assert!(!stalled(10_000, 11_000));
        assert!(stalled(10_000, 11_001));
        assert!(!stalled(10_000, 9_000), "a beat after the look");
        assert!(BEAT_EVERY * 4 <= STALL, "several beats fit before a stall");
    }

    fn hook_state(thread_id: u32, beat_ms: u64) -> (Option<HookState>, Receiver<Heard>) {
        let (heard, heard_rx) = mpsc::channel();
        let (wake, _) = mpsc::channel();
        let state = HookState {
            keys: HookKeys::default(),
            heard,
            wake,
            beat: Arc::new(AtomicU64::new(beat_ms)),
            thread_id,
            window: WINDOW,
            gone: false,
        };
        (Some(state), heard_rx)
    }

    const WINDOW: isize = 0x1234;

    fn win(down: bool) -> Stroke {
        stroke(WIN.0, WIN.1, WIN.2, down)
    }

    /// The procedure's own decisions (review I6): its thread's state takes
    /// the Windows key and says it, waking that thread.
    #[test]
    fn the_procedure_takes_and_says_a_key_on_its_own_thread() {
        let (mut slot, heard) = hook_state(7, 1_000);
        assert!(decide(&mut slot, 7, 1_100, WINDOW, win(true)));
        assert_eq!(heard.try_recv(), Ok(Heard::Key(125, true)));
        assert!(decide(&mut slot, 7, 1_100, WINDOW, win(false)));
        assert_eq!(heard.try_recv(), Ok(Heard::Key(125, false)));
    }

    /// Review I2: the window is woken, through the waker's channel, when
    /// something was said to it, and only then.
    #[test]
    fn the_window_is_woken_only_when_told_something() {
        let (mut slot, _heard) = hook_state(7, 1_000);
        let (wake, woken) = mpsc::channel();
        if let Some(state) = slot.as_mut() {
            state.wake = wake;
        }
        let tab = stroke(TAB.0, TAB.1, TAB.2, true);
        assert!(!decide(&mut slot, 7, 1_100, WINDOW, tab));
        assert!(woken.try_recv().is_err(), "a key passed says nothing");
        assert!(decide(&mut slot, 7, 1_100, WINDOW, win(true)));
        assert_eq!(woken.try_recv(), Ok(()));
    }

    /// Review I1: another window in front (a toast, a click on a second
    /// screen) has every key, before the window's next pass takes the hook
    /// out; back in front, the grab takes them again.
    #[test]
    fn another_window_in_front_has_every_key() {
        let (mut slot, heard) = hook_state(7, 1_000);
        assert!(!decide(&mut slot, 7, 1_100, 0x9999, win(true)));
        assert!(!decide(&mut slot, 7, 1_100, 0, win(true)), "none");
        assert!(heard.try_recv().is_err());
        assert!(decide(&mut slot, 7, 1_100, WINDOW, win(true)));
    }

    /// Review I3: with the window no longer listening, a key is Windows':
    /// the press it would have taken, its release, and every key after.
    #[test]
    fn a_key_nobody_hears_is_not_taken() {
        let (mut slot, heard) = hook_state(7, 1_000);
        drop(heard);
        assert!(!decide(&mut slot, 7, 1_100, WINDOW, win(true)));
        assert!(!decide(&mut slot, 7, 1_100, WINDOW, win(false)));
        let alt = stroke(ALT.0, ALT.1, ALT.2, true);
        assert!(!decide(&mut slot, 7, 1_100, WINDOW, alt));
        let tab = stroke(TAB.0, TAB.1, TAB.2, true);
        assert!(!decide(&mut slot, 7, 1_100, WINDOW, tab));
        assert!(slot.as_ref().is_some_and(|s| s.gone));
    }

    /// A stale beat: every key is Windows', nothing said.
    #[test]
    fn a_stale_beat_passes_every_key() {
        let (mut slot, heard) = hook_state(7, 1_000);
        assert!(!decide(&mut slot, 7, 2_001, WINDOW, win(true)));
        assert!(heard.try_recv().is_err());
    }

    /// PR #559's fix: a hook from an earlier window, on another thread,
    /// takes nothing of the new one's state; and with no state at all,
    /// nothing either.
    #[test]
    fn a_hook_from_another_thread_takes_nothing() {
        let (mut slot, heard) = hook_state(7, 1_000);
        assert!(!decide(&mut slot, 8, 1_100, WINDOW, win(true)));
        assert!(heard.try_recv().is_err());
        assert!(!decide(&mut None, 7, 1_100, WINDOW, win(true)));
    }

    /// The release chord's last key goes on to Windows and is said; a key
    /// with no code that the grab takes is taken and says nothing.
    #[test]
    fn a_release_passes_and_a_key_with_no_code_is_taken_silently() {
        let (mut slot, heard) = hook_state(7, 1_000);
        for key in [CTRL, ALT, SHIFT] {
            assert!(!decide(
                &mut slot,
                7,
                1_000,
                WINDOW,
                stroke(key.0, key.1, key.2, true)
            ));
        }
        assert!(!decide(
            &mut slot,
            7,
            1_000,
            WINDOW,
            stroke(SHIFT.0, SHIFT.1, SHIFT.2, false)
        ));
        assert_eq!(heard.try_recv(), Ok(Heard::Release));
        // Win held: a key with no evdev code is still kept from Windows.
        decide(&mut slot, 7, 1_000, WINDOW, win(true));
        let _ = heard.try_recv();
        assert!(decide(
            &mut slot,
            7,
            1_000,
            WINDOW,
            stroke(0xFF, 0x7F, false, true)
        ));
        assert!(heard.try_recv().is_err());
    }

    /// No test puts the hook in: the window's installer in a test build is
    /// the stand-in.
    #[test]
    fn tests_never_install_the_hook() {
        let refuse_ptr: Installer = refuse;
        assert!(std::ptr::fn_addr_eq(INSTALL, refuse_ptr));
        let before = refused_here();
        let ctx = egui::Context::default();
        assert!(INSTALL(&ctx, egui::ViewportId::ROOT).is_none());
        assert_eq!(refused_here(), before + 1);
    }

    /// The variable that must say this is a test machine before the real
    /// hook goes in (review M9): `--ignored` alone never puts it in.
    const HOOK_TEST_MACHINE: &str = "POCKETDESK_HOOK_TEST_MACHINE";

    /// Ignored, and with `POCKETDESK_HOOK_TEST_MACHINE=1` only: puts the
    /// real hook in on this computer, for no window (it takes no key).
    /// Never run on the owner's box; an owed desk check on a Windows test
    /// machine.
    #[test]
    #[ignore = "installs a real keyboard hook: run only on a test machine, with POCKETDESK_HOOK_TEST_MACHINE=1 (owed device check)"]
    fn the_real_hook_goes_in_and_comes_out() {
        if std::env::var(HOOK_TEST_MACHINE).as_deref() != Ok("1") {
            eprintln!("skipped: set {HOOK_TEST_MACHINE}=1 on a test machine");
            return;
        }
        let ctx = egui::Context::default();
        let hook = install_for(&ctx, egui::ViewportId::ROOT, -1).expect("the hook goes in");
        assert!(hook.is_alive());
        // No beat: the watchdog takes it out within a second and a beat.
        std::thread::sleep(STALL + BEAT_EVERY * 3);
        assert!(!hook.is_alive(), "the watchdog took it out");
    }
}
