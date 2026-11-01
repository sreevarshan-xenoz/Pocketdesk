//! The apps each phone listed for its computer (P2), kept owner-only in
//! `<state>/phone_apps.json` for the desktop app: `{device_id: {apps, at}}`.
//! Beside it (Apps part 1): how each phone's last ask went
//! (`phone_app_results.json`, written by the server) and the apps this
//! computer opened on each phone, last first (`phone_apps_recent.json`,
//! written by the desktop app, since Android gives an app no way to know
//! what was used last without a permission of its own).

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::phone_action::{AppReport, AppResult, PhoneApp};

use super::paths::{PHONE_APP_RESULTS_FILE, PHONE_APPS_RECENT_FILE};

/// The file the lists are kept in.
pub const PHONE_APPS_FILE: &str = "phone_apps.json";

/// The most recently opened apps kept for one phone.
pub const MAX_RECENT: usize = 50;

/// One phone's list, and when it came (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppList {
    pub apps: Vec<PhoneApp>,
    pub at: u64,
}

/// Every phone's list; none when the file is missing or unreadable.
pub fn read(state_dir: &Path) -> BTreeMap<String, AppList> {
    std::fs::read_to_string(state_dir.join(PHONE_APPS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_json(file: &Path, json: &str) -> io::Result<()> {
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, file)
}

/// Keep `apps` as `device_id`'s list at `at`. Only phones `keep` says are
/// still paired stay in the file.
pub fn keep(
    state_dir: &Path,
    device_id: &str,
    apps: Vec<PhoneApp>,
    at: u64,
    keep: &dyn Fn(&str) -> bool,
) -> io::Result<()> {
    let mut all = read(state_dir);
    all.retain(|id, _| keep(id));
    all.insert(device_id.to_string(), AppList { apps, at });
    let json = serde_json::to_string(&all).map_err(io::Error::other)?;
    write_json(&state_dir.join(PHONE_APPS_FILE), &json)
}

/// How a phone's last ask went, and when (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    #[serde(flatten)]
    pub report: AppReport,
    pub at: u64,
}

/// Every phone's last outcome; none when the file is missing or unreadable.
pub fn read_results(state_dir: &Path) -> BTreeMap<String, Outcome> {
    std::fs::read_to_string(state_dir.join(PHONE_APP_RESULTS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Keep `report` as `device_id`'s last outcome at `at`. Only phones `keep`
/// says are still paired stay in the file.
pub fn keep_result(
    state_dir: &Path,
    device_id: &str,
    report: AppReport,
    at: u64,
    keep: &dyn Fn(&str) -> bool,
) -> io::Result<()> {
    let mut all = read_results(state_dir);
    all.retain(|id, _| keep(id));
    all.insert(device_id.to_string(), Outcome { report, at });
    let json = serde_json::to_string(&all).map_err(io::Error::other)?;
    write_json(&state_dir.join(PHONE_APP_RESULTS_FILE), &json)
}

impl Outcome {
    /// Whether this is the answer to an ask of `about` made at `asked_at`
    /// (this computer's clock, as both stamps are).
    pub fn answers(&self, about: &str, asked_at: u64) -> bool {
        self.report.about == about && self.at >= asked_at
    }

    /// Whether the phone opened the app.
    pub fn opened(&self) -> bool {
        self.report.result == AppResult::Opened
    }
}

/// The apps opened on each phone, with when (Unix seconds); none when the
/// file is missing or unreadable.
pub fn read_recent(state_dir: &Path) -> BTreeMap<String, BTreeMap<String, u64>> {
    std::fs::read_to_string(state_dir.join(PHONE_APPS_RECENT_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Note that `app` was opened on `device_id` at `at`; only the newest
/// [`MAX_RECENT`] apps of a phone are kept.
pub fn note_open(state_dir: &Path, device_id: &str, app: &str, at: u64) -> io::Result<()> {
    if !crate::protocol::phone_action::valid_app_id(app) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    let mut all = read_recent(state_dir);
    let phone = all.entry(device_id.to_string()).or_default();
    phone.insert(app.to_string(), at);
    while phone.len() > MAX_RECENT {
        let Some(oldest) = phone
            .iter()
            .min_by_key(|(_, at)| **at)
            .map(|(id, _)| id.clone())
        else {
            break;
        };
        phone.remove(&oldest);
    }
    let json = serde_json::to_string(&all).map_err(io::Error::other)?;
    write_json(&state_dir.join(PHONE_APPS_RECENT_FILE), &json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-phone-apps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// P2: a phone's list is kept and read back, others pruned.
    #[test]
    fn a_phone_s_list_is_kept() {
        let dir = dir("list");
        assert!(read(&dir).is_empty());
        let app = PhoneApp {
            id: "org.example.app".into(),
            name: "Example".into(),
            version: 5,
        };
        keep(&dir, "old", vec![app.clone()], 1, &|_| true).unwrap();
        keep(&dir, "phone-a", vec![app.clone()], 2, &|id| id != "old").unwrap();
        let all = read(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(
            all["phone-a"],
            AppList {
                apps: vec![app],
                at: 2
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A list kept before versions were sent still reads.
    #[test]
    fn an_old_list_without_versions_reads() {
        let dir = dir("old");
        std::fs::write(
            dir.join(PHONE_APPS_FILE),
            r#"{"p":{"apps":[{"id":"org.example.app","name":"Example"}],"at":4}}"#,
        )
        .unwrap();
        assert_eq!(read(&dir)["p"].apps[0].version, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Apps part 1: a phone's last word is kept, answers only the ask it follows.
    #[test]
    fn a_phone_s_last_word_answers_the_ask_before_it() {
        let dir = dir("result");
        assert!(read_results(&dir).is_empty());
        let report = AppReport {
            about: "open".into(),
            app: Some("org.example.app".into()),
            name: None,
            result: AppResult::Locked,
        };
        keep_result(&dir, "old", report.clone(), 1, &|_| true).unwrap();
        keep_result(&dir, "phone-a", report.clone(), 10, &|id| id != "old").unwrap();
        let all = read_results(&dir);
        assert_eq!(all.len(), 1);
        let outcome = &all["phone-a"];
        assert_eq!(outcome.report, report);
        assert!(outcome.answers("open", 10));
        assert!(outcome.answers("open", 9));
        assert!(
            !outcome.answers("open", 11),
            "an earlier word is not the answer"
        );
        assert!(!outcome.answers("apps", 9));
        assert!(!outcome.opened());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Apps part 1: this computer's own recents, newest kept, an app once.
    #[test]
    fn the_apps_opened_are_remembered_newest_kept() {
        let dir = dir("recent");
        assert!(read_recent(&dir).is_empty());
        assert!(note_open(&dir, "phone-a", "no", 1).is_err());
        note_open(&dir, "phone-a", "org.example.a", 5).unwrap();
        note_open(&dir, "phone-a", "org.example.b", 6).unwrap();
        note_open(&dir, "phone-a", "org.example.a", 9).unwrap();
        note_open(&dir, "phone-b", "org.example.a", 2).unwrap();
        let all = read_recent(&dir);
        assert_eq!(
            all["phone-a"]["org.example.a"], 9,
            "an app once, at its last open"
        );
        assert_eq!(all["phone-a"].len(), 2);
        assert_eq!(all["phone-b"].len(), 1);
        for i in 0..MAX_RECENT + 5 {
            note_open(
                &dir,
                "phone-c",
                &format!("org.example.n{i}"),
                100 + i as u64,
            )
            .unwrap();
        }
        let c = &read_recent(&dir)["phone-c"];
        assert_eq!(c.len(), MAX_RECENT);
        assert!(!c.contains_key("org.example.n0"), "the oldest went");
        assert!(c.contains_key(&format!("org.example.n{}", MAX_RECENT + 4)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
