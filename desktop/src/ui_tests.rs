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

/// M2b: the Pair page asks whose phone it is, beside its code, and
/// "Someone else's" makes a code that says so in the PIN file.
#[test]
fn pair_asks_whose_phone_it_is_and_someone_elses_code_says_so() {
    use pocketdesk_core::local::pin;
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    go_to(&app, Tab::Pair);
    let asked = (0..50).any(|_| {
        app.run_steps(2);
        let found = app.query_by_label("Whose phone is it?").is_some();
        if !found {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        found
    });
    assert!(asked, "the Pair page never asked whose phone it is");
    app.get_by_role(egui::accesskit::Role::ComboBox).click();
    app.run_steps(2);
    app.get_by_label("Someone else's").click();
    let said = (0..50).any(|_| {
        app.run_steps(2);
        let file = pin::read(&folders().state, pin::now_unix());
        let said = file.is_some_and(|f| f.someone_else && f.grant_secs.is_none());
        if !said {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        said
    });
    assert!(said, "someone else's code was never written");
}

/// PC1: the Pair page's box for another of the owner's computers is off
/// until ticked; ticked, it makes a code that says so in the PIN file.
#[test]
fn pair_ticks_another_of_my_computers_into_a_new_code() {
    use pocketdesk_core::local::pin;
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    go_to(&app, Tab::Pair);
    app.run_steps(2);
    // UX 2: on Pair's Computer side.
    app.get_by_label(crate::pair_view::FOR_COMPUTER).click();
    let shown = (0..50).any(|_| {
        app.run_steps(2);
        let found = app.query_by_label(crate::pair_view::MINE_BOX).is_some();
        if !found {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        found
    });
    assert!(shown, "the Pair page never showed the box");
    let plain = pin::read(&folders().state, pin::now_unix()).expect("a code on file");
    assert!(!plain.mine, "off until ticked");
    app.get_by_label(crate::pair_view::MINE_BOX).click();
    let said = (0..50).any(|_| {
        app.run_steps(2);
        let file = pin::read(&folders().state, pin::now_unix());
        let said = file.is_some_and(|f| f.mine && !f.someone_else);
        if !said {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        said
    });
    assert!(
        said,
        "the code for another of my computers was never written"
    );
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

/// L7: a phone's report shows each feature's sides, with what holds one
/// back; the table is closed until opened.
#[test]
fn devices_show_each_features_sides_and_what_holds_it_back() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let mut phones = std::collections::BTreeMap::new();
    phones.insert(
        PHONE.to_string(),
        pocketdesk_core::local::phone_switches::PhoneSwitches {
            switches: pocketdesk_core::local::phone_switches::from_body(&serde_json::json!({
                "switches": {"notifications": {"on": true, "android": false}}
            }))
            .expect("a report"),
            updated_unix: pocketdesk_core::local::pin::now_unix(),
        },
    );
    pocketdesk_core::local::phone_switches::write(fixture.state(), &phones).expect("written");
    let mut app = fixture.open(Some(Tab::Devices));
    let advice = "Give Pocketdesk notification access on the phone.";
    assert!(app.query_by_label(advice).is_none(), "closed until opened");
    app.get_by_label("Features on both sides for Test Phone")
        .click();
    app.run_steps(3);
    assert!(app.query_by_label(advice).is_some());
    assert!(
        app.query_by_label("This computer allows: yes · The phone sends: on · Android allows: no")
            .is_some()
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

/// BA3: a computer is unpaired by the server, so with it stopped, Unpair
/// waits for it, as Pair does.
#[test]
fn unpairing_a_computer_waits_for_the_server() {
    let fixture = Fixture::new();
    pocketdesk_core::local::computers::update(fixture.state(), |file| {
        file.computers
            .push(pocketdesk_core::local::computers::PeerComputer {
                device_id: "00000000-0000-4000-8000-0000000000ba".to_string(),
                name: "Test Desk".to_string(),
                addresses: Vec::new(),
                control_port: 1716,
                stream_port: 4716,
                fingerprint: String::new(),
                endpoint: None,
                paired_unix: 0,
                link: String::new(),
                link_unix: 0,
                lets_drive: false,
                mine: false,
                wake: None,
                viewed_unix: 0,
            });
    })
    .expect("a computers file");
    let mut app = fixture.open(Some(Tab::Settings));
    // Its section opened by the search, as it starts closed.
    find(&app).focus();
    find(&app).type_text("Other computers");
    app.run_steps(3);
    assert!(app.query_by_label("Test Desk").is_some());
    use egui_kittest::kittest::NodeT;
    assert!(
        app.get_by_label("Unpair…").accesskit_node().is_disabled(),
        "no server here, so no Unpair"
    );
}

/// UX 2: Settings → Other computers points to the Pair page, whose
/// Computer side has both halves: this computer's link and the box for the
/// other's.
#[test]
fn other_computers_point_to_pairing_on_the_pair_page() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Settings));
    find(&app).focus();
    find(&app).type_text("Other computers");
    app.run_steps(3);
    assert!(app.query_by_label(crate::app::PAIR_ELSEWHERE).is_some());
    assert!(
        app.query_by_label("From the other computer").is_none(),
        "no paste box in Settings"
    );
    // Below the section's settings, which can push it past the window.
    app.get_by_label(crate::app::PAIR_A_COMPUTER).scroll_to_me();
    app.run_steps(3);
    app.get_by_label(crate::app::PAIR_A_COMPUTER).click();
    let both = (0..50).any(|_| {
        app.run_steps(2);
        let found = app.query_by_label(crate::pair_view::COPY_LINK).is_some()
            && app.query_by_label("From the other computer").is_some();
        if !found {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        found
    });
    assert!(both, "Pair opened on Computer with both halves");
    assert!(
        app.query_by_label("Pair a phone").is_none(),
        "not the phone's half"
    );
}

/// Pair a computer nearby: Pair → Computer starts with the Nearby list
/// (looking, until something answers), the link below it.
#[test]
fn pair_computer_starts_with_the_nearby_list() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    go_to(&app, Tab::Pair);
    app.run_steps(2);
    app.get_by_label(crate::pair_view::FOR_COMPUTER).click();
    app.run_steps(3);
    assert!(app.query_by_label(crate::nearby_view::NEARBY).is_some());
    assert!(app.query_by_label(crate::nearby_view::NOT_LISTED).is_some());
}

/// Pair a computer nearby: another computer's ask shows over any page,
/// with its code; Decline answers it.
#[test]
fn a_nearby_computers_ask_shows_over_any_page() {
    let fixture = Fixture::new();
    pocketdesk_core::local::computers::update(fixture.state(), |file| {
        file.nearby_in = Some(pocketdesk_core::local::nearby::NearbyRequest {
            id: "0a1b2c3d4e5f6071".to_string(),
            device_id: "00000000-0000-4000-8000-0000000000e1".to_string(),
            name: "Test Tower".to_string(),
            address: "192.0.2.60".to_string(),
            code: "482913".to_string(),
            unix: pocketdesk_core::local::pin::now_unix(),
        });
    })
    .expect("a computers file");
    let mut app = fixture.open(Some(Tab::Devices));
    app.run_steps(3);
    assert!(
        app.query_by_label(&crate::nearby_view::prompt_title("Test Tower"))
            .is_some()
    );
    assert!(app.query_by_label("482 913").is_some());
    app.get_by_label("Decline").click();
    app.run_steps(3);
    assert!(app.query_by_label("482 913").is_none());
    let now = pocketdesk_core::local::pin::now_unix();
    let asks = pocketdesk_core::local::computers::take_all(fixture.state(), now);
    assert_eq!(asks.len(), 1, "one answer");
}

/// PC1: Other computers says which computer is paired both ways, seeing
/// and controlling this one without asking, and only that one.
#[test]
fn other_computers_say_which_is_mine() {
    let fixture = Fixture::new();
    let computer =
        |id: &str, name: &str, mine: bool| pocketdesk_core::local::computers::PeerComputer {
            device_id: id.to_string(),
            name: name.to_string(),
            addresses: Vec::new(),
            control_port: 1716,
            stream_port: 4716,
            fingerprint: String::new(),
            endpoint: None,
            paired_unix: 0,
            link: "linked".to_string(),
            link_unix: 0,
            lets_drive: false,
            mine,
            wake: None,
            viewed_unix: 0,
        };
    pocketdesk_core::local::computers::update(fixture.state(), |file| {
        file.computers.push(computer(
            "00000000-0000-4000-8000-0000000000e1",
            "Test Tower",
            true,
        ));
    })
    .expect("a computers file");
    let mut app = fixture.open(Some(Tab::Settings));
    find(&app).focus();
    find(&app).type_text("Other computers");
    app.run_steps(3);
    assert!(app.query_by_label("Test Tower").is_some());
    assert_eq!(
        app.query_all_by_label(crate::computers_view::MINE).count(),
        1,
        "said for the computer paired both ways"
    );
    // One paired one way says nothing of the kind.
    pocketdesk_core::local::computers::update(fixture.state(), |file| {
        file.computers[0].mine = false;
    })
    .expect("a computers file");
    let mut app = fixture.open(Some(Tab::Settings));
    find(&app).focus();
    find(&app).type_text("Other computers");
    app.run_steps(3);
    assert!(app.query_by_label("Test Tower").is_some());
    assert!(app.query_by_label(crate::computers_view::MINE).is_none());
}

/// Test computers for the Keyboard and mouse page: this one ("Test Desk")
/// and one it paired with ("Test Laptop"), linked directly, that answered
/// with its screens.
const DESK: &str = "00000000-0000-4000-8000-0000000000d1";
const LAPTOP: &str = "00000000-0000-4000-8000-0000000000d2";

fn keyboard_mouse_computers(state: &Path) {
    use pocketdesk_core::local::shared_input::{self, Screen, StatusFile};
    pocketdesk_core::local::computers::update(state, |file| {
        file.computers
            .push(pocketdesk_core::local::computers::PeerComputer {
                device_id: LAPTOP.to_string(),
                name: "Test Laptop".to_string(),
                addresses: Vec::new(),
                control_port: 1716,
                stream_port: 4716,
                fingerprint: String::new(),
                endpoint: None,
                paired_unix: 0,
                link: "linked".to_string(),
                link_unix: 0,
                lets_drive: false,
                mine: false,
                wake: None,
                viewed_unix: 0,
            });
    })
    .expect("a computers file");
    let screen = |width, height| Screen {
        x: 0,
        y: 0,
        width,
        height,
    };
    shared_input::update(state, |file| {
        file.set_own(DESK, "Test Desk", &[screen(1920, 1080)], 1);
        file.heard(LAPTOP, Some("Test Laptop"), &[screen(1600, 1000)], 1);
    })
    .expect("a screens file");
    let status = StatusFile {
        keyboard_on: String::new(),
        link: [(LAPTOP.to_string(), "direct".to_string())].into(),
        answered: [LAPTOP.to_string()].into(),
        sharing_on: true,
        ..StatusFile::default()
    };
    shared_input::write_status(state, &status).expect("a status file");
}

/// The Keyboard and mouse page: the paired computer by name and how it is
/// linked; its switch, for a computer this one paired with, waits for the
/// server; Identify asks for each computer that answered; the sharing
/// switch is the setting.
#[test]
fn the_keyboard_and_mouse_page_lets_either_side_drive() {
    let fixture = Fixture::new();
    keyboard_mouse_computers(fixture.state());
    let mut app = fixture.open(Some(Tab::KeyboardMouse));
    app.run_steps(3);
    assert!(app.query_by_label("Test Laptop").is_some());
    // Its own line, and Setup's step.
    assert_eq!(app.query_all_by_label("Linked directly").count(), 2);
    assert!(
        app.query_by_label("Test Desk may drive Test Laptop")
            .is_some(),
        "it answered: it lets this one drive it"
    );
    assert!(
        app.query_by_label("The keyboard and mouse are on this computer.")
            .is_some()
    );

    // Kept by the server for a computer this one paired with: no server
    // here, so the switch waits for it (the request itself is core's test).
    use egui_kittest::kittest::NodeT;
    assert!(
        app.get_by_label("Test Laptop may drive Test Desk")
            .accesskit_node()
            .is_disabled(),
        "no server here, so no switch"
    );
    // Below Setup: by the keyboard, which scrolls to it.
    app.get_by_label("Identify").focus();
    app.run_steps(2);
    app.key_press(egui::Key::Enter);
    app.run_steps(3);
    // Read after the click: a request made in the next second would be
    // "from the future" to a clock read before it.
    let now = pocketdesk_core::local::pin::now_unix();
    let identify = pocketdesk_core::local::shared_input::take_all(fixture.state(), now);
    assert_eq!(identify.len(), 1, "one Identify asked");

    app.get_by_label(crate::arrange_view::SHARE_SWITCH).click();
    app.run_steps(3);
    assert!(
        load_config(&fixture).shared_input,
        "the page's switch is the setting"
    );
}

/// BK5: a phone that said its screen is on the Keyboard and mouse page,
/// with why it is not ready yet, and this computer's permission for it,
/// which the switch there sets in the trust store.
#[test]
fn the_keyboard_and_mouse_page_shows_the_phone_as_a_screen() {
    use pocketdesk_core::local::shared_input::{self, PHONE_OFF, PhoneScreen, StatusFile};
    use pocketdesk_core::protocol::permissions::Capability;
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    shared_input::update(fixture.state(), |file| {
        file.set_own(
            DESK,
            "Test Desk",
            &[shared_input::Screen {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }],
            1,
        );
        file.heard_phone(
            PHONE,
            "Test Phone",
            &PhoneScreen {
                width_px: 1080,
                height_px: 2400,
                width_dp: 411,
                height_dp: 914,
                rotation: 0,
            },
            1,
        );
    })
    .expect("a screens file");
    let status = StatusFile {
        phones: [(PHONE.to_string(), PHONE_OFF.to_string())].into(),
        ..StatusFile::default()
    };
    shared_input::write_status(fixture.state(), &status).expect("a status file");
    let mut app = fixture.open(Some(Tab::KeyboardMouse));
    app.run_steps(3);
    assert!(app.query_by_label("Your phones").is_some());
    assert!(
        app.query_by_label(
            "Off on Test Phone: turn on \"Let my computer's keyboard and mouse use this phone\" in its Settings"
        )
        .is_some()
    );
    // Below the fold in the test window: clicked as a screen reader would.
    app.get_by_label("Test Desk may drive Test Phone")
        .click_accesskit();
    app.run_steps(3);
    let store = TrustStore::read(fixture.state().join(TRUST_FILE)).expect("the trust store");
    assert!(
        store.permissions(PHONE).allows(Capability::SharedInput),
        "the switch is the phone's Keyboard and mouse permission"
    );
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

/// Q2: Apply a profile asks first, saying what it turns off; Cancel
/// changes nothing, and Apply gives the phone the profile's permissions.
#[test]
fn applying_a_profile_asks_first() {
    use pocketdesk_core::protocol::permissions::{Capability, DevicePermissions};
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let mut kids = DevicePermissions::default();
    kids.set(Capability::Input, false);
    pocketdesk_core::local::permission_profiles::save(fixture.state(), "Kids", &kids)
        .expect("saved");
    let input = |state: &Path| {
        TrustStore::read(state.join(TRUST_FILE))
            .expect("the trust store")
            .permissions(PHONE)
            .allows(Capability::Input)
    };
    assert!(input(fixture.state()), "a paired phone has input");
    let mut app = fixture.open(Some(Tab::Devices));
    app.get_by_label("What Test Phone may do").click();
    app.run_steps(3);
    let asked = "Give Test Phone the Kids profile?";

    app.get_by_label("Apply a profile").click();
    app.run_steps(3);
    app.get_by_label("Kids").click();
    app.run_steps(3);
    assert!(app.query_by_label_contains(asked).is_some(), "asked first");
    assert!(
        app.query_by_label_contains("It turns off: Control the mouse and keyboard")
            .is_some(),
        "says what it turns off"
    );
    app.get_by_label("Cancel").click();
    app.run_steps(3);
    assert!(app.query_by_label_contains(asked).is_none());
    assert!(input(fixture.state()), "Cancel changes nothing");

    app.get_by_label("Apply a profile").click();
    app.run_steps(3);
    app.get_by_label("Kids").click();
    app.run_steps(3);
    app.get_by_label("Apply").click();
    app.run_steps(3);
    assert!(!input(fixture.state()), "Apply turns input off");
    pocketdesk_core::local::permission_profiles::remove(fixture.state(), "Kids").expect("removed");
}

/// I3: Ctrl+Page Down opens the next page down the left.
#[test]
fn control_page_down_opens_the_next_page() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Devices));
    app.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::PageDown);
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::KeyboardMouse.purpose()).is_some(),
        "Keyboard and mouse follows Devices"
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
            ..Default::default()
        },
    )]
    .into_iter()
    .collect();
    pocketdesk_core::local::phone_status::write(fixture.state(), &batteries).expect("battery");
    let app = fixture.open(None);
    // UX 3: phones and computers under one heading.
    assert!(app.query_by_label(crate::app::YOUR_DEVICES).is_some());
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
    // No server runs here: the bar at the top says so (UX 1), not the page.
    let mut app = fixture.open(Some(Tab::Sessions));
    assert!(app.query_by_label(crate::app::STOPPED_BAR).is_some());
    assert!(
        app.query_by_label_contains("Start it from Overview")
            .is_none()
    );

    go_to(&app, Tab::Files);
    app.run_steps(3);
    assert!(
        app.query_by_label_contains("No phone or computer can take a file right now.")
            .is_some()
    );
    // AS3: the sentence after it says what can, with its verb.
    assert!(
        app.query_by_label_contains("A phone takes one in a session")
            .is_some()
    );
    app.get_by_label("Pair a phone").click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Pair.purpose()).is_some(),
        "Pair opened"
    );
}

