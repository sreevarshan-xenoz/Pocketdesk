//! The computer's state for its phones (F2): CPU in use, memory, each
//! fixed disk, the battery and uptime, answered to
//! `kdeconnect.pocketdesk.system` without a session. Readings block (a CPU
//! sample waits 250 ms): run [`reading`] on a blocking thread.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocketdesk_core::error::Result;
use pocketdesk_core::protocol::kdeconnect::{DeviceSender, NetworkPacket, Plugin};
use serde_json::{Value, json};

pub const SYSTEM: &str = "kdeconnect.pocketdesk.system";

/// One fixed disk: its name (a drive letter or a mount point) and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub name: String,
    pub used: u64,
    pub total: u64,
}

/// What the computer reads of itself; a part it could not read is None.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub cpu: Option<f32>,
    /// (used, total) bytes.
    pub memory: Option<(u64, u64)>,
    pub disks: Vec<Disk>,
    /// (charge %, charging).
    pub battery: Option<(u8, bool)>,
    pub uptime_secs: Option<u64>,
}

/// The answer's body for `reading`.
pub(crate) fn body(reading: &Reading) -> Value {
    json!({
        "cpu": reading.cpu.map(|c| (c * 10.0).round() / 10.0),
        "memory": reading.memory.map(|(used, total)| json!({ "used": used, "total": total })),
        "disks": reading
            .disks
            .iter()
            .map(|d| json!({ "name": d.name, "used": d.used, "total": d.total }))
            .collect::<Vec<_>>(),
        "battery": reading
            .battery
            .map(|(charge, charging)| json!({ "charge": charge, "charging": charging })),
        "uptime_secs": reading.uptime_secs,
    })
}

/// CPU in use between two (idle, total) samples, in percent.
pub(crate) fn cpu_percent(before: (u64, u64), after: (u64, u64)) -> Option<f32> {
    let idle = after.0.checked_sub(before.0)?;
    let total = after.1.checked_sub(before.1)?;
    if total == 0 {
        return None;
    }
    Some((100.0 * (1.0 - idle as f64 / total as f64)).clamp(0.0, 100.0) as f32)
}

/// `/proc/stat`'s first line as (idle, total) jiffies: idle and iowait idle.
// Linux reads it; the tests hold it on every system.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let values: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some((idle, values.iter().sum()))
}

/// `/proc/meminfo` as (used, total) bytes: total less what is available.
// Linux reads it; the tests hold it on every system.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        line.split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    Some((total.saturating_sub(available), total))
}

/// `df -kP`'s disks: real filesystems (`/dev/…`), each mount point once.
// Linux reads it; the tests hold it on every system.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_df(text: &str) -> Vec<Disk> {
    let mut disks: Vec<Disk> = Vec::new();
    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 6 || !parts[0].starts_with("/dev/") || parts[0].starts_with("/dev/loop") {
            continue;
        }
        let (Ok(total_kb), Ok(used_kb)) = (parts[1].parse::<u64>(), parts[2].parse::<u64>()) else {
            continue;
        };
        let mount = parts[5..].join(" ");
        if disks.iter().any(|d| d.name == mount) {
            continue;
        }
        disks.push(Disk {
            name: mount,
            used: used_kb * 1024,
            total: total_kb * 1024,
        });
    }
    disks
}

/// `/proc/uptime`'s first number, in whole seconds.
// Linux reads it; the tests hold it on every system.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_uptime(text: &str) -> Option<u64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

/// The last CPU sample and when it was taken, so a phone asking every few
/// seconds needs no wait.
static LAST_CPU: Mutex<Option<(Instant, (u64, u64))>> = Mutex::new(None);

/// CPU in use since the last sample (within a minute), or over 250 ms.
fn cpu_now() -> Option<f32> {
    let now = cpu_sample()?;
    let mut last = LAST_CPU.lock().unwrap_or_else(|e| e.into_inner());
    let before = match *last {
        Some((at, sample))
            if at.elapsed() < Duration::from_secs(60)
                && at.elapsed() >= Duration::from_millis(200) =>
        {
            sample
        }
        _ => {
            drop(last);
            std::thread::sleep(Duration::from_millis(250));
            let later = cpu_sample()?;
            *LAST_CPU.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), later));
            return cpu_percent(now, later);
        }
    };
    *last = Some((Instant::now(), now));
    cpu_percent(before, now)
}

#[cfg(windows)]
fn cpu_sample() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: three valid out-pointers for the call.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    // Kernel time includes idle time.
    Some((ticks(idle), ticks(kernel) + ticks(user)))
}

#[cfg(not(windows))]
fn cpu_sample() -> Option<(u64, u64)> {
    parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
}

#[cfg(windows)]
fn memory() -> Option<(u64, u64)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: a valid MEMORYSTATUSEX with its length set.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some((
        status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
        status.ullTotalPhys,
    ))
}

