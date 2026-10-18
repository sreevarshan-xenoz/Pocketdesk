//! The app driven headless through AccessKit (C4, `egui_kittest`), over
//! throwaway state and config folders: what the checklist's owed desktop
//! checks did by eye.

use crate::app::{PocketdeskApp, Tab};
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pocketdesk_core::local::paths::TRUST_FILE;
use pocketdesk_core::local::phone_status::PhoneBattery;
use pocketdesk_core::protocol::kdeconnect::TrustStore;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The window's size in every test, the snapshots' too.
pub const SIZE: egui::Vec2 = egui::vec2(720.0, 480.0);

/// The throwaway state and config folders, one pair for the test binary,
/// set as the process's own (the core's test seam) before any test reads a
/// path, so no test reads or writes this computer's.
struct Folders {
    state: PathBuf,
    config: PathBuf,
}

fn folders() -> &'static Folders {
    static FOLDERS: OnceLock<Folders> = OnceLock::new();
    FOLDERS.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("pd-ui-{}", std::process::id()));
        let state = root.join("state");
        let config = root.join("config");
        // In-process, not the environment (C4 review): no `unsafe` setenv
        // while other tests run on other threads.
        pocketdesk_core::local::paths::use_state_dir(state.clone());
        pocketdesk_core::local::paths::use_config_dir(config.clone());
        Folders { state, config }
    })
}

/// The tests share those folders, so they run one at a time, each over
/// empty ones.
pub struct Fixture {
    _turn: MutexGuard<'static, ()>,
}

impl Fixture {
    pub fn new() -> Self {
        static TURN: Mutex<()> = Mutex::new(());
        let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        let f = folders();
        for dir in [&f.state, &f.config] {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir).expect("a throwaway folder");
        }
        Self { _turn: turn }
    }

    pub fn state(&self) -> &Path {
        &folders().state
    }

    pub fn config(&self) -> &Path {
        &folders().config
    }

    /// The app over these folders, open on `page`.
    pub fn open(&self, page: Option<Tab>) -> Harness<'static, PocketdeskApp> {
        let state = self.state().to_path_buf();
        let mut harness = Harness::builder().with_size(SIZE).build_eframe(move |cc| {
            // As the app sets itself up (main.rs), less the system's
            // fonts: egui's own draw the same on every machine.
            crate::theme::install(&cc.egui_ctx);
            PocketdeskApp::new(state, None, page)
        });
        harness.run_steps(3);
        harness
    }
}

/// Open `page` from the left: the first of its name, since its heading
/// says it too and the left is drawn first.
pub fn go_to(app: &Harness<'static, PocketdeskApp>, page: Tab) {
    app.get_all_by_label(page.name())
        .next()
        .unwrap_or_else(|| panic!("{} down the left", page.name()))
        .click();
}

/// Every page down the left opens, with its heading and its purpose line.
#[test]
fn every_page_opens_from_the_left() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    for page in Tab::ALL {
        if page == Tab::Pair {
            // Opening Pair mints a code into the state folder; its own test.
            continue;
        }
        go_to(&app, page);
        app.run_steps(3);
        assert!(
            app.query_by_label(page.purpose()).is_some(),
            "{} shows what it is for",
            page.name()
        );
    }
}

/// A phone paired as pairing leaves it: trusted, with the name it gave.
fn pair_phone(state: &Path, id: &str, name: &str) {
    let mut store = TrustStore::load_or_create(state.join(TRUST_FILE)).expect("a trust store");
    store.trust_device(id).expect("trusted");
    store.note_claimed_name(id, name).expect("named");
}

/// A test phone's id: as long as a real one, and no real phone's.
const PHONE: &str = "00000000-0000-4000-8000-0000000000c4";

/// With no phone paired, Devices says how to pair one, and its button goes
/// there.
#[test]
fn devices_with_none_paired_say_how_to_pair() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Devices));
    assert!(
        app.query_by_label("No phone is paired yet. The phone scans a code from the Pair page.")
            .is_some()
    );
    app.get_by_label("Pair a phone").click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Pair.purpose()).is_some(),
        "Pair opened"
    );
}

/// Unpair… asks by name; Cancel keeps the phone, and Unpair removes it
/// from the trust store.
#[test]
fn unpair_asks_by_name_and_only_the_answer_unpairs() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let mut app = fixture.open(Some(Tab::Devices));
    let asked = "Unpair Test Phone? It must pair again to connect.";

    app.get_by_label("Unpair…").click();
    app.run_steps(3);
    assert!(app.query_by_label(asked).is_some(), "asked by name");
    app.get_by_label("Cancel").click();
    app.run_steps(3);
    assert!(app.query_by_label(asked).is_none());
    let store = TrustStore::read(fixture.state().join(TRUST_FILE)).expect("the trust store");
    assert!(store.is_trusted(PHONE), "Cancel keeps it");

    app.get_by_label("Unpair…").click();
    app.run_steps(3);
    app.get_by_label("Unpair").click();
    app.run_steps(3);
    let store = TrustStore::read(fixture.state().join(TRUST_FILE)).expect("the trust store");
    assert!(!store.is_trusted(PHONE), "Unpair removes it");
}

