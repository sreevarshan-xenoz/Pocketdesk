//! The apps each phone listed for its computer (P2), kept owner-only in
//! `<state>/phone_apps.json` for the desktop app: `{device_id: {apps, at}}`.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::phone_action::PhoneApp;

/// The file the lists are kept in.
pub const PHONE_APPS_FILE: &str = "phone_apps.json";

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
    let file = state_dir.join(PHONE_APPS_FILE);
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2: a phone's list is kept and read back, others pruned.
    #[test]
    fn a_phone_s_list_is_kept() {
        let dir = std::env::temp_dir().join(format!("pd-phone-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read(&dir).is_empty());
        let app = PhoneApp {
            id: "org.example.app".into(),
            name: "Example".into(),
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
}