/// UX 4: Sessions with none live says, in a card, what shows there and
/// how; its Pair a phone opens Pair. Files says what it will list.
#[test]
fn empty_sessions_and_files_are_cards_with_the_next_step() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Sessions));
    assert!(app.query_by_label(crate::app::SESSIONS_EMPTY).is_some());
    assert!(app.query_by_label(crate::app::SESSIONS_HOW).is_some());
    app.get_by_label("Pair a phone").click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::Pair.purpose()).is_some(),
        "Pair opened"
    );

    go_to(&app, Tab::Files);
    app.run_steps(3);
    assert!(
        app.query_by_label(crate::app::FILES_NONE_RECEIVED)
            .is_some()
    );
    assert!(app.query_by_label_contains("shows here").is_some());
}

/// UX 4: with a phone paired, Sessions' empty card has no Pair button.
#[test]
fn sessions_with_a_phone_paired_offer_no_pairing() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let app = fixture.open(Some(Tab::Sessions));
    assert!(app.query_by_label(crate::app::SESSIONS_EMPTY).is_some());
    assert!(app.query_by_label("Pair a phone").is_none());
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

/// The Overview, Devices, Settings and Keyboard and mouse at 720×480 in both themes, compared
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
        // The Keyboard and mouse page with a computer beside this one,
        // drawn last: its computer would show on the pages above.
        keyboard_mouse_computers(fixture.state());
        for (theme, shade) in [(egui::Theme::Light, "light"), (egui::Theme::Dark, "dark")] {
            let state = fixture.state().to_path_buf();
            let mut app = Harness::builder()
                .with_size(SIZE)
                .with_theme(theme)
                .wgpu()
                .build_eframe(move |cc| {
                    crate::theme::install(&cc.egui_ctx);
                    PocketdeskApp::new(state, None, Some(Tab::KeyboardMouse))
                });
            app.run_steps(3);
            let name = format!("keyboard_mouse_{shade}");
            if let Err(e) = app.try_snapshot_options(name, &options) {
                failed.push(e.to_string());
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

/// AM5: with no log file yet, the page says how the server writes one, in
/// words: no Markdown backticks, which a label shows as they are.
#[test]
fn no_log_file_says_so_in_plain_words() {
    let fixture = Fixture::new();
    let app = fixture.open(Some(Tab::Logs));
    assert!(app.query_by_label_contains("No log file yet").is_some());
    assert!(
        app.query_by_label_contains("pocketdesk start --background")
            .is_some()
    );
    assert!(
        app.query_by_label_contains("`").is_none(),
        "a backtick shown as it is"
    );
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

/// UX 2: Settings' Keyboard and mouse section is a pointer to the page,
/// found by its title or one of its options, whose button opens the page.
#[test]
fn the_keyboard_and_mouse_section_points_to_its_page() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::Settings));
    assert!(
        app.query_by_label(crate::settings_view::INPUT_ELSEWHERE)
            .is_none(),
        "closed until looked for"
    );
    for words in ["keyboard and mouse", "game runs"] {
        let mut app = fixture.open(Some(Tab::Settings));
        find(&app).focus();
        find(&app).type_text(words);
        app.run_steps(3);
        assert!(
            app.query_by_label(crate::settings_view::INPUT_ELSEWHERE)
                .is_some(),
            "{words}"
        );
    }
    find(&app).focus();
    find(&app).type_text("keyboard and mouse");
    app.run_steps(3);
    assert!(app.query_by_label("Control port (TCP)").is_none());
    app.get_by_label(crate::settings_view::OPEN_KEYBOARD_MOUSE)
        .click();
    app.run_steps(3);
    assert!(
        app.query_by_label(Tab::KeyboardMouse.purpose()).is_some(),
        "the page opened"
    );
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
    assert!(app.query_by_label(crate::app::STOPPED_BAR).is_some());
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

/// K2b: "Not from this app" on a notification's Activity row mutes it here,
/// and Settings lists it with Unmute.
#[test]
fn an_app_is_muted_from_activity_and_unmuted_in_settings() {
    use pocketdesk_core::local::events::{Event, EventsDetail, append};
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let note = Event::new(
        "notification",
        Some(PHONE),
        now,
        serde_json::json!({ "app": "Gmail" }),
        serde_json::json!({}),
        EventsDetail::Metadata,
    );
    append(fixture.state(), &note).expect("logged");
    let mut app = fixture.open(Some(Tab::Activity));
    app.run_steps(3);
    app.get_by_label("Not from this app").click();
    app.run_steps(3);
    assert!(app.query_by_label("muted here").is_some());
    assert!(app.query_by_label("Not from this app").is_none());
    assert!(pocketdesk_core::local::muted_apps::is_muted(
        &pocketdesk_core::local::muted_apps::read(fixture.state()),
        "gmail"
    ));

    go_to(&app, Tab::Settings);
    app.run_steps(3);
    find(&app).focus();
    find(&app).type_text("muted");
    app.run_steps(3);
    assert!(app.query_by_label("Gmail").is_some(), "listed");
    app.get_by_label("Unmute").click();
    app.run_steps(3);
    assert!(pocketdesk_core::local::muted_apps::read(fixture.state()).is_empty());
}

/// K2b review: Settings says when the muted list cannot be read.
#[test]
fn a_broken_muted_list_is_said_in_settings() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture
            .state()
            .join(pocketdesk_core::local::paths::MUTED_APPS_FILE),
        "not json",
    )
    .expect("file");
    let mut app = fixture.open(Some(Tab::Settings));
    find(&app).focus();
    find(&app).type_text("muted");
    app.run_steps(3);
    assert!(app.query_by_label_contains("could not be read").is_some());
}

