//! GNOME as the source of windows to lend (phase 6 of the window handoff):
//! the windows GNOME Shell lists, through the Pocketdesk GNOME Shell
//! extension where the owner has installed it, else through
//! `org.gnome.Shell.Introspect`.
//!
//! **The window list is the one thing GNOME keeps from a program.** Shell's
//! `Introspect.GetWindows` answers each window's id (the one mutter's
//! `RecordWindow` takes), title, application, size and whether it is hidden,
//! but it answers "Access denied" to any caller that is not one of GNOME's own
//! apps (measured on GNOME Shell 51 over the session bus, from an ordinary
//! program of the same user), and mutter's own interfaces give no list at all;
//! window ids are large numbers (a new base each time the Shell starts, then
//! the next one for each window made; none from 0 to 100000 was a window), so
//! they cannot be found by trying. Where the owner has installed the
//! Pocketdesk GNOME Shell extension (`contrib/gnome-shell-extension`, core's
//! `streaming::gnome_windows`) it gives the list, and the places, stacking and
//! focus a window's input is kept to it by; it is asked first. Without it this
//! asks `GetWindows`, which a Shell started in its "unsafe mode"
//! (development) answers and a stock Shell refuses, and says so in the words
//! of [`denial`]. Nothing here tries to get round a refusal.
//!
//! The decisions are plain functions over what the Shell said ([`parse_windows`],
//! [`from_extension`], [`denial`], [`cannot_because`]), tested on fixtures; the
//! call is thin.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use pocketdesk_core::streaming::gnome_windows::{self, Extension, Unavailable};
use zbus::zvariant::{OwnedValue, Value};

use super::place::{Shell, ShellWindow};

const SHELL: &str = "org.gnome.Shell";
const PATH: &str = "/org/gnome/Shell/Introspect";
const INTERFACE: &str = "org.gnome.Shell.Introspect";

/// How long the Shell may take to answer: it answers at once or not at all.
const CALL_WAIT: Duration = Duration::from_secs(5);

/// `GetWindows`' answer, as it is on the wire: `a{ta{sv}}`.
type Raw = HashMap<u64, HashMap<String, OwnedValue>>;

/// GNOME Shell, as this computer's source of windows.
pub struct Gnome;

impl Shell for Gnome {
    fn cannot(&self) -> Option<&'static str> {
        // The config as it is now, not the capture's copy of it, which is set
        // as each session starts: a lend comes before any session.
        cannot_because(crate::events::current_config().gnome_unattended)
    }

    fn windows(&self) -> io::Result<Vec<ShellWindow>> {
        // The extension first: it is the owner's own gift of the list. Not
        // being there is the usual case and no error; anything else it does
        // wrong is said if the Shell's own list is refused too.
        let extension = match gnome_windows::bus::Bus.list() {
            Ok(windows) => return Ok(from_extension(windows)),
            Err(Unavailable::NotInstalled) => None,
            Err(Unavailable::Failed(why)) => Some(why),
        };
        match on_a_thread(get_windows())? {
            Ok(raw) => Ok(parse_windows(raw)),
            Err(Failure::Named(name)) => Err(io::Error::other(denial_with(extension, &name))),
            Err(Failure::Other(why)) => Err(io::Error::other(why)),
        }
    }
}

/// Why a window cannot be lent from this computer, if one cannot. This is
/// only asked where the lend picked GNOME (`here()` does so only on GNOME).
/// GNOME's own screen cast is what shows a lent window with no dialog, which
/// is the "Unattended on GNOME" path (off by default), so the lend needs its
/// switch.
pub fn cannot_because(unattended: bool) -> Option<&'static str> {
    if !unattended {
        Some(
            "This computer lends a window of GNOME only with \"Unattended on GNOME\" on (Settings, Connection): GNOME's own screen cast is what shows it with no dialog",
        )
    } else {
        None
    }
}

/// The windows the extension lists, as the lend reads them (a window put away
/// counts as hidden, as the Shell's own list says it), in id order.
pub fn from_extension(windows: Vec<gnome_windows::Window>) -> Vec<ShellWindow> {
    let mut windows: Vec<ShellWindow> = windows
        .into_iter()
        .map(|w| ShellWindow {
            id: w.id,
            title: w.title,
            app: w.app,
            width: w.frame.2,
            height: w.frame.3,
            hidden: w.hidden || w.minimized,
        })
        .collect();
    windows.sort_by_key(|w| w.id);
    windows
}

/// [`denial`], knowing what the extension did: if it was there and failed
/// (`extension` says how), that is what the owner hears, not that it is not
/// installed.
pub fn denial_with(extension: Option<&'static str>, error_name: &str) -> String {
    match extension {
        Some(why) => format!(
            "The Pocketdesk GNOME Shell extension did not give the list of windows ({why}), and GNOME Shell itself does not give it to a program: turn the extension off and on again, or log out and in"
        ),
        None => denial(error_name).to_string(),
    }
}

