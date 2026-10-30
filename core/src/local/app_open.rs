//! Opening the window on a page (E2): the tray program, or a second launch
//! of the window, writes `<state>/app_open.json` (owner-only) and starts
//! the window; the window, started or raised, takes it. A request older
//! than [`TTL_SECS`] is ignored: it was meant for a window that never came.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::paths::APP_OPEN_FILE;

/// How long a request waits for the window.
pub const TTL_SECS: u64 = 10;

/// The page that asks the window to close (the tray's Quit).
pub const QUIT: &str = "quit";

/// Which page to open, by its key (the window's `Tab::from_key`), and the
/// phone it is for (Files' send-to), if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub page: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    pub created_unix: u64,
}

impl OpenRequest {
    /// Whether this asks a window started at `window_started_unix` to
    /// close: a Quit written before it started was for a window gone since
    /// (E2 review), and must not close the next one.
    pub fn quits(&self, window_started_unix: u64) -> bool {
        self.page == QUIT && self.created_unix >= window_started_unix
    }
}

/// The page asked for to open the typing window on a phone (N9; M4's
/// window, not a page of its own).
pub const TYPE: &str = "type";

/// The page asked for to show a paired computer's screen (PC6; BK7's
/// window, not a page of its own), the computer's id in `phone`.
pub const COMPUTER_VIEW: &str = "computer_view";

/// Ask the window to open `page` (for `phone`).
pub fn request(state_dir: &Path, page: &str, phone: Option<&str>, now_unix: u64) -> io::Result<()> {
    let request = OpenRequest {
        page: page.to_string(),
        phone: phone.map(str::to_string),
        created_unix: now_unix,
    };
    let path = state_dir.join(APP_OPEN_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)
}

/// The request waiting, taken (removed); `None` when there is none, or it
/// is stale, from the future, or does not read.
pub fn take(state_dir: &Path, now_unix: u64) -> Option<OpenRequest> {
    let path = state_dir.join(APP_OPEN_FILE);
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let request: OpenRequest = serde_json::from_str(&raw).ok()?;
    let fresh = request.created_unix <= now_unix && now_unix - request.created_unix <= TTL_SECS;
    fresh.then_some(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_taken_once_while_fresh() {
        let dir = std::env::temp_dir().join(format!("pd-app-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(take(&dir, 100), None);
        request(&dir, "files", Some("phone-a"), 100).unwrap();
        assert_eq!(
            take(&dir, 105),
            Some(OpenRequest {
                page: "files".into(),
                phone: Some("phone-a".into()),
                created_unix: 100
            })
        );
        assert_eq!(take(&dir, 105), None, "taken");
        request(&dir, "pair", None, 100).unwrap();
        assert_eq!(take(&dir, 100 + TTL_SECS + 1), None, "stale");
        request(&dir, "pair", None, 200).unwrap();
        assert_eq!(take(&dir, 100), None, "from the future");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quit_closes_only_a_window_that_was_open_when_it_was_asked() {
        let quit = OpenRequest {
            page: QUIT.into(),
            phone: None,
            created_unix: 100,
        };
        assert!(quit.quits(90), "the window ran then");
        assert!(quit.quits(100));
        assert!(!quit.quits(101), "a window started after the Quit");
        let files = OpenRequest {
            page: "files".into(),
            ..quit
        };
        assert!(!files.quits(90));
    }
}
