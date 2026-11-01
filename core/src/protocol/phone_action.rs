//! What the computer asks the phone to do to itself (O2): take a screenshot,
//! or lock its screen. The phone does it through its control service (C8)
//! with its own switch on; the screenshot reaches the computer as T10h's
//! new screenshots do. P2 adds listing the phone's apps and opening one.

use serde_json::json;

/// Computer → phone: `{action, app?}`. Under the Phone actions permission.
pub const PHONE_ACTION: &str = "kdeconnect.pocketdesk.phone_action";

/// Phone → computer (P2): `{apps: [{id, name}]}`, the phone's apps that
/// open, answering `apps`. Under the Phone actions permission.
pub const PHONE_APPS: &str = "kdeconnect.pocketdesk.phone_apps";

/// Phone → computer (Apps part 1): `{icons: [{id, v, png}]}`, the icons of
/// apps the computer did not have at that version, `png` base64, sent in
/// batches after the list. Under the Phone actions permission.
pub const PHONE_APP_ICONS: &str = "kdeconnect.pocketdesk.phone_app_icons";

/// Phone → computer (Apps part 1): `{about, app?, result}`, how an `apps`
/// or `open` ask went, given only to the phone's own computer. Under the
/// Phone actions permission.
pub const PHONE_APP_OPENED: &str = "kdeconnect.pocketdesk.phone_app_opened";

/// The most apps a list holds.
pub const MAX_APPS: usize = 500;

/// The most icons one [`PHONE_APP_ICONS`] packet holds.
pub const MAX_ICONS_PER_PACKET: usize = 40;

/// Phone → computer (P10): `{contacts: [{name, numbers}]}`, the phone's
/// contacts, names and numbers only, answering `contacts`. Under the Texts
/// permission, as the texts they go with are.
pub const PHONE_CONTACTS: &str = "kdeconnect.pocketdesk.phone_contacts";

/// The most contacts a list holds.
pub const MAX_CONTACTS: usize = 5_000;

/// The most numbers one contact keeps.
pub const MAX_NUMBERS: usize = 5;

/// One of the phone's contacts (P10): a name and its numbers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhoneContact {
    pub name: String,
    pub numbers: Vec<String>,
}

/// The contacts in a [`PHONE_CONTACTS`] body: each with a name and a
/// number a call takes (L4's rule), names cleaned to one line, numbers kept
/// as the phone wrote them but each once, at most [`MAX_NUMBERS`] each and
/// [`MAX_CONTACTS`] in all.
pub fn parse_contacts(body: &serde_json::Value) -> Vec<PhoneContact> {
    let mut contacts = Vec::new();
    for contact in body
        .get("contacts")
        .and_then(|c| c.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let name = crate::protocol::phone_media::clean_line(
            contact.get("name").and_then(|v| v.as_str()).unwrap_or(""),
        );
        if name.is_empty() {
            continue;
        }
        let mut numbers: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for number in contact
            .get("numbers")
            .and_then(|n| n.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|n| n.as_str())
        {
            let number = number.trim();
            let Some(clean) = crate::protocol::dial::clean_number(number) else {
                continue;
            };
            if seen.contains(&clean) {
                continue;
            }
            seen.push(clean);
            numbers.push(number.to_string());
            if numbers.len() == MAX_NUMBERS {
                break;
            }
        }
        if numbers.is_empty() {
            continue;
        }
        contacts.push(PhoneContact { name, numbers });
        if contacts.len() == MAX_CONTACTS {
            break;
        }
    }
    contacts
}

/// The longest app id (an Android package name) taken.
pub const MAX_APP_ID: usize = 255;

/// One of the phone's apps (P2): its package name and its label, and (Apps
/// part 1) its version code, which names the icon the computer keeps for
/// it (0 from a phone that does not say).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhoneApp {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: u64,
}

