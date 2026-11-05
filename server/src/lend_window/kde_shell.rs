//! KDE Plasma's window table and screen-sharing portal, over D-Bus: the real
//! [`Kwin`] (see [`super::kde`] for what is lent and why, and
//! `pocketdesk_core::streaming::window_kde` for the table and its rules).
//!
//! **The script.** One resident KWin script ([`super::kde::script`]) is loaded
//! the first time a window is listed or lent and unloaded when none is lent
//! (at once) or a minute after the last ask (a list nobody lent from).
//! Loading is `org.kde.KWin /Scripting loadScript` and `run` on the script's
//! object, under a fixed plugin name, so a script a killed server left is
//! replaced, never doubled. The script file is written owner-only in the
//! user's runtime folder (never a shared one) and removed once KWin has it.
//!
//! **What comes back.** The script calls `Update` on a D-Bus object this
//! program serves on a connection of its own, at the connection's unique name.
//! Only the owner of `org.kde.KWin` on the bus is listened to (any other
//! sender's report is dropped), and every report is checked whole
//! (`window_kde::publish`): a message that is not exactly what the script
//! sends changes nothing, and the table then ages until it is not believed.
//! The reply to a report carries the windows the guard asked to raise.
//!
//! **Asking the script to report at once.** KWin scripts have no object of
//! their own on the bus, so the script registers a global shortcut with no
//! keys and the server triggers it through KDE's global-shortcut service
//! (`org.kde.kglobalaccel`, component `kwin`, `invokeShortcut`); the script
//! then reports and the reply carries any raise. That is asked on each look at
//! the windows (the lend watcher looks once a second), so a lent window's
//! table is never more than about a second old. If loading finds that the
//! shortcut does nothing, it is not used and the table is only as current as
//! KWin's own change signals, which cover every change; a table that is a
//! minute old is replaced by loading the script again.
//!
//! The decisions are in [`super::kde`] and `window_kde` and are tested there;
//! this file is the calls, which are thin and checked on a KDE session only
//! (the live test of `server/tests/lend_window_kde_live.rs`).

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pocketdesk_core::streaming::window_kde::{self, Refusal, Snapshot};
use zbus::Connection;

use super::kde::{Kwin, OBJECT_PATH, PLUGIN, POKE_SHORTCUT, Portal, Session, script};

const KWIN: &str = "org.kde.KWin";
const SCRIPTING: &str = "/Scripting";
const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_INTERFACE: &str = "org.kde.kwin.Script";

/// How long a call to the bus may take: it answers at once or not at all.
const CALL_WAIT: Duration = Duration::from_secs(5);
/// How long a freshly loaded script may take to report once.
const FIRST_REPORT_WAIT: Duration = Duration::from_secs(3);
/// How long a triggered report may take.
const POKE_WAIT: Duration = Duration::from_millis(1500);
/// A table this old with no report is taken as the script gone: it is loaded
/// again.
const DEAD_AFTER: Duration = Duration::from_secs(60);
/// How long a script nobody lent from stays loaded after the last ask.
const IDLE_UNLOAD: Duration = Duration::from_secs(60);
/// How long a caller waits for the whole of a call on the runtime.
const WHOLE_WAIT: Duration = Duration::from_secs(30);

/// Why the window table could not be had.
#[derive(Debug, Clone, Copy)]
enum Failure {
    Refused(Refusal),
    Say(&'static str),
}

impl Failure {
    fn said(self) -> &'static str {
        match self {
            Self::Refused(why) => window_kde::say(why),
            Self::Say(text) => text,
        }
    }
}

/// The owner of `org.kde.KWin` on the bus: the only sender a report is taken
/// from.
static OWNER: Mutex<String> = Mutex::new(String::new());

/// How many windows are lent (set by the lend's record).
static LENT: AtomicUsize = AtomicUsize::new(0);

/// Whether the reaper of an idle script is running.
static REAPER: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct State {
    /// The bus connection that serves the reports' object, made on first use.
    conn: Option<Connection>,
    /// The script is loaded and has reported.
    loaded: bool,
    /// The shortcut that makes it report was shown to work.
    poke_works: bool,
    last_ask: Option<Instant>,
}

static STATE: tokio::sync::Mutex<State> = tokio::sync::Mutex::const_new(State {
    conn: None,
    loaded: false,
    poke_works: false,
    last_ask: None,
});

/// The object the script reports to.
struct Sink;

