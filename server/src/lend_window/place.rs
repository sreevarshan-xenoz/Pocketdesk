//! Lending a window of a computer whose compositor shows windows but cannot
//! be asked to move one (phase 6 of the window handoff: GNOME as the source).
//! The same `list`, `lend` and `return` as on Sway, Hyprland and Windows, the
//! same gate, and a viewer that cannot tell the difference; what differs is
//! what a lend is, and that it is to look at only.
//!
//! **A lend is a capture in place.** GNOME has no way for a program to move a
//! window to another screen (that is the Shell's own to do), so nothing moves
//! and no screen is made. The window is registered
//! ([`window_target::register_native`]) under an index past every monitor's,
//! which is the "monitor index" the answer carries and the borrower's session
//! dials, as on Windows; that session captures the window alone through
//! mutter's `RecordWindow` (core's `capture_mutter::start_window_capture`),
//! which records a window by id whatever is in front of it. The window is lent
//! at the size it has: the viewer scales the picture, and "resize follows the
//! viewer" has nothing to size.
//!
//! **It is to look at only.** The lend is a grant of one window to one device
//! and nothing a borrower sends may reach past that window; mutter's
//! RemoteDesktop would put the borrower's pointer in the window's rectangle
//! (a position outside it is clamped to its edge) but sends the click to
//! whichever window is topmost at that spot, and a key goes to the window that
//! has the keyboard, and no program can ask GNOME which window covers a spot
//! or which one has the keyboard (`org.gnome.Shell.Introspect` is refused to
//! anyone but GNOME's own apps). So a session on such a lend is view-only
//! (core's `window_target::takes_input`), by the rule the Windows guard
//! follows: when in doubt, nothing is sent.
//!
//! **What is lent, and to whom.** (1) Only a window the list shows (shown, not
//! this program's own) may be lent, whatever id is named; any other is "not
//! there". (2) The registration names the device it was lent to and a
//! generation no other registration has; a session is accepted only from that
//! device and holds a lease on exactly that registration, so a window given
//! back, closed or lent again ends the sessions of the old grant at once. (3)
//! A window id is never given to another window by mutter, so there is no
//! handle to be recycled. (4) Giving a window back ends the grant first.
//!
//! **What is kept.** Nothing: no window is moved, so there is nothing to put
//! back and nothing to write to `lent_windows.json`; a restart of the server
//! ends every lend with its registrations.
//!
//! All of it is written against [`Shell`], the two things asked of GNOME (why
//! no window can be lent, and the window list), so the rules and the lend's
//! whole flow run on any system over a fake; `gnome_shell` is the real one.

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use pocketdesk_core::streaming::window_target::{self, Lease, RegisterError};
use serde_json::{Value, json};

use super::{Env, Lent, Seen, error};
use crate::hyprland::{HyprWindow, HyprWorkspaceRef};
use crate::virtual_screen::{Output, Rect};

/// A window as the shell lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWindow {
    /// The id the compositor gives it, which a stream is made from.
    pub id: u64,
    pub title: String,
    /// Its app: the class it sets, else the application id the shell found.
    pub app: String,
    pub width: u32,
    pub height: u32,
    /// Minimised, or otherwise not shown.
    pub hidden: bool,
}

/// What lending a window asks of the compositor. [`super::gnome_shell::Gnome`]
/// is the real one; the tests have a fake.
pub trait Shell: Sync {
    /// Why no window can be lent here, if none can.
    fn cannot(&self) -> Option<&'static str>;
    /// Every window the compositor shows, or why it cannot say.
    fn windows(&self) -> io::Result<Vec<ShellWindow>>;
}

// ---- The rules ----

/// The window `id` names: plain decimal digits (one to twelve of them, no
/// leading zero, not zero), the shape of every window id the lend path takes;
/// nothing else is a window.
pub fn parse_window(id: &str) -> Option<u64> {
    if !(1..=12).contains(&id.len())
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.starts_with('0')
    {
        return None;
    }
    id.parse().ok()
}

/// The id a borrower is given for a window.
pub fn window_id(id: u64) -> String {
    id.to_string()
}

/// The device and the window of the key a lend is recorded under
/// (`<device>#lend-<window>`, [`super::lend_key`]).
fn split_key(key: &str) -> Option<(&str, u64)> {
    let (device, window) = key.rsplit_once("#lend-")?;
    Some((device, parse_window(window)?))
}

