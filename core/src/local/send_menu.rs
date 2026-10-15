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

/// `s` as one word for `sh`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `s` as one argument of a Desktop Entry `Exec` line: double-quoted, with
/// `"`, `` ` ``, `$` and `\` escaped by the quoting rule, then a `\` doubled
/// again by the string rule (so four), and `%` written `%%`.
pub fn desktop_exec_quote(s: &str) -> String {
    let mut quoted = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The Nautilus and Nemo script. Both name the chosen local files, with
/// their full paths, one per line, in an environment variable (the
/// arguments are bare names, and not even that in search results); the
/// arguments are the fallback. `--` keeps a file named like an option a
/// file.
pub fn file_manager_script(exe: &Path) -> String {
    let exe = shell_quote(&exe.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         # Pocketdesk: send the chosen files to your phone.\n\
         paths=\"${{NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-$NEMO_SCRIPT_SELECTED_FILE_PATHS}}\"\n\
         if [ -n \"$paths\" ]; then\n\
         \x20   # One path per line; split only there, and expand no wildcard.\n\
         \x20   IFS='\n'\n\
         \x20   set -f\n\
         \x20   exec {exe} {COMMAND} -- $paths\n\
         fi\n\
         exec {exe} {COMMAND} -- \"$@\"\n"
    )
}

/// The Dolphin service menu, for every file and folder.
pub fn dolphin_menu(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Service\nMimeType=all/allfiles;inode/directory;\nActions=sendToPhone\nX-KDE-Priority=TopLevel\n\n[Desktop Action sendToPhone]\nName={LABEL}\nIcon=phone\nExec={} {} -- %F\n",
        desktop_exec_quote(&exe.to_string_lossy()),
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
    let plain = |s: &Path| {
        let s = s.to_string_lossy().into_owned();
        s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
    };
    // The paths reach the script as environment variables, never in its
    // text: no name can end a string and run code there.
    let script = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:PD_LNK); $s.TargetPath = $env:PD_EXE; $s.Arguments = '{COMMAND} --'; $s.WindowStyle = 7; $s.Description = 'Send to your phone with Pocketdesk'; $s.Save()"
    );
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new(powershell)
        .env("PD_LNK", plain(path))
        .env("PD_EXE", plain(exe))
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
            script.contains(r#"exec '/opt/pocket desk/it'\''s/pocketdesk' send-to-phone -- "$@""#)
        );
        assert!(
            script
                .contains(r#"exec '/opt/pocket desk/it'\''s/pocketdesk' send-to-phone -- $paths"#)
        );
        assert!(script.contains("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS"));
        assert!(script.contains("set -f"));
        let menu = dolphin_menu(exe);
        assert!(menu.contains(r#"Exec="/opt/pocket desk/it's/pocketdesk" send-to-phone -- %F"#));
        assert!(menu.contains("Name=Send to phone"));
    }

    #[test]
    fn an_exec_argument_is_quoted_as_the_desktop_entry_spec_says() {
        assert_eq!(desktop_exec_quote("/a b/c"), r#""/a b/c""#);
        assert_eq!(desktop_exec_quote(r#"/x"y`z$w"#), r#""/x\"y\`z\$w""#);
        assert_eq!(desktop_exec_quote("/100%/p"), r#""/100%%/p""#);
        assert_eq!(desktop_exec_quote(r"/a\b"), r#""/a\\\\b""#);
    }

    /// Run the script through `sh` with names a shell could misread, and
    /// read the arguments it passes on.
    #[cfg(unix)]
    #[test]
    fn the_script_passes_each_chosen_path_whole() {
        let dir = std::env::temp_dir().join(format!("pd-send-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in for the server that prints its arguments, one a line.
        let exe = dir.join("print args");
        std::fs::write(&exe, "#!/bin/sh\nfor a in \"$@\"; do echo \"[$a]\"; done\n").unwrap();
        make_executable(&exe).unwrap();
        let script = dir.join("script");
        std::fs::write(&script, file_manager_script(&exe)).unwrap();
        let out = std::process::Command::new("sh")
            .arg(&script)
            .env(
                "NAUTILUS_SCRIPT_SELECTED_FILE_PATHS",
                "/a b/it's $x *\n/-c.txt\n",
            )
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[send-to-phone]\n[--]\n[/a b/it's $x *]\n[/-c.txt]\n"
        );
        let out = std::process::Command::new("sh")
            .arg(&script)
            .args(["-h", "plain name"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "[send-to-phone]\n[--]\n[-h]\n[plain name]\n"
        );
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
