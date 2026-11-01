//! Screenshots and locks the computer asks a phone for (O2):
//! `<state>/phone_actions_out/<id>.json`, one file an ask, written by the
//! desktop app and taken (read and deleted) by the server, which sends it
//! to the phone if it is connected within a minute.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::approval::is_valid_id;
use super::paths::PHONE_ACTIONS_DIR;
use crate::protocol::phone_action::{PhoneAction, WindowAsk};

/// How long an ask waits for its phone: a minute, as a call does.
pub const ACTION_TTL_SECS: u64 = 60;

/// One ask of a phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneActionRequest {
    pub id: String,
    /// The phone (its paired device id).
    pub device_id: String,
    /// The action's word ([`PhoneAction::as_str`]).
    pub action: String,
    /// The app an `open` opens (P2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// An `open` that wants the phone's screen shown here too (Apps part 1).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub screen: bool,
    /// The file a `share` is for (Apps part 2): its name and size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// An `open` that wants the app in a window of its own (Phone apps
    /// tier 2), with that window's id and size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowAsk>,
    pub created_unix: u64,
}

/// What an ask carries beside its action.
#[derive(Default)]
struct Extras<'a> {
    app: Option<&'a str>,
    screen: bool,
    share: Option<(&'a str, u64)>,
    window: Option<WindowAsk>,
}

impl PhoneActionRequest {
    /// The action asked for, when it is one.
    pub fn action(&self) -> Option<PhoneAction> {
        PhoneAction::parse(&self.action)
    }
}

/// Ask the server to have `device_id` do `action`.
pub fn request(
    state_dir: &Path,
    device_id: &str,
    action: PhoneAction,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    write_request(state_dir, device_id, action, Extras::default(), now_unix)
}

/// Apps part 2: ask the server to have `device_id` share the file it is
/// being sent, `name` of `size` bytes, into the app in front there.
pub fn request_share(
    state_dir: &Path,
    device_id: &str,
    name: &str,
    size: u64,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    if !crate::protocol::phone_action::valid_share_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name",
        ));
    }
    write_request(
        state_dir,
        device_id,
        PhoneAction::Share,
        Extras {
            share: Some((name, size)),
            ..Extras::default()
        },
        now_unix,
    )
}

/// P2: ask the server to have `device_id` open the app `app`.
pub fn request_open(
    state_dir: &Path,
    device_id: &str,
    app: &str,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    request_open_with_screen(state_dir, device_id, app, false, now_unix)
}

/// Apps part 1: [`request_open`], and with `screen` the phone's screen
/// shown here as well (shared with any cast already running).
pub fn request_open_with_screen(
    state_dir: &Path,
    device_id: &str,
    app: &str,
    screen: bool,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    if !crate::protocol::phone_action::valid_app_id(app) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    write_request(
        state_dir,
        device_id,
        PhoneAction::Open,
        Extras {
            app: Some(app),
            screen,
            ..Extras::default()
        },
        now_unix,
    )
}

/// Phone apps tier 2: ask the server to have `device_id` open `app` in a
/// window of its own, `window` its id and size. The phone's screen is not
/// asked for.
pub fn request_open_window(
    state_dir: &Path,
    device_id: &str,
    app: &str,
    window: WindowAsk,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    if !crate::protocol::phone_action::valid_app_id(app) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app"));
    }
    if !window.is_sound() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an app window",
        ));
    }
    write_request(
        state_dir,
        device_id,
        PhoneAction::Open,
        Extras {
            app: Some(app),
            window: Some(window),
            ..Extras::default()
        },
        now_unix,
    )
}

fn write_request(
    state_dir: &Path,
    device_id: &str,
    action: PhoneAction,
    extras: Extras<'_>,
    now_unix: u64,
) -> io::Result<PhoneActionRequest> {
    let Extras {
        app,
        screen,
        share,
        window,
    } = extras;
    if device_id.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no phone"));
    }
    let request = PhoneActionRequest {
        id: uuid::Uuid::new_v4().to_string(),
        device_id: device_id.to_string(),
        action: action.as_str().to_string(),
        app: app.map(str::to_string),
        screen: screen && action == PhoneAction::Open,
        name: share.map(|(name, _)| name.to_string()),
        size: share.map(|(_, size)| size),
        window: window.filter(|_| action == PhoneAction::Open),
        created_unix: now_unix,
    };
    let file = path(state_dir, &request.id)?;
    crate::secure_fs::create_private_dir(&state_dir.join(PHONE_ACTIONS_DIR))?;
    let json = serde_json::to_string(&request).map_err(io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    crate::secure_fs::write_private(&tmp, json.as_bytes())?;
    crate::atomic_file::replace(&tmp, &file)?;
    Ok(request)
}

/// Take every waiting ask (the server's side), oldest first. Each file is
/// removed once read; one stale, from the future, unreadable, named for
/// another or not an action is dropped.
pub fn take_all(state_dir: &Path, now_unix: u64) -> Vec<PhoneActionRequest> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(PHONE_ACTIONS_DIR)) else {
        return Vec::new();
    };
    let mut taken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            .map(str::to_string)
        else {
            continue; // a temp file mid-write
        };
        let raw = std::fs::read_to_string(&path);
        let _ = std::fs::remove_file(&path);
        let Some(request) = raw
            .ok()
            .and_then(|raw| serde_json::from_str::<PhoneActionRequest>(&raw).ok())
        else {
            continue;
        };
        let fresh =
            request.created_unix <= now_unix && now_unix - request.created_unix <= ACTION_TTL_SECS;
        // An open names a valid app, and nothing else names one (P2).
        let app_right = match request.action() {
            Some(PhoneAction::Open) => request
                .app
                .as_deref()
                .is_some_and(crate::protocol::phone_action::valid_app_id),
            Some(_) => request.app.is_none(),
            None => false,
        };
        // A window is an open's, and a sound one (Phone apps tier 2).
        let window_right = request
            .window
            .is_none_or(|window| request.action() == Some(PhoneAction::Open) && window.is_sound());
        if fresh && request.id == stem && is_valid_id(&request.id) && app_right && window_right {
            taken.push(request);
        }
    }
    taken.sort_by_key(|r| r.created_unix);
    taken
}