// The interface name is `super::kde::INTERFACE`.
#[zbus::interface(name = "dev.pocketdesk.Windows")]
impl Sink {
    /// The script's report; the reply names the windows to raise.
    async fn update(
        &self,
        json: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Vec<String> {
        let sender = header.sender().map(ToString::to_string);
        let owner = OWNER.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if owner.is_empty() || sender.as_deref() != Some(owner.as_str()) {
            tracing::debug!("A report to the window table from another sender was dropped");
            return Vec::new();
        }
        match window_kde::publish(&json) {
            Ok(()) => window_kde::take_raises(),
            Err(e) => {
                tracing::debug!("A report to the window table was not taken: {e}");
                Vec::new()
            }
        }
    }
}

/// The runtime the connection lives on: one for the life of the program, with
/// a worker of its own, so a call can be made from blocking code that is or
/// is not inside the server's runtime.
fn runtime() -> io::Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: std::sync::OnceLock<Option<tokio::runtime::Runtime>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("kwin-call")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| io::Error::other("the call to KDE could not be started"))
}

/// Run `fut` to its end on [`runtime`] and wait for it here, at most
/// [`WHOLE_WAIT`].
fn on_a_thread<T: Send + 'static>(
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> io::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    runtime()?.spawn(async move {
        let _ = tx.send(fut.await);
    });
    rx.recv_timeout(WHOLE_WAIT)
        .map_err(|_| io::Error::other("KDE did not answer"))
}

async fn connection(state: &mut State) -> Result<Connection, Failure> {
    if let Some(conn) = &state.conn {
        return Ok(conn.clone());
    }
    let built = tokio::time::timeout(CALL_WAIT, async {
        zbus::connection::Builder::session()?
            .serve_at(OBJECT_PATH, Sink)?
            .build()
            .await
    })
    .await
    .map_err(|_| Failure::Say("the session bus did not answer"))?
    .map_err(|_| Failure::Say("no session bus"))?;
    state.conn = Some(built.clone());
    Ok(built)
}

/// One call to a bus service, its error and its wait both folded to `None`.
async fn call<B>(
    conn: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> Option<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    tokio::time::timeout(
        CALL_WAIT,
        conn.call_method(Some(destination), path, Some(interface), method, body),
    )
    .await
    .ok()?
    .ok()
}

/// Who `org.kde.KWin` is on the bus.
async fn kwin_owner(conn: &Connection) -> Result<String, Failure> {
    let reply = call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        &(KWIN,),
    )
    .await
    .ok_or(Failure::Refused(Refusal::ScriptRefused))?;
    reply
        .body()
        .deserialize::<String>()
        .map_err(|_| Failure::Refused(Refusal::ScriptRefused))
}

/// Make the script report now, through the global shortcut it registered:
/// whether the call was taken.
async fn poke(conn: &Connection) -> bool {
    // Newer KDE takes an activation token as a second argument; older takes
    // the name alone.
    for with_token in [true, false] {
        let reply = if with_token {
            call(
                conn,
                "org.kde.kglobalaccel",
                "/component/kwin",
                "org.kde.kglobalaccel.Component",
                "invokeShortcut",
                &(POKE_SHORTCUT, ""),
            )
            .await
        } else {
            call(
                conn,
                "org.kde.kglobalaccel",
                "/component/kwin",
                "org.kde.kglobalaccel.Component",
                "invokeShortcut",
                &(POKE_SHORTCUT,),
            )
            .await
        };
        if reply.is_some() {
            return true;
        }
    }
    false
}

/// Where the script file is written: the user's runtime folder, in a folder
/// of ours, owner-only.
fn script_path() -> Result<PathBuf, Failure> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .ok_or(Failure::Say(
            "there is no runtime folder to put the KWin script in",
        ))?;
    let dir = runtime.join("pocketdesk");
    pocketdesk_core::secure_fs::create_private_dir(&dir)
        .map_err(|_| Failure::Say("the KWin script could not be written"))?;
    Ok(dir.join("kde-windows.js"))
}

/// Wait for the table to be set by a report.
async fn first_report(updates: &mut tokio::sync::watch::Receiver<u64>) -> bool {
    loop {
        if window_kde::snapshot().is_some() {
            return true;
        }
        if updates.changed().await.is_err() {
            return false;
        }
    }
}

