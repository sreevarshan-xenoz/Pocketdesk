//! H3's and N9's keys on Linux (R4), through the desktop's GlobalShortcuts
//! portal (KDE Plasma 6, GNOME 48 and later, Hyprland's portal): one
//! session binds both, with the keys Settings names as their preferred
//! triggers; the desktop asks its owner once, and may give others. A press
//! comes as the portal's `Activated` signal. A desktop without the portal
//! takes no key, and Settings says so. Blocking zbus on threads of its own:
//! this crate's zbus runs on async-io (N10).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The shortcuts' ids with the portal.
pub const CLIPBOARD: &str = "send-clipboard";
pub const TYPING: &str = "type-on-phone";

/// The app id the portal knows this program by, and its desktop file's name.
const APP_ID: &str = "dev.pocketdesk.Pocketdesk";

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// How long the owner has to answer the desktop's question (R4 review: an
/// unanswered one had held every later bind).
const ANSWER_WITHIN: Duration = Duration::from_secs(120);

/// What the portal took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bound {
    pub clipboard: bool,
    pub typing: bool,
    /// The desktop has no GlobalShortcuts portal, or it stopped passing
    /// presses on.
    pub unsupported: bool,
}

/// The portal's keys for the tray program.
pub struct PortalKeys {
    asks: mpsc::Sender<(Option<String>, Option<String>)>,
    bound: Arc<Mutex<Bound>>,
}

