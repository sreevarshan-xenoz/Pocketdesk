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

/// The hidden desktop file earlier trays wrote (its `Exec=` aside), which
/// named the tray and hid the packaged menu entry behind it.
const OLD_HIDDEN_STUB: &str =
    "[Desktop Entry]\nType=Application\nName=Pocketdesk\nExec=\nNoDisplay=true\n";

/// The desktop file this program writes now (its `Exec=` aside). When it
/// changes, the text it had joins [`PAST_STUBS`], so the files earlier
/// releases wrote are still known as ours (review N10).
const CURRENT_STUB: &str = "[Desktop Entry]\nType=Application\nName=Pocketdesk\nComment=Your computer, on your Android phone\nExec=\nIcon=pocketdesk\nTerminal=false\nCategories=Network;RemoteAccess;\nX-Pocketdesk-Stub=true\n";

/// What an `Exec=` value of ours looks like in each template.
#[derive(Debug, Clone, Copy)]
enum ExecShape {
    /// The old tray's own path, unquoted, with no argument.
    OldTray,
    /// One quoted path, as [`exec_value`] writes it, with nothing after it.
    OneQuotedPath,
}

/// Every desktop file this program has written, with the shape of its
/// `Exec=` value: a file is ours only when it is one of these exactly.
const PAST_STUBS: &[(&str, ExecShape)] = &[
    (OLD_HIDDEN_STUB, ExecShape::OldTray),
    (CURRENT_STUB, ExecShape::OneQuotedPath),
];

/// What to do with the desktop file of [`APP_ID`] in the owner's own
/// applications folder.
#[derive(Debug, PartialEq, Eq)]
enum Stub {
    Leave,
    Write(String),
    Remove,
}

/// The desktop file this program writes where no package installed one,
/// with `exec` as its `Exec=` value: it starts the app, as the packaged one
/// does, and is marked as ours.
fn stub_with_exec(exec: &str) -> String {
    CURRENT_STUB.replacen("Exec=\n", &format!("Exec={exec}\n"), 1)
}

/// Our desktop file starting `app`; none for a path a desktop file cannot
/// carry (a line break in it).
fn stub_entry(app: &str) -> Option<String> {
    exec_value(app).map(|exec| stub_with_exec(&exec))
}

/// `path` as a desktop file's `Exec=` value, one argument: quoted, with the
/// characters the spec reserves inside quotes (`"`, a backtick, `$` and the
/// backslash) escaped by a backslash; then each backslash doubled, as a
/// desktop file's strings escape it, and `%` doubled, as it is not a field
/// code (review N7). None for a path with a line break.
fn exec_value(path: &str) -> Option<String> {
    if path.contains(['\n', '\r']) {
        return None;
    }
    let backslash = char::from(92u8);
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$') || c == backslash {
            quoted.push(backslash);
        }
        quoted.push(c);
    }
    quoted.push('"');
    let mut value = String::new();
    for c in quoted.chars() {
        match c {
            '%' => value.push_str("%%"),
            c if c == backslash => {
                value.push(backslash);
                value.push(backslash);
            }
            c => value.push(c),
        }
    }
    Some(value)
}

/// `content` with its `Exec=` line's value left out, so two desktop files
/// that differ only in the program they start compare equal.
fn without_exec(content: &str) -> String {
    content
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("Exec=") {
                if line.ends_with('\n') {
                    "Exec=\n"
                } else {
                    "Exec="
                }
            } else {
                line
            }
        })
        .collect()
}

/// The `Exec=` value of `content`'s first `Exec=` line.
fn exec_of(content: &str) -> Option<&str> {
    content.lines().find_map(|line| line.strip_prefix("Exec="))
}

