//! Opening a phone's app for a request that does not come from a click on
//! the Phone apps page (Apps part 2): a pinned app's shortcut, and a
//! phone notification's "Open the app on the phone". It asks as the page's
//! click does (the phone's screen shown here too), then watches the phone's
//! word on it, so that the app's window comes up when it opened and the
//! one line says what is wrong when it did not.
//!
//! The phone decides everything that matters (its switch, its own
//! computer, its lock, its list of apps); this side only checks that the
//! app is one the phone listed, so that nothing else is ever asked for.

use std::path::Path;

use pocketdesk_core::local::phone_apps::{self as store, AppList};
use pocketdesk_core::protocol::permissions::Capability;
use pocketdesk_core::protocol::phone_action::{AppResult, PhoneAction};

use crate::apps_view::{Known, Opening, Status, status};
use crate::model::Device;
#[cfg(test)]
use crate::phone_windows::Route;

/// How long a wait for the phone's owner (one tap, or the screen question)
/// is watched before it is given up on.
const WATCH_SECS: u64 = 120;

/// How long a refresh of the phone's list is given before the open is
/// asked again.
const RELIST_SECS: u64 = 3;

/// Whether `app` on `device` can be asked for, and its name as the phone
/// listed it; the words to say when not. `who` is the phone's name as shown.
pub fn check(
    device: Option<&Device>,
    list: Option<&AppList>,
    app: &str,
    who: &str,
) -> Result<String, String> {
    let Some(device) = device.filter(|d| !d.computer) else {
        return Err("That phone is not paired with this computer.".into());
    };
    if !device.permissions.allows(Capability::PhoneActions) {
        return Err(format!(
            "{who} may not be asked to open apps: \"Screenshot, lock and open its apps\" is off for it in Devices."
        ));
    }
    if !device.linked {
        return Err(format!(
            "{who} is not linked now. Open Pocketdesk on it, or turn on one of its background features, to link it."
        ));
    }
    list.and_then(|l| l.apps.iter().find(|a| a.id == app))
        .map(|a| a.name.clone())
        .ok_or_else(|| {
            format!(
                "{who} has not listed that app: open the Phone apps page once, with \"Let the computer open my apps\" on there."
            )
        })
}

/// What an open is for: the phone (its id and the name shown for it) and the
/// app (its package name and the name shown for it).
pub struct OpenAsk<'a> {
    pub phone: &'a str,
    pub who: &'a str,
    pub app: &'a str,
    pub name: &'a str,
}

/// One open being watched.
struct Pending {
    phone: String,
    who: String,
    open: Opening,
    /// How the open is made, kept for the ask that follows a new list.
    route: crate::phone_windows::Route,
    /// The first ask of the open, to give up on a long wait by.
    first_at: u64,
    /// The phone did not know the app, so its list was asked for again and
    /// the open follows it, once.
    relisted: bool,
    /// When the open is asked again after the list.
    retry_at: Option<u64>,
    /// What was last said of a wait for the phone's owner.
    said: Option<AppResult>,
}

/// What a look at the phone's word makes of it.
#[derive(Debug, Default)]
pub struct Done {
    /// The one line to say, if any.
    pub notice: Option<crate::notice::Notice>,
    /// The app opened: the phone's screen window should come up (not for an
    /// app window, which comes up by itself).
    pub raise: bool,
    /// An app window that was asked for and will not come (the phone could
    /// not make it): free again.
    pub released: Option<u8>,
}

/// The opens being watched (one at a time: a new one replaces the last).
#[derive(Default)]
pub struct OpenWatch {
    pending: Option<Pending>,
}

impl OpenWatch {
    /// Ask `phone` to open `app` (called `name`) with its screen shown here.
    ///
    /// With a phone that offers windows (Phone apps tier 2) and this
    /// computer's switch on, `route` makes it an app window and the id is
    /// returned, for the caller to hold.
    pub fn start(
        &mut self,
        state_dir: &Path,
        ask: &OpenAsk<'_>,
        route: &crate::phone_windows::Route,
        now: u64,
    ) -> Result<Option<u8>, String> {
        let window =
            match crate::phone_windows::ask_open(state_dir, ask.phone, ask.app, route, now)? {
                crate::phone_windows::Asked::Window(id) => Some(id),
                crate::phone_windows::Asked::Screen => None,
            };
        self.pending = Some(Pending {
            phone: ask.phone.to_string(),
            who: ask.who.to_string(),
            open: Opening {
                app: ask.app.to_string(),
                name: ask.name.to_string(),
                asked_at: now,
                window,
            },
            route: route.clone(),
            first_at: now,
            relisted: false,
            retry_at: None,
            said: None,
        });
        Ok(window)
    }

