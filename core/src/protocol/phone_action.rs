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

/// The most apps a list holds.
pub const MAX_APPS: usize = 500;

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

/// One of the phone's apps (P2): its package name and its label.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhoneApp {
    pub id: String,
    pub name: String,
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
        });
        if apps.len() == MAX_APPS {
            break;
        }
    }
    apps
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
            {"id": "org.example.app", "name": "Example\nApp"},
            {"id": "org.example.app", "name": "Again"},
            {"id": "bad id", "name": "Bad"},
            {"id": "org.example.blank", "name": "  "},
        ]}));
        assert_eq!(
            apps,
            vec![
                PhoneApp {
                    id: "org.example.app".into(),
                    name: "Example App".into()
                },
                PhoneApp {
                    id: "org.example.blank".into(),
                    name: "org.example.blank".into()
                },
            ]
        );
        let many: Vec<_> = (0..MAX_APPS + 5)
            .map(|i| json!({"id": format!("org.app.a{i}"), "name": "A"}))
            .collect();
        assert_eq!(parse_apps(&json!({ "apps": many })).len(), MAX_APPS);
        assert!(parse_apps(&json!({})).is_empty());
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
