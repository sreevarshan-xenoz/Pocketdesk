//! What the desktop app's window and its tray program share (E2): the
//! icon, the state the pages read, the tray and its menu, and the words.

pub mod hotkey;
pub mod icon;
pub mod model;
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
