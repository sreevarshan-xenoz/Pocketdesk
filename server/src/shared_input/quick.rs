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
//!
//! The same while, on any system, a [`Nudge`] keeps this computer's Wi-Fi
//! awake. A Wi-Fi card saving power dozes after about 100 ms without
//! sending, and what comes for it meanwhile waits at the access point for
//! its next wake, up to a beacon's 102 ms: the first key after a pause
//! was held up by as much. A byte sent to the other computer's discard
//! port every 50 ms keeps the card awake, through whichever card carries
//! the link, without changing any setting of the system's (Wi-Fi's own
//! power saving is left on, so nothing needs putting back, even after a
//! crash). It goes outside the link on purpose: on the link it would spend
//! the link's pacing, which on a quiet link held the next key back.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

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

/// How often a nudge goes: well inside the 100 ms a Wi-Fi card waits
/// before it dozes, a Windows timer's coarse tick (62 ms) included.
pub const NUDGE_EVERY: Duration = Duration::from_millis(50);

/// The discard service's port: nothing answers there, and a firewall in
/// front of it drops the byte.
const DISCARD_PORT: u16 = 9;

/// How long the address a computer is nudged at is trusted before it is
/// looked up again (its link may move to another path).
const LOOK_AGAIN: Duration = Duration::from_secs(1);

/// A byte now and then to the computer the keyboard and mouse are shared
/// with, keeping this computer's Wi-Fi awake (see the module's words).
/// Nothing goes while it follows no computer; dropped, it stops.
pub struct Nudge {
    aim: Arc<Mutex<Aim>>,
    /// The computer followed, and when its address was looked up.
    following: Option<(String, Instant)>,
    /// Where a computer's link reaches it ([`super::link::peer_ip`]), and
    /// the port: a test's stand-ins.
    look_up: fn(&str) -> Option<IpAddr>,
    port: u16,
}

/// Where the bytes go now (none ends the sending thread), and whether a
/// thread sends them: both under one lock, so a thread about to end is
/// never counted on to send to a new address.
#[derive(Default)]
struct Aim {
    to: Option<SocketAddr>,
    sending: bool,
}

impl Default for Nudge {
    fn default() -> Self {
        Nudge {
            aim: Arc::default(),
            following: None,
            look_up: super::link::peer_ip,
            port: DISCARD_PORT,
        }
    }
}

impl Nudge {
    /// Nudge toward `computer` (its link's address, looked up again every
    /// [`LOOK_AGAIN`]), or stop with none. Cheap to call as often as a
    /// loop goes round.
    pub fn follow(&mut self, computer: Option<&str>) {
        let Some(computer) = computer else {
            if self.following.take().is_some() {
                self.aim(None);
            }
            return;
        };
        let fresh = self
            .following
            .as_ref()
            .is_some_and(|(id, at)| id == computer && at.elapsed() < LOOK_AGAIN);
        if fresh {
            return;
        }
        self.following = Some((computer.to_string(), Instant::now()));
        let to = (self.look_up)(computer).map(|ip| SocketAddr::new(ip, self.port));
        self.aim(to);
    }

    fn aim(&mut self, to: Option<SocketAddr>) {
        let mut aim = self.aim.lock().unwrap_or_else(PoisonError::into_inner);
        aim.to = to;
        let Some(first) = to else {
            return;
        };
        if aim.sending {
            return;
        }
        let aimed = Arc::clone(&self.aim);
        let spawned = std::thread::Builder::new()
            .name("pd-wifi-nudge".into())
            .spawn(move || send_nudges(&aimed, first));
        match spawned {
            Ok(_) => aim.sending = true,
            Err(e) => tracing::debug!("Shared input: no thread to keep the Wi-Fi awake: {e}"),
        }
    }
}

impl Drop for Nudge {
    fn drop(&mut self) {
        self.aim(None);
    }
}

/// Send a byte to where `aimed` says every [`NUDGE_EVERY`] until it says
/// nowhere. `first` picks the socket's family.
fn send_nudges(aimed: &Mutex<Aim>, first: SocketAddr) {
    let any = if first.is_ipv4() {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    };
    let lock = || aimed.lock().unwrap_or_else(PoisonError::into_inner);
    let socket = match UdpSocket::bind(SocketAddr::new(any, 0)) {
        Ok(socket) => socket,
        Err(e) => {
            tracing::debug!("Shared input: no socket to keep the Wi-Fi awake: {e}");
            lock().sending = false;
            return;
        }
    };
    let mut told = false;
    loop {
        let to = {
            let mut aim = lock();
            let Some(to) = aim.to else {
                aim.sending = false;
                return;
            };
            to
        };
        if let Err(e) = socket.send_to(&[0], to)
            && !std::mem::replace(&mut told, true)
        {
            tracing::debug!("Shared input: a byte to keep the Wi-Fi awake did not go: {e}");
        }
        std::thread::sleep(NUDGE_EVERY);
    }
}

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

    /// A nudge follows a computer to its link's address, a byte every
    /// 50 ms, and stops when it follows none or is dropped; a computer with
    /// no direct link gets nothing.
    #[test]
    fn a_nudge_sends_while_it_follows_and_stops_after() {
        let catcher = UdpSocket::bind("127.0.0.1:0").unwrap();
        catcher
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut nudge = Nudge {
            aim: Arc::default(),
            following: None,
            look_up: |id| (id == "pc").then_some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: catcher.local_addr().unwrap().port(),
        };
        let caught = |n: usize| {
            let mut got = 0;
            let mut byte = [0u8; 8];
            while got < n && catcher.recv(&mut byte).is_ok() {
                got += 1;
            }
            got
        };
        nudge.follow(Some("pc"));
        let started = Instant::now();
        assert_eq!(caught(3), 3);
        assert!(started.elapsed() >= NUDGE_EVERY, "spaced, not a burst");
        // Following it again changes nothing; nothing goes to no link.
        nudge.follow(Some("pc"));
        nudge.follow(Some("relayed"));
        nudge.follow(None);
        std::thread::sleep(NUDGE_EVERY * 2);
        while catcher.recv(&mut [0u8; 8]).is_ok() {}
        assert_eq!(caught(1), 0, "stopped");
        // Again, then dropped.
        nudge.follow(Some("pc"));
        assert_eq!(caught(1), 1);
        drop(nudge);
        std::thread::sleep(NUDGE_EVERY * 2);
        while catcher.recv(&mut [0u8; 8]).is_ok() {}
        assert_eq!(caught(1), 0, "stopped when dropped");
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