/// What the Shell's refusal of the window list says to the owner, from the
/// D-Bus error's name, where the extension is not installed.
pub fn denial(error_name: &str) -> &'static str {
    match error_name {
        "org.freedesktop.DBus.Error.AccessDenied" => {
            "GNOME Shell does not give a program its list of windows (only GNOME's own apps may ask): install the Pocketdesk GNOME Shell extension and turn it on to lend a window"
        }
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownMethod" => {
            "GNOME Shell's window list is not there on this computer"
        }
        _ => "GNOME Shell did not give its list of windows",
    }
}

/// The windows of `GetWindows`' answer, in id order. A window with no size
/// or no title is kept as it is said (the list's own rules drop what is not
/// shown or not sized).
pub fn parse_windows(raw: Raw) -> Vec<ShellWindow> {
    let mut windows: Vec<ShellWindow> = raw
        .into_iter()
        .map(|(id, props)| {
            let class = text(&props, "wm-class");
            ShellWindow {
                id,
                title: text(&props, "title"),
                app: if class.is_empty() {
                    text(&props, "app-id")
                } else {
                    class
                },
                width: number(&props, "width"),
                height: number(&props, "height"),
                hidden: flag(&props, "is-hidden"),
            }
        })
        .collect();
    windows.sort_by_key(|w| w.id);
    windows
}

fn text(props: &HashMap<String, OwnedValue>, key: &str) -> String {
    match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => s.as_str().to_string(),
        _ => String::new(),
    }
}

