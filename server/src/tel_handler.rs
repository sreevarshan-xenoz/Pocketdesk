//! Phone numbers in `tel:` and `callto:` links open on the phone (M8).
//!
//! `pocketdesk tel <uri>` takes a link's number to the phone that calls it
//! ([`tel_in`]); `pocketdesk tel-handler [--remove]` registers it for the
//! links, HKCU only on Windows (a ProgID, the Capabilities key and
//! `RegisteredApplications`, after which the owner picks Pocketdesk), a
//! desktop file made the default with `xdg-mime` on Linux. A marker in the
//! config folder tells the desktop app's row it is registered.

use std::path::Path;

use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use pocketdesk_core::protocol::permissions::Capability;

use crate::local_api::exit;

/// The marker the desktop app reads, in the config folder.
pub const MARKER: &str = "tel_handler.json";

/// The ProgID the links' associations name (Windows).
pub const PROG_ID: &str = "Pocketdesk.tel";

/// The schemes taken.
pub const SCHEMES: [&str; 2] = ["tel", "callto"];

/// Where a link's number goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// This phone calls it.
    Phone(String),
    /// Several could: the desktop app asks.
    Ask,
    /// None may: why.
    Nobody(&'static str),
}

/// The phone a link's number goes to, of the paired phones that allow
/// "Call numbers from here" (`allowed`) and those linked now (`linked`):
/// the one linked, else the one at all, else ask.
pub fn pick(allowed: &[String], linked: &[String]) -> Pick {
    if allowed.is_empty() {
        return Pick::Nobody(
            "No paired phone allows \"Call numbers from here\" (the Devices page).",
        );
    }
    let reached: Vec<&String> = allowed.iter().filter(|id| linked.contains(id)).collect();
    match (reached.as_slice(), allowed) {
        ([one], _) => Pick::Phone((*one).clone()),
        ([], [one]) => Pick::Phone(one.clone()),
        _ => Pick::Ask,
    }
}

/// `pocketdesk tel <uri>` over `state_dir`: the notification's summary and
/// body, the exit code, and whether the desktop app should open to ask.
pub fn tel_in(
    state_dir: &Path,
    uri: &str,
    now_unix: u64,
    running: bool,
) -> (String, String, i32, bool) {
    let not_called = |body: String, code: i32| ("Not called".to_string(), body, code, false);
    let Some(number) = pocketdesk_core::protocol::dial::tel_number(uri) else {
        return not_called(
            "That link names no number the phone can dial: digits, a + only at the start, no codes or extensions.".to_string(),
            exit::BAD_ARGUMENTS,
        );
    };
    if !running {
        return not_called(
            "Pocketdesk is not running on this computer.".to_string(),
            exit::NO_SERVER,
        );
    }
    let store = match TrustStore::read(state_dir.join(TRUST_FILE)) {
        Ok(store) => store,
        Err(e) => {
            return not_called(
                format!("The paired phones could not be read: {e:#}"),
                exit::FAILED,
            );
        }
    };
    let allowed: Vec<String> = store
        .trusted_devices()
        .into_iter()
        .filter(|id| !store.is_computer(id) && store.permissions(id).allows(Capability::Dial))
        .collect();
    let linked = crate::local_api::linked_phones(state_dir, now_unix, running);
    match pick(&allowed, &linked) {
        Pick::Nobody(why) => not_called(why.to_string(), exit::FAILED),
        Pick::Ask => match pocketdesk_core::local::tel_ask::put(state_dir, &number, now_unix) {
            Ok(()) => (
                format!("Call {number}?"),
                "Choose the phone in Pocketdesk.".to_string(),
                exit::DONE,
                true,
            ),
            Err(e) => not_called(format!("The question could not be kept: {e}"), exit::FAILED),
        },
        Pick::Phone(id) => {
            let (said, code) = crate::local_api::call_in(state_dir, &id, &number, running);
            if code == exit::DONE {
                let name = store.name(&id).unwrap_or(&id).to_string();
                (
                    format!("Calling {number}"),
                    format!("Press Call on {name}."),
                    code,
                    false,
                )
            } else {
                let why = said["error"]
                    .as_str()
                    .unwrap_or("it could not be queued")
                    .to_string();
                not_called(why, code)
            }
        }
    }
}

