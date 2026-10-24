//! The keyboard's and mouse's idle time from compositors that offer
//! `ext-idle-notify-v1` (Sway, Hyprland, KDE Plasma 6 and other wlroots
//! desktops; T8), where GNOME's idle monitor and KDE's screen saver do not
//! answer. A notification asks the compositor to say when no input came for
//! [`NOTICE`], and when input comes again; a thread of its own keeps the
//! connection and reads those. Until the first "idle", the owner counts as
//! at the keyboard: a lock comes later, never early.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

/// How long without input before the compositor says idle.
pub const NOTICE: Duration = Duration::from_secs(5);

/// How long after a failed start (no compositor, no protocol) it is tried
/// again.
const RETRY: Duration = Duration::from_secs(60);

/// Since when the compositor has said idle; `None` while there is input.
type Seen = Arc<Mutex<Option<Instant>>>;

struct State {
    seen: Seen,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut seen = state.seen.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            // Idle since the last input, NOTICE ago.
            ext_idle_notification_v1::Event::Idled => {
                *seen = Some(
                    Instant::now()
                        .checked_sub(NOTICE)
                        .unwrap_or_else(Instant::now),
                )
            }
            ext_idle_notification_v1::Event::Resumed => *seen = None,
            _ => {}
        }
    }
}

enum Watch {
    /// Watching: since when idle, and whether its thread still runs.
    Running(Seen, Arc<AtomicBool>),
    /// Not to be had, or ended, as of then.
    Failed(Instant),
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// How long connecting to the compositor may take (T8 review: a stalled
/// one held the away lock's watcher).
const CONNECT: Duration = Duration::from_secs(2);

/// Seconds since the last input, `idle_since` being when the compositor's
/// idle began (`None` while there is input).
pub fn seconds(idle_since: Option<Instant>, now: Instant) -> u64 {
    idle_since.map_or(0, |since| now.saturating_duration_since(since).as_secs())
}

/// Start watching, unless it runs, or failed (or ended) within [`RETRY`].
fn ensure(watch: &mut Option<Watch>) {
    match watch {
        Some(Watch::Running(_, alive)) if alive.load(Ordering::Relaxed) => return,
        // Ended (the compositor went): tried again a while later.
        Some(Watch::Running(..)) => {
            *watch = Some(Watch::Failed(Instant::now()));
            return;
        }
        Some(Watch::Failed(at)) if at.elapsed() < RETRY => return,
        _ => {}
    }
    *watch = Some(match start() {
        Ok((seen, alive)) => Watch::Running(seen, alive),
        Err(why) => {
            tracing::debug!("No idle time from the compositor: {why}");
            Watch::Failed(Instant::now())
        }
    });
}

/// Watch from now (T8 review: the compositor counts from when it is asked,
/// so a watch begun only once a lock could follow said "idle" that much
/// later). Cheap while it runs.
pub fn watch() {
    ensure(&mut WATCH.lock().unwrap_or_else(|e| e.into_inner()));
}

/// How long the keyboard and mouse have been idle, in seconds, where the
/// compositor says (`ext-idle-notify-v1`); `None` where it does not (no
/// Wayland session, no such protocol) or the watch has ended.
pub fn idle_secs() -> Option<u64> {
    let mut watch = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    ensure(&mut watch);
    match &*watch {
        Some(Watch::Running(seen, alive)) if alive.load(Ordering::Relaxed) => {
            let since = *seen.lock().unwrap_or_else(|e| e.into_inner());
            Some(seconds(since, Instant::now()))
        }
        _ => None,
    }
}

/// Connect, ask for the notification and keep reading, all on a thread of
/// its own; the setup is waited for [`CONNECT`] at most. Where the
/// compositor offers version 2, the notification counts only input (T8
/// review: version 1's honours idle inhibitors, so a video playing kept the
/// owner "present" for good).
fn start() -> Result<(Seen, Arc<AtomicBool>), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|d| d.is_empty()) {
        return Err("no Wayland session".to_string());
    }
    let seen: Seen = Arc::new(Mutex::new(None));
    let alive = Arc::new(AtomicBool::new(true));
    let (ready, started) = std::sync::mpsc::channel::<Result<(), String>>();
    let (theirs, running) = (seen.clone(), alive.clone());
    std::thread::Builder::new()
        .name("idle-notify".into())
        .spawn(move || {
            let setup = || -> Result<_, String> {
                let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
                let (globals, mut queue) =
                    registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
                let qh = queue.handle();
                let notifier: ExtIdleNotifierV1 = globals
                    .bind(&qh, 1..=2, ())
                    .map_err(|_| "the compositor has no ext-idle-notify-v1".to_string())?;
                let seat: WlSeat = globals
                    .bind(&qh, 1..=1, ())
                    .map_err(|_| "the compositor has no seat".to_string())?;
                let timeout = NOTICE.as_millis() as u32;
                let notification = if notifier.version() >= 2 {
                    notifier.get_input_idle_notification(timeout, &seat, &qh, ())
                } else {
                    notifier.get_idle_notification(timeout, &seat, &qh, ())
                };
                let mut state = State { seen: theirs };
                queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
                Ok((conn, queue, state, (notification, seat, notifier)))
            };
            match setup() {
                Ok((conn, mut queue, mut state, held)) => {
                    let _ = ready.send(Ok(()));
                    // Kept for as long as the watch reads.
                    let _held = (held, conn);
                    while queue.blocking_dispatch(&mut state).is_ok() {}
                }
                Err(why) => {
                    let _ = ready.send(Err(why));
                }
            }
            running.store(false, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    match started.recv_timeout(CONNECT) {
        Ok(Ok(())) => Ok((seen, alive)),
        Ok(Err(why)) => Err(why),
        Err(_) => Err("the compositor did not answer in time".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_seconds_count_from_when_the_compositor_said_idle() {
        let now = Instant::now();
        assert_eq!(seconds(None, now), 0, "input now");
        assert_eq!(seconds(Some(now - Duration::from_secs(95)), now), 95);
    }
}
