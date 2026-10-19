//! The tray icon and its menu ([`crate::tray_menu`]): Open, Pair a phone,
//! each paired phone's actions (D5), Quit.
//!
//! Menu clicks arrive on the menu's own channel. The window may be hidden
//! (closing it hides it to the tray), and a hidden eframe app only runs
//! `App::logic` when something requests a repaint, so the click handler
//! wakes it; `logic` then drains the channel.

use std::collections::HashMap;

use tray_icon::menu::{IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};

use crate::tray_menu::{Entry, TrayAction};

pub struct Tray {
    icon: TrayIcon,
    /// What each item of the menu shown does.
    actions: HashMap<MenuId, TrayAction>,
    /// The menu shown, so it is rebuilt only when it changes.
    shown: Vec<Entry>,
}

impl Tray {
    /// Build the tray icon. On Windows this must run on the thread that runs
    /// the window's event loop.
    pub fn new() -> Result<Self, String> {
        let shown = crate::tray_menu::menu(&[], &[], 0);
        let (menu, actions) = build(&shown)?;
        let icon = tray_icon::Icon::from_rgba(crate::icon::icon_rgba(32), 32, 32)
            .map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pocketdesk")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            actions,
            shown,
        })
    }

    /// Show `entries`, if they differ from the menu shown.
    pub fn show(&mut self, entries: Vec<Entry>) {
        if entries == self.shown {
            return;
        }
        match build(&entries) {
            Ok((menu, actions)) => {
                self.icon.set_menu(Some(Box::new(menu)));
                self.actions = actions;
                self.shown = entries;
            }
            Err(e) => eprintln!("Pocketdesk: the tray's menu was not rebuilt: {e}"),
        }
    }

    /// Call `wake` on every menu click: the window's repaint, even while it
    /// is hidden, or the tray program's own loop (E2).
    pub fn wake_on_click(wake: impl Fn() + Send + Sync + 'static) {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            PENDING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.id);
            wake();
        }));
    }

    /// Whether the tray has gone: never on Windows, where the icon lives as
    /// long as its process (Linux's can end with its D-Bus service).
    pub fn closed(&self) -> bool {
        false
    }

    /// The clicks since the last call, oldest first.
    pub fn take_actions(&self) -> Vec<TrayAction> {
        let ids: Vec<MenuId> =
            std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
        ids.into_iter()
            .filter_map(|id| self.actions.get(&id).cloned())
            .collect()
    }
}

/// The menu for `entries`, and what each of its items does.
fn build(entries: &[Entry]) -> Result<(Menu, HashMap<MenuId, TrayAction>), String> {
    let mut actions = HashMap::new();
    let menu = Menu::new();
    for entry in entries {
        let item = item(entry, &mut actions)?;
        menu.append(item.as_ref()).map_err(|e| e.to_string())?;
    }
    Ok((menu, actions))
}

fn item(
    entry: &Entry,
    actions: &mut HashMap<MenuId, TrayAction>,
) -> Result<Box<dyn IsMenuItem>, String> {
    Ok(match entry {
        Entry::Item { label, action } => match action {
            // Each action's id is the same in every menu built (D5 review):
            // a click queued just before a rebuild still finds it.
            Some(action) => {
                let item = MenuItem::with_id(action_id(action), text(label), true, None);
                actions.insert(item.id().clone(), action.clone());
                Box::new(item)
            }
            None => Box::new(MenuItem::new(text(label), false, None)),
        },
        Entry::Phone { label, items } => {
            let sub = Submenu::new(text(label), true);
            for entry in items {
                let item = item(entry, actions)?;
                sub.append(item.as_ref()).map_err(|e| e.to_string())?;
            }
            Box::new(sub)
        }
        Entry::Separator => Box::new(PredefinedMenuItem::separator()),
    })
}

/// The menu id an action's item has.
fn action_id(action: &TrayAction) -> String {
    match action {
        TrayAction::Open => "open".to_string(),
        TrayAction::Pair => "pair".to_string(),
        TrayAction::Quit => "quit".to_string(),
        TrayAction::Ring(id) => format!("ring:{id}"),
        TrayAction::Mute(id, minutes) => format!("mute:{minutes}:{id}"),
        TrayAction::SendClipboard(id) => format!("clipboard:{id}"),
        TrayAction::SendFile(id) => format!("file:{id}"),
        TrayAction::TakePhoto(id) => format!("photo:{id}"),
    }
}

/// A label as Windows shows it: `&` marks the next letter as the item's
/// key, so a name's own is doubled.
fn text(label: &str) -> String {
    label.replace('&', "&&")
}

/// Clicks waiting for the app. The handler runs on the event-loop thread
/// while the app is borrowed elsewhere, so they are queued here.
static PENDING: std::sync::Mutex<Vec<MenuId>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_has_its_own_id_and_names_keep_their_ampersands() {
        let actions = [
            TrayAction::Open,
            TrayAction::Pair,
            TrayAction::Quit,
            TrayAction::Ring("p".into()),
            TrayAction::Mute("p".into(), 60),
            TrayAction::Mute("p".into(), 0),
            TrayAction::SendClipboard("p".into()),
            TrayAction::SendFile("p".into()),
            TrayAction::TakePhoto("p".into()),
            TrayAction::Ring("q".into()),
        ];
        let ids: std::collections::HashSet<String> = actions.iter().map(action_id).collect();
        assert_eq!(ids.len(), actions.len());
        assert_eq!(text("Tom & Jerry"), "Tom && Jerry");
    }
}