    /// Whether an open is being watched.
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Look at what the phone said, and act on it once.
    pub fn tick(&mut self, state_dir: &Path, now: u64) -> Done {
        let Some(p) = self.pending.as_mut() else {
            return Done::default();
        };
        // The list was asked for again: the open follows it.
        if let Some(at) = p.retry_at {
            if now < at {
                return Done::default();
            }
            p.retry_at = None;
            let asked =
                crate::phone_windows::ask_open(state_dir, &p.phone, &p.open.app, &p.route, now);
            let Ok(asked) = asked else {
                self.pending = None;
                return Done {
                    notice: crate::notice::Notice::error("Could not ask the phone."),
                    ..Done::default()
                };
            };
            p.open.window = match asked {
                crate::phone_windows::Asked::Window(id) => Some(id),
                crate::phone_windows::Asked::Screen => None,
            };
            p.open.asked_at = now;
            return Done::default();
        }
        let outcome = store::read_results(state_dir).remove(&p.phone);
        let known = Known {
            has_phone: true,
            allowed: true,
            linked: true,
            list_asked_at: None,
            listed_at: None,
            outcome: outcome.as_ref(),
            opening: Some(&p.open),
            now,
        };
        let status = status(&known);
        let who = p.who.clone();
        match status {
            Status::Opened(name) => {
                let _ = store::note_open(state_dir, &p.phone, &p.open.app, now);
                // An app window comes up by itself.
                let window = p.open.window.is_some();
                self.pending = None;
                Done {
                    notice: crate::notice::Notice::done(format!("Opened {name} on {who}.")),
                    raise: !window,
                    ..Done::default()
                }
            }
            // The phone could not make a window: the app opens the ordinary way.
            Status::OpenRefused(AppResult::WindowsUnavailable, _) if p.open.window.is_some() => {
                let released = p.open.window;
                p.route = p.route.screen_only();
                let asked =
                    crate::phone_windows::ask_open(state_dir, &p.phone, &p.open.app, &p.route, now);
                if asked.is_err() {
                    self.pending = None;
                    return Done {
                        notice: crate::notice::Notice::error("Could not ask the phone."),
                        released,
                        ..Done::default()
                    };
                }
                p.open.window = None;
                p.open.asked_at = now;
                Done {
                    released,
                    ..Done::default()
                }
            }
            // The phone's list is asked for again once, and the open after it.
            Status::OpenRefused(AppResult::NotListed, _) if !p.relisted => {
                p.relisted = true;
                p.retry_at = Some(now + RELIST_SECS);
                let _ = pocketdesk_core::local::phone_actions::request(
                    state_dir,
                    &p.phone,
                    PhoneAction::Apps,
                    now,
                );
                Done::default()
            }
            Status::NeedsScreen(_) | Status::NeedsTap(_) => {
                let word = if matches!(status, Status::NeedsTap(_)) {
                    AppResult::NeedsTap
                } else {
                    AppResult::NeedsScreen
                };
                if now.saturating_sub(p.first_at) > WATCH_SECS {
                    self.pending = None;
                    return Done::default();
                }
                if p.said == Some(word) {
                    return Done::default();
                }
                p.said = Some(word);
                Done {
                    notice: crate::notice::Notice::working(status.message(&who)),
                    ..Done::default()
                }
            }
            Status::Opening(_) => Done::default(),
            other => {
                // A window asked for and refused (the phone's limit) is free again.
                let released = p.open.window;
                self.pending = None;
                Done {
                    notice: crate::notice::Notice::error(other.message(&who)),
                    released,
                    ..Done::default()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocketdesk_core::local::phone_apps::AppList;
    use pocketdesk_core::protocol::phone_action::{AppReport, PhoneApp};

    fn dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pd-phone-open-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn list(ids: &[&str]) -> AppList {
        AppList {
            apps: ids
                .iter()
                .map(|id| PhoneApp {
                    id: (*id).into(),
                    name: format!("Name of {id}"),
                    version: 1,
                })
                .collect(),
            at: 1,
            windows: Default::default(),
        }
    }

    fn said(dir: &Path, phone: &str, app: &str, result: AppResult, at: u64) {
        store::keep_result(
            dir,
            phone,
            AppReport {
                about: "open".into(),
                app: Some(app.into()),
                name: None,
                window: None,
                result,
            },
            at,
            &|_| true,
        )
        .unwrap();
    }

    const ASK: OpenAsk<'static> = OpenAsk {
        phone: "phone-1",
        who: "My phone",
        app: "org.example.app",
        name: "Example",
    };

    fn asks(
        dir: &Path,
        now: u64,
    ) -> Vec<pocketdesk_core::local::phone_actions::PhoneActionRequest> {
        pocketdesk_core::local::phone_actions::take_all(dir, now)
    }

    fn device(linked: bool, allowed: bool) -> Device {
        let mut device = Device {
            id: "phone-1".to_string(),
            linked,
            ..Default::default()
        };
        device.permissions.set(Capability::PhoneActions, allowed);
        device
    }

    /// Only an app the phone listed, for a paired linked phone that may be
    /// asked, is asked for; each other case says why.
    #[test]
    fn only_a_listed_app_of_a_phone_that_may_be_asked_is_asked_for() {
        let l = list(&["org.example.app"]);
        let none: Option<&Device> = None;
        assert!(check(none, Some(&l), "org.example.app", "Phone").is_err());
        assert!(check(none, None, "org.example.app", "Phone").is_err());
        let dev = device(true, true);
        assert_eq!(
            check(Some(&dev), Some(&l), "org.example.app", "Phone").unwrap(),
            "Name of org.example.app"
        );
        assert!(check(Some(&dev), Some(&l), "org.example.other", "Phone").is_err());
        assert!(check(Some(&dev), None, "org.example.app", "Phone").is_err());
        let unlinked = device(false, true);
        assert!(
            check(Some(&unlinked), Some(&l), "org.example.app", "Phone")
                .unwrap_err()
                .contains("not linked")
        );
        let denied = device(true, false);
        assert!(
            check(Some(&denied), Some(&l), "org.example.app", "Phone")
                .unwrap_err()
                .contains("may not be asked")
        );
    }

    /// The phone's answers decide the one line and whether the window comes up.
    #[test]
    fn the_phone_s_word_decides_what_is_said() {
        let dir = dir("word");
        let mut watch = OpenWatch::default();
        assert!(!watch.waiting());
        assert!(watch.tick(&dir, 100).notice.is_none());
        watch.start(&dir, &ASK, &Route::default(), 100).unwrap();
        assert_eq!(asks(&dir, 100).len(), 1);
        assert!(watch.waiting());
        // Nothing yet: still waiting.
        assert!(watch.tick(&dir, 101).notice.is_none());
        // The owner is asked once at the phone: said once, still watched.
        said(
            &dir,
            "phone-1",
            "org.example.app",
            AppResult::NeedsScreen,
            102,
        );
        let first = watch.tick(&dir, 102);
        assert!(first.notice.is_some() && !first.raise);
        assert!(watch.tick(&dir, 103).notice.is_none(), "said once");
        assert!(watch.waiting());
        // Then it opened: the screen window comes up and it is a recent app.
        said(&dir, "phone-1", "org.example.app", AppResult::Opened, 104);
        let done = watch.tick(&dir, 104);
        assert!(done.raise);
        assert!(done.notice.unwrap().text.contains("Opened Example"));
        assert!(!watch.waiting());
        assert_eq!(store::read_recent(&dir)["phone-1"]["org.example.app"], 104);
        // A refusal says why and ends the watch.
        watch.start(&dir, &ASK, &Route::default(), 200).unwrap();
        said(
            &dir,
            "phone-1",
            "org.example.app",
            AppResult::LockedSecure,
            201,
        );
        let refused = watch.tick(&dir, 201);
        assert!(!refused.raise);
        assert!(refused.notice.unwrap().text.contains("never bypasses"));
        assert!(!watch.waiting());
        // No answer in time is said too.
        watch.start(&dir, &ASK, &Route::default(), 300).unwrap();
        let late = watch.tick(&dir, 300 + 60);
        assert!(late.notice.unwrap().text.contains("did not answer"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A phone that forgot its list is asked for it, and the open follows once.
    #[test]
    fn a_forgotten_list_is_asked_for_and_the_open_follows_once() {
        let dir = dir("relist");
        let mut watch = OpenWatch::default();
        watch.start(&dir, &ASK, &Route::default(), 100).unwrap();
        asks(&dir, 100);
        said(
            &dir,
            "phone-1",
            "org.example.app",
            AppResult::NotListed,
            101,
        );
        assert!(watch.tick(&dir, 101).notice.is_none());
        let list_ask = asks(&dir, 101);
        assert_eq!(list_ask.len(), 1);
        assert_eq!(list_ask[0].action, "apps");
        assert!(
            watch.tick(&dir, 102).notice.is_none(),
            "waiting for the list"
        );
        assert!(asks(&dir, 102).is_empty());
        assert!(watch.tick(&dir, 101 + RELIST_SECS).notice.is_none());
        let again = asks(&dir, 104);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].action, "open");
        assert!(again[0].screen);
        // Not listed again: said, not asked a third time.
        said(
            &dir,
            "phone-1",
            "org.example.app",
            AppResult::NotListed,
            110,
        );
        let end = watch.tick(&dir, 110);
        assert!(end.notice.is_some());
        assert!(asks(&dir, 110).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
