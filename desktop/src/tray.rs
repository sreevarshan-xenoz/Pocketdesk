//! The tray icon and its menu: Open, Pair a phone…, Quit.
//!
//! Menu clicks arrive on the menu's own channel. The window may be hidden
//! (closing it hides it to the tray), and a hidden eframe app only runs
//! `App::logic` when something requests a repaint, so the click handler
//! wakes it; `logic` then drains the channel.

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// What the user picked from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Pair,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    open: MenuId,
    pair: MenuId,
    quit: MenuId,
}

impl Tray {
    /// Build the tray icon. On Windows this must run on the thread that runs
    /// the window's event loop.
    pub fn new() -> Result<Self, String> {
        let open = MenuItem::new("Open Pocketdesk", true, None);
        let pair = MenuItem::new("Pair a phone…", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &pair, &PredefinedMenuItem::separator(), &quit])
            .map_err(|e| e.to_string())?;
        let icon = tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pocketdesk")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _icon: tray,
            open: open.id().clone(),
            pair: pair.id().clone(),
            quit: quit.id().clone(),
        })
    }

    /// Wake the app on every menu click, even while its window is hidden.
    pub fn wake_on_click(ctx: egui::Context) {
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            PENDING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(event.id);
            ctx.request_repaint();
        }));
    }

    /// The clicks since the last call, oldest first.
    pub fn take_actions(&self) -> Vec<TrayAction> {
        let ids: Vec<MenuId> =
            std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
        ids.into_iter()
            .filter_map(|id| {
                if id == self.open {
                    Some(TrayAction::Open)
                } else if id == self.pair {
                    Some(TrayAction::Pair)
                } else if id == self.quit {
                    Some(TrayAction::Quit)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Clicks waiting for the app. The handler runs on the event-loop thread
/// while the app is borrowed elsewhere, so they are queued here.
static PENDING: std::sync::Mutex<Vec<MenuId>> = std::sync::Mutex::new(Vec::new());

/// The app icon, drawn rather than shipped: a monitor on the brand pine
/// plate, readable at 16 px.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let pine = [0x1f, 0x4e, 0x4a, 0xff];
    let screen = [0xf2, 0xf5, 0xf4, 0xff];
    let amber = [0xf2, 0xa9, 0x3b, 0xff];
    let s = size as f32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            // Rounded plate: drop the four corner pixels' worth of each corner.
            let corner = (fx < 0.12 || fx > 0.88) && (fy < 0.12 || fy > 0.88);
            let in_screen = (0.18..0.82).contains(&fx) && (0.22..0.62).contains(&fy);
            let in_stand = (0.44..0.56).contains(&fx) && (0.62..0.74).contains(&fy);
            let in_base = (0.32..0.68).contains(&fx) && (0.74..0.80).contains(&fy);
            let pixel = if corner {
                [0, 0, 0, 0]
            } else if in_screen && (0.62..0.74).contains(&fx) && (0.46..0.56).contains(&fy) {
                amber
            } else if in_screen || in_stand || in_base {
                screen
            } else {
                pine
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_square_rgba_with_transparent_corners() {
        let icon = icon_rgba(32);
        assert_eq!(icon.len(), 32 * 32 * 4);
        assert_eq!(icon[3], 0, "the corner is transparent");
        let centre = ((16 * 32 + 16) * 4) as usize;
        assert_eq!(icon[centre + 3], 0xff);
    }
}