/// `pocketdesk tel <uri>`: [`tel_in`] for the owner's state folder, said on
/// stdout and in a desktop notification (a browser shows no console), and
/// the desktop app opened when it should ask. The exit code.
pub fn run_tel(uri: &str) -> i32 {
    let (summary, body, code, ask) = match crate::state::state_dir() {
        Ok(dir) => {
            let running = pocketdesk_core::local::server::is_running(&dir);
            tel_in(&dir, uri, pocketdesk_core::local::pin::now_unix(), running)
        }
        Err(e) => (
            "Not called".to_string(),
            format!("No state folder: {e:#}"),
            exit::FAILED,
            false,
        ),
    };
    if code == exit::DONE {
        println!("{summary}: {body}");
    } else {
        eprintln!("{summary}: {body}");
    }
    let (summary, body) = if ask && !open_app() {
        (
            "Not called".to_string(),
            "Pocketdesk's window could not be opened to ask which phone.".to_string(),
        )
    } else {
        (summary, body)
    };
    if !ask || summary == "Not called" {
        crate::platform::notify(&summary, &body, std::time::Duration::from_secs(5));
    }
    code
}

/// Start the desktop app on the Overview, where the question is: a window
/// running already is raised by the new one, which then exits. Whether it
/// started.
fn open_app() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let app = exe.with_file_name(if cfg!(windows) {
        "pocketdesk-app.exe"
    } else {
        "pocketdesk-app"
    });
    match std::process::Command::new(&app)
        .args(["--page", "overview"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Pocketdesk could not be opened to ask which phone ({e})");
            false
        }
    }
}

/// The Linux desktop file: `Exec` runs `exe tel %u`.
pub fn desktop_file(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Pocketdesk: call on the phone\nExec={} tel %u\nNoDisplay=true\nMimeType=x-scheme-handler/tel;x-scheme-handler/callto;\n",
        pocketdesk_core::local::send_menu::desktop_exec_quote(&exe.to_string_lossy())
    )
}

/// The desktop file's name.
pub const DESKTOP_FILE: &str = "pocketdesk-tel.desktop";

/// `pocketdesk tel-handler [--remove]`.
pub fn run_tel_handler(remove: bool) -> anyhow::Result<()> {
    let config = pocketdesk_core::local::paths::config_dir()?;
    if remove {
        let was = config.join(MARKER).exists();
        unregister()?;
        let _ = std::fs::remove_file(config.join(MARKER));
        if was {
            println!("Phone numbers no longer open on the phone");
        }
    } else {
        let exe = std::env::current_exe()?;
        register(&exe)?;
        std::fs::create_dir_all(&config)?;
        std::fs::write(config.join(MARKER), b"{\"registered\":true}")?;
        if cfg!(windows) {
            println!(
                "Registered for phone numbers: pick Pocketdesk when Windows asks, or in Settings, Apps, Default apps"
            );
        } else {
            println!("Phone numbers now open on the phone");
        }
    }
    Ok(())
}

/// What Windows runs for a link: the window's program, which has no console
/// and hands the link to the server program with none either (M8 review:
/// the server program flashed a console at each link); the server program
/// itself when the window's is not beside it.
#[cfg(windows)]
pub fn windows_command(exe: &Path) -> String {
    let app = exe.with_file_name("pocketdesk-app.exe");
    if app.is_file() {
        format!("\"{}\" --tel \"%1\"", app.display())
    } else {
        format!("\"{}\" tel \"%1\"", exe.display())
    }
}

