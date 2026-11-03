//! A window of this computer as what a session captures (window handoff,
//! phase 5: Windows as the source).
//!
//! A session names what it shows by a monitor index. A window lent to another
//! computer (the server's `lend_window`) has no monitor, so the server
//! registers the window here and gives the borrowing computer an index past
//! every monitor's ([`FIRST_INDEX`] on): the borrower dials that index as it
//! dials any screen's, and the session that starts for it holds a [`Lease`] on
//! the registration, captures that window alone, and lands the pointer in its
//! frame and nowhere else (`window_guard`). Nothing on the borrowing side
//! knows the difference.
//!
//! **A registration is a grant to one device for one window, and a session is
//! bound to exactly that grant.** The registration names the device it was
//! lent to, the window's handle and the program and thread that own the
//! window ([`Identity`]), and a generation number no other registration ever
//! has, even one made under the same index later. A session is refused unless
//! its authenticated device is the one lent to ([`lease_for`]), and holds a
//! [`Lease`] copy of the grant: every input packet and the capture's own loop
//! ask that the registration is still exactly the lease's and that the handle
//! still names the window that was lent ([`lease_alive`]), and a session whose
//! lease is not held any more ends and sends nothing. A window given back, a
//! window closed, a handle that Windows gave to another window, an index
//! given to another lend or another device: each ends the sessions bound to
//! the old grant, and none can drive the new one.
//!
//! A session bound to a window never falls back to a monitor: the lend is made
//! for one window, and a monitor would show whatever its owner has open. An
//! index in this range that no window is registered under (the window was
//! given back) is refused a session.
//!
//! The table and the decisions on it are the same on every system, so they
//! are tested everywhere; the calls that reach Windows are thin and answer
//! "no" off Windows ([`Machine`]).

use std::sync::{Mutex, OnceLock};

use super::window_guard::Os;

/// The first monitor index that names a lent window rather than a monitor. No
/// computer has a thousand monitors.
pub const FIRST_INDEX: u32 = 1000;

/// How many windows may be registered at once.
pub const MOST: u32 = 8;

/// A rectangle as `(x, y, width, height)` in the desktop's pixels.
pub type Rect = (i32, i32, u32, u32);

/// Who owns a window: its program and the thread that made it. A window
/// handle is reused by Windows once its window is gone, so a handle alone does
/// not say it is still the window that was lent; the pair does (a new window
/// of the same thread under the same handle is the one risk left, and it is
/// the same program's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Identity {
    pub pid: u32,
    pub tid: u32,
}

/// What a session holds of a registration: the grant it was started under.
/// A lease is not a permission by itself, it is what the registration is
/// compared with ([`lease_alive`]); a lease whose registration changed holds
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub index: u32,
    pub hwnd: isize,
    /// No two registrations ever share one.
    pub generation: u64,
    pub identity: Identity,
    /// The device the window was lent to.
    pub device: String,
    /// The compositor's own name for the window when the host is not Windows
    /// (`gnome:<id>`, see [`gnome_native`]); empty for a Windows handle. A
    /// native id is never reused by its compositor, so no program-and-thread
    /// check is needed for it; the registration holding is the whole of it.
    pub native: String,
}

/// Why a window could not be registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterError {
    /// Every index is taken.
    Full,
    /// No handle, or no device to lend to.
    Nothing,
    /// The window is lent to another device.
    Held,
}

/// Why a session may not have the window under an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No window is registered under it (given back, closed or never lent).
    NotLent,
    /// It is lent to another device, or the session's device is not known:
    /// a window is only for the device it was lent to.
    OtherDevice,
}

#[derive(Debug, Clone)]
struct Entry {
    index: u32,
    hwnd: isize,
    generation: u64,
    identity: Identity,
    device: String,
    native: String,
}

impl Entry {
    fn lease(&self) -> Lease {
        Lease {
            index: self.index,
            hwnd: self.hwnd,
            generation: self.generation,
            identity: self.identity,
            device: self.device.clone(),
            native: self.native.clone(),
        }
    }
}