fn path(state_dir: &Path, id: &str) -> io::Result<PathBuf> {
    if !is_valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not an ask's id",
        ));
    }
    Ok(state_dir.join(PHONE_ACTIONS_DIR).join(format!("{id}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pd-phone-actions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Apps part 2: a share names its file; one that names a path is not made.
    #[test]
    fn a_share_carries_its_file_and_refuses_a_path() {
        let dir = dir("share");
        let ask = request_share(&dir, "phone-a", "photo.png", 1234, 1_000).unwrap();
        assert_eq!(ask.action(), Some(PhoneAction::Share));
        assert_eq!(ask.name.as_deref(), Some("photo.png"));
        assert_eq!(ask.size, Some(1234));
        assert_eq!(take_all(&dir, 1_001), vec![ask]);
        for bad in ["", "a/b.png", "..\\x", "a\nb"] {
            assert!(
                request_share(&dir, "phone-a", bad, 1, 1_000).is_err(),
                "{bad:?}"
            );
        }
        assert!(request_share(&dir, "", "photo.png", 1, 1_000).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// O2: an ask is taken once within its minute, and a stale or unknown one dropped.
    #[test]
    fn an_ask_is_taken_once_within_its_time() {
        let dir = dir("take");
        let kept = request(&dir, "phone-a", PhoneAction::Screenshot, 1_000).unwrap();
        assert_eq!(kept.action(), Some(PhoneAction::Screenshot));
        assert_eq!(take_all(&dir, 1_010), vec![kept]);
        assert!(take_all(&dir, 1_010).is_empty(), "taken");
        request(&dir, "phone-a", PhoneAction::Lock, 1_000).unwrap();
        assert!(
            take_all(&dir, 1_000 + ACTION_TTL_SECS + 1).is_empty(),
            "stale"
        );
        // A file naming no action, written by hand, is dropped.
        let mut odd = request(&dir, "phone-a", PhoneAction::Lock, 1_000).unwrap();
        odd.action = "unlock".to_string();
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "not an action");
        assert!(request(&dir, "", PhoneAction::Lock, 1).is_err());
        // P2: an open carries its app; a bad one is refused or dropped.
        let open = request_open(&dir, "phone-a", "org.example.app", 1_000).unwrap();
        assert_eq!(take_all(&dir, 1_000), vec![open]);
        assert!(request_open(&dir, "phone-a", "bad id", 1_000).is_err());
        // Apps part 1: an open may want the screen, an ask for the list never does.
        let plain = request_open(&dir, "phone-a", "org.example.app", 1_000).unwrap();
        assert!(!plain.screen);
        let shown =
            request_open_with_screen(&dir, "phone-a", "org.example.app", true, 1_001).unwrap();
        assert!(shown.screen);
        assert_eq!(take_all(&dir, 1_001), vec![plain, shown]);
        assert!(
            !request(&dir, "phone-a", PhoneAction::Apps, 1_000)
                .unwrap()
                .screen
        );
        let _ = take_all(&dir, 1_000);
        let mut odd = request(&dir, "phone-a", PhoneAction::Open, 1_000).unwrap();
        odd.app = None;
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "an open without its app");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Phone apps tier 2: an open for a window carries its id and size, only an open
    /// does, and a window that is not a sound one is refused or dropped.
    #[test]
    fn an_open_may_ask_for_a_window_of_its_own() {
        let dir = dir("window");
        let ask = WindowAsk::standard(2);
        let open = request_open_window(&dir, "phone-a", "org.example.app", ask, 1_000).unwrap();
        assert_eq!(open.window, Some(ask));
        assert!(!open.screen, "a window leaves the phone's screen alone");
        assert_eq!(take_all(&dir, 1_000), vec![open]);
        for bad in [
            WindowAsk { id: 0, ..ask },
            WindowAsk { id: 5, ..ask },
            WindowAsk { width: 10, ..ask },
            WindowAsk {
                height: 9_000,
                ..ask
            },
            WindowAsk { dpi: 0, ..ask },
        ] {
            assert!(
                request_open_window(&dir, "phone-a", "org.example.app", bad, 1_000).is_err(),
                "{bad:?}"
            );
        }
        assert!(request_open_window(&dir, "phone-a", "bad id", ask, 1_000).is_err());
        // A file written by hand: a window on a lock, or an unsound one, is dropped.
        let mut odd = request(&dir, "phone-a", PhoneAction::Lock, 1_000).unwrap();
        odd.window = Some(ask);
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "a window on a lock");
        let mut odd = request_open(&dir, "phone-a", "org.example.app", 1_000).unwrap();
        odd.window = Some(WindowAsk { id: 9, ..ask });
        std::fs::write(
            path(&dir, &odd.id).unwrap(),
            serde_json::to_string(&odd).unwrap(),
        )
        .unwrap();
        assert!(take_all(&dir, 1_000).is_empty(), "an unsound window");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