/// The path an [`exec_value`] value names: its two escaping layers undone,
/// and none unless writing that path again gives the very same value (one
/// quoted argument, nothing added).
fn exec_path(value: &str) -> Option<String> {
    let backslash = char::from(92u8);
    // The string layer: `%%` and doubled backslashes.
    let mut unstrung = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                if chars.next() != Some('%') {
                    return None;
                }
                unstrung.push('%');
            }
            c if c == backslash => {
                if chars.next() != Some(backslash) {
                    return None;
                }
                unstrung.push(backslash);
            }
            c => unstrung.push(c),
        }
    }
    // The quoting layer.
    let inner = unstrung.strip_prefix('"')?.strip_suffix('"')?;
    let mut path = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == backslash {
            path.push(chars.next()?);
        } else {
            path.push(c);
        }
    }
    (exec_value(&path).as_deref() == Some(value)).then_some(path)
}

/// Whether `value` has the shape of an `Exec=` value of ours (review I6:
/// an owner's added argument makes the file the owner's).
fn exec_is_ours(value: &str, shape: ExecShape) -> bool {
    match shape {
        ExecShape::OldTray => value == "pocketdesk-tray" || value.ends_with("/pocketdesk-tray"),
        ExecShape::OneQuotedPath => exec_path(value).is_some(),
    }
}

/// Whether `content` is a desktop file this program wrote and no one has
/// changed since: one of [`PAST_STUBS`] exactly, starting a program the way
/// that one did. Anything else, an entry of ours the owner edited in a menu
/// editor among them (its name, a line more, an argument), is the owner's
/// (review I5, I6).
fn is_ours(content: &str) -> bool {
    let Some(exec) = exec_of(content) else {
        return false;
    };
    let blank = without_exec(content);
    PAST_STUBS
        .iter()
        .any(|(template, shape)| blank == *template && exec_is_ours(exec, *shape))
}

/// The portal names a host program only when a desktop file of its id is
/// installed. A packaged one (in a system data folder) is never shadowed:
/// ours is removed when one is there, and a file the owner wrote or edited
/// is never touched. Otherwise ours is written, or replaced when it starts
/// another program; `app` is None when its path is not one a desktop file
/// can carry, and then nothing is written.
fn plan_stub(own: Option<&str>, packaged: bool, app: Option<&str>) -> Stub {
    match own {
        Some(content) if !is_ours(content) => return Stub::Leave,
        Some(_) if packaged => return Stub::Remove,
        None if packaged => return Stub::Leave,
        _ => {}
    }
    match app.and_then(stub_entry) {
        Some(entry) if own != Some(entry.as_str()) => Stub::Write(entry),
        _ => Stub::Leave,
    }
}

/// The owner's own data folder: `XDG_DATA_HOME` when it is absolute, else
/// `~/.local/share`.
fn data_home(xdg_data_home: Option<&str>, home: Option<&str>) -> Option<std::path::PathBuf> {
    xdg_data_home
        .map(std::path::Path::new)
        .filter(|d| d.is_absolute())
        .map(std::path::Path::to_path_buf)
        .or_else(|| {
            home.map(std::path::Path::new)
                .filter(|h| h.is_absolute())
                .map(|h| h.join(".local/share"))
        })
}

/// The system data folders from `XDG_DATA_DIRS` (its default when unset or
/// empty): absolute ones only, as the spec says, and never the owner's own,
/// which some sessions list there too, so our own file is not taken for a
/// packaged one (review N8).
fn system_data_dirs(
    xdg_data_dirs: Option<&str>,
    own: Option<&std::path::Path>,
) -> Vec<std::path::PathBuf> {
    let same = |a: &std::path::Path, b: &std::path::Path| {
        a == b
            || matches!(
                (a.canonicalize(), b.canonicalize()),
                (Ok(a), Ok(b)) if a == b
            )
    };
    xdg_data_dirs
        .filter(|d| !d.is_empty())
        .unwrap_or("/usr/local/share:/usr/share")
        .split(':')
        .map(std::path::Path::new)
        .filter(|d| d.is_absolute())
        .filter(|d| !own.is_some_and(|own| same(d, own)))
        .map(std::path::Path::to_path_buf)
        .collect()
}

