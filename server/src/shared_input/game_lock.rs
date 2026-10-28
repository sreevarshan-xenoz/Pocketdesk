//! The gaming lock: while a game is in front on Windows, this computer's
//! keyboard and mouse stay on it.
//!
//! [`decide`] is pure: what is in front ([`Foreground`]), the settings and
//! where the keyboard and mouse are go in, and what the lock wants comes out
//! ([`Lock`]). The driver ([`super::drive`]) acts on it: locked, the input
//! thread's hooks are taken out altogether (only the switch key is listened
//! for), so a game's anti-cheat sees no hook; the switch key can still force
//! the keyboard and mouse off the game, unless a kernel-level anti-cheat runs
//! or the program in front is elevated above this server.
//!
//! On Windows, [`foreground`] and [`kernel_anti_cheat_running`] are the thin
//! probes the driver's loop polls every half second while sharing is on;
//! the tests never run them.

/// What is in front on this computer, as the probes see it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreground {
    /// Windows says a full-screen program runs (Direct3D full screen, or a
    /// full-screen app or game).
    pub fullscreen: bool,
    /// The cursor is hidden and held inside part of its screen, as a game
    /// that steers by the mouse holds it.
    pub cursor_captured: bool,
    /// The file name of the program whose window is in front
    /// (`eldenring.exe`); empty when it could not be read.
    pub program: String,
    /// That program runs above this server (elevated while the server is
    /// not), so this server's hooks cannot hold input back from it.
    pub elevated: bool,
}

/// What the gaming lock wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    /// Nothing holds the keyboard and mouse here (or the switch key may take
    /// them off the game in front).
    Free,
    /// A game is in front: the keyboard and mouse stay here, no hooks.
    Locked,
    /// A game came to the front while they were on another computer (not
    /// by the switch key): they come home.
    SnapBack,
    /// The switch key may not take them off the game in front: why, for the
    /// owner.
    ForceRefused(&'static str),
}

/// The kernel-level anti-cheat services that refuse the switch key mid-game
/// while one runs: Riot Vanguard, FACEIT, Easy Anti-Cheat, BattlEye.
pub const KERNEL_ANTI_CHEAT: [&str; 7] = [
    "vgk",
    "vgc",
    "FACEIT",
    "FACEITService",
    "EasyAntiCheat",
    "EasyAntiCheat_EOS",
    "BEService",
];

/// The switch key's refusal while a kernel-level anti-cheat runs.
pub const ANTI_CHEAT_REFUSED: &str = "A kernel-level anti-cheat (Vanguard, FACEIT, Easy Anti-Cheat \
     or BattlEye) is running: the keyboard and mouse stay with the game here";

/// The switch key's refusal while the program in front is elevated.
pub const ELEVATED_REFUSED: &str = "The program in front runs as administrator, so this \
     computer's keyboard and mouse cannot be held back from it: they stay here. Installing \
     with `pocketdesk install --elevated` lifts this";

/// Whether `program` (a file name, `EldenRing.exe`) is on `lock_programs`:
/// by file name, in any case, a listed name's folder and its `.exe` left
/// out as the owner likes (`eldenring` is `EldenRing.exe`).
pub fn listed(program: &str, lock_programs: &[String]) -> bool {
    let program = file_name(program).to_lowercase();
    if program.is_empty() {
        return false;
    }
    let stem = program.strip_suffix(".exe").unwrap_or(&program);
    lock_programs.iter().any(|entry| {
        let entry = file_name(entry.trim()).to_lowercase();
        !entry.is_empty() && (entry == program || entry == stem)
    })
}

/// The last part of a path, by either slash.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Whether a game holds the keyboard and mouse here: the lock is on and a
/// full-screen program, a captured cursor or a listed program is in front.
pub fn in_front(fg: &Foreground, lock_programs: &[String], game_lock: bool) -> bool {
    game_lock && (fg.fullscreen || fg.cursor_captured || listed(&fg.program, lock_programs))
}