/// K2a: the Overview pauses for an hour, says so, and resumes.
#[test]
fn the_overview_pauses_and_resumes() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    // AU1: it says what it holds, not only on hover.
    app.get_by_label("Pause notifications").click();
    app.run_steps(3);
    app.get_by_label("For an hour").click();
    app.run_steps(3);
    assert!(app.query_by_label_contains("Paused,").is_some());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    assert!(pocketdesk_core::local::pause::current(fixture.state(), now).is_some());
    app.get_by_label("Resume").click();
    app.run_steps(3);
    assert!(app.query_by_label("Pause notifications").is_some());
    assert!(pocketdesk_core::local::pause::current(fixture.state(), now).is_none());
}

/// K2a review: a pause that cannot be read is said so on the Overview,
/// not shown as "not paused" with a Pause that fails.
#[test]
fn the_overview_says_a_broken_pause() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture
            .state()
            .join(pocketdesk_core::local::paths::PAUSE_FILE),
        "x",
    )
    .expect("pause");
    let app = fixture.open(None);
    assert!(app.query_by_label_contains("could not be read").is_some());
    assert!(app.query_by_label(crate::tray_menu::PAUSE_LABEL).is_none());
}

/// K3: a notification on the Notifications page shows its app's icon, as
/// the phone sent it, named for the app.
#[test]
fn a_notification_shows_its_app_s_icon() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    pocketdesk_core::local::phone_notes::keep(
        fixture.state(),
        pocketdesk_core::local::phone_notes::PhoneNote {
            device_id: PHONE.to_string(),
            key: "0|chat|1".to_string(),
            app: "Chat".to_string(),
            title: "Ann".to_string(),
            text: "hi".to_string(),
            at_unix: 1,
            actions: vec!["Mark read".to_string()],
            reply: true,
            open: false,
            messages: Vec::new(),
        },
    )
    .expect("note");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 128, 255, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png");
    pocketdesk_core::local::notification_icons::keep_icon(
        fixture.state(),
        PHONE,
        "Chat",
        png.get_ref(),
    )
    .expect("icon");
    let app = fixture.open(Some(Tab::Notifications));
    assert!(app.query_by_label("Chat").is_some(), "the icon, named");
}

