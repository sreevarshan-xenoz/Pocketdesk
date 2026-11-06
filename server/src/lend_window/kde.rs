//! KDE Plasma as the source of windows to lend (phase 6 follow-up of the
//! window handoff): the rules, the words and the flow, over a [`Kwin`] that is
//! the real one on a KDE session (`kde_shell`) and a fake in the tests.
//!
//! **What a lend is.** As on GNOME ([`super::place`]): a capture in place.
//! Nothing moves; the window is registered under an index past every
//! monitor's, under the name `kde:<n>` (`window_target`'s table, the number
//! being the one the window table gave the window's uuid, see
//! `window_kde`), and the borrower's session captures it alone, through the
//! screen-sharing portal ([`pocketdesk_core::streaming::capture_kde`]):
//! **one dialog on the owner's screen for each window the first time**, and
//! none after that (the grant is kept for the window). The viewer cannot tell
//! the difference except that the first picture of a new window waits for
//! someone at the computer to choose it.
//!
//! **It is to look at only for now.** The guard over KWin's table is built
//! (`window_kde::KdeOs`) but a lent compositor window sends no input yet
//! (`window_target::takes_input`): the portal's RemoteDesktop has not been
//! shown to put a pointer in one window and nowhere else.
//!
//! **Where the windows come from.** A resident KWin script ([`script`]) is
//! loaded when a window is listed or lent and unloaded when none is lent (and
//! a minute after a list nobody lent from); it reports the window table to
//! this program over D-Bus on each change and when asked. Which windows may be
//! offered: ordinary windows of programs (not panels, the desktop, menus,
//! notifications or dialogs), shown on the desktop in view, not this
//! program's own ([`super::place::keep_listed`]). A window put away or on
//! another desktop stays lent; one closed ends the lend.
//!
//! **Behind the same switch** as every lend (`lend_windows`); nothing new is
//! on by default. A KDE computer that cannot lend says why in the owner's
//! words ([`cannot_because`]).

use std::io;
use std::sync::Arc;

use pocketdesk_core::streaming::window_kde::{self, Refusal, Snapshot};

use super::place::{Shell, ShellWindow};

/// What the owner reads when this is not KDE Plasma (it is only asked where
/// the lend picked KDE, from `XDG_CURRENT_DESKTOP`).
pub const NOT_KDE: &str = "This computer is not running KDE Plasma";

/// The name the script is loaded under, so a script left from a server that
/// was killed is found and replaced.
pub const PLUGIN: &str = "pocketdesk-windows";

/// The global shortcut the script registers, which the server triggers (over
/// KDE's global-shortcut service) to make the script report at once.
pub const POKE_SHORTCUT: &str = "PocketdeskPoke";

/// Where the script's reports are received, and what they are called.
pub const OBJECT_PATH: &str = "/dev/pocketdesk/Windows";
pub const INTERFACE: &str = "dev.pocketdesk.Windows";

/// The portal's `AvailableSourceTypes` bit for a single window.
const WINDOW_SOURCE: u32 = 2;

/// What the session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub kde: bool,
    pub wayland: bool,
}

/// What the screen-sharing portal answered about its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Portal {
    /// It could not be asked (not running, or no ScreenCast).
    Missing,
    /// The source types it offers (a bit field: 1 monitors, 2 windows).
    Sources(u32),
}

/// Why no window can be lent from this computer, if one cannot.
pub fn cannot_because(session: Session, portal: Portal) -> Option<&'static str> {
    if !session.kde {
        return Some(NOT_KDE);
    }
    if !session.wayland {
        return Some(window_kde::say(Refusal::NotWayland));
    }
    match portal {
        Portal::Missing => Some(window_kde::say(Refusal::NoPortal)),
        Portal::Sources(bits) if bits & WINDOW_SOURCE == 0 => {
            Some(window_kde::say(Refusal::PortalNoWindows))
        }
        Portal::Sources(_) => None,
    }
}

/// What lending a window of KDE asks of the computer.
pub trait Kwin: Sync {
    fn session(&self) -> Session;
    fn portal(&self) -> Portal;
    /// The window table as it is now: the script is loaded if need be and
    /// asked to report; `Err` says why there is none, in the owner's words.
    fn table(&self) -> io::Result<Arc<Snapshot>>;
    /// How many windows are lent now (returns at once).
    fn lent(&self, count: usize);
    /// The server is stopping: unload the script whether or not a window is
    /// lent (a list alone loads it), without waiting more than a moment.
    fn stop(&self);
}