/// The windows registered, each under the index its session captures it by.
#[derive(Default)]
struct Table {
    entries: Vec<Entry>,
    /// The generation the last registration got.
    generation: u64,
    /// Where the next free index is looked for from (an offset into the
    /// range), so an index just freed is the last to be handed out again.
    cursor: u32,
}

impl Table {
    /// Register `hwnd` for `device`: the lease, and whether this call made the
    /// registration (a window already registered to the same device with the
    /// same owner keeps its lease, and a caller undoing its own work must not
    /// undo that). `prefer` is an index to take if it is free (a lend that
    /// comes back keeps the screen's name).
    fn register(
        &mut self,
        hwnd: isize,
        device: &str,
        identity: Identity,
        prefer: Option<u32>,
    ) -> Result<(Lease, bool), RegisterError> {
        self.register_with(hwnd, "", device, identity, prefer)
    }

    /// [`Table::register`] for a window of any host: a Windows handle with
    /// its owner (`native` empty), or a compositor's own id (`native` set,
    /// the handle a name made from it, the owner none).
    fn register_with(
        &mut self,
        hwnd: isize,
        native: &str,
        device: &str,
        identity: Identity,
        prefer: Option<u32>,
    ) -> Result<(Lease, bool), RegisterError> {
        if hwnd == 0 || device.is_empty() {
            return Err(RegisterError::Nothing);
        }
        if let Some(at) = self.entries.iter().position(|e| e.hwnd == hwnd) {
            let held = &self.entries[at];
            if held.identity == identity && held.native == native {
                return if held.device == device {
                    Ok((held.lease(), false))
                } else {
                    Err(RegisterError::Held)
                };
            }
            // The handle was given to another window since: the old grant is
            // void (its sessions end), and this is a new one.
            self.entries.remove(at);
        }
        let free = |i: u32| !self.entries.iter().any(|e| e.index == i);
        let index = prefer
            .filter(|i| is_window_index(*i) && free(*i))
            .or_else(|| {
                (0..MOST)
                    .map(|k| FIRST_INDEX + (self.cursor + k) % MOST)
                    .find(|i| free(*i))
            })
            .ok_or(RegisterError::Full)?;
        self.cursor = (index - FIRST_INDEX + 1) % MOST;
        self.generation += 1;
        let entry = Entry {
            index,
            hwnd,
            generation: self.generation,
            identity,
            device: device.to_string(),
            native: native.to_string(),
        };
        let lease = entry.lease();
        self.entries.push(entry);
        Ok((lease, true))
    }

    /// Remove the registration the lease is (and no other: an index that was
    /// given to another lend since is not the lease's to undo).
    fn unregister(&mut self, lease: &Lease) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.index == lease.index && e.generation == lease.generation));
        self.entries.len() != before
    }

    /// The lease a session of `device` may hold on the window under `index`.
    fn lease_for(&self, index: u32, device: Option<&str>) -> Result<Lease, Refusal> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.index == index)
            .ok_or(Refusal::NotLent)?;
        match device {
            Some(device) if !device.is_empty() && device == entry.device => Ok(entry.lease()),
            _ => Err(Refusal::OtherDevice),
        }
    }

    /// Whether the registration under the lease's index is exactly the lease's.
    fn holds(&self, lease: &Lease) -> bool {
        self.entries.iter().any(|e| {
            e.index == lease.index
                && e.generation == lease.generation
                && e.hwnd == lease.hwnd
                && e.identity == lease.identity
                && e.device == lease.device
                && e.native == lease.native
        })
    }

    #[cfg(test)]
    fn lease_of(&self, index: u32) -> Option<Lease> {
        self.entries
            .iter()
            .find(|e| e.index == index)
            .map(Entry::lease)
    }
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    entries: Vec::new(),
    generation: 0,
    cursor: 0,
});