/// L1: Clear on the Notifications page asks the phone to clear it.
#[test]
fn the_notifications_page_clears_on_the_phone() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    pocketdesk_core::local::phone_notes::keep(
        fixture.state(),
        pocketdesk_core::local::phone_notes::PhoneNote {
            device_id: PHONE.to_string(),
            key: "0|com.chat|9|null|10".to_string(),
            app: "Chat".to_string(),
            title: "Ann".to_string(),
            text: "hi".to_string(),
            at_unix: 1,
            actions: vec!["Mark read".to_string()],
            reply: false,
            open: false,
            messages: Vec::new(),
        },
    )
    .expect("note");
    let mut app = fixture.open(Some(Tab::Notifications));
    app.get_by_label("Clear").click();
    app.run_steps(3);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let cleared: Vec<_> = pocketdesk_core::local::note_clears::take_all(fixture.state(), now)
        .into_iter()
        .map(|c| (c.device_id, c.key))
        .collect();
    assert_eq!(
        cleared,
        vec![(PHONE.to_string(), "0|com.chat|9|null|10".to_string())]
    );
}

/// N3: Open on the phone asks the phone to open it, and shows only where
/// the phone offers it.
#[test]
fn the_notifications_page_opens_on_the_phone() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let note = |key: &str, open: bool| pocketdesk_core::local::phone_notes::PhoneNote {
        device_id: PHONE.to_string(),
        key: key.to_string(),
        app: "Chat".to_string(),
        title: format!("Ann {key}"),
        text: "hi".to_string(),
        at_unix: 1,
        actions: vec!["Mark read".to_string()],
        reply: false,
        open,
        messages: Vec::new(),
    };
    pocketdesk_core::local::phone_notes::keep(fixture.state(), note("0|com.chat|8|null|10", true))
        .expect("note");
    pocketdesk_core::local::phone_notes::keep(fixture.state(), note("0|com.chat|7|null|10", false))
        .expect("note");
    let mut app = fixture.open(Some(Tab::Notifications));
    assert_eq!(
        app.query_all_by_label("Open on the phone").count(),
        1,
        "only where offered"
    );
    app.get_by_label("Open on the phone").click();
    app.run_steps(3);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
        + 2;
    let asked: Vec<_> = pocketdesk_core::local::phone_notes::take_all(fixture.state(), now)
        .into_iter()
        .map(|a| (a.key, a.answer))
        .collect();
    assert_eq!(
        asked,
        vec![(
            "0|com.chat|8|null|10".to_string(),
            pocketdesk_core::local::phone_notes::Answer::Open
        )]
    );
}