/// The Overview shows a paired phone's block: where it is, its battery,
/// and Ring, Send file… and Send link… (N6).
#[test]
fn the_overview_shows_a_phone_with_its_battery_and_actions() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock")
        .as_secs();
    let batteries = [(
        PHONE.to_string(),
        PhoneBattery {
            charge: 42,
            charging: true,
            updated_unix: now,
        },
    )]
    .into_iter()
    .collect();
    pocketdesk_core::local::phone_status::write(fixture.state(), &batteries).expect("battery");
    let app = fixture.open(None);
    assert!(app.query_by_label("Your phones").is_some());
    assert!(app.query_by_label("Test Phone").is_some());
    assert!(app.query_by_label("Away").is_some(), "no session or link");
    assert!(
        app.query_by_label("Battery 42 %, charging · no recent session")
            .is_some()
    );
    for action in ["Ring", "Send file…", "Send link…"] {
        assert!(app.query_by_label(action).is_some(), "{action}");
    }
}

/// A change in Settings shows the save bar; Undo takes it back, and Save
/// writes the config file and says so.
#[test]
fn the_save_bar_comes_with_a_change_and_goes_with_undo_or_save() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Settings));
    let pending = "Settings have changes not saved yet.";
    let toggle = "phones on the same network list this computer";
    assert!(app.query_by_label(pending).is_none(), "nothing changed yet");

    app.get_by_label(toggle).click();
    app.run_steps(3);
    assert!(
        app.query_by_label(pending).is_some(),
        "a change shows the bar"
    );
    app.get_by_label("Undo changes").click();
    app.run_steps(3);
    assert!(app.query_by_label(pending).is_none(), "Undo hides it");

    app.get_by_label(toggle).click();
    app.run_steps(3);
    app.get_by_label("Save").click();
    app.run_steps(3);
    assert!(app.query_by_label(pending).is_none(), "saved");
    assert!(app.query_by_label("Saved.").is_some());
    let saved = std::fs::read_to_string(fixture.config().join("config.toml")).expect("written");
    assert!(saved.contains("lan_discovery = false"), "{saved}");
}

/// Sessions and Files with nothing to show say what to do next, and their
/// button goes there (U5).
#[test]
fn empty_sessions_and_files_say_what_to_do_next() {
    let fixture = Fixture::new();
    // No server runs here, so Sessions sends the owner to start it.
    let mut app = fixture.open(Some(Tab::Sessions));
    assert!(
        app.query_by_label(
            "The server is not running, so no phone is connected. Start it from Overview."
        )
        .is_some()
    );
    app.get_all_by_label("Overview")
        .last()
        .expect("the page's own Overview button")
        .click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Status.purpose()).is_some(),
        "Overview opened"
    );

    go_to(&app, Tab::Files);
    app.run_steps(3);
    assert!(
        app.query_by_label_contains("No phone or computer can take a file right now.")
            .is_some()
    );
    app.get_by_label("Pair a phone").click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Pair.purpose()).is_some(),
        "Pair opened"
    );
}

/// The Overview, Devices and Settings at 720×480 in both themes, compared
/// with the pictures under `desktop/tests/snapshots/` (C4). They need a GPU
/// renderer, so only with the `snapshots` feature; CI draws them with
/// Mesa's software Vulkan, where they were made. `UPDATE_SNAPSHOTS=1`
/// draws them anew.
#[cfg(feature = "snapshots")]
mod snapshots {
    use super::*;

    #[test]
    fn the_pages_look_as_they_did() {
        let fixture = Fixture::new();
        pair_phone(fixture.state(), PHONE, "Test Phone");
        // egui's own colour threshold, and a few pixels of slack for a
        // Mesa update's anti-aliasing (C4 review); more could hide a moved
        // border.
        let options = egui_kittest::SnapshotOptions::new().max_failed_pixels(8);
        let mut failed = Vec::new();
        for (page, name) in [
            (None, "overview"),
            (Some(Tab::Devices), "devices"),
            (Some(Tab::Settings), "settings"),
        ] {
            for (theme, shade) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
                let state = fixture.state().to_path_buf();
                let mut app = Harness::builder()
                    .with_size(SIZE)
                    .with_theme(theme)
                    .wgpu()
                    .build_eframe(move |cc| {
                        crate::theme::install(&cc.egui_ctx);
                        PocketdeskApp::new(state, None, page)
                    });
                app.run_steps(3);
                if let Err(e) = app.try_snapshot_options(format!("{name}_{shade}"), &options) {
                    failed.push(e.to_string());
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
