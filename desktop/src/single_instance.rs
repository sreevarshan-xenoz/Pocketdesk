//! One app at a time: two would race on the pairing PIN file. A second launch
//! brings the first window forward and exits: on Windows by finding it, on
//! Linux by connecting to the first's socket, which raises it (N6 review: a
//! window hidden to a tray that went away was otherwise out of reach).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

/// Held for the process lifetime; dropping it releases the claim.
pub struct Instance {
    #[cfg(windows)]
    _mutex: windows::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    socket: std::os::unix::net::UnixListener,
}

/// The byte a later launch sends to hand over a request without asking for
/// the window.
#[cfg(unix)]
const QUIET: u8 = b'q';

/// A later launch asked for the window.
static RAISE: AtomicBool = AtomicBool::new(false);

/// The app's context, to wake it when a later launch asks.
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

impl Instance {
    /// Listen for later launches: each asks for the window ([`take_raise`]).
    /// Windows raises the window from the later launch itself.
    pub fn listen_for_launches(&self) {
        #[cfg(unix)]
        if let Ok(socket) = self.socket.try_clone() {
            std::thread::spawn(move || {
                for launch in socket.incoming() {
                    // A launch that only hands over a request (a pinned
                    // phone app) says so with one byte and asks for no window.
                    let quiet = launch.is_ok_and(|mut stream| {
                        use std::io::Read;
                        let _ =
                            stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
                        let mut byte = [0u8; 1];
                        stream
                            .read(&mut byte)
                            .is_ok_and(|n| n == 1 && byte[0] == QUIET)
                    });
                    if !quiet {
                        RAISE.store(true, Ordering::Release);
                    }
                    if let Some(ctx) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        ctx.request_repaint();
                    }
                }
            });
        }
    }
}

/// Wake the app, hidden or not, when a later launch asks for the window.
pub fn wake_with(ctx: egui::Context) {
    *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
}

/// Whether a later launch asked for the window since the last call.
pub fn take_raise() -> bool {
    RAISE.swap(false, Ordering::AcqRel)
}

/// Claim the single instance; `None` when another copy holds it, which is
/// brought forward when `raise` (a launch that only hands a request over, as
/// a pinned phone app does, leaves the window where it is).
#[cfg(windows)]
pub fn acquire(window_title: &str, raise: bool) -> Option<Instance> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    use windows::core::{HSTRING, w};

    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\PocketdeskDesktop")) }.ok()?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if raise && let Ok(hwnd) = unsafe { FindWindowW(None, &HSTRING::from(window_title)) } {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
        }
        return None;
    }
    Some(Instance { _mutex: mutex })
}

/// Claim the single instance; `None` when another copy holds it, which is
/// brought forward when `raise`. A socket in the state dir is the claim:
/// binding fails while a live copy listens, and a socket left by a crash is
/// replaced.
#[cfg(unix)]
pub fn acquire(_window_title: &str, raise: bool) -> Option<Instance> {
    use std::io::Write;
    use std::os::unix::net::{UnixListener, UnixStream};
    let dir = pocketdesk_core::local::paths::state_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("pocketdesk.sock");
    match UnixListener::bind(&path) {
        Ok(socket) => Some(Instance { socket }),
        Err(_) => match UnixStream::connect(&path) {
            Ok(mut live) => {
                if !raise {
                    let _ = live.write_all(&[QUIET]);
                }
                None
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                UnixListener::bind(&path)
                    .ok()
                    .map(|socket| Instance { socket })
            }
        },
    }
}
