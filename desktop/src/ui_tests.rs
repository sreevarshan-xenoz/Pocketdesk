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

/// The folders go when each test ends (C4 review), so a run leaves no
/// pairing or PIN behind; the next test makes them again.
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(root) = folders().state.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
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
            // Opening Pair mints a code into the state folder; its own test,
            // `pair_opens_from_the_left`.
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

/// Pair opens from the left too, with its purpose (it mints a code into
/// the throwaway state folder as it does).
#[test]
fn pair_opens_from_the_left() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    go_to(&app, Tab::Pair);
    app.run_steps(3);
    assert!(app.query_by_label(Tab::Pair.purpose()).is_some());
}

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

/// I3: Esc answers an open question with Cancel.
#[test]
fn escape_cancels_the_unpair_question() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let mut app = fixture.open(Some(Tab::Devices));
    let asked = "Unpair Test Phone? It must pair again to connect.";
    app.get_by_label("Unpair…").click();
    app.run_steps(3);
    assert!(app.query_by_label(asked).is_some());
    app.key_press(egui::Key::Escape);
    app.run_steps(3);
    assert!(
        app.query_by_label(asked).is_none(),
        "Esc took the question away"
    );
    let store = TrustStore::read(fixture.state().join(TRUST_FILE)).expect("the trust store");
    assert!(store.is_trusted(PHONE), "and kept the phone");
}

/// I3: Ctrl+Page Down opens the next page down the left.
#[test]
fn control_page_down_opens_the_next_page() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Devices));
    app.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::PageDown);
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Sessions.purpose()).is_some(),
        "Sessions follows Devices"
    );
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
    // The battery's words beside its bar (I3), the rest on their own line.
    assert!(app.query_by_label("42 %, charging").is_some());
    assert!(app.query_by_label("no recent session").is_some());
    for action in [
        "Ring",
        "Mute for an hour",
        "Send file…",
        "Send link…",
        "Send the clipboard",
    ] {
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

/// With no phone paired, the Notifications and Phone files pages each offer
/// Pair a phone, which opens Pair (E5). (Clipboard's, the same, shows only
/// with its history on.)
#[test]
fn empty_pages_with_no_phone_offer_pairing() {
    let fixture = Fixture::new();
    for page in [Tab::Notifications, Tab::Phone] {
        let mut app = fixture.open(Some(page));
        app.run_steps(3);
        app.get_by_label("Pair a phone").click();
        app.run_steps(3);
        assert!(
            app.query_by_label(Tab::Pair.purpose()).is_some(),
            "{page:?}: Pair opened"
        );
    }
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

/// J1: the Logs page shows the newest log's lines and how many it shows.
#[test]
fn the_logs_page_shows_the_newest_lines() {
    let fixture = Fixture::new();
    let logs = fixture.state().join(pocketdesk_core::local::paths::LOG_DIR);
    std::fs::create_dir_all(&logs).expect("logs");
    std::fs::write(
        logs.join("pocketdesk.2026-10-20.log"),
        "2026-10-20T06:00:00Z  INFO pocketdesk: server starting\n2026-10-20T06:00:01Z  WARN pocketdesk: a refusal\n",
    )
    .expect("log");
    let mut app = fixture.open(Some(Tab::Logs));
    assert!(
        app.query_by_label("2026-10-20T06:00:01Z  WARN pocketdesk: a refusal")
            .is_some()
    );
    assert!(app.query_by_label("2 of the last 2 lines").is_some());
    // The filter narrows them, and the count follows in the same frame.
    let filter = app.get_by_role(egui::accesskit::Role::TextInput);
    filter.focus();
    filter.type_text("refusal");
    app.run();
    assert!(app.query_by_label("1 of the last 2 lines").is_some());
    assert!(
        app.query_by_label("2026-10-20T06:00:00Z  INFO pocketdesk: server starting")
            .is_none()
    );
}

/// J1 review: a log that exists but is empty yet (just rotated) is not
/// "no log file", and its folder can still be opened.
#[test]
fn an_empty_log_is_a_log() {
    let fixture = Fixture::new();
    let logs = fixture.state().join(pocketdesk_core::local::paths::LOG_DIR);
    std::fs::create_dir_all(&logs).expect("logs");
    std::fs::write(logs.join("pocketdesk.2026-10-21.log"), "").expect("log");
    let app = fixture.open(Some(Tab::Logs));
    assert!(app.query_by_label("Open log folder").is_some());
    assert!(app.query_by_label("0 of the last 0 lines").is_some());
}

/// J2: "Find a setting" shows the rows it finds, their sections open, and
/// says when it finds none.
#[test]
fn settings_search_shows_what_it_finds() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Settings));
    assert!(app.query_by_label("Control port (TCP)").is_some());
    assert!(
        app.query_by_label("Outside wake port").is_none(),
        "Wake starts closed"
    );
    find(&app).focus();
    find(&app).type_text("wake port");
    app.run_steps(3);
    assert!(
        app.query_by_label("Outside wake port").is_some(),
        "found, its section opened"
    );
    assert!(app.query_by_label("Control port (TCP)").is_none());
    assert!(app.query_by_label("Wake from the phone").is_none());

    find(&app).focus();
    find(&app).type_text(" zzzz");
    app.run_steps(3);
    assert!(app.query_by_label_contains("No setting matches").is_some());
    assert!(app.query_by_label("Outside wake port").is_none());

    // Esc in the box empties it: every section back.
    find(&app).focus();
    app.run();
    app.key_press(egui::Key::Escape);
    app.run_steps(3);
    assert!(app.query_by_label_contains("No setting matches").is_none());
    assert!(app.query_by_label("Control port (TCP)").is_some());
}