/// M8: a tel: link with several phones is asked on the Overview, and the
/// phone chosen dials it.
#[test]
fn the_overview_asks_which_phone_calls_a_link() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    pair_phone(
        fixture.state(),
        "00000000-0000-4000-8000-0000000000c5",
        "Other Phone",
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    pocketdesk_core::local::tel_ask::put(fixture.state(), "+15550100", now).expect("asked");
    let mut app = fixture.open(Some(Tab::Status));
    assert!(app.query_by_label("Call +15550100 on:").is_some());
    app.get_by_role_and_label(egui::accesskit::Role::Button, "Test Phone")
        .click();
    app.run_steps(3);
    // Read after the click: the request carries the clock's second then.
    let now = now + 2;
    let dialled: Vec<_> = pocketdesk_core::local::dials::take_all(fixture.state(), now)
        .into_iter()
        .map(|d| (d.device_id, d.number))
        .collect();
    assert_eq!(dialled, vec![(PHONE.to_string(), "+15550100".to_string())]);
    assert_eq!(
        pocketdesk_core::local::tel_ask::read(fixture.state(), now),
        None,
        "answered"
    );
    assert!(app.query_by_label("Call +15550100 on:").is_none());
}

/// M8: Call back on a ringing call's note dials its number.
#[test]
fn the_notifications_page_calls_back() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    pocketdesk_core::local::phone_notes::keep(
        fixture.state(),
        pocketdesk_core::local::phone_notes::PhoneNote {
            device_id: PHONE.to_string(),
            key: "pocketdesk:call:3".to_string(),
            app: "Pocketdesk".to_string(),
            title: "Call from Ann (+15550100)".to_string(),
            text: "Ringing on the phone".to_string(),
            at_unix: 1,
            actions: vec!["Silence".to_string()],
            reply: false,
            open: false,
            messages: Vec::new(),
        },
    )
    .expect("note");
    let mut app = fixture.open(Some(Tab::Notifications));
    app.get_by_label("Call back").click();
    app.run_steps(3);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let dialled: Vec<_> = pocketdesk_core::local::dials::take_all(fixture.state(), now)
        .into_iter()
        .map(|d| (d.device_id, d.number))
        .collect();
    assert_eq!(dialled, vec![(PHONE.to_string(), "+15550100".to_string())]);
}

