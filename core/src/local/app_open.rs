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
    /// The window a [`BORROW_DROP`] request opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<DroppedWindow>,
    pub created_unix: u64,
}

/// A window of another computer, dragged here and dropped (window handoff,
/// phase 4): the app opens it as a borrowed window, its top left at (`x`,
/// `y`) in this computer's desktop space, `width`x`height` to start with.
/// The computer is `phone` of the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedWindow {
    /// The window's id on the computer it comes from (Sway's container id
    /// or Hyprland's address).
    pub window: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DroppedWindow {
    /// Whether the window id could be one: digits, or `0x` and hex, as the
    /// compositors give them. A request is held to it as written and again
    /// as taken.
    fn is_sound(&self) -> bool {
        let sway = (1..=12).contains(&self.window.len())
            && self.window.bytes().all(|b| b.is_ascii_digit());
        let hypr = self.window.strip_prefix("0x").is_some_and(|hex| {
            (1..=16).contains(&hex.len())
                && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        });
        (sway || hypr)
            && (64..=16384).contains(&self.width)
            && (64..=16384).contains(&self.height)
            && self.x.unsigned_abs() <= 1_000_000
            && self.y.unsigned_abs() <= 1_000_000
            && self.title.chars().count() <= 200
            && !self.title.chars().any(super::shared_input::is_hidden)
    }
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

/// The page asked for to open a window of another computer that was dragged
/// here (window handoff 4), the computer's id in `phone` and the window in
/// `dropped`. It brings up no page and does not raise the window.
pub const BORROW_DROP: &str = "borrow_drop";

/// Ask the window to open `page` (for `phone`).
pub fn request(state_dir: &Path, page: &str, phone: Option<&str>, now_unix: u64) -> io::Result<()> {
    write(state_dir, page, phone, None, None, now_unix)
}

/// Ask the window to open `dropped`, a window of the computer `computer`
/// dragged here. Both are checked as the packets' are, here and again when
/// the request is taken.
pub fn request_borrow_drop(
    state_dir: &Path,
    computer: &str,
    dropped: &DroppedWindow,
    now_unix: u64,
) -> io::Result<()> {
    if !super::approval::is_valid_id(computer) || !dropped.is_sound() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a window of a computer",
        ));
    }
    write(
        state_dir,
        BORROW_DROP,
        Some(computer),
        None,
        Some(dropped.clone()),
        now_unix,
    )
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
    write(state_dir, PHONE_APP, Some(phone), Some(app), None, now_unix)
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
    dropped: Option<DroppedWindow>,
    now_unix: u64,
) -> io::Result<()> {
    let request = OpenRequest {
        page: page.to_string(),
        phone: phone.map(str::to_string),
        app: app.map(str::to_string),
        dropped,
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
    // A dropped window names a valid computer and window, or is not one.
    let dropped = request.page != BORROW_DROP
        || (super::approval::is_valid_id(request.phone.as_deref().unwrap_or(""))
            && request
                .dropped
                .as_ref()
                .is_some_and(DroppedWindow::is_sound));
    (fresh && named && dropped).then_some(request)
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
                dropped: None,
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

    /// Window handoff 4: a window dragged here is asked for with the
    /// computer and window named, and held to the same rules when taken.
    #[test]
    fn a_dropped_window_names_a_valid_computer_window_and_place() {
        let dir = std::env::temp_dir().join(format!("pd-app-open-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let window = |id: &str| DroppedWindow {
            window: id.into(),
            title: "A title".into(),
            x: -20,
            y: 30,
            width: 800,
            height: 600,
        };
        assert!(request_borrow_drop(&dir, "desk-1", &window("0x55d1"), 100).is_ok());
        let taken = take(&dir, 101).unwrap();
        assert_eq!(taken.page, BORROW_DROP);
        assert_eq!(taken.phone.as_deref(), Some("desk-1"));
        assert_eq!(taken.dropped, Some(window("0x55d1")));
        assert!(request_borrow_drop(&dir, "desk-1", &window("42"), 100).is_ok());
        for (computer, id) in [
            ("desk 1", "42"),
            ("", "42"),
            ("desk-1", "42; exec x"),
            ("desk-1", "0xZZ"),
            ("desk-1", ""),
        ] {
            assert!(
                request_borrow_drop(&dir, computer, &window(id), 100).is_err(),
                "{computer} {id}"
            );
        }
        let mut odd = window("42");
        odd.width = 10;
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        let mut odd = window("42");
        odd.x = 2_000_000;
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        let mut odd = window("42");
        odd.title = "x".repeat(201);
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        // A title that turns text around is not one the app is asked to show.
        let mut odd = window("42");
        odd.title = "safe\u{202E}txt.exe".to_string();
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        // A file written by hand is held to the same rules when taken.
        std::fs::write(
            dir.join(APP_OPEN_FILE),
            r#"{"page":"borrow_drop","phone":"desk-1","created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take(&dir, 101), None, "no window named");
        std::fs::write(
            dir.join(APP_OPEN_FILE),
            r#"{"page":"borrow_drop","phone":"desk-1","dropped":{"window":"4 2","title":"t","x":0,"y":0,"width":800,"height":600},"created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take(&dir, 101), None, "not a window id");
        // Stale, like any request.
        assert!(request_borrow_drop(&dir, "desk-1", &window("42"), 100).is_ok());
        assert_eq!(take(&dir, 100 + TTL_SECS + 1), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_quit_closes_only_a_window_that_was_open_when_it_was_asked() {
        let quit = OpenRequest {
            page: QUIT.into(),
            phone: None,
            app: None,
            dropped: None,
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