/// J2 review: a section held open by a problem stays, whole, under a
/// search that does not find it, so the field the save bar names is there.
#[test]
fn settings_search_keeps_a_section_with_a_problem() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.config().join("config.toml"),
        "streaming_port = 5000
wake_port = 5000
",
    )
    .expect("config");
    let mut app = fixture.open(Some(Tab::Settings));
    find(&app).focus();
    find(&app).type_text("laser");
    app.run_steps(3);
    assert!(app.query_by_label("Laser pointer from phones").is_some());
    assert!(app.query_by_label("Outside wake port").is_some());
    assert!(
        app.query_by_label("Wake from the phone").is_some(),
        "the held section whole"
    );
    assert!(app.query_by_label("Control port (TCP)").is_none());
}

/// The "Find a setting" box: Settings' first text box.
fn find<'a>(app: &'a Harness<'static, PocketdeskApp>) -> egui_kittest::Node<'a> {
    app.get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the search box")
}

/// J3: the Sessions page lists the recent sessions from the log, by the
/// phone's name, a refused one as refused, with the server not running.
#[test]
fn the_sessions_page_lists_the_recent_sessions() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let log = format!(
        "{{\"unix_secs\":{},\"outcome\":\"lan_direct\",\"duration_secs\":125,\"device_id\":\"{PHONE}\",\"e2e_tail\":{{\"count\":3,\"mean_ms\":40,\"p50_ms\":38,\"p90_ms\":50,\"p95_ms\":55,\"p99_ms\":70,\"max_ms\":90}}}}\n\
         {{\"unix_secs\":{},\"outcome\":\"rejected\",\"duration_secs\":0,\"device_id\":\"{PHONE}\"}}\n",
        now - 7200,
        now - 120,
    );
    std::fs::write(
        fixture
            .state()
            .join(pocketdesk_core::local::paths::SESSIONS_FILE),
        log,
    )
    .expect("log");
    let mut app = fixture.open(Some(Tab::Sessions));
    app.run_steps(3);
    assert!(
        app.query_by_label_contains("The server is not running")
            .is_some()
    );
    assert!(app.query_by_label("Recent sessions").is_some());
    assert!(
        app.query_by_label("2 h ago · for 2m 5s · on this network · 38 ms capture to arrival")
            .is_some()
    );
    assert!(app.query_by_label("2 min ago · refused").is_some());
    // A refusal is named by its claim, never as the phone (J3 review).
    assert_eq!(app.query_all_by_label("Test Phone").count(), 1);
    assert!(
        app.query_by_label_contains("A device claiming to be")
            .is_some()
    );
}

/// J4: the Activity page's Search narrows the events by their line, and
/// under a kind chip says the chip narrows too.
#[test]
fn the_activity_page_searches_its_events() {
    use pocketdesk_core::local::events::{Event, EventsDetail, append};
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    for (kind, meta) in [
        ("phone_linked", serde_json::json!({})),
        ("link_received", serde_json::json!({"host": "example.org"})),
    ] {
        let event = Event::new(
            kind,
            Some(PHONE),
            now,
            meta,
            serde_json::json!({}),
            EventsDetail::Metadata,
        );
        append(fixture.state(), &event).expect("logged");
    }
    let mut app = fixture.open(Some(Tab::Activity));
    app.run_steps(3);
    assert!(app.query_by_label("Connected").is_some());
    assert!(app.query_by_label_contains("example.org").is_some());

    // Ctrl+F puts the cursor in the box, as on Settings.
    app.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::F);
    app.run_steps(2);
    assert!(
        app.get_by_role(egui::accesskit::Role::TextInput)
            .is_focused()
    );
    app.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("connected");
    app.run_steps(3);
    assert!(app.query_by_label("Connected").is_some());
    assert!(app.query_by_label_contains("example.org").is_none());

    app.get_by_label("Files and links").click();
    app.run_steps(3);
    assert!(
        app.query_by_label("Nothing of this kind matches \"connected\".")
            .is_some()
    );
}

