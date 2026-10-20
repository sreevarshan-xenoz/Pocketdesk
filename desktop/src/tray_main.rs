//! Pocketdesk's tray on its own (E2): the tray icon and its menu (D5), with
//! no window, so it holds a few MB at rest where the window holds 80
//! (D7). It reads the state files the window reads, writes the same
//! requests, and starts or raises the window for a page.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::Path;
use std::time::{Duration, Instant};

use pocketdesk_desktop::{
    hotkey, model, tray::Tray, tray_instance, tray_menu::TrayAction, tray_watch,
};

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
    // What only the window shows (E2 review): it is started for each new one.
    let mut seen = tray_watch::Seen::default();
    // H3: the key that sends the clipboard, as Settings has it now.
    let mut key = Key::default();
    // K7: a phone's player in this computer's media controls.
    #[cfg(windows)]
    let mut player = pocketdesk_desktop::phone_player::Watch::default();
    loop {
        let pressed = wait(&woken, REFRESH);
        if tray.closed() {
            // No icon any more: free the claim, so a tray can start again.
            return;
        }
        for action in tray.take_actions() {
            if !act(&state_dir, action) {
                return;
            }
        }
        if pressed || refreshed.is_none_or(|at| at.elapsed() >= REFRESH) {
            let now = now_unix();
            let snapshot = model::snapshot(&state_dir, now);
            key.follow(&state_dir);
            #[cfg(windows)]
            {
                let in_session: Vec<&str> = snapshot
                    .sessions
                    .iter()
                    .filter_map(|s| s.device_id.as_deref())
                    .collect();
                player.follow(&state_dir, &snapshot.phone_media, &in_session, now);
            }
            let stamp = model::session_log_stamp(&state_dir);
            if stamp != log.0 {
                log = (
                    stamp,
                    model::last_sessions(&model::session_log_tail(&state_dir)),
                );
            }
            let approvals: Vec<String> = snapshot.approvals.iter().map(|r| r.id.clone()).collect();
            if seen.any_new(tray_watch::waiting(&state_dir, &approvals, now))
                && let Err(e) = open_window(&state_dir, "overview", None)
            {
                eprintln!("Pocketdesk tray: the window did not start ({e})");
            }
            let rows = model::phone_rows(&snapshot, &log.1);
            // A press queued before its key was taken back is not acted on (H3 review).
            if pressed && key.status.registered {
                let to = hotkey::phone(&rows);
                // Said where the owner looks: this program has no console (H3 review).
                key.said_press(&state_dir, to.is_some());
                if let Some(id) = to {
                    act(&state_dir, TrayAction::SendClipboard(id.to_string()));
                }
            }
            let mut entries = pocketdesk_desktop::tray_menu::with_pause(
                pocketdesk_desktop::tray_menu::menu(&rows, &snapshot.devices, now),
                pocketdesk_core::local::pause::current(&state_dir, now).as_ref(),
                now,
            );
            // H4: the key names itself where its action is.
            if key.status.registered
                && let Some(phone) = hotkey::phone(&rows)
            {
                entries = pocketdesk_desktop::tray_menu::with_key(
                    entries,
                    phone,
                    &hotkey::display(&key.status.key),
                );
            }
            tray.show(entries);
            refreshed = Some(Instant::now());
        }
    }
}

/// The send-the-clipboard key's id with `RegisterHotKey` (H3).
#[cfg(windows)]
const HOTKEY_ID: i32 = 0x5044;

/// The send-the-clipboard key as registered (H3), kept to Settings: read
/// again at each refresh, and registered on this thread, which pumps the
/// `WM_HOTKEY` it posts. Windows only; elsewhere it does nothing.
#[derive(Default)]
struct Key {
    text: Option<String>,
    status: hotkey::Status,
    /// When a taken key was last tried again.
    tried: Option<Instant>,
    /// The config file's time when last read: it is read again only when it changes.
    stamp: Option<std::time::SystemTime>,
    #[cfg(windows)]
    registered: bool,
}

