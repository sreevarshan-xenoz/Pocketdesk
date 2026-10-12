//! Pocketdesk desktop app: pair phones, see and end sessions, read the log.
//!
//! It manages the server through the files they share (`core::local`), so it
//! works whether or not the server is running and opens no port of its own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod model;
mod pair_view;
mod settings_view;
mod single_instance;
mod tray;

use eframe::egui;

const TITLE: &str = "Pocketdesk";

fn main() -> eframe::Result {
    let Some(_instance) = single_instance::acquire(TITLE) else {
        // Another copy is running; it has been brought to the front.
        return Ok(());
    };
    let state_dir = match linux_link_core::local::paths::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Pocketdesk: {e}");
            std::process::exit(1);
        }
    };

    // The tray must exist on the event-loop thread (Windows) before the
    // window opens. Without one (e.g. no StatusNotifier host), closing the
    // window quits instead of hiding.
    let tray = match tray::Tray::new() {
        Ok(tray) => Some(tray),
        Err(e) => {
            eprintln!("Pocketdesk: no tray icon ({e}); closing the window will quit");
            None
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([720.0, 480.0])
            .with_min_inner_size([520.0, 360.0])
            .with_icon(egui::IconData {
                rgba: tray::icon_rgba(64),
                width: 64,
                height: 64,
            }),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            if tray.is_some() {
                tray::Tray::wake_on_click(cc.egui_ctx.clone());
            }
            Ok(Box::new(app::PocketdeskApp::new(state_dir, tray)))
        }),
    )
}
