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
    write(state_dir, page, phone, None, now_unix)
}

/// The folder of the state folder where each dropped window's request has a
/// file of its own, so one never overwrites another (the page requests
/// above share [`APP_OPEN_FILE`], one at a time).
pub const DROPS_DIR: &str = "app_open_drops";

/// The most dropped windows' requests waiting for the window at once: one
/// more is refused, which the computer that offered the window is told.
pub const MOST_DROPS: usize = 8;

/// Ask the window to open `dropped`, a window of the computer `computer`
/// dragged here. Both are checked as the packets' are, here and again when
/// the request is taken. The request waits in a file of its own
/// ([`DROPS_DIR`]); an error when [`MOST_DROPS`] already wait, so the offer
/// is refused rather than lost.
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
    let dir = state_dir.join(DROPS_DIR);
    crate::secure_fs::create_private_dir(&dir)?;
    let waiting = drop_files(&dir)
        .into_iter()
        .filter(|path| {
            // One nobody took in a long while is no one's.
            let stale = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age.as_secs() > TTL_SECS * 3);
            if stale {
                let _ = std::fs::remove_file(path);
            }
            !stale
        })
        .count();
    if waiting >= MOST_DROPS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "too many windows are waiting to be opened",
        ));
    }
    let request = OpenRequest {
        page: BORROW_DROP.to_string(),
        phone: Some(computer.to_string()),
        app: None,
        dropped: Some(dropped.clone()),
        created_unix: now_unix,
    };
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let path = dir.join(format!("drop-{now_unix}-{}.json", unique()));
    let tmp = crate::atomic_file::temp_beside(&path);
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &path)
}

/// A name no other request of this process, or of another, shares.
fn unique() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// The request files in the drops folder (not the temp files being written).
fn drop_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("drop-"))
        })
        .collect()
}

