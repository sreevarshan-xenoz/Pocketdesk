//! One tray program at a time (E2), and whether one runs, for the window:
//! with it running, the window shows no tray of its own and closing it
//! exits. Windows: a named mutex; Linux: a socket in the runtime folder.

/// Held for the tray program's life; dropping it lets another start.
pub struct TrayClaim {
    #[cfg(windows)]
    _mutex: windows::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    _socket: std::os::unix::net::UnixListener,
}

#[cfg(windows)]
const MUTEX_NAME: &str = "Local\\PocketdeskTray";

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Claim the tray; `None` when another tray program holds it.
#[cfg(windows)]
pub fn claim() -> Option<TrayClaim> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    let name = wide(MUTEX_NAME);
    // SAFETY: a valid, NUL-terminated name; the handle lives in the claim.
    let mutex = unsafe { CreateMutexW(None, true, windows::core::PCWSTR(name.as_ptr())) }.ok()?;
    // SAFETY: no arguments.
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return None;
    }
    Some(TrayClaim { _mutex: mutex })
}

/// Whether a tray program runs now.
#[cfg(windows)]
pub fn running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name = wide(MUTEX_NAME);
    // SAFETY: a valid, NUL-terminated name; the handle is closed at once.
    match unsafe {
        OpenMutexW(
            SYNCHRONIZATION_SYNCHRONIZE,
            false,
            windows::core::PCWSTR(name.as_ptr()),
        )
    } {
        Ok(handle) => {
            // SAFETY: the handle just opened.
            let _ = unsafe { CloseHandle(handle) };
            true
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
fn socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("pocketdesk-tray.sock")
}

/// Claim the tray; `None` when another tray program holds it. Binding
/// first, so two starting at once cannot both win (E2 review); a socket
/// left by a tray that died, which nothing answers, is replaced.
#[cfg(unix)]
pub fn claim() -> Option<TrayClaim> {
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = socket_path();
    let socket = match UnixListener::bind(&path) {
        Ok(socket) => socket,
        Err(_) if UnixStream::connect(&path).is_ok() => return None,
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path).ok()?
        }
    };
    // Each probe ([`running`], a second tray's) is taken and dropped: left
    // waiting, they would fill the backlog and a later connect would hang
    // (E2 review).
    if let Ok(probes) = socket.try_clone() {
        std::thread::spawn(move || {
            for probe in probes.incoming() {
                drop(probe);
            }
        });
    }
    Some(TrayClaim { _socket: socket })
}

/// Whether a tray program runs now.
#[cfg(unix)]
pub fn running() -> bool {
    std::os::unix::net::UnixStream::connect(socket_path()).is_ok()
}

/// The tray program beside `exe` (the window's own path), if installed.
pub fn program_beside(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "pocketdesk-tray.exe"
    } else {
        "pocketdesk-tray"
    };
    let program = exe.with_file_name(name);
    program.is_file().then_some(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_program_is_found_beside_the_window_only_when_there() {
        let dir = std::env::temp_dir().join(format!("pd-tray-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let window = dir.join(if cfg!(windows) {
            "pocketdesk-app.exe"
        } else {
            "pocketdesk-app"
        });
        assert_eq!(program_beside(&window), None);
        let tray = dir.join(if cfg!(windows) {
            "pocketdesk-tray.exe"
        } else {
            "pocketdesk-tray"
        });
        std::fs::write(&tray, b"").unwrap();
        assert_eq!(program_beside(&window), Some(tray));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
