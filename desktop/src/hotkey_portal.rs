//! H3's and N9's keys on Linux (R4), through the desktop's GlobalShortcuts
//! portal (KDE Plasma 6, GNOME 48 and later, Hyprland's portal): one
//! session binds both, with the keys Settings names as their preferred
//! triggers; the desktop asks its owner once, and may give others. A press
//! comes as the portal's `Activated` signal. A desktop without the portal
//! takes no key, and Settings says so. Blocking zbus on threads of its own:
//! this crate's zbus runs on async-io (N10).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The shortcuts' ids with the portal.
pub const CLIPBOARD: &str = "send-clipboard";
pub const TYPING: &str = "type-on-phone";

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";

/// What the portal took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bound {
    pub clipboard: bool,
    pub typing: bool,
    /// The desktop has no GlobalShortcuts portal.
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
    pub fn bind(&self, clipboard: Option<String>, typing: Option<String>) {
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
    let session: Arc<Mutex<Option<OwnedObjectPath>>> = Arc::new(Mutex::new(None));
    // Presses, for whichever session is current.
    {
        let conn = conn.clone();
        let session = session.clone();
        let _ = std::thread::Builder::new()
            .name("portal-keys-presses".into())
            .spawn(move || {
                let Ok(portal) = Proxy::new(&conn, DESTINATION, PATH, INTERFACE) else {
                    return;
                };
                let Ok(signals) = portal.receive_signal("Activated") else {
                    return;
                };
                for message in signals {
                    let Ok((handle, id, _, _)) = message.body().deserialize::<(
                        OwnedObjectPath,
                        String,
                        u64,
                        HashMap<String, OwnedValue>,
                    )>() else {
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
            Ok(handle) => {
                *session.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
                set(Bound {
                    clipboard: shortcuts.iter().any(|(id, _, _)| *id == CLIPBOARD),
                    typing: shortcuts.iter().any(|(id, _, _)| *id == TYPING),
                    unsupported: false,
                });
            }
            Err(_) => set(Bound::default()),
        }
    }
}

/// A session with `shortcuts` bound; its handle.
fn bind(
    conn: &Connection,
    portal: &Proxy<'_>,
    shortcuts: &[(&'static str, String, &'static str)],
    token: u32,
) -> zbus::Result<OwnedObjectPath> {
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
    let (code, _) = request(conn, &bind_token, || {
        portal.call_method("BindShortcuts", &(path, list, "", options))
    })?;
    if code != 0 {
        close(conn, &handle);
        return Err(zbus::Error::Failure("the keys were refused".into()));
    }
    Ok(handle)
}

/// Make `call`, then wait for its Request's Response: subscribed before
/// the call, at the path the portal will use, so none is missed.
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
        path.as_str(),
        "org.freedesktop.portal.Request",
    )?;
    let mut responses = request.receive_signal("Response")?;
    call()?;
    let response = responses
        .next()
        .ok_or_else(|| zbus::Error::Failure("no response".into()))?;
    response
        .body()
        .deserialize::<(u32, HashMap<String, OwnedValue>)>()
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
