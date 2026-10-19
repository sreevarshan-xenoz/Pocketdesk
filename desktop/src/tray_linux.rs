//! The tray on Linux (N6): a StatusNotifierItem over D-Bus (`ksni`), with
//! the same icon, tooltip and menu as on Windows ([`crate::tray_menu`]:
//! Open Pocketdesk, Pair a phone…, each paired phone's actions (D5), Quit);
//! a click on the icon opens the window.
//!
//! tray-icon's Linux menu needs a GTK main loop the app does not run; this
//! needs none. ksni serves the item from a thread of its own with its own
//! D-Bus connection, so a pick arrives there: it is queued for the app and
//! the app woken, as the Windows tray does. With no StatusNotifierWatcher on
//! the session bus (a desktop with no tray host, such as GNOME without the
//! AppIndicator extension) `Tray::new` fails, and closing the window quits.

use std::sync::Mutex;

use ksni::blocking::TrayMethods;

use crate::tray_menu::{Entry, TrayAction};

pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
    /// The menu shown, so it is replaced only when it changes.
    shown: Vec<Entry>,
}

impl Tray {
    /// Register the tray with the desktop's tray host.
    pub fn new() -> Result<Self, String> {
        let icon = [32, 64]
            .into_iter()
            .map(|size| ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: crate::icon::argb32(&crate::icon::icon_rgba(size)),
            })
            .collect();
        // A tray host that never answers must not hold up the window (N6
        // review): registration gets a few seconds, on a thread of its own.
        let shown = crate::tray_menu::menu(&[], &[], 0);
        let entries = shown.clone();
        let (sent, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(Item { icon, entries }.spawn().map_err(|e| e.to_string()));
        });
        match got.recv_timeout(REGISTER_WAIT) {
            Ok(handle) => Ok(Self {
                handle: handle?,
                shown,
            }),
            Err(_) => Err(format!(
                "the tray host did not answer within {} s",
                REGISTER_WAIT.as_secs()
            )),
        }
    }

    /// Show `entries`, if they differ from the menu shown.
    pub fn show(&mut self, entries: Vec<Entry>) {
        if entries == self.shown {
            return;
        }
        let next = entries.clone();
        // `None`: the tray's service has ended; nothing to update.
        if self
            .handle
            .update(move |item| item.entries = next)
            .is_some()
        {
            self.shown = entries;
        }
    }

    /// Wake the app on every pick, even while its window is hidden.
    pub fn wake_on_click(wake: impl Fn() + Send + 'static) {
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(wake));
    }

    /// The picks since the last call, oldest first.
    pub fn take_actions(&self) -> Vec<TrayAction> {
        std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// How long registering with the tray host may take before the app goes on
/// without a tray.
const REGISTER_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Picks waiting for the app, made on ksni's thread.
static PENDING: Mutex<Vec<TrayAction>> = Mutex::new(Vec::new());

/// The app's context, to wake it from ksni's thread.
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn picked(action: TrayAction) {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(action);
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        wake();
    }
}

/// The item ksni serves.
pub struct Item {
    icon: Vec<ksni::Icon>,
    entries: Vec<Entry>,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "pocketdesk".into()
    }

    fn title(&self) -> String {
        "Pocketdesk".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Pocketdesk".into(),
            ..Default::default()
        }
    }

    /// A click on the icon opens the window, as on Windows.
    fn activate(&mut self, _x: i32, _y: i32) {
        picked(TrayAction::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.entries.iter().map(item).collect()
    }
}

fn item(entry: &Entry) -> ksni::MenuItem<Item> {
    match entry {
        Entry::Item { label, action } => ksni::menu::StandardItem {
            label: text(label),
            enabled: action.is_some(),
            activate: match action.clone() {
                Some(action) => Box::new(move |_: &mut Item| picked(action.clone())),
                None => Box::new(|_: &mut Item| {}),
            },
            ..Default::default()
        }
        .into(),
        Entry::Phone { label, items } => ksni::menu::SubMenu {
            label: text(label),
            submenu: items.iter().map(item).collect(),
            ..Default::default()
        }
        .into(),
        Entry::Separator => ksni::MenuItem::Separator,
    }
}

/// A label as a tray host shows it: `_` marks the next letter as the
/// item's key, so a name's own is doubled.
fn text(label: &str) -> String {
    label.replace('_', "__")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    /// The tests share the queue of picks.
    static QUEUE: Mutex<()> = Mutex::new(());

    #[test]
    fn the_menu_is_the_windows_one_and_each_entry_is_queued() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let item = Item {
            icon: Vec::new(),
            entries: crate::tray_menu::menu(&[], &[], 0),
        };
        let menu = item.menu();
        let labels: Vec<String> = menu
            .iter()
            .map(|m| match m {
                ksni::MenuItem::Standard(s) => s.label.clone(),
                ksni::MenuItem::Separator => "-".into(),
                _ => "?".into(),
            })
            .collect();
        assert_eq!(labels, ["Open Pocketdesk", "Pair a phone…", "-", "Quit"]);
        let mut item = item;
        for entry in &menu {
            if let ksni::MenuItem::Standard(s) = entry {
                (s.activate)(&mut item);
            }
        }
        item.activate(0, 0);
        let tray_actions = std::mem::take(&mut *PENDING.lock().unwrap());
        assert_eq!(
            tray_actions,
            [
                TrayAction::Open,
                TrayAction::Pair,
                TrayAction::Quit,
                TrayAction::Open
            ]
        );
    }

    /// D5: a phone is a submenu, its disabled items picking nothing, and
    /// a name's underscore is shown as itself.
    #[test]
    fn a_phone_is_a_submenu() {
        let _queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let item = Item {
            icon: Vec::new(),
            entries: vec![Entry::Phone {
                label: "my_phone".to_string(),
                items: vec![
                    Entry::Item {
                        label: "Ring".to_string(),
                        action: Some(TrayAction::Ring("p".to_string())),
                    },
                    Entry::Item {
                        label: "Send file…".to_string(),
                        action: None,
                    },
                ],
            }],
        };
        let menu = item.menu();
        let ksni::MenuItem::SubMenu(sub) = &menu[0] else {
            panic!("not a submenu");
        };
        assert_eq!(sub.label, "my__phone");
        let ksni::MenuItem::Standard(off) = &sub.submenu[1] else {
            panic!("not an item");
        };
        assert!(!off.enabled);
        let mut item = item;
        for entry in &sub.submenu {
            if let ksni::MenuItem::Standard(s) = entry {
                (s.activate)(&mut item);
            }
        }
        assert_eq!(
            std::mem::take(&mut *PENDING.lock().unwrap()),
            [TrayAction::Ring("p".to_string())]
        );
    }
}
