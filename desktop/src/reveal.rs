//! "Show in folder": open the system file manager at a file, selected
//! where the system supports it. Never opens the file itself.

use std::path::Path;

/// Show `path` in the file manager. Errors are the launch's, not the
/// file manager's.
pub fn show_in_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Explorer wants "/select," and the path as one argument.
        std::process::Command::new(windows_dir().join("explorer.exe"))
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(drop)
    }
    #[cfg(target_os = "linux")]
    {
        let folder = path.parent().unwrap_or(path);
        let mut child = std::process::Command::new("xdg-open").arg(folder).spawn()?;
        // Reaped off the UI thread, so it does not linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        Err(std::io::Error::other("not supported on this system"))
    }
}

/// The Windows folder (explorer.exe lives there, not in System32), asked of
/// the system rather than found on PATH.
#[cfg(windows)]
pub(crate) fn windows_dir() -> std::path::PathBuf {
    use windows::Win32::System::SystemInformation::GetWindowsDirectoryW;
    let mut buf = [0u16; 260];
    let len = unsafe { GetWindowsDirectoryW(Some(&mut buf)) } as usize;
    std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}