/// Whether `id` is an Android package name: dotted words of letters,
/// digits and underscores, each starting with a letter.
pub fn valid_app_id(id: &str) -> bool {
    id.len() <= MAX_APP_ID
        && id.contains('.')
        && id.split('.').all(|part| {
            part.starts_with(|c: char| c.is_ascii_alphabetic())
                && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The package an Android notification key names, if it is a valid one:
/// the key the phone sends as a notification's id is Android's own,
/// `user|package|id|tag|uid` (the user a number). Anything else, a call's
/// note or another system's key, names no app.
pub fn app_of_notification_key(key: &str) -> Option<String> {
    let mut parts = key.split('|');
    let user = parts.next()?;
    let app = parts.next()?;
    // An id follows the package in every key Android makes.
    parts.next()?;
    let user_ok = !user.is_empty() && user.bytes().all(|b| b.is_ascii_digit());
    (user_ok && valid_app_id(app)).then(|| app.to_string())
}

/// The apps in a [`PHONE_APPS`] body: those with a valid id, their names
/// cleaned (one line, the media's limit), each once, at most [`MAX_APPS`].
pub fn parse_apps(body: &serde_json::Value) -> Vec<PhoneApp> {
    let mut apps: Vec<PhoneApp> = Vec::new();
    for app in body
        .get("apps")
        .and_then(|a| a.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(id) = app
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|id| valid_app_id(id))
        else {
            continue;
        };
        if apps.iter().any(|a| a.id == id) {
            continue;
        }
        let name = crate::protocol::phone_media::clean_line(
            app.get("name").and_then(|v| v.as_str()).unwrap_or(id),
        );
        apps.push(PhoneApp {
            id: id.to_string(),
            name: if name.is_empty() {
                id.to_string()
            } else {
                name
            },
            version: app.get("v").and_then(|v| v.as_u64()).unwrap_or(0),
        });
        if apps.len() == MAX_APPS {
            break;
        }
    }
    apps
}

/// The icons the computer already has, from an `apps` ask's `have`
/// (`{id: version}`): only valid ids, at most [`MAX_APPS`].
pub fn parse_have(body: &serde_json::Value) -> std::collections::BTreeMap<String, u64> {
    body.get("have")
        .and_then(|h| h.as_object())
        .map(|have| {
            have.iter()
                .filter(|(id, _)| valid_app_id(id))
                .filter_map(|(id, v)| Some((id.clone(), v.as_u64()?)))
                .take(MAX_APPS)
                .collect()
        })
        .unwrap_or_default()
}

/// The packet's body for an `apps` ask: the icons already kept (an app's
/// id and the version of its icon), so the phone sends only the rest.
pub fn apps_body(have: &std::collections::BTreeMap<String, u64>) -> serde_json::Value {
    if have.is_empty() {
        return json!({ "action": PhoneAction::Apps.as_str() });
    }
    json!({ "action": PhoneAction::Apps.as_str(), "have": have })
}

/// The packet's body for an `open` ask; `screen` asks that the phone's
/// screen be shown here too (one consent for the connection, not one per
/// app), and is left out when not.
pub fn open_body(app: &str, screen: bool) -> serde_json::Value {
    let mut open = body(PhoneAction::Open, Some(app));
    if screen {
        open["screen"] = json!(true);
    }
    open
}

/// What came of an ask the phone answers with [`PHONE_APP_OPENED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppResult {
    /// The app was started.
    Opened,
    /// The phone's screen is not shown here yet: its owner is asked once,
    /// and the app opens after.
    NeedsScreen,
    /// Pocketdesk's control service on the phone is off.
    ControlOff,
    /// The phone is locked.
    Locked,
    /// Hands-free: the phone has a secure lock (PIN, pattern, password or
    /// biometric) that is on now. It is never bypassed from here: unlock the
    /// phone, or let Android keep it unlocked near this computer.
    LockedSecure,
    /// Hands-free: the phone could not accept its own screen-sharing
    /// question (the option is off, Android hid the button from the control
    /// service, or the screen names it differently), so one notification on
    /// the phone waits for one tap, and the app opens after it.
    NeedsTap,
    /// The phone's screen is off.
    Asleep,
    /// "Let the computer open my apps" is off.
    Off,
    /// An app the phone did not list for the computer.
    NotListed,
    /// The app is gone or has nothing to open.
    NotFound,
    /// More asks than the phone does a minute.
    TooMany,
    /// Android refused it.
    Failed,
}

impl AppResult {
    /// Every result.
    pub const ALL: [AppResult; 12] = [
        AppResult::Opened,
        AppResult::NeedsScreen,
        AppResult::ControlOff,
        AppResult::Locked,
        AppResult::LockedSecure,
        AppResult::NeedsTap,
        AppResult::Asleep,
        AppResult::Off,
        AppResult::NotListed,
        AppResult::NotFound,
        AppResult::TooMany,
        AppResult::Failed,
    ];

    /// The word the packet carries.
    pub fn as_str(self) -> &'static str {
        match self {
            AppResult::Opened => "opened",
            AppResult::NeedsScreen => "needs_screen",
            AppResult::ControlOff => "control_off",
            AppResult::Locked => "locked",
            AppResult::LockedSecure => "locked_secure",
            AppResult::NeedsTap => "needs_tap",
            AppResult::Asleep => "asleep",
            AppResult::Off => "off",
            AppResult::NotListed => "not_listed",
            AppResult::NotFound => "not_found",
            AppResult::TooMany => "too_many",
            AppResult::Failed => "failed",
        }
    }

    /// The result `word` names, or `None`.
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == word)
    }
}