#[cfg(windows)]
fn register(exe: &Path) -> anyhow::Result<()> {
    use crate::platform::{set_user_default_value, set_user_value};
    let command = windows_command(exe);
    // No "URL Protocol": the ProgID is not a scheme of its own (M8 review).
    let class = format!(r"Software\Classes\{PROG_ID}");
    set_user_default_value(&class, "Phone number (Pocketdesk)")?;
    set_user_default_value(&format!(r"{class}\shell\open\command"), &command)?;
    let caps = r"Software\Pocketdesk\Capabilities";
    set_user_value(caps, "ApplicationName", "Pocketdesk")?;
    set_user_value(
        caps,
        "ApplicationDescription",
        "Phone numbers open in your phone's dialer",
    )?;
    for scheme in SCHEMES {
        set_user_value(&format!(r"{caps}\URLAssociations"), scheme, PROG_ID)?;
    }
    set_user_value(r"Software\RegisteredApplications", "Pocketdesk", caps)?;
    Ok(())
}

#[cfg(windows)]
fn unregister() -> anyhow::Result<()> {
    use crate::platform::{delete_user_key, delete_user_value};
    delete_user_key(&format!(r"Software\Classes\{PROG_ID}"));
    delete_user_key(r"Software\Pocketdesk\Capabilities");
    delete_user_value(r"Software\RegisteredApplications", "Pocketdesk");
    // Its parent, only once empty (M8 review: an empty key was left).
    crate::platform::delete_user_key_if_empty(r"Software\Pocketdesk");
    Ok(())
}

#[cfg(not(windows))]
fn applications_dir() -> anyhow::Result<std::path::PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .ok_or_else(|| anyhow::anyhow!("no home folder"))?;
    Ok(data.join("applications"))
}

#[cfg(not(windows))]
fn register(exe: &Path) -> anyhow::Result<()> {
    let dir = applications_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(DESKTOP_FILE), desktop_file(exe))?;
    for scheme in SCHEMES {
        let made = std::process::Command::new("xdg-mime")
            .args([
                "default",
                DESKTOP_FILE,
                &format!("x-scheme-handler/{scheme}"),
            ])
            .status();
        if !made.is_ok_and(|s| s.success()) {
            // Nothing half made is left (M8 review).
            let _ = unregister();
            anyhow::bail!("xdg-mime could not make it the default for {scheme}: links");
        }
    }
    Ok(())
}

/// The user's `mimeapps.list`.
#[cfg(not(windows))]
fn mimeapps() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|config| config.join("mimeapps.list"))
}

#[cfg(not(windows))]
fn unregister() -> anyhow::Result<()> {
    let _ = std::fs::remove_file(applications_dir()?.join(DESKTOP_FILE));
    // The defaults xdg-mime set name a file gone now: dropped, so the
    // system finds its other handlers again (M8 review).
    if let Some(list) = mimeapps()
        && let Ok(raw) = std::fs::read_to_string(&list)
    {
        let kept = without_desktop_file(&raw);
        if kept != raw {
            std::fs::write(&list, kept)?;
        }
    }
    Ok(())
}