/// What the picture's own name says: a screen's, so the lend's answer and
/// record read as they do anywhere.
pub fn output_name(index: u32) -> String {
    format!("GNOME-{index}")
}

/// Whether `app` is this program's own: the app and the server never lend
/// their own windows (the viewing windows would show themselves).
pub fn is_own_program(app: &str) -> bool {
    app.to_ascii_lowercase().contains("pocketdesk")
}

/// Whether a window is one to list: shown, with a size, not this program's own
/// and with an id the lend path can take.
pub fn keep_listed(window: &ShellWindow) -> bool {
    !window.hidden
        && window.width > 0
        && window.height > 0
        && !is_own_program(&window.app)
        && parse_window(&window_id(window.id)).is_some()
}

/// A listed window as the lend's list reads it.
fn as_listed(window: &ShellWindow) -> HyprWindow {
    HyprWindow {
        address: window_id(window.id),
        title: window.title.clone(),
        class: window.app.clone(),
        pid: 0,
        mapped: true,
        hidden: false,
        at: [0, 0],
        size: [
            i32::try_from(window.width).unwrap_or(i32::MAX),
            i32::try_from(window.height).unwrap_or(i32::MAX),
        ],
        monitor: 0,
        fullscreen: 0,
        workspace: HyprWorkspaceRef {
            id: 1,
            name: "desktop".to_string(),
        },
    }
}

/// What a lend of a window that cannot be lent says, whatever the reason (it
/// is not there, it is not one that is listed): a borrower learns nothing
/// about windows it was not offered.
const NOT_THERE: &str = "No such window on this computer (it may have closed)";

// ---- The environment ----

#[derive(Default)]
struct State {
    /// The lends, by the key the lend path uses.
    screens: BTreeMap<String, Lease>,
}

/// Lending as GNOME does it: [`Env`] over a [`Shell`].
pub struct PlaceEnv<S> {
    shell: S,
    state: Mutex<State>,
    /// How many looks running the shell could not be read.
    unreadable: std::sync::atomic::AtomicU32,
}

/// How many looks running that find the shell unreadable make a lent window
/// count as gone (the sweep then ends it after its own two looks): a window
/// that closed while the list was unreadable must not stay lent for ever.
const UNREADABLE_LOOKS: u32 = 10;

