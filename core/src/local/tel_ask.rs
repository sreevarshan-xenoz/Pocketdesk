//! A `tel:` link with several phones to take it (M8):
//! `<state>/tel_ask.json`, the number and when it came, written by
//! `pocketdesk tel` and read by the desktop app, which asks which phone
//! calls it. One question at a time: a newer link replaces it. It goes
//! after [`ASK_TTL_SECS`].

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The question's file in the state folder.
pub const TEL_ASK_FILE: &str = "tel_ask.json";

/// How long the question waits for an answer: two minutes.
pub const ASK_TTL_SECS: u64 = 120;

/// The number waiting for a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelAsk {
    pub number: String,
    pub created_unix: u64,
}

/// Ask which phone calls `number` (cleaned by the caller).
pub fn put(state_dir: &Path, number: &str, now_unix: u64) -> io::Result<()> {
    let ask = TelAsk {
        number: number.to_string(),
        created_unix: now_unix,
    };
    let file = state_dir.join(TEL_ASK_FILE);
    let json = serde_json::to_string(&ask).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &file)
}

/// The question waiting, if one is fresh; a stale, future-dated or
/// unreadable one is removed.
pub fn read(state_dir: &Path, now_unix: u64) -> Option<TelAsk> {
    let file = state_dir.join(TEL_ASK_FILE);
    let raw = std::fs::read_to_string(&file).ok()?;
    let ask = serde_json::from_str::<TelAsk>(&raw)
        .ok()
        .filter(|a| a.created_unix <= now_unix && now_unix - a.created_unix <= ASK_TTL_SECS)
        .filter(|a| crate::protocol::dial::clean_number(&a.number).as_deref() == Some(&a.number));
    if ask.is_none() {
        let _ = std::fs::remove_file(&file);
    }
    ask
}

/// The question answered or dropped: removed, only if it is still `ask`
/// (a newer link's question stays).
pub fn clear(state_dir: &Path, ask: &TelAsk) {
    let file = state_dir.join(TEL_ASK_FILE);
    let same = std::fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str::<TelAsk>(&raw).ok())
        .is_some_and(|now| &now == ask);
    if same {
        let _ = std::fs::remove_file(&file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-tel-ask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_question_is_kept_read_and_cleared_once_answered() {
        let dir = dir("kept");
        put(&dir, "+15550100", 1_000).unwrap();
        let ask = read(&dir, 1_010).expect("fresh");
        assert_eq!(ask.number, "+15550100");
        // A newer link's question is not cleared by an answer to the old.
        put(&dir, "5550199", 1_020).unwrap();
        clear(&dir, &ask);
        assert_eq!(
            read(&dir, 1_030).map(|a| a.number).as_deref(),
            Some("5550199")
        );
        let newer = read(&dir, 1_030).unwrap();
        clear(&dir, &newer);
        assert_eq!(read(&dir, 1_030), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_future_or_bad_question_is_dropped() {
        let dir = dir("stale");
        put(&dir, "5550100", 1_000).unwrap();
        assert_eq!(read(&dir, 1_000 + ASK_TTL_SECS + 1), None, "stale");
        assert!(!dir.join(TEL_ASK_FILE).exists(), "and removed");
        put(&dir, "5550100", 2_000).unwrap();
        assert_eq!(read(&dir, 1_000), None, "from the future");
        put(&dir, "*#06#", 1_000).unwrap();
        assert_eq!(read(&dir, 1_000), None, "not a number");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
