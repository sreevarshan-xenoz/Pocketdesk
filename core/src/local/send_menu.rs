//! "Send to phone" in the file manager (T10e): Explorer's Send To menu on
//! Windows; a Nautilus and Nemo script and a Dolphin service menu on Linux.
//! Each runs `pocketdesk send-to-phone` with the files chosen. The server's
//! `send-menu` command and the desktop app's Settings add and remove them.

use std::io;
use std::path::{Path, PathBuf};

/// The command the entries run, after the server's path.
pub const COMMAND: &str = "send-to-phone";

/// What each entry is called in the menu.
pub const LABEL: &str = "Send to phone";

/// One file an entry consists of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A script or a `.desktop` file, written as is and made executable.
    File(String),
    /// A Windows shortcut (`.lnk`) to `pocketdesk.exe send-to-phone`.
    Shortcut,
}

/// `s` as one word for `sh` and for a `.desktop` Exec line.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The Nautilus and Nemo script: they pass the chosen files as arguments.
pub fn file_manager_script(exe: &Path) -> String {
    format!(
        "#!/bin/sh\n# Pocketdesk: send the chosen files to your phone.\nexec {} {} \"$@\"\n",
        shell_quote(&exe.to_string_lossy()),
        COMMAND
    )
}

/// The Dolphin service menu, for every file and folder.
pub fn dolphin_menu(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Service\nMimeType=all/allfiles;inode/directory;\nActions=sendToPhone\nX-KDE-Priority=TopLevel\n\n[Desktop Action sendToPhone]\nName={LABEL}\nIcon=phone\nExec={} {} %F\n",
        shell_quote(&exe.to_string_lossy()),
        COMMAND
    )
}

/// Where the entries go, under the user's data folder `data` (Linux:
/// `~/.local/share`; Windows: the roaming AppData).
pub fn places(data: &Path, exe: &Path) -> Vec<Place> {
    if cfg!(windows) {
        vec![Place {
            path: data
                .join("Microsoft")
                .join("Windows")
                .join("SendTo")
                .join("Pocketdesk (phone).lnk"),
            kind: Kind::Shortcut,
        }]
    } else {
        vec![
            Place {
                path: data.join("nautilus").join("scripts").join(LABEL),
                kind: Kind::File(file_manager_script(exe)),
            },
            Place {
                path: data.join("nemo").join("scripts").join(LABEL),
                kind: Kind::File(file_manager_script(exe)),
            },
            Place {
                path: data
                    .join("kio")
                    .join("servicemenus")
                    .join("pocketdesk-send.desktop"),
                kind: Kind::File(dolphin_menu(exe)),
            },
        ]
    }
}

/// The user's data folder the entries go under.
pub fn data_dir() -> io::Result<PathBuf> {
    dirs::data_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no data folder"))
}

/// Add the entries for `exe`.
pub fn install_in(data: &Path, exe: &Path) -> io::Result<()> {
    for place in places(data, exe) {
        if let Some(parent) = place.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match &place.kind {
            Kind::File(content) => {
                std::fs::write(&place.path, content)?;
                make_executable(&place.path)?;
            }
            Kind::Shortcut => make_shortcut(&place.path, exe)?,
        }
    }
    Ok(())
}

/// Remove the entries; one that is not there is fine.
pub fn remove_in(data: &Path) -> io::Result<()> {
    for place in places(data, Path::new("")) {
        match std::fs::remove_file(&place.path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Whether the entries are there.
pub fn installed_in(data: &Path) -> bool {
    places(data, Path::new(""))
        .iter()
        .all(|place| place.path.exists())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> io::Result<()> {
    Ok(())
}

/// A `.lnk` at `path` running `exe send-to-phone`, minimized (Explorer
/// adds the chosen files as arguments). Written through the shell's own
/// COM object by PowerShell, resolved under System32.
#[cfg(windows)]
fn make_shortcut(path: &Path, exe: &Path) -> io::Result<()> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    // A canonical path's `\\?\` prefix is no use to a shortcut.
    let quote = |s: &Path| {
        let s = s.to_string_lossy();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        format!("'{}'", s.replace('\'', "''"))
    };
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({}); $s.TargetPath = {}; $s.Arguments = '{}'; $s.WindowStyle = 7; $s.Description = 'Send to your phone with Pocketdesk'; $s.Save()",
        quote(path),
        quote(exe),
        COMMAND
    );
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() && path.exists() {
        Ok(())
    } else {
        Err(io::Error::other("the Send To shortcut could not be made"))
    }
}

#[cfg(not(windows))]
fn make_shortcut(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "shortcuts are Windows'",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entries_run_the_server_with_the_chosen_files() {
        let exe = Path::new("/opt/pocket desk/it's/pocketdesk");
        let script = file_manager_script(exe);
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(
            script.contains(r#"exec '/opt/pocket desk/it'\''s/pocketdesk' send-to-phone "$@""#)
        );
        let menu = dolphin_menu(exe);
        assert!(menu.contains("Exec='/opt/pocket desk/it'\\''s/pocketdesk' send-to-phone %F"));
        assert!(menu.contains("Name=Send to phone"));
    }

    #[test]
    fn they_are_added_and_removed_where_the_file_manager_looks() {
        let data = std::env::temp_dir().join(format!("pd-send-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let exe = Path::new("pocketdesk");
        if cfg!(windows) {
            assert!(
                places(&data, exe)[0]
                    .path
                    .ends_with("SendTo/Pocketdesk (phone).lnk")
            );
        } else {
            assert!(!installed_in(&data));
            install_in(&data, exe).unwrap();
            assert!(installed_in(&data));
            assert!(data.join("nautilus/scripts/Send to phone").exists());
            remove_in(&data).unwrap();
            assert!(!installed_in(&data));
        }
        remove_in(&data).unwrap();
    }
}
