//! Desktop-side privacy mode (Tier-3 #15).
//!
//! While a remote session is driving the desktop, the user may want nobody
//! at the machine to watch or touch it. This module exclusively grabs every
//! *physical* keyboard and mouse (`EVIOCGRAB`, via the `evdev` crate), which
//! hides those devices from the compositor entirely — local input stops
//! working while the remote path (our uinput devices) keeps flowing. It also
//! provides a screen-lock helper.
//!
//! Safety: a grab with no release would strand the physical machine. Every
//! activation carries a TTL that the phone must keep refreshing; a watchdog
//! thread releases it when the TTL lapses (phone crash / dropped link), and
//! closing the device files releases the grabs on process exit anyway.
//!
//! R4 E4: on a wlroots desktop that advertises `zwlr_layer_shell_v1`, grabbing
//! also paints a full-perimeter "session active" shield (core
//! [`pocketdesk_core::streaming::shield`]) so bystanders can see the machine is
//! being driven remotely. It rides the same lifecycle — shown on the first
//! grab, torn down on release/expiry — and is a no-op where the protocol is
//! absent. (Local input *blocking* is the `EVIOCGRAB` above; this box's
//! Hyprland does not advertise `zwp_input_inhibitor_v1`, so the compositor-
//! native alternative is intentionally not attempted.)

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use evdev::{AttributeSetRef, Device, KeyCode};
use pocketdesk_core::streaming::shield::{self, PrivacyShield};

/// How long a grab lives without a refresh from the phone. The client
/// re-arms well before this (see the Kotlin poll cadence).
pub const GRAB_TTL: Duration = Duration::from_secs(600);

/// Watchdog sweep interval.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(5);

struct GrabState {
    devices: Vec<Device>,
    expires: Option<Instant>,
    /// R4 E4 compositor shield — present only while a grab is active *and* the
    /// desktop advertised `zwlr_layer_shell_v1`. Dropped (unmapped) on release.
    shield: Option<PrivacyShield>,
}

fn grab_state() -> &'static Mutex<GrabState> {
    static GRAB: OnceLock<Mutex<GrabState>> = OnceLock::new();
    GRAB.get_or_init(|| {
        Mutex::new(GrabState {
            devices: Vec::new(),
            expires: None,
            shield: None,
        })
    })
}

/// Whether the session here is locked, polled: logind says it by its
/// Unlock signal and `LockedHint` instead ([`watch_unlocks`]).
pub fn session_locked() -> Option<bool> {
    None
}

/// Call `unlocked` each time logind says this session was unlocked (PR #572
/// review I1): its `Unlock` signal, or its `LockedHint` going false. Ends
/// when the system bus or the session cannot be reached.
pub async fn watch_unlocks(unlocked: impl Fn() + Send + Sync + 'static) {
    // Tried again after each failure, waiting longer each time (PR #572
    // re-review N7), and said once at info until it runs again.
    let mut wait = UNLOCK_RETRY_FIRST;
    let mut said: Option<&'static str> = None;
    loop {
        let started = Instant::now();
        let why = watch_unlocks_once(&unlocked).await;
        if started.elapsed() >= UNLOCK_RETRY_MOST {
            // It ran a good while: the next failure starts the waits over.
            wait = UNLOCK_RETRY_FIRST;
            said = None;
        }
        if said != Some(why) {
            tracing::info!(
                "Privacy: an unlock here is not heard for now ({why}); tried again in {} s",
                wait.as_secs()
            );
            said = Some(why);
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(UNLOCK_RETRY_MOST);
    }
}

/// The first wait before the unlock listener is tried again.
const UNLOCK_RETRY_FIRST: Duration = Duration::from_secs(5);

/// The longest wait before it is tried again, and how long it must have
/// run for the waits to start over.
const UNLOCK_RETRY_MOST: Duration = Duration::from_secs(300);

/// How often the listener checks it still watches this user's session (a
/// server that outlives a logout and a new login).
const UNLOCK_SESSION_LOOK: Duration = Duration::from_secs(60);

/// One run of [`watch_unlocks`]: why it could not run, or why it ended.
async fn watch_unlocks_once(unlocked: &(impl Fn() + Send + Sync)) -> &'static str {
    use futures_lite::StreamExt as _;
    let Ok(conn) = zbus::Connection::system().await else {
        return "no system bus";
    };
    let Some(path) = session_path(&conn).await else {
        return "no logind session";
    };
    let Ok(session) = zbus::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        path.clone(),
        "org.freedesktop.login1.Session",
    )
    .await
    else {
        return "the session could not be read";
    };
    let Ok(mut unlocks) = session.receive_signal("Unlock").await else {
        return "the session's signals could not be heard";
    };
    let mut hints = session.receive_property_changed::<bool>("LockedHint").await;
    let mut look = tokio::time::interval(UNLOCK_SESSION_LOOK);
    look.tick().await;
    tracing::debug!("Privacy: an unlock here is heard from logind");
    loop {
        tokio::select! {
            signal = unlocks.next() => {
                if signal.is_none() {
                    return "the session's signals ended";
                }
                unlocked();
            }
            hint = hints.next() => {
                let Some(hint) = hint else {
                    return "the session's locked hint ended";
                };
                if matches!(hint.get().await, Ok(false)) {
                    unlocked();
                }
            }
            _ = look.tick() => {
                if session_path(&conn).await.as_deref() != Some(path.as_str()) {
                    return "this user's session changed";
                }
            }
        }
    }
}

