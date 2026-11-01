//! Approve with my phone, between the PAM helper and the server:
//! `pocketdesk-pam-ask` (run as the user) leaves a [`Request`] in
//! `<state>/approve_out/<id>.json`; the server takes it, puts it to the
//! phones it names that are linked, and says how it went in
//! `<state>/approve_in/<id>.json` ([`Status`]): put to phones, no phone (so
//! the helper gives up at once and the password prompt shows), or answered.
//! Everything here is the user's and may be written by anything running as
//! the user, so none of it is trusted: the helper checks the phone's
//! signature itself, against the key root pinned.
//!
//! The phones' keys as each gave it are kept in `<state>/approve_keys.json`
//! ([`PhoneKey`]), for Settings to list and for root to pin.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::asks::{take_files, write};
use super::paths::{APPROVE_DIR, APPROVE_KEYS_FILE, APPROVE_STATUS_DIR};
use crate::protocol::approve::{Approval, Kind, MAX_TIMEOUT_SECS, PIN_TIMEOUT_SECS};
use crate::protocol::ask::{Answer, is_ask_id};

/// One request for the server: the approval, and the phones (device ids)
/// whose keys root pinned, the only ones it may be put to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub approval: Approval,
    pub phones: Vec<String>,
}

/// How a request went, as the server says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum Status {
    /// Put to this many linked phones; the answer may come.
    Asked { phones: usize },
    /// No phone it names is linked (or the feature is off here): give up
    /// now, so the password prompt shows without a wait.
    NoPhone,
    /// A phone answered first.
    Answered {
        device_id: String,
        answer: Answer,
        /// base64 DER, for a Yes.
        signature: Option<String>,
        /// A pin request's Key Attestation chain, leaf first (base64 DER).
        #[serde(default)]
        attestation: Option<Vec<String>>,
    },
}

/// One phone's approval key as it gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneKey {
    pub device_id: String,
    /// base64 X.509 SubjectPublicKeyInfo (EC P-256).
    pub public_key: String,
    /// Takes the fingerprint only, not the PIN.
    pub biometric_only: bool,
    /// What both apps show to compare: the key's whole SHA-256, in groups
    /// of four hex digits.
    pub fingerprint: String,
    pub added_unix: u64,
    /// Where the phone's Key Attestation says the key lives (`tee`,
    /// `strongbox`), as root checked it at pin; `None` for a key pinned
    /// without attestation, or one only the phone gave.
    #[serde(default)]
    pub attested: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeysFile {
    #[serde(default)]
    keys: Vec<PhoneKey>,
}

/// Leave `request` for the server.
pub fn request(state_dir: &Path, request: &Request) -> io::Result<()> {
    write(&state_dir.join(APPROVE_DIR), &request.approval.id, request)
}

/// Take every waiting request (the server's side). One expired, from the
/// future, waiting too long or named for another is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<Request> {
    take_files(&state_dir.join(APPROVE_DIR))
        .into_iter()
        .filter_map(|(stem, raw)| {
            let request = serde_json::from_str::<Request>(&raw).ok()?;
            let a = &request.approval;
            let longest = if a.kind == Kind::Pin {
                PIN_TIMEOUT_SECS
            } else {
                MAX_TIMEOUT_SECS
            };
            let fresh = a.issued_unix <= now_unix + 5
                && now_unix < a.expires_unix
                && a.expires_unix.saturating_sub(a.issued_unix) <= longest;
            (fresh && a.id == stem && is_ask_id(&a.id)).then_some(request)
        })
        .collect()
}

/// Say how request `id` went (the server's side).
pub fn set_status(state_dir: &Path, id: &str, status: &Status) -> io::Result<()> {
    write(&state_dir.join(APPROVE_STATUS_DIR), id, status)
}

/// How request `id` went, if the server said.
pub fn status(state_dir: &Path, id: &str) -> Option<Status> {
    if !is_ask_id(id) {
        return None;
    }
    let raw = std::fs::read_to_string(
        state_dir
            .join(APPROVE_STATUS_DIR)
            .join(format!("{id}.json")),
    )
    .ok()?;
    serde_json::from_str(&raw).ok()
}