/// J4: the Clipboard page's Search narrows the texts, and says when it
/// finds none, with Clear search.
#[test]
fn the_clipboard_page_searches_its_texts() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    std::fs::write(
        fixture.config().join("config.toml"),
        "clipboard_history = 20\n",
    )
    .expect("config");
    use pocketdesk_core::local::clip_history::{ClipEntry, record};
    record(
        fixture.state(),
        ClipEntry::new("12 Main Street", Some(PHONE), 100),
        20,
    )
    .expect("kept");
    record(
        fixture.state(),
        ClipEntry::new("the meeting link", None, 200),
        20,
    )
    .expect("kept");
    let mut app = fixture.open(Some(Tab::Clipboard));
    app.get_by_role(egui::accesskit::Role::TextInput).focus();
    app.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("test phone");
    app.run_steps(3);
    assert!(app.query_by_label("12 Main Street").is_some());
    assert!(app.query_by_label("the meeting link").is_none());

    app.get_by_role(egui::accesskit::Role::TextInput).focus();
    app.get_by_role(egui::accesskit::Role::TextInput)
        .type_text(" nowhere");
    app.run_steps(3);
    assert!(app.query_by_label_contains("No kept text holds").is_some());
    app.get_by_label("Clear search").click();
    app.run_steps(3);
    assert!(app.query_by_label("the meeting link").is_some());
    assert!(app.query_by_label("12 Main Street").is_some());

    // Esc in the box empties the search and leaves Clear's question open
    // (J4 review: Clear's Cancel, drawn first, had taken it).
    app.get_by_label("Clear history…").click();
    app.run_steps(3);
    app.get_by_role(egui::accesskit::Role::TextInput).focus();
    app.get_by_role(egui::accesskit::Role::TextInput)
        .type_text("main");
    app.run_steps(3);
    assert!(app.query_by_label("the meeting link").is_none());
    app.key_press(egui::Key::Escape);
    app.run_steps(3);
    assert!(app.query_by_label("the meeting link").is_some(), "emptied");
    assert!(
        app.query_by_label("Forget all 2 texts kept here?")
            .is_some(),
        "the question stays"
    );
}

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct TestDrop(PathBuf);

impl egui::DroppedFile for TestDrop {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// J5: a file dropped with no phone or computer to take it opens Files and
/// says so, sending nothing.
#[test]
fn a_drop_with_nothing_to_take_it_says_so() {
    let fixture = Fixture::new();
    let file = fixture.state().join("dropped.txt");
    std::fs::write(&file, "hello").expect("a file");
    let mut app = fixture.open(None);
    app.input_mut()
        .dropped_files
        .push(std::sync::Arc::new(TestDrop(file)));
    app.run_steps(3);
    assert!(app.query_by_label("Send a file").is_some(), "Files opened");
    assert!(
        app.query_by_label_contains("No phone or computer is connected")
            .is_some()
    );
    assert!(app.query_by_label_contains("Send 1 file to").is_none());
}

/// D3: the Clipboard page says the history is off, and with it on lists
/// the texts that crossed, each with Copy and Send to….
#[test]
fn the_clipboard_page_lists_what_crossed() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let app = fixture.open(Some(Tab::Clipboard));
    assert!(
        app.query_by_label("Turn it on in Settings").is_some(),
        "off by default"
    );
    drop(app);

    std::fs::write(
        fixture.config().join("config.toml"),
        "clipboard_history = 20\n",
    )
    .expect("config");
    use pocketdesk_core::local::clip_history::{ClipEntry, record};
    record(
        fixture.state(),
        ClipEntry::new("from the phone", Some(PHONE), 100),
        20,
    )
    .expect("kept");
    record(fixture.state(), ClipEntry::new("from here", None, 200), 20).expect("kept");
    let app = fixture.open(Some(Tab::Clipboard));
    assert!(app.query_by_label("from here").is_some());
    assert!(app.query_by_label("from the phone").is_some());
    assert!(app.query_by_label_contains("From Test Phone").is_some());
    assert!(app.query_by_label_contains("From this computer").is_some());
    assert_eq!(app.query_all_by_label("Copy").count(), 2, "one each");
    // Clear asks, with what it forgets, and Cancel keeps them (E5).
    let mut app = app;
    app.get_by_label("Clear history…").click();
    app.run_steps(3);
    assert!(
        app.query_by_label("Forget all 2 texts kept here?")
            .is_some()
    );
    app.get_by_label("Cancel").click();
    app.run_steps(3);
    assert!(app.query_by_label("from here").is_some(), "kept");
    app.get_by_label("Clear history…").click();
    app.run_steps(3);
    app.get_by_label("Clear").click();
    app.run_steps(3);
    assert!(app.query_by_label("from here").is_none(), "cleared");
}
