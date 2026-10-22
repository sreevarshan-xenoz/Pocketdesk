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
}

impl PhoneAction {
    pub const ALL: [PhoneAction; 4] = [
        PhoneAction::Screenshot,
        PhoneAction::Lock,
        PhoneAction::Apps,
        PhoneAction::Open,
    ];

    /// The word the packet and the request files carry.
    pub fn as_str(self) -> &'static str {
        match self {
            PhoneAction::Screenshot => "screenshot",
            PhoneAction::Lock => "lock",
            PhoneAction::Apps => "apps",
            PhoneAction::Open => "open",
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
}
