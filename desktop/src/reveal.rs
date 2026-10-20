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

/// Open `path` with the system's viewer for it.
pub fn open_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        // Explorer opens a file with its default program.
        std::process::Command::new(windows_dir().join("explorer.exe"))
            .arg(path)
            .spawn()
            .map(drop)
    }
    #[cfg(target_os = "linux")]
    {
        let mut child = std::process::Command::new("xdg-open").arg(path).spawn()?;
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

/// Whether a file received as `name` may be opened from the Files page
/// (I3): pictures, PDFs, text, video and audio, which open in a viewer; not
/// a program, a script, a shortcut or an office file (which can carry
/// macros). The name is the phone's, and a guest phone may send files, so
/// anything else is shown in its folder only.
pub fn openable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some((_, ext)) = lower.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext,
        "jpg"
            | "jpeg"
            | "png"
            | "webp"
            | "heic"
            | "heif"
            | "gif"
            | "bmp"
            | "pdf"
            | "txt"
            | "mp4"
            | "mov"
            | "mkv"
            | "webm"
            | "3gp"
            | "mp3"
            | "m4a"
            | "aac"
            | "ogg"
            | "opus"
            | "wav"
            | "flac"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_media_and_plain_documents_open() {
        for name in [
            "IMG_0001.JPG",
            "scan.pdf",
            "notes.txt",
            "clip.mp4",
            "voice.m4a",
        ] {
            assert!(openable(name), "{name}");
        }
        for name in [
            "setup.exe",
            "run.bat",
            "x.ps1",
            "a.lnk",
            "report.docm",
            "sheet.xlsx",
            "page.html",
            "photo.jpg.exe",
            "README",
            "archive.zip",
        ] {
            assert!(!openable(name), "{name}");
        }
    }
}
