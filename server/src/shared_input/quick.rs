//! Keeping the link quick while the keyboard and mouse are shared (BK10,
//! `shared_input_low_latency`).
//!
//! Windows wakes a waiting thread only at its timer's tick, every 15.6 ms
//! unless a program asks for a finer one, and a key handed to the link
//! waited for the next tick before it left: a steady 16 ms one way on a
//! link whose own delay was a third of a millisecond. While held, this
//! process asks Windows for a 1 ms timer (`timeBeginPeriod`), and tells
//! Windows 11 to keep honouring that while none of its windows is in view
//! (its power throttling otherwise ignores a background process's timer).
//! It is given back as the last hold lets go.
//!
//! It is held only while it matters, never just for sharing being on: a
//! finer timer makes the whole computer wake more often, which costs a
//! laptop some battery. So it is held while this computer's keyboard and
//! mouse are on another computer or a phone, while another computer's
//! cursor is on this one, and while `share-test` runs. Elsewhere holding
//! does nothing: Linux's timers are fine-grained already.

use std::sync::{Mutex, PoisonError};

/// The holds made in this process, and what is done as the first is made
/// and the last let go.
pub struct Holds {
    count: Mutex<usize>,
    apply: fn(bool),
}

impl Holds {
    /// One hold more (`up`) or one fewer: the system is asked as the first
    /// comes and the last goes, under the lock, so its asks never cross.
    fn change(&self, up: bool) {
        let mut count = self.count.lock().unwrap_or_else(PoisonError::into_inner);
        if up {
            *count += 1;
            if *count == 1 {
                (self.apply)(true);
            }
        } else if *count > 0 {
            *count -= 1;
            if *count == 0 {
                (self.apply)(false);
            }
        }
    }
}

/// The system's: Windows' timer.
static SYSTEM: Holds = Holds {
    count: Mutex::new(0),
    apply: system,
};

/// One holder of the quick link: held or not, let go when dropped.
pub struct KeepQuick {
    held: bool,
    holds: &'static Holds,
}

impl Default for KeepQuick {
    fn default() -> Self {
        KeepQuick {
            held: false,
            holds: &SYSTEM,
        }
    }
}

impl KeepQuick {
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Hold it (`on`) or let it go; as it is already, nothing changes.
    pub fn set(&mut self, on: bool) {
        if on != self.held {
            self.held = on;
            self.holds.change(on);
        }
    }
}

impl Drop for KeepQuick {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// The timer resolution asked for while held, in milliseconds.
#[cfg(windows)]
const TIMER_MS: u32 = 1;

#[cfg(windows)]
fn system(on: bool) {
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };
    // In the control mask and not the state mask: this process's timer
    // request is never ignored. Neither: Windows decides again.
    let throttling = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: if on {
            PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION
        } else {
            0
        },
        StateMask: 0,
    };
    let set_throttling = || {
        // SAFETY: the structure lives across the call, its size is given,
        // and the handle is this process's pseudo-handle.
        let set = unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                std::ptr::from_ref(&throttling).cast(),
                u32::try_from(std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>())
                    .unwrap_or(u32::MAX),
            )
        };
        if let Err(e) = set {
            // Windows before 10 2004 has no such policy; the timer still
            // holds there while this process is in view.
            tracing::debug!("Shared input: Windows kept its timer policy for this process: {e}");
        }
    };
    if on {
        set_throttling();
        // SAFETY: a plain call; matched by the `timeEndPeriod` below.
        let asked = unsafe { timeBeginPeriod(TIMER_MS) };
        if asked == 0 {
            tracing::debug!("Shared input: the timer at 1 ms while sharing");
        } else {
            tracing::debug!("Shared input: Windows would not take a 1 ms timer ({asked})");
        }
    } else {
        // SAFETY: as above; one end for the one begin.
        unsafe { timeEndPeriod(TIMER_MS) };
        set_throttling();
        tracing::debug!("Shared input: the timer given back");
    }
}

#[cfg(not(windows))]
fn system(_on: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How often the stand-in system was asked to hold and to let go.
    static HELD: AtomicUsize = AtomicUsize::new(0);
    static LET_GO: AtomicUsize = AtomicUsize::new(0);

    fn stand_in(on: bool) {
        if on {
            HELD.fetch_add(1, Ordering::SeqCst);
        } else {
            LET_GO.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Never the real timer: a test must not change the owner's computer.
    static TEST: Holds = Holds {
        count: Mutex::new(0),
        apply: stand_in,
    };

    fn holder() -> KeepQuick {
        KeepQuick {
            held: false,
            holds: &TEST,
        }
    }

    /// Asked of the system once while any holder holds, however often each
    /// holds; given back as the last lets go, or is dropped.
    #[test]
    fn the_system_is_asked_once_for_all_holders_and_given_back_after_the_last() {
        let (mut driving, mut driven) = (holder(), holder());
        assert!(!driving.is_held());
        driving.set(true);
        driving.set(true);
        assert!(driving.is_held());
        assert_eq!(HELD.load(Ordering::SeqCst), 1);
        driven.set(true);
        assert_eq!(HELD.load(Ordering::SeqCst), 1, "held already");
        driving.set(false);
        driving.set(false);
        assert!(!driving.is_held());
        assert_eq!(LET_GO.load(Ordering::SeqCst), 0, "still held by the other");
        drop(driven);
        assert_eq!(LET_GO.load(Ordering::SeqCst), 1, "the last, dropped");
        // Held again, and dropped while held.
        driving.set(true);
        assert_eq!(HELD.load(Ordering::SeqCst), 2);
        drop(driving);
        assert_eq!(LET_GO.load(Ordering::SeqCst), 2);
        assert_eq!(*TEST.count.lock().unwrap(), 0);
    }
}