/// How an ask went, as a phone says it ([`PHONE_APP_OPENED`]): which ask
/// (`apps` or `open`), the app an open named, and the result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppReport {
    pub about: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub result: AppResult,
}

impl AppReport {
    /// The report in `body`: an `apps` or `open` ask with a result this
    /// computer knows, and for an open a valid app id.
    pub fn from_body(body: &serde_json::Value) -> Option<AppReport> {
        let about = body.get("about")?.as_str()?;
        let result = AppResult::parse(body.get("result")?.as_str()?)?;
        let app = body
            .get("app")
            .and_then(|a| a.as_str())
            .filter(|a| valid_app_id(a))
            .map(str::to_string);
        match (about, &app) {
            ("open", Some(_)) | ("apps", None) => {}
            _ => return None,
        }
        Some(AppReport {
            about: about.to_string(),
            app,
            result,
        })
    }

    /// The packet's body.
    pub fn to_body(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// What the phone is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneAction {
    /// Take a screenshot, as the phone's own buttons do.
    Screenshot,
    /// Lock its screen.
    Lock,
    /// List the apps that open (P2).
    Apps,
    /// Open one app, its id beside the action (P2).
    Open,
    /// List the contacts, names and numbers only (P10).
    Contacts,
}

impl PhoneAction {
    pub const ALL: [PhoneAction; 5] = [
        PhoneAction::Screenshot,
        PhoneAction::Lock,
        PhoneAction::Apps,
        PhoneAction::Open,
        PhoneAction::Contacts,
    ];

    /// The word the packet and the request files carry.
    pub fn as_str(self) -> &'static str {
        match self {
            PhoneAction::Screenshot => "screenshot",
            PhoneAction::Lock => "lock",
            PhoneAction::Apps => "apps",
            PhoneAction::Open => "open",
            PhoneAction::Contacts => "contacts",
        }
    }

    /// The action `word` names, or `None`.
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == word)
    }
}

