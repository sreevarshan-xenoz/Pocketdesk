//! Pocketdesk desktop app: pair phones, see and end sessions, read the log.
//!
//! It manages the server through the files they share (`core::local`), so it
//! works whether or not the server is running and opens no port of its own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod activity_view;
mod app;
mod cast_view;
mod computers_view;
mod kdeconnect_view;
mod diagnostics_view;
mod icon;
mod messages_view;
mod model;
mod notes_view;
mod pair_view;
mod phone_files_view;
mod pick_file;
mod restart;
mod reveal;
mod settings_view;
mod single_instance;
#[cfg(not(target_os = "linux"))]
mod tray;
#[cfg(target_os = "linux")]
#[path = "tray_none.rs"]
mod tray;

use eframe::egui;

const TITLE: &str = "Pocketdesk";

fn main() -> eframe::Result {
    let Some(_instance) = single_instance::acquire(TITLE) else {
        // Another copy is running; it has been brought to the front.
        return Ok(());
    };
    let state_dir = match pocketdesk_core::local::paths::state_dir() {
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
                rgba: icon::icon_rgba(64),
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

/// Type-check the Linux stub on the build hosts that cannot build Linux.
#[cfg(all(test, not(target_os = "linux")))]
#[path = "tray_none.rs"]
mod tray_none_check;
