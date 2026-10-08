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
//! [`linux_link_core::streaming::shield`]) so bystanders can see the machine is
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
use linux_link_core::streaming::shield::{self, PrivacyShield};

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

/// Classification, factored out for unit tests. A device is grabbed when it
/// looks like a physical keyboard or mouse. Our own injection devices
/// (`Linux Link Virtual *`) must never be grabbed — that would silence the
/// remote path itself — and switch-only devices (lid, power button) stay
/// visible to logind.
pub fn should_grab(name: &str, keys: &AttributeSetRef<KeyCode>, has_relative_axes: bool) -> bool {
    if name.starts_with("Linux Link") {
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
        assert!(!should_grab("Linux Link Virtual Input", &keyboard, true));
        let mut touch = AttributeSet::<KeyCode>::new();
        touch.insert(KeyCode::BTN_LEFT);
        assert!(!should_grab("Linux Link Virtual Touch", &touch, false));
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
