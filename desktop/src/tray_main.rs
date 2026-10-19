//! Pocketdesk's tray on its own (E2): the tray icon and its menu (D5), with
//! no window, so it holds a few MB at rest where the window holds 80
//! (D7). It reads the state files the window reads, writes the same
//! requests, and starts or raises the window for a page.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::time::{Duration, Instant};

use pocketdesk_desktop::{model, tray::Tray, tray_instance, tray_menu::TrayAction};

/// How often the menu is brought up to date with the phones.
const REFRESH: Duration = Duration::from_secs(1);

fn now_unix() -> u64 {
    pocketdesk_core::local::pin::now_unix()
}

fn main() {
    let Some(_claim) = tray_instance::claim() else {
        // Another tray program holds the tray.
        return;
    };
    let state_dir = match pocketdesk_core::local::paths::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Pocketdesk tray: {e}");
            std::process::exit(1);
        }
    };
    let mut tray = match Tray::new() {
        Ok(tray) => tray,
        Err(e) => {
            eprintln!("Pocketdesk tray: no tray icon ({e})");
            std::process::exit(1);
        }
    };
    let (wake, woken) = std::sync::mpsc::channel::<()>();
    Tray::wake_on_click(move || {
        let _ = wake.send(());
    });
    let mut refreshed: Option<Instant> = None;
    // The session log's tail, read again only when it changes (N6 review).
    let mut log = (None, std::collections::HashMap::new());
    loop {
        wait(&woken, REFRESH);
        for action in tray.take_actions() {
            if !act(&state_dir, action) {
                return;
            }
        }
        if refreshed.is_none_or(|at| at.elapsed() >= REFRESH) {
            let now = now_unix();
            let snapshot = model::snapshot(&state_dir, now);
            let stamp = model::session_log_stamp(&state_dir);
            if stamp != log.0 {
                log = (
                    stamp,
                    model::last_sessions(&model::session_log_tail(&state_dir)),
                );
            }
            let rows = model::phone_rows(&snapshot, &log.1);
            tray.show(pocketdesk_desktop::tray_menu::menu(
                &rows,
                &snapshot.devices,
                now,
            ));
            refreshed = Some(Instant::now());
        }
    }
}

/// Wait up to `timeout` for a pick. On Windows the tray's menu works through
/// this thread's messages, so they are pumped meanwhile (a pick is queued
/// while its message is handled).
fn wait(woken: &std::sync::mpsc::Receiver<()>, timeout: Duration) {
    #[cfg(windows)]
    {
        let _ = woken;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            TranslateMessage,
        };
        // SAFETY: no handles to wait on; only this thread's messages.
        unsafe {
            MsgWaitForMultipleObjects(None, false, timeout.as_millis() as u32, QS_ALLINPUT);
        }
        let mut msg = MSG::default();
        // SAFETY: a valid MSG to fill; each is translated and dispatched.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = woken.recv_timeout(timeout);
    }
}

/// Do what was picked; whether the tray goes on.
fn act(state_dir: &Path, action: TrayAction) -> bool {
    let now = now_unix();
    let done = match &action {
        TrayAction::Ring(id) => {
            pocketdesk_core::local::rings::request(state_dir, id, now).map(|_| ())
        }
        TrayAction::Mute(id, minutes) => {
            pocketdesk_core::local::quiet::request(state_dir, id, *minutes, now).map(|_| ())
        }
        TrayAction::SendClipboard(id) => {
            pocketdesk_core::local::clip_history::request_current(state_dir, id, now).map(|_| ())
        }
        TrayAction::Open => open_window(state_dir, "overview", None),
        TrayAction::Pair => open_window(state_dir, "pair", None),
        TrayAction::SendFile(id) => open_window(state_dir, "files", Some(id)),
        TrayAction::Quit => {
            // The window goes too, when it runs (E2).
            let _ = pocketdesk_core::local::app_open::request(
                state_dir,
                pocketdesk_core::local::app_open::QUIT,
                None,
                now,
            );
            return false;
        }
    };
    if let Err(e) = done {
        eprintln!("Pocketdesk tray: {action:?} failed: {e}");
    }
    true
}

/// Ask the window for `page` (for `phone`), and start it: a window that
/// runs already is raised by the new one, which then exits.
fn open_window(state_dir: &Path, page: &str, phone: Option<&str>) -> std::io::Result<()> {
    pocketdesk_core::local::app_open::request(state_dir, page, phone, now_unix())?;
    let exe = std::env::current_exe()?;
    let window = exe.with_file_name(if cfg!(windows) {
        "pocketdesk-app.exe"
    } else {
        "pocketdesk-app"
    });
    std::process::Command::new(window).spawn().map(|_| ())
}