/// Take request `id` and what was said of it away (the helper is done).
pub fn withdraw(state_dir: &Path, id: &str) {
    if !is_ask_id(id) {
        return;
    }
    for folder in [APPROVE_DIR, APPROVE_STATUS_DIR] {
        let _ = std::fs::remove_file(state_dir.join(folder).join(format!("{id}.json")));
    }
}

/// The phones' keys as they gave them.
pub fn keys(state_dir: &Path) -> Vec<PhoneKey> {
    std::fs::read_to_string(state_dir.join(APPROVE_KEYS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<KeysFile>(&raw).ok())
        .map(|f| f.keys)
        .unwrap_or_default()
}

/// Keep `key` for its phone, replacing what it gave before; `None` forgets
/// `device_id`'s. Whether anything changed.
pub fn set_key(state_dir: &Path, device_id: &str, key: Option<PhoneKey>) -> io::Result<bool> {
    let mut all = keys(state_dir);
    let before = all.clone();
    all.retain(|k| k.device_id != device_id);
    if let Some(key) = key {
        all.push(key);
    }
    if all == before {
        return Ok(false);
    }
    let path = state_dir.join(APPROVE_KEYS_FILE);
    let json = serde_json::to_string_pretty(&KeysFile { keys: all }).map_err(io::Error::other)?;
    let tmp = crate::atomic_file::temp_beside(&path);
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::approve::Kind;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-approve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn approval(id: &str, issued: u64, expires: u64) -> Approval {
        Approval {
            id: id.to_string(),
            kind: Kind::Unlock,
            service: "hyprlock".to_string(),
            user: "alice".to_string(),
            requester: String::new(),
            action: String::new(),
            command: String::new(),
            tty: String::new(),
            origin: String::new(),
            computer: "ab".repeat(32),
            nonce: "01".repeat(32),
            issued_unix: issued,
            expires_unix: expires,
        }
    }

    #[test]
    fn a_request_is_taken_once_while_fresh() {
        let dir = dir("take");
        let r = Request {
            approval: approval("r-1", 1_000, 1_020),
            phones: vec!["phone-a".to_string()],
        };
        request(&dir, &r).unwrap();
        assert_eq!(take_all(&dir, 1_001), vec![r.clone()]);
        assert!(take_all(&dir, 1_001).is_empty(), "taken");
        request(&dir, &r).unwrap();
        assert!(take_all(&dir, 1_020).is_empty(), "expired");
        let long = Request {
            approval: approval("r-2", 1_000, 1_000 + MAX_TIMEOUT_SECS + 1),
            phones: vec![],
        };
        request(&dir, &long).unwrap();
        assert!(take_all(&dir, 1_001).is_empty(), "waits too long");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_status_is_said_and_withdrawn() {
        let dir = dir("status");
        assert_eq!(status(&dir, "r-1"), None);
        set_status(&dir, "r-1", &Status::Asked { phones: 2 }).unwrap();
        assert_eq!(status(&dir, "r-1"), Some(Status::Asked { phones: 2 }));
        let said = Status::Answered {
            device_id: "phone-a".to_string(),
            answer: Answer::Yes,
            signature: Some("MEUC".to_string()),
            attestation: None,
        };
        set_status(&dir, "r-1", &said).unwrap();
        assert_eq!(status(&dir, "r-1"), Some(said));
        withdraw(&dir, "r-1");
        assert_eq!(status(&dir, "r-1"), None);
        assert_eq!(status(&dir, "../x"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_phone_key_is_kept_replaced_and_forgotten() {
        let dir = dir("keys");
        let key = |pk: &str| PhoneKey {
            device_id: "phone-a".to_string(),
            public_key: pk.to_string(),
            biometric_only: false,
            fingerprint: "0000 0000 0000 0000".to_string(),
            added_unix: 1,
            attested: None,
        };
        assert!(set_key(&dir, "phone-a", Some(key("A"))).unwrap());
        assert!(!set_key(&dir, "phone-a", Some(key("A"))).unwrap(), "same");
        assert!(set_key(&dir, "phone-a", Some(key("B"))).unwrap());
        assert_eq!(keys(&dir), vec![key("B")]);
        assert!(set_key(&dir, "phone-a", None).unwrap());
        assert!(keys(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