/// The table's windows as the lend reads them: an ordinary window of a
/// program, shown and not minimised, is listed; anything else is "hidden" (a
/// panel, the desktop, a menu, a dialog: never offered; a window put away or
/// on another desktop: not offered, but not lost to a lend it is in).
pub fn shell_windows(table: &Snapshot) -> Vec<ShellWindow> {
    let mut windows: Vec<ShellWindow> = table
        .windows
        .iter()
        .map(|w| ShellWindow {
            id: w.seq,
            title: w.caption.clone(),
            app: w.class.clone(),
            width: w.frame.2,
            height: w.frame.3,
            hidden: !w.normal || !w.shown || w.minimized,
        })
        .collect();
    windows.sort_by_key(|w| w.id);
    windows
}

/// KDE Plasma, as this computer's source of windows.
pub struct Kde<B>(pub B);

impl<B: Kwin> Shell for Kde<B> {
    fn cannot(&self) -> Option<&'static str> {
        cannot_because(self.0.session(), self.0.portal())
    }

    fn windows(&self) -> io::Result<Vec<ShellWindow>> {
        let table = self.0.table()?;
        Ok(shell_windows(&table))
    }

    fn native_of(&self, id: u64) -> String {
        window_kde::kde_native(id)
    }

    fn id_of(&self, native: &str) -> Option<u64> {
        window_kde::kde_seq(native)
    }

    fn screen_name(&self, index: u32) -> String {
        format!("KDE-{index}")
    }

    fn lent(&self, count: usize) {
        self.0.lent(count);
    }

    fn stop(&self) {
        self.0.stop();
    }
}

pub mod script {
    //! The KWin script, as text. Our own; it reads the windows and tells this
    //! program, and activates the one window it is told to. Everything it
    //! sends is checked on arrival (`window_kde::parse_message`).

    use super::{INTERFACE, OBJECT_PATH, POKE_SHORTCUT};

    const SOURCE: &str = r#"// Pocketdesk: the window table of this desktop, for lending a window.
// Written by the Pocketdesk server and loaded into KWin only while a window is
// lent. It reads the windows and tells the server over D-Bus on each change and
// when asked; the server's reply may name a window to bring to the front. It
// does nothing else.
(function () {
    "use strict";
    var DEST = "@DEST@";
    var PATH = "@PATH@";
    var IFACE = "@IFACE@";
    var last = "";

    function bare(id) {
        return String(id).replace(/[{}]/g, "").toLowerCase();
    }

    function all() {
        return workspace.windowList ? workspace.windowList() : workspace.clientList();
    }

    // Whether the window is on the desktop in view. When this cannot be told
    // the window counts as shown, which is the side that stops a click.
    function shown(w) {
        if (w.minimized) {
            return false;
        }
        try {
            if (w.onAllDesktops) {
                return true;
            }
            var current = workspace.currentDesktop;
            var desktops = w.desktops;
            if (desktops === undefined || current === undefined) {
                return true;
            }
            if (desktops.length === 0) {
                return true;
            }
            for (var i = 0; i < desktops.length; i++) {
                if (desktops[i] === current || String(desktops[i].id) === String(current.id)) {
                    return true;
                }
            }
            return false;
        } catch (e) {
            return true;
        }
    }

    // The scale of the output the window is on (0 when it cannot be told, which
    // the server reads as not known and refuses a picture for).
    function scaleOf(w) {
        try {
            var s = w.output ? Number(w.output.scale) : 0;
            return s > 0 ? s : 0;
        } catch (e) {
            return 0;
        }
    }

    function row(w) {
        var g = w.frameGeometry;
        var c = w.clientGeometry || g;
        var parent = "";
        try {
            if (w.transientFor) {
                parent = bare(w.transientFor.internalId);
            }
        } catch (e) {
            parent = "";
        }
        return {
            id: bare(w.internalId),
            caption: String(w.caption || ""),
            "class": String(w.resourceClass || ""),
            pid: w.pid | 0,
            x: Math.round(g.x),
            y: Math.round(g.y),
            w: Math.round(g.width),
            h: Math.round(g.height),
            cw: Math.round(c.width),
            ch: Math.round(c.height),
            scale: scaleOf(w),
            min: !!w.minimized,
            shown: shown(w),
            normal: !!w.normalWindow && !w.skipTaskbar,
            parent: parent
        };
    }

    // A window that cannot be read stops the whole report (the server then
    // takes its table as old), never leaves a window out.
    function dump(force) {
        var rows = [];
        var windows = all();
        for (var i = 0; i < windows.length; i++) {
            if (windows[i] && !windows[i].deleted) {
                rows.push(row(windows[i]));
            }
        }
        var stack = [];
        var order = workspace.stackingOrder;
        if (order) {
            for (var j = 0; j < order.length; j++) {
                stack.push(bare(order[j].internalId));
            }
        }
        var active = workspace.activeWindow;
        var text = JSON.stringify({
            v: 1,
            windows: rows,
            stack: stack,
            active: active ? bare(active.internalId) : ""
        });
        if (!force && text === last) {
            return;
        }
        last = text;
        callDBus(DEST, PATH, IFACE, "Update", text, function (ids) {
            raise(ids);
        });
    }

    function raise(ids) {
        if (!ids || !ids.length) {
            return;
        }
        var windows = all();
        for (var k = 0; k < ids.length; k++) {
            for (var i = 0; i < windows.length; i++) {
                if (bare(windows[i].internalId) === ids[k]) {
                    if (windows[i].minimized) {
                        windows[i].minimized = false;
                    }
                    workspace.activeWindow = windows[i];
                }
            }
        }
    }

    function push() {
        dump(false);
    }

    function watch(w) {
        var signals = ["frameGeometryChanged", "minimizedChanged", "captionChanged",
                       "desktopsChanged", "transientChanged", "windowClassChanged"];
        for (var i = 0; i < signals.length; i++) {
            if (w[signals[i]] && w[signals[i]].connect) {
                w[signals[i]].connect(push);
            }
        }
    }

    var present = all();
    for (var n = 0; n < present.length; n++) {
        watch(present[n]);
    }
    workspace.windowAdded.connect(function (w) {
        watch(w);
        push();
    });
    workspace.windowRemoved.connect(push);
    workspace.windowActivated.connect(push);
    if (workspace.stackingOrderChanged) {
        workspace.stackingOrderChanged.connect(push);
    }
    if (workspace.currentDesktopChanged) {
        workspace.currentDesktopChanged.connect(push);
    }
    registerShortcut("@POKE@", "Pocketdesk: report the windows", "", function () {
        dump(true);
    });
    dump(true);
})();
"#;