/// This user's session's object path on logind: the one `session/auto`
/// stands for (the server's own session, else the user's display session),
/// by its id, since signals come from the real path.
async fn session_path(conn: &zbus::Connection) -> Option<String> {
    let auto = zbus::Proxy::new(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
    )
    .await
    .ok()?;
    let id: String = auto.get_property("Id").await.ok()?;
    let manager = zbus::Proxy::new(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    .ok()?;
    let path: zbus::zvariant::OwnedObjectPath = manager.call("GetSession", &(id,)).await.ok()?;
    Some(path.as_str().to_string())
}

/// Classification, factored out for unit tests. A device is grabbed when it
/// looks like a physical keyboard or mouse. Our own injection devices
/// (`Pocketdesk Virtual *`) must never be grabbed — that would silence the
/// remote path itself — and switch-only devices (lid, power button) stay
/// visible to logind.
pub fn should_grab(name: &str, keys: &AttributeSetRef<KeyCode>, has_relative_axes: bool) -> bool {
    if name.starts_with("Pocketdesk") {
        return false;
    }
    let keyboard = keys.contains(KeyCode::KEY_A);
    let mouse = has_relative_axes || keys.contains(KeyCode::BTN_LEFT);
    keyboard || mouse
}

/// Sorted list of `/dev/input/event*` paths.
fn event_device_paths(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("event"))
                })
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    paths
}

fn has_relative_axes(device: &Device) -> bool {
    device
        .supported_relative_axes()
        .is_some_and(|axes| axes.iter().next().is_some())
}

/// Grab every physical keyboard/mouse (or refresh the TTL if already
/// grabbed). Returns the number of devices held.
pub fn grab_input(ttl: Duration) -> Result<usize, String> {
    ensure_watchdog();
    let mut state = grab_state().lock().expect("privacy grab state");
    if !state.devices.is_empty() {
        state.expires = Some(Instant::now() + ttl);
        return Ok(state.devices.len());
    }

    let mut grabbed = Vec::new();
    for path in event_device_paths(Path::new("/dev/input")) {
        // Permission-denied or vanished devices are skipped, not fatal.
        let Ok(mut device) = Device::open(&path) else {
            continue;
        };
        let name = device.name().unwrap_or("").to_string();
        let Some(keys) = device.supported_keys() else {
            continue;
        };
        let grab = should_grab(&name, keys, has_relative_axes(&device));
        if !grab {
            continue;
        }
        match device.grab() {
            Ok(()) => grabbed.push(device),
            // Already exclusively held by something else — leave it alone.
            Err(e) => tracing::warn!("privacy: could not grab {path:?} ({name}): {e}"),
        }
    }

    if grabbed.is_empty() {
        return Err("no grabbable input devices (check /dev/input permissions)".to_string());
    }
    let count = grabbed.len();
    tracing::info!("privacy: grabbed {count} physical input device(s) for {ttl:?}");
    state.devices = grabbed;
    state.expires = Some(Instant::now() + ttl);
    // R4 E4: paint the compositor shield so bystanders see the session is
    // live. Best-effort — on a headless/non-layer-shell desktop this is None
    // and the input grab stands on its own.
    state.shield = shield::show();
    if state.shield.is_some() {
        tracing::info!("privacy: layer-shell privacy shield engaged");
    }
    Ok(count)
}