/// The packet's body for `action`, with the app `open` opens.
pub fn body(action: PhoneAction, app: Option<&str>) -> serde_json::Value {
    match app {
        Some(app) => json!({ "action": action.as_str(), "app": app }),
        None => json!({ "action": action.as_str() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O2: each action reads back from its word, and nothing else does.
    #[test]
    fn an_action_reads_back_from_its_word() {
        for action in PhoneAction::ALL {
            assert_eq!(PhoneAction::parse(action.as_str()), Some(action));
            assert_eq!(body(action, None)["action"], action.as_str());
        }
        assert_eq!(
            body(PhoneAction::Open, Some("org.example.app"))["app"],
            "org.example.app"
        );
        assert_eq!(PhoneAction::parse("Lock"), None);
        assert_eq!(PhoneAction::parse("unlock"), None);
    }

    /// Apps part 2: a notification's key names its app only in Android's form.
    #[test]
    fn a_notification_key_names_its_app_in_androids_form() {
        assert_eq!(
            app_of_notification_key("0|org.example.chat|12|null|10123").as_deref(),
            Some("org.example.chat")
        );
        assert_eq!(
            app_of_notification_key("10|org.example.chat|7|tag|10123").as_deref(),
            Some("org.example.chat")
        );
        for bad in [
            "",
            "org.example.chat",
            "pocketdesk:call:1",
            "0|org.example.chat",
            "x|org.example.chat|1|null|1",
            "|org.example.chat|1|null|1",
            "0|nodots|1|null|1",
            "0|org.example.chat;rm|1|null|1",
            "0| |1|null|1",
        ] {
            assert_eq!(app_of_notification_key(bad), None, "{bad:?}");
        }
    }

    /// P2: an app is named by a package name, and a list keeps the valid ones once.
    #[test]
    fn a_phone_app_list_is_read_with_care() {
        assert!(valid_app_id("org.example.app"));
        assert!(valid_app_id("com.app_2.Main"));
        assert!(!valid_app_id("noDots"));
        assert!(!valid_app_id("org..app"));
        assert!(!valid_app_id("org.1app"));
        assert!(!valid_app_id("org.app;rm"));
        assert!(!valid_app_id(&format!("a.{}", "b".repeat(MAX_APP_ID))));
        let apps = parse_apps(&json!({"apps": [
            {"id": "org.example.app", "name": "Example\nApp", "v": 12},
            {"id": "org.example.app", "name": "Again"},
            {"id": "bad id", "name": "Bad"},
            {"id": "org.example.blank", "name": "  "},
        ]}));
        assert_eq!(
            apps,
            vec![
                PhoneApp {
                    id: "org.example.app".into(),
                    name: "Example App".into(),
                    version: 12
                },
                PhoneApp {
                    id: "org.example.blank".into(),
                    name: "org.example.blank".into(),
                    version: 0
                },
            ]
        );
        let many: Vec<_> = (0..MAX_APPS + 5)
            .map(|i| json!({"id": format!("org.app.a{i}"), "name": "A"}))
            .collect();
        assert_eq!(parse_apps(&json!({ "apps": many })).len(), MAX_APPS);
        assert!(parse_apps(&json!({})).is_empty());
    }

    /// Apps part 1: an ask says which icons the computer has, and an open
    /// may want the screen too.
    #[test]
    fn an_apps_ask_says_what_the_computer_has() {
        let none = std::collections::BTreeMap::new();
        assert_eq!(apps_body(&none), json!({"action": "apps"}));
        let have = std::collections::BTreeMap::from([("org.example.app".to_string(), 7u64)]);
        assert_eq!(parse_have(&apps_body(&have)), have);
        let odd = json!({"have": {"bad id": 1, "org.example.b": "x", "org.example.c": 3}});
        assert_eq!(
            parse_have(&odd),
            std::collections::BTreeMap::from([("org.example.c".to_string(), 3u64)])
        );
        assert!(parse_have(&json!({"have": [1]})).is_empty());
        assert!(open_body("org.example.app", false).get("screen").is_none());
        assert_eq!(open_body("org.example.app", true)["screen"], true);
        assert_eq!(open_body("org.example.app", true)["action"], "open");
    }

    /// Apps part 1: a report of an ask is read with care.
    #[test]
    fn a_report_of_an_ask_is_read_with_care() {
        for result in AppResult::ALL {
            assert_eq!(AppResult::parse(result.as_str()), Some(result));
            let report = AppReport {
                about: "open".into(),
                app: Some("org.example.app".into()),
                result,
            };
            assert_eq!(AppReport::from_body(&report.to_body()), Some(report));
        }
        let list = json!({"about": "apps", "result": "off"});
        assert_eq!(AppReport::from_body(&list).unwrap().app, None);
        // An open names its app, a list names none, and a result is a known word.
        assert!(AppReport::from_body(&json!({"about": "open", "result": "off"})).is_none());
        let named = json!({"about": "apps", "app": "org.example.app", "result": "off"});
        assert!(AppReport::from_body(&named).is_none());
        let bad = json!({"about": "open", "app": "no", "result": "off"});
        assert!(AppReport::from_body(&bad).is_none());
        let word = json!({"about": "open", "app": "org.example.app", "result": "yes"});
        assert!(AppReport::from_body(&word).is_none());
        assert!(AppReport::from_body(&json!({"about": "lock", "result": "off"})).is_none());
    }

    /// Hands-free: the two new words are in the packet's vocabulary under
    /// the names the phone sends, and an older word is read as before.
    #[test]
    fn the_hands_free_results_have_their_words() {
        for (word, result) in [
            ("locked_secure", AppResult::LockedSecure),
            ("needs_tap", AppResult::NeedsTap),
            ("locked", AppResult::Locked),
        ] {
            assert_eq!(AppResult::parse(word), Some(result));
            let body = json!({"about": "open", "app": "org.example.app", "result": word});
            assert_eq!(AppReport::from_body(&body).unwrap().result, result);
        }
        // Every word is unique, so none can be read as another.
        let mut words: Vec<_> = AppResult::ALL.iter().map(|r| r.as_str()).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), AppResult::ALL.len());
    }

    /// P10: contacts with a name and a callable number, each number once.
    #[test]
    fn a_contact_list_keeps_names_with_numbers() {
        let contacts = parse_contacts(&json!({"contacts": [
            {"name": "Ann\nLee", "numbers": ["+1 555 0100", "+15550100", "*21#", "555-0101"]},
            {"name": "No number", "numbers": ["**"]},
            {"name": "  ", "numbers": ["5550102"]},
            {"name": "Bo", "numbers": ["5550103", "5550104", "5550105", "5550106", "5550107", "5550108"]},
        ]}));
        assert_eq!(contacts.len(), 2);
        assert_eq!(contacts[0].name, "Ann Lee");
        assert_eq!(
            contacts[0].numbers,
            vec!["+1 555 0100", "555-0101"],
            "each once, no codes"
        );
        assert_eq!(contacts[1].numbers.len(), MAX_NUMBERS);
    }
}