    /// Whether `dest` is a D-Bus connection's unique name (`:1.42`): the only
    /// kind of text put in the script as where to report.
    pub fn is_unique_name(dest: &str) -> bool {
        dest.len() <= 64
            && dest.starts_with(':')
            && dest.len() > 1
            && dest
                .bytes()
                .skip(1)
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    }

    /// The script that reports to the connection `dest`, or `None` when
    /// `dest` is not a unique name.
    pub fn source(dest: &str) -> Option<String> {
        if !is_unique_name(dest) {
            return None;
        }
        Some(
            SOURCE
                .replace("@DEST@", dest)
                .replace("@PATH@", OBJECT_PATH)
                .replace("@IFACE@", INTERFACE)
                .replace("@POKE@", POKE_SHORTCUT),
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_a_unique_bus_name_is_put_in_the_script() {
            for good in [":1.42", ":1.7", ":1.1234"] {
                assert!(is_unique_name(good), "{good}");
            }
            let long = format!(":{}", "1".repeat(80));
            for bad in [
                "",
                ":",
                "org.kde.KWin",
                "1.42",
                ":1.4\"; evil(); //",
                ":1.4 5",
                ":1.4\n",
                ":1/4",
                long.as_str(),
            ] {
                assert!(!is_unique_name(bad), "{bad:?}");
                assert_eq!(source(bad), None, "{bad:?}");
            }
        }

        #[test]
        fn the_script_names_where_it_reports_and_leaves_no_placeholder() {
            let text = source(":1.42").unwrap();
            assert!(text.contains("var DEST = \":1.42\";"));
            assert!(text.contains(&format!("var PATH = \"{OBJECT_PATH}\";")));
            assert!(text.contains(&format!("var IFACE = \"{INTERFACE}\";")));
            assert!(text.contains(&format!("registerShortcut(\"{POKE_SHORTCUT}\"")));
            assert!(!text.contains('@'), "a placeholder is left");
            // The report is the one the server reads, and it asks for the
            // server's reply to name windows to raise.
            assert!(text.contains("callDBus(DEST, PATH, IFACE, \"Update\", text"));
            for field in [
                "v: 1", "windows:", "stack:", "active:", "normal:", "shown:", "parent:", "cw:",
                "scale:",
            ] {
                assert!(text.contains(field), "{field}");
            }
        }

        #[test]
        fn the_script_is_balanced_and_does_nothing_but_read_report_and_activate() {
            let text = source(":1.42").unwrap();
            for (open, close) in [('{', '}'), ('(', ')'), ('[', ']')] {
                assert_eq!(
                    text.matches(open).count(),
                    text.matches(close).count(),
                    "{open}{close}"
                );
            }
            // It never runs a program, reads a file or reaches the network;
            // it reports to one bus name and activates windows.
            for forbidden in [
                "XMLHttpRequest",
                "Qt.",
                "exec",
                "readConfig",
                "eval(",
                "require(",
            ] {
                assert!(!text.contains(forbidden), "{forbidden}");
            }
            assert_eq!(text.matches("callDBus(").count(), 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lend_window::place::PlaceEnv;
    use crate::lend_window::{Lends, lend_with, list_with, return_with};
    use pocketdesk_core::streaming::window_kde::KdeWindow;
    use pocketdesk_core::streaming::window_target;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    const DEVICE: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    /// A KDE computer that has the windows it is told of.
    struct Fake {
        session: Session,
        portal: Portal,
        windows: Mutex<Vec<KdeWindow>>,
        /// Why the table cannot be had.
        silent: Mutex<Option<&'static str>>,
        /// The count told last, and how many times it said none.
        last_lent: AtomicUsize,
        unloaded: AtomicUsize,
        /// How many times the server's stop was told.
        stopped: AtomicUsize,
    }

    impl Fake {
        fn new(windows: Vec<KdeWindow>) -> Self {
            Self {
                session: Session {
                    kde: true,
                    wayland: true,
                },
                portal: Portal::Sources(3),
                windows: Mutex::new(windows),
                silent: Mutex::new(None),
                last_lent: AtomicUsize::new(99),
                unloaded: AtomicUsize::new(0),
                stopped: AtomicUsize::new(0),
            }
        }
    }

    impl Kwin for &Fake {
        fn session(&self) -> Session {
            self.session
        }
        fn portal(&self) -> Portal {
            self.portal
        }
        fn table(&self) -> io::Result<Arc<Snapshot>> {
            if let Some(why) = *self.silent.lock().unwrap() {
                return Err(io::Error::other(why));
            }
            Ok(Arc::new(Snapshot {
                windows: self.windows.lock().unwrap().clone(),
                active: None,
                at: Instant::now(),
            }))
        }
        fn lent(&self, count: usize) {
            self.last_lent.store(count, Ordering::Relaxed);
            if count == 0 {
                self.unloaded.fetch_add(1, Ordering::Relaxed);
            }
        }
        fn stop(&self) {
            self.stopped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn kde_window(seq: u64, class: &str, size: (u32, u32)) -> KdeWindow {
        KdeWindow {
            seq,
            uuid: format!("00000000-0000-4000-8000-{seq:012x}"),
            caption: format!("title {seq}"),
            class: class.to_string(),
            pid: 100 + seq as u32,
            frame: (10, 10, size.0, size.1),
            client: size,
            scale_milli: Some(1000),
            minimized: false,
            shown: true,
            normal: true,
            parent: None,
            z: Some(seq as usize),
        }
    }

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        super::super::table_turn()
    }

    fn setup(fake: &Fake) -> (Lends, PlaceEnv<Kde<&Fake>>) {
        (Lends::new(), PlaceEnv::new(Kde(fake)))
    }

    fn index_of(reply: &serde_json::Value) -> u32 {
        let index = reply["index"].as_u64().expect("an index") as u32;
        assert!(window_target::is_window_index(index), "{reply}");
        index
    }

    // ---- The words ----

    #[test]
    fn a_computer_that_cannot_lend_says_why_in_the_owners_words() {
        let ok = Session {
            kde: true,
            wayland: true,
        };
        assert_eq!(cannot_because(ok, Portal::Sources(2)), None);
        assert_eq!(cannot_because(ok, Portal::Sources(3)), None);
        assert_eq!(
            cannot_because(
                Session {
                    kde: false,
                    wayland: true
                },
                Portal::Sources(3)
            ),
            Some(NOT_KDE)
        );
        let x11 = cannot_because(
            Session {
                kde: true,
                wayland: false,
            },
            Portal::Sources(3),
        )
        .unwrap();
        assert!(x11.contains("Wayland"), "{x11}");
        let missing = cannot_because(ok, Portal::Missing).unwrap();
        assert!(missing.contains("screen-sharing portal"), "{missing}");
        let monitors = cannot_because(ok, Portal::Sources(1)).unwrap();
        assert!(monitors.contains("single window"), "{monitors}");
        assert!(cannot_because(ok, Portal::Sources(0)).is_some());
        // The script's refusals are said in the owner's terms as well.
        assert!(window_kde::say(Refusal::ScriptRefused).contains("KWin Scripts"));
    }

    // ---- The list ----

    #[test]
    fn only_an_ordinary_shown_window_is_listed_and_a_put_away_one_is_hidden() {
        let mut panel = kde_window(1, "plasmashell", (1920, 40));
        panel.normal = false;
        let mut away = kde_window(2, "app", (800, 600));
        away.minimized = true;
        let mut elsewhere = kde_window(3, "app", (800, 600));
        elsewhere.shown = false;
        let own = kde_window(4, "dev.pocketdesk.Pocketdesk", (800, 600));
        let ok = kde_window(5, "org.kde.dolphin", (800, 600));
        let table = Snapshot {
            windows: vec![ok.clone(), own, elsewhere, away, panel],
            active: None,
            at: Instant::now(),
        };
        let windows = shell_windows(&table);
        assert_eq!(
            windows.iter().map(|w| w.id).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5],
            "in number order"
        );
        let hidden: Vec<_> = windows.iter().map(|w| w.hidden).collect();
        assert_eq!(hidden, [true, true, true, false, false]);
        let listed: Vec<u64> = windows
            .iter()
            .filter(|w| crate::lend_window::place::keep_listed(w))
            .map(|w| w.id)
            .collect();
        assert_eq!(listed, [5], "the program's own window is not offered");
        assert_eq!(windows[4].app, "org.kde.dolphin");
        assert_eq!(windows[4].title, "title 5");
        assert_eq!((windows[4].width, windows[4].height), (800, 600));
    }

    // ---- The flow ----

    #[test]
    fn a_window_is_lent_where_it_stands_under_a_kde_name_and_takes_no_input() {
        let _turn = serial();
        let fake = Fake::new(vec![kde_window(901, "org.kde.dolphin", (800, 600))]);
        let (lends, env) = setup(&fake);
        let reply = lend_with(&lends, &env, DEVICE, "901");
        assert!(reply.get("error").is_none(), "{reply}");
        let index = index_of(&reply);
        assert_eq!(reply["window"], "901");
        assert_eq!(reply["width"], 800);
        assert_eq!(reply["height"], 600);
        assert_eq!(reply["name"], format!("KDE-{index}").as_str());
        // The session that dials that index finds the window by its name.
        let lease = window_target::lease_for(index, Some(DEVICE)).unwrap();
        assert_eq!(lease.native, "kde:901");
        assert!(!window_target::takes_input(&lease), "view only for now");
        assert_eq!(
            fake.last_lent.load(Ordering::Relaxed),
            1,
            "the script is wanted"
        );
        // Another device is refused the index and the window.
        assert!(window_target::lease_for(index, Some(OTHER)).is_err());
        assert!(
            lend_with(&lends, &env, OTHER, "901")["error"]
                .as_str()
                .unwrap()
                .contains("Another computer")
        );
        // Back: the index is free again and the script is let go.
        let back = return_with(&lends, &env, DEVICE, Some("901"));
        assert_eq!(back["returned"], 1, "{back}");
        assert!(window_target::lease_for(index, Some(DEVICE)).is_err());
        assert_eq!(fake.last_lent.load(Ordering::Relaxed), 0);
        assert_eq!(fake.unloaded.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn only_an_ordinary_listed_window_is_lent_whatever_id_is_named() {
        let _turn = serial();
        let mut panel = kde_window(911, "plasmashell", (1920, 40));
        panel.normal = false;
        let mut away = kde_window(912, "app", (800, 600));
        away.minimized = true;
        let fake = Fake::new(vec![
            panel,
            away,
            kde_window(913, "dev.pocketdesk.Pocketdesk", (800, 600)),
        ]);
        let (lends, env) = setup(&fake);
        for id in ["911", "912", "913", "999"] {
            let reply = lend_with(&lends, &env, DEVICE, id);
            assert!(
                reply["error"].as_str().unwrap().contains("No such window"),
                "{id}: {reply}"
            );
        }
        // Ids that are no window numbers never reach the table.
        for bad in [
            "kde:911",
            "{00000000-0000-4000-8000-000000000001}",
            "0x10",
            "-1",
            "",
        ] {
            assert!(
                lend_with(&lends, &env, DEVICE, bad).get("error").is_some(),
                "{bad:?}"
            );
        }
        assert!(lends.all().is_empty());
        let listed = list_with(&lends, &env, DEVICE);
        assert_eq!(
            listed["windows"].as_array().map(Vec::len),
            Some(0),
            "{listed}"
        );
    }

    #[test]
    fn a_resize_says_the_size_the_window_has_and_a_closed_window_ends_the_lend() {
        let _turn = serial();
        let fake = Fake::new(vec![kde_window(921, "x", (800, 600))]);
        let (lends, env) = setup(&fake);
        let a = lend_with(&lends, &env, DEVICE, "921");
        let b = lend_with(&lends, &env, DEVICE, "921");
        assert_eq!(index_of(&a), index_of(&b), "asked again: the same screen");
        fake.windows.lock().unwrap()[0].frame.2 = 1000;
        fake.windows.lock().unwrap()[0].frame.3 = 700;
        let key = crate::lend_window::lend_key(DEVICE, "921");
        let sized = crate::lend_window::Env::resize_screen(&env, &key, 400, 300);
        assert_eq!(sized["width"], 1000, "{sized}");
        assert_eq!(sized["height"], 700);
        // The window is put away: still lent.
        fake.windows.lock().unwrap()[0].minimized = true;
        let seen = crate::lend_window::Env::seen(&env, "921").unwrap();
        assert!(seen.is_some(), "put away is not closed");
        // The window is closed: it is not there.
        fake.windows.lock().unwrap().clear();
        assert!(
            crate::lend_window::Env::seen(&env, "921")
                .unwrap()
                .is_none()
        );
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_script_that_cannot_report_lends_nothing_and_says_why() {
        let _turn = serial();
        let fake = Fake::new(vec![kde_window(931, "x", (800, 600))]);
        *fake.silent.lock().unwrap() = Some(window_kde::say(Refusal::ScriptRefused));
        let (lends, env) = setup(&fake);
        let reply = lend_with(&lends, &env, DEVICE, "931");
        assert!(
            reply["error"].as_str().unwrap().contains("KWin Scripts"),
            "{reply}"
        );
        let listed = list_with(&lends, &env, DEVICE);
        assert!(listed["error"].as_str().unwrap().contains("KWin Scripts"));
        assert!(lends.all().is_empty());
    }

    #[test]
    fn a_computer_that_cannot_lend_refuses_before_asking_for_windows() {
        let _turn = serial();
        let mut fake = Fake::new(vec![kde_window(941, "x", (800, 600))]);
        fake.portal = Portal::Missing;
        let (lends, env) = setup(&fake);
        let reply = lend_with(&lends, &env, DEVICE, "941");
        assert!(
            reply["error"]
                .as_str()
                .unwrap()
                .contains("screen-sharing portal"),
            "{reply}"
        );
        let listed = list_with(&lends, &env, DEVICE);
        assert!(
            listed["error"]
                .as_str()
                .unwrap()
                .contains("screen-sharing portal")
        );
    }

    #[test]
    fn the_server_stopping_is_told_to_the_shell_even_with_nothing_lent() {
        let fake = Fake::new(vec![kde_window(951, "x", (800, 600))]);
        let (_lends, env) = setup(&fake);
        // A list alone loads the script and lends nothing.
        assert!(
            crate::lend_window::Env::windows(&env).is_ok(),
            "the list is had"
        );
        assert_eq!(
            fake.last_lent.load(Ordering::Relaxed),
            99,
            "nothing was lent"
        );
        crate::lend_window::Env::stop(&env);
        assert_eq!(fake.stopped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_lend_keeps_its_names_apart_from_gnomes() {
        let fake = Fake::new(vec![]);
        let kde = Kde(&fake);
        assert_eq!(kde.native_of(7), "kde:7");
        assert_eq!(kde.id_of("kde:7"), Some(7));
        assert_eq!(
            kde.id_of("gnome:7"),
            None,
            "another desktop's name is not ours"
        );
        assert_eq!(kde.screen_name(1002), "KDE-1002");
    }
}