/// Unload the script (whether or not it was loaded) and forget its table.
async fn unload(state: &mut State) {
    if let Some(conn) = &state.conn {
        let _ = call(
            conn,
            KWIN,
            SCRIPTING,
            SCRIPTING_INTERFACE,
            "unloadScript",
            &(PLUGIN,),
        )
        .await;
    }
    // KDE's global-shortcut service may keep the shortcut the script
    // registered (it shows in System Settings, Shortcuts, KWin) after the
    // script is gone: it is asked to forget it, best effort.
    if let Some(conn) = &state.conn {
        let _ = call(
            conn,
            "org.kde.kglobalaccel",
            "/kglobalaccel",
            "org.kde.KGlobalAccel",
            "unregister",
            &("kwin", POKE_SHORTCUT),
        )
        .await;
    }
    state.loaded = false;
    state.poke_works = false;
    window_kde::set_poker(None);
    window_kde::clear();
    if let Ok(path) = script_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Load the script and wait for its first report.
async fn load(state: &mut State) -> Result<(), Failure> {
    let conn = connection(state).await?;
    let dest = conn
        .unique_name()
        .map(ToString::to_string)
        .ok_or(Failure::Say("the session bus gave no name"))?;
    let owner = kwin_owner(&conn).await?;
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = owner;
    // Where a lent window's kept screen-sharing grant is written (the portal
    // capture reads it): the state folder, owner-only, never the repo's.
    window_kde::set_grant_dir(
        crate::state::state_dir()
            .ok()
            .map(|dir| dir.join("kde_window_grants")),
    );
    window_kde::clear();
    let path = script_path()?;
    let text = script::source(&dest).ok_or(Failure::Say("the KWin script could not be made"))?;
    pocketdesk_core::secure_fs::write_private(&path, text.as_bytes())
        .map_err(|_| Failure::Say("the KWin script could not be written"))?;
    let path_text = path
        .to_str()
        .ok_or(Failure::Say("the KWin script could not be written"))?
        .to_string();
    // A script left by a server that was killed is replaced, not doubled.
    let _ = call(
        &conn,
        KWIN,
        SCRIPTING,
        SCRIPTING_INTERFACE,
        "unloadScript",
        &(PLUGIN,),
    )
    .await;
    let mut updates = window_kde::updates();
    let loaded = call(
        &conn,
        KWIN,
        SCRIPTING,
        SCRIPTING_INTERFACE,
        "loadScript",
        &(path_text.as_str(), PLUGIN),
    )
    .await
    .and_then(|reply| reply.body().deserialize::<i32>().ok())
    .filter(|id| *id >= 0);
    let Some(id) = loaded else {
        let _ = std::fs::remove_file(&path);
        return Err(Failure::Refused(Refusal::ScriptRefused));
    };
    let run = call(
        &conn,
        KWIN,
        &format!("{SCRIPTING}/Script{id}"),
        SCRIPT_INTERFACE,
        "run",
        &(),
    )
    .await;
    if run.is_none() {
        // Another KWin names the script's object otherwise: start every
        // script that is loaded and not yet running.
        let started = call(&conn, KWIN, SCRIPTING, SCRIPTING_INTERFACE, "start", &()).await;
        if started.is_none() {
            unload(state).await;
            return Err(Failure::Refused(Refusal::ScriptRefused));
        }
    }
    let reported = tokio::time::timeout(FIRST_REPORT_WAIT, first_report(&mut updates))
        .await
        .unwrap_or(false);
    let _ = std::fs::remove_file(&path);
    if !reported {
        unload(state).await;
        return Err(Failure::Refused(Refusal::ScriptSilent));
    }
    // Whether the shortcut that asks for a report works: a second report must
    // follow the trigger.
    let mut after = window_kde::updates();
    after.mark_unchanged();
    state.poke_works = poke(&conn).await
        && tokio::time::timeout(POKE_WAIT, after.changed())
            .await
            .is_ok_and(|changed| changed.is_ok());
    if !state.poke_works {
        tracing::warn!(
            "KWin's script does not report when asked: the window table follows KWin's own changes only"
        );
    }
    let for_raise = conn.clone();
    window_kde::set_poker(Some(Arc::new(move || {
        let conn = for_raise.clone();
        if let Ok(runtime) = runtime() {
            runtime.spawn(async move {
                let _ = poke(&conn).await;
            });
        }
    })));
    state.loaded = true;
    Ok(())
}

/// The window table as it is now: the script loaded if need be, asked to
/// report, and the newest table believed (see the module's notes).
async fn table_now() -> Result<Arc<Snapshot>, Failure> {
    let mut state = STATE.lock().await;
    state.last_ask = Some(Instant::now());
    if state.loaded {
        if state.poke_works
            && let Some(conn) = state.conn.clone()
        {
            let mut updates = window_kde::updates();
            updates.mark_unchanged();
            if poke(&conn).await {
                let _ = tokio::time::timeout(POKE_WAIT, updates.changed()).await;
            }
        }
    } else {
        load(&mut state).await?;
    }
    let now = Instant::now();
    let table = match window_kde::snapshot() {
        Some(table) if now.saturating_duration_since(table.at) < DEAD_AFTER => table,
        _ => {
            // No report for a minute: the script is taken as gone.
            unload(&mut state).await;
            load(&mut state).await?;
            window_kde::snapshot().ok_or(Failure::Refused(Refusal::TableStale))?
        }
    };
    drop(state);
    prune_grants(&table);
    if LENT.load(Ordering::Relaxed) == 0 {
        start_reaper();
    }
    Ok(table)
}

/// Forget the kept grants of windows KWin no longer has, at most every half
/// minute. A table with no window in it (KWin still starting) forgets none.
fn prune_grants(table: &Snapshot) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let Some(dir) = window_kde::grant_dir() else {
        return;
    };
    if table.windows.is_empty() {
        return;
    }
    let keep: std::collections::HashSet<String> = table
        .windows
        .iter()
        .filter_map(|w| window_kde::grant_name(&w.uuid))
        .collect();
    pocketdesk_core::portal_grant::prune_named(&dir, |name| keep.contains(name));
}