impl PortalKeys {
    /// Start, calling `pressed` with [`CLIPBOARD`] or [`TYPING`] on a press.
    pub fn start(pressed: impl Fn(&'static str) + Send + Sync + 'static) -> Self {
        let (asks, wanted) = mpsc::channel();
        let bound = Arc::new(Mutex::new(Bound::default()));
        let shared = bound.clone();
        let _ = std::thread::Builder::new()
            .name("portal-keys".into())
            .spawn(move || run(&wanted, &shared, Arc::new(pressed)));
        Self { asks, bound }
    }

    /// Bind these keys' texts (empty or unread: none), in place of the last.
    /// Until the portal answers, neither reads as taken (R4 review).
    pub fn bind(&self, clipboard: Option<String>, typing: Option<String>) {
        {
            let mut bound = self.bound.lock().unwrap_or_else(|e| e.into_inner());
            *bound = Bound {
                unsupported: bound.unsupported,
                ..Bound::default()
            };
        }
        let _ = self.asks.send((clipboard, typing));
    }

    pub fn bound(&self) -> Bound {
        *self.bound.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn run(
    wanted: &mpsc::Receiver<(Option<String>, Option<String>)>,
    bound: &Arc<Mutex<Bound>>,
    pressed: Arc<dyn Fn(&'static str) + Send + Sync>,
) {
    let set = |b: Bound| *bound.lock().unwrap_or_else(|e| e.into_inner()) = b;
    let unsupported = Bound {
        unsupported: true,
        ..Bound::default()
    };
    let Ok(conn) = Connection::session() else {
        set(unsupported);
        return;
    };
    let Ok(portal) = Proxy::new(&conn, DESTINATION, PATH, INTERFACE) else {
        set(unsupported);
        return;
    };
    // A portal without the interface answers no version.
    if portal.get_property::<u32>("version").is_err() {
        set(unsupported);
        return;
    }
    // Named, so the desktop knows whose keys these are (R4 review).
    register_host(&conn);
    let session: Arc<Mutex<Option<OwnedObjectPath>>> = Arc::new(Mutex::new(None));
    // Presses, for whichever session is current.
    {
        let conn = conn.clone();
        let session = session.clone();
        let bound = bound.clone();
        let _ = std::thread::Builder::new()
            .name("portal-keys-presses".into())
            .spawn(move || {
                presses(&conn, &session, &*pressed);
                // Gone: no key fires any more, and Settings says so (R4 review).
                *bound.lock().unwrap_or_else(|e| e.into_inner()) = Bound {
                    unsupported: true,
                    ..Bound::default()
                };
            });
    }
    let mut token = 0u32;
    while let Ok(mut keys) = wanted.recv() {
        // Only the newest ask matters.
        while let Ok(newer) = wanted.try_recv() {
            keys = newer;
        }
        if let Some(old) = session.lock().unwrap_or_else(|e| e.into_inner()).take() {
            close(&conn, &old);
        }
        let shortcuts: Vec<(&'static str, String, &'static str)> = [
            (
                CLIPBOARD,
                keys.0.as_deref(),
                "Send the clipboard to the phone",
            ),
            (TYPING, keys.1.as_deref(), "Type on the phone"),
        ]
        .into_iter()
        .filter_map(|(id, text, what)| {
            crate::hotkey::portal_trigger(text?).map(|trigger| (id, trigger, what))
        })
        .collect();
        if shortcuts.is_empty() {
            set(Bound::default());
            continue;
        }
        token += 1;
        match bind(&conn, &portal, &shortcuts, token) {
            Ok((handle, taken)) => {
                *session.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
                set(Bound {
                    clipboard: taken.iter().any(|id| id == CLIPBOARD),
                    typing: taken.iter().any(|id| id == TYPING),
                    unsupported: false,
                });
            }
            Err(_) => set(Bound::default()),
        }
    }
}

/// The portal's presses for the current session, until its signals end.
fn presses(
    conn: &Connection,
    session: &Arc<Mutex<Option<OwnedObjectPath>>>,
    pressed: &(dyn Fn(&'static str) + Send + Sync),
) {
    let Ok(portal) = Proxy::new(conn, DESTINATION, PATH, INTERFACE) else {
        return;
    };
    let Ok(signals) = portal.receive_signal("Activated") else {
        return;
    };
    for message in signals {
        let Ok((handle, id, _, _)) =
            message
                .body()
                .deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
        else {
            continue;
        };
        let current = session.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if current.as_ref() != Some(&handle) {
            continue;
        }
        match id.as_str() {
            CLIPBOARD => pressed(CLIPBOARD),
            TYPING => pressed(TYPING),
            _ => {}
        }
    }
}

/// The line that marks a desktop file as one this program wrote.
const STUB_MARK: &str = "X-Pocketdesk-Stub=true";

/// What to do with the desktop file of [`APP_ID`] in the owner's own
/// applications folder.
#[derive(Debug, PartialEq, Eq)]
enum Stub {
    Leave,
    Write(String),
    Remove,
}

/// The desktop file this program writes where no package installed one: it
/// starts the app, as the packaged one does, and is marked as ours.
fn stub_entry(app: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Pocketdesk\nComment=Your computer, on your \
         Android phone\nExec={}\nIcon=pocketdesk\nTerminal=false\nCategories=Network;RemoteAccess;\n\
         {STUB_MARK}\n",
        exec_quoted(app)
    )
}

/// `path` as one argument of a desktop file's `Exec`: quoted, with the
/// characters the spec reserves inside quotes escaped.
fn exec_quoted(path: &str) -> String {
    let mut out = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Whether `content` is a desktop file this program wrote: one marked so,
/// or the hidden one earlier trays wrote, which named the tray and hid the
/// packaged menu entry behind it.
fn is_ours(content: &str) -> bool {
    if content.lines().any(|l| l.trim() == STUB_MARK) {
        return true;
    }
    let lines: Vec<&str> = content.lines().collect();
    content.ends_with('\n')
        && lines.len() == 5
        && lines[0] == "[Desktop Entry]"
        && lines[1] == "Type=Application"
        && lines[2] == "Name=Pocketdesk"
        && lines[3].starts_with("Exec=")
        && lines[4] == "NoDisplay=true"
}

/// The portal names a host program only when a desktop file of its id is
/// installed. A packaged one (in a system data folder) is never shadowed:
/// ours is removed when one is there, and a file the owner wrote is never
/// touched. Otherwise ours is written, or replaced when it is out of date.
fn plan_stub(own: Option<&str>, packaged: bool, app: &str) -> Stub {
    match own {
        Some(content) if !is_ours(content) => Stub::Leave,
        Some(_) if packaged => Stub::Remove,
        None if packaged => Stub::Leave,
        Some(content) if content == stub_entry(app) => Stub::Leave,
        _ => Stub::Write(stub_entry(app)),
    }
}

/// The owner's own data folder, and the system ones, as XDG names them.
fn data_dirs() -> (Option<std::path::PathBuf>, Vec<std::path::PathBuf>) {
    let home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/share"))
        });
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    let system = system
        .split(':')
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    (home, system)
}

/// Name this program to the portal, as a sandboxed app is named by its
/// package, keeping a desktop file of its id where none is installed.
/// Portals older than the registry have none, which is no loss: they name
/// it themselves.
fn register_host(conn: &Connection) {
    let file_name = format!("{APP_ID}.desktop");
    let (home, system) = data_dirs();
    let packaged = system
        .iter()
        .any(|d| d.join("applications").join(&file_name).is_file());
    if let Some(dir) = home.map(|h| h.join("applications")) {
        let file = dir.join(&file_name);
        // The app beside this tray, as the packages install them.
        let app = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pocketdesk-app")))
            .filter(|p| p.is_file())
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "pocketdesk-app".to_string());
        let own = std::fs::read_to_string(&file).ok();
        match plan_stub(own.as_deref(), packaged, &app) {
            Stub::Leave => {}
            Stub::Remove => {
                let _ = std::fs::remove_file(&file);
            }
            Stub::Write(entry) => {
                let _ = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, entry));
            }
        }
    }
    if let Ok(registry) = Proxy::new(
        conn,
        DESTINATION,
        PATH,
        "org.freedesktop.host.portal.Registry",
    ) {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let _ = registry.call_method("Register", &(APP_ID, options));
    }
}

/// A session with `shortcuts` bound; its handle and the ids the desktop
/// took, which may be fewer than asked (R4 review).
fn bind(
    conn: &Connection,
    portal: &Proxy<'_>,
    shortcuts: &[(&'static str, String, &'static str)],
    token: u32,
) -> zbus::Result<(OwnedObjectPath, Vec<String>)> {
    let handle_token = format!("pocketdesk_keys_{}_{token}", std::process::id());
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(handle_token.as_str()));
    options.insert("session_handle_token", Value::from(handle_token.as_str()));
    let (code, results) = request(conn, &handle_token, || {
        portal.call_method("CreateSession", &(options,))
    })?;
    if code != 0 {
        return Err(zbus::Error::Failure("the session was refused".into()));
    }
    let handle = results
        .get("session_handle")
        .and_then(|v| {
            String::try_from(v.clone())
                .ok()
                .and_then(|s| OwnedObjectPath::try_from(s).ok())
                .or_else(|| OwnedObjectPath::try_from(v.clone()).ok())
        })
        .ok_or_else(|| zbus::Error::Failure("no session handle".into()))?;
    let list: Vec<(&str, HashMap<&str, Value<'_>>)> = shortcuts
        .iter()
        .map(|(id, trigger, what)| {
            let mut details: HashMap<&str, Value<'_>> = HashMap::new();
            details.insert("description", Value::from(*what));
            details.insert("preferred_trigger", Value::from(trigger.as_str()));
            (*id, details)
        })
        .collect();
    let bind_token = format!("{handle_token}_bind");
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(bind_token.as_str()));
    let path = ObjectPath::try_from(handle.as_str())?;
    let answer = request(conn, &bind_token, || {
        portal.call_method("BindShortcuts", &(path, list, "", options))
    });
    let (code, results) = match answer {
        Ok(answer) => answer,
        Err(e) => {
            close(conn, &handle);
            return Err(e);
        }
    };
    if code != 0 {
        close(conn, &handle);
        return Err(zbus::Error::Failure("the keys were refused".into()));
    }
    Ok((handle, taken_ids(&results)))
}

