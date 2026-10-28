//! Keeping this computer awake while someone uses it from another.
//!
//! Injected input counts as input, but the person may be typing on the
//! computer that drives this one with the cursor resting here, and this
//! screen should not go dark under them. While held: on Linux the session's
//! `org.freedesktop.ScreenSaver` is asked to inhibit idling (what a video
//! player asks); on Windows a thread of its own says the display and the
//! system are required (`SetThreadExecutionState` holds per thread, so the
//! thread lives while it is held).

/// Whether this computer is being kept awake, and the means of letting go.
pub struct KeepAwake {
    /// Dropping it lets go.
    held: Option<Box<dyn Send>>,
    /// How a hold is started: the system's, or a test's stand-in.
    start: fn() -> Option<Box<dyn Send>>,
}

impl Default for KeepAwake {
    fn default() -> Self {
        KeepAwake {
            held: None,
            start: || start().map(|holder| Box::new(holder) as Box<dyn Send>),
        }
    }
}

/// Dropping it lets go.
struct Holder {
    #[cfg(windows)]
    _stop: std::sync::mpsc::Sender<()>,
    #[cfg(target_os = "linux")]
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// The name and the reason the inhibitor is asked with.
#[cfg(target_os = "linux")]
const INHIBIT: (&str, &str) = ("Pocketdesk", "Using this computer from another");

impl KeepAwake {
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    /// Keep this computer awake until [`KeepAwake::release`]; held already,
    /// nothing changes. On Linux, called on a runtime.
    pub fn hold(&mut self) {
        if self.held.is_none() {
            self.held = (self.start)();
        }
    }

    /// Let it idle again.
    pub fn release(&mut self) {
        self.held = None;
    }
}

#[cfg(windows)]
fn start() -> Option<Holder> {
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("pd-keep-awake".into())
        .spawn(move || {
            // SAFETY: plain Win32 calls with flags, on this thread's own state.
            let was = unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
            };
            if was.0 == 0 {
                tracing::debug!("Shared input: Windows did not take the keep-awake request");
            }
            // Until the holder is dropped.
            let _ = stopped.recv();
            // SAFETY: as above.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
    match spawned {
        Ok(_) => Some(Holder { _stop: stop }),
        Err(e) => {
            tracing::debug!("Shared input: no thread to keep this computer awake: {e}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn start() -> Option<Holder> {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::debug!("Shared input: no runtime to keep this computer awake on");
        return None;
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    runtime.spawn(async move {
        const SERVICE: &str = "org.freedesktop.ScreenSaver";
        const PATH: &str = "/org/freedesktop/ScreenSaver";
        let conn = match zbus::Connection::session().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::debug!("Shared input: no session bus to keep awake on: {e}");
                return;
            }
        };
        let cookie = conn
            .call_method(Some(SERVICE), PATH, Some(SERVICE), "Inhibit", &INHIBIT)
            .await
            .and_then(|reply| reply.body().deserialize::<u32>());
        let cookie = match cookie {
            Ok(cookie) => cookie,
            Err(e) => {
                tracing::debug!("Shared input: the screensaver would not hold off: {e}");
                return;
            }
        };
        // Until the holder is dropped (or the server ends, which ends this
        // connection and the inhibitor with it).
        let _ = stopped.await;
        if let Err(e) = conn
            .call_method(Some(SERVICE), PATH, Some(SERVICE), "UnInhibit", &(cookie,))
            .await
        {
            tracing::debug!("Shared input: the screensaver's hold was not let go: {e}");
        }
    });
    Some(Holder { _stop: stop })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn start() -> Option<Holder> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How often the stand-in was started, and how many of its holds are
    /// still up.
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static UP: AtomicUsize = AtomicUsize::new(0);

    struct StandIn;

    impl Drop for StandIn {
        fn drop(&mut self) {
            UP.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Held once however often it is asked, and let go; the system is never
    /// asked (a test must not keep the owner's computer awake).
    #[test]
    fn holds_once_and_lets_go() {
        let mut awake = KeepAwake {
            held: None,
            start: || {
                STARTED.fetch_add(1, Ordering::SeqCst);
                UP.fetch_add(1, Ordering::SeqCst);
                Some(Box::new(StandIn))
            },
        };
        assert!(!awake.is_held());
        awake.hold();
        awake.hold();
        assert!(awake.is_held());
        assert_eq!(STARTED.load(Ordering::SeqCst), 1);
        assert_eq!(UP.load(Ordering::SeqCst), 1);
        awake.release();
        assert!(!awake.is_held());
        assert_eq!(UP.load(Ordering::SeqCst), 0);
        // Dropped while held lets go too.
        awake.hold();
        drop(awake);
        assert_eq!(UP.load(Ordering::SeqCst), 0);
    }
}