fn table() -> std::sync::MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped whenever a registration is made or removed, so a session that holds
/// a lease looks at once instead of at its next packet.
fn changes_tx() -> &'static tokio::sync::watch::Sender<u64> {
    static TX: OnceLock<tokio::sync::watch::Sender<u64>> = OnceLock::new();
    TX.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// A receiver that changes whenever a registration is made or removed: a
/// session holding a lease checks it still holds when this says so.
pub fn changes() -> tokio::sync::watch::Receiver<u64> {
    changes_tx().subscribe()
}

fn notify() {
    changes_tx().send_modify(|n| *n = n.wrapping_add(1));
}

/// Whether `index` is one a registered window may have (it names no monitor).
pub fn is_window_index(index: u32) -> bool {
    (FIRST_INDEX..FIRST_INDEX + MOST).contains(&index)
}

/// Register the window `hwnd`, owned by `identity`, as lent to `device`: its
/// lease, and whether this call made the registration. The index is the one
/// `prefer` names if free, else the next after the last handed out.
pub fn register(
    hwnd: isize,
    device: &str,
    identity: Identity,
    prefer: Option<u32>,
) -> Result<(Lease, bool), RegisterError> {
    let made = table().register(hwnd, device, identity, prefer);
    if matches!(made, Ok((_, true))) {
        notify();
    }
    made
}

/// Register the window a compositor names `native` (see [`gnome_native`]) as
/// lent to `device`, as [`register`] does a Windows handle: the lease, and
/// whether this call made the registration.
pub fn register_native(
    native: &str,
    device: &str,
    prefer: Option<u32>,
) -> Result<(Lease, bool), RegisterError> {
    if native.is_empty() {
        return Err(RegisterError::Nothing);
    }
    let made = table().register_with(
        native_handle(native),
        native,
        device,
        Identity::default(),
        prefer,
    );
    if matches!(made, Ok((_, true))) {
        notify();
    }
    made
}

/// The handle a compositor's window name is registered under: a stable,
/// non-zero number made from the name (FNV-1a), so the table's rules for a
/// handle apply to it. Two names that gave one handle would be taken for a
/// reused handle, which voids the older grant: never the other way round.
pub fn native_handle(native: &str) -> isize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in native.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Never zero, and positive on every pointer width the table is read on.
    ((hash & 0x7fff_ffff) | 1) as isize
}

/// What a window of GNOME is called to the table: `gnome:` and the id mutter
/// gives it in decimal.
pub fn gnome_native(id: u64) -> String {
    format!("gnome:{id}")
}

/// The mutter window id a native name holds: `gnome:` and plain decimal
/// digits (no sign, no spaces, no leading zero, one that fits), nothing else.
pub fn gnome_id(native: &str) -> Option<u64> {
    let digits = native.strip_prefix("gnome:")?;
    if digits.is_empty()
        || digits.len() > 20
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse::<u64>().ok().filter(|id| *id != 0)
}

/// Remove the registration `lease` is, if it is still that one: the sessions
/// bound to it end. Nothing happens to a registration made since.
pub fn unregister(lease: &Lease) {
    if table().unregister(lease) {
        notify();
    }
}

/// The lease a session of `device` (the device its connection proved, `None`
/// when none was) holds on the window registered under `index`; refused when
/// the index names no lent window or one lent to another device.
pub fn lease_for(index: u32, device: Option<&str>) -> Result<Lease, Refusal> {
    table().lease_for(index, device)
}

/// Whether the registration under the lease's index is still exactly the
/// lease's (the table's half of [`lease_alive`]).
pub fn lease_holds(lease: &Lease) -> bool {
    table().holds(lease)
}

/// Whether the lease still stands: the registration is exactly the lease's
/// and, for a Windows handle, the handle still names the window that was lent
/// (not a window Windows gave the handle to later). A Windows lease stands
/// nowhere else. A compositor's window (a native lease) has no handle to be
/// reused: its id is never given to another window, the server ends the grant
/// when its window is closed, and the stream of it ends with the window.
pub fn lease_alive(lease: &Lease) -> bool {
    lease_holds(lease)
        && if lease.native.is_empty() {
            same_window(lease.hwnd, lease.identity)
        } else {
            true
        }
}