/// Release all grabs (also happens on TTL lapse and process exit).
pub fn release_input() -> usize {
    let mut state = grab_state().lock().expect("privacy grab state");
    let count = state.devices.len();
    for device in &mut state.devices {
        let _ = device.ungrab();
    }
    state.devices.clear();
    state.expires = None;
    // Dropping the shield tears down its layer surface (unmaps the frame).
    state.shield = None;
    if count > 0 {
        tracing::info!("privacy: released {count} input device(s)");
    }
    count
}

/// `(held device count, seconds left before auto-release)`.
pub fn privacy_status() -> (usize, Option<u64>) {
    let state = grab_state().lock().expect("privacy grab state");
    let left = state
        .expires
        .map(|expiry| expiry.saturating_duration_since(Instant::now()).as_secs());
    (state.devices.len(), left)
}

fn ensure_watchdog() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(WATCHDOG_INTERVAL);
            let expired = {
                let state = grab_state().lock().expect("privacy grab state");
                state.expires.is_some_and(|expiry| Instant::now() >= expiry)
            };
            if expired {
                tracing::info!("privacy: grab TTL lapsed, releasing local input");
                release_input();
            }
        }
    });
}

/// Ask the session's screen locker to engage. Tries the standard systemd
/// path first, then the D-Bus screensaver, then Hyprland's own locker.
pub fn lock_screen() -> Result<&'static str, String> {
    const ATTEMPTS: [(&str, &[&str]); 3] = [
        ("loginctl", &["lock-session"]),
        ("xdg-screensaver", &["lock"]),
        ("hyprlock", &[]),
    ];
    let mut failures = Vec::new();
    for (program, args) in ATTEMPTS {
        match std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => return Ok(program),
            Ok(output) => failures.push(format!("{program}: exit {}", output.status)),
            Err(e) => failures.push(format!("{program}: {e}")),
        }
    }
    Err(failures.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::AttributeSet;

    #[test]
    fn keyboard_and_mouse_grab_physical_pointing_devices() {
        let mut keyboard = AttributeSet::<KeyCode>::new();
        keyboard.insert(KeyCode::KEY_A);
        keyboard.insert(KeyCode::KEY_Z);
        assert!(should_grab(
            "AT Translated Set 2 keyboard",
            &keyboard,
            false
        ));
        let mut mouse = AttributeSet::<KeyCode>::new();
        mouse.insert(KeyCode::BTN_LEFT);
        mouse.insert(KeyCode::BTN_RIGHT);
        assert!(should_grab("Logitech USB Mouse", &mouse, true));
        // A pure wheel has buttons but no relative axes still qualifies.
        assert!(should_grab("Some Mouse", &mouse, false));
    }

    #[test]
    fn virtual_devices_are_never_grabbed() {
        let mut keyboard = AttributeSet::<KeyCode>::new();
        keyboard.insert(KeyCode::KEY_A);
        assert!(!should_grab("Pocketdesk Virtual Input", &keyboard, true));
        let mut touch = AttributeSet::<KeyCode>::new();
        touch.insert(KeyCode::BTN_LEFT);
        assert!(!should_grab("Pocketdesk Virtual Touch", &touch, false));
    }

    #[test]
    fn switch_only_devices_stay_with_logind() {
        // Lid/power devices carry their own keycodes but no keyboard/mouse.
        let none = AttributeSet::<KeyCode>::new();
        assert!(!should_grab("Lid Switch", &none, false));
        let mut power = AttributeSet::<KeyCode>::new();
        power.insert(KeyCode::KEY_POWER);
        assert!(!should_grab("Power Button", &power, false));
    }

    #[test]
    fn enumerating_event_devices_does_not_panic_without_dev_input() {
        let paths = event_device_paths(Path::new("/definitely/not/a/device/dir"));
        assert!(paths.is_empty());
    }

    /// Live check on the dev box (never grabbing!): that the real
    /// `/dev/input/event*` set is enumerable and classifiable.
    #[test]
    fn live_classification_survey() {
        let paths = event_device_paths(Path::new("/dev/input"));
        if paths.is_empty() {
            eprintln!("skip: no /dev/input/event* visible");
            return;
        }
        for path in paths {
            let Ok(device) = Device::open(&path) else {
                continue;
            };
            let name = device.name().unwrap_or("").to_string();
            let Some(keys) = device.supported_keys() else {
                continue;
            };
            let classify = should_grab(&name, keys, has_relative_axes(&device));
            tracing::debug!("survey: {path:?} {name:?} grabbable={classify}");
        }
    }
}