fn flag(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    matches!(props.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

fn number(props: &HashMap<String, OwnedValue>, key: &str) -> u32 {
    match props.get(key).map(|v| &**v) {
        Some(Value::U32(n)) => *n,
        Some(Value::I32(n)) => u32::try_from(*n).unwrap_or(0),
        _ => 0,
    }
}

/// Why the list was not had.
#[derive(Debug)]
enum Failure {
    /// The Shell answered with this D-Bus error.
    Named(String),
    /// The bus or the call failed some other way.
    Other(&'static str),
}

/// The one session-bus connection every call to the Shell shares (the lend
/// watcher asks once a second, and a connection is not cheap to make). It is
/// made on first use, on [`runtime`] (which it needs to live as long as), and
/// dropped when a call fails any way but a D-Bus error from the Shell, so the
/// next call connects afresh.
static CONNECTION: tokio::sync::Mutex<Option<zbus::Connection>> =
    tokio::sync::Mutex::const_new(None);

async fn connection() -> Result<zbus::Connection, Failure> {
    let mut held = CONNECTION.lock().await;
    if let Some(conn) = &*held {
        return Ok(conn.clone());
    }
    let conn = tokio::time::timeout(CALL_WAIT, zbus::Connection::session())
        .await
        .map_err(|_| Failure::Other("the session bus did not answer"))?
        .map_err(|_| Failure::Other("no session bus"))?;
    *held = Some(conn.clone());
    Ok(conn)
}

/// The Shell's window list: the call, and nothing else.
async fn get_windows() -> Result<Raw, Failure> {
    let conn = connection().await?;
    let result = ask(&conn).await;
    if matches!(result, Err(Failure::Other(_))) {
        *CONNECTION.lock().await = None;
    }
    result
}

async fn ask(conn: &zbus::Connection) -> Result<Raw, Failure> {
    let reply = tokio::time::timeout(
        CALL_WAIT,
        conn.call_method(Some(SHELL), PATH, Some(INTERFACE), "GetWindows", &()),
    )
    .await
    .map_err(|_| Failure::Other("GNOME Shell did not answer"))?;
    match reply {
        Ok(message) => message
            .body()
            .deserialize::<Raw>()
            .map_err(|_| Failure::Other("GNOME Shell's window list was unreadable")),
        Err(zbus::Error::MethodError(name, _, _)) => Err(Failure::Named(name.as_str().to_string())),
        Err(_) => Err(Failure::Other("GNOME Shell could not be asked")),
    }
}

/// The runtime the shared connection lives on: one for the life of the
/// program, with a worker of its own, so a call can be made from blocking
/// code that is or is not inside the server's runtime.
fn runtime() -> io::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::OnceLock<Option<tokio::runtime::Runtime>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("gnome-shell-call")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| io::Error::other("the call to GNOME Shell could not be started"))
}

/// Run `fut` to its end on [`runtime`] and wait for it here.
fn on_a_thread<T: Send + 'static>(
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> io::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    runtime()?.spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv()
        .map_err(|_| io::Error::other("the call to GNOME Shell failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).unwrap()
    }

    fn props(
        title: &str,
        app_id: &str,
        class: &str,
        hidden: bool,
        size: (u32, u32),
    ) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("title".to_string(), owned(Value::from(title))),
            ("app-id".to_string(), owned(Value::from(app_id))),
            ("wm-class".to_string(), owned(Value::from(class))),
            ("client-type".to_string(), owned(Value::from(0u32))),
            ("is-hidden".to_string(), owned(Value::from(hidden))),
            ("has-focus".to_string(), owned(Value::from(false))),
            ("width".to_string(), owned(Value::from(size.0))),
            ("height".to_string(), owned(Value::from(size.1))),
        ])
    }

    #[test]
    fn the_shells_list_is_read_in_id_order_with_the_class_before_the_app_id() {
        let raw: Raw = HashMap::from([
            (
                2609479515,
                props(
                    "Files",
                    "org.gnome.Nautilus.desktop",
                    "org.gnome.Nautilus",
                    false,
                    (800, 600),
                ),
            ),
            (
                2609479514,
                props("wev", "window:1", "wev", true, (640, 480)),
            ),
            (
                2609479516,
                props("", "org.example.App.desktop", "", false, (0, 0)),
            ),
        ]);
        let windows = parse_windows(raw);
        assert_eq!(
            windows.iter().map(|w| w.id).collect::<Vec<_>>(),
            [2609479514, 2609479515, 2609479516]
        );
        assert_eq!(windows[0].app, "wev");
        assert!(windows[0].hidden);
        assert_eq!(windows[1].title, "Files");
        assert_eq!(windows[1].app, "org.gnome.Nautilus");
        assert_eq!((windows[1].width, windows[1].height), (800, 600));
        assert!(!windows[1].hidden);
        // No class: the app id stands in; no size: none is made up.
        assert_eq!(windows[2].app, "org.example.App.desktop");
        assert_eq!((windows[2].width, windows[2].height), (0, 0));
    }

    #[test]
    fn a_property_of_the_wrong_kind_or_missing_reads_as_nothing_never_as_something_else() {
        let odd: HashMap<String, OwnedValue> = HashMap::from([
            ("title".to_string(), owned(Value::from(5u32))),
            ("is-hidden".to_string(), owned(Value::from("true"))),
            ("width".to_string(), owned(Value::from(-4i32))),
            ("height".to_string(), owned(Value::from("tall"))),
        ]);
        let windows = parse_windows(HashMap::from([(7, odd), (8, HashMap::new())]));
        assert_eq!(windows.len(), 2);
        for w in &windows {
            assert_eq!(w.title, "");
            assert_eq!(w.app, "");
            assert!(!w.hidden, "a string is not a true");
            assert_eq!((w.width, w.height), (0, 0));
        }
        assert!(parse_windows(HashMap::new()).is_empty());
    }

    #[test]
    fn the_extensions_list_is_read_in_id_order_and_a_window_put_away_is_hidden() {
        let w = |id, minimized, hidden| gnome_windows::Window {
            id,
            title: format!("t{id}"),
            app: "app".into(),
            frame: (5, 6, 640, 480),
            minimized,
            hidden,
            ..gnome_windows::Window::default()
        };
        let windows = from_extension(vec![
            w(9, false, false),
            w(3, true, false),
            w(5, false, true),
        ]);
        assert_eq!(windows.iter().map(|w| w.id).collect::<Vec<_>>(), [3, 5, 9]);
        assert_eq!(
            windows.iter().map(|w| w.hidden).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert_eq!((windows[2].width, windows[2].height), (640, 480));
        assert_eq!(windows[2].title, "t9");
        assert!(from_extension(Vec::new()).is_empty());
    }

    #[test]
    fn a_broken_extension_is_said_and_a_missing_one_is_the_install_words() {
        let missing = denial_with(None, "org.freedesktop.DBus.Error.AccessDenied");
        assert!(
            missing.contains("install the Pocketdesk GNOME Shell extension"),
            "{missing}"
        );
        let broken = denial_with(
            Some("the GNOME Shell extension did not answer"),
            "org.freedesktop.DBus.Error.AccessDenied",
        );
        assert!(broken.contains("did not answer"), "{broken}");
        assert!(!broken.contains("install"), "{broken}");
    }

    #[test]
    fn the_refusal_is_said_in_the_owners_terms() {
        let denied = denial("org.freedesktop.DBus.Error.AccessDenied");
        assert!(denied.contains("Shell extension"), "{denied}");
        assert!(
            denied.contains("install the Pocketdesk GNOME Shell extension"),
            "{denied}"
        );
        assert!(denied.contains("GNOME's own apps"));
        for gone in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.UnknownObject",
            "org.freedesktop.DBus.Error.UnknownMethod",
        ] {
            assert!(denial(gone).contains("not there"), "{gone}");
        }
        assert_eq!(
            denial("org.freedesktop.DBus.Error.NoReply"),
            "GNOME Shell did not give its list of windows"
        );
        assert_eq!(denial(""), denial("something.else"));
    }

    #[test]
    fn a_window_is_lent_only_with_the_switch_on() {
        assert_eq!(cannot_because(true), None);
        let off = cannot_because(false).unwrap();
        assert!(off.contains("Unattended on GNOME"), "{off}");
    }

    #[test]
    fn a_call_runs_on_a_thread_of_its_own_from_inside_or_outside_a_runtime() {
        assert_eq!(on_a_thread(async { 7 }).unwrap(), 7);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert_eq!(on_a_thread(async { 8 }).unwrap(), 8);
        });
    }
}