/// L4: Call beside a conversation's number asks its phone to dial it.
#[test]
fn the_messages_page_calls_a_number() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    pocketdesk_core::local::sms::record(
        fixture.state(),
        pocketdesk_core::local::sms::Message {
            device_id: PHONE.to_string(),
            address: "+15550100100".to_string(),
            body: "call me".to_string(),
            at_unix: 1,
            direction: pocketdesk_core::local::sms::Direction::Received,
            reply: None,
            name: None,
        },
        100,
    )
    .expect("text");
    let mut app = fixture.open(Some(Tab::Messages));
    app.get_by_label_contains("+15550100100").click();
    app.run_steps(3);
    app.get_by_label("Call").click();
    app.run_steps(3);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let calls: Vec<_> = pocketdesk_core::local::dials::take_all(fixture.state(), now)
        .into_iter()
        .map(|c| (c.device_id, c.number))
        .collect();
    assert_eq!(calls, vec![(PHONE.to_string(), "+15550100100".to_string())]);
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

/// UX 2: the Keyboard and mouse page has its options, off while sharing
/// is with the reason beside them; on, one changed is saved at once.
#[test]
fn the_keyboard_and_mouse_options_are_on_its_page() {
    use egui_kittest::kittest::NodeT;
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::KeyboardMouse));
    app.run_steps(3);
    let clipboard = crate::settings_view::name("shared_input_clipboard");
    assert!(app.get_by_label(clipboard).accesskit_node().is_disabled());
    assert!(
        app.query_by_label(crate::input_options::SHARING_OFF)
            .is_some()
    );
    app.get_by_label(crate::arrange_view::SHARE_SWITCH).click();
    app.run_steps(3);
    assert!(!app.get_by_label(clipboard).accesskit_node().is_disabled());
    assert!(
        app.query_by_label(crate::input_options::SHARING_OFF)
            .is_none()
    );
    let was = load_config(&fixture).shared_input_clipboard;
    // Below the fold: by the keyboard, which scrolls to it.
    app.get_by_label(clipboard).focus();
    app.run_steps(2);
    app.key_press(egui::Key::Space);
    app.run_steps(1);
    assert_eq!(
        load_config(&fixture).shared_input_clipboard,
        !was,
        "saved as it changed"
    );
    // The box itself shows it on the next frame, not after the next read.
    assert_eq!(ticked(&app, clipboard), !was, "the box keeps the click");
}

/// Whether the checkbox labelled `label` is ticked.
fn ticked(app: &Harness<'static, PocketdeskApp>, label: &str) -> bool {
    use egui_kittest::kittest::NodeT;
    app.get_by_label(label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

/// The config the fixture's app saves.
fn load_config(fixture: &Fixture) -> pocketdesk_core::local::config::Config {
    pocketdesk_core::local::config::Config::load_from(
        &fixture
            .config()
            .join(pocketdesk_core::local::paths::CONFIG_FILE),
    )
    .expect("the config")
}

/// UX 2: the Keyboard and mouse page starts with its setup, each step
/// done or not with its fix; Pair a computer opens Pair on Computer.
#[test]
fn the_keyboard_and_mouse_page_lists_its_setup() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::KeyboardMouse));
    app.run_steps(3);
    assert!(app.query_by_label(crate::arrange_view::SETUP).is_some());
    for step in crate::arrange_view::checklist(Default::default()) {
        assert!(app.query_by_label(step.label).is_some(), "{}", step.label);
    }
    // Setup's is the first; Identify's fix may carry the same words
    // (PR #555 review m6).
    app.get_all_by_label(crate::app::PAIR_A_COMPUTER)
        .next()
        .expect("Setup's Pair a computer")
        .click();
    let opened = (0..50).any(|_| {
        app.run_steps(2);
        let found = app.query_by_label("From the other computer").is_some()
            && app.query_by_label(crate::pair_view::COPY_LINK).is_some();
        if !found {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        found
    });
    assert!(opened, "Pair opened on Computer with both halves");
}