impl<S: Shell> PlaceEnv<S> {
    pub fn new(shell: S) -> Self {
        Self {
            shell,
            state: Mutex::new(State::default()),
            unreadable: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The answer for a window lent: its screen's name and index and the size
    /// its picture has now.
    fn reply(lease: &Lease, window: &ShellWindow) -> Value {
        json!({
            "name": output_name(lease.index),
            "index": lease.index,
            "width": window.width,
            "height": window.height,
        })
    }
}

impl<S: Shell> Env for PlaceEnv<S> {
    fn cannot(&self) -> Option<&'static str> {
        self.shell.cannot()
    }

    fn seen(&self, window: &str) -> io::Result<Option<Seen>> {
        let id = parse_window(window).ok_or_else(|| super::other("not a window id"))?;
        // Any window the shell still shows, minimised or not: a lend is not
        // ended by its window being put away, only by its being closed.
        let all = match self.shell.windows() {
            Ok(all) => {
                self.unreadable
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                all
            }
            Err(e) => {
                let looks = self
                    .unreadable
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                // A shell that stays unreadable cannot say the window is
                // still there: after a while it is taken for gone.
                return if looks >= UNREADABLE_LOOKS {
                    Ok(None)
                } else {
                    Err(e)
                };
            }
        };
        Ok(all.into_iter().find(|w| w.id == id).map(|w| {
            let size = [
                i32::try_from(w.width).unwrap_or(i32::MAX),
                i32::try_from(w.height).unwrap_or(i32::MAX),
            ];
            Seen {
                id: window.to_string(),
                // Nothing moves, so there is nothing to keep of where it
                // was.
                floating: true,
                at: [0, 0],
                size,
                origin: [0, 0],
                frame: size,
                output: String::new(),
                workspace: "desktop".to_string(),
                workspace_id: 1,
                border: String::new(),
                border_width: 0,
                fullscreen: 0,
                fullscreen_client: 0,
            }
        }))
    }

    fn windows(&self) -> io::Result<Vec<HyprWindow>> {
        Ok(self
            .shell
            .windows()?
            .iter()
            .filter(|w| keep_listed(w))
            .map(as_listed)
            .collect())
    }

    fn add_screen(&self, key: &str, _width: u32, _height: u32) -> Value {
        let Some((device, id)) = split_key(key) else {
            return error("not a window");
        };
        // The shell is asked before the table is held: it may be slow.
        let all = match self.shell.windows() {
            Ok(all) => all,
            Err(e) => return error(format!("Could not read this computer's windows: {e}")),
        };
        let now = all.into_iter().find(|w| w.id == id);
        let mut state = self.state();
        if let Some(lease) = state.screens.get(key).cloned() {
            // Asked again: the same window, under a grant that stands (one
            // that was ended while the window would not go back is a new
            // grant for the same device, window and screen name).
            let Some(window) = now else {
                return error("The window closed");
            };
            if window_target::lease_holds(&lease) {
                return Self::reply(&lease, &window);
            }
            return match window_target::register_native(
                &lease.native,
                &lease.device,
                Some(lease.index),
            ) {
                Ok((lease, _)) => {
                    state.screens.insert(key.to_string(), lease.clone());
                    Self::reply(&lease, &window)
                }
                Err(RegisterError::Full) => error("Too many windows are lent at once"),
                Err(_) => error("Another computer has borrowed that window"),
            };
        }
        // Only a window the list shows may be lent, whatever id is named.
        let Some(window) = now.filter(keep_listed) else {
            return error(NOT_THERE);
        };
        let lease =
            match window_target::register_native(&window_target::gnome_native(id), device, None) {
                Ok((lease, _)) => lease,
                Err(RegisterError::Full) => return error("Too many windows are lent at once"),
                Err(RegisterError::Held) => {
                    return error("Another computer has borrowed that window");
                }
                Err(RegisterError::Nothing) => return error("not a window"),
            };
        state.screens.insert(key.to_string(), lease.clone());
        Self::reply(&lease, &window)
    }

    fn drop_screen(&self, key: &str) {
        if let Some(lease) = self.state().screens.remove(key) {
            window_target::unregister(&lease);
        }
    }

    fn resize_screen(&self, key: &str, _width: u32, _height: u32) -> Value {
        let Some(lease) = self.state().screens.get(key).cloned() else {
            return error("That window is not lent");
        };
        // A resize is no new grant: a lend that was ended stays ended until it
        // is asked for again. The window is not sized (nothing can size it
        // from here): the answer says the size it has, and the viewer scales.
        if !window_target::lease_holds(&lease) {
            return error("That window is not lent");
        }
        let Some(id) = window_target::gnome_id(&lease.native) else {
            return error("not a window");
        };
        match self.shell.windows() {
            Ok(all) => match all.into_iter().find(|w| w.id == id) {
                Some(window) => Self::reply(&lease, &window),
                None => error("The window closed"),
            },
            Err(e) => error(format!("Could not read this computer's windows: {e}")),
        }
    }

    fn output(&self, name: &str) -> Option<Output> {
        let lease = self
            .state()
            .screens
            .values()
            .find(|l| output_name(l.index) == name)
            .cloned()?;
        let id = window_target::gnome_id(&lease.native)?;
        let window = self
            .shell
            .windows()
            .ok()?
            .into_iter()
            .find(|w| w.id == id)?;
        Some(Output {
            name: name.to_string(),
            rect: Rect {
                x: 0,
                y: 0,
                width: i32::try_from(window.width).ok()?,
                height: i32::try_from(window.height).ok()?,
            },
        })
    }

    fn send(&self, _lent: &Lent, _to: &Output, _width: u32, _height: u32) -> io::Result<()> {
        // Nothing moves: the window is lent where it stands.
        Ok(())
    }

    fn follow(&self, _lent: &Lent, _to: &Output, _width: u32, _height: u32) -> io::Result<()> {
        Ok(())
    }

    fn back(&self, lent: &Lent) -> io::Result<()> {
        let id = parse_window(&lent.window).ok_or_else(super::no_place)?;
        let native = window_target::gnome_native(id);
        // The grant ends: the sessions bound to it stop at once. Nothing was
        // moved, so there is nothing else to put back.
        let state = self.state();
        for lease in state.screens.values().filter(|l| l.native == native) {
            window_target::unregister(lease);
        }
        Ok(())
    }

    fn save(&self, _lent: &[Lent]) -> io::Result<()> {
        // Nothing to put back after a restart (see the module's notes).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lend_window::{Gone, Lends, Looks, Present, lend_with, list_with, return_with};
    use std::time::Instant;

    const DEVICE: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    /// A compositor that shows the windows it is told of.
    struct Fake {
        windows: Mutex<Vec<ShellWindow>>,
        cannot: Option<&'static str>,
        /// The shell cannot say what windows there are.
        unreadable: Mutex<bool>,
    }

    impl Fake {
        fn new(windows: Vec<ShellWindow>) -> Self {
            Self {
                windows: Mutex::new(windows),
                cannot: None,
                unreadable: Mutex::new(false),
            }
        }

        fn close(&self, id: u64) {
            self.windows.lock().unwrap().retain(|w| w.id != id);
        }

        fn hide(&self, id: u64, hidden: bool) {
            for w in self.windows.lock().unwrap().iter_mut() {
                if w.id == id {
                    w.hidden = hidden;
                }
            }
        }

        fn resize(&self, id: u64, width: u32, height: u32) {
            for w in self.windows.lock().unwrap().iter_mut() {
                if w.id == id {
                    (w.width, w.height) = (width, height);
                }
            }
        }
    }

    impl Shell for Fake {
        fn cannot(&self) -> Option<&'static str> {
            self.cannot
        }

        fn windows(&self) -> io::Result<Vec<ShellWindow>> {
            if *self.unreadable.lock().unwrap() {
                return Err(io::Error::other("the shell did not answer"));
            }
            Ok(self.windows.lock().unwrap().clone())
        }
    }

    fn window(id: u64, app: &str, width: u32, height: u32) -> ShellWindow {
        ShellWindow {
            id,
            title: format!("title of {id}"),
            app: app.to_string(),
            width,
            height,
            hidden: false,
        }
    }

    /// The tests share the process-wide table of window indexes: the ones that
    /// lend take turns, so a test that fills the table does not starve another.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        super::super::table_turn()
    }

