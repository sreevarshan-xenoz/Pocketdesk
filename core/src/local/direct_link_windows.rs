//! A direct link on Windows: the Mobile Hotspot, through
//! `Windows.Networking.NetworkOperators.NetworkOperatorTetheringManager`.
//!
//! Windows has one Mobile Hotspot for the whole system, with one name, one
//! passphrase and one band, so a link has to change the owner's own and put
//! it back: [`start`] records what is set (and whether the hotspot was on)
//! through its `remember` callback *before* it changes anything (the server
//! keeps it in its owner-only record, so a server that is killed leaves
//! what a next start puts back), and [`restore`] puts it back, leaving the
//! hotspot off if it was off.
//!
//! The hotspot shares an internet connection, so it needs one: with none,
//! [`probe`] says so rather than failing later. Every call here blocks (the
//! WinRT calls are waited for, with deadlines) and is for a worker thread.
//! Nothing here logs: the name and passphrase are never in a log line, and
//! the owner's settings are as private as the link's.
//!
//! The decisions that need no Windows (which steps put the hotspot back,
//! which band to ask for, which address is the computer's) are in
//! [`super::direct_link`], where a test on any system reads them.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use windows::Networking::Connectivity::{ConnectionProfile, NetworkInformation};
use windows::Networking::NetworkOperators::{
    NetworkOperatorTetheringAccessPointConfiguration as AccessPoint,
    NetworkOperatorTetheringManager as Manager, TetheringOperationStatus,
    TetheringOperationalState, TetheringWiFiBand,
};
use windows::Win32::NetworkManagement::IpHelper::{
    GetTcpTable2, MIB_TCP_STATE_ESTAB, MIB_TCPROW2, MIB_TCPTABLE2,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::HSTRING;
use windows_future::{AsyncStatus, IAsyncAction, IAsyncOperation};

use super::direct_link::{
    self as dl, Band, HotspotRestore, HotspotSettings, HotspotStep, Mode, Plan, Unavailable,
    WifiJoin,
};

/// How long Windows has to apply the settings, to start the hotspot, and
/// to stop it.
const CONFIGURE_WITHIN: Duration = Duration::from_secs(15);
const START_WITHIN: Duration = Duration::from_secs(30);
const STOP_WITHIN: Duration = Duration::from_secs(20);
/// How long the hotspot's network has to give this computer an address.
const ADDRESS_WITHIN: Duration = Duration::from_secs(15);
/// `ERROR_INSUFFICIENT_BUFFER`.
const INSUFFICIENT_BUFFER: u32 = 122;

/// WinRT on this thread (a worker thread has no apartment of its own).
fn init() {
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
}

/// A failure as a short phrase: the step and Windows' code, which carries
/// no name or passphrase (its message can quote what was asked).
fn failed(step: &str, e: &windows::core::Error) -> String {
    format!(
        "Windows' Mobile Hotspot could not {step} (code {:#010x}).",
        e.code().0
    )
}

/// An asynchronous action, waited for at most `within`; one still running
/// then is cancelled.
fn finish_action(action: &IAsyncAction, step: &str, within: Duration) -> Result<(), String> {
    let deadline = Instant::now() + within;
    while action.Status().map_err(|e| failed(step, &e))? == AsyncStatus::Started {
        if Instant::now() >= deadline {
            let _ = action.Cancel();
            return Err(format!("Windows' Mobile Hotspot did not {step} in time."));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    action.GetResults().map_err(|e| failed(step, &e))
}

/// The same for an operation that has a result.
fn finish<T: windows::core::RuntimeType + 'static>(
    operation: &IAsyncOperation<T>,
    step: &str,
    within: Duration,
) -> Result<T, String> {
    let deadline = Instant::now() + within;
    while operation.Status().map_err(|e| failed(step, &e))? == AsyncStatus::Started {
        if Instant::now() >= deadline {
            let _ = operation.Cancel();
            return Err(format!("Windows' Mobile Hotspot did not {step} in time."));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    operation.GetResults().map_err(|e| failed(step, &e))
}

/// The connection Windows would share: its internet one.
fn internet_profile() -> Option<ConnectionProfile> {
    NetworkInformation::GetInternetConnectionProfile().ok()
}

/// The hotspot's manager for the internet connection, if Windows allows a
/// hotspot here.
fn manager_for(profile: &ConnectionProfile) -> Result<Manager, Unavailable> {
    let capability = Manager::GetTetheringCapabilityFromConnectionProfile(profile)
        .map_err(|_| Unavailable::Unsupported)?;
    if let Some(why) = dl::unavailable_for_tethering_capability(capability.0) {
        return Err(why);
    }
    Manager::CreateFromConnectionProfile(profile).map_err(|_| Unavailable::Blocked)
}

fn is_on(manager: &Manager) -> bool {
    manager.TetheringOperationalState().is_ok_and(|s| {
        s == TetheringOperationalState::On || s == TetheringOperationalState::InTransition
    })
}

/// What a link would do here: nothing to change to find out. A computer
/// with no internet connection has nothing for the hotspot to share, and
/// one whose own hotspot is on would have it changed under whatever uses
/// it.
pub fn probe() -> Result<Plan, Unavailable> {
    init();
    let profile = internet_profile().ok_or(Unavailable::NoConnectionProfile)?;
    let manager = manager_for(&profile)?;
    if is_on(&manager) {
        return Err(Unavailable::HotspotInUse);
    }
    Ok(Plan {
        // Windows makes its own virtual adapter, beside the card's own
        // connection where the driver allows (a start that cannot says so).
        device: String::new(),
        mode: Mode::SecondInterface,
        channel: None,
        disconnects: false,
    })
}

/// The hotspot's name as set now. A failed read is an error, never an
/// empty name: an empty one would be taken for the owner's.
fn read_ssid(config: &AccessPoint) -> Result<String, String> {
    config
        .Ssid()
        .map(|s| s.to_string_lossy())
        .map_err(|e| failed("read its name", &e))
}

/// The owner's settings as set now, or why they could not be read (a read
/// that failed is never recorded as empty: the owner's passphrase would
/// then be lost to the restore).
fn read_settings(config: &AccessPoint) -> Result<HotspotSettings, String> {
    Ok(HotspotSettings {
        ssid: read_ssid(config)?,
        passphrase: config
            .Passphrase()
            .map(|s| s.to_string_lossy())
            .map_err(|e| failed("read its passphrase", &e))?,
        band: config.Band().ok().map(|b| b.0),
    })
}

/// Put `ssid`, `passphrase` and (where this Windows has a band setting)
/// `band` on `config`, and have Windows apply it.
fn apply(
    manager: &Manager,
    config: &AccessPoint,
    ssid: &str,
    passphrase: &str,
    band: Option<i32>,
) -> Result<(), String> {
    config
        .SetSsid(&HSTRING::from(ssid))
        .map_err(|e| failed("take its new name", &e))?;
    config
        .SetPassphrase(&HSTRING::from(passphrase))
        .map_err(|e| failed("take its new passphrase", &e))?;
    // An older Windows has no band; the card's own choice stands there.
    if let Some(band) = band
        && config.Band().is_ok()
    {
        let _ = config.SetBand(TetheringWiFiBand(band));
    }
    let action = manager
        .ConfigureAccessPointAsync(config)
        .map_err(|e| failed("apply its settings", &e))?;
    finish_action(&action, "apply its settings", CONFIGURE_WITHIN)
}

fn start_hotspot(manager: &Manager) -> Result<(), String> {
    let operation = manager
        .StartTetheringAsync()
        .map_err(|e| failed("start", &e))?;
    let result = finish(&operation, "start", START_WITHIN)?;
    let status = result.Status().map_err(|e| failed("start", &e))?;
    if status == TetheringOperationStatus::Success || status == TetheringOperationStatus::AlreadyOn
    {
        Ok(())
    } else {
        Err(dl::tethering_failure_words(status.0).to_string())
    }
}

fn stop_hotspot(manager: &Manager) -> Result<(), String> {
    let operation = manager
        .StopTetheringAsync()
        .map_err(|e| failed("stop", &e))?;
    let result = finish(&operation, "stop", STOP_WITHIN)?;
    let status = result.Status().map_err(|e| failed("stop", &e))?;
    if status == TetheringOperationStatus::Success {
        Ok(())
    } else {
        Err(dl::tethering_failure_words(status.0).to_string())
    }
}

/// This computer's non-loopback IPv4 addresses now.
fn ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut seen = HashSet::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let IpAddr::V4(a) = interface.ip()
                && !a.is_loopback()
            {
                seen.insert(a);
            }
        }
    }
    seen.into_iter().collect()
}

/// Make the hotspot with `join`'s name and passphrase, on `band` where the
/// card can. `remember` is given what is set now before anything changes,
/// and must keep it (it returns why not, and then nothing is changed).
/// Returns that record, to give to [`restore`], and this computer's address
/// on the new network. On failure the hotspot is put back as it was.
pub fn start(
    join: &WifiJoin,
    band: Band,
    remember: impl FnOnce(&HotspotRestore) -> Result<(), String>,
) -> Result<(HotspotRestore, String), String> {
    init();
    let profile =
        internet_profile().ok_or_else(|| Unavailable::NoConnectionProfile.words().to_string())?;
    let manager = manager_for(&profile).map_err(|why| why.words().to_string())?;
    let config = manager
        .GetCurrentAccessPointConfiguration()
        .map_err(|e| failed("read its settings", &e))?;
    let before = HotspotRestore {
        was_on: is_on(&manager),
        settings: read_settings(&config)?,
    };
    // Before anything is recorded or changed: a hotspot that is on (it came
    // on since the probe), that could not be read, or that is a left-over
    // link's is not made into a link.
    if let Some(why) = dl::hotspot_start_refusal(&before) {
        return Err(why.to_string());
    }
    remember(&before)?;
    match bring_up(&manager, &config, join, band) {
        Ok(address) => Ok((before, address)),
        Err(reason) => {
            // This call changed the settings, so what is set is this
            // link's whatever it is now.
            let _ = put_back(&manager, &config, &before, true);
            Err(reason)
        }
    }
}

fn bring_up(
    manager: &Manager,
    config: &AccessPoint,
    join: &WifiJoin,
    band: Band,
) -> Result<String, String> {
    // The owner's own hotspot, if it came on since the probe.
    if is_on(manager) {
        stop_hotspot(manager)?;
    }
    let supported = |code: i32| {
        config
            .IsBandSupported(TetheringWiFiBand(code))
            .unwrap_or(false)
    };
    let band = dl::pick_tethering_band(band, supported);
    let before = ipv4_addresses();
    apply(manager, config, &join.ssid, &join.passphrase, Some(band))?;
    start_hotspot(manager)?;
    let deadline = Instant::now() + ADDRESS_WITHIN;
    loop {
        if let Some(address) = dl::pick_link_address(&before, &ipv4_addresses()) {
            return Ok(address.to_string());
        }
        if Instant::now() >= deadline {
            return Err("The network came up without an address for this computer.".to_string());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Put the hotspot back as `restore` says. `ours` says the settings now are
/// known to be the link's (this call made them); otherwise only a hotspot
/// still named as a link names it is touched.
fn put_back(
    manager: &Manager,
    config: &AccessPoint,
    restore: &HotspotRestore,
    ours: bool,
) -> Result<(), String> {
    let current = if ours {
        dl::SSID_PREFIX.to_string()
    } else {
        // A name that cannot be read is not "not ours": the record stays.
        read_ssid(config)?
    };
    let mut result = Ok(());
    // Set when the owner's settings did not go back: the hotspot still
    // carries the link's name and passphrase, and must not come up as if it
    // were the owner's.
    let mut configure_failed = false;
    for step in dl::hotspot_restore_steps(restore, &current, is_on(manager)) {
        let done = match step {
            HotspotStep::Stop => stop_hotspot(manager),
            HotspotStep::Configure => {
                let done = apply(
                    manager,
                    config,
                    &restore.settings.ssid,
                    &restore.settings.passphrase,
                    restore.settings.band,
                );
                configure_failed = done.is_err();
                done
            }
            HotspotStep::Start if configure_failed => continue,
            HotspotStep::Start => start_hotspot(manager),
        };
        // A step that failed does not stop the ones after it: a hotspot
        // that could not be stopped still has its name put back.
        if let Err(reason) = done
            && result.is_ok()
        {
            result = Err(reason);
        }
    }
    result
}

/// Any hotspot manager there is: the internet connection's, or else the
/// first connection's that has one (the internet may be gone by the time a
/// link ends).
fn any_manager() -> Option<(Manager, AccessPoint)> {
    let from = |profile: &ConnectionProfile| {
        let manager = Manager::CreateFromConnectionProfile(profile).ok()?;
        let config = manager.GetCurrentAccessPointConfiguration().ok()?;
        Some((manager, config))
    };
    if let Some(found) = internet_profile().as_ref().and_then(from) {
        return Some(found);
    }
    NetworkInformation::GetConnectionProfiles()
        .ok()?
        .into_iter()
        .find_map(|profile| from(&profile))
}

/// End a link: the hotspot goes off and the owner's settings go back (and
/// it comes back on, only if it was on). Also what a start after a crash
/// does with the record an earlier run left; in that case a hotspot the
/// owner has set up since is not touched.
pub fn restore(restore: &HotspotRestore) -> Result<(), String> {
    init();
    let (manager, config) =
        any_manager().ok_or_else(|| "Windows' Mobile Hotspot could not be reached.".to_string())?;
    put_back(&manager, &config, restore, false)
}

/// The peers of the established TCP connections to this computer's local
/// `port`: a phone on the link is one that holds a connection to the
/// control port.
pub fn established_peers(port: u16) -> Vec<Ipv4Addr> {
    let mut size: u32 = 0;
    // The first call says how big the table is.
    let first = unsafe { GetTcpTable2(None, &mut size, false) };
    if first != INSUFFICIENT_BUFFER || size == 0 {
        return Vec::new();
    }
    // A table can grow between the calls: a few tries, each with room.
    for _ in 0..4 {
        // u32 words, so the table's rows are aligned.
        let mut buffer = vec![0u32; (size as usize).div_ceil(4) + 1];
        let table = buffer.as_mut_ptr().cast::<MIB_TCPTABLE2>();
        let code = unsafe { GetTcpTable2(Some(table), &mut size, false) };
        if code == INSUFFICIENT_BUFFER {
            continue;
        }
        if code != 0 {
            return Vec::new();
        }
        let rows = unsafe {
            let count = (*table).dwNumEntries as usize;
            let start = std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW2>();
            std::slice::from_raw_parts(start, count)
        };
        return rows
            .iter()
            .filter(|row| {
                row.dwState == MIB_TCP_STATE_ESTAB.0 as u32
                    // The port is in network byte order, in the low half.
                    && u16::from_be(row.dwLocalPort as u16) == port
            })
            .map(|row| Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes()))
            .collect();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: the probe asks Windows what it could do and changes
    /// nothing. Whatever the runner's network is, it answers (a plan or a
    /// reason) on a thread of its own, as the server runs it.
    #[test]
    fn the_probe_answers_on_a_worker_thread_without_changing_anything() {
        let answer = std::thread::spawn(probe)
            .join()
            .expect("the probe did not crash");
        if let Ok(plan) = answer {
            assert_eq!(plan.mode, Mode::SecondInterface);
            assert!(!plan.disconnects);
        }
    }

    /// A connection to a port of this computer is found in the TCP table,
    /// with its peer's address.
    #[test]
    fn an_established_connection_to_a_local_port_is_found_with_its_peer() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (_server_side, _) = listener.accept().unwrap();
        let peers = established_peers(port);
        assert!(peers.contains(&Ipv4Addr::LOCALHOST), "{peers:?}");
        // Nothing listens on a port that was never opened.
        assert!(established_peers(1).is_empty());
    }
}