/// Name this program to the portal, as a sandboxed app is named by its
/// package, keeping a desktop file of its id where none is installed.
/// Portals older than the registry have none, which is no loss: they name
/// it themselves.
fn register_host(conn: &Connection) {
    let file_name = format!("{APP_ID}.desktop");
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let home = data_home(var("XDG_DATA_HOME").as_deref(), var("HOME").as_deref());
    let packaged = system_data_dirs(var("XDG_DATA_DIRS").as_deref(), home.as_deref())
        .iter()
        .any(|d| d.join("applications").join(&file_name).is_file());
    if let Some(dir) = home.map(|h| h.join("applications")) {
        let file = dir.join(&file_name);
        // The app beside this tray, as the packages install them; a path
        // that is not UTF-8 is not written into the file.
        let beside = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("pocketdesk-app")))
            .filter(|p| p.is_file());
        let app = match &beside {
            Some(path) => path.to_str().map(str::to_string),
            None => Some("pocketdesk-app".to_string()),
        };
        let own = std::fs::read_to_string(&file).ok();
        match plan_stub(own.as_deref(), packaged, app.as_deref()) {
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

    const OLD: &str = "[Desktop Entry]\nType=Application\nName=Pocketdesk\nExec=/opt/pd/pocketdesk-tray\nNoDisplay=true\n";

    fn ours(app: &str) -> String {
        stub_entry(app).expect("a path a desktop file carries")
    }

    #[test]
    fn a_packaged_desktop_file_is_never_shadowed() {
        let app = Some("/usr/bin/pocketdesk-app");
        // Nothing of ours and a packaged one: nothing written.
        assert_eq!(plan_stub(None, true, app), Stub::Leave);
        // The hidden one an earlier tray wrote goes, so the menu entry shows.
        assert_eq!(plan_stub(Some(OLD), true, app), Stub::Remove);
        // So does an entry of ours, once a package is installed, whatever
        // program it started.
        let earlier = ours("/opt/pd/pocketdesk-app");
        assert_eq!(plan_stub(Some(&earlier), true, app), Stub::Remove);
    }

    #[test]
    fn a_desktop_file_the_owner_wrote_or_edited_is_never_touched() {
        let app = Some("pocketdesk-app");
        let written =
            "[Desktop Entry]\nType=Application\nName=My Pocketdesk\nExec=pocketdesk-app --x\n";
        // Ours, edited in a menu editor (the mark kept): renamed, hidden,
        // or with a line more. Neither rewritten nor removed.
        let renamed = ours("pocketdesk-app").replace("Name=Pocketdesk", "Name=My Pocketdesk");
        let hidden = format!("{}Hidden=true\n", ours("pocketdesk-app"));
        let old_more = format!("{OLD}Icon=x\n");
        for edited in [written, &renamed, &hidden, &old_more] {
            assert_eq!(plan_stub(Some(edited), false, app), Stub::Leave, "{edited}");
            assert_eq!(plan_stub(Some(edited), true, app), Stub::Leave, "{edited}");
        }
    }

    #[test]
    fn an_argument_the_owner_added_makes_the_file_the_owners() {
        let app = "/opt/pd/pocketdesk-app";
        let entry = ours(app);
        let exec = exec_value(app).unwrap();
        let minimized = entry.replace(&exec, &format!("{exec} --minimized"));
        assert_eq!(plan_stub(Some(&minimized), false, Some(app)), Stub::Leave);
        assert_eq!(plan_stub(Some(&minimized), true, Some(app)), Stub::Leave);
        let wrapped = entry.replace(&exec, &format!("env FOO=1 {exec}"));
        assert_eq!(plan_stub(Some(&wrapped), true, Some(app)), Stub::Leave);
        // The old hidden one, with an argument added: the owner's too.
        let old_minimized = OLD.replace("pocketdesk-tray", "pocketdesk-tray --minimized");
        assert_eq!(
            plan_stub(Some(&old_minimized), true, Some(app)),
            Stub::Leave
        );
        // Ours as written, any path: still ours.
        for path in ["/a b/pocketdesk-app", "/50%/x$y/pocketdesk-app"] {
            assert_eq!(exec_path(&exec_value(path).unwrap()).as_deref(), Some(path));
        }
    }

    #[test]
    fn the_template_written_now_is_pinned_and_listed() {
        // Changing it fails here until the text it had joins PAST_STUBS.
        let expected = [
            "[Desktop Entry]",
            "Type=Application",
            "Name=Pocketdesk",
            "Comment=Your computer, on your Android phone",
            "Exec=",
            "Icon=pocketdesk",
            "Terminal=false",
            "Categories=Network;RemoteAccess;",
            "X-Pocketdesk-Stub=true",
            "",
        ]
        .join("\n");
        assert_eq!(CURRENT_STUB, expected);
        assert!(PAST_STUBS.iter().any(|(t, _)| *t == CURRENT_STUB));
        assert!(PAST_STUBS.iter().any(|(t, _)| *t == OLD_HIDDEN_STUB));
    }

    #[test]
    fn with_no_package_ours_starts_the_app_and_shows_in_menus() {
        let app = "/home/me/pd build/target/release/pocketdesk-app";
        let Stub::Write(entry) = plan_stub(None, false, Some(app)) else {
            panic!("nothing written");
        };
        assert!(entry.contains("Exec=\"/home/me/pd build/target/release/pocketdesk-app\"\n"));
        assert!(!entry.contains("NoDisplay"));
        assert!(!entry.contains("pocketdesk-tray"));
        assert!(is_ours(&entry));
        // Up to date: left alone. The old hidden one, or ours starting
        // another program, is replaced.
        assert_eq!(plan_stub(Some(&entry), false, Some(app)), Stub::Leave);
        assert_eq!(
            plan_stub(Some(OLD), false, Some(app)),
            Stub::Write(entry.clone())
        );
        let moved = ours("/elsewhere/pocketdesk-app");
        assert_eq!(
            plan_stub(Some(&moved), false, Some(app)),
            Stub::Write(entry)
        );
        // A path a desktop file cannot carry: nothing written.
        assert_eq!(plan_stub(None, false, None), Stub::Leave);
        assert_eq!(plan_stub(None, false, Some("/a/b\nc")), Stub::Leave);
    }

    #[test]
    fn exec_escapes_quotes_then_the_strings_backslashes_and_percent() {
        let b = char::from(92u8).to_string();
        // A space: quoted only.
        assert_eq!(exec_value("/a b/app").unwrap(), "\"/a b/app\"");
        // `$`, a backtick and `"` get a backslash inside the quotes, and
        // that backslash is doubled as a string escape: two in the file.
        assert_eq!(
            exec_value("/a$b`c\"d").unwrap(),
            format!("\"/a{b}{b}$b{b}{b}`c{b}{b}\"d\"")
        );
        // A backslash: escaped inside the quotes (two), each doubled (four).
        assert_eq!(
            exec_value(&format!("/a{b}x")).unwrap(),
            format!("\"/a{b}{b}{b}{b}x\"")
        );
        // `%` is doubled, so it is not a field code.
        assert_eq!(exec_value("/50%/app").unwrap(), "\"/50%%/app\"");
        // A line break: refused.
        assert_eq!(exec_value("/a\rb"), None);
    }

    #[test]
    fn only_absolute_system_folders_count_and_never_the_owners_own() {
        let home = data_home(Some("/home/me/.local/share"), Some("/home/me"));
        assert_eq!(
            home.as_deref(),
            Some(std::path::Path::new("/home/me/.local/share"))
        );
        // A relative XDG_DATA_HOME is ignored for the default.
        let home = data_home(Some("rel/share"), Some("/home/me")).unwrap();
        assert_eq!(home, std::path::Path::new("/home/me/.local/share"));
        let dirs = system_data_dirs(
            Some("/home/me/.local/share:relative/share::/usr/share"),
            Some(&home),
        );
        assert_eq!(dirs, vec![std::path::PathBuf::from("/usr/share")]);
        // Unset or empty: the spec's default.
        assert_eq!(
            system_data_dirs(None, Some(&home)),
            vec![
                std::path::PathBuf::from("/usr/local/share"),
                std::path::PathBuf::from("/usr/share")
            ]
        );
        assert_eq!(system_data_dirs(Some(""), None).len(), 2);
    }
}