/// What the lock wants, with `fg` in front: `away`, the keyboard and mouse
/// are on another computer; `forced`, the switch key took them (or is
/// taking them) off a game here; `kernel_anti_cheat`, one of
/// [`KERNEL_ANTI_CHEAT`] runs.
pub fn decide(
    fg: &Foreground,
    lock_programs: &[String],
    game_lock: bool,
    kernel_anti_cheat: bool,
    away: bool,
    forced: bool,
) -> Lock {
    if !in_front(fg, lock_programs, game_lock) {
        return Lock::Free;
    }
    match (forced, away) {
        (false, false) => Lock::Locked,
        (false, true) => Lock::SnapBack,
        // The switch key, mid-game: refused where the hooks would be seen
        // or could not hold input back.
        (true, _) if kernel_anti_cheat => Lock::ForceRefused(ANTI_CHEAT_REFUSED),
        (true, _) if fg.elevated => Lock::ForceRefused(ELEVATED_REFUSED),
        (true, _) => Lock::Free,
    }
}

/// A rectangle as Windows has them: left, top, right, bottom (exclusive).
pub type Rect = (i32, i32, i32, i32);

/// Whether the cursor's clip `clip` holds it inside part of `monitor`: it
/// lies within the monitor and is narrower or shorter than it.
pub fn clip_smaller(clip: Rect, monitor: Rect) -> bool {
    let (l, t, r, b) = clip;
    let (ml, mt, mr, mb) = monitor;
    let within = l >= ml && t >= mt && r <= mr && b <= mb && l < r && t < b;
    within && (r - l < mr - ml || b - t < mb - mt)
}

#[cfg(windows)]
pub use probes::{foreground, kernel_anti_cheat_running};

/// The Windows probes: thin glue over Win32, never run by the tests.
#[cfg(windows)]
mod probes {
    use std::sync::OnceLock;