/// How often a key another app holds is tried again.
const RETRY_TAKEN: Duration = Duration::from_secs(30);

impl Key {
    /// Say a press's outcome in the status Settings shows.
    fn said_press(&mut self, state_dir: &Path, reached: bool) {
        self.status.no_phone = !reached;
        hotkey::write_status(state_dir, &self.status);
    }

    fn follow(&mut self, state_dir: &Path) {
        // A key another app held is tried again now and then (H3 review).
        let retry = self.status.taken && self.tried.is_none_or(|at| at.elapsed() >= RETRY_TAKEN);
        let stamp = pocketdesk_core::local::config::Config::path()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if self.text.is_some() && stamp == self.stamp && !retry {
            return;
        }
        // A config that does not read (a typo, a save under way) keeps the
        // key as it is (H3 review); it is read again when it changes.
        let text = match pocketdesk_core::local::config::Config::load() {
            Ok(config) => config.clipboard_hotkey.trim().to_string(),
            Err(_) if self.text.is_some() => return,
            Err(_) => String::new(),
        };
        self.stamp = stamp;
        if self.text.as_deref() == Some(text.as_str()) && !retry {
            return;
        }
        self.text = Some(text.clone());
        self.tried = Some(Instant::now());
        self.status = hotkey::Status {
            key: text.clone(),
            ..Default::default()
        };
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{
                HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
            };
            if self.registered {
                // SAFETY: this thread registered it.
                let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
                self.registered = false;
            }
            if let Some(key) = hotkey::parse(&text) {
                // SAFETY: no window: WM_HOTKEY goes to this thread's queue.
                match unsafe {
                    RegisterHotKey(None, HOTKEY_ID, HOT_KEY_MODIFIERS(key.modifiers), key.key)
                } {
                    Ok(()) => self.registered = true,
                    // Another app holds the combination.
                    Err(_) => self.status.taken = true,
                }
            }
            self.status.registered = self.registered;
        }
        hotkey::write_status(state_dir, &self.status);
    }
}

/// Wait up to `timeout` for a pick. On Windows the tray's menu works through
/// this thread's messages, so they are pumped meanwhile (a pick is queued
/// while its message is handled). Whether the send-the-clipboard key was
/// pressed (H3).
fn wait(woken: &std::sync::mpsc::Receiver<()>, timeout: Duration) -> bool {
    #[cfg(windows)]
    {
        let _ = woken;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            TranslateMessage, WM_HOTKEY,
        };
        // SAFETY: no handles to wait on; only this thread's messages.
        unsafe {
            MsgWaitForMultipleObjects(None, false, timeout.as_millis() as u32, QS_ALLINPUT);
        }
        let mut msg = MSG::default();
        let mut pressed = false;
        // SAFETY: a valid MSG to fill; each is translated and dispatched.
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // The thread's own WM_HOTKEY has no window to go to.
            if msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_ID as usize {
                pressed = true;
                continue;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        pressed
    }
    #[cfg(not(windows))]
    {
        let _ = woken.recv_timeout(timeout);
        false
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
        TrayAction::TakePhoto(id) => pocketdesk_core::local::phone_photos::request(
            state_dir,
            id,
            pocketdesk_core::local::phone_photos::Ask::Take,
            now,
        )
        .map(|_| ()),
        TrayAction::Pause(minutes) => {
            pocketdesk_core::local::pause::pause(state_dir, *minutes, now).map(|_| ())
        }
        TrayAction::Resume => pocketdesk_core::local::pause::resume(state_dir),
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
        // The tray program has no console (K2a review): a pause that failed
        // opens the Overview, which says what became of it.
        if matches!(action, TrayAction::Pause(_) | TrayAction::Resume) {
            let _ = open_window(state_dir, "overview", None);
        }
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
    tray_instance::start(&window)
}
