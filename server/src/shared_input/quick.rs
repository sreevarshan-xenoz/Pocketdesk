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
//! cursor is on this one (in both, only while input comes: ten seconds
//! without any lets go until the next, [`super::receive::QUICK_FOR`]), and
//! while `share-test` runs. Elsewhere holding does nothing: Linux's timers
//! are fine-grained already.
//!
//! The same while, on any system, a [`Nudge`] keeps this computer's Wi-Fi
//! awake. A Wi-Fi card saving power dozes after about 100 ms without
//! sending, and what comes for it meanwhile waits at the access point for
//! its next wake, up to a beacon's 102 ms: the first key after a pause
//! was held up by as much. A byte sent every 50 ms to the other computer,
//! at the address and port its link reaches it at, keeps the card awake,
//! through whichever card carries the link: that port is one its firewall
//! lets through, and its QUIC stack drops a byte that is not QUIC without
//! a word back. No setting of the system's is changed (Wi-Fi's own
//! power saving is left on, so nothing needs putting back, even after a
//! crash). It goes outside the link on purpose: on the link it would spend
//! the link's pacing, which on a quiet link held the next key back. It
//! goes only to a computer on this one's own network ([`may_nudge`]),
//! never out to the internet.

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
            // Before Windows 11 there is no timer flag to set (the power
            // throttling policy itself came with 10 1709); there Windows
            // keeps a process's timer request in any case.
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

/// How long the address a computer is nudged at is trusted before it is
/// looked up again (its link may move to another path).
const LOOK_AGAIN: Duration = Duration::from_secs(1);

/// Whether a byte may go to `peer`: only on this computer's own network,
/// never out to the internet. It is on one of `networks` (an interface's
/// address and prefix length), or in a range that never leaves a site:
/// the private ones, link-local, loopback and IPv6 unique-local. A direct
/// link between two networks is hole-punched to the far side's public
/// address, which is its router's, not the computer's: none goes there.
/// The shared space carriers use (100.64.0.0/10) is not a site's own, so
/// it counts only when on one of `networks`.
pub(crate) fn may_nudge(peer: IpAddr, networks: &[(IpAddr, u8)]) -> bool {
    let peer = peer.to_canonical();
    let site_local = match peer {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    };
    site_local
        || networks
            .iter()
            .any(|&(address, prefix)| same_network(peer, address.to_canonical(), prefix))
}

/// Whether `a` and `b` share their first `prefix` bits (a prefix of 0 is
/// no network: everything would share it).
fn same_network(a: IpAddr, b: IpAddr, prefix: u8) -> bool {
    let (a, b, bits) = match (a, b) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (
            u128::from(u32::from(a)) << 96,
            u128::from(u32::from(b)) << 96,
            prefix.min(32),
        ),
        (IpAddr::V6(a), IpAddr::V6(b)) => (u128::from(a), u128::from(b), prefix.min(128)),
        _ => return false,
    };
    if bits == 0 {
        return false;
    }
    let mask = u128::MAX << (128 - u32::from(bits));
    a & mask == b & mask
}

/// This computer's networks: each interface's address and prefix length.
fn local_networks() -> Vec<(IpAddr, u8)> {
    if_addrs::get_if_addrs()
        .map(|interfaces| {
            interfaces
                .iter()
                .map(|interface| match &interface.addr {
                    if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), v4.prefixlen),
                    if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), v6.prefixlen),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A byte now and then to the computer the keyboard and mouse are shared
/// with, keeping this computer's Wi-Fi awake (see the module's words).
/// Nothing goes while it follows no computer; dropped, it stops.
pub struct Nudge {
    aim: Arc<Mutex<Aim>>,
    /// The computer followed, and when its address was looked up.
    following: Option<(String, Instant)>,
    /// Where a computer's link reaches it ([`super::link::peer_addr`]),
    /// and this computer's networks ([`local_networks`]): a test's
    /// stand-ins.
    look_up: fn(&str) -> Option<SocketAddr>,
    networks: fn() -> Vec<(IpAddr, u8)>,
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
            look_up: super::link::peer_addr,
            networks: local_networks,
        }
    }
}