    use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
    };
    use windows::Win32::UI::Shell::{
        QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GetClipCursor, GetCursorInfo, GetForegroundWindow,
        GetWindowThreadProcessId,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    use super::{Foreground, KERNEL_ANTI_CHEAT, Rect, clip_smaller, file_name};

    /// What is in front now.
    pub fn foreground() -> Foreground {
        // SAFETY: a query with no arguments but its answer.
        let fullscreen = unsafe { SHQueryUserNotificationState() }
            .is_ok_and(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_BUSY);
        let (program, elevated) = program_in_front();
        Foreground {
            fullscreen,
            cursor_captured: cursor_captured(),
            program,
            elevated: elevated && !self_elevated(),
        }
    }

    /// Whether one of [`KERNEL_ANTI_CHEAT`]'s services is running.
    pub fn kernel_anti_cheat_running() -> bool {
        // SAFETY: opens the service manager to connect only; closed below.
        let Ok(manager) =
            (unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) })
        else {
            return false;
        };
        let running = KERNEL_ANTI_CHEAT.iter().any(|name| {
            // SAFETY: the manager just opened; a service not installed fails.
            let Ok(service) =
                (unsafe { OpenServiceW(manager, &HSTRING::from(*name), SERVICE_QUERY_STATUS) })
            else {
                return false;
            };
            let mut status = SERVICE_STATUS::default();
            // SAFETY: the service just opened, an out value on this stack.
            let read = unsafe { QueryServiceStatus(service, &mut status) };
            // SAFETY: the handle opened above.
            let _ = unsafe { CloseServiceHandle(service) };
            read.is_ok() && status.dwCurrentState == SERVICE_RUNNING
        });
        // SAFETY: the handle opened above.
        let _ = unsafe { CloseServiceHandle(manager) };
        running
    }

    fn rect(r: RECT) -> Rect {
        (r.left, r.top, r.right, r.bottom)
    }

    /// The cursor hidden and held inside part of its monitor.
    fn cursor_captured() -> bool {
        // Physical pixels for the clip and the monitor alike, whatever this
        // thread's own awareness; put back after.
        // SAFETY: this thread's own setting.
        let before =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let captured = (|| {
            let mut info = CURSORINFO {
                cbSize: size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: an out value on this stack, its size set.
            unsafe { GetCursorInfo(&mut info) }.ok()?;
            if info.flags.0 & CURSOR_SHOWING.0 != 0 {
                return Some(false);
            }
            let mut clip = RECT::default();
            // SAFETY: an out value on this stack.
            unsafe { GetClipCursor(&mut clip) }.ok()?;
            // SAFETY: a point by value; the nearest monitor always answers.
            let monitor = unsafe { MonitorFromPoint(info.ptScreenPos, MONITOR_DEFAULTTONEAREST) };
            let mut about = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: the monitor just named, an out value on this stack.
            if !unsafe { GetMonitorInfoW(monitor, &mut about) }.as_bool() {
                return None;
            }
            Some(clip_smaller(rect(clip), rect(about.rcMonitor)))
        })()
        .unwrap_or(false);
        if !before.0.is_null() {
            // SAFETY: as above, the setting it had.
            unsafe { SetThreadDpiAwarenessContext(before) };
        }
        captured
    }

    /// The file name of the program in front, and whether it is elevated.
    fn program_in_front() -> (String, bool) {
        // SAFETY: reads which window is in front.
        let window = unsafe { GetForegroundWindow() };
        if window.is_invalid() {
            return (String::new(), false);
        }
        let mut pid = 0u32;
        // SAFETY: the window just read, an out value on this stack.
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if pid == 0 {
            return (String::new(), false);
        }
        // SAFETY: opens the process for the least there is; closed below.
        let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(process) => process,
            // A process that will not even be looked at runs above this one.
            Err(e) => return (String::new(), e.code() == E_ACCESSDENIED),
        };
        let program = image_name(process);
        let elevated = token_elevated(process);
        // SAFETY: the handle opened above.
        let _ = unsafe { CloseHandle(process) };
        (program, elevated)
    }

    fn image_name(process: HANDLE) -> String {
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        // SAFETY: a buffer on this stack, its length given.
        let named = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
        };
        if named.is_err() {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())]);
        file_name(&path).to_string()
    }

    /// Whether `process` runs elevated. Its token closed to this one is
    /// itself the sign: an elevated process's token refuses an unelevated
    /// caller.
    fn token_elevated(process: HANDLE) -> bool {
        let mut token = HANDLE::default();
        // SAFETY: a process handle this module holds, an out value here.
        if let Err(e) = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } {
            return e.code() == E_ACCESSDENIED;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        // SAFETY: the token just opened, an out value of the size given.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
        };
        // SAFETY: the handle opened above.
        let _ = unsafe { CloseHandle(token) };
        read.is_ok() && elevation.TokenIsElevated != 0
    }

    /// Whether this server runs elevated (installed with `--elevated`): its
    /// hooks then hold input back from elevated programs too.
    fn self_elevated() -> bool {
        static ELEVATED: OnceLock<bool> = OnceLock::new();
        // SAFETY: this process's own pseudo-handle, never closed.
        *ELEVATED.get_or_init(|| token_elevated(unsafe { GetCurrentProcess() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Foreground {
        Foreground {
            fullscreen: true,
            cursor_captured: false,
            program: "eldenring.exe".to_string(),
            elevated: false,
        }
    }

    fn windowed(program: &str) -> Foreground {
        Foreground {
            fullscreen: false,
            cursor_captured: false,
            program: program.to_string(),
            elevated: false,
        }
    }

    fn programs(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fullscreen_game_locks() {
        assert_eq!(
            decide(&game(), &[], true, false, false, false),
            Lock::Locked
        );
        // A windowed game that holds the cursor hidden in its window is one too.
        let captured = Foreground {
            cursor_captured: true,
            ..windowed("cs2.exe")
        };
        assert_eq!(
            decide(&captured, &[], true, false, false, false),
            Lock::Locked
        );
        // An ordinary window is not.
        assert_eq!(
            decide(&windowed("notepad.exe"), &[], true, false, false, false),
            Lock::Free
        );
    }

    #[test]
    fn listed_program_locks_only_in_front() {
        let list = programs(&["Blender.exe"]);
        // In front, in a window: locked.
        assert_eq!(
            decide(&windowed("blender.exe"), &list, true, false, false, false),
            Lock::Locked
        );
        // Running, but something else is in front: free (the probe names
        // only the program in front).
        assert_eq!(
            decide(&windowed("explorer.exe"), &list, true, false, false, false),
            Lock::Free
        );
        assert!(!in_front(&windowed(""), &list, true));
    }

    #[test]
    fn game_coming_to_front_while_away_snaps_back() {
        assert_eq!(
            decide(&game(), &[], true, false, true, false),
            Lock::SnapBack
        );
        let list = programs(&["eldenring"]);
        assert_eq!(
            decide(&windowed("EldenRing.exe"), &list, true, false, true, false),
            Lock::SnapBack
        );
        // Nothing in front: away stays away.
        assert_eq!(
            decide(&windowed("explorer.exe"), &list, true, false, true, false),
            Lock::Free
        );
    }

    #[test]
    fn forced_switch_allowed_without_kernel_anti_cheat() {
        // The switch key pressed mid-game, and the keyboard and mouse away
        // by it: both free, no snap back.
        assert_eq!(decide(&game(), &[], true, false, false, true), Lock::Free);
        assert_eq!(decide(&game(), &[], true, false, true, true), Lock::Free);
    }

    #[test]
    fn forced_switch_refused_with_kernel_anti_cheat() {
        assert_eq!(
            decide(&game(), &[], true, true, false, true),
            Lock::ForceRefused(ANTI_CHEAT_REFUSED)
        );
        // Started while away by the switch key: refused then too.
        assert_eq!(
            decide(&game(), &[], true, true, true, true),
            Lock::ForceRefused(ANTI_CHEAT_REFUSED)
        );
        // With no game in front, the anti-cheat is none of the lock's business.
        assert_eq!(
            decide(&windowed("explorer.exe"), &[], true, true, false, true),
            Lock::Free
        );
        // Not forced, it is the plain lock.
        assert_eq!(decide(&game(), &[], true, true, false, false), Lock::Locked);
    }

    #[test]
    fn forced_switch_refused_for_elevated_foreground() {
        let elevated = Foreground {
            elevated: true,
            ..game()
        };
        assert_eq!(
            decide(&elevated, &[], true, false, false, true),
            Lock::ForceRefused(ELEVATED_REFUSED)
        );
        assert!(ELEVATED_REFUSED.contains("install --elevated"));
        // The anti-cheat is named first when both hold.
        assert_eq!(
            decide(&elevated, &[], true, true, false, true),
            Lock::ForceRefused(ANTI_CHEAT_REFUSED)
        );
    }

    #[test]
    fn lock_off_never_locks() {
        let list = programs(&["eldenring.exe"]);
        for (away, forced) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(
                decide(&game(), &list, false, true, away, forced),
                Lock::Free,
                "away {away}, forced {forced}"
            );
        }
        assert!(!in_front(&game(), &list, false));
    }

    #[test]
    fn program_names_match_by_file_name_in_any_case() {
        let list = programs(&["EldenRing.exe", "blender", r"C:\Games\Hades\Hades.exe", ""]);
        assert!(listed("eldenring.exe", &list));
        assert!(listed("ELDENRING.EXE", &list));
        assert!(listed("Blender.exe", &list), "the .exe may be left off");
        assert!(
            listed("hades.exe", &list),
            "a listed path counts by its file name"
        );
        assert!(!listed("eldenring2.exe", &list));
        assert!(!listed("blender-launcher.exe", &list));
        // An empty entry or an unknown program matches nothing.
        assert!(!listed("", &list));
        assert!(!listed("", &programs(&[""])));
        // A listed name with another extension does not lose it.
        assert!(!listed("game.exe", &programs(&["game.bin"])));
    }

    #[test]
    fn a_clip_inside_part_of_a_monitor_is_captured() {
        let monitor = (0, 0, 1920, 1080);
        // Held inside a window on it.
        assert!(clip_smaller((100, 100, 1380, 820), monitor));
        // Held to one pixel (a shooter's centre).
        assert!(clip_smaller((960, 540, 961, 541), monitor));
        // Not clipped: the whole desktop (two monitors), or the monitor itself.
        assert!(!clip_smaller((0, 0, 3456, 1080), monitor));
        assert!(!clip_smaller(monitor, monitor));
        // On another monitor than the one measured.
        assert!(!clip_smaller((2000, 100, 2400, 500), monitor));
    }
}