/// The ids in a BindShortcuts answer's `shortcuts`, `a(sa{sv})`.
fn taken_ids(results: &HashMap<String, OwnedValue>) -> Vec<String> {
    let Some(Value::Array(list)) = results.get("shortcuts").map(|v| &**v) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| match item {
            Value::Structure(fields) => match fields.fields().first() {
                Some(Value::Str(id)) => Some(id.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Make `call`, then wait for its Request's Response, at most
/// [`ANSWER_WITHIN`]: subscribed before the call, at the path the portal
/// will use, so none is missed; unanswered, the request is closed.
fn request(
    conn: &Connection,
    token: &str,
    call: impl FnOnce() -> zbus::Result<zbus::Message>,
) -> zbus::Result<(u32, HashMap<String, OwnedValue>)> {
    let sender = conn
        .unique_name()
        .map(|n| n.trim_start_matches(':').replace('.', "_"))
        .ok_or_else(|| zbus::Error::Failure("no bus name".into()))?;
    let path = format!("{PATH}/request/{sender}/{token}");
    let request = Proxy::new(
        conn,
        DESTINATION,
        path.clone(),
        "org.freedesktop.portal.Request",
    )?;
    let mut responses = request.receive_signal("Response")?;
    call()?;
    let (answered, answer) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("portal-keys-answer".into())
        .spawn(move || {
            let _ = answered.send(responses.next());
        });
    match answer.recv_timeout(ANSWER_WITHIN) {
        Ok(Some(response)) => response
            .body()
            .deserialize::<(u32, HashMap<String, OwnedValue>)>(),
        Ok(None) => Err(zbus::Error::Failure("no response".into())),
        Err(_) => {
            let _ = request.call_method("Close", &());
            Err(zbus::Error::Failure("the question was not answered".into()))
        }
    }
}

fn close(conn: &Connection, session: &OwnedObjectPath) {
    if let Ok(proxy) = Proxy::new(
        conn,
        DESTINATION,
        session.as_str(),
        "org.freedesktop.portal.Session",
    ) {
        let _ = proxy.call_method("Close", &());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_STUB: &str = "[Desktop Entry]\nType=Application\nName=Pocketdesk\nExec=/opt/pd/pocketdesk-tray\nNoDisplay=true\n";

    #[test]
    fn a_packaged_desktop_file_is_never_shadowed() {
        // Nothing of ours and a packaged one: nothing written.
        assert_eq!(
            plan_stub(None, true, "/usr/bin/pocketdesk-app"),
            Stub::Leave
        );
        // The hidden one an earlier tray wrote goes, so the menu entry shows.
        assert_eq!(
            plan_stub(Some(OLD_STUB), true, "/usr/bin/pocketdesk-app"),
            Stub::Remove
        );
        // So does a stub of ours, once a package is installed.
        let ours = stub_entry("/opt/pd/pocketdesk-app");
        assert_eq!(
            plan_stub(Some(&ours), true, "/usr/bin/pocketdesk-app"),
            Stub::Remove
        );
    }

    #[test]
    fn a_desktop_file_the_owner_wrote_is_never_touched() {
        let edited =
            "[Desktop Entry]\nType=Application\nName=My Pocketdesk\nExec=pocketdesk-app --x\n";
        assert_eq!(
            plan_stub(Some(edited), false, "pocketdesk-app"),
            Stub::Leave
        );
        assert_eq!(plan_stub(Some(edited), true, "pocketdesk-app"), Stub::Leave);
        // The old stub's shape, with a line more, is not ours either.
        let more = format!("{OLD_STUB}Icon=x\n");
        assert_eq!(plan_stub(Some(&more), true, "pocketdesk-app"), Stub::Leave);
    }

    #[test]
    fn with_no_package_ours_starts_the_app_and_shows_in_menus() {
        let app = "/home/me/pd build/target/release/pocketdesk-app";
        let Stub::Write(entry) = plan_stub(None, false, app) else {
            panic!("nothing written");
        };
        assert!(entry.contains("Exec=\"/home/me/pd build/target/release/pocketdesk-app\"\n"));
        assert!(!entry.contains("NoDisplay"));
        assert!(!entry.contains("pocketdesk-tray"));
        assert!(is_ours(&entry));
        // Up to date: left alone; the old hidden one is replaced.
        assert_eq!(plan_stub(Some(&entry), false, app), Stub::Leave);
        assert_eq!(plan_stub(Some(OLD_STUB), false, app), Stub::Write(entry));
    }

    #[test]
    fn exec_quotes_reserved_characters() {
        assert_eq!(exec_quoted("/a b/c"), "\"/a b/c\"");
        assert_eq!(exec_quoted(r#"/a"$`\b"#), r#""/a\"\$\`\\b""#);
    }
}