/// The dropped windows' requests waiting, taken (removed), oldest first:
/// each fresh and naming a valid computer and window, or dropped. At most
/// [`MOST_DROPS`] a call.
pub fn take_drops(state_dir: &Path, now_unix: u64) -> Vec<OpenRequest> {
    let mut taken: Vec<OpenRequest> = drop_files(&state_dir.join(DROPS_DIR))
        .into_iter()
        .take(MOST_DROPS)
        .filter_map(|path| {
            let raw = std::fs::read_to_string(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            serde_json::from_str::<OpenRequest>(&raw).ok()
        })
        .filter(|request| {
            request.page == BORROW_DROP
                && request.created_unix <= now_unix
                && now_unix - request.created_unix <= TTL_SECS
                && super::approval::is_valid_id(request.phone.as_deref().unwrap_or(""))
                && request
                    .dropped
                    .as_ref()
                    .is_some_and(DroppedWindow::is_sound)
        })
        .collect();
    taken.sort_by_key(|r| r.created_unix);
    taken
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
        dropped: None,
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
    // A dropped window's request has a file of its own ([`take_drops`]): one
    // here is no such request.
    let ordinary = request.page != BORROW_DROP;
    (fresh && named && ordinary).then_some(request)
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

    fn dropped(id: &str) -> DroppedWindow {
        DroppedWindow {
            window: id.into(),
            title: "A title".into(),
            x: -20,
            y: 30,
            width: 800,
            height: 600,
        }
    }

    fn drops_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-app-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Window handoff 4: a window dragged here is asked for with the
    /// computer and window named, and held to the same rules when taken.
    #[test]
    fn a_dropped_window_names_a_valid_computer_window_and_place() {
        let dir = drops_dir("drop");
        assert!(request_borrow_drop(&dir, "desk-1", &dropped("0x55d1"), 100).is_ok());
        let taken = take_drops(&dir, 101);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].page, BORROW_DROP);
        assert_eq!(taken[0].phone.as_deref(), Some("desk-1"));
        assert_eq!(taken[0].dropped, Some(dropped("0x55d1")));
        assert!(take_drops(&dir, 101).is_empty(), "taken once");
        for (computer, id) in [
            ("desk 1", "42"),
            ("", "42"),
            ("desk-1", "42; exec x"),
            ("desk-1", "0xZZ"),
            ("desk-1", ""),
        ] {
            assert!(
                request_borrow_drop(&dir, computer, &dropped(id), 100).is_err(),
                "{computer} {id}"
            );
        }
        let mut odd = dropped("42");
        odd.width = 10;
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        let mut odd = dropped("42");
        odd.x = 2_000_000;
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        let mut odd = dropped("42");
        odd.title = "x".repeat(201);
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        // A title that turns text around is not one the app is asked to show.
        let mut odd = dropped("42");
        odd.title = "safe\u{202E}txt.exe".to_string();
        assert!(request_borrow_drop(&dir, "desk-1", &odd, 100).is_err());
        // A file written by hand is held to the same rules when taken.
        let spool = dir.join(DROPS_DIR);
        std::fs::write(
            spool.join("drop-hand-1.json"),
            r#"{"page":"borrow_drop","phone":"desk-1","created_unix":100}"#,
        )
        .unwrap();
        std::fs::write(
            spool.join("drop-hand-2.json"),
            r#"{"page":"borrow_drop","phone":"desk-1","dropped":{"window":"4 2","title":"t","x":0,"y":0,"width":800,"height":600},"created_unix":100}"#,
        )
        .unwrap();
        std::fs::write(
            spool.join("drop-hand-3.json"),
            r#"{"page":"files","phone":"desk-1","dropped":{"window":"42","title":"t","x":0,"y":0,"width":800,"height":600},"created_unix":100}"#,
        )
        .unwrap();
        assert!(
            take_drops(&dir, 101).is_empty(),
            "no window, a bad id, another page"
        );
        // Stale, like any request.
        assert!(request_borrow_drop(&dir, "desk-1", &dropped("42"), 100).is_ok());
        assert!(take_drops(&dir, 100 + TTL_SECS + 1).is_empty());
        // The shared file takes no dropped window's request.
        std::fs::write(
            dir.join(APP_OPEN_FILE),
            r#"{"page":"borrow_drop","phone":"desk-1","dropped":{"window":"42","title":"t","x":0,"y":0,"width":800,"height":600},"created_unix":100}"#,
        )
        .unwrap();
        assert_eq!(take(&dir, 101), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two windows dropped at once are both opened: each request has a file
    /// of its own, and past [`MOST_DROPS`] waiting the next is refused (so
    /// the computer that offered it is told, never left to a lost file).
    #[test]
    fn dropped_windows_never_overwrite_one_another_and_a_full_spool_refuses() {
        let dir = drops_dir("spool");
        assert!(request_borrow_drop(&dir, "desk-1", &dropped("42"), 100).is_ok());
        assert!(request_borrow_drop(&dir, "desk-2", &dropped("0x5a"), 100).is_ok());
        // A page asked for beside them does not take their place either.
        request(&dir, "files", None, 100).unwrap();
        let taken = take_drops(&dir, 101);
        let mut windows: Vec<_> = taken
            .iter()
            .map(|r| r.dropped.clone().unwrap().window)
            .collect();
        windows.sort();
        assert_eq!(windows, ["0x5a", "42"], "both were kept");
        assert_eq!(take(&dir, 101).unwrap().page, "files");
        for n in 0..MOST_DROPS {
            assert!(
                request_borrow_drop(&dir, "desk-1", &dropped(&n.to_string()), 100).is_ok(),
                "{n}"
            );
        }
        let full = request_borrow_drop(&dir, "desk-1", &dropped("99"), 100).unwrap_err();
        assert_eq!(full.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(take_drops(&dir, 101).len(), MOST_DROPS);
        assert!(request_borrow_drop(&dir, "desk-1", &dropped("99"), 101).is_ok());
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