/// PR #555 review m6: each option the page draws is saved as it changes,
/// the two text boxes as they are left.
#[test]
fn every_keyboard_and_mouse_option_is_saved() {
    let fixture = Fixture::new();
    let mut app = fixture.open(Some(Tab::KeyboardMouse));
    app.run_steps(3);
    app.get_by_label(crate::arrange_view::SHARE_SWITCH).click();
    app.run_steps(3);
    let read = |key: &str| -> bool {
        let c = load_config(&fixture);
        match key {
            "shared_input_drive" => c.shared_input_drive,
            "shared_input_game_lock" => c.shared_input_game_lock,
            "shared_input_clipboard" => c.shared_input_clipboard,
            "shared_input_files" => c.shared_input_files,
            "shared_input_sound" => c.shared_input_sound,
            "shared_input_low_latency" => c.shared_input_low_latency,
            other => panic!("{other}"),
        }
    };
    for key in [
        "shared_input_drive",
        "shared_input_game_lock",
        "shared_input_clipboard",
        "shared_input_files",
        "shared_input_sound",
        "shared_input_low_latency",
    ] {
        if !crate::settings_view::drawn_here(key) {
            continue;
        }
        let label = crate::settings_view::name(key);
        let was = read(key);
        app.get_by_label(label).focus();
        app.run_steps(2);
        app.key_press(egui::Key::Space);
        app.run_steps(1);
        assert_eq!(read(key), !was, "{key} saved");
        assert_eq!(ticked(&app, label), !was, "{key}'s box");
    }
    // The text boxes: the switch key, then (by Tab, which scrolls to it)
    // the programs'.
    app.get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the switch key's box")
        .focus();
    app.run_steps(2);
    app.get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the switch key's box")
        .type_text("x");
    app.run_steps(2);
    app.key_press(egui::Key::Tab);
    app.run_steps(3);
    assert_eq!(
        load_config(&fixture).shared_input_hotkey,
        "ctrl+alt+rightx",
        "saved as it was left"
    );
    app.get_all_by_role(egui::accesskit::Role::TextInput)
        .last()
        .expect("the programs' box")
        .type_text("blender");
    app.run_steps(2);
    app.key_press(egui::Key::Tab);
    app.run_steps(3);
    assert_eq!(
        load_config(&fixture).shared_input_lock_programs,
        ["blender"],
        "saved as it was left"
    );
}

/// UX 1: Identify, off, says why beside it: sharing first, with its fix.
#[test]
fn identify_says_why_it_is_off() {
    use egui_kittest::kittest::NodeT;
    let fixture = Fixture::new();
    let app = fixture.open(Some(Tab::KeyboardMouse));
    assert!(app.get_by_label("Identify").accesskit_node().is_disabled());
    assert!(
        app.query_by_label_contains("Identify needs sharing on")
            .is_some()
    );
    assert!(app.query_by_label("Turn sharing on").is_some());
}

/// UX 1: an away phone's greyed actions say why, by its name, in a line
/// and not only on hover.
#[test]
fn an_away_phones_actions_say_why_they_are_off() {
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    let app = fixture.open(None);
    assert!(
        app.query_by_label(&crate::app::away_words("Test Phone", false))
            .is_some()
    );
}

/// UX 1: a stopped server is said once, in a bar at the top of every page,
/// with its Start; not by each page in its own words.
#[test]
fn a_stopped_server_is_said_at_the_top_of_every_page() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    for page in Tab::ALL {
        go_to(&app, page);
        app.run_steps(3);
        assert!(
            app.query_by_label(crate::app::STOPPED_BAR).is_some(),
            "{page:?}"
        );
        // The bar's Start; never pressed here, as it would start a server.
        assert!(app.query_by_label("Start server").is_some(), "{page:?}");
    }
    go_to(&app, Tab::KeyboardMouse);
    app.run_steps(3);
    assert!(
        app.query_by_label_contains("start it on the Overview")
            .is_none()
    );
}

/// UX 1: Diagnostics says nothing of the fonts while they are still looked
/// for (a stray line before any check ran).
#[test]
fn diagnostics_say_nothing_of_fonts_still_looked_for() {
    let fixture = Fixture::new();
    let app = fixture.open(Some(Tab::Diagnostics));
    assert!(
        app.query_by_label_contains("Fonts for other scripts")
            .is_none()
    );
}

/// A computer this one paired with, as the computers file keeps it.
fn add_computer(state: &Path, name: &str) {
    pocketdesk_core::local::computers::update(state, |file| {
        file.computers
            .push(pocketdesk_core::local::computers::PeerComputer {
                device_id: "00000000-0000-4000-8000-0000000000ba".to_string(),
                name: name.to_string(),
                addresses: Vec::new(),
                control_port: 1716,
                stream_port: 4716,
                fingerprint: String::new(),
                endpoint: None,
                paired_unix: 0,
                link: String::new(),
                link_unix: 0,
                lets_drive: false,
                mine: false,
                wake: None,
                viewed_unix: 0,
            });
    })
    .expect("a computers file");
}

/// UX 3: the Overview's Start is in its status card, between the words
/// that the server is stopped and the pause, and the bar at the top is not
/// drawn there (one Start, not two); other pages keep the bar. Restart
/// takes its place with the server running, which no test here starts.
#[test]
fn the_overviews_start_is_in_its_status_card() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    let words = app.get_by_label(crate::app::STOPPED_BAR).rect();
    // One Start on the page: get_by_label fails on two.
    let start = app.get_by_label(crate::app::START_SERVER).rect();
    let pause = app.get_by_label(crate::tray_menu::PAUSE_LABEL).rect();
    assert!(words.bottom() <= start.top(), "{words:?} above {start:?}");
    assert!(start.bottom() <= pause.top(), "{start:?} above {pause:?}");
    go_to(&app, Tab::Devices);
    app.run_steps(3);
    assert!(
        app.query_by_label(crate::app::START_SERVER).is_some(),
        "the bar"
    );
}

/// UX 3: the ports and the pairing rule are in a Details fold, closed
/// until opened.
#[test]
fn the_overviews_details_start_closed() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    assert!(app.query_by_label("Control port").is_none(), "closed");
    assert!(app.query_by_label("Pairing required").is_none(), "closed");
    app.get_by_label(crate::app::DETAILS).click();
    app.run_steps(3);
    assert!(app.query_by_label("Control port").is_some());
    assert!(app.query_by_label("Streaming port").is_some());
    assert!(app.query_by_label("Pairing required").is_some());
}

