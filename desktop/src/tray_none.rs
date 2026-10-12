//! Linux, for now: no tray icon.
//!
//! tray-icon's Linux backend builds its menu with GTK, which needs
//! `gtk::init` and a GTK main loop on its own thread; eframe provides
//! neither, and without them the menu panics at startup. Until that is built
//! and tested on a Linux desktop, the app runs as a plain window there:
//! `Tray::new` reports no tray, so closing the window quits. Same API as the
//! real tray, so the app does not care which one it got.
//!
//! The tray is never built here, so its types are never constructed: dead
//! code by design (API parity with `tray.rs`), hence the allowance.
#![allow(dead_code)]

use eframe::egui;

/// What the user picked from the tray menu (never, here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Open,
    Pair,
    Quit,
}

pub struct Tray;

impl Tray {
    pub fn new() -> Result<Self, String> {
        Err("there is no tray on Linux yet".to_string())
    }

    pub fn wake_on_click(_ctx: egui::Context) {}

    pub fn take_actions(&self) -> Vec<TrayAction> {
        Vec::new()
    }
}
