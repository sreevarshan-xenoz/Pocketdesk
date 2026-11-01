//! Phone apps pinned to this computer (Apps part 2): a shortcut named
//! "<App> (phone)" in the Start menu's Programs folder on Windows, a
//! `.desktop` file in `~/.local/share/applications/` on Linux, each with
//! the app's own icon, that starts the desktop app with
//! `--open-phone-app <phone-id> <package>`. The running copy takes the
//! request and opens the app as the Phone apps page's click does.
//!
//! Nothing but a checked phone id and a checked package name ever reaches
//! a command line: [`parse_args`] and [`pin`] hold both to the same rules
//! as the packets ([`is_valid_id`], [`valid_app_id`]), and the `Exec` line
//! of a `.desktop` file is quoted as the Desktop Entry specification says
//! ([`exec_quote`]). What was pinned is kept owner-only in
//! `<state>/phone_pins.json` so that Unpin knows which files are ours; it
//! removes only files that sit directly in the shortcut or icon folder.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::approval::is_valid_id;
use super::paths::PHONE_PINS_FILE;
use crate::protocol::phone_action::valid_app_id;

/// The argument that asks the desktop app to open a phone's app.
pub const OPEN_ARG: &str = "--open-phone-app";

/// The longest label kept in a shortcut's name, in characters.
const MAX_LABEL: usize = 80;

/// What the shortcut's name ends with.
const SUFFIX: &str = " (phone)";

/// The phone id and package `--open-phone-app <phone> <package>` names,
/// or `None` when the arguments do not name exactly a valid pair.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Option<(String, String)> {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.to_str() != Some(OPEN_ARG) {
            continue;
        }
        let phone = args.next()?.into_string().ok()?;
        let app = args.next()?.into_string().ok()?;
        return (is_valid_id(&phone) && valid_app_id(&app)).then_some((phone, app));
    }
    None
}

/// The command line arguments of a shortcut for `phone`'s `app`.
pub fn arguments(phone: &str, app: &str) -> Option<String> {
    (is_valid_id(phone) && valid_app_id(app)).then(|| format!("{OPEN_ARG} {phone} {app}"))
}

