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
    /// The phone's app a [`PHONE_APP`] request opens (its package name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
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

/// The page asked for to open one of a phone's apps there (Apps part 2: a
/// pinned app's shortcut), the phone's id in `phone` and the package in
/// `app`. It brings up no page and does not raise the window.
pub const PHONE_APP: &str = "phone_app";

/// Ask the window to open `page` (for `phone`).
pub fn request(state_dir: &Path, page: &str, phone: Option<&str>, now_unix: u64) -> io::Result<()> {
    write(state_dir, page, phone, None, now_unix)
}

/// Ask the window to open `app` on `phone` (Apps part 2). Both are checked
/// as the packets' are, here and again when the request is taken.
pub fn request_phone_app(
    state_dir: &Path,
    phone: &str,
    app: &str,
    now_unix: u64,
) -> io::Result<()> {
    if !is_phone_app(phone, Some(app)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    write(state_dir, PHONE_APP, Some(phone), Some(app), now_unix)
}

/// Whether `phone` and `app` can be a [`PHONE_APP`] request's.
fn is_phone_app(phone: &str, app: Option<&str>) -> bool {
    super::approval::is_valid_id(phone)
        && app.is_some_and(crate::protocol::phone_action::valid_app_id)
}

fn write(
    state_dir: &Path,
    page: &str,
    phone: Option<&str>,
    app: Option<&str>,
    now_unix: u64,
) -> io::Result<()> {
    let request = OpenRequest {
        page: page.to_string(),
        phone: phone.map(str::to_string),
        app: app.map(str::to_string),
        created_unix: now_unix,
    };
    let path = state_dir.join(APP_OPEN_FILE);
    let tmp = crate::atomic_file::temp_beside(&path);
    std::fs::create_dir_all(state_dir)?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// The request waiting, taken (removed); `None` when there is none, or it
/// is stale, from the future, or does not read.
pub fn take(state_dir: &Path, now_unix: u64) -> Option<OpenRequest> {
    let path = state_dir.join(APP_OPEN_FILE);
    let raw = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let request: OpenRequest = serde_json::from_str(&raw).ok()?;
    let fresh = request.created_unix <= now_unix && now_unix - request.created_unix <= TTL_SECS;
    // A request to open an app names a valid phone and package, or is not one.
    let named = request.page != PHONE_APP
        || is_phone_app(
            request.phone.as_deref().unwrap_or(""),
            request.app.as_deref(),
        );
    (fresh && named).then_some(request)
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
                app: None,
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

    /// Apps part 2: a pinned app's request carries a valid phone and
    /// package, or none is made or taken.
    #[test]
    fn a_request_to_open_an_app_names_a_valid_phone_and_package() {
        let dir = std::env::temp_dir().join(format!("pd-app-open-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(request_phone_app(&dir, "phone-a", "org.example.app", 100).is_ok());
        let taken = take(&dir, 101).unwrap();
        assert_eq!(taken.page, PHONE_APP);
        assert_eq!(taken.phone.as_deref(), Some("phone-a"));
        assert_eq!(taken.app.as_deref(), Some("org.example.app"));
        for (phone, app) in [
            ("phone a", "org.example.app"),
            ("phone-a", "org.example.app;x"),
            ("", "org.example.app"),
            ("phone-a", "nodots"),
        ] {
            assert!(
                request_phone_app(&dir, phone, app, 100).is_err(),
                "{phone} {app}"
            );
        }
        // A file written by hand is held to the same rules when taken.
        std::fs::write(
            dir.join(APP_OPEN_FILE),
            r#"{"page":"phone_app","phone":"phone-a","app":"x y","created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take(&dir, 101), None);
        std::fs::write(
            dir.join(APP_OPEN_FILE),
            r#"{"page":"phone_app","phone":"phone-a","created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take(&dir, 101), None, "no app named");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quit_closes_only_a_window_that_was_open_when_it_was_asked() {
        let quit = OpenRequest {
            page: QUIT.into(),
            phone: None,
            app: None,
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