    fn setup(windows: Vec<ShellWindow>) -> (Lends, PlaceEnv<Fake>) {
        (Lends::new(), PlaceEnv::new(Fake::new(windows)))
    }

    fn lent_native(index: u32) -> Option<String> {
        window_target::lease_for(index, Some(DEVICE))
            .ok()
            .map(|l| l.native)
    }

    fn index_of(reply: &Value) -> u32 {
        let index = reply["index"].as_u64().expect("an index") as u32;
        assert!(window_target::is_window_index(index), "{reply}");
        index
    }

    // ---- The rules ----

    #[test]
    fn only_plain_decimal_digits_name_a_window() {
        assert_eq!(parse_window("2609479513"), Some(2609479513));
        assert_eq!(parse_window("1"), Some(1));
        assert_eq!(parse_window("999999999999"), Some(999_999_999_999));
        for bad in [
            "",
            "0",
            "01",
            "0x10",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1;2",
            "1000000000000",
            "gnome:1",
            "\u{661}",
            "1\n",
        ] {
            assert_eq!(parse_window(bad), None, "{bad:?}");
        }
        assert_eq!(window_id(2609479513), "2609479513");
    }

    #[test]
    fn the_key_of_a_lend_names_its_device_and_window() {
        assert_eq!(
            split_key(&super::super::lend_key(DEVICE, "2609479513")),
            Some((DEVICE, 2609479513))
        );
        assert_eq!(split_key("no-key"), None);
        assert_eq!(split_key(&format!("{DEVICE}#lend-0x10")), None);
        assert_eq!(split_key(&format!("{DEVICE}#lend-")), None);
    }

    #[test]
    fn the_screen_of_a_window_is_a_plain_name() {
        let name = output_name(1003);
        assert_eq!(name, "GNOME-1003");
        assert!(super::super::is_plain(&name));
    }

