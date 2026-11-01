//! Phone apps tier 2 on the computer: each app in a window of its own.
//!
//! When a phone says it opens apps in windows of its own (its "Separate
//! windows (Shizuku)", off by default there) and this computer's "Open phone
//! apps in windows of their own" is on, opening an app asks for a window: the
//! phone's own screen is left alone, and the app's picture arrives as one
//! more stream of the phone's cast, shown by a [`CastView`] of its own per
//! window id (several at once, as many as the phone takes). Anything else
//! opens as before, the app on the phone's screen shown here.
//!
//! The phone decides everything that matters; this side only picks the id
//! of a free window, asks, and shows what comes.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;
use pocketdesk_core::local::cast as wire;
use pocketdesk_core::protocol::phone_action::{OpenMode, WindowAsk, WindowsMode, open_mode};
use pocketdesk_core::streaming::cast::MAX_WINDOWS;

use crate::cast_view::CastView;
use crate::model::Device;

/// An id asked for is held this long while the phone's picture has not
/// come, so a second open does not ask for the same window.
const RESERVE: Duration = Duration::from_secs(30);

/// How often the folder is looked at for new windows.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// How an open is made: the phone's word on windows, this computer's own
/// switch, and the windows already in use.
#[derive(Debug, Clone, Default)]
pub struct Route {
    pub mode: WindowsMode,
    pub setting: bool,
    pub used: Vec<u8>,
}

/// What an open asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// An app window with this id.
    Window(u8),
    /// The app on the phone's screen, shown here.
    Screen,
}

impl Route {
    /// How this route opens an app.
    pub fn pick(&self) -> OpenMode {
        open_mode(Some(self.mode), self.setting, &self.used)
    }

    /// The same, but never in a window (the phone could not make one).
    pub fn screen_only(&self) -> Route {
        Route {
            setting: false,
            ..self.clone()
        }
    }
}

/// What to say when the phone takes no more windows.
pub fn at_limit_words(limit: u8) -> String {
    let n = limit.max(1);
    format!(
        "The phone already shows {n} app window{} here: close one, then open the app again.",
        if n == 1 { "" } else { "s" }
    )
}

/// Ask `phone` to open `app` as `route` says. The text on failure is one
/// line for the owner.
pub fn ask_open(
    state_dir: &Path,
    phone: &str,
    app: &str,
    route: &Route,
    now: u64,
) -> Result<Asked, String> {
    use pocketdesk_core::local::phone_actions as ask;
    let failed = |e: std::io::Error| format!("Could not ask the phone: {e}");
    match route.pick() {
        OpenMode::Window(id) => {
            ask::request_open_window(state_dir, phone, app, WindowAsk::standard(id), now)
                .map(|_| Asked::Window(id))
                .map_err(failed)
        }
        OpenMode::Screen => ask::request_open_with_screen(state_dir, phone, app, true, now)
            .map(|_| Asked::Screen)
            .map_err(failed),
        OpenMode::AtLimit => Err(at_limit_words(route.mode.limit())),
    }
}

/// The words for the phone's mode, shown on the Phone apps page: whether an
/// app opens in a window of its own or on the phone's screen, and why.
pub fn mode_words(mode: WindowsMode, setting: bool, open: usize) -> String {
    let limit = mode.limit();
    match (limit, setting) {
        (0, _) => "Apps open on the phone's screen and are shown here. For separate windows, turn on \u{201c}Separate windows (Shizuku)\u{201d} in Pocketdesk's Settings on the phone (it needs Shizuku installed and running there; optional).".into(),
        (_, false) => "This phone can open apps in windows of their own; \u{201c}Open phone apps in windows of their own\u{201d} is off in this computer's Settings, so they open on the phone's screen.".into(),
        (n, true) => format!(
            "Apps open in windows of their own ({open} of {n} open): the phone's screen stays as it is."
        ),
    }
}

/// The app windows of phones, each with its viewer.
#[derive(Default)]
pub struct Windows {
    views: [Option<CastView>; MAX_WINDOWS as usize],
    /// The app each window was asked for, for its title.
    names: [Option<String>; MAX_WINDOWS as usize],
    /// Windows asked for whose picture has not come.
    reserved: [Option<Instant>; MAX_WINDOWS as usize],
    /// The windows whose files were there at the last look.
    present: [bool; MAX_WINDOWS as usize],
    looked: Option<Instant>,
}

impl Windows {
    /// The ids in use: shown, or asked for lately.
    pub fn used(&self) -> Vec<u8> {
        (1..=MAX_WINDOWS)
            .filter(|id| {
                let slot = usize::from(*id) - 1;
                self.present[slot] || self.reserved[slot].is_some_and(|at| at.elapsed() < RESERVE)
            })
            .collect()
    }

    /// How many windows are in use.
    pub fn count(&self) -> usize {
        self.used().len()
    }