/// UX 3: the first-run steps follow the facts: none done at first (no
/// server here, nothing paired); a computer paired ticks its step and
/// lists the computer; a phone paired ends the steps.
#[test]
fn the_first_run_steps_follow_the_facts() {
    let label = |i: usize, done: bool| {
        let (words, _) = crate::app::first_steps(false, 0, 0)[i];
        crate::app::step_label(i, words, done)
    };
    let fixture = Fixture::new();
    let app = fixture.open(None);
    for i in 0..3 {
        assert!(app.query_by_label(&label(i, false)).is_some(), "step {i}");
        assert!(app.query_by_label(&label(i, true)).is_none(), "step {i}");
    }
    assert!(app.query_by_label("Pair a phone").is_some());
    assert!(app.query_by_label(crate::app::PAIR_A_COMPUTER).is_some());
    drop(app);

    add_computer(fixture.state(), "Test Desk");
    let app = fixture.open(None);
    assert!(app.query_by_label(&label(0, false)).is_some());
    assert!(app.query_by_label(&label(1, false)).is_some());
    assert!(app.query_by_label(&label(2, true)).is_some(), "ticked");
    assert!(app.query_by_label(crate::app::YOUR_DEVICES).is_some());
    assert!(app.query_by_label("Test Desk").is_some(), "its card");
    assert!(app.query_by_label("Show its screen").is_some());
    drop(app);

    pair_phone(fixture.state(), PHONE, "Test Phone");
    let app = fixture.open(None);
    for i in 0..3 {
        assert!(app.query_by_label(&label(i, false)).is_none(), "step {i}");
        assert!(app.query_by_label(&label(i, true)).is_none(), "step {i}");
    }
    assert!(app.query_by_label("Test Phone").is_some());
}

/// UX 3: Hide these steps hides them, and the page then says nothing is
/// paired, with its button.
#[test]
fn the_first_run_steps_hide() {
    let fixture = Fixture::new();
    let mut app = fixture.open(None);
    app.get_by_label(crate::app::HIDE_STEPS).click();
    app.run_steps(3);
    assert!(app.query_by_label(crate::app::HIDE_STEPS).is_none());
    assert!(
        app.query_by_label_contains("Nothing is paired yet")
            .is_some()
    );
    assert!(app.query_by_label("Pair a phone").is_some());
}

/// UX 3: each group's switch on Devices saves its own switches and no
/// others: the groups spelled out here, not taken from the app's helpers.
/// Every other switch and "Ask before each session" stay as they were, a
/// part-on group shows as mixed, and Automation never turns Run commands
/// on but does turn it off (PR #558 review).
#[test]
fn each_permission_group_switch_saves_its_permissions() {
    use egui_kittest::kittest::NodeT;
    use pocketdesk_core::protocol::permissions::{Capability as C, DevicePermissions};
    // AwayLock can be set on Windows and Linux, where these tests run.
    let groups: [(&str, Vec<C>); 5] = [
        ("Control", vec![C::Input, C::SharedInput, C::Power, C::Peek]),
        (
            "Screen, sound and media",
            vec![C::Cast, C::Audio, C::Mic, C::Media, C::PhoneSound],
        ),
        (
            "Files and clipboard",
            vec![C::Clipboard, C::Files, C::Browse, C::Links, C::Notes],
        ),
        (
            "Phone on this computer",
            vec![
                C::Notifications,
                C::Texts,
                C::Dial,
                C::PhoneActions,
                C::Ring,
                C::Mute,
            ],
        ),
        (
            "Automation",
            vec![C::Command, C::Ask, C::Tasks, C::AwayLock],
        ),
    ];
    let fixture = Fixture::new();
    pair_phone(fixture.state(), PHONE, "Test Phone");
    // Not the defaults: Run commands on, one switch off in each group, and
    // each session asked for.
    let mut start = DevicePermissions::default();
    start.set(C::Exec, true);
    for c in [C::Input, C::Audio, C::Clipboard, C::Texts, C::Tasks] {
        start.set(c, false);
    }
    // And one on in each, so each shows part-on.
    for c in [C::Power, C::Cast, C::Files, C::Notifications, C::Command] {
        start.set(c, true);
    }
    start.ask_first = true;
    TrustStore::update(fixture.state().join(TRUST_FILE), |store| {
        store.set_permissions(PHONE, start.clone())
    })
    .expect("set");
    let read = |state: &Path| {
        TrustStore::read(state.join(TRUST_FILE))
            .expect("the trust store")
            .permissions(PHONE)
    };
    let mut app = fixture.open(Some(Tab::Devices));
    app.get_by_label("What Test Phone may do").click();
    app.run_steps(3);
    let click = |app: &mut Harness<'static, PocketdeskApp>, label: &str| {
        app.get_by_label(label).scroll_to_me();
        app.run_steps(3);
        app.get_by_label(label).click();
        app.run_steps(3);
    };
    // Every switch outside `members` (and ask_first) as `before` had it.
    let others_kept = |before: &DevicePermissions, members: &[C], what: &str| {
        let now = read(fixture.state());
        assert_eq!(now.ask_first, before.ask_first, "{what}: ask_first");
        for c in C::ALL {
            if !members.contains(&c) {
                assert_eq!(now.allows(c), before.allows(c), "{what}: {c:?} kept");
            }
        }
    };
    for (group, members) in &groups {
        // Part-on at first: one of each group was turned off above.
        assert_eq!(
            app.get_by_label(group).accesskit_node().toggled(),
            Some(egui::accesskit::Toggled::Mixed),
            "{group} shows part-on"
        );
        let before = read(fixture.state());
        click(&mut app, group);
        for c in members {
            assert!(read(fixture.state()).allows(*c), "{group}: {c:?} on");
        }
        // Run commands is kept as it was: on.
        let mut kept = members.clone();
        others_kept(&before, &kept, group);
        assert!(read(fixture.state()).allows(C::Exec), "{group}: Exec kept");

        let before = read(fixture.state());
        click(&mut app, group);
        if *group == "Automation" {
            // Off takes Run commands with the rest.
            kept.push(C::Exec);
            assert!(!read(fixture.state()).allows(C::Exec), "Exec off");
        }
        for c in members {
            assert!(!read(fixture.state()).allows(*c), "{group}: {c:?} off");
        }
        others_kept(&before, &kept, group);
    }
}