/// `arg` as one argument of a `.desktop` file's `Exec` line (Desktop Entry
/// Specification, "The Exec key"), which is read in two layers: the file's
/// string escapes first (`\\` is one backslash), then the quoting rule (in
/// double quotes `\"`, `` \` ``, `\$` and `\\` stand for `"`, a backtick,
/// `$` and a backslash). So in the file each of `"`, a backtick and `$`
/// is written as a doubled backslash and the character (`\\"`, `` \\` ``,
/// `\\$`), a backslash as four, and `%` is doubled so that it is never a
/// field code. `None` for an argument with a control character, which would
/// end the line.
pub fn exec_quote(arg: &str) -> Option<String> {
    if arg.chars().any(char::is_control) {
        return None;
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '\\' => out.push_str(r"\\\\"),
            '"' => out.push_str(r#"\\""#),
            '`' => out.push_str(r"\\`"),
            '$' => out.push_str(r"\\$"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    Some(out)
}

/// `text` as the value of a `.desktop` key of type string: a backslash is
/// written twice. `None` when it has a control character.
fn value(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    Some(text.replace('\\', "\\\\"))
}

/// An app's label as part of a file name: no character a file name may not
/// have on Windows, no control character, white space as one space, no
/// trailing dot or space, at most [`MAX_LABEL`] characters; the package's
/// last word when nothing is left.
pub fn clean_label(label: &str, app: &str) -> String {
    let kept: String = label
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let one_line = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = one_line.chars().take(MAX_LABEL).collect();
    let clean = cut.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if clean.is_empty() {
        app.rsplit('.').next().unwrap_or("app").to_string()
    } else {
        clean
    }
}

/// The text of the `.desktop` file for `phone`'s `app`, called `label`,
/// starting `exe` with `icon`; `None` when anything in it could not be
/// written safely or the phone id or package is not valid.
pub fn desktop_entry(label: &str, exe: &str, phone: &str, app: &str, icon: &str) -> Option<String> {
    arguments(phone, app)?;
    let name = value(&format!("{label}{SUFFIX}"))?;
    let comment = value(&format!("Open {label} on the phone"))?;
    let exec = format!(
        "{} {} {} {}",
        exec_quote(exe)?,
        exec_quote(OPEN_ARG)?,
        exec_quote(phone)?,
        exec_quote(app)?
    );
    Some(format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName={name}\nComment={comment}\nExec={exec}\nIcon={}\nTerminal=false\nCategories=Utility;\nStartupNotify=false\nX-Pocketdesk-Phone={phone}\nX-Pocketdesk-App={app}\n",
        value(icon)?
    ))
}

/// A Windows icon file holding `png` as it is (Vista and later read a PNG
/// inside an `.ico`); `None` when it is not a PNG of at most 256 px a side.
pub fn ico_from_png(png: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || !png.starts_with(SIGNATURE) || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    if width == 0 || height == 0 || width > 256 || height > 256 {
        return None;
    }
    let mut ico = Vec::with_capacity(22 + png.len());
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    // 256 is written as 0 in the directory.
    ico.push((width % 256) as u8);
    ico.push((height % 256) as u8);
    ico.extend_from_slice(&[0, 0, 1, 0, 32, 0]);
    ico.extend_from_slice(&u32::try_from(png.len()).ok()?.to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png);
    Some(ico)
}

/// A name for a pin that is the same each time and safe as a file name.
fn hash_name(phone: &str, app: &str) -> String {
    let hash = Sha256::digest(format!("{phone}\0{app}").as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Where the shortcuts and the icons of this system go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The Start menu's Programs folder, or the applications folder.
    pub shortcuts: PathBuf,
    /// Where the icons are kept.
    pub icons: PathBuf,
}

/// The places of this system; `None` when the user's folders are unknown.
pub fn places(state_dir: &Path) -> Option<Places> {
    #[cfg(windows)]
    {
        let programs = dirs::data_dir()?
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        Some(Places {
            shortcuts: programs,
            icons: state_dir.join("phone_pin_icons"),
        })
    }
    #[cfg(not(windows))]
    {
        let _ = state_dir;
        let data = dirs::data_dir()?;
        Some(Places {
            shortcuts: data.join("applications"),
            icons: data
                .join("icons")
                .join("hicolor")
                .join("64x64")
                .join("apps"),
        })
    }
}

/// The name of "Pin to ..." on this system, for the page's words.
pub const MENU_NAME: &str = if cfg!(windows) {
    "Start"
} else {
    "the applications menu"
};

/// One pinned app: what it was called and the files made for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub label: String,
    pub file: String,
    pub icon: String,
}

type Registry = BTreeMap<String, BTreeMap<String, Pin>>;

fn read_registry(state_dir: &Path) -> Registry {
    std::fs::read_to_string(state_dir.join(PHONE_PINS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_registry(state_dir: &Path, all: &Registry) -> io::Result<()> {
    let json = serde_json::to_string(all).map_err(io::Error::other)?;
    let file = state_dir.join(PHONE_PINS_FILE);
    let tmp = crate::atomic_file::temp_beside(&file);
    std::fs::create_dir_all(state_dir)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

/// The apps of `phone` pinned here whose shortcut is still there.
pub fn pinned(state_dir: &Path, phone: &str) -> BTreeSet<String> {
    read_registry(state_dir)
        .remove(phone)
        .map(|apps| {
            apps.into_iter()
                .filter(|(_, pin)| Path::new(&pin.file).is_file())
                .map(|(app, _)| app)
                .collect()
        })
        .unwrap_or_default()
}

fn utf8(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path that is not Unicode"))
}

/// The shortcut's file name on Windows: `<label> (phone).lnk`, with a
/// number after it for the second of the same name.
pub fn lnk_name(label: &str, n: u32) -> String {
    if n <= 1 {
        format!("{label}{SUFFIX}.lnk")
    } else {
        format!("{label}{SUFFIX} {n}.lnk")
    }
}

/// Pin `phone`'s `app`, called `label` on the phone, with its icon `png`,
/// starting `exe`: its shortcut and icon are written, replacing the ones of
/// an earlier pin of the same app, and kept in the registry. The shortcut's
/// path.
pub fn pin(
    state_dir: &Path,
    places: &Places,
    exe: &Path,
    phone: &str,
    app: &str,
    label: &str,
    png: &[u8],
) -> io::Result<PathBuf> {
    if arguments(phone, app).is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    if !exe.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the app's own path is not known",
        ));
    }
    let exe_text = utf8(exe)?;
    let label = clean_label(label, app);
    let mut all = read_registry(state_dir);
    let ours = all.get(phone).and_then(|apps| apps.get(app)).cloned();
    std::fs::create_dir_all(&places.shortcuts)?;
    std::fs::create_dir_all(&places.icons)?;
    let hashed = hash_name(phone, app);
    let (icon_path, file_path) = if cfg!(windows) {
        let ico = ico_from_png(png)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an icon"))?;
        let icon_path = places.icons.join(format!("{hashed}.ico"));
        std::fs::write(&icon_path, ico)?;
        // The first name that no other file holds (the shortcut of this pin
        // is replaced).
        let own = ours.as_ref().map(|p| PathBuf::from(&p.file));
        let file_path = (1..1000)
            .map(|n| places.shortcuts.join(lnk_name(&label, n)))
            .find(|path| !path.exists() || own.as_deref() == Some(path))
            .ok_or_else(|| io::Error::other("no free name for the shortcut"))?;
        (icon_path, file_path)
    } else {
        if ico_from_png(png).is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not an icon"));
        }
        let icon_path = places.icons.join(format!("pocketdesk-phone-{hashed}.png"));
        std::fs::write(&icon_path, png)?;
        let file_path = places
            .shortcuts
            .join(format!("pocketdesk-phone-{hashed}.desktop"));
        (icon_path, file_path)
    };
    write_shortcut(&file_path, exe_text, &label, phone, app, utf8(&icon_path)?)?;
    // A name taken by an earlier pin of this app, now changed, goes.
    if let Some(old) = ours
        && Path::new(&old.file) != file_path
    {
        remove_if_in(&old.file, &places.shortcuts);
    }
    all.entry(phone.to_string()).or_default().insert(
        app.to_string(),
        Pin {
            label,
            file: utf8(&file_path)?.to_string(),
            icon: utf8(&icon_path)?.to_string(),
        },
    );
    write_registry(state_dir, &all)?;
    Ok(file_path)
}

/// Remove `file` if it is a file that sits directly in `folder`.
fn remove_if_in(file: &str, folder: &Path) {
    let path = Path::new(file);
    if path.parent() == Some(folder) && path.is_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// Unpin `phone`'s `app`: its shortcut and icon go (only files directly in
/// the shortcut and icon folders) and so does its registry entry. Whether
/// it was pinned.
pub fn unpin(state_dir: &Path, places: &Places, phone: &str, app: &str) -> io::Result<bool> {
    let mut all = read_registry(state_dir);
    let Some(apps) = all.get_mut(phone) else {
        return Ok(false);
    };
    let Some(pin) = apps.remove(app) else {
        return Ok(false);
    };
    if apps.is_empty() {
        all.remove(phone);
    }
    remove_if_in(&pin.file, &places.shortcuts);
    remove_if_in(&pin.icon, &places.icons);
    write_registry(state_dir, &all)?;
    Ok(true)
}

#[cfg(not(windows))]
fn write_shortcut(
    path: &Path,
    exe: &str,
    label: &str,
    phone: &str,
    app: &str,
    icon: &str,
) -> io::Result<()> {
    let text = desktop_entry(label, exe, phone, app, icon)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not safe to write"))?;
    std::fs::write(path, text)
}

#[cfg(windows)]
fn write_shortcut(
    path: &Path,
    exe: &str,
    label: &str,
    phone: &str,
    app: &str,
    icon: &str,
) -> io::Result<()> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    let arguments = arguments(phone, app)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not an app"))?;
    let folder = Path::new(exe)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default()
        .to_string();
    // SAFETY: COM is started for this call on this thread and released after
    // it, as the file dialog's is (`desktop/src/pick_file.rs`).
    let saved = unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let saved = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe))?;
            link.SetArguments(&HSTRING::from(arguments.as_str()))?;
            link.SetWorkingDirectory(&HSTRING::from(folder.as_str()))?;
            link.SetIconLocation(&HSTRING::from(icon), 0)?;
            link.SetDescription(&HSTRING::from(format!("Open {label} on the phone")))?;
            let file: IPersistFile = link.cast()?;
            file.Save(&HSTRING::from(path.as_os_str()), true)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        saved
    };
    saved.map_err(|e| io::Error::other(e.message()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-pins-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A tiny PNG header and a few bytes: enough for the checks that read it.
    fn png(side: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&side.to_be_bytes());
        png.extend_from_slice(&side.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4]);
        png
    }

    /// An `Exec` line as a launcher reads it (Desktop Entry Specification):
    /// the string escapes of the value first, then the arguments split at
    /// unquoted spaces, a backslash in quotes escaping `"`, a backtick, `$`
    /// and itself, and `%%` one percent sign.
    fn read_exec(line: &str) -> Vec<String> {
        let value = line.strip_prefix("Exec=").unwrap();
        // Layer one: \s \n \t \r and \\.
        let mut text = String::new();
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                text.push(c);
                continue;
            }
            match chars.next() {
                Some('\\') => text.push('\\'),
                Some('s') => text.push(' '),
                Some('n') => text.push('\n'),
                Some('t') => text.push('\t'),
                Some('r') => text.push('\r'),
                Some(other) => {
                    text.push('\\');
                    text.push(other);
                }
                None => text.push('\\'),
            }
        }
        // Layer two: quoting.
        let mut args = Vec::new();
        let mut current = String::new();
        let mut quoted = false;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => quoted = !quoted,
                '\\' if quoted => match chars.next() {
                    Some(e) if "\"`$\\".contains(e) => current.push(e),
                    other => panic!("bad escape in {line}: {other:?}"),
                },
                '%' if chars.peek() == Some(&'%') => {
                    chars.next();
                    current.push('%');
                }
                '%' => panic!("a field code in {line}"),
                ' ' if !quoted => {
                    args.push(std::mem::take(&mut current));
                }
                c => current.push(c),
            }
        }
        args.push(current);
        args
    }

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// Only a valid phone id and a valid package name are taken as the arguments.
    #[test]
    fn the_arguments_are_held_to_the_rules_of_the_packets() {
        assert_eq!(
            parse_args(args(&[
                "app",
                "--open-phone-app",
                "phone-1",
                "org.example.app"
            ])),
            Some(("phone-1".into(), "org.example.app".into()))
        );
        for bad in [
            &["app", "--open-phone-app", "phone-1"][..],
            &["app", "--open-phone-app", "phone-1", "noDots"],
            &["app", "--open-phone-app", "phone 1", "org.example.app"],
            &["app", "--open-phone-app", "../x", "org.example.app"],
            &["app", "--open-phone-app", "phone-1", "org.example.app;rm"],
            &["app", "--open-phone-app", "phone-1", "org.example.app\"x"],
            &["app", "--open-phone-app", "", "org.example.app"],
            &["app", "--page", "settings"],
            &["app", "--open-phone-app=phone-1", "org.example.app"],
        ] {
            assert_eq!(parse_args(args(bad)), None, "{bad:?}");
        }
        assert_eq!(
            arguments("phone-1", "org.example.app").as_deref(),
            Some("--open-phone-app phone-1 org.example.app")
        );
        assert_eq!(arguments("phone 1", "org.example.app"), None);
        assert_eq!(arguments("phone-1", "a b.c"), None);
    }

    /// The Exec rules: quoted, the reserved characters escaped, % doubled.
    #[test]
    fn an_exec_argument_is_quoted_as_the_specification_says() {
        assert_eq!(exec_quote("plain").as_deref(), Some("\"plain\""));
        assert_eq!(
            exec_quote("/home/a b/pocketdesk app").as_deref(),
            Some("\"/home/a b/pocketdesk app\"")
        );
        // In the file, each escape has its backslash doubled (the string
        // layer takes one off, the quoting layer reads the rest).
        assert_eq!(exec_quote("a\"b").as_deref(), Some(r#""a\\"b""#));
        assert_eq!(exec_quote("a$b`c").as_deref(), Some(r#""a\\$b\\`c""#));
        assert_eq!(
            exec_quote("a\\b").as_deref(),
            Some(r#""a\\\\b""#),
            "a backslash is four"
        );
        // And read as a launcher reads them, every argument comes back whole.
        for arg in [
            "/opt/a b/pocketdesk-app",
            "/opt/q\"uote/app",
            "/opt/back`tick/app",
            "/opt/dollar$HOME/app",
            "/opt/back\\slash/app",
            "/opt/100%/app",
            "/opt/all \"`$\\%/app",
            "--open-phone-app",
        ] {
            let line = format!(
                "Exec={} {}",
                exec_quote(arg).unwrap(),
                exec_quote("phone-1").unwrap()
            );
            assert_eq!(
                read_exec(&line),
                vec![arg.to_string(), "phone-1".to_string()],
                "{line}"
            );
        }
        assert_eq!(exec_quote("100%").as_deref(), Some("\"100%%\""));
        assert_eq!(exec_quote("a\nb"), None, "a new line would end the line");
        assert_eq!(exec_quote("a\u{0}b"), None);
    }

    /// A label from the phone can say anything; the entry still has one line each.
    #[test]
    fn a_desktop_entry_holds_only_what_was_checked() {
        let entry = desktop_entry(
            "My App",
            "/opt/pocket desk/pocketdesk-app",
            "phone-1",
            "org.example.app",
            "/home/u/.local/share/icons/x.png",
        )
        .unwrap();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nName=My App (phone)\n"));
        assert!(entry.contains(
            "\nExec=\"/opt/pocket desk/pocketdesk-app\" \"--open-phone-app\" \"phone-1\" \"org.example.app\"\n"
        ));
        assert!(entry.contains("\nIcon=/home/u/.local/share/icons/x.png\n"));
        assert!(entry.contains("\nTerminal=false\n"));
        // Every line is a key and its value: nothing a label said starts a new one.
        assert!(
            entry
                .lines()
                .all(|l| l.is_empty() || l.contains('=') || l.starts_with('['))
        );
        assert_eq!(
            desktop_entry("A\nExec=evil", "/x", "phone-1", "org.example.app", "/i.png"),
            None
        );
        assert_eq!(
            desktop_entry("A", "/x", "phone 1", "org.example.app", "/i.png"),
            None
        );
        assert_eq!(
            desktop_entry("A", "/x", "phone-1", "org.example.app;x", "/i.png"),
            None
        );
        // A backslash in a label is doubled; % in the path is never a field code.
        let odd = desktop_entry("A\\B", "/p%f/x", "phone-1", "org.example.app", "/i.png").unwrap();
        assert!(odd.contains("\nName=A\\\\B (phone)\n"));
        assert!(odd.contains("\"/p%%f/x\""));
    }

    /// A label becomes a safe part of a file name.
    #[test]
    fn a_label_is_made_safe_for_a_file_name() {
        assert_eq!(clean_label("My App", "org.example.app"), "My App");
        assert_eq!(
            clean_label("A<b>:c\"d/e\\f|g?h*i", "org.example.app"),
            "A b c d e f g h i"
        );
        assert_eq!(
            clean_label("  line\none\t two  ", "org.example.app"),
            "line one two"
        );
        assert_eq!(clean_label("Name...", "org.example.app"), "Name");
        assert_eq!(clean_label("???", "org.example.app"), "app");
        assert_eq!(clean_label("", "org.example.chat_2"), "chat_2");
        assert_eq!(
            clean_label(&"x".repeat(200), "org.example.app").len(),
            MAX_LABEL
        );
        assert_eq!(lnk_name("A", 1), "A (phone).lnk");
        assert_eq!(lnk_name("A", 2), "A (phone) 2.lnk");
    }

    /// The icon file holds the PNG whole behind a one-entry directory.
    #[test]
    fn an_icon_file_holds_the_png() {
        let png = png(64);
        let ico = ico_from_png(&png).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!((ico[6], ico[7]), (64, 64));
        assert_eq!(
            u32::from_le_bytes(ico[14..18].try_into().unwrap()) as usize,
            png.len()
        );
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 22);
        assert_eq!(&ico[22..], &png[..]);
        assert_eq!(ico_from_png(&self::png(256)).unwrap()[6], 0, "256 is 0");
        assert_eq!(ico_from_png(&self::png(257)), None);
        assert_eq!(ico_from_png(&self::png(0)), None);
        assert_eq!(ico_from_png(b"not a png at all, not at all"), None);
        assert_eq!(ico_from_png(b""), None);
    }

    /// Pin writes the shortcut and the icon and remembers them; Unpin removes
    /// exactly those, and nothing a tampered registry names.
    #[test]
    fn a_pin_is_made_and_unmade() {
        let state = dir("state");
        let root = dir("places");
        let places = Places {
            shortcuts: root.join("shortcuts"),
            icons: root.join("icons"),
        };
        let exe = std::env::current_exe().unwrap();
        let bystander = {
            std::fs::create_dir_all(&places.shortcuts).unwrap();
            let f = places.shortcuts.join("Bystander.txt");
            std::fs::write(&f, "mine").unwrap();
            f
        };
        assert!(pinned(&state, "phone-1").is_empty());
        let file = pin(
            &state,
            &places,
            &exe,
            "phone-1",
            "org.example.app",
            "My App",
            &png(64),
        )
        .unwrap();
        assert!(file.is_file());
        assert_eq!(file.parent(), Some(places.shortcuts.as_path()));
        if cfg!(windows) {
            assert_eq!(
                file.file_name().unwrap().to_str(),
                Some("My App (phone).lnk")
            );
            let bytes = std::fs::read(&file).unwrap();
            // A shell link: its header says 0x4C, and its strings are UTF-16.
            assert_eq!(&bytes[..4], &[0x4c, 0, 0, 0]);
            let wide: Vec<u8> = "--open-phone-app phone-1 org.example.app"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            assert!(bytes.windows(wide.len()).any(|w| w == wide.as_slice()));
        } else {
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(text.contains("Name=My App (phone)"));
            assert!(text.contains("\"--open-phone-app\" \"phone-1\" \"org.example.app\""));
        }
        assert_eq!(std::fs::read_dir(&places.icons).unwrap().count(), 1);
        assert_eq!(
            pinned(&state, "phone-1"),
            BTreeSet::from(["org.example.app".to_string()])
        );
        assert!(pinned(&state, "phone-2").is_empty());
        // Pinned again: the same files, no second shortcut.
        let again = pin(
            &state,
            &places,
            &exe,
            "phone-1",
            "org.example.app",
            "My App",
            &png(64),
        )
        .unwrap();
        assert_eq!(again, file);
        assert_eq!(std::fs::read_dir(&places.shortcuts).unwrap().count(), 2);
        // A registry that names a file elsewhere is not followed out of the folders.
        let elsewhere = root.join("elsewhere.txt");
        std::fs::write(&elsewhere, "keep").unwrap();
        let mut all = read_registry(&state);
        all.get_mut("phone-1").unwrap().insert(
            "org.example.other".into(),
            Pin {
                label: "Other".into(),
                file: elsewhere.to_str().unwrap().into(),
                icon: elsewhere.to_str().unwrap().into(),
            },
        );
        write_registry(&state, &all).unwrap();
        assert!(unpin(&state, &places, "phone-1", "org.example.other").unwrap());
        assert!(elsewhere.is_file(), "a file outside the folders stays");
        assert!(unpin(&state, &places, "phone-1", "org.example.app").unwrap());
        assert!(!file.exists());
        assert_eq!(std::fs::read_dir(&places.icons).unwrap().count(), 0);
        assert!(bystander.is_file(), "a file that is not ours stays");
        assert!(!unpin(&state, &places, "phone-1", "org.example.app").unwrap());
        assert!(pinned(&state, "phone-1").is_empty());
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing invalid is pinned, and a shortcut a person took away is not "pinned".
    #[test]
    fn a_bad_pin_is_refused_and_a_removed_shortcut_is_not_pinned() {
        let state = dir("bad-state");
        let root = dir("bad-places");
        let places = Places {
            shortcuts: root.join("s"),
            icons: root.join("i"),
        };
        let exe = std::env::current_exe().unwrap();
        for (phone, app, png) in [
            ("phone 1", "org.example.app", png(64)),
            ("phone-1", "org example", png(64)),
            ("phone-1", "org.example.app", b"nope".to_vec()),
        ] {
            assert!(pin(&state, &places, &exe, phone, app, "A", &png).is_err());
        }
        assert!(
            pin(
                &state,
                &places,
                Path::new("relative/exe"),
                "phone-1",
                "org.example.app",
                "A",
                &png(64)
            )
            .is_err()
        );
        assert!(
            !places.shortcuts.exists()
                || std::fs::read_dir(&places.shortcuts).unwrap().count() == 0
        );
        let file = pin(
            &state,
            &places,
            &exe,
            "phone-1",
            "org.example.app",
            "A",
            &png(64),
        )
        .unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(pinned(&state, "phone-1").is_empty());
        let _ = std::fs::remove_dir_all(&state);
        let _ = std::fs::remove_dir_all(&root);
    }
}
