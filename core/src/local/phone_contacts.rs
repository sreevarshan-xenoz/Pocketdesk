//! The contacts each phone listed for its computer (P10), names and numbers
//! only, kept owner-only in `<state>/phone_contacts.json` for the desktop
//! app's Messages page: `{device_id: {contacts, at}}`.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::phone_action::PhoneContact;

/// The file the lists are kept in.
pub const PHONE_CONTACTS_FILE: &str = "phone_contacts.json";

/// One phone's list, and when it came (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactList {
    pub contacts: Vec<PhoneContact>,
    pub at: u64,
}

/// Every phone's list; none when the file is missing or unreadable.
pub fn read(state_dir: &Path) -> BTreeMap<String, ContactList> {
    std::fs::read_to_string(state_dir.join(PHONE_CONTACTS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Keep `contacts` as `device_id`'s list at `at`. Only phones `keep` says are
/// still paired stay in the file.
pub fn keep(
    state_dir: &Path,
    device_id: &str,
    contacts: Vec<PhoneContact>,
    at: u64,
    keep: &dyn Fn(&str) -> bool,
) -> io::Result<()> {
    let mut all = read(state_dir);
    all.retain(|id, _| keep(id));
    all.insert(device_id.to_string(), ContactList { contacts, at });
    let json = serde_json::to_string(&all).map_err(io::Error::other)?;
    let file = state_dir.join(PHONE_CONTACTS_FILE);
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

/// Forget `device_id`'s list (P10: its owner turned it off).
pub fn forget(state_dir: &Path, device_id: &str) -> io::Result<()> {
    let mut all = read(state_dir);
    if all.remove(device_id).is_none() {
        return Ok(());
    }
    let json = serde_json::to_string(&all).map_err(io::Error::other)?;
    let file = state_dir.join(PHONE_CONTACTS_FILE);
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P10: a phone's list is kept and read back, others pruned, and forgot.
    #[test]
    fn a_phone_s_contacts_are_kept_and_forgot() {
        let dir = std::env::temp_dir().join(format!("pd-phone-contacts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ann = PhoneContact {
            name: "Ann".into(),
            numbers: vec!["5550100".into()],
        };
        keep(&dir, "old", vec![ann.clone()], 1, &|_| true).unwrap();
        keep(&dir, "phone-a", vec![ann.clone()], 2, &|id| id != "old").unwrap();
        let all = read(&dir);
        assert_eq!(all.len(), 1);
        assert_eq!(all["phone-a"].contacts, vec![ann]);
        forget(&dir, "phone-a").unwrap();
        assert!(read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