/// `mimeapps.list` without [`DESKTOP_FILE`] in its `tel` and `callto`
/// handlers; a line left naming nothing goes.
pub fn without_desktop_file(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let scheme_line = SCHEMES
            .iter()
            .any(|s| line.starts_with(&format!("x-scheme-handler/{s}=")));
        if scheme_line && let Some((key, apps)) = line.split_once('=') {
            let rest: Vec<&str> = apps
                .split(';')
                .filter(|a| !a.is_empty() && *a != DESKTOP_FILE)
                .collect();
            if !rest.is_empty() {
                out.push_str(&format!("{key}={};\n", rest.join(";")));
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_linked_now_else_the_only_one_else_ask() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick(&ids(&["a"]), &[]),
            Pick::Phone("a".into()),
            "the only one"
        );
        assert_eq!(
            pick(&ids(&["a", "b"]), &ids(&["b"])),
            Pick::Phone("b".into()),
            "the one linked"
        );
        assert_eq!(
            pick(&ids(&["a", "b"]), &[]),
            Pick::Ask,
            "several, none linked"
        );
        assert_eq!(
            pick(&ids(&["a", "b"]), &ids(&["a", "b"])),
            Pick::Ask,
            "several linked"
        );
        assert_eq!(
            pick(&ids(&["a"]), &ids(&["c"])),
            Pick::Phone("a".into()),
            "a linked phone that may not dial does not count"
        );
        assert!(matches!(pick(&[], &ids(&["a"])), Pick::Nobody(_)));
    }

    #[test]
    fn a_bad_link_is_refused_before_anything_is_read() {
        let dir = std::path::Path::new("/nonexistent/pocketdesk-tel-test");
        let (_, body, code, ask) = tel_in(dir, "tel:*%2306%23", 0, true);
        assert_eq!(code, exit::BAD_ARGUMENTS);
        assert!(body.contains("no number"));
        assert!(!ask);
        let (_, body, code, _) = tel_in(dir, "tel:5550100", 0, false);
        assert_eq!(code, exit::NO_SERVER);
        assert!(body.contains("not running"));
    }

    /// Writes and removes this user's registration: run by hand.
    #[cfg(windows)]
    #[test]
    #[ignore = "writes HKCU; run by hand"]
    fn the_registry_keys_are_written_read_back_and_removed() {
        use crate::platform::user_value;
        let exe = Path::new(r"C:\Program Files\Pocketdesk\pocketdesk.exe");
        register(exe).unwrap();
        let class = format!(r"Software\Classes\{PROG_ID}");
        assert_eq!(
            user_value(&format!(r"{class}\shell\open\command"), None).as_deref(),
            Some(r#""C:\Program Files\Pocketdesk\pocketdesk.exe" tel "%1""#)
        );
        for scheme in SCHEMES {
            assert_eq!(
                user_value(
                    r"Software\Pocketdesk\Capabilities\URLAssociations",
                    Some(scheme)
                )
                .as_deref(),
                Some(PROG_ID)
            );
        }
        assert_eq!(
            user_value(r"Software\RegisteredApplications", Some("Pocketdesk")).as_deref(),
            Some(r"Software\Pocketdesk\Capabilities")
        );
        unregister().unwrap();
        assert_eq!(user_value(&class, None), None);
        assert_eq!(
            user_value(r"Software\RegisteredApplications", Some("Pocketdesk")),
            None
        );
    }

    /// M8 review: the phone that may not dial is never chosen.
    #[test]
    fn only_a_phone_that_may_dial_is_called() {
        let dir = std::env::temp_dir().join(format!("pd-tel-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = TrustStore::load_or_create(dir.join(TRUST_FILE)).unwrap();
        store.trust_device("phone-a").unwrap();
        store.trust_device("phone-b").unwrap();
        let mut off = store.permissions("phone-a");
        off.set(Capability::Dial, false);
        store.set_permissions("phone-a", off).unwrap();
        let (_, _, code, ask) = tel_in(&dir, "tel:5550100", 1_000, true);
        assert_eq!(code, exit::DONE);
        assert!(!ask, "one phone may dial: no question");
        // The request is stamped with the clock's time.
        let dialled =
            pocketdesk_core::local::dials::take_all(&dir, pocketdesk_core::local::pin::now_unix());
        assert_eq!(dialled.len(), 1);
        assert_eq!(dialled[0].device_id, "phone-b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mimeapps_loses_only_this_handler() {
        let raw = "[Default Applications]\nx-scheme-handler/tel=pocketdesk-tel.desktop;\nx-scheme-handler/callto=other.desktop;pocketdesk-tel.desktop;\ntext/html=firefox.desktop\n";
        assert_eq!(
            without_desktop_file(raw),
            "[Default Applications]\nx-scheme-handler/callto=other.desktop;\ntext/html=firefox.desktop\n"
        );
    }

    #[test]
    fn the_desktop_file_runs_tel_with_the_link() {
        let file = desktop_file(Path::new("/opt/pocket desk/pocketdesk"));
        assert!(file.contains("Exec=\"/opt/pocket desk/pocketdesk\" tel %u\n"));
        assert!(file.contains("MimeType=x-scheme-handler/tel;x-scheme-handler/callto;"));
    }
}