/// Whether a session on this lease may send input at all. A Windows window
/// may (its guard places and checks every packet). A compositor's window may
/// not: no compositor that a program can ask says whether another window
/// covers the lent one, or which window has the keyboard, so nothing a
/// borrower sent could be shown to land only in that window; the session is
/// view-only, as the guard's every answer of "no" would make it anyway.
pub fn takes_input(lease: &Lease) -> bool {
    lease.native.is_empty()
}

/// The lease on whatever is registered under `index`, for tests.
#[cfg(test)]
pub(crate) fn lease_at(index: u32) -> Option<Lease> {
    table().lease_of(index)
}

// ---- Windows ----

/// The frame of the window `hwnd` as the capture shows it (DWM's frame
/// without its shadow): `None` off Windows, for a window that is gone or one
/// with no size.
#[cfg(windows)]
pub fn frame_of(hwnd: isize) -> Option<Rect> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    let mut rect = RECT::default();
    // SAFETY: a window handle (a stale one is refused, not followed) and an
    // out value of the size the attribute is told.
    unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    (width > 0 && height > 0).then_some((rect.left, rect.top, width as u32, height as u32))
}

/// The frame of the window `hwnd` as the capture shows it.
#[cfg(not(windows))]
pub fn frame_of(_hwnd: isize) -> Option<Rect> {
    None
}

/// The program and thread that own the window `hwnd`; `None` when there is no
/// such window (and always off Windows).
#[cfg(windows)]
pub fn identity_of(hwnd: isize) -> Option<Identity> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: a handle is only asked about; one that is no window says no.
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: a window handle and an out value on this stack.
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (tid != 0 && pid != 0).then_some(Identity { pid, tid })
}

/// The program and thread that own the window `hwnd`.
#[cfg(not(windows))]
pub fn identity_of(_hwnd: isize) -> Option<Identity> {
    None
}

/// Whether the handle still names the window that `identity` owned when it
/// was lent.
pub fn same_window(hwnd: isize, identity: Identity) -> bool {
    identity_of(hwnd) == Some(identity)
}

/// Whether the window is minimised.
#[cfg(windows)]
pub fn is_minimized(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    // SAFETY: a handle is only asked about; one that is no window says no.
    unsafe { IsIconic(HWND(hwnd as *mut _)) }.as_bool()
}

/// Whether the window is minimised.
#[cfg(not(windows))]
pub fn is_minimized(_hwnd: isize) -> bool {
    false
}

/// Bring a minimised window out of the taskbar (asynchronously, so a window
/// whose program has hung cannot hold the caller), if the lease still stands.
/// A window sends the capture no frames while it is minimised.
pub fn unminimize(lease: &Lease) {
    if lease_alive(lease) {
        unminimize_hwnd(lease.hwnd);
    }
}

#[cfg(windows)]
fn unminimize_hwnd(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, ShowWindowAsync};
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: calls on a window handle; one that is no window is refused by
    // each, and nothing is read through it.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
    }
}

#[cfg(not(windows))]
fn unminimize_hwnd(_hwnd: isize) {}

/// What a session asks of this computer's windows (see [`Os`]): the real one.
/// Off Windows every question is answered "no", so nothing is ever placed or
/// sent.
pub struct Machine;