/// Unload a script nobody has lent from once it has been idle for a minute.
fn start_reaper() {
    if REAPER.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(runtime) = runtime() else {
        REAPER.store(false, Ordering::SeqCst);
        return;
    };
    runtime.spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let mut state = STATE.lock().await;
            if !state.loaded {
                break;
            }
            let idle = state.last_ask.is_none_or(|at| at.elapsed() >= IDLE_UNLOAD);
            if LENT.load(Ordering::Relaxed) == 0 && idle {
                unload(&mut state).await;
                break;
            }
        }
        REAPER.store(false, Ordering::SeqCst);
    });
}

/// The source types the screen-sharing portal offers.
async fn portal_sources() -> Portal {
    let conn = {
        let mut state = STATE.lock().await;
        match connection(&mut state).await {
            Ok(conn) => conn,
            Err(_) => return Portal::Missing,
        }
    };
    let Ok(proxy) = zbus::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast",
    )
    .await
    else {
        return Portal::Missing;
    };
    match tokio::time::timeout(CALL_WAIT, proxy.get_property::<u32>("AvailableSourceTypes")).await {
        Ok(Ok(bits)) => Portal::Sources(bits),
        _ => Portal::Missing,
    }
}

/// KDE Plasma's session and bus, as the real [`Kwin`].
pub struct Bus;

impl Kwin for Bus {
    fn session(&self) -> Session {
        Session {
            kde: window_kde::is_kde(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()),
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        }
    }

    fn portal(&self) -> Portal {
        on_a_thread(portal_sources()).unwrap_or(Portal::Missing)
    }

    fn table(&self) -> io::Result<Arc<Snapshot>> {
        on_a_thread(table_now())?.map_err(|failure| io::Error::other(failure.said()))
    }

    fn stop(&self) {
        // The server is stopping: the script goes whether or not a window is
        // lent (a list alone loads it), if this server ever reached KWin. The
        // wait is a moment, never the server's stop held up.
        LENT.store(0, Ordering::Relaxed);
        let Ok(runtime) = runtime() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            if let Ok(mut state) =
                tokio::time::timeout(Duration::from_millis(1500), STATE.lock()).await
            {
                unload(&mut state).await;
            }
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }

    fn lent(&self, count: usize) {
        LENT.store(count, Ordering::Relaxed);
        if count > 0 {
            return;
        }
        // Nothing is lent: the script goes, without holding the caller (it
        // is called with the lend's record held) for more than a moment.
        let Ok(runtime) = runtime() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.spawn(async move {
            let mut state = STATE.lock().await;
            if LENT.load(Ordering::Relaxed) == 0 && state.loaded {
                unload(&mut state).await;
            }
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_millis(500));
    }
}
