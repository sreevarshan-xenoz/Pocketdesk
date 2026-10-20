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

use super::paths::PHONE_SWITCHES_FILE;

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
}