#[cfg(windows)]
mod live {
    use super::*;
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GW_OWNER, GetAncestor, GetForegroundWindow, GetWindow, GetWindowThreadProcessId,
        HWND_TOP, IsHungAppWindow, IsIconic, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
        SetForegroundWindow, SetWindowPos, ShowWindow, WindowFromPoint,
    };

    fn handle(hwnd: isize) -> HWND {
        HWND(hwnd as *mut _)
    }

    fn owner_of(hwnd: isize) -> isize {
        // SAFETY: a handle is only asked about; one that is no window has no
        // owner.
        unsafe { GetWindow(handle(hwnd), GW_OWNER) }.map_or(0, |o| o.0 as isize)
    }

    fn pid_of(hwnd: isize) -> u32 {
        let mut pid = 0u32;
        // SAFETY: a handle is only asked about, with an out value on this
        // stack.
        unsafe { GetWindowThreadProcessId(handle(hwnd), Some(&mut pid)) };
        pid
    }

    /// Whether the window `candidate` is the lent window or belongs to it
    /// (see `window_guard::belongs_to`).
    fn belongs(candidate: isize, lease: &Lease) -> bool {
        super::super::window_guard::belongs_to(
            candidate,
            pid_of(candidate),
            lease.hwnd,
            lease.identity.pid,
            owner_of,
        )
    }

    impl Os for Machine {
        fn lease_holds(&self, lease: &Lease) -> bool {
            lease_alive(lease)
        }

        fn frame(&self, lease: &Lease) -> Option<Rect> {
            frame_of(lease.hwnd)
        }

        fn front_is_lent(&self, lease: &Lease) -> bool {
            // SAFETY: no arguments; the answer is a handle, which is only
            // compared and asked about.
            let front = unsafe { GetForegroundWindow() }.0 as isize;
            front != 0 && belongs(front, lease)
        }

        fn hit_is_lent(&self, lease: &Lease, at: (i32, i32)) -> bool {
            // SAFETY: plain numbers; the answer is a handle (or none), which
            // is only compared and asked about.
            let hit = unsafe { WindowFromPoint(POINT { x: at.0, y: at.1 }) };
            if hit.0.is_null() {
                return false;
            }
            // SAFETY: as above.
            let root = unsafe { GetAncestor(hit, GA_ROOT) };
            let root = if root.0.is_null() { hit } else { root };
            belongs(root.0 as isize, lease)
        }

        fn raise(&self, lease: &Lease) {
            if !lease_alive(lease) {
                return;
            }
            let hwnd = handle(lease.hwnd);
            // SAFETY: calls on a window handle; one that is no window is
            // refused by each, and nothing is read through it. The calls
            // wait for the window's program, which a hung one never
            // answers: such a window is not asked.
            unsafe {
                if IsHungAppWindow(hwnd).as_bool() {
                    return;
                }
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
}

#[cfg(not(windows))]
impl Os for Machine {
    fn lease_holds(&self, lease: &Lease) -> bool {
        // A compositor's window stands while its grant does; the rest of the
        // guard's questions have no answer there, so every one is "no".
        lease_alive(lease)
    }
    fn frame(&self, _lease: &Lease) -> Option<Rect> {
        None
    }
    fn front_is_lent(&self, _lease: &Lease) -> bool {
        false
    }
    fn hit_is_lent(&self, _lease: &Lease, _at: (i32, i32)) -> bool {
        false
    }
    fn raise(&self, _lease: &Lease) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: Identity = Identity { pid: 10, tid: 11 };
    const DEVICE: &str = "0123456789abcdef";
    const OTHER: &str = "fedcba9876543210";

    // The global table is shared by the tests running side by side: the one
    // that uses it takes a handle and a device of its own and leaves none
    // behind; every rule is tried on a table of its own.

    fn made(table: &mut Table, hwnd: isize) -> Lease {
        table.register(hwnd, DEVICE, ME, None).unwrap().0
    }

    #[test]
    fn a_window_gets_an_index_past_every_monitor_and_keeps_it() {
        let mut table = Table::default();
        let first = made(&mut table, 0x7101);
        assert!(is_window_index(first.index));
        assert!(first.index >= FIRST_INDEX);
        let again = table.register(0x7101, DEVICE, ME, None).unwrap();
        assert_eq!(
            again,
            (first.clone(), false),
            "the same lend, the same lease"
        );
        assert!(table.holds(&first));
        assert!(table.unregister(&first));
        assert!(!table.holds(&first));
    }

    #[test]
    fn two_windows_never_share_an_index() {
        let mut table = Table::default();
        let a = made(&mut table, 0x7201);
        let b = made(&mut table, 0x7202);
        assert_ne!(a.index, b.index);
        assert_ne!(a.generation, b.generation);
        assert!(table.holds(&a) && table.holds(&b));
    }

    #[test]
    fn no_handle_or_no_device_is_no_lend() {
        let mut table = Table::default();
        assert_eq!(
            table.register(0, DEVICE, ME, None).unwrap_err(),
            RegisterError::Nothing
        );
        assert_eq!(
            table.register(0x7301, "", ME, None).unwrap_err(),
            RegisterError::Nothing
        );
        assert!(table.entries.is_empty());
    }

    #[test]
    fn a_monitor_index_is_no_window_index() {
        for monitor in [0, 1, 3, 15, FIRST_INDEX - 1, FIRST_INDEX + MOST, u32::MAX] {
            assert!(!is_window_index(monitor), "{monitor}");
            assert_eq!(
                Table::default()
                    .lease_for(monitor, Some(DEVICE))
                    .unwrap_err(),
                Refusal::NotLent
            );
        }
        assert!(is_window_index(FIRST_INDEX));
        assert!(is_window_index(FIRST_INDEX + MOST - 1));
    }

    #[test]
    fn a_full_table_says_so_and_a_freed_index_comes_back_when_the_range_has_been_walked() {
        let mut table = Table::default();
        let leases: Vec<Lease> = (0..MOST as isize)
            .map(|n| made(&mut table, 0x7400 + n))
            .collect();
        let mut sorted: Vec<u32> = leases.iter().map(|l| l.index).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), MOST as usize, "every window its own index");
        assert_eq!(
            table.register(0x7499, DEVICE, ME, None).unwrap_err(),
            RegisterError::Full
        );
        // A window already registered still finds its own, full or not.
        assert_eq!(
            table.register(0x7400, DEVICE, ME, None).unwrap(),
            (leases[0].clone(), false)
        );
        // Two freed: found from the cursor, each with a generation of its own.
        assert!(table.unregister(&leases[3]));
        assert!(table.unregister(&leases[5]));
        let next = made(&mut table, 0x7498);
        let after = made(&mut table, 0x7497);
        let mut got = [next.index, after.index];
        got.sort_unstable();
        assert_eq!(got, [leases[3].index, leases[5].index]);
        assert!(next.generation > leases[7].generation);
    }

    #[test]
    fn a_freed_index_is_not_handed_out_at_once() {
        let mut table = Table::default();
        let a = made(&mut table, 0x7451);
        assert!(table.unregister(&a));
        let b = made(&mut table, 0x7452);
        assert_ne!(
            b.index, a.index,
            "the next window is not the index just freed"
        );
    }

    #[test]
    fn an_index_made_again_is_a_new_grant_the_old_lease_does_not_hold() {
        let mut table = Table::default();
        let old = made(&mut table, 0x7501);
        assert!(table.unregister(&old));
        // The same window, the same device, asked for the same index back (a
        // lend that comes again): a registration with a generation of its own.
        let new = table
            .register(0x7501, DEVICE, ME, Some(old.index))
            .unwrap()
            .0;
        assert_eq!(new.index, old.index, "the screen keeps its name");
        assert_ne!(new.generation, old.generation);
        assert!(table.holds(&new));
        assert!(
            !table.holds(&old),
            "a session of the old grant holds nothing"
        );
        // And the old lease cannot take the new grant away.
        assert!(!table.unregister(&old));
        assert!(table.holds(&new));
    }

    #[test]
    fn a_lease_holds_only_while_every_part_of_it_is_the_registrations() {
        let mut table = Table::default();
        let lease = made(&mut table, 0x7601);
        assert!(table.holds(&lease));
        for changed in [
            Lease {
                hwnd: 0x7602,
                ..lease.clone()
            },
            Lease {
                generation: lease.generation + 1,
                ..lease.clone()
            },
            Lease {
                identity: Identity { pid: 99, tid: 11 },
                ..lease.clone()
            },
            Lease {
                device: OTHER.into(),
                ..lease.clone()
            },
            Lease {
                index: lease.index + 1,
                ..lease.clone()
            },
        ] {
            assert!(!table.holds(&changed), "{changed:?}");
        }
    }

    #[test]
    fn only_the_device_a_window_was_lent_to_may_dial_its_index() {
        let mut table = Table::default();
        let lease = made(&mut table, 0x7701);
        assert_eq!(table.lease_for(lease.index, Some(DEVICE)).unwrap(), lease);
        assert_eq!(
            table.lease_for(lease.index, Some(OTHER)).unwrap_err(),
            Refusal::OtherDevice
        );
        // A session that proved no device, or an empty name, is no device.
        assert_eq!(
            table.lease_for(lease.index, None).unwrap_err(),
            Refusal::OtherDevice
        );
        assert_eq!(
            table.lease_for(lease.index, Some("")).unwrap_err(),
            Refusal::OtherDevice
        );
        // An index nothing is registered under.
        let empty = Table::default();
        assert_eq!(
            empty.lease_for(lease.index, Some(DEVICE)).unwrap_err(),
            Refusal::NotLent
        );
    }

    #[test]
    fn a_window_lent_to_one_device_is_not_registered_for_another() {
        let mut table = Table::default();
        let lease = made(&mut table, 0x7801);
        assert_eq!(
            table.register(0x7801, OTHER, ME, None).unwrap_err(),
            RegisterError::Held
        );
        assert!(table.holds(&lease), "the lend that was made is untouched");
    }

    #[test]
    fn a_handle_given_to_another_window_voids_the_old_grant() {
        let mut table = Table::default();
        let old = made(&mut table, 0x7901);
        let recycled = Identity { pid: 77, tid: 78 };
        let (new, created) = table.register(0x7901, DEVICE, recycled, None).unwrap();
        assert!(created);
        assert!(!table.holds(&old), "the old window's sessions end");
        assert!(table.holds(&new));
        assert_eq!(new.identity, recycled);
    }

    fn native_made(
        table: &mut Table,
        id: u64,
        device: &str,
    ) -> Result<(Lease, bool), RegisterError> {
        let native = gnome_native(id);
        table.register_with(
            native_handle(&native),
            &native,
            device,
            Identity::default(),
            None,
        )
    }

    #[test]
    fn a_gnome_window_is_named_by_plain_decimal_digits_and_nothing_else() {
        assert_eq!(gnome_native(2609479513), "gnome:2609479513");
        assert_eq!(gnome_id("gnome:2609479513"), Some(2609479513));
        assert_eq!(gnome_id("gnome:1"), Some(1));
        assert_eq!(gnome_id(&gnome_native(u64::MAX)), Some(u64::MAX));
        for bad in [
            "",
            "gnome:",
            "gnome:0",
            "gnome:01",
            "gnome:-1",
            "gnome:+1",
            "gnome: 1",
            "gnome:1 ",
            "gnome:1x",
            "gnome:0x10",
            "gnome:1.5",
            "gnome:18446744073709551616",
            "gnome:111111111111111111111",
            "GNOME:1",
            "kde:1",
            "2609479513",
            "gnome:\u{661}",
        ] {
            assert_eq!(gnome_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_native_name_has_a_handle_that_is_never_zero_and_names_it_alone() {
        let handles: Vec<isize> = (1..500u64)
            .map(|id| native_handle(&gnome_native(id)))
            .collect();
        assert!(handles.iter().all(|h| *h > 0));
        let mut sorted = handles.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), handles.len(), "no two of these share one");
        assert_eq!(
            native_handle("gnome:5"),
            native_handle("gnome:5"),
            "the same name, the same handle"
        );
        assert_eq!(native_handle(""), native_handle("") | 1, "odd, so non-zero");
    }

    #[test]
    fn a_compositors_window_is_registered_like_a_handle_with_its_own_name_in_the_lease() {
        let mut table = Table::default();
        let (lease, created) = native_made(&mut table, 2609479513, DEVICE).unwrap();
        assert!(created);
        assert_eq!(lease.native, "gnome:2609479513");
        assert!(is_window_index(lease.index));
        assert!(table.holds(&lease));
        // Asked again: the same lease, nothing made.
        assert_eq!(
            native_made(&mut table, 2609479513, DEVICE).unwrap(),
            (lease.clone(), false)
        );
        // Lent to one device, it is not lent to another.
        assert_eq!(
            native_made(&mut table, 2609479513, OTHER).unwrap_err(),
            RegisterError::Held
        );
        // Another window of the same device is another lease.
        let (other, _) = native_made(&mut table, 2609479514, DEVICE).unwrap();
        assert_ne!(other.index, lease.index);
        assert_ne!(other.generation, lease.generation);
        // Only the name that was registered is the lease's.
        let renamed = Lease {
            native: "gnome:2609479514".into(),
            ..lease.clone()
        };
        assert!(!table.holds(&renamed));
        let windows = Lease {
            native: String::new(),
            ..lease.clone()
        };
        assert!(!table.holds(&windows), "a handle is not a native window");
        // Given back, it holds nothing.
        assert!(table.unregister(&lease));
        assert!(!table.holds(&lease));
        assert!(table.holds(&other));
    }

    #[test]
    fn a_compositors_window_lends_no_input_and_a_windows_one_does() {
        let mut table = Table::default();
        let (native, _) = native_made(&mut table, 2609479515, DEVICE).unwrap();
        assert!(!takes_input(&native), "view only: nothing to confine it by");
        let handle = made(&mut table, 0x7C01);
        assert!(takes_input(&handle));
    }

    #[test]
    fn a_native_lease_stands_while_its_grant_does_and_a_handle_lease_off_windows_never() {
        let device = "native-lease-device";
        let native = gnome_native(2609479516);
        let (lease, _) = register_native(&native, device, None).unwrap();
        assert!(lease_alive(&lease), "holds, and has no handle to reuse");
        if cfg!(not(windows)) {
            let machine = Machine;
            assert!(machine.lease_holds(&lease));
            // Every other question the guard asks has no answer: no move is
            // placed, nothing is in front, nothing is hit, nothing raised.
            assert_eq!(machine.frame(&lease), None);
            assert!(!machine.front_is_lent(&lease));
            assert!(!machine.hit_is_lent(&lease, (1, 1)));
            machine.raise(&lease);
        }
        unregister(&lease);
        assert!(!lease_alive(&lease), "given back, it stands no more");
        assert_eq!(
            register_native("", device, None).unwrap_err(),
            RegisterError::Nothing
        );
    }

    #[test]
    fn off_windows_nothing_stands_and_nothing_is_placed() {
        let mut table = Table::default();
        let lease = made(&mut table, 0x7A01);
        if cfg!(not(windows)) {
            assert!(!lease_alive(&lease));
            assert!(!same_window(0x7A01, ME));
            assert_eq!(frame_of(0x7A01), None);
            assert_eq!(identity_of(0x7A01), None);
            let machine = Machine;
            assert!(!machine.lease_holds(&lease));
            assert!(!machine.front_is_lent(&lease));
            assert!(!machine.hit_is_lent(&lease, (1, 1)));
            assert_eq!(machine.frame(&lease), None);
            machine.raise(&lease);
            unminimize(&lease);
        }
    }

    #[test]
    fn the_global_table_gives_leases_and_ends_them() {
        let device = "global-table-device";
        let (lease, created) = register(0x7B01, device, ME, None).unwrap();
        assert!(created);
        assert!(lease_holds(&lease));
        assert_eq!(lease_at(lease.index).as_ref(), Some(&lease));
        assert_eq!(lease_for(lease.index, Some(device)).unwrap(), lease);
        assert_eq!(
            lease_for(lease.index, Some("someone-else")).unwrap_err(),
            Refusal::OtherDevice
        );
        let mut seen = changes();
        seen.mark_unchanged();
        unregister(&lease);
        assert!(!lease_holds(&lease));
        assert!(seen.has_changed().unwrap(), "a session is told at once");
    }
}