#[cfg(not(windows))]
fn memory() -> Option<(u64, u64)> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(windows)]
fn disks() -> Vec<Disk> {
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };
    use windows::core::PCWSTR;
    const DRIVE_FIXED: u32 = 3;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    let mut found = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + i as u8);
        let root: Vec<u16> = format!("{letter}:\\")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let root = PCWSTR(root.as_ptr());
        // SAFETY: a NUL-terminated wide string living for the calls.
        if unsafe { GetDriveTypeW(root) } != DRIVE_FIXED {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: as above, with valid out-pointers.
        if unsafe { GetDiskFreeSpaceExW(root, None, Some(&mut total), Some(&mut free)) }.is_ok()
            && total > 0
        {
            found.push(Disk {
                name: format!("{letter}:"),
                used: total.saturating_sub(free),
                total,
            });
        }
    }
    found
}

#[cfg(not(windows))]
fn disks() -> Vec<Disk> {
    std::process::Command::new("df")
        .args(["-kP"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_df(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

#[cfg(windows)]
fn uptime() -> Option<u64> {
    // SAFETY: no arguments.
    Some(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } / 1000)
}

#[cfg(not(windows))]
fn uptime() -> Option<u64> {
    parse_uptime(&std::fs::read_to_string("/proc/uptime").ok()?)
}

/// Read it all. Blocks for up to 250 ms on the first CPU sample.
pub fn reading(battery: Option<(u8, bool)>) -> Reading {
    Reading {
        cpu: cpu_now(),
        memory: memory(),
        disks: disks(),
        battery,
        uptime_secs: uptime(),
    }
}

/// The plugin answering a phone's ask.
pub struct SystemPlugin;

#[async_trait::async_trait]
impl Plugin for SystemPlugin {
    fn name(&self) -> &'static str {
        "system"
    }

    fn incoming_capabilities(&self) -> &'static [&'static str] {
        &[SYSTEM]
    }

    fn outgoing_capabilities(&self) -> &'static [&'static str] {
        &[SYSTEM]
    }

    async fn handle_packet(&self, packet: &NetworkPacket, sender: &dyn DeviceSender) -> Result<()> {
        if packet.packet_type != SYSTEM {
            return Ok(());
        }
        let body = if crate::events::current_config().share_state {
            let battery = crate::plugins::battery::read_battery().await;
            tokio::task::spawn_blocking(move || body(&reading(battery)))
                .await
                .unwrap_or_else(|_| json!({}))
        } else {
            json!({ "off": true })
        };
        sender
            .send_packet(&NetworkPacket::new(SYSTEM).with_body(body))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_is_the_share_of_ticks_not_idle() {
        assert_eq!(cpu_percent((100, 1000), (150, 1100)), Some(50.0));
        assert_eq!(cpu_percent((100, 1000), (200, 1100)), Some(0.0));
        assert_eq!(
            cpu_percent((100, 1000), (100, 1000)),
            None,
            "no time passed"
        );
        assert_eq!(
            cpu_percent((100, 1000), (50, 900)),
            None,
            "a counter that went back"
        );
    }

    #[test]
    fn proc_stat_counts_iowait_as_idle() {
        let stat = "cpu  100 20 30 400 50 6 7 8 0 0\ncpu0 1 2 3 4 5 6 7 8 0 0\n";
        assert_eq!(parse_proc_stat(stat), Some((450, 621)));
        assert_eq!(parse_proc_stat("intr 1 2 3"), None);
    }

    #[test]
    fn meminfo_counts_what_is_not_available() {
        let info = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    6000000 kB\n";
        assert_eq!(
            parse_meminfo(info),
            Some((10_000_000 * 1024, 16_000_000 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn df_keeps_real_filesystems_once_each() {
        let df = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                  /dev/nvme0n1p2   488281250 244140625 244140625      50% /\n\
                  tmpfs              8000000         0   8000000       0% /dev/shm\n\
                  /dev/loop3          65536     65536         0     100% /snap/core/1\n\
                  /dev/nvme0n1p1     1048576    262144    786432      25% /boot/efi\n\
                  /dev/sdb1         10000000   5000000   5000000      50% /media/my disk\n";
        let disks = parse_df(df);
        assert_eq!(
            disks.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["/", "/boot/efi", "/media/my disk"]
        );
        assert_eq!(disks[0].total, 488_281_250 * 1024);
        assert_eq!(disks[0].used, 244_140_625 * 1024);
    }

    #[test]
    fn uptime_is_whole_seconds() {
        assert_eq!(parse_uptime("86400.57 300000.00\n"), Some(86400));
        assert_eq!(parse_uptime(""), None);
    }

    #[test]
    fn the_body_says_what_could_not_be_read_as_null() {
        let reading = Reading {
            cpu: Some(23.456),
            memory: Some((1, 2)),
            disks: vec![Disk {
                name: "C:".into(),
                used: 3,
                total: 4,
            }],
            battery: None,
            uptime_secs: Some(5),
        };
        let body = body(&reading);
        assert_eq!(body["cpu"], 23.5);
        assert_eq!(body["memory"], json!({"used": 1, "total": 2}));
        assert_eq!(
            body["disks"],
            json!([{"name": "C:", "used": 3, "total": 4}])
        );
        assert!(body["battery"].is_null());
        assert_eq!(body["uptime_secs"], 5);
        assert!(super::body(&Reading::default())["cpu"].is_null());
    }

    #[test]
    fn this_computer_reads_something() {
        let reading = reading(None);
        assert!(
            reading
                .memory
                .is_some_and(|(used, total)| used <= total && total > 0)
        );
        assert!(reading.uptime_secs.is_some());
    }
}