    #[test]
    fn a_window_is_listed_when_shown_sized_and_not_this_programs_own() {
        assert!(keep_listed(&window(5, "org.gnome.Nautilus", 800, 600)));
        for own in [
            "pocketdesk-desktop",
            "dev.pocketdesk.Pocketdesk",
            "PocketDesk",
        ] {
            assert!(!keep_listed(&window(5, own, 800, 600)), "{own}");
        }
        assert!(!keep_listed(&window(5, "x", 0, 600)));
        assert!(!keep_listed(&window(5, "x", 800, 0)));
        let mut hidden = window(5, "x", 800, 600);
        hidden.hidden = true;
        assert!(!keep_listed(&hidden));
        // An id the lend path could not take is not offered.
        assert!(!keep_listed(&window(0, "x", 800, 600)));
        assert!(!keep_listed(&window(u64::MAX, "x", 800, 600)));
    }

    // ---- The flow ----

    #[test]
    fn a_window_is_lent_where_it_stands_and_the_answer_names_its_capture_index() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479513, "org.gnome.Nautilus", 800, 600)]);
        let reply = lend_with(&lends, &env, DEVICE, "2609479513");
        assert!(reply.get("error").is_none(), "{reply}");
        let index = index_of(&reply);
        assert_eq!(reply["window"], "2609479513");
        assert_eq!(reply["width"], 800);
        assert_eq!(reply["height"], 600);
        assert_eq!(reply["name"], output_name(index).as_str());
        // The session that dials that index finds the window by its name.
        assert_eq!(lent_native(index).as_deref(), Some("gnome:2609479513"));
        // The session is view-only: this lease takes no input.
        let lease = window_target::lease_for(index, Some(DEVICE)).unwrap();
        assert!(!window_target::takes_input(&lease));
        // Back: the index is free again.
        let back = return_with(&lends, &env, DEVICE, Some("2609479513"));
        assert_eq!(back["returned"], 1, "{back}");
        assert_eq!(lent_native(index), None);
        assert!(env.state().screens.is_empty());
    }

    #[test]
    fn asked_again_the_same_screen_comes_back_and_a_resize_says_the_size_it_has() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479514, "x", 800, 600)]);
        let a = lend_with(&lends, &env, DEVICE, "2609479514");
        let b = lend_with(&lends, &env, DEVICE, "2609479514");
        assert_eq!(index_of(&a), index_of(&b));
        assert_eq!(b["width"], 800);
        // The window grew; the viewer's resize cannot size it, the answer is
        // the size it has.
        env.shell.resize(2609479514, 1000, 700);
        let key = super::super::lend_key(DEVICE, "2609479514");
        let sized = env.resize_screen(&key, 400, 300);
        assert_eq!(sized["width"], 1000, "{sized}");
        assert_eq!(sized["height"], 700);
        assert_eq!(index_of(&sized), index_of(&a));
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn another_computer_cannot_take_a_lent_window() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479515, "x", 800, 600)]);
        assert!(
            lend_with(&lends, &env, DEVICE, "2609479515")
                .get("error")
                .is_none()
        );
        let refused = lend_with(&lends, &env, OTHER, "2609479515");
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("Another computer"),
            "{refused}"
        );
        // And it cannot return what it did not borrow.
        let none = return_with(&lends, &env, OTHER, Some("2609479515"));
        assert_eq!(none["returned"], 0);
        // Nor take it by the side door of the table.
        let side = window_target::register_native("gnome:2609479515", OTHER, None);
        assert_eq!(side.unwrap_err(), RegisterError::Held);
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn only_a_window_the_list_shows_is_lent_whatever_id_is_named() {
        let _turn = serial();
        let mut hidden = window(2609479516, "x", 800, 600);
        hidden.hidden = true;
        let (lends, env) = setup(vec![
            hidden,
            window(2609479517, "pocketdesk-desktop", 800, 600),
            window(2609479518, "x", 0, 0),
        ]);
        for id in ["2609479516", "2609479517", "2609479599"] {
            let reply = lend_with(&lends, &env, DEVICE, id);
            assert!(
                reply["error"].as_str().unwrap().contains("No such window"),
                "{id}: {reply}"
            );
        }
        // A window with no size is no window to make a picture of.
        let flat = lend_with(&lends, &env, DEVICE, "2609479518");
        assert_eq!(flat["error"], "not a window size", "{flat}");
        assert!(env.state().screens.is_empty());
        assert!(lends.all().is_empty());
        // An id that is no window id never reaches the shell.
        for bad in ["0x10", "gnome:1", "-5", "", "abc"] {
            assert!(
                lend_with(&lends, &env, DEVICE, bad).get("error").is_some(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_shell_that_cannot_say_lends_nothing_and_says_why() {
        let _turn = serial();
        let mut fake = Fake::new(vec![window(2609479520, "x", 800, 600)]);
        fake.cannot = Some("the shell is not there");
        let env = PlaceEnv::new(fake);
        let lends = Lends::new();
        let reply = lend_with(&lends, &env, DEVICE, "2609479520");
        assert_eq!(reply["error"], "the shell is not there");
        let listed = list_with(&lends, &env, DEVICE);
        assert_eq!(listed["error"], "the shell is not there");
        // A shell that fails to answer is an error, not an empty list.
        let (lends, env) = setup(vec![window(2609479521, "x", 800, 600)]);
        *env.shell.unreadable.lock().unwrap() = true;
        assert!(
            list_with(&lends, &env, DEVICE)["error"]
                .as_str()
                .unwrap()
                .contains("Could not read")
        );
        assert!(
            lend_with(&lends, &env, DEVICE, "2609479521")
                .get("error")
                .is_some()
        );
        assert!(env.state().screens.is_empty());
    }

    #[test]
    fn the_list_names_other_programs_windows_only_with_what_is_lent_marked() {
        let _turn = serial();
        let (lends, env) = setup(vec![
            window(2609479530, "org.gnome.Nautilus", 800, 600),
            window(2609479531, "pocketdesk-desktop", 800, 600),
            window(2609479532, "org.gnome.TextEditor", 640, 480),
        ]);
        assert!(
            lend_with(&lends, &env, DEVICE, "2609479532")
                .get("error")
                .is_none()
        );
        let listed = list_with(&lends, &env, DEVICE);
        let names: Vec<&str> = listed["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["address"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["2609479530", "2609479532"], "{listed}");
        assert_eq!(listed["windows"][1]["lent"], true);
        assert_eq!(listed["windows"][0]["lent"], false);
        assert_eq!(listed["windows"][0]["class"], "org.gnome.Nautilus");
        assert_eq!(listed["windows"][0]["width"], 800);
        // Another computer sees the window lent to the first no more.
        let theirs = list_with(&lends, &env, OTHER);
        assert_eq!(theirs["windows"].as_array().unwrap().len(), 1, "{theirs}");
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn a_window_put_away_stays_lent_and_one_closed_is_ended_after_two_looks() {
        let _turn = serial();
        let (shown, closing) = (2609479540, 2609479541);
        let (lends, env) = setup(vec![
            window(shown, "x", 800, 600),
            window(closing, "x", 640, 480),
        ]);
        let ia = index_of(&lend_with(&lends, &env, DEVICE, &shown.to_string()));
        let ib = index_of(&lend_with(&lends, &env, DEVICE, &closing.to_string()));
        let live = vec![DEVICE.to_string()];
        let captures = vec![(DEVICE.to_string(), ia), (DEVICE.to_string(), ib)];
        let present = Present {
            live: &live,
            captures: &captures,
            linked: &|_| true,
        };
        let mut looks = Looks::default();
        let now = Instant::now();
        // Minimised is not closed: the lend goes on.
        env.shell.hide(shown, true);
        env.shell.close(closing);
        let first = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now);
        assert!(first.is_empty(), "one look is not enough");
        let second = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, now);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].window, closing.to_string());
        assert_eq!(second[0].why, Gone::Closed);
        assert_eq!(lent_native(ib), None, "its grant ended with it");
        assert_eq!(
            lent_native(ia).as_deref(),
            Some("gnome:2609479540"),
            "the window put away stays lent"
        );
        // The switch turned off gives the rest back.
        let off = crate::lend_window::sweep_with(&lends, &env, false, &present, &mut looks, now);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].why, Gone::SwitchOff);
        assert_eq!(lent_native(ia), None);
    }

    #[test]
    fn a_shell_that_stays_unreadable_ends_the_lend_in_the_end_and_a_short_gap_does_not() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479570, "x", 800, 600)]);
        let index = index_of(&lend_with(&lends, &env, DEVICE, "2609479570"));
        let live = vec![DEVICE.to_string()];
        let captures = vec![(DEVICE.to_string(), index)];
        let present = Present {
            live: &live,
            captures: &captures,
            linked: &|_| true,
        };
        let mut looks = Looks::default();
        let now = Instant::now();
        let sweep = |looks: &mut Looks| {
            crate::lend_window::sweep_with(&lends, &env, true, &present, looks, now)
        };
        // A gap shorter than the limit changes nothing, and a readable look
        // starts the count again.
        *env.shell.unreadable.lock().unwrap() = true;
        for _ in 0..UNREADABLE_LOOKS - 1 {
            assert!(sweep(&mut looks).is_empty());
        }
        *env.shell.unreadable.lock().unwrap() = false;
        assert!(sweep(&mut looks).is_empty());
        assert!(lent_native(index).is_some());
        // Unreadable for good: gone at the limit, ended at the look after.
        *env.shell.unreadable.lock().unwrap() = true;
        let mut ended = Vec::new();
        for _ in 0..UNREADABLE_LOOKS + 2 {
            ended.extend(sweep(&mut looks));
        }
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].why, Gone::Closed);
        assert_eq!(lent_native(index), None);
    }

    #[test]
    fn a_borrower_that_went_away_has_its_grant_ended_at_the_next_look() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479550, "x", 800, 600)]);
        let index = index_of(&lend_with(&lends, &env, DEVICE, "2609479550"));
        let present = Present {
            live: &[],
            captures: &[],
            linked: &|_| false,
        };
        let mut looks = Looks::default();
        let later = Instant::now() + std::time::Duration::from_secs(10);
        let ended = crate::lend_window::sweep_with(&lends, &env, true, &present, &mut looks, later);
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].why, Gone::BorrowerAway);
        assert_eq!(lent_native(index), None);
    }

    #[test]
    fn a_lend_asked_for_again_after_its_grant_was_ended_is_a_new_grant_for_the_same_screen() {
        let _turn = serial();
        let (lends, env) = setup(vec![window(2609479560, "x", 800, 600)]);
        let first = lend_with(&lends, &env, DEVICE, "2609479560");
        let index = index_of(&first);
        let old = window_target::lease_for(index, Some(DEVICE)).unwrap();
        // The grant is ended with the record kept (a return that went wrong).
        window_target::unregister(&old);
        assert!(!window_target::lease_alive(&old), "its sessions end");
        let again = lend_with(&lends, &env, DEVICE, "2609479560");
        assert_eq!(index_of(&again), index, "the screen keeps its name");
        let new = window_target::lease_for(index, Some(DEVICE)).unwrap();
        assert_ne!(new.generation, old.generation);
        assert!(window_target::lease_alive(&new));
        assert!(
            !window_target::lease_alive(&old),
            "the old grant holds nothing of the new one"
        );
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn more_windows_than_there_are_indexes_are_refused_and_leave_the_rest_alone() {
        let _turn = serial();
        let many: Vec<ShellWindow> = (0..=window_target::MOST as u64)
            .map(|n| window(2609479600 + n, "x", 800, 600))
            .collect();
        let (lends, env) = setup(many);
        let mut taken = Vec::new();
        for n in 0..window_target::MOST as u64 {
            let reply = lend_with(&lends, &env, DEVICE, &(2609479600 + n).to_string());
            taken.push(index_of(&reply));
        }
        let last = (2609479600 + window_target::MOST as u64).to_string();
        let refused = lend_with(&lends, &env, DEVICE, &last);
        assert!(
            refused["error"].as_str().unwrap().contains("Too many"),
            "{refused}"
        );
        assert!(
            lends.all().iter().all(|l| l.window != last),
            "no record of the one refused"
        );
        for index in &taken {
            assert!(lent_native(*index).is_some(), "{index} is still lent");
        }
        return_with(&lends, &env, DEVICE, None);
    }

    #[test]
    fn nothing_is_written_for_the_next_start() {
        let (_, env) = setup(Vec::new());
        assert!(env.save(&[]).is_ok());
        let lent = [];
        assert!(env.save(&lent).is_ok());
    }
}
