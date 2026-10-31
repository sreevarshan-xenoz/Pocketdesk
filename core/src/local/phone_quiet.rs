//! Whether each phone is in Do Not Disturb, as it last said (D2).
//!
//! A phone with "Let the computer mute this phone" on sends
//! `kdeconnect.pocketdesk.quiet_state {quiet, until?, byComputer}` when its
//! state changes and when its link comes up; the server keeps the newest per
//! phone in `<state>/phone_quiet.json`, which the desktop app and the local
//! API read. Only the server writes the file.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::PHONE_QUIET_FILE;

/// A phone's quiet state and when it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneQuiet {
    /// In Do Not Disturb, of any kind.
    pub quiet: bool,
    /// The end of this computer's mute (unix seconds), when the mute is ours.
    pub until_unix: Option<u64>,
    /// The mute is one a computer asked for.
    pub by_computer: bool,
    pub updated_unix: u64,
    /// When the phone last said no to a mute (D2 review): its switch is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused_unix: Option<u64>,
}

impl PhoneQuiet {
    /// The state in a `quiet_state` body, or `None` for one without `quiet`.
    /// An `until` is kept only for a computer's mute, and only a plausible one.
    pub fn from_body(body: &serde_json::Value, now_unix: u64) -> Option<Self> {
        let quiet = body.get("quiet")?.as_bool()?;
        let by_computer = quiet && body.get("byComputer").and_then(|v| v.as_bool()) == Some(true);
        let until_unix = body
            .get("until")
            .and_then(|v| v.as_u64())
            // At most a day and a little from now: a mute is a day at most.
            .filter(|until| by_computer && *until <= now_unix + 2 * 86_400);
        let refused = body.get("refused").and_then(|v| v.as_bool()) == Some(true);
        Some(Self {
            quiet,
            until_unix,
            by_computer,
            updated_unix: now_unix,
            refused_unix: refused.then_some(now_unix),
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct QuietFile {
    phones: BTreeMap<String, PhoneQuiet>,
}

/// Each phone's last quiet state, by device id. A missing or unreadable
/// file is empty.
pub fn read(state_dir: &Path) -> BTreeMap<String, PhoneQuiet> {
    std::fs::read_to_string(state_dir.join(PHONE_QUIET_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<QuietFile>(&raw).ok())
        .map(|file| file.phones)
        .unwrap_or_default()
}

/// Replace the file with `phones`, whole, so a reader never sees it half
/// written.
pub fn write(state_dir: &Path, phones: &BTreeMap<String, PhoneQuiet>) -> io::Result<()> {
    let json = serde_json::to_string(&QuietFile {
        phones: phones.clone(),
    })
    .map_err(io::Error::other)?;
    let path = state_dir.join(PHONE_QUIET_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    std::fs::write(&tmp, json)?;
    crate::atomic_file::replace(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_report_reads_as_sent() {
        let now = 1_000_000;
        assert_eq!(
            PhoneQuiet::from_body(
                &json!({"quiet": true, "until": now + 3600, "byComputer": true}),
                now
            ),
            Some(PhoneQuiet {
                quiet: true,
                until_unix: Some(now + 3600),
                by_computer: true,
                updated_unix: now,
                refused_unix: None,
            })
        );
        // A mute said no to.
        assert_eq!(
            PhoneQuiet::from_body(
                &json!({"quiet": false, "byComputer": false, "refused": true}),
                now
            )
            .unwrap()
            .refused_unix,
            Some(now)
        );
        // The owner's own Do Not Disturb: no end of ours.
        assert_eq!(
            PhoneQuiet::from_body(
                &json!({"quiet": true, "until": now + 3600, "byComputer": false}),
                now
            )
            .unwrap()
            .until_unix,
            None
        );
        // Not quiet: never by a computer.
        assert!(
            !PhoneQuiet::from_body(&json!({"quiet": false, "byComputer": true}), now)
                .unwrap()
                .by_computer
        );
        // An end a week away is not believed.
        assert_eq!(
            PhoneQuiet::from_body(
                &json!({"quiet": true, "until": now + 7 * 86_400, "byComputer": true}),
                now
            )
            .unwrap()
            .until_unix,
            None
        );
        assert_eq!(PhoneQuiet::from_body(&json!({"until": now}), now), None);
        assert_eq!(PhoneQuiet::from_body(&json!({"quiet": "yes"}), now), None);
    }

    #[test]
    fn written_then_read() {
        let dir = std::env::temp_dir().join(format!("pd-phone-quiet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read(&dir).is_empty(), "no file");
        let mut phones = BTreeMap::new();
        phones.insert(
            "phone-1".to_string(),
            PhoneQuiet {
                quiet: true,
                until_unix: None,
                by_computer: false,
                updated_unix: 5,
                refused_unix: None,
            },
        );
        write(&dir, &phones).unwrap();
        assert_eq!(read(&dir), phones);
        std::fs::write(dir.join(PHONE_QUIET_FILE), "not json").unwrap();
        assert!(read(&dir).is_empty(), "unreadable is empty");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
