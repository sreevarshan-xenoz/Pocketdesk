//! The tray on Linux (N6): a StatusNotifierItem over D-Bus (`ksni`), with
//! the same icon, tooltip and menu as on Windows: Open Pocketdesk, Pair a
//! phone…, Quit; a click on the icon opens the window.
//!
//! tray-icon's Linux menu needs a GTK main loop the app does not run; this
//! needs none. ksni serves the item from a thread of its own with its own
//! D-Bus connection, so a pick arrives there: it is queued for the app and
//! the app woken, as the Windows tray does. With no StatusNotifierWatcher on
//! the session bus (a desktop with no tray host, such as GNOME without the
//! AppIndicator extension) `Tray::new` fails, and closing the window quits.

use std::sync::Mutex;

use eframe::egui;
use ksni::blocking::TrayMethods;

/// What the user picked from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Pair,
    Quit,
}

/// The menu's entries, in order; a separator goes before Quit.
const MENU: [(&str, TrayAction); 3] = [
    ("Open Pocketdesk", TrayAction::Open),
    ("Pair a phone…", TrayAction::Pair),
    ("Quit", TrayAction::Quit),
];

pub struct Tray {
    _handle: ksni::blocking::Handle<Item>,
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
        let (sent, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sent.send(Item { icon }.spawn().map_err(|e| e.to_string()));
        });
        match got.recv_timeout(REGISTER_WAIT) {
            Ok(handle) => Ok(Self { _handle: handle? }),
            Err(_) => Err(format!(
                "the tray host did not answer within {} s",
                REGISTER_WAIT.as_secs()
            )),
        }
    }

    /// Wake the app on every pick, even while its window is hidden.
    pub fn wake_on_click(ctx: egui::Context) {
        *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
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
static WAKE: Mutex<Option<egui::Context>> = Mutex::new(None);

fn picked(action: TrayAction) {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(action);
    if let Some(ctx) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        ctx.request_repaint();
    }
}

/// The item ksni serves.
pub struct Item {
    icon: Vec<ksni::Icon>,
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
        let mut items = Vec::new();
        for (label, action) in MENU {
            if action == TrayAction::Quit {
                items.push(ksni::MenuItem::Separator);
            }
            items.push(
                ksni::menu::StandardItem {
                    label: label.into(),
                    activate: Box::new(move |_: &mut Self| picked(action)),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray as _;

    #[test]
    fn the_menu_is_the_windows_one_and_each_entry_is_queued() {
        let item = Item { icon: Vec::new() };
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
}
