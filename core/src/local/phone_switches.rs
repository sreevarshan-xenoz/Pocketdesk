//! Which of its features each phone has on, as it last said (L7).
//!
//! A phone sends `kdeconnect.pocketdesk.switches {switches: {<feature>:
//! {on, android}}}` to its own computer: for each feature on a fixed list,
//! its own switch and whether Android lets it work (null where Android
//! needs nothing). Only those booleans: no app, folder or package names.
//! The server keeps the newest per phone in `<state>/phone_switches.json`
//! for the desktop app's Devices page and the local API.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::config::Config;
use super::paths::PHONE_SWITCHES_FILE;
use crate::protocol::permissions::{Capability, DevicePermissions};

/// The packet a phone tells its switches with.
pub const SWITCHES: &str = "kdeconnect.pocketdesk.switches";

/// The features a phone may tell, in the order the Devices page shows them.
/// Any other name in a report is dropped.
pub const FEATURES: &[&str] = &[
    "notifications",
    "answers",
    "texts",
    "clipboard",
    "files",
    "folders",
    "photos",
    "screenshots",
    "camera",
    "links",
    "ring",
    "media",
    "media_background",
    "mute",
    "control",
    "dial",
    "commands",
];

/// One feature as the phone says it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    /// The phone's own switch for it.
    pub on: bool,
    /// Whether Android lets it work; `None` where Android needs nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android: Option<bool>,
}

/// A phone's last report and when it came.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneSwitches {
    pub switches: BTreeMap<String, Switch>,
    pub updated_unix: u64,
}

/// The switches in a report's body: the known features whose `on` is a
/// boolean, `android` kept only when it is one. `None` when the body has
/// no `switches` object.
pub fn from_body(body: &serde_json::Value) -> Option<BTreeMap<String, Switch>> {
    let given = body.get("switches")?.as_object()?;
    Some(
        FEATURES
            .iter()
            .filter_map(|name| {
                let entry = given.get(*name)?;
                let on = entry.get("on")?.as_bool()?;
                let android = entry.get("android").and_then(|v| v.as_bool());
                Some((name.to_string(), Switch { on, android }))
            })
            .collect(),
    )
}

#[derive(Default, Serialize, Deserialize)]
struct SwitchesFile {
    phones: BTreeMap<String, PhoneSwitches>,
}