    /// Window `id` was asked for, to show `app`'s name.
    pub fn reserve(&mut self, id: u8, name: &str) {
        if let Some(slot) = slot(id) {
            self.reserved[slot] = Some(Instant::now());
            self.names[slot] = Some(name.to_string());
        }
    }

    /// The phone did not make window `id`: it is free again.
    pub fn release(&mut self, id: u8) {
        if let Some(slot) = slot(id) {
            self.reserved[slot] = None;
        }
    }

    /// Look for new windows (twice a second at most): a viewer starts for
    /// each one that has a file.
    pub fn tick(&mut self, ctx: &egui::Context, state_dir: &Path) {
        if self.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return;
        }
        self.looked = Some(Instant::now());
        for id in 1..=MAX_WINDOWS {
            let slot = usize::from(id) - 1;
            let there = wire::read_window(state_dir, id).is_some();
            self.present[slot] = there;
            if there {
                self.reserved[slot] = None;
                if self.views[slot].is_none() {
                    self.views[slot] = Some(CastView::for_window(ctx, state_dir.to_path_buf(), id));
                }
            }
        }
    }

    /// Whether any window is there or asked for, so the page keeps looking.
    pub fn busy(&self) -> bool {
        self.count() > 0
    }

    /// Show each window that has a cast.
    pub fn show(&self, ctx: &egui::Context, devices: &[Device], control: bool) {
        for (slot, view) in self.views.iter().enumerate() {
            if let Some(view) = view {
                let name = self.names[slot].as_deref().unwrap_or("Phone app");
                view.show_window(ctx, devices, control, name);
            }
        }
    }
}

fn slot(id: u8) -> Option<usize> {
    pocketdesk_core::streaming::cast::valid_window(id).then(|| usize::from(id) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(on: bool) -> WindowsMode {
        WindowsMode { on, max: 4 }
    }

    fn route(mode: WindowsMode, setting: bool, used: &[u8]) -> Route {
        Route {
            mode,
            setting,
            used: used.to_vec(),
        }
    }

    #[test]
    fn a_window_needs_the_phone_and_this_computer_to_say_so() {
        assert_eq!(route(phone(true), true, &[]).pick(), OpenMode::Window(1));
        assert_eq!(route(phone(true), false, &[]).pick(), OpenMode::Screen);
        assert_eq!(route(phone(false), true, &[]).pick(), OpenMode::Screen);
        assert_eq!(
            route(phone(true), true, &[1, 2, 3, 4]).pick(),
            OpenMode::AtLimit
        );
        // A refused window is asked again as the ordinary open.
        assert_eq!(
            route(phone(true), true, &[]).screen_only().pick(),
            OpenMode::Screen
        );
    }

    #[test]
    fn the_words_say_what_an_open_does() {
        assert!(mode_words(phone(false), true, 0).contains("Shizuku"));
        assert!(mode_words(phone(true), false, 0).contains("off in this computer"));
        assert!(mode_words(phone(true), true, 2).contains("2 of 4"));
        assert!(at_limit_words(4).contains("4 app windows"));
        assert!(at_limit_words(1).contains("1 app window "));
    }

    #[test]
    fn an_id_asked_for_is_held_until_its_picture_comes_or_it_is_given_up() {
        let mut windows = Windows::default();
        assert!(windows.used().is_empty());
        windows.reserve(2, "Notes");
        assert_eq!(windows.used(), vec![2]);
        windows.release(2);
        assert!(windows.used().is_empty());
        windows.present[0] = true;
        windows.reserve(9, "No such window");
        assert_eq!(windows.used(), vec![1], "an id past the cap holds nothing");
        assert_eq!(windows.count(), 1);
        assert!(windows.busy());
    }

    #[test]
    fn asking_for_a_window_writes_the_ask_with_its_size() {
        let dir = std::env::temp_dir().join(format!("pd-phone-windows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let asked = ask_open(
            &dir,
            "phone-a",
            "org.example.app",
            &route(phone(true), true, &[1]),
            1_000,
        )
        .unwrap();
        assert_eq!(asked, Asked::Window(2));
        let taken = pocketdesk_core::local::phone_actions::take_all(&dir, 1_000);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].window, Some(WindowAsk::standard(2)));
        assert!(!taken[0].screen);
        // The ordinary open shows the screen, with no window.
        let asked = ask_open(
            &dir,
            "phone-a",
            "org.example.app",
            &route(phone(false), true, &[]),
            1_000,
        )
        .unwrap();
        assert_eq!(asked, Asked::Screen);
        let taken = pocketdesk_core::local::phone_actions::take_all(&dir, 1_000);
        assert!(taken[0].window.is_none() && taken[0].screen);
        // At the limit nothing is asked.
        let none = ask_open(
            &dir,
            "phone-a",
            "org.example.app",
            &route(phone(true), true, &[1, 2, 3, 4]),
            1_000,
        );
        assert!(none.is_err());
        assert!(pocketdesk_core::local::phone_actions::take_all(&dir, 1_000).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