impl Nudge {
    /// Nudge toward `computer` (its link's address and port, looked up again every
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
        // Only on this computer's own network (BK10 review I1).
        // An IPv4 address iroh holds as IPv6 (`::ffff:a.b.c.d`) goes as IPv4.
        let to = (self.look_up)(computer)
            .map(|to| match to.ip().to_canonical() {
                ip @ IpAddr::V4(_) => SocketAddr::new(ip, to.port()),
                IpAddr::V6(_) => to,
            })
            .filter(|to| may_nudge(to.ip(), &(self.networks)()));
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
/// nowhere, on a socket of the family of where it is aimed.
fn send_nudges(aimed: &Mutex<Aim>, first: SocketAddr) {
    let lock = || aimed.lock().unwrap_or_else(PoisonError::into_inner);
    let mut socket = match socket_for(first) {
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
        // The link moved between IPv4 and IPv6 (review M1): a socket of
        // the other family, kept should one not be made.
        if to.is_ipv4() != socket.local_addr().is_ok_and(|a| a.is_ipv4()) {
            match socket_for(to) {
                Ok(other) => socket = other,
                Err(e) if !std::mem::replace(&mut told, true) => {
                    tracing::debug!("Shared input: no socket to keep the Wi-Fi awake: {e}");
                }
                Err(_) => {}
            }
        }
        if let Err(e) = socket.send_to(&[0], to)
            && !std::mem::replace(&mut told, true)
        {
            tracing::debug!("Shared input: a byte to keep the Wi-Fi awake did not go: {e}");
        }
        std::thread::sleep(NUDGE_EVERY);
    }
}

/// A socket of `to`'s family, on any address and port.
fn socket_for(to: SocketAddr) -> std::io::Result<UdpSocket> {
    let any = if to.is_ipv4() {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    };
    UdpSocket::bind(SocketAddr::new(any, 0))
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

    /// A nudge follows a computer to its link's address and port, a byte every
    /// 50 ms, and stops when it follows none or is dropped; a computer with
    /// no direct link gets nothing.
    #[test]
    fn a_nudge_sends_while_it_follows_and_stops_after() {
        let catcher = UdpSocket::bind("127.0.0.1:0").unwrap();
        catcher
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        // The catcher stands in for the other computer's link port.
        static CATCHER: Mutex<Option<SocketAddr>> = Mutex::new(None);
        *CATCHER.lock().unwrap() = Some(catcher.local_addr().unwrap());
        let mut nudge = Nudge {
            aim: Arc::default(),
            following: None,
            look_up: |id| CATCHER.lock().unwrap().filter(|_| id == "pc"),
            networks: Vec::new,
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
        // A public address is not followed (I1), and nothing goes to no
        // link.
        let mut public = Nudge {
            aim: Arc::default(),
            following: None,
            look_up: |_| Some(SocketAddr::from(([203, 0, 113, 9], 4433))),
            networks: Vec::new,
        };
        public.follow(Some("far"));
        assert!(!public.aim.lock().unwrap().sending);
        // Following it again changes nothing.
        nudge.follow(Some("pc"));
        nudge.follow(Some("relayed"));
        nudge.follow(None);
        // Its thread has ended (it may have been between reading the aim
        // and sending, under a loaded test run), then what it sent before
        // ending is drained (review M4).
        let ended = |aim: &Arc<Mutex<Aim>>| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while aim.lock().unwrap().sending {
                assert!(Instant::now() < deadline, "the sending thread never ended");
                std::thread::sleep(Duration::from_millis(5));
            }
            while catcher.recv(&mut [0u8; 8]).is_ok() {}
        };
        ended(&nudge.aim);
        assert_eq!(caught(1), 0, "stopped");
        // Again, then dropped.
        nudge.follow(Some("pc"));
        assert_eq!(caught(1), 1);
        let aim = Arc::clone(&nudge.aim);
        drop(nudge);
        ended(&aim);
        assert_eq!(caught(1), 0, "stopped when dropped");
    }

    /// BK10 review M1: a link that moves from IPv4 to IPv6 is followed
    /// there, the socket made again for the new family.
    #[test]
    fn a_nudge_follows_a_link_from_ipv4_to_ipv6() {
        let Ok(six) = UdpSocket::bind("[::1]:0") else {
            return; // No IPv6 loopback here: nothing to check.
        };
        let four = UdpSocket::bind("127.0.0.1:0").unwrap();
        for socket in [&four, &six] {
            socket
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
        }
        static AT: Mutex<Option<SocketAddr>> = Mutex::new(None);
        *AT.lock().unwrap() = Some(four.local_addr().unwrap());
        let mut nudge = Nudge {
            aim: Arc::default(),
            following: None,
            look_up: |_| *AT.lock().unwrap(),
            networks: Vec::new,
        };
        nudge.follow(Some("pc"));
        assert!(four.recv(&mut [0u8; 8]).is_ok(), "over IPv4");
        *AT.lock().unwrap() = Some(six.local_addr().unwrap());
        // Looked up again a second on.
        std::thread::sleep(LOOK_AGAIN);
        nudge.follow(Some("pc"));
        assert!(six.recv(&mut [0u8; 8]).is_ok(), "then over IPv6");
    }

    /// BK10 review I1: a byte goes only to a computer on this one's own
    /// network or in a range that stays on a site, never to a public
    /// address (a link hole-punched across the internet).
    #[test]
    fn a_byte_goes_only_to_this_network() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let networks = [
            (ip("192.0.2.10"), 24),
            (ip("2001:db8:1::10"), 64),
            (ip("100.100.1.1"), 32),
        ];
        // On one of this computer's networks, public prefix or not.
        assert!(may_nudge(ip("192.0.2.77"), &networks));
        assert!(may_nudge(ip("2001:db8:1::99"), &networks));
        // Off them, with a public address: never.
        assert!(!may_nudge(ip("198.51.100.7"), &networks));
        assert!(!may_nudge(ip("192.0.3.1"), &networks));
        assert!(!may_nudge(ip("2001:db8:2::1"), &networks));
        assert!(!may_nudge(ip("203.0.113.9"), &[]));
        // A site's own ranges, on its networks or not.
        for local in [
            "10.1.2.3",
            "172.16.0.9",
            "192.168.50.2",
            "169.254.3.4",
            "127.0.0.1",
            "fe80::1",
            "fd12:3456::1",
            "::1",
            "::ffff:192.168.1.5",
        ] {
            assert!(may_nudge(ip(local), &[]), "{local}");
        }
        // Carriers' shared space only when on a network here.
        assert!(!may_nudge(ip("100.64.0.5"), &[]));
        assert!(may_nudge(ip("100.100.1.1"), &networks));
        assert!(!may_nudge(ip("100.100.1.2"), &networks), "a /32 is itself");
        // A prefix of 0 is no network.
        assert!(!may_nudge(ip("198.51.100.7"), &[(ip("192.0.2.1"), 0)]));
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
