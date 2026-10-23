//! What the desktop app's window and its tray program share (E2): the
//! icon, the state the pages read, the tray and its menu, and the words.

pub mod hotkey;
// R4: the keys through the GlobalShortcuts portal.
#[cfg(target_os = "linux")]
pub mod hotkey_portal;
pub mod icon;
pub mod model;
pub mod phone_player;
#[cfg(not(target_os = "linux"))]
pub mod tray;
// N6: a StatusNotifierItem over D-Bus, no GTK.
#[cfg(target_os = "linux")]
#[path = "tray_linux.rs"]
pub mod tray;
pub mod tray_instance;
pub mod tray_menu;
pub mod tray_watch;
pub mod words;