/// Each phone's last report, by device id; empty when there is no file or
/// it does not read.
pub fn read(state_dir: &Path) -> BTreeMap<String, PhoneSwitches> {
    std::fs::read_to_string(state_dir.join(PHONE_SWITCHES_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<SwitchesFile>(&raw).ok())
        .map(|file| file.phones)
        .unwrap_or_default()
}

/// Replace the file with `phones`, whole, so a reader never sees it
/// half-written.
pub fn write(state_dir: &Path, phones: &BTreeMap<String, PhoneSwitches>) -> io::Result<()> {
    let json = serde_json::to_string(&SwitchesFile {
        phones: phones.clone(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_SWITCHES_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

/// The packet a phone asks what this computer allows it with (M2): `{}`
/// asked, [`allowed_body`] answered.
pub const ALLOWED: &str = "kdeconnect.pocketdesk.allowed";

/// The per-phone permission a feature needs on this computer (L7, M2): the
/// one table the Devices page and the phone's mirror both read.
pub fn capability(feature: &str) -> Option<Capability> {
    Some(match feature {
        "notifications" | "answers" => Capability::Notifications,
        "texts" => Capability::Texts,
        "clipboard" => Capability::Clipboard,
        "files" | "folders" | "photos" | "screenshots" | "camera" => Capability::Files,
        "links" => Capability::Links,
        "ring" => Capability::Ring,
        "media" | "media_background" => Capability::Media,
        "mute" => Capability::Mute,
        "control" => Capability::Cast,
        "dial" => Capability::Dial,
        "commands" => Capability::Command,
        _ => return None,
    })
}

/// The features this computer's Settings turn off for every phone (L7
/// review), each with the setting's label: they hold the feature back
/// whatever the phone's permission says.
pub fn settings_off(config: &Config) -> Vec<(&'static str, &'static str)> {
    let mut off = Vec::new();
    if !config.phone_notifications {
        off.push(("notifications", "Phone notifications"));
        off.push(("answers", "Phone notifications"));
    }
    if !config.clipboard_push {
        off.push(("clipboard", "Clipboard to phones"));
    }
    if !config.cast_control {
        off.push(("control", "Control phones from their screen window"));
    }
    off
}

/// What this computer allows a phone (M2), as the phone's mirror reads it:
/// for each feature, whether this phone's permission allows it and whether
/// this computer's Settings let any phone have it. Booleans only.
pub fn allowed_body(permissions: &DevicePermissions, config: &Config) -> serde_json::Value {
    let off = settings_off(config);
    let allowed: serde_json::Map<String, serde_json::Value> = FEATURES
        .iter()
        .map(|feature| {
            let permission = capability(feature).is_some_and(|c| permissions.allows(c));
            let setting = !off.iter().any(|(key, _)| key == feature);
            (
                feature.to_string(),
                serde_json::json!({"permission": permission, "setting": setting}),
            )
        })
        .collect();
    serde_json::json!({ "allowed": allowed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_known_features_with_boolean_switches_are_taken() {
        let body = json!({"switches": {
            "notifications": {"on": true, "android": false},
            "links": {"on": false},
            "clipboard": {"on": true, "android": null},
            "texts": {"on": "yes"},
            "com.example.app": {"on": true},
            "ring": {"on": true, "android": "granted"},
        }});
        let taken = from_body(&body).unwrap();
        assert_eq!(
            taken.keys().map(String::as_str).collect::<Vec<_>>(),
            ["clipboard", "links", "notifications", "ring"]
        );
        assert_eq!(
            taken["notifications"],
            Switch {
                on: true,
                android: Some(false)
            }
        );
        assert_eq!(taken["links"].android, None);
        assert_eq!(taken["ring"].android, None, "a word is not a boolean");
        assert_eq!(from_body(&json!({})), None);
        assert_eq!(from_body(&json!({"switches": []})), None);
    }

    /// The phone's list (`SwitchReport.FEATURES`) is this one, in order.
    #[test]
    fn the_phones_list_is_this_one() {
        let kotlin = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../android/app/src/main/java/dev/pocketdesk/android/status/SwitchReport.kt"
        ))
        .expect("SwitchReport.kt");
        let start = kotlin.find("val FEATURES = listOf(").expect("its FEATURES");
        let end = start + kotlin[start..].find(')').expect("its end");
        let names: Vec<&str> = kotlin[start..end].split('"').skip(1).step_by(2).collect();
        assert_eq!(names, FEATURES);
    }

    #[test]
    fn reports_are_written_and_read_back_and_a_bad_file_is_empty() {
        let dir = std::env::temp_dir().join(format!(
            "pocketdesk-phone-switches-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read(&dir).is_empty());
        let mut phones = BTreeMap::new();
        phones.insert(
            "phone-a".to_string(),
            PhoneSwitches {
                switches: from_body(&json!({"switches": {"links": {"on": true}}})).unwrap(),
                updated_unix: 100,
            },
        );
        write(&dir, &phones).unwrap();
        assert_eq!(read(&dir), phones);
        std::fs::write(dir.join(PHONE_SWITCHES_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_feature_names_the_permission_it_needs() {
        use crate::protocol::permissions::Capability;
        for feature in FEATURES {
            assert!(capability(feature).is_some(), "{feature}");
        }
        assert_eq!(capability("files"), Some(Capability::Files));
        assert_eq!(capability("answers"), Some(Capability::Notifications));
        assert_eq!(capability("control"), Some(Capability::Cast));
        assert_eq!(capability("commands"), Some(Capability::Command));
        assert_eq!(capability("com.example"), None);
    }

    #[test]
    fn settings_turn_off_their_features_for_every_phone() {
        let on = crate::local::config::Config::default();
        assert!(settings_off(&on).is_empty());
        let off = crate::local::config::Config {
            phone_notifications: false,
            clipboard_push: false,
            cast_control: false,
            ..on
        };
        assert_eq!(
            settings_off(&off),
            [
                ("notifications", "Phone notifications"),
                ("answers", "Phone notifications"),
                ("clipboard", "Clipboard to phones"),
                ("control", "Control phones from their screen window"),
            ]
        );
    }

    #[test]
    fn the_answer_says_each_side_this_computer_holds() {
        use crate::protocol::permissions::{Capability, DevicePermissions};
        let mut perms = DevicePermissions::default();
        perms.set(Capability::Files, false);
        let config = crate::local::config::Config {
            clipboard_push: false,
            ..Default::default()
        };
        let body = allowed_body(&perms, &config);
        let allowed = body["allowed"].as_object().unwrap();
        assert_eq!(allowed.len(), FEATURES.len());
        assert_eq!(
            body["allowed"]["files"],
            json!({"permission": false, "setting": true})
        );
        assert_eq!(
            body["allowed"]["photos"],
            json!({"permission": false, "setting": true})
        );
        assert_eq!(
            body["allowed"]["clipboard"],
            json!({"permission": true, "setting": false})
        );
        assert_eq!(
            body["allowed"]["links"],
            json!({"permission": true, "setting": true})
        );
    }
}
