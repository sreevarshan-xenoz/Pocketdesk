//! One app at a time: two would race on the pairing PIN file. A second launch
//! brings the first window forward (Windows) and exits.

/// Held for the process lifetime; dropping it releases the claim.
pub struct Instance {
    #[cfg(windows)]
    _mutex: windows::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    _socket: std::os::unix::net::UnixListener,
}

/// Claim the single instance; `None` when another copy holds it.
#[cfg(windows)]
pub fn acquire(window_title: &str) -> Option<Instance> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
    };
    use windows::core::{HSTRING, w};

    let mutex = unsafe { CreateMutexW(None, false, w!("Local\\PocketdeskDesktop")) }.ok()?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(hwnd) = unsafe { FindWindowW(None, &HSTRING::from(window_title)) } {
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

/// Claim the single instance; `None` when another copy holds it. A socket in
/// the state dir is the claim: binding fails while a live copy listens, and a
/// socket left by a crash is replaced.
#[cfg(unix)]
pub fn acquire(_window_title: &str) -> Option<Instance> {
    use std::os::unix::net::{UnixListener, UnixStream};
    let dir = pocketdesk_core::local::paths::state_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("pocketdesk.sock");
    match UnixListener::bind(&path) {
        Ok(socket) => Some(Instance { _socket: socket }),
        Err(_) if UnixStream::connect(&path).is_ok() => None,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path)
                .ok()
                .map(|socket| Instance { _socket: socket })
        }
    }
}
