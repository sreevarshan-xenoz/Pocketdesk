//! Pocketdesk desktop app: pair phones, see and end sessions, read the log.
//!
//! It manages the server through the files they share (`core::local`), so it
//! works whether or not the server is running and opens no port of its own.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod activity_view;
mod app;
mod cast_view;
mod computers_view;
mod diagnostics_view;
mod fonts;
mod icon;
mod kdeconnect_view;
mod messages_view;
mod model;
mod nav;
mod notes_view;
mod notice;
mod pair_view;
mod phone_files_view;
mod photos_view;
mod pick_file;
mod restart;
mod reveal;
mod settings_view;
mod single_instance;
mod theme;
#[cfg(not(target_os = "linux"))]
mod tray;
#[cfg(test)]
mod ui_tests;
// N6: a StatusNotifierItem over D-Bus, no GTK.
#[cfg(target_os = "linux")]
#[path = "tray_linux.rs"]
mod tray;
mod words;

use eframe::egui;

const TITLE: &str = "Pocketdesk";

fn main() -> eframe::Result {
    let Some(instance) = single_instance::acquire(TITLE) else {
        // Another copy is running; it has been brought to the front.
        return Ok(());
    };
    // `--page <name>` opens on a page (nav.rs names them).
    let page = nav::page_from_args(std::env::args_os());
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
            // Room for the navigation and a page's widest rows (U1 review:
            // at 520x360 the navigation's last group was cut off).
            .with_inner_size([860.0, 600.0])
            .with_min_inner_size([640.0, 440.0])
            .with_icon(egui::IconData {
                rgba: icon::icon_rgba(64),
                width: 64,
                height: 64,
            }),
        // A throwaway instance (`POCKETDESK_CONFIG_DIR`, as R6a's second
        // server) keeps its window and page beside its config, never the
        // installed app's.
        persistence_path: std::env::var_os(pocketdesk_core::local::paths::CONFIG_DIR_VAR)
            .map(std::path::PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("app-window.ron")),
        ..Default::default()
    };
    // A later launch brings this window forward.
    instance.listen_for_launches();
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            // Glyphs for scripts egui's own fonts lack (U1).
            fonts::install(&cc.egui_ctx);
            // Lesser text readable in both themes (U1 review).
            theme::install(&cc.egui_ctx);
            if tray.is_some() {
                tray::Tray::wake_on_click(cc.egui_ctx.clone());
            }
            single_instance::wake_with(cc.egui_ctx.clone());
            // `--page`, else the page shown when the app last closed.
            let page = page.or_else(|| {
                cc.storage
                    .and_then(|s| s.get_string(app::PAGE_KEY))
                    .and_then(|key| app::Tab::from_key(&key))
                    .map(app::kept_page)
            });
            Ok(Box::new(app::PocketdeskApp::new(state_dir, tray, page)))
        }),
    )
}
